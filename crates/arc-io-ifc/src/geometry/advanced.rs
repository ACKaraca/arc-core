//! Advanced B-reps (CORE-IO-015): `IfcAdvancedBrep` and `IfcAdvancedBrepWithVoids`, a closed shell of
//! `IfcAdvancedFace`s (and plain `IfcFace`s with polygon loops) evaluated into one triangle mesh.
//!
//! - **Edges.** An `IfcOrientedEdge` is its `EdgeElement` (an `IfcEdgeCurve`, or an edge without
//!   geometry, which is the straight line between its vertices) in its `Orientation`. An edge curve
//!   runs from `EdgeStart` to `EdgeEnd` along `EdgeGeometry` in the direction of the curve
//!   (`SameSense` `.T.`) or against it. The geometry is a line, polyline, circle, ellipse, trimmed
//!   curve, composite curve, B-spline curve, the 3D curve of a surface curve, or an offset curve; the
//!   portion between the two vertices is sampled with the chord tolerance, and a closed curve whose
//!   vertices coincide is the whole curve. The samples of an edge are made once, so the two faces that
//!   share it share its vertices; the end points are the vertex points exactly. Another geometry is the
//!   straight line between the vertices and the item is reported as approximated.
//! - **Faces.** The `FaceSurface` is a plane, cylindrical, conical, spherical or toroidal surface,
//!   a surface of linear extrusion or of revolution, or a B-spline surface (see [`super::surface`]).
//!   The face normal is the surface normal when `SameSense` is `.T.` and its opposite otherwise. A
//!   planar face is triangulated in its plane. For a curved face the boundary loops are mapped into
//!   the parameter space of the surface (an angle is unwrapped along the loop), the region between
//!   them is triangulated there with a constrained Delaunay triangulation with a lattice of interior
//!   points that keeps flat triangles within the chord tolerance of the surface (none for a ruled
//!   surface, whose boundary points suffice), and the points of the lattice are mapped back to
//!   the surface. The parameters are scaled to lengths before the triangulation, so triangles are
//!   not stretched.
//! - **Periodic faces.** A loop that is closed in space but not in the parameters (it winds once
//!   about the axis, a circle around a cylinder) bounds a strip: two such loops are joined by the seam
//!   of the period, one such loop on a sphere or a cone is closed through the pole or the apex on the
//!   side the face lies on. A loop that winds in both directions, or any other number of winding loops,
//!   is not evaluated.
//! - **Item.** Any face that is not evaluated skips the whole item, so no shell is emitted with a face
//!   missing. If the shell has negative volume the winding of every face is reversed.

use std::collections::HashMap;
use std::f64::consts::TAU;

