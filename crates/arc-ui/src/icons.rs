//! Vector icons (CORE-UI-023).
//!
//! Icons are single-color SVG sources drawn by the project. [`IconAtlas::build`] rasterizes
//! every icon with `resvg` into one square cell of an atlas image at the current pixel size and
//! keeps only coverage: each texel is white with the icon's alpha. Drawing multiplies the texel
//! by a theme color token, so one atlas serves every theme. Sources, counts and sizes are
//! bounded, and SVG references to external files or embedded images are never resolved.

use std::collections::BTreeMap;

use arc_base::{Error, Result};
use egui::{Color32, ColorImage, Rect, TextureHandle, TextureOptions, Vec2, load::SizedTexture};
use resvg::{tiny_skia, usvg};

use crate::error::codes;

/// The largest SVG source accepted, in bytes.
pub const MAX_SVG_BYTES: usize = 16 * 1024;
/// The most icons one atlas holds.
pub const MAX_ICONS: usize = 256;
/// The largest icon edge, in pixels.
pub const MAX_EDGE_PX: u32 = 128;
/// The longest icon name, in bytes.
pub const MAX_NAME_BYTES: usize = 64;

/// The starter set shipped with this crate: name and SVG source.
pub const STARTER: &[(&str, &str)] = &[
    ("check", include_str!("../icons/check.svg")),
    ("chevron-right", include_str!("../icons/chevron-right.svg")),
    ("close", include_str!("../icons/close.svg")),
    ("error", include_str!("../icons/error.svg")),
    ("info", include_str!("../icons/info.svg")),
    ("plus", include_str!("../icons/plus.svg")),
    ("search", include_str!("../icons/search.svg")),
    ("warning", include_str!("../icons/warning.svg")),
];

/// One pixel of padding around each cell keeps linear filtering from bleeding between icons.
const PAD: u32 = 1;

/// Icons rasterized into one coverage image.
#[derive(Debug, Clone)]
pub struct IconAtlas {
    image: ColorImage,
    edge: u32,
    uv: BTreeMap<String, Rect>,
}

fn invalid(name: &str, reason: &'static str) -> Error {
    Error::new(codes::ICON_INVALID)
        .with_arg("name", name)
        .with_detail("reason", reason)
}

fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn options() -> usvg::Options<'static> {
    usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    }
}

/// Rasterizes `svg` into an `edge` × `edge` coverage mask, scaled to fit and centered.
fn rasterize(name: &str, svg: &str, edge: u32, options: &usvg::Options<'_>) -> Result<Vec<u8>> {
    if svg.len() > MAX_SVG_BYTES {
        return Err(Error::new(codes::ICON_TOO_LARGE)
            .with_arg("name", name)
            .with_detail("bytes", i64::try_from(svg.len()).unwrap_or(i64::MAX)));
    }
    let tree =
        usvg::Tree::from_str(svg, options).map_err(|_| invalid(name, "svg_does_not_parse"))?;
    let size = tree.size();
    let longest = size.width().max(size.height());
    if !(longest.is_finite() && longest > 0.0) {
        return Err(invalid(name, "empty_size"));
    }
    let mut pixmap = tiny_skia::Pixmap::new(edge, edge).ok_or_else(|| invalid(name, "edge"))?;
    #[expect(clippy::cast_precision_loss, reason = "edge is at most 128")]
    let edge_f = edge as f32;
    let scale = edge_f / longest;
    let dx = (edge_f - size.width() * scale) / 2.0;
    let dy = (edge_f - size.height() * scale) / 2.0;
    let transform = tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(pixmap.pixels().iter().map(|p| p.alpha()).collect())
}

