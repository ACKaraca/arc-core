//! Options of the reader and the progress and cancellation control it shares with the views.

use arc_base::{Cancel, Limits, Result};

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

/// Options of the reader.
pub struct ReadOptions<'a> {
    /// The limits profile; [`Limits::CONSTRAINED`] by default.
    pub limits: Limits,
    /// Cancelling it makes the read return `base.cancelled` at its next check.
    pub cancel: Cancel,
    /// Called on the first instance, every 1024 steps and at the end.
    pub progress: Option<&'a mut dyn FnMut(Progress)>,
}

impl Default for ReadOptions<'_> {
    fn default() -> Self {
        Self {
            limits: Limits::CONSTRAINED,
            cancel: Cancel::new(),
            progress: None,
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

    /// The progress and cancellation control of one read.
    pub(crate) fn control(&mut self) -> Control<'_> {
        Control::new(
            &self.cancel,
            self.progress
                .as_deref_mut()
                .map(|p| p as &mut dyn FnMut(Progress)),
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

/// Progress reporting and cancellation shared by the reader and the typed views.
pub(crate) struct Control<'a> {
    cancel: Cancel,
    progress: Option<&'a mut dyn FnMut(Progress)>,
    steps: u64,
}

impl<'a> Control<'a> {
    pub(crate) fn new(cancel: &Cancel, progress: Option<&'a mut dyn FnMut(Progress)>) -> Self {
        Self {
            cancel: cancel.clone(),
            progress,
            steps: 0,
        }
    }

    /// Checks the token and reports progress every 1024 steps.
    pub(crate) fn step(&mut self, done: u64, total: u64) -> Result<()> {
        self.cancel.check()?;
        self.steps = self.steps.saturating_add(1);
        if self.steps % 1024 == 1 {
            self.report(Progress { done, total });
        }
        Ok(())
    }

    /// Reports the end of the transfer.
    pub(crate) fn finish(&mut self, total: u64) {
        self.report(Progress { done: total, total });
    }

    fn report(&mut self, progress: Progress) {
        if let Some(callback) = self.progress.as_mut() {
            callback(progress);
        }
    }
}
