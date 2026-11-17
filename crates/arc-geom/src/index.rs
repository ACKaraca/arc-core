//! R*-tree spatial indexes over axis-aligned boxes keyed by caller identifiers (CORE-GEOM-027).
//!
//! [`SpatialIndex2`] indexes [`Aabb2`] boxes and [`SpatialIndex3`] indexes [`Aabb3`] boxes. Each
//! entry is a key of type `K` and a non-empty, finite box. The tree implementation is private; no
//! type of it appears in this API.
//!
//! # Semantics
//!
//! - A key occurs at most once. [`insert`](SpatialIndex2::insert) with an existing key replaces
//!   that key's box and returns the old one.
//! - Boxes are closed: a box touching a query box or a point on its boundary counts as a hit.
//! - Every query returns a deterministic result that does not depend on insertion order:
//!   [`window`](SpatialIndex2::window) and [`crossing`](SpatialIndex2::crossing) sort by key;
//!   [`nearest`](SpatialIndex2::nearest) and [`ray`](SpatialIndex2::ray) sort by distance, then
//!   key. Distances are compared as computed `f64` values, so ties are exact.
//! - An empty box, or a box or query input with a non-finite coordinate, is rejected with
//!   `geom.bbox.invalid` or `geom.input.not_finite` before the index is touched; a failed call
//!   never changes the index.

use std::collections::HashMap;
use std::hash::Hash;

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Error, Result};
use rstar::{AABB, PointDistance, RTree, RTreeObject, SelectionFunction};

use crate::bbox::{Aabb2, Aabb3};
use crate::error::codes;

/// Corners of a box: minimum, then maximum.
type Corners<const N: usize> = ([f64; N], [f64; N]);

/// One tree entry.
#[derive(Clone, Copy, PartialEq)]
struct Entry<K, const N: usize> {
    key: K,
    lo: [f64; N],
    hi: [f64; N],
}

impl<K, const N: usize> RTreeObject for Entry<K, N> {
    type Envelope = AABB<[f64; N]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners(self.lo, self.hi)
    }
}

/// Squared distance from `point` to the closed box `lo..=hi`; zero inside.
fn distance_2<const N: usize>(lo: &[f64; N], hi: &[f64; N], point: &[f64; N]) -> f64 {
    (0..N)
        .map(|i| {
            let gap = (lo[i] - point[i]).max(point[i] - hi[i]).max(0.0);
            gap * gap
        })
        .sum()
}

impl<K, const N: usize> PointDistance for Entry<K, N> {
    fn distance_2(&self, point: &[f64; N]) -> f64 {
        distance_2(&self.lo, &self.hi, point)
    }
}

/// Parameter at which the ray `origin + t * direction`, `0 <= t <= max`, first meets the closed
/// box (slab method), or `None`. A zero direction component hits only when the origin lies
/// within that slab; an origin inside the box gives 0.
fn ray_entry<const N: usize>(
    origin: &[f64; N],
    direction: &[f64; N],
    max: f64,
    lo: &[f64; N],
    hi: &[f64; N],
) -> Option<f64> {
    let (mut near, mut far) = (0.0_f64, max);
    for i in 0..N {
        if direction[i] == 0.0 {
            if origin[i] < lo[i] || origin[i] > hi[i] {
                return None;
            }
            continue;
        }
        let a = (lo[i] - origin[i]) / direction[i];
        let b = (hi[i] - origin[i]) / direction[i];
        near = near.max(a.min(b));
        far = far.min(a.max(b));
    }
    (near <= far).then_some(near)
}

struct RaySelect<const N: usize> {
    origin: [f64; N],
    direction: [f64; N],
    max: f64,
}

impl<K, const N: usize> SelectionFunction<Entry<K, N>> for RaySelect<N> {
    fn should_unpack_parent(&self, envelope: &AABB<[f64; N]>) -> bool {
        ray_entry(
            &self.origin,
            &self.direction,
            self.max,
            &envelope.lower(),
            &envelope.upper(),
        )
        .is_some()
    }

    fn should_unpack_leaf(&self, leaf: &Entry<K, N>) -> bool {
        ray_entry(&self.origin, &self.direction, self.max, &leaf.lo, &leaf.hi).is_some()
    }
}

