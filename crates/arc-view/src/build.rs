//! Building a view: classification, sections, joining, hidden lines, merging, crop, fills,
//! symbols and back-links.

use std::collections::{BTreeMap, HashSet};

use arc_base::math::{Point2, Point3};
use arc_base::{Cancel, Error, ObjectId, Result, Tolerance};
use arc_draft::appearance::{Source, Sourced};
use arc_draft::clip::{Piece, cut};
use arc_draft::display::{Primitive, Shape};
use arc_draft::hatch::{Mark, Placement, generate};
use arc_geom::edit::Composite2;
use arc_geom::{
    Aabb2, BooleanOp, Curve2, Region, Segment2, SpatialIndex2, boolean, build_region, triangulate,
};

use crate::def::ViewDef;
use crate::error::codes;
use crate::hidden::{Occlusion, complement};
use crate::merge::{Seg, merge};
use crate::mesh::{Prepared, mesh_limits, prepare, section, too_many};
use crate::result::{BackLink, Diagnostic, PrimitiveKind, ViewResult, VisibilityClass};
use crate::source::{
    ClassStyles, CutFill, LineStyle, SourceObject, Symbol, ViewSource, apply, sanitize,
};

/// How a part relates to the ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placing {
    Cut,
    Beyond,
    Overhead,
}

/// An object in scope: its identifier, styles and symbols.
struct Object {
    id: ObjectId,
    styles: ClassStyles,
    symbols: Vec<Symbol>,
}

/// One part of an object in range.
struct Unit {
    object: usize,
    key: u32,
    fill: Option<CutFill>,
    join: Option<u64>,
    mesh: Prepared,
    placing: Placing,
}

/// A cut region of one or more joined units.
struct Group {
    members: Vec<usize>,
    region: Region,
}

/// One section segment with its unit, for the attribution of boundary lines.
#[derive(Clone, Copy)]
struct CutSeg {
    unit: usize,
    seg: Seg,
}

/// The lines to draw, by (draw order, object, unit, class).
type Lines = BTreeMap<(i64, usize, usize, VisibilityClass), Vec<Seg>>;

type Diagnostics = Vec<(usize, Diagnostic)>;

/// Cut regions with their unit.
type UnitRegions = Vec<(usize, Region)>;

const ORDER_FILL: i64 = 0;
const ORDER_PATTERN: i64 = 1;

/// The draw order of a line class: beyond, hidden, visible and silhouette, cut, overhead.
fn order(class: VisibilityClass) -> i64 {
    match class {
        VisibilityClass::Beyond => 2,
        VisibilityClass::Hidden => 3,
        VisibilityClass::Visible | VisibilityClass::Silhouette => 4,
        VisibilityClass::Cut => 5,
        VisibilityClass::Overhead => 6,
    }
}

fn diagnose(diagnostics: &mut Diagnostics, at: usize, object: ObjectId, error: Error) {
    diagnostics.push((at, Diagnostic { object, error }));
}

