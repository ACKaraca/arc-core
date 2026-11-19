//! Bounded reads of binary files in either byte order. Every read checks the remaining length
//! first and fails with `io.mesh.truncated` instead of reading past the end.

use arc_base::Result;

use crate::error::{at_offset, codes};
use crate::text::len_u64;

/// A cursor over a byte buffer with a fixed byte order.
pub(crate) struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
    big: bool,
}

impl<'a> Bytes<'a> {
    pub(crate) fn new(data: &'a [u8], big_endian: bool) -> Self {
        Self {
            data,
            at: 0,
            big: big_endian,
        }
    }

    /// The bytes not read yet.
    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.at)
    }

    /// The byte offset of the cursor.
    pub(crate) fn offset(&self) -> u64 {
        len_u64(self.at)
    }

    fn truncated(&self) -> arc_base::Error {
        at_offset(codes::MESH_TRUNCATED, self.offset())
    }

    /// The next `count` bytes.
    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(count).ok_or_else(|| self.truncated())?;
        let slice = self
            .data
            .get(self.at..end)
            .ok_or_else(|| self.truncated())?;
        self.at = end;
        Ok(slice)
    }

    /// The next `N` bytes as an array.
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        <[u8; N]>::try_from(slice).map_err(|_| self.truncated())
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(u8::from_ne_bytes(self.array::<1>()?))
    }

    pub(crate) fn i8(&mut self) -> Result<i8> {
        Ok(i8::from_ne_bytes(self.array::<1>()?))
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        let raw = self.array::<2>()?;
        Ok(if self.big {
            u16::from_be_bytes(raw)
        } else {
            u16::from_le_bytes(raw)
        })
    }

    pub(crate) fn i16(&mut self) -> Result<i16> {
        let raw = self.array::<2>()?;
        Ok(if self.big {
            i16::from_be_bytes(raw)
        } else {
            i16::from_le_bytes(raw)
        })
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let raw = self.array::<4>()?;
        Ok(if self.big {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        })
    }

    pub(crate) fn i32(&mut self) -> Result<i32> {
        let raw = self.array::<4>()?;
        Ok(if self.big {
            i32::from_be_bytes(raw)
        } else {
            i32::from_le_bytes(raw)
        })
    }

    pub(crate) fn f32(&mut self) -> Result<f32> {
        let raw = self.array::<4>()?;
        Ok(if self.big {
            f32::from_be_bytes(raw)
        } else {
            f32::from_le_bytes(raw)
        })
    }

    pub(crate) fn f64(&mut self) -> Result<f64> {
        let raw = self.array::<8>()?;
        Ok(if self.big {
            f64::from_be_bytes(raw)
        } else {
            f64::from_le_bytes(raw)
        })
    }
}
