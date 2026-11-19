//! Undo and redo for the surfaces that script a document (the protocol methods `history.undo`
//! and `history.redo`).
//!
//! [`step`] undoes or redoes the top step of a document's undo or redo stack through
//! [`Document::undo`] and [`Document::redo`], so the change is journaled as an interactive undo
//! is: a transaction of kind `undo_of` or `redo_of` that names the transaction it reverses. It
//! needs the `edit` permission, because it changes the document, and it answers what it stepped
//! over: the command identifier and the message identifier of the step's last transaction, read
//! from the journal record of the transaction the step reverses.

use arc_base::{Error, Result};
use arc_doc::{Document, TxnKind};
use serde_json::Value;

use crate::context::Session;
use crate::error::codes;
use crate::spec::Permission;

/// Which way the stacks move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Undo the top undo step.
    Undo,
    /// Redo the top redo step.
    Redo,
}

impl Direction {
    /// The operation's name as the protocol and the error details spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
        }
    }
}

/// What a step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stepped {
    /// The command identifier of the step's last transaction; `None` when the record does not
    /// name one (a transaction that no command made).
    pub command: Option<String>,
    /// The message identifier of that transaction's label; `None` when the record has none.
    pub label: Option<String>,
    /// The revision of the document after the step.
    pub revision: u64,
}

/// Undoes or redoes the top step of `document` for `session`.
///
/// # Errors
///
/// `cmd.permission_denied` (detail `permission` `edit`) without the `edit` class;
/// `cmd.history.empty` when the stack has no step; the document's own errors, such as
/// `doc.undo.sequence_invalid` inside an open undo group, and the commit's errors, which leave
/// the stacks as they were.
pub fn step(document: &Document, session: &Session, direction: Direction) -> Result<Stepped> {
    if !session.permissions.contains(&Permission::Edit) {
        return Err(Error::new(codes::PERMISSION_DENIED)
            .with_arg("command", format!("history.{}", direction.name()))
            .with_detail("permission", "edit"));
    }
    let done = match direction {
        Direction::Undo => document.undo(),
        Direction::Redo => document.redo(),
    }
    .wait()?;
    if !done.value {
        return Err(Error::new(codes::HISTORY_EMPTY).with_arg("operation", direction.name()));
    }
    // The transaction the operation committed names the one it reverses; that record carries
    // the command and the label. Reading through the commit's own identifier keeps the answer
    // right when another writer committed in between.
    let reversed = done
        .txn_id
        .and_then(|txn| document.with_journal(|journal| named_target(journal, txn)));
    let (command, label) = reversed.unwrap_or((None, None));
    Ok(Stepped {
        command,
        label,
        revision: done.snapshot.revision(),
    })
}

/// The longest chain of undo and redo records followed to a named one. A redo record names no
/// command, so an undo of a redo follows back to the step that redo restored.
const MAX_CHAIN: usize = 64;

/// The command and message identifiers of the record that `txn` reverses, following undo and
/// redo records (each names the record it reverses) until one names a command. `None` when the
/// chain ends, leaves the journal, or is longer than `MAX_CHAIN`.
fn named_target(
    journal: &arc_doc::Journal,
    txn: arc_base::TransactionId,
) -> Option<(Option<String>, Option<String>)> {
    let mut at = match journal.get(txn)?.record().kind {
        TxnKind::UndoOf(target) | TxnKind::RedoOf(target) => target,
        _ => return None,
    };
    for _ in 0..MAX_CHAIN {
        let record = journal.get(at)?.record();
        let command = member(&record.command, "command");
        if command.is_some() {
            return Some((command, member(&record.label, "message_id")));
        }
        at = match record.kind {
            TxnKind::UndoOf(target) | TxnKind::RedoOf(target) => target,
            _ => return None,
        };
    }
    None
}

/// The text member `name` of the opaque JSON text `text`.
fn member(text: &str, name: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    value.get(name)?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_are_read_from_the_opaque_record_text() {
        assert_eq!(
            member(r#"{"command":"a.b.c","params":{}}"#, "command"),
            Some("a.b.c".into())
        );
        assert_eq!(
            member(r#"{"message_id":"m"}"#, "message_id"),
            Some("m".into())
        );
        assert_eq!(member("", "command"), None);
        assert_eq!(member("[1]", "command"), None);
        assert_eq!(member(r#"{"command":1}"#, "command"), None);
    }
}
