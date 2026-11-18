//! The commit validators of the drafting types: block nesting and cycles (CORE-DRAFT-023) and
//! unique names of layers, linetypes and block definitions.
//!
//! A product adds them to every document it opens with [`install`] (or
//! `Document::add_validator` with [`validators`]); they are not stored in the package.
//!
//! **Nesting.** A block definition *contains* the block references whose header owner it is.
//! The nesting depth of a chain of references is the number of references in it: a reference
//! in a space to a definition that contains a reference to a third definition is depth 2. For
//! every block reference a transaction creates, modifies or recreates, the validator follows
//! the definitions below the reference's target and the references above its container, with
//! each definition visited once, and rejects the transaction when
//!
//! - a definition is reached from itself: `draft.block.cycle`, with the detail `cycle` listing
//!   the definitions of the cycle in order, the first repeated at the end, as identifiers
//!   separated by ` -> `; or
//! - the longest chain through the reference is deeper than the limit (default
//!   [`DEFAULT_MAX_NESTING`]): `draft.block.nesting_too_deep` with the argument `limit`.
//!
//! The search stops below the limit, so a cycle longer than the limit is reported as too deep.
//!
//! **Names.** A layer, linetype, block definition or style that a transaction creates or
//! modifies must not have the exact name (byte for byte) of another live object of its type:
//! `draft.name.taken` with the argument `name`.
//!
//! **Layouts.** Layout settings and viewports must be owned by a space of kind `layout`, and a
//! layout space owns at most one settings object (`draft.layout.owner_invalid`,
//! `draft.layout.duplicate`); a touched space of kind `model` owns neither.
//!
//! **Dimension style parents.** A dimension style with a parent has a kind other than `all`,
//! and its parent has the kind `all` and no parent (`draft.style.parent_invalid`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Error, ObjectId};
use arc_doc::relationship::ReferenceField;
use arc_doc::{
    Change, Document, DocumentValidator, FieldPath, Finding, Severity, Snapshot, TypeKey,
};

use crate::error::codes;
use crate::layout::{LAYOUT, VIEWPORT};
use crate::style::{DIM_STYLE, STYLES, dim};
use crate::types::{
    BLOCK, BLOCK_REF, LAYER, LINETYPE, SPACE, SpaceKind, entity, reference_at, table, text_at,
};

/// The default bound on the nesting depth of block references.
pub const DEFAULT_MAX_NESTING: u32 = 64;

fn type_of(snapshot: &Snapshot, id: ObjectId) -> Option<&TypeKey> {
    snapshot.type_key(snapshot.resolve(id).ok()?)
}

fn is(snapshot: &Snapshot, id: ObjectId, key: &str) -> bool {
    type_of(snapshot, id).is_some_and(|k| k.as_str() == key)
}

/// The definitions the block references contained in `block` point to.
fn children(snapshot: &Snapshot, block: ObjectId) -> Vec<ObjectId> {
    snapshot
        .referrers(block)
        .filter(|r| r.field == ReferenceField::Owner && is(snapshot, r.source, BLOCK_REF))
        .filter_map(|r| {
            let object = snapshot.get(snapshot.resolve(r.source).ok()?).ok()?;
            reference_at(&object.values, entity::KIND)
        })
        .filter(|target| is(snapshot, *target, BLOCK))
        .collect()
}

/// The containers of the block references that point to `block`, `None` for a container that
/// is not a block definition.
fn parents(snapshot: &Snapshot, block: ObjectId) -> Vec<Option<ObjectId>> {
    let field = ReferenceField::Field(FieldPath::new("block"));
    snapshot
        .referrers(block)
        .filter(|r| r.field == field && is(snapshot, r.source, BLOCK_REF))
        .filter_map(|r| {
            let object = snapshot.get(snapshot.resolve(r.source).ok()?).ok()?;
            Some(object.header.owner.filter(|o| is(snapshot, *o, BLOCK)))
        })
        .collect()
}

/// What a depth search found.
enum Found {
    /// The longest chain, in references.
    Depth(u32),
    /// A cycle of definitions, the first repeated at the end.
    Cycle(Vec<ObjectId>),
    /// A chain longer than the limit.
    TooDeep,
}

