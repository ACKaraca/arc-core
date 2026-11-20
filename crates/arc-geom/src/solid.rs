//! Polyhedral solids: closed, oriented solids bounded by planar faces.
//!
//! A [`Polyhedron`] is a vertex list and faces. Each [`PolyFace`] is an outer loop and hole
//! loops of vertex indices lying in one plane. The specification does not name this type; it is
//! the simplest representation that holds the planar elements of a building model (walls,
//! floors, roofs, openings) exactly, without triangulating them.
//!
//! # Conventions
//!
//! - The outer loop of a face runs counterclockwise seen from outside the solid, holes run
//!   clockwise, so the face normal (the normalized sum of the loops' Newell vectors) points out
//!   of the solid and the signed volume of a valid solid is positive.
//! - Edges are directed by the loops; every directed edge `(a, b)` appears exactly once, and its
//!   reverse `(b, a)` appears exactly once, in another face or the same one.
//!
//! # Validation ([`Polyhedron::new`])
//!
//! Counts are bounded by [`Limits::array_length`] (vertices and faces; loop entries by eight
//! times that); vertices are finite; every vertex belongs to a face. Per face: each loop has at
//! least three vertices, no vertex index repeats within the face (so a loop cannot touch itself
//! or another loop at a vertex), consecutive vertices are farther apart than
//! `tolerance.linear`, the net area is not smaller than `tolerance.linear` times the longest
//! edge, the outer loop is counterclockwise and each hole clockwise about the face normal, every
//! vertex lies within [`PLANARITY_FACTOR`] times `tolerance.linear` of the plane through the
//! outer loop's mean with the face normal, the first vertex of each hole lies inside the outer
//! loop, and the loops triangulate: the exact edge tests of [`crate::triangulate`] find no edge
//! that crosses, touches or folds back over another (a triangle always passes; a quadrilateral is
//! split along a diagonal that gives two counterclockwise triangles; every other face runs the
//! constrained triangulation, whose result is kept for [`Polyhedron::triangulate_face`]). Over
//! the solid:
//! edges are matched as above (`geom.solid.not_closed`, `geom.solid.non_manifold`,
//! `geom.solid.orientation_inconsistent`), and no two triangles of the faces' triangulations
//! use the same directed edge (`geom.solid.non_manifold`: two parts of the solid meet along a
//! line inside a face, as the two prongs of a V-shaped notch meet at its apex) and the volume exceeds the cube of the linear
//! tolerance. A non-finite intermediate result (coordinates beyond about 1e100) is
//! `geom.precision.insufficient`.
//!
//! Cost: the checks are linear in the number of loop entries except the triangulation of faces
//! with five or more vertices or holes, which is `O(n log n)` in the vertices of the face for
//! ordinary faces (triangles and quadrilaterals are checked in constant time).
//!
//! Not checked: whether faces intersect each other away from shared edges, whether two vertices
//! of different index coincide, whether the boundary is connected, and manifoldness at vertices
//! (two parts of a surface may touch at a vertex). The generators of this crate and
//! [`Polyhedron::clip`] do not produce such solids from valid input. A solid may have several
//! shells; a shell oriented inward is a cavity.

use std::collections::HashMap;

use arc_base::math::{Point2, Point3, Transform3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::bbox::Aabb3;
use crate::error::codes;
use crate::plane3::Plane3;
use crate::predicates::{Sign, orient2d};
use crate::triangulate::triangulate_point_loops;

/// The role of a face, carried through every operation so that a face keeps its meaning when it
/// is renumbered, split or trimmed. The integers are caller-visible numbers; the meaning of each
/// is stated on the operation that sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FaceTag {
    /// No role; the tag of faces built directly with [`PolyFace::new`].
    #[default]
    Untagged,
    /// A side face of an extrusion, identified by the curve of the input it comes from, not by
    /// the facets: see [`crate::extrude`] for the numbering.
    Side {
        /// The loop number: the outer loop of each part followed by its holes, parts in order.
        loop_index: u32,
        /// The index of the source curve in the loop as the region stores it.
        curve: u32,
        /// The index of the facet in that curve: 0 for a straight edge.
        facet: u32,
    },
    /// The cap of an extrusion whose outward normal is the normal of the frame, whatever the
    /// sign of the height.
    Top,
    /// The cap of an extrusion whose outward normal is against the normal of the frame.
    Bottom,
    /// A wall of an opening made by [`crate::Polyhedron::subtract_prism`]: the opening number
    /// (the tool's number plus the index of the profile part) and the source curve and facet
    /// of the outline, numbered as for [`FaceTag::Side`] with the profile's outer loop.
    OpeningWall {
        /// The opening number.
        opening: u32,
        /// The index of the source curve in the outline as the region stores it.
        curve: u32,
        /// The index of the facet in that curve: 0 for a straight edge.
        facet: u32,
    },
    /// A face made by [`crate::Polyhedron::clip`] where the cutting plane meets the solid,
    /// with the cut number the caller gave to that clip.
    Cap {
        /// The caller's number of the cut that made the face.
        cut: u32,
    },
}

