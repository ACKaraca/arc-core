//! Objects, snapshots, working sets and snapshot publication (CORE-DOC-002, CORE-DOC-003,
//! CORE-DOC-017).
//!
//! A [`Snapshot`] is an immutable view of every object of a document at one revision: one typed
//! store per registered type key, the identifier index, the revision number and the type
//! registry, behind one `Arc`. Cloning it is the constant-time way to take a snapshot. A
//! [`WorkingSet`] starts from a snapshot, applies creates, replacements and deletes with
//! structural sharing, and yields a candidate snapshot; a [`SnapshotCell`] publishes candidates
//! atomically and hands the latest one to readers without locks.
//!
//! This layer stores what it is given. Field schema checks, relationship rules, the journal and
//! events belong to the transaction that wraps a working set.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use arc_base::{Error, ObjectId, Result};
use arc_swap::ArcSwap;

use crate::error::codes;
use crate::index::IdIndex;
use crate::registry::TypeRegistry;
use crate::schema::{SchemaVersion, TypeKey, Value};
use crate::store::{Handle, Store, usize_of};

/// The flags of an object header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Flags {
    /// The object is not listed in user interfaces.
    pub hidden_from_ui: bool,
    /// The object is maintained by the application, not by users.
    pub system: bool,
    /// Commands do not change the object.
    pub read_only: bool,
}

/// The header fields of an object besides its identifier, type key and schema version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// The containing object.
    pub owner: Option<ObjectId>,
    /// The name.
    pub name: Option<String>,
    /// The product-defined partition the object loads with.
    pub partition: Option<String>,
    /// The flags.
    pub flags: Flags,
}

/// An object: its identifier, schema version, header and field values. The type key is the key
/// of the store that holds it ([`Snapshot::type_key`]).
///
/// Objects in a snapshot are immutable and shared by reference counting between revisions.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Object {
    /// The persistent identifier.
    pub id: ObjectId,
    /// The schema version of the values.
    pub version: SchemaVersion,
    /// The header.
    pub header: Header,
    /// One optional value per field of the schema, in declared order.
    pub values: Vec<Option<Value>>,
}

impl Object {
    /// Creates an object.
    #[must_use]
    pub fn new(
        id: ObjectId,
        version: SchemaVersion,
        header: Header,
        values: Vec<Option<Value>>,
    ) -> Self {
        Self {
            id,
            version,
            header,
            values,
        }
    }
}

/// How a working set changed an object relative to its base snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Change {
    /// The object is new.
    Created,
    /// The header or values of the object were replaced.
    Modified,
    /// The object was deleted.
    Deleted,
}

#[derive(Debug, Clone)]
struct Roots {
    registry: Arc<TypeRegistry>,
    /// The registered type keys in ascending order; store `i` holds objects of key `i`.
    keys: Arc<[TypeKey]>,
    revision: u64,
    stores: Arc<Vec<Store>>,
    index: IdIndex,
    // The reverse reference index root (CORE-DOC-010) joins these roots here.
}

/// An immutable view of a document at one revision.
///
/// Cloning a snapshot copies one pointer, whatever the size of the document. Reading an object
/// by handle walks at most five trie levels and takes no lock; resolving an identifier walks at
/// most 26 levels of the identifier index. Iteration is in handle order: by type key, then by
/// slot.
#[derive(Clone)]
pub struct Snapshot(Arc<Roots>);

impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("revision", &self.revision())
            .field("objects", &self.len())
            .finish_non_exhaustive()
    }
}

fn not_found() -> Error {
    Error::new(codes::OBJECT_NOT_FOUND)
}

impl Snapshot {
    /// The empty snapshot of revision 0 with one store per type of `registry`.
    #[must_use]
    pub fn empty(registry: Arc<TypeRegistry>) -> Self {
        let keys: Arc<[TypeKey]> = registry.iter().map(|decl| decl.key.clone()).collect();
        Self(Arc::new(Roots {
            stores: Arc::new(vec![Store::default(); keys.len()]),
            keys,
            registry,
            revision: 0,
            index: IdIndex::default(),
        }))
    }

