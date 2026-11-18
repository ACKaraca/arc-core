//! Encodings shared by run results, queries and events: change summaries and diagnostics.

use std::collections::BTreeSet;

use arc_base::{ObjectId, TransactionId};
use arc_doc::events::Changes;
use arc_doc::{Diagnostic, Document, Op, Severity};
use serde_json::{Value, json};

use super::errors;

/// The most identifiers a summary lists per kind; counts are always complete.
pub(super) const MAX_LISTED_IDS: usize = 1000;

/// The objects a transaction (or a coalesced event) created, modified and deleted. An object
/// created and deleted within one transaction appears in none.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Summary {
    pub(super) created: BTreeSet<ObjectId>,
    pub(super) modified: BTreeSet<ObjectId>,
    pub(super) deleted: BTreeSet<ObjectId>,
    /// The sets are incomplete: the event covered more objects than it lists.
    pub(super) overflow: bool,
}

impl Summary {
    /// The net effect of `ops`, applied in order.
    pub(super) fn of_ops(ops: &[Op]) -> Self {
        let mut s = Self::default();
        for op in ops {
            // A foreign data operation changes no object.
            let Some(id) = op.object_id() else {
                continue;
            };
            match op {
                // Deleted earlier in the transaction: the object was replaced.
                Op::Create { .. } if s.deleted.remove(&id) => {
                    s.modified.insert(id);
                }
                Op::Create { .. } => {
                    s.created.insert(id);
                }
                Op::Delete { .. } => {
                    s.modified.remove(&id);
                    if !s.created.remove(&id) {
                        s.deleted.insert(id);
                    }
                }
                _ => {
                    if !s.created.contains(&id) {
                        s.modified.insert(id);
                    }
                }
            }
        }
        s
    }

    /// The summary of committed transaction `txn`, read from the journal; empty when it made
    /// no change.
    pub(super) fn of_transaction(document: &Document, txn: Option<TransactionId>) -> Self {
        txn.and_then(|id| {
            document.with_journal(|journal| {
                journal
                    .get(id)
                    .map(|entry| Self::of_ops(&entry.record().ops))
            })
        })
        .unwrap_or_default()
    }

    /// The summary of an event's changes; a recreated object counts as modified.
    pub(super) fn of_changes(changes: &Changes) -> Self {
        let mut s = Self::default();
        match changes {
            Changes::All => s.overflow = true,
            Changes::Listed(by_type) => {
                for groups in by_type.values() {
                    s.created.extend(&groups.created);
                    s.modified.extend(&groups.modified);
                    s.modified.extend(&groups.recreated);
                    s.deleted.extend(&groups.deleted);
                }
            }
        }
        s
    }

    /// `created`, `modified` and `deleted` (identifiers, at most [`MAX_LISTED_IDS`] each),
    /// `counts` and `truncated`.
    pub(super) fn json(&self) -> Value {
        let list = |set: &BTreeSet<ObjectId>| -> Vec<String> {
            set.iter()
                .take(MAX_LISTED_IDS)
                .map(ToString::to_string)
                .collect()
        };
        let longest = self
            .created
            .len()
            .max(self.modified.len())
            .max(self.deleted.len());
        json!({
            "created": list(&self.created),
            "modified": list(&self.modified),
            "deleted": list(&self.deleted),
            "counts": {
                "created": self.created.len(),
                "modified": self.modified.len(),
                "deleted": self.deleted.len(),
            },
            "truncated": self.overflow || longest > MAX_LISTED_IDS,
        })
    }
}

/// A diagnostic as the protocol sends it: the error data plus `severity` and `objects`.
pub(super) fn diagnostic_json(d: &Diagnostic) -> Value {
    let mut entry = errors::error_data(&d.error, serde_json::Map::new());
    if let Some(map) = entry.as_object_mut() {
        let severity = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        map.insert("severity".into(), json!(severity));
        let objects: Vec<String> = d.objects.iter().map(ToString::to_string).collect();
        map.insert("objects".into(), json!(objects));
    }
    entry
}