/// Builds the view `def` of the objects `source` supplies; see `docs/crates/arc-view.md`,
/// section "Plan views as implemented", for the rules.
///
/// Each part is classified by the height span of its geometry in view space against the cut
/// height `h`, applying the rule that a face lying in the cut plane belongs to the part above
/// it: cut when the span starts below `h` and reaches `h`; beyond when it ends below `h` and
/// reaches `bottom`; overhead when it starts at or above `h` below `top`; otherwise omitted.
///
/// The view is rebuilt whole. Its cost is bounded by `def.limits`, and `cancel` is checked
/// between objects, parts and unions.
///
/// # Errors
///
/// `view.definition.invalid` for an invalid definition or tolerance; `view.limit.exceeded` with
/// the argument `limit` when a bound is exceeded; `base.cancelled`; the errors of
/// [`ViewSource::objects`]. Errors of one object (its source, styles, geometry, section, fill or
/// crop) are diagnostics instead.
pub fn build_view(
    def: &ViewDef,
    source: &dyn ViewSource,
    tol: &Tolerance,
    cancel: &Cancel,
) -> Result<ViewResult> {
    def.validate(tol)?;
    let limits = def.limits;
    let mut ids = source.objects(def)?;
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(*id));
    if ids.len() > limits.max_objects {
        return Err(too_many("objects"));
    }
    let mut diagnostics: Diagnostics = Vec::new();
    let (objects, mut units) = load(def, source, &ids, tol, cancel, &mut diagnostics)?;
    let h = def.range.cut;
    let mut lines: Lines = BTreeMap::new();
    let mut edges = units.iter().map(|u| u.mesh.edges.len()).sum::<usize>();
    let (regions, cut_segs) = sections(
        &objects,
        &mut units,
        h,
        tol,
        def,
        &mut edges,
        &mut lines,
        &mut diagnostics,
    )?;
    let groups = join_regions(
        regions,
        &units,
        &objects,
        tol,
        def,
        cancel,
        &mut diagnostics,
    )?;

    // Occluders: surfaces below the cut and the cut regions.
    let mut occlusion = Occlusion::default();
    for unit in &units {
        if matches!(unit.placing, Placing::Cut | Placing::Beyond) {
            occlusion.add_mesh(&unit.mesh, h, tol)?;
        }
    }
    for group in &groups {
        let cap = triangulate(
            &group.region,
            tol.tessellation_chord,
            tol.tessellation_angle,
            limits.max_triangles,
        );
        match cap {
            Ok(cap) => occlusion.add_cap(&cap, tol)?,
            Err(error) => {
                if let Some(unit) = group.members.first().and_then(|&m| units.get(m))
                    && let Some(object) = objects.get(unit.object)
                {
                    diagnose(&mut diagnostics, unit.object, object.id, error);
                }
            }
        }
    }

    let mut budget = limits.max_occlusion_tests;
    hidden_lines(
        &occlusion,
        &objects,
        &units,
        def,
        tol,
        cancel,
        &mut budget,
        &mut lines,
    )?;

    let mut out = Output {
        result: ViewResult::default(),
        max: limits.max_primitives,
    };
    let origin = def.direction.to_view(Point3::origin());
    let origin = Point2::new(origin.x, origin.y);
    fills(
        &groups,
        &objects,
        &units,
        &cut_segs,
        def,
        origin,
        tol,
        &mut lines,
        &mut out,
        &mut diagnostics,
    )?;
    draw(
        &objects,
        &units,
        lines,
        def,
        tol,
        &mut budget,
        &mut out,
        &mut diagnostics,
    )?;
    diagnostics.sort_by_key(|d| d.0);
    out.result.diagnostics = diagnostics.into_iter().map(|d| d.1).collect();
    Ok(out.result)
}

