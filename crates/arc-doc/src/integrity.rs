//! Reference integrity at commit (CORE-DOC-008, CORE-DOC-009): the on-delete rules, applied to a
//! working set, and the check that no reference dangles.
//!
//! Commit validation runs the field schema check, then the deletion rules
//! ([`crate::txn::Txn::apply_delete_rules`] inside a transaction, [`apply_delete_rules`] on a bare
//! working set), then [`check_references`], then type and document validators. The deletion
//! rules cost time proportional to the deleted objects, the references they hold, their
//! referrers and the referrers of the objects they own. The reference check costs time
//! proportional to the touched objects and the references they hold, the referrers of deleted
//! and recreated identifiers, and the referrers of the targets of owning references the working
//! set added; it does not depend on the size of the rest of the document.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use arc_base::{Error, Limits, ObjectId, Result};

use crate::error::codes;
use crate::op::Op;
use crate::relationship::{
    ReferenceField, changed_fields, references, references_and_cardinality, rewrite_fields, spec_of,
};
use crate::reverse_index::Referrer;
use crate::schema::{FieldPath, OnDelete, ReferenceSpec, Strength, TypeKey, options_bits_eq};
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
    /// The object before the change.
    pub before: Arc<Object>,
    /// The object after the change.
    pub after: Arc<Object>,
    /// The object fields whose values changed, in field order. A list that lost an element is
    /// named as a whole, since the indices of later elements change.
    pub fields: Vec<FieldPath>,
}

/// What the deletion rules changed beyond the operations that caused them.
///
/// [`DeleteReport::operations`] turns it into the operations the journal records after the
/// operations that caused it; [`crate::txn::Txn::apply_delete_rules`] appends them to the
/// transaction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeleteReport {
    /// Objects deleted by cascades, in the order they were deleted.
    pub deleted: Vec<CascadeDelete>,
    /// Objects whose references were cleared, in ascending order of identifier, after every
    /// cascade deletion.
    pub cleared: Vec<ClearedFields>,
}

impl DeleteReport {
    /// True when the rules changed nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deleted.is_empty() && self.cleared.is_empty()
    }

    /// The operations that reproduce the report, in the order the rules applied them: one
    /// [`Op::Delete`] per cascade deletion in order, then for each cleared object one
    /// [`Op::Modify`] per changed field, in field order, with the whole field's before and
    /// after values.
    #[must_use]
    pub fn operations(&self) -> Vec<Op> {
        let deletes = self.deleted.iter().map(|delete| Op::Delete {
            type_key: delete.type_key.clone(),
            object: Arc::clone(&delete.before),
        });
        let modifies = self.cleared.iter().flat_map(|cleared| {
            let changed = cleared
                .before
                .values
                .iter()
                .zip(&cleared.after.values)
                .filter(|(before, after)| {
                    !options_bits_eq(std::slice::from_ref(*before), std::slice::from_ref(*after))
                });
            changed
                .zip(&cleared.fields)
                .map(|((before, after), path)| Op::Modify {
                    id: cleared.before.id,
                    version: cleared.before.version,
                    path: path.clone(),
                    before: before.clone(),
                    after: after.clone(),
                })
        });
        deletes.chain(modifies).collect()
    }
}

/// The states of an identifier whose owning references apply, as a transaction knows them, or
/// `None` without a transaction: then a deleted object's state is read from the base, and a
/// recreated object counts only when its type key differs from the base's.
pub(crate) type DeletedStates<'a> = &'a dyn Fn(ObjectId) -> Option<Vec<(TypeKey, Arc<Object>)>>;

