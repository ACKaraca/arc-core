//! A bounded reader for ZIP archives held in a byte buffer.
//!
//! [`ZipArchive::open`] validates the whole directory before any entry is inflated: the end
//! record (with its ZIP64 extension), every central directory record, the names, the methods,
//! the declared sizes, every local header and the layout of the entries. Only then does
//! [`ZipArchive::read_entry`] inflate an entry, on request, under the decompressed-size budget
//! that the archive keeps across reads.
//!
//! Supported: stored and deflate entries, ZIP64 sizes and offsets within the limits, data
//! descriptors whose CRC and sizes agree with the central directory. Refused with a specific
//! code: encryption, other methods, several disks, unverifiable descriptors, overlapping entries,
//! disagreeing headers and unsafe names. Offsets are absolute: an archive with data before its
//! first entry (a self-extracting stub) is refused as a structure error.

mod inflate;
mod names;

use std::collections::HashSet;

use arc_base::{Cancel, Error, Limits, Result};

use crate::bytes::{slice, to_u64, to_usize, u16_at, u32_at, u64_at};
use crate::crc32::Crc32;
use crate::error::{at_entry, codes, limit};
use crate::options::ReadOptions;
use inflate::{Bounds, inflate_entry};

const END_SIGNATURE: u32 = 0x0605_4B50;
const END_LEN: usize = 22;
const ZIP64_END_SIGNATURE: u32 = 0x0606_4B50;
const ZIP64_END_LEN: usize = 56;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4B50;
const ZIP64_LOCATOR_LEN: usize = 20;
const CENTRAL_SIGNATURE: u32 = 0x0201_4B50;
const CENTRAL_LEN: usize = 46;
const LOCAL_SIGNATURE: u32 = 0x0403_4B50;
const LOCAL_LEN: usize = 30;
const DESCRIPTOR_SIGNATURE: u32 = 0x0807_4B50;
const ZIP64_EXTRA_ID: u16 = 0x0001;
const MARK32: u32 = u32::MAX;
const FLAG_ENCRYPTED: u16 = 1 << 0;
const FLAG_DESCRIPTOR: u16 = 1 << 3;
const FLAG_STRONG_ENCRYPTION: u16 = 1 << 6;
const FLAG_UTF8: u16 = 1 << 11;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
/// The bytes between two cancellation checks while a CRC-32 is computed.
const CRC_CHUNK: usize = 64 * 1024;

/// The compression method of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// The data is stored without compression (method 0).
    Stored,
    /// The data is deflated (method 8).
    Deflate,
}

/// An entry of an archive, validated when the archive was opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    /// The name as the archive wrote it, with its trailing `/` for a directory.
    pub name: String,
    /// The compression method.
    pub method: Method,
    /// The uncompressed size, which the data must produce exactly.
    pub size: u64,
    /// The compressed size, which the local header agrees with.
    pub compressed_size: u64,
    /// The CRC-32 of the uncompressed data.
    pub crc32: u32,
    /// The offset of the first byte of the compressed data.
    data_start: usize,
}

impl ZipEntry {
    /// Whether the entry is a directory, that is, its name ends with `/`.
    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.name.ends_with('/')
    }
}

/// A ZIP archive over a byte buffer whose directory has been validated.
#[derive(Debug)]
pub struct ZipArchive<'a> {
    data: &'a [u8],
    limits: Limits,
    cancel: Cancel,
    entries: Vec<ZipEntry>,
    decompressed: u64,
}

impl<'a> ZipArchive<'a> {
    /// Validates the archive in `data` under the limits of `options`.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` when the input, the entry count, a name, a declared size or a ratio
    /// exceeds the profile; a `io.zip.*` code for every structural or safety violation.
    pub fn open(data: &'a [u8], options: &ReadOptions) -> Result<Self> {
        let limits = options.limits;
        let length = to_u64(data.len());
        if length > limits.input_bytes {
            return Err(limit("input_bytes", length));
        }
        let directory = locate_directory(data, &limits)?;
        let records = read_central(data, &directory)?;
        let mut entries = declare_entries(&records, &limits)?;
        let mut spans = vec![
            (
                directory.offset,
                directory.offset.saturating_add(directory.size),
            ),
            (directory.tail, data.len()),
        ];
        for (index, (entry, record)) in entries.iter_mut().zip(&records).enumerate() {
            let located = check_local(data, record, index)?;
            entry.data_start = located.data_start;
            spans.push((located.start, located.end));
        }
        check_layout(spans)?;
        Ok(Self {
            data,
            limits,
            cancel: options.cancel.clone(),
            entries,
            decompressed: 0,
        })
    }

