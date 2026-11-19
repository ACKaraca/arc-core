//! The command `core.draft.plot` (CORE-DRAFT-033), registered with [`register_commands`].
//!
//! The command builds a plot stream per selected layout (or one for a window of a model space),
//! writes it as PDF, SVG or PNG and stores the bytes through [`Context::create_write`], so the
//! session's roots and permission classes decide what it may touch. All files are encoded
//! before the first one is created: a failing page leaves no partial output behind.

use std::io::Write;
use std::path::{Path, PathBuf};

use arc_base::{Color, Error, ObjectId, Result};
use arc_command::{CommandSpec, Context, IdParam, Permission, Registry, UndoBehavior};
use arc_draft::display::{DisplayOptions, Lists};
use arc_draft::plot::{
    Offset, Paper, PlotArea, PlotOptions, PlotScale, PlotStream, layout_stream, model_stream,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::loss::LossReport;
use crate::{WriteOptions, write_pdf, write_svg};
#[cfg(feature = "raster")]
use crate::{
    error::reasons,
    loss::{LossLog, Source},
};

/// A message identifier of this crate that is not an error code or a loss reason.
pub struct Message {
    /// The identifier in `locales/en-US/arc-io-pdf.ftl`.
    pub message_id: &'static str,
}

/// The label of `core.draft.plot`.
pub const PLOT_LABEL: Message = Message {
    message_id: "io-pdf-command-plot",
};
/// The description of `core.draft.plot`.
pub const PLOT_DESCRIPTION: Message = Message {
    message_id: "io-pdf-command-plot-description",
};

/// The most layouts one invocation plots.
pub const MAX_LAYOUTS: usize = 256;

/// The output format of `core.draft.plot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// One PDF file, one page per layout.
    Pdf,
    /// One SVG file per layout.
    Svg,
    /// One PNG file per layout; needs the feature `raster`.
    Png,
}

/// A window of a model space plotted at a scale.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelSource {
    /// The model space.
    pub space: IdParam,
    /// The window in model coordinates: `[min x, min y, max x, max y]`.
    pub window: [f64; 4],
    /// Paper length per model length in one unit (1:100 is 0.01).
    pub scale: f64,
    /// The paper `[width, height]` in millimeters; A4 portrait when absent.
    #[serde(default)]
    pub paper: Option<[f64; 2]>,
}

/// Which part of the source is plotted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AreaName {
    /// The printable area of the layout.
    Layout,
    /// The bounding box of everything drawn.
    Extents,
    /// The window of `options.window`.
    Window,
}

/// Options that override the plot settings of a layout (or the defaults of a model plot).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    /// The plotted part of the source. A `window` alone implies `window`.
    #[serde(default)]
    pub area: Option<AreaName>,
    /// The window in source coordinates: `[min x, min y, max x, max y]`.
    #[serde(default)]
    pub window: Option<[f64; 4]>,
    /// Paper length per source length.
    #[serde(default)]
    pub scale: Option<f64>,
    /// Fill the printable area as far as the aspect ratios allow; replaces `scale`.
    #[serde(default)]
    pub fit: bool,
    /// Center the plotted area on the printable area; replaces `origin`.
    #[serde(default)]
    pub center: bool,
    /// The offset in millimeters from the lower left of the printable area.
    #[serde(default)]
    pub origin: Option<[f64; 2]>,
    /// Quarter turns counterclockwise.
    #[serde(default)]
    pub rotation: Option<u8>,
    /// Whether lineweights plot.
    #[serde(default)]
    pub lineweights: Option<bool>,
    /// Whether plot style tables apply.
    #[serde(default)]
    pub styles: Option<bool>,
    /// The name of the plot style table.
    #[serde(default)]
    pub style_table: Option<String>,
    /// The linear RGBA paper color; opaque white when absent.
    #[serde(default)]
    pub paper_color: Option<[f32; 4]>,
    /// The chord tolerance in millimeters of the writers.
    #[serde(default)]
    pub chord_mm: Option<f64>,
    /// The title of the PDF or SVG document.
    #[serde(default)]
    pub title: Option<String>,
}

/// Parameters of `core.draft.plot`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlotParams {
    /// The layout spaces to plot, in page order; up to 256. Exactly one of `layouts` and `model`.
    #[serde(default)]
    pub layouts: Vec<IdParam>,
    /// A window of a model space; exactly one of `layouts` and `model`.
    #[serde(default)]
    pub model: Option<ModelSource>,
    /// The output format.
    pub format: Format,
    /// The file to write, inside the session's write roots. For SVG and PNG with several
    /// layouts, `-N` (N from 1) is inserted before the extension of each file name.
    #[schemars(length(min = 1, max = 4095))]
    pub path: String,
    /// Pixels per inch of PNG output, 1 to 10000; 300 when absent.
    #[serde(default)]
    pub dpi: Option<f64>,
    /// Options that override the plot settings.
    #[serde(default)]
    pub options: Option<Overrides>,
}