/// Applies the deletion rules to a working set without a transaction, reading the state of each
/// object the working set deleted from the base: a working set keeps no state between the base
/// and the candidate. [`crate::txn::Txn::apply_delete_rules`] reads the state each object had when
/// the transaction deleted it instead.
///
/// The rules start from every object of the base that the working set deleted
/// ([`Change::Deleted`]), every object of the base that it deleted and created again with another
/// type key ([`Change::Recreated`]), and every object deleted on the way:
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
/// An object recreated with another type key is a delete followed by a create for the block,
/// owning and header-owner rules: the targets the old object owned are deleted unless a live
/// object owns them, and the references of the base to it that the working set kept (entries of
/// the base's reverse index still present in the candidate) refuse with `block` or, as header
/// owners, delete their objects. References it holds or receives otherwise are checked by
/// [`check_references`] against the new type. An object recreated with the same type key keeps
/// its referrers, and the targets the old object owned stay, as when an owning reference is
/// replaced.
///
/// Cascades run iteratively and visit each object once, so chains of any length and cycles of
/// owning references terminate. More than `limits.record_count` objects deleted by cascades
/// fails with `doc.reference.cascade_limit`. An object created and deleted in the same working
/// set leaves no rule to apply, so a reference to it dangles. Any failure restores the working
/// set to its state before the call; savepoints taken before the call stay valid.
pub fn apply_delete_rules(set: &mut WorkingSet, limits: &Limits) -> Result<DeleteReport> {
    let savepoint = set.savepoint();
    let outcome = run(set, limits, &|_| None);
    if outcome.is_err() {
        set.rollback_to(&savepoint)?;
    }
    set.release_savepoint(&savepoint)?;
    outcome
}

/// An object deleted, or recreated with another type, and still to be processed.
struct Pending {
    id: ObjectId,
    /// The states whose owning references are followed.
    states: Vec<(TypeKey, Arc<Object>)>,
    /// Recreated with another type: only the base's references to it apply, and only the
    /// header-owner and block rules.
    recreated: bool,
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
            id,
            states: vec![(type_key, before)],
            recreated: false,
        });
        Ok(())
    }
}

/// Queues the objects of the base that `set` deleted, or recreated with another type key, with
/// the states whose owning references apply.
fn roots(set: &WorkingSet, states: DeletedStates<'_>, run: &mut Cascade) -> Result<()> {
    let base = set.base();
    for (&id, change) in set.touched() {
        let recreated = match change {
            Change::Deleted => false,
            Change::Recreated => true,
            Change::Created | Change::Modified => continue,
        };
        let handle = base.resolve(id)?;
        let type_key = base.type_key(handle).cloned().ok_or_else(not_found)?;
        let known = states(id);
        if recreated {
            let changed = match &known {
                Some(known) => !known.is_empty(),
                None => {
                    let now = set.current();
                    now.resolve(id).ok().and_then(|handle| now.type_key(handle)) != Some(&type_key)
                }
            };
            if !changed {
                continue;
            }
        } else {
            run.dead.insert(id);
        }
        let mut known = known.unwrap_or_default();
        if known.is_empty() {
            known.push((type_key, Arc::clone(base.get(handle)?)));
        }
        run.queue.push_back(Pending {
            id,
            states: known,
            recreated,
        });
    }
    Ok(())
}

