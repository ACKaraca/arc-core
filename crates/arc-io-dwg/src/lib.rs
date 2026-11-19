//! A bounded DWG reader over a byte-level wrapper, mapped to the neutral drawing model of
//! `arc-io-dxf`.
//!
//! [`read`] takes the bytes of a DWG file of versions R13 to R2018 and returns the same
//! [`arc_io_dxf::Drawing`] and [`arc_io_dxf::LossReport`] that the DXF reader produces. The file is
//! read by the library `acadrust`, which is never part of this crate's interface: no type of it
//! appears in a signature, a result or an error.
//!
//! Before the library sees the bytes, the input size is checked against [`arc_base::Limits`] and
//! the version signature against the supported versions. Anything the library reads and the model
//! does not carry is counted in the loss report; nothing is dropped silently.
//!
//! The crate reads byte buffers, never paths, and makes no operating-system calls of its own. The
//! library's own buffers are bounded by the decompression limit of the profile; the call into the
//! library cannot be cancelled.

mod check;
mod convert;
mod entity;
pub mod error;
mod library;
mod loss;
mod map;

pub use error::{codes, reasons};

use arc_base::Result;
use arc_io_dxf::loss::{Direction, LossHeader};
use arc_io_dxf::{ReadOptions, ReadOutput};

/// Reads a DWG file from bytes into a drawing and its loss report.
///
/// The options are those of the DXF reader: the limits profile, the cancellation token and the
/// progress callback. Progress is reported while the drawing is mapped.
///
/// # Errors
///
/// `io.limit_exceeded` naming the limit, `io.dwg.signature_invalid` for input without a DWG
/// signature, `io.dwg.version_unsupported` for a version outside R13 to R2018, `io.dwg.truncated`,
/// `io.dwg.structure_invalid`, `io.dwg.checksum_mismatch`, `io.dwg.library_failed` when the
/// wrapped library panics, and `base.cancelled`.
pub fn read(bytes: &[u8], options: &mut ReadOptions<'_>) -> Result<ReadOutput> {
    let limits = options.limits;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if size > limits.input_bytes {
        return Err(error::limit("input_bytes", size));
    }
    let (version, signature) = check::signature(bytes)?;
    options.cancel.check()?;
    let document = library::read(bytes, signature, &limits)?;
    options.cancel.check()?;
    let total = u64::try_from(document.entity_count()).unwrap_or(u64::MAX);
    let mut mapper = map::Mapper::new(
        &limits,
        &options.cancel,
        options.progress.as_deref_mut(),
        total,
    );
    let drawing = mapper.drawing(&document, version)?;
    let report = mapper.finish(LossHeader {
        format: "dwg".to_owned(),
        version: version.name().to_owned(),
        direction: Direction::Import,
        file_name: None,
        file_hash: None,
        application: None,
        application_version: None,
        time: None,
        limits: Some(limits),
    });
    Ok(ReadOutput { drawing, report })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_input_is_refused_before_the_library() {
        let mut options = ReadOptions::with_limits(arc_base::Limits {
            input_bytes: 4,
            ..arc_base::Limits::CONSTRAINED
        });
        let err = read(b"AC1032 and more", &mut options).err();
        assert_eq!(err.map(|e| e.code()), Some("io.limit_exceeded"));
    }
}
