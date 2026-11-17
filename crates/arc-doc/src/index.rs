//! The identifier index (CORE-DOC-003): a persistent map from [`ObjectId`] to [`Handle`].
//!
//! The 128 bits of an identifier first pass through a fixed bijective mix, so identifiers created
//! in sequence spread evenly. The top 6 bits of the mixed key select one of 64 shards; each shard
//! is a hash array mapped trie that consumes the following bits 5 at a time. A node holds a
//! 32-bit occupancy bitmap and one entry per set bit, either a leaf (key and handle) or a child
//! node. Because the mix is a bijection, distinct identifiers have distinct keys and the trie
//! needs no collision lists; its depth is at most 25 levels. A write copies the shard table (64
//! pointers, once per working set) and the nodes on the path to the key; every other node is
//! shared.
//!
//! The mix spreads generated identifiers; it is fixed and invertible, so it is no defense against
//! identifiers chosen to share long prefixes after the mix (for example in a hostile file). Such
//! identifiers make lookups and inserts slower and the index larger by a constant factor, never
//! deeper than the bound above.

use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::id::Identifier;

use crate::store::{Handle, usize_of};

/// The number of shards.
pub(crate) const SHARDS: usize = 64;
const SHARD_BITS: u32 = 6;
const LEVEL_BITS: u32 = 5;
/// The deepest level: by then every bit of the key has been consumed.
const MAX_LEVEL: u32 = (128 - SHARD_BITS).div_ceil(LEVEL_BITS);

#[derive(Debug, Clone)]
pub(crate) struct Node {
    bitmap: u32,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub(crate) enum Entry {
    Leaf(u128, Handle),
    Child(Arc<Node>),
}

/// The persistent identifier index.
#[derive(Debug, Clone)]
pub(crate) struct IdIndex {
    shards: Arc<[Option<Arc<Node>>; SHARDS]>,
}

impl Default for IdIndex {
    fn default() -> Self {
        Self {
            shards: Arc::new(std::array::from_fn(|_| None)),
        }
    }
}

/// A fixed bijection of the 128-bit space: xor-shifts and multiplications by odd constants.
fn mix(id: ObjectId) -> u128 {
    let mut x = id.as_u128();
    x ^= x >> 64;
    x = x.wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C835);
    x ^= x >> 61;
    x = x.wrapping_mul(0xD6E8_FEB8_6659_FD93_2B4B_CDA6_D1F3_8A2D);
    x ^ (x >> 64)
}

fn shard_of(key: u128) -> usize {
    usize::try_from(key >> (128 - SHARD_BITS)).unwrap_or(0)
}

/// The 5-bit index of `key` at `level` below the shard; 0 once every bit is consumed.
fn index_at(key: u128, level: u32) -> u32 {
    key.checked_shl(SHARD_BITS + LEVEL_BITS * level)
        .and_then(|rest| u32::try_from(rest >> (128 - LEVEL_BITS)).ok())
        .unwrap_or(0)
}

fn position(bitmap: u32, bit: u32) -> usize {
    usize_of((bitmap & (bit - 1)).count_ones())
}

impl IdIndex {
    /// The handle of `id`.
    pub(crate) fn get(&self, id: ObjectId) -> Option<Handle> {
        let key = mix(id);
        let mut node = self.shards.get(shard_of(key))?.as_ref()?;
        let mut level = 0;
        loop {
            let bit = 1 << index_at(key, level);
            if node.bitmap & bit == 0 {
                return None;
            }
            match node.entries.get(position(node.bitmap, bit))? {
                Entry::Leaf(stored, handle) => return (*stored == key).then_some(*handle),
                Entry::Child(child) => node = child,
            }
            level += 1;
        }
    }

    /// Maps `id` to `handle`, replacing an earlier handle of `id`.
    pub(crate) fn insert(&mut self, id: ObjectId, handle: Handle) {
        let key = mix(id);
        let Some(shard) = Arc::make_mut(&mut self.shards).get_mut(shard_of(key)) else {
            return;
        };
        match shard {
            Some(node) => insert_at(Arc::make_mut(node), key, handle, 0),
            None => {
                *shard = Some(Arc::new(Node {
                    bitmap: 1 << index_at(key, 0),
                    entries: vec![Entry::Leaf(key, handle)],
                }));
            }
        }
    }

    /// Removes `id` and returns its handle.
    pub(crate) fn remove(&mut self, id: ObjectId) -> Option<Handle> {
        // Look first, so a miss copies nothing.
        self.get(id)?;
        let key = mix(id);
        let shard = Arc::make_mut(&mut self.shards).get_mut(shard_of(key))?;
        let node = Arc::make_mut(shard.as_mut()?);
        let removed = remove_at(node, key, 0);
        if node.entries.is_empty() {
            *shard = None;
        }
        removed
    }