/// The loops of one planar face: vertex indices into the vertex list of the solid, and the
/// face's role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolyFace {
    /// The outer loop, counterclockwise seen from outside the solid.
    pub outer: Vec<u32>,
    /// The holes, clockwise seen from outside the solid.
    pub holes: Vec<Vec<u32>>,
    /// The role of the face.
    pub tag: FaceTag,
}

impl PolyFace {
    /// A face without holes and without a role.
    #[must_use]
    pub fn new(outer: Vec<u32>) -> Self {
        Self {
            outer,
            holes: Vec::new(),
            tag: FaceTag::Untagged,
        }
    }

    /// The face with the role `tag`.
    #[must_use]
    pub fn with_tag(mut self, tag: FaceTag) -> Self {
        self.tag = tag;
        self
    }

    /// Every loop: the outer loop first, then the holes.
    pub fn loops(&self) -> impl Iterator<Item = &[u32]> {
        std::iter::once(self.outer.as_slice()).chain(self.holes.iter().map(Vec::as_slice))
    }
}

/// A closed solid with planar faces, validated as described in the [module
/// documentation](self).
#[derive(Debug, Clone, PartialEq)]
pub struct Polyhedron {
    vertices: Vec<Point3>,
    faces: Vec<PolyFace>,
    normals: Vec<Vector3>,
    areas: Vec<f64>,
    triangles: Vec<Vec<[u32; 3]>>,
    volume: f64,
    centroid: Point3,
}

/// The Newell vector of a loop (twice its area times its normal), computed relative to the
/// first vertex.
pub(crate) fn newell(vertices: &[Point3], loop_: &[u32]) -> Vector3 {
    let point = |index: u32| usize::try_from(index).ok().and_then(|i| vertices.get(i));
    let Some(origin) = loop_.first().and_then(|&i| point(i)) else {
        return Vector3::zeros();
    };
    let mut sum = Vector3::zeros();
    for (k, &a) in loop_.iter().enumerate() {
        let b = loop_.get((k + 1) % loop_.len()).copied().unwrap_or(a);
        if let (Some(pa), Some(pb)) = (point(a), point(b)) {
            sum += (pa - origin).cross(&(pb - origin));
        }
    }
    sum
}

/// Refuses triangles that use one directed edge twice. A diagonal of a face's triangulation that
/// is also an edge of another face means the surface touches itself along that edge (two parts
/// of the solid meet along a line that lies inside a face), which no mesh of the solid can
/// represent without duplicating vertices.
fn check_triangle_edges(triangles: &[Vec<[u32; 3]>]) -> Result<()> {
    let mut seen: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    for [a, b, c] in triangles.iter().flatten() {
        for edge in [(*a, *b), (*b, *c), (*c, *a)] {
            if !seen.insert(edge) {
                return Err(Error::new(codes::SOLID_NON_MANIFOLD));
            }
        }
    }
    Ok(())
}

fn too_large() -> Error {
    Error::new(codes::SOLID_LIMIT_EXCEEDED)
}

fn topology(face: usize) -> Error {
    Error::new(codes::SOLID_TOPOLOGY_INVALID)
        .with_detail("face", i64::try_from(face).unwrap_or(i64::MAX))
}

/// Whether `point` lies inside the polygon `ring` in the plane spanned by the two coordinate
/// axes left after dropping `axis` (even-odd rule).
pub(crate) fn inside_projected(point: &Point3, ring: &[Point3], axis: usize) -> bool {
    let (i, j) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let at = |p: &Point3, k: usize| p.coords.get(k).copied().unwrap_or(0.0);
    let (px, py) = (at(point, i), at(point, j));
    let mut inside = false;
    for (k, a) in ring.iter().enumerate() {
        let Some(b) = ring.get((k + 1) % ring.len()) else {
            continue;
        };
        let (ax, ay, bx, by) = (at(a, i), at(a, j), at(b, i), at(b, j));
        if (ay > py) != (by > py) && px < ax + (py - ay) / (by - ay) * (bx - ax) {
            inside = !inside;
        }
    }
    inside
}

