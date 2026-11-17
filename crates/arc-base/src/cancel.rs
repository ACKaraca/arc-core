//! Cancellation tokens (CORE-BASE-014).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result, codes};

/// A token shared between a requester and its workers.
///
/// Clones share one state: cancelling any clone is observed by all of them and cannot be undone.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// Creates a token that is not cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels the token for every clone, irreversibly.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether the token was cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Returns `base.cancelled` once the token was cancelled, for use with `?` in long-running
    /// work.
    ///
    /// # Errors
    ///
    /// `base.cancelled` when the token was cancelled.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::new(codes::CANCELLED))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_is_seen_by_every_clone_and_stays() {
        let token = Cancel::new();
        let worker = token.clone();
        assert!(!worker.is_cancelled());
        assert!(worker.check().is_ok());

        let seen = std::thread::spawn({
            let token = token.clone();
            move || {
                token.cancel();
                token.is_cancelled()
            }
        });
        assert!(seen.join().unwrap());

        assert!(token.is_cancelled());
        assert!(worker.is_cancelled());
        assert_eq!(worker.check().unwrap_err().code(), "base.cancelled");
    }

    #[test]
    fn independent_tokens_do_not_share_state() {
        let first = Cancel::new();
        first.cancel();
        assert!(!Cancel::new().is_cancelled());
    }
}