use arc_base::math::{Point2, Point3, Vector3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;
use crate::schema::access::known;

use super::attr::{self, named, skip};
use super::faces::Welder;
use super::surface::{Kind, Surf, unwrap_near};
use super::{Evaluator, MAX_LOOP_VERTICES, Piece, Shape, Step};

/// The most lattice points of one face.
const MAX_LATTICE: usize = 250_000;

/// The samples of a curve: parameter and point, and the period of the parameter if the curve is closed.
struct Sampled {
    samples: Vec<(f64, Point3)>,
    period: Option<f64>,
}

/// The surface of a face.
enum FaceSurface {
    Plane(Vector3),
    Curved(Box<Surf>),
}

impl Evaluator<'_> {
    /// The pieces of an `IfcAdvancedBrep` or `IfcAdvancedBrepWithVoids`.
    pub(crate) fn advanced_brep(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let outer = attr::reference(file.attr(record, known::BREP_OUTER), kind)?;
        let mut welder = Welder::new();
        let mut edges: HashMap<u64, Vec<Point3>> = HashMap::new();
        let mut approximated = false;
        let mut deviation = 0.0_f64;
        let mut shells = vec![outer];
        if kind == "IFCADVANCEDBREPWITHVOIDS" {
            let (voids, _) = attr::distinct_references(
                &mut self.budget,
                named(file, record, "Voids", 1),
                kind,
                usize::MAX,
            )?;
            shells.extend(voids);
        }
        for shell in shells {
            for face in self.shell_faces(shell)? {
                self.budget.step()?;
                self.advanced_face(
                    face,
                    &mut welder,
                    &mut edges,
                    &mut approximated,
                    &mut deviation,
                )?;
            }
        }
        if welder.triangles.is_empty() {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        if approximated || deviation > self.tol.linear {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        self.charge_shape()?;
        let mesh = welder.finish(&self.limits)?;
        let mut acc = super::solid::MeshAcc::default();
        let triangles: Vec<[usize; 3]> = mesh
            .triangles()
            .iter()
            .map(|t| t.map(|i| i as usize))
            .collect();
        // `push_part` reverses an inward-wound shell; the voids of a shell are wound the other way,
        // so only a shell without voids is corrected.
        if kind == "IFCADVANCEDBREP" {
            acc.push_part(mesh.positions().to_vec(), &triangles)?;
            return self.finish_mesh(acc);
        }
        Ok(Self::own(Shape::Mesh(mesh)))
    }

    fn advanced_face(
        &mut self,
        face: u64,
        welder: &mut Welder,
        edges: &mut HashMap<u64, Vec<Point3>>,
        approximated: &mut bool,
        deviation: &mut f64,
    ) -> Step<()> {
        let file = self.file;
        let record = attr::typed_first(file, face)
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCADVANCEDFACE"))?;
        let kind = record.type_name.as_str();
        match kind {
            "IFCFACE" => return self.add_face(face, welder, deviation),
            "IFCADVANCEDFACE" | "IFCFACESURFACE" => {}
            other => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
        let surface = attr::reference(named(file, record, "FaceSurface", 1), kind)?;
        let sense = attr::boolean(named(file, record, "SameSense", 2)).unwrap_or(true);
        let bounds = attr::ordered_references(
            &mut self.budget,
            file.attr(record, known::FACE_BOUNDS),
            kind,
            usize::MAX,
        )?;
        let mut loops: Vec<Vec<Point3>> = Vec::new();
        let mut outer = 0;
        for bound in bounds {
            self.budget.step()?;
            let bound_record = attr::typed_first(file, bound)
                .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCFACEBOUND"))?;
            let is_outer = match bound_record.type_name.as_str() {
                "IFCFACEOUTERBOUND" => true,
                "IFCFACEBOUND" => false,
                other => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
            };
            let loop_name = attr::reference(
                file.attr(bound_record, known::FACE_BOUND),
                &bound_record.type_name,
            )?;
            let Some(mut points) = self.face_loop(loop_name, edges, approximated)? else {
                continue;
            };
            if attr::enumeration(file.attr(bound_record, known::FACE_BOUND_ORIENTATION))
                == Some("F")
            {
                points.reverse();
            }
            if is_outer {
                outer = loops.len();
            }
            loops.push(points);
        }
        if loops.is_empty() {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let total: usize = loops.iter().map(Vec::len).sum();
        if total > MAX_LOOP_VERTICES {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        match self.face_surface(surface)? {
            FaceSurface::Plane(normal) => {
                let wanted = if sense { normal } else { -normal };
                loops.swap(0, outer);
                let mut newell = Vector3::zeros();
                if let Some(first) = loops.first() {
                    for (k, a) in first.iter().enumerate() {
                        let b = first[(k + 1) % first.len()];
                        newell += Vector3::new(
                            (a.y - b.y) * (a.z + b.z),
                            (a.z - b.z) * (a.x + b.x),
                            (a.x - b.x) * (a.y + b.y),
                        );
                    }
                }
                if newell.dot(&wanted) < 0.0 {
                    loops.iter_mut().for_each(|l| l.reverse());
                }
                self.emit_planar(&loops, welder, deviation)
            }
            FaceSurface::Curved(surf) => {
                loops.swap(0, outer);
                self.curved_face(&surf, &loops, sense, welder)
            }
        }
    }

    /// The surface of a face.
    #[allow(clippy::too_many_lines, reason = "one arm per surface kind")]
    fn face_surface(&mut self, name: u64) -> Step<FaceSurface> {
        let file = self.file;
        let record = attr::typed_first(file, name)
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCSURFACE"))?;
        let kind = record.type_name.as_str();
        let position = |this: &Self| -> Step<arc_base::math::Transform3> {
            this.axis3(attr::opt_reference(
                named(file, record, "Position", 0),
                kind,
            )?)
        };
        let length = |this: &Self, attribute: &str, index: usize| -> Step<f64> {
            let v = attr::real(named(file, record, attribute, index), this.scale, kind)?;
            if v > 0.0 {
                Ok(v)
            } else {
                Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
            }
        };
        let (frame, surface) = match kind {
            "IFCPLANE" => {
                let frame = position(self)?;
                return Ok(FaceSurface::Plane(frame.transform_vector(&Vector3::z())));
            }
            "IFCCYLINDRICALSURFACE" => (
                position(self)?,
                Kind::Cylinder {
                    r: length(self, "Radius", 1)?,
                },
            ),
            "IFCCONICALSURFACE" => {
                let angle = attr::real(named(file, record, "SemiAngle", 2), self.angle, kind)?;
                (
                    position(self)?,
                    Kind::Cone {
                        r: attr::real(named(file, record, "Radius", 1), self.scale, kind)?.max(0.0),
                        tan: angle.tan(),
                    },
                )
            }
            "IFCSPHERICALSURFACE" => (
                position(self)?,
                Kind::Sphere {
                    r: length(self, "Radius", 1)?,
                },
            ),
            "IFCTOROIDALSURFACE" => (
                position(self)?,
                Kind::Torus {
                    big: length(self, "MajorRadius", 1)?,
                    small: length(self, "MinorRadius", 2)?,
                },
            ),
            "IFCSURFACEOFLINEAREXTRUSION" | "IFCSURFACEOFREVOLUTION" => {
                let profile = attr::reference(named(file, record, "SweptCurve", 0), kind)?;
                let frame = self.axis3(attr::opt_reference(
                    named(file, record, "Position", 1),
                    kind,
                )?)?;
                let profile_record = attr::typed_first(file, profile)
                    .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCPROFILEDEF"))?;
                let curve = attr::reference(profile_record.parameters.get(2), kind)?;
                let points = self.curve_points(curve, MAX_LOOP_VERTICES)?;
                let curve: Vec<Point3> = points.iter().map(|p| frame.transform_point(p)).collect();
                if kind == "IFCSURFACEOFLINEAREXTRUSION" {
                    let direction =
                        attr::reference(named(file, record, "ExtrudedDirection", 2), kind)?;
                    let dir = super::place::unit(self.direction(direction)?, kind)?;
                    (
                        arc_base::math::Transform3::identity(),
                        Kind::Extrusion {
                            curve,
                            dir: frame.transform_vector(&dir),
                        },
                    )
                } else {
                    let axis = attr::reference(named(file, record, "AxisPosition", 2), kind)?;
                    let axis = super::place::expect(file, axis, "IFCAXIS1PLACEMENT")?;
                    let origin = match attr::opt_reference(named(file, axis, "Location", 0), kind)?
                    {
                        Some(p) => frame.transform_point(&self.point(p)?),
                        None => frame.transform_point(&Point3::origin()),
                    };
                    let direction = match attr::opt_reference(named(file, axis, "Axis", 1), kind)? {
                        Some(d) => super::place::unit(self.direction(d)?, kind)?,
                        None => Vector3::z(),
                    };
                    (
                        arc_base::math::Transform3::identity(),
                        Kind::Revolution {
                            curve,
                            origin,
                            axis: frame.transform_vector(&direction),
                        },
                    )
                }
            }
            "IFCBSPLINESURFACEWITHKNOTS" | "IFCRATIONALBSPLINESURFACEWITHKNOTS" => (
                arc_base::math::Transform3::identity(),
                Kind::Spline(self.bspline_surface(record)?),
            ),
            other => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        };
        Ok(FaceSurface::Curved(Box::new(Surf {
            frame,
            kind: surface,
        })))
    }

    /// The points of a face bound loop in its direction; `None` for a vertex loop.
    fn face_loop(
        &mut self,
        name: u64,
        edges: &mut HashMap<u64, Vec<Point3>>,
        approximated: &mut bool,
    ) -> Step<Option<Vec<Point3>>> {
        let file = self.file;
        let record = attr::typed_first(file, name)
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCLOOP"))?;
        match record.type_name.as_str() {
            "IFCPOLYLOOP" => Ok(Some(self.polygon_loop(name)?)),
            "IFCVERTEXLOOP" => Ok(None),
            "IFCEDGELOOP" => {
                let list = attr::ordered_references(
                    &mut self.budget,
                    named(file, record, "EdgeList", 0),
                    "IFCEDGELOOP",
                    MAX_LOOP_VERTICES,
                )?;
                let mut out: Vec<Point3> = Vec::new();
                for oriented in list {
                    self.budget.step()?;
                    let oriented = super::place::expect(file, oriented, "IFCORIENTEDEDGE")?;
                    let edge = attr::reference(
                        named(file, oriented, "EdgeElement", 2),
                        "IFCORIENTEDEDGE",
                    )?;
                    let forward =
                        attr::boolean(named(file, oriented, "Orientation", 3)).unwrap_or(true);
                    let mut points = match edges.get(&edge) {
                        Some(p) => p.clone(),
                        None => {
                            let p = self.edge_points(edge, approximated)?;
                            edges.insert(edge, p.clone());
                            p
                        }
                    };
                    if !forward {
                        points.reverse();
                    }
                    if out
                        .last()
                        .is_some_and(|l| (l - points[0]).norm() <= self.tol.linear)
                    {
                        points.remove(0);
                    }
                    out.extend(points);
                    if out.len() > MAX_LOOP_VERTICES {
                        return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, "IFCEDGELOOP"));
                    }
                }
                if out.len() > 1 && (out[0] - out[out.len() - 1]).norm() <= self.tol.linear {
                    out.pop();
                }
                if out.len() < 3 {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, "IFCEDGELOOP"));
                }
                Ok(Some(out))
            }
            other => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
    }

    fn vertex_point(&self, name: u64) -> Step<Point3> {
        let record = super::place::expect(self.file, name, "IFCVERTEXPOINT")?;
        let point = attr::reference(
            named(self.file, record, "VertexGeometry", 0),
            "IFCVERTEXPOINT",
        )?;
        self.point(point)
    }

    /// The points of an edge from its start to its end vertex, ends included.
    fn edge_points(&mut self, edge: u64, approximated: &mut bool) -> Step<Vec<Point3>> {
        let file = self.file;
        let record = attr::typed_first(file, edge)
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCEDGE"))?;
        let kind = record.type_name.as_str();
        let a = self.vertex_point(attr::reference(named(file, record, "EdgeStart", 0), kind)?)?;
        let b = self.vertex_point(attr::reference(named(file, record, "EdgeEnd", 1), kind)?)?;
        if kind != "IFCEDGECURVE" {
            return Ok(vec![a, b]);
        }
        let geometry = attr::reference(named(file, record, "EdgeGeometry", 2), kind)?;
        let same = attr::boolean(named(file, record, "SameSense", 3)).unwrap_or(true);
        let target = attr::typed_first(file, geometry)
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCCURVE"))?;
        if target.type_name == "IFCLINE" {
            return Ok(vec![a, b]);
        }
        match self.sampled_curve(geometry, 0) {
            Ok(sampled) => Ok(portion(&sampled, a, b, same)),
            Err(super::Flow::Skip(_)) => {
                *approximated = true;
                Ok(vec![a, b])
            }
            Err(hard) => Err(hard),
        }
    }

    /// The samples of a curve with their parameters.
    fn sampled_curve(&mut self, name: u64, depth: u32) -> Step<Sampled> {
        let file = self.file;
        let record = attr::typed_first(file, name)
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCCURVE"))?;
        let kind = record.type_name.as_str();
        match kind {
            "IFCCIRCLE" | "IFCELLIPSE" => {
                let conic = self.conic(record)?;
                let mut points = Vec::new();
                self.sample_conic(name, &conic, 0.0, TAU, MAX_LOOP_VERTICES, &mut points)?;
                let last = (points.len() - 1).max(1) as f64;
                let samples = points
                    .into_iter()
                    .enumerate()
                    .map(|(k, p)| (TAU * (k as f64) / last, p))
                    .collect();
                Ok(Sampled {
                    samples,
                    period: Some(TAU),
                })
            }
            "IFCTRIMMEDCURVE" => {
                let (points, (t1, t2), _) = self.trimmed_ranged(name, record, MAX_LOOP_VERTICES)?;
                let last = (points.len() - 1).max(1) as f64;
                let samples = points
                    .into_iter()
                    .enumerate()
                    .map(|(k, p)| (t1 + (t2 - t1) * (k as f64) / last, p))
                    .collect();
                Ok(Sampled {
                    samples,
                    period: None,
                })
            }
            "IFCBSPLINECURVEWITHKNOTS" | "IFCRATIONALBSPLINECURVEWITHKNOTS" => {
                let curve = self.bspline_curve(record)?;
                let samples = curve.samples(self.tol.tessellation_chord);
                let (a, b) = curve.domain();
                let closed = samples
                    .first()
                    .zip(samples.last())
                    .is_some_and(|(p, q)| (p.1 - q.1).norm() <= self.tol.linear);
                Ok(Sampled {
                    samples,
                    period: closed.then_some(b - a),
                })
            }
            "IFCSURFACECURVE" | "IFCINTERSECTIONCURVE" | "IFCSEAMCURVE" => {
                if depth > 4 {
                    return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
                }
                let curve = attr::reference(named(file, record, "Curve3D", 0), kind)?;
                self.sampled_curve(curve, depth + 1)
            }
            "IFCPOLYLINE"
            | "IFCCOMPOSITECURVE"
            | "IFCINDEXEDPOLYCURVE"
            | "IFCOFFSETCURVE2D"
            | "IFCOFFSETCURVE3D"
            | "IFCOFFSETCURVEBYDISTANCES" => {
                let points = self.curve_points(name, MAX_LOOP_VERTICES)?;
                let closed = points.len() > 2
                    && points
                        .first()
                        .zip(points.last())
                        .is_some_and(|(p, q)| (p - q).norm() <= self.tol.linear);
                let period = closed.then(|| (points.len() - 1) as f64);
                let samples = points
                    .into_iter()
                    .enumerate()
                    .map(|(k, p)| (k as f64, p))
                    .collect();
                Ok(Sampled { samples, period })
            }
            other => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
    }
}

/// The parameter of the point of the sampled curve nearest to `p`.
fn parameter_of(sampled: &Sampled, p: &Point3) -> f64 {
    let mut best = (f64::MAX, 0.0);
    for w in sampled.samples.windows(2) {
        let line = w[1].1 - w[0].1;
        let len2 = line.norm_squared();
        let t = if len2 > 0.0 {
            ((p - w[0].1).dot(&line) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = (p - (w[0].1 + line * t)).norm();
        if d < best.0 {
            best = (d, w[0].0 + (w[1].0 - w[0].0) * t);
        }
    }
    best.1
}

/// The points from `a` to `b` along the curve in the direction of increasing parameter when `forward`.
fn portion(sampled: &Sampled, a: Point3, b: Point3, forward: bool) -> Vec<Point3> {
    if !forward {
        let mut out = portion(sampled, b, a, true);
        out.reverse();
        return out;
    }
    let (ta, tb) = (parameter_of(sampled, &a), parameter_of(sampled, &b));
    let mut sorted = sampled.samples.clone();
    sorted.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut out = vec![a];
    let span = sorted.last().map_or(0.0, |l| l.0) - sorted.first().map_or(0.0, |f| f.0);
    let eps = 1e-12 * span.abs().max(1.0);
    if tb > ta + eps {
        out.extend(
            sorted
                .iter()
                .filter(|(t, _)| *t > ta && *t < tb)
                .map(|(_, p)| *p),
        );
    } else if let Some(period) = sampled.period {
        let first = sorted.first().map_or(0.0, |f| f.0);
        let last_t = sorted.last().map_or(0.0, |l| l.0);
        let _ = period;
        out.extend(
            sorted
                .iter()
                .filter(|(t, _)| *t > ta && *t < last_t)
                .map(|(_, p)| *p),
        );
        out.extend(
            sorted
                .iter()
                .filter(|(t, _)| *t > first && *t < tb)
                .map(|(_, p)| *p),
        );
    } else {
        out.extend(
            sorted
                .iter()
                .rev()
                .filter(|(t, _)| *t < ta && *t > tb)
                .map(|(_, p)| *p),
        );
    }
    out.push(b);
    out
}

/// A vertex of a ring in parameter space and its point when it is a boundary point.
#[derive(Clone, Copy)]
struct Ring {
    uv: [f64; 2],
    at: Option<Point3>,
}

impl Evaluator<'_> {
    /// The loops of a face in parameter space (unwrapped along each loop, poles expanded) with the
    /// number of periods each winds in `u` and in `v`.
    fn parametrize(
        &mut self,
        surf: &Surf,
        loops: &[Vec<Point3>],
    ) -> Step<Vec<(Vec<Ring>, [i32; 2])>> {
        let (pu, pv) = surf.periods();
        let mut rings: Vec<(Vec<Ring>, [i32; 2])> = Vec::new();
        let mut reference: Option<(f64, f64)> = None;
        for points in loops {
            let mut hint = reference;
            let mut ring: Vec<Ring> = Vec::with_capacity(points.len());
            for p in points {
                self.budget.step()?;
                let (u, v) = surf.uv(p, hint);
                hint = Some((u, v));
                ring.push(Ring {
                    uv: [u, v],
                    at: Some(*p),
                });
            }
            let first = ring[0].uv;
            reference = reference.or(Some((first[0], first[1])));
            let ring = expand_poles(surf, ring);
            let (cu, cv) = surf.uv(&points[0], hint);
            let winding = |closing: f64, start: f64, period: Option<f64>| -> i32 {
                period.map_or(0, |p| {
                    #[allow(clippy::cast_possible_truncation, reason = "a small winding number")]
                    let w = ((closing - start) / p).round() as i32;
                    w
                })
            };
            let w = [winding(cu, first[0], pu), winding(cv, first[1], pv)];
            rings.push((ring, w));
        }
        Ok(rings)
    }

    /// Tessellates a face on a curved surface, as the module documentation describes.
    #[allow(clippy::too_many_lines, reason = "one pass from loops to triangles")]
    fn curved_face(
        &mut self,
        surf: &Surf,
        loops: &[Vec<Point3>],
        sense: bool,
        welder: &mut Welder,
    ) -> Step<()> {
        let kind = "IFCADVANCEDFACE";
        let (pu, pv) = surf.periods();
        let rings = self.parametrize(surf, loops)?;
        // The outer ring is first; the others are shifted by periods to lie near it.
        let winding_rings: Vec<usize> = rings
            .iter()
            .enumerate()
            .filter(|(_, (_, w))| w[0] != 0 || w[1] != 0)
            .map(|(k, _)| k)
            .collect();
        let mut polygons: Vec<Vec<Ring>> = Vec::new();
        if winding_rings.is_empty() {
            let anchor = rings[0].0[0].uv;
            for (k, (ring, _)) in rings.iter().enumerate() {
                let mut ring = ring.clone();
                if k > 0 {
                    for (axis, period) in [(0, pu), (1, pv)] {
                        if let Some(p) = period {
                            let shift =
                                unwrap_near(ring[0].uv[axis], anchor[axis], p) - ring[0].uv[axis];
                            ring.iter_mut().for_each(|r| r.uv[axis] += shift);
                        }
                    }
                }
                polygons.push(ring);
            }
        } else {
            let axis = if rings[winding_rings[0]].1[0] != 0 {
                0
            } else {
                1
            };
            let period = if axis == 0 { pu } else { pv }.unwrap_or(TAU);
            let other = 1 - axis;
            let mut windings: Vec<(Vec<Ring>, i32)> = Vec::new();
            for &k in &winding_rings {
                let (ring, w) = &rings[k];
                if w[other] != 0 || w[axis].abs() != 1 {
                    return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
                }
                windings.push((ring.clone(), w[axis]));
            }
            // Ascending along the winding axis, closed by a copy of the first vertex one period on.
            let ascending: Vec<Vec<Ring>> = windings
                .into_iter()
                .map(|(mut ring, w)| {
                    if w < 0 {
                        ring.reverse();
                        // The reversed ring starts at its last point; make it continue upward.
                        let base = ring[0].uv[axis];
                        let mut previous = base;
                        for r in &mut ring {
                            let mut u = r.uv[axis];
                            while u < previous - 1e-12 {
                                u += period;
                            }
                            while u > previous + period {
                                u -= period;
                            }
                            previous = u;
                            r.uv[axis] = u;
                        }
                    }
                    let first = ring[0];
                    ring.push(Ring {
                        uv: {
                            let mut c = first.uv;
                            c[axis] += period;
                            c
                        },
                        at: first.at,
                    });
                    ring
                })
                .collect();
            let mean = |r: &[Ring]| r.iter().map(|x| x.uv[other]).sum::<f64>() / r.len() as f64;
            let mut ascending = ascending;
            match ascending.len() {
                1 => {
                    let ring = ascending.remove(0);
                    let w = windings_sign(&rings[winding_rings[0]].1, axis);
                    let up = (w > 0) == sense;
                    let Some(pole) = surf.pole(up).filter(|_| axis == 0) else {
                        return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
                    };
                    let (a, z) = (ring[0].uv, ring[ring.len() - 1].uv);
                    let tip = surf.point(a[0], pole);
                    let side = seam(surf, self.tol.tessellation_chord, other, a, [a[0], pole]);
                    let mut polygon = ring;
                    // Up the right side (a copy of the left side's points one period on), across the
                    // pole, and down the left side.
                    for (uv, at) in &side {
                        polygon.push(Ring {
                            uv: [uv[0] + period, uv[1]],
                            at: Some(*at),
                        });
                    }
                    polygon.push(Ring {
                        uv: [z[0], pole],
                        at: Some(tip),
                    });
                    polygon.push(Ring {
                        uv: [a[0], pole],
                        at: Some(tip),
                    });
                    for (uv, at) in side.iter().rev() {
                        polygon.push(Ring {
                            uv: *uv,
                            at: Some(*at),
                        });
                    }
                    polygons.push(polygon);
                }
                2 => {
                    ascending.sort_by(|x, y| mean(x).total_cmp(&mean(y)));
                    let mut high = ascending.remove(1);
                    let low = ascending.remove(0);
                    let shift =
                        unwrap_near(high[0].uv[axis], low[0].uv[axis], period) - high[0].uv[axis];
                    high.iter_mut().for_each(|r| r.uv[axis] += shift);
                    let side = seam(
                        surf,
                        self.tol.tessellation_chord,
                        other,
                        low[0].uv,
                        high[0].uv,
                    );
                    let mut polygon = low;
                    for (uv, at) in &side {
                        polygon.push(Ring {
                            uv: [
                                uv[0] + if axis == 0 { period } else { 0.0 },
                                uv[1] + if axis == 1 { period } else { 0.0 },
                            ],
                            at: Some(*at),
                        });
                    }
                    high.reverse();
                    polygon.extend(high);
                    for (uv, at) in side.iter().rev() {
                        polygon.push(Ring {
                            uv: *uv,
                            at: Some(*at),
                        });
                    }
                    polygons.push(polygon);
                }
                _ => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind)),
            }
            for (k, (ring, _)) in rings.iter().enumerate() {
                if !winding_rings.contains(&k) {
                    polygons.push(ring.clone());
                }
            }
        }
        if polygons.iter().any(|p| p.len() < 3) {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
        for r in polygons.iter().flatten() {
            lo = (lo.0.min(r.uv[0]), lo.1.min(r.uv[1]));
            hi = (hi.0.max(r.uv[0]), hi.1.max(r.uv[1]));
        }
        let (su, sv) = surf.scales(0.5 * (lo.0 + hi.0), 0.5 * (lo.1 + hi.1));
        let mut vertices: Vec<Ring> = Vec::new();
        let mut index_loops: Vec<Vec<u32>> = Vec::new();
        for polygon in &polygons {
            let start = vertices.len();
            index_loops.push((start..start + polygon.len()).map(index_u32).collect());
            vertices.extend(polygon.iter().copied());
        }
        if let Some((us, vs)) = surf.lattice(self.tol.tessellation_chord, lo, hi) {
            if us.len().saturating_mul(vs.len()) > MAX_LATTICE {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            }
            for &u in &us {
                self.budget.step()?;
                for &v in &vs {
                    if inside(&polygons, [u, v]) {
                        vertices.push(Ring {
                            uv: [u, v],
                            at: None,
                        });
                    }
                }
            }
        }
        self.budget.array(vertices.len())?;
        self.budget.charge(
            u64::try_from(vertices.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(super::VERTEX_BYTES * 8),
        )?;
        let points: Vec<Point2> = vertices
            .iter()
            .map(|r| Point2::new(r.uv[0] * su, r.uv[1] * sv))
            .collect();
        let triangles = arc_geom::triangulate_graph(&points, &index_loops, &[])
            .map_err(|_| skip(reasons::GEOMETRY_NOT_EVALUATED, kind))?;
        let position = |i: u32| -> Point3 {
            let r = vertices[i as usize];
            r.at.unwrap_or_else(|| surf.point(r.uv[0], r.uv[1]))
        };
        let flip = !sense ^ surf.reversed();
        for t in triangles {
            let [a, b, c] = t.map(position);
            welder.triangle(&mut self.budget, if flip { [a, c, b] } else { [a, b, c] })?;
        }
        Ok(())
    }
}

/// The ring with each vertex at a pole replaced by the two ends of the degenerate edge there: at the
/// parameter `u` of the vertex before it and of the vertex after it.
fn expand_poles(surf: &Surf, ring: Vec<Ring>) -> Vec<Ring> {
    let n = ring.len();
    let period = surf.periods().0;
    let mut out = Vec::with_capacity(n);
    for (k, r) in ring.iter().enumerate() {
        if !surf.is_pole(r.uv[1]) {
            out.push(*r);
            continue;
        }
        let prev = ring[(k + n - 1) % n].uv[0];
        let next = ring[(k + 1) % n].uv[0];
        let next = period.map_or(next, |p| unwrap_near(next, prev, p));
        out.push(Ring {
            uv: [prev, r.uv[1]],
            at: r.at,
        });
        out.push(Ring {
            uv: [next, r.uv[1]],
            at: r.at,
        });
    }
    out
}

/// The interior points of the seam from `from` to `to` (parameters), spaced by the largest step along
/// `other` that keeps the chord, with their points on the surface.
fn seam(
    surf: &Surf,
    chord: f64,
    other: usize,
    from: [f64; 2],
    to: [f64; 2],
) -> Vec<([f64; 2], Point3)> {
    let span = (to[other] - from[other]).abs();
    let count = surf
        .max_step(chord, other)
        .filter(|d| *d > 1e-4)
        .map_or(1.0, |d| (span / d).ceil().clamp(1.0, 4096.0));
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded above"
    )]
    let count = count as usize;
    (1..count)
        .map(|k| {
            let f = (k as f64) / (count as f64);
            let uv = [
                from[0] + (to[0] - from[0]) * f,
                from[1] + (to[1] - from[1]) * f,
            ];
            (uv, surf.point(uv[0], uv[1]))
        })
        .collect()
}

fn windings_sign(w: &[i32; 2], axis: usize) -> i32 {
    w[axis]
}

/// Whether `p` is inside the polygons by the even-odd rule.
fn inside(polygons: &[Vec<Ring>], p: [f64; 2]) -> bool {
    let mut crossings = 0_u32;
    for polygon in polygons {
        for (k, a) in polygon.iter().enumerate() {
            let b = &polygon[(k + 1) % polygon.len()];
            if (a.uv[1] > p[1]) != (b.uv[1] > p[1]) {
                let t = (p[1] - a.uv[1]) / (b.uv[1] - a.uv[1]);
                if a.uv[0] + t * (b.uv[0] - a.uv[0]) > p[0] {
                    crossings += 1;
                }
            }
        }
    }
    crossings % 2 == 1
}

fn index_u32(k: usize) -> u32 {
    u32::try_from(k).unwrap_or(u32::MAX)
}
