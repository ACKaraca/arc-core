//! Bounded inflation of one deflate entry. The output is capped by the declared size, the ratio
//! limit and the remaining decompressed-size budget, and the cap is enforced while inflating, so
//! a stream that is larger than it declares is stopped one byte past the cap.

use arc_base::{Cancel, Error, Result};
use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZError, MZFlush, MZStatus};

use crate::bytes::to_u64;
use crate::error::{codes, limit};

/// The size of the output window of one inflate call.
const WINDOW: usize = 32 * 1024;
/// The largest growth of the output vector in one reservation.
const RESERVE_STEP: usize = 4 * 1024 * 1024;

/// The output bounds of one entry.
pub(super) struct Bounds {
    /// The declared uncompressed size, which the stream must produce exactly.
    pub(super) expected: u64,
    /// The largest output the ratio limit allows: compressed size times the ratio.
    pub(super) ratio_cap: u64,
    /// The decompressed bytes the archive may still produce.
    pub(super) budget: u64,
}

/// Inflates `input` (the raw deflate stream of one entry) under `bounds`, appending the output to
/// `out`. On error, `out` holds the bytes produced so far, which the caller charges to its budget.
pub(super) fn inflate_entry(
    input: &[u8],
    bounds: &Bounds,
    cancel: &Cancel,
    out: &mut Vec<u8>,
) -> Result<()> {
    let hard = bounds.expected.min(bounds.ratio_cap).min(bounds.budget);
    let overflow = if hard == bounds.expected {
        Error::new(codes::ZIP_SIZE_MISMATCH)
    } else if hard == bounds.ratio_cap {
        // Unreachable while `declare_entries` enforces the ratio; kept as defense in depth.
        limit("decompression_ratio", bounds.ratio_cap)
    } else {
        limit("decompressed_bytes", bounds.budget)
    };
    // The output never exceeds `hard + 1`, which is at most the declared size plus one.
    let ceiling = usize::try_from(bounds.expected.saturating_add(1)).unwrap_or(usize::MAX);

    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut window = vec![0_u8; WINDOW];
    let mut consumed = 0_usize;
    loop {
        cancel.check()?;
        let room = hard.saturating_add(1).saturating_sub(to_u64(out.len()));
        let want = usize::try_from(room).unwrap_or(WINDOW).min(WINDOW);
        let target = window
            .get_mut(..want)
            .ok_or_else(|| Error::new(codes::ZIP_INFLATE_FAILED))?;
        let rest = input.get(consumed..).unwrap_or_default();
        let step = inflate(&mut state, rest, target, MZFlush::None);
        consumed = consumed.saturating_add(step.bytes_consumed);
        let written = target.get(..step.bytes_written).unwrap_or_default();
        reserve(out, written.len(), ceiling)?;
        out.extend_from_slice(written);
        if to_u64(out.len()) > hard {
            return Err(overflow);
        }
        match step.status {
            Ok(MZStatus::StreamEnd) => break,
            Ok(_) if step.bytes_written == 0 && step.bytes_consumed == 0 => {
                return Err(Error::new(codes::ZIP_INFLATE_FAILED));
            }
            Ok(_) => {}
            Err(MZError::Buf) => return Err(Error::new(codes::ZIP_TRUNCATED)),
            Err(_) => return Err(Error::new(codes::ZIP_INFLATE_FAILED)),
        }
    }
    if consumed != input.len() || to_u64(out.len()) != bounds.expected {
        return Err(Error::new(codes::ZIP_SIZE_MISMATCH));
    }
    Ok(())
}

/// Makes room for `incoming` more bytes in `out`. The vector grows by the bytes already held (at
/// least one window and at most `RESERVE_STEP`) and never beyond `ceiling`, so its capacity is
/// bounded by the declared size rather than by a doubling of the output.
fn reserve(out: &mut Vec<u8>, incoming: usize, ceiling: usize) -> Result<()> {
    let needed = out.len().saturating_add(incoming);
    if needed <= out.capacity() {
        return Ok(());
    }
    let headroom = ceiling.saturating_sub(out.len());
    let grow = out
        .len()
        .clamp(WINDOW, RESERVE_STEP)
        .min(headroom)
        .max(incoming);
    out.try_reserve_exact(grow)
        .map_err(|_| limit("decompressed_bytes", to_u64(needed)))
}

#[cfg(test)]
mod tests {
    use arc_base::Cancel;

    use super::{Bounds, inflate_entry};

    #[test]
    fn the_output_capacity_stays_near_the_declared_size() {
        let data = vec![0_u8; 100_000];
        let input = miniz_oxide::deflate::compress_to_vec(&data, 6);
        let bounds = Bounds {
            expected: 100_000,
            ratio_cap: u64::MAX,
            budget: u64::MAX,
        };
        let mut out = Vec::new();
        let inflated = inflate_entry(&input, &bounds, &Cancel::new(), &mut out).is_ok();
        assert!(inflated);
        assert_eq!(out.len(), 100_000);
        assert!(out.capacity() <= 100_001, "capacity {}", out.capacity());
    }
}
