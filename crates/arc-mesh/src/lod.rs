//! Level-of-detail chains (CORE-MESH-016).
//!
//! [`build_lod_chain`] simplifies the mesh repeatedly, each level from the one before it, to
//! about `ratio` times the triangles of the previous level, until the floor is reached, the
//! level limit is reached or [`crate::simplify`] cannot reduce the mesh further.
//!
//! - **Recorded error.** The geometric error of a level is the running maximum, over the
//!   levels so far, of the sampled deviation from the *original* mesh
//!   ([`crate::simplify::measure_deviation`]), in model units. The running maximum makes the
//!   errors non-decreasing along the chain, which a coarser level normally satisfies anyway.
//!   Level 0 is the original mesh with error 0.
//! - **Screen-space error.** A geometric error `e` seen from `distance` with a projection
//!   scale of `pixels_per_unit` (the size in pixels of one model unit at unit distance, that is
//!   the viewport height in pixels over twice the tangent of half the vertical field of view)
//!   spans `e · pixels_per_unit / distance` pixels.
//! - **Selection.** [`LodChain::select`] returns the coarsest level whose screen-space error is
//!   at most the budget; level 0 always qualifies. Because errors do not decrease along the
//!   chain, the choice never gets finer when the budget grows or the viewer moves away.

use arc_base::{Error, Limits, Result, Tolerance};

use crate::TriMesh;
use crate::bvh::MeshBvh;
use crate::error::codes;
use crate::simplify::{SimplifyOptions, deviation, simplify_core};

/// The most levels a chain may have, original included.
pub const MAX_LEVELS: u32 = 64;

/// Options of [`build_lod_chain`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodOptions {
    /// The triangle count of each level relative to the one before, in (0, 1).
    pub ratio: f64,
    /// The chain stops once a level has at most this many triangles (at least 1).
    pub min_triangles: usize,
    /// The most levels after the original, in 1..=[`MAX_LEVELS`] − 1.
    pub max_levels: u32,
}

impl Default for LodOptions {
    fn default() -> Self {
        Self {
            ratio: 0.5,
            min_triangles: 64,
            max_levels: 8,
        }
    }
}

/// One level of a chain.
#[derive(Debug, Clone, PartialEq)]
pub struct LodLevel {
    /// The mesh of the level.
    pub mesh: TriMesh,
    /// The recorded geometric error in model units; see the module documentation.
    pub geometric_error: f64,
}

/// How a mesh is seen: the viewing distance and the projection scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewParams {
    /// The distance from the eye to the object, in model units; positive.
    pub distance: f64,
    /// The size in pixels of one model unit at unit distance; positive.
    pub pixels_per_unit: f64,
}

/// A chain of simplified levels, finest first.
#[derive(Debug, Clone, PartialEq)]
pub struct LodChain {
    levels: Vec<LodLevel>,
}

impl LodChain {
    /// The levels, level 0 (the original) first. Never empty.
    #[must_use]
    pub fn levels(&self) -> &[LodLevel] {
        &self.levels
    }

    /// The screen-space error of level `level`, in pixels; `None` for a level out of range.
    ///
    /// # Errors
    ///
    /// `mesh.lod.invalid` when the view is not finite and positive.
    pub fn screen_error(&self, level: usize, view: &ViewParams) -> Result<Option<f64>> {
        validate_view(view)?;
        Ok(self
            .levels
            .get(level)
            .map(|l| l.geometric_error * view.pixels_per_unit / view.distance))
    }

    /// The coarsest level whose screen-space error is at most `budget_pixels`.
    ///
    /// # Errors
    ///
    /// `mesh.lod.invalid` when the budget is negative or not finite, or the view is not finite
    /// and positive.
    pub fn select(&self, budget_pixels: f64, view: &ViewParams) -> Result<usize> {
        validate_view(view)?;
        if !(budget_pixels.is_finite() && budget_pixels >= 0.0) {
            return Err(Error::new(codes::LOD_INVALID));
        }
        Ok(self
            .levels
            .iter()
            .rposition(|l| {
                l.geometric_error * view.pixels_per_unit / view.distance <= budget_pixels
            })
            .unwrap_or(0))
    }
}

