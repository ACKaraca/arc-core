//! Objects, snapshots, working sets and snapshot publication (CORE-DOC-002, CORE-DOC-003,
//! CORE-DOC-017).
//!
//! A [`Snapshot`] is an immutable view of every object of a document at one revision: one typed
//! store per registered type key, the identifier index, the revision number and the type
//! registry, behind one `Arc`. Cloning it is the constant-time way to take a snapshot. A
//! [`WorkingSet`] starts from a snapshot, applies creates, replacements and deletes with
//! structural sharing, and yields a candidate snapshot; a [`SnapshotCell`] publishes a working
//! set's candidate atomically when its base is still the published snapshot, and hands the latest
//! snapshot to readers without locks.
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
use crate::preserved::HeldObject;
use crate::registry::TypeRegistry;
use crate::reverse_index::ReverseIndex;
use crate::schema::{SchemaVersion, TypeKey, Value};
use crate::store::{Handle, Store, usize_of};
use crate::version::ReadOnlyReason;

/// The store index of handles to held objects (preserved and quarantined objects), which live
/// outside the typed stores.
pub(crate) const HELD_STORE: u32 = u32::MAX;

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
    /// The object was deleted and an object with the same identifier was created, possibly of
    /// another type and in another slot. The base snapshot holds the state before, the candidate
    /// the state after.
    Recreated,
}

#[derive(Debug, Clone)]
struct Roots {
    registry: Arc<TypeRegistry>,
    /// The registered type keys in ascending order; store `i` holds objects of key `i`.
    keys: Arc<[TypeKey]>,
    revision: u64,
    /// The number of live objects in all stores.
    live: usize,
    stores: Arc<Vec<Store>>,
    index: IdIndex,
    reverse: ReverseIndex,
    /// The preserved and quarantined objects, fixed at load; a handle's slot indexes it.
    held: Arc<Vec<HeldObject>>,
    /// Why the document is read-only; empty for a writable document.
    read_only: Arc<[ReadOnlyReason]>,
}

