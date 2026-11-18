//! Small geometric helpers shared by the mapping code.

use std::f64::consts::TAU;

use arc_base::math::Point2;

/// The point `(x, y)` scaled by `f`.
#[must_use]
pub fn pt([x, y]: [f64; 2], f: f64) -> Point2 {
    Point2::new(x * f, y * f)
}

/// Degrees to radians.
#[must_use]
pub fn rad(degrees: f64) -> f64 {
    degrees.to_radians()
}

/// The counterclockwise sweep in radians from `start` to `end` (both radians), in `(0, 2π]`,
/// and whether it is a full turn: the two angles name the same direction within `angular`.
#[must_use]
pub fn sweep(start: f64, end: f64, angular: f64) -> (f64, bool) {
    let sweep = (end - start).rem_euclid(TAU);
    if sweep <= angular || TAU - sweep <= angular {
        (TAU, true)
    } else {
        (sweep, false)
    }
}

/// The bulge of an arc segment with the signed sweep `sweep` (positive counterclockwise).
#[must_use]
pub fn bulge_of(sweep: f64) -> f64 {
    (sweep / 4.0).tan()
}
