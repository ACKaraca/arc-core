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
//! - external drawing references to linked drawings, read-only, with clip and bind (CORE-DRAFT-025,
//!   [`external`]);
//! - hatches with islands, associativity, pattern line generation and the Arc hatch-pattern
//!   file format and library (CORE-DRAFT-003 and 020, [`hatch`], [`hatch_pattern`]);
//! - spline entities (CORE-DRAFT-002, [`spline`]);
//! - fields in text (CORE-DRAFT-007, [`fields`]), tables with merged cells and formulas
//!   (CORE-DRAFT-008, [`table`]) and reference arrays (CORE-DRAFT-024, [`array`]);
//! - plot style tables (CORE-DRAFT-030, [`plot_style`]), plot streams (CORE-DRAFT-031, [`plot`])
//!   and, with the feature `raster`, raster output (CORE-DRAFT-032, `raster`);
//! - the `core.draft.*` commands ([`register_commands`]).
//!
//! No function in this crate panics.

pub mod annotation;
mod annotation_commands;
mod annotation_display;
pub mod appearance;
pub mod array;
mod bind;
pub mod clip;
pub mod commands;
pub mod dimension;
pub mod display;
pub mod error;
pub mod external;
pub mod external_commands;
pub mod fields;
pub mod fonts;
pub mod format;
pub mod hatch;
mod hatch_commands;
pub mod hatch_pattern;
pub mod layout;
pub mod leader;
pub mod linetype;
mod lock;
pub mod order;
#[cfg(feature = "outline-text")]
mod outline;
pub mod plot;
pub mod plot_style;
pub mod purge;
#[cfg(feature = "raster")]
pub mod raster;
pub mod spline;
pub mod stroke_font;
pub mod style;
pub mod table;
mod table_commands;
pub mod text;
pub mod text_layout;
mod transform;
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