/// Loads, styles, checks, prepares and classifies the objects in scope.
fn load(
    def: &ViewDef,
    source: &dyn ViewSource,
    ids: &[ObjectId],
    tol: &Tolerance,
    cancel: &Cancel,
    diagnostics: &mut Diagnostics,
) -> Result<(Vec<Object>, Vec<Unit>)> {
    let limits = def.limits;
    let (cut, bottom, top) = (def.range.cut, def.range.bottom, def.range.top);
    let mut objects = Vec::new();
    let mut units = Vec::new();
    let (mut triangles, mut edges) = (0_usize, 0_usize);
    for &id in ids {
        cancel.check()?;
        let at = objects.len();
        let mut content: SourceObject = match source.object(id, def) {
            Ok(Some(content)) => content,
            Ok(None) => continue,
            Err(error) => {
                diagnose(diagnostics, at, id, error);
                continue;
            }
        };
        if !apply(&def.overrides.of(id, &content.category), &mut content) {
            continue;
        }
        for error in sanitize(&mut content) {
            diagnose(diagnostics, at, id, error);
        }
        let bound = limits.max_coordinate;
        let before = content.symbols.len();
        content.symbols.retain(|s| {
            s.curve.bounding_box().is_some_and(|b| {
                [b.min().x, b.min().y, b.max().x, b.max().y]
                    .iter()
                    .all(|c| c.abs() <= bound)
            })
        });
        if content.symbols.len() < before {
            diagnose(
                diagnostics,
                at,
                id,
                Error::new(codes::GEOMETRY_OUT_OF_RANGE).with_detail("what", "symbol"),
            );
        }
        for part in content.parts {
            cancel.check()?;
            let mesh = match prepare(
                &part.geometry,
                def.direction,
                def.crease_angle,
                bound,
                limits.max_triangles,
                tol,
            ) {
                Ok(mesh) => mesh,
                Err(error) => {
                    diagnose(
                        diagnostics,
                        at,
                        id,
                        error.with_detail("part", i64::from(part.key)),
                    );
                    continue;
                }
            };
            // A face in the cut plane belongs to the part above it, as in the section.
            let placing = if mesh.z_min < cut && mesh.z_max >= cut {
                Placing::Cut
            } else if mesh.z_max < cut && mesh.z_max >= bottom {
                Placing::Beyond
            } else if mesh.z_min >= cut && top.is_some_and(|t| mesh.z_min < t) {
                Placing::Overhead
            } else {
                continue;
            };
            triangles = triangles.saturating_add(mesh.triangles.len());
            edges = edges.saturating_add(mesh.edges.len());
            if triangles > limits.max_triangles {
                return Err(too_many("triangles"));
            }
            if edges > limits.max_edges {
                return Err(too_many("edges"));
            }
            units.push(Unit {
                object: at,
                key: part.key,
                fill: part.cut_fill,
                join: part.join,
                mesh,
                placing,
            });
        }
        objects.push(Object {
            id,
            styles: content.styles,
            symbols: content.symbols,
        });
    }
    Ok((objects, units))
}

/// The regions and section segments of the cut units; open or invalid sections are filed as cut
/// lines with a diagnostic.
#[expect(clippy::too_many_arguments, reason = "the parts of one build step")]
fn sections(
    objects: &[Object],
    units: &mut [Unit],
    h: f64,
    tol: &Tolerance,
    def: &ViewDef,
    edges: &mut usize,
    lines: &mut Lines,
    diagnostics: &mut Diagnostics,
) -> Result<(UnitRegions, Vec<CutSeg>)> {
    let mut regions = Vec::new();
    let mut cut_segs = Vec::new();
    for (index, unit) in units.iter_mut().enumerate() {
        if unit.placing != Placing::Cut {
            continue;
        }
        let Some(object) = objects.get(unit.object) else {
            continue;
        };
        let cut = section(&unit.mesh, h, tol);
        let segs: Vec<Seg> = cut
            .loops
            .iter()
            .flat_map(|c| chain_segments(c, true))
            .chain(cut.open.iter().flat_map(|c| chain_segments(c, false)))
            .collect();
        *edges = edges.saturating_add(segs.len());
        if *edges > def.limits.max_edges {
            return Err(too_many("edges"));
        }
        let key = (
            order(VisibilityClass::Cut),
            unit.object,
            index,
            VisibilityClass::Cut,
        );
        let part = i64::from(unit.key);
        if !cut.open.is_empty() {
            let error = Error::new(codes::SECTION_OPEN).with_detail("part", part);
            diagnose(diagnostics, unit.object, object.id, error);
            lines.entry(key).or_default().extend_from_slice(&segs);
            continue;
        }
        if cut.loops.is_empty() {
            continue;
        }
        let region = cut
            .loops
            .iter()
            .map(|c| {
                Composite2::new(
                    chain_segments(c, true)
                        .iter()
                        .filter_map(|s| Segment2::new(s.a, s.b).ok().map(Curve2::Segment))
                        .collect(),
                    tol,
                )
            })
            .collect::<Result<Vec<_>>>()
            .and_then(|c| build_region(&c, tol, &mesh_limits(def.limits.max_edges)));
        match region {
            Ok(region) => {
                cut_segs.extend(segs.iter().map(|&seg| CutSeg { unit: index, seg }));
                regions.push((index, region));
            }
            Err(error) => {
                let error = error.with_detail("part", part);
                diagnose(diagnostics, unit.object, object.id, error);
                lines.entry(key).or_default().extend_from_slice(&segs);
            }
        }
    }
    Ok((regions, cut_segs))
}

