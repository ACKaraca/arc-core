//! Pooled GPU buffers with sub-allocation, free lists and compaction (CORE-GPU-006).
//!
//! A pool is one `wgpu` buffer divided into units of a fixed stride (one vertex, one index or
//! one instance). Allocations are first-fit from a sorted, coalesced free list. When no free
//! block is large enough the pool compacts if its total free space suffices, and otherwise grows
//! to at least twice its capacity, up to its byte limit. Compaction and growth copy live ranges
//! on the GPU; they are not counted as written bytes.

use std::collections::BTreeMap;
use std::ops::Range;

use arc_base::{Error, Result};

use crate::error::codes;

/// The capacity of a new pool in units.
const INITIAL_UNITS: u64 = 1024;

/// A pool is compacted after an update when at least this share of it is free ...
pub const COMPACT_FREE_SHARE: f64 = 0.25;
/// ... and its largest free block is less than this share of its free space.
pub const COMPACT_LARGEST_SHARE: f64 = 0.5;

/// The handle of an allocation in one pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AllocId(u64);

/// The CPU side of a pool: ranges in units.
#[derive(Debug, Default)]
pub(crate) struct RangeAllocator {
    capacity: u64,
    free: Vec<Range<u64>>,
    live: BTreeMap<AllocId, Range<u64>>,
    next: u64,
}

impl RangeAllocator {
    pub(crate) fn capacity(&self) -> u64 {
        self.capacity
    }

    pub(crate) fn used(&self) -> u64 {
        self.live.values().map(|r| r.end - r.start).sum()
    }

    fn free_total(&self) -> u64 {
        self.capacity - self.used()
    }

    fn largest_free(&self) -> u64 {
        self.free.iter().map(|r| r.end - r.start).max().unwrap_or(0)
    }

    /// Whether the documented compaction rule asks for compaction.
    pub(crate) fn wants_compaction(&self) -> bool {
        let free = self.free_total();
        #[allow(clippy::cast_precision_loss)] // A ratio; counts beyond 2^53 do not occur.
        let (free_f, capacity_f, largest_f) = (
            free as f64,
            self.capacity as f64,
            self.largest_free() as f64,
        );
        free > 0
            && free_f >= COMPACT_FREE_SHARE * capacity_f
            && largest_f < COMPACT_LARGEST_SHARE * free_f
    }

    pub(crate) fn range(&self, id: AllocId) -> Option<Range<u64>> {
        self.live.get(&id).cloned()
    }

    /// First-fit allocation of `units > 0`.
    pub(crate) fn alloc(&mut self, units: u64) -> Option<AllocId> {
        let index = self.free.iter().position(|r| r.end - r.start >= units)?;
        let block = self.free.get_mut(index)?;
        let start = block.start;
        block.start += units;
        if block.start == block.end {
            self.free.remove(index);
        }
        let id = AllocId(self.next);
        self.next += 1;
        self.live.insert(id, start..start + units);
        Some(id)
    }

    pub(crate) fn free(&mut self, id: AllocId) {
        let Some(range) = self.live.remove(&id) else {
            return;
        };
        let at = self.free.partition_point(|r| r.start < range.start);
        self.free.insert(at, range);
        // Coalesce with the neighbors.
        self.merge_with_next(at);
        if let Some(before) = at.checked_sub(1) {
            self.merge_with_next(before);
        }
    }

    /// Merges free block `at` with the block after it when they touch.
    fn merge_with_next(&mut self, at: usize) {
        let Some(next_start) = self.free.get(at + 1).map(|r| r.start) else {
            return;
        };
        let Some(next_end) = self.free.get(at + 1).map(|r| r.end) else {
            return;
        };
        if let Some(this) = self.free.get_mut(at).filter(|this| this.end == next_start) {
            this.end = next_end;
            self.free.remove(at + 1);
        }
    }

    pub(crate) fn grow(&mut self, capacity: u64) {
        if capacity <= self.capacity {
            return;
        }
        match self.free.last_mut() {
            Some(last) if last.end == self.capacity => last.end = capacity,
            _ => self.free.push(self.capacity..capacity),
        }
        self.capacity = capacity;
    }

