//! Bounded reader for IFC files (CORE-IO-014, first part): the STEP physical file of ISO 10303-21
//! (clear text encoding) and an IFC-ZIP archive holding one, read into a neutral entity model and
//! typed views of the spatial structure, the element types, the property sets and the quantity
//! sets.
//!
//! [`read`] takes byte buffers and never paths. It returns an [`IfcFile`] with every instance,
//! indexed by name and by entity type, and a [`LossReport`] that counts the entity types the views
//! do not read as preserved. [`IfcFile::typed_views`] builds the typed views. Geometry evaluation
//! (CORE-IO-015) is [`IfcFile::geometry`]. [`write()`](fn@write) writes an IFC4, IFC4X3 or IFC2X3 file from the neutral
//! description of a building in [`write`](mod@write) (CORE-IO-016, first part).
//!
//! Every count, string, nesting level and instance is checked against the caller's
//! [`arc_base::Limits`] before it is stored. Malformed input returns a stable `io.ifc.*` code,
//! cancellation returns `base.cancelled`, and nothing panics.
//!
//! ```
//! use arc_base::Cancel;
//! use arc_base::Limits;
//! use arc_io_ifc::{ReadOptions, read};
//!
//! let file = b"ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((),'2;1');\n\
//!     FILE_NAME('a.ifc','',(),(),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n\
//!     #1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Demo',$,$,$,$,$,$);\nENDSEC;\n\
//!     END-ISO-10303-21;\n";
//! let output = read(file, &mut ReadOptions::default())?;
//! assert_eq!(output.file.instance(1).map(|i| i.records.len()), Some(1));
//! let views = output.file.typed_views(&Limits::CONSTRAINED, &Cancel::new())?;
//! assert_eq!(views.spatial[0].entity.name.as_deref(), Some("Demo"));
//! # Ok::<(), arc_base::Error>(())
//! ```

pub mod alignment;
mod edit;
pub mod error;
pub mod geometry;
pub mod ids;
pub mod loss;
pub mod model;
pub mod options;
pub mod psets;
mod read;
pub mod schema;
mod step;
pub mod stream;
mod text;
pub mod views;
pub mod write;

pub use error::{codes, reasons, rules};
pub use geometry::{Geometry, GeometryItem, GeometryOptions, Shape, Units};
pub use loss::{LossItem, LossReport, Outcome, Severity};
pub use model::{IfcFile, Instance, Parameter, Record, Schema};
#[cfg(not(target_arch = "wasm32"))]
pub use options::SystemClock;
pub use options::{Clock, Progress, ReadOptions, ReadOutput};
pub use read::read;
pub use schema::{
    Conformance, Finding, InverseIndex, Referrer, RuleCount, RuleScope, Typed, WhereCount,
};
pub use views::{
    ClassCoverage, ClassificationRef, ClassificationSystem, Connection, ConstituentView, CrsView,
    GeoReference, GeoSource, GroupView, ImportViews, LayerUsage, LayerView, MappedView,
    MaterialAssociation, MaterialRef, Matrix, Nominal, Number, ObjectKind, ObjectView, Origin,
    OwnerHistory, PlacementState, PlacementView, ProfileView, Property, PropertySet, PropertyValue,
    Quantity, QuantityKind, QuantitySet, ResolvedProperty, ResolvedPropertySet, ResolvedQuantity,
    ResolvedQuantitySet, RootedEntity, SpaceBoundary, SpatialNode, StyleView, TypeAssignment,
    TypedViews, UnitKind, UnitSystem, UnitView, Value,
};
pub use write::{WriteModel, WriteOptions, WriteOutput, global_id, ifc_zip, write};