    /// The entries in the order of the central directory.
    #[must_use]
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// Inflates entry `index` and returns its data after checking its CRC-32.
    ///
    /// The bytes produced by every call count against `Limits::decompressed_bytes` for the whole
    /// archive, so reading an entry again uses budget again.
    ///
    /// # Errors
    ///
    /// `io.zip.entry_missing` for an index out of range; `io.limit_exceeded` when the entry no
    /// longer fits the budget; `io.zip.size_mismatch`, `io.zip.inflate_failed`,
    /// `io.zip.truncated` or `io.zip.crc_mismatch` when the data does not match its record;
    /// `base.cancelled` when the token was cancelled.
    pub fn read_entry(&mut self, index: usize) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| at_entry(codes::ZIP_ENTRY_MISSING, index))?;
        let (method, size, compressed, crc32, start) = (
            entry.method,
            entry.size,
            entry.compressed_size,
            entry.crc32,
            entry.data_start,
        );
        let remaining = self
            .limits
            .decompressed_bytes
            .saturating_sub(self.decompressed);
        if size > remaining {
            return Err(limit("decompressed_bytes", size));
        }
        let input = to_usize(compressed)
            .and_then(|len| slice(self.data, start, len))
            .ok_or_else(|| Error::new(codes::ZIP_TRUNCATED))?;
        let out = match method {
            Method::Stored => input.to_vec(),
            Method::Deflate => {
                let bounds = Bounds {
                    expected: size,
                    ratio_cap: compressed
                        .saturating_mul(u64::from(self.limits.decompression_ratio)),
                    budget: remaining,
                };
                inflate_entry(input, &bounds, &self.cancel)?
            }
        };
        if !crc_matches(&out, crc32, &self.cancel)? {
            return Err(at_entry(codes::ZIP_CRC_MISMATCH, index));
        }
        self.decompressed = self.decompressed.saturating_add(to_u64(out.len()));
        Ok(out)
    }
}

/// The end of the central directory, as far as the reader needs it.
struct Directory {
    count: u64,
    offset: usize,
    size: usize,
    /// Where the trailing records start (the ZIP64 end record or the end record itself).
    tail: usize,
}

/// A central directory record after its fixed fields and its ZIP64 extension were read.
struct Central<'a> {
    raw_name: &'a [u8],
    flags: u16,
    method: u16,
    crc32: u32,
    sizes: Sizes,
}

/// Sizes and the offset of a record, with its ZIP64 flag.
struct Sizes {
    size: u64,
    compressed: u64,
    offset: u64,
    zip64: bool,
}

/// Where an entry's local header and data lie in the buffer.
struct Located {
    start: usize,
    data_start: usize,
    end: usize,
}

fn truncated() -> Error {
    Error::new(codes::ZIP_TRUNCATED)
}

fn structure() -> Error {
    Error::new(codes::ZIP_STRUCTURE_INVALID)
}

fn multi_disk() -> Error {
    Error::new(codes::ZIP_MULTI_DISK)
}

/// Finds the end record: the last signature within the last 64 KiB plus its own length whose
/// comment length ends exactly at the end of the buffer. The search is bounded by 65,558 offsets.
fn find_end_record(data: &[u8]) -> Result<usize> {
    let last = data
        .len()
        .checked_sub(END_LEN)
        .ok_or_else(|| Error::new(codes::ZIP_END_MISSING))?;
    let lowest = last.saturating_sub(usize::from(u16::MAX));
    for at in (lowest..=last).rev() {
        let comment = at.checked_add(END_LEN - 2).and_then(|p| u16_at(data, p));
        let fits = comment.is_some_and(|len| at + END_LEN + usize::from(len) == data.len());
        if fits && u32_at(data, at) == Some(END_SIGNATURE) {
            return Ok(at);
        }
    }
    Err(Error::new(codes::ZIP_END_MISSING))
}

