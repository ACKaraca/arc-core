//! The inputs evaluators read from the document and the change sets that invalidate them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::ObjectId;

use crate::op::Op;
use crate::registry::TypeRegistry;
use crate::relationship::{Reference, ReferenceField, changed_references, references};
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
        // Foreign data rows are no inputs of derived data.
        for op in ops {
            if let Some(id) = op.object_id() {
                by_id.entry(id).or_default().push(op);
            }
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
        let held = |found: Vec<Reference<'_>>| -> BTreeSet<(ObjectId, ReferenceField)> {
            found
                .into_iter()
                .map(|reference| (reference.target, reference.field))
                .collect()
        };
        let all = |side: Option<(&TypeKey, &Arc<Object>)>| {
            side.map(|(type_key, object)| references(registry, type_key.as_str(), object))
                .unwrap_or_default()
        };
        let (before, after) = match (before, after) {
            (Some((_, old)), Some((_, new))) if Arc::ptr_eq(old, new) => return,
            // Unchanged fields hold the same references on both sides, so they cancel in the
            // symmetric difference and need not be walked.
            (Some((old_key, old)), Some((type_key, new)))
                if old_key == type_key
                    && old.version.major == new.version.major
                    && old.values.len() == new.values.len() =>
            {
                changed_references(registry, type_key.as_str(), old, new)
            }
            (before, after) => (all(before), all(after)),
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
            Op::Create { .. } | Op::Delete { .. } | Op::ModifyForeign { .. } => return None,
        }
    }
    Some(Changed::Parts { header, aspects })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_base::id::Identifier;

    use super::*;
    use crate::registry::TypeRegistry;
    use crate::schema::{
        Cardinality, FieldDecl, OnCopy, OnDelete, ReferenceSpec, RelationshipRule, Schema,
        SchemaVersion, Strength, TypeDecl, Value, ValueType,
    };
    use crate::snapshot::{Header, Snapshot, WorkingSet};

    fn id(n: u128) -> ObjectId {
        ObjectId::from_u128(n)
    }

    #[test]
    fn only_targets_whose_reference_changed_have_changed_referrers() {
        let reference = ValueType::Reference(ReferenceSpec {
            targets: Vec::new(),
            rule: RelationshipRule {
                strength: Strength::Associative,
                on_delete: OnDelete::Clear,
                on_copy: OnCopy::ShareTarget,
                cardinality: Cardinality::Many { max: 100 },
            },
        });
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![
                FieldDecl::new("n", ValueType::I64),
                FieldDecl::new("refs", ValueType::List(Box::new(reference))),
            ],
        );
        let mut registry = TypeRegistry::new();
        registry
            .register(TypeDecl::new(TypeKey::new("test.a").unwrap(), schema))
            .unwrap();
        let refs = |targets: &[u128]| {
            Some(Value::List(
                targets.iter().map(|n| Value::Reference(id(*n))).collect(),
            ))
        };
        let values = |n: i64, targets: &[u128]| vec![Some(Value::I64(n)), refs(targets)];
        let mut set = WorkingSet::new(&Snapshot::empty(Arc::new(registry)));
        let object = Object::new(
            id(100),
            SchemaVersion::new(1, 0),
            Header::default(),
            values(0, &[1, 2]),
        );
        let handle = set.create("test.a", object).unwrap();
        let base = set.current().clone();
        let targets = |set: &WorkingSet| -> Vec<u128> {
            ChangeSet::from_commit(set, &[])
                .referrer_targets()
                .map(|t| t.as_u128())
                .collect()
        };

        let mut set = WorkingSet::new(&base);
        set.replace_values(handle, values(1, &[1, 2])).unwrap();
        assert!(targets(&set).is_empty(), "a move changes no references");
        // Fields compare by position: the entry at the same position changed from 2 to 3.
        set.replace_values(handle, values(1, &[1, 3])).unwrap();
        assert_eq!(targets(&set), [2, 3]);
    }

    #[test]
    fn the_diff_reports_owner_and_changed_fields_only() {
        let optional = |name: &str| FieldDecl {
            optional: true,
            ..FieldDecl::new(
                name,
                ValueType::Reference(ReferenceSpec {
                    targets: Vec::new(),
                    rule: RelationshipRule {
                        strength: Strength::Associative,
                        on_delete: OnDelete::Clear,
                        on_copy: OnCopy::ShareTarget,
                        cardinality: Cardinality::Optional,
                    },
                }),
            )
        };
        let schema = Schema::new(SchemaVersion::new(1, 0), vec![optional("a"), optional("b")]);
        let mut registry = TypeRegistry::new();
        registry
            .register(TypeDecl::new(TypeKey::new("test.two").unwrap(), schema))
            .unwrap();
        let reference = |n: Option<u128>| n.map(|n| Value::Reference(id(n)));
        let header = |owner: u128| Header {
            owner: Some(id(owner)),
            ..Header::default()
        };
        let object = |owner: u128, a: Option<u128>, b: Option<u128>| {
            Object::new(
                id(100),
                SchemaVersion::new(1, 0),
                header(owner),
                vec![reference(a), reference(b)],
            )
        };
        let mut set = WorkingSet::new(&Snapshot::empty(Arc::new(registry)));
        let handle = set
            .create("test.two", object(1, Some(10), Some(20)))
            .unwrap();
        let base = set.current().clone();
        let targets = |set: &WorkingSet| -> Vec<u128> {
            ChangeSet::from_commit(set, &[])
                .referrer_targets()
                .map(|t| t.as_u128())
                .collect()
        };
        let edit = |owner: u128, a: Option<u128>, b: Option<u128>| {
            let mut set = WorkingSet::new(&base);
            set.replace_header(handle, header(owner)).unwrap();
            set.replace_values(handle, vec![reference(a), reference(b)])
                .unwrap();
            targets(&set)
        };
        // The owner moves and field a changes; field b is equal.
        assert_eq!(edit(2, Some(11), Some(20)), [1, 2, 10, 11]);
        // A present reference becomes absent, the other field unchanged.
        assert_eq!(edit(1, None, Some(20)), [10]);
        // Both fields hold 30 in the base of this case: only one changes, the target is reported.
        let mut shared = WorkingSet::new(&base);
        shared
            .replace_values(handle, vec![reference(Some(30)), reference(Some(30))])
            .unwrap();
        let base = shared.current().clone();
        let mut set = WorkingSet::new(&base);
        set.replace_values(handle, vec![reference(Some(31)), reference(Some(30))])
            .unwrap();
        assert_eq!(targets(&set), [30, 31]);
        let mut set = WorkingSet::new(&base);
        set.replace_values(handle, vec![reference(Some(30)), reference(Some(31))])
            .unwrap();
        assert_eq!(targets(&set), [30, 31]);
        // One of two fields holding the same target becomes absent: the target is reported.
        let mut set = WorkingSet::new(&base);
        set.replace_values(handle, vec![None, reference(Some(30))])
            .unwrap();
        assert_eq!(targets(&set), [30]);
    }
}