    /// Moves every live range down to close the gaps, keeping their order. Returns the moves
    /// as `(old start, new start, length)`.
    pub(crate) fn compact(&mut self) -> Vec<(u64, u64, u64)> {
        let mut order: Vec<(AllocId, Range<u64>)> =
            self.live.iter().map(|(id, r)| (*id, r.clone())).collect();
        order.sort_by_key(|(_, r)| r.start);
        let mut moves = Vec::with_capacity(order.len());
        let mut cursor = 0;
        for (id, range) in order {
            let len = range.end - range.start;
            moves.push((range.start, cursor, len));
            self.live.insert(id, cursor..cursor + len);
            cursor += len;
        }
        self.free.clear();
        if cursor < self.capacity {
            self.free.push(cursor..self.capacity);
        }
        moves
    }
}

/// Bytes in use and reserved by one pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolUsage {
    /// Bytes of the GPU buffers the pool holds.
    pub capacity_bytes: u64,
    /// Bytes of live allocations.
    pub used_bytes: u64,
}

/// One pooled GPU buffer, optionally with a companion buffer of the same capacity in units
/// (the per-instance camera-relative translations).
#[derive(Debug)]
pub(crate) struct Pool {
    label: &'static str,
    usage: wgpu::BufferUsages,
    stride: u64,
    companion_stride: u64,
    max_bytes: u64,
    alloc: RangeAllocator,
    buffer: wgpu::Buffer,
    companion: Option<wgpu::Buffer>,
}

impl Pool {
    pub(crate) fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        stride: u64,
        companion_stride: u64,
        max_bytes: u64,
    ) -> Self {
        let usage = usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let mut alloc = RangeAllocator::default();
        let units = INITIAL_UNITS.min(max_bytes / stride.max(1)).max(1);
        alloc.grow(units);
        let buffer = create(device, label, usage, units * stride);
        let companion =
            (companion_stride > 0).then(|| create(device, label, usage, units * companion_stride));
        Self {
            label,
            usage,
            stride,
            companion_stride,
            max_bytes,
            alloc,
            buffer,
            companion,
        }
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub(crate) fn companion(&self) -> Option<&wgpu::Buffer> {
        self.companion.as_ref()
    }

    pub(crate) fn range(&self, id: AllocId) -> Option<Range<u64>> {
        self.alloc.range(id)
    }

    pub(crate) fn usage(&self) -> PoolUsage {
        PoolUsage {
            capacity_bytes: self.alloc.capacity() * self.stride,
            used_bytes: self.alloc.used() * self.stride,
        }
    }

    pub(crate) fn companion_usage(&self) -> PoolUsage {
        PoolUsage {
            capacity_bytes: self.alloc.capacity() * self.companion_stride,
            used_bytes: self.alloc.used() * self.companion_stride,
        }
    }

    pub(crate) fn free(&mut self, id: AllocId) {
        self.alloc.free(id);
    }

    /// Allocates `units` and writes `bytes` (exactly `units * stride` long) into them. Returns
    /// the handle and whether existing ranges moved (so companions must be rewritten).
    pub(crate) fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        units: u64,
        bytes: &[u8],
    ) -> Result<(AllocId, bool)> {
        let mut moved = false;
        let id = match self.alloc.alloc(units) {
            Some(id) => id,
            None => {
                moved = true;
                if self.alloc.free_total() >= units {
                    self.compact(device, queue);
                } else {
                    self.grow(device, queue, units)?;
                }
                self.alloc
                    .alloc(units)
                    .ok_or_else(|| Error::new(codes::LIMIT_EXCEEDED))?
            }
        };
        let start = self.alloc.range(id).map_or(0, |r| r.start);
        queue.write_buffer(&self.buffer, start * self.stride, bytes);
        Ok((id, moved))
    }

    /// Writes `bytes` to the companion buffer at the start of allocation `id`.
    pub(crate) fn write_companion(&self, queue: &wgpu::Queue, id: AllocId, bytes: &[u8]) {
        if let (Some(buffer), Some(range)) = (&self.companion, self.alloc.range(id)) {
            queue.write_buffer(buffer, range.start * self.companion_stride, bytes);
        }
    }

    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, units: u64) -> Result<()> {
        let max_units = self.max_bytes / self.stride.max(1);
        let needed = self.alloc.used().saturating_add(units);
        if needed > max_units {
            return Err(Error::new(codes::LIMIT_EXCEEDED)
                .with_detail("pool", self.label)
                .with_detail(
                    "bytes",
                    i64::try_from(needed.saturating_mul(self.stride)).unwrap_or(i64::MAX),
                ));
        }
        let capacity = (self.alloc.capacity() * 2).max(needed).min(max_units);
        // Compacting into the new buffer leaves all free space at the end.
        let moves = self.alloc.compact();
        self.alloc.grow(capacity);
        self.relocate(device, queue, capacity, &moves);
        Ok(())
    }

    /// Compacts the pool on the GPU.
    pub(crate) fn compact(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let moves = self.alloc.compact();
        self.relocate(device, queue, self.alloc.capacity(), &moves);
    }

    pub(crate) fn wants_compaction(&self) -> bool {
        self.alloc.wants_compaction()
    }

    fn relocate(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        capacity: u64,
        moves: &[(u64, u64, u64)],
    ) {
        let buffer = create(device, self.label, self.usage, capacity * self.stride);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("arc-gpu pool relocation"),
        });
        for &(from, to, len) in moves {
            if len > 0 {
                encoder.copy_buffer_to_buffer(
                    &self.buffer,
                    from * self.stride,
                    &buffer,
                    to * self.stride,
                    len * self.stride,
                );
            }
        }
        queue.submit([encoder.finish()]);
        self.buffer = buffer;
        if self.companion.is_some() {
            // Companion contents are rewritten by the next frame.
            self.companion = Some(create(
                device,
                self.label,
                self.usage,
                capacity * self.companion_stride,
            ));
        }
    }
}

