//! Raster output of plot streams (CORE-DRAFT-032), behind the feature `raster`.
//!
//! [`render`] draws a [`PlotStream`] with anti-aliasing at a requested resolution onto an
//! opaque page in the paper color; [`Raster::png`] encodes the pixels. The rules are stated in
//! the crate specification, section "Plotting as implemented".
//!
//! - One pixel is `25.4 / dpi` millimeters; the image is the page rounded up to whole pixels,
//!   with the page's lower left corner at the lower left of the image.
//! - Curves are flattened with a chord tolerance of a tenth of a pixel.
//! - A pen is `width · dpi / 25.4` pixels wide, at least one pixel, with the pen's end and join
//!   styles; each drawn piece of a dashed stroke is its own subpath, so dash and gap lengths are
//!   those of the stream.
//! - Masks paint the paper color. Everything is clipped to the plotted area and, for viewport
//!   content, to the viewport's clip region.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel coordinates are narrowed to f32 and sizes are checked first"
)]

use arc_base::{Color, Error, Result};
use arc_geom::flatten::CurveFlatten;
use arc_geom::{Curve2, Polyline2, Region};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};

use crate::clip::Piece;
use crate::error::codes;
use crate::plot::{Item, ItemKind, PlotStream};
use crate::plot_style::{EndStyle, JoinStyle, Pen};

/// The options of rasterization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterOptions {
    /// Pixels per inch, from 1 to 10000.
    pub dpi: f64,
    /// The most pixels of the image.
    pub max_pixels: u64,
}

impl Default for RasterOptions {
    fn default() -> Self {
        Self {
            dpi: 300.0,
            max_pixels: 400_000_000,
        }
    }
}

/// The most vertices of one flattened curve.
const MAX_VERTICES: usize = 1_000_000;
/// The chord tolerance in pixels.
const CHORD_PIXELS: f64 = 0.1;
/// The turning angle tolerance of flattening in radians.
const ANGLE: f64 = std::f64::consts::PI / 12.0;

/// A rendered page: opaque RGBA pixels, row 0 at the top of the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The pixels, 4 bytes each (sRGB, alpha 255), `width · height` of them.
    pub rgba: Vec<u8>,
    /// The number of items that were not drawn: glyph runs (outline glyphs are not rasterized
    /// by this part) and curves that could not be flattened.
    pub skipped: usize,
}

impl Raster {
    /// The pixel at column `x` and row `y` (row 0 at the top), if inside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = (usize::try_from(y).ok()? * usize::try_from(self.width).ok()?
            + usize::try_from(x).ok()?)
            * 4;
        self.rgba.get(at..at + 4)?.try_into().ok()
    }

    /// The PNG bytes of the image.
    ///
    /// # Errors
    ///
    /// `draft.raster.encode_failed`.
    pub fn png(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        image::ImageEncoder::write_image(
            encoder,
            &self.rgba,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| {
            Error::new(codes::RASTER_ENCODE_FAILED).with_detail("reason", e.to_string())
        })?;
        Ok(bytes)
    }
}

fn invalid(option: &'static str) -> Error {
    Error::new(codes::RASTER_INVALID).with_arg("option", option)
}

fn srgb(color: Color) -> tiny_skia::Color {
    let [r, g, b, a] = color.to_srgb8();
    tiny_skia::Color::from_rgba8(r, g, b, a)
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(srgb(color));
    paint.anti_alias = true;
    paint
}

/// Paths from page millimeters to pixels.
struct Space {
    /// Pixels per millimeter.
    scale: f64,
    /// The page height in pixels, the row of the page's lower edge.
    top: f64,
}

impl Space {
    fn point(&self, p: arc_base::math::Point2) -> (f32, f32) {
        (
            (p.x * self.scale) as f32,
            (self.top - p.y * self.scale) as f32,
        )
    }

    fn line(&self, builder: &mut PathBuilder, points: &[arc_base::math::Point2], close: bool) {
        for (i, p) in points.iter().enumerate() {
            let (x, y) = self.point(*p);
            if i == 0 {
                builder.move_to(x, y);
            } else {
                builder.line_to(x, y);
            }
        }
        if close {
            builder.close();
        }
    }

    fn chord(&self) -> f64 {
        CHORD_PIXELS / self.scale
    }

