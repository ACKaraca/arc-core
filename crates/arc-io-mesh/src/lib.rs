//! Bounded readers and deterministic writers for four triangle-mesh formats over a neutral
//! scene: Wavefront OBJ with its MTL material library, STL (ASCII and binary), PLY (ASCII,
//! binary little endian and binary big endian), and glTF 2.0 with its GLB container, whose scene
//! hierarchy the scene keeps as well.
//!
//! Each format is a module with a `read` and a `write` function. Readers take byte buffers and
//! never paths; the caller supplies the MTL bytes of an OBJ file. Every count, index, line length
//! and total size is checked against the caller's [`arc_base::Limits`] before anything is
//! allocated, malformed input returns a stable `io.mesh.*` code, and nothing panics. Writers
//! produce identical bytes for identical scenes and options. Every read and write returns a
//! [`LossReport`] that says what the format could not keep.
//!
//! The formats are implemented here from their published descriptions; the one third-party
//! dependency is `serde_json`, which parses the glTF JSON; the loss report types are shared through
//! `arc-io-common`.
//!
//! ```
//! use arc_base::{Limits, math::Point3};
//! use arc_io_mesh::{MeshPart, MeshScene, ReadOptions, WriteOptions, stl};
//! use arc_mesh::TriMesh;
//!
//! let positions = vec![
//!     Point3::new(0.0, 0.0, 0.0),
//!     Point3::new(1.0, 0.0, 0.0),
//!     Point3::new(0.0, 1.0, 0.0),
//! ];
//! let mesh = TriMesh::new(positions, vec![[0, 1, 2]], &Limits::DEFAULT)?;
//! let scene = MeshScene {
//!     meshes: vec![MeshPart { name: String::new(), mesh, material: None, preserved: Default::default() }],
//!     materials: Vec::new(),
//!     ..MeshScene::default()
//! };
//! let written = stl::write(&scene, &mut WriteOptions::default())?;
//! let read = stl::read(&written.bytes, &mut ReadOptions::default())?;
//! assert_eq!(read.scene.meshes[0].mesh.triangle_count(), 1);
//! # Ok::<(), arc_base::Error>(())
//! ```

mod binary;
pub mod error;
pub mod gltf;
pub mod loss;
pub mod model;
pub mod obj;
pub mod options;
pub mod ply;
pub mod stl;
mod text;

pub use error::{codes, reasons};
pub use loss::{LossItem, LossReport, Outcome, Severity};
pub use model::{
    AlphaMode, Camera, ImageSource, Light, LightKind, Material, MeshGroup, MeshPart, MeshScene,
    Node, Preserved, Projection, TextureMap, Transform,
};
pub use options::{Encoding, Progress, ReadOptions, ReadOutput, WriteOptions, WriteOutput};
