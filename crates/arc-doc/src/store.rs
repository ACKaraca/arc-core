//! Typed stores (CORE-DOC-002).
//!
//! A [`Store`] holds the objects of one type key. Its slots live in chunks of [`CHUNK_SLOTS`]
//! entries; each entry is either a live object (a generation and a reference-counted immutable
//! [`Object`]) or a free slot that remembers the generation of its last occupant. The chunks are
//! the leaves of a radix trie with 32-way branch nodes, so a slot is reached through at most five
//! levels and a write copies one chunk's pointer table and the branch nodes above it, never the
//! whole directory. Copy-on-write uses [`Arc::make_mut`]: a node shared with a snapshot is copied
//! once, a node already private to the writer is changed in place.

use std::sync::Arc;

use crate::snapshot::Object;

/// The number of slots in one chunk.
pub(crate) const CHUNK_SLOTS: u32 = 1 << CHUNK_BITS;
const CHUNK_BITS: u32 = 10;
const BRANCH_BITS: u32 = 5;
const BRANCH_MASK: u32 = (1 << BRANCH_BITS) - 1;

/// The in-memory address of an object in one snapshot: the index of its type's store, a 32-bit
/// slot and a 32-bit generation.
///
/// A handle is valid for reading the snapshots in which its object is live. A handle whose slot
/// was freed, or reused by a later object with a newer generation, resolves to
/// `doc.object.not_found`, never to another object. Handles are never persisted, sent or used
/// across documents. They order by store (type key order), then slot, then generation, which is
/// the iteration order of a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle {
    pub(crate) store: u32,
    pub(crate) slot: u32,
    pub(crate) generation: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum Slot {
    Live {
        generation: u32,
        object: Arc<Object>,
    },
    /// A free slot and the generation of its last occupant.
    Free { generation: u32 },
}

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Chunk(Arc<Vec<Slot>>),
    Branch(Arc<Vec<Node>>),
}

/// A free slot in a persistent stack shared between revisions.
#[derive(Debug)]
struct FreeSlot {
    slot: u32,
    next: Option<Arc<FreeSlot>>,
}

impl Drop for FreeSlot {
    // Unlinks iteratively: a long free list must not recurse once per element.
    fn drop(&mut self) {
        let mut next = self.next.take();
        while let Some(node) = next {
            next = Arc::into_inner(node).and_then(|mut node| node.next.take());
        }
    }
}

/// The objects of one type key.
#[derive(Debug, Clone, Default)]
pub(crate) struct Store {
    root: Option<Node>,
    /// The branch levels above the chunks.
    depth: u32,
    /// The number of allocated slots, live or free.
    slots: u32,
    live: usize,
    free: Option<Arc<FreeSlot>>,
}

impl Store {
    /// The number of live objects.
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    /// The chunk with index `index`.
    pub(crate) fn chunk(&self, index: u32) -> Option<&Arc<Vec<Slot>>> {
        if index >= self.slots.div_ceil(CHUNK_SLOTS) {
            return None;
        }
        let mut node = self.root.as_ref()?;
        let mut level = self.depth;
        loop {
            match node {
                Node::Chunk(slots) => return Some(slots),
                Node::Branch(children) => {
                    level = level.checked_sub(1)?;
                    node = children.get(child_index(index, level))?;
                }
            }
        }
    }

    fn slot(&self, slot: u32) -> Option<&Slot> {
        self.chunk(slot >> CHUNK_BITS)?.get(offset(slot))
    }

    /// The live object at `slot` with `generation`.
    pub(crate) fn get(&self, slot: u32, generation: u32) -> Option<&Arc<Object>> {
        match self.slot(slot)? {
            Slot::Live {
                generation: live,
                object,
            } if *live == generation => Some(object),
            _ => None,
        }
    }

    fn slot_mut(&mut self, slot: u32) -> Option<&mut Slot> {
        if slot >= self.slots {
            return None;
        }
        let depth = self.depth;
        slot_mut_in(self.root.as_mut()?, slot >> CHUNK_BITS, offset(slot), depth)
    }

