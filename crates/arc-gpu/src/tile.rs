//! Tiled large items (CORE-GPU-005).
//!
//! Vertex positions on the GPU are `f32` offsets from an item origin, which keeps the error
//! below 0.1 mm only while every vertex is within 500 m of the origin on each axis. Items larger
//! than [`TILE_SIZE`] are therefore split into tiles, each with its own `f64` origin at the
//! center of its bounds.
//!
//! - [`split_mesh`] partitions the triangles by recursive median bisection of their centroids
//!   along the longest axis of the bounds until the bounds of a set are at most the bound on
//!   every axis. A triangle is never cut: a triangle larger than the bound is a tile of its own
//!   and exceeds the bound. Tiles copy their vertices exactly and keep normals; other channels
//!   are not used by the renderer and are dropped.
//! - [`split_lines`] splits polylines into runs whose bounds are at most the bound. A segment
//!   longer than half the bound is divided into equal parts first. Runs of one polyline share
//!   their end points; every run is a tile. Closed polylines are split as open ones that return
//!   to their first point.
//!
//! The functions are pure and run in `O(n log n)` with explicit stacks, so they are safe on
//! hostile input sizes within the scene limits.

use arc_base::math::Point3;
use arc_base::{Error, Limits, Result};
use arc_mesh::{AttributeKey, Channel, TriMesh, TriMeshBuilder};

use crate::error::codes;
use crate::scene::{GeometryId, LinePath, MeshInstance, MeshItem};

/// The largest extent of an item on each axis before it is split, in model units (1 km).
pub const TILE_SIZE: f64 = 1000.0;

/// A tile of a mesh: its triangles with vertices copied exactly, and the origin of its bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshTile {
    /// The tile's mesh in the coordinates of the input mesh.
    pub mesh: TriMesh,
    /// The center of the tile's bounds, the origin its geometry and item use.
    pub origin: Point3,
}

/// An uploaded tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TiledGeometry {
    /// The geometry of the tile, uploaded relative to [`TiledGeometry::origin`].
    pub geometry: GeometryId,
    /// The tile origin.
    pub origin: Point3,
}

impl TiledGeometry {
    /// A mesh item for this tile. The instance transforms map tile coordinates to offsets from
    /// the tile origin, so the identity places the tile where the input mesh was.
    #[must_use]
    pub fn item(&self, instances: Vec<MeshInstance>) -> MeshItem {
        MeshItem {
            geometry: self.geometry,
            origin: self.origin,
            instances,
        }
    }
}

/// A run of polylines with its own origin; points are relative to the origin.
#[derive(Debug, Clone, PartialEq)]
pub struct LineTile {
    /// The center of the bounds of the run's points.
    pub origin: Point3,
    /// The paths of the run, as `f32` offsets from the origin.
    pub paths: Vec<LinePath>,
}

fn bounds(points: impl Iterator<Item = Point3>) -> Option<(Point3, Point3)> {
    points.fold(None, |acc, p| match acc {
        None => Some((p, p)),
        Some((lo, hi)) => Some((lo.inf(&p), hi.sup(&p))),
    })
}

fn center(low: &Point3, high: &Point3) -> Point3 {
    // Halving first keeps the sum finite for any finite corners.
    Point3::from(low.coords * 0.5 + high.coords * 0.5)
}

fn check_bound(max_extent: f64) -> Result<()> {
    if max_extent.is_finite() && max_extent > 0.0 {
        Ok(())
    } else {
        Err(Error::new(codes::SETTINGS_INVALID))
    }
}

