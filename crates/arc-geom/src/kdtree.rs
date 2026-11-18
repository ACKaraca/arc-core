//! Point k-d trees for nearest-neighbor and radius queries (CORE-GEOM-028).
//!
//! [`KdTree2`] indexes 2D points and [`KdTree3`] 3D points, each with a caller key of type `K`.
//! The tree is built once from all points and is not changed afterwards; it is meant for
//! snapping to vertices and for point clouds, whose point sets are rebuilt rather than edited.
//!
//! - The points are stored in one array arranged as an implicit balanced tree: the median of a
//!   range, by the coordinate of the axis that cycles with the depth (ties by key, then by input
//!   order), is the node, and the halves before and after it are its subtrees. Building takes
//!   `O(n log n)` time without recursion and no memory beyond the array.
//! - A query visits the near subtree first and skips the far one only when the distance from
//!   the query to the splitting plane is strictly larger than the distance it must beat, so
//!   entries tied with that distance are always seen. A query takes `O(log n)` time for
//!   well-spread points and at most `O(n)`.
//! - Distances are ranked by the squared distance with an extended exponent range, as in the
//!   [`crate::index`] R*-tree: the ranking is exact where the squared distance would overflow or
//!   underflow. Results are sorted by distance, then key, then input order, so they do not
//!   depend on the arrangement of the tree, and equal a brute-force scan.
//! - A point or query with a non-finite coordinate, or a non-finite radius, fails with
//!   `geom.input.not_finite`; more points than `limits.array_length` fail with
//!   `geom.index.too_many_points`. A negative radius finds nothing; the radius ball is closed.

use std::cmp::Ordering;

use arc_base::math::{Point2, Point3};
use arc_base::{Error, Limits, Result};

use crate::error::codes;
use crate::index::Ranked;

/// One indexed point with its key and its position in the input.
#[derive(Debug, Clone, Copy)]
struct Entry<K, const N: usize> {
    key: K,
    point: [f64; N],
    order: usize,
}

/// The implicit tree over `N`-dimensional points.
#[derive(Debug, Clone)]
struct Tree<K, const N: usize> {
    entries: Vec<Entry<K, N>>,
}

/// A candidate result: its rank, key and input order.
struct Hit<K> {
    rank: Ranked,
    key: K,
    order: usize,
}

impl<K: Ord> Hit<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank
            .cmp(&other.rank)
            .then_with(|| self.key.cmp(&other.key))
            .then(self.order.cmp(&other.order))
    }
}

fn not_finite() -> Error {
    Error::new(codes::INPUT_NOT_FINITE)
}

impl<K: Copy + Ord, const N: usize> Tree<K, N> {
    fn build(points: Vec<(K, [f64; N])>, limits: &Limits) -> Result<Self> {
        let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
        if points.len() > limit {
            return Err(Error::new(codes::INDEX_TOO_MANY_POINTS)
                .with_detail("count", i64::try_from(points.len()).unwrap_or(i64::MAX)));
        }
        if points.iter().any(|(_, p)| p.iter().any(|x| !x.is_finite())) {
            return Err(not_finite());
        }
        let mut entries: Vec<Entry<K, N>> = points
            .into_iter()
            .enumerate()
            .map(|(order, (key, point))| Entry { key, point, order })
            .collect();
        let mut stack = vec![(0, entries.len(), 0usize)];
        while let Some((lo, hi, depth)) = stack.pop() {
            if hi - lo < 2 {
                continue;
            }
            let axis = depth % N;
            let mid = lo + (hi - lo) / 2;
            if let Some(range) = entries.get_mut(lo..hi) {
                range.select_nth_unstable_by(mid - lo, |a, b| {
                    let (x, y) = (
                        a.point.get(axis).copied().unwrap_or(0.0),
                        b.point.get(axis).copied().unwrap_or(0.0),
                    );
                    x.total_cmp(&y)
                        .then_with(|| a.key.cmp(&b.key))
                        .then(a.order.cmp(&b.order))
                });
            }
            stack.push((lo, mid, depth + 1));
            stack.push((mid + 1, hi, depth + 1));
        }
        Ok(Self { entries })
    }

