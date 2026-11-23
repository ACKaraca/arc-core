//! Derived drawings of 3D models for the Arc suite (first part: plan views).
//!
//! A product describes a view with a [`ViewDef`] (direction, cut plane and ranges, crop, detail
//! level, crease angle, scale, overrides and bounds) and supplies its objects through the
//! [`ViewSource`] trait: each object's parts (each a closed planar solid or a triangle mesh with
//! a stable key, a cut fill and a join key), its 2D symbols, its category and a line style per
//! [`VisibilityClass`]. [`build_view`]
//! returns a [`ViewResult`]: `arc-draft` display primitives in view coordinates, a back-link per
//! primitive to its source object, sub-object key and class, and diagnostics for objects whose
//! section is open or whose geometry could not be used.
//!
//! The algorithms are polyhedral and exact up to rounding and the linear tolerance of the
//! caller's [`arc_base::Tolerance`]: plane sections chained by mesh topology, region unions from
//! `arc-geom` for joining, and hidden-line removal of every edge against every occluding
//! triangle, each giving one exact parameter interval. Every count and every occlusion test is
//! bounded by [`ViewLimits`].
//!
//! ```
//! use arc_base::{Cancel, Tolerance};
//! use arc_view::{PlanRange, ViewDef, build_view, ViewSource, SourceObject};
//! # use arc_base::{ObjectId, Result};
//! struct Empty;
//! impl ViewSource for Empty {
//!     fn objects(&self, _: &ViewDef) -> Result<Vec<ObjectId>> { Ok(Vec::new()) }
//!     fn object(&self, _: ObjectId, _: &ViewDef) -> Result<Option<SourceObject>> { Ok(None) }
//! }
//! let def = ViewDef::plan(PlanRange::new(1.2, 0.0, Some(3.0)));
//! let view = build_view(&def, &Empty, &Tolerance::DEFAULT, &Cancel::new())?;
//! assert!(view.list.primitives.is_empty());
//! # Ok::<(), arc_base::Error>(())
//! ```
//!
//! No function in this crate panics.

mod build;
pub mod def;
pub mod error;
mod hidden;
mod merge;
mod mesh;
pub mod result;
pub mod source;

pub use build::build_view;
pub use def::{
    DetailLevel, PlanRange, SHEET_ERROR, ViewDef, ViewDirection, ViewLimits, crop_polygon,
    crop_rectangle,
};
pub use result::{BackLink, Diagnostic, PrimitiveKind, ViewResult, VisibilityClass};
pub use source::{
    ClassStyles, CutFill, Geometry, GraphicOverride, LineStyle, Overrides, Part, SourceObject,
    Symbol, ViewSource,
};
