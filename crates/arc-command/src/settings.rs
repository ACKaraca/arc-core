//! Session settings: values a product keeps per session, outside the document.
//!
//! A setting is a JSON value under a dotted key. The first segment of the key is the product's
//! namespace (for example `acme.object_snap`), so products do not read each other's settings.
//! Writing a setting creates no journal record, no undo step and no revision change. Clones of a
//! session share its settings, as they share its selection; two sessions never do.
//!
//! A product writes a setting directly on its session, and the write takes effect when the call
//! returns. A command writes through its context: the write is buffered, reads inside the command
//! see it, and it is applied only when the command succeeds and is not a dry run, as the selection
//! is. A failed command or a dry run leaves the settings as they were.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use arc_base::{Error, Result};
use serde_json::Value;

use crate::error::codes;

/// The most settings one session holds.
pub const MAX_SETTINGS: usize = 256;

/// The most bytes one setting value may take as JSON text.
pub const MAX_SETTING_BYTES: usize = 4096;

/// The most bytes one setting key may take.
pub const MAX_SETTING_KEY_BYTES: usize = 128;

/// Whether `key` is a setting key: at least two dot-separated segments, each one or more ASCII
/// letters, digits, `_` or `-`, and at most [`MAX_SETTING_KEY_BYTES`] bytes in all.
#[must_use]
pub fn is_valid_key(key: &str) -> bool {
    key.len() <= MAX_SETTING_KEY_BYTES
        && key.contains('.')
        && key.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

/// Writes of settings not yet applied: a value to store, or `None` to remove the setting. A later
/// write of a key replaces an earlier one.
pub(crate) type Writes = BTreeMap<String, Option<Value>>;

/// Checks that `key` and `value` are a setting, and that storing them in `current` stays within
/// the bounds.
///
/// # Errors
///
/// `cmd.params.invalid` (detail `what` = `setting_key` or `setting_value`) for a malformed key or
/// a value that does not serialize; `cmd.params.limit_exceeded` (detail `limit`) when the value is
/// larger than [`MAX_SETTING_BYTES`] or a new key would exceed [`MAX_SETTINGS`].
pub(crate) fn check(key: &str, value: &Value, current: &BTreeMap<String, Value>) -> Result<()> {
    if !is_valid_key(key) {
        return Err(Error::new(codes::PARAMS_INVALID).with_detail("what", "setting_key"));
    }
    let size = serde_json::to_vec(value)
        .map_err(|_| Error::new(codes::PARAMS_INVALID).with_detail("what", "setting_value"))?
        .len();
    if size > MAX_SETTING_BYTES {
        return Err(Error::new(codes::PARAMS_LIMIT_EXCEEDED).with_detail("limit", "setting_bytes"));
    }
    if !current.contains_key(key) && current.len() >= MAX_SETTINGS {
        return Err(
            Error::new(codes::PARAMS_LIMIT_EXCEEDED).with_detail("limit", "setting_entries")
        );
    }
    Ok(())
}

/// The settings of a session. Clones share them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Settings(Arc<Mutex<BTreeMap<String, Value>>>);

impl Settings {
    fn map(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Value>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn get(&self, key: &str) -> Option<Value> {
        self.map().get(key).cloned()
    }

    /// The settings as they are once `writes` are applied.
    pub(crate) fn projected(&self, writes: &Writes) -> BTreeMap<String, Value> {
        let mut map = self.map().clone();
        for (key, value) in writes {
            match value {
                Some(value) => map.insert(key.clone(), value.clone()),
                None => map.remove(key),
            };
        }
        map
    }

    /// Stores `value` under `key`, replacing any value the key had.
    ///
    /// # Errors
    ///
    /// As [`check`].
    pub(crate) fn set(&self, key: &str, value: Value) -> Result<()> {
        let mut map = self.map();
        check(key, &value, &map)?;
        map.insert(key.to_owned(), value);
        Ok(())
    }

    /// Applies `writes` together: all of them or none.
    ///
    /// # Errors
    ///
    /// As [`check`], for the settings the writes give.
    pub(crate) fn apply(&self, writes: &Writes) -> Result<()> {
        let mut map = self.map();
        let next = {
            let mut next = map.clone();
            for (key, value) in writes {
                match value {
                    Some(value) => {
                        check(key, value, &next)?;
                        next.insert(key.clone(), value.clone());
                    }
                    None => {
                        next.remove(key);
                    }
                }
            }
            next
        };
        *map = next;
        Ok(())
    }

    pub(crate) fn remove(&self, key: &str) -> Option<Value> {
        self.map().remove(key)
    }
}