    #[cfg(test)]
    pub(crate) fn shard(&self, index: usize) -> Option<&Arc<Node>> {
        self.shards.get(index)?.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn shard_of_id(id: ObjectId) -> usize {
        shard_of(mix(id))
    }
}

#[cfg(test)]
impl Node {
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

fn insert_at(node: &mut Node, key: u128, handle: Handle, level: u32) {
    let bit = 1 << index_at(key, level);
    let at = position(node.bitmap, bit);
    if node.bitmap & bit == 0 {
        node.bitmap |= bit;
        node.entries
            .insert(at.min(node.entries.len()), Entry::Leaf(key, handle));
        return;
    }
    let Some(entry) = node.entries.get_mut(at) else {
        return;
    };
    match entry {
        Entry::Leaf(stored, stored_handle) if *stored == key => *stored_handle = handle,
        Entry::Leaf(stored, stored_handle) => {
            let pair = split(level + 1, (*stored, *stored_handle), (key, handle));
            *entry = Entry::Child(Arc::new(pair));
        }
        Entry::Child(child) => insert_at(Arc::make_mut(child), key, handle, level + 1),
    }
}

/// The node at `level` that holds two leaves with distinct keys.
fn split(level: u32, a: (u128, Handle), b: (u128, Handle)) -> Node {
    let (index_a, index_b) = (index_at(a.0, level), index_at(b.0, level));
    if index_a == index_b && level < MAX_LEVEL {
        return Node {
            bitmap: 1 << index_a,
            entries: vec![Entry::Child(Arc::new(split(level + 1, a, b)))],
        };
    }
    let (first, second) = if index_a <= index_b { (a, b) } else { (b, a) };
    Node {
        bitmap: (1 << index_a) | (1 << index_b),
        entries: vec![
            Entry::Leaf(first.0, first.1),
            Entry::Leaf(second.0, second.1),
        ],
    }
}

fn remove_at(node: &mut Node, key: u128, level: u32) -> Option<Handle> {
    let bit = 1 << index_at(key, level);
    if node.bitmap & bit == 0 {
        return None;
    }
    let at = position(node.bitmap, bit);
    if let Entry::Leaf(stored, handle) = node.entries.get(at)? {
        if *stored != key {
            return None;
        }
        let handle = *handle;
        node.entries.remove(at);
        node.bitmap &= !bit;
        return Some(handle);
    }
    let Some(Entry::Child(child)) = node.entries.get_mut(at) else {
        return None;
    };
    let child = Arc::make_mut(child);
    let removed = remove_at(child, key, level + 1)?;
    // A child left with a single leaf collapses into that leaf, keeping the trie minimal.
    match child.entries.as_slice() {
        [] => {
            node.entries.remove(at);
            node.bitmap &= !bit;
        }
        [Entry::Leaf(stored, handle)] => {
            let leaf = Entry::Leaf(*stored, *handle);
            if let Some(entry) = node.entries.get_mut(at) {
                *entry = leaf;
            }
        }
        _ => {}
    }
    Some(removed)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;

    use super::*;

    fn handle(n: u32) -> Handle {
        Handle {
            store: 0,
            slot: n,
            generation: 0,
        }
    }

    /// The level of the node that holds the leaf of `key`.
    fn depth_of(node: &Node, key: u128, level: u32) -> u32 {
        let bit = 1 << index_at(key, level);
        match node.entries.get(position(node.bitmap, bit)) {
            Some(Entry::Child(child)) => depth_of(child, key, level + 1),
            _ => level,
        }
    }

    #[test]
    fn keys_sharing_all_but_the_last_bits_split_to_the_deepest_level() {
        let a = 0x1234_5678_9ABC_DEF0_1234_5678_9ABC_DEF0_u128;
        let b = a ^ 1;
        let mut node = Node {
            bitmap: 1 << index_at(a, 0),
            entries: vec![Entry::Leaf(a, handle(1))],
        };
        insert_at(&mut node, b, handle(2), 0);
        assert_eq!(depth_of(&node, a, 0), MAX_LEVEL - 1);
        assert_eq!(remove_at(&mut node, a, 0), Some(handle(1)));
        // The chain collapsed back into a single leaf at the top.
        assert!(matches!(node.entries.as_slice(), [Entry::Leaf(key, _)] if *key == b));
        assert_eq!(remove_at(&mut node, a, 0), None);
    }

    #[test]
    fn a_write_copies_only_the_path_to_its_key() {
        let mut index = IdIndex::default();
        for n in 0..20_000 {
            index.insert(ObjectId::from_u128(n), handle(u32::try_from(n).unwrap()));
        }
        let before = index.clone();
        let id = ObjectId::from_u128(1_000_000);
        index.insert(id, handle(7));
        let touched = IdIndex::shard_of_id(id);
        for shard in (0..SHARDS).filter(|&shard| shard != touched) {
            assert!(Arc::ptr_eq(
                before.shard(shard).unwrap(),
                index.shard(shard).unwrap()
            ));
        }
        let (old, new) = (
            before.shard(touched).unwrap(),
            index.shard(touched).unwrap(),
        );
        assert!(!Arc::ptr_eq(old, new));
        // Below the copied root, every subtree but the one on the key's path is shared.
        let shared = old
            .entries()
            .iter()
            .zip(new.entries())
            .filter(|pair| matches!(pair, (Entry::Child(a), Entry::Child(b)) if Arc::ptr_eq(a, b)))
            .count();
        let children = old
            .entries()
            .iter()
            .filter(|entry| matches!(entry, Entry::Child(_)))
            .count();
        assert!(
            shared + 1 >= children && shared > 0,
            "{shared} of {children}"
        );
        assert_eq!(before.get(id), None);
        assert_eq!(index.get(id), Some(handle(7)));
    }

    proptest! {
        #[test]
        fn the_index_behaves_like_a_map(
            ops in prop::collection::vec((any::<bool>(), 0_u128..64, any::<u32>()), 0..300),
            spread in any::<u128>(),
        ) {
            let mut index = IdIndex::default();
            let mut model = BTreeMap::new();
            for (insert, n, slot) in ops {
                // A few raw values that differ only in their low bits, scattered by `spread`.
                let id = ObjectId::from_u128(spread ^ n);
                if insert {
                    index.insert(id, handle(slot));
                    model.insert(id, handle(slot));
                } else {
                    prop_assert_eq!(index.remove(id), model.remove(&id));
                }
            }
            for n in 0..64 {
                let id = ObjectId::from_u128(spread ^ n);
                prop_assert_eq!(index.get(id), model.get(&id).copied());
            }
        }
    }
}