    /// The revision number: 0 for an empty snapshot, one more than its base for a candidate.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.0.revision
    }

    /// The type registry of the document.
    #[must_use]
    pub fn registry(&self) -> &Arc<TypeRegistry> {
        &self.0.registry
    }

    /// The handle of the live object `id`; an unknown or deleted identifier fails with
    /// `doc.object.not_found`.
    pub fn resolve(&self, id: ObjectId) -> Result<Handle> {
        self.0
            .index
            .get(id)
            .ok_or_else(|| not_found().with_detail("id", id.to_string()))
    }

    /// The object at `handle`; a stale handle fails with `doc.object.not_found`.
    pub fn get(&self, handle: Handle) -> Result<&Arc<Object>> {
        self.0
            .stores
            .get(usize_of(handle.store))
            .and_then(|store| store.get(handle.slot, handle.generation))
            .ok_or_else(not_found)
    }

    /// The type key of the store `handle` points into.
    #[must_use]
    pub fn type_key(&self, handle: Handle) -> Option<&TypeKey> {
        self.0.keys.get(usize_of(handle.store))
    }

    /// The number of live objects.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.stores.iter().map(Store::len).sum()
    }

    /// True when the snapshot holds no object.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every live object with its handle, in handle order.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &Arc<Object>)> + '_ {
        self.0
            .stores
            .iter()
            .zip(0..)
            .flat_map(|(store, index)| objects_of(store, index))
    }

    /// Every live object of type `key` with its handle, in slot order. An unregistered key
    /// fails with `doc.type.unknown`.
    pub fn iter_type(&self, key: &str) -> Result<impl Iterator<Item = (Handle, &Arc<Object>)>> {
        let index = self.store_index(key)?;
        let store = self
            .0
            .stores
            .get(usize_of(index))
            .ok_or_else(|| unknown(key))?;
        Ok(objects_of(store, index))
    }

    fn store_index(&self, key: &str) -> Result<u32> {
        self.0
            .keys
            .binary_search_by(|probe| probe.as_str().cmp(key))
            .ok()
            .and_then(|index| u32::try_from(index).ok())
            .ok_or_else(|| unknown(key))
    }

    #[cfg(test)]
    pub(crate) fn store(&self, key: &str) -> Option<&Store> {
        self.0.stores.get(usize_of(self.store_index(key).ok()?))
    }

    #[cfg(test)]
    pub(crate) fn index(&self) -> &IdIndex {
        &self.0.index
    }
}

fn unknown(key: &str) -> Error {
    Error::new(codes::TYPE_UNKNOWN).with_arg("type", key)
}

fn objects_of(store: &Store, index: u32) -> impl Iterator<Item = (Handle, &Arc<Object>)> {
    store.iter().map(move |(slot, generation, object)| {
        (
            Handle {
                store: index,
                slot,
                generation,
            },
            object,
        )
    })
}

/// A set of changes on top of a base snapshot, not visible to anyone else until its candidate
/// snapshot is published.
///
/// Writes copy only what they touch: the store table and the shard table once, then the chunk,
/// branch nodes and index nodes on the path to each written object. [`WorkingSet::current`] is
/// the candidate; cloning it is a constant-time savepoint, and the next write copies around it.
pub struct WorkingSet {
    base: Snapshot,
    current: Snapshot,
    touched: BTreeMap<ObjectId, Change>,
}

impl fmt::Debug for WorkingSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkingSet")
            .field("base", &self.base.revision())
            .field("touched", &self.touched.len())
            .finish_non_exhaustive()
    }
}

impl WorkingSet {
    /// An empty set of changes on `base`. Its candidate has the next revision number.
    #[must_use]
    pub fn new(base: &Snapshot) -> Self {
        let mut roots = (*base.0).clone();
        roots.revision = roots.revision.saturating_add(1);
        Self {
            base: base.clone(),
            current: Snapshot(Arc::new(roots)),
            touched: BTreeMap::new(),
        }
    }

    /// The snapshot the changes apply to.
    #[must_use]
    pub fn base(&self) -> &Snapshot {
        &self.base
    }

    /// The candidate snapshot: the base with every change of this working set applied. Reads
    /// through it see the uncommitted changes.
    #[must_use]
    pub fn current(&self) -> &Snapshot {
        &self.current
    }

    /// The objects changed relative to the base, by identifier. An object created and then
    /// deleted in the same working set is not listed.
    #[must_use]
    pub fn touched(&self) -> &BTreeMap<ObjectId, Change> {
        &self.touched
    }