/// An immutable view of a document at one revision.
///
/// Cloning a snapshot copies one pointer, whatever the size of the document. Reading an object
/// by handle walks at most five trie levels and takes no lock; resolving an identifier walks at
/// most 26 levels of the identifier index. Iteration is in handle order: by type key, then by
/// slot.
///
/// Dropping the last holder of a snapshot frees everything only it holds, on the dropping
/// thread: for the last snapshot of a document of one million objects that takes about half a
/// second, so a reader that may hold the last reference should drop it off latency-sensitive
/// threads.
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
    /// The empty snapshot of revision 0 with one store per type of `registry`: a new document.
    /// Every snapshot and working set derived from it shares its stores' generation counters.
    #[must_use]
    pub fn empty(registry: Arc<TypeRegistry>) -> Self {
        let keys: Arc<[TypeKey]> = registry.iter().map(|decl| decl.key.clone()).collect();
        Self(Arc::new(Roots {
            // One store each: a cloned store would share its generation counter.
            stores: Arc::new(keys.iter().map(|_| Store::default()).collect()),
            keys,
            registry,
            revision: 0,
            live: 0,
            index: IdIndex::default(),
            reverse: ReverseIndex::default(),
            held: Arc::new(Vec::new()),
            read_only: Arc::from(Vec::new()),
        }))
    }

    /// The revision number: 0 for an empty snapshot, one more than its base for a candidate. It
    /// is a label: publication checks the base itself, not this number, and the number saturates
    /// at `u64::MAX`.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.0.revision
    }

    /// True when both are the same snapshot (one pointer), not merely equal in content.
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
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

    /// The object at `handle`; a stale handle fails with `doc.object.not_found`. The object of
    /// a held (preserved or quarantined) object has no field values.
    pub fn get(&self, handle: Handle) -> Result<&Arc<Object>> {
        if handle.store == HELD_STORE {
            return self
                .held_object(handle)
                .map(HeldObject::object)
                .ok_or_else(not_found);
        }
        self.0
            .stores
            .get(usize_of(handle.store))
            .and_then(|store| store.get(handle.slot, handle.generation))
            .ok_or_else(not_found)
    }

    /// The type key of the store `handle` points into; for a held object, the type key of its
    /// stored row (`None` when that key is malformed).
    #[must_use]
    pub fn type_key(&self, handle: Handle) -> Option<&TypeKey> {
        if handle.store == HELD_STORE {
            return self.held_object(handle)?.type_key();
        }
        self.0.keys.get(usize_of(handle.store))
    }

    /// The held object at `handle`, or `None` when `handle` does not point to a live held
    /// object.
    #[must_use]
    pub fn held_object(&self, handle: Handle) -> Option<&HeldObject> {
        if handle.store != HELD_STORE || handle.generation != 0 {
            return None;
        }
        self.0.held.get(usize_of(handle.slot))
    }

    /// Every held object (preserved and quarantined objects) with its handle, in load order.
    /// Held objects are not part of [`Snapshot::iter`] and [`Snapshot::len`].
    pub fn held(&self) -> impl Iterator<Item = (Handle, &HeldObject)> + '_ {
        self.0.held.iter().zip(0..).map(|(held, slot)| {
            (
                Handle {
                    store: HELD_STORE,
                    slot,
                    generation: 0,
                },
                held,
            )
        })
    }

    /// The number of held objects, in constant time.
    #[must_use]
    pub fn held_len(&self) -> usize {
        self.0.held.len()
    }

    /// Why the document is read-only, in type-key order; empty when it is writable. A working
    /// set on a read-only snapshot refuses every write with `doc.document.read_only`.
    #[must_use]
    pub fn read_only(&self) -> &[ReadOnlyReason] {
        &self.0.read_only
    }

    /// The number of live objects in the typed stores, in constant time; held objects are
    /// counted by [`Snapshot::held_len`].
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.live
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

    pub(crate) fn reverse(&self) -> &ReverseIndex {
        &self.0.reverse
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

/// A set of changes on top of a base snapshot, not visible to anyone else until it is published
/// with [`SnapshotCell::publish`].
///
/// Writes copy only what they touch: the store table and the shard table once, then the chunk,
/// branch nodes and index nodes on the path to each written object. [`WorkingSet::current`] is
/// the candidate. [`WorkingSet::savepoint`] marks a state that [`WorkingSet::rollback_to`]
/// restores.
pub struct WorkingSet {
    base: Snapshot,
    current: Snapshot,
    touched: BTreeMap<ObjectId, Change>,
    /// Identifies this working set to its savepoints.
    token: Arc<()>,
    /// The live savepoints, oldest first.
    savepoints: Vec<Saved>,
    next_savepoint: u64,
}

struct Saved {
    sequence: u64,
    current: Snapshot,
    touched: BTreeMap<ObjectId, Change>,
}

/// A state of one [`WorkingSet`] to roll back to, from [`WorkingSet::savepoint`].
///
/// It is valid for the working set that made it until it is released, or until a rollback to an
/// earlier savepoint or the release of an earlier savepoint invalidates it.
#[derive(Debug, Clone)]
pub struct Savepoint {
    set: Arc<()>,
    sequence: u64,
}

impl fmt::Debug for WorkingSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkingSet")
            .field("base", &self.base.revision())
            .field("touched", &self.touched.len())
            .field("savepoints", &self.savepoints.len())
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
            token: Arc::new(()),
            savepoints: Vec::new(),
            next_savepoint: 0,
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

    /// The net change of each identifier relative to the base. An object created and then
    /// deleted in the same working set is not listed; an object of the base deleted and then
    /// created again with the same identifier is [`Change::Recreated`], and deleting it again
    /// makes it [`Change::Deleted`]. Before states are read from [`WorkingSet::base`], after
    /// states from [`WorkingSet::current`].
    #[must_use]
    pub fn touched(&self) -> &BTreeMap<ObjectId, Change> {
        &self.touched
    }

    /// Adds `object` to the store of `type_key` and returns its handle.
    ///
    /// Fails with `doc.type.unknown` for an unregistered key, `doc.object.duplicate_id` when the
    /// identifier is live in the candidate, and `doc.store.capacity_exceeded` when the store has
    /// no slot or generation left. An identifier deleted earlier in this working set may be
    /// created again, in any store. The values are not checked against the schema here. On a
    /// read-only document every write of a working set fails with `doc.document.read_only`.
    pub fn create(&mut self, type_key: &str, object: Object) -> Result<Handle> {
        self.create_shared(type_key, Arc::new(object))
    }

    /// [`WorkingSet::create`] for an object the caller also holds, such as the operation that
    /// records it: the store shares it instead of copying it.
    pub fn create_shared(&mut self, type_key: &str, object: Arc<Object>) -> Result<Handle> {
        self.writable()?;
        let store = self.current.store_index(type_key)?;
        let id = object.id;
        if self.current.0.index.get(id).is_some() {
            return Err(Error::new(codes::OBJECT_DUPLICATE_ID).with_arg("id", id.to_string()));
        }
        let roots = Arc::make_mut(&mut self.current.0);
        let (slot, generation) = Arc::make_mut(&mut roots.stores)
            .get_mut(usize_of(store))
            .and_then(|entry| entry.insert(Arc::clone(&object)))
            .ok_or_else(|| Error::new(codes::STORE_CAPACITY_EXCEEDED).with_arg("type", type_key))?;
        let handle = Handle {
            store,
            slot,
            generation,
        };
        roots.index.insert(id, handle);
        roots
            .reverse
            .update(&roots.registry, type_key, None, Some(&object));
        roots.live += 1;
        // Only an object of the base is listed as deleted.
        let change = if self.touched.get(&id) == Some(&Change::Deleted) {
            Change::Recreated
        } else {
            Change::Created
        };
        self.touched.insert(id, change);
        Ok(handle)
    }

    /// Replaces the field values of the object at `handle` and returns the previous object. A
    /// stale handle fails with `doc.object.not_found`, a held object with
    /// `doc.object.not_modifiable`.
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
    /// handle fails with `doc.object.not_found`, a held object with `doc.object.not_modifiable`.
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
        let object = Arc::new(object);
        let old = self
            .store_mut(handle)?
            .replace(handle.slot, handle.generation, Arc::clone(&object))
            .ok_or_else(not_found)?;
        self.update_reverse(handle, Some(&old), Some(&object));
        if !matches!(
            self.touched.get(&id),
            Some(Change::Created | Change::Recreated)
        ) {
            self.touched.insert(id, Change::Modified);
        }
        Ok(old)
    }

    /// Deletes the object at `handle` and returns it. A stale handle fails with
    /// `doc.object.not_found`, a held object with `doc.object.not_modifiable`. The handle and
    /// every copy of it are stale from now on.
    pub fn delete(&mut self, handle: Handle) -> Result<Arc<Object>> {
        let old = self
            .store_mut(handle)?
            .remove(handle.slot, handle.generation)
            .ok_or_else(not_found)?;
        self.update_reverse(handle, Some(&old), None);
        let roots = Arc::make_mut(&mut self.current.0);
        roots.index.remove(old.id);
        roots.live = roots.live.saturating_sub(1);
        if self.touched.remove(&old.id) != Some(Change::Created) {
            self.touched.insert(old.id, Change::Deleted);
        }
        Ok(old)
    }

    /// Marks the current state (candidate and touched identifiers) so that
    /// [`WorkingSet::rollback_to`] can restore it. Taking a savepoint shares the candidate in
    /// constant time and copies the touched map.
    pub fn savepoint(&mut self) -> Savepoint {
        let sequence = self.next_savepoint;
        self.next_savepoint = sequence.saturating_add(1);
        self.savepoints.push(Saved {
            sequence,
            current: self.current.clone(),
            touched: self.touched.clone(),
        });
        Savepoint {
            set: Arc::clone(&self.token),
            sequence,
        }
    }

    /// Restores the state marked by `savepoint`, which stays valid; every savepoint taken after
    /// it becomes invalid. Handles obtained after the savepoint become stale and are never handed
    /// out again. A savepoint of another working set, or one released or invalidated, fails with
    /// `doc.savepoint.invalid` and changes nothing.
    pub fn rollback_to(&mut self, savepoint: &Savepoint) -> Result<()> {
        let at = self.position(savepoint)?;
        let saved = self.savepoints.get(at).ok_or_else(invalid_savepoint)?;
        self.current = saved.current.clone();
        self.touched = saved.touched.clone();
        self.savepoints.truncate(at + 1);
        Ok(())
    }

    /// Releases `savepoint` and every savepoint taken after it, keeping the changes made since.
    /// A savepoint of another working set, or one released or invalidated, fails with
    /// `doc.savepoint.invalid`.
    pub fn release_savepoint(&mut self, savepoint: &Savepoint) -> Result<()> {
        let at = self.position(savepoint)?;
        self.savepoints.truncate(at);
        Ok(())
    }

    fn position(&self, savepoint: &Savepoint) -> Result<usize> {
        if !Arc::ptr_eq(&savepoint.set, &self.token) {
            return Err(invalid_savepoint());
        }
        self.savepoints
            .binary_search_by_key(&savepoint.sequence, |saved| saved.sequence)
            .map_err(|_| invalid_savepoint())
    }

    fn update_reverse(&mut self, handle: Handle, old: Option<&Object>, new: Option<&Object>) {
        let roots = Arc::make_mut(&mut self.current.0);
        let key = roots
            .keys
            .get(usize_of(handle.store))
            .map_or("", TypeKey::as_str);
        roots.reverse.update(&roots.registry, key, old, new);
    }

    /// Fails with `doc.document.read_only` when the document is read-only.
    fn writable(&self) -> Result<()> {
        match self.current.read_only().first() {
            Some(reason) => Err(reason.error()),
            None => Ok(()),
        }
    }

    fn store_mut(&mut self, handle: Handle) -> Result<&mut Store> {
        self.writable()?;
        // Check first, so a stale handle copies nothing.
        self.current.get(handle)?;
        if let Some(held) = self.current.held_object(handle) {
            // Preserved and quarantined objects survive every save; `id` names the held object,
            // also when the write is the cascade of deleting its owner.
            return Err(held.not_modifiable());
        }
        let roots = Arc::make_mut(&mut self.current.0);
        Arc::make_mut(&mut roots.stores)
            .get_mut(usize_of(handle.store))
            .ok_or_else(not_found)
    }
}

