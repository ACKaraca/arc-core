//! A bounded DXF reader and deterministic writer over a neutral drawing model.
//!
//! [`read`] takes ASCII or binary DXF bytes of versions R12 to 2018 and returns a [`Drawing`]
//! with a [`LossReport`]; [`write`] produces ASCII or binary DXF of versions 2000 to 2018 from a
//! drawing, also with a report. Records the model does not type are kept as preserved records
//! and preserved groups and written back unchanged, so a file read and written again keeps them.
//! Every read enforces the caller's [`arc_base::Limits`] profile before allocating and stops on
//! cancellation; both directions report progress.
//!
//! The crate reads and writes byte buffers and streams, never paths, and makes no
//! operating-system calls. No function in this crate panics.
//!
//! ```
//! use arc_io_dxf::{ReadOptions, WriteOptions, read, write};
//! let text = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n1\n21\n1\n31\n0\n0\nENDSEC\n0\nEOF\n";
//! let input = read(text.as_bytes(), &mut ReadOptions::default())?;
//! assert_eq!(input.drawing.entities.len(), 1);
//! let output = write(&input.drawing, &mut WriteOptions::default())?;
//! let again = read(&output.bytes, &mut ReadOptions::default())?;
//! assert_eq!(again.drawing.entities[0].kind, input.drawing.entities[0].kind);
//! # Ok::<(), arc_base::Error>(())
//! ```

pub mod encoding;
pub mod error;
mod fields;
pub mod group;
pub mod loss;
pub mod model;
mod read;
mod token;
mod write;

pub use error::{Position, codes, reasons};
pub use group::{Group, Handle, Value, ValueKind, Version, value_kind};
pub use loss::{LossItem, LossReport, Outcome, Severity};
pub use model::{Drawing, Entity, EntityKind, Object, ObjectKind, PreservedRecord};
pub use read::{Progress, ReadOptions, ReadOutput, read, read_stream};
pub use write::{WriteOptions, WriteOutput, value_writable, write};
