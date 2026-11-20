//! Undo and redo for the surfaces that script a document (the protocol methods `history.undo`
//! and `history.redo`).
//!
//! [`step`] undoes or redoes the top step of a document's undo or redo stack through
//! [`Document::undo`] and [`Document::redo`], so the change is journaled as an interactive undo
//! is: a transaction of kind `undo_of` or `redo_of` that names the transaction it reverses. It
//! needs the `edit` permission, because it changes the document, and it answers what it stepped
//! over: the command identifier and the message identifier of the step's last transaction, read
//! from the journal record of the transaction the step reverses.
//!
//! In-process only (no protocol method): [`list`] names the steps of both stacks and
//! [`step_many`] undoes or redoes several at once in one transaction.

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

/// The most steps [`list`] returns per stack.
pub const MAX_LISTED_STEPS: usize = 1_000;

/// One step of a stack, as [`list`] names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepInfo {
    /// The command identifier of the step's last transaction; `None` when the record names none.
    pub command: Option<String>,
    /// The message identifier of that transaction's label; `None` when the record has none.
    pub label: Option<String>,
    /// The number of transactions the step holds (a group or a coalesced gesture holds several).
    pub transactions: usize,
}

/// The steps of both stacks, the next to step first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepList {
    /// The undo stack.
    pub undo: Vec<StepInfo>,
    /// The redo stack.
    pub redo: Vec<StepInfo>,
}

/// The steps of `document`'s undo and redo stacks, at most `limit` (capped at
/// [`MAX_LISTED_STEPS`]) per stack. Needs the `read` permission.
///
/// # Errors
///
/// `cmd.permission_denied` (detail `permission` `read`) without the `read` class.
pub fn list(document: &Document, session: &Session, limit: usize) -> Result<StepList> {
    if !session.permissions.contains(&Permission::Read) {
        return Err(Error::new(codes::PERMISSION_DENIED)
            .with_arg("command", "history.list")
            .with_detail("permission", "read"));
    }
    let limit = limit.min(MAX_LISTED_STEPS);
    let info = |steps: Vec<arc_doc::StepSummary>| {
        steps
            .into_iter()
            .map(|step| StepInfo {
                command: member(&step.command, "command"),
                label: member(&step.label, "message_id"),
                transactions: step.transactions,
            })
            .collect()
    };
    Ok(StepList {
        undo: info(document.undo_step_list(limit)),
        redo: info(document.redo_step_list(limit)),
    })
}

/// Undoes or redoes the top `count` steps of `document` for `session` in one transaction: all
/// or nothing, one revision and one notification. Answers the revision after the step.
///
/// # Errors
///
/// `cmd.permission_denied` without the `edit` class; `doc.undo.sequence_invalid` when `count`
/// is zero, above the stack or inside an open undo
/// group; the commit's errors, which leave the stacks as they were.
pub fn step_many(
    document: &Document,
    session: &Session,
    direction: Direction,
    count: usize,
) -> Result<u64> {
    if !session.permissions.contains(&Permission::Edit) {
        return Err(Error::new(codes::PERMISSION_DENIED)
            .with_arg("command", format!("history.{}", direction.name()))
            .with_detail("permission", "edit"));
    }
    let done = match direction {
        Direction::Undo => document.undo_many(count),
        Direction::Redo => document.redo_many(count),
    }
    .wait()?;
    Ok(done.snapshot.revision())
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
    use arc_base::ObjectId;
    use arc_doc::graph::Evaluators;
    use arc_doc::{
        FieldDecl, Header, Schema, SchemaVersion, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
    };
    use std::sync::Arc;

    use crate::spec::Surface;
    use arc_base::id::Identifier;

    fn document() -> Document {
        let mut registry = TypeRegistry::new();
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![FieldDecl::new("n", ValueType::I64)],
        );
        let key = TypeKey::new("test.n").unwrap();
        registry.register(TypeDecl::new(key, schema)).unwrap();
        Document::new(Arc::new(registry), Arc::new(Evaluators::new())).unwrap()
    }

    #[test]
    fn steps_are_listed_with_their_commands_and_stepped_in_batches() {
        let doc = document();
        for n in 1..=3u128 {
            doc.submit(move |txn| {
                txn.meta_mut().command = format!(r#"{{"command":"c.{n}"}}"#);
                txn.meta_mut().label = format!(r#"{{"message_id":"m.{n}"}}"#);
                txn.create(
                    "test.n",
                    ObjectId::from_u128(n),
                    Header::default(),
                    vec![Some(Value::I64(0))],
                )
                .map(drop)
            })
            .wait()
            .unwrap();
        }
        let full = Session::new(Surface::Automation, [Permission::Read, Permission::Edit]);
        let listed = list(&doc, &full, 2).unwrap();
        assert_eq!(listed.undo.len(), 2);
        assert_eq!(listed.undo[0].command.as_deref(), Some("c.3"));
        assert_eq!(listed.undo[1].label.as_deref(), Some("m.2"));
        assert!(listed.redo.is_empty());

        let read_only = Session::new(Surface::Automation, [Permission::Read]);
        let denied = step_many(&doc, &read_only, Direction::Undo, 2).unwrap_err();
        assert_eq!(denied.code(), "cmd.permission_denied");
        let blind = Session::new(Surface::Automation, [Permission::Edit]);
        assert_eq!(
            list(&doc, &blind, 1).unwrap_err().code(),
            "cmd.permission_denied"
        );

        let before = doc.snapshot().revision();
        assert_eq!(
            step_many(&doc, &full, Direction::Undo, 9)
                .unwrap_err()
                .code(),
            "doc.undo.sequence_invalid"
        );
        assert_eq!(doc.snapshot().revision(), before);
        assert_eq!(
            step_many(&doc, &full, Direction::Undo, 2).unwrap(),
            before + 1
        );
        assert_eq!(doc.snapshot().iter().count(), 1);
        let listed = list(&doc, &full, usize::MAX).unwrap();
        assert_eq!((listed.undo.len(), listed.redo.len()), (1, 2));
        assert_eq!(listed.redo[0].command.as_deref(), Some("c.2"));
        step_many(&doc, &full, Direction::Redo, 2).unwrap();
        assert_eq!(doc.snapshot().iter().count(), 3);
    }

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
