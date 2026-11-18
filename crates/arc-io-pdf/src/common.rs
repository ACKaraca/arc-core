//! Options, validation, limits, progress and number formatting shared by both writers.

use arc_base::{Cancel, Color, Limits, Result};
use arc_draft::plot::PlotStream;

use crate::error::{limit, option, stream};
use crate::loss::LossReport;
use crate::path::{Cx, MAX_COORD};

/// The width, in millimeters, of a pen of width 0 where the output has no thinnest line
/// (SVG; PDF writes a line width of 0, its thinnest line) and of dots.
pub const HAIRLINE_MM: f64 = 0.05;

/// The default chord tolerance in millimeters: 5 micrometers, a hundredth of a 0.5 mm pen.
pub const DEFAULT_CHORD_MM: f64 = 0.005;

/// The bytes of a written file and its loss report.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// The file bytes.
    pub bytes: Vec<u8>,
    /// The loss report.
    pub report: LossReport,
}

/// Progress of a write: items are plot items plus one per page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Items written so far.
    pub items_done: u64,
    /// Items in total.
    pub items_total: u64,
}

/// Options of [`crate::write_svg`] and [`crate::write_pdf`].
pub struct WriteOptions<'a> {
    /// The limits profile. For an export `input_bytes` bounds the file size, `record_count` the
    /// plot items plus pages, `array_length` the vertices of one curve or polyline and
    /// `string_bytes` the title.
    pub limits: Limits,
    /// Cancelling it makes the write return `base.cancelled` before the next item.
    pub cancel: Cancel,
    /// Called on the first item, every 256 items and at the end.
    pub progress: Option<&'a mut dyn FnMut(Progress)>,
    /// The chord tolerance in millimeters of NURBS flattening and, in PDF, of the Bezier
    /// approximation of arcs; from 0.000001 to 100.
    pub chord_mm: f64,
    /// The document title: the SVG `title` element and the PDF information `Title`.
    pub title: Option<String>,
    /// The PDF creation date as `YYYY-MM-DDTHH:MM:SSZ`; without it the file has no date, so the
    /// bytes depend on the input only. Ignored by SVG.
    pub creation_date: Option<String>,
}

impl Default for WriteOptions<'_> {
    fn default() -> Self {
        Self {
            limits: Limits::CONSTRAINED,
            cancel: Cancel::new(),
            progress: None,
            chord_mm: DEFAULT_CHORD_MM,
            title: None,
            creation_date: None,
        }
    }
}

impl WriteOptions<'_> {
    /// Checks the options; every writer calls it first.
    pub(crate) fn validate(&self) -> Result<()> {
        if !(self.chord_mm.is_finite() && (1e-6..=100.0).contains(&self.chord_mm)) {
            return Err(option("chord_mm"));
        }
        if let Some(title) = &self.title {
            if title.len() as u64 > self.limits.string_bytes {
                return Err(limit("string_bytes"));
            }
            if title.chars().any(char::is_control) {
                return Err(option("title"));
            }
        }
        if let Some(date) = &self.creation_date
            && pdf_date(date).is_none()
        {
            return Err(option("creation_date"));
        }
        Ok(())
    }

    pub(crate) fn cx(&self) -> Cx {
        Cx {
            chord: self.chord_mm,
            max_vertices: usize::try_from(self.limits.array_length).unwrap_or(usize::MAX),
        }
    }
}

