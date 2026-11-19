//! Headless rendering of a document's default view (CORE-CMD-026, CORE-APP-026): the drawn
//! objects of a snapshot, extracted by the product, zoomed to the default top view, shaded on a
//! white background through `arc-gpu` on the adapter its selection rules choose, and encoded as
//! PNG. The reference CLI's `render` subcommand and the protocol's `view.render` share this code;
//! the windowed shell and the automation socket attach [`SnapshotRenderer`].

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Error, ObjectId, Result, Tolerance};
use arc_command::ViewRequest;
use arc_command::error::codes as command_codes;
use arc_doc::{Object, Snapshot};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{Gpu, GpuSettings, RenderItem, RenderSettings, Renderer, Scene, SceneLimits};
use image::{ExtendedColorType, ImageEncoder};
use serde_json::{Value, json};

/// The only view a snapshot renderer draws: the default top view.
pub const DEFAULT_VIEW: &str = "default";

/// The display mode a snapshot renderer draws: shaded.
pub const SHADED: &str = "shaded";

/// The extents are fitted with this much room around them.
const MARGIN: f64 = 1.1;

/// Turns one object of a snapshot into the render items it draws; an object that draws nothing
/// returns an empty list.
pub type Extract = fn(&Snapshot, &Object) -> Vec<RenderItem>;

/// A rendered view.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The image, PNG encoded (8-bit RGBA).
    pub png: Vec<u8>,
    /// The number of objects drawn.
    pub objects: usize,
    /// The adapter the image was rendered on: `name`, `software` and `fallback`.
    pub adapter: Value,
}

/// The smallest box around the points of the drawn line items, or `None` when nothing is drawn.
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
fn top_view(center: Point3, height: f64) -> Result<ViewFrame> {
    ViewFrame::new(
        center + Vector3::new(0.0, 0.0, height.max(1.0)),
        center,
        Vector3::y(),
        Projection::Orthographic { height },
        1e-3,
        1e3,
    )
}

/// Renders the default view of `snapshot` at `width` by `height` pixels, drawing what `extract`
/// returns for each object.
///
/// # Errors
///
/// The `gpu.*` errors of adapter selection and rendering, and `cmd.path.io` when the PNG cannot
/// be encoded.
pub fn render(snapshot: &Snapshot, extract: Extract, width: u32, height: u32) -> Result<Rendered> {
    let changes: Vec<(ObjectId, Vec<RenderItem>)> = snapshot
        .iter()
        .map(|(_, object)| (object.id, extract(snapshot, object)))
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
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&image.rgba8, width, height, ExtendedColorType::Rgba8)
        .map_err(|_| Error::new(command_codes::PATH_IO).with_detail("reason", "other"))?;
    let report = gpu.report();
    Ok(Rendered {
        png,
        objects,
        adapter: json!({"name": report.name, "software": report.software, "fallback": report.fallback}),
    })
}

/// The view renderer of a product whose documents are drawn by `extract`: the default view,
/// shaded. Camera, section and other view names or display modes are refused with
/// `cmd.render.unsupported`.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotRenderer {
    extract: Extract,
}

impl SnapshotRenderer {
    /// A renderer drawing with `extract`.
    #[must_use]
    pub fn new(extract: Extract) -> Self {
        Self { extract }
    }
}

fn unsupported(feature: &'static str) -> Error {
    Error::new(command_codes::RENDER_UNSUPPORTED).with_detail("feature", feature)
}

impl arc_command::ViewRenderer for SnapshotRenderer {
    fn render(&self, request: &ViewRequest<'_>) -> Result<Vec<u8>> {
        if request.view != DEFAULT_VIEW {
            return Err(unsupported("view"));
        }
        if request.display_mode != SHADED {
            return Err(unsupported("display_mode"));
        }
        if request.camera.is_some() {
            return Err(unsupported("camera"));
        }
        if !request.sections.is_empty() {
            return Err(unsupported("sections"));
        }
        Ok(render(
            request.snapshot,
            self.extract,
            request.width,
            request.height,
        )?
        .png)
    }
}
