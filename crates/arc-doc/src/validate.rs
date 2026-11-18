//! The validation stages of commit (CORE-DOC-015) besides the deletion rules and the reference
//! check of [`crate::integrity`]: the field schema check, the read-only flag of objects, type
//! validators and document validators.
//!
//! Every stage looks only at the touched identifiers of the transaction, so its cost follows
//! the size of the change, not the size of the document. Document validators receive the touched
//! map and decide themselves what else they read.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::{Error, Limits, ObjectId, Result};

use crate::diagnostic::Diagnostic;
use crate::error::codes;
use crate::op::Op;
use crate::schema::{Finding, Severity, TypeKey, check_values, options_bits_eq};
use crate::snapshot::{Change, Object, Snapshot};

/// A document validator, registered on a [`crate::Document`] by the product: receives the base
/// snapshot, the candidate snapshot and the touched map of a transaction (the net change of each
/// identifier, cascades included) and returns its findings. Errors reject the transaction;
/// warnings become diagnostics. It runs on the document executor inside the commit, after the
/// type validators, and must be fast and free of side effects: work that needs derived data
/// belongs in a dependency-graph node.
pub type DocumentValidator =
    Arc<dyn Fn(&Snapshot, &Snapshot, &BTreeMap<ObjectId, Change>) -> Vec<Finding> + Send + Sync>;

/// Stage 1b: the foreign data rows a transaction writes stay within `limits`: each text column
/// within `string_bytes`, `data` within `input_bytes`, and the candidate holds at most
/// `record_count` rows (`doc.txn.limit_exceeded`, argument `limit`).
pub(crate) fn check_foreign(candidate: &Snapshot, ops: &[Op], limits: &Limits) -> Result<()> {
    let mut written = false;
    for op in ops {
        let Op::ModifyForeign { after, .. } = op else {
            continue;
        };
        written = true;
        let Some(row) = after else { continue };
        let texts = [
            Some(&row.source_format),
            row.source_version.as_ref(),
            row.source_id.as_ref(),
            Some(&row.record_kind),
        ];
        if texts
            .into_iter()
            .flatten()
            .any(|text| size(text.len()) > limits.string_bytes)
        {
            return Err(over("string_bytes", limits.string_bytes));
        }
        if size(row.data.len()) > limits.input_bytes {
            return Err(over("input_bytes", limits.input_bytes));
        }
    }
    if written && size(candidate.foreign_len()) > limits.record_count {
        return Err(over("record_count", limits.record_count));
    }
    Ok(())
}

fn size(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn over(limit: &'static str, value: u64) -> Error {
    Error::new(codes::TXN_LIMIT_EXCEEDED)
        .with_arg("limit", limit)
        .with_detail("value", i64::try_from(value).unwrap_or(i64::MAX))
}

/// The type key and object of the live object `id` in `snapshot`.
fn live(snapshot: &Snapshot, id: ObjectId) -> Option<(&TypeKey, &Arc<Object>)> {
    let handle = snapshot.resolve(id).ok()?;
    Some((snapshot.type_key(handle)?, snapshot.get(handle).ok()?))
}

/// The objects a transaction leaves live and changed: created, modified and recreated.
fn changed_live<'a>(
    candidate: &'a Snapshot,
    touched: &'a BTreeMap<ObjectId, Change>,
) -> impl Iterator<Item = (ObjectId, &'a TypeKey, &'a Arc<Object>)> + 'a {
    touched
        .iter()
        .filter(|(_, change)| **change != Change::Deleted)
        .filter_map(|(&id, _)| live(candidate, id).map(|(key, object)| (id, key, object)))
}

/// Stage 1: every created, modified or recreated object passes the full field schema check of
/// its type ([`check_values`]: count, required fields, types, bounds, enum codes, finiteness).
/// The error keeps its code and `field` argument and gains the details `object` and `type`.
pub(crate) fn check_fields(
    candidate: &Snapshot,
    touched: &BTreeMap<ObjectId, Change>,
) -> Result<()> {
    for (id, key, object) in changed_live(candidate, touched) {
        let schema = candidate
            .registry()
            .get(key.as_str())
            .and_then(|decl| decl.schema(object.version.major))
            .ok_or_else(|| Error::new(codes::TYPE_UNKNOWN).with_arg("type", key.as_str()))?;
        check_values(&schema.fields, &object.values).map_err(|error| {
            error
                .with_detail("object", id.to_string())
                .with_detail("type", key.as_str())
        })?;
    }
    Ok(())
}

/// Part of stage 2, after the deletion rules: an object whose header in the base has the
/// `read_only` flag keeps its values, its owner and its existence; only its name, partition and
/// flags may change, so that a command can clear the flag in a transaction of its own. Fails
/// with `doc.object.read_only` (argument `id`).
pub(crate) fn check_read_only_objects(
    base: &Snapshot,
    candidate: &Snapshot,
    touched: &BTreeMap<ObjectId, Change>,
) -> Result<()> {
    for (&id, &change) in touched {
        let Some((_, before)) = live(base, id) else {
            continue;
        };
        if !before.header.flags.read_only {
            continue;
        }
        let kept = change == Change::Modified
            && live(candidate, id).is_some_and(|(_, after)| {
                after.header.owner == before.header.owner
                    && after.version == before.version
                    && options_bits_eq(&after.values, &before.values)
            });
        if !kept {
            return Err(Error::new(codes::OBJECT_READ_ONLY).with_arg("id", id.to_string()));
        }
    }
    Ok(())
}

/// Sorts findings: the first error fails with its own error, warnings are appended to
/// `warnings` as diagnostics about `objects`.
fn sort_findings(
    findings: Vec<Finding>,
    objects: &[ObjectId],
    origin: &'static str,
    warnings: &mut Vec<Diagnostic>,
) -> Result<()> {
    for finding in findings {
        match finding.severity {
            Severity::Error => {
                let error = finding.error.with_detail("origin", origin);
                return Err(match objects {
                    [one] => error.with_detail("object", one.to_string()),
                    _ => error,
                });
            }
            Severity::Warning => warnings.push(Diagnostic::from_finding(finding, objects.to_vec())),
        }
    }
    Ok(())
}

/// Stage 3: the type validators of every created, modified or recreated object, in identifier
/// order and in each type's validator order. The first error fails the commit with the
/// validator's error, with the details `origin` (`type_validator`) and `object`.
pub(crate) fn run_type_validators(
    candidate: &Snapshot,
    touched: &BTreeMap<ObjectId, Change>,
    warnings: &mut Vec<Diagnostic>,
) -> Result<()> {
    for (id, key, object) in changed_live(candidate, touched) {
        let Some(decl) = candidate.registry().get(key.as_str()) else {
            continue;
        };
        for validator in &decl.validators {
            sort_findings(validator(&object.values), &[id], "type_validator", warnings)?;
        }
    }
    Ok(())
}

/// Stage 4: the document validators in registration order. The first error fails the commit
/// with the validator's error, with the detail `origin` (`document_validator`).
pub(crate) fn run_document_validators(
    validators: &[DocumentValidator],
    base: &Snapshot,
    candidate: &Snapshot,
    touched: &BTreeMap<ObjectId, Change>,
    warnings: &mut Vec<Diagnostic>,
) -> Result<()> {
    for validator in validators {
        sort_findings(
            validator(base, candidate, touched),
            &[],
            "document_validator",
            warnings,
        )?;
    }
    Ok(())
}
