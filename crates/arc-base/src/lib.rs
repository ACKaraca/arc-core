//! The L0 vocabulary every Arc crate and product uses: identifiers, units and quantities,
//! tolerances, hostile-input limits, cancellation, the error model, color, math aliases and the
//! coordinate reference record.
//!
//! This crate makes no operating-system calls: time and randomness reach it through the traits in
//! [`id`], so it behaves identically on every target, including WebAssembly. It formats no
//! user-facing sentences; errors carry message identifiers that the application resolves.
//!
//! No function in this crate panics.

pub mod cancel;
pub mod color;
pub mod crs;
pub mod display;
pub mod error;
pub mod id;
pub mod limits;
pub mod math;
pub mod quantity;
pub mod tolerance;
pub mod unit;

pub use cancel::Cancel;
pub use color::Color;
pub use crs::CrsRecord;
pub use display::{DisplayFormat, DisplayUnits};
pub use error::{Category, Error, ErrorCode, Result};
pub use id::{DocumentId, IdGenerator, ObjectId, SessionId, TransactionId};
pub use limits::Limits;
pub use quantity::{Dimension, Quantity};
pub use tolerance::Tolerance;
pub use unit::Unit;
