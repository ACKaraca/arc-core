//! Export of the entities of a space or block definition.

use std::f64::consts::TAU;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Limits, ObjectId, Result};
use arc_doc::relationship::ReferenceField;
use arc_doc::{ColorSource, Value};
use arc_draft::array::ARRAY;
use arc_draft::dimension::{DefinitionPoint, Kind as DimensionKind};
use arc_draft::display::DisplayOptions;
use arc_draft::order::sorted;
use arc_draft::text::{Paragraph, Stack, paragraphs_of};
use arc_draft::types::{self, BLOCK_REF, ENTITIES, POINT, SPLINE, WIPEOUT};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Nurbs2, Polyline2};
use arc_io_dxf::model::{
    Arc, AttDef, Attrib, Circle, Dimension, Ellipse, EntityProps, Hatch, HatchBoundary, HatchEdge,
    HatchLoop, HatchVertex, Image, Insert, Line, LwPolyline, LwVertex, MText, PatternLine, Point,
    Ray, Spline, Text,
};
use arc_io_dxf::{Entity, EntityKind, Handle, PreservedRecord};

use crate::color::to_true;
use crate::error::reasons;
use crate::export::{Exporter, alpha_of, invalid, lineweight_code, note_what};
use crate::fields::Reader;
use crate::plan::{FOREIGN_ENTITY, decode_record};
use crate::records;

fn p3(p: Point2, z: f64) -> [f64; 3] {
    [p.x, p.y, z]
}

fn point_of(v: Option<&Value>) -> Option<Point2> {
    match v? {
        Value::Point2(p) => Some(*p),
        _ => None,
    }
}

