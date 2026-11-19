//! The indexed color table of the DXF format (the table lives in `arc_base::color`) and its
//! mapping to drafting colors.

use arc_base::Color;
use arc_doc::{ColorSource, ColorValue};

/// The `[r, g, b]` bytes of the indexed color `index`, for 1 to 255; 0 gives black.
#[must_use]
pub fn aci_rgb(index: u8) -> [u8; 3] {
    arc_base::color::indexed_rgb(index)
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
