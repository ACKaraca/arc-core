//! Mapping of hatches, wipeouts and block references.

use std::collections::BTreeSet;

use arc_base::math::{Point2, Vector2};
use arc_draft::hatch_pattern::{MAX_DASHES, MAX_LINES};
use arc_geom::{Circle2, Curve2, Ellipse2, Polyline2, PolylineVertex};
use arc_io_dxf::model::{Hatch, HatchBoundary, HatchEdge, Image, Insert};
use arc_io_dxf::{Entity, EntityKind};

use crate::error::reasons;
use crate::geo::{bulge_of, pt, rad, sweep};
use crate::map_tables::{clean_line, truncate};
use crate::mapper::{Built, Mapper, extra_int};
use crate::plan::{Container, EntitySpec, FamilySpec, HatchSpec, InsertSpec, Shape};

/// A bounded segment of a boundary chain: a line when `bulge` is 0, else an arc.
#[derive(Debug, Clone, Copy)]
struct Seg {
    a: Point2,
    b: Point2,
    bulge: f64,
}

impl Seg {
    fn reversed(self) -> Self {
        Self {
            a: self.b,
            b: self.a,
            bulge: -self.bulge,
        }
    }
}

fn near(p: Point2, q: Point2, tol: f64) -> bool {
    (p.x - q.x).hypot(p.y - q.y) <= tol
}

