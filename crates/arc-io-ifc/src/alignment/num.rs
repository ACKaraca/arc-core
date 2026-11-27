//! Numerical integration of a heading function, for the spiral parent curves.

/// Nodes of the 8-point Gauss-Legendre rule on [-1, 1].
const NODES: [f64; 8] = [
    -0.960_289_856_497_536_3,
    -0.796_666_477_413_626_7,
    -0.525_532_409_916_329,
    -0.183_434_642_495_649_8,
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_3,
];
/// Weights of the 8-point Gauss-Legendre rule.
const WEIGHTS: [f64; 8] = [
    0.101_228_536_290_376_26,
    0.222_381_034_453_374_47,
    0.313_706_645_877_887_3,
    0.362_683_783_378_362,
    0.362_683_783_378_362,
    0.313_706_645_877_887_3,
    0.222_381_034_453_374_47,
    0.101_228_536_290_376_26,
];

/// The most panels one integral uses; it bounds the cost of an evaluation.
const MAX_PANELS: usize = 2048;
/// The largest change of heading, in radians, inside one panel.
const PANEL_TURN: f64 = 0.2;

/// The integral of (cos θ(s), sin θ(s)) from `a` to `b`, where `curvature_bound` bounds |θ'| on
/// the interval. The panel count follows the turn of the heading, so the error stays near the
/// rounding error of the sum.
pub(crate) fn heading_integral(
    theta: &dyn Fn(f64) -> f64,
    curvature_bound: f64,
    a: f64,
    b: f64,
) -> [f64; 2] {
    let span = b - a;
    if span == 0.0 {
        return [0.0, 0.0];
    }
    let wanted = (span.abs() * curvature_bound / PANEL_TURN).ceil();
    // The count is clamped before the conversion, so it is a small non-negative integer.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 2..=MAX_PANELS first"
    )]
    let panels = if wanted.is_finite() {
        wanted.clamp(2.0, MAX_PANELS as f64) as usize
    } else {
        MAX_PANELS
    };
    let h = span / panels as f64;
    let (mut x, mut y) = (0.0, 0.0);
    for i in 0..panels {
        let mid = a + h * (i as f64 + 0.5);
        for (node, weight) in NODES.iter().zip(WEIGHTS) {
            let t = theta(mid + 0.5 * h * node);
            x += weight * t.cos();
            y += weight * t.sin();
        }
    }
    [x * 0.5 * h, y * 0.5 * h]
}

/// The largest |f| over 33 samples of `[a, b]`, with a margin for the points between them.
pub(crate) fn sampled_bound(f: &dyn Fn(f64) -> f64, a: f64, b: f64) -> f64 {
    let mut max = 0.0f64;
    for i in 0..=32 {
        let s = a + (b - a) * f64::from(i) / 32.0;
        max = max.max(f(s).abs());
    }
    max * 1.25
}
