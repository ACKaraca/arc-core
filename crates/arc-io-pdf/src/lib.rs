//! Deterministic SVG and PDF writers for drafting plot streams (CORE-IO-037, CORE-IO-038).
//!
//! [`write_svg`] writes one page as an SVG document; [`write_pdf`] writes one PDF page per
//! stream. Both take an [`arc_draft::plot::PlotStream`] (page in millimeters, items in drawing
//! order) and return an [`Output`]: the file bytes and a [`LossReport`] that is present also
//! when nothing was lost. Both enforce the caller's [`arc_base::Limits`], check the
//! [`arc_base::Cancel`] token before every item and report progress.
//!
//! [`register_commands`] adds the batch plotting command `core.draft.plot`, which writes files
//! only through the command context.
//!
//! The writers produce byte buffers, never paths, and make no operating-system calls. No function
//! in this crate panics. The rules are in `docs/crates/arc-io.md`, section "SVG and PDF output
//! as implemented".
//!
//! ```
//! use arc_base::Color;
//! use arc_base::math::Point2;
//! use arc_draft::plot::PlotStream;
//! use arc_io_pdf::{WriteOptions, write_pdf, write_svg};
//! let stream = PlotStream {
//!     width: 210.0,
//!     height: 297.0,
//!     paper_color: Color::WHITE,
//!     area: [Point2::new(0.0, 0.0), Point2::new(210.0, 297.0)],
//!     clips: vec![],
//!     items: vec![],
//!     failed: vec![],
//! };
//! let svg = write_svg(&stream, &mut WriteOptions::default())?;
//! assert!(svg.bytes.starts_with(b"<?xml"));
//! let pdf = write_pdf(&[&stream], &mut WriteOptions::default())?;
//! assert!(pdf.bytes.starts_with(b"%PDF-1.7"));
//! assert!(!pdf.report.has_loss());
//! # Ok::<(), arc_base::Error>(())
//! ```

mod command;
mod common;
pub mod error;
mod font;
pub mod loss;
mod path;
mod pdf;
mod svg;

pub use command::{
    AreaName, Format, MAX_LAYOUTS, Message, ModelSource, Overrides, PLOT_DESCRIPTION, PLOT_LABEL,
    PlotParams, PlotResult, numbered_path, register_commands,
};
pub use common::{DEFAULT_CHORD_MM, HAIRLINE_MM, Output, Progress, WriteOptions};
pub use error::{codes, reasons};
pub use loss::{LossItem, LossReport, Outcome, Severity};
pub use pdf::write_pdf;
pub use svg::write_svg;
