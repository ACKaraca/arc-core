//! Draw order (CORE-DRAFT-005).
//!
//! Every entity stores an `i64` draw-order key; within its container (a space or a block
//! definition) entities draw in ascending order of (key, identifier), so the order is total and
//! deterministic even when keys tie. [`reorder`] computes the keys a move needs:
//!
//! - The moved entities keep their relative order and are placed at the front (drawn last), at
//!   the back, or directly above or below a reference entity.
//! - They take keys spread evenly over the open interval between their new neighbors' keys;
//!   at an end of the order the interval extends [`GAP`] per moved entity beyond the extreme key.
//!   Only the moved entities change.
//! - When the interval has fewer free keys than entities to place, or it would leave the `i64`
//!   range, every entity of the container is renumbered `GAP, 2·GAP, …` in the new order. A
//!   renumbering costs time proportional to the container; all other moves to the moved
//!   entities and their two neighbors.

use std::collections::BTreeSet;

use arc_base::{Error, ObjectId, Result};

use crate::error::codes;

/// The key distance of a renumbering and of placements beyond the ends of the order.
pub const GAP: i64 = 1 << 20;

/// Where [`reorder`] places the moved entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// After every other entity: drawn last, on top.
    Front,
    /// Before every other entity: drawn first, underneath.
    Back,
    /// Directly after the reference entity.
    Above(ObjectId),
    /// Directly before the reference entity.
    Below(ObjectId),
}

/// The entities of `current` (identifier and key) in draw order.
#[must_use]
pub fn sorted(current: &[(ObjectId, i64)]) -> Vec<(ObjectId, i64)> {
    let mut order = current.to_vec();
    order.sort_by_key(|&(id, key)| (key, id));
    order
}

