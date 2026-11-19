//! SVG output of a plot stream (CORE-IO-038). The rules are stated in the crate specification,
//! section "SVG and PDF output as implemented".

use std::collections::BTreeSet;
use std::f64::consts::PI;

use arc_base::math::Point2;
use arc_base::{Color, Result};
use arc_draft::plot::{Item, ItemKind, PlotStream};
use arc_draft::plot_style::{EndStyle, JoinStyle, Pen};

use crate::common::{
    Gate, HAIRLINE_MM, Output, WriteOptions, check_count, check_stream, push_num, srgb,
};
use crate::error::reasons;
use crate::loss::{Argument, Direction, LossHeader, LossLog, Source};
use crate::path::{
    ArcSeg, Counts, Cx, FillRule, Seg, SubPath, polyline_path, range, region_loops, stroke_paths,
    stroke_width,
};

/// The largest page edge of an SVG document in millimeters.
const MAX_PAGE_MM: f64 = 1e6;

struct Svg {
    out: Vec<u8>,
    bad: bool,
    /// The page height, for the flip of the y axis.
    height: f64,
}

impl Svg {
    fn s(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
    }

    fn n(&mut self, v: f64) {
        push_num(&mut self.out, v, &mut self.bad);
    }

    fn xy(&mut self, p: Point2) {
        self.n(p.x);
        self.s(" ");
        self.n(self.height - p.y);
    }

    fn arc(&mut self, a: &ArcSeg) {
        let n = ((a.dt.abs() / PI) - 1e-9).ceil().max(1.0);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "1 or 2"
        )]
        let n = n as u32;
        let (rx, ry) = (a.a.norm(), a.b.norm());
        let rotation = if (rx - ry).abs() <= 1e-12 * rx {
            0.0
        } else {
            (-a.a.y).atan2(a.a.x).to_degrees()
        };
        // The y axis is flipped: a counterclockwise sweep draws with sweep flag 0.
        let flag = if a.dt > 0.0 { "0" } else { "1" };
        for i in 0..n {
            let (_, _, end) = a.piece(i, n);
            self.s("A");
            self.n(rx);
            self.s(" ");
            self.n(ry);
            self.s(" ");
            self.n(rotation);
            self.s(" 0 ");
            self.s(flag);
            self.s(" ");
            self.xy(end);
        }
    }

    fn path(&mut self, p: &SubPath) {
        self.s("M");
        self.xy(p.start);
        for seg in &p.segs {
            match seg {
                Seg::Line(q) => {
                    self.s("L");
                    self.xy(*q);
                }
                Seg::Arc(a) => self.arc(a),
            }
        }
        if p.closed {
            self.s("Z");
        }
    }

    fn color(&mut self, attribute: &str, color: Color) {
        let [r, g, b, a] = srgb(color);
        self.s(&format!(" {attribute}=\"#{r:02x}{g:02x}{b:02x}\""));
        if a != 255 {
            self.s(&format!(" {attribute}-opacity=\""));
            self.n(f64::from(a) / 255.0);
            self.s("\"");
        }
    }

    /// One `path` filled from closed loops, which combine by `rule`.
    fn fill_loops(&mut self, loops: &[SubPath], color: Color, rule: FillRule) {
        self.s("<path d=\"");
        for l in loops {
            self.path(l);
        }
        self.s("\"");
        self.color("fill", color);
        self.s(match rule {
            FillRule::NonZero => " fill-rule=\"nonzero\"/>\n",
            FillRule::EvenOdd => " fill-rule=\"evenodd\"/>\n",
        });
    }

    fn dot(&mut self, at: Point2, pen: &Pen) {
        self.s("<circle cx=\"");
        self.n(at.x);
        self.s("\" cy=\"");
        self.n(self.height - at.y);
        self.s("\" r=\"");
        self.n(0.5 * visible(pen.width));
        self.s("\"");
        self.color("fill", pen.color);
        self.s("/>\n");
    }

    fn stroke(&mut self, paths: &[SubPath], width: f64, pen: &Pen) {
        self.s("<path d=\"");
        for p in paths {
            self.path(p);
        }
        self.s("\" fill=\"none\"");
        self.color("stroke", pen.color);
        self.s(" stroke-width=\"");
        self.n(visible(width));
        self.s("\" stroke-linecap=\"");
        self.s(match pen.cap {
            EndStyle::Butt => "butt",
            EndStyle::Square => "square",
            EndStyle::Round => "round",
        });
        self.s("\" stroke-linejoin=\"");
        self.s(match pen.join {
            JoinStyle::Miter => "miter",
            JoinStyle::Bevel => "bevel",
            JoinStyle::Round => "round",
        });
        self.s("\" stroke-miterlimit=\"4\"/>\n");
    }
}