impl Polyhedron {
    /// Validates and creates a solid.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid`; `geom.input.not_finite` for a non-finite vertex;
    /// `geom.solid.limit_exceeded`; `geom.solid.topology_invalid`; `geom.solid.not_planar`;
    /// `geom.solid.not_closed`; `geom.solid.non_manifold`;
    /// `geom.solid.orientation_inconsistent`; `geom.solid.volume_not_positive`, each as described
    /// in the [module documentation](self).
    pub fn new(
        vertices: Vec<Point3>,
        faces: Vec<PolyFace>,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Self> {
        crate::ensure_tolerance(tolerance)?;
        let limit = usize::try_from(limits.array_length)
            .unwrap_or(usize::MAX)
            .min(usize::try_from(u32::MAX).unwrap_or(usize::MAX));
        let entries: usize = faces
            .iter()
            .map(|f| f.loops().map(<[u32]>::len).sum::<usize>())
            .fold(0, usize::saturating_add);
        if vertices.len() > limit || faces.len() > limit || entries > limit.saturating_mul(8) {
            return Err(too_large());
        }
        for vertex in &vertices {
            crate::ensure_finite(vertex.coords.as_slice())?;
        }
        let mut used = vec![false; vertices.len()];
        let mut normals = Vec::with_capacity(faces.len());
        let mut areas = Vec::with_capacity(faces.len());
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for (index, face) in faces.iter().enumerate() {
            let (normal, area) =
                check_face(index, face, &vertices, &mut used, &mut edges, tolerance)?;
            normals.push(normal);
            areas.push(area);
        }
        if used.iter().any(|&u| !u) {
            return Err(Error::new(codes::SOLID_TOPOLOGY_INVALID));
        }
        check_edges(&edges)?;
        let (volume, centroid) = volume_and_centroid(&vertices, &faces);
        if !volume.is_finite() {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT));
        }
        let positive = volume > tolerance.linear * tolerance.linear * tolerance.linear;
        if !positive {
            return Err(Error::new(codes::SOLID_VOLUME));
        }
        let mut triangles = Vec::with_capacity(faces.len());
        for (index, (face, normal)) in faces.iter().zip(&normals).enumerate() {
            triangles.push(triangulate_loops(index, face, normal, &vertices)?);
        }
        check_triangle_edges(&triangles)?;
        Ok(Self {
            vertices,
            faces,
            normals,
            areas,
            triangles,
            volume,
            centroid,
        })
    }

    /// The vertices.
    #[must_use]
    pub fn vertices(&self) -> &[Point3] {
        &self.vertices
    }

    /// The faces.
    #[must_use]
    pub fn faces(&self) -> &[PolyFace] {
        &self.faces
    }

    /// The outward unit normal of face `index`, or `None` for an index outside the faces.
    #[must_use]
    pub fn face_normal(&self, index: usize) -> Option<Vector3> {
        self.normals.get(index).copied()
    }

    /// The area of face `index` (holes subtracted), or `None` for an index outside the faces.
    #[must_use]
    pub fn face_area(&self, index: usize) -> Option<f64> {
        self.areas.get(index).copied()
    }

    /// The plane of face `index` with the outward normal, through its first outer vertex.
    #[must_use]
    pub fn face_plane(&self, index: usize) -> Option<Plane3> {
        let face = self.faces.get(index)?;
        let origin = *self
            .vertices
            .get(usize::try_from(*face.outer.first()?).ok()?)?;
        Plane3::new(origin, self.face_normal(index)?).ok()
    }

    /// The enclosed volume.
    #[must_use]
    pub fn volume(&self) -> f64 {
        self.volume
    }

    /// The total area of the faces.
    #[must_use]
    pub fn surface_area(&self) -> f64 {
        self.areas.iter().sum()
    }

    /// The centroid of the enclosed volume.
    #[must_use]
    pub fn centroid(&self) -> Point3 {
        self.centroid
    }

    /// The bounding box of the vertices.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb3 {
        Aabb3::from_points(self.vertices.iter())
    }

    /// The image of the solid under `transform`. A mirroring transform reverses every loop so
    /// the normals keep pointing outward. The result is validated again.
    ///
    /// # Errors
    ///
    /// `geom.transform.singular` when the linear part is not invertible;
    /// `geom.precision.insufficient` for a non-finite image; otherwise as [`Self::new`].
    pub fn transformed(
        &self,
        transform: &Transform3,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Self> {
        let linear = transform.matrix().fixed_view::<3, 3>(0, 0).into_owned();
        let determinant = linear.determinant();
        if !determinant.is_finite() {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT));
        }
        if determinant == 0.0 {
            return Err(Error::new(codes::TRANSFORM_SINGULAR));
        }
        let vertices: Vec<Point3> = self.vertices.iter().map(|p| transform * p).collect();
        if vertices
            .iter()
            .any(|p| p.coords.iter().any(|c| !c.is_finite()))
        {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT));
        }
        let mirrored = determinant < 0.0;
        let flip = |loop_: &Vec<u32>| {
            let mut out = loop_.clone();
            if mirrored {
                out.reverse();
            }
            out
        };
        let faces = self
            .faces
            .iter()
            .map(|f| PolyFace {
                outer: flip(&f.outer),
                holes: f.holes.iter().map(flip).collect(),
                tag: f.tag,
            })
            .collect();
        Self::new(vertices, faces, tolerance, limits)
    }

    /// The triangles of face `index`, computed when the solid was validated and without adding
    /// vertices. They index [`Self::vertices`] and run counterclockwise seen from outside, so
    /// their normals agree with [`Self::face_normal`].
    ///
    /// # Errors
    ///
    /// `geom.solid.topology_invalid` for an index outside the faces.
    pub fn triangulate_face(&self, index: usize, _tolerance: &Tolerance) -> Result<Vec<[u32; 3]>> {
        self.triangles
            .get(index)
            .cloned()
            .ok_or_else(|| topology(index))
    }

    /// The number of connected shells: groups of faces that share vertices. A shell oriented
    /// inward (a cavity) counts as one, and so do two parts that touch at a single vertex.
    #[must_use]
    pub fn shell_count(&self) -> usize {
        let mut parent: Vec<usize> = (0..self.vertices.len()).collect();
        fn root(parent: &mut [usize], mut v: usize) -> usize {
            while let Some(&up) = parent.get(v) {
                if up == v {
                    break;
                }
                let grand = parent.get(up).copied().unwrap_or(up);
                if let Some(slot) = parent.get_mut(v) {
                    *slot = grand;
                }
                v = grand;
            }
            v
        }
        for face in &self.faces {
            let mut ids = face
                .loops()
                .flatten()
                .filter_map(|&i| usize::try_from(i).ok());
            let Some(first) = ids.next() else { continue };
            for other in ids {
                let (a, b) = (root(&mut parent, first), root(&mut parent, other));
                if let Some(slot) = parent.get_mut(a) {
                    *slot = b;
                }
            }
        }
        let mut roots: Vec<usize> = (0..parent.len()).map(|v| root(&mut parent, v)).collect();
        roots.sort_unstable();
        roots.dedup();
        roots.len()
    }

    /// The role of face `index`, or `None` for an index outside the faces.
    #[must_use]
    pub fn face_tag(&self, index: usize) -> Option<FaceTag> {
        self.faces.get(index).map(|f| f.tag)
    }
}

