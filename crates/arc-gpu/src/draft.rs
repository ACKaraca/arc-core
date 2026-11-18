//! The shared extractor of `arc-draft` display lists (CORE-GPU-013), behind the feature
//! `draft`.
//!
//! Display lists are built with dash generation turned off ([`display_options`]), so strokes
//! stay whole (or cut by clips and wipeouts) and their linetypes are drawn by the line shader
//! (CORE-GPU-009). A primitive becomes:
//!
//! - **Stroke or point**: a path of a [`CurveItem`] in the primitive's color (alpha times one
//!   minus its transparency), with its lineweight as the width (screen pixels in model spaces,
//!   paper millimeters on sheets) marked as a lineweight, round caps and joins, and polyline
//!   widths kept. Consecutive primitives of one source object with the same appearance share
//!   one item; [`DraftExtraction::sub_objects`] gives the sub-object of every path, and
//!   [`crate::Scene::path_of`] the path of a picked segment. Unbounded curves that no clip
//!   bounded are not drawn.
//! - **Linetype**: the pattern is scaled by the entity's linetype scale times the scale of the
//!   map from the drawing entity to the list (the ratio of the curve's length in the list to
//!   its length in the entity), and each drawn piece starts at the pattern position of its
//!   start on the whole curve. Patterns with at most [`crate::MAX_DASH_INTERVALS`] drawn
//!   elements are evaluated on the GPU; longer ones are generated on the CPU with
//!   `arc_draft::linetype::dashes`, intersected with the drawn pieces, and drawn solid.
//! - **Fill or wipeout mask**: a mesh triangulated from the boundary, in the primitive's color
//!   for a fill, and painted in the background color (model) or the paper color (sheet) for a
//!   mask. Masks and fills of a viewport list that need a clip are intersected with the clip
//!   boundary on the CPU: both are triangulated and every pair of triangles is clipped
//!   (Sutherland–Hodgman against the convex clip triangle).
//!
//! Sheets ([`sheet_extractions`]) draw the paper rectangle in the paper color, the printable
//! area inside the margins as a one-pixel gray outline, the layout's own entities, and every
//! viewport's list as one object (the viewport) whose strokes carry the viewport's clip
//! rectangle when the clip is an axis-aligned rectangle, so the GPU scissors them (curves
//! are already cut by the display list; the scissor also cuts the width of a line that
//! reaches the boundary).

use std::collections::BTreeMap;

use arc_base::math::{Matrix3, Point2, Point3, Transform2};
use arc_base::{Cancel, Color, Error, Limits, ObjectId, Result};
use arc_doc::relationship::ReferenceField;
use arc_doc::{Object, Snapshot, TypeKey, Value};
use arc_draft::clip::{Piece, piece};
use arc_draft::display::{
    DisplayList, DisplayOptions, Lists, Primitive, Shape, entity_list, space_list, viewport_list,
};
use arc_draft::layout::{LAYOUT, VIEWPORT, settings, sheet_size};
use arc_draft::linetype::{DashOptions, Dashes, Pattern, dashes};
use arc_draft::types::{ENTITIES, ElementKind, SPACE, SpaceUnit, layer, table};
use arc_geom::encoding::GeomCodec;
use arc_geom::region::{Loop2, Region, RegionPart};
use arc_geom::{Curve2, CurveEval, CurveProject, Polyline2, PolylineVertex, triangulate};
use arc_mesh::TriMesh;

use crate::curve::{CurveItem, CurvePath, CurveShape, DashPattern, MAX_DASH_INTERVALS};
use crate::error::codes;
use crate::extract::{ApplyReport, Extraction, ExtractionResult, ExtractorRegistry, Outcome};
use crate::scene::{LineStyle, LineWidth, MeshInstance, MeshItem, RenderItem, Scene, SceneLimits};

/// Mesh instance flag: paint in the background color (wipeout masks of model spaces).
pub const PAINT_BACKGROUND: u32 = 1;
/// Mesh instance flag: paint in the paper color (sheet backgrounds and masks on sheets).
pub const PAINT_PAPER: u32 = 2;
/// The color of the printable-area outline on sheets.
pub const MARGIN_COLOR: Color = Color::new(0.5, 0.5, 0.5, 1.0);

/// The options of drafting extraction.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftOptions {
    /// The display-list options; [`display_options`] turns their dash generation off.
    pub display: DisplayOptions,
    /// The lineweight in millimeters of a primitive with the default lineweight.
    pub default_lineweight_mm: f64,
    /// Pixels per millimeter of lineweight in model spaces.
    pub lineweight_px_per_mm: f32,
    /// The scene limits that bound item, triangle and segment counts.
    pub limits: SceneLimits,
}

impl Default for DraftOptions {
    fn default() -> Self {
        Self {
            display: DisplayOptions::default(),
            default_lineweight_mm: 0.25,
            lineweight_px_per_mm: 96.0 / 25.4,
            limits: SceneLimits::DEFAULT,
        }
    }
}