impl Mapper<'_> {
    pub(crate) fn hatch(&mut self, e: &Entity, h: &Hatch, f: f64) -> Option<Built> {
        let mut loops = Vec::with_capacity(h.loops.len());
        for l in &h.loops {
            let Some(curve) = self.loop_curve(&l.boundary, f) else {
                self.note(reasons::HATCH_DROPPED, "HATCH", e.source_id());
                return None;
            };
            loops.push(curve);
        }
        if loops.is_empty() {
            self.note(reasons::GEOMETRY_INVALID, "HATCH", e.source_id());
            return None;
        }
        let gradient = extra_int(&e.extras, 450).is_some_and(|g| g != 0);
        let mut solid = h.solid != 0 || gradient;
        if gradient {
            self.note_what(reasons::HATCH_APPROXIMATED, e, "gradient");
        }
        let families = if solid {
            Vec::new()
        } else {
            self.families(e, h, f)
        };
        if !solid && families.is_empty() {
            solid = true;
            self.note_what(reasons::HATCH_APPROXIMATED, e, "pattern");
        }
        let island = u32::try_from(h.style.unwrap_or(0)).unwrap_or(0).min(2);
        let spec = HatchSpec {
            solid,
            loops,
            island,
            families,
        };
        Built::with_z(Shape::Hatch(spec), vec![h.elevation_point[2]])
    }

    fn families(&mut self, e: &Entity, h: &Hatch, f: f64) -> Vec<FamilySpec> {
        let mut out = Vec::new();
        for line in &h.pattern_lines {
            if out.len() >= MAX_LINES {
                self.note_what(reasons::HATCH_APPROXIMATED, e, "pattern_lines");
                break;
            }
            let a = rad(line.angle);
            let (sin, cos) = a.sin_cos();
            let shift = (line.offset[0] * cos + line.offset[1] * sin) * f;
            let spacing = (-line.offset[0] * sin + line.offset[1] * cos) * f;
            let origin = pt(line.base, f);
            let limit = 1e6;
            let valid = spacing != 0.0
                && [shift, spacing, origin.x, origin.y]
                    .iter()
                    .all(|v| v.abs() <= limit);
            if !valid {
                self.note_what(reasons::HATCH_APPROXIMATED, e, "pattern_line");
                continue;
            }
            let mut dashes: Vec<f64> = line.dashes.iter().take(MAX_DASHES).map(|d| d * f).collect();
            let total: f64 = dashes.iter().map(|d| d.abs()).sum();
            if dashes.iter().any(|d| d.abs() > limit) || total <= 0.0 {
                dashes.clear();
            }
            out.push(FamilySpec {
                angle: line.angle % 360.0,
                origin,
                shift,
                spacing,
                dashes,
            });
        }
        out
    }

    /// The closed curve a boundary loop stores, `None` when it is not a chain of lines and arcs
    /// or one full circle or ellipse.
    fn loop_curve(&self, boundary: &HatchBoundary, f: f64) -> Option<Curve2> {
        match boundary {
            HatchBoundary::Polyline { vertices, .. } => {
                let raw: Vec<(Point2, f64)> = vertices
                    .iter()
                    .map(|v| (pt(v.location, f), v.bulge))
                    .collect();
                closed_polyline(&raw)
            }
            HatchBoundary::Edges(edges) => self.edge_loop(edges, f),
        }
    }

    fn edge_loop(&self, edges: &[HatchEdge], f: f64) -> Option<Curve2> {
        let ang = self.o.tolerance.angular;
        if let [single] = edges {
            match single {
                HatchEdge::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                    ..
                } if sweep(rad(*start_angle), rad(*end_angle), ang).1 => {
                    return Circle2::new(pt(*center, f), radius * f)
                        .ok()
                        .map(Curve2::Circle);
                }
                HatchEdge::Ellipse {
                    center,
                    major_axis,
                    ratio,
                    start_angle,
                    end_angle,
                    ..
                } if sweep(rad(*start_angle), rad(*end_angle), ang).1 => {
                    return Ellipse2::new(
                        pt(*center, f),
                        Vector2::new(major_axis[0] * f, major_axis[1] * f),
                        *ratio,
                    )
                    .ok()
                    .map(Curve2::Ellipse);
                }
                _ => {}
            }
        }
        let mut segs = Vec::with_capacity(edges.len());
        for edge in edges {
            segs.push(self.segment(edge, f)?);
        }
        chain(&mut segs)
    }

    fn segment(&self, edge: &HatchEdge, f: f64) -> Option<Seg> {
        match edge {
            HatchEdge::Line { start, end } => Some(Seg {
                a: pt(*start, f),
                b: pt(*end, f),
                bulge: 0.0,
            }),
            HatchEdge::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                counterclockwise,
            } => {
                let (s, e) = (rad(*start_angle), rad(*end_angle));
                let (ccw, full) = sweep(s, e, self.o.tolerance.angular);
                if full {
                    return None;
                }
                let signed = if *counterclockwise {
                    ccw
                } else {
                    ccw - std::f64::consts::TAU
                };
                let at = |t: f64| {
                    Point2::new(
                        (center[0] + radius * t.cos()) * f,
                        (center[1] + radius * t.sin()) * f,
                    )
                };
                Some(Seg {
                    a: at(s),
                    b: at(e),
                    bulge: bulge_of(signed),
                })
            }
            _ => None,
        }
    }

    pub(crate) fn wipeout(&mut self, e: &Entity, w: &Image, f: f64) -> Option<Built> {
        let world = |px: f64, py: f64| {
            let (u, v) = (w.u_vector, w.v_vector);
            Point2::new(
                (w.insert[0] + u[0] * (px + 0.5) + v[0] * (py + 0.5)) * f,
                (w.insert[1] + u[1] * (px + 0.5) + v[1] * (py + 0.5)) * f,
            )
        };
        let corners = |a: [f64; 2], b: [f64; 2]| {
            vec![
                world(a[0], a[1]),
                world(b[0], a[1]),
                world(b[0], b[1]),
                world(a[0], b[1]),
            ]
        };
        let points: Vec<Point2> = match (w.clip_type, w.clip_vertices.as_slice()) {
            (Some(1) | None, [a, b]) => corners(*a, *b),
            (_, []) => corners([-0.5, -0.5], [w.size[0] - 0.5, w.size[1] - 0.5]),
            (_, vs) => vs.iter().map(|v| world(v[0], v[1])).collect(),
        };
        let raw: Vec<(Point2, f64)> = points.into_iter().map(|p| (p, 0.0)).collect();
        match closed_polyline(&raw) {
            Some(Curve2::Polyline(p)) => Built::with_z(Shape::Wipeout(p), vec![w.insert[2]]),
            _ => {
                self.note(reasons::GEOMETRY_INVALID, "WIPEOUT", e.source_id());
                None
            }
        }
    }

    pub(crate) fn add_insert(&mut self, e: &Entity, i: &Insert, container: &Container, f: f64) {
        let source = e.source_id();
        let Some(block) = self.blocks.find(&i.block) else {
            self.note(reasons::REFERENCE_UNRESOLVED, "INSERT", source);
            return;
        };
        let block_name = block.name.clone();
        let known: BTreeSet<String> = block
            .attributes
            .iter()
            .map(|a| a.tag.to_lowercase())
            .collect();
        if let Container::Block(owner) = container
            && self.would_cycle(owner, &block_name)
        {
            self.note(reasons::REFERENCE_UNRESOLVED, "INSERT", source);
            return;
        }
        // The extended data belongs to the reference as a whole: it goes with the first item.
        let xdata = self.extended(&e.xdata, "INSERT", source);
        let mut scale = [
            i.x_scale.unwrap_or(1.0),
            i.y_scale.unwrap_or(1.0),
            i.z_scale.unwrap_or(1.0),
        ];
        for s in &mut scale {
            if *s == 0.0 || !s.is_finite() {
                *s = 1.0;
                self.note_what(reasons::VALUE_ADJUSTED, e, "scale");
            }
        }
        let mut attributes = Vec::new();
        for child in &i.attribs {
            if let EntityKind::Attrib(a) = &child.kind {
                if known.contains(&a.tag.to_lowercase()) {
                    attributes.push((a.tag.clone(), truncate(&clean_line(&a.value), 4096)));
                } else {
                    self.note(reasons::ATTRIBUTE_DROPPED, "ATTRIB", child.source_id());
                }
            }
        }
        let rotation = rad(i.rotation.unwrap_or(0.0));
        let appearance = self.appearance(e, f, &[i.insert[2]]);
        let (cols, rows) = self.array_size(e, i);
        let (sin, cos) = rotation.sin_cos();
        let (cs, rs) = (
            i.column_spacing.unwrap_or(0.0),
            i.row_spacing.unwrap_or(0.0),
        );
        for row in 0..rows {
            for col in 0..cols {
                let (dx, dy) = (f64::from(col) * cs, f64::from(row) * rs);
                let position = pt(
                    [
                        i.insert[0] + dx * cos - dy * sin,
                        i.insert[1] + dx * sin + dy * cos,
                    ],
                    f,
                );
                self.rep.mapped(1);
                self.entities.push((
                    container.clone(),
                    EntitySpec {
                        appearance: appearance.clone(),
                        shape: Shape::Insert(InsertSpec {
                            block: block_name.clone(),
                            position,
                            scale,
                            rotation,
                            attributes: attributes.clone(),
                        }),
                        source: source.filter(|_| row == 0 && col == 0),
                        xdata: if row == 0 && col == 0 {
                            xdata.clone()
                        } else {
                            Vec::new()
                        },
                    },
                ));
            }
        }
    }

    /// The columns and rows of an insert array, with the product bounded by the array limit.
    fn array_size(&mut self, e: &Entity, i: &Insert) -> (u32, u32) {
        let cols = u32::try_from(i.columns.unwrap_or(1).max(1)).unwrap_or(1);
        let rows = u32::try_from(i.rows.unwrap_or(1).max(1)).unwrap_or(1);
        if cols == 1 && rows == 1 {
            return (1, 1);
        }
        self.note(reasons::ARRAY_EXPANDED, "INSERT", e.source_id());
        let cap = u32::try_from(self.o.limits.array_length.min(100_000))
            .unwrap_or(100_000)
            .max(1);
        let rows = rows.min(cap);
        (cols.min(cap / rows).max(1), rows)
    }

    /// Whether a reference from block `owner` to `target` would make a block contain itself;
    /// otherwise the edge is recorded.
    fn would_cycle(&mut self, owner: &str, target: &str) -> bool {
        let (owner, target) = (owner.to_lowercase(), target.to_lowercase());
        let mut stack = vec![target.clone()];
        let mut seen = BTreeSet::new();
        while let Some(at) = stack.pop() {
            if at == owner {
                return true;
            }
            if seen.insert(at.clone())
                && let Some(next) = self.edges.get(&at)
            {
                stack.extend(next.iter().cloned());
            }
        }
        self.edges.entry(owner).or_default().insert(target);
        false
    }
}