    /// The flattened vertices of a curve; `None` when it cannot be flattened.
    fn flatten(&self, curve: &Curve2) -> Option<Vec<arc_base::math::Point2>> {
        curve.flatten(self.chord(), ANGLE, MAX_VERTICES).ok()
    }

    /// One path of every loop of every part, to be filled with the even-odd rule.
    fn region(&self, region: &Region) -> Option<Path> {
        let mut builder = PathBuilder::new();
        for part in region.parts() {
            for lp in std::iter::once(part.outer()).chain(part.holes()) {
                let mut points: Vec<arc_base::math::Point2> = Vec::new();
                for curve in lp.curves() {
                    for p in curve.flatten(self.chord(), ANGLE, MAX_VERTICES).ok()? {
                        if points.last() != Some(&p) {
                            points.push(p);
                        }
                    }
                }
                if points.len() > 1 && points.first() == points.last() {
                    points.pop();
                }
                self.line(&mut builder, &points, true);
            }
        }
        builder.finish()
    }

    fn polygon(&self, region: &Polyline2) -> Option<Path> {
        let points = region.flatten(self.chord(), ANGLE, MAX_VERTICES).ok()?;
        let mut builder = PathBuilder::new();
        self.line(&mut builder, &points, true);
        builder.finish()
    }
}

fn stroke_of(pen: &Pen, width: f64, scale: f64) -> Stroke {
    Stroke {
        width: (width * scale).max(1.0) as f32,
        miter_limit: 4.0,
        line_cap: match pen.cap {
            EndStyle::Butt => LineCap::Butt,
            EndStyle::Square => LineCap::Square,
            EndStyle::Round => LineCap::Round,
        },
        line_join: match pen.join {
            JoinStyle::Miter => LineJoin::Miter,
            JoinStyle::Bevel => LineJoin::Bevel,
            JoinStyle::Round => LineJoin::Round,
        },
        dash: None,
    }
}

/// The width of a stroke: the pen's, or the mean width of a polyline with widths when larger.
fn width_of(curve: &Curve2, pen: &Pen) -> f64 {
    // shortcut: a polyline with widths is stroked with its mean vertex width instead of
    // tapering per segment; draw it as a filled outline when tapering matters.
    match curve {
        Curve2::Polyline(p) => {
            let n = p.vertices().len().max(1) as f64;
            let mean = p
                .vertices()
                .iter()
                .map(|v| 0.5 * (v.start_width + v.end_width))
                .sum::<f64>()
                / n;
            pen.width.max(mean)
        }
        _ => pen.width,
    }
}

/// Renders `stream` at `options.dpi`.
///
/// # Errors
///
/// `draft.raster.invalid` for a resolution outside 1 to 10000 or a page without area;
/// `draft.raster.too_large` when the image would have more than `options.max_pixels` pixels.
pub fn render(stream: &PlotStream, options: &RasterOptions) -> Result<Raster> {
    let dpi = options.dpi;
    if !(dpi.is_finite() && (1.0..=10_000.0).contains(&dpi)) {
        return Err(invalid("dpi"));
    }
    let scale = dpi / 25.4;
    // Rounded up, with a margin for the rounding of products like 297 mm · 300 / 25.4.
    let size = |mm: f64| (mm * scale - 1e-6).ceil().max(1.0);
    let (w, h) = (size(stream.width), size(stream.height));
    if !(w.is_finite() && h.is_finite()) || w > f64::from(u32::MAX) || h > f64::from(u32::MAX) {
        return Err(invalid("page"));
    }
    let pixels = w * h;
    #[expect(clippy::cast_precision_loss, reason = "a bound far below 2^53")]
    if pixels > options.max_pixels as f64 {
        return Err(Error::new(codes::RASTER_TOO_LARGE).with_arg(
            "limit",
            i64::try_from(options.max_pixels).unwrap_or(i64::MAX),
        ));
    }
    let (width, height) = (w as u32, h as u32);
    let mut pixmap = Pixmap::new(width, height).ok_or_else(|| invalid("page"))?;
    let paper = Color::new(
        stream.paper_color.r,
        stream.paper_color.g,
        stream.paper_color.b,
        1.0,
    );
    pixmap.fill(srgb(paper));
    // The page's lower edge is the bottom of the image.
    let space = Space {
        scale,
        top: f64::from(height),
    };
    let area_path = {
        let [lo, hi] = stream.area;
        let mut builder = PathBuilder::new();
        space.line(
            &mut builder,
            &[
                lo,
                arc_base::math::Point2::new(hi.x, lo.y),
                hi,
                arc_base::math::Point2::new(lo.x, hi.y),
            ],
            true,
        );
        builder.finish()
    };
    let mut base = Mask::new(width, height).ok_or_else(|| invalid("page"))?;
    if let Some(path) = &area_path {
        base.fill_path(path, FillRule::Winding, true, Transform::identity());
    }
    let mut masks: Vec<Option<Mask>> = Vec::with_capacity(stream.clips.len());
    for clip in &stream.clips {
        let mut mask = base.clone();
        match space.polygon(clip) {
            Some(path) => {
                mask.intersect_path(&path, FillRule::Winding, true, Transform::identity());
                masks.push(Some(mask));
            }
            None => masks.push(None),
        }
    }

    let mut skipped = 0;
    for item in &stream.items {
        let mask = match item.clip {
            Some(i) => match masks.get(i) {
                Some(Some(m)) => m,
                _ => continue,
            },
            None => &base,
        };
        let drawn = draw(&mut pixmap, &space, item, mask, paper);
        if !drawn {
            skipped += 1;
        }
    }
    // Straight alpha equals premultiplied for an opaque page.
    Ok(Raster {
        width,
        height,
        rgba: pixmap.take(),
        skipped,
    })
}

