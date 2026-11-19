//! The call into the wrapped library. Nothing outside this module names a library type.

use std::io::{Cursor, ErrorKind};
use std::panic::{AssertUnwindSafe, catch_unwind};

use acadrust::{CadDocument, DwgReadOptions, DwgReader, DxfError};
use arc_base::{Error, Limits, Result};

use crate::error::codes;

/// Reads a DWG document from bytes with the library.
///
/// The decompression budget is the caller's `decompressed_bytes`. A panic inside the library is
/// caught and reported as `io.dwg.library_failed`. Aborts, stack overflows and memory use inside
/// the library are not caught; see the specification.
pub(crate) fn read(bytes: &[u8], signature: &str, limits: &Limits) -> Result<CadDocument> {
    let budget = usize::try_from(limits.decompressed_bytes).unwrap_or(usize::MAX);
    let options = DwgReadOptions::default().with_max_decompressed_bytes(budget);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let mut reader = DwgReader::from_stream_with_options(Cursor::new(bytes), options);
        reader.read()
    }));
    match outcome {
        Ok(Ok(document)) => Ok(document),
        Ok(Err(error)) => Err(library_error(&error, signature)),
        Err(_) => Err(Error::new(codes::LIBRARY_FAILED)),
    }
}

/// Maps a library error to a code of this crate.
fn library_error(error: &DxfError, signature: &str) -> Error {
    match error {
        DxfError::Io(io) if io.kind() == ErrorKind::UnexpectedEof => Error::new(codes::TRUNCATED),
        DxfError::UnsupportedVersion(_) => {
            Error::new(codes::VERSION_UNSUPPORTED).with_arg("version", signature.to_owned())
        }
        DxfError::ChecksumMismatch { .. } => Error::new(codes::CHECKSUM_MISMATCH),
        // The library refuses a section that would exceed the decompression budget with an
        // `InvalidFormat` message that names the budget. Its text is the only signal, which the
        // tests of this crate check against the pinned version.
        DxfError::InvalidFormat(message) if message.contains("budget") => {
            Error::new(arc_io_dxf::error::codes::LIMIT_EXCEEDED)
                .with_arg("limit", "decompressed_bytes")
        }
        _ => Error::new(codes::STRUCTURE_INVALID),
    }
}
