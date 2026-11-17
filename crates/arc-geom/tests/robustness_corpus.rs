//! The intersection robustness corpus (CORE-GEOM-005): named degenerate and near-degenerate
//! cases with their expected outcomes, each run in both argument orders with the default
//! tolerance (linear 1e-6, angular 1e-9).

#![allow(
    clippy::unwrap_used,
    reason = "the corpus is built from valid literal curves"
)]

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use arc_base::Tolerance;
use arc_base::math::{Point2, Vector2};
use arc_geom::{
    Arc2, Circle2, Contact, Curve2, Ellipse2, EllipticalArc2, Intersection, Line2, Orientation,
    Polyline2, PolylineVertex, Ray2, Segment2, intersect,
};

const TOL: Tolerance = Tolerance::DEFAULT;
/// How closely a result must match the expected numbers.
const MATCH: f64 = 1e-6;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Curve2 {
    Curve2::Segment(Segment2::new(p(x0, y0), p(x1, y1)).unwrap())
}

fn ray(x: f64, y: f64, dx: f64, dy: f64) -> Curve2 {
    Curve2::Ray(Ray2::new(p(x, y), Vector2::new(dx, dy)).unwrap())
}

fn line(x: f64, y: f64, dx: f64, dy: f64) -> Curve2 {
    Curve2::Line(Line2::new(p(x, y), Vector2::new(dx, dy)).unwrap())
}

fn circ(x: f64, y: f64, r: f64) -> Curve2 {
    Curve2::Circle(Circle2::new(p(x, y), r).unwrap())
}

fn arc(x: f64, y: f64, r: f64, start: f64, sweep: f64) -> Curve2 {
    Curve2::Arc(Arc2::new(p(x, y), r, start, sweep).unwrap())
}

fn ellipse(x: f64, y: f64, ax: f64, ay: f64, ratio: f64) -> Ellipse2 {
    Ellipse2::new(p(x, y), Vector2::new(ax, ay), ratio).unwrap()
}

fn ell(x: f64, y: f64, ax: f64, ay: f64, ratio: f64) -> Curve2 {
    Curve2::Ellipse(ellipse(x, y, ax, ay, ratio))
}

/// The ellipse with semi-axes 2 (along x) and 1 at the origin.
fn e21() -> Curve2 {
    ell(0.0, 0.0, 2.0, 0.0, 0.5)
}

fn earc(start: f64, sweep: f64) -> Curve2 {
    Curve2::EllipticalArc(
        EllipticalArc2::new(ellipse(0.0, 0.0, 2.0, 0.0, 0.5), start, sweep).unwrap(),
    )
}

fn poly(vertices: &[(f64, f64, f64)], closed: bool) -> Curve2 {
    let vertices = vertices
        .iter()
        .map(|&(x, y, bulge)| PolylineVertex::new(p(x, y), bulge))
        .collect();
    Curve2::Polyline(Polyline2::new(vertices, closed).unwrap())
}

#[derive(Debug, Clone, Copy)]
enum Item {
    /// A point at (x, y) with its contact and, when given, its parameters on a and b.
    Point(f64, f64, Contact, Option<(f64, f64)>),
    Overlap([f64; 2], [f64; 2], Orientation),
}

use Contact::{Crossing, Tangent};
use Orientation::{Opposite, Same};

fn cross(x: f64, y: f64) -> Item {
    Item::Point(x, y, Crossing, None)
}

fn touch(x: f64, y: f64) -> Item {
    Item::Point(x, y, Tangent, None)
}

fn at(item: Item, a: f64, b: f64) -> Item {
    match item {
        Item::Point(x, y, c, _) => Item::Point(x, y, c, Some((a, b))),
        overlap @ Item::Overlap(..) => overlap,
    }
}

fn overlap(a: [f64; 2], b: [f64; 2], o: Orientation) -> Item {
    Item::Overlap(a, b, o)
}

