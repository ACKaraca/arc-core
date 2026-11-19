//! The linear color model (CORE-BASE-019), the indexed palette (CORE-BASE-020) and color sources
//! (CORE-BASE-021).

use serde::{Deserialize, Serialize};

use crate::id::ObjectId;

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

/// The 256-entry indexed color table of the DXF format, as sRGB bytes. Index 0 is black and the
/// by-block marker, 1 to 9 are fixed colors, 10 to 249 are 24 hues in steps of 15 degrees with
/// ten brightness and saturation variants each, and 250 to 255 are grays.
pub const INDEXED_PALETTE: [[u8; 3]; 256] = [
    [0, 0, 0],       // 0
    [255, 0, 0],     // 1
    [255, 255, 0],   // 2
    [0, 255, 0],     // 3
    [0, 255, 255],   // 4
    [0, 0, 255],     // 5
    [255, 0, 255],   // 6
    [255, 255, 255], // 7
    [65, 65, 65],    // 8
    [128, 128, 128], // 9
    [255, 0, 0],     // 10
    [255, 127, 127], // 11
    [204, 0, 0],     // 12
    [204, 102, 102], // 13
    [153, 0, 0],     // 14
    [153, 76, 76],   // 15
    [127, 0, 0],     // 16
    [127, 63, 63],   // 17
    [76, 0, 0],      // 18
    [76, 38, 38],    // 19
    [255, 63, 0],    // 20
    [255, 159, 127], // 21
    [204, 51, 0],    // 22
    [204, 127, 102], // 23
    [153, 38, 0],    // 24
    [153, 95, 76],   // 25
    [127, 31, 0],    // 26
    [127, 79, 63],   // 27
    [76, 19, 0],     // 28
    [76, 47, 38],    // 29
    [255, 127, 0],   // 30
    [255, 191, 127], // 31
    [204, 102, 0],   // 32
    [204, 153, 102], // 33
    [153, 76, 0],    // 34
    [153, 114, 76],  // 35
    [127, 63, 0],    // 36
    [127, 95, 63],   // 37
    [76, 38, 0],     // 38
    [76, 57, 38],    // 39
    [255, 191, 0],   // 40
    [255, 223, 127], // 41
    [204, 153, 0],   // 42
    [204, 178, 102], // 43
    [153, 114, 0],   // 44
    [153, 133, 76],  // 45
    [127, 95, 0],    // 46
    [127, 111, 63],  // 47
    [76, 57, 0],     // 48
    [76, 66, 38],    // 49
    [255, 255, 0],   // 50
    [255, 255, 127], // 51
    [204, 204, 0],   // 52
    [204, 204, 102], // 53
    [153, 153, 0],   // 54
    [153, 153, 76],  // 55
    [127, 127, 0],   // 56
    [127, 127, 63],  // 57
    [76, 76, 0],     // 58
    [76, 76, 38],    // 59
    [191, 255, 0],   // 60
    [223, 255, 127], // 61
    [153, 204, 0],   // 62
    [178, 204, 102], // 63
    [114, 153, 0],   // 64
    [133, 153, 76],  // 65
    [95, 127, 0],    // 66
    [111, 127, 63],  // 67
    [57, 76, 0],     // 68
    [66, 76, 38],    // 69
    [127, 255, 0],   // 70
    [191, 255, 127], // 71
    [102, 204, 0],   // 72
    [153, 204, 102], // 73
    [76, 153, 0],    // 74
    [114, 153, 76],  // 75
    [63, 127, 0],    // 76
    [95, 127, 63],   // 77
    [38, 76, 0],     // 78
    [57, 76, 38],    // 79
    [63, 255, 0],    // 80
    [159, 255, 127], // 81
    [51, 204, 0],    // 82
    [127, 204, 102], // 83
    [38, 153, 0],    // 84
    [95, 153, 76],   // 85
    [31, 127, 0],    // 86
    [79, 127, 63],   // 87
    [19, 76, 0],     // 88
    [47, 76, 38],    // 89
    [0, 255, 0],     // 90
    [127, 255, 127], // 91
    [0, 204, 0],     // 92
    [102, 204, 102], // 93
    [0, 153, 0],     // 94
    [76, 153, 76],   // 95
    [0, 127, 0],     // 96
    [63, 127, 63],   // 97
    [0, 76, 0],      // 98
    [38, 76, 38],    // 99
    [0, 255, 63],    // 100
    [127, 255, 159], // 101
    [0, 204, 51],    // 102
    [102, 204, 127], // 103
    [0, 153, 38],    // 104
    [76, 153, 95],   // 105
    [0, 127, 31],    // 106
    [63, 127, 79],   // 107
    [0, 76, 19],     // 108
    [38, 76, 47],    // 109
    [0, 255, 127],   // 110
    [127, 255, 191], // 111
    [0, 204, 102],   // 112
    [102, 204, 153], // 113
    [0, 153, 76],    // 114
    [76, 153, 114],  // 115
    [0, 127, 63],    // 116
    [63, 127, 95],   // 117
    [0, 76, 38],     // 118
    [38, 76, 57],    // 119
    [0, 255, 191],   // 120
    [127, 255, 223], // 121
    [0, 204, 153],   // 122
    [102, 204, 178], // 123
    [0, 153, 114],   // 124
    [76, 153, 133],  // 125
    [0, 127, 95],    // 126
    [63, 127, 111],  // 127
    [0, 76, 57],     // 128
    [38, 76, 66],    // 129
    [0, 255, 255],   // 130
    [127, 255, 255], // 131
    [0, 204, 204],   // 132
    [102, 204, 204], // 133
    [0, 153, 153],   // 134
    [76, 153, 153],  // 135
    [0, 127, 127],   // 136
    [63, 127, 127],  // 137
    [0, 76, 76],     // 138
    [38, 76, 76],    // 139
    [0, 191, 255],   // 140
    [127, 223, 255], // 141
    [0, 153, 204],   // 142
    [102, 178, 204], // 143
    [0, 114, 153],   // 144
    [76, 133, 153],  // 145
    [0, 95, 127],    // 146
    [63, 111, 127],  // 147
    [0, 57, 76],     // 148
    [38, 66, 76],    // 149
    [0, 127, 255],   // 150
    [127, 191, 255], // 151
    [0, 102, 204],   // 152
    [102, 153, 204], // 153
    [0, 76, 153],    // 154
    [76, 114, 153],  // 155
    [0, 63, 127],    // 156
    [63, 95, 127],   // 157
    [0, 38, 76],     // 158
    [38, 57, 76],    // 159
    [0, 63, 255],    // 160
    [127, 159, 255], // 161
    [0, 51, 204],    // 162
    [102, 127, 204], // 163
    [0, 38, 153],    // 164
    [76, 95, 153],   // 165
    [0, 31, 127],    // 166
    [63, 79, 127],   // 167
    [0, 19, 76],     // 168
    [38, 47, 76],    // 169
    [0, 0, 255],     // 170
    [127, 127, 255], // 171
    [0, 0, 204],     // 172
    [102, 102, 204], // 173
    [0, 0, 153],     // 174
    [76, 76, 153],   // 175
    [0, 0, 127],     // 176
    [63, 63, 127],   // 177
    [0, 0, 76],      // 178
    [38, 38, 76],    // 179
    [63, 0, 255],    // 180
    [159, 127, 255], // 181
    [51, 0, 204],    // 182
    [127, 102, 204], // 183
    [38, 0, 153],    // 184
    [95, 76, 153],   // 185
    [31, 0, 127],    // 186
    [79, 63, 127],   // 187
    [19, 0, 76],     // 188
    [47, 38, 76],    // 189
    [127, 0, 255],   // 190
    [191, 127, 255], // 191
    [102, 0, 204],   // 192
    [153, 102, 204], // 193
    [76, 0, 153],    // 194
    [114, 76, 153],  // 195
    [63, 0, 127],    // 196
    [95, 63, 127],   // 197
    [38, 0, 76],     // 198
    [57, 38, 76],    // 199
    [191, 0, 255],   // 200
    [223, 127, 255], // 201
    [153, 0, 204],   // 202
    [178, 102, 204], // 203
    [114, 0, 153],   // 204
    [133, 76, 153],  // 205
    [95, 0, 127],    // 206
    [111, 63, 127],  // 207
    [57, 0, 76],     // 208
    [66, 38, 76],    // 209
    [255, 0, 255],   // 210
    [255, 127, 255], // 211
    [204, 0, 204],   // 212
    [204, 102, 204], // 213
    [153, 0, 153],   // 214
    [153, 76, 153],  // 215
    [127, 0, 127],   // 216
    [127, 63, 127],  // 217
    [76, 0, 76],     // 218
    [76, 38, 76],    // 219
    [255, 0, 191],   // 220
    [255, 127, 223], // 221
    [204, 0, 153],   // 222
    [204, 102, 178], // 223
    [153, 0, 114],   // 224
    [153, 76, 133],  // 225
    [127, 0, 95],    // 226
    [127, 63, 111],  // 227
    [76, 0, 57],     // 228
    [76, 38, 66],    // 229
    [255, 0, 127],   // 230
    [255, 127, 191], // 231
    [204, 0, 102],   // 232
    [204, 102, 153], // 233
    [153, 0, 76],    // 234
    [153, 76, 114],  // 235
    [127, 0, 63],    // 236
    [127, 63, 95],   // 237
    [76, 0, 38],     // 238
    [76, 38, 57],    // 239
    [255, 0, 63],    // 240
    [255, 127, 159], // 241
    [204, 0, 51],    // 242
    [204, 102, 127], // 243
    [153, 0, 38],    // 244
    [153, 76, 95],   // 245
    [127, 0, 31],    // 246
    [127, 63, 79],   // 247
    [76, 0, 19],     // 248
    [76, 38, 47],    // 249
    [51, 51, 51],    // 250
    [91, 91, 91],    // 251
    [132, 132, 132], // 252
    [173, 173, 173], // 253
    [214, 214, 214], // 254
    [255, 255, 255], // 255
];

