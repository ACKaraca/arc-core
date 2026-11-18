//! The `render` subcommand of the reference CLI: renders a package's sample objects headless
//! to a PNG file inside the write roots.
//!
//! `render <file> --output <png> [--view <name>] [--width <n>] [--height <n>]` opens the
//! package read-only, extracts points and lines with [`extract_sample`], zooms the default view
//! (orthographic, looking down −z with +y up) to the extents of the drawn objects, renders
//! through `arc-gpu` on the best adapter (a software adapter when nothing better exists) and
//! writes the image as PNG through [`Roots::create_write`]. The sample set defines no named
//! views, so `--view` accepts only `default`.

use std::ffi::OsString;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Error, Limits, ObjectId, Tolerance};
use arc_command::Roots;
use arc_command::error::codes;
use arc_command::headless::{Exit, Product};
use arc_doc::package::{Mode, Package};
use arc_doc::{DocumentConfig, LoadOptions, Object, Snapshot, Value};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{
    Gpu, GpuSettings, LineItem, LinePath, LineStyle, LineWidth, RenderItem, RenderSettings,
    Renderer, Scene, SceneLimits,
};
use image::{ExtendedColorType, ImageEncoder};
use serde_json::json;

/// The width of drawn points and lines in pixels.
const WIDTH_PX: f32 = 3.0;
/// The color of drawn objects (sRGB).
pub const INK: [u8; 4] = [40, 40, 40, 255];
/// The default image size in pixels.
const DEFAULT_SIZE: (u32, u32) = (800, 600);
/// The only view of the sample set.
pub const DEFAULT_VIEW: &str = "default";
/// The extents are fitted with this much room around them.
const MARGIN: f64 = 1.1;

/// The parsed arguments.
#[derive(Debug, Default)]
struct Options {
    file: Option<PathBuf>,
    output: Option<PathBuf>,
    view: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
}

fn usage(reason: impl Into<String>) -> Error {
    Error::new(codes::CLI_USAGE).with_detail("reason", reason.into())
}

fn parse(args: Vec<OsString>) -> Result<Options, Error> {
    let mut o = Options::default();
    let mut args = args.into_iter();
    let mut flags_ended = false;
    while let Some(arg) = args.next() {
        let Some(flag) = arg.to_str().filter(|a| !flags_ended && a.starts_with("--")) else {
            if o.file.replace(PathBuf::from(arg)).is_some() {
                return Err(usage("arguments"));
            }
            continue;
        };
        if flag == "--" {
            flags_ended = true;
            continue;
        }
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(OsString::from(value))),
            None => (flag.to_owned(), None),
        };
        if name == "--json" {
            if inline.is_some() {
                return Err(usage(format!("unexpected_value:{name}")));
            }
            continue;
        }
        let value = inline
            .or_else(|| args.next())
            .ok_or_else(|| usage(format!("missing_value:{name}")))?;
        let size = |value: OsString| {
            value
                .to_str()
                .filter(|v| !v.starts_with('+'))
                .and_then(|v| v.parse::<u32>().ok())
                .ok_or_else(|| usage(format!("size:{name}")))
        };
        let repeated = || usage(format!("repeated:{name}"));
        match name.as_str() {
            "--read-root" => o.read_roots.push(PathBuf::from(value)),
            "--write-root" => o.write_roots.push(PathBuf::from(value)),
            "--output" if o.output.is_none() => o.output = Some(PathBuf::from(value)),
            "--view" if o.view.is_none() => {
                let text = value
                    .into_string()
                    .map_err(|_| usage(format!("not_utf8:{name}")))?;
                o.view = Some(text);
            }
            "--width" if o.width.is_none() => o.width = Some(size(value)?),
            "--height" if o.height.is_none() => o.height = Some(size(value)?),
            "--output" | "--view" | "--width" | "--height" => return Err(repeated()),
            _ => return Err(usage(format!("unknown_flag:{name}"))),
        }
    }
    if o.file.is_none() {
        return Err(usage("arguments"));
    }
    if o.output.is_none() {
        return Err(usage("missing_flag:--output"));
    }
    if o.view.as_deref().is_some_and(|v| v != DEFAULT_VIEW) {
        return Err(usage("unknown_view"));
    }
    Ok(o)
}