fn locate_directory(data: &[u8], limits: &Limits) -> Result<Directory> {
    let end = find_end_record(data)?;
    let disk = u16_at(data, end + 4).ok_or_else(truncated)?;
    let cd_disk = u16_at(data, end + 6).ok_or_else(truncated)?;
    let disk_entries = u16_at(data, end + 8).ok_or_else(truncated)?;
    let total = u16_at(data, end + 10).ok_or_else(truncated)?;
    let cd_size = u32_at(data, end + 12).ok_or_else(truncated)?;
    let cd_offset = u32_at(data, end + 16).ok_or_else(truncated)?;
    if disk != 0 || cd_disk != 0 || disk_entries != total {
        return Err(multi_disk());
    }
    let (count, size, offset, tail) =
        if total == u16::MAX || cd_size == MARK32 || cd_offset == MARK32 {
            zip64_directory(data, end)?
        } else {
            (
                u64::from(total),
                u64::from(cd_size),
                u64::from(cd_offset),
                end,
            )
        };
    let offset = to_usize(offset).ok_or_else(truncated)?;
    let size = to_usize(size).ok_or_else(truncated)?;
    if offset.checked_add(size).is_none_or(|stop| stop > tail) {
        return Err(structure());
    }
    if count > limits.record_count {
        return Err(limit("record_count", count));
    }
    if count.saturating_mul(to_u64(CENTRAL_LEN)) > to_u64(size) {
        return Err(structure());
    }
    Ok(Directory {
        count,
        offset,
        size,
        tail,
    })
}

/// Reads the ZIP64 locator and end record that precede the end record at `end`.
fn zip64_directory(data: &[u8], end: usize) -> Result<(u64, u64, u64, usize)> {
    let locator = end.checked_sub(ZIP64_LOCATOR_LEN).ok_or_else(structure)?;
    if u32_at(data, locator) != Some(ZIP64_LOCATOR_SIGNATURE) {
        return Err(structure());
    }
    if u32_at(data, locator + 4) != Some(0) || u32_at(data, locator + 16) != Some(1) {
        return Err(multi_disk());
    }
    let record = u64_at(data, locator + 8).ok_or_else(truncated)?;
    let record = to_usize(record).ok_or_else(structure)?;
    if u32_at(data, record) != Some(ZIP64_END_SIGNATURE)
        || record
            .checked_add(ZIP64_END_LEN)
            .is_none_or(|stop| stop > locator)
    {
        return Err(structure());
    }
    if u32_at(data, record + 16) != Some(0) || u32_at(data, record + 20) != Some(0) {
        return Err(multi_disk());
    }
    let disk_entries = u64_at(data, record + 24).ok_or_else(truncated)?;
    let total = u64_at(data, record + 32).ok_or_else(truncated)?;
    if disk_entries != total {
        return Err(multi_disk());
    }
    let size = u64_at(data, record + 40).ok_or_else(truncated)?;
    let offset = u64_at(data, record + 48).ok_or_else(truncated)?;
    Ok((total, size, offset, record))
}

fn read_central<'a>(data: &'a [u8], directory: &Directory) -> Result<Vec<Central<'a>>> {
    let stop = directory.offset + directory.size;
    let mut records = Vec::with_capacity(to_usize(directory.count).unwrap_or(0));
    let mut at = directory.offset;
    for _ in 0..directory.count {
        let (record, next) = read_central_record(data, at, stop)?;
        records.push(record);
        at = next;
    }
    if at != stop {
        return Err(structure());
    }
    Ok(records)
}

fn read_central_record(data: &[u8], at: usize, stop: usize) -> Result<(Central<'_>, usize)> {
    if at.checked_add(CENTRAL_LEN).is_none_or(|fixed| fixed > stop)
        || u32_at(data, at) != Some(CENTRAL_SIGNATURE)
    {
        return Err(structure());
    }
    let flags = u16_at(data, at + 8).ok_or_else(truncated)?;
    let method = u16_at(data, at + 10).ok_or_else(truncated)?;
    let crc32 = u32_at(data, at + 16).ok_or_else(truncated)?;
    let compressed32 = u32_at(data, at + 20).ok_or_else(truncated)?;
    let size32 = u32_at(data, at + 24).ok_or_else(truncated)?;
    let name_len = usize::from(u16_at(data, at + 28).ok_or_else(truncated)?);
    let extra_len = usize::from(u16_at(data, at + 30).ok_or_else(truncated)?);
    let comment_len = usize::from(u16_at(data, at + 32).ok_or_else(truncated)?);
    let disk = u16_at(data, at + 34).ok_or_else(truncated)?;
    let offset32 = u32_at(data, at + 42).ok_or_else(truncated)?;
    if disk != 0 {
        return Err(multi_disk());
    }
    let name_at = at + CENTRAL_LEN;
    let extra_at = name_at + name_len;
    let next = extra_at + extra_len + comment_len;
    if next > stop {
        return Err(structure());
    }
    let raw_name = slice(data, name_at, name_len).ok_or_else(truncated)?;
    let extra = slice(data, extra_at, extra_len).ok_or_else(truncated)?;
    let sizes = resolve_sizes(size32, compressed32, Some(offset32), extra)?;
    let record = Central {
        raw_name,
        flags,
        method,
        crc32,
        sizes,
    };
    Ok((record, next))
}