fn invalid_savepoint() -> Error {
    Error::new(codes::SAVEPOINT_INVALID)
}

/// Builds the snapshot of a loaded document from many objects at once (the bulk load path).
///
/// It keeps the invariants of a working set (stores, identifier index, reverse reference index,
/// live count) without its bookkeeping: there is no base, no touched map and no savepoint, and
/// every node it writes is private to it, so it is changed in place.
pub(crate) struct BulkLoad {
    roots: Roots,
    held: Vec<HeldObject>,
}

impl BulkLoad {
    /// An empty document of `registry`.
    pub(crate) fn new(registry: Arc<TypeRegistry>) -> Self {
        Self {
            roots: Arc::unwrap_or_clone(Snapshot::empty(registry).0),
            held: Vec::new(),
        }
    }

    /// The registry of the document.
    pub(crate) fn registry(&self) -> &Arc<TypeRegistry> {
        &self.roots.registry
    }

    /// True when `id` is already loaded.
    pub(crate) fn contains(&self, id: ObjectId) -> bool {
        self.roots.index.get(id).is_some()
    }

    /// Adds `object` to the store of the registered `type_key` and returns it as stored.
    /// Fails like [`WorkingSet::create`] for an unregistered key, a loaded identifier or a full
    /// store.
    pub(crate) fn insert(&mut self, type_key: &str, object: Object) -> Result<Arc<Object>> {
        let index = self
            .roots
            .keys
            .binary_search_by(|probe| probe.as_str().cmp(type_key))
            .map_err(|_| unknown(type_key))?;
        let id = object.id;
        if self.contains(id) {
            return Err(Error::new(codes::OBJECT_DUPLICATE_ID).with_arg("id", id.to_string()));
        }
        let object = Arc::new(object);
        let (slot, generation) = Arc::make_mut(&mut self.roots.stores)
            .get_mut(index)
            .and_then(|entry| entry.insert(Arc::clone(&object)))
            .ok_or_else(|| Error::new(codes::STORE_CAPACITY_EXCEEDED).with_arg("type", type_key))?;
        let store = u32::try_from(index).map_err(|_| unknown(type_key))?;
        self.roots.index.insert(
            id,
            Handle {
                store,
                slot,
                generation,
            },
        );
        self.roots
            .reverse
            .update(&self.roots.registry, type_key, None, Some(&object));
        self.roots.live += 1;
        Ok(object)
    }

