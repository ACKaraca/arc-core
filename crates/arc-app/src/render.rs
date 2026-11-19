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
use arc_gpu::{
    CurveShape, Extraction, ExtractionResult, Gpu, GpuSettings, Outcome, RenderItem,
    RenderSettings, Renderer, Scene, SceneLimits,
};
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

/// The drawn objects of a snapshot as extractions, one per object, for drawings that need
/// geometries (fills, outline text): for example `arc_gpu::draft::space_extractions`. An object
/// that draws nothing is left out. An error stops the render and is returned to its caller.
pub type Drawing = dyn Fn(&Snapshot) -> Result<Vec<(ObjectId, Extraction)>> + Send + Sync;

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

/// The box `(min, max)` around `points`, grown by each point in turn.
fn include(bounds: Option<(Point3, Point3)>, p: Point3) -> Option<(Point3, Point3)> {
    Some(match bounds {
        None => (p, p),
        Some((min, max)) => (min.inf(&p), max.sup(&p)),
    })
}

/// The smallest box around what the objects draw in their own planes: the points of line items,
/// the points of curve items and the exact bounds of their curves (an arc's bulge included). Mesh
/// items are not measured here; [`paint`] adds the bounds of their objects from the scene.
fn extents(objects: &[(ObjectId, Extraction)]) -> Option<(Point3, Point3)> {
    let mut bounds = None;
    for (_, extraction) in objects {
        for item in &extraction.items {
            match item {
                RenderItem::Lines(line) => {
                    for path in &line.paths {
                        for [x, y, z] in &path.points {
                            let p = line.origin
                                + Vector3::new(f64::from(*x), f64::from(*y), f64::from(*z));
                            bounds = include(bounds, p);
                        }
                    }
                }
                RenderItem::Curves(curves) => {
                    for path in &curves.paths {
                        let corners = match &path.shape {
                            CurveShape::Point(p) => vec![*p, *p],
                            CurveShape::Curve(curve) => match curve.bounding_box() {
                                Some(aabb) => vec![aabb.min(), aabb.max()],
                                // Rays and lines are unbounded: they frame nothing.
                                None => Vec::new(),
                            },
                        };
                        for c in corners {
                            let p = curves.origin + Vector3::new(c.x, c.y, 0.0);
                            bounds = include(bounds, p);
                        }
                    }
                }
                RenderItem::Mesh(_) => {}
            }
        }
    }
    bounds
}

