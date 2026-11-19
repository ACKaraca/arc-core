//! The call into the wrapped library. Nothing outside this module names a library type.

use std::io::{Cursor, ErrorKind};
use std::panic::{AssertUnwindSafe, catch_unwind};

use acadrust::{CadDocument, DwgReadOptions, DwgReader, DxfError};
use arc_base::{Error, Limits, Result};

use crate::error::codes;

/// Reads a DWG document with the library and maps it, inside one isolation boundary.
///
/// The read, the call of `map` on the document and the drop of the library's document all run in
/// [`isolate`]: a panic in any of them is reported as `io.dwg.library_failed`. The value returned
/// is the one `map` produced; no library type leaves this function. Errors of `map` (limits and
/// cancellation) pass through unchanged.
///
/// The decompression budget is the caller's `decompressed_bytes`. MESH records are read with no
/// faces kept, so no memory is reserved for them; see the specification.
pub(crate) fn read<T>(
    bytes: &[u8],
    signature: &str,
    limits: &Limits,
    map: impl FnOnce(&CadDocument) -> Result<T>,
) -> Result<T> {
    isolate(|| {
        let budget = usize::try_from(limits.decompressed_bytes).unwrap_or(usize::MAX);
        let options = DwgReadOptions::default()
            .with_max_decompressed_bytes(budget)
            .with_mesh_limits(0, 0);
        let mut reader = DwgReader::from_stream_with_options(Cursor::new(bytes), options);
        let document = reader
            .read()
            .map_err(|error| library_error(&error, signature))?;
        map(&document)
    })
}

/// Runs `f` and reports a panic inside it as `io.dwg.library_failed`. Every call into the library,
/// and every value taken from it, runs through this function.
pub(crate) fn isolate<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(result) => result,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_inside_the_boundary_is_a_library_failure() {
        let outcome: Result<()> = isolate(|| panic!("a panic in the library"));
        assert_eq!(
            outcome.err().map(|e| e.code()),
            Some("io.dwg.library_failed")
        );
    }

    #[test]
    fn a_panic_while_dropping_inside_the_boundary_is_a_library_failure() {
        struct Bomb;
        impl Drop for Bomb {
            fn drop(&mut self) {
                panic!("a panic in a destructor");
            }
        }
        let outcome: Result<()> = isolate(|| {
            let _bomb = Bomb;
            Ok(())
        });
        assert_eq!(
            outcome.err().map(|e| e.code()),
            Some("io.dwg.library_failed")
        );
    }

    #[test]
    fn errors_of_the_boundary_pass_through_unchanged() {
        let outcome: Result<()> = isolate(|| Err(Error::new(codes::TRUNCATED)));
        assert_eq!(outcome.err().map(|e| e.code()), Some("io.dwg.truncated"));
    }
}