/// The longest chain of references below `start` (0 for a definition without references),
/// following `next`, with an explicit stack and a memo so every definition is expanded once.
fn depth(
    start: ObjectId,
    limit: u32,
    memo: &mut BTreeMap<ObjectId, u32>,
    next: &impl Fn(ObjectId) -> Vec<ObjectId>,
) -> Found {
    if let Some(d) = memo.get(&start) {
        return Found::Depth(*d);
    }
    // Each frame: the definition, its successors and the deepest successor result so far.
    let mut stack: Vec<(ObjectId, Vec<ObjectId>, u32)> = vec![(start, next(start), 0)];
    let mut on_stack: BTreeSet<ObjectId> = BTreeSet::from([start]);
    loop {
        let Some((node, successors, best)) = stack.last_mut() else {
            return Found::Depth(0);
        };
        let node = *node;
        if let Some(child) = successors.pop() {
            if let Some(d) = memo.get(&child) {
                *best = (*best).max(d.saturating_add(1));
                continue;
            }
            if on_stack.contains(&child) {
                let from = stack.iter().position(|(n, _, _)| *n == child).unwrap_or(0);
                let mut cycle: Vec<ObjectId> = stack
                    .get(from..)
                    .unwrap_or_default()
                    .iter()
                    .map(|f| f.0)
                    .collect();
                cycle.push(child);
                return Found::Cycle(cycle);
            }
            if u32::try_from(stack.len()).unwrap_or(u32::MAX) > limit {
                return Found::TooDeep;
            }
            on_stack.insert(child);
            stack.push((child, next(child), 0));
            continue;
        }
        let done = *best;
        stack.pop();
        on_stack.remove(&node);
        memo.insert(node, done);
        match stack.last_mut() {
            Some(parent) => parent.2 = parent.2.max(done.saturating_add(1)),
            None => return Found::Depth(done),
        }
    }
}

fn error(severity_error: Error) -> Finding {
    Finding {
        severity: Severity::Error,
        error: severity_error,
    }
}

/// The nesting and cycle findings of the touched block references.
fn nesting(candidate: &Snapshot, touched: &BTreeMap<ObjectId, Change>, limit: u32) -> Vec<Finding> {
    let too_deep = || {
        vec![error(
            Error::new(codes::BLOCK_NESTING_TOO_DEEP).with_arg("limit", i64::from(limit)),
        )]
    };
    let mut down = BTreeMap::new();
    let mut up = BTreeMap::new();
    let below = |b| children(candidate, b);
    // Above a definition: the definitions containing references to it; a reference in a space
    // leads to `space`, which has nothing above, so it counts one level.
    let space = ObjectId::from_u128(0);
    let above = |b: ObjectId| -> Vec<ObjectId> {
        if b == space {
            return Vec::new();
        }
        parents(candidate, b)
            .into_iter()
            .map(|p| p.unwrap_or(space))
            .collect()
    };
    for (&id, &change) in touched {
        if change == Change::Deleted || !is(candidate, id, BLOCK_REF) {
            continue;
        }
        let Some(object) = candidate
            .resolve(id)
            .ok()
            .and_then(|h| candidate.get(h).ok())
        else {
            continue;
        };
        let Some(target) = reference_at(&object.values, entity::KIND) else {
            continue;
        };
        let container = object.header.owner.filter(|o| is(candidate, *o, BLOCK));
        // The chain through this reference: above its container, the reference, below its target.
        let start = container.unwrap_or(target);
        let below_depth = match depth(start, limit, &mut down, &below) {
            Found::Depth(d) => d,
            Found::Cycle(cycle) => {
                let listed: Vec<String> = cycle.iter().map(ToString::to_string).collect();
                return vec![error(
                    Error::new(codes::BLOCK_CYCLE).with_detail("cycle", listed.join(" -> ")),
                )];
            }
            Found::TooDeep => return too_deep(),
        };
        // `below_depth` counts this reference when `start` is its container; otherwise add it.
        let above_depth = match container {
            None => 1,
            // References in spaces above `start` count one more level than its parents.
            Some(block) => match depth(block, limit, &mut up, &above) {
                Found::Depth(d) => d,
                Found::Cycle(_) | Found::TooDeep => return too_deep(),
            },
        };
        if above_depth.saturating_add(below_depth) > limit {
            return too_deep();
        }
    }
    Vec::new()
}

/// The duplicate-name findings of the touched layers, linetypes and block definitions.
fn names(candidate: &Snapshot, touched: &BTreeMap<ObjectId, Change>) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (&id, &change) in touched {
        if change == Change::Deleted {
            continue;
        }
        let Some(key) = type_of(candidate, id).map(|k| k.as_str().to_owned()) else {
            continue;
        };
        if ![LAYER, LINETYPE, BLOCK].contains(&key.as_str()) && !STYLES.contains(&key.as_str()) {
            continue;
        }
        let Some(object) = candidate
            .resolve(id)
            .ok()
            .and_then(|h| candidate.get(h).ok())
        else {
            continue;
        };
        let Some(name) = text_at(&object.values, table::NAME) else {
            continue;
        };
        let taken = candidate.iter_type(&key).is_ok_and(|mut others| {
            others.any(|(_, other)| {
                other.id != id && text_at(&other.values, table::NAME) == Some(name)
            })
        });
        if taken {
            findings.push(error(Error::new(codes::NAME_TAKEN).with_arg("name", name)));
        }
    }
    findings
}