/// The view sphere of drawn content, for a view that looks down −z: `bounds` is the box of the
/// curves, lines and points, `sphere` the bounding sphere of the mesh objects. Only the planar
/// extent matters, so the center is the middle of the planar box holding both, and the radius
/// reaches the farthest box corner and the far side of the sphere, measured in the plane. A sphere
/// alone frames itself with its own radius.
fn frame_of(
    bounds: Option<(Point3, Point3)>,
    sphere: Option<(Point3, f64)>,
) -> Option<(Point3, f64)> {
    let mut merged = bounds;
    if let Some((center, radius)) = sphere {
        let planar = Vector3::new(radius, radius, 0.0);
        merged = include(merged, center - planar);
        merged = include(merged, center + planar);
    }
    let (min, max) = merged?;
    let center = min + (max - min) / 2.0;
    let planar = |p: Point3| (p.x - center.x).hypot(p.y - center.y);
    let mut radius: f64 = 0.0;
    if let Some((low, high)) = bounds {
        for corner in [
            low,
            high,
            Point3::new(low.x, high.y, 0.0),
            Point3::new(high.x, low.y, 0.0),
        ] {
            radius = radius.max(planar(corner));
        }
    }
    if let Some((c, r)) = sphere {
        radius = radius.max(planar(c) + r);
    }
    Some((center, radius))
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

/// Runs `extract` on every object of `snapshot` and keeps the objects that draw something. It
/// runs before any device is created, so an extraction error creates none.
///
/// # Errors
///
/// The error of `extract`, unchanged.
fn draw(snapshot: &Snapshot, extract: Extract) -> Result<Vec<(ObjectId, Extraction)>> {
    let mut objects = Vec::new();
    for (_, object) in snapshot.iter() {
        let items = extract(snapshot, object)?;
        if !items.is_empty() {
            objects.push((
                object.id,
                Extraction {
                    items,
                    ..Extraction::default()
                },
            ));
        }
    }
    Ok(objects)
}

/// Renders the default view of `snapshot` at `width` by `height` pixels, drawing what `extract`
/// returns for each object, on a new device.
///
/// # Errors
///
/// The error of `extract`, the `gpu.*` errors of adapter selection and rendering, and
/// `cmd.path.io` when the PNG cannot be encoded.
pub fn render(snapshot: &Snapshot, extract: Extract, width: u32, height: u32) -> Result<Rendered> {
    let objects = draw(snapshot, extract)?;
    let gpu = Gpu::new(&GpuSettings::default())?;
    paint(&gpu, objects, width, height)
}

/// Renders the default view of the objects `drawing` returns for `snapshot`, on a new device.
/// The frame covers the exact bounds of curves and the bounds of meshes.
///
/// # Errors
///
/// The error of `drawing`, the `gpu.*` errors of adapter selection and rendering, and
/// `cmd.path.io` when the PNG cannot be encoded.
pub fn render_drawing(
    snapshot: &Snapshot,
    drawing: &Drawing,
    width: u32,
    height: u32,
) -> Result<Rendered> {
    let objects = drawing(snapshot)?;
    let gpu = Gpu::new(&GpuSettings::default())?;
    paint(&gpu, objects, width, height)
}

/// Draws `objects` on `gpu` as the default view at `width` by `height` pixels, and encodes it.
fn paint(
    gpu: &Gpu,
    objects: Vec<(ObjectId, Extraction)>,
    width: u32,
    height: u32,
) -> Result<Rendered> {
    let count = objects.len();
    let bounds = extents(&objects);
    let meshed: Vec<ObjectId> = objects
        .iter()
        .filter(|(_, e)| e.items.iter().any(|i| matches!(i, RenderItem::Mesh(_))))
        .map(|(id, _)| *id)
        .collect();
    let results = objects
        .into_iter()
        .enumerate()
        .map(|(sequence, (object, extraction))| ExtractionResult {
            object,
            revision: 0,
            sequence: sequence as u64,
            outcome: Outcome::Items(extraction),
        })
        .collect();
    let mut scene = Scene::new(gpu, SceneLimits::DEFAULT)?;
    if let Some((_, error)) = scene.apply_extractions(results).failed.into_iter().next() {
        return Err(error);
    }
    let sphere = scene.bounding_sphere(Some(&meshed));
    let aspect = f64::from(width.max(1)) / f64::from(height.max(1));
    let frame = match frame_of(bounds, sphere) {
        Some((center, radius)) => arc_gpu::navigation::zoom_to_sphere(
            &top_view(center, 1.0)?,
            &center,
            radius * MARGIN,
            aspect,
            &Tolerance::DEFAULT,
        )?,
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
        objects: count,
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
pub struct SnapshotRenderer {
    source: Source,
    /// The device between renders. A render takes it out, so the lock also serializes renders; a
    /// render that fails or panics leaves none, and the next one creates a device.
    gpu: Mutex<Option<Gpu>>,
    /// The devices created, for the tests.
    #[cfg(test)]
    devices: AtomicUsize,
}

/// What a snapshot renderer draws: the objects of a per-object function, or a drawing.
enum Source {
    Objects(Extract),
    Drawing(Box<Drawing>),
}

impl Source {
    fn objects(&self, snapshot: &Snapshot) -> Result<Vec<(ObjectId, Extraction)>> {
        match self {
            Self::Objects(extract) => draw(snapshot, *extract),
            Self::Drawing(drawing) => drawing(snapshot),
        }
    }
}

impl std::fmt::Debug for SnapshotRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotRenderer").finish_non_exhaustive()
    }
}

impl SnapshotRenderer {
    /// A renderer drawing with `extract`.
    #[must_use]
    pub fn new(extract: Extract) -> Self {
        Self::from_source(Source::Objects(extract))
    }

    /// A renderer drawing what `drawing` returns: extractions with geometries, such as the
    /// drafting extraction of `arc_gpu::draft`. The closure is called with the snapshot of each
    /// request.
    #[must_use]
    pub fn with_drawing(
        drawing: impl Fn(&Snapshot) -> Result<Vec<(ObjectId, Extraction)>> + Send + Sync + 'static,
    ) -> Self {
        Self::from_source(Source::Drawing(Box::new(drawing)))
    }

    fn from_source(source: Source) -> Self {
        Self {
            source,
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
        let objects = self.source.objects(request.snapshot)?;
        let gpu = match slot.take() {
            Some(gpu) => gpu,
            None => {
                let gpu = Gpu::new(&GpuSettings::default())?;
                #[cfg(test)]
                self.devices.fetch_add(1, Ordering::SeqCst);
                gpu
            }
        };
        let rendered = paint(&gpu, objects, request.width, request.height);
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
    use arc_base::math::Vector2;
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
    fn the_frame_of_a_mesh_sphere_is_that_sphere_and_a_box_keeps_its_half_diagonal() {
        let center = Point3::new(1.0, 2.0, 0.0);
        let (c, r) = frame_of(None, Some((center, 3.0))).unwrap();
        assert!(
            (c - center).norm() < 1e-9 && (r - 3.0).abs() < 1e-9,
            "{c:?} {r}"
        );
        let (c, r) =
            frame_of(Some((Point3::origin(), Point3::new(10.0, 10.0, 0.0))), None).unwrap();
        assert!((c.x - 5.0).abs() < 1e-9 && (c.y - 5.0).abs() < 1e-9);
        assert!((r - 50f64.sqrt()).abs() < 1e-9, "{r}");
    }

    #[test]
    fn extents_of_an_arc_include_its_bulge_and_unbounded_lines_frame_nothing() {
        use arc_base::math::Point2;
        use arc_geom::{Arc2, Curve2, Ray2};
        use arc_gpu::{CurveItem, CurvePath, LineStyle, LineWidth};
        let arc =
            Curve2::Arc(Arc2::new(Point2::origin(), 5.0, 0.0, 1.5 * std::f64::consts::PI).unwrap());
        let ray = Curve2::Ray(Ray2::new(Point2::new(100.0, 100.0), Vector2::x()).unwrap());
        let item = |curve| {
            RenderItem::Curves(CurveItem {
                origin: Point3::origin(),
                paths: vec![CurvePath::curve(curve)],
                style: LineStyle::round(LineWidth::Screen(1.0)),
                color: Color::WHITE,
                pattern: None,
                clip: None,
            })
        };
        let objects = vec![(
            ObjectId::from_u128(1),
            Extraction {
                items: vec![item(arc), item(ray)],
                ..Extraction::default()
            },
        )];
        let (min, max) = extents(&objects).unwrap();
        // The quarter turn missing from the sweep ends at (0, 0) and (5, 0); the sweep reaches
        // (−5, 0), (0, 5) and (0, −5), so the box is the full square of the circle.
        assert!(
            (min.x + 5.0).abs() < 1e-9 && (min.y + 5.0).abs() < 1e-9,
            "{min:?}"
        );
        assert!(
            (max.x - 5.0).abs() < 1e-9 && (max.y - 5.0).abs() < 1e-9,
            "{max:?}"
        );
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