fn point_of(object: &Object, at: usize) -> Option<Point3> {
    match object.values.get(at)? {
        Some(Value::Point3(p)) => Some(*p),
        _ => None,
    }
}

/// The offset of `to` from `from` as GPU coordinates.
#[allow(
    clippy::cast_possible_truncation,
    reason = "render data is f32 relative to an origin"
)]
fn offset(from: Point3, to: Point3) -> [f32; 3] {
    let d = to - from;
    [d.x as f32, d.y as f32, d.z as f32]
}

/// The render items of a sample object: a dot for a point, a segment for a line; layers have
/// none.
#[must_use]
pub fn extract_sample(snapshot: &Snapshot, object: &Object) -> Vec<RenderItem> {
    let key = snapshot
        .resolve(object.id)
        .ok()
        .and_then(|h| snapshot.type_key(h).map(|k| k.as_str().to_owned()))
        .unwrap_or_default();
    let item = |origin: Point3, points: Vec<[f32; 3]>| {
        RenderItem::Lines(LineItem {
            origin,
            paths: vec![LinePath {
                points,
                closed: false,
            }],
            style: LineStyle::round(LineWidth::Screen(WIDTH_PX)),
            color: Color::from_srgb8(INK),
        })
    };
    match key.as_str() {
        arc_shell_sample::POINT => point_of(object, 0)
            .map(|p| item(p, vec![[0.0, 0.0, 0.0]]))
            .into_iter()
            .collect(),
        arc_shell_sample::LINE => match (point_of(object, 0), point_of(object, 1)) {
            (Some(a), Some(b)) => vec![item(a, vec![[0.0, 0.0, 0.0], offset(a, b)])],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The smallest box around the points of the line items, or `None` when nothing is drawn.
fn extents(changes: &[(ObjectId, Vec<RenderItem>)]) -> Option<(Point3, Point3)> {
    let points = changes
        .iter()
        .flat_map(|(_, items)| items)
        .flat_map(|item| {
            let RenderItem::Lines(line) = item else {
                return Vec::new();
            };
            let origin = line.origin;
            line.paths
                .iter()
                .flat_map(|path| &path.points)
                .map(|[x, y, z]| origin + Vector3::new(f64::from(*x), f64::from(*y), f64::from(*z)))
                .collect()
        });
    points.fold(None, |bounds, p| {
        Some(match bounds {
            None => (p, p),
            Some((min, max)) => (min.inf(&p), max.sup(&p)),
        })
    })
}

/// The default view looking down −z at `center`, `height` model units tall.
fn top_view(center: Point3, height: f64) -> arc_base::Result<ViewFrame> {
    ViewFrame::new(
        center + Vector3::new(0.0, 0.0, height.max(1.0)),
        center,
        Vector3::y(),
        Projection::Orthographic { height },
        1e-3,
        1e3,
    )
}

/// Runs `render` with `args` (after the subcommand) and writes its JSON result to `stdout`.
///
/// # Errors
///
/// `cmd.cli.usage` for malformed arguments; `cmd.roots.invalid`; the package's `doc.*`
/// errors; the `gpu.*` errors of adapter selection and rendering; the errors of
/// [`Roots::create_write`] for the output path; `cmd.path.io` when the PNG cannot be written.
pub fn render<W: Write>(
    args: Vec<OsString>,
    product: &Product,
    stdout: &mut W,
) -> Result<Exit, Error> {
    let o = parse(args)?;
    let roots = Roots::new(&o.read_roots, &o.write_roots)?;
    let (file, output) = match (o.file, o.output) {
        (Some(file), Some(output)) => (file, output),
        _ => return Err(usage("arguments")),
    };
    let (width, height) = (
        o.width.unwrap_or(DEFAULT_SIZE.0),
        o.height.unwrap_or(DEFAULT_SIZE.1),
    );
    let limits = Limits::DEFAULT;
    let package = Package::open(&file, Mode::ReadOnly, &limits)?;
    let document = package.open_document(
        Arc::clone(&product.types),
        Arc::clone(&product.evaluators),
        &LoadOptions {
            limits,
            ..LoadOptions::default()
        },
        DocumentConfig::default(),
    )?;
    let snapshot = document.snapshot();
    let changes: Vec<(ObjectId, Vec<RenderItem>)> = snapshot
        .iter()
        .map(|(_, object)| (object.id, extract_sample(&snapshot, object)))
        .filter(|(_, items)| !items.is_empty())
        .collect();
    let objects = changes.len();
    let extents = extents(&changes);

    let gpu = Gpu::new(&GpuSettings::default())?;
    let mut scene = Scene::new(&gpu, SceneLimits::DEFAULT)?;
    scene.update(changes)?;
    let aspect = f64::from(width.max(1)) / f64::from(height.max(1));
    let frame = match extents {
        Some((min, max)) => {
            let center = min + (max - min) / 2.0;
            let radius = (max - min).norm() / 2.0;
            arc_gpu::navigation::zoom_to_sphere(
                &top_view(center, 1.0)?,
                &center,
                radius * MARGIN,
                aspect,
                &Tolerance::DEFAULT,
            )?
        }
        None => top_view(Point3::origin(), 10.0)?,
    };
    let settings = RenderSettings {
        background: Color::WHITE,
        ..RenderSettings::DEFAULT
    };
    let image =
        Renderer::new(&gpu).render_to_image(&mut scene, &frame, width, height, &settings)?;

    let io = |_| Error::new(codes::PATH_IO).with_detail("reason", "other");
    let mut writer = BufWriter::new(roots.create_write(&output)?);
    image::codecs::png::PngEncoder::new(&mut writer)
        .write_image(&image.rgba8, width, height, ExtendedColorType::Rgba8)
        .map_err(|_| Error::new(codes::PATH_IO).with_detail("reason", "other"))?;
    writer.flush().map_err(io)?;
    let report = gpu.report();
    let result = json!({
        "view": DEFAULT_VIEW,
        "width": width,
        "height": height,
        "objects": objects,
        "adapter": {"name": report.name, "software": report.software, "fallback": report.fallback},
    });
    writeln!(stdout, "{result}")
        .and_then(|()| stdout.flush())
        .map_err(io)?;
    Ok(Exit::Success)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    fn reason(list: &[&str]) -> String {
        let error = parse(list.iter().map(OsString::from).collect()).unwrap_err();
        assert_eq!(error.code(), "cmd.cli.usage");
        match error.details().get("reason") {
            Some(arc_base::error::Value::Text(t)) => t.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn arguments_parse_and_malformed_ones_are_usage_errors() {
        let o = parse(
            [
                "a.arc",
                "--output=o.png",
                "--view",
                "default",
                "--width",
                "16",
            ]
            .iter()
            .map(OsString::from)
            .collect(),
        )
        .unwrap();
        assert_eq!((o.width, o.height), (Some(16), None));
        assert_eq!(reason(&[]), "arguments");
        assert_eq!(reason(&["a", "b"]), "arguments");
        assert_eq!(reason(&["a"]), "missing_flag:--output");
        assert_eq!(
            reason(&["a", "--output", "o", "--view", "top"]),
            "unknown_view"
        );
        assert_eq!(reason(&["a", "--width", "-1"]), "size:--width");
        assert_eq!(
            reason(&["a", "--output", "o", "--output", "p"]),
            "repeated:--output"
        );
        assert_eq!(reason(&["a", "--nope", "x"]), "unknown_flag:--nope");
        assert_eq!(reason(&["a", "--height"]), "missing_value:--height");
    }
}