    /// Stores `object` in a free slot with the next generation, or in a new slot with
    /// generation 0, and returns the slot and generation. `None` means the store has no slot
    /// left.
    pub(crate) fn insert(&mut self, object: Arc<Object>) -> Option<(u32, u32)> {
        let reused = loop {
            let Some(head) = self.free.take() else {
                break None;
            };
            self.free.clone_from(&head.next);
            if let Some(Slot::Free { generation }) = self.slot(head.slot)
                && let Some(next) = generation.checked_add(1)
            {
                break Some((head.slot, next));
            }
        };
        let (slot, generation) = match reused {
            Some((slot, generation)) => {
                *self.slot_mut(slot)? = Slot::Live { generation, object };
                (slot, generation)
            }
            None => (
                self.push(Slot::Live {
                    generation: 0,
                    object,
                })?,
                0,
            ),
        };
        self.live += 1;
        Some((slot, generation))
    }

    /// Replaces the live object at `slot` with `generation` and returns the previous one.
    pub(crate) fn replace(
        &mut self,
        slot: u32,
        generation: u32,
        object: Arc<Object>,
    ) -> Option<Arc<Object>> {
        self.get(slot, generation)?;
        match self.slot_mut(slot)? {
            Slot::Live { object: stored, .. } => Some(std::mem::replace(stored, object)),
            Slot::Free { .. } => None,
        }
    }

    /// Frees the live object at `slot` with `generation` and returns it. The slot is reused with
    /// the next generation, unless its generation is exhausted: then it is retired for good.
    pub(crate) fn remove(&mut self, slot: u32, generation: u32) -> Option<Arc<Object>> {
        self.get(slot, generation)?;
        let entry = self.slot_mut(slot)?;
        let Slot::Live { object, .. } = std::mem::replace(entry, Slot::Free { generation }) else {
            return None;
        };
        if generation < u32::MAX {
            self.free = Some(Arc::new(FreeSlot {
                slot,
                next: self.free.take(),
            }));
        }
        self.live -= 1;
        Some(object)
    }

    /// Every live object as slot, generation and object, in ascending slot order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u32, u32, &Arc<Object>)> + '_ {
        (0..self.slots.div_ceil(CHUNK_SLOTS))
            .filter_map(|index| self.chunk(index).map(|chunk| (index, chunk)))
            .flat_map(|(index, chunk)| {
                chunk
                    .iter()
                    .zip(index << CHUNK_BITS..)
                    .filter_map(|(entry, slot)| match entry {
                        Slot::Live { generation, object } => Some((slot, *generation, object)),
                        Slot::Free { .. } => None,
                    })
            })
    }

    /// Appends a new slot and returns its index.
    fn push(&mut self, entry: Slot) -> Option<u32> {
        let slot = self.slots;
        if slot == u32::MAX {
            return None;
        }
        let chunk = slot >> CHUNK_BITS;
        let root = match self.root.take() {
            None => Node::Chunk(Arc::new(Vec::new())),
            // The trie is full at this depth: the old root becomes the first child of a new one.
            Some(root) if offset(slot) == 0 && chunk == 1 << (BRANCH_BITS * self.depth) => {
                self.depth += 1;
                Node::Branch(Arc::new(vec![root]))
            }
            Some(root) => root,
        };
        let root = self.root.insert(root);
        append(root, chunk, self.depth, entry)?;
        self.slots += 1;
        Some(slot)
    }
}

fn offset(slot: u32) -> usize {
    usize_of(slot & (CHUNK_SLOTS - 1))
}

fn child_index(chunk: u32, level: u32) -> usize {
    usize_of((chunk >> (BRANCH_BITS * level)) & BRANCH_MASK)
}