/// Splits `mesh` into tiles whose bounds are at most `max_extent` on every axis (see the module
/// documentation for the one exception). A mesh within the bound is one tile.
///
/// # Errors
///
/// `gpu.settings.invalid` for a bound that is not positive and finite; `gpu.geometry.invalid`
/// for a mesh without triangles; `gpu.limit.exceeded` when a tile cannot be built.
pub fn split_mesh(mesh: &TriMesh, max_extent: f64) -> Result<Vec<MeshTile>> {
    check_bound(max_extent)?;
    if mesh.triangle_count() == 0 {
        return Err(Error::new(codes::GEOMETRY_INVALID));
    }
    let positions = mesh.positions();
    let corner = |i: u32| positions.get(i as usize).copied();
    let triangles = mesh.triangles();
    let mut stack: Vec<Vec<u32>> =
        vec![(0..u32::try_from(triangles.len()).unwrap_or(u32::MAX)).collect()];
    let mut tiles = Vec::new();
    while let Some(mut set) = stack.pop() {
        let points = |set: &[u32]| {
            set.iter()
                .filter_map(|t| triangles.get(*t as usize))
                .flat_map(|t| t.iter().filter_map(|i| corner(*i)).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        };
        let Some((low, high)) = bounds(points(&set).into_iter()) else {
            continue;
        };
        let extent = high - low;
        let axis = (0..3)
            .max_by(|a, b| extent[*a].total_cmp(&extent[*b]))
            .unwrap_or(0);
        if extent[axis] <= max_extent || set.len() == 1 {
            tiles.push(build_tile(mesh, &set, center(&low, &high))?);
            continue;
        }
        let key = |t: &u32| {
            triangles.get(*t as usize).map_or(0.0, |tri| {
                tri.iter()
                    .filter_map(|i| corner(*i))
                    .map(|p| p[axis])
                    .sum::<f64>()
            })
        };
        set.sort_by(|a, b| key(a).total_cmp(&key(b)).then(a.cmp(b)));
        let upper = set.split_off(set.len() / 2);
        // Pushed in reverse so tiles come out in sorted order.
        stack.push(upper);
        stack.push(set);
    }
    Ok(tiles)
}

fn build_tile(mesh: &TriMesh, set: &[u32], origin: Point3) -> Result<MeshTile> {
    let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut positions = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let source_normals = mesh.normals();
    let mut triangles = Vec::with_capacity(set.len());
    for t in set {
        let Some(tri) = mesh.triangles().get(*t as usize) else {
            continue;
        };
        let mut out = [0_u32; 3];
        for (slot, vertex) in out.iter_mut().zip(tri) {
            let next =
                u32::try_from(positions.len()).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
            let index = *map.entry(*vertex).or_insert(next);
            if index == next {
                positions.push(
                    mesh.positions()
                        .get(*vertex as usize)
                        .copied()
                        .ok_or_else(|| Error::new(codes::GEOMETRY_INVALID))?,
                );
                if let Some(n) = source_normals {
                    normals.push(n.get(*vertex as usize).copied().unwrap_or([0.0; 3]));
                }
            }
            *slot = index;
        }
        triangles.push(out);
    }
    let mut builder = TriMeshBuilder::new(positions, triangles);
    if source_normals.is_some() {
        builder = builder.channel(AttributeKey::Normal, Channel::Vec3(normals));
    }
    let limits = Limits {
        array_length: u64::from(u32::MAX),
        memory_bytes: None,
        ..Limits::DEFAULT
    };
    let mesh = builder
        .build(&limits)
        .map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
    Ok(MeshTile { mesh, origin })
}

/// Splits polylines (points and a closed flag) into runs whose bounds are at most `max_extent`
/// on every axis. Empty paths are skipped. `max_segments` bounds the segments of all the paths
/// together, after the divisions of long segments (the segments of one item, as
/// [`crate::SceneLimits::max_segments_per_item`] counts them).
///
/// # Errors
///
/// `gpu.settings.invalid` for a bound that is not positive and finite; `gpu.value.not_finite`
/// for a point that is not finite; `gpu.limit.exceeded` when the divided segments exceed
/// `max_segments`, which is checked before the divided points are built.
pub fn split_lines(
    paths: &[(Vec<Point3>, bool)],
    max_extent: f64,
    max_segments: u32,
) -> Result<Vec<LineTile>> {
    check_bound(max_extent)?;
    let mut tiles = Vec::new();
    let mut budget = max_segments;
    for (points, closed) in paths {
        if points.iter().any(|p| p.iter().any(|c| !c.is_finite())) {
            return Err(Error::new(codes::VALUE_NOT_FINITE));
        }
        let mut chain: Vec<Point3> = points.clone();
        if *closed && let Some(first) = points.first() {
            chain.push(*first);
        }
        let divided = divide(&chain, max_extent / 2.0, &mut budget)?;
        let mut run: Vec<Point3> = Vec::new();
        // The bounds of the run so far, grown by each point.
        let mut span: Option<(Point3, Point3)> = None;
        let flush = |run: &mut Vec<Point3>, tiles: &mut Vec<LineTile>| {
            if let Some((low, high)) = bounds(run.iter().copied()) {
                let origin = center(&low, &high);
                let points = run
                    .iter()
                    .map(|p| (p - origin).map(narrow).into())
                    .collect();
                tiles.push(LineTile {
                    origin,
                    paths: vec![LinePath {
                        points,
                        closed: false,
                    }],
                });
            }
        };
        for p in divided {
            let grown = |span: Option<(Point3, Point3)>| {
                span.map_or((p, p), |(low, high)| (low.inf(&p), high.sup(&p)))
            };
            let (low, high) = grown(span);
            if (high - low).max() > max_extent && run.len() > 1 {
                let last = run.last().copied();
                flush(&mut run, &mut tiles);
                run.clear();
                run.extend(last);
                span = last.map(|l| (l, l));
            }
            span = Some(grown(span));
            run.push(p);
        }
        flush(&mut run, &mut tiles);
    }
    Ok(tiles)
}

fn narrow(v: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Narrowing to the GPU's precision is the intent.
    let narrowed = v as f32;
    narrowed
}

/// `points` with every segment longer than `piece` divided into equal parts of at most `piece`.
/// Each segment takes its parts from `budget` before any point is built; a segment that needs
/// more than the budget left is `gpu.limit.exceeded`.
fn divide(points: &[Point3], piece: f64, budget: &mut u32) -> Result<Vec<Point3>> {
    let mut out = Vec::new();
    for pair in points.windows(2) {
        let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        if out.is_empty() {
            out.push(*a);
        }
        let length = (b - a).norm();
        let parts = (length / piece).ceil().max(1.0);
        // The comparison is in `f64`, so a huge or infinite ratio fails before any cast.
        if !(parts.is_finite() && parts <= f64::from(*budget)) {
            return Err(
                Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_segments_per_item")
            );
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // At most the budget.
        let parts = parts as u32;
        *budget -= parts;
        for k in 1..parts {
            out.push(a + (b - a) * (f64::from(k) / f64::from(parts)));
        }
        out.push(*b);
    }
    if out.is_empty() {
        out.extend(points.first().copied());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests fail by panicking")]

    use proptest::prelude::*;

    use super::*;

    /// The segments of one line item by default.
    const SEGMENTS: u32 = crate::scene::SceneLimits::DEFAULT.max_segments_per_item;

    fn grid(n: u32, spacing: f64, origin: Point3) -> TriMesh {
        let mut positions = Vec::new();
        for y in 0..=n {
            for x in 0..=n {
                positions.push(
                    origin
                        + arc_base::math::Vector3::new(
                            f64::from(x) * spacing,
                            f64::from(y) * spacing,
                            0.0,
                        ),
                );
            }
        }
        let mut triangles = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let i = y * (n + 1) + x;
                triangles.push([i, i + 1, i + n + 2]);
                triangles.push([i, i + n + 2, i + n + 1]);
            }
        }
        TriMesh::new(positions, triangles, &Limits::DEFAULT).unwrap()
    }

    fn corners(mesh: &TriMesh) -> Vec<[[u64; 3]; 3]> {
        let mut out: Vec<_> = mesh
            .triangles()
            .iter()
            .map(|t| t.map(|i| mesh.positions()[i as usize].coords.map(f64::to_bits).into()))
            .collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn small_meshes_are_one_tile_and_bad_bounds_are_rejected() {
        let mesh = grid(2, 10.0, Point3::origin());
        let tiles = split_mesh(&mesh, TILE_SIZE).unwrap();
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].origin, Point3::new(10.0, 10.0, 0.0));
        assert_eq!(
            split_mesh(&mesh, 0.0).unwrap_err().code(),
            codes::SETTINGS_INVALID.code
        );
        assert_eq!(
            split_mesh(&mesh, f64::NAN).unwrap_err().code(),
            codes::SETTINGS_INVALID.code
        );
    }

    #[test]
    fn a_triangle_larger_than_the_bound_is_its_own_tile() {
        let mesh = TriMesh::new(
            vec![
                Point3::origin(),
                Point3::new(5000.0, 0.0, 0.0),
                Point3::new(0.0, 10.0, 0.0),
            ],
            vec![[0, 1, 2]],
            &Limits::DEFAULT,
        )
        .unwrap();
        assert_eq!(split_mesh(&mesh, TILE_SIZE).unwrap().len(), 1);
    }

    #[test]
    fn a_path_that_divides_into_too_many_segments_is_refused_before_it_is_built() {
        // Two points 2e12 apart need 4e9 segments at a bound of 1 km; the budget refuses them
        // before any divided point is built.
        let far = (vec![Point3::origin(), Point3::new(2.0e12, 0.0, 0.0)], false);
        assert_eq!(
            split_lines(std::slice::from_ref(&far), TILE_SIZE, SEGMENTS)
                .unwrap_err()
                .code(),
            codes::LIMIT_EXCEEDED.code
        );
        // The budget is shared by the paths of one call.
        let near = (vec![Point3::origin(), Point3::new(5000.0, 0.0, 0.0)], false);
        assert!(split_lines(std::slice::from_ref(&near), TILE_SIZE, 100).is_ok());
        assert_eq!(
            split_lines(&[near.clone(), near], TILE_SIZE, 10)
                .unwrap_err()
                .code(),
            codes::LIMIT_EXCEEDED.code
        );
    }

    #[test]
    fn lines_longer_than_the_bound_are_cut_and_stay_connected() {
        let path = (
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(5000.0, 0.0, 0.0)],
            false,
        );
        let tiles = split_lines(&[path], TILE_SIZE, SEGMENTS).unwrap();
        assert!(tiles.len() >= 5);
        for pair in tiles.windows(2) {
            let end = |t: &LineTile| {
                let p = t.paths[0].points.last().copied().unwrap();
                t.origin
                    + arc_base::math::Vector3::new(
                        f64::from(p[0]),
                        f64::from(p[1]),
                        f64::from(p[2]),
                    )
            };
            let start = |t: &LineTile| {
                let p = t.paths[0].points[0];
                t.origin
                    + arc_base::math::Vector3::new(
                        f64::from(p[0]),
                        f64::from(p[1]),
                        f64::from(p[2]),
                    )
            };
            assert!((end(&pair[0]) - start(&pair[1])).norm() < 1e-3);
        }
    }

    #[test]
    fn a_long_run_of_short_segments_stays_one_run() {
        // 30,000 short segments within 10 units form one run. The run's bounds are grown per
        // point, so the split takes time linear in the points, not quadratic.
        let points: Vec<Point3> = (0..30_000_u32)
            .map(|i| Point3::new(f64::from(i % 2) * 10.0, f64::from(i) * 1e-4, 0.0))
            .collect();
        let tiles = split_lines(&[(points, false)], TILE_SIZE, SEGMENTS).unwrap();
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].paths[0].points.len(), 30_000);
    }

    proptest! {
        /// Every tile is within the bound, the tiles hold exactly the input triangles, and
        /// positions rebuilt from the tile origin are within 0.1 mm of the input even 10^7
        /// from the origin.
        #[test]
        fn mesh_tiles_are_bounded_complete_and_precise(
            n in 1_u32..14,
            spacing in 50.0_f64..400.0,
            far in prop::sample::select(vec![0.0_f64, 1.0e5, 1.0e7]),
            bound in 700.0_f64..1500.0,
        ) {
            let mesh = grid(n, spacing, Point3::new(far + 0.123_456_789, -far, far / 3.0));
            let tiles = split_mesh(&mesh, bound).unwrap();
            let mut all = Vec::new();
            for tile in &tiles {
                let b = tile.mesh.bounding_box();
                let extent = b.max() - b.min();
                prop_assert!(extent.max() <= bound, "{extent:?} > {bound}");
                let relative = tile.mesh.gpu_positions(&tile.origin).unwrap();
                for (p, r) in tile.mesh.positions().iter().zip(relative) {
                    let back = tile.origin + arc_base::math::Vector3::new(f64::from(r.x), f64::from(r.y), f64::from(r.z));
                    prop_assert!((back - p).norm() < 1.0e-4);
                }
                all.extend(corners(&tile.mesh));
            }
            all.sort_unstable();
            prop_assert_eq!(all, corners(&mesh));
        }

        /// Every run of a split polyline is within the bound, and the runs join end to end.
        #[test]
        fn line_runs_are_bounded_and_joined(
            points in prop::collection::vec((-4000.0_f64..4000.0, -4000.0_f64..4000.0, -50.0_f64..50.0), 2..8),
            bound in 700.0_f64..1500.0,
        ) {
            let points: Vec<Point3> = points.into_iter().map(|(x, y, z)| Point3::new(x + 1.0e7, y, z)).collect();
            let tiles = split_lines(&[(points.clone(), false)], bound, SEGMENTS).unwrap();
            let world = |t: &LineTile, p: [f32; 3]| t.origin + arc_base::math::Vector3::new(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
            for t in &tiles {
                let pts: Vec<Point3> = t.paths[0].points.iter().map(|p| world(t, *p)).collect();
                let (low, high) = bounds(pts.iter().copied()).unwrap();
                prop_assert!((high - low).max() <= bound + 1.0e-3);
            }
            let first = world(&tiles[0], tiles[0].paths[0].points[0]);
            prop_assert!((first - points[0]).norm() < 1.0e-3);
            let last_tile = tiles.last().unwrap();
            let last = world(last_tile, *last_tile.paths[0].points.last().unwrap());
            prop_assert!((last - points.last().copied().unwrap()).norm() < 1.0e-3);
        }
    }
}
