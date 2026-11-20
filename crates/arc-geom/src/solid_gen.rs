//! Generators of polyhedral solids from planar regions: extrusion and prisms.
//!
//! [`extrude`] sweeps a [`Region`] placed in a [`PlaneFrame`] along the frame's normal; the
//! region's outer loops and holes become the side walls, and its parts become two caps each.
//!
//! - **Arc rule.** Every curved edge is replaced by the chords of
//!   [`CurveFlatten`](crate::CurveFlatten) at the caller's `chord` tolerance with no angle
//!   limit: the chords' end points lie on the curve and the curve lies within `chord` of its
//!   chords. The solid therefore differs from the exact swept shape by at most `chord` times
//!   the length of the curved edges times the height, and the difference is inside the exact
//!   shape along a convex arc (the outer arc of a column) and outside it along a concave one (a
//!   round hole: the faceted hole is smaller than the round one, so the solid is larger). Straight edges are kept as
//!   they are. The same vertices serve the top, bottom and side faces, so the facets of
//!   neighboring faces meet exactly.
//! - **Straight edges.** The chord applies to curved edges only. A straight edge is kept
//!   exactly, whatever its length, so a jog or a notch shorter than the chord is part of the
//!   solid. Only a straight segment not longer than `tolerance.linear` between two other curves
//!   (a joint that closed a gap when the loop was made) is not an edge.
//! - **Tags.** Each side face is tagged [`FaceTag::Side`] with `loop_index`, `curve` and
//!   `facet`, which name the input and do not depend on the chord.
//!   Loops are numbered in the order of the region: the outer loop of each part is followed by
//!   its holes in the order given to [`RegionPart::new`](crate::RegionPart::new), parts in order,
//!   from 0. `curve` is the index of the curve in the loop as the region stores it (the outer loop
//!   counterclockwise, holes clockwise), counting the curves that are edges (a joint, see above,
//!   is not counted). Tags follow the region as stored, not the caller's drawing direction: a
//!   clockwise outline is reversed by [`Region::new`], so its curve 0 is the reverse of the last
//!   curve drawn, and a region that is saved and loaded or compared equal numbers identically.
//!   `facet` is the index of the chord within a curved edge, counted in the stored direction of
//!   the curve, and 0 for a straight edge: the face of a straight edge keeps its tag when the
//!   chord or the radius of a neighboring arc changes. A mirrored [`Region::transformed`]
//!   region is a different region: its loops are renumbered as it stores them. The cap whose outward
//!   normal is the frame's normal is [`FaceTag::Top`], the other [`FaceTag::Bottom`], whatever
//!   the sign of the height.
//! - **Height.** A negative height extrudes against the normal. A height whose magnitude is not
//!   larger than `tolerance.linear`, or not finite, fails with `geom.solid.extrusion_invalid`.
//! - **Region validity.** `Region::new` checks only bounding boxes, so the flattened loops are
//!   also run through the exact tests of [`crate::triangulate`], which refuse crossing or touching
//!   loops, holes outside their outer loop and overlapping parts. Such a region fails with that
//!   function's code instead of producing a self-intersecting solid.
//! - **Bounds.** The solid has twice the flattened vertex count of the region; more than
//!   `limits.array_length` vertices fail with `geom.solid.limit_exceeded`.

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::error::codes;
use crate::plane3::{Plane3, PlaneFrame};
use crate::region::Region;
use crate::solid::{FaceTag, PolyFace, Polyhedron};
use crate::triangulate::{Merge, flatten_region_edges, triangulate_point_loops};

