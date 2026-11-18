//! The 2D drafting model of the Arc suite.
//!
//! This first part provides:
//!
//! - the drafting object types registered through the `arc-doc` type registry
//!   ([`register_types`], [`types`]): spaces with declared units (CORE-DRAFT-001), curve
//!   entities with exact `arc-geom` geometry (CORE-DRAFT-002) carrying elevation, thickness and
//!   extrusion (CORE-DRAFT-009), layers (CORE-DRAFT-016), linetypes (CORE-DRAFT-018), block
//!   definitions and references (CORE-DRAFT-022);
//! - draw-order keys and their placement rules (CORE-DRAFT-005, [`order`]);
//! - appearance resolution as a pure function (CORE-DRAFT-017, [`appearance`]);
//! - linetype dash generation as a pure function (CORE-DRAFT-018, [`linetype`]);
//! - the nesting bound and cycle rejection of block definitions as commit validators
//!   (CORE-DRAFT-023, [`validate`]);
//! - the `core.draft.*` commands ([`register_commands`]).
//!
//! Splines wait for the NURBS curves of `arc-geom`. No function in this crate panics.

pub mod appearance;
pub mod commands;
pub mod error;
pub mod linetype;
pub mod order;
pub mod types;
pub mod validate;

pub use commands::register_commands;
pub use types::register_types;
pub use validate::install;