/// The dimension-generic implementation: the tree plus a key-to-box map for remove and update.
struct Core<K, const N: usize> {
    tree: RTree<Entry<K, N>>,
    boxes: HashMap<K, Corners<N>>,
}

impl<K: Copy + Eq + Hash + Ord, const N: usize> Core<K, N> {
    fn new() -> Self {
        Self {
            tree: RTree::new(),
            boxes: HashMap::new(),
        }
    }

    fn from_entries(entries: Vec<(K, Corners<N>)>) -> Self {
        let boxes: HashMap<K, Corners<N>> = entries.into_iter().collect();
        let tree = RTree::bulk_load(
            boxes
                .iter()
                .map(|(&key, &(lo, hi))| Entry { key, lo, hi })
                .collect(),
        );
        Self { tree, boxes }
    }

    fn remove(&mut self, key: K) -> Option<Corners<N>> {
        let (lo, hi) = self.boxes.remove(&key)?;
        self.tree.remove(&Entry { key, lo, hi });
        Some((lo, hi))
    }

    fn insert(&mut self, key: K, corners: Corners<N>) -> Option<Corners<N>> {
        let old = self.remove(key);
        self.tree.insert(Entry {
            key,
            lo: corners.0,
            hi: corners.1,
        });
        self.boxes.insert(key, corners);
        old
    }

    fn keys(mut keys: Vec<K>) -> Vec<K> {
        keys.sort_unstable();
        keys
    }

    fn window(&self, lo: [f64; N], hi: [f64; N]) -> Vec<K> {
        let hits = self.tree.locate_in_envelope(AABB::from_corners(lo, hi));
        Self::keys(hits.map(|e| e.key).collect())
    }

    fn crossing(&self, lo: [f64; N], hi: [f64; N]) -> Vec<K> {
        let hits = self
            .tree
            .locate_in_envelope_intersecting(AABB::from_corners(lo, hi));
        Self::keys(hits.map(|e| e.key).collect())
    }

    fn sorted_by_distance(mut hits: Vec<(f64, K)>) -> Vec<(K, f64)> {
        hits.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        hits.into_iter().map(|(d, key)| (key, d)).collect()
    }

    fn nearest(&self, point: [f64; N], k: usize) -> Vec<(K, f64)> {
        if k == 0 {
            return Vec::new();
        }
        let mut hits: Vec<(f64, K)> = Vec::new();
        let mut kth = f64::NAN;
        for (entry, d2) in self.tree.nearest_neighbor_iter_with_distance_2(point) {
            if hits.len() == k {
                kth = d2;
            }
            // Keep every entry tied with the k-th distance so the key tie-break is exact.
            if hits.len() >= k && d2 > kth {
                break;
            }
            hits.push((d2, entry.key));
        }
        let mut sorted = Self::sorted_by_distance(hits);
        sorted.truncate(k);
        sorted
            .into_iter()
            .map(|(key, d2)| (key, d2.sqrt()))
            .collect()
    }

    fn ray(&self, origin: [f64; N], direction: [f64; N], max: f64) -> Vec<(K, f64)> {
        let select = RaySelect {
            origin,
            direction,
            max,
        };
        let hits = self
            .tree
            .locate_with_selection_function(select)
            .filter_map(|e| ray_entry(&origin, &direction, max, &e.lo, &e.hi).map(|t| (t, e.key)))
            .collect();
        Self::sorted_by_distance(hits)
    }
}

/// A unit direction, or an error for a zero or non-finite vector.
fn unit<const N: usize>(direction: [f64; N]) -> Result<[f64; N]> {
    ensure_finite(&direction)?;
    // Scale first so squaring cannot overflow or underflow.
    let scale = direction.iter().fold(0.0_f64, |m, c| m.max(c.abs()));
    if scale == 0.0 {
        return Err(Error::new(codes::DIRECTION_ZERO));
    }
    let scaled = direction.map(|c| c / scale);
    let norm = scaled.iter().map(|c| c * c).sum::<f64>().sqrt();
    Ok(scaled.map(|c| c / norm))
}

fn ensure_finite(values: &[f64]) -> Result<()> {
    crate::ensure_finite(values)
}

