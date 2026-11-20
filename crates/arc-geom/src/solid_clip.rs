//! Clipping a polyhedral solid by a plane, and the section of a solid by a plane.
//!
//! [`Polyhedron::clip`] keeps the positive or negative side of a [`Plane3`] and closes the cut
//! with new faces; [`Polyhedron::section`] returns those cut faces as 2D regions in the frame of
//! the plane, which is what a plan or section view cuts.
//!
//! # Method
//!
//! Vertices are classified by their signed distance to the plane as *on* (within
//! `tolerance.linear`), strictly positive or strictly negative. The cut is made as if the plane
//! were moved by an infinitesimal amount against its normal, for both sides alike: an *on* vertex
//! lies on the positive side of the moved plane and on the negative side of it is not kept. So
//! `Positive` keeps the *on* vertices and `Negative` drops them, the two results are exact
//! complements of each other, and their volumes sum to the volume of the solid (to within the
//! tolerance times the area of the cut). A cut within the tolerance of a vertex is a cut through
//! the vertex, on both sides.
//!
//! 1. No vertex strictly on the kept side: `None`. No vertex strictly on the other side: the
//!    solid is returned unchanged (and has no section).
//! 2. Each face is clipped on its own. A face lying in the plane (all vertices *on*) is kept by
//!    `Positive` when its outward normal points away from the kept side and dropped otherwise,
//!    and always dropped by `Negative`; its place is taken by the cap where the solid continues.
//!    A face with all vertices kept is unchanged and one with none is dropped. A face with both
//!    kinds is cut: the loops are cut into runs of kept boundary, and the crossing points along
//!    the cut line are joined in pairs (new vertices on crossing edges are shared by the two
//!    faces of an edge, so neighbors match exactly, and lie exactly on the plane). All boundary
//!    edges of the piece, the joining edges split at the *on* vertices of the face between them,
//!    are collected and edges that meet their exact reverse cancel: a zero-width bridge, such as
//!    the door head of a wall cut at the height of its top edge, or the sill edge of a window
//!    hole cut at the height of the sill, disappears and the face splits or the hole opens into
//!    a notch. The remaining edges are linked into loops, turning at a vertex that more than two
//!    edges meet by the tightest left turn, which separates pieces that touch at a vertex.
//! 3. The directed edges left without a reverse partner all lie in the cutting plane. Reversed,
//!    they are linked in the same way into the loops of the cap; loops that run against the cap
//!    normal are holes and are assigned to the outer loop that contains them. Each outer loop
//!    with its holes is one new face, tagged [`FaceTag::Cap`] with the cut number the caller gave to [`Polyhedron::clip`].
//! 4. The result is validated as [`Polyhedron::new`] validates, which includes that every face
//!    triangulates. A result whose volume is not positive (a sliver within the tolerance) is
//!    `None`.
//!
//! Every output face keeps the tag of its source face; a face that is split yields several faces
//! with the same tag. The cap faces carry [`FaceTag::Cap`] with the `cut` number the caller
//! passes to [`Polyhedron::clip`], so caps of different cuts stay apart. The result may consist
//! of several shells (a cut through two walls, or through the two prongs of a notch above its
//! apex): see [`Polyhedron::shell_count`]; every shell is closed and the mesh of the result uses
//! every edge exactly twice.
//!
//! # Section
//!
//! [`Polyhedron::section`] is the cap of the `Positive` clip: the part of the plane where the
//! solid has material on both sides. A plane through the top edge of a door gives the wall
//! without the door (the head above the door is a face of the solid in the plane, not a cut),
//! and a plane through the sill of a window gives the wall without the window.
//!
//! # Degenerate cases
//!
//! | Case | Result |
//! |---|---|
//! | Plane through vertices, along edges, along an opening's head, sill or jamb | Handled by the rule above; both sides are valid, every face triangulates |
//! | Plane contains a face | See step 2; the cap replaces it or it stays |
//! | Plane touches the solid at a face, edge or vertex only | `None` or the unchanged solid, by side |
//! | Plane splits one face into several pieces, or cuts a hole open | Handled |
//! | Parts meeting along an edge or at a vertex after the cut | Both parts are returned; the loops are separated by the left-turn rule |
//! | Two parts of the solid left meeting along an edge that lies inside the cap (the `Negative` side of a plane through the apex of a V-shaped notch: the prongs share the apex edge) | `geom.solid.clip_degenerate`: the cap would use that edge twice when meshed. The `Positive` side is the block above the notch and is valid |
//! | A hole of a face touching the cut line at a single vertex (a pointed hole with its tip in the plane) | `geom.solid.clip_degenerate`: the piece would have a hole sharing a vertex with its outer loop |
//! | Plane within `tolerance.angular` of parallel to a face with vertices on both sides of it (possible only for solids over about 1000 km in size) | `geom.solid.clip_degenerate` |
//! | Crossing points along a cut line that cannot be paired | `geom.solid.clip_degenerate` |
//! | Result fails validation | `geom.solid.clip_degenerate` |

use std::collections::{HashMap, HashSet};
use std::f64::consts::TAU;

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::plane3::Plane3;
use crate::primitives::Segment2;
use crate::region::{Loop2, Region, RegionPart};
use crate::solid::{
    FaceTag, PolyFace, Polyhedron, inside_projected, newell, project_point, projection,
};

/// The side of a cutting plane that [`Polyhedron::clip`] keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSide {
    /// The side the plane's normal points to.
    Positive,
    /// The side opposite to the plane's normal.
    Negative,
}

fn degenerate() -> Error {
    Error::new(codes::SOLID_CLIP_DEGENERATE)
}

/// Removes every directed edge that meets an exact reverse partner, one pair per match, and
/// returns the others in their original order.
pub(crate) fn cancel_opposite(edges: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut count: HashMap<(u32, u32), u32> = HashMap::with_capacity(edges.len());
    for &(a, b) in edges {
        let partner = count.get(&(b, a)).copied().unwrap_or(0) > 0;
        if partner {
            if let Some(c) = count.get_mut(&(b, a)) {
                *c -= 1;
            }
        } else {
            *count.entry((a, b)).or_insert(0) += 1;
        }
    }
    let mut out = Vec::new();
    for &edge in edges {
        if let Some(c) = count.get_mut(&edge)
            && *c > 0
        {
            *c -= 1;
            out.push(edge);
        }
    }
    out
}