fn validate_view(view: &ViewParams) -> Result<()> {
    let ok = |x: f64| x.is_finite() && x > 0.0;
    if ok(view.distance) && ok(view.pixels_per_unit) {
        Ok(())
    } else {
        Err(Error::new(codes::LOD_INVALID))
    }
}

/// Builds the chain of simplified levels of `mesh`.
///
/// # Errors
///
/// `mesh.lod.invalid` for options out of range; `mesh.query.empty` for a mesh without
/// triangles; otherwise as [`crate::simplify::simplify`].
pub fn build_lod_chain(
    mesh: &TriMesh,
    options: &LodOptions,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<LodChain> {
    let ratio_ok = options.ratio > 0.0 && options.ratio < 1.0;
    if !ratio_ok
        || options.min_triangles == 0
        || options.max_levels == 0
        || options.max_levels >= MAX_LEVELS
    {
        return Err(Error::new(codes::LOD_INVALID));
    }
    let original = MeshBvh::build(mesh, limits)?;
    let mut levels = vec![LodLevel {
        mesh: mesh.clone(),
        geometric_error: 0.0,
    }];
    for _ in 0..options.max_levels {
        let Some(previous) = levels.last() else { break };
        let count = previous.mesh.triangle_count();
        if count <= options.min_triangles {
            break;
        }
        let target = floor_to_usize(count as f64 * options.ratio).max(options.min_triangles);
        let wanted = SimplifyOptions {
            target_triangles: Some(target),
            max_error: None,
        };
        let (next, ..) = simplify_core(&previous.mesh, &wanted, tolerance, limits)?;
        if next.triangle_count() >= count || next.triangle_count() == 0 {
            break;
        }
        let measured = deviation(&original, &MeshBvh::build(&next, limits)?)?;
        let error = measured.max(previous.geometric_error);
        levels.push(LodLevel {
            mesh: next,
            geometric_error: error,
        });
    }
    Ok(LodChain { levels })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is a small non-negative count"
)]
fn floor_to_usize(x: f64) -> usize {
    x.floor().max(0.0) as usize
}

#[cfg(test)]
mod tests {
    use arc_base::math::Point3;
    use proptest::prelude::*;

    use super::*;
    use crate::testing::sphere;

    const TOL: Tolerance = Tolerance::DEFAULT;
    const LIMITS: Limits = Limits::DEFAULT;