/// Draws one item; `false` when it was skipped.
fn draw(pixmap: &mut Pixmap, space: &Space, item: &Item, mask: &Mask, paper: Color) -> bool {
    let identity = Transform::identity();
    match &item.kind {
        ItemKind::Point { at, pen } => {
            let (x, y) = space.point(*at);
            let radius = ((pen.width * space.scale) as f32).max(1.0) / 2.0;
            let mut builder = PathBuilder::new();
            builder.push_circle(x, y, radius);
            if let Some(path) = builder.finish() {
                pixmap.fill_path(
                    &path,
                    &paint(pen.color),
                    FillRule::Winding,
                    identity,
                    Some(mask),
                );
            }
            true
        }
        ItemKind::Stroke { curve, pieces, pen } => {
            let mut builder = PathBuilder::new();
            let mut dots = Vec::new();
            let mut ok = true;
            match pieces {
                None => match space.flatten(curve) {
                    Some(points) => space.line(&mut builder, &points, false),
                    None => ok = false,
                },
                Some(list) => {
                    for piece in list {
                        match piece {
                            Piece::Point(p) => dots.push(*p),
                            Piece::Curve(c) => match space.flatten(c) {
                                Some(points) => space.line(&mut builder, &points, false),
                                None => ok = false,
                            },
                        }
                    }
                }
            }
            if let Some(path) = builder.finish() {
                let stroke = stroke_of(pen, width_of(curve, pen), space.scale);
                pixmap.stroke_path(&path, &paint(pen.color), &stroke, identity, Some(mask));
            }
            for p in dots {
                let (x, y) = space.point(p);
                let mut builder = PathBuilder::new();
                builder.push_circle(x, y, ((pen.width * space.scale) as f32).max(1.0) / 2.0);
                if let Some(path) = builder.finish() {
                    pixmap.fill_path(
                        &path,
                        &paint(pen.color),
                        FillRule::Winding,
                        identity,
                        Some(mask),
                    );
                }
            }
            ok
        }
        ItemKind::Fill { region, pen } => match space.polygon(region) {
            Some(path) => {
                pixmap.fill_path(
                    &path,
                    &paint(pen.color),
                    FillRule::Winding,
                    identity,
                    Some(mask),
                );
                true
            }
            None => false,
        },
        ItemKind::RegionFill { region, pen } => match space.region(region) {
            Some(path) => {
                pixmap.fill_path(
                    &path,
                    &paint(pen.color),
                    FillRule::EvenOdd,
                    identity,
                    Some(mask),
                );
                true
            }
            None => false,
        },
        ItemKind::Mask { region } => match space.polygon(region) {
            Some(path) => {
                // Opaque paper color, anti-aliased at the edge.
                pixmap.fill_path(
                    &path,
                    &paint(paper),
                    FillRule::Winding,
                    identity,
                    Some(mask),
                );
                true
            }
            None => false,
        },
        ItemKind::Glyphs { .. } => false,
    }
}
