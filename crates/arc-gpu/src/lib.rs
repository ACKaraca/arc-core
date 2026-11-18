//! The real-time viewport renderer of the Arc suite on `wgpu`.
//!
//! This first part provides adapter selection with fallbacks and a reported feature set
//! (CORE-GPU-001), a retained viewport scene keyed by object identifier (CORE-GPU-002) on
//! pooled GPU buffers with compaction and per-category memory reports (CORE-GPU-006),
//! camera-relative precision (CORE-GPU-004), instanced meshes with reversed-Z depth
//! (CORE-GPU-007), exact-width lines with round and square caps and round, miter and bevel
//! joins (CORE-GPU-008), and headless rendering to images (CORE-GPU-031).
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
pub mod pool;
pub mod render;
pub mod scene;

pub use adapter::{AdapterKind, AdapterReport, BackendKind, Gpu, GpuSettings, ReducedFeature};
pub use pool::PoolUsage;
pub use render::{Image, Pass, RenderSettings, Renderer};
pub use scene::{
    GeometryId, ItemKey, LineCap, LineItem, LineJoin, LinePath, LineStyle, LineWidth, MemoryReport,
    MeshInstance, MeshItem, RenderItem, Scene, SceneLimits, WriteCounters,
};
/// The pinned `wgpu` version, re-exported for products that add their own passes.
pub use wgpu;
