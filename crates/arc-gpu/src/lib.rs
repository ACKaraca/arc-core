//! The real-time viewport renderer of the Arc suite on `wgpu`.
//!
//! This first part provides adapter selection with fallbacks and a reported feature set
//! (CORE-GPU-001), a retained viewport scene keyed by object identifier (CORE-GPU-002) on
//! pooled GPU buffers with compaction and per-category memory reports (CORE-GPU-006),
//! camera-relative precision (CORE-GPU-004), instanced meshes with reversed-Z depth
//! (CORE-GPU-007), exact-width lines with round and square caps and round, miter and bevel
//! joins (CORE-GPU-008), and headless rendering to images (CORE-GPU-031).
//!
//! The second part adds shading modes with feature edges extracted once per geometry
//! (CORE-GPU-014), the identifier pass for picking a pixel or a rectangle (CORE-GPU-022),
//! pre-selection and selection highlights with outline and tint (CORE-GPU-023), overlays drawn
//! above the scene (CORE-GPU-025) and `f64` navigation functions (CORE-GPU-027).
//!
//! The third part adds incremental extraction from document snapshots on a worker pool
//! (CORE-GPU-003) and viewports rendered on their own thread (CORE-GPU-029) that draw only when
//! an input changed (CORE-GPU-030).
//!
//! Typical use: create a [`Gpu`] from [`GpuSettings`], a [`Scene`] on it, upload geometry with
//! [`Scene::add_geometry`], set items with [`Scene::update`], and render with
//! [`Renderer::render_to_image`].
//!
//! Device errors never panic: validation and out-of-memory errors are captured in error scopes
//! and returned as `gpu.validation` and `gpu.out_of_memory`; device loss is `gpu.device.lost`.
//! Every wait on the device is bounded.

mod block;

pub mod adapter;
pub mod camera;
pub mod error;
pub mod extract;
pub mod navigation;
pub mod overlay;
pub mod pool;
pub mod render;
pub mod scene;
pub mod viewport;

pub use adapter::{AdapterKind, AdapterReport, BackendKind, Gpu, GpuSettings, ReducedFeature};
pub use extract::{
    ApplyReport, Extraction, ExtractionDriver, ExtractionResult, Extractor, ExtractorRegistry,
    Outcome,
};
pub use overlay::{Anchor, Grid, MarkerShape, Overlay, OverlayPrimitive};
pub use pool::PoolUsage;
pub use render::{
    FrameExtras, Image, Pass, PickHit, PickRegion, RenderSettings, Renderer, ShadingMode,
};
pub use scene::{
    FEATURE_EDGE_ANGLE, GeometryId, ItemKey, LineCap, LineItem, LineJoin, LinePath, LineStyle,
    LineWidth, MemoryReport, MeshInstance, MeshItem, RenderItem, Scene, SceneLimits, WriteCounters,
};
pub use viewport::{DirtyReasons, Frame, Viewport, ViewportConfig};
/// The pinned `wgpu` version, re-exported for products that add their own passes.
pub use wgpu;