    /// Adds `object` to the store of `type_key` and returns its handle.
    ///
    /// Fails with `doc.type.unknown` for an unregistered key, `doc.object.duplicate_id` when the
    /// identifier is live or was deleted in this working set, and `doc.store.capacity_exceeded`
    /// when the store has no slot left. The values are not checked against the schema here.
    pub fn create(&mut self, type_key: &str, object: Object) -> Result<Handle> {
        let store = self.current.store_index(type_key)?;
        let id = object.id;
        if self.current.0.index.get(id).is_some() || self.touched.get(&id) == Some(&Change::Deleted)
        {
            return Err(Error::new(codes::OBJECT_DUPLICATE_ID).with_arg("id", id.to_string()));
        }
        let roots = Arc::make_mut(&mut self.current.0);
        let (slot, generation) = Arc::make_mut(&mut roots.stores)
            .get_mut(usize_of(store))
            .and_then(|entry| entry.insert(Arc::new(object)))
            .ok_or_else(|| Error::new(codes::STORE_CAPACITY_EXCEEDED).with_arg("type", type_key))?;
        let handle = Handle {
            store,
            slot,
            generation,
        };
        roots.index.insert(id, handle);
        self.touched.insert(id, Change::Created);
        Ok(handle)
    }

    /// Replaces the field values of the object at `handle` and returns the previous object. A
    /// stale handle fails with `doc.object.not_found`.
    pub fn replace_values(
        &mut self,
        handle: Handle,
        values: Vec<Option<Value>>,
    ) -> Result<Arc<Object>> {
        let old = self.current.get(handle)?;
        let object = Object {
            header: old.header.clone(),
            values,
            ..**old
        };
        self.replace(handle, object)
    }

    /// Replaces the header of the object at `handle` and returns the previous object. A stale
    /// handle fails with `doc.object.not_found`.
    pub fn replace_header(&mut self, handle: Handle, header: Header) -> Result<Arc<Object>> {
        let old = self.current.get(handle)?;
        let object = Object {
            header,
            values: old.values.clone(),
            ..**old
        };
        self.replace(handle, object)
    }

    fn replace(&mut self, handle: Handle, object: Object) -> Result<Arc<Object>> {
        let id = object.id;
        let old = self
            .store_mut(handle)?
            .replace(handle.slot, handle.generation, Arc::new(object))
            .ok_or_else(not_found)?;
        if self.touched.get(&id) != Some(&Change::Created) {
            self.touched.insert(id, Change::Modified);
        }
        Ok(old)
    }

    /// Deletes the object at `handle` and returns it. A stale handle fails with
    /// `doc.object.not_found`. The handle and every copy of it are stale from now on.
    pub fn delete(&mut self, handle: Handle) -> Result<Arc<Object>> {
        let old = self
            .store_mut(handle)?
            .remove(handle.slot, handle.generation)
            .ok_or_else(not_found)?;
        Arc::make_mut(&mut self.current.0).index.remove(old.id);
        if self.touched.remove(&old.id) != Some(Change::Created) {
            self.touched.insert(old.id, Change::Deleted);
        }
        Ok(old)
    }

    fn store_mut(&mut self, handle: Handle) -> Result<&mut Store> {
        // Check first, so a stale handle copies nothing.
        self.current.get(handle)?;
        let roots = Arc::make_mut(&mut self.current.0);
        Arc::make_mut(&mut roots.stores)
            .get_mut(usize_of(handle.store))
            .ok_or_else(not_found)
    }
}

/// The latest published snapshot of a document.
///
/// Readers load it without locks and keep what they loaded for as long as they need; the writer
/// replaces it atomically with [`SnapshotCell::publish`]. Old snapshots are freed when their
/// last reader drops them.
pub struct SnapshotCell {
    current: ArcSwap<Roots>,
}

impl fmt::Debug for SnapshotCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotCell")
            .field("revision", &self.current.load().revision)
            .finish()
    }
}

impl SnapshotCell {
    /// A cell that publishes `snapshot`.
    #[must_use]
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            current: ArcSwap::new(snapshot.0),
        }
    }

    /// The latest published snapshot, in constant time.
    #[must_use]
    pub fn load(&self) -> Snapshot {
        Snapshot(self.current.load_full())
    }

    /// Publishes `candidate` atomically. Fails with `doc.snapshot.stale`, publishing nothing,
    /// unless the candidate's revision follows the published one directly, that is, unless it
    /// was built on the latest published snapshot.
    pub fn publish(&self, candidate: Snapshot) -> Result<()> {
        let published = self.current.load_full();
        let stale = || {
            Error::new(codes::SNAPSHOT_STALE)
                .with_detail("published_revision", revision_detail(published.revision))
                .with_detail("candidate_revision", revision_detail(candidate.revision()))
        };
        if published.revision.checked_add(1) != Some(candidate.revision()) {
            return Err(stale());
        }
        let previous = self
            .current
            .compare_and_swap(&published, Arc::clone(&candidate.0));
        if Arc::ptr_eq(&previous, &published) {
            Ok(())
        } else {
            Err(stale())
        }
    }
}