impl IconAtlas {
    /// Rasterizes `icons` (name, SVG source) at `edge` pixels per icon.
    ///
    /// # Errors
    /// `ui.icon.too_many` above [`MAX_ICONS`]; `ui.icon.invalid` for an `edge` outside 1 to
    /// [`MAX_EDGE_PX`], a name that is not 1 to 64 lowercase letters, digits and `-`, or a
    /// source that does not parse or has no size; `ui.icon.too_large` for a source above
    /// [`MAX_SVG_BYTES`]; `ui.icon.duplicate` for a repeated name.
    pub fn build(icons: &[(&str, &str)], edge: u32) -> Result<Self> {
        if icons.len() > MAX_ICONS {
            return Err(Error::new(codes::ICON_TOO_MANY)
                .with_detail("count", i64::try_from(icons.len()).unwrap_or(i64::MAX)));
        }
        if !(1..=MAX_EDGE_PX).contains(&edge) {
            return Err(invalid("", "edge"));
        }
        // At most 16 cells per row and column (MAX_ICONS = 256).
        let count = u32::try_from(icons.len()).unwrap_or(1).max(1);
        let columns = (1..=16).find(|c| c * c >= count).unwrap_or(16);
        let rows = count.div_ceil(columns);
        let cell = edge + 2 * PAD;
        let (width, height) = (columns * cell, rows * cell);
        let mut image = ColorImage::filled([width as usize, height as usize], Color32::TRANSPARENT);
        let options = options();
        let mut uv = BTreeMap::new();
        for (index, (name, svg)) in (0u32..).zip(icons) {
            if !is_name(name) {
                return Err(invalid(name, "name"));
            }
            if uv.contains_key(*name) {
                return Err(Error::new(codes::ICON_DUPLICATE).with_arg("name", *name));
            }
            let mask = rasterize(name, svg, edge, &options)?;
            let (x0, y0) = (
                (index % columns) * cell + PAD,
                (index / columns) * cell + PAD,
            );
            for (i, alpha) in (0u32..).zip(mask) {
                let at = ((y0 + i / edge) * width + x0 + i % edge) as usize;
                if let Some(texel) = image.pixels.get_mut(at) {
                    *texel = Color32::from_white_alpha(alpha);
                }
            }
            #[expect(clippy::cast_precision_loss, reason = "atlas sides are below 2^12")]
            let (w, h, x, y, e) = (
                width as f32,
                height as f32,
                x0 as f32,
                y0 as f32,
                edge as f32,
            );
            uv.insert(
                (*name).to_owned(),
                Rect::from_min_max(
                    egui::pos2(x / w, y / h),
                    egui::pos2((x + e) / w, (y + e) / h),
                ),
            );
        }
        Ok(Self { image, edge, uv })
    }

    /// The pixel edge for icons of `points` logical size at `pixels_per_point`, rounded up and
    /// clamped to 1 to [`MAX_EDGE_PX`].
    #[must_use]
    pub fn edge_for(points: f32, pixels_per_point: f32) -> u32 {
        let px = (points * pixels_per_point).ceil();
        if px.is_finite() && px >= 1.0 {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to the u32 range first"
            )]
            let px = px.min(MAX_EDGE_PX as f32) as u32;
            px
        } else {
            1
        }
    }

    /// The pixel edge of every icon.
    #[must_use]
    pub fn edge(&self) -> u32 {
        self.edge
    }

    /// The coverage image: white texels with the icons' alpha, premultiplied.
    #[must_use]
    pub fn image(&self) -> &ColorImage {
        &self.image
    }

    /// The texture coordinates of icon `name`.
    #[must_use]
    pub fn uv(&self, name: &str) -> Option<Rect> {
        self.uv.get(name).copied()
    }

    /// Uploads the atlas to `ctx` as a texture.
    #[must_use]
    pub fn upload(&self, ctx: &egui::Context) -> Icons {
        Icons {
            texture: ctx.load_texture("arc-ui-icons", self.image.clone(), TextureOptions::LINEAR),
            uv: self.uv.clone(),
        }
    }
}

/// An uploaded atlas, ready to draw.
#[derive(Clone)]
pub struct Icons {
    texture: TextureHandle,
    uv: BTreeMap<String, Rect>,
}

impl std::fmt::Debug for Icons {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Icons")
            .field("names", &self.uv.keys())
            .finish_non_exhaustive()
    }
}

impl Icons {
    /// An image of icon `name` at `size` logical points, recolored with `color` (a theme
    /// token), or `None` for an unknown name.
    #[must_use]
    pub fn image(&self, name: &str, size: Vec2, color: Color32) -> Option<egui::Image<'static>> {
        let uv = self.uv.get(name)?;
        Some(
            egui::Image::from_texture(SizedTexture::new(self.texture.id(), size))
                .uv(*uv)
                .tint(color),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(result: Result<IconAtlas>) -> &'static str {
        result.map(|_| "ok").unwrap_or_else(|e| e.code())
    }