/// The new keys of the entities whose key changes when `moved` is placed at `place` among
/// `current`, every entity of one container; see the [module documentation](self).
///
/// # Errors
///
/// `draft.order.container_mismatch` when a moved entity or the reference is not in `current`;
/// `draft.order.reference_moved` when the reference is one of the moved entities.
pub fn reorder(
    current: &[(ObjectId, i64)],
    moved: &[ObjectId],
    place: Place,
) -> Result<Vec<(ObjectId, i64)>> {
    let order = sorted(current);
    let moving: BTreeSet<ObjectId> = moved.iter().copied().collect();
    let present: BTreeSet<ObjectId> = order.iter().map(|(id, _)| *id).collect();
    if let Some(id) = moving.difference(&present).next() {
        return Err(Error::new(codes::ORDER_CONTAINER_MISMATCH).with_detail("id", id.to_string()));
    }
    let (block, rest): (Vec<_>, Vec<_>) = order.iter().partition(|(id, _)| moving.contains(id));
    let position = |reference: ObjectId| {
        if moving.contains(&reference) {
            return Err(Error::new(codes::ORDER_REFERENCE_MOVED));
        }
        rest.iter()
            .position(|(id, _)| *id == reference)
            .ok_or_else(|| {
                Error::new(codes::ORDER_CONTAINER_MISMATCH).with_detail("id", reference.to_string())
            })
    };
    let at = match place {
        Place::Front => rest.len(),
        Place::Back => 0,
        Place::Above(reference) => position(reference)? + 1,
        Place::Below(reference) => position(reference)?,
    };
    let below = at
        .checked_sub(1)
        .and_then(|i| rest.get(i))
        .map(|(_, k)| i128::from(*k));
    let above = rest.get(at).map(|(_, k)| i128::from(*k));
    let count = i128::try_from(block.len()).unwrap_or(i128::MAX);
    let reach = (count + 1) * i128::from(GAP);
    let (low, high) = match (below, above) {
        (None, None) => (0, reach),
        (Some(low), None) => (low, low + reach),
        (None, Some(high)) => (high - reach, high),
        (Some(low), Some(high)) => (low, high),
    };
    let span = high - low;
    let fits = span > count && low >= i128::from(i64::MIN) && high <= i128::from(i64::MAX);
    if fits {
        // `span ≥ count + 1`, so the keys are distinct and strictly inside (low, high).
        return Ok(block
            .iter()
            .zip(1_i128..)
            .filter_map(|((id, old), n)| {
                let key = i64::try_from(low + span * n / (count + 1)).ok()?;
                (key != *old).then_some((*id, key))
            })
            .collect());
    }
    let mut renumbered: Vec<(ObjectId, i64)> = rest.get(..at).unwrap_or_default().to_vec();
    renumbered.extend(block.iter().copied());
    renumbered.extend(rest.get(at..).unwrap_or_default().iter().copied());
    let old: std::collections::BTreeMap<ObjectId, i64> = order.into_iter().collect();
    Ok(renumbered
        .into_iter()
        .zip(1_i64..)
        // A container holds far fewer than i64::MAX / GAP (about 8.8e12) entities.
        .map(|((id, _), rank)| (id, rank.saturating_mul(GAP)))
        .filter(|(id, key)| old.get(id) != Some(key))
        .collect())
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;
    use proptest::prelude::*;

    use super::*;

    fn id(n: usize) -> ObjectId {
        ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n as u128)
    }

    fn apply(current: &mut [(ObjectId, i64)], changes: &[(ObjectId, i64)]) {
        for (id, key) in changes {
            if let Some(entry) = current.iter_mut().find(|(i, _)| i == id) {
                entry.1 = *key;
            }
        }
    }

    fn model(order: &[ObjectId], moved: &BTreeSet<ObjectId>, place: Place) -> Vec<ObjectId> {
        let block: Vec<_> = order
            .iter()
            .copied()
            .filter(|i| moved.contains(i))
            .collect();
        let mut rest: Vec<_> = order
            .iter()
            .copied()
            .filter(|i| !moved.contains(i))
            .collect();
        let at = match place {
            Place::Front => rest.len(),
            Place::Back => 0,
            Place::Above(r) => rest.iter().position(|i| *i == r).unwrap() + 1,
            Place::Below(r) => rest.iter().position(|i| *i == r).unwrap(),
        };
        rest.splice(at..at, block);
        rest
    }

    proptest! {
        #[test]
        fn reorder_matches_a_list_model(
            keys in prop::collection::vec(prop_oneof![Just(0_i64), Just(i64::MAX), Just(i64::MIN), any::<i64>(), -3_i64..3], 1..12),
            ops in prop::collection::vec((prop::collection::vec(any::<prop::sample::Index>(), 1..4), 0_u8..4, any::<prop::sample::Index>()), 1..40),
        ) {
            let mut current: Vec<_> = keys.iter().enumerate().map(|(n, k)| (id(n), *k)).collect();
            let mut list: Vec<_> = sorted(&current).iter().map(|(i, _)| *i).collect();
            for (picks, kind, reference) in ops {
                let moved: BTreeSet<_> = picks.iter().map(|p| list[p.index(list.len())]).collect();
                let reference = list[reference.index(list.len())];
                let place = match kind {
                    0 => Place::Front,
                    1 => Place::Back,
                    2 => Place::Above(reference),
                    _ => Place::Below(reference),
                };
                let moved_list: Vec<_> = moved.iter().copied().collect();
                let result = reorder(&current, &moved_list, place);
                if moved.contains(&reference) && kind >= 2 {
                    prop_assert_eq!(result.unwrap_err().code(), "draft.order.reference_moved");
                    continue;
                }
                apply(&mut current, &result.unwrap());
                list = model(&list, &moved, place);
                let order: Vec<_> = sorted(&current).iter().map(|(i, _)| *i).collect();
                prop_assert_eq!(&order, &list);
            }
        }
    }

    #[test]
    fn moves_touch_only_the_moved_entities_unless_renumbering() {
        let current = [(id(0), 0), (id(1), GAP), (id(2), 2 * GAP)];
        assert_eq!(
            reorder(&current, &[id(0)], Place::Front).unwrap(),
            [(id(0), 3 * GAP)]
        );
        assert_eq!(
            reorder(&current, &[id(2)], Place::Below(id(1))).unwrap(),
            [(id(2), GAP / 2)]
        );
        let tight = [(id(0), 0), (id(1), 1), (id(2), 2)];
        assert_eq!(
            reorder(&tight, &[id(2)], Place::Above(id(0)))
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            reorder(&tight, &[id(7)], Place::Front).unwrap_err().code(),
            "draft.order.container_mismatch"
        );
        assert_eq!(
            reorder(&tight, &[id(0)], Place::Above(id(7)))
                .unwrap_err()
                .code(),
            "draft.order.container_mismatch"
        );
    }
}