/// Extrudes `region`, placed in `frame` (a region point `(x, y)` is the world point
/// `frame.to_world`), by `height` along the frame's normal, as the [module
/// documentation](self) describes.
///
/// # Errors
///
/// `geom.tolerance.invalid` for an invalid tolerance or a `chord` that is not finite and
/// positive; `geom.solid.extrusion_invalid` for an unusable height; `geom.solid.limit_exceeded`;
/// the errors of [`crate::triangulate`] for loops that do not form a valid region; the errors of
/// [`Polyhedron::new`] (a solid whose faces are within the tolerances of degenerate).
pub fn extrude(
    region: &Region,
    frame: &PlaneFrame,
    height: f64,
    chord: f64,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Polyhedron> {
    crate::ensure_tolerance(tolerance)?;
    let positive_chord = chord.is_finite() && chord > 0.0;
    if !positive_chord {
        return Err(Error::new(codes::TOLERANCE_INVALID));
    }
    let usable_height = height.is_finite() && height.abs() > tolerance.linear;
    if !usable_height {
        return Err(Error::new(codes::SOLID_EXTRUSION_INVALID));
    }
    let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX) / 2;
    let angle = std::f64::consts::PI;
    let loops = flatten_region_edges(region, chord, angle, limit, Merge::Run(tolerance.linear))?;
    // The exact tests of the triangulation refuse crossing loops and holes outside their outer
    // loop.
    let point_loops: Vec<Vec<arc_base::math::Point2>> =
        loops.iter().map(|flat| flat.points.clone()).collect();
    triangulate_point_loops(&point_loops)?;
    let base = frame.origin() + frame.normal() * height.min(0.0);
    let top = height.abs();
    let place = |p: &arc_base::math::Point2, level: f64| -> Point3 {
        base + frame.u() * p.x + frame.v() * p.y + frame.normal() * level
    };
    let mut vertices: Vec<Point3> = Vec::new();
    let mut faces: Vec<PolyFace> = Vec::new();
    let overflow = || Error::new(codes::SOLID_LIMIT_EXCEEDED);
    let mut next_loop = loops.iter().enumerate();
    for part in region.parts() {
        let mut caps: Vec<(Vec<u32>, Vec<u32>)> = Vec::new();
        for _ in 0..=part.holes().len() {
            let Some((loop_index, flat)) = next_loop.next() else {
                return Err(Error::new(codes::SOLID_TOPOLOGY_INVALID));
            };
            let points = &flat.points;
            let count = u32::try_from(points.len()).map_err(|_| overflow())?;
            let first = u32::try_from(vertices.len()).map_err(|_| overflow())?;
            let middle = first.checked_add(count).ok_or_else(overflow)?;
            let end = middle.checked_add(count).ok_or_else(overflow)?;
            vertices.extend(points.iter().map(|p| place(p, 0.0)));
            vertices.extend(points.iter().map(|p| place(p, top)));
            let bottom: Vec<u32> = (first..middle).collect();
            let upper: Vec<u32> = (middle..end).collect();
            for k in 0..bottom.len() {
                let next = (k + 1) % bottom.len();
                if let (Some(&a), Some(&b), Some(&c), Some(&d)) = (
                    bottom.get(k),
                    bottom.get(next),
                    upper.get(next),
                    upper.get(k),
                ) {
                    let source = flat.sources.get(k).ok_or_else(overflow)?;
                    faces.push(PolyFace::new(vec![a, b, c, d]).with_tag(FaceTag::Side {
                        loop_index: u32::try_from(loop_index).map_err(|_| overflow())?,
                        curve: source.curve,
                        facet: source.facet,
                    }));
                }
            }
            caps.push((bottom, upper));
        }
        let mut caps = caps.into_iter();
        if let Some((bottom, upper)) = caps.next() {
            let (hole_bottoms, hole_tops): (Vec<_>, Vec<_>) = caps.unzip();
            let reversed = |l: Vec<u32>| l.into_iter().rev().collect::<Vec<u32>>();
            let (low, high) = (
                PolyFace {
                    outer: reversed(bottom),
                    holes: hole_bottoms.into_iter().map(reversed).collect(),
                    tag: FaceTag::Bottom,
                },
                PolyFace {
                    outer: upper,
                    holes: hole_tops,
                    tag: FaceTag::Top,
                },
            );
            faces.push(high);
            faces.push(low);
        }
    }
    Polyhedron::new(vertices, faces, tolerance, limits)
}