fn curve_count(region: &Region) -> u64 {
    region
        .parts()
        .iter()
        .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
        .map(|l| u64::try_from(l.curves().len()).unwrap_or(u64::MAX))
        .fold(0, u64::saturating_add)
}

fn region_box(region: &Region) -> Aabb2 {
    region
        .parts()
        .iter()
        .fold(Aabb2::EMPTY, |b, p| b.union(&p.outer().bounding_box()))
}

/// Joins the regions of units with the same join key.
///
/// The regions of one key are split into components of overlapping or touching boxes (found
/// with an R*-tree), so regions far apart are never united; each component is united as a
/// balanced tree of pairwise unions, which keeps operands small. The curves entering every
/// union count against `max_join_curves`. A union that fails leaves its operands unjoined, with
/// the error as a diagnostic of the second operand's object.
fn join_regions(
    regions: Vec<(usize, Region)>,
    units: &[Unit],
    objects: &[Object],
    tol: &Tolerance,
    def: &ViewDef,
    cancel: &Cancel,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<Group>> {
    let mut by_key: BTreeMap<u64, Vec<(usize, Region)>> = BTreeMap::new();
    let mut groups = Vec::new();
    for (unit, region) in regions {
        match units.get(unit).and_then(|u| u.join) {
            Some(key) => by_key.entry(key).or_default().push((unit, region)),
            None => groups.push(Group {
                members: vec![unit],
                region,
            }),
        }
    }
    let mut budget = def.limits.max_join_curves;
    let limits = mesh_limits(def.limits.max_edges);
    for (_, list) in by_key {
        for component in components(list, tol)? {
            let mut work: Vec<Group> = component
                .into_iter()
                .map(|(unit, region)| Group {
                    members: vec![unit],
                    region,
                })
                .collect();
            while work.len() > 1 {
                let mut next = Vec::with_capacity(work.len().div_ceil(2));
                let mut items = work.into_iter();
                while let Some(mut a) = items.next() {
                    let Some(b) = items.next() else {
                        next.push(a);
                        break;
                    };
                    cancel.check()?;
                    let curves = curve_count(&a.region).saturating_add(curve_count(&b.region));
                    budget = budget.checked_sub(curves).ok_or_else(|| too_many("join"))?;
                    match boolean(&a.region, &b.region, BooleanOp::Union, tol, &limits) {
                        Ok(Some(joined)) => {
                            a.region = joined;
                            a.members.extend(b.members);
                            next.push(a);
                        }
                        Ok(None) => next.push(a),
                        Err(error) => {
                            if let Some(unit) = b.members.first().and_then(|&m| units.get(m))
                                && let Some(object) = objects.get(unit.object)
                            {
                                diagnose(diagnostics, unit.object, object.id, error);
                            }
                            groups.push(b);
                            next.push(a);
                        }
                    }
                }
                work = next;
            }
            groups.extend(work);
        }
    }
    for group in &mut groups {
        group.members.sort_unstable();
    }
    groups.sort_by_key(|g| g.members.first().copied());
    Ok(groups)
}

/// Splits regions into groups whose boxes overlap or touch, transitively, in input order.
fn components(list: Vec<(usize, Region)>, tol: &Tolerance) -> Result<Vec<Vec<(usize, Region)>>> {
    let near = 2.0 * tol.linear;
    let boxes: Vec<Aabb2> = list
        .iter()
        .map(|(_, r)| region_box(r).expanded(near))
        .collect();
    let index = SpatialIndex2::from_entries((0_u32..).zip(boxes.iter().copied()))?;
    let mut parent: Vec<usize> = (0..list.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while let Some(&p) = parent.get(i) {
            if p == i {
                break;
            }
            let grand = parent.get(p).copied().unwrap_or(p);
            if let Some(slot) = parent.get_mut(i) {
                *slot = grand;
            }
            i = p;
        }
        i
    }
    for (i, b) in boxes.iter().enumerate() {
        for other in index.crossing(b)? {
            let Ok(j) = usize::try_from(other) else {
                continue;
            };
            let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
            if ri != rj
                && let Some(slot) = parent.get_mut(ri.max(rj))
            {
                *slot = ri.min(rj);
            }
        }
    }
    let mut out: BTreeMap<usize, Vec<(usize, Region)>> = BTreeMap::new();
    for (i, item) in list.into_iter().enumerate() {
        let r = root(&mut parent, i);
        out.entry(r).or_default().push(item);
    }
    Ok(out.into_values().collect())
}

/// Clips, occludes and files the edges of every unit.
#[expect(clippy::too_many_arguments, reason = "the parts of one build step")]
fn hidden_lines(
    occlusion: &Occlusion,
    objects: &[Object],
    units: &[Unit],
    def: &ViewDef,
    tol: &Tolerance,
    cancel: &Cancel,
    budget: &mut u64,
    lines: &mut Lines,
) -> Result<()> {
    let h = def.range.cut;
    let crop_box = def.crop.as_ref().map(region_box);
    let (bottom, top) = (def.range.bottom, def.range.top);
    for (index, unit) in units.iter().enumerate() {
        cancel.check()?;
        let Some(object) = objects.get(unit.object) else {
            continue;
        };
        let (lo, hi, class) = match unit.placing {
            Placing::Cut => (bottom, h + tol.linear, VisibilityClass::Visible),
            Placing::Beyond => (bottom, h + tol.linear, VisibilityClass::Beyond),
            Placing::Overhead => (h - tol.linear, top.unwrap_or(h), VisibilityClass::Overhead),
        };
        let draws_hidden = object.styles.hidden.is_some();
        for edge in &unit.mesh.edges {
            let Some((p, q)) = clip_z(edge.a, edge.b, lo, hi) else {
                continue;
            };
            let (a, b) = (Point2::new(p.x, p.y), Point2::new(q.x, q.y));
            if (b - a).norm() <= tol.linear {
                continue;
            }
            if let Some(boxed) = &crop_box
                && !boxed.intersects(&Aabb2::from_points(&[a, b]).expanded(tol.linear))
            {
                continue;
            }
            let hidden = if class == VisibilityClass::Overhead {
                Vec::new()
            } else {
                // shortcut: one thread; split the edges by screen tile and run tiles in parallel
                // when plans of large models need it.
                occlusion.hidden(p, q, tol, budget)?
            };
            let piece = |&(t0, t1): &(f64, f64)| Seg {
                a: a + (b - a) * t0,
                b: a + (b - a) * t1,
                key: edge.key,
            };
            lines
                .entry((order(class), unit.object, index, class))
                .or_default()
                .extend(complement(&hidden).iter().map(piece));
            if draws_hidden && !hidden.is_empty() {
                let hidden_class = VisibilityClass::Hidden;
                lines
                    .entry((order(hidden_class), unit.object, index, hidden_class))
                    .or_default()
                    .extend(hidden.iter().map(piece));
            }
        }
    }
    Ok(())
}

/// Merges, crops and emits the lines, then the symbols, in draw order.
#[expect(clippy::too_many_arguments, reason = "the parts of one build step")]
fn draw(
    objects: &[Object],
    units: &[Unit],
    lines: Lines,
    def: &ViewDef,
    tol: &Tolerance,
    budget: &mut u64,
    out: &mut Output,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    // Symbols take the draw order of their class, after the object's lines of that class.
    let mut symbols: BTreeMap<(i64, usize, usize), &Symbol> = BTreeMap::new();
    for (o, object) in objects.iter().enumerate() {
        for (k, symbol) in object.symbols.iter().enumerate() {
            symbols.insert((order(symbol.class), o, k), symbol);
        }
    }
    let mut pending = symbols.into_iter().peekable();
    let mut emit_symbols_before = |limit: Option<(i64, usize)>,
                                   out: &mut Output,
                                   budget: &mut u64,
                                   diagnostics: &mut Diagnostics|
     -> Result<()> {
        while let Some(&((order_key, o, _), symbol)) = pending.peek() {
            if limit.is_some_and(|l| (order_key, o) >= l) {
                break;
            }
            pending.next();
            let Some(object) = objects.get(o) else {
                continue;
            };
            let Some(style) = symbol
                .style
                .as_ref()
                .or_else(|| object.styles.get(symbol.class))
            else {
                continue;
            };
            let pieces = match crop_curve(&symbol.curve, def.crop.as_ref(), tol, budget) {
                Ok(pieces) => pieces,
                Err(error) if error.code() == codes::LIMIT_EXCEEDED.code => return Err(error),
                Err(error) => {
                    diagnose(diagnostics, o, object.id, error);
                    continue;
                }
            };
            for curve in pieces {
                out.push(
                    Shape::Stroke { curve, drawn: None },
                    line_appearance(style),
                    order(symbol.class),
                    BackLink {
                        source: object.id,
                        sub_key: symbol.key,
                        face: None,
                        class: symbol.class,
                        kind: PrimitiveKind::Symbol,
                        joined: Vec::new(),
                    },
                )?;
            }
        }
        Ok(())
    };
    for ((order_key, o, u, class), segs) in lines {
        emit_symbols_before(Some((order_key, o)), out, budget, diagnostics)?;
        let (Some(object), Some(unit)) = (objects.get(o), units.get(u)) else {
            continue;
        };
        let Some(style) = object.styles.get(class) else {
            continue;
        };
        for seg in merge(segs, tol) {
            let Ok(segment) = Segment2::new(seg.a, seg.b) else {
                continue;
            };
            if (seg.b - seg.a).norm() <= tol.linear {
                continue;
            }
            for curve in crop_curve(&Curve2::Segment(segment), def.crop.as_ref(), tol, budget)? {
                out.push(
                    Shape::Stroke { curve, drawn: None },
                    line_appearance(style),
                    order_key,
                    BackLink {
                        source: object.id,
                        sub_key: unit.key,
                        face: seg.key,
                        class,
                        kind: PrimitiveKind::Line,
                        joined: Vec::new(),
                    },
                )?;
            }
        }
    }
    emit_symbols_before(None, out, budget, diagnostics)
}

/// The segments of a chain, closing it when `closed`.
fn chain_segments(chain: &crate::mesh::Chain, closed: bool) -> Vec<Seg> {
    let n = chain.points.len();
    let count = if closed { n } else { n.saturating_sub(1) };
    (0..count)
        .filter_map(|i| {
            Some(Seg {
                a: *chain.points.get(i)?,
                b: *chain.points.get((i + 1) % n)?,
                key: chain.keys.get(i).copied().flatten(),
            })
        })
        .collect()
}

/// The part of the segment `a`–`b` with `lo <= z <= hi`.
fn clip_z(a: Point3, b: Point3, lo: f64, hi: f64) -> Option<(Point3, Point3)> {
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    let dz = b.z - a.z;
    for (bound, below) in [(lo, true), (hi, false)] {
        if dz == 0.0 {
            let outside = if below { a.z < bound } else { a.z > bound };
            if outside {
                return None;
            }
            continue;
        }
        let t = (bound - a.z) / dz;
        // Inside is z >= lo for the first bound and z <= hi for the second.
        if (dz > 0.0) == below {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
    }
    (t0 < t1).then(|| (a + (b - a) * t0, a + (b - a) * t1))
}

/// The pieces of `curve` inside or on `crop` (all of it without a crop): inside the outer loop
/// of a part and outside its holes, for every part. Every loop curve tested counts against
/// `budget`.
fn crop_curve(
    curve: &Curve2,
    crop: Option<&Region>,
    tol: &Tolerance,
    budget: &mut u64,
) -> Result<Vec<Curve2>> {
    let Some(crop) = crop else {
        return Ok(vec![curve.clone()]);
    };
    let mut out = Vec::new();
    for part in crop.parts() {
        let mut spend = |n: usize| -> Result<()> {
            *budget = budget
                .checked_sub(u64::try_from(n).unwrap_or(u64::MAX))
                .ok_or_else(|| too_many("occlusion_tests"))?;
            Ok(())
        };
        spend(part.outer().curves().len())?;
        let mut pieces: Vec<Curve2> = curves_of(cut(curve, part.outer(), true, tol)?);
        for hole in part.holes() {
            let mut kept = Vec::new();
            for piece in pieces {
                spend(hole.curves().len())?;
                kept.extend(curves_of(cut(&piece, hole, false, tol)?));
            }
            pieces = kept;
        }
        out.extend(pieces);
    }
    Ok(out)
}

fn curves_of(pieces: Vec<Piece>) -> Vec<Curve2> {
    pieces
        .into_iter()
        .filter_map(|p| match p {
            Piece::Curve(c) => Some(c),
            Piece::Point(_) => None,
        })
        .collect()
}

struct Output {
    result: ViewResult,
    max: usize,
}

struct Appearance {
    color: arc_doc::ColorValue,
    linetype: Option<ObjectId>,
    lineweight: Option<f64>,
    layer: Option<ObjectId>,
}

impl Output {
    fn push(
        &mut self,
        shape: Shape,
        appearance: Appearance,
        draw_order: i64,
        link: BackLink,
    ) -> Result<()> {
        if self.result.links.len() >= self.max {
            return Err(too_many("primitives"));
        }
        self.result.list.primitives.push(Primitive {
            shape,
            color: Some(Sourced {
                value: appearance.color,
                source: Source::Object,
            }),
            linetype: Some(Sourced {
                value: appearance.linetype,
                source: Source::Object,
            }),
            lineweight: Some(Sourced {
                value: appearance.lineweight,
                source: Source::Object,
            }),
            transparency: Some(Sourced {
                value: 0.0,
                source: Source::Default,
            }),
            linetype_scale: 1.0,
            continuous: false,
            layers: appearance.layer.into_iter().collect(),
            draw_order,
            source: link.source,
            path: Vec::new(),
            part: 0,
            annotation_scale: None,
        });
        self.result.links.push(link);
        Ok(())
    }
}

fn line_appearance(style: &LineStyle) -> Appearance {
    Appearance {
        color: style.color.clone(),
        linetype: style.linetype,
        lineweight: style.lineweight,
        layer: style.layer,
    }
}

fn fill_appearance(fill: &CutFill) -> Appearance {
    Appearance {
        color: fill.color.clone(),
        linetype: None,
        lineweight: None,
        layer: fill.layer,
    }
}

/// Crops each group's region, emits its fill or pattern and files its boundary pieces as cut
/// lines of the unit whose section they lie on; pieces on no section (the crop boundary) are
/// not drawn. A crop or pattern that fails is a diagnostic: the region is left out, or filled
/// solid.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the parts of one build step"
)]
fn fills(
    groups: &[Group],
    objects: &[Object],
    units: &[Unit],
    cut_segs: &[CutSeg],
    def: &ViewDef,
    origin: Point2,
    tol: &Tolerance,
    lines: &mut Lines,
    out: &mut Output,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    let near = 2.0 * tol.linear;
    let index = SpatialIndex2::from_entries(
        cut_segs
            .iter()
            .zip(0_u32..)
            .map(|(c, k)| (k, Aabb2::from_points(&[c.seg.a, c.seg.b]).expanded(near))),
    )?;
    for group in groups {
        let Some(first_unit) = group.members.first().and_then(|&m| units.get(m)) else {
            continue;
        };
        let Some(first) = objects.get(first_unit.object) else {
            continue;
        };
        let region = match &def.crop {
            Some(crop) => match boolean(
                &group.region,
                crop,
                BooleanOp::Intersection,
                tol,
                &mesh_limits(def.limits.max_edges),
            ) {
                Ok(region) => region,
                Err(error) => {
                    diagnose(diagnostics, first_unit.object, first.id, error);
                    continue;
                }
            },
            None => Some(group.region.clone()),
        };
        let Some(region) = region else { continue };
        let joined: Vec<(ObjectId, u32)> = group
            .members
            .iter()
            .skip(1)
            .filter_map(|&m| {
                let u = units.get(m)?;
                Some((objects.get(u.object)?.id, u.key))
            })
            .collect();
        let link = |kind| BackLink {
            source: first.id,
            sub_key: first_unit.key,
            face: None,
            class: VisibilityClass::Cut,
            kind,
            joined: joined.clone(),
        };
        if let Some(fill) = &first_unit.fill {
            let mut marks = None;
            if !fill.lines.is_empty() {
                let placement = Placement {
                    scale: fill.scale,
                    angle: fill.angle,
                    origin,
                };
                let room = out
                    .max
                    .saturating_sub(out.result.links.len())
                    .min(def.limits.max_pattern_marks);
                match generate(&region, &fill.lines, &placement, room, tol) {
                    Ok(g) if !g.fallback => marks = Some(g.marks),
                    Ok(_) => {}
                    Err(error) => diagnose(diagnostics, first_unit.object, first.id, error),
                }
            }
            match marks {
                Some(marks) => {
                    for mark in marks {
                        let shape = match mark {
                            Mark::Segment(a, b) => match Segment2::new(a, b) {
                                Ok(s) => Shape::Stroke {
                                    curve: Curve2::Segment(s),
                                    drawn: None,
                                },
                                Err(_) => continue,
                            },
                            Mark::Dot(p) => Shape::Point(p),
                        };
                        out.push(
                            shape,
                            fill_appearance(fill),
                            ORDER_PATTERN,
                            link(PrimitiveKind::Pattern),
                        )?;
                    }
                }
                None => out.push(
                    Shape::Region(region.clone()),
                    fill_appearance(fill),
                    ORDER_FILL,
                    link(PrimitiveKind::Fill),
                )?,
            }
        }
        for part in region.parts() {
            for lp in std::iter::once(part.outer()).chain(part.holes()) {
                for c in lp.curves() {
                    let Curve2::Segment(s) = c else { continue };
                    let mid = s.point_at(0.5);
                    let owner = index
                        .crossing(&Aabb2::from_points(&[mid]).expanded(near))?
                        .into_iter()
                        .filter_map(|k| usize::try_from(k).ok().and_then(|k| cut_segs.get(k)))
                        .find(|c| {
                            group.members.contains(&c.unit)
                                && distance_to_segment(mid, c.seg.a, c.seg.b) <= near
                        });
                    if let Some(owner) = owner
                        && let Some(unit) = units.get(owner.unit)
                    {
                        let class = VisibilityClass::Cut;
                        lines
                            .entry((order(class), unit.object, owner.unit, class))
                            .or_default()
                            .push(Seg {
                                a: s.start(),
                                b: s.end(),
                                key: owner.seg.key,
                            });
                    }
                }
            }
        }
    }
    Ok(())
}

fn distance_to_segment(p: Point2, a: Point2, b: Point2) -> f64 {
    let d = b - a;
    let len2 = d.norm_squared();
    let t = if len2 > 0.0 {
        ((p - a).dot(&d) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + d * t)).norm()
}

#[cfg(test)]
mod tests {
    use super::clip_z;
    use arc_base::math::Point3;

    #[test]
    fn clip_to_a_height_band() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(0.0, 0.0, 4.0);
        let (p, q) = clip_z(a, b, 1.0, 3.0).expect("inside");
        assert_eq!((p.z, q.z), (1.0, 3.0));
        let (p, q) = clip_z(b, a, 1.0, 3.0).expect("inside");
        assert_eq!((p.z, q.z), (3.0, 1.0));
        assert!(clip_z(a, a, 1.0, 3.0).is_none());
        assert!(clip_z(a, Point3::new(1.0, 0.0, 0.0), 0.0, 3.0).is_some());
    }
}