/// Runs the deletion rules on `set`; `states` gives the states the working set deleted, or
/// nothing to read them from the base. On failure the working set is left partly changed: the
/// caller restores it.
pub(crate) fn run(
    set: &mut WorkingSet,
    limits: &Limits,
    states: DeletedStates<'_>,
) -> Result<DeleteReport> {
    let registry = Arc::clone(set.current().registry());
    let base = set.base().clone();
    let mut run = Cascade {
        limit: limits.record_count,
        dead: BTreeSet::new(),
        queue: VecDeque::new(),
        report: DeleteReport::default(),
    };
    roots(set, states, &mut run)?;
    let mut blocks: Vec<(ObjectId, ReferenceField, ObjectId)> = Vec::new();
    let mut clears = BTreeSet::new();
    while let Some(pending) = run.queue.pop_front() {
        // The targets this object owned.
        for (type_key, object) in &pending.states {
            for reference in references(&registry, type_key.as_str(), object) {
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
        }
        // The objects that refer to it.
        let referrers: Vec<Referrer> = if pending.recreated {
            let kept: BTreeSet<&Referrer> = set.current().referrers(pending.id).collect();
            base.referrers(pending.id)
                .filter(|referrer| kept.contains(referrer))
                .cloned()
                .collect()
        } else {
            set.current().referrers(pending.id).cloned().collect()
        };
        for referrer in referrers {
            if run.dead.contains(&referrer.source) {
                continue;
            }
            let Some(spec) = live_spec(set.current(), referrer.source, &referrer.field) else {
                continue;
            };
            let header_owner = referrer.field == ReferenceField::Owner;
            match spec.rule.on_delete {
                OnDelete::Block => blocks.push((referrer.source, referrer.field, pending.id)),
                _ if pending.recreated && !header_owner => {}
                OnDelete::Cascade => run.delete(set, referrer.source)?,
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
        let after = Arc::clone(set.current().get(handle)?);
        report.cleared.push(ClearedFields {
            before,
            after,
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
fn owners(snapshot: &Snapshot, target: ObjectId) -> impl Iterator<Item = &Referrer> + '_ {
    snapshot.referrers(target).filter(move |referrer| {
        matches!(referrer.field, ReferenceField::Field(_))
            && live_spec(snapshot, referrer.source, &referrer.field)
                .is_some_and(|spec| spec.rule.strength == Strength::Owning)
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
///   (`doc.reference.owner_conflict`, argument `field`; details `source`, `target`, `owner` and
///   `owner_field`, the other owning reference, which may be another field or list element of
///   the same object). Only owning references the working set added are checked, since the base
///   passed this check: an owning reference of a touched object counts as added unless the base
///   state of the same identifier holds it at the same field path.
///
/// The first violation in identifier order fails. The working set is not changed.
pub fn check_references(set: &WorkingSet) -> Result<()> {
    let current = set.current();
    let base = set.base();
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
        // The owning references of the base state, computed when the first owning one is met.
        let mut owned_before: Option<BTreeSet<(ObjectId, ReferenceField)>> = None;
        for reference in &held {
            let target_key = current
                .resolve(reference.target)
                .ok()
                .and_then(|target| current.type_key(target));
            if !target_key.is_some_and(|key| allows(reference.spec, key)) {
                return Err(dangling(id, &reference.field, reference.target));
            }
            if reference.spec.rule.strength != Strength::Owning
                || reference.field == ReferenceField::Owner
            {
                continue;
            }
            let before = owned_before.get_or_insert_with(|| owning_in(base, id));
            if before.contains(&(reference.target, reference.field.clone())) {
                continue;
            }
            let other = owners(current, reference.target)
                .find(|owner| !(owner.source == id && owner.field == reference.field));
            if let Some(other) = other {
                return Err(Error::new(codes::REFERENCE_OWNER_CONFLICT)
                    .with_arg("field", reference.field.to_string())
                    .with_detail("source", id.to_string())
                    .with_detail("target", reference.target.to_string())
                    .with_detail("owner", other.source.to_string())
                    .with_detail("owner_field", other.field.to_string()));
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

/// The owning field references the object `id` holds in `snapshot`, if it is live there.
fn owning_in(snapshot: &Snapshot, id: ObjectId) -> BTreeSet<(ObjectId, ReferenceField)> {
    let Ok(handle) = snapshot.resolve(id) else {
        return BTreeSet::new();
    };
    let (Ok(object), Some(type_key)) = (snapshot.get(handle), snapshot.type_key(handle)) else {
        return BTreeSet::new();
    };
    references(snapshot.registry(), type_key.as_str(), object)
        .into_iter()
        .filter(|reference| {
            reference.spec.rule.strength == Strength::Owning
                && reference.field != ReferenceField::Owner
        })
        .map(|reference| (reference.target, reference.field))
        .collect()
}