pub(crate) fn usize_of(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn slot_mut_in(node: &mut Node, chunk: u32, offset: usize, level: u32) -> Option<&mut Slot> {
    match node {
        Node::Chunk(slots) => Arc::make_mut(slots).get_mut(offset),
        Node::Branch(children) => {
            let level = level.checked_sub(1)?;
            let child = Arc::make_mut(children).get_mut(child_index(chunk, level))?;
            slot_mut_in(child, chunk, offset, level)
        }
    }
}

fn append(node: &mut Node, chunk: u32, level: u32, entry: Slot) -> Option<()> {
    match node {
        Node::Chunk(slots) => {
            Arc::make_mut(slots).push(entry);
            Some(())
        }
        Node::Branch(children) => {
            let level = level.checked_sub(1)?;
            let index = child_index(chunk, level);
            let children = Arc::make_mut(children);
            if index == children.len() {
                children.push(empty_path(level));
            }
            append(children.get_mut(index)?, chunk, level, entry)
        }
    }
}

/// A new branch path `level` levels high that ends in an empty chunk.
fn empty_path(level: u32) -> Node {
    let mut node = Node::Chunk(Arc::new(Vec::new()));
    for _ in 0..level {
        node = Node::Branch(Arc::new(vec![node]));
    }
    node
}

#[cfg(test)]
mod tests {
    use arc_base::ObjectId;
    use arc_base::id::Identifier;

    use super::*;
    use crate::schema::SchemaVersion;
    use crate::snapshot::Header;

    fn object(n: u128) -> Arc<Object> {
        Arc::new(Object::new(
            ObjectId::from_u128(n),
            SchemaVersion::new(1, 0),
            Header::default(),
            Vec::new(),
        ))
    }

    fn id_at(store: &Store, slot: u32, generation: u32) -> Option<u128> {
        store
            .get(slot, generation)
            .map(|object| object.id.as_u128())
    }

    #[test]
    fn slots_grow_through_chunks_and_branch_levels() {
        let mut store = Store::default();
        let count = CHUNK_SLOTS * 33 + 5;
        for n in 0..count {
            assert_eq!(store.insert(object(n.into())), Some((n, 0)));
        }
        // 34 chunks need two branch levels of 32.
        assert_eq!(store.depth, 2);
        assert_eq!(store.len(), usize_of(count));
        for n in [0, 1023, 1024, 32 * 1024, count - 1] {
            assert_eq!(id_at(&store, n, 0), Some(n.into()));
        }
        assert_eq!(id_at(&store, count, 0), None);
        let order: Vec<u32> = store.iter().map(|(slot, _, _)| slot).collect();
        assert_eq!(order, (0..count).collect::<Vec<_>>());
    }

    #[test]
    fn freed_slots_are_reused_with_the_next_generation() {
        let mut store = Store::default();
        for n in 0..3 {
            store.insert(object(n));
        }
        assert_eq!(store.remove(1, 0).map(|o| o.id.as_u128()), Some(1));
        assert_eq!(
            store.remove(1, 0),
            None,
            "a freed slot is not removed twice"
        );
        assert_eq!(store.replace(1, 0, object(9)), None);
        assert_eq!(store.insert(object(7)), Some((1, 1)));
        assert_eq!(id_at(&store, 1, 0), None, "the old generation is stale");
        assert_eq!(id_at(&store, 1, 1), Some(7));
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn an_exhausted_generation_retires_the_slot() {
        let mut store = Store::default();
        store.insert(object(1));
        *store.slot_mut(0).unwrap() = Slot::Live {
            generation: u32::MAX,
            object: object(1),
        };
        assert!(store.remove(0, u32::MAX).is_some());
        assert_eq!(store.insert(object(2)), Some((1, 0)));
        assert_eq!(id_at(&store, 0, u32::MAX), None);
    }

    #[test]
    fn a_write_copies_only_the_touched_chunk_and_its_path() {
        let mut store = Store::default();
        for n in 0..CHUNK_SLOTS * 40 {
            store.insert(object(n.into()));
        }
        let before = store.clone();
        store.replace(5, 0, object(99)).unwrap();
        assert!(!Arc::ptr_eq(
            before.chunk(0).unwrap(),
            store.chunk(0).unwrap()
        ));
        for index in 1..40 {
            assert!(Arc::ptr_eq(
                before.chunk(index).unwrap(),
                store.chunk(index).unwrap()
            ));
        }
        // Untouched objects of the copied chunk are shared, not cloned.
        assert!(Arc::ptr_eq(
            before.get(6, 0).unwrap(),
            store.get(6, 0).unwrap()
        ));
        assert_eq!(id_at(&before, 5, 0), Some(5));
        assert_eq!(id_at(&store, 5, 0), Some(99));
    }

    #[test]
    fn dropping_a_long_free_list_does_not_recurse() {
        let mut store = Store::default();
        let count = 200_000;
        for n in 0..count {
            store.insert(object(n.into()));
        }
        for slot in 0..count {
            store.remove(slot, 0);
        }
        assert_eq!(store.len(), 0);
        drop(store);
    }
}