fn create(
    device: &wgpu::Device,
    label: &str,
    usage: wgpu::BufferUsages,
    size: u64,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn freed_blocks_coalesce_and_are_reused_first_fit() {
        let mut a = RangeAllocator::default();
        a.grow(10);
        let x = a.alloc(3).unwrap();
        let y = a.alloc(3).unwrap();
        let z = a.alloc(3).unwrap();
        a.free(x);
        a.free(y);
        assert_eq!(a.free, vec![0..6, 9..10]);
        let w = a.alloc(5).unwrap();
        assert_eq!(a.range(w), Some(0..5));
        a.free(z);
        assert_eq!(a.free, vec![5..10]);
    }

    #[test]
    fn compaction_rule_needs_free_space_and_fragmentation() {
        let mut a = RangeAllocator::default();
        a.grow(8);
        let ids: Vec<_> = (0..8).map(|_| a.alloc(1).unwrap()).collect();
        assert!(!a.wants_compaction());
        // Free every other unit: half free, largest block 1 of 4 free units.
        for id in ids.iter().step_by(2) {
            a.free(*id);
        }
        assert!(a.wants_compaction());
        a.compact();
        assert!(!a.wants_compaction());
        assert_eq!(a.free, vec![4..8]);
    }

    proptest! {
        /// Random allocation sequences never overlap, and compaction keeps every allocation's
        /// length and relative order while leaving one free block at the end.
        #[test]
        fn allocations_never_overlap(ops in prop::collection::vec((any::<bool>(), 1_u64..50), 1..200)) {
            let mut a = RangeAllocator::default();
            a.grow(500);
            let mut live = Vec::new();
            for (free, units) in ops {
                if free && !live.is_empty() {
                    let id = live.remove(usize::try_from(units).unwrap() % live.len());
                    a.free(id);
                } else if let Some(id) = a.alloc(units) {
                    live.push(id);
                }
                let mut ranges: Vec<_> = live.iter().map(|id| a.range(*id).unwrap()).collect();
                ranges.sort_by_key(|r| r.start);
                for pair in ranges.windows(2) {
                    prop_assert!(pair[0].end <= pair[1].start);
                }
                prop_assert_eq!(a.used() + a.free.iter().map(|r| r.end - r.start).sum::<u64>(), 500);
            }
            let before: Vec<_> = live.iter().map(|id| a.range(*id).unwrap()).collect();
            a.compact();
            let after: Vec<_> = live.iter().map(|id| a.range(*id).unwrap()).collect();
            for (b, c) in before.iter().zip(&after) {
                prop_assert_eq!(b.end - b.start, c.end - c.start);
            }
            prop_assert!(a.free.len() <= 1);
        }
    }
}