enum Expect {
    Items(Vec<Item>),
    Error(&'static str),
}

struct Case {
    name: &'static str,
    a: Curve2,
    b: Curve2,
    expect: Expect,
}

fn case(name: &'static str, a: Curve2, b: Curve2, items: Vec<Item>) -> Case {
    Case {
        name,
        a,
        b,
        expect: Expect::Items(items),
    }
}

#[allow(clippy::too_many_lines, reason = "the corpus is one table")]
fn corpus() -> Vec<Case> {
    let s3 = 3.0_f64.sqrt() / 2.0;
    let r2 = 2.0_f64.sqrt();
    let inf = f64::INFINITY;
    let near_tangent_x = (1.0 - (1.0 - 2e-6_f64).powi(2)).sqrt();
    let (ce_x, ce_y) = ((5.0_f64 / 3.0).sqrt(), (2.25_f64 - 5.0 / 3.0).sqrt());
    let ee = 0.8_f64.sqrt();
    let big = 1e6;
    vec![
        // Segments, rays and lines.
        case(
            "segment_segment_crossing",
            seg(0.0, 0.0, 2.0, 2.0),
            seg(0.0, 2.0, 2.0, 0.0),
            vec![at(cross(1.0, 1.0), 0.5, 0.5)],
        ),
        case(
            "segment_segment_parallel_disjoint",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(0.0, 1.0, 1.0, 1.0),
            vec![],
        ),
        case(
            "segment_end_on_segment_interior",
            seg(0.0, 0.0, 2.0, 0.0),
            seg(1.0, 0.0, 1.0, 1.0),
            vec![at(cross(1.0, 0.0), 0.5, 0.0)],
        ),
        case(
            "segment_end_on_segment_end",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(1.0, 0.0, 1.0, 1.0),
            vec![at(cross(1.0, 0.0), 1.0, 0.0)],
        ),
        case(
            "collinear_partial_overlap",
            seg(0.0, 0.0, 2.0, 0.0),
            seg(1.0, 0.0, 3.0, 0.0),
            vec![overlap([0.5, 1.0], [0.0, 0.5], Same)],
        ),
        case(
            "collinear_opposite_overlap",
            seg(0.0, 0.0, 2.0, 0.0),
            seg(3.0, 0.0, 1.0, 0.0),
            vec![overlap([0.5, 1.0], [0.5, 1.0], Opposite)],
        ),
        case(
            "collinear_contained",
            seg(0.0, 0.0, 4.0, 0.0),
            seg(1.0, 0.0, 2.0, 0.0),
            vec![overlap([0.25, 0.5], [0.0, 1.0], Same)],
        ),
        case(
            "collinear_identical",
            seg(0.0, 0.0, 1.0, 1.0),
            seg(0.0, 0.0, 1.0, 1.0),
            vec![overlap([0.0, 1.0], [0.0, 1.0], Same)],
        ),
        case(
            "collinear_touching_end",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(1.0, 0.0, 2.0, 0.0),
            vec![at(touch(1.0, 0.0), 1.0, 0.0)],
        ),
        case(
            "collinear_disjoint",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(2.0, 0.0, 3.0, 0.0),
            vec![],
        ),
        case(
            "collinear_gap_within_tolerance",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(1.0 + 5e-7, 0.0, 2.0, 0.0),
            vec![touch(1.0, 0.0)],
        ),
        case(
            "collinear_gap_beyond_tolerance",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(1.0 + 2e-6, 0.0, 2.0, 0.0),
            vec![],
        ),
        case(
            "parallel_offset_within_tolerance",
            seg(0.0, 0.0, 2.0, 0.0),
            seg(1.0, 5e-7, 3.0, 5e-7),
            vec![overlap([0.5, 1.0], [0.0, 0.5], Same)],
        ),
        case(
            "parallel_offset_beyond_tolerance",
            seg(0.0, 0.0, 2.0, 0.0),
            seg(1.0, 2e-6, 3.0, 2e-6),
            vec![],
        ),
        case(
            "angle_within_angular_tolerance",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(0.5, 0.0, 1.5, 1e-13),
            vec![overlap([0.5, 1.0], [0.0, 0.5], Same)],
        ),
        case(
            "shallow_crossing_beyond_angular_tolerance",
            seg(0.0, 0.0, 2.0, 0.0),
            seg(0.0, -1e-4, 2.0, 1e-4),
            vec![at(cross(1.0, 0.0), 0.5, 0.5)],
        ),
        case(
            "end_near_segment_within_tolerance",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(0.5, 5e-7, 0.5, 1.0),
            vec![at(cross(0.5, 0.0), 0.5, 0.0)],
        ),
        case(
            "end_near_segment_beyond_tolerance",
            seg(0.0, 0.0, 1.0, 0.0),
            seg(0.5, 2e-6, 0.5, 1.0),
            vec![],
        ),
        case(
            "large_coordinates_millimeter_segments",
            seg(big, big, big + 0.002, big + 0.002),
            seg(big, big + 0.002, big + 0.002, big),
            vec![at(cross(big + 0.001, big + 0.001), 0.5, 0.5)],
        ),
        Case {
            name: "extreme_coordinates_segments",
            a: seg(1e12, 1e12, 1e12 + 1.0, 1e12 + 1.0),
            b: seg(1e12, 1e12 + 1.0, 1e12 + 1.0, 1e12),
            expect: Expect::Error("geom.precision.insufficient"),
        },
        case(
            "ray_line_crossing",
            ray(0.0, 0.0, 1.0, 0.0),
            line(5.0, -1.0, 0.0, 1.0),
            vec![at(cross(5.0, 0.0), 5.0, 1.0)],
        ),
        case(
            "ray_line_behind_origin",
            ray(0.0, 0.0, 1.0, 0.0),
            line(-5.0, -1.0, 0.0, 1.0),
            vec![],
        ),
        case(
            "ray_origin_on_line",
            ray(1.0, 0.0, 0.0, 1.0),
            line(0.0, 0.0, 1.0, 0.0),
            vec![at(cross(1.0, 0.0), 0.0, 1.0)],
        ),
        case(
            "lines_collinear_opposite",
            line(0.0, 0.0, 1.0, 0.0),
            line(5.0, 0.0, -2.0, 0.0),
            vec![overlap([-inf, inf], [-inf, inf], Opposite)],
        ),
        case(
            "rays_opposite_overlap",
            ray(0.0, 0.0, 1.0, 0.0),
            ray(3.0, 0.0, -1.0, 0.0),
            vec![overlap([0.0, 3.0], [0.0, 3.0], Opposite)],
        ),
        case(
            "rays_opposite_touching_origins",
            ray(0.0, 0.0, 1.0, 0.0),
            ray(0.0, 0.0, -1.0, 0.0),
            vec![at(touch(0.0, 0.0), 0.0, 0.0)],
        ),
        case(
            "ray_contains_segment",
            ray(0.0, 0.0, 1.0, 0.0),
            seg(2.0, 0.0, 5.0, 0.0),
            vec![overlap([2.0, 5.0], [0.0, 1.0], Same)],
        ),
        case(
            "lines_parallel_disjoint",
            line(0.0, 0.0, 1.0, 0.0),
            line(0.0, 1.0, 1.0, 0.0),
            vec![],
        ),
        // Lines and circular arcs.
        case(
            "segment_circle_secant",
            seg(-2.0, 0.0, 2.0, 0.0),
            circ(0.0, 0.0, 1.0),
            vec![
                at(cross(-1.0, 0.0), 0.25, PI),
                at(cross(1.0, 0.0), 0.75, 0.0),
            ],
        ),
        case(
            "segment_circle_tangent",
            seg(-2.0, 1.0, 2.0, 1.0),
            circ(0.0, 0.0, 1.0),
            vec![at(touch(0.0, 1.0), 0.5, FRAC_PI_2)],
        ),
        case(
            "segment_circle_tangent_outside_within",
            seg(-2.0, 1.0 + 5e-7, 2.0, 1.0 + 5e-7),
            circ(0.0, 0.0, 1.0),
            vec![touch(0.0, 1.0)],
        ),
        case(
            "segment_circle_tangent_inside_within",
            seg(-2.0, 1.0 - 5e-7, 2.0, 1.0 - 5e-7),
            circ(0.0, 0.0, 1.0),
            vec![touch(0.0, 1.0)],
        ),
        case(
            "segment_circle_inside_beyond",
            seg(-2.0, 1.0 - 2e-6, 2.0, 1.0 - 2e-6),
            circ(0.0, 0.0, 1.0),
            vec![
                cross(-near_tangent_x, 1.0 - 2e-6),
                cross(near_tangent_x, 1.0 - 2e-6),
            ],
        ),
        case(
            "segment_circle_outside_beyond",
            seg(-2.0, 1.0 + 2e-6, 2.0, 1.0 + 2e-6),
            circ(0.0, 0.0, 1.0),
            vec![],
        ),
        case(
            "segment_circle_disjoint",
            seg(2.0, 2.0, 3.0, 3.0),
            circ(0.0, 0.0, 1.0),
            vec![],
        ),
        case(
            "segment_end_on_circle",
            seg(0.0, 0.0, 1.0, 0.0),
            circ(0.0, 0.0, 1.0),
            vec![at(cross(1.0, 0.0), 1.0, 0.0)],
        ),
        case(
            "circle_seam_tangent",
            seg(1.0, -1.0, 1.0, 1.0),
            circ(0.0, 0.0, 1.0),
            vec![at(touch(1.0, 0.0), 0.5, 0.0)],
        ),
        case(
            "arc_through_angle_zero",
            arc(0.0, 0.0, 1.0, -FRAC_PI_2, PI),
            seg(0.0, 0.0, 2.0, 0.0),
            vec![at(cross(1.0, 0.0), FRAC_PI_2, 0.5)],
        ),
        case(
            "arc_wrapping_two_pi",
            arc(0.0, 0.0, 1.0, 3.0 * FRAC_PI_2, PI),
            seg(0.0, 0.0, 2.0, 0.0),
            vec![at(cross(1.0, 0.0), FRAC_PI_2, 0.5)],
        ),
        case(
            "arc_end_on_segment",
            arc(0.0, 0.0, 1.0, 0.0, FRAC_PI_2),
            seg(0.0, -2.0, 0.0, 2.0),
            vec![at(cross(0.0, 1.0), FRAC_PI_2, 0.75)],
        ),
        case(
            "arc_misses_where_circle_crosses",
            arc(0.0, 0.0, 1.0, 0.0, FRAC_PI_2),
            seg(-2.0, -0.5, 2.0, -0.5),
            vec![],
        ),
        case(
            "clockwise_arc_secant",
            arc(0.0, 0.0, 1.0, 0.0, -PI),
            seg(-2.0, -0.5, 2.0, -0.5),
            vec![
                at(cross(s3, -0.5), PI / 6.0, (2.0 + s3) / 4.0),
                at(cross(-s3, -0.5), 5.0 * PI / 6.0, (2.0 - s3) / 4.0),
            ],
        ),
        // Circles and circular arcs.
        case(
            "circles_crossing",
            circ(0.0, 0.0, 1.0),
            circ(1.0, 0.0, 1.0),
            vec![
                at(cross(0.5, s3), PI / 3.0, 2.0 * PI / 3.0),
                at(cross(0.5, -s3), 5.0 * PI / 3.0, 4.0 * PI / 3.0),
            ],
        ),
        case(
            "circles_external_tangent_at_seam",
            circ(0.0, 0.0, 1.0),
            circ(2.0, 0.0, 1.0),
            vec![at(touch(1.0, 0.0), 0.0, PI)],
        ),
        case(
            "circles_external_tangent_within",
            circ(0.0, 0.0, 1.0),
            circ(2.0 + 5e-7, 0.0, 1.0),
            vec![touch(1.0, 0.0)],
        ),
        case(
            "circles_overlapping_beyond_tangent",
            circ(0.0, 0.0, 1.0),
            circ(2.0 - 2e-6, 0.0, 1.0),
            vec![
                cross(1.0 - 1e-6, 0.001_414_213_2),
                cross(1.0 - 1e-6, -0.001_414_213_2),
            ],
        ),
        case(
            "circles_apart_beyond_tangent",
            circ(0.0, 0.0, 1.0),
            circ(2.0 + 2e-6, 0.0, 1.0),
            vec![],
        ),
        case(
            "circles_internal_tangent",
            circ(0.0, 0.0, 2.0),
            circ(1.0, 0.0, 1.0),
            vec![at(touch(2.0, 0.0), 0.0, 0.0)],
        ),
        case(
            "circles_internal_tangent_within",
            circ(0.0, 0.0, 2.0),
            circ(1.0 + 5e-7, 0.0, 1.0),
            vec![touch(2.0, 0.0)],
        ),
        case(
            "circles_concentric_different_radii",
            circ(0.0, 0.0, 1.0),
            circ(0.0, 0.0, 2.0),
            vec![],
        ),
        case(
            "circles_coincident",
            circ(0.0, 0.0, 1.0),
            circ(0.0, 0.0, 1.0),
            vec![overlap([0.0, TAU], [0.0, TAU], Same)],
        ),
        case(
            "circles_coincident_within_tolerance",
            circ(0.0, 0.0, 1.0),
            circ(3e-7, 0.0, 1.0 + 3e-7),
            vec![overlap([0.0, TAU], [0.0, TAU], Same)],
        ),
        case(
            "arcs_partial_overlap",
            arc(0.0, 0.0, 1.0, 0.0, PI),
            arc(0.0, 0.0, 1.0, FRAC_PI_2, PI),
            vec![overlap([FRAC_PI_2, PI], [0.0, FRAC_PI_2], Same)],
        ),
        case(
            "arcs_opposite_overlap",
            arc(0.0, 0.0, 1.0, 0.0, PI),
            arc(0.0, 0.0, 1.0, PI, -FRAC_PI_2),
            vec![overlap([FRAC_PI_2, PI], [0.0, FRAC_PI_2], Opposite)],
        ),
        case(
            "arcs_touching_at_end",
            arc(0.0, 0.0, 1.0, 0.0, FRAC_PI_2),
            arc(0.0, 0.0, 1.0, FRAC_PI_2, FRAC_PI_2),
            vec![at(touch(0.0, 1.0), FRAC_PI_2, 0.0)],
        ),
        case(
            "arcs_disjoint_on_one_circle",
            arc(0.0, 0.0, 1.0, 0.0, FRAC_PI_2),
            arc(0.0, 0.0, 1.0, PI, FRAC_PI_2),
            vec![],
        ),
        case(
            "arc_contained_in_arc",
            arc(0.0, 0.0, 1.0, 0.0, PI),
            arc(0.0, 0.0, 1.0, FRAC_PI_4, FRAC_PI_2),
            vec![overlap(
                [FRAC_PI_4, 3.0 * FRAC_PI_4],
                [0.0, FRAC_PI_2],
                Same,
            )],
        ),
        case(
            "circle_and_arc_across_its_seam",
            circ(0.0, 0.0, 1.0),
            arc(0.0, 0.0, 1.0, -FRAC_PI_4, FRAC_PI_2),
            vec![
                overlap([0.0, FRAC_PI_4], [FRAC_PI_4, FRAC_PI_2], Same),
                overlap([7.0 * FRAC_PI_4, TAU], [0.0, FRAC_PI_4], Same),
            ],
        ),
        case(
            "arcs_overlapping_at_both_ends",
            arc(0.0, 0.0, 1.0, 0.0, 3.0 * FRAC_PI_2),
            arc(0.0, 0.0, 1.0, PI, 3.0 * FRAC_PI_2),
            vec![
                overlap([0.0, FRAC_PI_2], [PI, 3.0 * FRAC_PI_2], Same),
                overlap([PI, 3.0 * FRAC_PI_2], [0.0, FRAC_PI_2], Same),
            ],
        ),
        // Lines and ellipses.
        case(
            "segment_ellipse_secant",
            seg(-3.0, 0.0, 3.0, 0.0),
            e21(),
            vec![
                at(cross(-2.0, 0.0), 1.0 / 6.0, PI),
                at(cross(2.0, 0.0), 5.0 / 6.0, 0.0),
            ],
        ),
        case(
            "segment_ellipse_tangent",
            seg(-3.0, 1.0, 3.0, 1.0),
            e21(),
            vec![at(touch(0.0, 1.0), 0.5, FRAC_PI_2)],
        ),
        case(
            "segment_ellipse_tangent_within",
            seg(-3.0, 1.0 - 5e-7, 3.0, 1.0 - 5e-7),
            e21(),
            vec![touch(0.0, 1.0)],
        ),
        case(
            "segment_ellipse_inside_beyond",
            seg(-3.0, 1.0 - 2e-6, 3.0, 1.0 - 2e-6),
            e21(),
            vec![
                cross(-2.0 * near_tangent_x, 1.0 - 2e-6),
                cross(2.0 * near_tangent_x, 1.0 - 2e-6),
            ],
        ),
        case(
            "segment_rotated_ellipse",
            seg(-3.0, -3.0, 3.0, 3.0),
            ell(0.0, 0.0, r2, r2, 0.5),
            vec![cross(-r2, -r2), cross(r2, r2)],
        ),
        case(
            "elliptical_arc_end_on_segment",
            earc(0.0, FRAC_PI_2),
            seg(0.0, -2.0, 0.0, 2.0),
            vec![at(cross(0.0, 1.0), FRAC_PI_2, 0.75)],
        ),
        case(
            "elliptical_arc_across_angle_zero",
            earc(-FRAC_PI_4, FRAC_PI_2),
            seg(0.0, 0.0, 3.0, 0.0),
            vec![at(cross(2.0, 0.0), FRAC_PI_4, 2.0 / 3.0)],
        ),
        // Circles and ellipses.
        case(
            "ellipse_circle_four_crossings",
            e21(),
            circ(0.0, 0.0, 1.5),
            vec![
                cross(ce_x, ce_y),
                cross(-ce_x, ce_y),
                cross(-ce_x, -ce_y),
                cross(ce_x, -ce_y),
            ],
        ),
        case(
            "ellipse_circle_inner_tangent",
            e21(),
            circ(0.0, 0.0, 1.0),
            vec![touch(0.0, 1.0), touch(0.0, -1.0)],
        ),
        case(
            "ellipse_circle_outer_tangent",
            e21(),
            circ(0.0, 0.0, 2.0),
            vec![at(touch(2.0, 0.0), 0.0, 0.0), at(touch(-2.0, 0.0), PI, PI)],
        ),
        case(
            "unit_ratio_ellipse_is_its_circle",
            ell(0.0, 0.0, 1.0, 0.0, 1.0),
            circ(0.0, 0.0, 1.0),
            vec![overlap([0.0, TAU], [0.0, TAU], Same)],
        ),
        // Ellipses and elliptical arcs.
        case(
            "ellipses_coincident_other_axis_sign",
            e21(),
            ell(0.0, 0.0, -2.0, 0.0, 0.5),
            vec![
                overlap([0.0, PI], [PI, TAU], Same),
                overlap([PI, TAU], [0.0, PI], Same),
            ],
        ),
        case(
            "ellipses_four_crossings",
            e21(),
            ell(0.0, 0.0, 0.0, 2.0, 0.5),
            vec![
                cross(ee, ee),
                cross(-ee, ee),
                cross(-ee, -ee),
                cross(ee, -ee),
            ],
        ),
        case(
            "ellipses_external_tangent",
            e21(),
            ell(4.0, 0.0, 2.0, 0.0, 0.5),
            vec![at(touch(2.0, 0.0), 0.0, PI)],
        ),
        case(
            "ellipses_external_tangent_within",
            e21(),
            ell(4.0 + 5e-7, 0.0, 2.0, 0.0, 0.5),
            vec![touch(2.0, 0.0)],
        ),
        case(
            "ellipses_overlapping_beyond_tangent",
            e21(),
            ell(4.0 - 2e-6, 0.0, 2.0, 0.0, 0.5),
            vec![cross(2.0 - 1e-6, 0.001), cross(2.0 - 1e-6, -0.001)],
        ),
        case(
            "ellipses_disjoint",
            e21(),
            ell(10.0, 10.0, 2.0, 0.0, 0.5),
            vec![],
        ),
        case(
            "ellipses_large_coordinates_millimeter_size",
            ell(big, big, 0.002, 0.0, 0.5),
            ell(big, big, 0.0, 0.002, 0.5),
            vec![
                cross(big + ee * 1e-3, big + ee * 1e-3),
                cross(big - ee * 1e-3, big + ee * 1e-3),
                cross(big - ee * 1e-3, big - ee * 1e-3),
                cross(big + ee * 1e-3, big - ee * 1e-3),
            ],
        ),
        case(
            "elliptical_arcs_partial_overlap",
            earc(0.0, PI),
            earc(FRAC_PI_2, PI),
            vec![overlap([FRAC_PI_2, PI], [0.0, FRAC_PI_2], Same)],
        ),
        case(
            "ellipses_concentric_homothetic",
            e21(),
            ell(0.0, 0.0, 3.0, 0.0, 0.5),
            vec![],
        ),
        // Polylines.
        case(
            "polyline_crossing_at_vertex",
            poly(&[(0.0, 0.0, 0.0), (1.0, 1.0, 0.0), (2.0, 0.0, 0.0)], false),
            seg(1.0, 0.0, 1.0, 2.0),
            vec![at(cross(1.0, 1.0), 1.0, 0.5)],
        ),
        case(
            "closed_polyline_secant",
            poly(
                &[
                    (0.0, 0.0, 0.0),
                    (2.0, 0.0, 0.0),
                    (2.0, 2.0, 0.0),
                    (0.0, 2.0, 0.0),
                ],
                true,
            ),
            seg(-1.0, 1.0, 3.0, 1.0),
            vec![
                at(cross(2.0, 1.0), 1.5, 0.75),
                at(cross(0.0, 1.0), 3.5, 0.25),
            ],
        ),
        case(
            "closed_polyline_seam_vertex",
            poly(
                &[
                    (0.0, 0.0, 0.0),
                    (2.0, 0.0, 0.0),
                    (2.0, 2.0, 0.0),
                    (0.0, 2.0, 0.0),
                ],
                true,
            ),
            seg(-1.0, -1.0, 1.0, 1.0),
            vec![at(cross(0.0, 0.0), 0.0, 0.5)],
        ),
        case(
            "polyline_overlap_across_vertex",
            poly(
                &[
                    (0.0, 0.0, 0.0),
                    (1.0, 0.0, 0.0),
                    (2.0, 0.0, 0.0),
                    (2.0, 1.0, 0.0),
                ],
                false,
            ),
            seg(0.5, 0.0, 3.0, 0.0),
            vec![overlap([0.5, 2.0], [0.0, 0.6], Same)],
        ),
        case(
            "polyline_opposite_overlap_across_vertex",
            poly(&[(3.0, 0.0, 0.0), (2.0, 0.0, 0.0), (0.0, 0.0, 0.0)], false),
            seg(0.5, 0.0, 2.5, 0.0),
            vec![overlap([0.5, 1.75], [0.0, 1.0], Opposite)],
        ),
        case(
            "polyline_bulge_arc_crossing",
            poly(&[(0.0, 0.0, 1.0), (2.0, 0.0, 0.0)], false),
            seg(1.0, -2.0, 1.0, 0.0),
            vec![at(cross(1.0, -1.0), 0.5, 0.5)],
        ),
        case(
            "polylines_meeting_at_shared_vertex",
            poly(&[(0.0, 0.0, 0.0), (1.0, 1.0, 0.0), (2.0, 0.0, 0.0)], false),
            poly(&[(0.0, 3.0, 0.0), (1.0, 1.0, 0.0), (2.0, 3.0, 0.0)], false),
            vec![at(cross(1.0, 1.0), 1.0, 1.0)],
        ),
        case(
            "polylines_touching_with_collinear_arms",
            poly(&[(0.0, 0.0, 0.0), (1.0, 1.0, 0.0), (2.0, 0.0, 0.0)], false),
            poly(&[(0.0, 2.0, 0.0), (1.0, 1.0, 0.0), (2.0, 2.0, 0.0)], false),
            vec![at(touch(1.0, 1.0), 1.0, 1.0)],
        ),
        case(
            "closed_square_tangent_to_circle",
            poly(
                &[
                    (-1.0, -1.0, 0.0),
                    (1.0, -1.0, 0.0),
                    (1.0, 1.0, 0.0),
                    (-1.0, 1.0, 0.0),
                ],
                true,
            ),
            circ(0.0, 0.0, 1.0),
            vec![
                at(touch(0.0, -1.0), 0.5, 3.0 * FRAC_PI_2),
                at(touch(1.0, 0.0), 1.5, 0.0),
                at(touch(0.0, 1.0), 2.5, FRAC_PI_2),
                at(touch(-1.0, 0.0), 3.5, PI),
            ],
        ),
        case(
            "polyline_arc_on_circle",
            poly(&[(1.0, 0.0, (PI / 8.0).tan()), (0.0, 1.0, 0.0)], false),
            circ(0.0, 0.0, 1.0),
            vec![overlap([0.0, 1.0], [0.0, FRAC_PI_2], Same)],
        ),
        case(
            "closed_polylines_crossing",
            poly(
                &[
                    (0.0, 0.0, 0.0),
                    (2.0, 0.0, 0.0),
                    (2.0, 2.0, 0.0),
                    (0.0, 2.0, 0.0),
                ],
                true,
            ),
            poly(
                &[
                    (1.0, 1.0, 0.0),
                    (3.0, 1.0, 0.0),
                    (3.0, 3.0, 0.0),
                    (1.0, 3.0, 0.0),
                ],
                true,
            ),
            vec![at(cross(2.0, 1.0), 1.5, 0.5), at(cross(1.0, 2.0), 2.5, 3.5)],
        ),
        // Nearly straight bulges: a sagitta of at most half the linear tolerance is intersected
        // as the chord, a flat bulge with a huge radius as chords, and neither fails.
        case(
            "polyline_bulge_1e-6_crossing",
            poly(&[(0.0, 0.0, 1e-6), (1.0, 0.0, 0.0)], false),
            seg(0.5, -1.0, 0.5, 1.0),
            vec![at(cross(0.5, -5e-7), 0.5, 0.5)],
        ),
        case(
            "polyline_bulge_1e-12_crossing",
            poly(&[(0.0, 0.0, 1e-12), (1.0, 0.0, 0.0)], false),
            seg(0.5, -1.0, 0.5, 1.0),
            vec![at(cross(0.5, 0.0), 0.5, 0.5)],
        ),
        case(
            "polyline_bulge_1e-12_far_segment",
            poly(&[(0.0, 0.0, 1e-12), (1.0, 0.0, 0.0)], false),
            seg(50.0, 50.0, 51.0, 51.0),
            vec![],
        ),
        case(
            "long_flat_bulge_as_chords",
            poly(&[(0.0, 0.0, 1e-8), (1000.0, 0.0, 0.0)], false),
            seg(500.0, -1.0, 500.0, 1.0),
            vec![at(cross(500.0, -5e-6), 0.5, 0.499_997_5)],
        ),
        // Thin ellipses and tiny polyline segments.
        case(
            "thin_ellipse_crossings_stay_apart",
            ell(0.0, 0.0, 2.0, 0.0, 1e-7),
            seg(1.0, -1.0, 1.0, 1.0),
            vec![cross(1.0, 1.732_050_8e-7), cross(1.0, -1.732_050_8e-7)],
        ),
        case(
            "self_crossing_polyline_with_tiny_tail",
            poly(
                &[
                    (-1.0, -1.0, 0.0),
                    (1.0, 1.0, 0.0),
                    (1.0, -1.0, 0.0),
                    (-1.0, 1.0, 0.0),
                    (-1.0, 1.0 + 4e-7, 0.0),
                ],
                false,
            ),
            seg(0.0, -2.0, 0.0, 2.0),
            vec![at(cross(0.0, 0.0), 0.5, 0.5), at(cross(0.0, 0.0), 2.5, 0.5)],
        ),
        case(
            "crossing_next_to_overlap",
            poly(
                &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 5e-7, 0.0)],
                false,
            ),
            poly(
                &[
                    (-1.0, 0.0, 0.0),
                    (500.0, 0.0, 0.0),
                    (500.4, 1.0, 0.0),
                    (500.8, -1.0, 0.0),
                    (500.8, -1.0 + 5e-7, 0.0),
                ],
                false,
            ),
            vec![
                overlap([0.0, 0.5], [1.0 / 501.0, 1.0], Same),
                at(cross(500.6, 0.0), 0.5006, 2.5),
            ],
        ),
        case(
            "collinear_stretches_with_gap",
            poly(
                &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 2e-7, 0.0)],
                false,
            ),
            poly(
                &[
                    (100.0, 0.0, 0.0),
                    (200.0, 0.0, 0.0),
                    (200.25, 0.5, 0.0),
                    (200.5, 0.0, 0.0),
                    (300.0, 0.0, 0.0),
                    (300.0, 2e-7, 0.0),
                ],
                false,
            ),
            vec![
                overlap([0.1, 0.2], [0.0, 1.0], Same),
                overlap([0.2005, 0.3], [3.0, 4.0], Same),
            ],
        ),
        // Seams, rays, lines and self-intersection.
        case(
            "ellipses_nearly_coincident_at_seam",
            e21(),
            ell(0.0, 1.1e-6, 2.0, 0.0, 0.5),
            vec![cross(2.0, 5.5e-7), cross(-2.0, 5.5e-7)],
        ),
        case(
            "closed_polyline_overlap_across_seam",
            poly(
                &[
                    (1.0, 0.0, 0.0),
                    (2.0, 0.0, 0.0),
                    (2.0, 2.0, 0.0),
                    (0.0, 2.0, 0.0),
                    (0.0, 0.0, 0.0),
                ],
                true,
            ),
            seg(1.5, 0.0, 0.5, 0.0),
            vec![
                overlap([0.0, 0.5], [0.0, 0.5], Opposite),
                overlap([4.5, 5.0], [0.5, 1.0], Opposite),
            ],
        ),
        case(
            "rays_same_direction_overlap",
            ray(0.0, 0.0, 1.0, 0.0),
            ray(3.0, 0.0, 2.0, 0.0),
            vec![overlap([3.0, inf], [0.0, inf], Same)],
        ),
        case(
            "lines_collinear_same",
            line(0.0, 0.0, 1.0, 0.0),
            line(3.0, 0.0, 5.0, 0.0),
            vec![overlap([-inf, inf], [-inf, inf], Same)],
        ),
        case(
            "ray_along_polyline",
            ray(-1.0, 0.0, 1.0, 0.0),
            poly(&[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (2.0, 0.0, 0.0)], false),
            vec![overlap([1.0, 3.0], [0.0, 2.0], Same)],
        ),
        case(
            "self_crossing_polyline_with_itself",
            poly(
                &[
                    (0.0, 0.0, 0.0),
                    (2.0, 2.0, 0.0),
                    (2.0, 0.0, 0.0),
                    (0.0, 2.0, 0.0),
                ],
                false,
            ),
            poly(
                &[
                    (0.0, 0.0, 0.0),
                    (2.0, 2.0, 0.0),
                    (2.0, 0.0, 0.0),
                    (0.0, 2.0, 0.0),
                ],
                false,
            ),
            vec![overlap([0.0, 3.0], [0.0, 3.0], Same)],
        ),
        case(
            "retracing_polyline_with_itself",
            poly(&[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 0.0, 0.0)], false),
            poly(&[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 0.0, 0.0)], false),
            vec![
                overlap([0.0, 2.0], [0.0, 2.0], Same),
                overlap([0.0, 2.0], [0.0, 2.0], Opposite),
            ],
        ),
        Case {
            name: "extreme_coordinates_circles",
            a: circ(1e12, 0.0, 1.0),
            b: circ(1e12 + 1.0, 0.0, 1.0),
            expect: Expect::Error("geom.precision.insufficient"),
        },
    ]
}