/// The sRGB bytes of an indexed palette entry. Every `u8` is an index of the table.
#[must_use]
pub fn indexed_rgb(index: u8) -> [u8; 3] {
    INDEXED_PALETTE
        .get(usize::from(index))
        .copied()
        .unwrap_or([0, 0, 0])
}

/// The index of the palette entry nearest to an sRGB color, by Euclidean distance in OKLab.
///
/// Every entry is a candidate, index 0 included. When several entries are equally near, the
/// lowest index wins, so an entry maps to itself or to an earlier entry with the same bytes.
#[must_use]
pub fn nearest_index(rgb: [u8; 3]) -> u8 {
    let target = oklab(rgb);
    let mut best = 0_u8;
    let mut best_distance = f64::INFINITY;
    for (index, entry) in INDEXED_PALETTE.iter().enumerate() {
        let lab = oklab(*entry);
        let distance = (lab[0] - target[0]).powi(2)
            + (lab[1] - target[1]).powi(2)
            + (lab[2] - target[2]).powi(2);
        let Ok(index) = u8::try_from(index) else {
            continue;
        };
        if distance < best_distance {
            best = index;
            best_distance = distance;
        }
    }
    best
}

/// The OKLab coordinates of an sRGB color, from the published matrices of the OKLab definition.
fn oklab(rgb: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(srgb_to_linear);
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
}