/// Links directed edges into closed loops. At a vertex with several unused outgoing edges the
/// loop takes the one that turns left the most (the smallest clockwise angle from the reversed
/// incoming edge), so regions that touch at a vertex come out as separate loops. `point` gives
/// the 2D position of a vertex in coordinates where counterclockwise is the loop direction.
/// `None` when the edges do not form closed loops.
pub(crate) fn link_loops(
    edges: &[(u32, u32)],
    point: &dyn Fn(u32) -> Point2,
) -> Option<Vec<Vec<u32>>> {
    let mut outgoing: HashMap<u32, Vec<usize>> = HashMap::new();
    for (k, &(a, _)) in edges.iter().enumerate() {
        outgoing.entry(a).or_default().push(k);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = Vec::new();
    for first in 0..edges.len() {
        if used.get(first).copied().unwrap_or(true) {
            continue;
        }
        let start = edges.get(first)?.0;
        let mut cycle = Vec::new();
        let mut current = first;
        for _ in 0..=edges.len() {
            *used.get_mut(current)? = true;
            let (a, b) = *edges.get(current)?;
            cycle.push(a);
            let (pa, pb) = (point(a), point(b));
            let reverse = pa - pb;
            let turn = |k: usize| -> f64 {
                let c = edges.get(k).map_or(b, |e| e.1);
                let forward = point(c) - pb;
                let mut angle = reverse.y.atan2(reverse.x) - forward.y.atan2(forward.x);
                if angle <= 0.0 {
                    angle += TAU;
                }
                angle
            };
            let next = outgoing
                .get(&b)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&k| !used.get(k).copied().unwrap_or(true) || (k == first && b == start))
                .min_by(|&x, &y| turn(x).total_cmp(&turn(y)))?;
            if next == first {
                loops.push(cycle);
                break;
            }
            current = next;
        }
    }
    Some(loops)
}

/// A crossing of a face loop with the cut line.
#[derive(Debug, Clone, Copy)]
struct Event {
    t: f64,
    exit: bool,
    vertex: u32,
    run: usize,
}

struct Clipper<'a> {
    vertices: Vec<Point3>,
    side: &'a [i8],
    distance: &'a [f64],
    crossings: HashMap<(u32, u32), u32>,
    plane: Plane3,
    keep_normal: Vector3,
    on_is_in: bool,
    tolerance: &'a Tolerance,
}