impl Exporter<'_> {
    /// The entities owned by `container`, in draw order, with `owner` as their owner handle.
    pub(crate) fn container_entities(
        &mut self,
        container: ObjectId,
        owner: Handle,
    ) -> Result<Vec<Entity>> {
        let snapshot = self.snapshot;
        let mut members = Vec::new();
        for r in snapshot.referrers(container) {
            if r.field != ReferenceField::Owner {
                continue;
            }
            let handle = snapshot.resolve(r.source)?;
            let Some(key) = snapshot.type_key(handle) else {
                continue;
            };
            if !ENTITIES.contains(&key.as_str()) {
                self.rep
                    .note(reasons::EXPORT_UNMAPPED, key.as_str(), None, &[]);
                continue;
            }
            let object = snapshot.get(handle)?;
            let order = Reader::new(snapshot.registry(), key.as_str(), &object.values);
            let order = match order.get("draw_order") {
                Some(Value::I64(k)) => *k,
                _ => 0,
            };
            members.push((r.source, order));
        }
        let mut out = Vec::new();
        for (id, _) in sorted(&members) {
            let handle = snapshot.resolve(id)?;
            let Some(key) = snapshot.type_key(handle) else {
                continue;
            };
            let object = snapshot.get(handle)?;
            let reader = Reader::new(snapshot.registry(), key.as_str(), &object.values);
            if key.as_str() == ARRAY {
                out.extend(self.array_inserts(&reader, owner, id)?);
                continue;
            }
            if let Some(entity) = self.entity(key.as_str(), &reader, owner, id)? {
                self.count += 1;
                self.rep.mapped(1);
                out.push(entity);
            }
        }
        for mut record in self.preserved_records(container, FOREIGN_ENTITY) {
            self.claim_record(&mut record);
            // The record's owner is the container it is written to now. The owner position skips
            // the groups of application data, so a reactor inside them keeps its number.
            if let Some(i) = records::owner_position(&record.groups)
                && let Some(group) = record.groups.get_mut(i)
            {
                group.value = arc_io_dxf::Value::Text(owner.to_hex());
            }
            self.count += 1;
            out.push(Entity::new(EntityKind::Preserved(record)));
        }
        Ok(out)
    }

    /// The block references that write the array `object`: one per item, in item order, each at
    /// the item's placement and with the array's scale, rotation and layer properties. The
    /// placements are those of the display list ([`arc_draft::array::expansion`]), with the
    /// tolerance of the default display options. The first reference takes the array's bound
    /// handle when the policy keeps it; the others get new handles. The array's definition
    /// (counts, spacings, path) is not written: it is reported once (`draft.dxf.array_approximated`,
    /// with the number of references). An array whose block is missing, or whose placements the
    /// display cannot compute, is reported as unmapped and writes nothing.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` (argument `limit` = `record_count`) when the items would exceed the
    /// export's array budget.
    fn array_inserts(
        &mut self,
        r: &Reader<'_>,
        owner: Handle,
        object: ObjectId,
    ) -> Result<Vec<Entity>> {
        let tolerance = DisplayOptions::default().tolerance;
        let expansion = match arc_draft::array::expansion(self.snapshot, object, &tolerance) {
            Ok(expansion) => expansion,
            Err(_) => {
                self.rep.note(reasons::EXPORT_UNMAPPED, ARRAY, None, &[]);
                return Ok(Vec::new());
            }
        };
        let name = expansion
            .block
            .and_then(|block| self.blocks.get(&block).cloned());
        let Some(name) = name else {
            self.rep.note(reasons::EXPORT_UNMAPPED, ARRAY, None, &[]);
            return Ok(Vec::new());
        };
        let count = u64::try_from(expansion.items.len()).unwrap_or(u64::MAX);
        if count > self.array_budget {
            return Err(Error::new(arc_io_dxf::codes::LIMIT_EXCEEDED)
                .with_arg("limit", "record_count")
                .with_detail("value", i64::try_from(count).unwrap_or(i64::MAX)));
        }
        self.array_budget -= count;
        let z = r.f64_or("elevation", 0.0);
        let scale = expansion.scale;
        let scale = [scale.x, scale.y, scale.z];
        let props = self.props(r, false);
        let mut out = Vec::with_capacity(expansion.items.len());
        let mut first = None;
        for (i, item) in expansion.items.iter().enumerate() {
            let fresh = self.handle();
            // Only the first reference may take the array's bound handle.
            let handle = if i == 0 {
                self.handle_for(fresh, Some(object), "ARRAY")
            } else {
                fresh
            };
            first.get_or_insert(handle);
            self.count += 1;
            out.push(Entity {
                head: arc_io_dxf::model::Head {
                    handle: Some(handle),
                    owner: Some(owner),
                    app_groups: Vec::new(),
                },
                props: props.clone(),
                kind: EntityKind::Insert(Insert {
                    block: name.clone(),
                    insert: p3(item.position, z),
                    x_scale: Some(scale[0]).filter(|s| *s != 1.0),
                    y_scale: Some(scale[1]).filter(|s| *s != 1.0),
                    z_scale: Some(scale[2]).filter(|s| *s != 1.0),
                    rotation: Some(item.rotation.to_degrees()).filter(|a| *a != 0.0),
                    ..Insert::default()
                }),
                extras: Vec::new(),
                // The array's extended data goes with its first reference.
                xdata: if i == 0 {
                    self.xdata_of(object)
                } else {
                    Vec::new()
                },
            });
        }
        if let Some(handle) = first {
            self.written.push((object, handle));
            self.rep.mapped(1);
            self.rep.note_n(
                reasons::ARRAY_APPROXIMATED,
                ARRAY,
                Some(handle.to_hex()),
                count,
                &[],
            );
        }
        Ok(out)
    }

    /// The records of the foreign data rows of format `dxf` and kind `kind` attached to
    /// `container`, in row order; a row that does not decode is reported and skipped.
    pub(crate) fn preserved_records(
        &mut self,
        container: ObjectId,
        kind: &'static str,
    ) -> Vec<PreservedRecord> {
        let rows = self
            .foreign
            .get(&(container, kind))
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for row in rows {
            // A record the writer refuses in the output form is not written, as a header is not.
            let record = decode_record(&row.data).filter(|r| {
                r.groups
                    .iter()
                    .all(|g| arc_io_dxf::value_writable(g, self.binary))
                    && r.groups
                        .iter()
                        .filter(|g| g.code == 1001)
                        .all(|g| g.as_text().is_some_and(records::valid_name))
            });
            if let Some(record) = record {
                self.rep.mapped(1);
                out.push(record);
            } else {
                self.rep
                    .note(reasons::EXPORT_UNMAPPED, "foreign data", None, &[]);
            }
        }
        out
    }

    fn entity(
        &mut self,
        key: &str,
        r: &Reader<'_>,
        owner: Handle,
        object: ObjectId,
    ) -> Result<Option<Entity>> {
        // The bound handle is claimed only when the entity is written.
        let fresh = self.handle();
        let wanted = self.bound.get(&object).copied();
        let claimable = wanted.filter(|w| !self.taken.contains(&w.0));
        let handle = claimable.unwrap_or(fresh);
        let z = r.f64_or("elevation", 0.0);
        let kind = match key {
            POINT => point_of(r.get("position")).map(|p| {
                EntityKind::Point(Point {
                    location: p3(p, z),
                    angle: None,
                })
            }),
            SPLINE => Some(EntityKind::Spline(self.spline(r, z)?)),
            types::WIPEOUT => Some(self.wipeout(r, z)?),
            BLOCK_REF => self.insert(r, z, handle)?,
            arc_draft::text::TEXT => Some(self.text(r, z)),
            arc_draft::text::RICH_TEXT => Some(self.rich_text(r, z)),
            arc_draft::dimension::DIMENSION => self.dimension(r, z),
            arc_draft::hatch::HATCH => self.hatch(r, z)?,
            types::LINE
            | types::RAY
            | types::CONSTRUCTION_LINE
            | types::CIRCLE
            | types::ARC
            | types::ELLIPSE
            | types::ELLIPTICAL_ARC
            | types::POLYLINE => self.curve(r, z)?,
            _ => None,
        };
        let Some(kind) = kind else {
            if !matches!(
                key,
                BLOCK_REF | arc_draft::dimension::DIMENSION | arc_draft::hatch::HATCH
            ) {
                self.rep.note(reasons::EXPORT_UNMAPPED, key, None, &[]);
            }
            return Ok(None);
        };
        let props = self.props(r, matches!(kind, EntityKind::LwPolyline(_)));
        match wanted {
            Some(w) if claimable.is_some() => {
                self.taken.insert(w.0);
            }
            Some(w) => self
                .rep
                .note(reasons::HANDLE_REASSIGNED, "ENTITY", Some(w.to_hex()), &[]),
            None => {}
        }
        self.written.push((object, handle));
        Ok(Some(Entity {
            head: arc_io_dxf::model::Head {
                handle: Some(handle),
                owner: Some(owner),
                app_groups: Vec::new(),
            },
            props,
            kind,
            extras: Vec::new(),
            xdata: self.xdata_of(object),
        }))
    }

    pub(crate) fn props(&self, r: &Reader<'_>, lightweight: bool) -> EntityProps {
        let name = |id: Option<ObjectId>, map: &std::collections::BTreeMap<ObjectId, String>| {
            id.and_then(|i| map.get(&i).cloned())
        };
        let mut p = EntityProps {
            layer: name(r.reference("layer"), &self.layers).unwrap_or_else(|| "0".to_owned()),
            ..EntityProps::default()
        };
        match r.enum_code("color_by") {
            Some(1) => p.color = Some(0),
            Some(2) => match r.get("color") {
                Some(Value::Color(c)) => match c.source {
                    Some(ColorSource::Indexed(n)) => p.color = Some(i16::from(n)),
                    _ => p.true_color = Some(to_true(c)),
                },
                _ => p.color = Some(7),
            },
            _ => {}
        }
        match r.enum_code("linetype_by") {
            Some(1) => p.linetype = Some("ByBlock".to_owned()),
            Some(2) => {
                p.linetype = Some(
                    name(r.reference("linetype"), &self.linetypes)
                        .unwrap_or_else(|| "Continuous".to_owned()),
                );
            }
            _ => {}
        }
        match r.enum_code("lineweight_by") {
            Some(1) => p.lineweight = Some(-2),
            Some(2) => p.lineweight = Some(lineweight_code(r.f64("lineweight"))),
            _ => {}
        }
        match r.enum_code("transparency_by") {
            Some(1) => p.transparency = Some(0x0100_0000),
            Some(2) => {
                p.transparency =
                    Some(0x0200_0000 | i32::from(alpha_of(r.f64_or("transparency", 0.0))))
            }
            _ => {}
        }
        let scale = r.f64_or("linetype_scale", 1.0);
        p.linetype_scale = (scale != 1.0).then_some(scale);
        p.invisible = !r.bool_or("visible", true);
        p.thickness = Some(r.f64_or("thickness", 0.0)).filter(|t| *t != 0.0);
        p.elevation = lightweight
            .then(|| r.f64_or("elevation", 0.0))
            .filter(|e| *e != 0.0);
        if let Some(Value::Vector3(v)) = r.get("extrusion")
            && (v.x, v.y, v.z) != (0.0, 0.0, 1.0)
        {
            p.extrusion = Some([v.x, v.y, v.z]);
        }
        p
    }

    fn geometry<T: GeomCodec>(&self, r: &Reader<'_>) -> Result<T> {
        match r.get("geometry") {
            Some(Value::Geometry(b)) => {
                T::decode(&b.0, &Limits::DEFAULT).map_err(|e| invalid("geometry").with_source(e))
            }
            _ => Err(invalid("geometry")),
        }
    }

    fn curve(&mut self, r: &Reader<'_>, z: f64) -> Result<Option<EntityKind>> {
        let curve: Curve2 = self.geometry(r)?;
        Ok(Some(match curve {
            Curve2::Segment(s) => EntityKind::Line(Line {
                start: p3(s.start(), z),
                end: p3(s.end(), z),
            }),
            Curve2::Ray(ray) => EntityKind::Ray(Ray {
                start: p3(ray.origin(), z),
                direction: [ray.direction().x, ray.direction().y, 0.0],
            }),
            Curve2::Line(line) => EntityKind::ConstructionLine(Ray {
                start: p3(line.point(), z),
                direction: [line.direction().x, line.direction().y, 0.0],
            }),
            Curve2::Circle(c) => EntityKind::Circle(Circle {
                center: p3(c.center(), z),
                radius: c.radius(),
            }),
            Curve2::Arc(a) => {
                let (start, end) = ccw_range(a.start_angle(), a.sweep());
                EntityKind::Arc(Arc {
                    center: p3(a.center(), z),
                    radius: a.radius(),
                    start_angle: start.to_degrees(),
                    end_angle: end.to_degrees(),
                })
            }
            Curve2::Ellipse(e) => EntityKind::Ellipse(ellipse(&e, z, 0.0, TAU)),
            Curve2::EllipticalArc(a) => {
                let (start, end) = ccw_range(a.start_parameter(), a.sweep());
                EntityKind::Ellipse(ellipse(a.ellipse(), z, start, end))
            }
            Curve2::Polyline(p) => EntityKind::LwPolyline(lw_polyline(&p)),
            Curve2::Nurbs(_) => return Ok(None),
        }))
    }

    fn spline(&mut self, r: &Reader<'_>, z: f64) -> Result<Spline> {
        let curve: Nurbs2 = self.geometry(r)?;
        let fit = match r.get("fit_points") {
            Some(Value::List(list)) => list.iter().filter_map(|v| point_of(Some(v))).collect(),
            _ => Vec::new(),
        };
        let rational = curve.is_rational();
        Ok(Spline {
            flags: 8 | if rational { 4 } else { 0 },
            degree: i16::try_from(curve.degree()).unwrap_or(3),
            knots: curve.knots().to_vec(),
            weights: if rational {
                curve.weights().to_vec()
            } else {
                Vec::new()
            },
            control_points: curve.control_points().iter().map(|p| p3(*p, z)).collect(),
            fit_points: Vec::<Point2>::into_iter(fit).map(|p| p3(p, z)).collect(),
            ..Spline::default()
        })
    }

    fn style_name(&self, r: &Reader<'_>) -> Option<String> {
        r.reference("style")
            .and_then(|s| self.text_styles.get(&s).cloned())
    }

    fn text(&mut self, r: &Reader<'_>, z: f64) -> EntityKind {
        let at = point_of(r.get("position")).unwrap_or_else(Point2::origin);
        let (h, v) = (
            r.enum_code("halign").unwrap_or(0),
            r.enum_code("valign").unwrap_or(0),
        );
        let aligned = h != 0 || v != 0;
        let height = r.f64_or("height", 0.0);
        EntityKind::Text(Text {
            insert: p3(at, z),
            height: if height > 0.0 {
                height
            } else {
                arc_draft::text::DEFAULT_HEIGHT
            },
            value: r.text("content").unwrap_or_default().to_owned(),
            rotation: Some(r.f64_or("rotation", 0.0).to_degrees()).filter(|a| *a != 0.0),
            style: self.style_name(r),
            horizontal: aligned.then_some(i16::try_from(h).unwrap_or(0)),
            vertical: aligned.then_some(i16::try_from(v).unwrap_or(0)),
            align: aligned.then(|| p3(at, z)),
            ..Text::default()
        })
    }

    fn rich_text(&mut self, r: &Reader<'_>, z: f64) -> EntityKind {
        let at = point_of(r.get("position")).unwrap_or_else(Point2::origin);
        let paragraphs: Vec<Paragraph> = match r.get("paragraphs") {
            Some(Value::List(items)) => items.iter().filter_map(Paragraph::from_value).collect(),
            _ => Vec::new(),
        };
        let _ = paragraphs_of;
        let mut formatted = false;
        let text = paragraphs
            .iter()
            .map(|p| {
                formatted |= p.alignment != arc_draft::text::Alignment::Left
                    || p.indent != 0.0
                    || p.indent_first != 0.0;
                p.runs
                    .iter()
                    .map(|run| {
                        formatted |= run.bold
                            || run.italic
                            || run.underline
                            || run.overline
                            || run.strike
                            || run.color.is_some()
                            || run.font.is_some()
                            || run.height.is_some();
                        let body = escape(&run.text);
                        match (&run.stack, &run.denominator) {
                            (Stack::None, _) | (_, None) => body,
                            (_, Some(d)) => {
                                formatted = true;
                                format!("\\S{body}/{};", escape(d))
                            }
                        }
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\\P");
        if formatted {
            note_what(
                &mut self.rep,
                reasons::EXPORT_APPROXIMATED,
                "core.draft.rich_text",
                "text_formatting",
            );
        }
        let height = r.f64_or("height", 0.0);
        EntityKind::MText(MText {
            insert: p3(at, z),
            height: if height > 0.0 {
                height
            } else {
                arc_draft::text::DEFAULT_HEIGHT
            },
            width: Some(r.f64_or("width", 0.0)).filter(|w| *w > 0.0),
            attachment: Some(
                i16::try_from(r.enum_code("attachment").unwrap_or(0) + 1).unwrap_or(1),
            ),
            style: self.style_name(r),
            rotation: Some(r.f64_or("rotation", 0.0).to_degrees()).filter(|a| *a != 0.0),
            text,
            ..MText::default()
        })
    }

    fn wipeout(&mut self, r: &Reader<'_>, z: f64) -> Result<EntityKind> {
        let polyline: Polyline2 = self.geometry(r)?;
        if r.bool_or("frame", false) {
            note_what(
                &mut self.rep,
                reasons::EXPORT_APPROXIMATED,
                WIPEOUT,
                "wipeout_frame",
            );
        }
        let shift = |p: Point2| [p.x - 0.5, p.y - 0.5];
        let mut clip: Vec<[f64; 2]> = polyline.vertices().iter().map(|v| shift(v.point)).collect();
        if let Some(first) = clip.first().copied() {
            clip.push(first);
        }
        Ok(EntityKind::Wipeout(Image {
            class_version: Some(0),
            insert: [0.0, 0.0, z],
            u_vector: [1.0, 0.0, 0.0],
            v_vector: [0.0, 1.0, 0.0],
            size: [1.0, 1.0],
            display: Some(7),
            clipping: Some(1),
            brightness: Some(50),
            contrast: Some(50),
            fade: Some(0),
            clip_type: Some(2),
            clip_vertices: clip,
            ..Image::default()
        }))
    }

    fn dimension(&mut self, r: &Reader<'_>, z: f64) -> Option<EntityKind> {
        let kind = DimensionKind::of(r.enum_code("kind")?)?;
        let points: Vec<DefinitionPoint> = match r.get("points") {
            Some(Value::List(items)) => items
                .iter()
                .filter_map(DefinitionPoint::from_value)
                .collect(),
            _ => Vec::new(),
        };
        if points
            .iter()
            .any(|p| p.key != arc_draft::dimension::Key::None)
        {
            note_what(
                &mut self.rep,
                reasons::EXPORT_APPROXIMATED,
                "core.draft.dimension",
                "dimension_association",
            );
        }
        let pts: Vec<Point2> = points.iter().map(|p| p.position).collect();
        let line = point_of(r.get("line_position"))?;
        let text_position = point_of(r.get("text_position"));
        let mut d = dimension_data(kind, &pts, line, r.f64_or("rotation", 0.0), z)?;
        if let Some(t) = text_position {
            d.text_mid = p3(t, z);
            d.kind |= 128;
        }
        d.style = r
            .reference("style")
            .and_then(|s| self.dim_styles.get(&s).cloned())
            .or_else(|| Some("Standard".to_owned()));
        d.text = r
            .text("text_override")
            .map(|t| t.replace("{}", "<>"))
            .filter(|t| !t.is_empty());
        d.attachment = Some(5);
        Some(EntityKind::Dimension(d))
    }

    fn hatch(&mut self, r: &Reader<'_>, z: f64) -> Result<Option<EntityKind>> {
        let mut loops = Vec::new();
        if let Some(Value::List(items)) = r.get("loops") {
            for (i, item) in items.iter().enumerate() {
                let Value::Geometry(b) = item else { continue };
                let curve = Curve2::decode(&b.0, &Limits::DEFAULT)
                    .map_err(|e| invalid("hatch loop").with_source(e))?;
                loops.push(hatch_loop(&curve, i == 0));
            }
        }
        let kind = r.enum_code("kind").unwrap_or(0);
        if kind == 2 {
            note_what(
                &mut self.rep,
                reasons::EXPORT_APPROXIMATED,
                arc_draft::hatch::HATCH,
                "gradient",
            );
        }
        let (name, lines) = if kind == 0 {
            self.pattern_lines(r)
        } else {
            ("SOLID".to_owned(), Vec::new())
        };
        let scale = r.f64_or("scale", 1.0);
        let angle = r.f64_or("angle", 0.0);
        let origin = point_of(r.get("origin")).unwrap_or_else(Point2::origin);
        let solid = kind != 0 || lines.is_empty();
        Ok(Some(EntityKind::Hatch(Hatch {
            elevation_point: [0.0, 0.0, z],
            pattern_name: if solid { "SOLID".to_owned() } else { name },
            solid: i16::from(solid),
            associative: 0,
            style: Some(i16::try_from(r.enum_code("island").unwrap_or(0)).unwrap_or(0)),
            pattern_type: Some(1),
            pattern_angle: Some(angle.to_degrees()),
            pattern_scale: Some(scale),
            pattern_double: Some(0),
            pixel_size: None,
            loops,
            pattern_lines: if solid {
                Vec::new()
            } else {
                lines
                    .iter()
                    .map(|l| final_line(l, scale, angle, origin))
                    .collect()
            },
            seeds: Vec::new(),
        })))
    }

    /// The pattern name and line families of a pattern hatch: the inline families, or those of
    /// the pattern object it references.
    fn pattern_lines(&self, r: &Reader<'_>) -> (String, Vec<Value>) {
        let lines_of = |v: Option<&Value>| match v {
            Some(Value::List(items)) => items.clone(),
            _ => Vec::new(),
        };
        let inline = lines_of(r.get("inline"));
        if !inline.is_empty() {
            return ("USER".to_owned(), inline);
        }
        let Some(pattern) = r.reference("pattern") else {
            return (String::new(), Vec::new());
        };
        let Ok(handle) = self.snapshot.resolve(pattern) else {
            return (String::new(), Vec::new());
        };
        let Ok(object) = self.snapshot.get(handle) else {
            return (String::new(), Vec::new());
        };
        let reader = Reader::new(
            self.snapshot.registry(),
            arc_draft::hatch::HATCH_PATTERN,
            &object.values,
        );
        (
            reader.text("name").unwrap_or("USER").to_owned(),
            lines_of(reader.get("lines")),
        )
    }

    fn insert(&mut self, r: &Reader<'_>, z: f64, handle: Handle) -> Result<Option<EntityKind>> {
        let Some(block) = r.reference("block") else {
            return Ok(None);
        };
        let Some(name) = self.blocks.get(&block).cloned() else {
            return Ok(None);
        };
        let position = point_of(r.get("position")).unwrap_or_else(Point2::origin);
        let scale = match r.get("scale") {
            Some(Value::Vector3(s)) => [s.x, s.y, s.z],
            _ => [1.0; 3],
        };
        let rotation = r.f64_or("rotation", 0.0);
        let attribs = self.attribs(r, block, handle, (position, scale, rotation), z)?;
        Ok(Some(EntityKind::Insert(Insert {
            block: name,
            insert: p3(position, z),
            x_scale: Some(scale[0]).filter(|s| *s != 1.0),
            y_scale: Some(scale[1]).filter(|s| *s != 1.0),
            z_scale: Some(scale[2]).filter(|s| *s != 1.0),
            rotation: Some(rotation.to_degrees()).filter(|a| *a != 0.0),
            attribs,
            ..Insert::default()
        })))
    }

    fn attribs(
        &mut self,
        r: &Reader<'_>,
        block: ObjectId,
        owner: Handle,
        (position, scale, rotation): (Point2, [f64; 3], f64),
        z: f64,
    ) -> Result<Vec<Entity>> {
        let Some(Value::Map(values)) = r.get("attributes") else {
            return Ok(Vec::new());
        };
        let object = self.snapshot.get(self.snapshot.resolve(block)?)?;
        let def = Reader::new(self.snapshot.registry(), types::BLOCK, &object.values);
        let base = point_of(def.get("base_point")).unwrap_or_else(Point2::origin);
        let mut out = Vec::new();
        for record in attribute_records(&def) {
            if record.constant {
                continue;
            }
            let Some(Value::String(value)) = values.get(&record.tag) else {
                continue;
            };
            let local = Vector2::new(
                (record.position.x - base.x) * scale[0],
                (record.position.y - base.y) * scale[1],
            );
            let (sin, cos) = rotation.sin_cos();
            let at = Point2::new(
                position.x + local.x * cos - local.y * sin,
                position.y + local.x * sin + local.y * cos,
            );
            let handle = self.handle();
            out.push(Entity {
                head: arc_io_dxf::model::Head {
                    handle: Some(handle),
                    owner: Some(owner),
                    app_groups: Vec::new(),
                },
                props: EntityProps {
                    layer: "0".to_owned(),
                    ..EntityProps::default()
                },
                kind: EntityKind::Attrib(Attrib {
                    insert: p3(at, z),
                    height: arc_draft::text::DEFAULT_HEIGHT * scale[1].abs(),
                    value: value.clone(),
                    rotation: Some(rotation.to_degrees()).filter(|a| *a != 0.0),
                    tag: record.tag.clone(),
                    flags: i16::from(record.invisible),
                    ..Attrib::default()
                }),
                extras: Vec::new(),
                xdata: Vec::new(),
            });
        }
        Ok(out)
    }

    /// The attribute definitions of a block as `ATTDEF` entities.
    pub(crate) fn attribute_definitions(&mut self, def: &Reader<'_>, owner: Handle) -> Vec<Entity> {
        attribute_records(def)
            .into_iter()
            .map(|a| Entity {
                head: self.head(owner),
                props: EntityProps {
                    layer: "0".to_owned(),
                    ..EntityProps::default()
                },
                kind: EntityKind::AttDef(AttDef {
                    insert: p3(a.position, 0.0),
                    height: arc_draft::text::DEFAULT_HEIGHT,
                    value: a.default,
                    prompt: a.prompt,
                    tag: a.tag,
                    flags: i16::from(a.invisible)
                        | (i16::from(a.constant) << 1)
                        | (i16::from(a.preset) << 3),
                    ..AttDef::default()
                }),
                extras: Vec::new(),
                xdata: Vec::new(),
            })
            .collect()
    }
}

struct AttributeRecord {
    tag: String,
    prompt: String,
    default: String,
    constant: bool,
    invisible: bool,
    preset: bool,
    position: Point2,
}

fn attribute_records(def: &Reader<'_>) -> Vec<AttributeRecord> {
    let Some(Value::List(list)) = def.get("attributes") else {
        return Vec::new();
    };
    let text = |f: &[Option<Value>], i: usize| match f.get(i) {
        Some(Some(Value::String(s))) => s.clone(),
        _ => String::new(),
    };
    let flag = |f: &[Option<Value>], i: usize| matches!(f.get(i), Some(Some(Value::Bool(true))));
    list.iter()
        .filter_map(|item| {
            let Value::Record(f) = item else { return None };
            Some(AttributeRecord {
                tag: text(f, 0),
                prompt: text(f, 1),
                default: text(f, 2),
                constant: flag(f, 3),
                invisible: flag(f, 4),
                preset: flag(f, 5),
                position: point_of(f.get(7).and_then(Option::as_ref))
                    .unwrap_or_else(Point2::origin),
            })
        })
        .collect()
}

/// The start and end of a counterclockwise range from `start` over the signed `sweep`.
fn ccw_range(start: f64, sweep: f64) -> (f64, f64) {
    if sweep >= 0.0 {
        (start, start + sweep)
    } else {
        (start + sweep, start)
    }
}

fn ellipse(e: &arc_geom::Ellipse2, z: f64, start: f64, end: f64) -> Ellipse {
    Ellipse {
        center: p3(e.center(), z),
        major_axis: [e.major_axis().x, e.major_axis().y, 0.0],
        ratio: e.ratio(),
        start_param: start,
        end_param: end,
    }
}

fn lw_polyline(p: &Polyline2) -> LwPolyline {
    LwPolyline {
        flags: i16::from(p.is_closed()),
        constant_width: None,
        vertices: p
            .vertices()
            .iter()
            .map(|v| LwVertex {
                location: [v.point.x, v.point.y],
                id: None,
                start_width: Some(v.start_width).filter(|w| *w != 0.0),
                end_width: Some(v.end_width).filter(|w| *w != 0.0),
                bulge: Some(v.bulge).filter(|b| *b != 0.0),
            })
            .collect(),
    }
}

/// Escapes the characters that start formatting codes in multi-line text.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '{' | '}') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn hatch_loop(curve: &Curve2, first: bool) -> HatchLoop {
    let external = if first { 1 } else { 0 };
    let boundary = match curve {
        Curve2::Circle(c) => HatchBoundary::Edges(vec![HatchEdge::Arc {
            center: [c.center().x, c.center().y],
            radius: c.radius(),
            start_angle: 0.0,
            end_angle: 360.0,
            counterclockwise: true,
        }]),
        Curve2::Ellipse(e) => HatchBoundary::Edges(vec![HatchEdge::Ellipse {
            center: [e.center().x, e.center().y],
            major_axis: [e.major_axis().x, e.major_axis().y],
            ratio: e.ratio(),
            start_angle: 0.0,
            end_angle: 360.0,
            counterclockwise: true,
        }]),
        Curve2::Polyline(p) => HatchBoundary::Polyline {
            has_bulge: p.vertices().iter().any(|v| v.bulge != 0.0),
            closed: true,
            vertices: p
                .vertices()
                .iter()
                .map(|v| HatchVertex {
                    location: [v.point.x, v.point.y],
                    bulge: v.bulge,
                })
                .collect(),
        },
        _ => HatchBoundary::Edges(Vec::new()),
    };
    HatchLoop {
        flags: external
            | if matches!(boundary, HatchBoundary::Polyline { .. }) {
                2
            } else {
                0
            },
        boundary,
        sources: Vec::new(),
    }
}

/// A pattern line family in the final frame of the hatch: scaled by `scale`, rotated by `angle`
/// about `origin`, as the format stores lines.
fn final_line(family: &Value, scale: f64, angle: f64, origin: Point2) -> PatternLine {
    let Value::Record(f) = family else {
        return PatternLine::default();
    };
    let real = |i: usize| match f.get(i) {
        Some(Some(Value::F64(v))) => *v,
        _ => 0.0,
    };
    let base = point_of(f.get(1).and_then(Option::as_ref)).unwrap_or_else(Point2::origin);
    let total = real(0).to_radians() + angle;
    let (su, cu) = total.sin_cos();
    let (sh, ch) = angle.sin_cos();
    let (shift, spacing) = (real(2) * scale, real(3) * scale);
    let dashes = match f.get(4) {
        Some(Some(Value::List(items))) => items
            .iter()
            .filter_map(|d| {
                if let Value::F64(v) = d {
                    Some(v * scale)
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    PatternLine {
        angle: total.to_degrees(),
        base: [
            origin.x + (base.x * ch - base.y * sh) * scale,
            origin.y + (base.x * sh + base.y * ch) * scale,
        ],
        offset: [shift * cu - spacing * su, shift * su + spacing * cu],
        dashes,
    }
}

/// The DXF dimension of a drafting dimension, `None` for the kinds the format has no
/// counterpart for here.
fn dimension_data(
    kind: DimensionKind,
    pts: &[Point2],
    line: Point2,
    rotation: f64,
    z: f64,
) -> Option<Dimension> {
    let at = |i: usize| pts.get(i).copied();
    let mut d = Dimension::default();
    match kind {
        DimensionKind::Linear | DimensionKind::Aligned => {
            let (p0, p1) = (at(0)?, at(1)?);
            let u = if kind == DimensionKind::Linear {
                Vector2::new(rotation.cos(), rotation.sin())
            } else {
                let v = p1 - p0;
                let len = v.x.hypot(v.y);
                if len == 0.0 {
                    return None;
                }
                Vector2::new(v.x / len, v.y / len)
            };
            let n = Vector2::new(-u.y, u.x);
            let foot = |p: Point2| p + n * ((line - p).dot(&n));
            let (f0, f1) = (foot(p0), foot(p1));
            d.kind = if kind == DimensionKind::Linear { 0 } else { 1 };
            d.def_point = p3(f1, z);
            d.text_mid = p3(Point2::new((f0.x + f1.x) / 2.0, (f0.y + f1.y) / 2.0), z);
            d.point13 = Some(p3(p0, z));
            d.point14 = Some(p3(p1, z));
            d.angle = Some(rotation.to_degrees());
        }
        DimensionKind::Radial => {
            d.kind = 4;
            d.def_point = p3(at(0)?, z);
            d.point15 = Some(p3(at(1)?, z));
            d.text_mid = p3(line, z);
        }
        DimensionKind::Diameter => {
            let (c, on) = (at(0)?, at(1)?);
            d.kind = 3;
            d.def_point = p3(Point2::new(2.0 * c.x - on.x, 2.0 * c.y - on.y), z);
            d.point15 = Some(p3(on, z));
            d.text_mid = p3(line, z);
        }
        DimensionKind::AngularPoints => {
            d.kind = 5;
            d.point15 = Some(p3(at(0)?, z));
            d.point13 = Some(p3(at(1)?, z));
            d.point14 = Some(p3(at(2)?, z));
            d.def_point = p3(line, z);
            d.text_mid = p3(line, z);
        }
        DimensionKind::AngularLines => {
            d.kind = 2;
            d.point13 = Some(p3(at(0)?, z));
            d.point14 = Some(p3(at(1)?, z));
            d.point15 = Some(p3(at(2)?, z));
            d.def_point = p3(at(3)?, z);
            d.point16 = Some(p3(line, z));
            d.text_mid = p3(line, z);
        }
        DimensionKind::OrdinateX | DimensionKind::OrdinateY => {
            d.kind = 6 | if kind == DimensionKind::OrdinateX {
                64
            } else {
                0
            };
            d.def_point = p3(at(0)?, z);
            d.point13 = Some(p3(at(1)?, z));
            d.point14 = Some(p3(line, z));
            d.text_mid = p3(line, z);
        }
        DimensionKind::AngularArc | DimensionKind::ArcLength => return None,
    }
    Some(d)
}