/// The sizes and, for a central record, the offset. A 32-bit field holding the marker
/// `0xFFFFFFFF` takes its value from the ZIP64 extension, in the order size, compressed size,
/// offset.
fn resolve_sizes(
    size32: u32,
    compressed32: u32,
    offset32: Option<u32>,
    extra: &[u8],
) -> Result<Sizes> {
    let offset_marked = offset32 == Some(MARK32);
    let zip64 = size32 == MARK32 || compressed32 == MARK32 || offset_marked;
    let mut sizes = Sizes {
        size: u64::from(size32),
        compressed: u64::from(compressed32),
        offset: offset32.map_or(0, u64::from),
        zip64,
    };
    if !zip64 {
        return Ok(sizes);
    }
    let body = zip64_body(extra).ok_or_else(structure)?;
    let mut at = 0;
    if size32 == MARK32 {
        sizes.size = take_u64(body, &mut at)?;
    }
    if compressed32 == MARK32 {
        sizes.compressed = take_u64(body, &mut at)?;
    }
    if offset_marked {
        sizes.offset = take_u64(body, &mut at)?;
    }
    Ok(sizes)
}

fn take_u64(body: &[u8], at: &mut usize) -> Result<u64> {
    let value = u64_at(body, *at).ok_or_else(structure)?;
    *at = at.checked_add(8).ok_or_else(structure)?;
    Ok(value)
}

/// The body of the ZIP64 extra field in an extra field block, if the block holds one.
fn zip64_body(extra: &[u8]) -> Option<&[u8]> {
    let mut at = 0_usize;
    while at.checked_add(4)? <= extra.len() {
        let id = u16_at(extra, at)?;
        let len = usize::from(u16_at(extra, at + 2)?);
        let body_start = at + 4;
        let body = slice(extra, body_start, len)?;
        if id == ZIP64_EXTRA_ID {
            return Some(body);
        }
        at = body_start + len;
    }
    None
}

/// Checks the entries that need no data: names, duplicates, encryption, methods, declared sizes
/// and the ratio. The returned entries have no data offset yet.
fn declare_entries(records: &[Central<'_>], limits: &Limits) -> Result<Vec<ZipEntry>> {
    let mut seen = HashSet::with_capacity(records.len());
    let mut total: u64 = 0;
    let mut entries = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        let checked = names::check(
            record.raw_name,
            record.flags & FLAG_UTF8 != 0,
            limits,
            index,
        )?;
        if !seen.insert(checked.key) {
            return Err(at_entry(codes::ZIP_NAME_DUPLICATE, index));
        }
        if record.flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION) != 0 {
            return Err(at_entry(codes::ZIP_ENCRYPTED, index));
        }
        let method = match record.method {
            METHOD_STORED => Method::Stored,
            METHOD_DEFLATE => Method::Deflate,
            _ => return Err(at_entry(codes::ZIP_METHOD_UNSUPPORTED, index)),
        };
        let size = record.sizes.size;
        let compressed = record.sizes.compressed;
        if size > limits.decompressed_bytes {
            return Err(limit("decompressed_bytes", size));
        }
        total = total.saturating_add(size);
        if total > limits.decompressed_bytes {
            return Err(limit("decompressed_bytes", total));
        }
        match method {
            Method::Stored if compressed != size => {
                return Err(at_entry(codes::ZIP_STRUCTURE_INVALID, index));
            }
            Method::Deflate => {
                let cap = compressed.saturating_mul(u64::from(limits.decompression_ratio));
                if size > cap {
                    return Err(limit("decompression_ratio", size));
                }
            }
            Method::Stored => {}
        }
        entries.push(ZipEntry {
            name: checked.name,
            method,
            size,
            compressed_size: compressed,
            crc32: record.crc32,
            data_start: 0,
        });
    }
    Ok(entries)
}