impl Clipper<'_> {
    fn side_of(&self, vertex: u32) -> i8 {
        usize::try_from(vertex)
            .ok()
            .and_then(|i| self.side.get(i))
            .copied()
            .unwrap_or(0)
    }

    /// Whether the vertex is kept: strictly on the kept side, or *on* the plane when `on_is_in`.
    fn inside(&self, vertex: u32) -> bool {
        let s = self.side_of(vertex);
        s > 0 || (s == 0 && self.on_is_in)
    }

    fn position(&self, vertex: u32) -> Point3 {
        usize::try_from(vertex)
            .ok()
            .and_then(|i| self.vertices.get(i))
            .copied()
            .unwrap_or_else(Point3::origin)
    }

    /// The vertex where the edge `a`–`b` meets the plane: an end that is on the plane, or a new
    /// vertex on the plane, created once per undirected edge and computed in index order so that
    /// both faces of the edge see identical coordinates.
    fn crossing(&mut self, a: u32, b: u32) -> Result<u32> {
        if self.side_of(a) == 0 {
            return Ok(a);
        }
        if self.side_of(b) == 0 {
            return Ok(b);
        }
        let (low, high) = (a.min(b), a.max(b));
        if let Some(&id) = self.crossings.get(&(low, high)) {
            return Ok(id);
        }
        let dist = |v: u32| {
            usize::try_from(v)
                .ok()
                .and_then(|i| self.distance.get(i))
                .copied()
                .unwrap_or(0.0)
        };
        let (d_low, d_high) = (dist(low), dist(high));
        let denominator = d_low - d_high;
        if denominator == 0.0 || !denominator.is_finite() {
            return Err(degenerate());
        }
        let lambda = d_low / denominator;
        let (p_low, p_high) = (self.position(low), self.position(high));
        let point = self.plane.project(&(p_low + (p_high - p_low) * lambda));
        let id = u32::try_from(self.vertices.len())
            .map_err(|_| Error::new(codes::SOLID_LIMIT_EXCEEDED))?;
        self.vertices.push(point);
        self.crossings.insert((low, high), id);
        Ok(id)
    }

    /// Clips one face; see the module documentation.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the loops of a face, in the order the method describes"
    )]
    fn clip_face(&mut self, face: &PolyFace, normal: &Vector3) -> Result<Vec<PolyFace>> {
        let all: Vec<u32> = face.loops().flatten().copied().collect();
        if all.iter().all(|&v| self.side_of(v) == 0) {
            let toward_kept = normal.dot(&self.keep_normal) > 0.0;
            return Ok(if self.on_is_in && !toward_kept {
                vec![face.clone()]
            } else {
                Vec::new()
            });
        }
        if all.iter().all(|&v| self.inside(v)) {
            return Ok(vec![face.clone()]);
        }
        if all.iter().all(|&v| !self.inside(v)) {
            return Ok(Vec::new());
        }
        let cut_direction = self.keep_normal.cross(normal);
        if cut_direction.norm() <= self.tolerance.angular {
            return Err(degenerate());
        }
        let direction = cut_direction.normalize();

        let mut runs: Vec<Vec<u32>> = Vec::new();
        let mut events: Vec<Event> = Vec::new();
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for loop_ in face.loops() {
            let m = loop_.len();
            let at = |k: usize| loop_.get(k % m).copied().unwrap_or(0);
            if loop_.iter().all(|&v| self.inside(v)) {
                edges.extend((0..m).map(|k| (at(k), at(k + 1))));
                continue;
            }
            let Some(start) = (0..m).find(|&k| !self.inside(at(k)) && self.inside(at(k + 1)))
            else {
                continue; // entirely out
            };
            let mut current: Option<Vec<u32>> = None;
            let push = |run: &mut Option<Vec<u32>>, v: u32| {
                if let Some(run) = run
                    && run.last() != Some(&v)
                {
                    run.push(v);
                }
            };
            for step in 0..m {
                let (a, b) = (at(start + step), at(start + step + 1));
                match (self.inside(a), self.inside(b)) {
                    (false, true) => {
                        let x = self.crossing(a, b)?;
                        events.push(Event {
                            t: 0.0,
                            exit: false,
                            vertex: x,
                            run: runs.len(),
                        });
                        current = Some(vec![x]);
                        push(&mut current, b);
                    }
                    (true, true) => push(&mut current, b),
                    (true, false) => {
                        let x = self.crossing(a, b)?;
                        push(&mut current, x);
                        events.push(Event {
                            t: 0.0,
                            exit: true,
                            vertex: x,
                            run: runs.len(),
                        });
                        runs.extend(current.take());
                    }
                    (false, false) => {}
                }
            }
        }
        for event in &mut events {
            event.t = direction.dot(&self.position(event.vertex).coords);
        }
        events.sort_by(|a, b| a.t.total_cmp(&b.t));
        // At a shared vertex the order of an entry and an exit is decided by the alternation
        // (exit, entry, exit, ...) that a simple region forces along the cut line.
        for k in 0..events.len().saturating_sub(1) {
            let (first, second) = (events[k], events[k + 1]);
            if first.exit != (k % 2 == 0)
                && first.vertex == second.vertex
                && second.exit == (k % 2 == 0)
            {
                events.swap(k, k + 1);
            }
        }
        if !events.len().is_multiple_of(2)
            || events
                .iter()
                .enumerate()
                .any(|(k, e)| e.exit != (k % 2 == 0))
        {
            return Err(degenerate());
        }
        // The vertices of the face on the cut line split the joining edges, so that an edge of
        // the face lying on the line meets its reverse partner exactly.
        let mut on_line: Vec<(f64, u32)> = all
            .iter()
            .filter(|&&v| self.side_of(v) == 0)
            .map(|&v| (direction.dot(&self.position(v).coords), v))
            .collect();
        on_line.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut next_run = vec![usize::MAX; runs.len()];
        for pair in events.chunks(2) {
            if let [exit, entry] = pair {
                if let Some(slot) = next_run.get_mut(exit.run) {
                    *slot = entry.run;
                }
                let mut chain = vec![exit.vertex];
                chain.extend(
                    on_line
                        .iter()
                        .filter(|(t, v)| {
                            *t > exit.t && *t < entry.t && *v != exit.vertex && *v != entry.vertex
                        })
                        .map(|&(_, v)| v),
                );
                chain.push(entry.vertex);
                edges.extend(chain.windows(2).map(|w| (w[0], w[1])));
            }
        }
        for run in &runs {
            edges.extend(run.windows(2).map(|w| (w[0], w[1])));
        }
        edges.retain(|(a, b)| a != b);
        let edges = cancel_opposite(&edges);
        let frame = projection(normal);
        let cycles = link_loops(&edges, &|v| project_point(&self.position(v), frame))
            .ok_or_else(degenerate)?;
        let mut outers: Vec<Vec<u32>> = Vec::new();
        let mut holes: Vec<Vec<u32>> = Vec::new();
        for cycle in cycles {
            match self.orientation(&cycle, normal) {
                Some(true) => outers.push(cycle),
                Some(false) => holes.push(cycle),
                None => {}
            }
        }
        self.assemble(outers, holes, normal, face.tag)
    }

    /// Whether the loop runs counterclockwise about `normal` (`Some(true)`), clockwise
    /// (`Some(false)`), or has no area within the tolerance (`None`).
    fn orientation(&self, cycle: &[u32], normal: &Vector3) -> Option<bool> {
        if cycle.len() < 3 {
            return None;
        }
        let area = newell(&self.vertices, cycle);
        let longest = cycle
            .iter()
            .enumerate()
            .map(|(k, &a)| {
                let b = cycle.get((k + 1) % cycle.len()).copied().unwrap_or(a);
                (self.position(b) - self.position(a)).norm()
            })
            .fold(0.0, f64::max);
        let projected = area.dot(normal);
        if projected.abs() < self.tolerance.linear * longest || projected == 0.0 {
            None
        } else {
            Some(projected > 0.0)
        }
    }

    /// Faces from outer loops, assigning each hole to the outer loop containing it.
    fn assemble(
        &self,
        outers: Vec<Vec<u32>>,
        holes: Vec<Vec<u32>>,
        normal: &Vector3,
        tag: FaceTag,
    ) -> Result<Vec<PolyFace>> {
        let axis = (0..3)
            .max_by(|&a, &b| {
                let at = |k: usize| normal.get(k).copied().unwrap_or(0.0).abs();
                at(a).total_cmp(&at(b))
            })
            .unwrap_or(2);
        let mut faces: Vec<PolyFace> = outers
            .into_iter()
            .map(|o| PolyFace::new(o).with_tag(tag))
            .collect();
        for hole in holes {
            let probe = self.position(hole.first().copied().unwrap_or(0));
            let owner = faces.iter_mut().find(|face| {
                let ring: Vec<Point3> = face.outer.iter().map(|&v| self.position(v)).collect();
                inside_projected(&probe, &ring, axis)
            });
            match owner {
                Some(face) => face.holes.push(hole),
                None => return Err(degenerate()),
            }
        }
        Ok(faces)
    }
}

