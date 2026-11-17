//! The linear color model (CORE-BASE-019).

use serde::{Deserialize, Serialize};

/// A color as linear RGBA with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Color {
    /// Linear red.
    pub r: f32,
    /// Linear green.
    pub g: f32,
    /// Linear blue.
    pub b: f32,
    /// Opacity from 0 (transparent) to 1 (opaque).
    pub a: f32,
}

impl Color {
    /// Opaque black.
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    /// Opaque white.
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);

    /// Creates a color from linear components.
    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Converts 8-bit sRGB with 8-bit alpha to linear color.
    #[must_use]
    pub fn from_srgb8([r, g, b, a]: [u8; 4]) -> Self {
        let decode = |level: u8| {
            let encoded = f32::from(level) / 255.0;
            if encoded <= 0.040_45 {
                encoded / 12.92
            } else {
                ((encoded + 0.055) / 1.055).powf(2.4)
            }
        };
        Self::new(decode(r), decode(g), decode(b), f32::from(a) / 255.0)
    }

    /// Converts to 8-bit sRGB with 8-bit alpha, clamping to the displayable range and rounding
    /// to the nearest level. Every color made by [`Color::from_srgb8`] converts back exactly.
    #[must_use]
    pub fn to_srgb8(self) -> [u8; 4] {
        let encode = |linear: f32| {
            let linear = linear.clamp(0.0, 1.0);
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            level(encoded)
        };
        [
            encode(self.r),
            encode(self.g),
            encode(self.b),
            level(self.a),
        ]
    }
}

/// The nearest 8-bit level of a value in 0..=1; values outside, and NaN, are clamped.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast, and a float-to-integer cast saturates"
)]
fn level(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_256_levels_round_trip_exactly_in_every_channel() {
        for value in 0..=u8::MAX {
            let srgb = [
                value,
                value.wrapping_add(85),
                value.wrapping_add(170),
                value,
            ];
            assert_eq!(Color::from_srgb8(srgb).to_srgb8(), srgb);
        }
    }

    #[test]
    fn end_points_and_middle_gray_have_the_standard_values() {
        assert_eq!(Color::from_srgb8([0, 0, 0, 255]), Color::BLACK);
        assert_eq!(Color::from_srgb8([255, 255, 255, 255]), Color::WHITE);
        let gray = Color::from_srgb8([128, 128, 128, 0]);
        assert!((gray.r - 0.215_86).abs() < 1e-4, "{}", gray.r);
        assert_eq!(gray.a, 0.0);
    }

    #[test]
    fn out_of_range_components_are_clamped() {
        let wild = Color::new(-1.0, 2.0, f32::NAN, 7.0);
        assert_eq!(wild.to_srgb8(), [0, 255, 0, 255]);
    }
}