/// What `core.draft.plot` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PlotResult {
    /// The files written, in page order.
    pub paths: Vec<String>,
    /// The pages plotted.
    pub pages: u64,
    /// One loss report per file written, in the order of `paths`, in the form of
    /// `schemas/loss-report.schema.json`.
    pub reports: Vec<serde_json::Value>,
}

fn invalid(what: &'static str) -> Error {
    Error::new(codes::PLOT_PARAMS_INVALID).with_arg("what", what)
}

fn finite(values: &[f64]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// The path of file `index` (from 0) of `count`: the path itself for one file, else `-N`
/// (N = index + 1) inserted before the extension of the file name.
#[must_use]
pub fn numbered_path(path: &str, index: usize, count: usize) -> String {
    if count <= 1 {
        return path.to_owned();
    }
    let n = index + 1;
    let p = Path::new(path);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
    let name = match (stem, p.extension()) {
        (Some(stem), Some(ext)) => format!("{stem}-{n}.{}", ext.to_string_lossy()),
        (Some(stem), None) => format!("{stem}-{n}"),
        (None, _) => format!("-{n}"),
    };
    let mut out: PathBuf = p.to_path_buf();
    out.set_file_name(name);
    out.to_string_lossy().into_owned()
}

/// Applies the overrides to the options of the source.
fn apply(options: &mut PlotOptions, o: &Overrides) -> Result<()> {
    let window = match o.window {
        Some([a, b, c, d]) => {
            if !(finite(&[a, b, c, d]) && a < c && b < d) {
                return Err(invalid("options.window"));
            }
            Some(PlotArea::Window {
                min: arc_base::math::Point2::new(a, b),
                max: arc_base::math::Point2::new(c, d),
            })
        }
        None => None,
    };
    match (o.area, window) {
        (Some(AreaName::Window), None) => return Err(invalid("options.window")),
        (Some(AreaName::Layout), _) => options.area = PlotArea::Layout,
        (Some(AreaName::Extents), _) => options.area = PlotArea::Extents,
        (_, Some(area)) => options.area = area,
        (None, None) => {}
    }
    if o.fit && o.scale.is_some() {
        return Err(invalid("options.scale"));
    }
    if o.fit {
        options.scale = PlotScale::Fit;
    } else if let Some(scale) = o.scale {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(invalid("options.scale"));
        }
        options.scale = PlotScale::Ratio(scale);
    }
    if o.center && o.origin.is_some() {
        return Err(invalid("options.origin"));
    }
    if o.center {
        options.offset = Offset::Center;
    } else if let Some([x, y]) = o.origin {
        if !finite(&[x, y]) {
            return Err(invalid("options.origin"));
        }
        options.offset = Offset::Origin(arc_base::math::Point2::new(x, y));
    }
    if let Some(turns) = o.rotation {
        options.rotation = turns;
    }
    if let Some(on) = o.lineweights {
        options.lineweights = on;
    }
    if let Some(on) = o.styles {
        options.styles = on;
    }
    if let Some(name) = &o.style_table {
        options.style_table = Some(name.clone()).filter(|n| !n.is_empty());
    }
    if let Some([r, g, b, a]) = o.paper_color {
        options.paper_color = Color::new(r, g, b, a);
    }
    Ok(())
}

fn model_options(m: &ModelSource) -> Result<(Paper, PlotOptions)> {
    let [a, b, c, d] = m.window;
    if !(finite(&[a, b, c, d]) && a < c && b < d) {
        return Err(invalid("model.window"));
    }
    if !(m.scale.is_finite() && m.scale > 0.0) {
        return Err(invalid("model.scale"));
    }
    let [w, h] = m.paper.unwrap_or([210.0, 297.0]);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return Err(invalid("model.paper"));
    }
    let options = PlotOptions {
        area: PlotArea::Window {
            min: arc_base::math::Point2::new(a, b),
            max: arc_base::math::Point2::new(c, d),
        },
        scale: PlotScale::Ratio(m.scale),
        offset: Offset::Center,
        ..PlotOptions::default()
    };
    let paper = Paper {
        width: w,
        height: h,
        margins: [0.0; 4],
    };
    Ok((paper, options))
}

fn streams_of(ctx: &Context<'_>, p: &PlotParams, overrides: &Overrides) -> Result<Vec<PlotStream>> {
    let snapshot = ctx.snapshot();
    let display = DisplayOptions::default();
    match (&p.model, p.layouts.is_empty()) {
        (Some(_), false) => Err(invalid("model")),
        (None, true) => Err(invalid("layouts")),
        (Some(m), true) => {
            let (paper, mut options) = model_options(m)?;
            apply(&mut options, overrides)?;
            Ok(vec![model_stream(
                snapshot,
                Lists::Direct,
                m.space.0,
                &paper,
                &options,
                &display,
            )?])
        }
        (None, false) => {
            if p.layouts.len() > MAX_LAYOUTS {
                return Err(invalid("layouts"));
            }
            let mut out = Vec::with_capacity(p.layouts.len());
            for IdParam(layout) in &p.layouts {
                ctx.checkpoint()?;
                let layout: ObjectId = *layout;
                let mut options = PlotOptions::from_layout(snapshot, layout)?;
                apply(&mut options, overrides)?;
                out.push(layout_stream(
                    snapshot,
                    Lists::Direct,
                    layout,
                    &options,
                    &display,
                )?);
            }
            Ok(out)
        }
    }
}

