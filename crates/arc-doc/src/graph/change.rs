//! The inputs evaluators read from the document and the change sets that invalidate them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::ObjectId;

use crate::op::Op;
use crate::registry::TypeRegistry;
use crate::relationship::{ReferenceField, references};
use crate::schema::{PathSegment, TypeKey};
use crate::snapshot::{Change, Object, Snapshot, WorkingSet};

/// One document input an evaluation read, recorded by the [`EvalContext`](super::EvalContext).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Input {
    /// Whether the object is live, and its type key.
    Exists(ObjectId),
    /// The header of the object: owner, name, partition and flags.
    Header(ObjectId),
    /// The fields of the object that belong to one aspect.
    Aspect(ObjectId, Arc<str>),
    /// The set of references that point at the object (its referrers in the reverse index).
    Referrers(ObjectId),
}

/// A modified object's changed header flag and aspects; `None` for a whole change.
pub(super) type Parts<'a> = Option<(bool, &'a BTreeSet<Arc<str>>)>;

/// What a committed change did to one object.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Changed {
    /// Created, deleted or recreated: every input of the object changed.
    Whole,
    /// Modified: the header and the listed aspects changed.
    Parts {
        header: bool,
        aspects: BTreeSet<Arc<str>>,
    },
}

/// The inputs one commit changed: whole objects (created, deleted, recreated), the header and
/// aspects of modified objects, and the targets whose set of referrers changed. The dependency
/// graph marks the nodes that read any of them dirty ([`Graph::apply`](super::Graph::apply)).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSet {
    objects: BTreeMap<ObjectId, Changed>,
    referrers: BTreeSet<ObjectId>,
}

impl ChangeSet {
    /// An empty change set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The change set of a commit: `set` is the committed working set and `ops` every operation
    /// that made its candidate, including those the deletion rules added.
    ///
    /// Created, deleted and recreated objects change as a whole. A modified object changes the
    /// aspect of the object field at the start of each modified path, and its header for each
    /// property or relink operation; a modified object without operations in `ops`, or with a
    /// path that does not resolve, changes as a whole. A target's referrers change when a touched
    /// object gains or loses a reference to it (compared by field), owner included.
    #[must_use]
    pub fn from_commit(set: &WorkingSet, ops: &[Op]) -> Self {
        let registry = set.current().registry();
        let mut by_id: BTreeMap<ObjectId, Vec<&Op>> = BTreeMap::new();
        for op in ops {
            by_id.entry(op.id()).or_default().push(op);
        }
        let mut changes = Self::default();
        for (&id, &change) in set.touched() {
            let before = live(set.base(), id);
            let after = live(set.current(), id);
            changes.diff_references(registry, before, after);
            let parts = match (change, after) {
                (Change::Modified, Some((type_key, _))) => {
                    modified_parts(registry, type_key, by_id.get(&id).map(Vec::as_slice))
                }
                _ => None,
            };
            changes.objects.insert(id, parts.unwrap_or(Changed::Whole));
        }
        changes
    }

    /// Marks every input of `id` changed: its existence, type, header and all aspects.
    pub fn insert_object(&mut self, id: ObjectId) {
        self.objects.insert(id, Changed::Whole);
    }

    /// Marks the header of `id` changed.
    pub fn insert_header(&mut self, id: ObjectId) {
        if let Changed::Parts { header, .. } = self.parts(id) {
            *header = true;
        }
    }

    /// Marks the aspect `aspect` of `id` changed.
    pub fn insert_aspect(&mut self, id: ObjectId, aspect: &str) {
        if let Changed::Parts { aspects, .. } = self.parts(id) {
            aspects.insert(Arc::from(aspect));
        }
    }

    /// Marks the set of referrers of `target` changed.
    pub fn insert_referrers(&mut self, target: ObjectId) {
        self.referrers.insert(target);
    }

    /// Whether the change set changes `input`.
    #[must_use]
    pub fn contains(&self, input: &Input) -> bool {
        let (id, part) = match input {
            Input::Referrers(target) => return self.referrers.contains(target),
            Input::Exists(id) => (id, None),
            Input::Header(id) => (id, Some(None)),
            Input::Aspect(id, aspect) => (id, Some(Some(aspect))),
        };
        match (self.objects.get(id), part) {
            (Some(Changed::Whole), _) => true,
            (Some(Changed::Parts { header, .. }), Some(None)) => *header,
            (Some(Changed::Parts { aspects, .. }), Some(Some(aspect))) => aspects.contains(aspect),
            _ => false,
        }
    }

    /// True when nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.referrers.is_empty()
    }

    /// The objects that changed as a whole, and the modified objects with their changed header
    /// flag and aspects.
    pub(super) fn objects(&self) -> impl Iterator<Item = (ObjectId, Parts<'_>)> {
        self.objects.iter().map(|(&id, changed)| match changed {
            Changed::Whole => (id, None),
            Changed::Parts { header, aspects } => (id, Some((*header, aspects))),
        })
    }

    /// The targets whose referrers changed.
    pub(super) fn referrer_targets(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.referrers.iter().copied()
    }

    fn parts(&mut self, id: ObjectId) -> &mut Changed {
        self.objects.entry(id).or_insert_with(|| Changed::Parts {
            header: false,
            aspects: BTreeSet::new(),
        })
    }

    fn diff_references(
        &mut self,
        registry: &TypeRegistry,
        before: Option<(&TypeKey, &Arc<Object>)>,
        after: Option<(&TypeKey, &Arc<Object>)>,
    ) {
        let held =
            |side: Option<(&TypeKey, &Arc<Object>)>| -> BTreeSet<(ObjectId, ReferenceField)> {
                side.map(|(type_key, object)| {
                    references(registry, type_key.as_str(), object)
                        .into_iter()
                        .map(|reference| (reference.target, reference.field))
                        .collect()
                })
                .unwrap_or_default()
            };
        let (before, after) = (held(before), held(after));
        for (target, _) in before.symmetric_difference(&after) {
            self.referrers.insert(*target);
        }
    }
}

fn live(snapshot: &Snapshot, id: ObjectId) -> Option<(&TypeKey, &Arc<Object>)> {
    let handle = snapshot.resolve(id).ok()?;
    Some((snapshot.type_key(handle)?, snapshot.get(handle).ok()?))
}

/// The header flag and aspects the operations of one modified object change, or `None` when
/// they cannot be told apart from a whole change.
fn modified_parts(
    registry: &TypeRegistry,
    type_key: &TypeKey,
    ops: Option<&[&Op]>,
) -> Option<Changed> {
    let ops = ops.filter(|ops| !ops.is_empty())?;
    let decl = registry.get(type_key.as_str())?;
    let mut header = false;
    let mut aspects = BTreeSet::new();
    for op in ops {
        match op {
            Op::Modify { version, path, .. } => {
                let PathSegment::Field(name) = path.segments().first()? else {
                    return None;
                };
                let field = decl
                    .schema(version.major)?
                    .fields
                    .iter()
                    .find(|field| field.name == *name)?;
                aspects.insert(Arc::from(field.aspect.as_str()));
            }
            Op::ModifyProperties { .. } | Op::Relink { .. } => header = true,
            Op::Create { .. } | Op::Delete { .. } => return None,
        }
    }
    Some(Changed::Parts { header, aspects })
}
