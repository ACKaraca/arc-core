//! The options shared by the readers.

use arc_base::{Cancel, Limits};

/// The options of a read: the limits profile and the cancellation token.
#[derive(Debug, Clone)]
pub struct ReadOptions {
    /// The limits profile; [`Limits::CONSTRAINED`] by default.
    pub limits: Limits,
    /// Cancelling the token makes the reader return `base.cancelled` at its next check.
    pub cancel: Cancel,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            limits: Limits::CONSTRAINED,
            cancel: Cancel::new(),
        }
    }
}

impl ReadOptions {
    /// Options with the given limits profile and a token that is not cancelled.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            cancel: Cancel::new(),
        }
    }
}