/// The display-list options extraction builds lists with: `options.display` without dash
/// generation, so dashes are left to the GPU.
#[must_use]
pub fn display_options(options: &DraftOptions) -> DisplayOptions {
    DisplayOptions {
        max_dashes: 0,
        ..options.display.clone()
    }
}

/// How lineweights become widths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Widths {
    /// Model space: screen pixels through [`DraftOptions::lineweight_px_per_mm`].
    Screen,
    /// Sheets: paper millimeters through the plot scale of the render settings.
    Paper,
}

/// The sub-object a drawn path comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubObject {
    /// The entity in the list's space (for a viewport list, in the shown space).
    pub source: ObjectId,
    /// The nested entities below `source`, outermost first.
    pub path: Vec<ObjectId>,
    /// The part of the drawing entity: 0 its shape, 1 the frame of a wipeout.
    pub part: u32,
}

/// The extraction of one object and the sub-objects of its items' paths.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DraftExtraction {
    /// The render items and geometries.
    pub extraction: Extraction,
    /// For every item, the sub-object of each of its paths (one for a mesh item).
    pub sub_objects: Vec<Vec<SubObject>>,
}

/// A sheet: its objects and how to render it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    /// The extractions by object: the layout settings (paper and margins), the layout's
    /// entities and the viewports.
    pub objects: Vec<(ObjectId, DraftExtraction)>,
    /// The sheet width and height in the layout's unit; the paper spans `(0, 0)` to them.
    pub size: (f64, f64),
    /// Sheet units per paper millimeter: the [`crate::RenderSettings::plot_scale`] that draws
    /// lineweights at their paper width.
    pub plot_scale: f64,
}

fn values(snapshot: &Snapshot, id: ObjectId) -> Option<&[Option<Value>]> {
    let object = snapshot.get(snapshot.resolve(id).ok()?).ok()?;
    Some(object.values.as_slice())
}

fn type_of(snapshot: &Snapshot, id: ObjectId) -> Option<&str> {
    snapshot
        .type_key(snapshot.resolve(id).ok()?)
        .map(TypeKey::as_str)
}

fn field(snapshot: &Snapshot, id: ObjectId, name: &str) -> Option<Value> {
    let key = type_of(snapshot, id)?;
    let position = snapshot
        .registry()
        .get(key)?
        .schema
        .fields
        .iter()
        .position(|f| f.name == name)?;
    values(snapshot, id)?.get(position).cloned().flatten()
}

fn unit_of(snapshot: &Snapshot, space: ObjectId) -> SpaceUnit {
    match values(snapshot, space).and_then(|v| v.get(table::SPACE_UNIT)) {
        Some(Some(Value::Enum(code))) => SpaceUnit::of(*code).unwrap_or(SpaceUnit::Unitless),
        _ => SpaceUnit::Unitless,
    }
}

fn is_layout(snapshot: &Snapshot, space: ObjectId) -> bool {
    matches!(
        values(snapshot, space).and_then(|v| v.get(table::SPACE_KIND)),
        Some(Some(Value::Enum(1)))
    )
}

fn layer_hidden(snapshot: &Snapshot, id: ObjectId) -> bool {
    values(snapshot, id).is_some_and(|v| {
        matches!(v.get(layer::ON), Some(Some(Value::Bool(false))))
            || matches!(v.get(layer::FROZEN), Some(Some(Value::Bool(true))))
    })
}

/// The objects of type `key` that `owner` owns.
fn owned(snapshot: &Snapshot, owner: ObjectId, key: &str) -> Vec<ObjectId> {
    let mut ids: Vec<ObjectId> = snapshot
        .referrers(owner)
        .filter(|r| r.field == ReferenceField::Owner)
        .map(|r| r.source)
        .filter(|id| type_of(snapshot, *id) == Some(key))
        .collect();
    ids.sort_unstable();
    ids
}

/// The color of a primitive with its transparency in the alpha.
fn color_of(p: &Primitive) -> Color {
    let mut color = p.color.as_ref().map_or(Color::WHITE, |c| c.value.rgba);
    let transparency = p.transparency.as_ref().map_or(0.0, |t| t.value);
    #[allow(clippy::cast_possible_truncation)] // A transparency is in [0, 1].
    let opacity = (1.0 - transparency.clamp(0.0, 1.0)) as f32;
    color.a *= opacity;
    color
}

fn narrow(value: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Narrowing to the GPU's precision is the intent.
    let narrowed = value as f32;
    narrowed
}