#[cfg(feature = "raster")]
fn png(stream: &PlotStream, dpi: Option<f64>) -> Result<(Vec<u8>, LossReport)> {
    let mut options = arc_draft::raster::RasterOptions::default();
    if let Some(dpi) = dpi {
        options.dpi = dpi;
    }
    let raster = arc_draft::raster::render(stream, &options)?;
    let bytes = raster.png()?;
    let mut log = LossLog::bounded();
    let drawn = stream.items.len().saturating_sub(raster.skipped);
    log.add(reasons::ITEMS_WRITTEN, "item", None, drawn as u64);
    log.add(reasons::ITEMS_SKIPPED, "item", None, raster.skipped as u64);
    log.add(
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
    Ok((
        bytes,
        log.finish(crate::loss::LossHeader::new(
            "png",
            "1.2",
            crate::loss::Direction::Export,
            None,
        )),
    ))
}

#[cfg(not(feature = "raster"))]
fn png(_: &PlotStream, _: Option<f64>) -> Result<(Vec<u8>, LossReport)> {
    Err(Error::new(codes::RASTER_UNAVAILABLE))
}

fn plot(ctx: &mut Context<'_>, p: PlotParams) -> Result<PlotResult> {
    if p.format == Format::Png && !cfg!(feature = "raster") {
        return Err(Error::new(codes::RASTER_UNAVAILABLE));
    }
    if let Some(dpi) = p.dpi
        && !(dpi.is_finite() && (1.0..=10_000.0).contains(&dpi))
    {
        return Err(invalid("dpi"));
    }
    let overrides = p.options.as_ref();
    let default = Overrides::default();
    let overrides = overrides.unwrap_or(&default);
    let streams = streams_of(ctx, &p, overrides)?;
    ctx.checkpoint()?;
    let mut write = WriteOptions {
        limits: *ctx.limits(),
        title: overrides.title.clone(),
        ..WriteOptions::default()
    };
    if let Some(chord) = overrides.chord_mm {
        write.chord_mm = chord;
    }
    let mut files: Vec<(String, Vec<u8>, LossReport)> = Vec::new();
    match p.format {
        Format::Pdf => {
            let refs: Vec<&PlotStream> = streams.iter().collect();
            let out = write_pdf(&refs, &mut write)?;
            files.push((p.path.clone(), out.bytes, out.report));
        }
        Format::Svg | Format::Png => {
            for (i, stream) in streams.iter().enumerate() {
                ctx.checkpoint()?;
                let path = numbered_path(&p.path, i, streams.len());
                let (bytes, report) = if p.format == Format::Svg {
                    let out = write_svg(stream, &mut write)?;
                    (out.bytes, out.report)
                } else {
                    png(stream, p.dpi)?
                };
                files.push((path, bytes, report));
            }
        }
    }
    let mut result = PlotResult {
        paths: Vec::new(),
        pages: streams.len() as u64,
        reports: Vec::new(),
    };
    for (path, bytes, mut report) in files {
        let mut file = ctx.create_write(Path::new(&path))?;
        file.write_all(&bytes)
            .and_then(|()| file.flush())
            .map_err(|e| Error::new(arc_command::error::codes::PATH_IO).with_source(e))?;
        report.header.file_name = Some(path.clone());
        result.reports.push(
            serde_json::to_value(&report)
                .map_err(|_| Error::new(arc_command::error::codes::RESULT_INVALID))?,
        );
        result.paths.push(path);
    }
    Ok(result)
}

/// Registers `core.draft.plot` (permission classes `read` and `file_write`, not undoable, on
/// every surface) into `registry`.
///
/// # Errors
///
/// `cmd.duplicate` when the identifier is already registered.
pub fn register_commands(registry: &mut Registry) -> Result<()> {
    registry.register(
        CommandSpec::new(
            "core.draft.plot",
            PLOT_LABEL.message_id,
            PLOT_DESCRIPTION.message_id,
            plot,
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read, Permission::FileWrite]),
    )
}

#[cfg(test)]
mod tests {
    use super::numbered_path;

    #[test]
    fn numbering_inserts_the_index_before_the_extension() {
        // Windows joins the new name with its own separator; both separators are valid there.
        let n = |path, index, count| numbered_path(path, index, count).replace('\\', "/");
        assert_eq!(n("/o/sheet.svg", 0, 1), "/o/sheet.svg");
        assert_eq!(n("/o/sheet.svg", 0, 3), "/o/sheet-1.svg");
        assert_eq!(n("/o/sheet.v2.png", 2, 3), "/o/sheet.v2-3.png");
        assert_eq!(n("/o/sheet", 1, 2), "/o/sheet-2");
        assert_eq!(n("/o.d/sheet", 1, 2), "/o.d/sheet-2");
    }
}