fn close(x: f64, y: f64) -> bool {
    x == y || (x - y).abs() <= MATCH
}

fn matches(found: &Intersection, item: &Item) -> bool {
    match (*found, *item) {
        (
            Intersection::Point {
                point,
                a,
                b,
                contact,
            },
            Item::Point(x, y, c, params),
        ) => {
            close(point.x, x)
                && close(point.y, y)
                && contact == c
                && params.is_none_or(|(pa, pb)| close(a, pa) && close(b, pb))
        }
        (Intersection::Overlap { a, b, orientation }, Item::Overlap(ea, eb, eo)) => {
            orientation == eo
                && close(a[0], ea[0])
                && close(a[1], ea[1])
                && close(b[0], eb[0])
                && close(b[1], eb[1])
        }
        _ => false,
    }
}

fn swap(item: &Item) -> Item {
    match *item {
        Item::Point(x, y, c, params) => Item::Point(x, y, c, params.map(|(a, b)| (b, a))),
        Item::Overlap(a, b, o) => Item::Overlap(b, a, o),
    }
}

/// Checks one argument order; returns a description of the first mismatch.
fn check(name: &str, a: &Curve2, b: &Curve2, expect: &Expect, swapped: bool) -> Option<String> {
    let order = if swapped { "b, a" } else { "a, b" };
    let result = intersect(a, b, &TOL);
    let items: Vec<Item> = match (expect, &result) {
        (Expect::Error(code), Err(error)) if error.code() == *code => return None,
        (Expect::Items(items), Ok(_)) => items
            .iter()
            .map(|i| if swapped { swap(i) } else { *i })
            .collect(),
        _ => return Some(format!("{name} ({order}): unexpected {result:?}")),
    };
    let Ok(found) = result else { return None };
    let sorted = found.windows(2).all(|w| {
        let start = |i: &Intersection| match i {
            Intersection::Point { a, .. } => *a,
            Intersection::Overlap { a, .. } => a[0],
        };
        start(&w[0]) <= start(&w[1])
    });
    let mut unused = found.clone();
    for item in &items {
        match unused.iter().position(|f| matches(f, item)) {
            Some(k) => {
                unused.remove(k);
            }
            None => return Some(format!("{name} ({order}): missing {item:?} in {found:?}")),
        }
    }
    if !unused.is_empty() || !sorted {
        return Some(format!(
            "{name} ({order}): unexpected or unsorted {found:?}"
        ));
    }
    for hit in &found {
        if let Intersection::Point {
            point,
            a: ta,
            b: tb,
            ..
        } = hit
            && ((point - a.point_at(*ta)).norm() > TOL.linear
                || (point - b.point_at(*tb)).norm() > TOL.linear)
        {
            return Some(format!("{name} ({order}): {hit:?} is not on both curves"));
        }
    }
    None
}

#[test]
fn corpus_has_enough_distinct_cases() {
    let cases = corpus();
    assert!(cases.len() >= 60, "{} cases", cases.len());
    let mut names: Vec<_> = cases.iter().map(|c| c.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), cases.len(), "case names must be unique");
}

#[test]
fn every_case_gives_its_expected_outcome_in_both_orders() {
    let failures: Vec<String> = corpus()
        .iter()
        .flat_map(|c| {
            [
                check(c.name, &c.a, &c.b, &c.expect, false),
                check(c.name, &c.b, &c.a, &c.expect, true),
            ]
        })
        .flatten()
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
