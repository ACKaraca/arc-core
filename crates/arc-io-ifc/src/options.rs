//! Options of the reader and the progress, cancellation and wall-clock control it shares with the
//! views.

use arc_base::{Cancel, Limits, Result};

use crate::error::limit;
use crate::loss::LossReport;
use crate::model::IfcFile;

/// How far a read has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes read, or instances read.
    pub done: u64,
    /// Total bytes, or the total when it is known.
    pub total: u64,
}

/// A source of monotonic time for the wall-clock limit. The values are milliseconds from an origin
/// the source chooses; they never decrease.
pub trait Clock {
    /// Milliseconds since the source's origin.
    fn millis(&mut self) -> u64;
}

/// The system monotonic clock, with its origin at its creation. It is not built for `wasm32`,
/// which has no clock: there [`ReadOptions::default`] has no clock and the wall-clock limit is not
/// enforced unless the caller supplies one.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct SystemClock {
    origin: std::time::Instant,
}

#[cfg(not(target_arch = "wasm32"))]
impl SystemClock {
    /// A clock whose origin is now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Clock for SystemClock {
    fn millis(&mut self) -> u64 {
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn default_clock() -> Option<Box<dyn Clock>> {
    Some(Box::new(SystemClock::new()))
}

#[cfg(target_arch = "wasm32")]
fn default_clock() -> Option<Box<dyn Clock>> {
    None
}

/// Options of the reader.
pub struct ReadOptions<'a> {
    /// The limits profile; [`Limits::CONSTRAINED`] by default.
    pub limits: Limits,
    /// Cancelling it makes the read return `base.cancelled` at its next check.
    pub cancel: Cancel,
    /// Called on the first instance, every 1024 steps and at the end.
    pub progress: Option<&'a mut dyn FnMut(Progress)>,
    /// The clock of the wall-clock limit. The system clock by default, none on `wasm32`; without a
    /// clock the wall-clock limit is not enforced.
    pub clock: Option<Box<dyn Clock + 'a>>,
}

impl Default for ReadOptions<'_> {
    fn default() -> Self {
        Self {
            limits: Limits::CONSTRAINED,
            cancel: Cancel::new(),
            progress: None,
            clock: default_clock(),
        }
    }
}

impl ReadOptions<'_> {
    /// Options with the given limits profile.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// The progress, cancellation and wall-clock control of one read.
    pub(crate) fn control(&mut self) -> Control<'_> {
        Control::new(
            &self.cancel,
            self.progress
                .as_deref_mut()
                .map(|p| p as &mut dyn FnMut(Progress)),
            self.clock.as_deref_mut().map(|c| c as &mut dyn Clock),
            self.limits.wall_clock_seconds,
        )
    }
}

/// The result of a read.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadOutput {
    /// The parsed file.
    pub file: IfcFile,
    /// The loss report of the read.
    pub report: LossReport,
}

/// Progress reporting, cancellation and the wall-clock limit shared by the reader.
pub(crate) struct Control<'a> {
    cancel: Cancel,
    progress: Option<&'a mut dyn FnMut(Progress)>,
    clock: Option<&'a mut dyn Clock>,
    wall_clock_seconds: Option<u64>,
    started_ms: u64,
    steps: u64,
}

impl<'a> Control<'a> {
    pub(crate) fn new(
        cancel: &Cancel,
        progress: Option<&'a mut dyn FnMut(Progress)>,
        mut clock: Option<&'a mut dyn Clock>,
        wall_clock_seconds: Option<u64>,
    ) -> Self {
        let started_ms = clock.as_mut().map_or(0, |clock| clock.millis());
        Self {
            cancel: cancel.clone(),
            progress,
            clock,
            wall_clock_seconds,
            started_ms,
            steps: 0,
        }
    }

    /// Checks the token on every step; every 1024 steps it checks the wall-clock limit and reports
    /// progress.
    pub(crate) fn step(&mut self, done: u64, total: u64) -> Result<()> {
        self.cancel.check()?;
        self.steps = self.steps.saturating_add(1);
        if self.steps % 1024 == 1 {
            self.check_wall_clock()?;
            self.report(Progress { done, total });
        }
        Ok(())
    }

    /// Reports the end of the transfer.
    pub(crate) fn finish(&mut self, total: u64) {
        self.report(Progress { done: total, total });
    }

    fn check_wall_clock(&mut self) -> Result<()> {
        let (Some(clock), Some(seconds)) = (self.clock.as_mut(), self.wall_clock_seconds) else {
            return Ok(());
        };
        let elapsed = clock.millis().saturating_sub(self.started_ms);
        if elapsed > seconds.saturating_mul(1000) {
            Err(limit("wall_clock_seconds", elapsed / 1000))
        } else {
            Ok(())
        }
    }

    fn report(&mut self, progress: Progress) {
        if let Some(callback) = self.progress.as_mut() {
            callback(progress);
        }
    }
}