fn revision_detail(revision: u64) -> i64 {
    i64::try_from(revision).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;

    use super::*;
    use crate::schema::{FieldDecl, Schema, TypeDecl, ValueType};

    fn registry() -> Arc<TypeRegistry> {
        let mut registry = TypeRegistry::new();
        for key in ["test.a", "test.b"] {
            let schema = Schema::new(
                SchemaVersion::new(1, 0),
                vec![FieldDecl::new("n", ValueType::I64)],
            );
            registry
                .register(TypeDecl::new(TypeKey::new(key).unwrap(), schema))
                .unwrap();
        }
        Arc::new(registry)
    }

    fn object(id: u128, n: i64) -> Object {
        Object::new(
            ObjectId::from_u128(id),
            SchemaVersion::new(1, 0),
            Header::default(),
            vec![Some(Value::I64(n))],
        )
    }

    fn value(snapshot: &Snapshot, handle: Handle) -> Option<i64> {
        match snapshot.get(handle).ok()?.values.first()? {
            Some(Value::I64(n)) => Some(*n),
            _ => None,
        }
    }

    #[test]
    fn a_working_set_reads_its_changes_and_leaves_the_base_alone() {
        let base = Snapshot::empty(registry());
        let mut set = WorkingSet::new(&base);
        let a = set.create("test.a", object(1, 10)).unwrap();
        let b = set.create("test.b", object(2, 20)).unwrap();
        set.replace_values(a, vec![Some(Value::I64(11))]).unwrap();
        assert_eq!(value(set.current(), a), Some(11));
        assert_eq!(set.current().resolve(ObjectId::from_u128(2)).unwrap(), b);
        assert_eq!(set.current().type_key(b).unwrap().as_str(), "test.b");
        assert!(base.is_empty());
        assert_eq!(set.current().revision(), 1);
        assert_eq!(
            set.touched().values().copied().collect::<Vec<_>>(),
            [Change::Created, Change::Created]
        );
    }

    #[test]
    fn errors_have_their_codes() {
        let base = Snapshot::empty(registry());
        let mut set = WorkingSet::new(&base);
        let error = set.create("test.zzz", object(1, 0)).unwrap_err();
        assert_eq!(error.code(), "doc.type.unknown");
        let a = set.create("test.a", object(1, 0)).unwrap();
        let error = set.create("test.b", object(1, 0)).unwrap_err();
        assert_eq!(error.code(), "doc.object.duplicate_id");
        set.delete(a).unwrap();
        assert_eq!(set.delete(a).unwrap_err().code(), "doc.object.not_found");
        assert_eq!(
            set.replace_header(a, Header::default()).unwrap_err().code(),
            "doc.object.not_found"
        );
        assert_eq!(
            set.current()
                .resolve(ObjectId::from_u128(1))
                .unwrap_err()
                .code(),
            "doc.object.not_found"
        );
        assert!(set.touched().is_empty(), "created and deleted is no change");
        assert_eq!(
            set.current().iter_type("x.y").err().map(|e| e.code()),
            Some("doc.type.unknown")
        );
    }

    #[test]
    fn a_deleted_identifier_cannot_be_created_again_in_the_same_set() {
        let mut base = WorkingSet::new(&Snapshot::empty(registry()));
        base.create("test.a", object(1, 0)).unwrap();
        let base = base.current().clone();
        let mut set = WorkingSet::new(&base);
        set.delete(base.resolve(ObjectId::from_u128(1)).unwrap())
            .unwrap();
        let error = set.create("test.a", object(1, 0)).unwrap_err();
        assert_eq!(error.code(), "doc.object.duplicate_id");
        assert_eq!(
            set.touched().get(&ObjectId::from_u128(1)),
            Some(&Change::Deleted)
        );
    }

    #[test]
    fn stale_handles_never_resolve_to_another_object() {
        let mut set = WorkingSet::new(&Snapshot::empty(registry()));
        let a = set.create("test.a", object(1, 1)).unwrap();
        let b = set.create("test.b", object(2, 2)).unwrap();
        let held = set.current().clone();
        set.delete(a).unwrap();
        // Same slot, next generation.
        let c = set.create("test.a", object(3, 3)).unwrap();
        assert_ne!(a, c);
        assert_eq!(
            set.current().get(a).unwrap_err().code(),
            "doc.object.not_found"
        );
        assert_eq!(value(set.current(), c), Some(3));
        // The other type's store has an object at the same slot and generation.
        assert_eq!(value(set.current(), b), Some(2));
        // The held snapshot still reads its own state.
        assert_eq!(value(&held, a), Some(1));
        assert!(held.get(c).is_err());
    }

    #[test]
    fn untouched_stores_chunks_and_index_shards_are_shared() {
        let mut set = WorkingSet::new(&Snapshot::empty(registry()));
        for n in 0..3_000 {
            set.create("test.a", object(n, 0)).unwrap();
        }
        for n in 10_000..10_100 {
            set.create("test.b", object(n, 0)).unwrap();
        }
        let first = set.current().clone();

        let mut set = WorkingSet::new(&first);
        let handle = first.resolve(ObjectId::from_u128(5)).unwrap();
        set.replace_values(handle, vec![Some(Value::I64(1))])
            .unwrap();
        let second = set.current().clone();
        let (a1, a2) = (
            first.store("test.a").unwrap(),
            second.store("test.a").unwrap(),
        );
        assert!(!Arc::ptr_eq(a1.chunk(0).unwrap(), a2.chunk(0).unwrap()));
        for chunk in 1..3 {
            assert!(Arc::ptr_eq(
                a1.chunk(chunk).unwrap(),
                a2.chunk(chunk).unwrap()
            ));
        }
        let (b1, b2) = (
            first.store("test.b").unwrap(),
            second.store("test.b").unwrap(),
        );
        assert!(Arc::ptr_eq(b1.chunk(0).unwrap(), b2.chunk(0).unwrap()));
        // A replacement keeps the handle, so the index is shared entirely.
        for shard in 0..crate::index::SHARDS {
            match (first.index().shard(shard), second.index().shard(shard)) {
                (Some(x), Some(y)) => assert!(Arc::ptr_eq(x, y)),
                (x, y) => assert_eq!(x.is_none(), y.is_none()),
            }
        }

        let mut set = WorkingSet::new(&second);
        let id = ObjectId::from_u128(99_999);
        set.create("test.b", object(99_999, 0)).unwrap();
        let third = set.current().clone();
        let touched = IdIndex::shard_of_id(id);
        for shard in (0..crate::index::SHARDS).filter(|&shard| shard != touched) {
            match (second.index().shard(shard), third.index().shard(shard)) {
                (Some(x), Some(y)) => assert!(Arc::ptr_eq(x, y)),
                (x, y) => assert_eq!(x.is_none(), y.is_none()),
            }
        }
        let (a2, a3) = (
            second.store("test.a").unwrap(),
            third.store("test.a").unwrap(),
        );
        for chunk in 0..3 {
            assert!(Arc::ptr_eq(
                a2.chunk(chunk).unwrap(),
                a3.chunk(chunk).unwrap()
            ));
        }
    }

    #[test]
    fn a_cloned_candidate_is_a_savepoint() {
        let mut set = WorkingSet::new(&Snapshot::empty(registry()));
        let a = set.create("test.a", object(1, 1)).unwrap();
        let savepoint = set.current().clone();
        set.replace_values(a, vec![Some(Value::I64(2))]).unwrap();
        set.create("test.a", object(2, 0)).unwrap();
        assert_eq!(value(&savepoint, a), Some(1));
        assert_eq!(savepoint.len(), 1);
        assert_eq!(value(set.current(), a), Some(2));
        assert_eq!(set.current().len(), 2);
    }

    #[test]
    fn publication_requires_the_latest_base() {
        let cell = SnapshotCell::new(Snapshot::empty(registry()));
        let base = cell.load();
        let mut first = WorkingSet::new(&base);
        first.create("test.a", object(1, 1)).unwrap();
        let mut second = WorkingSet::new(&base);
        second.create("test.a", object(2, 2)).unwrap();
        cell.publish(first.current().clone()).unwrap();
        let error = cell.publish(second.current().clone()).unwrap_err();
        assert_eq!(error.code(), "doc.snapshot.stale");
        assert_eq!(cell.load().revision(), 1);
        assert!(cell.load().resolve(ObjectId::from_u128(1)).is_ok());
        assert!(cell.load().resolve(ObjectId::from_u128(2)).is_err());
        assert!(base.is_empty());
    }
}