    #[test]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "small texel coordinates"
    )]
    fn starter_set_rasterizes_at_every_scale() {
        for edge in [12, 16, 24, 32, 48, 96, MAX_EDGE_PX] {
            let atlas = IconAtlas::build(STARTER, edge).unwrap();
            for (name, _) in STARTER {
                let uv = atlas.uv(name).unwrap();
                assert!(uv.min.x >= 0.0 && uv.max.y <= 1.0);
                // Each icon covers some texels, all of them white with alpha.
                let [w, h] = atlas.image().size;
                let (x0, y0) = (
                    (uv.min.x * w as f32).round() as usize,
                    (uv.min.y * h as f32).round() as usize,
                );
                let e = edge as usize;
                let covered = (0..e * e)
                    .filter(|i| atlas.image().pixels[(y0 + i / e) * w + x0 + i % e].a() > 0)
                    .count();
                assert!(covered > e, "{name} at {edge}");
            }
            assert!(
                atlas
                    .image()
                    .pixels
                    .iter()
                    .all(|p| p.r() == p.a() && p.g() == p.a() && p.b() == p.a())
            );
        }
    }

    #[test]
    fn edge_follows_scale_and_pixel_density() {
        assert_eq!(IconAtlas::edge_for(16.0, 1.0), 16);
        assert_eq!(IconAtlas::edge_for(16.0 * 3.0, 2.0), 96);
        assert_eq!(IconAtlas::edge_for(16.0 * 3.0, 4.0), MAX_EDGE_PX);
        assert_eq!(IconAtlas::edge_for(f32::NAN, 1.0), 1);
        assert_eq!(IconAtlas::edge_for(-3.0, 1.0), 1);
    }

    #[test]
    fn bounds_and_malformed_sources_are_rejected() {
        let svg = STARTER[0].1;
        assert_eq!(code(IconAtlas::build(&[("a", svg)], 0)), "ui.icon.invalid");
        assert_eq!(
            code(IconAtlas::build(&[("a", svg)], MAX_EDGE_PX + 1)),
            "ui.icon.invalid"
        );
        assert_eq!(
            code(IconAtlas::build(&[("Bad Name", svg)], 16)),
            "ui.icon.invalid"
        );
        assert_eq!(
            code(IconAtlas::build(&[("a", "<svg")], 16)),
            "ui.icon.invalid"
        );
        assert_eq!(
            code(IconAtlas::build(&[("a", "not svg at all")], 16)),
            "ui.icon.invalid"
        );
        assert_eq!(
            code(IconAtlas::build(&[("a", svg), ("a", svg)], 16)),
            "ui.icon.duplicate"
        );
        let big = format!("{}{}", svg, " ".repeat(MAX_SVG_BYTES));
        assert_eq!(
            code(IconAtlas::build(&[("a", &big)], 16)),
            "ui.icon.too_large"
        );
        let many: Vec<(String, &str)> = (0..=MAX_ICONS).map(|i| (format!("i{i}"), svg)).collect();
        let many: Vec<(&str, &str)> = many.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        assert_eq!(code(IconAtlas::build(&many, 16)), "ui.icon.too_many");
        assert_eq!(code(IconAtlas::build(&many[..MAX_ICONS], 16)), "ok");
    }

    #[test]
    fn external_references_are_not_resolved() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 16 16"><image width="16" height="16" xlink:href="/etc/passwd"/></svg>"#;
        let atlas = IconAtlas::build(&[("x", svg)], 16).unwrap();
        assert!(atlas.image().pixels.iter().all(|p| p.a() == 0));
    }

    #[test]
    fn drawing_recolors_with_the_token() {
        let ctx = egui::Context::default();
        let atlas = IconAtlas::build(STARTER, 16).unwrap();
        let icons = atlas.upload(&ctx);
        let token = crate::theme::ThemeId::Dark.theme().colors.accent;
        assert!(icons.image("missing", Vec2::splat(16.0), token).is_none());
        let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(icons.image("check", Vec2::splat(16.0), token).unwrap());
        });
        full.textures_delta.clear();
        let uv = atlas.uv("check").unwrap();
        let tinted = full.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => {
                rect.fill == token && rect.brush.as_ref().is_some_and(|b| b.uv == uv)
            }
            _ => false,
        });
        assert!(tinted);
    }
}
