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
//! - wipeouts (CORE-DRAFT-004), layouts (CORE-DRAFT-027, [`layout`]) and sheet viewports
//!   (CORE-DRAFT-028) with their mapping, clip and layer overrides;
//! - text, dimension, leader and table styles with their update and deletion rules
//!   (CORE-DRAFT-019, [`style`]) and the purge of unused objects (CORE-DRAFT-021, [`purge`]);
//! - display lists of exact primitives built per entity in the dependency graph and assembled
//!   for a space or viewport (CORE-DRAFT-029, [`display`], [`register_evaluators`]);
//! - single-line and rich text (CORE-DRAFT-006, [`text`]), their layout for viewport, plot and
//!   export (CORE-DRAFT-034, [`text_layout`]), the Arc stroke-font format and the shipped font
//!   (CORE-DRAFT-035, [`stroke_font`]), and font providers with substitution (CORE-DRAFT-036,
//!   [`fonts`]); outline fonts lay out with `parley` under the feature `outline-text`;
//! - dimensions with measurement formatting, associativity through sub-object keys and
//!   dimension chains (CORE-DRAFT-010 to 012, [`dimension`], [`format`]), leaders and
//!   multi-leaders (CORE-DRAFT-013, [`leader`]) and annotation scaling (CORE-DRAFT-014,
//!   [`annotation`]);
//! - the `core.draft.*` commands ([`register_commands`]).
//!
//! Splines wait for the NURBS curves of `arc-geom`. No function in this crate panics.

pub mod annotation;
mod annotation_commands;
mod annotation_display;
pub mod appearance;
pub mod clip;
pub mod commands;
pub mod dimension;
pub mod display;
pub mod error;
pub mod fonts;
pub mod format;
pub mod layout;
pub mod leader;
pub mod linetype;
pub mod order;
#[cfg(feature = "outline-text")]
mod outline;
pub mod purge;
pub mod stroke_font;
pub mod style;
pub mod text;
pub mod text_layout;
pub mod types;
pub mod validate;

pub use commands::register_commands;
pub use types::register_types;
pub use validate::install;

/// Registers the derived nodes of the drafting types: the display list of every entity
/// ([`display::DISPLAY`]) with `options`, the resolution of every dimension
/// ([`dimension::RESOLVED`]), and the effective data of every style
/// ([`style::EFFECTIVE`]).
///
/// # Errors
///
/// `doc.graph.duplicate_key` when one is registered already.
pub fn register_evaluators(
    evaluators: &mut arc_doc::graph::Evaluators,
    options: display::DisplayOptions,
) -> arc_base::Result<()> {
    display::register_evaluators(evaluators, options)?;
    style::register_evaluators(evaluators)
}