/// `D:YYYYMMDDHHMMSSZ` for a valid `YYYY-MM-DDTHH:MM:SSZ`.
fn pdf_date(text: &str) -> Option<String> {
    let b = text.as_bytes();
    if b.len() != 20 || !text.is_ascii() {
        return None;
    }
    let digits = |from: usize, to: usize| -> Option<u32> {
        let part = text.get(from..to)?;
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let seps = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'Z'),
    ];
    if seps.iter().any(|(at, c)| b.get(*at) != Some(c)) {
        return None;
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (hour, minute, second) = (digits(11, 13)?, digits(14, 16)?, digits(17, 19)?);
    ((1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60
        && second < 61
        && year > 0)
        .then(|| format!("D:{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z"))
}

/// The PDF form of a validated creation date.
pub(crate) fn pdf_date_of(options: &WriteOptions<'_>) -> Option<String> {
    options.creation_date.as_deref().and_then(pdf_date)
}

/// Checks a page of a stream; the page size may not exceed `max_page` millimeters.
pub(crate) fn check_stream(s: &PlotStream, max_page: f64) -> Result<()> {
    let sizes = [s.width, s.height];
    if !sizes
        .iter()
        .all(|v| v.is_finite() && *v > 0.0 && *v <= max_page)
    {
        return Err(stream("page_size"));
    }
    let [lo, hi] = s.area;
    if ![lo.x, lo.y, hi.x, hi.y]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= MAX_COORD)
        || lo.x > hi.x
        || lo.y > hi.y
    {
        return Err(stream("area"));
    }
    if s.items
        .iter()
        .any(|i| i.clip.is_some_and(|c| c >= s.clips.len()))
    {
        return Err(stream("clip_index"));
    }
    Ok(())
}

/// Counts items, pages and (`clips`) viewport clips against `record_count` before any work.
pub(crate) fn check_count(
    options: &WriteOptions<'_>,
    streams: &[&PlotStream],
    clips: bool,
) -> Result<u64> {
    let total = streams.iter().fold(0u64, |n, s| {
        let clips = if clips { s.clips.len() as u64 } else { 0 };
        n.saturating_add(s.items.len() as u64)
            .saturating_add(clips)
            .saturating_add(1)
    });
    if total > options.limits.record_count {
        return Err(limit("record_count"));
    }
    Ok(total)
}

/// Cancellation, progress and the output size limit.
pub(crate) struct Gate<'a, 'b> {
    options: &'b mut WriteOptions<'a>,
    done: u64,
    total: u64,
}

impl<'a, 'b> Gate<'a, 'b> {
    pub(crate) fn new(options: &'b mut WriteOptions<'a>, total: u64) -> Self {
        Self {
            options,
            done: 0,
            total,
        }
    }

    /// Called before every item with the output length so far.
    pub(crate) fn tick(&mut self, written: usize) -> Result<()> {
        self.options.cancel.check()?;
        if written as u64 > self.options.limits.input_bytes {
            return Err(limit("input_bytes"));
        }
        if self.done.is_multiple_of(256) {
            self.report();
        }
        self.done += 1;
        Ok(())
    }

    pub(crate) fn finish(&mut self, written: usize) -> Result<()> {
        if written as u64 > self.options.limits.input_bytes {
            return Err(limit("input_bytes"));
        }
        self.done = self.total;
        self.report();
        Ok(())
    }

    fn report(&mut self) {
        let (items_done, items_total) = (self.done, self.total);
        if let Some(f) = self.options.progress.as_deref_mut() {
            f(Progress {
                items_done,
                items_total,
            });
        }
    }
}

/// Appends `v` with at most four decimals, trailing zeros removed and no negative zero. A
/// value that is not finite or beyond [`MAX_COORD`] writes `0` and sets `bad`.
pub(crate) fn push_num(out: &mut Vec<u8>, v: f64, bad: &mut bool) {
    if !(v.is_finite() && v.abs() <= MAX_COORD) {
        *bad = true;
        out.push(b'0');
        return;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the magnitude is at most 1e9, so the scaled value is at most 1e13"
    )]
    let scaled = (v.abs() * 1e4).round() as u64;
    if v < 0.0 && scaled != 0 {
        out.push(b'-');
    }
    out.extend_from_slice((scaled / 10_000).to_string().as_bytes());
    let mut frac = scaled % 10_000;
    if frac != 0 {
        let mut digits = 4;
        while frac.is_multiple_of(10) {
            frac /= 10;
            digits -= 1;
        }
        out.push(b'.');
        out.extend_from_slice(format!("{frac:0digits$}").as_bytes());
    }
}

/// The sRGB bytes and alpha of a pen color.
pub(crate) fn srgb(color: Color) -> [u8; 4] {
    color.to_srgb8()
}
