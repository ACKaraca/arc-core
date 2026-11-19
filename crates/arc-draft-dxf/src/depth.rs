//! The nesting limit of the block references of an import.
//!
//! The drafting model rejects a transaction whose block references form a chain deeper than
//! `arc_draft::validate::DEFAULT_MAX_NESTING`, so one such chain in a file would fail the whole
//! import. The mapping drops the references that would exceed the limit before anything is applied.
//!
//! The level of a reference is the number of references on the longest chain ending at it, the
//! reference included, where a chain may start at any root: the model space, or a block that nothing
//! references. A reference inserted by the model space has level 1; a reference inside a block that
//! the model space inserts has level 2. Each reference of a level above the limit is dropped, and
//! every other one is kept. The depths are taken over the kept references only, so along any chain
//! of kept references the level grows by at least one per reference and no kept chain is deeper
//! than the limit.

use std::collections::VecDeque;

/// Which references are dropped by the limit.
///
/// `edges` holds one `(from, to)` pair per block reference: the node of the definition that
/// contains the reference (node `0` is the model space) and the node of the definition it inserts.
/// `nodes` is the number of nodes, the model space included. The result has one flag per edge in
/// the same order, `true` for a dropped reference: one whose level exceeds `limit`, and any on a
/// cycle (the mapping refuses cycles already, so this is a guard, not a normal path).
#[must_use]
pub fn too_deep(nodes: usize, edges: &[(usize, usize)], limit: usize) -> Vec<bool> {
    // The depth of a node is the longest chain of kept references ending at it. The edge loop
    // below then keeps a reference when its source is settled and its level is within the limit.
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); nodes];
    let mut pending = vec![0usize; nodes];
    for (at, &(from, to)) in edges.iter().enumerate() {
        if let (Some(list), Some(count)) = (out.get_mut(from), pending.get_mut(to)) {
            list.push(at);
            *count += 1;
        }
    }
    // Longest chain ending at each node, by Kahn's order: a node is settled once all its
    // references are read. Nodes on a cycle are never settled.
    let mut depth = vec![0usize; nodes];
    let mut settled = vec![false; nodes];
    let mut ready: VecDeque<usize> = (0..nodes).filter(|&n| pending.get(n) == Some(&0)).collect();
    while let Some(node) = ready.pop_front() {
        if let Some(flag) = settled.get_mut(node) {
            *flag = true;
        }
        let here = depth.get(node).copied().unwrap_or(0);
        let refs = out.get(node).map_or(&[][..], Vec::as_slice);
        for &at in refs {
            let Some(&(_, to)) = edges.get(at) else {
                continue;
            };
            // Only a kept reference (level within the limit) raises the depth of its target; a
            // dropped one must not push the blocks below it past the limit.
            let level = here.saturating_add(1);
            if level <= limit
                && let Some(d) = depth.get_mut(to)
            {
                *d = (*d).max(level);
            }
            if let Some(count) = pending.get_mut(to) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    ready.push_back(to);
                }
            }
        }
    }
    edges
        .iter()
        .map(|&(from, _)| {
            let settled = settled.get(from) == Some(&true);
            let level = depth.get(from).copied().unwrap_or(0).saturating_add(1);
            !settled || level > limit
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::too_deep;

    /// The references of a chain: the model space (node 0) inserts node 1, which inserts node 2
    /// and so on, `links` references in all.
    fn chain(links: usize) -> Vec<(usize, usize)> {
        (0..links).map(|n| (n, n + 1)).collect()
    }

    #[test]
    fn chain_at_the_limit_is_kept() {
        let edges = chain(64);
        let dropped = too_deep(65, &edges, 64);
        assert!(dropped.iter().all(|d| !d));
    }

    #[test]
    fn only_the_deepest_reference_of_a_longer_chain_is_dropped() {
        let edges = chain(65);
        let dropped = too_deep(66, &edges, 64);
        let expected: Vec<bool> = (0..65).map(|n| n == 64).collect();
        assert_eq!(dropped, expected);
    }

    #[test]
    fn shared_sub_blocks_keep_the_longest_level() {
        // Space -> A -> C, space -> B -> C and C -> D. C is reached at level 2 through either
        // path, so C -> D is at level 3 and is the only reference above the limit of 2.
        // Nodes: 0 space, 1 A, 2 B, 3 C, 4 D.
        let edges = [(0, 1), (1, 3), (0, 2), (2, 3), (3, 4)];
        let dropped = too_deep(5, &edges, 2);
        assert_eq!(dropped, [false, false, false, false, true]);
    }

    #[test]
    fn a_cycle_is_dropped_and_its_entry_kept() {
        // Space -> 1, and 1 <-> 2.
        let edges = [(0, 1), (1, 2), (2, 1)];
        let dropped = too_deep(3, &edges, 64);
        assert_eq!(dropped, [false, true, true]);
    }

    #[test]
    fn a_short_kept_chain_keeps_what_a_dropped_chain_reaches() {
        // Space -> C1 -> ... -> C64 -> D (65 references, the last one dropped), space -> E -> D,
        // and D -> X. D is reached at level 2 through E, so D -> X (level 3) must stay.
        // Nodes: 0 space, 1..=64 C1..C64, 65 D, 66 E, 67 X.
        let mut edges = vec![(0, 1)];
        edges.extend((1..64).map(|c| (c, c + 1)));
        edges.push((64, 65));
        edges.push((0, 66));
        edges.push((66, 65));
        edges.push((65, 67));
        let dropped = too_deep(68, &edges, 64);
        let expected: Vec<bool> = edges.iter().map(|&e| e == (64, 65)).collect();
        assert_eq!(dropped, expected);
        assert!(!dropped[dropped.len() - 1]);
    }

    #[test]
    fn a_reference_from_an_unknown_node_is_dropped() {
        assert_eq!(too_deep(1, &[(9, 0)], 64), [true]);
    }
}