/// The linear value of one sRGB byte, with the transfer function of IEC 61966-2-1.
fn srgb_to_linear(level: u8) -> f64 {
    let encoded = f64::from(level) / 255.0;
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Where a color comes from. The model that consumes a color resolves the source; this type only
/// records it. Its variants mirror the DXF color rules and the document color records.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSource {
    /// A color given directly.
    Explicit(Color),
    /// An entry of the indexed palette, from 0 to 255.
    Indexed(u8),
    /// A color by name from the color table of a document.
    Named(String),
    /// The color of the layer the object is on.
    ByLayer,
    /// The color of the block reference that inserts the object.
    ByBlock,
    /// The color of a material object.
    Material(ObjectId),
}

impl ColorSource {
    /// The color the source states itself: an explicit color or an indexed palette entry. Named,
    /// layer, block and material colors need the consuming model and give `None`.
    #[must_use]
    pub fn direct_color(&self) -> Option<Color> {
        match self {
            Self::Explicit(color) => Some(*color),
            Self::Indexed(index) => {
                let [r, g, b] = indexed_rgb(*index);
                Some(Color::from_srgb8([r, g, b, 255]))
            }
            Self::Named(_) | Self::ByLayer | Self::ByBlock | Self::Material(_) => None,
        }
    }
}

