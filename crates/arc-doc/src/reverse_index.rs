//! The reverse reference index (CORE-DOC-010): for every target identifier, the objects and
//! fields that refer to it.
//!
//! The index is a persistent ordered set of entries `(target, source, field)`. Entries are spread
//! over 64 shards by target; each shard is an AVL tree of reference-counted nodes ordered by the
//! entry. A write copies the shard table once per working set and then only the nodes on the
//! path to the entry; every other node is shared with the snapshots the working set started from.
//! The height of an AVL tree of `n` nodes is below `1.45 log2(n + 2)`, so the recursive writes
//! and the drop of a tree are bounded by about 90 levels for any document.
//!
//! Looking up the referrers of a target walks one root-to-leaf path to the first entry, then
//! yields the entries in order: `O(log n + k)` for `k` results, amortized constant time per
//! result after the first.

use std::cmp::Ordering;
use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::id::Identifier;

use crate::registry::TypeRegistry;
use crate::relationship::{Reference, ReferenceField, changed_references, references};
use crate::snapshot::{Object, Snapshot};

const SHARDS: usize = 64;

/// One reference to a target: the referring object and the field that holds the reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Referrer {
    /// The referring object.
    pub source: ObjectId,
    /// The field of the referring object, or its header owner.
    pub field: ReferenceField,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    target: ObjectId,
    referrer: Referrer,
}

type Link = Option<Arc<Node>>;

#[derive(Debug, Clone)]
pub(crate) struct Node {
    key: Key,
    height: u8,
    left: Link,
    right: Link,
}

/// The persistent reverse reference index of a snapshot.
#[derive(Debug, Clone)]
pub(crate) struct ReverseIndex {
    shards: Arc<[Link; SHARDS]>,
    len: usize,
}

impl Default for ReverseIndex {
    fn default() -> Self {
        Self {
            shards: Arc::new(std::array::from_fn(|_| None)),
            len: 0,
        }
    }
}

fn shard_of(target: ObjectId) -> usize {
    let raw = target.as_u128();
    // The low bits of version 7 identifiers are random; folding in the high half spreads others.
    usize::try_from((raw ^ (raw >> 64)) % SHARDS as u128).unwrap_or(0)
}

impl ReverseIndex {
    /// Applies the change of one object from `old` to `new` (either absent for a create or a
    /// delete): removes the references only `old` holds and adds those only `new` holds. A
    /// replacement within one major schema version walks only the owner, if it changed, and
    /// the fields whose values differ.
    pub(crate) fn update(
        &mut self,
        registry: &TypeRegistry,
        type_key: &str,
        old: Option<&Object>,
        new: Option<&Object>,
    ) {
        let all = |object: Option<&Object>| {
            object
                .map(|object| references(registry, type_key, object))
                .unwrap_or_default()
        };
        let (before, after) = match (old, new) {
            (Some(old), Some(new)) if old.version.major == new.version.major => {
                changed_references(registry, type_key, old, new)
            }
            _ => (all(old), all(new)),
        };
        let entries = |found: Vec<Reference<'_>>| {
            let mut entries: Vec<(ObjectId, ReferenceField)> = found
                .into_iter()
                .map(|reference| (reference.target, reference.field))
                .collect();
            entries.sort_unstable();
            entries.dedup();
            entries
        };
        let (before, after) = (entries(before), entries(after));
        if before == after {
            return;
        }
        let source = new.or(old).map(|object| object.id);
        let Some(source) = source else { return };
        for (target, field) in &before {
            if after.binary_search(&(*target, field.clone())).is_err() {
                self.remove(*target, source, field.clone());
            }
        }
        for (target, field) in after {
            if before.binary_search(&(target, field.clone())).is_err() {
                self.insert(target, source, field);
            }
        }
    }

    pub(crate) fn insert(&mut self, target: ObjectId, source: ObjectId, field: ReferenceField) {
        let key = Key {
            target,
            referrer: Referrer { source, field },
        };
        if let Some(shard) = Arc::make_mut(&mut self.shards).get_mut(shard_of(target))
            && insert(shard, key)
        {
            self.len += 1;
        }
    }

    fn remove(&mut self, target: ObjectId, source: ObjectId, field: ReferenceField) {
        let key = Key {
            target,
            referrer: Referrer { source, field },
        };
        // Look first, so a miss copies nothing.
        if !self
            .shards
            .get(shard_of(target))
            .is_some_and(|shard| contains(shard, &key))
        {
            return;
        }
        if let Some(shard) = Arc::make_mut(&mut self.shards).get_mut(shard_of(target))
            && remove(shard, &key)
        {
            self.len = self.len.saturating_sub(1);
        }
    }

    /// The entries of `target` in ascending order of source, then field.
    pub(crate) fn referrers(&self, target: ObjectId) -> Referrers<'_> {
        let mut stack = Vec::new();
        let mut current = self.shards.get(shard_of(target)).and_then(Option::as_ref);
        // The path to the first entry whose target is not below `target`.
        while let Some(node) = current {
            if node.key.target < target {
                current = node.right.as_ref();
            } else {
                stack.push(node);
                current = node.left.as_ref();
            }
        }
        Referrers { target, stack }
    }

    #[cfg(test)]
    pub(crate) fn shard_root(&self, target: ObjectId) -> Option<&Arc<Node>> {
        self.shards.get(shard_of(target))?.as_ref()
    }
}