    fn hit(entry: &Entry<K, N>, query: &[f64; N]) -> Hit<K> {
        Hit {
            rank: Ranked::new(&entry.point, &entry.point, query),
            key: entry.key,
            order: entry.order,
        }
    }

    /// Visits the tree near side first, skipping a far side whose plane distance `bound`
    /// rejects; `visit` sees each entry and returns the rank a far side must not exceed.
    fn search(
        &self,
        query: &[f64; N],
        mut visit: impl FnMut(&Entry<K, N>) -> Option<Ranked>,
        initial: Option<Ranked>,
    ) {
        let mut limit = initial;
        // (lo, hi, depth, plane bound of the range, if any)
        let mut stack: Vec<(usize, usize, usize, Option<Ranked>)> =
            vec![(0, self.entries.len(), 0, None)];
        while let Some((lo, hi, depth, bound)) = stack.pop() {
            if hi <= lo {
                continue;
            }
            if let (Some(bound), Some(limit)) = (&bound, &limit)
                && bound.cmp(limit) == Ordering::Greater
            {
                continue;
            }
            let mid = lo + (hi - lo) / 2;
            let Some(node) = self.entries.get(mid) else {
                continue;
            };
            if let Some(new) = visit(node) {
                limit = Some(new);
            }
            let axis = depth % N;
            let split = node.point.get(axis).copied().unwrap_or(0.0);
            let q = query.get(axis).copied().unwrap_or(0.0);
            let mut plane = *query;
            if let Some(slot) = plane.get_mut(axis) {
                *slot = split;
            }
            let far_bound = Ranked::new(&plane, &plane, query);
            let (near, far) = if q < split {
                ((lo, mid), (mid + 1, hi))
            } else {
                ((mid + 1, hi), (lo, mid))
            };
            // The far side, popped last; it keeps the larger of its own and the parent bound.
            let far_bound = match bound {
                Some(b) if b.cmp(&far_bound) == Ordering::Greater => b,
                _ => far_bound,
            };
            stack.push((far.0, far.1, depth + 1, Some(far_bound)));
            stack.push((near.0, near.1, depth + 1, bound));
        }
    }

    fn nearest(&self, query: &[f64; N], k: usize) -> Result<Vec<(K, f64)>> {
        if query.iter().any(|x| !x.is_finite()) {
            return Err(not_finite());
        }
        let k = k.min(self.entries.len());
        if k == 0 {
            return Ok(Vec::new());
        }
        let mut best: Vec<Hit<K>> = Vec::with_capacity(k + 1);
        self.search(
            query,
            |entry| {
                let hit = Self::hit(entry, query);
                let at = best.partition_point(|b| b.cmp(&hit) == Ordering::Less);
                if at < k {
                    best.insert(at, hit);
                    best.truncate(k);
                }
                if best.len() == k {
                    best.last().map(|h| h.rank)
                } else {
                    None
                }
            },
            None,
        );
        Ok(best.into_iter().map(|h| (h.key, h.rank.distance)).collect())
    }

    fn within(&self, query: &[f64; N], radius: f64) -> Result<Vec<(K, f64)>> {
        if query.iter().any(|x| !x.is_finite()) || !radius.is_finite() {
            return Err(not_finite());
        }
        if radius < 0.0 {
            return Ok(Vec::new());
        }
        let limit = Ranked::new(&[radius], &[radius], &[0.0]);
        let mut found: Vec<Hit<K>> = Vec::new();
        self.search(
            query,
            |entry| {
                let hit = Self::hit(entry, query);
                if hit.rank.cmp(&limit) != Ordering::Greater {
                    found.push(hit);
                }
                None
            },
            Some(limit),
        );
        found.sort_by(Hit::cmp);
        Ok(found
            .into_iter()
            .map(|h| (h.key, h.rank.distance))
            .collect())
    }
}