fn style_of(p: &Primitive, widths: Widths, options: &DraftOptions) -> LineStyle {
    let mm = p
        .lineweight
        .as_ref()
        .and_then(|l| l.value)
        .unwrap_or(options.default_lineweight_mm);
    let width = match widths {
        Widths::Screen => LineWidth::Screen(
            (narrow(mm) * options.lineweight_px_per_mm)
                .clamp(1.0, options.limits.max_line_width_px),
        ),
        Widths::Paper => LineWidth::Paper(narrow(mm.max(f64::MIN_POSITIVE))),
    };
    LineStyle {
        lineweight: true,
        ..LineStyle::round(width)
    }
}

/// The curve of the drawing entity of `p`, in its own coordinates.
fn entity_curve(snapshot: &Snapshot, p: &Primitive) -> Option<Curve2> {
    let entity = p.path.last().copied().unwrap_or(p.source);
    match field(snapshot, entity, "geometry")? {
        Value::Geometry(bytes) => Curve2::decode(&bytes.0, &Limits::DEFAULT).ok(),
        _ => None,
    }
}

/// The scale of the map from the drawing entity to the list: the ratio of the lengths of the
/// curve in both; 1 when it cannot be measured.
fn map_scale(snapshot: &Snapshot, p: &Primitive, curve: &Curve2, options: &DraftOptions) -> f64 {
    let tolerance = &options.display.tolerance;
    let ratio = entity_curve(snapshot, p)
        .and_then(|own| Some(curve.length(tolerance).ok()? / own.length(tolerance).ok()?));
    ratio.filter(|r| r.is_finite() && *r > 0.0).unwrap_or(1.0)
}

/// The drawn intervals of one period of `pattern` scaled by `scale`, and the period.
fn intervals(pattern: &Pattern, scale: f64) -> (Vec<[f64; 2]>, f64) {
    let mut at = 0.0;
    let mut out = Vec::new();
    for element in pattern.elements() {
        let end = at + element.length * scale;
        match element.kind {
            ElementKind::Dash => out.push([at, end]),
            ElementKind::Dot => out.push([at, at]),
            ElementKind::Gap | ElementKind::Text | ElementKind::Shape => {}
        }
        at = end;
    }
    (out, pattern.period() * scale)
}

/// The linetype of a stroke: its pattern and total scale, or `None` for continuous.
fn linetype(
    snapshot: &Snapshot,
    p: &Primitive,
    curve: &Curve2,
    options: &DraftOptions,
) -> Result<Option<(Pattern, f64)>> {
    let Some(id) = p.linetype.as_ref().and_then(|l| l.value) else {
        return Ok(None);
    };
    if !curve.length(&options.display.tolerance)?.is_finite() {
        // Unbounded curves are drawn solid, as in dash generation.
        return Ok(None);
    }
    let pattern = Pattern::from_values(values(snapshot, id).unwrap_or(&[]))?;
    let scale = p.linetype_scale * map_scale(snapshot, p, curve, options);
    Ok(Some((pattern, scale)))
}

/// The parameter interval of a drawn piece on the whole curve.
fn interval_of(curve: &Curve2, c: &Curve2, options: &DraftOptions) -> Result<(f64, f64)> {
    let tolerance = &options.display.tolerance;
    let (Some(start), Some(end)) = (c.start_point(), c.end_point()) else {
        return Ok(curve.parameter_range());
    };
    let a = curve.closest_point(start, tolerance)?.parameter;
    let mut b = curve.closest_point(end, tolerance)?.parameter;
    if b <= a {
        // A piece ending at the seam of a closed curve.
        b = curve.parameter_range().1;
    }
    Ok((a, b))
}

/// The pattern position at parameter `t`: the length from the start of the curve, or from the
/// start of its polyline segment when the pattern restarts at vertices.
fn position_at(curve: &Curve2, t: f64, restart: bool, options: &DraftOptions) -> Result<f64> {
    let from = match curve {
        Curve2::Polyline(_) if restart => t.floor().min(t),
        _ => curve.parameter_range().0,
    };
    curve.length_between(from, t, &options.display.tolerance)
}