#[cfg(test)]
impl Node {
    pub(crate) fn children(&self) -> [Option<&Arc<Node>>; 2] {
        [self.left.as_ref(), self.right.as_ref()]
    }
}

/// The referrers of one target, from [`Snapshot::referrers`].
#[derive(Debug)]
pub struct Referrers<'a> {
    target: ObjectId,
    /// The nodes still to visit, the next on top.
    stack: Vec<&'a Arc<Node>>,
}

impl<'a> Iterator for Referrers<'a> {
    type Item = &'a Referrer;

    fn next(&mut self) -> Option<&'a Referrer> {
        let node = self.stack.pop()?;
        if node.key.target != self.target {
            self.stack.clear();
            return None;
        }
        let mut current = node.right.as_ref();
        while let Some(child) = current {
            self.stack.push(child);
            current = child.left.as_ref();
        }
        Some(&node.key.referrer)
    }
}

impl Snapshot {
    /// Every reference to `target` held by a live object of the snapshot, as the referring object
    /// and field, in ascending order of source identifier and then field.
    ///
    /// The target need not be live: the index records what fields hold, and commit validation
    /// rejects references to missing objects. Finding the first entry walks one tree path of
    /// `O(log n)` nodes for `n` references in the document; each further entry costs amortized
    /// constant time.
    pub fn referrers(&self, target: ObjectId) -> Referrers<'_> {
        self.reverse().referrers(target)
    }

    /// The number of references held by the live objects of the snapshot (entries of the reverse
    /// reference index), in constant time.
    #[must_use]
    pub fn reference_count(&self) -> usize {
        self.reverse().len
    }
}

fn height(link: &Link) -> u8 {
    link.as_ref().map_or(0, |node| node.height)
}

fn contains(link: &Link, key: &Key) -> bool {
    let mut current = link.as_ref();
    while let Some(node) = current {
        current = match key.cmp(&node.key) {
            Ordering::Less => node.left.as_ref(),
            Ordering::Greater => node.right.as_ref(),
            Ordering::Equal => return true,
        };
    }
    false
}

/// Inserts `key`; false when it is present, which copies nothing.
fn insert(link: &mut Link, key: Key) -> bool {
    if contains(link, &key) {
        return false;
    }
    insert_absent(link, key);
    true
}

/// Inserts a key the tree does not hold. Recursion depth is the tree height.
fn insert_absent(link: &mut Link, key: Key) {
    let Some(node) = link else {
        *link = Some(Arc::new(Node {
            key,
            height: 1,
            left: None,
            right: None,
        }));
        return;
    };
    let node = Arc::make_mut(node);
    if key < node.key {
        insert_absent(&mut node.left, key);
    } else {
        insert_absent(&mut node.right, key);
    }
    rebalance(link);
}

/// Removes `key`, which the caller found in the tree.
fn remove(link: &mut Link, key: &Key) -> bool {
    let Some(node) = link else { return false };
    let removed = match key.cmp(&node.key) {
        Ordering::Less => remove(&mut Arc::make_mut(node).left, key),
        Ordering::Greater => remove(&mut Arc::make_mut(node).right, key),
        Ordering::Equal => {
            let Some(node) = link.take() else {
                return false;
            };
            let Node {
                left, mut right, ..
            } = Arc::unwrap_or_clone(node);
            *link = match (left, pop_min(&mut right)) {
                (left, None) => left,
                (left, Some(key)) => Some(Arc::new(Node {
                    key,
                    height: 1,
                    left,
                    right,
                })),
            };
            true
        }
    };
    if removed {
        rebalance(link);
    }
    removed
}

/// Removes and returns the least key.
fn pop_min(link: &mut Link) -> Option<Key> {
    let node = link.as_mut()?;
    if node.left.is_some() {
        let key = pop_min(&mut Arc::make_mut(node).left);
        rebalance(link);
        return key;
    }
    let node = link.take()?;
    let Node { key, right, .. } = Arc::unwrap_or_clone(node);
    *link = right;
    Some(key)
}