fn values(
    snapshot: &Snapshot,
    id: ObjectId,
) -> Option<(&arc_doc::Header, &[Option<arc_doc::Value>])> {
    let object = snapshot.get(snapshot.resolve(id).ok()?).ok()?;
    Some((&object.header, &object.values))
}

fn is_layout_space(snapshot: &Snapshot, id: Option<ObjectId>) -> bool {
    id.filter(|id| is(snapshot, *id, SPACE))
        .and_then(|id| values(snapshot, id))
        .is_some_and(|(_, v)| {
            v.get(table::SPACE_KIND) == Some(&Some(arc_doc::Value::Enum(SpaceKind::Layout as u32)))
        })
}

/// The owned objects of `space` of type `key`.
fn owned(snapshot: &Snapshot, space: ObjectId, key: &str) -> Vec<ObjectId> {
    snapshot
        .referrers(space)
        .filter(|r| r.field == ReferenceField::Owner && is(snapshot, r.source, key))
        .map(|r| r.source)
        .collect()
}

/// Layout settings and viewports belong to a layout space, at most one settings object each.
fn layouts(candidate: &Snapshot, touched: &BTreeMap<ObjectId, Change>) -> Vec<Finding> {
    let mut findings = Vec::new();
    let owner_invalid = || error(Error::new(codes::LAYOUT_OWNER_INVALID));
    for (&id, &change) in touched {
        if change == Change::Deleted {
            continue;
        }
        let Some((header, _)) = values(candidate, id) else {
            continue;
        };
        if is(candidate, id, LAYOUT) || is(candidate, id, VIEWPORT) {
            if !is_layout_space(candidate, header.owner) {
                findings.push(owner_invalid());
            } else if let Some(space) = header.owner
                && is(candidate, id, LAYOUT)
                && owned(candidate, space, LAYOUT).len() > 1
            {
                findings.push(error(Error::new(codes::LAYOUT_DUPLICATE)));
            }
        } else if is(candidate, id, SPACE)
            && !is_layout_space(candidate, Some(id))
            && (!owned(candidate, id, LAYOUT).is_empty()
                || !owned(candidate, id, VIEWPORT).is_empty())
        {
            findings.push(owner_invalid());
        }
    }
    findings
}

/// A dimension style with a parent applies to one kind, and its parent applies to all kinds
/// and has no parent.
fn dim_parents(candidate: &Snapshot, touched: &BTreeMap<ObjectId, Change>) -> Vec<Finding> {
    let all = |id: ObjectId| {
        values(candidate, id).is_some_and(|(_, v)| {
            v.get(dim::KIND) == Some(&Some(arc_doc::Value::Enum(0)))
                && reference_at(v, dim::PARENT).is_none()
        })
    };
    let parent_field = ReferenceField::Field(FieldPath::new("parent"));
    let mut findings = Vec::new();
    for (&id, &change) in touched {
        if change == Change::Deleted || !is(candidate, id, DIM_STYLE) {
            continue;
        }
        let Some((_, v)) = values(candidate, id) else {
            continue;
        };
        let parent = reference_at(v, dim::PARENT);
        let child_ok = parent
            .is_none_or(|p| v.get(dim::KIND) != Some(&Some(arc_doc::Value::Enum(0))) && all(p));
        let has_children = candidate
            .referrers(id)
            .any(|r| r.field == parent_field && is(candidate, r.source, DIM_STYLE));
        if !child_ok || (has_children && !all(id)) {
            findings.push(error(Error::new(codes::STYLE_PARENT_INVALID)));
        }
    }
    findings
}

/// The drafting document validators: nesting with the depth bound `max_nesting`, names,
/// layout owners and dimension style parents.
#[must_use]
pub fn validators(max_nesting: u32) -> Vec<DocumentValidator> {
    let nesting: DocumentValidator =
        Arc::new(move |_, candidate, touched| nesting(candidate, touched, max_nesting));
    let names: DocumentValidator = Arc::new(|_, candidate, touched| names(candidate, touched));
    let layouts: DocumentValidator = Arc::new(|_, candidate, touched| layouts(candidate, touched));
    let parents: DocumentValidator =
        Arc::new(|_, candidate, touched| dim_parents(candidate, touched));
    vec![nesting, names, layouts, parents]
}

/// Adds [`validators`] with [`DEFAULT_MAX_NESTING`] to `document`.
pub fn install(document: &Document) {
    for validator in validators(DEFAULT_MAX_NESTING) {
        document.add_validator(validator);
    }
}
