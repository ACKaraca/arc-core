//! The 256-entry indexed color table of the DXF format and its mapping to drafting colors.
//!
//! Index 0 and 256 are the by-block and by-layer markers, 1 to 9 are fixed colors, 10 to 249
//! are 24 hues in steps of 15 degrees with ten brightness and saturation variants each, and
//! 250 to 255 are grays.

use arc_base::Color;
use arc_doc::{ColorSource, ColorValue};

/// A float in 0..=255 truncated to a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast"
)]
fn byte(value: f64) -> u8 {
    value.clamp(0.0, 255.0) as u8
}

/// The `[r, g, b]` bytes of the indexed color `index`, for 1 to 255; 0 gives black.
#[must_use]
pub fn aci_rgb(index: u8) -> [u8; 3] {
    match index {
        0 => [0, 0, 0],
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        7 => [255, 255, 255],
        8 => [65, 65, 65],
        9 => [128, 128, 128],
        250 => [51, 51, 51],
        251 => [91, 91, 91],
        252 => [132, 132, 132],
        253 => [173, 173, 173],
        254 => [214, 214, 214],
        255 => [255, 255, 255],
        _ => hue_entry(index - 10),
    }
}

/// Entries 10 to 249, counted from 0.
fn hue_entry(offset: u8) -> [u8; 3] {
    const LEVELS: [f64; 5] = [255.0, 204.0, 153.0, 127.0, 76.0];
    let hue = offset / 10;
    let variant = offset % 10;
    let value = LEVELS
        .get(usize::from(variant / 2))
        .copied()
        .unwrap_or(255.0);
    let low = if variant % 2 == 1 { value * 0.5 } else { 0.0 };
    let frac = f64::from(hue % 4) * 0.25;
    let up = low + (value - low) * frac;
    let down = low + (value - low) * (1.0 - frac);
    let [r, g, b] = match hue / 4 {
        0 => [value, up, low],
        1 => [down, value, low],
        2 => [low, value, up],
        3 => [low, down, value],
        4 => [up, low, value],
        _ => [value, low, down],
    };
    [byte(r), byte(g), byte(b)]
}

/// The drafting color of an indexed color, keeping the index as its source.
#[must_use]
pub fn from_index(index: u8) -> ColorValue {
    let [r, g, b] = aci_rgb(index);
    ColorValue {
        rgba: Color::from_srgb8([r, g, b, 255]),
        source: Some(ColorSource::Indexed(index)),
    }
}

/// The drafting color of a true color `0x00RRGGBB`.
#[must_use]
pub fn from_true(value: i32) -> ColorValue {
    let bits = value.to_be_bytes();
    ColorValue {
        rgba: Color::from_srgb8([bits[1], bits[2], bits[3], 255]),
        source: None,
    }
}

/// The true color `0x00RRGGBB` of a drafting color.
#[must_use]
pub fn to_true(color: &ColorValue) -> i32 {
    let [r, g, b, _] = color.rgba.to_srgb8();
    i32::from_be_bytes([0, r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_entries_match() {
        let expected = [
            (10, [255, 0, 0]),
            (11, [255, 127, 127]),
            (12, [204, 0, 0]),
            (13, [204, 102, 102]),
            (16, [127, 0, 0]),
            (17, [127, 63, 63]),
            (18, [76, 0, 0]),
            (19, [76, 38, 38]),
            (20, [255, 63, 0]),
            (21, [255, 159, 127]),
            (30, [255, 127, 0]),
            (40, [255, 191, 0]),
            (50, [255, 255, 0]),
            (90, [0, 255, 0]),
            (130, [0, 255, 255]),
            (170, [0, 0, 255]),
            (210, [255, 0, 255]),
            (240, [255, 0, 63]),
        ];
        for (index, rgb) in expected {
            assert_eq!(aci_rgb(index), rgb, "index {index}");
        }
        assert_eq!(aci_rgb(8), [65, 65, 65]);
        assert_eq!(aci_rgb(255), [255, 255, 255]);
    }

    #[test]
    fn true_colors_round_trip_exactly() {
        for value in [0x00FF_8040, 0x0000_0000, 0x00FF_FFFF, 0x0012_3456] {
            assert_eq!(to_true(&from_true(value)), value);
        }
        for index in 1..=255u8 {
            let rgb = aci_rgb(index);
            let back = to_true(&from_index(index));
            assert_eq!(back, i32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]));
        }
    }
}