/// The two coordinate axes left after dropping the dominant axis of `normal`, ordered so that a
/// loop counterclockwise about `normal` is counterclockwise in these coordinates. The 2D
/// coordinates are the vertex coordinates themselves, so they are never smaller than the
/// coordinates of the solid.
pub(crate) fn projection(normal: &Vector3) -> (usize, usize, bool) {
    let component = |k: usize| normal.get(k).copied().unwrap_or(0.0);
    let axis = (0..3)
        .max_by(|&a, &b| component(a).abs().total_cmp(&component(b).abs()))
        .unwrap_or(2);
    let (i, j) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    (i, j, component(axis) >= 0.0)
}

/// The 2D coordinates of `p` in the projection of [`projection`].
pub(crate) fn project_point(p: &Point3, (i, j, positive): (usize, usize, bool)) -> Point2 {
    let (a, b) = (
        p.coords.get(i).copied().unwrap_or(0.0),
        p.coords.get(j).copied().unwrap_or(0.0),
    );
    if positive {
        Point2::new(a, b)
    } else {
        Point2::new(b, a)
    }
}

fn project_loop(
    vertices: &[Point3],
    loop_: &[u32],
    (i, j, positive): (usize, usize, bool),
) -> Vec<Point2> {
    loop_
        .iter()
        .filter_map(|&v| usize::try_from(v).ok().and_then(|v| vertices.get(v)))
        .map(|p| {
            let (a, b) = (
                p.coords.get(i).copied().unwrap_or(0.0),
                p.coords.get(j).copied().unwrap_or(0.0),
            );
            if positive {
                Point2::new(a, b)
            } else {
                Point2::new(b, a)
            }
        })
        .collect()
}