/// A closed polyline through `raw` vertices (point, bulge), repeated points removed.
fn closed_polyline(raw: &[(Point2, f64)]) -> Option<Curve2> {
    let mut vertices: Vec<PolylineVertex> = Vec::with_capacity(raw.len());
    for (point, bulge) in raw {
        if vertices.last().is_some_and(|v| v.point == *point) {
            continue;
        }
        vertices.push(PolylineVertex::new(*point, *bulge));
    }
    if vertices.len() > 1 && vertices.first().map(|v| v.point) == vertices.last().map(|v| v.point) {
        vertices.pop();
    }
    Polyline2::new(vertices, true).ok().map(Curve2::Polyline)
}

/// Orders `segs` into a closed chain (reversing a segment that runs backwards) and builds the
/// polyline; `None` when the segments do not close.
fn chain(segs: &mut [Seg]) -> Option<Curve2> {
    let extent = segs
        .iter()
        .flat_map(|s| [s.a.x.abs(), s.a.y.abs(), s.b.x.abs(), s.b.y.abs()])
        .fold(1.0_f64, f64::max);
    let tol = 1e-6 * extent;
    for i in 1..segs.len() {
        let end = segs.get(i - 1)?.b;
        let next = *segs.get(i)?;
        if !near(end, next.a, tol) && near(end, next.b, tol) {
            *segs.get_mut(i)? = next.reversed();
        }
        if !near(end, segs.get(i)?.a, tol) {
            return None;
        }
    }
    if !near(segs.last()?.b, segs.first()?.a, tol) {
        return None;
    }
    let raw: Vec<(Point2, f64)> = segs.iter().map(|s| (s.a, s.bulge)).collect();
    closed_polyline(&raw)
}