/// The shapes of a stroke primitive: what it draws, as paths, and its GPU pattern.
fn stroke_paths(
    snapshot: &Snapshot,
    p: &Primitive,
    curve: &Curve2,
    drawn: Option<&Vec<Piece>>,
    options: &DraftOptions,
) -> Result<(Vec<CurvePath>, Option<DashPattern>)> {
    let restart = !p.continuous;
    let path = |shape: CurveShape, phase: f64| CurvePath {
        shape,
        phase,
        restart_at_vertices: restart,
    };
    let whole = [Piece::Curve(curve.clone())];
    let pieces: &[Piece] = drawn.map_or(&whole, Vec::as_slice);
    let bounded = |c: &Curve2| !matches!(c, Curve2::Ray(_) | Curve2::Line(_));
    let Some((pattern, scale)) = linetype(snapshot, p, curve, options)? else {
        let paths = pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Point(q) => Some(path(CurveShape::Point(*q), 0.0)),
                Piece::Curve(c) if bounded(c) => Some(path(CurveShape::Curve(c.clone()), 0.0)),
                Piece::Curve(_) => None,
            })
            .collect();
        return Ok((paths, None));
    };
    let (drawn_intervals, period) = intervals(&pattern, scale);
    if drawn_intervals.len() <= MAX_DASH_INTERVALS && period.is_finite() && period > 0.0 {
        let mut paths = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match piece {
                Piece::Point(q) => paths.push(path(CurveShape::Point(*q), 0.0)),
                Piece::Curve(c) => {
                    let (a, _) = interval_of(curve, c, options)?;
                    let phase = position_at(curve, a, restart, options)?;
                    paths.push(path(CurveShape::Curve(c.clone()), phase));
                }
            }
        }
        let pattern = DashPattern {
            intervals: drawn_intervals,
            period,
        };
        return Ok((paths, Some(pattern)));
    }
    // Patterns beyond the shader's intervals: dashes on the CPU, intersected with the pieces.
    let dash_options = DashOptions {
        global_scale: 1.0,
        object_scale: scale,
        continuous: p.continuous,
        max_dashes: options.display.max_dashes,
    };
    let mut kept = Vec::new();
    let mut paths = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Point(q) => paths.push(path(CurveShape::Point(*q), 0.0)),
            Piece::Curve(c) => kept.push(interval_of(curve, c, options)?),
        }
    }
    match dashes(curve, &pattern, &dash_options, &options.display.tolerance)? {
        Dashes::Continuous => {
            for piece in pieces {
                if let Piece::Curve(c) = piece {
                    paths.push(path(CurveShape::Curve(c.clone()), 0.0));
                }
            }
        }
        Dashes::Pieces(list) => {
            for d in list {
                for (a, b) in &kept {
                    let (low, high) = (d.t0.max(*a), d.t1.min(*b));
                    let shape = if d.t0 == d.t1 && *a <= d.t0 && d.t0 <= *b {
                        Some(CurveShape::Point(curve.point_at(d.t0)))
                    } else if low < high {
                        // A dash too small to construct has no extent to draw.
                        match piece(curve, low, high) {
                            Ok(Piece::Curve(c)) => Some(CurveShape::Curve(c)),
                            Ok(Piece::Point(q)) => Some(CurveShape::Point(q)),
                            Err(_) => None,
                        }
                    } else {
                        None
                    };
                    paths.extend(shape.map(|s| path(s, 0.0)));
                }
            }
        }
    }
    Ok((paths, None))
}

fn translate(curve: &Curve2, by: Point2, options: &DraftOptions) -> Result<Curve2> {
    let map = Transform2::from_matrix_unchecked(Matrix3::new(
        1.0, 0.0, -by.x, 0.0, 1.0, -by.y, 0.0, 0.0, 1.0,
    ));
    curve.transformed(&map, &options.display.tolerance)
}

fn shape_origin(shape: &CurveShape) -> Option<Point2> {
    match shape {
        CurveShape::Point(q) => Some(*q),
        CurveShape::Curve(c) => c.bounding_box().map(|b| b.min()),
    }
}

/// A triangle, counterclockwise.
type Triangle = [Point2; 3];

fn cross(o: Point2, a: Point2, b: Point2) -> f64 {
    (a - o).perp(&(b - o))
}

/// The triangles of the region inside a closed polyline.
fn triangles(boundary: &Polyline2, options: &DraftOptions) -> Result<Vec<Triangle>> {
    let tolerance = &options.display.tolerance;
    let outer = Loop2::new(vec![Curve2::Polyline(boundary.clone())], tolerance)?;
    let region = Region::new(vec![RegionPart::new(outer, Vec::new())])?;
    let max = usize::try_from(options.limits.max_geometry_vertices).unwrap_or(usize::MAX);
    let mesh = triangulate(
        &region,
        tolerance.tessellation_chord,
        tolerance.tessellation_angle,
        max,
    )?;
    let at = |i: u32| mesh.vertices().get(i as usize).copied();
    Ok(mesh
        .triangles()
        .iter()
        .filter_map(|[a, b, c]| {
            let (a, b, c) = (at(*a)?, at(*b)?, at(*c)?);
            Some(if cross(a, b, c) < 0.0 {
                [a, c, b]
            } else {
                [a, b, c]
            })
        })
        .collect())
}

/// The part of `polygon` inside the counterclockwise triangle `clip` (Sutherland–Hodgman).
fn clip_polygon(polygon: Vec<Point2>, clip: &Triangle) -> Vec<Point2> {
    let mut out = polygon;
    for k in 0..3 {
        let (Some(a), Some(b)) = (clip.get(k), clip.get((k + 1) % 3)) else {
            continue;
        };
        let input = std::mem::take(&mut out);
        let Some(mut previous) = input.last().copied() else {
            break;
        };
        for current in input {
            let (pc, cc) = (cross(*a, *b, previous), cross(*a, *b, current));
            if (cc >= 0.0) != (pc >= 0.0) {
                let t = pc / (pc - cc);
                out.push(previous + (current - previous) * t);
            }
            if cc >= 0.0 {
                out.push(current);
            }
            previous = current;
        }
    }
    out
}

