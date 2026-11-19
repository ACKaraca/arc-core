//! Fixtures built in the tests: ZIP archives written byte by byte, so that every malformation a
//! test needs can be produced exactly.

#![allow(dead_code, reason = "each test file uses part of the fixtures")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test data"
)]

use arc_io_common::crc32::checksum;

/// One entry of an archive. The fields are the values written; a test changes one of them to
/// produce the malformation it checks.
#[derive(Clone, Debug)]
pub struct Spec {
    pub name: Vec<u8>,
    pub method: u16,
    pub flags: u16,
    /// The bytes written as the entry's data.
    pub payload: Vec<u8>,
    /// The uncompressed size in the central record.
    pub size: u32,
    /// The compressed size in both records.
    pub compressed: u32,
    /// The CRC-32 in the central record.
    pub crc: u32,
    /// A local header name that differs from the central record's, if set.
    pub local_name: Option<Vec<u8>>,
    /// A local header method that differs from the central record's, if set.
    pub local_method: Option<u16>,
    /// A local header CRC that differs from the central record's, if set.
    pub local_crc: Option<u32>,
    /// A data descriptor (CRC, compressed size, size) written after the data, if set. The local
    /// header then carries zeros, as the format allows.
    pub descriptor: Option<(u32, u32, u32)>,
}

impl Spec {
    /// A stored entry with the correct sizes and CRC.
    pub fn stored(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.as_bytes().to_vec(),
            method: 0,
            flags: 0,
            payload: data.to_vec(),
            size: len32(data),
            compressed: len32(data),
            crc: checksum(data),
            local_name: None,
            local_method: None,
            local_crc: None,
            descriptor: None,
        }
    }

    /// A deflated entry with the correct sizes and CRC.
    pub fn deflated(name: &str, data: &[u8]) -> Self {
        let payload = deflate(data);
        Self {
            name: name.as_bytes().to_vec(),
            method: 8,
            flags: 0,
            compressed: len32(&payload),
            payload,
            size: len32(data),
            crc: checksum(data),
            local_name: None,
            local_method: None,
            local_crc: None,
            descriptor: None,
        }
    }
}

/// Raw deflate of `data`, the form ZIP stores.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(data, 6)
}

/// The archive layout options.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Writes the ZIP64 end record and locator, and the ZIP64 extension of every central record.
    pub zip64: bool,
    /// The disk number written in the end record (zero unless a test needs another disk).
    pub disk: u16,
    /// The archive comment.
    pub comment: Vec<u8>,
}

/// Writes an archive of `entries` with the default layout.
pub fn archive(entries: &[Spec]) -> Vec<u8> {
    archive_with(entries, &Layout::default())
}

/// Writes an archive of `entries` with `layout`.
pub fn archive_with(entries: &[Spec], layout: &Layout) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    for spec in entries {
        offsets.push(len32(&out));
        write_local(&mut out, spec);
    }
    let central_start = out.len();
    for (spec, offset) in entries.iter().zip(&offsets) {
        write_central(&mut out, spec, *offset, layout.zip64);
    }
    let central_size = len32(&out[central_start..]);
    let count = u16::try_from(entries.len()).unwrap_or(u16::MAX);
    if layout.zip64 {
        let record = out.len();
        write_zip64_end(
            &mut out,
            to_u64(entries.len()),
            to_u64(central_start),
            u64::from(central_size),
        );
        out.extend_from_slice(&0x0706_4B50_u32.to_le_bytes());
        out.extend_from_slice(&0_u32.to_le_bytes());
        out.extend_from_slice(&to_u64(record).to_le_bytes());
        out.extend_from_slice(&1_u32.to_le_bytes());
    }
    out.extend_from_slice(&0x0605_4B50_u32.to_le_bytes());
    out.extend_from_slice(&layout.disk.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    let (cd_count, cd_size, cd_offset) = if layout.zip64 {
        (u16::MAX, u32::MAX, u32::MAX)
    } else {
        (count, central_size, len32(&out[..central_start]))
    };
    out.extend_from_slice(&cd_count.to_le_bytes());
    out.extend_from_slice(&cd_count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(
        &u16::try_from(layout.comment.len())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    out.extend_from_slice(&layout.comment);
    out
}

fn write_local(out: &mut Vec<u8>, spec: &Spec) {
    let descriptor = spec.descriptor.is_some();
    let flags = spec.flags | if descriptor { 1 << 3 } else { 0 };
    let (crc, compressed, size) = if descriptor {
        (0, 0, 0)
    } else {
        (
            spec.local_crc.unwrap_or(spec.crc),
            spec.compressed,
            spec.size,
        )
    };
    let name = spec.local_name.clone().unwrap_or_else(|| spec.name.clone());
    out.extend_from_slice(&0x0403_4B50_u32.to_le_bytes());
    out.extend_from_slice(&20_u16.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&spec.local_method.unwrap_or(spec.method).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&compressed.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&u16::try_from(name.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&name);
    out.extend_from_slice(&spec.payload);
    if let Some((crc, compressed, size)) = spec.descriptor {
        out.extend_from_slice(&0x0807_4B50_u32.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&compressed.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
    }
}

fn write_central(out: &mut Vec<u8>, spec: &Spec, offset: u32, zip64: bool) {
    let mut extra = Vec::new();
    if zip64 {
        extra.extend_from_slice(&0x0001_u16.to_le_bytes());
        extra.extend_from_slice(&24_u16.to_le_bytes());
        extra.extend_from_slice(&u64::from(spec.size).to_le_bytes());
        extra.extend_from_slice(&u64::from(spec.compressed).to_le_bytes());
        extra.extend_from_slice(&u64::from(offset).to_le_bytes());
    }
    let (size32, compressed32, offset32) = if zip64 {
        (u32::MAX, u32::MAX, u32::MAX)
    } else {
        (spec.size, spec.compressed, offset)
    };
    let flags = spec.flags | if spec.descriptor.is_some() { 1 << 3 } else { 0 };
    out.extend_from_slice(&0x0201_4B50_u32.to_le_bytes());
    out.extend_from_slice(&20_u16.to_le_bytes());
    out.extend_from_slice(&20_u16.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&spec.method.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&spec.crc.to_le_bytes());
    out.extend_from_slice(&compressed32.to_le_bytes());
    out.extend_from_slice(&size32.to_le_bytes());
    out.extend_from_slice(&u16::try_from(spec.name.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&u16::try_from(extra.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&[0; 6]);
    out.extend_from_slice(&offset32.to_le_bytes());
    out.extend_from_slice(&spec.name);
    out.extend_from_slice(&extra);
}

fn write_zip64_end(out: &mut Vec<u8>, count: u64, cd_offset: u64, cd_size: u64) {
    out.extend_from_slice(&0x0606_4B50_u32.to_le_bytes());
    out.extend_from_slice(&44_u64.to_le_bytes());
    out.extend_from_slice(&45_u16.to_le_bytes());
    out.extend_from_slice(&45_u16.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
}

fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn len32(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).unwrap_or(u32::MAX)
}

/// Deterministic pseudo-random bytes (a linear congruential generator), for payloads that do not
/// compress well.
pub fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 56) as u8
        })
        .collect()
}
