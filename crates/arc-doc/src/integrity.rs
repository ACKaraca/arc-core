//! Reference integrity at commit (CORE-DOC-008, CORE-DOC-009): the on-delete rules, applied to a
//! working set, and the check that no reference dangles.
//!
//! Commit validation runs the field schema check, then [`apply_delete_rules`], then
//! [`check_references`], then type and document validators. Both functions cost time
//! proportional to the touched objects, the references they hold and the referrers of the
//! deleted ones, never to the size of the document.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use arc_base::{Error, Limits, ObjectId, Result};

use crate::error::codes;
use crate::relationship::{
    ReferenceField, changed_fields, references, references_and_cardinality, rewrite_fields, spec_of,
};
use crate::schema::{FieldPath, OnDelete, ReferenceSpec, Strength, TypeKey};
use crate::snapshot::{Change, Object, Snapshot, WorkingSet};

/// An object deleted by a cascade, with its state before the deletion.
#[derive(Debug, Clone, PartialEq)]
pub struct CascadeDelete {
    /// The type key of the object.
    pub type_key: TypeKey,
    /// The object before deletion: identifier, schema version, header and values.
    pub before: Arc<Object>,
}

/// An object whose references to deleted objects were cleared.
#[derive(Debug, Clone, PartialEq)]
pub struct ClearedFields {
    /// The object before the change; its state after is read from the working set.
    pub before: Arc<Object>,
    /// The object fields whose values changed, in field order. A list that lost an element is
    /// named as a whole, since the indices of later elements change.
    pub fields: Vec<FieldPath>,
}

/// What [`apply_delete_rules`] changed beyond the working set's own operations.
///
/// The journal layer records it as operations of the same transaction, after the operations
/// that caused it: one `delete` per entry of `deleted`, in order, then one `modify` per entry of
/// `cleared` with before values from `before` and after values from the candidate.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeleteReport {
    /// Objects deleted by cascades, in the order they were deleted.
    pub deleted: Vec<CascadeDelete>,
    /// Objects whose references were cleared, in ascending order of identifier.
    pub cleared: Vec<ClearedFields>,
}

impl DeleteReport {
    /// True when the rules changed nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deleted.is_empty() && self.cleared.is_empty()
    }
}

/// Applies the deletion rules to every object of the base that the working set deleted
/// ([`Change::Deleted`]) and to every object deleted on the way:
///
/// - each live referrer whose field has on-delete `cascade`, and each object whose header owner
///   is deleted, is deleted too;
/// - each target of an `owning` reference of a deleted object is deleted too, unless a live
///   object holds an owning reference to it by now;
/// - each live referrer whose field has on-delete `block` refuses the deletion with
///   `doc.reference.delete_blocked` (arguments `field`; details `source`, `target`), unless a
///   cascade deletes that referrer as well;
/// - each live referrer whose field has on-delete `clear` loses the reference: an optional field
///   becomes absent, a list loses the element.
///
/// Cascades run iteratively and visit each object once, so chains of any length and cycles of
/// owning references terminate. More than `limits.record_count` objects deleted by cascades
/// fails with `doc.reference.cascade_limit`. The rules follow the net change relative to the
/// base: the owning references of an object of the base are read from the base, and an object
/// created and deleted in the same working set leaves no rule to apply, so a reference to it
/// dangles. Any failure restores the working set to its state before the call.
pub fn apply_delete_rules(set: &mut WorkingSet, limits: &Limits) -> Result<DeleteReport> {
    let savepoint = set.savepoint();
    let outcome = cascade(set, limits);
    match outcome {
        Ok(report) => {
            set.release_savepoint(&savepoint)?;
            Ok(report)
        }
        Err(error) => {
            set.rollback_to(&savepoint)?;
            set.release_savepoint(&savepoint)?;
            Err(error)
        }
    }
}

/// An object deleted and still to be processed.
struct Pending {
    type_key: TypeKey,
    object: Arc<Object>,
}

/// The state of one run of the deletion rules.
struct Cascade {
    limit: u64,
    /// Every identifier deleted so far: by the working set and by the cascades.
    dead: BTreeSet<ObjectId>,
    queue: VecDeque<Pending>,
    report: DeleteReport,
}

