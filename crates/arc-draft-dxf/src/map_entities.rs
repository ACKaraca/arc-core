//! Mapping of entity appearance and of the curve entities: point, line, ray, construction
//! line, circle, arc, ellipse, polylines and splines.

use arc_base::math::{Point2, Vector2};
use arc_draft::types::MAX_LINEWEIGHT_MM;
use arc_geom::{
    Arc2, Circle2, Curve2, Ellipse2, EllipticalArc2, Line2, Nurbs2, Polyline2, PolylineVertex,
    Ray2, Segment2,
};
use arc_io_dxf::model::{LwPolyline, Polyline, Spline};
use arc_io_dxf::{Entity, EntityKind};

use crate::color;
use crate::error::reasons;
use crate::geo::{pt, rad, sweep};
use crate::map_tables::transparency_of;
use crate::mapper::{Built, Mapper};
use crate::plan::{Appearance, By, Shape};

impl Mapper<'_> {
    /// The common fields of `e`; `f` scales its lengths and `z` lists the elevations of its
    /// coordinates (the first one is the entity's elevation unless the entity states one).
    pub(crate) fn appearance(&mut self, e: &Entity, f: f64, z: &[f64]) -> Appearance {
        let p = &e.props;
        let source = e.source_id();
        let layer = self.layer_name(&p.layer, source);
        let color = match (p.true_color, p.color) {
            (Some(rgb), _) => By::Object(color::from_true(rgb)),
            (None, Some(0)) => By::Block,
            (None, Some(i)) => match u8::try_from(i) {
                Ok(i) if i >= 1 => By::Object(color::from_index(i)),
                _ => By::Layer,
            },
            (None, None) => By::Layer,
        };
        let linetype = match p.linetype.as_deref() {
            None => By::Layer,
            Some(n) if n.eq_ignore_ascii_case("bylayer") => By::Layer,
            Some(n) if n.eq_ignore_ascii_case("byblock") => By::Block,
            Some(n) => By::Object(self.linetype_name(n, source)),
        };
        let lineweight = match p.lineweight {
            None | Some(-1) => By::Layer,
            Some(-2) => By::Block,
            Some(w) if w < 0 => By::Object(None),
            Some(w) => By::Object(Some((f64::from(w) / 100.0).min(MAX_LINEWEIGHT_MM))),
        };
        let transparency = match p.transparency {
            None | Some(0) => By::Layer,
            Some(v) if v & 0x0300_0000 == 0x0100_0000 => By::Block,
            Some(v) => transparency_of(i64::from(v)).map_or(By::Layer, By::Object),
        };
        let first = z.first().copied();
        let elevation = p.elevation.or(first).unwrap_or(0.0) * f;
        let first_z = first.unwrap_or(0.0);
        if z.iter()
            .any(|v| (v - first_z).abs() > 1e-9 * (1.0 + v.abs()))
        {
            self.note(reasons::Z_DROPPED, e.kind.type_name(), source);
        }
        Appearance {
            layer,
            color,
            linetype,
            linetype_scale: p
                .linetype_scale
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(1.0),
            lineweight,
            transparency,
            visible: !p.invisible,
            elevation,
            thickness: p.thickness.unwrap_or(0.0) * f,
            extrusion: p
                .extrusion
                .filter(|v| v.iter().any(|c| *c != 0.0))
                .unwrap_or([0.0, 0.0, 1.0]),
        }
    }

    /// The shape of `e`, or `None` after reporting why it is not imported.
    pub(crate) fn map_shape(&mut self, e: &Entity, f: f64) -> Option<Built> {
        let handle = e.source_id();
        match &e.kind {
            EntityKind::Point(p) => Built::with_z(
                Shape::Point(pt([p.location[0], p.location[1]], f)),
                vec![p.location[2]],
            ),
            EntityKind::Line(l) => self.curve(
                e,
                [l.start[2], l.end[2]],
                Segment2::new(pt([l.start[0], l.start[1]], f), pt([l.end[0], l.end[1]], f))
                    .map(Curve2::Segment),
            ),
            EntityKind::Ray(r) => self.curve(
                e,
                [r.start[2], r.start[2]],
                Ray2::new(
                    pt([r.start[0], r.start[1]], f),
                    Vector2::new(r.direction[0], r.direction[1]),
                )
                .map(Curve2::Ray),
            ),
            EntityKind::ConstructionLine(r) => self.curve(
                e,
                [r.start[2], r.start[2]],
                Line2::new(
                    pt([r.start[0], r.start[1]], f),
                    Vector2::new(r.direction[0], r.direction[1]),
                )
                .map(Curve2::Line),
            ),
            EntityKind::Circle(c) => self.curve(
                e,
                [c.center[2], c.center[2]],
                Circle2::new(pt([c.center[0], c.center[1]], f), c.radius * f).map(Curve2::Circle),
            ),
            EntityKind::Arc(a) => {
                let r = self.arc(a, f);
                self.curve(e, [a.center[2], a.center[2]], r)
            }
            EntityKind::Ellipse(el) => {
                let r = self.ellipse(el, f);
                self.curve(e, [el.center[2], el.center[2]], r)
            }
            EntityKind::LwPolyline(p) => self.lw_polyline(e, p, f),
            EntityKind::Polyline(p) => self.polyline(e, p, f),
            EntityKind::Spline(s) => self.spline(e, s, f),
            EntityKind::Text(_) | EntityKind::MText(_) => self.text_shape(e, f),
            EntityKind::Dimension(d) => self.dimension(e, d, f),
            EntityKind::Hatch(h) => self.hatch(e, h, f),
            EntityKind::Wipeout(w) => self.wipeout(e, w, f),
            EntityKind::Insert(_) => None,
            _ => {
                self.note(reasons::ENTITY_UNMAPPED, e.kind.type_name(), handle);
                None
            }
        }
    }

    fn curve(
        &mut self,
        e: &Entity,
        z: [f64; 2],
        result: arc_base::Result<Curve2>,
    ) -> Option<Built> {
        match result {
            Ok(curve) => Built::with_z(Shape::Curve(curve), z.to_vec()),
            Err(_) => {
                self.note(reasons::GEOMETRY_INVALID, e.kind.type_name(), e.source_id());
                None
            }
        }
    }

    fn arc(&self, a: &arc_io_dxf::model::Arc, f: f64) -> arc_base::Result<Curve2> {
        let center = pt([a.center[0], a.center[1]], f);
        let (sw, full) = sweep(
            rad(a.start_angle),
            rad(a.end_angle),
            self.o.tolerance.angular,
        );
        if full {
            Circle2::new(center, a.radius * f).map(Curve2::Circle)
        } else {
            Arc2::new(center, a.radius * f, rad(a.start_angle), sw).map(Curve2::Arc)
        }
    }

    fn ellipse(&self, el: &arc_io_dxf::model::Ellipse, f: f64) -> arc_base::Result<Curve2> {
        let ellipse = Ellipse2::new(
            pt([el.center[0], el.center[1]], f),
            Vector2::new(el.major_axis[0] * f, el.major_axis[1] * f),
            el.ratio,
        )?;
        let (sw, full) = sweep(el.start_param, el.end_param, self.o.tolerance.angular);
        if full {
            Ok(Curve2::Ellipse(ellipse))
        } else {
            EllipticalArc2::new(ellipse, el.start_param, sw).map(Curve2::EllipticalArc)
        }
    }

    fn lw_polyline(&mut self, e: &Entity, p: &LwPolyline, f: f64) -> Option<Built> {
        let constant = p.constant_width.unwrap_or(0.0);
        let vertices = p
            .vertices
            .iter()
            .map(|v| {
                let w = |own: Option<f64>| own.unwrap_or(constant).max(0.0) * f;
                (
                    pt(v.location, f),
                    v.bulge.unwrap_or(0.0),
                    w(v.start_width),
                    w(v.end_width),
                )
            })
            .collect();
        self.finish_polyline(e, vertices, p.flags & 1 != 0, Vec::new())
    }

    fn polyline(&mut self, e: &Entity, p: &Polyline, f: f64) -> Option<Built> {
        if p.flags & (8 | 16 | 64) != 0 {
            self.note(reasons::ENTITY_UNMAPPED, "POLYLINE", e.source_id());
            return None;
        }
        if p.flags & 6 != 0 {
            self.note_what(reasons::VALUE_ADJUSTED, e, "smoothed_polyline");
        }
        let (dw0, dw1) = (p.start_width.unwrap_or(0.0), p.end_width.unwrap_or(0.0));
        let mut z = p.origin.map(|o| vec![o[2]]).unwrap_or_default();
        let mut vertices = Vec::new();
        for child in &p.vertices {
            if let EntityKind::Vertex(v) = &child.kind {
                if v.flags & (1 | 8) != 0 {
                    continue;
                }
                if z.is_empty() {
                    z.push(v.location[2]);
                }
                let w = |own: Option<f64>, default: f64| own.unwrap_or(default).max(0.0) * f;
                vertices.push((
                    pt([v.location[0], v.location[1]], f),
                    v.bulge.unwrap_or(0.0),
                    w(v.start_width, dw0),
                    w(v.end_width, dw1),
                ));
            }
        }
        self.finish_polyline(e, vertices, p.flags & 1 != 0, z)
    }

    /// Builds the polyline from `(point, bulge, start width, end width)` vertices, dropping
    /// repeated points.
    fn finish_polyline(
        &mut self,
        e: &Entity,
        raw: Vec<(Point2, f64, f64, f64)>,
        closed: bool,
        z: Vec<f64>,
    ) -> Option<Built> {
        let mut vertices: Vec<PolylineVertex> = Vec::with_capacity(raw.len());
        for (point, bulge, start_width, end_width) in raw {
            if vertices.last().is_some_and(|last| last.point == point) {
                continue;
            }
            vertices.push(PolylineVertex {
                point,
                bulge,
                start_width,
                end_width,
            });
        }
        if closed
            && vertices.len() > 1
            && vertices.first().map(|v| v.point) == vertices.last().map(|v| v.point)
        {
            vertices.pop();
        }
        match Polyline2::new(vertices, closed) {
            Ok(polyline) => Built::with_z(Shape::Curve(Curve2::Polyline(polyline)), z),
            Err(_) => {
                self.note(reasons::GEOMETRY_INVALID, e.kind.type_name(), e.source_id());
                None
            }
        }
    }

    fn spline(&mut self, e: &Entity, s: &Spline, f: f64) -> Option<Built> {
        let flat = |points: &[[f64; 3]]| -> Vec<Point2> {
            points.iter().map(|p| pt([p[0], p[1]], f)).collect()
        };
        let control = flat(&s.control_points);
        let fit = flat(&s.fit_points);
        let degree = usize::try_from(s.degree).unwrap_or(0);
        let z: Vec<f64> = s
            .control_points
            .iter()
            .chain(s.fit_points.iter())
            .map(|p| p[2])
            .collect();
        let from_control = (!control.is_empty())
            .then(|| {
                let weights = if s.weights.len() == control.len() {
                    s.weights.clone()
                } else {
                    vec![1.0; control.len()]
                };
                Nurbs2::with_limits(
                    degree,
                    s.knots.clone(),
                    control.clone(),
                    weights,
                    &self.o.limits,
                )
                .ok()
            })
            .flatten();
        let curve = from_control.or_else(|| {
            (!fit.is_empty())
                .then(|| Nurbs2::interpolate(&fit, degree.max(1), &self.o.tolerance).ok())
                .flatten()
        });
        match curve {
            Some(curve) => Built::with_z(Shape::Spline { curve, fit }, z),
            None => {
                self.note(reasons::GEOMETRY_INVALID, "SPLINE", e.source_id());
                None
            }
        }
    }
}
