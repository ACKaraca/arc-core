//! Mapping between the neutral DXF drawing model of `arc-io-dxf` and the drafting model of
//! `arc-draft`.
//!
//! - [`plan::plan`] turns a [`arc_io_dxf::Drawing`] into the creation operations of drafting
//!   objects and a loss report; [`apply::apply`] creates them in the transaction of a command.
//! - [`export::export_drawing`] turns a model space with its tables and blocks into a
//!   [`arc_io_dxf::Drawing`].
//! - [`commands::register_commands`] registers `core.draft.import.dxf` and
//!   `core.draft.export.dxf`.
//!
//! No function in this crate panics.

pub mod apply;
pub mod color;
pub mod commands;
mod depth;
pub mod error;
pub mod export;
mod export_entities;
pub mod fields;
mod geo;
mod map_entities;
mod map_hatch;
mod map_tables;
mod map_text;
mod mapper;
pub mod plan;
pub mod report;
pub mod units;

pub use apply::apply;
pub use commands::register_commands;
pub use export::export_drawing;
pub use plan::{ImportOptions, Plan, plan, plan_with_report};