/// Checks the local header of one entry against its central record and locates its data (and
/// its data descriptor, when the record has one).
fn check_local(data: &[u8], record: &Central<'_>, index: usize) -> Result<Located> {
    let start = to_usize(record.sizes.offset).ok_or_else(truncated)?;
    if u32_at(data, start) != Some(LOCAL_SIGNATURE) {
        return Err(at_entry(codes::ZIP_STRUCTURE_INVALID, index));
    }
    let flags = u16_at(data, start + 6).ok_or_else(truncated)?;
    let method = u16_at(data, start + 8).ok_or_else(truncated)?;
    let crc32 = u32_at(data, start + 14).ok_or_else(truncated)?;
    let compressed32 = u32_at(data, start + 18).ok_or_else(truncated)?;
    let size32 = u32_at(data, start + 22).ok_or_else(truncated)?;
    let name_len = usize::from(u16_at(data, start + 26).ok_or_else(truncated)?);
    let extra_len = usize::from(u16_at(data, start + 28).ok_or_else(truncated)?);
    let name_at = start + LOCAL_LEN;
    let extra_at = name_at + name_len;
    let data_start = extra_at + extra_len;
    let local_name = slice(data, name_at, name_len).ok_or_else(truncated)?;
    let local_extra = slice(data, extra_at, extra_len).ok_or_else(truncated)?;
    if local_name != record.raw_name || method != record.method {
        return Err(at_entry(codes::ZIP_HEADER_MISMATCH, index));
    }
    let descriptor = record.flags & FLAG_DESCRIPTOR != 0;
    if (flags & FLAG_DESCRIPTOR != 0) != descriptor {
        return Err(at_entry(codes::ZIP_HEADER_MISMATCH, index));
    }
    if !descriptor {
        let local = resolve_sizes(size32, compressed32, None, local_extra)?;
        if crc32 != record.crc32
            || local.size != record.sizes.size
            || local.compressed != record.sizes.compressed
        {
            return Err(at_entry(codes::ZIP_HEADER_MISMATCH, index));
        }
    }
    let data_end = to_usize(record.sizes.compressed)
        .and_then(|len| data_start.checked_add(len))
        .filter(|end| *end <= data.len())
        .ok_or_else(truncated)?;
    let end = if descriptor {
        check_descriptor(data, data_end, record, index)?
    } else {
        data_end
    };
    Ok(Located {
        start,
        data_start,
        end,
    })
}

/// Checks the data descriptor that follows the data of an entry and returns its end. The
/// descriptor's CRC and sizes must equal the central record's; the signature is optional.
fn check_descriptor(data: &[u8], at: usize, record: &Central<'_>, index: usize) -> Result<usize> {
    let unverified = || at_entry(codes::ZIP_DESCRIPTOR_UNVERIFIED, index);
    let at = if u32_at(data, at) == Some(DESCRIPTOR_SIGNATURE) {
        at.checked_add(4).ok_or_else(unverified)?
    } else {
        at
    };
    let wide = record.sizes.zip64;
    let width = if wide { 8 } else { 4 };
    let (crc32, compressed, size) = descriptor_fields(data, at, wide).ok_or_else(unverified)?;
    if crc32 != record.crc32 || compressed != record.sizes.compressed || size != record.sizes.size {
        return Err(unverified());
    }
    at.checked_add(4 + 2 * width).ok_or_else(unverified)
}

fn descriptor_fields(data: &[u8], at: usize, wide: bool) -> Option<(u32, u64, u64)> {
    let crc32 = u32_at(data, at)?;
    let first = at.checked_add(4)?;
    if wide {
        let second = first.checked_add(8)?;
        Some((crc32, u64_at(data, first)?, u64_at(data, second)?))
    } else {
        let second = first.checked_add(4)?;
        Some((
            crc32,
            u64::from(u32_at(data, first)?),
            u64::from(u32_at(data, second)?),
        ))
    }
}

/// Fails when two of the given `[start, end)` ranges share a byte. Empty ranges share nothing.
fn check_layout(mut spans: Vec<(usize, usize)>) -> Result<()> {
    spans.retain(|(start, end)| start < end);
    spans.sort_unstable();
    let mut reach = 0;
    for (start, end) in spans {
        if start < reach {
            return Err(Error::new(codes::ZIP_OVERLAP));
        }
        reach = reach.max(end);
    }
    Ok(())
}

/// Whether the CRC-32 of `out` equals `expected`, checking the token between chunks.
fn crc_matches(out: &[u8], expected: u32, cancel: &Cancel) -> Result<bool> {
    let mut crc = Crc32::new();
    for chunk in out.chunks(CRC_CHUNK) {
        cancel.check()?;
        crc.update(chunk);
    }
    Ok(crc.value() == expected)
}