macro_rules! kd_tree {
    ($name:ident, $point:ty, $n:literal, $doc:literal) => {
        #[doc = $doc]
        ///
        /// See the [module documentation](self) for the semantics.
        #[derive(Debug, Clone)]
        pub struct $name<K> {
            tree: Tree<K, $n>,
        }

        impl<K: Copy + Ord> $name<K> {
            fn array(point: &$point) -> [f64; $n] {
                std::array::from_fn(|i| point[i])
            }

            /// Builds the tree from keyed points, with at most `Limits::DEFAULT.array_length`
            /// of them. Keys need not be distinct.
            ///
            /// # Errors
            ///
            /// As [`Self::with_limits`].
            pub fn new(points: impl IntoIterator<Item = (K, $point)>) -> Result<Self> {
                Self::with_limits(points, &Limits::DEFAULT)
            }

            /// Builds the tree from keyed points, with at most `limits.array_length` of them.
            ///
            /// # Errors
            ///
            /// `geom.index.too_many_points` for more points than the limit;
            /// `geom.input.not_finite` for a point with a non-finite coordinate.
            pub fn with_limits(
                points: impl IntoIterator<Item = (K, $point)>,
                limits: &Limits,
            ) -> Result<Self> {
                let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
                let mut collected = Vec::new();
                for (key, point) in points {
                    if collected.len() >= limit {
                        return Err(Error::new(codes::INDEX_TOO_MANY_POINTS)
                            .with_detail("limit", i64::try_from(limit).unwrap_or(i64::MAX)));
                    }
                    collected.push((key, Self::array(&point)));
                }
                Ok(Self {
                    tree: Tree::build(collected, limits)?,
                })
            }

            /// The number of points.
            #[must_use]
            pub fn len(&self) -> usize {
                self.tree.entries.len()
            }

            /// Whether the tree holds no points.
            #[must_use]
            pub fn is_empty(&self) -> bool {
                self.tree.entries.is_empty()
            }

            /// The `k` points nearest to `query` with their distances, sorted by distance, then
            /// key, then input order; fewer when the tree holds fewer.
            ///
            /// # Errors
            ///
            /// `geom.input.not_finite` for a non-finite query.
            pub fn nearest(&self, query: &$point, k: usize) -> Result<Vec<(K, f64)>> {
                self.tree.nearest(&Self::array(query), k)
            }

            /// Every point at a distance of at most `radius` from `query`, with its distance,
            /// sorted by distance, then key, then input order.
            ///
            /// # Errors
            ///
            /// `geom.input.not_finite` for a non-finite query or radius.
            pub fn within(&self, query: &$point, radius: f64) -> Result<Vec<(K, f64)>> {
                self.tree.within(&Self::array(query), radius)
            }
        }
    };
}