    /// Adds a held object. Its header owner and its decoded field references
    /// ([`HeldObject::references`]) enter the reverse reference index. Fails with
    /// `doc.object.duplicate_id` for a loaded identifier.
    pub(crate) fn hold(&mut self, held: HeldObject) -> Result<()> {
        let id = held.object().id;
        if self.contains(id) {
            return Err(Error::new(codes::OBJECT_DUPLICATE_ID).with_arg("id", id.to_string()));
        }
        let slot = u32::try_from(self.held.len())
            .map_err(|_| Error::new(codes::STORE_CAPACITY_EXCEEDED).with_arg("type", "(held)"))?;
        self.roots.index.insert(
            id,
            Handle {
                store: HELD_STORE,
                slot,
                generation: 0,
            },
        );
        let key = held.type_key().map_or("", TypeKey::as_str);
        self.roots
            .reverse
            .update(&self.roots.registry, key, None, Some(held.object()));
        for reference in held.references() {
            self.roots
                .reverse
                .insert(reference.target, id, reference.field.clone());
        }
        self.held.push(held);
        Ok(())
    }

    /// The snapshot of revision 0 holding everything added, read-only for `read_only` reasons
    /// when there are any.
    pub(crate) fn finish(mut self, read_only: Vec<ReadOnlyReason>) -> Snapshot {
        self.roots.held = Arc::new(self.held);
        self.roots.read_only = Arc::from(read_only);
        Snapshot(Arc::new(self.roots))
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

    /// Publishes the candidate of `set` atomically when the published snapshot is still the
    /// base of `set`, the very snapshot it was built on, not merely one with the same revision
    /// number. Otherwise fails with `doc.snapshot.stale` and publishes nothing; the changes must
    /// then be made again on the latest snapshot.
    pub fn publish(&self, set: &WorkingSet) -> Result<()> {
        let previous = self
            .current
            .compare_and_swap(&set.base.0, Arc::clone(&set.current.0));
        if Arc::ptr_eq(&previous, &set.base.0) {
            Ok(())
        } else {
            Err(Error::new(codes::SNAPSHOT_STALE)
                .with_detail("published_revision", revision_detail(previous.revision))
                .with_detail("base_revision", revision_detail(set.base.revision())))
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
    fn a_deleted_identifier_can_be_created_again_in_the_same_set() {
        let mut base = WorkingSet::new(&Snapshot::empty(registry()));
        let old = base.create("test.a", object(1, 0)).unwrap();
        let base = base.current().clone();
        let id = ObjectId::from_u128(1);
        let mut set = WorkingSet::new(&base);
        set.delete(old).unwrap();
        // Recreated as another type: the base has the before state, the candidate the after.
        let new = set.create("test.b", object(1, 5)).unwrap();
        assert_eq!(set.touched().get(&id), Some(&Change::Recreated));
        assert_eq!(set.current().resolve(id).unwrap(), new);
        assert_eq!(base.resolve(id).unwrap(), old);
        assert!(set.current().get(old).is_err());
        assert_eq!(set.current().len(), 1);
        set.replace_values(new, vec![Some(Value::I64(6))]).unwrap();
        assert_eq!(set.touched().get(&id), Some(&Change::Recreated));
        let error = set.create("test.a", object(1, 0)).unwrap_err();
        assert_eq!(error.code(), "doc.object.duplicate_id");
        set.delete(new).unwrap();
        assert_eq!(set.touched().get(&id), Some(&Change::Deleted));
        assert!(set.current().is_empty());
        // An object new in this set: create, delete and create again is a creation.
        let a = set.create("test.a", object(2, 0)).unwrap();
        set.delete(a).unwrap();
        set.create("test.b", object(2, 0)).unwrap();
        assert_eq!(
            set.touched().get(&ObjectId::from_u128(2)),
            Some(&Change::Created)
        );
    }

    #[test]
    fn rollback_restores_the_savepoint_and_never_reissues_handles() {
        let mut set = WorkingSet::new(&Snapshot::empty(registry()));
        let a = set.create("test.a", object(1, 1)).unwrap();
        let first = set.savepoint();
        set.replace_values(a, vec![Some(Value::I64(2))]).unwrap();
        let b = set.create("test.a", object(2, 2)).unwrap();
        let second = set.savepoint();
        set.delete(a).unwrap();
        set.rollback_to(&first).unwrap();
        assert_eq!(value(set.current(), a), Some(1));
        assert!(set.current().get(b).is_err());
        assert_eq!(set.current().len(), 1);
        assert_eq!(set.touched().len(), 1);
        assert_eq!(
            set.rollback_to(&second).unwrap_err().code(),
            "doc.savepoint.invalid"
        );
        // The slot of `b` is taken again with a new generation.
        let c = set.create("test.a", object(3, 3)).unwrap();
        assert_ne!(b, c);
        assert!(set.current().get(b).is_err());
        // A savepoint stays valid after a rollback to it, until it is released.
        set.rollback_to(&first).unwrap();
        assert!(set.current().get(c).is_err());
        set.release_savepoint(&first).unwrap();
        assert_eq!(
            set.rollback_to(&first).unwrap_err().code(),
            "doc.savepoint.invalid"
        );
        let mut other = WorkingSet::new(&Snapshot::empty(registry()));
        let foreign = other.savepoint();
        assert_eq!(
            set.rollback_to(&foreign).unwrap_err().code(),
            "doc.savepoint.invalid"
        );
        assert_eq!(
            set.release_savepoint(&foreign).unwrap_err().code(),
            "doc.savepoint.invalid"
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
        cell.publish(&first).unwrap();
        assert_eq!(
            cell.publish(&first).unwrap_err().code(),
            "doc.snapshot.stale",
            "a set is published once"
        );
        let error = cell.publish(&second).unwrap_err();
        assert_eq!(error.code(), "doc.snapshot.stale");
        assert_eq!(cell.load().revision(), 1);
        assert!(cell.load().resolve(ObjectId::from_u128(1)).is_ok());
        assert!(cell.load().resolve(ObjectId::from_u128(2)).is_err());
        assert!(base.is_empty());
    }
}