/// The triangles of `subject` inside the region of `clip`.
fn intersect(
    subject: &[Triangle],
    clip: &[Triangle],
    options: &DraftOptions,
) -> Result<Vec<Triangle>> {
    let max = u64::from(options.limits.max_geometry_triangles);
    if (subject.len() as u64).saturating_mul(clip.len() as u64) > max {
        return Err(
            Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_geometry_triangles")
        );
    }
    let mut out = Vec::new();
    for s in subject {
        for c in clip {
            let polygon = clip_polygon(s.to_vec(), c);
            if let Some((first, rest)) = polygon.split_first() {
                for pair in rest.windows(2) {
                    if let [b, c] = pair {
                        out.push([*first, *b, *c]);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The mesh item of a fill or mask, and the geometry it uses.
fn region_item(
    extraction: &mut Extraction,
    boundary: &Polyline2,
    clip: Option<&[Triangle]>,
    instance: MeshInstance,
    options: &DraftOptions,
) -> Result<Option<RenderItem>> {
    let mut tris = triangles(boundary, options)?;
    if let Some(clip) = clip {
        tris = intersect(&tris, clip, options)?;
    }
    let Some(origin) = tris.first().map(|t| t[0]) else {
        return Ok(None);
    };
    let mut positions = Vec::with_capacity(tris.len() * 3);
    let mut indices = Vec::with_capacity(tris.len());
    for (t, i) in tris.iter().zip(0_u32..) {
        positions.extend(t.iter().map(|p| Point3::new(p.x, p.y, 0.0)));
        indices.push([3 * i, 3 * i + 1, 3 * i + 2]);
    }
    let mesh = TriMesh::new(positions, indices, &Limits::DEFAULT)?;
    let origin = Point3::new(origin.x, origin.y, 0.0);
    let geometry = extraction.add_geometry(mesh, origin);
    Ok(Some(RenderItem::Mesh(MeshItem {
        geometry,
        origin,
        instances: vec![instance],
    })))
}

/// The clip of a viewport list: its triangles, and its rectangle when it is an axis-aligned
/// rectangle.
struct Clip {
    triangles: Vec<Triangle>,
    rectangle: Option<[Point2; 2]>,
}

fn clip_of(boundary: &Polyline2, options: &DraftOptions) -> Result<Clip> {
    let v = boundary.vertices();
    let rectangle = match v {
        [a, b, c, d] if v.iter().all(|x| x.bulge == 0.0) => {
            let xs = [a.point.x, b.point.x, c.point.x, d.point.x];
            let ys = [a.point.y, b.point.y, c.point.y, d.point.y];
            let horizontal = (a.point.y == b.point.y
                && c.point.y == d.point.y
                && a.point.x == d.point.x
                && b.point.x == c.point.x)
                || (a.point.x == b.point.x
                    && c.point.x == d.point.x
                    && a.point.y == d.point.y
                    && b.point.y == c.point.y);
            let min = |s: [f64; 4]| s.iter().copied().fold(f64::INFINITY, f64::min);
            let max = |s: [f64; 4]| s.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            horizontal.then(|| [Point2::new(min(xs), min(ys)), Point2::new(max(xs), max(ys))])
        }
        _ => None,
    };
    Ok(Clip {
        triangles: triangles(boundary, options)?,
        rectangle,
    })
}

/// Turns primitives into the items of one object, in order.
struct Builder<'a> {
    snapshot: &'a Snapshot,
    options: &'a DraftOptions,
    widths: Widths,
    mask: u32,
    clip: Option<&'a Clip>,
    out: DraftExtraction,
}

impl Builder<'_> {
    fn primitive(&mut self, p: &Primitive) -> Result<()> {
        let sub = SubObject {
            source: p.source,
            path: p.path.clone(),
            part: p.part,
        };
        match &p.shape {
            // Outline glyph runs are drawn by the text pipeline (CORE-GPU-012), which does not
            // exist yet; stroke text arrives as strokes and is drawn.
            Shape::Glyphs { .. } => Ok(()),
            Shape::Fill(boundary) | Shape::Mask(boundary) => {
                let mut instance = MeshInstance::new(color_of(p));
                if matches!(p.shape, Shape::Mask(_)) {
                    instance.flags = self.mask;
                }
                let clip = self.clip.map(|c| c.triangles.as_slice());
                if let Some(item) = region_item(
                    &mut self.out.extraction,
                    boundary,
                    clip,
                    instance,
                    self.options,
                )? {
                    self.out.extraction.items.push(item);
                    self.out.sub_objects.push(vec![sub]);
                }
                Ok(())
            }
            Shape::Point(q) => self.stroke(
                p,
                vec![CurvePath {
                    shape: CurveShape::Point(*q),
                    phase: 0.0,
                    restart_at_vertices: false,
                }],
                None,
                sub,
            ),
            Shape::Stroke { curve, drawn } => {
                let (paths, pattern) =
                    stroke_paths(self.snapshot, p, curve, drawn.as_ref(), self.options)?;
                self.stroke(p, paths, pattern, sub)
            }
        }
    }

    fn stroke(
        &mut self,
        p: &Primitive,
        paths: Vec<CurvePath>,
        pattern: Option<DashPattern>,
        sub: SubObject,
    ) -> Result<()> {
        let Some(first) = paths.first().and_then(|p| shape_origin(&p.shape)) else {
            return Ok(());
        };
        let list_clip = self.clip.and_then(|c| c.rectangle);
        let style = style_of(p, self.widths, self.options);
        let color = color_of(p);
        // Consecutive primitives with the same appearance share the previous item.
        let shared = match self.out.extraction.items.last() {
            Some(RenderItem::Curves(last)) => {
                let o = Point2::new(last.origin.x, last.origin.y).coords;
                last.style == style
                    && last.color == color
                    && last.pattern == pattern
                    && last.clip.map(|[a, b]| [a + o, b + o]) == list_clip
            }
            _ => false,
        };
        if !shared {
            self.out
                .extraction
                .items
                .push(RenderItem::Curves(CurveItem {
                    origin: Point3::new(first.x, first.y, 0.0),
                    paths: Vec::new(),
                    style,
                    color,
                    pattern,
                    clip: list_clip.map(|[a, b]| [a - first.coords, b - first.coords]),
                }));
            self.out.sub_objects.push(Vec::new());
        }
        let (Some(RenderItem::Curves(target)), Some(subs)) = (
            self.out.extraction.items.last_mut(),
            self.out.sub_objects.last_mut(),
        ) else {
            return Ok(());
        };
        let origin = Point2::new(target.origin.x, target.origin.y);
        for mut path in paths {
            path.shape = match path.shape {
                CurveShape::Point(q) => CurveShape::Point(q - origin.coords),
                CurveShape::Curve(c) => CurveShape::Curve(translate(&c, origin, self.options)?),
            };
            target.paths.push(path);
            subs.push(sub.clone());
        }
        Ok(())
    }
}

fn group(
    snapshot: &Snapshot,
    primitives: &[Primitive],
    widths: Widths,
    mask: u32,
    clip: Option<&Clip>,
    options: &DraftOptions,
) -> Result<DraftExtraction> {
    let mut builder = Builder {
        snapshot,
        options,
        widths,
        mask,
        clip,
        out: DraftExtraction::default(),
    };
    for p in primitives {
        builder.primitive(p)?;
    }
    let max = usize::try_from(options.limits.max_items_per_object).unwrap_or(usize::MAX);
    if builder.out.extraction.items.len() > max {
        return Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_items_per_object"));
    }
    Ok(builder.out)
}

/// The extraction of the primitives of one object of a list: masks painted with `mask`
/// ([`PAINT_BACKGROUND`] or [`PAINT_PAPER`]), and fills and masks clipped to `clip` when it is
/// given (a viewport list's clip boundary).
///
/// # Errors
///
/// The errors of linetype reading, length measurement, triangulation and transforms;
/// `gpu.limit.exceeded` beyond the scene limits.
pub fn extract_primitives(
    snapshot: &Snapshot,
    primitives: &[Primitive],
    widths: Widths,
    mask: u32,
    clip: Option<&Polyline2>,
    options: &DraftOptions,
) -> Result<DraftExtraction> {
    let clip = clip.map(|c| clip_of(c, options)).transpose()?;
    group(snapshot, primitives, widths, mask, clip.as_ref(), options)
}

/// The extraction of one entity by itself: its display list with the primitives on off or
/// frozen layers left out, as screen widths in a model space and paper widths in a layout. An
/// entity in a block definition has no items (its references draw it). Wipeouts cut other
/// entities only in assembled lists ([`space_extractions`]): drawn one by one, a mask paints
/// over the meshes beneath it but not over lines.
///
/// # Errors
///
/// The errors of [`entity_list`] and [`extract_primitives`].
pub fn extract_entity(
    snapshot: &Snapshot,
    entity: ObjectId,
    options: &DraftOptions,
) -> Result<DraftExtraction> {
    let owner = snapshot.get(snapshot.resolve(entity)?)?.header.owner;
    let Some(space) = owner.filter(|o| type_of(snapshot, *o) == Some(SPACE)) else {
        return Ok(DraftExtraction::default());
    };
    let list = entity_list(snapshot, entity, &display_options(options))?;
    let visible: Vec<Primitive> = list
        .primitives
        .into_iter()
        .filter(|p| !p.layers.iter().any(|l| layer_hidden(snapshot, *l)))
        .collect();
    let (widths, mask) = if is_layout(snapshot, space) {
        (Widths::Paper, PAINT_PAPER)
    } else {
        (Widths::Screen, PAINT_BACKGROUND)
    };
    extract_primitives(snapshot, &visible, widths, mask, None, options)
}

/// Registers [`extract_entity`] for every drafting entity type.
pub fn register_extractors(registry: &mut ExtractorRegistry, options: DraftOptions) -> Result<()> {
    for key in ENTITIES {
        let options = options.clone();
        registry.register(
            TypeKey::new(*key)?,
            move |snapshot: &Snapshot, object: &Object, _: &Cancel| {
                extract_entity(snapshot, object.id, &options).map(|d| d.extraction)
            },
        );
    }
    Ok(())
}

/// Splits a list into the primitives of each source object, in order of first appearance.
fn by_source(list: DisplayList) -> Vec<(ObjectId, Vec<Primitive>)> {
    let mut order: Vec<ObjectId> = Vec::new();
    let mut groups: BTreeMap<ObjectId, Vec<Primitive>> = BTreeMap::new();
    for p in list.primitives {
        let entry = groups.entry(p.source).or_default();
        if entry.is_empty() {
            order.push(p.source);
        }
        entry.push(p);
    }
    order
        .into_iter()
        .filter_map(|id| Some((id, groups.remove(&id)?)))
        .collect()
}

/// The extractions of the entities of `space` from its assembled list (layers, draw order and
/// wipeouts applied), one per source object, with the widths of the space's kind. Entities
/// whose list failed are left out, as in the list.
///
/// # Errors
///
/// The errors of [`space_list`] and [`extract_primitives`].
pub fn space_extractions(
    snapshot: &Snapshot,
    space: ObjectId,
    options: &DraftOptions,
) -> Result<Vec<(ObjectId, DraftExtraction)>> {
    let list = space_list(snapshot, Lists::Direct, space, &display_options(options))?;
    let (widths, mask) = if is_layout(snapshot, space) {
        (Widths::Paper, PAINT_PAPER)
    } else {
        (Widths::Screen, PAINT_BACKGROUND)
    };
    by_source(list)
        .into_iter()
        .map(|(id, primitives)| {
            Ok((
                id,
                extract_primitives(snapshot, &primitives, widths, mask, None, options)?,
            ))
        })
        .collect()
}

fn rectangle(low: Point2, high: Point2) -> Result<Polyline2> {
    let corner = |x: f64, y: f64| PolylineVertex::new(Point2::new(x, y), 0.0);
    Polyline2::new(
        vec![
            corner(low.x, low.y),
            corner(high.x, low.y),
            corner(high.x, high.y),
            corner(low.x, high.y),
        ],
        true,
    )
}

/// The paper rectangle and the printable-area outline of a layout's settings.
fn paper(
    values: &[Option<Value>],
    options: &DraftOptions,
) -> Result<(DraftExtraction, (f64, f64))> {
    let (w, h) = sheet_size(values);
    let at = |i| arc_draft::types::f64_at(values, i).unwrap_or(0.0);
    let mut out = DraftExtraction::default();
    let sheet = rectangle(Point2::origin(), Point2::new(w, h))?;
    let mut instance = MeshInstance::new(Color::WHITE);
    instance.flags = PAINT_PAPER;
    if let Some(item) = region_item(&mut out.extraction, &sheet, None, instance, options)? {
        out.extraction.items.push(item);
        out.sub_objects.push(Vec::new());
    }
    let low = Point2::new(at(settings::MARGIN_LEFT), at(settings::MARGIN_BOTTOM));
    let high = Point2::new(w - at(settings::MARGIN_RIGHT), h - at(settings::MARGIN_TOP));
    if low.x < high.x && low.y < high.y {
        let margin = rectangle(Point2::origin(), Point2::from(high - low))?;
        out.extraction.items.push(RenderItem::Curves(CurveItem {
            origin: Point3::new(low.x, low.y, 0.0),
            paths: vec![CurvePath::curve(Curve2::Polyline(margin))],
            style: LineStyle::round(LineWidth::Screen(1.0)),
            color: MARGIN_COLOR,
            pattern: None,
            clip: None,
        }));
        out.sub_objects.push(Vec::new());
    }
    Ok((out, (w, h)))
}

/// The extractions of the layout space `space` drawn as a sheet: the paper (keyed by the
/// layout settings object), the layout's entities, and each viewport's clipped list (keyed by
/// the viewport), with paper widths. A viewport whose list fails is left out.
///
/// # Errors
///
/// `draft.object.wrong_type` when `space` is not a layout space; the errors of
/// [`space_extractions`] and [`extract_primitives`].
pub fn sheet_extractions(
    snapshot: &Snapshot,
    space: ObjectId,
    options: &DraftOptions,
) -> Result<Sheet> {
    if type_of(snapshot, space) != Some(SPACE) || !is_layout(snapshot, space) {
        return Err(Error::new(arc_draft::error::codes::OBJECT_WRONG_TYPE)
            .with_detail("id", space.to_string()));
    }
    let mut sheet = Sheet {
        plot_scale: SpaceUnit::Millimeter.factor_to(unit_of(snapshot, space)),
        ..Sheet::default()
    };
    for layout in owned(snapshot, space, LAYOUT) {
        let (extraction, size) = paper(values(snapshot, layout).unwrap_or(&[]), options)?;
        sheet.size = size;
        sheet.objects.push((layout, extraction));
    }
    let display = display_options(options);
    for viewport in owned(snapshot, space, VIEWPORT) {
        let Ok(list) = viewport_list(snapshot, Lists::Direct, viewport, &display) else {
            continue;
        };
        let clip = list.clip.clone();
        let extraction = extract_primitives(
            snapshot,
            &list.primitives,
            Widths::Paper,
            PAINT_PAPER,
            clip.as_ref(),
            options,
        )?;
        sheet.objects.push((viewport, extraction));
    }
    sheet
        .objects
        .extend(space_extractions(snapshot, space, options)?);
    Ok(sheet)
}

/// Applies extractions to `scene` as results of `revision`, each newer than what the scene
/// holds for its object (see [`Scene::apply_extractions`]).
pub fn apply(
    scene: &mut Scene,
    revision: u64,
    objects: Vec<(ObjectId, DraftExtraction)>,
) -> ApplyReport {
    let results = objects
        .into_iter()
        .map(|(object, d)| {
            let sequence = scene
                .applied_stamp(&object)
                .filter(|(r, _)| *r == revision)
                .map_or(0, |(_, s)| s.saturating_add(1));
            ExtractionResult {
                object,
                revision,
                sequence,
                outcome: Outcome::Items(d.extraction),
            }
        })
        .collect();
    scene.apply_extractions(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> Triangle {
        [
            Point2::new(a.0, a.1),
            Point2::new(b.0, b.1),
            Point2::new(c.0, c.1),
        ]
    }

    fn area(ts: &[Triangle]) -> f64 {
        ts.iter().map(|[a, b, c]| cross(*a, *b, *c) / 2.0).sum()
    }

    #[test]
    fn triangle_intersection_is_exact_for_squares() {
        // Two unit squares overlapping in a quarter.
        let square = |x: f64, y: f64| {
            vec![
                tri((x, y), (x + 1.0, y), (x + 1.0, y + 1.0)),
                tri((x, y), (x + 1.0, y + 1.0), (x, y + 1.0)),
            ]
        };
        let options = DraftOptions::default();
        let out = intersect(&square(0.0, 0.0), &square(0.5, 0.5), &options).unwrap();
        assert!((area(&out) - 0.25).abs() < 1e-12);
        let none = intersect(&square(0.0, 0.0), &square(2.0, 0.0), &options).unwrap();
        assert!(area(&none).abs() < 1e-12);
    }

    #[test]
    fn rectangular_clips_are_recognized() {
        let options = DraftOptions::default();
        let r = rectangle(Point2::new(1.0, 2.0), Point2::new(3.0, 5.0)).unwrap();
        let clip = clip_of(&r, &options).unwrap();
        assert_eq!(
            clip.rectangle,
            Some([Point2::new(1.0, 2.0), Point2::new(3.0, 5.0)])
        );
        assert!((area(&clip.triangles) - 6.0).abs() < 1e-12);
        let skew = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::new(0.0, 0.0), 0.0),
                PolylineVertex::new(Point2::new(2.0, 0.0), 0.0),
                PolylineVertex::new(Point2::new(3.0, 1.0), 0.0),
                PolylineVertex::new(Point2::new(0.0, 1.0), 0.0),
            ],
            true,
        )
        .unwrap();
        assert_eq!(clip_of(&skew, &options).unwrap().rectangle, None);
    }

    #[test]
    fn intervals_follow_the_pattern_and_scale() {
        use arc_draft::linetype::Element;
        let pattern = Pattern::new(vec![
            Element {
                kind: ElementKind::Dash,
                length: 2.0,
            },
            Element {
                kind: ElementKind::Gap,
                length: 1.0,
            },
            Element {
                kind: ElementKind::Dot,
                length: 0.5,
            },
            Element {
                kind: ElementKind::Gap,
                length: 0.5,
            },
        ])
        .unwrap();
        let (i, period) = intervals(&pattern, 2.0);
        assert_eq!(i, vec![[0.0, 4.0], [6.0, 6.0]]);
        assert_eq!(period, 8.0);
    }
}