macro_rules! spatial_index {
    ($name:ident, $aabb:ident, $point:ident, $vector:ident, $n:literal, $dim:literal) => {
        #[doc = concat!("An R*-tree over ", $dim, "D boxes keyed by `K`; see the [module](self) rules.")]
        ///
        /// Cloning is not provided; build a second index with [`Self::from_entries`].
        pub struct $name<K> {
            core: Core<K, $n>,
        }

        impl<K: Copy + Eq + Hash + Ord> Default for $name<K> {
            fn default() -> Self {
                Self::new()
            }
        }

        impl<K: Copy + Eq + Hash + Ord> $name<K> {
            /// Validates a box: non-empty (`geom.bbox.invalid`) and finite
            /// (`geom.input.not_finite`).
            fn corners(boxed: &$aabb) -> Result<Corners<$n>> {
                if boxed.is_empty() {
                    return Err(Error::new(codes::BBOX_INVALID));
                }
                let (lo, hi): ([f64; $n], [f64; $n]) = (boxed.min().into(), boxed.max().into());
                ensure_finite(&lo)?;
                ensure_finite(&hi)?;
                Ok((lo, hi))
            }

            fn point_array(point: &$point) -> Result<[f64; $n]> {
                let array: [f64; $n] = (*point).into();
                ensure_finite(&array)?;
                Ok(array)
            }

            fn to_box((lo, hi): Corners<$n>) -> $aabb {
                $aabb::from_corners(lo.into(), hi.into())
            }

            /// An empty index.
            #[must_use]
            pub fn new() -> Self {
                Self { core: Core::new() }
            }

            /// Builds an index from `(key, box)` pairs by bulk loading. When a key repeats, the
            /// last pair wins. Fails on the first invalid box, building nothing.
            ///
            /// # Errors
            ///
            /// `geom.bbox.invalid` for an empty box, `geom.input.not_finite` for a non-finite
            /// coordinate.
            pub fn from_entries(entries: impl IntoIterator<Item = (K, $aabb)>) -> Result<Self> {
                let entries = entries
                    .into_iter()
                    .map(|(key, boxed)| Ok((key, Self::corners(&boxed)?)))
                    .collect::<Result<Vec<_>>>()?;
                Ok(Self {
                    core: Core::from_entries(entries),
                })
            }

            /// The number of entries.
            #[must_use]
            pub fn len(&self) -> usize {
                self.core.boxes.len()
            }

            /// Whether the index has no entries.
            #[must_use]
            pub fn is_empty(&self) -> bool {
                self.core.boxes.is_empty()
            }

            /// The box stored for `key`.
            #[must_use]
            pub fn get(&self, key: K) -> Option<$aabb> {
                self.core.boxes.get(&key).map(|&c| Self::to_box(c))
            }

            /// Removes every entry.
            pub fn clear(&mut self) {
                self.core = Core::new();
            }

            /// Inserts `key` with `boxed`, replacing and returning the previous box of an
            /// existing key.
            ///
            /// # Errors
            ///
            /// `geom.bbox.invalid` for an empty box, `geom.input.not_finite` for a non-finite
            /// coordinate; the index is unchanged.
            pub fn insert(&mut self, key: K, boxed: $aabb) -> Result<Option<$aabb>> {
                let corners = Self::corners(&boxed)?;
                Ok(self.core.insert(key, corners).map(Self::to_box))
            }

            /// Removes `key` and returns its box, or `None` when the key is absent.
            pub fn remove(&mut self, key: K) -> Option<$aabb> {
                self.core.remove(key).map(Self::to_box)
            }

            /// Replaces the box of an existing key. Returns `false`, changing nothing, when the
            /// key is absent (use [`Self::insert`] to add it).
            ///
            /// # Errors
            ///
            /// As [`Self::insert`].
            pub fn update(&mut self, key: K, boxed: $aabb) -> Result<bool> {
                let corners = Self::corners(&boxed)?;
                if !self.core.boxes.contains_key(&key) {
                    return Ok(false);
                }
                self.core.insert(key, corners);
                Ok(true)
            }

            /// The keys of the entries lying entirely inside `query` (boundary contact allowed),
            /// ascending.
            ///
            /// # Errors
            ///
            /// As [`Self::insert`] for an empty or non-finite `query`.
            pub fn window(&self, query: &$aabb) -> Result<Vec<K>> {
                let (lo, hi) = Self::corners(query)?;
                Ok(self.core.window(lo, hi))
            }

            /// The keys of the entries whose box intersects `query`; touching counts. Ascending.
            ///
            /// # Errors
            ///
            /// As [`Self::insert`] for an empty or non-finite `query`.
            pub fn crossing(&self, query: &$aabb) -> Result<Vec<K>> {
                let (lo, hi) = Self::corners(query)?;
                Ok(self.core.crossing(lo, hi))
            }

            /// The `k` entries nearest to `point` with their distances, ordered by distance and
            /// then key. The distance to a box is the Euclidean distance to its closest point,
            /// zero inside or on it. Fewer than `k` are returned when the index is smaller;
            /// `k == 0` gives an empty result.
            ///
            /// # Errors
            ///
            /// `geom.input.not_finite` for a non-finite `point`.
            pub fn nearest(&self, point: &$point, k: usize) -> Result<Vec<(K, f64)>> {
                Ok(self.core.nearest(Self::point_array(point)?, k))
            }

            /// The entries hit by the ray from `origin` along `direction`, up to `max_distance`,
            /// with their entry distances, ordered by distance and then key.
            ///
            /// The distance is measured along the ray in length units (the direction is
            /// normalized) and is 0 when the origin is inside or on the box. A box is hit when
            /// the ray meets it at a distance in `0..=max_distance`; touching counts. A zero
            /// direction component hits only boxes whose slab on that axis contains the origin
            /// coordinate. A negative `max_distance` yields an empty result.
            ///
            /// # Errors
            ///
            /// `geom.input.not_finite` for a non-finite `origin`, `direction` or
            /// `max_distance`; `geom.direction.zero` for a zero `direction`.
            pub fn ray(
                &self,
                origin: &$point,
                direction: &$vector,
                max_distance: f64,
            ) -> Result<Vec<(K, f64)>> {
                let origin = Self::point_array(origin)?;
                let direction = unit((*direction).into())?;
                ensure_finite(&[max_distance])?;
                Ok(self.core.ray(origin, direction, max_distance))
            }
        }
    };
}

