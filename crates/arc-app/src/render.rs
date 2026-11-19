//! Headless rendering of a document's default view (CORE-CMD-026, CORE-APP-026): the drawn
//! objects of a snapshot, extracted by the product, zoomed to the default top view, shaded on a
//! white background through `arc-gpu` on the adapter its selection rules choose, and encoded as
//! PNG. The reference CLI's `render` subcommand and the protocol's `view.render` share this code;
//! the windowed shell and the automation socket attach [`SnapshotRenderer`], which keeps one device
//! and renders one image at a time.

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

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
/// returns an empty list. An error stops the render and is returned to its caller.
pub type Extract = fn(&Snapshot, &Object) -> Result<Vec<RenderItem>>;

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

/// The objects of a snapshot that draw something, with the extents of their lines.
struct Drawn {
    changes: Vec<(ObjectId, Vec<RenderItem>)>,
    extents: Option<(Point3, Point3)>,
}

/// Runs `extract` on every object of `snapshot`. It runs before any device is created, so an
/// extraction error creates none.
///
/// # Errors
///
/// The error of `extract`, unchanged.
fn draw(snapshot: &Snapshot, extract: Extract) -> Result<Drawn> {
    let mut changes = Vec::new();
    for (_, object) in snapshot.iter() {
        let items = extract(snapshot, object)?;
        if !items.is_empty() {
            changes.push((object.id, items));
        }
    }
    let extents = extents(&changes);
    Ok(Drawn { changes, extents })
}

/// Renders the default view of `snapshot` at `width` by `height` pixels, drawing what `extract`
/// returns for each object, on a new device.
///
/// # Errors
///
/// The error of `extract`, the `gpu.*` errors of adapter selection and rendering, and
/// `cmd.path.io` when the PNG cannot be encoded.
pub fn render(snapshot: &Snapshot, extract: Extract, width: u32, height: u32) -> Result<Rendered> {
    let drawn = draw(snapshot, extract)?;
    let gpu = Gpu::new(&GpuSettings::default())?;
    paint(&gpu, drawn, width, height)
}

/// Draws `drawn` on `gpu` as the default view at `width` by `height` pixels, and encodes it.
fn paint(gpu: &Gpu, drawn: Drawn, width: u32, height: u32) -> Result<Rendered> {
    let Drawn { changes, extents } = drawn;
    let objects = changes.len();
    let mut scene = Scene::new(gpu, SceneLimits::DEFAULT)?;
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
    let image = Renderer::new(gpu).render_to_image(&mut scene, &frame, width, height, &settings)?;
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
///
/// The renderer keeps one device, created by the first render that needs it, and renders one image
/// at a time: a render waits for the one in progress. A failed device creation is not kept; the
/// next render tries again.
#[derive(Debug)]
pub struct SnapshotRenderer {
    extract: Extract,
    /// The device between renders. A render takes it out, so the lock also serializes renders; a
    /// render that fails or panics leaves none, and the next one creates a device.
    gpu: Mutex<Option<Gpu>>,
    /// The devices created, for the tests.
    #[cfg(test)]
    devices: AtomicUsize,
}

impl SnapshotRenderer {
    /// A renderer drawing with `extract`.
    #[must_use]
    pub fn new(extract: Extract) -> Self {
        Self {
            extract,
            gpu: Mutex::new(None),
            #[cfg(test)]
            devices: AtomicUsize::new(0),
        }
    }

    /// The number of devices this renderer created.
    #[cfg(test)]
    fn devices_created(&self) -> usize {
        self.devices.load(Ordering::SeqCst)
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
        // The lock covers the extraction too, so renders never overlap; the device is created only
        // after the extraction succeeded.
        let mut slot = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        let drawn = draw(request.snapshot, self.extract)?;
        let gpu = match slot.take() {
            Some(gpu) => gpu,
            None => {
                let gpu = Gpu::new(&GpuSettings::default())?;
                #[cfg(test)]
                self.devices.fetch_add(1, Ordering::SeqCst);
                gpu
            }
        };
        let rendered = paint(&gpu, drawn, request.width, request.height);
        *slot = Some(gpu);
        Ok(rendered?.png)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use arc_base::id::Identifier;
    use arc_command::{ViewRenderer, ViewRequest};
    use arc_doc::graph::Evaluators;
    use arc_doc::{Document, Header, Schema, SchemaVersion, TypeDecl, TypeKey, TypeRegistry};

    use super::*;

    /// Renders in flight at the same time, and the most there were; the extraction runs inside
    /// every render.
    static INSIDE: AtomicUsize = AtomicUsize::new(0);
    static MOST_INSIDE: AtomicUsize = AtomicUsize::new(0);

    /// Holds each render inside the extraction for a bounded number of yields, so a second render
    /// that is not serialized with the first overlaps it.
    fn probe(_: &Snapshot, _: &Object) -> Result<Vec<RenderItem>> {
        let inside = INSIDE.fetch_add(1, Ordering::SeqCst) + 1;
        MOST_INSIDE.fetch_max(inside, Ordering::SeqCst);
        for _ in 0..200_000 {
            if INSIDE.load(Ordering::SeqCst) > 1 {
                break;
            }
            std::thread::yield_now();
        }
        INSIDE.fetch_sub(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn snapshot_with_one_object() -> Snapshot {
        let mut types = TypeRegistry::new();
        let schema = Schema::new(SchemaVersion::new(1, 0), Vec::new());
        types
            .register(TypeDecl::new(TypeKey::new("test.point").unwrap(), schema))
            .unwrap();
        let document = Document::new(Arc::new(types), Arc::new(Evaluators::new())).unwrap();
        document
            .submit(|txn| {
                txn.create(
                    "test.point",
                    ObjectId::from_u128(1),
                    Header::default(),
                    Vec::new(),
                )
                .map(|_| ())
            })
            .wait()
            .unwrap();
        document.snapshot()
    }

    fn request(snapshot: &Snapshot) -> ViewRequest<'_> {
        ViewRequest {
            snapshot,
            view: DEFAULT_VIEW,
            width: 16,
            height: 16,
            display_mode: SHADED,
            camera: None,
            sections: &[],
        }
    }

    #[test]
    fn two_renders_never_overlap_and_share_one_device() {
        let snapshot = snapshot_with_one_object();
        let renderer = SnapshotRenderer::new(probe);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    barrier.wait();
                    renderer.render(&request(&snapshot)).unwrap();
                });
            }
        });
        assert_eq!(
            MOST_INSIDE.load(Ordering::SeqCst),
            1,
            "one render at a time"
        );
        assert_eq!(renderer.devices_created(), 1, "the device is created once");
        renderer.render(&request(&snapshot)).unwrap();
        assert_eq!(renderer.devices_created(), 1, "a later render reuses it");
    }

    fn failing(_: &Snapshot, _: &Object) -> Result<Vec<RenderItem>> {
        Err(Error::new(command_codes::RENDER_UNSUPPORTED).with_detail("feature", "extraction"))
    }

    #[test]
    fn an_extraction_error_is_returned_and_no_device_is_created() {
        let snapshot = snapshot_with_one_object();
        let renderer = SnapshotRenderer::new(failing);
        let error = renderer.render(&request(&snapshot)).unwrap_err();
        assert_eq!(error.code(), "cmd.render.unsupported");
        assert_eq!(renderer.devices_created(), 0);
        assert!(
            render(&snapshot, failing, 16, 16).is_err(),
            "the free function returns the error too"
        );
    }
}