    fn chain() -> &'static LodChain {
        static CHAIN: std::sync::OnceLock<LodChain> = std::sync::OnceLock::new();
        CHAIN.get_or_init(|| {
            let s = sphere(Point3::origin(), 2.0, 12, 24);
            let options = LodOptions {
                min_triangles: 40,
                ..LodOptions::default()
            };
            build_lod_chain(&s, &options, &TOL, &LIMITS).unwrap()
        })
    }

    #[test]
    fn levels_shrink_with_non_decreasing_errors() {
        let c = chain();
        let l = c.levels();
        assert!(l.len() >= 4, "{}", l.len());
        assert_eq!(l[0].geometric_error, 0.0);
        for (i, w) in l.windows(2).enumerate() {
            assert!(w[1].mesh.triangle_count() < w[0].mesh.triangle_count());
            // Every level but the last reaches about half of the one before.
            if i + 2 < l.len() {
                assert!(w[1].mesh.triangle_count() * 10 <= w[0].mesh.triangle_count() * 6);
            }
            assert!(w[1].geometric_error >= w[0].geometric_error);
        }
        assert!(l.last().unwrap().mesh.triangle_count() * 4 <= l[0].mesh.triangle_count());
        assert!(l.last().unwrap().geometric_error > 0.0);
        // The same input gives the same chain.
        assert_eq!(
            c,
            &build_lod_chain(
                &sphere(Point3::origin(), 2.0, 12, 24),
                &LodOptions {
                    min_triangles: 40,
                    ..LodOptions::default()
                },
                &TOL,
                &LIMITS
            )
            .unwrap()
        );
    }

    #[test]
    fn selection_matches_the_screen_space_rule() {
        let c = chain();
        let view = ViewParams {
            distance: 10.0,
            pixels_per_unit: 1000.0,
        };
        for (i, l) in c.levels().iter().enumerate() {
            let px = c.screen_error(i, &view).unwrap().unwrap();
            assert!((px - l.geometric_error * 100.0).abs() < 1e-9);
            // A budget just at the level's error selects it or a coarser level of equal error.
            assert!(c.select(px, &view).unwrap() >= i);
        }
        assert_eq!(c.select(0.0, &view).unwrap(), 0);
        assert_eq!(c.select(f64::MAX, &view).unwrap(), c.levels().len() - 1);
        assert_eq!(c.screen_error(99, &view).unwrap(), None);
    }

    #[test]
    fn invalid_parameters_have_a_code() {
        let c = chain();
        let view = ViewParams {
            distance: 10.0,
            pixels_per_unit: 1000.0,
        };
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(c.select(bad, &view).unwrap_err().code(), "mesh.lod.invalid");
        }
        for v in [
            ViewParams {
                distance: 0.0,
                ..view
            },
            ViewParams {
                distance: f64::NAN,
                ..view
            },
            ViewParams {
                pixels_per_unit: -1.0,
                ..view
            },
        ] {
            assert_eq!(c.select(1.0, &v).unwrap_err().code(), "mesh.lod.invalid");
        }
        let s = sphere(Point3::origin(), 1.0, 6, 12);
        for o in [
            LodOptions {
                ratio: 1.0,
                ..LodOptions::default()
            },
            LodOptions {
                ratio: 0.0,
                ..LodOptions::default()
            },
            LodOptions {
                ratio: f64::NAN,
                ..LodOptions::default()
            },
            LodOptions {
                min_triangles: 0,
                ..LodOptions::default()
            },
            LodOptions {
                max_levels: 0,
                ..LodOptions::default()
            },
            LodOptions {
                max_levels: MAX_LEVELS,
                ..LodOptions::default()
            },
        ] {
            assert_eq!(
                build_lod_chain(&s, &o, &TOL, &LIMITS).unwrap_err().code(),
                "mesh.lod.invalid"
            );
        }
        assert_eq!(
            build_lod_chain(&TriMesh::default(), &LodOptions::default(), &TOL, &LIMITS)
                .unwrap_err()
                .code(),
            "mesh.query.empty"
        );
        // A mesh at the floor has a chain of one level.
        let tiny = build_lod_chain(
            &s,
            &LodOptions {
                min_triangles: 1000,
                ..LodOptions::default()
            },
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(tiny.levels().len(), 1);
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        #[test]
        fn selection_is_monotonic(
            b1 in 0.0..50.0_f64, b2 in 0.0..50.0_f64,
            d1 in 0.1..1e6_f64, d2 in 0.1..1e6_f64,
            ppu in 1.0..1e4_f64,
        ) {
            let c = chain();
            let (lo, hi) = (b1.min(b2), b1.max(b2));
            let view = ViewParams { distance: 5.0, pixels_per_unit: ppu };
            prop_assert!(c.select(lo, &view).unwrap() <= c.select(hi, &view).unwrap());
            let (near, far) = (d1.min(d2), d1.max(d2));
            let at = |d| c.select(lo, &ViewParams { distance: d, pixels_per_unit: ppu }).unwrap();
            prop_assert!(at(near) <= at(far));
        }
    }
}
