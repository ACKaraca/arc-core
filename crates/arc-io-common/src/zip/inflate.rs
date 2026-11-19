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

/// The output bounds of one entry.
pub(super) struct Bounds {
    /// The declared uncompressed size, which the stream must produce exactly.
    pub(super) expected: u64,
    /// The largest output the ratio limit allows: compressed size times the ratio.
    pub(super) ratio_cap: u64,
    /// The decompressed bytes the archive may still produce.
    pub(super) budget: u64,
}

/// Inflates `input` (the raw deflate stream of one entry) under `bounds`.
pub(super) fn inflate_entry(input: &[u8], bounds: &Bounds, cancel: &Cancel) -> Result<Vec<u8>> {
    let hard = bounds.expected.min(bounds.ratio_cap).min(bounds.budget);
    let overflow = if hard == bounds.expected {
        Error::new(codes::ZIP_SIZE_MISMATCH)
    } else if hard == bounds.ratio_cap {
        limit("decompression_ratio", bounds.ratio_cap)
    } else {
        limit("decompressed_bytes", bounds.budget)
    };

    let mut state = InflateState::new_boxed(DataFormat::Raw);
    let mut window = vec![0_u8; WINDOW];
    let mut out: Vec<u8> = Vec::new();
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
    Ok(out)
}