/// Extrudes `region`, placed in `frame`, from the plane of the frame to `target`, a plane
/// parallel to it, along the frame's normal. The height is the signed distance between the
/// planes along the normal.
///
/// # Errors
///
/// `geom.solid.extrusion_invalid` when the normals are not parallel within
/// `tolerance.angular` or the planes coincide; otherwise as [`extrude`].
pub fn extrude_between(
    region: &Region,
    frame: &PlaneFrame,
    target: &Plane3,
    chord: f64,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Polyhedron> {
    crate::ensure_tolerance(tolerance)?;
    let normal: Vector3 = frame.normal();
    if normal.cross(&target.normal()).norm() > tolerance.angular {
        return Err(Error::new(codes::SOLID_EXTRUSION_INVALID));
    }
    let height = normal.dot(&(target.origin() - frame.origin()));
    extrude(region, frame, height, chord, tolerance, limits)
}

/// A prism over `footprint`, a region in the world x–y plane, from the elevation
/// `base_elevation` up by `height` (down for a negative height), with arcs faceted as in
/// [`extrude`].
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite elevation; otherwise as [`extrude`].
pub fn prism(
    footprint: &Region,
    base_elevation: f64,
    height: f64,
    chord: f64,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Polyhedron> {
    crate::ensure_finite(&[base_elevation])?;
    let frame = PlaneFrame::new(
        Point3::new(0.0, 0.0, base_elevation),
        Vector3::x(),
        Vector3::y(),
        tolerance,
    )?;
    extrude(footprint, &frame, height, chord, tolerance, limits)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::curve::Curve2;
    use crate::primitives::{Circle2, Segment2};
    use crate::region::{Loop2, RegionPart};
    use arc_base::math::Point2;
    use proptest::prelude::*;

    const TOL: Tolerance = Tolerance::DEFAULT;

    pub fn polygon(points: &[(f64, f64)]) -> Loop2 {
        let pts: Vec<Point2> = points.iter().map(|&(x, y)| Point2::new(x, y)).collect();
        let curves = (0..pts.len())
            .map(|k| {
                Curve2::Segment(Segment2::new(pts[k], pts[(k + 1) % pts.len()]).expect("segment"))
            })
            .collect();
        Loop2::new(curves, &TOL).expect("loop")
    }

    pub fn region(outer: &[(f64, f64)], holes: &[&[(f64, f64)]]) -> Region {
        Region::new(vec![RegionPart::new(
            polygon(outer),
            holes.iter().map(|h| polygon(h)).collect(),
        )])
        .expect("region")
    }

    pub fn l_shape() -> Region {
        region(
            &[
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 1.0),
                (1.0, 1.0),
                (1.0, 3.0),
                (0.0, 3.0),
            ],
            &[],
        )
    }

    fn code<T>(result: Result<T>) -> Option<&'static str> {
        result.err().map(|e| e.code())
    }

    #[test]
    fn l_shaped_prism_has_exact_volume_and_faces() {
        let solid = prism(&l_shape(), 2.0, 3.0, 1e-3, &TOL, &Limits::DEFAULT).expect("prism");
        // Area 4 + 2 = 6.
        assert!((solid.volume() - 18.0).abs() < 1e-12);
        assert_eq!(solid.faces().len(), 6 + 2);
        assert_eq!(solid.bounding_box().min(), Point3::new(0.0, 0.0, 2.0));
        assert_eq!(solid.bounding_box().max(), Point3::new(4.0, 3.0, 5.0));
        // Surface: two caps of 6, sides = perimeter 14 * 3.
        assert!((solid.surface_area() - (12.0 + 42.0)).abs() < 1e-9);
    }

    #[test]
    fn prism_with_a_hole_has_the_hole_in_both_caps() {
        let r = region(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            &[&[(2.0, 2.0), (2.0, 4.0), (4.0, 4.0), (4.0, 2.0)]],
        );
        let solid = prism(&r, 0.0, 2.0, 1e-3, &TOL, &Limits::DEFAULT).expect("prism");
        assert!((solid.volume() - 192.0).abs() < 1e-10);
        let with_holes = solid.faces().iter().filter(|f| f.holes.len() == 1).count();
        assert_eq!(with_holes, 2);
        // Faces with holes triangulate and cover the face area.
        for (index, face) in solid.faces().iter().enumerate() {
            if face.holes.is_empty() {
                continue;
            }
            let triangles = solid.triangulate_face(index, &TOL).expect("triangulation");
            let area: f64 = triangles
                .iter()
                .map(|t| {
                    let v = |k: u32| solid.vertices()[k as usize];
                    (v(t[1]) - v(t[0])).cross(&(v(t[2]) - v(t[0]))).norm() / 2.0
                })
                .sum();
            assert!((area - 96.0).abs() < 1e-9);
        }
    }

    #[test]
    fn negative_height_and_between_planes_agree() {
        let frame = Plane3::new(Point3::new(0.0, 0.0, 5.0), Vector3::z())
            .expect("plane")
            .frame(&TOL);
        let down = extrude(&l_shape(), &frame, -2.0, 1e-3, &TOL, &Limits::DEFAULT).expect("down");
        assert_eq!(down.bounding_box().min().z, 3.0);
        let target = Plane3::new(Point3::new(9.0, 9.0, 3.0), Vector3::z()).expect("plane");
        let between = extrude_between(&l_shape(), &frame, &target, 1e-3, &TOL, &Limits::DEFAULT)
            .expect("between");
        assert_eq!(between, down);
        let tilted =
            Plane3::new(Point3::new(0.0, 0.0, 3.0), Vector3::new(0.0, 0.1, 1.0)).expect("plane");
        assert_eq!(
            code(extrude_between(
                &l_shape(),
                &frame,
                &tilted,
                1e-3,
                &TOL,
                &Limits::DEFAULT
            )),
            Some("geom.solid.extrusion_invalid")
        );
    }

    #[test]
    fn unusable_inputs_are_refused() {
        let l = l_shape();
        for height in [0.0, 1e-7, f64::NAN, f64::INFINITY] {
            assert_eq!(
                code(prism(&l, 0.0, height, 1e-3, &TOL, &Limits::DEFAULT)),
                Some("geom.solid.extrusion_invalid")
            );
        }
        assert_eq!(
            code(prism(&l, 0.0, 1.0, 0.0, &TOL, &Limits::DEFAULT)),
            Some("geom.tolerance.invalid")
        );
        assert_eq!(
            code(prism(&l, f64::NAN, 1.0, 1e-3, &TOL, &Limits::DEFAULT)),
            Some("geom.input.not_finite")
        );
        let small = Limits {
            array_length: 10,
            ..Limits::DEFAULT
        };
        assert_eq!(
            code(prism(&l, 0.0, 1.0, 1e-3, &TOL, &small)),
            Some("geom.flatten.too_many_vertices")
        );
        // A hole that crosses its outer loop is refused by the triangulation tests.
        let crossing = region(
            &[
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 1.0),
                (1.0, 1.0),
                (1.0, 3.0),
                (0.0, 3.0),
            ],
            &[&[(0.5, 0.5), (0.5, 2.0), (2.0, 2.0), (2.0, 0.5)]],
        );
        assert!(prism(&crossing, 0.0, 1.0, 1e-3, &TOL, &Limits::DEFAULT).is_err());
    }

    #[test]
    fn circles_are_faceted_within_the_chord_inside_the_exact_shape() {
        let circle = Circle2::new(Point2::new(1.0, 1.0), 2.0).expect("circle");
        let curves = vec![Curve2::Circle(circle)];
        let r = Region::new(vec![RegionPart::new(
            Loop2::new(curves, &TOL).expect("loop"),
            vec![],
        )])
        .expect("region");
        let chord = 1e-3;
        let solid = prism(&r, 0.0, 1.5, chord, &TOL, &Limits::DEFAULT).expect("cylinder");
        let exact = std::f64::consts::PI * 4.0 * 1.5;
        assert!(solid.volume() < exact);
        assert!(exact - solid.volume() <= 2.0 * std::f64::consts::PI * 2.0 * chord * 1.5);
        // Every vertex is on the circle (chords inscribed).
        for v in solid.vertices() {
            let d = ((v.x - 1.0).powi(2) + (v.y - 1.0).powi(2)).sqrt();
            assert!((d - 2.0).abs() < 1e-9);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        #[test]
        fn extruded_volume_is_area_times_height_and_rigid_transforms_preserve_it(
            w in 0.5..20.0_f64, d in 0.5..20.0_f64, notch in 0.1..0.45_f64,
            height in prop_oneof![0.2..10.0_f64, -10.0..-0.2_f64],
            angle in -3.0..3.0_f64, tx in -50.0..50.0_f64, ty in -50.0..50.0_f64, tz in -50.0..50.0_f64,
        ) {
            // A concave outline: a rectangle with a notch cut from the top.
            let region = region(
                &[(0.0, 0.0), (w, 0.0), (w, d), (w * (1.0 - notch), d), (w * (1.0 - notch), d * 0.5), (w * notch, d * 0.5), (w * notch, d), (0.0, d)],
                &[],
            );
            let area = w * d - w * (1.0 - 2.0 * notch) * d * 0.5;
            let solid = prism(&region, 1.0, height, 1e-3, &TOL, &Limits::DEFAULT).expect("prism");
            prop_assert!((solid.volume() - area * height.abs()).abs() < 1e-8 * (1.0 + area * height.abs()));
            let transform = arc_base::math::Transform3::from_matrix_unchecked(
                arc_base::math::Matrix4::new_translation(&Vector3::new(tx, ty, tz))
                    * arc_base::math::Matrix4::new_rotation(Vector3::new(1.0, 2.0, 3.0).normalize() * angle),
            );
            let moved = solid.transformed(&transform, &TOL, &Limits::DEFAULT).expect("rigid");
            prop_assert!((moved.volume() - solid.volume()).abs() < 1e-7 * (1.0 + solid.volume()));
            prop_assert!((moved.surface_area() - solid.surface_area()).abs() < 1e-7 * (1.0 + solid.surface_area()));
        }
    }

    use crate::primitives::Arc2;

    fn segment(a: (f64, f64), b: (f64, f64)) -> Curve2 {
        Curve2::Segment(
            Segment2::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1)).expect("segment"),
        )
    }

    fn solid_of(loop_: Loop2, chord: f64) -> Polyhedron {
        let region = Region::new(vec![RegionPart::new(loop_, vec![])]).expect("region");
        prism(&region, 0.0, 3.0, chord, &TOL, &Limits::DEFAULT).expect("prism")
    }

    #[test]
    fn straight_edges_shorter_than_the_chord_are_kept() {
        // A wall with a 5 mm jog: 5 x 0.2 plus 2.5 x 0.005, height 1.
        let jog = region(
            &[
                (0.0, 0.0),
                (5.0, 0.0),
                (5.0, 0.2),
                (2.5, 0.2),
                (2.5, 0.205),
                (0.0, 0.205),
            ],
            &[],
        );
        for chord in [1e-1, 1e-2, 1e-3] {
            let solid = prism(&jog, 0.0, 1.0, chord, &TOL, &Limits::DEFAULT).expect("jog");
            assert!((solid.volume() - 1.0125).abs() < 1e-12, "chord {chord}");
            assert_eq!(solid.vertices().len(), 12);
            assert_eq!(solid.faces().len(), 8);
        }
        // A 5 mm wide notch, 5 cm deep.
        let notch = region(
            &[
                (0.0, 0.0),
                (5.0, 0.0),
                (5.0, 0.2),
                (2.505, 0.2),
                (2.505, 0.15),
                (2.5, 0.15),
                (2.5, 0.2),
                (0.0, 0.2),
            ],
            &[],
        );
        let solid = prism(&notch, 0.0, 1.0, 1e-2, &TOL, &Limits::DEFAULT).expect("notch");
        assert!((solid.volume() - (1.0 - 0.005 * 0.05)).abs() < 1e-12);
        assert_eq!(solid.vertices().len(), 16);
        // A pane 6 mm thick at a chord of 1 cm and a wall 1 mm thick at a chord of 1 mm.
        for (thickness, chord) in [(0.006, 1e-2), (0.001, 1e-3)] {
            let thin = region(
                &[(0.0, 0.0), (5.0, 0.0), (5.0, thickness), (0.0, thickness)],
                &[],
            );
            let solid = prism(&thin, 0.0, 3.0, chord, &TOL, &Limits::DEFAULT).expect("thin");
            assert!((solid.volume() - 15.0 * thickness).abs() < 1e-12);
        }
    }

    fn side_tag_of_normal(solid: &Polyhedron, direction: Vector3) -> Option<FaceTag> {
        (0..solid.faces().len())
            .find(|&k| {
                solid
                    .face_normal(k)
                    .is_some_and(|n| (n - direction).norm() < 1e-9)
                    && matches!(solid.face_tag(k), Some(FaceTag::Side { .. }))
            })
            .and_then(|k| solid.face_tag(k))
    }

    /// The curved wall: the outer arc, the end at `y = 5`, the inner arc and the end at `x = 0`.
    fn curved_wall_curves() -> Vec<Curve2> {
        let outer = Arc2::new(
            Point2::new(0.0, 5.0),
            5.0,
            -std::f64::consts::FRAC_PI_2,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("outer");
        let inner = Arc2::new(
            Point2::new(0.0, 5.0),
            4.8,
            0.0,
            -std::f64::consts::FRAC_PI_2,
        )
        .expect("inner");
        vec![
            Curve2::Arc(outer),
            segment((5.0, 5.0), (4.8, 5.0)),
            Curve2::Arc(inner),
            segment((0.0, 0.2), (0.0, 0.0)),
        ]
    }

    #[test]
    fn a_side_tag_names_the_input_curve_whatever_the_chord() {
        let mut facets = Vec::new();
        for chord in [1e-2, 1e-3] {
            let solid = solid_of(Loop2::new(curved_wall_curves(), &TOL).expect("loop"), chord);
            // The end faces are curves 1 and 3 of loop 0 and have one facet each.
            assert_eq!(
                side_tag_of_normal(&solid, Vector3::y()),
                Some(FaceTag::Side {
                    loop_index: 0,
                    curve: 1,
                    facet: 0
                })
            );
            assert_eq!(
                side_tag_of_normal(&solid, -Vector3::x()),
                Some(FaceTag::Side {
                    loop_index: 0,
                    curve: 3,
                    facet: 0
                })
            );
            // The arcs are curves 0 and 2; their facets are numbered from the start of the curve.
            let outer: Vec<u32> = solid
                .faces()
                .iter()
                .filter_map(|f| match f.tag {
                    FaceTag::Side {
                        curve: 0, facet, ..
                    } => Some(facet),
                    _ => None,
                })
                .collect();
            assert_eq!(
                outer,
                (0..u32::try_from(outer.len()).expect("small")).collect::<Vec<_>>()
            );
            facets.push(outer.len());
        }
        assert!(facets[1] > 2 * facets[0]);
    }

    #[test]
    fn side_tags_follow_the_stored_region_through_a_round_trip() {
        use crate::encoding::GeomCodec;
        // A clockwise rectangle whose first edge is the end at x = 0.
        let cw = [(0.0, 0.0), (0.0, 0.2), (5.0, 0.2), (5.0, 0.0)];
        let curves: Vec<Curve2> = (0..4).map(|k| segment(cw[k], cw[(k + 1) % 4])).collect();
        let region = Region::new(vec![RegionPart::new(
            Loop2::new(curves, &TOL).expect("loop"),
            vec![],
        )])
        .expect("region");
        let decoded = Region::decode(&region.encode(), &Limits::DEFAULT).expect("decode");
        let served: Region =
            serde_json::from_str(&serde_json::to_string(&region).expect("json")).expect("serde");
        assert_eq!(decoded, region);
        assert_eq!(served, region);
        let tags = |r: &Region| {
            let solid = prism(r, 0.0, 3.0, 1e-3, &TOL, &Limits::DEFAULT).expect("prism");
            [-Vector3::x(), Vector3::y(), Vector3::x(), -Vector3::y()]
                .map(|d| side_tag_of_normal(&solid, d))
        };
        let before = tags(&region);
        assert!(before.iter().all(Option::is_some));
        assert_eq!(tags(&decoded), before);
        assert_eq!(tags(&served), before);
        // A mirrored solid keeps every tag.
        let solid = prism(&region, 0.0, 3.0, 1e-3, &TOL, &Limits::DEFAULT).expect("prism");
        let mut matrix = arc_base::math::Matrix4::identity();
        matrix[(1, 1)] = -1.0;
        let mirror = arc_base::math::Transform3::from_matrix_unchecked(matrix);
        let mirrored = solid
            .transformed(&mirror, &TOL, &Limits::DEFAULT)
            .expect("mirror");
        assert!((0..solid.faces().len()).all(|k| solid.face_tag(k) == mirrored.face_tag(k)));
    }
}