/// The width a pen draws with: its own, or the hairline for a pen of width 0.
pub(crate) fn visible(width: f64) -> f64 {
    if width > 0.0 { width } else { HAIRLINE_MM }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn kind(item: &Item) -> &'static str {
    match item.kind {
        ItemKind::Point { .. } => "point",
        ItemKind::Stroke { .. } => "stroke",
        ItemKind::Fill { .. } => "fill",
        ItemKind::RegionFill { .. } => "region_fill",
        ItemKind::Mask { .. } => "mask",
        ItemKind::Glyphs { .. } => "glyphs",
    }
}

/// Writes one page as an SVG document sized in millimeters, with a view box in paper
/// millimeters.
///
/// The bytes depend only on the stream and the options. Strokes keep exact arcs (`A` commands),
/// dash pieces are separate subpaths, fills use the nonzero rule, masks are filled in the paper
/// color, viewport clips are `clipPath` elements; outline glyph runs are reported and not
/// written.
///
/// # Errors
///
/// `io.pdf.option_invalid`, `io.pdf.stream_invalid`, `io.limit_exceeded` and `base.cancelled`.
pub fn write_svg(stream: &PlotStream, options: &mut WriteOptions<'_>) -> Result<Output> {
    options.validate()?;
    check_stream(stream, MAX_PAGE_MM)?;
    let total = check_count(options, &[stream], true)?;
    let cx = options.cx();
    let title = options.title.clone();
    let mut gate = Gate::new(options, total);
    let mut loss = LossLog::unbounded();
    let mut svg = Svg {
        out: Vec::new(),
        bad: false,
        height: stream.height,
    };
    svg.s("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"");
    svg.n(stream.width);
    svg.s("mm\" height=\"");
    svg.n(stream.height);
    svg.s("mm\" viewBox=\"0 0 ");
    svg.n(stream.width);
    svg.s(" ");
    svg.n(stream.height);
    svg.s("\">\n");
    if let Some(title) = &title {
        svg.s(&format!("<title>{}</title>\n", escape(title)));
    }
    svg.s("<defs>\n<clipPath id=\"a\"><rect x=\"");
    let [lo, hi] = stream.area;
    svg.n(lo.x);
    svg.s("\" y=\"");
    svg.n(stream.height - hi.y);
    svg.s("\" width=\"");
    svg.n(hi.x - lo.x);
    svg.s("\" height=\"");
    svg.n(hi.y - lo.y);
    svg.s("\"/></clipPath>\n");
    for (i, region) in stream.clips.iter().enumerate() {
        gate.tick(svg.out.len())?;
        let path = polyline_path(region, &cx)?;
        svg.s(&format!("<clipPath id=\"c{i}\"><path d=\""));
        svg.path(&path);
        svg.s("\"/></clipPath>\n");
    }
    svg.s("</defs>\n<rect width=\"100%\" height=\"100%\"");
    let paper = Color::new(
        stream.paper_color.r,
        stream.paper_color.g,
        stream.paper_color.b,
        1.0,
    );
    svg.color("fill", paper);
    svg.s("/>\n<g clip-path=\"url(#a)\">\n");

    let mut open = None::<usize>;
    for item in &stream.items {
        gate.tick(svg.out.len())?;
        if item.clip != open {
            if open.is_some() {
                svg.s("</g>\n");
            }
            if let Some(c) = item.clip {
                svg.s(&format!("<g clip-path=\"url(#c{c})\">\n"));
            }
            open = item.clip;
        }
        item_out(&mut svg, item, paper, &cx, &mut loss)?;
    }
    if open.is_some() {
        svg.s("</g>\n");
    }
    svg.s("</g>\n</svg>\n");

    let layers: BTreeSet<_> = stream.items.iter().filter_map(|i| i.layer).collect();
    loss.add(reasons::LAYERS_DROPPED, "layer", None, layers.len() as u64);
    loss.add(
        reasons::SOURCES_FAILED,
        "entity",
        stream
            .failed
            .first()
            .map(ToString::to_string)
            .as_deref()
            .map(Source::Text),
        stream.failed.len() as u64,
    );
    if svg.bad {
        return Err(range());
    }
    gate.finish(svg.out.len())?;
    Ok(Output {
        bytes: svg.out,
        report: loss.finish(LossHeader::new("svg", "1.1", Direction::Export, None)),
    })
}

/// Writes one item and records what happened to it.
fn item_out(svg: &mut Svg, item: &Item, paper: Color, cx: &Cx, loss: &mut LossLog) -> Result<()> {
    let source = || item.source.to_string();
    let mut counts = Counts::default();
    let drawn = match &item.kind {
        ItemKind::Point { at, pen } => {
            svg.dot(*at, pen);
            true
        }
        ItemKind::Stroke { curve, pieces, pen } => {
            match stroke_paths(curve, pieces.as_deref(), cx, &mut counts)? {
                Some((paths, dots)) => {
                    let (width, tapered) = stroke_width(curve, pen.width);
                    counts.widths += u64::from(tapered);
                    if !paths.is_empty() {
                        svg.stroke(&paths, width, pen);
                    }
                    for d in dots {
                        svg.dot(d, pen);
                    }
                    true
                }
                None => false,
            }
        }
        ItemKind::Fill { region, pen } => {
            let path = polyline_path(region, cx)?;
            svg.fill_loops(&[path], pen.color, FillRule::NonZero);
            true
        }
        ItemKind::RegionFill { region, pen } => match region_loops(region, cx, &mut counts)? {
            Some(loops) => {
                svg.fill_loops(&loops, pen.color, FillRule::EvenOdd);
                true
            }
            None => false,
        },
        ItemKind::Mask { region } => {
            let path = polyline_path(region, cx)?;
            svg.fill_loops(&[path], paper, FillRule::NonZero);
            true
        }
        ItemKind::Glyphs { .. } => {
            loss.add(
                reasons::GLYPHS_DROPPED,
                kind(item),
                Some(Source::Text(&source())),
                1,
            );
            return Ok(());
        }
    };
    let name = kind(item);
    if !drawn {
        loss.add(
            reasons::ITEMS_FAILED,
            name,
            Some(Source::Text(&source())),
            1,
        );
        return Ok(());
    }
    loss.add_with(
        reasons::NURBS_FLATTENED,
        name,
        Some(Source::Text(&source())),
        counts.nurbs,
        vec![(
            "tolerance_mm".to_owned(),
            Argument::Text(tolerance_text(cx.chord)),
        )],
    );
    loss.add(
        reasons::POLYLINE_WIDTH_APPROXIMATED,
        name,
        Some(Source::Text(&source())),
        counts.widths,
    );
    if counts.nurbs == 0 && counts.widths == 0 {
        loss.add(
            reasons::ITEMS_WRITTEN,
            name,
            Some(Source::Text(&source())),
            1,
        );
    }
    Ok(())
}

pub(crate) fn tolerance_text(chord: f64) -> String {
    chord.to_string()
}