/// The triangles of one face, indexing the solid's vertices, or an error when its loops do not
/// triangulate (they touch or cross after projection). A triangle is its own triangulation; a
/// quadrilateral without holes is split along the diagonal that gives two counterclockwise
/// triangles; every other face uses the constrained triangulation of [`crate::triangulate`].
fn triangulate_loops(
    index: usize,
    face: &PolyFace,
    normal: &Vector3,
    vertices: &[Point3],
) -> Result<Vec<[u32; 3]>> {
    let invalid = |source: Error| topology(index).with_source(source);
    if face.holes.is_empty() {
        if let [a, b, c] = face.outer.as_slice() {
            return Ok(vec![[*a, *b, *c]]);
        }
        if let [a, b, c, d] = face.outer.as_slice() {
            let frame = projection(normal);
            let points = project_loop(vertices, &face.outer, frame);
            if let [pa, pb, pc, pd] = points.as_slice() {
                let positive = |p: &Point2, q: &Point2, r: &Point2| {
                    orient2d(p, q, r).map(|s| s == Sign::Positive)
                };
                if positive(pa, pb, pc).map_err(invalid)?
                    && positive(pa, pc, pd).map_err(invalid)?
                {
                    return Ok(vec![[*a, *b, *c], [*a, *c, *d]]);
                }
                if positive(pa, pb, pd).map_err(invalid)?
                    && positive(pb, pc, pd).map_err(invalid)?
                {
                    return Ok(vec![[*a, *b, *d], [*b, *c, *d]]);
                }
            }
            return Err(topology(index));
        }
    }
    let frame = projection(normal);
    let loops: Vec<Vec<Point2>> = face
        .loops()
        .map(|l| project_loop(vertices, l, frame))
        .collect();
    let ids: Vec<u32> = face.loops().flatten().copied().collect();
    let (points, triangles) = triangulate_point_loops(&loops).map_err(invalid)?;
    if points.len() != ids.len() {
        return Err(topology(index));
    }
    triangles
        .iter()
        .map(|t| {
            let id = |k: u32| usize::try_from(k).ok().and_then(|k| ids.get(k).copied());
            match (id(t[0]), id(t[1]), id(t[2])) {
                (Some(a), Some(b), Some(c)) => Ok([a, b, c]),
                _ => Err(topology(index)),
            }
        })
        .collect()
}

/// How many linear tolerances a vertex may be off the plane of its face. Two vertices that each
/// lie within the linear tolerance of a cutting plane, on opposite sides, are up to twice the
/// tolerance apart, and the cap through them must still be a face.
pub const PLANARITY_FACTOR: f64 = 2.0;