impl Cascade {
    /// Deletes the live object `id` as a consequence of another deletion.
    fn delete(&mut self, set: &mut WorkingSet, id: ObjectId) -> Result<()> {
        if u64::try_from(self.report.deleted.len()).map_or(true, |n| n >= self.limit) {
            return Err(Error::new(codes::REFERENCE_CASCADE_LIMIT)
                .with_arg("limit", i64::try_from(self.limit).unwrap_or(i64::MAX)));
        }
        let handle = set.current().resolve(id)?;
        let type_key = set
            .current()
            .type_key(handle)
            .cloned()
            .ok_or_else(not_found)?;
        let before = set.delete(handle)?;
        self.dead.insert(id);
        self.report.deleted.push(CascadeDelete {
            type_key: type_key.clone(),
            before: Arc::clone(&before),
        });
        self.queue.push_back(Pending {
            type_key,
            object: before,
        });
        Ok(())
    }
}

fn cascade(set: &mut WorkingSet, limits: &Limits) -> Result<DeleteReport> {
    let registry = Arc::clone(set.current().registry());
    let mut run = Cascade {
        limit: limits.record_count,
        dead: BTreeSet::new(),
        queue: VecDeque::new(),
        report: DeleteReport::default(),
    };
    let base = set.base();
    for (&id, change) in set.touched() {
        if *change != Change::Deleted {
            continue;
        }
        let handle = base.resolve(id)?;
        let object = Arc::clone(base.get(handle)?);
        let type_key = base.type_key(handle).cloned().ok_or_else(not_found)?;
        run.dead.insert(id);
        run.queue.push_back(Pending { type_key, object });
    }
    let mut blocks: Vec<(ObjectId, ReferenceField, ObjectId)> = Vec::new();
    let mut clears = BTreeSet::new();
    while let Some(Pending { type_key, object }) = run.queue.pop_front() {
        // The targets this object owned.
        for reference in references(&registry, type_key.as_str(), &object) {
            let owning = reference.spec.rule.strength == Strength::Owning;
            if !owning
                || run.dead.contains(&reference.target)
                || set.current().resolve(reference.target).is_err()
                || owned_by_live(set.current(), reference.target)
            {
                continue;
            }
            run.delete(set, reference.target)?;
        }
        // The objects that refer to it.
        let referrers: Vec<_> = set.current().referrers(object.id).cloned().collect();
        for referrer in referrers {
            if run.dead.contains(&referrer.source) {
                continue;
            }
            let Some(spec) = live_spec(set.current(), referrer.source, &referrer.field) else {
                continue;
            };
            match spec.rule.on_delete {
                OnDelete::Cascade => run.delete(set, referrer.source)?,
                OnDelete::Block => blocks.push((referrer.source, referrer.field, object.id)),
                OnDelete::Clear => {
                    clears.insert(referrer.source);
                }
            }
        }
    }
    let Cascade {
        dead, mut report, ..
    } = run;
    // Blocks and clears wait for the cascades, which may delete the referrer anyway.
    if let Some((source, field, target)) = blocks
        .into_iter()
        .find(|(source, _, _)| !dead.contains(source))
    {
        return Err(Error::new(codes::REFERENCE_DELETE_BLOCKED)
            .with_arg("field", field.to_string())
            .with_detail("source", source.to_string())
            .with_detail("target", target.to_string()));
    }
    for source in clears.into_iter().filter(|source| !dead.contains(source)) {
        let current = set.current();
        let handle = current.resolve(source)?;
        let before = Arc::clone(current.get(handle)?);
        let type_key = current.type_key(handle).cloned().ok_or_else(not_found)?;
        let Some(fields) = registry
            .get(type_key.as_str())
            .and_then(|decl| decl.schema(before.version.major))
            .map(|schema| schema.fields.as_slice())
        else {
            continue;
        };
        let mut values = before.values.clone();
        rewrite_fields(fields, &mut values, &mut |spec, target| {
            let cleared = spec.rule.on_delete == OnDelete::Clear && dead.contains(&target);
            (!cleared).then_some(target)
        });
        let changed = changed_fields(fields, &before.values, &values);
        if changed.is_empty() {
            continue;
        }
        set.replace_values(handle, values)?;
        report.cleared.push(ClearedFields {
            before,
            fields: changed,
        });
    }
    Ok(report)
}

/// True when a live object holds an owning reference to `target`.
fn owned_by_live(snapshot: &Snapshot, target: ObjectId) -> bool {
    owners(snapshot, target).next().is_some()
}