impl Polyhedron {
    /// Clips the solid by `plane`, keeping `keep`, as the [module documentation](self)
    /// describes; the new cap faces are tagged [`FaceTag::Cap`] with `cut`. `Ok(None)` means
    /// nothing of volume remains on the kept side.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid`; `geom.solid.clip_degenerate` for the cases the module
    /// documentation lists; `geom.solid.limit_exceeded` when the result exceeds `limits`.
    pub fn clip(
        &self,
        plane: &Plane3,
        keep: ClipSide,
        cut: u32,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Option<Self>> {
        Ok(self
            .clip_with_cap(plane, keep, cut, tolerance, limits)?
            .map(|(solid, _)| solid))
    }

    /// The section of the solid by `plane` as a region in the frame of the plane
    /// ([`Plane3::frame`]): the cut faces of [`Self::clip`] keeping the positive side. `None`
    /// when the plane does not cut through the solid's volume, including when it only touches
    /// it or lies in one of its faces. A cut through a hole of a face gives a region with a
    /// hole; a cut through several walls gives several parts.
    ///
    /// # Errors
    ///
    /// As [`Self::clip`], and the errors of region construction when a cap loop cannot be
    /// closed within `tolerance.linear`.
    pub fn section(
        &self,
        plane: &Plane3,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Option<Region>> {
        let Some((solid, first_cap)) =
            self.clip_with_cap(plane, ClipSide::Positive, 0, tolerance, limits)?
        else {
            return Ok(None);
        };
        let frame = plane.frame(tolerance);
        let to_loop = |ids: &[u32]| -> Result<Loop2> {
            let points: Vec<_> = ids
                .iter()
                .filter_map(|&i| {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| solid.vertices().get(i))
                })
                .map(|p| frame.to_plane(p))
                .collect();
            let mut curves = Vec::with_capacity(points.len());
            for (k, a) in points.iter().enumerate() {
                let b = points.get((k + 1) % points.len()).copied().unwrap_or(*a);
                curves.push(Curve2::Segment(Segment2::new(*a, b)?));
            }
            Loop2::new(curves, tolerance)
        };
        let mut parts = Vec::new();
        for face in solid.faces().iter().skip(first_cap) {
            let holes = face
                .holes
                .iter()
                .map(|h| to_loop(h))
                .collect::<Result<Vec<_>>>()?;
            parts.push(RegionPart::new(to_loop(&face.outer)?, holes));
        }
        if parts.is_empty() {
            Ok(None)
        } else {
            Region::new(parts).map(Some)
        }
    }

    /// The clipped solid and the index of its first cap face.
    #[expect(
        clippy::too_many_lines,
        reason = "classify, clip faces, close the cap and validate, in the order the method describes"
    )]
    fn clip_with_cap(
        &self,
        plane: &Plane3,
        keep: ClipSide,
        cut: u32,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Option<(Self, usize)>> {
        crate::ensure_tolerance(tolerance)?;
        let sign = match keep {
            ClipSide::Positive => 1.0,
            ClipSide::Negative => -1.0,
        };
        let distance: Vec<f64> = self
            .vertices()
            .iter()
            .map(|p| sign * plane.signed_distance(p))
            .collect();
        let side: Vec<i8> = distance
            .iter()
            .map(|&d| {
                if d > tolerance.linear {
                    1
                } else if d < -tolerance.linear {
                    -1
                } else {
                    0
                }
            })
            .collect();
        if !side.contains(&1) {
            return Ok(None);
        }
        if !side.contains(&-1) {
            return Ok(Some((self.clone(), self.faces().len())));
        }
        let mut clipper = Clipper {
            vertices: self.vertices().to_vec(),
            side: &side,
            distance: &distance,
            crossings: HashMap::new(),
            plane: *plane,
            keep_normal: plane.normal() * sign,
            on_is_in: keep == ClipSide::Positive,
            tolerance,
        };
        let mut faces: Vec<PolyFace> = Vec::new();
        for (index, face) in self.faces().iter().enumerate() {
            let normal = self.face_normal(index).ok_or_else(degenerate)?;
            faces.extend(clipper.clip_face(face, &normal)?);
        }
        if faces.is_empty() {
            return Ok(None);
        }
        let first_cap = faces.len();
        // The edges without a reverse partner lie in the cutting plane; their reverses are the
        // loops of the cap.
        let edges: Vec<(u32, u32)> = faces
            .iter()
            .flat_map(PolyFace::loops)
            .flat_map(|l| {
                l.iter()
                    .enumerate()
                    .map(|(k, &a)| (a, l.get((k + 1) % l.len()).copied().unwrap_or(a)))
            })
            .collect();
        let present: HashSet<(u32, u32)> = edges.iter().copied().collect();
        let cap_edges: Vec<(u32, u32)> = edges
            .iter()
            .filter(|&&(a, b)| !present.contains(&(b, a)))
            .map(|&(a, b)| (b, a))
            .collect();
        let cap_normal = -clipper.keep_normal;
        let cap_frame = projection(&cap_normal);
        let cycles = link_loops(&cap_edges, &|v| {
            project_point(&clipper.position(v), cap_frame)
        })
        .ok_or_else(degenerate)?;
        let mut outers = Vec::new();
        let mut holes = Vec::new();
        for cycle in cycles {
            match clipper.orientation(&cycle, &cap_normal) {
                Some(true) => outers.push(cycle),
                Some(false) => holes.push(cycle),
                None => return Err(degenerate()),
            }
        }
        faces.extend(clipper.assemble(outers, holes, &cap_normal, FaceTag::Cap { cut })?);

        // Keep the vertices in use, in order.
        let mut remap: HashMap<u32, u32> = HashMap::new();
        let mut vertices = Vec::new();
        let mut used: Vec<u32> = faces
            .iter()
            .flat_map(PolyFace::loops)
            .flatten()
            .copied()
            .collect();
        used.sort_unstable();
        used.dedup();
        for id in used {
            let point = clipper.position(id);
            remap.insert(
                id,
                u32::try_from(vertices.len())
                    .map_err(|_| Error::new(codes::SOLID_LIMIT_EXCEEDED))?,
            );
            vertices.push(point);
        }
        let map = |l: &Vec<u32>| {
            l.iter()
                .filter_map(|v| remap.get(v).copied())
                .collect::<Vec<u32>>()
        };
        let faces = faces
            .iter()
            .map(|f| PolyFace {
                outer: map(&f.outer),
                holes: f.holes.iter().map(map).collect(),
                tag: f.tag,
            })
            .collect();
        match Self::new(vertices, faces, tolerance, limits) {
            Ok(solid) => Ok(Some((solid, first_cap))),
            Err(error) if error.code() == codes::SOLID_VOLUME.code => Ok(None),
            Err(error) if error.code() == codes::SOLID_LIMIT_EXCEEDED.code => Err(error),
            Err(error) => Err(degenerate().with_source(error)),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::plane3::PlaneFrame;
    use crate::solid::tests::{TOL, cuboid, p, watertight};
    use crate::solid_gen::tests::{l_shape, region};
    use crate::solid_gen::{extrude, prism};
    use proptest::prelude::*;

    const LIMITS: Limits = Limits::DEFAULT;

    fn plane(origin: Point3, normal: Vector3) -> Plane3 {
        Plane3::new(origin, normal).expect("plane")
    }

    fn both(solid: &Polyhedron, plane: &Plane3) -> (Option<Polyhedron>, Option<Polyhedron>) {
        (
            solid
                .clip(plane, ClipSide::Positive, 0, &TOL, &LIMITS)
                .expect("positive"),
            solid
                .clip(plane, ClipSide::Negative, 0, &TOL, &LIMITS)
                .expect("negative"),
        )
    }

    fn volume(solid: &Option<Polyhedron>) -> f64 {
        solid.as_ref().map_or(0.0, Polyhedron::volume)
    }

    #[test]
    fn a_plane_through_the_middle_halves_a_cube_and_the_section_is_the_square() {
        let cube = cuboid(1.0, 1.0, 1.0);
        let cut = plane(p(0.0, 0.0, 0.25), Vector3::z());
        let (above, below) = both(&cube, &cut);
        assert!((volume(&above) - 0.75).abs() < 1e-12);
        assert!((volume(&below) - 0.25).abs() < 1e-12);
        let above = above.expect("above");
        assert_eq!(above.faces().len(), 6);
        let section = cube
            .section(&cut, &TOL, &LIMITS)
            .expect("section")
            .expect("some");
        let area: f64 = section
            .parts()
            .iter()
            .map(|part| part.outer().signed_area().abs())
            .sum();
        assert!((area - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_cut_through_vertices_and_edges() {
        let cube = cuboid(1.0, 1.0, 1.0);
        // The diagonal plane x = y contains four vertices and two edges.
        let diagonal = plane(p(0.0, 0.0, 0.0), Vector3::new(1.0, -1.0, 0.0));
        let (a, b) = both(&cube, &diagonal);
        assert!((volume(&a) - 0.5).abs() < 1e-12);
        assert!((volume(&b) - 0.5).abs() < 1e-12);
        let section = cube
            .section(&diagonal, &TOL, &LIMITS)
            .expect("section")
            .expect("some");
        let area: f64 = section
            .parts()
            .iter()
            .map(|part| part.outer().signed_area().abs())
            .sum();
        assert!((area - 2.0_f64.sqrt()).abs() < 1e-12);
        // A corner cut through three vertices.
        let corner = plane(p(1.0, 0.0, 0.0), Vector3::new(1.0, 1.0, 1.0));
        let (a, b) = both(&cube, &corner);
        assert!((volume(&a) - 5.0 / 6.0).abs() < 1e-12);
        assert!((volume(&b) - 1.0 / 6.0).abs() < 1e-12);
    }

    #[test]
    fn planes_along_faces_and_touching_give_nothing_or_the_whole_solid() {
        let cube = cuboid(1.0, 1.0, 1.0);
        for z in [0.0, 1.0] {
            let along = plane(p(0.0, 0.0, z), Vector3::z());
            let (above, below) = both(&cube, &along);
            let (whole, none) = if z == 0.0 {
                (above, below)
            } else {
                (below, above)
            };
            assert_eq!(whole.as_ref(), Some(&cube));
            assert!(none.is_none());
            assert!(
                cube.section(&along, &TOL, &LIMITS)
                    .expect("section")
                    .is_none()
            );
        }
        let vertex = plane(p(0.0, 0.0, 0.0), Vector3::new(-1.0, -1.0, -1.0));
        let (a, b) = both(&cube, &vertex);
        assert!(a.is_none() && b.as_ref() == Some(&cube));
        let edge = plane(p(0.0, 0.0, 0.0), Vector3::new(-1.0, -1.0, 0.0));
        let (a, b) = both(&cube, &edge);
        assert!(a.is_none() && b.as_ref() == Some(&cube));
        let far = plane(p(0.0, 0.0, 9.0), Vector3::z());
        assert!(
            cube.clip(&far, ClipSide::Positive, 0, &TOL, &LIMITS)
                .expect("far")
                .is_none()
        );
    }

    #[test]
    fn a_plane_in_the_step_of_an_l_shaped_solid() {
        // The L outline stood up in the x-z plane and extruded 2 deep: a step top at z = 1.
        let frame =
            PlaneFrame::new(p(0.0, 0.0, 0.0), Vector3::x(), Vector3::z(), &TOL).expect("frame");
        let solid = extrude(&l_shape(), &frame, 2.0, 1e-3, &TOL, &LIMITS).expect("solid");
        assert!((solid.volume() - 12.0).abs() < 1e-12);
        let step = plane(p(0.0, 0.0, 1.0), Vector3::z());
        let (above, below) = both(&solid, &step);
        assert!((volume(&above) - 4.0).abs() < 1e-12, "block above the step");
        assert!((volume(&below) - 8.0).abs() < 1e-12, "slab below the step");
        let section = solid
            .section(&step, &TOL, &LIMITS)
            .expect("section")
            .expect("some");
        let area: f64 = section
            .parts()
            .iter()
            .map(|part| part.outer().signed_area().abs())
            .sum();
        assert!((area - 2.0).abs() < 1e-12, "the block footprint");
    }

    #[test]
    fn a_cut_through_a_hole_gives_a_section_with_a_hole_or_a_notch() {
        let r = region(
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            &[&[(2.0, 2.0), (2.0, 4.0), (4.0, 4.0), (4.0, 2.0)]],
        );
        let solid = prism(&r, 0.0, 2.0, 1e-3, &TOL, &LIMITS).expect("prism");
        // Horizontal: an annulus.
        let level = plane(p(0.0, 0.0, 1.0), Vector3::z());
        let section = solid
            .section(&level, &TOL, &LIMITS)
            .expect("section")
            .expect("some");
        assert_eq!(section.parts().len(), 1);
        assert_eq!(section.parts()[0].holes().len(), 1);
        let (a, b) = both(&solid, &level);
        assert!((volume(&a) - 96.0).abs() < 1e-9 && (volume(&b) - 96.0).abs() < 1e-9);
        // Vertical through the hole: the section is two blocks, volumes still add up.
        let through = plane(p(3.0, 0.0, 0.0), Vector3::x());
        let (a, b) = both(&solid, &through);
        assert!((volume(&a) + volume(&b) - 192.0).abs() < 1e-9);
        let section = solid
            .section(&through, &TOL, &LIMITS)
            .expect("section")
            .expect("some");
        assert_eq!(section.parts().len(), 2, "above and below the hole");
        // Through the hole's edge plane x = 2 (a face of the hole lies in the plane).
        let along = plane(p(2.0, 0.0, 0.0), Vector3::x());
        let (a, b) = both(&solid, &along);
        assert!((volume(&a) + volume(&b) - 192.0).abs() < 1e-9);
    }

    #[test]
    fn a_cut_through_the_apex_where_two_legs_meet_is_refused_on_the_leg_side() {
        // A rectangle with a triangular notch from below whose apex is at y = 3: the plane
        // y = 3 leaves two legs that meet along the apex edge.
        let notched = region(
            &[
                (0.0, 0.0),
                (1.0, 0.0),
                (2.0, 3.0),
                (3.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
            ],
            &[],
        );
        let solid = prism(&notched, 0.0, 1.0, 1e-3, &TOL, &LIMITS).expect("prism");
        let at_apex = plane(p(0.0, 3.0, 0.0), Vector3::y());
        let top = solid
            .clip(&at_apex, ClipSide::Positive, 0, &TOL, &LIMITS)
            .expect("top");
        assert!((volume(&top) - 4.0).abs() < 1e-12);
        // The legs would share the apex edge inside the cap: refused, see the module table.
        assert_eq!(
            solid
                .clip(&at_apex, ClipSide::Negative, 0, &TOL, &LIMITS)
                .err()
                .map(|e| e.code()),
            Some("geom.solid.clip_degenerate")
        );
        let below = plane(p(0.0, 2.9, 0.0), Vector3::y());
        let (a, b) = both(&solid, &below);
        assert!((volume(&a) + volume(&b) - solid.volume()).abs() < 1e-9);
    }

    #[test]
    fn planes_within_the_tolerance_of_a_face_are_exact_cuts() {
        let cube = cuboid(1.0, 1.0, 1.0);
        let tiny_tilt = plane(p(0.0, 0.0, 0.5), Vector3::new(1e-12, 0.0, 1.0));
        assert!(
            cube.clip(&tiny_tilt, ClipSide::Positive, 0, &TOL, &LIMITS)
                .expect("clip")
                .is_some()
        );
        // The top vertices are within the tolerance of the plane: they count as on it.
        let near_top = plane(p(0.0, 0.0, 1.0 - 4e-7), Vector3::z());
        let (a, b) = both(&cube, &near_top);
        assert!(a.is_none());
        assert_eq!(b.as_ref(), Some(&cube));
        assert_eq!(
            cube.clip(
                &tiny_tilt,
                ClipSide::Positive,
                0,
                &Tolerance {
                    linear: f64::NAN,
                    ..TOL
                },
                &LIMITS
            )
            .err()
            .map(|e| e.code()),
            Some("geom.tolerance.invalid")
        );
    }

    fn convex_notched(w: f64, d: f64, h: f64) -> Polyhedron {
        let r = region(
            &[
                (0.0, 0.0),
                (w, 0.0),
                (w, d),
                (0.6 * w, d),
                (0.6 * w, 0.5 * d),
                (0.4 * w, 0.5 * d),
                (0.4 * w, d),
                (0.0, d),
            ],
            &[&[
                (0.1 * w, 0.1 * d),
                (0.1 * w, 0.3 * d),
                (0.3 * w, 0.3 * d),
                (0.3 * w, 0.1 * d),
            ]],
        );
        prism(&r, 0.0, h, 1e-3, &TOL, &LIMITS).expect("prism")
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]

        #[test]
        fn clipping_splits_the_volume_and_every_part_validates(
            w in 2.0..12.0_f64, d in 2.0..12.0_f64, h in 1.0..6.0_f64,
            nx in -1.0..1.0_f64, ny in -1.0..1.0_f64, nz in -1.0..1.0_f64,
            fx in 0.05..0.95_f64, fy in 0.05..0.95_f64, fz in 0.05..0.95_f64,
        ) {
            prop_assume!(nx * nx + ny * ny + nz * nz > 0.1);
            let solid = convex_notched(w, d, h);
            let cut = plane(p(fx * w, fy * d, fz * h), Vector3::new(nx, ny, nz));
            let (a, b) = both(&solid, &cut);
            prop_assert!((volume(&a) + volume(&b) - solid.volume()).abs() < 1e-8 * solid.volume());
            // Parts are already validated by construction; rebuild one to be sure.
            for part in [&a, &b].into_iter().flatten() {
                prop_assert!(Polyhedron::new(part.vertices().to_vec(), part.faces().to_vec(), &TOL, &LIMITS).is_ok());
            }
        }

        #[test]
        fn axis_planes_through_vertices_and_hole_edges_keep_the_volume(
            w in 2.0..12.0_f64, d in 2.0..12.0_f64, h in 1.0..6.0_f64,
            axis in 0usize..3, pick in 0usize..5,
        ) {
            let solid = convex_notched(w, d, h);
            // Planes through the coordinates of outline vertices, hole edges and the caps.
            let value = match axis {
                0 => [0.0, 0.1 * w, 0.3 * w, 0.4 * w, 0.6 * w][pick],
                1 => [0.0, 0.1 * d, 0.3 * d, 0.5 * d, d][pick],
                _ => [0.0, h, 0.5 * h, h, 0.0][pick],
            };
            let mut origin = [0.0; 3];
            let mut normal = [0.0; 3];
            origin[axis] = value;
            normal[axis] = 1.0;
            let cut = plane(p(origin[0], origin[1], origin[2]), Vector3::new(normal[0], normal[1], normal[2]));
            let (a, b) = both(&solid, &cut);
            prop_assert!((volume(&a) + volume(&b) - solid.volume()).abs() < 1e-8 * solid.volume());
            for part in [&a, &b].into_iter().flatten() {
                prop_assert!(watertight(part));
            }
        }
    }

    // ---- edge-in-plane cuts of a wall with a door and a window.

    /// The elevation of a 5 x 3 wall with a door (x 1..1.9, z 0..2.1) and a window (x 3..4.2,
    /// z 0.9..2.4) as one region, extruded 0.2 thick toward -y.
    pub(crate) fn elevation_wall() -> Polyhedron {
        let elevation = region(
            &[
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 2.1),
                (1.9, 2.1),
                (1.9, 0.0),
                (5.0, 0.0),
                (5.0, 3.0),
                (0.0, 3.0),
            ],
            &[&[(3.0, 0.9), (3.0, 2.4), (4.2, 2.4), (4.2, 0.9)]],
        );
        let frame =
            PlaneFrame::new(p(0.0, 0.0, 0.0), Vector3::x(), Vector3::z(), &TOL).expect("frame");
        extrude(&elevation, &frame, -0.2, 1e-3, &TOL, &LIMITS).expect("wall")
    }

    fn assert_cut(solid: &Polyhedron, cut: &Plane3, positive: f64, negative: f64) {
        let (a, b) = both(solid, cut);
        assert!(
            (volume(&a) - positive).abs() < 1e-6,
            "positive {} vs {positive}",
            volume(&a)
        );
        assert!(
            (volume(&b) - negative).abs() < 1e-6,
            "negative {} vs {negative}",
            volume(&b)
        );
        for part in [&a, &b].into_iter().flatten() {
            assert!(watertight(part));
            assert!(
                Polyhedron::new(
                    part.vertices().to_vec(),
                    part.faces().to_vec(),
                    &TOL,
                    &LIMITS
                )
                .is_ok()
            );
        }
    }

    fn area(region: &Region) -> f64 {
        region
            .parts()
            .iter()
            .map(|part| {
                part.outer().signed_area().abs()
                    - part
                        .holes()
                        .iter()
                        .map(|h| h.signed_area().abs())
                        .sum::<f64>()
            })
            .sum()
    }

    #[test]
    fn cuts_through_the_head_sill_and_jambs_of_openings() {
        let wall = elevation_wall();
        let exact = (15.0 - 0.9 * 2.1 - 1.2 * 1.5) * 0.2;
        assert!((wall.volume() - exact).abs() < 1e-12);
        let level = |z: f64| plane(p(0.0, 0.0, z), Vector3::z());
        // Volumes below z: the wall slab minus the door and window parts below z.
        let below = |z: f64| {
            let door = 0.9 * z.min(2.1);
            let window = 1.2 * (z.clamp(0.9, 2.4) - 0.9);
            (5.0 * z - door - window) * 0.2
        };
        for z in [
            0.9,
            1.2,
            2.1,
            2.25,
            2.4,
            0.9 + 0.6e-6,
            0.9 - 0.7e-6,
            2.1 + 0.9e-6,
        ] {
            assert_cut(&wall, &level(z), exact - below(z), below(z));
        }
        // Sections: the wall just above the plane.
        let section = |z: f64| {
            area(
                &wall
                    .section(&level(z), &TOL, &LIMITS)
                    .expect("section")
                    .expect("some"),
            )
        };
        // The section is where the material continues through the plane.
        assert!(
            (section(2.1) - 2.9 * 0.2).abs() < 1e-9,
            "head: no door, no window"
        );
        assert!(
            (section(0.9) - 2.9 * 0.2).abs() < 1e-9,
            "sill: no door, no window"
        );
        assert!(
            (section(2.4) - 3.8 * 0.2).abs() < 1e-9,
            "window head: no window"
        );
        assert!(
            (section(1.2) - 2.9 * 0.2).abs() < 1e-9,
            "between: no door, no window"
        );
        assert_eq!(
            wall.section(&level(2.1), &TOL, &LIMITS)
                .expect("s")
                .expect("some")
                .parts()
                .len(),
            3
        );
        // Jambs: x = 1 and x = 1.9 (vertical planes through the door's edges), x = 3.
        for x in [1.0, 1.9, 3.0, 4.2] {
            let cut = plane(p(x, 0.0, 0.0), Vector3::x());
            let left = |x: f64| {
                let door = if x > 1.0 {
                    (x.min(1.9) - 1.0) * 2.1
                } else {
                    0.0
                };
                let window = if x > 3.0 {
                    (x.min(4.2) - 3.0) * 1.5
                } else {
                    0.0
                };
                (3.0 * x - door - window) * 0.2
            };
            assert_cut(&wall, &cut, exact - left(x), left(x));
        }
    }

    #[test]
    fn faces_of_clipped_walls_keep_their_tags() {
        use crate::solid::FaceTag;
        let wall = elevation_wall();
        let top = wall
            .clip(
                &plane(p(0.0, 0.0, 2.1), Vector3::z()),
                ClipSide::Negative,
                0,
                &TOL,
                &LIMITS,
            )
            .expect("clip")
            .expect("some");
        let caps = top
            .faces()
            .iter()
            .filter(|f| matches!(f.tag, FaceTag::Cap { .. }))
            .count();
        assert_eq!(
            caps, 3,
            "the cut pieces left of the door, between door and window, right of the window"
        );
        assert!(
            top.faces()
                .iter()
                .any(|f| matches!(f.tag, FaceTag::Side { .. }))
        );
        assert!(
            top.faces()
                .iter()
                .any(|f| f.tag == FaceTag::Bottom || f.tag == FaceTag::Top)
        );
        assert!(top.faces().iter().all(|f| f.tag != FaceTag::Untagged));
    }

    #[test]
    fn cuts_within_a_tolerance_of_a_vertex_agree_on_both_sides() {
        let wall = elevation_wall();
        for offset in [0.4e-6, 0.6e-6, 0.9e-6, 1.1e-6, 2e-6, -0.7e-6] {
            let z = 3.0 - offset;
            let (a, b) = both(&wall, &plane(p(0.0, 0.0, z), Vector3::z()));
            assert!(
                (volume(&a) + volume(&b) - wall.volume()).abs() < 1e-5,
                "offset {offset}"
            );
            for part in [&a, &b].into_iter().flatten() {
                assert!(watertight(part));
            }
        }
        // Within the tolerance of the top: nothing above, the whole wall below.
        let (a, b) = both(&wall, &plane(p(0.0, 0.0, 3.0 - 0.7e-6), Vector3::z()));
        assert!(a.is_none());
        assert_eq!(b.as_ref(), Some(&wall));
    }

    #[test]
    fn a_wall_cut_far_from_the_origin_and_slanted() {
        let angle = 0.5235987755982988_f64;
        let far = p(512_345.678, 4_423_456.789, 1_234.5);
        let m = arc_base::math::Matrix4::new_translation(&far.coords)
            * arc_base::math::Matrix4::new_rotation(Vector3::z() * angle);
        let wall = elevation_wall()
            .transformed(
                &arc_base::math::Transform3::from_matrix_unchecked(m),
                &TOL,
                &LIMITS,
            )
            .expect("far wall");
        for z in [0.9, 2.1, 2.4, 1.2] {
            let cut = plane(p(far.x, far.y, far.z + z), Vector3::z());
            let (a, b) = both(&wall, &cut);
            assert!(
                (volume(&a) + volume(&b) - wall.volume()).abs() < 1e-6,
                "z {z}"
            );
            for part in [&a, &b].into_iter().flatten() {
                assert!(watertight(part), "z {z}");
            }
        }
    }

    /// The footprint of a V-shaped notch: a 2 x 2 square with the triangle (0,0), (1,1), (2,0)
    /// removed from its bottom edge, extruded 1 high.
    fn v_notch() -> Polyhedron {
        let footprint = region(
            &[(0.0, 0.0), (1.0, 1.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)],
            &[],
        );
        prism(&footprint, 0.0, 1.0, 1e-3, &TOL, &LIMITS).expect("notch")
    }

    fn assert_part(solid: &Option<Polyhedron>, volume: f64, shells: usize) {
        let solid = solid.as_ref().expect("a part");
        assert!(
            (solid.volume() - volume).abs() < 1e-12,
            "volume {} instead of {volume}",
            solid.volume()
        );
        assert_eq!(solid.shell_count(), shells);
        assert!(watertight(solid), "every mesh edge is used exactly twice");
    }

    #[test]
    fn a_cut_through_the_apex_of_a_v_notch_is_refused_and_the_other_side_is_usable() {
        let notch = v_notch();
        let apex = plane(p(0.0, 1.0, 0.0), Vector3::y());
        // The prongs would meet along the apex edge, which lies inside the cap: refused at the
        // first cut instead of returning a solid that cannot be cut again.
        assert_eq!(
            notch
                .clip(&apex, ClipSide::Negative, 0, &TOL, &LIMITS)
                .err()
                .map(|e| e.code()),
            Some("geom.solid.clip_degenerate")
        );
        // The other side is the block above the notch, and it can be cut again.
        let above = notch
            .clip(&apex, ClipSide::Positive, 0, &TOL, &LIMITS)
            .expect("above");
        assert_part(&above, 2.0, 1);
        let above = above.expect("some");
        for (origin, normal) in [
            (p(0.0, 0.0, 0.5), Vector3::z()),
            (p(1.0, 0.0, 0.0), Vector3::x()),
            (p(0.5, 0.0, 0.0), Vector3::x()),
        ] {
            let second = plane(origin, normal);
            let (a, b) = both(&above, &second);
            let half = if normal == Vector3::x() && origin.x == 0.5 {
                (1.5, 0.5)
            } else {
                (1.0, 1.0)
            };
            assert_part(&a, half.0, 1);
            assert_part(&b, half.1, 1);
        }
        // Just below the apex the prongs are two separate shells, each a valid solid.
        let below = notch
            .clip(
                &plane(p(0.0, 0.5, 0.0), Vector3::y()),
                ClipSide::Negative,
                0,
                &TOL,
                &LIMITS,
            )
            .expect("below");
        assert_part(&below, 0.25, 2);
        let prongs = below.expect("some");
        let (a, b) = both(&prongs, &plane(p(0.25, 0.0, 0.0), Vector3::x()));
        // The right prong (0.125) and the corner of the left one (0.03125) lie at x >= 0.25.
        assert_part(&a, 0.125 + 0.03125, 2);
        assert_part(&b, 0.125 - 0.03125, 1);
    }

    #[test]
    fn a_cap_carries_the_number_of_the_cut_that_made_it() {
        let wall = prism(
            &region(&[(0.0, 0.0), (5.0, 0.0), (5.0, 0.2), (0.0, 0.2)], &[]),
            0.0,
            3.0,
            1e-3,
            &TOL,
            &LIMITS,
        )
        .expect("wall");
        let plan = wall
            .clip(
                &plane(p(0.0, 0.0, 1.2), Vector3::z()),
                ClipSide::Negative,
                7,
                &TOL,
                &LIMITS,
            )
            .expect("plan cut")
            .expect("some");
        let both_cuts = plan
            .clip(
                &plane(p(2.5, 0.0, 0.0), Vector3::x()),
                ClipSide::Positive,
                9,
                &TOL,
                &LIMITS,
            )
            .expect("section cut")
            .expect("some");
        let area_of = |cut: u32| -> (f64, f64) {
            both_cuts
                .faces()
                .iter()
                .enumerate()
                .filter(|(_, f)| f.tag == FaceTag::Cap { cut })
                .fold((0.0, 0.0), |(area, up), (k, _)| {
                    let normal = both_cuts.face_normal(k).expect("normal");
                    (area + both_cuts.face_area(k).expect("area"), up + normal.z)
                })
        };
        // The plan cut at z = 1.2 keeps the part of its cap with x >= 2.5, facing up; the
        // section cut at x = 2.5 is a face of 1.2 x 0.2 facing -x.
        let (plan_area, plan_up) = area_of(7);
        assert!((plan_area - 2.5 * 0.2).abs() < 1e-12 && (plan_up - 1.0).abs() < 1e-12);
        let (section_area, section_up) = area_of(9);
        assert!((section_area - 1.2 * 0.2).abs() < 1e-12 && section_up.abs() < 1e-12);
        assert!(
            both_cuts
                .faces()
                .iter()
                .filter(|f| matches!(f.tag, FaceTag::Cap { .. }))
                .all(|f| matches!(f.tag, FaceTag::Cap { cut: 7 | 9 }))
        );
        assert!((both_cuts.volume() - 1.2 * 2.5 * 0.2).abs() < 1e-12);
    }
}
