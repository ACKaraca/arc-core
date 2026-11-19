//! Bounded reads of little-endian fields and byte ranges. Each read returns `None` when the
//! range does not lie inside the buffer, and the caller maps that to its own truncation error.

/// The `N` bytes at `at`.
pub(crate) fn array_at<const N: usize>(data: &[u8], at: usize) -> Option<[u8; N]> {
    slice(data, at, N)?.try_into().ok()
}

/// The little-endian `u16` at `at`.
pub(crate) fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    array_at::<2>(data, at).map(u16::from_le_bytes)
}

/// The little-endian `u32` at `at`.
pub(crate) fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    array_at::<4>(data, at).map(u32::from_le_bytes)
}

/// The little-endian `u64` at `at`.
pub(crate) fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    array_at::<8>(data, at).map(u64::from_le_bytes)
}

/// The `len` bytes starting at `start`.
pub(crate) fn slice(data: &[u8], start: usize, len: usize) -> Option<&[u8]> {
    let end = start.checked_add(len)?;
    data.get(start..end)
}

/// A length as `u64`. Lengths of in-memory buffers always fit; the saturating conversion only
/// keeps the function total.
pub(crate) fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// A value from the input as an offset into the buffer, or `None` when it does not fit `usize`.
pub(crate) fn to_usize(value: u64) -> Option<usize> {
    usize::try_from(value).ok()
}
