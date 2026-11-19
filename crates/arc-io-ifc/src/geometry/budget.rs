//! The running budget of one geometry evaluation: memory, record count, array length and
//! cancellation. Every allocation is charged before it is made.

use arc_base::{Cancel, Limits, Result};

use crate::error::limit;

/// Checks the cancellation token once every this many steps inside long loops.
const CHECK_EVERY: u64 = 1024;

/// The budget of one evaluation.
#[derive(Debug)]
pub(crate) struct Budget {
    cancel: Cancel,
    /// Bytes left for the evaluation, `None` when `memory_bytes` is unlimited.
    memory: Option<u64>,
    used: u64,
    records: u64,
    emitted: u64,
    array: u64,
    steps: u64,
}

impl Budget {
    /// The budget of an evaluation of a file whose parser used `parsed_bytes`.
    pub(crate) fn new(limits: &Limits, parsed_bytes: u64, cancel: &Cancel) -> Self {
        Self {
            cancel: cancel.clone(),
            memory: limits
                .memory_bytes
                .map(|max| max.saturating_sub(parsed_bytes)),
            used: 0,
            records: limits.record_count,
            emitted: 0,
            array: limits.array_length,
            steps: 0,
        }
    }

    /// Charges `bytes` before they are allocated; `io.limit_exceeded` for `memory_bytes` when the
    /// evaluation would exceed its share.
    pub(crate) fn charge(&mut self, bytes: u64) -> Result<()> {
        self.used = self.used.saturating_add(bytes);
        match self.memory {
            Some(max) if self.used > max => Err(limit("memory_bytes", self.used)),
            _ => Ok(()),
        }
    }

    /// Counts one record: an evaluated representation item, a mapped instance or an emitted item.
    pub(crate) fn record(&mut self) -> Result<()> {
        self.emitted = self.emitted.saturating_add(1);
        if self.emitted > self.records {
            Err(limit("record_count", self.emitted))
        } else {
            Ok(())
        }
    }

    /// Refuses a mesh or polyline whose length would exceed `array_length`.
    pub(crate) fn array(&self, length: usize) -> Result<()> {
        let length = u64::try_from(length).unwrap_or(u64::MAX);
        if length > self.array {
            Err(limit("array_length", length))
        } else {
            Ok(())
        }
    }

    /// One step of a long loop; every 1024 steps the cancellation token is checked.
    pub(crate) fn step(&mut self) -> Result<()> {
        self.steps = self.steps.saturating_add(1);
        if self.steps.is_multiple_of(CHECK_EVERY) {
            self.cancel.check()?;
        }
        Ok(())
    }

    /// Checks the cancellation token now.
    pub(crate) fn check(&self) -> Result<()> {
        self.cancel.check()
    }
}
