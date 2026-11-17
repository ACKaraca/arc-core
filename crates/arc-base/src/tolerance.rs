//! The tolerance context (CORE-BASE-012).

use serde::{Deserialize, Serialize};

/// The tolerances geometric comparisons use. Functions that compare geometry take a `Tolerance`
/// and never use literals.
///
/// A document stores its tolerance; the defaults are those of [`Tolerance::DEFAULT`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tolerance {
    /// Two points closer than this distance, in meters, are coincident.
    pub linear: f64,
    /// Two directions closer than this angle, in radians, are parallel.
    pub angular: f64,
    /// Maximum distance, in meters, between a curve and its polyline when the caller supplies no
    /// view-dependent or export-specific value.
    pub tessellation_chord: f64,
    /// Maximum turning angle per segment, in radians, when the caller supplies no value.
    pub tessellation_angle: f64,
}

impl Tolerance {
    /// The documented defaults: 1 µm, 1 nrad, a 1 mm chord and a 15° turning angle.
    pub const DEFAULT: Self = Self {
        linear: 1e-6,
        angular: 1e-9,
        tessellation_chord: 1e-3,
        tessellation_angle: std::f64::consts::PI / 12.0,
    };

    /// The parametric tolerance on a curve or surface whose parameterization moves `speed` meters
    /// per unit of parameter: the parameter distance that corresponds to [`Self::linear`].
    ///
    /// A speed that is zero, negative or not finite yields the linear tolerance unchanged.
    #[must_use]
    pub fn parametric(&self, speed: f64) -> f64 {
        if speed.is_finite() && speed > 0.0 {
            self.linear / speed
        } else {
            self.linear
        }
    }
}

impl Default for Tolerance {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values() {
        let tolerance = Tolerance::default();
        assert_eq!(tolerance.linear, 1e-6);
        assert_eq!(tolerance.angular, 1e-9);
        assert_eq!(tolerance.tessellation_chord, 1e-3);
        assert_eq!(tolerance.tessellation_angle.to_degrees().round(), 15.0);
    }

    #[test]
    fn parametric_tolerance_scales_with_speed() {
        let tolerance = Tolerance::default();
        assert_eq!(tolerance.parametric(2.0), 5e-7);
        for degenerate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(tolerance.parametric(degenerate), tolerance.linear);
        }
    }
}
