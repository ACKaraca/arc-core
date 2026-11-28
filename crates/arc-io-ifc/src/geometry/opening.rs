//! Openings (CORE-IO-015): the subtraction of the bodies of `IfcOpeningElement`s from the elements
//! they void (`IfcRelVoidsElement`).
//!
//! An element is cut when each of its body items is an `IfcExtrudedAreaSolid` (not tapered) whose
//! profile is one part of straight or discretized edges. The item is rebuilt as a polyhedron of planar
//! faces (the caps of the profile with its holes, one quad per edge), and every opening body that is
//! an `IfcExtrudedAreaSolid` whose direction is perpendicular to its profile is subtracted as a prism
//! with [`arc_geom::Polyhedron::subtract_prism`], which supports a prism that runs between two
//! parallel faces of the solid (a through opening, a notch, an opening at the end of a wall). An
//! opening that stops inside the solid, or that starts or ends exactly on its faces, is a through
//! opening when it spans the solid along its axis (a prism that starts or ends on a face is extended
//! past it by a millimeter), and is otherwise handed to the prism subtraction unchanged. A round edge
//! of an opening is the inscribed polygon of the chord tolerance, so a round opening is never larger
//! than the circle.
//!
//! Whatever the prism subtraction does not do (an opening whose body is not an extrusion or is refused,
//! or an element body of any other kind of solid) is done by the mesh boolean of `arc-mesh`: the body
//! meshes of the opening and the mesh of the element, both in world coordinates, with the opening
//! subtracted; the item is then in world coordinates (identity transform). An opening that neither
//! path can subtract (its body is not evaluated or is not a closed solid, or the result cannot be
//! closed at the tolerance) is reported once as `io.ifc.opening_not_subtracted`; the element is then
//! emitted without that opening. An opening that does not meet the solid at all is skipped without a
//! report.

use std::sync::Arc;

use arc_base::math::{Point2, Point3, Transform3, Vector3};
use arc_geom::curve::Curve2;
use arc_geom::plane3::PlaneFrame;
use arc_geom::primitives::Segment2;
use arc_geom::region::{Loop2, Region, RegionPart};
use arc_geom::solid::{PolyFace, Polyhedron};
use arc_geom::solid_opening::PrismTool;
use arc_mesh::TriMesh;

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;
use crate::schema::access::known;

use super::attr::{self, named, skip};
use super::{Evaluator, GeometryItem, Shape, Step};

/// The most triangles of an element and an opening body in a mesh boolean.
const MAX_BOOLEAN_TRIANGLES: usize = 100_000;

/// One opening body in the coordinates of the solid it cuts.
struct Tool {
    region: Region,
    frame: PlaneFrame,
    depth: f64,
    /// The corner points of the prism, to test whether it meets the solid.
    corners: Vec<Point3>,
    /// For a box, the same body as a prism along each of its other two axes, tried when the
    /// subtraction along the extrusion axis is refused (a window whose body is extruded along the
    /// height but passes through the thickness of the wall).
    alternatives: Vec<Tool>,
}

