//! PDF output of plot streams (CORE-IO-037). The rules are stated in the crate specification,
//! section "SVG and PDF output as implemented".

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{FRAC_PI_2, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Color, ObjectId, Result};
use arc_draft::plot::{Item, ItemKind, PlotStream};
use arc_draft::plot_style::{EndStyle, JoinStyle, Pen};

use crate::common::{
    Gate, Output, WriteOptions, check_count, check_stream, pdf_date_of, push_num, srgb,
};
use crate::error::reasons;
use crate::loss::{Argument, Direction, LossHeader, LossLog, Source};
use crate::path::{
    ArcSeg, Counts, Cx, FillRule, Seg, SubPath, polyline_path, range, region_loops, stroke_paths,
    stroke_width,
};
use crate::svg::{kind, tolerance_text, visible};

/// The most Bezier pieces of one arc.
const MAX_ARC_PIECES: f64 = 1024.0;
/// The largest page edge: 14400 points, the limit of common viewers, in millimeters.
const MAX_PAGE_MM: f64 = 14400.0 * 25.4 / 72.0;
/// The largest radial error of one cubic piece of a quarter circle, relative to the radius.
const QUARTER_ERROR: f64 = 2.7e-4;

struct Content {
    out: Vec<u8>,
    bad: bool,
    chord: f64,
    arcs: u64,
    alphas: BTreeSet<u8>,
}

fn pt(mm: f64) -> f64 {
    mm * 72.0 / 25.4
}

impl Content {
    fn new(chord: f64) -> Self {
        Self {
            out: Vec::new(),
            bad: false,
            chord,
            arcs: 0,
            alphas: BTreeSet::new(),
        }
    }

    fn s(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
    }

    /// A length in millimeters, written in points.
    fn n(&mut self, mm: f64) {
        push_num(&mut self.out, pt(mm), &mut self.bad);
        self.s(" ");
    }

    fn raw(&mut self, v: f64) {
        push_num(&mut self.out, v, &mut self.bad);
        self.s(" ");
    }

    fn xy(&mut self, p: Point2) {
        self.n(p.x);
        self.n(p.y);
    }

    fn pieces(&self, a: &ArcSeg) -> u32 {
        let base = (a.dt.abs() / FRAC_PI_2).ceil().max(1.0);
        let ratio = (QUARTER_ERROR * a.radius() / self.chord).max(1.0);
        let n = (a.dt.abs() / FRAC_PI_2 * ratio.powf(1.0 / 6.0)).ceil();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 1..=1024"
        )]
        let n = n.clamp(base, MAX_ARC_PIECES.max(base)) as u32;
        n.max(1)
    }

    /// Cubic pieces from `cur`; the last piece ends at the exact end point.
    fn arc(&mut self, a: &ArcSeg, cur: Point2) {
        let n = self.pieces(a);
        let mut from = cur;
        for i in 0..n {
            let (ta, tb, end) = a.piece(i, n);
            let k: f64 = 4.0 / 3.0 * ((tb - ta) / 4.0).tan();
            let c1: Point2 = from + a.tangent(ta) * k;
            let c2: Point2 = end - a.tangent(tb) * k;
            self.xy(c1);
            self.xy(c2);
            self.xy(end);
            self.s("c\n");
            from = end;
        }
    }

    fn path(&mut self, p: &SubPath) {
        self.xy(p.start);
        self.s("m\n");
        let mut cur = p.start;
        for seg in &p.segs {
            match seg {
                Seg::Line(q) => {
                    self.xy(*q);
                    self.s("l\n");
                    cur = *q;
                }
                Seg::Arc(a) => {
                    self.arcs += 1;
                    self.arc(a, cur);
                    cur = a.to;
                }
            }
        }
        if p.closed {
            self.s("h\n");
        }
    }

    fn rgb(&mut self, color: Color, operator: &str) {
        let [r, g, b, _] = srgb(color);
        for c in [r, g, b] {
            self.raw(f64::from(c) / 255.0);
        }
        self.s(operator);
        self.s("\n");
    }

    /// Opens a graphics state for a color with alpha; returns whether `Q` must follow.
    fn state(&mut self, color: Color) -> bool {
        let alpha = srgb(color)[3];
        if alpha == 255 {
            return false;
        }
        self.alphas.insert(alpha);
        self.s(&format!("q\n/A{alpha} gs\n"));
        true
    }

    /// Fills closed loops, which combine by `rule`.
    fn fill(&mut self, paths: &[SubPath], color: Color, rule: FillRule) {
        let q = self.state(color);
        self.rgb(color, "rg");
        for p in paths {
            self.path(p);
        }
        self.s(match rule {
            FillRule::NonZero => "f\n",
            FillRule::EvenOdd => "f*\n",
        });
        if q {
            self.s("Q\n");
        }
    }

    fn dot(&mut self, at: Point2, pen: &Pen) {
        let r = 0.5 * visible(pen.width);
        let start = at + Vector2::new(r, 0.0);
        let circle = ArcSeg {
            from: start,
            to: start,
            c: at,
            a: Vector2::new(r, 0.0),
            b: Vector2::new(0.0, r),
            t0: 0.0,
            dt: TAU,
        };
        let path = SubPath {
            start,
            segs: vec![Seg::Arc(circle)],
            closed: true,
        };
        let before = self.arcs;
        self.fill(&[path], pen.color, FillRule::NonZero);
        self.arcs = before;
    }

    fn stroke(&mut self, paths: &[SubPath], width: f64, pen: &Pen) {
        let q = self.state(pen.color);
        self.rgb(pen.color, "RG");
        self.n(width.max(0.0));
        self.s("w ");
        self.s(match pen.cap {
            EndStyle::Butt => "0 J ",
            EndStyle::Round => "1 J ",
            EndStyle::Square => "2 J ",
        });
        self.s(match pen.join {
            JoinStyle::Miter => "0 j\n",
            JoinStyle::Round => "1 j\n",
            JoinStyle::Bevel => "2 j\n",
        });
        for p in paths {
            self.path(p);
        }
        self.s("S\n");
        if q {
            self.s("Q\n");
        }
    }
}