/// Checks one face; returns its outward unit normal and area.
fn check_face(
    index: usize,
    face: &PolyFace,
    vertices: &[Point3],
    used: &mut [bool],
    edges: &mut Vec<(u32, u32)>,
    tolerance: &Tolerance,
) -> Result<(Vector3, f64)> {
    let mut seen: Vec<u32> = Vec::new();
    let mut longest = 0.0_f64;
    for loop_ in face.loops() {
        if loop_.len() < 3 {
            return Err(topology(index));
        }
        for (k, &a) in loop_.iter().enumerate() {
            let b = loop_.get((k + 1) % loop_.len()).copied().unwrap_or(a);
            let (Some(pa), Some(pb)) = (
                usize::try_from(a).ok().and_then(|i| vertices.get(i)),
                usize::try_from(b).ok().and_then(|i| vertices.get(i)),
            ) else {
                return Err(topology(index));
            };
            let length = (pb - pa).norm();
            if !length.is_finite() {
                return Err(Error::new(codes::PRECISION_INSUFFICIENT));
            }
            let long_enough = length > tolerance.linear;
            if !long_enough {
                return Err(topology(index));
            }
            longest = longest.max(length);
            seen.push(a);
            edges.push((a, b));
            if let Some(flag) = usize::try_from(a).ok().and_then(|i| used.get_mut(i)) {
                *flag = true;
            }
        }
    }
    seen.sort_unstable();
    if seen.windows(2).any(|w| w[0] == w[1]) {
        return Err(topology(index));
    }
    let sum = face
        .loops()
        .map(|l| newell(vertices, l))
        .fold(Vector3::zeros(), |a, b| a + b);
    let magnitude = sum.norm();
    if !magnitude.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    let big_enough = magnitude >= tolerance.linear * longest && magnitude > 0.0;
    if !big_enough {
        return Err(topology(index));
    }
    let normal = sum / magnitude;
    if newell(vertices, &face.outer).dot(&normal) <= 0.0
        || face
            .holes
            .iter()
            .any(|h| newell(vertices, h).dot(&normal) >= 0.0)
    {
        return Err(topology(index));
    }
    let ring: Vec<Point3> = face
        .outer
        .iter()
        .filter_map(|&i| {
            usize::try_from(i)
                .ok()
                .and_then(|i| vertices.get(i))
                .copied()
        })
        .collect();
    let mean = ring.iter().fold(Vector3::zeros(), |a, p| a + p.coords) / ring.len() as f64;
    let mean = Point3::from(mean);
    for loop_ in face.loops() {
        for &i in loop_ {
            let p = usize::try_from(i).ok().and_then(|i| vertices.get(i));
            if p.is_none_or(|p| normal.dot(&(p - mean)).abs() > PLANARITY_FACTOR * tolerance.linear)
            {
                return Err(Error::new(codes::SOLID_NOT_PLANAR)
                    .with_detail("face", i64::try_from(index).unwrap_or(i64::MAX)));
            }
        }
    }
    let axis = (0..3)
        .max_by(|&a, &b| {
            let at = |k: usize| normal.get(k).copied().unwrap_or(0.0).abs();
            at(a).total_cmp(&at(b))
        })
        .unwrap_or(2);
    for hole in &face.holes {
        let first = hole
            .first()
            .and_then(|&i| usize::try_from(i).ok())
            .and_then(|i| vertices.get(i));
        if first.is_none_or(|p| !inside_projected(p, &ring, axis)) {
            return Err(topology(index));
        }
    }
    Ok((normal, magnitude / 2.0))
}

/// Checks that every directed edge appears once and has exactly one reverse.
fn check_edges(edges: &[(u32, u32)]) -> Result<()> {
    let mut count: HashMap<(u32, u32), u32> = HashMap::with_capacity(edges.len());
    for &edge in edges {
        *count.entry(edge).or_insert(0) += 1;
    }
    let twice = |edge: (u32, u32)| count.get(&edge).copied().unwrap_or(0);
    for &(a, b) in edges {
        let forward = twice((a, b));
        let reverse = twice((b, a));
        let detail = |error: Error| {
            error
                .with_detail("edge_start", i64::from(a))
                .with_detail("edge_end", i64::from(b))
        };
        if forward > 1 && reverse >= 1 || reverse > 1 {
            return Err(detail(Error::new(codes::SOLID_NON_MANIFOLD)));
        }
        if forward > 1 {
            return Err(detail(Error::new(codes::SOLID_ORIENTATION)));
        }
        if reverse == 0 {
            return Err(detail(Error::new(codes::SOLID_NOT_CLOSED)));
        }
    }
    Ok(())
}

