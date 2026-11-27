//! Writing ZIP archives held in memory: stored or deflated entries, without ZIP64.
//!
//! Every name passes the checks the reader applies, and the output is deterministic: entries keep
//! the given order, the modification time is the earliest DOS time (1980-01-01 00:00) and names are
//! flagged as UTF-8.

use std::collections::BTreeSet;

use arc_base::{Limits, Result};

use super::{FLAG_UTF8, METHOD_DEFLATE, METHOD_STORED, Method, names};
use crate::bytes::to_u64;
use crate::crc32::checksum;
use crate::error::{at_entry, codes, limit};

/// The DEFLATE level: the default of the format's common tools.
const LEVEL: u8 = 6;
/// The version written as "needed to extract": 2.0, deflate.
const VERSION: u16 = 20;
/// The DOS date of 1980-01-01.
const DOS_DATE: u16 = (1 << 5) | 1;

/// Writes an archive holding `entries` (name, data) in order, each with `method`.
///
/// # Errors
///
/// `io.zip.name_invalid`, `io.zip.path_unsafe` or `io.zip.name_duplicate` for a name the reader
/// would refuse; `io.limit_exceeded` for a name longer than `string_bytes`, an entry or archive
/// that needs ZIP64 (4 GiB and more) or more than 65 534 entries.
pub fn write_archive(entries: &[(&str, &[u8])], method: Method) -> Result<Vec<u8>> {
    let count = u16::try_from(entries.len())
        .ok()
        .filter(|&n| n < u16::MAX)
        .ok_or_else(|| limit("record_count", to_u64(entries.len())))?;
    let mut keys = BTreeSet::new();
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (index, (name, data)) in entries.iter().enumerate() {
        let checked = names::check(name.as_bytes(), true, &Limits::LARGE_MODELS, index)?;
        if !keys.insert(checked.key) {
            return Err(at_entry(codes::ZIP_NAME_DUPLICATE, index));
        }
        let name_len =
            u16::try_from(name.len()).map_err(|_| limit("string_bytes", to_u64(name.len())))?;
        let (code, packed) = match method {
            Method::Stored => (METHOD_STORED, None),
            Method::Deflate => (
                METHOD_DEFLATE,
                Some(miniz_oxide::deflate::compress_to_vec(data, LEVEL)),
            ),
        };
        let payload: &[u8] = packed.as_deref().unwrap_or(data);
        let size = small(data.len())?;
        let packed_size = small(payload.len())?;
        let offset = small(out.len())?;
        let crc = checksum(data);
        // The fields shared by the local header and the central record, from the version needed.
        let mut shared = Vec::with_capacity(26);
        for value in [VERSION, FLAG_UTF8, code, 0, DOS_DATE] {
            shared.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, packed_size, size] {
            shared.extend_from_slice(&value.to_le_bytes());
        }
        shared.extend_from_slice(&name_len.to_le_bytes());
        shared.extend_from_slice(&0u16.to_le_bytes());

        out.extend_from_slice(&super::LOCAL_SIGNATURE.to_le_bytes());
        out.extend_from_slice(&shared);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(payload);

        central.extend_from_slice(&super::CENTRAL_SIGNATURE.to_le_bytes());
        central.extend_from_slice(&VERSION.to_le_bytes());
        central.extend_from_slice(&shared);
        // Comment length, disk, internal and external attributes, then the local header offset.
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let directory_offset = small(out.len())?;
    let directory_size = small(central.len())?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&super::END_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// A size or offset that fits the 32-bit fields; the value `u32::MAX` would mark ZIP64.
fn small(value: usize) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|&v| v < u32::MAX)
        .ok_or_else(|| limit("input_bytes", to_u64(value)))
}