/// Writes a document of one page per stream.
///
/// The bytes depend only on the streams and the options (no date unless
/// [`WriteOptions::creation_date`] is set). Content is uncompressed; layers are optional content
/// groups; circular and elliptical arcs are cubic Bezier curves within [`WriteOptions::chord_mm`];
/// fills use the nonzero rule; masks are filled in the paper color; outline glyph runs are
/// reported and not written.
///
/// # Errors
///
/// `io.pdf.option_invalid`, `io.pdf.stream_invalid`, `io.limit_exceeded` and `base.cancelled`.
pub fn write_pdf(streams: &[&PlotStream], options: &mut WriteOptions<'_>) -> Result<Output> {
    options.validate()?;
    if streams.is_empty() {
        return Err(crate::error::option("streams"));
    }
    for s in streams {
        check_stream(s, MAX_PAGE_MM)?;
    }
    let total = check_count(options, streams, false)?;
    let cx = options.cx();
    let (title, date) = (options.title.clone(), pdf_date_of(options));
    let layers: Vec<ObjectId> = streams
        .iter()
        .flat_map(|s| s.items.iter().filter_map(|i| i.layer))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let index: BTreeMap<ObjectId, usize> = layers.iter().copied().zip(0..).collect();

    let mut gate = Gate::new(options, total);
    let mut loss = LossLog::unbounded();
    let mut pages = Vec::with_capacity(streams.len());
    let mut written = 0usize;
    for stream in streams {
        gate.tick(written)?;
        let page = page_content(stream, &cx, &index, &mut gate, &mut loss, written)?;
        written += page.0.out.len();
        pages.push(page);
    }
    let failed: u64 = streams.iter().map(|s| s.failed.len() as u64).sum();
    let first = streams.iter().find_map(|s| s.failed.first());
    loss.add(
        reasons::SOURCES_FAILED,
        "entity",
        first.map(ToString::to_string).as_deref().map(Source::Text),
        failed,
    );
    if pages.iter().any(|(c, _)| c.bad) {
        return Err(range());
    }

    let mut file = File::default();
    file.out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    let first_layer = 4;
    let refs = |ids: &mut dyn Iterator<Item = usize>| {
        ids.map(|n| format!("{n} 0 R"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let ocgs = refs(&mut (first_layer..first_layer + layers.len()));
    let first_page = first_layer + layers.len();
    let kids = refs(&mut (0..pages.len()).map(|p| first_page + 2 * p));
    let mut catalog = String::from("<< /Type /Catalog /Pages 2 0 R");
    if !layers.is_empty() {
        catalog +=
            &format!(" /OCProperties << /OCGs [{ocgs}] /D << /Order [{ocgs}] /ON [{ocgs}] >> >>");
    }
    file.object(1, format!("{catalog} >>").as_bytes());
    file.object(
        2,
        format!("<< /Type /Pages /Count {} /Kids [{kids}] >>", pages.len()).as_bytes(),
    );
    let mut info = String::from("<< /Producer (Arc arc-io-pdf)");
    if let Some(t) = &title {
        info += &format!(" /Title {}", pdf_string(t));
    }
    if let Some(d) = &date {
        info += &format!(" /CreationDate ({d})");
    }
    file.object(3, format!("{info} >>").as_bytes());
    for (i, id) in layers.iter().enumerate() {
        file.object(
            first_layer + i,
            format!("<< /Type /OCG /Name {} >>", pdf_string(&id.to_string())).as_bytes(),
        );
    }
    for (p, (content, used)) in pages.iter().enumerate() {
        if let Some(stream) = streams.get(p) {
            file.page(first_page + 2 * p, stream, content, used, first_layer);
        }
    }
    file.trailer(first_page + 2 * pages.len());
    gate.finish(file.out.len())?;
    Ok(Output {
        bytes: file.out,
        report: loss.finish(LossHeader::new("pdf", "1.7", Direction::Export, None)),
    })
}

/// A PDF text string: a literal for printable ASCII, UTF-16 in hexadecimal otherwise.
fn pdf_string(text: &str) -> String {
    if text.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        let mut out = String::from("(");
        for c in text.chars() {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out + ")"
    } else {
        let mut out = String::from("<FEFF");
        for unit in text.encode_utf16() {
            out += &format!("{unit:04X}");
        }
        out + ">"
    }
}

#[derive(Default)]
struct File {
    out: Vec<u8>,
    /// The byte offset of each written object, with its number.
    offsets: BTreeMap<usize, usize>,
}

impl File {
    fn object(&mut self, number: usize, body: &[u8]) {
        self.offsets.insert(number, self.out.len());
        self.out
            .extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        self.out.extend_from_slice(body);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    fn trailer(&mut self, size: usize) {
        let xref = self.out.len();
        self.out
            .extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for n in 1..size {
            let at = self.offsets.get(&n).copied().unwrap_or(0);
            self.out
                .extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
        }
        self.out.extend_from_slice(
            format!(
                "trailer\n<< /Size {size} /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
    }
}

impl File {
    /// The page object `number` and its content stream `number + 1`.
    fn page(
        &mut self,
        number: usize,
        stream: &PlotStream,
        content: &Content,
        used: &BTreeSet<usize>,
        first_layer: usize,
    ) {
        let mut resources = String::from("<< ");
        if !content.alphas.is_empty() {
            resources += "/ExtGState << ";
            for a in &content.alphas {
                let mut v = Vec::new();
                push_num(&mut v, f64::from(*a) / 255.0, &mut false);
                let v = String::from_utf8(v).unwrap_or_default();
                resources += &format!("/A{a} << /Type /ExtGState /CA {v} /ca {v} >> ");
            }
            resources += ">> ";
        }
        if !used.is_empty() {
            resources += "/Properties << ";
            for l in used {
                resources += &format!("/L{l} {} 0 R ", first_layer + l);
            }
            resources += ">> ";
        }
        resources += ">>";
        let mut media = Content::new(0.0);
        media.n(stream.width);
        media.n(stream.height);
        let media = String::from_utf8(media.out).unwrap_or_default();
        self.object(
            number,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {}] /Resources {resources} /Contents {} 0 R >>",
                media.trim_end(),
                number + 1
            )
            .as_bytes(),
        );
        let mut body = format!("<< /Length {} >>\nstream\n", content.out.len()).into_bytes();
        body.extend_from_slice(&content.out);
        body.extend_from_slice(b"endstream");
        self.object(number + 1, &body);
    }
}

/// The content stream of one page and the indices of the layers it uses.
fn page_content(
    stream: &PlotStream,
    cx: &Cx,
    index: &BTreeMap<ObjectId, usize>,
    gate: &mut Gate<'_, '_>,
    loss: &mut LossLog,
    written: usize,
) -> Result<(Content, BTreeSet<usize>)> {
    let mut c = Content::new(cx.chord);
    let paper = Color::new(
        stream.paper_color.r,
        stream.paper_color.g,
        stream.paper_color.b,
        1.0,
    );
    c.rgb(paper, "rg");
    c.s("0 0 ");
    c.n(stream.width);
    c.n(stream.height);
    c.s("re f\nq\n4 M\n");
    let [lo, hi] = stream.area;
    c.xy(lo);
    c.n(hi.x - lo.x);
    c.n(hi.y - lo.y);
    c.s("re W n\n");
    let mut used = BTreeSet::new();
    let mut open: Option<(Option<usize>, Option<usize>)> = None;
    for item in &stream.items {
        gate.tick(written + c.out.len())?;
        let key = (item.clip, item.layer.and_then(|l| index.get(&l).copied()));
        if open != Some(key) {
            close_run(&mut c, open);
            if let Some(clip) = key.0.and_then(|i| stream.clips.get(i)) {
                c.s("q\n");
                let path = polyline_path(clip, cx)?;
                c.path(&path);
                c.s("W n\n");
            }
            if let Some(l) = key.1 {
                used.insert(l);
                c.s(&format!("/OC /L{l} BDC\n"));
            }
            open = Some(key);
        }
        item_out(&mut c, item, paper, cx, loss)?;
    }
    close_run(&mut c, open);
    c.s("Q\n");
    Ok((c, used))
}

fn close_run(c: &mut Content, open: Option<(Option<usize>, Option<usize>)>) {
    if let Some((clip, layer)) = open {
        if layer.is_some() {
            c.s("EMC\n");
        }
        if clip.is_some() {
            c.s("Q\n");
        }
    }
}

fn item_out(c: &mut Content, item: &Item, paper: Color, cx: &Cx, loss: &mut LossLog) -> Result<()> {
    let source = || item.source.to_string();
    let name = kind(item);
    let mut counts = Counts::default();
    let arcs_before = c.arcs;
    match &item.kind {
        ItemKind::Point { at, pen } => c.dot(*at, pen),
        ItemKind::Stroke { curve, pieces, pen } => {
            match stroke_paths(curve, pieces.as_deref(), cx, &mut counts)? {
                Some((paths, dots)) => {
                    let (width, tapered) = stroke_width(curve, pen.width);
                    counts.widths += u64::from(tapered);
                    if !paths.is_empty() {
                        c.stroke(&paths, width, pen);
                    }
                    for d in dots {
                        c.dot(d, pen);
                    }
                }
                None => {
                    loss.add(
                        reasons::ITEMS_FAILED,
                        name,
                        Some(Source::Text(&source())),
                        1,
                    );
                    return Ok(());
                }
            }
        }
        ItemKind::Fill { region, pen } => {
            let path = polyline_path(region, cx)?;
            c.fill(&[path], pen.color, FillRule::NonZero);
        }
        ItemKind::RegionFill { region, pen } => match region_loops(region, cx, &mut counts)? {
            Some(loops) => c.fill(&loops, pen.color, FillRule::EvenOdd),
            None => {
                loss.add(
                    reasons::ITEMS_FAILED,
                    name,
                    Some(Source::Text(&source())),
                    1,
                );
                return Ok(());
            }
        },
        ItemKind::Mask { region } => {
            let path = polyline_path(region, cx)?;
            c.fill(&[path], paper, FillRule::NonZero);
        }
        ItemKind::Glyphs { .. } => {
            loss.add(
                reasons::GLYPHS_DROPPED,
                name,
                Some(Source::Text(&source())),
                1,
            );
            return Ok(());
        }
    }
    let arcs = c.arcs - arcs_before;
    let argument = |v: f64| vec![("tolerance_mm".to_owned(), Argument::Text(tolerance_text(v)))];
    loss.add_with(
        reasons::NURBS_FLATTENED,
        name,
        Some(Source::Text(&source())),
        counts.nurbs,
        argument(cx.chord),
    );
    loss.add_with(
        reasons::ARCS_APPROXIMATED,
        name,
        Some(Source::Text(&source())),
        arcs,
        argument(cx.chord),
    );
    loss.add(
        reasons::POLYLINE_WIDTH_APPROXIMATED,
        name,
        Some(Source::Text(&source())),
        counts.widths,
    );
    if counts.nurbs + counts.widths + arcs == 0 {
        loss.add(
            reasons::ITEMS_WRITTEN,
            name,
            Some(Source::Text(&source())),
            1,
        );
    }
    Ok(())
}