/// Restores the height and the AVL balance of the node at `link`, whose subtrees are balanced
/// and differ in height by at most two.
fn rebalance(link: &mut Link) {
    let Some(node) = link.as_mut() else { return };
    let node = Arc::make_mut(node);
    let (left, right) = (height(&node.left), height(&node.right));
    if left > right + 1 {
        if let Some(child) = node.left.as_ref()
            && height(&child.left) < height(&child.right)
        {
            rotate_left(&mut node.left);
        }
        rotate_right(link);
    } else if right > left + 1 {
        if let Some(child) = node.right.as_ref()
            && height(&child.right) < height(&child.left)
        {
            rotate_right(&mut node.right);
        }
        rotate_left(link);
    } else {
        node.height = left.max(right) + 1;
    }
}

fn fix_height(node: &mut Node) {
    node.height = height(&node.left).max(height(&node.right)) + 1;
}

fn rotate_right(link: &mut Link) {
    let Some(mut top) = link.take() else { return };
    let node = Arc::make_mut(&mut top);
    let Some(mut pivot) = node.left.take() else {
        *link = Some(top);
        return;
    };
    let lifted = Arc::make_mut(&mut pivot);
    node.left = lifted.right.take();
    fix_height(node);
    lifted.right = Some(top);
    fix_height(lifted);
    *link = Some(pivot);
}

