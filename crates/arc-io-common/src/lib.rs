//! Shared bounded readers for the ZIP- and XML-based interchange formats: a ZIP archive reader
//! over a byte buffer ([`zip`]) and an XML pull reader over `quick-xml` ([`xml`]).
//!
//! Format crates use these readers instead of depending on each other (ADR-0001, amendment
//! "Shared bounded ZIP and XML readers"). Both readers take byte buffers and never paths. Every
//! length, count and offset from the input is checked against the caller's [`arc_base::Limits`]
//! before it is read or allocated, hostile input returns a stable `io.zip.*`, `io.xml.*` or
//! `io.limit_exceeded` error, a [`arc_base::Cancel`] token is checked at a fixed interval, and
//! nothing panics.
//!
//! The loss report types ([`loss`]) are shared by the format crates in the same way.
//!
//! The crate has no product type and makes no operating-system call. It builds for
//! `wasm32-unknown-unknown`.

mod bytes;
pub mod crc32;
pub mod error;
pub mod loss;
mod options;
pub mod xml;
pub mod zip;

pub use options::ReadOptions;
