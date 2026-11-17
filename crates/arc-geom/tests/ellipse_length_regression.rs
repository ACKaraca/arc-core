//! Regression: the lengths of two parts of a flat ellipse add up to the length of the whole
//! within the linear tolerance (found by a property test on one platform's seed).

use arc_base::Tolerance;
use arc_base::math::{Point2, Vector2};
use arc_geom::{Curve2, CurveEval, Ellipse2};

#[test]
fn parts_of_a_flat_ellipse_add_up_to_its_length() {
    let ellipse = Ellipse2::new(
        Point2::new(1.0, 1.0),
        Vector2::new(5928.035710254783, 1.0),
        8.236551410080941e-6,
    )
    .unwrap();
    let curve = Curve2::Ellipse(ellipse);
    let tolerance = Tolerance::DEFAULT;
    let (start, end) = curve.parameter_range();
    let split = start + 0.8427410035859059 * (end - start);
    let total = curve.length(&tolerance).unwrap();
    let part = curve.length_between(start, split, &tolerance).unwrap();
    let rest = curve.length_between(split, end, &tolerance).unwrap();
    assert!((part + rest - total).abs() <= tolerance.linear);
}