/// The live referrers of `target` through owning references.
fn owners(snapshot: &Snapshot, target: ObjectId) -> impl Iterator<Item = ObjectId> + '_ {
    snapshot.referrers(target).filter_map(move |referrer| {
        let spec = live_spec(snapshot, referrer.source, &referrer.field)?;
        let owning = spec.rule.strength == Strength::Owning
            && matches!(referrer.field, ReferenceField::Field(_));
        owning.then_some(referrer.source)
    })
}

/// The spec of `field` of the live object `source`.
fn live_spec<'a>(
    snapshot: &'a Snapshot,
    source: ObjectId,
    field: &ReferenceField,
) -> Option<&'a ReferenceSpec> {
    let handle = snapshot.resolve(source).ok()?;
    let object = snapshot.get(handle).ok()?;
    let type_key = snapshot.type_key(handle)?;
    spec_of(snapshot.registry(), type_key.as_str(), object, field)
}

fn not_found() -> Error {
    Error::new(codes::OBJECT_NOT_FOUND)
}

fn dangling(source: ObjectId, field: &ReferenceField, target: ObjectId) -> Error {
    Error::new(codes::REFERENCE_DANGLING)
        .with_arg("field", field.to_string())
        .with_detail("source", source.to_string())
        .with_detail("target", target.to_string())
}

/// True when an object of type `key` may be the target of `spec`.
fn allows(spec: &ReferenceSpec, key: &TypeKey) -> bool {
    spec.targets.is_empty() || spec.targets.contains(key)
}

/// Checks the references of the candidate after the deletion rules (CORE-DOC-009):
///
/// - every reference held by a created, modified or recreated object, header owner included,
///   names a live object whose type the field allows; otherwise `doc.reference.dangling`
///   (argument `field`; details `source` and `target`);
/// - no live object refers to an object the working set deleted (`doc.reference.dangling`), and
///   every live referrer of a recreated identifier allows its new type;
/// - every reference field of a touched object satisfies its cardinality: a `one` field holds a
///   reference and a `many` list holds at most its bound (`doc.reference.cardinality`, argument
///   `field`);
/// - no object is the target of more than one owning reference
///   (`doc.reference.owner_conflict`, argument `field`; details `source`, `target`, `owner`).
///
/// The first violation in identifier order fails. The working set is not changed.
pub fn check_references(set: &WorkingSet) -> Result<()> {
    let current = set.current();
    let registry = current.registry();
    for (&id, change) in set.touched() {
        if *change == Change::Deleted {
            if let Some(referrer) = current.referrers(id).next() {
                return Err(dangling(referrer.source, &referrer.field, id));
            }
            continue;
        }
        let handle = current.resolve(id)?;
        let object = current.get(handle)?;
        let type_key = current.type_key(handle).ok_or_else(not_found)?;
        let (held, violation) = references_and_cardinality(registry, type_key.as_str(), object);
        if let Some(path) = violation {
            return Err(Error::new(codes::REFERENCE_CARDINALITY)
                .with_arg("field", path.to_string())
                .with_detail("source", id.to_string()));
        }
        for reference in &held {
            let target_key = current
                .resolve(reference.target)
                .ok()
                .and_then(|target| current.type_key(target));
            if !target_key.is_some_and(|key| allows(reference.spec, key)) {
                return Err(dangling(id, &reference.field, reference.target));
            }
            if reference.spec.rule.strength == Strength::Owning
                && matches!(reference.field, ReferenceField::Field(_))
            {
                // Each owning entry of the target, this object's own included.
                let mut owning = owners(current, reference.target);
                let (first, second) = (owning.next(), owning.next());
                if let (Some(first), Some(second)) = (first, second) {
                    let other = if first == id { second } else { first };
                    return Err(Error::new(codes::REFERENCE_OWNER_CONFLICT)
                        .with_arg("field", reference.field.to_string())
                        .with_detail("source", id.to_string())
                        .with_detail("target", reference.target.to_string())
                        .with_detail("owner", other.to_string()));
                }
            }
        }
        if *change == Change::Recreated {
            for referrer in current.referrers(id) {
                let allowed = live_spec(current, referrer.source, &referrer.field)
                    .is_none_or(|spec| allows(spec, type_key));
                if !allowed {
                    return Err(dangling(referrer.source, &referrer.field, id));
                }
            }
        }
    }
    Ok(())
}