fn rotate_left(link: &mut Link) {
    let Some(mut top) = link.take() else { return };
    let node = Arc::make_mut(&mut top);
    let Some(mut pivot) = node.right.take() else {
        *link = Some(top);
        return;
    };
    let lifted = Arc::make_mut(&mut pivot);
    node.right = lifted.left.take();
    fix_height(node);
    lifted.left = Some(top);
    fix_height(lifted);
    *link = Some(pivot);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;
    use crate::schema::FieldPath;

    fn id(n: u128) -> ObjectId {
        ObjectId::from_u128(n)
    }

    fn field(n: u8) -> ReferenceField {
        ReferenceField::Field(FieldPath::new(format!("f{n}")))
    }

    /// Checks order, heights and balance of a tree; returns its height.
    fn check(link: &Link, low: Option<&Key>, high: Option<&Key>) -> u8 {
        let Some(node) = link else { return 0 };
        assert!(low.is_none_or(|low| *low < node.key));
        assert!(high.is_none_or(|high| node.key < *high));
        let left = check(&node.left, low, Some(&node.key));
        let right = check(&node.right, Some(&node.key), high);
        assert!(left.abs_diff(right) <= 1, "balanced");
        assert_eq!(node.height, left.max(right) + 1);
        node.height
    }

    #[test]
    fn a_large_index_stays_balanced_and_shares_untouched_nodes() {
        let mut index = ReverseIndex::default();
        // One target with many referrers, inserted in ascending order: the worst case of an
        // unbalanced tree.
        for n in 0..5_000 {
            index.insert(id(7), id(n), field(0));
        }
        let root = index.shard_root(id(7)).cloned();
        check(&root, None, None);
        assert!(root.as_ref().unwrap().height <= 17);
        assert_eq!(index.referrers(id(7)).count(), 5_000);
        assert_eq!(index.referrers(id(8)).count(), 0);

        let before = index.clone();
        index.insert(id(7), id(10_000), field(1));
        let (old, new) = (
            before.shard_root(id(7)).unwrap(),
            index.shard_root(id(7)).unwrap(),
        );
        assert!(!Arc::ptr_eq(old, new));
        // The new entry is the greatest, so the left subtree of the root is shared.
        assert!(Arc::ptr_eq(
            old.children()[0].unwrap(),
            new.children()[0].unwrap()
        ));
        assert_eq!(before.referrers(id(7)).count(), 5_000);
        assert_eq!(index.referrers(id(7)).count(), 5_001);
    }

    #[test]
    fn a_write_walks_only_the_fields_it_changes() {
        use crate::registry::TypeRegistry;
        use crate::schema::{
            Cardinality, FieldDecl, OnCopy, OnDelete, ReferenceSpec, RelationshipRule, Schema,
            SchemaVersion, Strength, TypeDecl, TypeKey, Value, ValueType,
        };
        use crate::snapshot::{Header, Snapshot, WorkingSet};

        let reference = ValueType::Reference(ReferenceSpec {
            targets: Vec::new(),
            rule: RelationshipRule {
                strength: Strength::Associative,
                on_delete: OnDelete::Clear,
                on_copy: OnCopy::ShareTarget,
                cardinality: Cardinality::Many { max: 100_000 },
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
        let refs = Value::List((0..100_000).map(|n| Value::Reference(id(n % 7))).collect());
        let mut set = WorkingSet::new(&Snapshot::empty(Arc::new(registry)));
        let object = Object::new(
            id(1_000),
            SchemaVersion::new(1, 0),
            Header::default(),
            vec![Some(Value::I64(0)), Some(refs.clone())],
        );
        let handle = set.create("test.a", object).unwrap();
        let first = set.current().clone();
        assert_eq!(first.reference_count(), 100_000);
        let mut set = WorkingSet::new(&first);
        set.replace_values(handle, vec![Some(Value::I64(1)), Some(refs)])
            .unwrap();
        let header = Header {
            name: Some("x".into()),
            ..Header::default()
        };
        set.replace_header(handle, header).unwrap();
        // Neither write touched the index: the shard table is still the base's.
        assert!(Arc::ptr_eq(
            &first.reverse().shards,
            &set.current().reverse().shards
        ));
        let owner = Header {
            owner: Some(id(3)),
            ..Header::default()
        };
        set.replace_header(handle, owner).unwrap();
        assert_eq!(set.current().reference_count(), 100_001);
        assert_eq!(set.current().referrers(id(3)).count(), 100_000 / 7 + 2);
    }

    /// A registry with `test.a`: a number and a list of references.
    fn list_registry() -> Arc<crate::registry::TypeRegistry> {
        use crate::schema::{
            Cardinality, FieldDecl, OnCopy, OnDelete, ReferenceSpec, RelationshipRule, Schema,
            SchemaVersion, Strength, TypeDecl, TypeKey, ValueType,
        };
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
        let mut registry = crate::registry::TypeRegistry::new();
        registry
            .register(TypeDecl::new(TypeKey::new("test.a").unwrap(), schema))
            .unwrap();
        Arc::new(registry)
    }

    #[test]
    fn snapshots_keep_their_own_referrers_across_changes() {
        use crate::schema::{SchemaVersion, Value};
        use crate::snapshot::{Header, Snapshot, WorkingSet};

        let refs = |targets: &[u128]| {
            Some(Value::List(
                targets.iter().map(|n| Value::Reference(id(*n))).collect(),
            ))
        };
        let mut set = WorkingSet::new(&Snapshot::empty(list_registry()));
        let object = |n: u128, targets: &[u128]| {
            Object::new(
                id(n),
                SchemaVersion::new(1, 0),
                Header::default(),
                vec![Some(Value::I64(0)), refs(targets)],
            )
        };
        let a = set.create("test.a", object(100, &[1, 2])).unwrap();
        set.create("test.a", object(101, &[2])).unwrap();
        let first = set.current().clone();

        let mut set = WorkingSet::new(&first);
        // A reference change, a create and a delete in one working set.
        set.replace_values(a, vec![Some(Value::I64(0)), refs(&[2, 3])])
            .unwrap();
        set.create("test.a", object(102, &[1])).unwrap();
        let b = first.resolve(id(101)).unwrap();
        set.delete(b).unwrap();
        let second = set.current().clone();

        let sources = |snapshot: &Snapshot, target: u128| -> Vec<u128> {
            snapshot
                .referrers(id(target))
                .map(|r| r.source.as_u128())
                .collect()
        };
        assert_eq!(sources(&first, 1), [100]);
        assert_eq!(sources(&first, 2), [100, 101]);
        assert_eq!(sources(&first, 3), Vec::<u128>::new());
        assert_eq!(first.reference_count(), 3);
        assert_eq!(sources(&second, 1), [102]);
        assert_eq!(sources(&second, 2), [100]);
        assert_eq!(sources(&second, 3), [100]);
        assert_eq!(second.reference_count(), 3);
    }

    proptest! {
        #[test]
        fn the_index_behaves_like_an_ordered_set(
            ops in prop::collection::vec((any::<bool>(), 0_u128..4, 0_u128..40, 0_u8..3), 0..120),
        ) {
            let mut index = ReverseIndex::default();
            let mut model = BTreeSet::new();
            let mut held = Vec::new();
            for (insert, target, source, f) in ops {
                let entry = (id(target), id(source), field(f));
                if insert {
                    index.insert(entry.0, entry.1, entry.2.clone());
                    model.insert(entry);
                } else {
                    index.remove(entry.0, entry.1, entry.2.clone());
                    model.remove(&entry);
                }
                held.push((index.clone(), model.clone()));
            }
            // Every earlier version still reads its own state.
            for (index, model) in &held {
                prop_assert_eq!(index.len, model.len());
                for target in 0..4 {
                    let found: Vec<_> = index
                        .referrers(id(target))
                        .map(|r| (id(target), r.source, r.field.clone()))
                        .collect();
                    let expected: Vec<_> =
                        model.iter().filter(|e| e.0 == id(target)).cloned().collect();
                    prop_assert_eq!(found, expected);
                }
            }
            for shard in index.shards.iter() {
                check(shard, None, None);
            }
        }
    }
}
