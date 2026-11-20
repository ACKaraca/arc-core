//! The selection of a session: the current and the previous selection, kept by the
//! session rather than by the document, so a change is never a journal record or an undo step.
//!
//! The keywords `selection` (the current selection), `previous` (the selection before it) and
//! `last` (the product's hook, see [`Session::with_last_objects`](crate::Session::with_last_objects))
//! stand for identifier lists wherever a parameter is a list of identifiers: in typed lines, in
//! prompt answers and in invocation parameters. They are expanded from the session when the
//! command is invoked. Identifiers of objects that no longer exist are dropped when a list is read.

use std::sync::{Arc, Mutex, PoisonError};

use arc_base::{Error, ObjectId, Result};
use arc_doc::Snapshot;
use serde_json::Value;

use crate::context::Session;
use crate::error::codes;

/// The keyword of the current selection.
pub const KEYWORD_CURRENT: &str = "selection";
/// The keyword of the selection before the current one.
pub const KEYWORD_PREVIOUS: &str = "previous";
/// The keyword of the product's last-object hook.
pub const KEYWORD_LAST: &str = "last";

/// The product's answer for the keyword `last`: the objects it names, live or not.
pub type LastHook = Arc<dyn Fn(&Snapshot) -> Vec<ObjectId> + Send + Sync>;

/// True when `text` is one of the three keywords.
#[must_use]
pub fn is_keyword(text: &str) -> bool {
    [KEYWORD_CURRENT, KEYWORD_PREVIOUS, KEYWORD_LAST].contains(&text)
}

#[derive(Debug, Default)]
struct Lists {
    current: Vec<ObjectId>,
    previous: Vec<ObjectId>,
}

/// The selection state of a session. Clones of a session share it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Selections(Arc<Mutex<Lists>>);

fn live(ids: &[ObjectId], snapshot: &Snapshot) -> Vec<ObjectId> {
    ids.iter()
        .copied()
        .filter(|id| snapshot.resolve(*id).is_ok())
        .collect()
}

impl Selections {
    fn lists(&self) -> std::sync::MutexGuard<'_, Lists> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The live current selection; the stored list is pruned to it.
    pub(crate) fn current(&self, snapshot: &Snapshot) -> Vec<ObjectId> {
        let mut lists = self.lists();
        lists.current = live(&lists.current, snapshot);
        lists.current.clone()
    }

    /// The live previous selection; the stored list is pruned to it.
    pub(crate) fn previous(&self, snapshot: &Snapshot) -> Vec<ObjectId> {
        let mut lists = self.lists();
        lists.previous = live(&lists.previous, snapshot);
        lists.previous.clone()
    }

    /// The current selection becomes the previous one and `ids` the current one. `ids` are
    /// stored without duplicates, in order, and live ones only.
    pub(crate) fn record(&self, ids: &[ObjectId], snapshot: &Snapshot) {
        let mut lists = self.lists();
        let mut current = Vec::with_capacity(ids.len());
        for id in live(ids, snapshot) {
            if !current.contains(&id) {
                current.push(id);
            }
        }
        lists.previous = std::mem::replace(&mut lists.current, current);
        lists.previous = live(&lists.previous, snapshot);
    }
}

/// The identifiers a keyword stands for in `session`, live in `snapshot`. `last` needs the hook
/// the product gave the session.
///
/// # Errors
///
/// `cmd.selection.unavailable` (detail `keyword`) for `last` without a hook.
pub(crate) fn keyword_ids(
    keyword: &str,
    session: &Session,
    snapshot: &Snapshot,
) -> Result<Vec<ObjectId>> {
    match keyword {
        KEYWORD_CURRENT => Ok(session.selection.current(snapshot)),
        KEYWORD_PREVIOUS => Ok(session.selection.previous(snapshot)),
        _ => match &session.last {
            Some(hook) => Ok(live(&hook(snapshot), snapshot)),
            None => Err(Error::new(codes::SELECTION_UNAVAILABLE).with_detail("keyword", keyword)),
        },
    }
}

/// True when `schema` is a list of identifiers: an array of `format: uuid` strings, directly or as
/// one alternative of `anyOf` or `oneOf`.
fn is_id_list(schema: &Value) -> bool {
    let uuid_items = schema
        .get("items")
        .and_then(|items| items.get("format"))
        .and_then(Value::as_str)
        == Some("uuid");
    if schema.get("type").and_then(Value::as_str) == Some("array") && uuid_items {
        return true;
    }
    ["anyOf", "oneOf"].iter().any(|key| {
        schema
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|alternatives| alternatives.iter().any(is_id_list))
    })
}

/// Expands the keywords in the identifier-list parameters of `params` (a JSON object) against the
/// parameter schema `root`: a keyword given as the whole value, or as an item of an array, is
/// replaced by the identifiers it stands for. Other values are left as they are.
///
/// # Errors
///
/// `cmd.params.limit_exceeded` (detail `limit`) when the expanded list exceeds the session's
/// `array_length`, and the errors of [`keyword_ids`].
pub(crate) fn expand(
    root: &Value,
    params: &mut Value,
    session: &Session,
    snapshot: &Snapshot,
) -> Result<()> {
    let (Some(properties), Some(object)) = (
        root.get("properties").and_then(Value::as_object),
        params.as_object_mut(),
    ) else {
        return Ok(());
    };
    let limit = session.limits.array_length;
    for (name, schema) in properties {
        if !is_id_list(schema) {
            continue;
        }
        let Some(value) = object.get_mut(name) else {
            continue;
        };
        let items = match value {
            Value::String(text) if is_keyword(text) => vec![Value::String(text.clone())],
            Value::Array(items) => std::mem::take(items),
            _ => continue,
        };
        let mut expanded = Vec::with_capacity(items.len());
        for item in items {
            match item.as_str() {
                Some(text) if is_keyword(text) => {
                    for id in keyword_ids(text, session, snapshot)? {
                        expanded.push(Value::String(id.to_string()));
                    }
                }
                _ => expanded.push(item),
            }
            if u64::try_from(expanded.len()).unwrap_or(u64::MAX) > limit {
                return Err(
                    Error::new(codes::PARAMS_LIMIT_EXCEEDED).with_detail("limit", "array_length")
                );
            }
        }
        *value = Value::Array(expanded);
    }
    Ok(())
}