spatial_index!(SpatialIndex2, Aabb2, Point2, Vector2, 2, 2);
spatial_index!(SpatialIndex3, Aabb3, Point3, Vector3, 3, 3);

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;

    use super::*;

    fn p2(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn b2(x0: f64, y0: f64, x1: f64, y1: f64) -> Aabb2 {
        Aabb2::from_corners(p2(x0, y0), p2(x1, y1))
    }

    #[test]
    fn insert_replaces_and_remove_update_by_key() {
        let mut index = SpatialIndex2::new();
        assert!(index.is_empty());
        assert_eq!(index.insert(1_u32, b2(0.0, 0.0, 1.0, 1.0)).ok(), Some(None));
        assert_eq!(
            index.insert(1, b2(5.0, 5.0, 6.0, 6.0)).ok(),
            Some(Some(b2(0.0, 0.0, 1.0, 1.0)))
        );
        assert_eq!(index.len(), 1);
        assert_eq!(index.update(2, b2(0.0, 0.0, 1.0, 1.0)).ok(), Some(false));
        assert_eq!(index.update(1, b2(0.0, 0.0, 2.0, 2.0)).ok(), Some(true));
        assert_eq!(index.get(1), Some(b2(0.0, 0.0, 2.0, 2.0)));
        assert_eq!(index.window(&b2(-1.0, -1.0, 3.0, 3.0)).ok(), Some(vec![1]));
        assert_eq!(index.remove(1), Some(b2(0.0, 0.0, 2.0, 2.0)));
        assert_eq!(index.remove(1), None);
        assert!(
            index
                .crossing(&b2(-9.0, -9.0, 9.0, 9.0))
                .unwrap()
                .is_empty()
        );
        index.insert(3, b2(0.0, 0.0, 1.0, 1.0)).unwrap();
        index.clear();
        assert!(index.is_empty());
    }

    #[test]
    fn invalid_input_is_rejected_without_changing_the_index() {
        let mut index = SpatialIndex2::from_entries([(1_u32, b2(0.0, 0.0, 1.0, 1.0))]).unwrap();
        let bad_box = Aabb2::from_corners(p2(0.0, 0.0), p2(f64::INFINITY, 1.0));
        let code = |e: Error| e.code();
        assert_eq!(
            index.insert(2, Aabb2::EMPTY).map_err(code),
            Err("geom.bbox.invalid")
        );
        assert_eq!(
            index.insert(2, bad_box).map_err(code),
            Err("geom.input.not_finite")
        );
        assert_eq!(
            index.update(1, Aabb2::EMPTY).map_err(code),
            Err("geom.bbox.invalid")
        );
        assert_eq!(
            index.window(&Aabb2::EMPTY).map_err(code),
            Err("geom.bbox.invalid")
        );
        assert_eq!(
            index.nearest(&p2(f64::NAN, 0.0), 1).map_err(code),
            Err("geom.input.not_finite")
        );
        let origin = p2(0.0, 0.0);
        assert_eq!(
            index.ray(&origin, &Vector2::zeros(), 1.0).map_err(code),
            Err("geom.direction.zero")
        );
        assert_eq!(
            index
                .ray(&origin, &Vector2::new(1.0, 0.0), f64::NAN)
                .map_err(code),
            Err("geom.input.not_finite")
        );
        assert!(
            SpatialIndex2::<u32>::from_entries([(1, b2(0.0, 0.0, 1.0, 1.0)), (2, Aabb2::EMPTY)])
                .is_err()
        );
        assert_eq!(index.len(), 1);
        assert_eq!(index.get(1), Some(b2(0.0, 0.0, 1.0, 1.0)));
    }

    #[test]
    fn touching_counts_for_crossing_but_window_needs_containment() {
        let index = SpatialIndex2::from_entries([(1_u32, b2(0.0, 0.0, 1.0, 1.0))]).unwrap();
        assert_eq!(index.crossing(&b2(1.0, 1.0, 2.0, 2.0)).ok(), Some(vec![1]));
        assert_eq!(index.crossing(&b2(1.1, 1.0, 2.0, 2.0)).ok(), Some(vec![]));
        assert_eq!(index.window(&b2(0.0, 0.0, 1.0, 1.0)).ok(), Some(vec![1]));
        assert_eq!(index.window(&b2(0.0, 0.0, 0.9, 1.0)).ok(), Some(vec![]));
    }

    #[test]
    fn nearest_orders_by_distance_then_key_with_inside_zero() {
        let index = SpatialIndex2::from_entries([
            (9_u32, b2(2.0, 0.0, 3.0, 1.0)),
            (4, b2(-3.0, 0.0, -2.0, 1.0)),
            (7, b2(-1.0, -1.0, 1.0, 1.0)),
        ])
        .unwrap();
        let hits = index.nearest(&p2(0.0, 0.0), 3).unwrap();
        assert_eq!(hits, vec![(7, 0.0), (4, 2.0), (9, 2.0)]);
        // The tie at distance 2 is cut by key, not by tree order.
        assert_eq!(index.nearest(&p2(0.0, 0.0), 2).unwrap()[1].0, 4);
        assert!(index.nearest(&p2(0.0, 0.0), 0).unwrap().is_empty());
    }

    #[test]
    fn ray_handles_axis_parallel_inside_and_range() {
        let index = SpatialIndex2::from_entries([
            (1_u32, b2(2.0, -1.0, 3.0, 1.0)),
            (2, b2(5.0, -0.5, 6.0, 2.0)),
            (3, b2(-1.0, -1.0, 1.0, 1.0)),
            (4, b2(2.0, 1.5, 3.0, 2.0)),
        ])
        .unwrap();
        let east = Vector2::new(10.0, 0.0);
        // Entry 3 contains the origin (distance 0); 4 is outside the zero-y slab; 2 starts at 5.
        assert_eq!(
            index.ray(&p2(0.0, 0.0), &east, 100.0).unwrap(),
            vec![(3, 0.0), (1, 2.0), (2, 5.0)]
        );
        assert_eq!(
            index.ray(&p2(0.0, 0.0), &east, 4.0).unwrap(),
            vec![(3, 0.0), (1, 2.0)]
        );
        assert!(index.ray(&p2(0.0, 0.0), &east, -1.0).unwrap().is_empty());
        // Behind the origin is not hit; a grazing ray along an edge is.
        assert_eq!(
            index
                .ray(&p2(4.0, 1.0), &Vector2::new(-1.0, 0.0), 100.0)
                .unwrap(),
            vec![(1, 1.0), (3, 3.0)]
        );
    }

    #[test]
    fn three_dimensional_index_answers_all_queries() {
        let b3 = |lo: [f64; 3], hi: [f64; 3]| Aabb3::from_corners(lo.into(), hi.into());
        let index = SpatialIndex3::from_entries([
            (1_u32, b3([0.0; 3], [1.0; 3])),
            (2, b3([4.0, 0.0, 0.0], [5.0, 1.0, 1.0])),
        ])
        .unwrap();
        let all = b3([-1.0; 3], [9.0; 3]);
        assert_eq!(index.window(&all).ok(), Some(vec![1, 2]));
        assert_eq!(index.crossing(&b3([1.0; 3], [2.0; 3])).unwrap(), vec![1]);
        assert_eq!(
            index.nearest(&[3.0, 0.5, 0.5].into(), 2).unwrap(),
            vec![(2, 1.0), (1, 2.0)]
        );
        assert_eq!(
            index
                .ray(&[-2.0, 0.5, 0.5].into(), &Vector3::new(1.0, 0.0, 0.0), 10.0)
                .unwrap(),
            vec![(1, 2.0), (2, 6.0)]
        );
    }

    #[derive(Debug, Clone)]
    enum Op<const N: usize> {
        Insert(u8, Corners<N>),
        Remove(u8),
        Update(u8, Corners<N>),
    }

    /// Integer-grid boxes force touching boxes and exact distance ties.
    fn corners<const N: usize>() -> impl Strategy<Value = Corners<N>> {
        prop::collection::vec((-8_i8..8, 0_i8..5), N).prop_map(|axes| {
            let mut lo = [0.0; N];
            let mut hi = [0.0; N];
            for (i, (a, len)) in axes.into_iter().enumerate() {
                lo[i] = f64::from(a);
                hi[i] = f64::from(a) + f64::from(len);
            }
            (lo, hi)
        })
    }

    fn point<const N: usize>() -> impl Strategy<Value = [f64; N]> {
        prop::collection::vec(-10_i8..10, N).prop_map(|v| {
            let mut p = [0.0; N];
            for (slot, c) in p.iter_mut().zip(v) {
                *slot = f64::from(c) / 2.0;
            }
            p
        })
    }

    fn op<const N: usize>() -> impl Strategy<Value = Op<N>> {
        prop_oneof![
            (0_u8..24, corners::<N>()).prop_map(|(k, c)| Op::Insert(k, c)),
            (0_u8..24).prop_map(Op::Remove),
            (0_u8..24, corners::<N>()).prop_map(|(k, c)| Op::Update(k, c)),
        ]
    }

    fn contains<const N: usize>(outer: &Corners<N>, inner: &Corners<N>) -> bool {
        (0..N).all(|i| outer.0[i] <= inner.0[i] && inner.1[i] <= outer.1[i])
    }

    fn intersects<const N: usize>(a: &Corners<N>, b: &Corners<N>) -> bool {
        (0..N).all(|i| a.0[i] <= b.1[i] && b.0[i] <= a.1[i])
    }

    /// Replays `ops` on the core and on a `BTreeMap` model, then compares every query.
    fn check_model<const N: usize>(
        ops: &[Op<N>],
        query: Corners<N>,
        point: [f64; N],
        direction: [f64; N],
        k: usize,
        max: f64,
        reversed: bool,
    ) -> std::result::Result<(), TestCaseError> {
        let mut core = Core::<u8, N>::new();
        let mut model: BTreeMap<u8, Corners<N>> = BTreeMap::new();
        for op in ops {
            match *op {
                Op::Insert(key, c) => {
                    prop_assert_eq!(core.insert(key, c), model.insert(key, c));
                }
                Op::Remove(key) => prop_assert_eq!(core.remove(key), model.remove(&key)),
                Op::Update(key, c) => {
                    if model.contains_key(&key) {
                        core.insert(key, c);
                        model.insert(key, c);
                    }
                }
            }
        }
        prop_assert_eq!(core.boxes.len(), model.len());
        prop_assert_eq!(core.tree.size(), model.len());
        // A bulk-loaded tree over the same entries, in either order, must agree.
        let mut entries: Vec<_> = model.iter().map(|(&k, &c)| (k, c)).collect();
        if reversed {
            entries.reverse();
        }
        let bulk = Core::from_entries(entries);
        for tree in [&core, &bulk] {
            let want: Vec<u8> = model
                .iter()
                .filter(|(_, c)| contains(&query, c))
                .map(|(&k, _)| k)
                .collect();
            prop_assert_eq!(tree.window(query.0, query.1), want);
            let want: Vec<u8> = model
                .iter()
                .filter(|(_, c)| intersects(&query, c))
                .map(|(&k, _)| k)
                .collect();
            prop_assert_eq!(tree.crossing(query.0, query.1), want);

            let mut by_distance: Vec<(u8, f64)> = model
                .iter()
                .map(|(&k, c)| (k, distance_2(&c.0, &c.1, &point).sqrt()))
                .collect();
            by_distance.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            by_distance.truncate(k);
            prop_assert_eq!(tree.nearest(point, k), by_distance);

            if let Ok(unit_dir) = unit(direction) {
                let mut hits: Vec<(u8, f64)> = model
                    .iter()
                    .filter_map(|(&k, c)| {
                        ray_entry(&point, &unit_dir, max, &c.0, &c.1).map(|t| (k, t))
                    })
                    .collect();
                hits.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                prop_assert_eq!(tree.ray(point, unit_dir, max), hits);
            }
        }
        Ok(())
    }

    fn direction<const N: usize>() -> impl Strategy<Value = [f64; N]> {
        // Zero components are common so the axis-parallel branch is exercised.
        prop::collection::vec(-2_i8..=2, N).prop_map(|v| {
            let mut d = [0.0; N];
            for (slot, c) in d.iter_mut().zip(v) {
                *slot = f64::from(c);
            }
            d
        })
    }

    proptest! {
        #[test]
        fn index_equals_brute_force_2d(
            ops in prop::collection::vec(op::<2>(), 0..60),
            query in corners::<2>(),
            p in point::<2>(),
            d in direction::<2>(),
            k in 0_usize..8,
            max in 0.0..40.0_f64,
            reversed in any::<bool>(),
        ) {
            check_model(&ops, query, p, d, k, max, reversed)?;
        }

        #[test]
        fn index_equals_brute_force_3d(
            ops in prop::collection::vec(op::<3>(), 0..60),
            query in corners::<3>(),
            p in point::<3>(),
            d in direction::<3>(),
            k in 0_usize..8,
            max in 0.0..40.0_f64,
            reversed in any::<bool>(),
        ) {
            check_model(&ops, query, p, d, k, max, reversed)?;
        }

        #[test]
        fn results_do_not_depend_on_insertion_order(
            entries in prop::collection::vec((0_u8..40, corners::<2>()), 0..50),
            p in point::<2>(),
        ) {
            let boxed = |(k, (lo, hi)): &(u8, Corners<2>)| {
                (*k, Aabb2::from_corners((*lo).into(), (*hi).into()))
            };
            let mut reversed = entries.clone();
            reversed.reverse();
            // Distinct keys, so order cannot change which box a key holds.
            let distinct: BTreeMap<u8, Corners<2>> = entries.into_iter().collect();
            let forward: Vec<_> = distinct.iter().map(|(&k, &c)| (k, c)).collect();
            let backward: Vec<_> = forward.iter().rev().cloned().collect();
            let a = SpatialIndex2::from_entries(forward.iter().map(boxed)).unwrap();
            let mut b = SpatialIndex2::new();
            for e in &backward {
                let (k, bx) = boxed(e);
                b.insert(k, bx).unwrap();
            }
            let point = Point2::new(p[0], p[1]);
            prop_assert_eq!(a.nearest(&point, 10).unwrap(), b.nearest(&point, 10).unwrap());
            let all = Aabb2::from_corners(Point2::new(-20.0, -20.0), Point2::new(20.0, 20.0));
            prop_assert_eq!(a.crossing(&all).unwrap(), b.crossing(&all).unwrap());
            drop(reversed);
        }
    }
}