/// The signed volume and centroid by summing signed tetrahedra from the center of the bounding
/// box over a fan of each loop; holes, running the other way, subtract.
fn volume_and_centroid(vertices: &[Point3], faces: &[PolyFace]) -> (f64, Point3) {
    let reference = {
        let boxed = Aabb3::from_points(vertices.iter());
        if boxed.is_empty() {
            Point3::origin()
        } else {
            Point3::from((boxed.min().coords + boxed.max().coords) * 0.5)
        }
    };
    let mut volume = 0.0;
    let mut moment = Vector3::zeros();
    for loop_ in faces.iter().flat_map(PolyFace::loops) {
        let at = |i: u32| {
            usize::try_from(i)
                .ok()
                .and_then(|i| vertices.get(i))
                .map_or_else(Vector3::zeros, |p| p - reference)
        };
        let Some(&first) = loop_.first() else {
            continue;
        };
        let a = at(first);
        for pair in loop_.windows(2).skip(1) {
            let (b, c) = (at(pair[0]), at(pair[1]));
            let tetra = a.dot(&b.cross(&c)) / 6.0;
            volume += tetra;
            moment += (a + b + c) * (tetra / 4.0);
        }
    }
    let centroid = if volume > 0.0 {
        reference + moment / volume
    } else {
        reference
    };
    (volume, centroid)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const TOL: Tolerance = Tolerance::DEFAULT;

    pub fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    /// The axis-aligned box from the origin to `(x, y, z)` with outward faces.
    pub fn cuboid(x: f64, y: f64, z: f64) -> Polyhedron {
        let vertices = vec![
            p(0.0, 0.0, 0.0),
            p(x, 0.0, 0.0),
            p(x, y, 0.0),
            p(0.0, y, 0.0),
            p(0.0, 0.0, z),
            p(x, 0.0, z),
            p(x, y, z),
            p(0.0, y, z),
        ];
        let faces = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ]
        .map(|f| PolyFace::new(f.to_vec()))
        .to_vec();
        Polyhedron::new(vertices, faces, &TOL, &Limits::DEFAULT).expect("valid cuboid")
    }

    /// Whether the triangles of every face together form a watertight surface: every directed
    /// triangle edge appears once and its reverse once, and the triangles have the area of the
    /// faces.
    pub fn watertight(solid: &Polyhedron) -> bool {
        let mut count: HashMap<(u32, u32), i32> = HashMap::new();
        let mut area = 0.0;
        for index in 0..solid.faces().len() {
            let Ok(triangles) = solid.triangulate_face(index, &TOL) else {
                return false;
            };
            for t in triangles {
                for k in 0..3 {
                    *count.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
                }
                let v = |k: u32| solid.vertices()[k as usize];
                area += (v(t[1]) - v(t[0])).cross(&(v(t[2]) - v(t[0]))).norm() / 2.0;
            }
        }
        let balanced = count
            .iter()
            .all(|(&(a, b), &n)| n == 1 && count.get(&(b, a)) == Some(&1));
        balanced && (area - solid.surface_area()).abs() <= 1e-9 * (1.0 + area)
    }

    fn code(result: Result<Polyhedron>) -> Option<&'static str> {
        result.err().map(|e| e.code())
    }

    fn raw_cube() -> (Vec<Point3>, Vec<PolyFace>) {
        let cube = cuboid(1.0, 1.0, 1.0);
        (cube.vertices().to_vec(), cube.faces().to_vec())
    }

    #[test]
    fn unit_cube_properties() {
        let cube = cuboid(1.0, 1.0, 1.0);
        assert!((cube.volume() - 1.0).abs() < 1e-12);
        assert!((cube.surface_area() - 6.0).abs() < 1e-12);
        assert!((cube.centroid() - p(0.5, 0.5, 0.5)).norm() < 1e-12);
        assert_eq!(cube.bounding_box().max(), p(1.0, 1.0, 1.0));
        assert_eq!(cube.face_normal(0), Some(Vector3::new(0.0, 0.0, -1.0)));
        assert_eq!(cube.face_area(1), Some(1.0));
        assert_eq!(cube.face_normal(6), None);
    }

    #[test]
    fn invalid_solids_have_specific_codes() {
        let (v, f) = raw_cube();
        // Missing face: open.
        assert_eq!(
            code(Polyhedron::new(
                v.clone(),
                f[..5].to_vec(),
                &TOL,
                &Limits::DEFAULT
            )),
            Some("geom.solid.not_closed")
        );
        // Inside out.
        let reversed = f
            .iter()
            .map(|face| PolyFace::new(face.outer.iter().rev().copied().collect()))
            .collect();
        assert_eq!(
            code(Polyhedron::new(v.clone(), reversed, &TOL, &Limits::DEFAULT)),
            Some("geom.solid.volume_not_positive")
        );
        // One face flipped: orientation inconsistent.
        let mut flipped = f.clone();
        flipped[1].outer.reverse();
        assert_eq!(
            code(Polyhedron::new(v.clone(), flipped, &TOL, &Limits::DEFAULT)),
            Some("geom.solid.orientation_inconsistent")
        );
        // A third face on an edge: non-manifold.
        let mut extra = f.clone();
        extra.push(PolyFace::new(vec![0, 1, 5]));
        assert_eq!(
            code(Polyhedron::new(v.clone(), extra, &TOL, &Limits::DEFAULT)),
            Some("geom.solid.non_manifold")
        );
        // Non-planar top.
        let mut warped = v.clone();
        warped[6].z += 0.01;
        assert_eq!(
            code(Polyhedron::new(warped, f.clone(), &TOL, &Limits::DEFAULT)),
            Some("geom.solid.not_planar")
        );
        // Index outside, short loop, repeated vertex, unused vertex.
        let mut bad = f.clone();
        bad[0].outer = vec![0, 3, 99];
        assert_eq!(
            code(Polyhedron::new(v.clone(), bad, &TOL, &Limits::DEFAULT)),
            Some("geom.solid.topology_invalid")
        );
        let mut short = f.clone();
        short[0].outer = vec![0, 3];
        assert_eq!(
            code(Polyhedron::new(v.clone(), short, &TOL, &Limits::DEFAULT)),
            Some("geom.solid.topology_invalid")
        );
        let mut unused = v.clone();
        unused.push(p(5.0, 5.0, 5.0));
        assert_eq!(
            code(Polyhedron::new(unused, f.clone(), &TOL, &Limits::DEFAULT)),
            Some("geom.solid.topology_invalid")
        );
        // Non-finite and limits.
        let mut nan = v.clone();
        nan[0].x = f64::NAN;
        assert_eq!(
            code(Polyhedron::new(nan, f.clone(), &TOL, &Limits::DEFAULT)),
            Some("geom.input.not_finite")
        );
        let small = Limits {
            array_length: 4,
            ..Limits::DEFAULT
        };
        assert_eq!(
            code(Polyhedron::new(v, f, &TOL, &small)),
            Some("geom.solid.limit_exceeded")
        );
    }

    #[test]
    fn transforms_keep_volume_and_normals_outward() {
        let cube = cuboid(1.0, 2.0, 3.0);
        let mut matrix = arc_base::math::Matrix4::identity();
        matrix[(0, 0)] = -1.0; // mirror
        matrix[(0, 3)] = 5.0;
        let mirrored = cube
            .transformed(
                &Transform3::from_matrix_unchecked(matrix),
                &TOL,
                &Limits::DEFAULT,
            )
            .expect("mirror");
        assert!((mirrored.volume() - 6.0).abs() < 1e-12);
        assert!((mirrored.centroid() - p(4.5, 1.0, 1.5)).norm() < 1e-12);
        let mut singular = arc_base::math::Matrix4::identity();
        singular[(2, 2)] = 0.0;
        assert_eq!(
            code(cube.transformed(
                &Transform3::from_matrix_unchecked(singular),
                &TOL,
                &Limits::DEFAULT
            )),
            Some("geom.transform.singular")
        );
    }

    #[test]
    fn faces_triangulate_with_agreeing_normals() {
        let cube = cuboid(1.0, 1.0, 1.0);
        for index in 0..cube.faces().len() {
            let triangles = cube.triangulate_face(index, &TOL).expect("triangulation");
            assert_eq!(triangles.len(), 2);
            let normal = cube.face_normal(index).expect("normal");
            let mut area = 0.0;
            for t in triangles {
                let v = |k: u32| cube.vertices()[k as usize];
                let n = (v(t[1]) - v(t[0])).cross(&(v(t[2]) - v(t[0])));
                assert!(n.dot(&normal) > 0.0);
                area += n.norm() / 2.0;
            }
            assert!((area - 1.0).abs() < 1e-12);
        }
        assert_eq!(
            cube.triangulate_face(9, &TOL).err().map(|e| e.code()),
            Some("geom.solid.topology_invalid")
        );
    }

    #[test]
    fn huge_coordinates_are_a_precision_error_not_a_volume_error() {
        let (v, f) = raw_cube();
        let scaled: Vec<Point3> = v
            .iter()
            .map(|q| Point3::new(q.x * 1e200, q.y * 1e200, q.z * 1e200))
            .collect();
        assert_eq!(
            code(Polyhedron::new(scaled, f, &TOL, &Limits::DEFAULT)),
            Some("geom.precision.insufficient")
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, reason = "tiny test indices")]
    fn a_face_that_does_not_triangulate_is_refused_by_the_validator() {
        // A prism over an outline with a zero-width spike: two distinct vertices at (2, 2), so
        // the top and bottom loops touch themselves. Every edge is matched and the volume is
        // positive, but the cap cannot be meshed.
        let outline = [
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 2.0),
            (2.0, 2.0),
            (2.0, 1.0),
            (2.0, 2.0),
            (0.0, 2.0),
        ];
        let n = outline.len();
        let mut vertices = Vec::new();
        for z in [0.0, 1.0] {
            vertices.extend(outline.iter().map(|&(x, y)| p(x, y, z)));
        }
        let mut faces = Vec::new();
        for k in 0..n {
            let next = (k + 1) % n;
            faces.push(PolyFace::new(vec![
                k as u32,
                next as u32,
                (n + next) as u32,
                (n + k) as u32,
            ]));
        }
        faces.push(PolyFace::new((n as u32..2 * n as u32).collect()));
        faces.push(PolyFace::new((0..n as u32).rev().collect()));
        assert_eq!(
            code(Polyhedron::new(vertices, faces, &TOL, &Limits::DEFAULT)),
            Some("geom.solid.topology_invalid")
        );
    }
}