impl From<Color> for ColorSource {
    fn from(color: Color) -> Self {
        Self::Explicit(color)
    }
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

    use crate::id::Identifier;

    fn table_entry(index: usize) -> [u8; 3] {
        INDEXED_PALETTE.get(index).copied().unwrap_or([0, 0, 0])
    }

    #[test]
    fn every_palette_entry_maps_to_itself_or_an_earlier_identical_entry() {
        for index in 0..INDEXED_PALETTE.len() {
            let rgb = table_entry(index);
            let mapped = usize::from(nearest_index(rgb));
            assert!(
                mapped == index || (mapped < index && table_entry(mapped) == rgb),
                "index {index} maps to {mapped}"
            );
        }
    }

    #[test]
    fn known_colors_map_to_their_palette_entries() {
        let cases = [
            ([0, 0, 0], 0),
            ([255, 0, 0], 1),
            ([255, 255, 0], 2),
            ([0, 255, 0], 3),
            ([0, 255, 255], 4),
            ([0, 0, 255], 5),
            ([255, 0, 255], 6),
            ([255, 255, 255], 7),
            ([65, 65, 65], 8),
            ([128, 128, 128], 9),
            ([255, 127, 127], 11),
            ([250, 4, 2], 1),
        ];
        for (rgb, index) in cases {
            assert_eq!(nearest_index(rgb), index, "{rgb:?}");
        }
    }

    #[test]
    fn indexed_rgb_reads_the_table_and_direct_colors_resolve() {
        assert_eq!(indexed_rgb(1), [255, 0, 0]);
        assert_eq!(indexed_rgb(255), [255, 255, 255]);
        let red = ColorSource::Indexed(1).direct_color();
        assert_eq!(red, Some(Color::from_srgb8([255, 0, 0, 255])));
        assert_eq!(ColorSource::ByLayer.direct_color(), None);
        let given = Color::new(0.5, 0.25, 0.125, 1.0);
        assert_eq!(ColorSource::from(given).direct_color(), Some(given));
    }

    #[test]
    fn color_sources_serialize_with_snake_case_tags() {
        let sources = [
            ColorSource::Explicit(Color::BLACK),
            ColorSource::Indexed(3),
            ColorSource::Named("Blue".to_owned()),
            ColorSource::ByLayer,
            ColorSource::ByBlock,
            ColorSource::Material(ObjectId::from_u128(1)),
        ];
        for source in sources {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(serde_json::from_str::<ColorSource>(&json).unwrap(), source);
        }
        assert_eq!(
            serde_json::to_string(&ColorSource::ByLayer).unwrap(),
            "\"by_layer\""
        );
        assert_eq!(
            serde_json::to_string(&ColorSource::Indexed(3)).unwrap(),
            "{\"indexed\":3}"
        );
    }
}