impl Evaluator<'_> {
    /// The polyhedron of an extruded area solid with one profile part, in the coordinates of its
    /// representation.
    fn extrusion_polyhedron(&mut self, record: &Record) -> Step<Polyhedron> {
        let kind = "IFCEXTRUDEDAREASOLID";
        let file = self.file;
        let parameter = |a| file.attr(record, a);
        let profile = attr::reference(parameter(known::EXTRUSION_AREA), kind)?;
        let position = self.axis3(attr::opt_reference(
            parameter(known::EXTRUSION_POSITION),
            kind,
        )?)?;
        let direction = attr::reference(parameter(known::EXTRUSION_DIRECTION), kind)?;
        let direction = super::place::unit(self.direction(direction)?, kind)?;
        let depth = attr::real(parameter(known::EXTRUSION_DEPTH), self.scale, kind)?;
        if depth <= 0.0 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let mut parts = self.profile(profile)?;
        let (Some(loops), true) = (parts.pop(), parts.is_empty()) else {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        };
        let part = self.prepare_part(loops, kind)?;
        let normal = position.transform_vector(&Vector3::z());
        let along = position.transform_vector(&direction) * depth;
        let up = along.dot(&normal) > 0.0;
        if along.dot(&normal).abs() <= self.tol.angular * depth {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        // Only the points the rings use are vertices (a closing repeat or a collinear point is not).
        let mut used: Vec<usize> = part.rings.iter().flatten().copied().collect();
        used.sort_unstable();
        used.dedup();
        let n = used.len();
        let slot = |k: usize| used.binary_search(&k).unwrap_or(0);
        let mut vertices: Vec<Point3> = used
            .iter()
            .filter_map(|&k| part.points.get(k))
            .map(|p| position.transform_point(&Point3::new(p[0], p[1], 0.0)))
            .collect();
        let top: Vec<Point3> = vertices.iter().map(|p| p + along).collect();
        vertices.extend(top);
        let index = |k: usize| u32::try_from(k).map_err(|_| attr::wrong(kind));
        let mut faces: Vec<PolyFace> = Vec::new();
        for ring in &part.rings {
            for (k, &a) in ring.iter().enumerate() {
                let b = ring.get((k + 1) % ring.len()).copied().unwrap_or(a);
                let (a, b) = (slot(a), slot(b));
                let quad = [index(a)?, index(b)?, index(b + n)?, index(a + n)?];
                faces.push(PolyFace::new(if up {
                    quad.to_vec()
                } else {
                    quad.iter().rev().copied().collect()
                }));
            }
        }
        let ring_ids = |offset: usize, reverse: bool| -> Step<Vec<Vec<u32>>> {
            part.rings
                .iter()
                .map(|r| {
                    let mut ids = r
                        .iter()
                        .map(|&k| index(slot(k) + offset))
                        .collect::<Step<Vec<u32>>>()?;
                    if reverse {
                        ids.reverse();
                    }
                    Ok(ids)
                })
                .collect()
        };
        // The cap whose outward normal is the profile normal has the loops as they are.
        let upper = ring_ids(n, !up)?;
        let lower = ring_ids(0, up)?;
        for loops in [upper, lower] {
            let mut loops = loops.into_iter();
            let Some(outer) = loops.next() else {
                return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
            };
            let mut face = PolyFace::new(outer);
            face.holes = loops.collect();
            faces.push(face);
        }
        Polyhedron::new(vertices, faces, &self.tol, &self.limits)
            .map_err(|_| skip(reasons::GEOMETRY_NOT_EVALUATED, kind))
    }

    /// The prisms of the body of an opening element, in the coordinates of the solid whose world
    /// transform inverse is `to_solid` composed with the opening's own world transform.
    fn opening_tools(
        &mut self,
        shape: u64,
        world: Transform3,
        solid_inverse: &Transform3,
    ) -> Step<Vec<Tool>> {
        let kind = "IFCOPENINGELEMENT";
        let file = self.file;
        let Some(definition) = attr::typed(file, shape, "IFCPRODUCTDEFINITIONSHAPE") else {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        };
        let (representations, _) = attr::distinct_references(
            &mut self.budget,
            file.attr(definition, known::PRODUCT_REPRESENTATIONS),
            kind,
            usize::MAX,
        )?;
        let to_solid = solid_inverse * world;
        let mut tools = Vec::new();
        for representation in representations {
            let Some(rep) = file
                .instance(representation)
                .and_then(|i| i.records.first())
            else {
                continue;
            };
            let identifier = attr::text(file.attr(rep, known::REPRESENTATION_IDENTIFIER), kind)?;
            if identifier.as_deref().is_some_and(|i| i != "Body") {
                continue;
            }
            let (items, _) = attr::distinct_references(
                &mut self.budget,
                file.attr(rep, known::REPRESENTATION_ITEMS),
                kind,
                usize::MAX,
            )?;
            for item in items {
                self.budget.step()?;
                let Some(record) = file.instance(item).and_then(|i| i.records.first()) else {
                    return Err(skip(reasons::TARGET_MISSING, kind));
                };
                if record.type_name != "IFCEXTRUDEDAREASOLID" {
                    return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
                }
                tools.extend(self.extrusion_tools(record, &to_solid)?);
            }
        }
        Ok(tools)
    }

    /// The prisms of one extruded area solid of an opening.
    fn extrusion_tools(&mut self, record: &Record, to_solid: &Transform3) -> Step<Vec<Tool>> {
        let kind = "IFCEXTRUDEDAREASOLID";
        let file = self.file;
        let parameter = |a| file.attr(record, a);
        let profile = attr::reference(parameter(known::EXTRUSION_AREA), kind)?;
        let position = self.axis3(attr::opt_reference(
            parameter(known::EXTRUSION_POSITION),
            kind,
        )?)?;
        let direction = attr::reference(parameter(known::EXTRUSION_DIRECTION), kind)?;
        let direction = super::place::unit(self.direction(direction)?, kind)?;
        let depth = attr::real(parameter(known::EXTRUSION_DEPTH), self.scale, kind)?;
        if depth <= 0.0 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let placed = to_solid * position;
        let normal = placed.transform_vector(&Vector3::z());
        let axis = placed.transform_vector(&direction);
        if normal.cross(&axis).norm() > 1e-6 {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        let u = placed.transform_vector(&Vector3::x());
        let v = axis.cross(&u);
        let origin = placed.transform_point(&Point3::origin());
        let frame = PlaneFrame::new(origin, u, v, &self.tol)
            .map_err(|_| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        let parts = self.profile(profile)?;
        let mut tools = Vec::new();
        for loops in parts {
            let [outline] = loops.as_slice() else {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            };
            let mut flat: Vec<Point2> = Vec::with_capacity(outline.len());
            let mut corners: Vec<Point3> = Vec::new();
            let mut flat3: Vec<Point3> = Vec::new();
            for p in outline {
                let point = placed.transform_point(&Point3::new(p[0], p[1], 0.0));
                let w = point - origin;
                let q = Point2::new(w.dot(&u), w.dot(&v));
                if flat.last().is_none_or(|l| (l - q).norm() > self.tol.linear) {
                    flat.push(q);
                    flat3.push(point);
                    corners.push(point);
                    corners.push(point + axis * depth);
                }
            }
            if flat.len() > 1 && (flat[0] - flat[flat.len() - 1]).norm() <= self.tol.linear {
                flat.pop();
                flat3.pop();
                corners.truncate(corners.len().saturating_sub(2));
            }
            if flat.len() < 3 {
                return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
            }
            let degenerate = || skip(reasons::GEOMETRY_DEGENERATE, kind);
            let mut curves = Vec::with_capacity(flat.len());
            for (k, a) in flat.iter().enumerate() {
                let b = flat.get((k + 1) % flat.len()).copied().unwrap_or(*a);
                curves.push(Curve2::Segment(
                    Segment2::new(*a, b).map_err(|_| degenerate())?,
                ));
            }
            let outline = Loop2::new(curves, &self.tol).map_err(|_| degenerate())?;
            let region = Region::new(vec![RegionPart::new(outline, Vec::new())])
                .map_err(|_| degenerate())?;
            let alternatives = self.box_alternatives(&flat3, axis, depth);
            tools.push(Tool {
                region,
                frame,
                depth,
                corners,
                alternatives,
            });
        }
        Ok(tools)
    }

    /// The prisms along the two axes of the profile of a box body whose profile is the rectangle
    /// `base` (four corners in order) extruded by `depth` along `axis`; empty for any other profile.
    fn box_alternatives(&self, base: &[Point3], axis: Vector3, depth: f64) -> Vec<Tool> {
        let [p0, p1, p2, p3] = base else {
            return Vec::new();
        };
        let (e1, e2) = (p1 - p0, p3 - p0);
        let tolerance = self.tol.linear;
        let square = e1.dot(&e2).abs() <= 1e-9 * e1.norm() * e2.norm()
            && (p2 - (p1 + e2)).norm() <= tolerance;
        if !square || e1.norm() <= tolerance || e2.norm() <= tolerance {
            return Vec::new();
        }
        let span = axis * depth;
        let mut out = Vec::new();
        for (along, other) in [(e1, e2), (e2, e1)] {
            let along_unit = along / along.norm();
            let u = other / other.norm();
            let v = along_unit.cross(&u);
            let Ok(frame) = PlaneFrame::new(*p0, u, v, &self.tol) else {
                continue;
            };
            let (w, h) = (other.norm(), span.dot(&v));
            let flat = [
                Point2::new(0.0, 0.0),
                Point2::new(w, 0.0),
                Point2::new(w, h),
                Point2::new(0.0, h),
            ];
            let mut curves = Vec::with_capacity(4);
            for (k, a) in flat.iter().enumerate() {
                let b = flat[(k + 1) % 4];
                let Ok(segment) = Segment2::new(*a, b) else {
                    return Vec::new();
                };
                curves.push(Curve2::Segment(segment));
            }
            let Ok(outline) = Loop2::new(curves, &self.tol) else {
                continue;
            };
            let Ok(region) = Region::new(vec![RegionPart::new(outline, Vec::new())]) else {
                continue;
            };
            let corners = [*p0, *p1, *p2, *p3]
                .iter()
                .flat_map(|p| [*p, p + span])
                .collect();
            out.push(Tool {
                region,
                frame,
                depth: along.norm(),
                corners,
                alternatives: Vec::new(),
            });
        }
        out
    }

    /// Subtracts the openings of `element` from the items `items[from..]` of the element: the exact
    /// prism subtraction first, and the mesh boolean for every (item, opening) pair it cannot do.
    #[allow(
        clippy::too_many_lines,
        reason = "tools, items and the report of one element"
    )]
    pub(crate) fn subtract_openings(
        &mut self,
        element: u64,
        world: Transform3,
        openings: &[(u64, Option<u64>, u64)],
        items: &mut [GeometryItem],
    ) -> arc_base::Result<()> {
        let file = self.file;
        let candidates: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                i.transform == world
                    && file
                        .instance(i.item)
                        .and_then(|x| x.records.first())
                        .is_some_and(|r| r.type_name == "IFCEXTRUDEDAREASOLID")
            })
            .map(|(k, _)| k)
            .collect();
        let solid_inverse = world.inverse();
        // The (item, opening) pairs the prism subtraction did not do.
        let mut todo: Vec<(usize, usize)> = Vec::new();
        for (k, item) in items.iter().enumerate() {
            if matches!(item.shape.as_ref(), Shape::Mesh(_)) && !candidates.contains(&k) {
                todo.extend((0..openings.len()).map(|o| (k, o)));
            }
        }
        let mut tool_sets: Vec<Option<Vec<Tool>>> = Vec::with_capacity(openings.len());
        for (k, &(_, placement, shape)) in openings.iter().enumerate() {
            self.budget.step()?;
            let transform = match placement {
                Some(p) => self.local_placement(p),
                None => Ok(Transform3::identity()),
            };
            let tools = match transform {
                Ok(t) => self.opening_tools(shape, t, &solid_inverse),
                Err(flow) => Err(flow),
            };
            match tools {
                Ok(t) => tool_sets.push(Some(t)),
                Err(super::Flow::Hard(error)) => return Err(error),
                Err(super::Flow::Skip(_)) => {
                    todo.extend(candidates.iter().map(|&c| (c, k)));
                    tool_sets.push(None);
                }
            }
        }
        for index in candidates {
            let Some(record) = items
                .get(index)
                .and_then(|i| file.instance(i.item))
                .and_then(|i| i.records.first())
            else {
                continue;
            };
            let built = self.extrusion_polyhedron(record);
            let mut solid = match built {
                Ok(s) => s,
                Err(super::Flow::Hard(error)) => return Err(error),
                Err(super::Flow::Skip(_)) => {
                    todo.extend(
                        tool_sets
                            .iter()
                            .enumerate()
                            .filter(|(_, t)| t.is_some())
                            .map(|(k, _)| (index, k)),
                    );
                    continue;
                }
            };
            let mut changed = false;
            let mut number = 0_u32;
            for (k, tools) in tool_sets.iter().enumerate() {
                let Some(tools) = tools else { continue };
                for tool in tools {
                    self.budget.step()?;
                    let mut result = self.apply_tool(&solid, tool, number);
                    if matches!(result, Some(Err(()))) {
                        for alternative in &tool.alternatives {
                            let again = self.apply_tool(&solid, alternative, number);
                            if matches!(again, Some(Ok(_))) {
                                result = again;
                                break;
                            }
                        }
                    }
                    match result {
                        Some(Ok(next)) => {
                            solid = next;
                            changed = true;
                            number = number.saturating_add(1);
                        }
                        Some(Err(())) => todo.push((index, k)),
                        None => {}
                    }
                }
            }
            if changed {
                let mesh = TriMesh::from_polyhedron(&solid, &self.tol, &self.limits)?;
                if let Some(item) = items.get_mut(index) {
                    item.shape = Arc::new(Shape::Mesh(mesh));
                }
            }
        }
        todo.sort_unstable();
        todo.dedup();
        let mut failed: Vec<bool> = vec![false; openings.len()];
        let mut bodies: Vec<Option<Option<Vec<TriMesh>>>> = vec![None; openings.len()];
        for (index, k) in todo {
            self.budget.step()?;
            let Some(&opening) = openings.get(k) else {
                continue;
            };
            if bodies[k].is_none() {
                bodies[k] = Some(self.opening_bodies(opening)?);
            }
            let tools = bodies[k].as_ref().and_then(Option::as_ref);
            let Some(item) = items.get_mut(index) else {
                continue;
            };
            let done = match tools {
                Some(tools) => self.boolean_opening(item, tools)?,
                None => false,
            };
            failed[k] |= !done;
        }
        for (k, bad) in failed.into_iter().enumerate() {
            if bad && let Some(&(opening, _, _)) = openings.get(k) {
                self.log.add(
                    reasons::OPENING_NOT_SUBTRACTED,
                    "IFCRELVOIDSELEMENT",
                    Some(Source::Decimal(opening)),
                    1,
                );
            }
        }
        let _ = element;
        Ok(())
    }

    /// The body meshes of an opening in world coordinates; `None` when it has none that is evaluated.
    fn opening_bodies(
        &mut self,
        (opening, placement, shape): (u64, Option<u64>, u64),
    ) -> arc_base::Result<Option<Vec<TriMesh>>> {
        let Some(world) = self.product_transform(opening, placement)? else {
            return Ok(None);
        };
        let mut found: Vec<GeometryItem> = Vec::new();
        self.product_items(opening, shape, world, &mut found)?;
        let mut out = Vec::new();
        for item in found {
            if item
                .representation_identifier
                .as_deref()
                .is_some_and(|i| i != "Body")
            {
                continue;
            }
            if let Some(mesh) = item.world_mesh(&self.limits)? {
                out.push(mesh);
            }
        }
        Ok((!out.is_empty()).then_some(out))
    }

    /// Subtracts the opening bodies from the mesh of `item` by the mesh boolean, leaving the item in
    /// world coordinates. `Ok(false)` when the boolean is refused; the item is then unchanged.
    fn boolean_opening(
        &mut self,
        item: &mut GeometryItem,
        tools: &[TriMesh],
    ) -> arc_base::Result<bool> {
        let Some(mut current) = item.world_mesh(&self.limits)? else {
            return Ok(false);
        };
        let mut changed = false;
        for tool in tools {
            self.budget.step()?;
            let size = current
                .triangle_count()
                .saturating_add(tool.triangle_count());
            if size > MAX_BOOLEAN_TRIANGLES {
                return Ok(false);
            }
            self.budget.array(size.saturating_mul(4))?;
            self.budget.charge(
                u64::try_from(size)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(8 * (super::VERTEX_BYTES + super::TRIANGLE_BYTES)),
            )?;
            let (lo, hi) = (current.bounding_box(), tool.bounding_box());
            let slack = self.tol.linear;
            let apart = (0..3)
                .any(|a| hi.min()[a] > lo.max()[a] + slack || hi.max()[a] < lo.min()[a] - slack);
            if apart {
                continue;
            }
            match arc_mesh::boolean::boolean(
                &current,
                tool,
                arc_mesh::boolean::BooleanOp::Difference,
                &self.tol,
                &self.limits,
            ) {
                Ok(next) => {
                    current = next;
                    changed = true;
                }
                Err(_) => return Ok(false),
            }
        }
        if changed {
            item.shape = Arc::new(Shape::Mesh(current));
            item.transform = Transform3::identity();
        }
        Ok(true)
    }

    /// Subtracts one prism; `None` when the prism does not meet the solid, `Some(Err)` when the
    /// subtraction is refused.
    fn apply_tool(
        &self,
        solid: &Polyhedron,
        tool: &Tool,
        number: u32,
    ) -> Option<std::result::Result<Polyhedron, ()>> {
        let bounds = solid.bounding_box();
        let (mut lo, mut hi) = (
            Point3::new(f64::MAX, f64::MAX, f64::MAX),
            Point3::new(f64::MIN, f64::MIN, f64::MIN),
        );
        for c in &tool.corners {
            lo = Point3::new(lo.x.min(c.x), lo.y.min(c.y), lo.z.min(c.z));
            hi = Point3::new(hi.x.max(c.x), hi.y.max(c.y), hi.z.max(c.z));
        }
        let slack = self.tol.linear;
        if hi.x < bounds.min().x - slack
            || lo.x > bounds.max().x + slack
            || hi.y < bounds.min().y - slack
            || lo.y > bounds.max().y + slack
            || hi.z < bounds.min().z - slack
            || lo.z > bounds.max().z + slack
        {
            return None;
        }
        // The extent of the solid along the axis of the prism.
        let normal = tool.frame.normal();
        let origin = tool.frame.origin();
        let (mut low, mut high) = (f64::MAX, f64::MIN);
        for v in solid.vertices() {
            let t = (v - origin).dot(&normal);
            low = low.min(t);
            high = high.max(t);
        }
        let (mut start, mut end) = (0.0, tool.depth);
        let margin = (10.0 * self.tol.linear).max(1e-3);
        if start <= low + self.tol.linear && end >= high - self.tol.linear {
            start = start.min(low - margin);
            end = end.max(high + margin);
        }
        let prism = PrismTool {
            profile: &tool.region,
            frame: tool.frame,
            start,
            end,
            chord: self.tol.tessellation_chord,
            opening: number,
            allow_split: false,
        };
        Some(
            solid
                .subtract_prism(&prism, &self.tol, &self.limits)
                .map_err(|_| ()),
        )
    }
}

/// The name of the attribute pair of `IfcRelVoidsElement`: the voided element and the opening.
pub(crate) fn voids(file: &crate::model::IfcFile) -> std::collections::BTreeMap<u64, Vec<u64>> {
    let mut map: std::collections::BTreeMap<u64, Vec<u64>> = std::collections::BTreeMap::new();
    for instance in file.instances_of("IFCRELVOIDSELEMENT") {
        let Some(record) = instance.records.first() else {
            continue;
        };
        let element = attr::reference(named(file, record, "RelatingBuildingElement", 4), "");
        let opening = attr::reference(named(file, record, "RelatedOpeningElement", 5), "");
        if let (Ok(element), Ok(opening)) = (element, opening) {
            map.entry(element).or_default().push(opening);
        }
    }
    map
}