kd_tree!(KdTree2, Point2, 2, "A k-d tree of keyed 2D points.");
kd_tree!(KdTree3, Point3, 3, "A k-d tree of keyed 3D points.");

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn brute<const N: usize>(
        points: &[(u32, [f64; N])],
        query: &[f64; N],
    ) -> Vec<(u32, f64, Ranked, usize)> {
        let mut all: Vec<(u32, f64, Ranked, usize)> = points
            .iter()
            .enumerate()
            .map(|(order, (key, p))| {
                let rank = Ranked::new(p, p, query);
                (*key, rank.distance, rank, order)
            })
            .collect();
        all.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)).then(a.3.cmp(&b.3)));
        all
    }

    fn grid_point<const N: usize>() -> impl Strategy<Value = [f64; N]> {
        prop::array::uniform(-6i32..=6).prop_map(|a| a.map(f64::from))
    }

    fn scaled<const N: usize>(p: [f64; N], exponent: i32) -> [f64; N] {
        p.map(|x| x * 2f64.powi(exponent))
    }

    #[test]
    fn hostile_input_and_degenerate_queries() {
        let nan = KdTree2::new([(1, Point2::new(f64::NAN, 0.0))]);
        assert_eq!(nan.unwrap_err().code(), "geom.input.not_finite");
        let tight = Limits {
            array_length: 2,
            ..Limits::STRICT
        };
        let many = (0..10).map(|i| (i, Point2::new(f64::from(i), 0.0)));
        assert_eq!(
            KdTree2::with_limits(many, &tight).unwrap_err().code(),
            "geom.index.too_many_points"
        );
        let empty = KdTree2::<u8>::new([]).unwrap();
        assert!(empty.is_empty());
        assert!(empty.nearest(&Point2::origin(), 3).unwrap().is_empty());
        let tree = KdTree2::new([(2, Point2::new(1.0, 0.0)), (1, Point2::new(-1.0, 0.0))]).unwrap();
        assert_eq!(tree.len(), 2);
        // A tie at distance 1 goes to the smaller key.
        assert_eq!(tree.nearest(&Point2::origin(), 1).unwrap(), vec![(1, 1.0)]);
        assert_eq!(
            tree.within(&Point2::origin(), 1.0).unwrap(),
            vec![(1, 1.0), (2, 1.0)]
        );
        assert!(tree.within(&Point2::origin(), -1.0).unwrap().is_empty());
        assert_eq!(
            tree.within(&Point2::origin(), f64::NAN).unwrap_err().code(),
            "geom.input.not_finite"
        );
        assert_eq!(
            tree.nearest(&Point2::new(f64::INFINITY, 0.0), 1)
                .unwrap_err()
                .code(),
            "geom.input.not_finite"
        );
        let far = KdTree3::new([
            (0, Point3::new(1e300, -1e300, 0.0)),
            (1, Point3::new(-1e300, 1e300, 0.0)),
        ])
        .unwrap();
        let hits = far.nearest(&Point3::new(1e300, 1e300, 0.0), 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].1, 2e300);
    }

    fn check<const N: usize>(
        points: &[(u32, [f64; N])],
        query: &[f64; N],
        k: usize,
        radius: f64,
    ) -> std::result::Result<(), TestCaseError> {
        let tree = Tree::build(points.to_vec(), &Limits::DEFAULT).unwrap();
        let expected = brute(points, query);
        let nearest = tree.nearest(query, k).unwrap();
        let want: Vec<(u32, f64)> = expected.iter().take(k).map(|e| (e.0, e.1)).collect();
        prop_assert_eq!(nearest, want);
        let limit = Ranked::new(&[radius], &[radius], &[0.0]);
        let within = tree.within(query, radius).unwrap();
        let want: Vec<(u32, f64)> = expected
            .iter()
            .filter(|e| e.2.cmp(&limit) != Ordering::Greater)
            .map(|e| (e.0, e.1))
            .collect();
        prop_assert_eq!(within, want);
        Ok(())
    }

    proptest! {
        #[test]
        fn queries_equal_brute_force_in_2d(
            points in prop::collection::vec((0u32..8, grid_point::<2>()), 0..60),
            query in grid_point::<2>(),
            k in 0usize..12,
            radius in 0i32..8,
            exponent in -1000i32..1000,
        ) {
            let points: Vec<_> = points.into_iter().map(|(k, p)| (k, scaled(p, exponent))).collect();
            let radius = f64::from(radius) * 2f64.powi(exponent);
            check(&points, &scaled(query, exponent), k, radius)?;
        }

        #[test]
        fn queries_equal_brute_force_in_3d(
            points in prop::collection::vec((0u32..8, grid_point::<3>()), 0..60),
            query in grid_point::<3>(),
            k in 0usize..12,
            radius in 0i32..8,
        ) {
            check(&points, &query, k, f64::from(radius))?;
        }

        #[test]
        fn random_points_equal_brute_force(
            points in prop::collection::vec((0u32..1000, prop::array::uniform::<_, 2>(-1e3..1e3_f64)), 0..200),
            query in prop::array::uniform::<_, 2>(-1.2e3..1.2e3_f64),
            k in 0usize..20,
            radius in 0.0..500.0_f64,
        ) {
            check(&points, &query, k, radius)?;
        }
    }
}
