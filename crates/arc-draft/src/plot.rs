//! Plot streams (CORE-DRAFT-031): the ordered, pen-resolved primitives of a page in paper
//! millimeters.
//!
//! [`layout_stream`] plots a layout space: its sheet, its viewports and its own entities.
//! [`model_stream`] plots a window of a model space at a scale on a paper. Both produce a
//! [`PlotStream`] whose coordinates are millimeters from the lower left corner of the page. The
//! rules are stated in the crate specification, section "Plotting as implemented".
//!
//! The stream is deterministic: it is built from sorted identifiers and ordered vectors only,
//! and [`PlotStream::encode`] writes every number as its bit pattern, so identical input gives
//! identical bytes.

use std::collections::BTreeMap;

use arc_base::id::Identifier;
use arc_base::math::{Matrix3, Point2, Transform2};
use arc_base::{Color, Error, Limits, ObjectId, Result};
use arc_doc::relationship::ReferenceField;
use arc_doc::{Snapshot, Value};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Aabb2, Curve2, Loop2, Polyline2, PolylineVertex, Region};

use crate::appearance::default_color;
use crate::clip::{Piece, cut, piece};
use crate::display::{
    DisplayList, DisplayOptions, Lists, Primitive, Shape, space_list, transform_polyline,
    transform_shape, viewport_list,
};
use crate::error::codes;
use crate::layout::{LAYOUT, VIEWPORT, settings, sheet_size, view};
use crate::linetype::{DashOptions, Dashes, Pattern, dashes};
use crate::plot_style::{
    Appearance, DEFAULT_LINEWEIGHT_MM, PLOT_STYLE_TABLE, Pen, PlotStyleTable, resolve_pen_on_paper,
};
use crate::text_layout::PositionedGlyph;
use crate::types::{SPACE, SpaceUnit, entity, f64_at, layer, table, text_at};

/// The paper a model-space plot goes onto, in millimeters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paper {
    /// The page width.
    pub width: f64,
    /// The page height.
    pub height: f64,
    /// The unprintable margins: left, bottom, right, top.
    pub margins: [f64; 4],
}

/// The part of the source that is plotted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlotArea {
    /// The printable area of a layout's sheet. For a model space: the extents.
    Layout,
    /// The bounding box of everything drawn.
    Extents,
    /// A window in the source's coordinates; `min` is below and left of `max`.
    Window {
        /// The lower left corner.
        min: Point2,
        /// The upper right corner.
        max: Point2,
    },
}

/// The scale of a plot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlotScale {
    /// The plot area, turned as `rotation` says, fills the printable area as far as the
    /// aspect ratios allow.
    Fit,
    /// Paper length per source length, both in one unit: 1:100 is 0.01 whatever the units.
    Ratio(f64),
}

/// Where the plot area goes on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Offset {
    /// The lower left corner of the plotted area lies at this offset in millimeters from the
    /// lower left corner of the printable area.
    Origin(Point2),
    /// The plotted area is centered in the printable area.
    Center,
}

/// The options of a plot.
#[derive(Debug, Clone, PartialEq)]
pub struct PlotOptions {
    /// The plotted part of the source.
    pub area: PlotArea,
    /// The scale.
    pub scale: PlotScale,
    /// The placement on the page.
    pub offset: Offset,
    /// Quarter turns counterclockwise, reduced modulo 4.
    pub rotation: u8,
    /// Whether lineweights plot. Off: every pen is as wide as the default lineweight.
    pub lineweights: bool,
    /// Whether plot style tables apply. Off: every pen is the object's.
    pub styles: bool,
    /// The name of the plot style table; none: no table.
    pub style_table: Option<String>,
    /// The paper color, the color of masks.
    pub paper_color: Color,
    /// The most items of the stream.
    pub max_items: usize,
}

impl Default for PlotOptions {
    fn default() -> Self {
        Self {
            area: PlotArea::Extents,
            scale: PlotScale::Fit,
            offset: Offset::Center,
            rotation: 0,
            lineweights: true,
            styles: true,
            style_table: None,
            paper_color: Color::WHITE,
            max_items: usize::try_from(Limits::DEFAULT.array_length).unwrap_or(usize::MAX),
        }
    }
}

impl PlotOptions {
    /// The options a layout stores: `plot_area` with its window, `plot_scale`, `origin` (in
    /// millimeters after the unit conversion) and `plot_style_table`. Rotation 0, lineweights and
    /// styles on, white paper.
    ///
    /// # Errors
    ///
    /// `draft.plot.source_invalid` when `layout` is not a layout space with settings.
    pub fn from_layout(snapshot: &Snapshot, layout: ObjectId) -> Result<Self> {
        let sheet = Sheet::read(snapshot, layout)?;
        let v = &sheet.values;
        let point = |i| match v.get(i) {
            Some(Some(Value::Point2(p))) => Some(*p),
            _ => None,
        };
        let area = match v.get(settings::PLOT_AREA) {
            Some(Some(Value::Enum(1))) => PlotArea::Extents,
            Some(Some(Value::Enum(2))) => {
                match (point(settings::WINDOW_MIN), point(settings::WINDOW_MAX)) {
                    (Some(min), Some(max)) => PlotArea::Window { min, max },
                    _ => PlotArea::Layout,
                }
            }
            _ => PlotArea::Layout,
        };
        let origin = point(settings::ORIGIN).unwrap_or_else(Point2::origin);
        Ok(Self {
            area,
            scale: PlotScale::Ratio(f64_at(v, settings::PLOT_SCALE).unwrap_or(1.0)),
            offset: Offset::Origin(Point2::new(
                origin.x * sheet.factor,
                origin.y * sheet.factor,
            )),
            style_table: text_at(v, settings::PLOT_STYLE_TABLE)
                .filter(|n| !n.is_empty())
                .map(str::to_owned),
            ..Self::default()
        })
    }
}

/// The geometry of an item and its pen.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// A point entity, drawn as a dot of the pen's width.
    Point {
        /// The position.
        at: Point2,
        /// The pen.
        pen: Pen,
    },
    /// An exact curve with the pieces that are drawn when that is not the whole curve (dashes,
    /// or what a cut left); a curve that is unbounded is drawn only through its pieces.
    Stroke {
        /// The whole curve.
        curve: Curve2,
        /// The drawn pieces; `None` draws the whole curve.
        pieces: Option<Vec<Piece>>,
        /// The pen.
        pen: Pen,
    },
    /// A filled region.
    Fill {
        /// The closed boundary.
        region: Polyline2,
        /// The pen, whose color fills.
        pen: Pen,
    },
    /// A filled region with holes: every loop of every part, filled with the even-odd rule.
    RegionFill {
        /// The region.
        region: Region,
        /// The pen, whose color fills.
        pen: Pen,
    },
    /// A wipeout mask: the region is painted in the paper color.
    Mask {
        /// The closed boundary.
        region: Polyline2,
    },
    /// Outline glyphs: positions in text space, mapped to the page by `placement`.
    Glyphs {
        /// The font family the provider registered the font under.
        family: String,
        /// The face index.
        index: u32,
        /// The em size in text space.
        size: f64,
        /// The glyphs.
        glyphs: Vec<PositionedGlyph>,
        /// Text space to the page in millimeters.
        placement: Transform2,
        /// The pen, whose color fills.
        pen: Pen,
    },
}

/// One item of a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// The entity that produced it (in the plotted space or the viewport's space).
    pub source: ObjectId,
    /// The part of the source, as `Primitive::part`.
    pub part: u32,
    /// The layer of the drawing entity, for optional-content groups.
    pub layer: Option<ObjectId>,
    /// The index into [`PlotStream::clips`] of the region the item is clipped to.
    pub clip: Option<usize>,
    /// The geometry and pen.
    pub kind: ItemKind,
}

/// The stream of one page.
#[derive(Debug, Clone, PartialEq)]
pub struct PlotStream {
    /// The page width in millimeters.
    pub width: f64,
    /// The page height in millimeters.
    pub height: f64,
    /// The paper color.
    pub paper_color: Color,
    /// The lower left and upper right corners of the plotted area on the page: the plot area
    /// placed on the page, cut to the printable area. Nothing is drawn outside it.
    pub area: [Point2; 2],
    /// The clip regions of viewports, closed polylines on the page.
    pub clips: Vec<Polyline2>,
    /// The items in drawing order.
    pub items: Vec<Item>,
    /// The entities and viewports left out because they could not be listed, mapped or cut,
    /// in ascending order.
    pub failed: Vec<ObjectId>,
}

/// A group of primitives in source coordinates with the clip of its viewport.
struct Group {
    clip: Option<Polyline2>,
    /// The scale of the viewport map, the factor between a pattern length in the viewport's
    /// model space and the sheet.
    pattern_scale: f64,
    primitives: Vec<Primitive>,
}

/// The layout settings of a layout space.
struct Sheet {
    values: Vec<Option<Value>>,
    /// The factor from the layout's unit to millimeters.
    factor: f64,
}

fn type_is(snapshot: &Snapshot, id: ObjectId, key: &str) -> bool {
    snapshot
        .resolve(id)
        .ok()
        .and_then(|h| snapshot.type_key(h))
        .is_some_and(|k| k.as_str() == key)
}

fn unit_of(snapshot: &Snapshot, space: ObjectId) -> SpaceUnit {
    snapshot
        .resolve(space)
        .ok()
        .and_then(|h| snapshot.get(h).ok())
        .and_then(|o| match o.values.get(table::SPACE_UNIT) {
            Some(Some(Value::Enum(code))) => SpaceUnit::of(*code),
            _ => None,
        })
        .unwrap_or(SpaceUnit::Unitless)
}

/// The objects of type `key` owned by `owner`, in ascending identifier order.
fn owned(snapshot: &Snapshot, owner: ObjectId, key: &str) -> Vec<ObjectId> {
    let mut out: Vec<ObjectId> = snapshot
        .referrers(owner)
        .filter(|r| r.field == ReferenceField::Owner && type_is(snapshot, r.source, key))
        .map(|r| r.source)
        .collect();
    out.sort();
    out
}

impl Sheet {
    fn read(snapshot: &Snapshot, layout: ObjectId) -> Result<Self> {
        let invalid =
            || Error::new(codes::PLOT_SOURCE_INVALID).with_detail("id", layout.to_string());
        if !type_is(snapshot, layout, SPACE) {
            return Err(invalid());
        }
        let settings_id = owned(snapshot, layout, LAYOUT)
            .into_iter()
            .next()
            .ok_or_else(invalid)?;
        let values = snapshot.get(snapshot.resolve(settings_id)?)?.values.clone();
        let unit = unit_of(snapshot, layout);
        Ok(Self {
            values,
            factor: unit.factor_to(SpaceUnit::Millimeter),
        })
    }
}

fn option_invalid(option: &'static str) -> Error {
    Error::new(codes::PLOT_OPTION_INVALID).with_arg("option", option)
}

fn positive(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

fn rectangle(min: Point2, max: Point2) -> Result<Polyline2> {
    let corner = |x: f64, y: f64| PolylineVertex::new(Point2::new(x, y), 0.0);
    Polyline2::new(
        vec![
            corner(min.x, min.y),
            corner(max.x, min.y),
            corner(max.x, max.y),
            corner(min.x, max.y),
        ],
        true,
    )
}

/// The bounding box of what a primitive draws; `None` for masks and unbounded curves.
fn extent(p: &Primitive) -> Option<Aabb2> {
    let join = |a: Option<Aabb2>, b: Option<Aabb2>| match (a, b) {
        (Some(a), Some(b)) => Some(a.union(&b)),
        (a, b) => a.or(b),
    };
    match &p.shape {
        Shape::Point(q) => Some(Aabb2::from_points(&[*q])),
        Shape::Stroke { curve, drawn } => match drawn {
            None => curve.bounding_box(),
            Some(pieces) => pieces.iter().fold(None, |acc, piece| {
                join(
                    acc,
                    match piece {
                        Piece::Point(q) => Some(Aabb2::from_points(&[*q])),
                        Piece::Curve(c) => c.bounding_box(),
                    },
                )
            }),
        },
        Shape::Fill(region) => Some(region.bounding_box()),
        Shape::Mask(_) => None,
        Shape::Region(region) => crate::display::region_box(region),
        Shape::Glyphs {
            glyphs, placement, ..
        } => {
            let points: Vec<Point2> = glyphs.iter().map(|g| placement * g.position).collect();
            (!points.is_empty()).then(|| Aabb2::from_points(&points))
        }
    }
}

/// The map from source coordinates to the page.
fn page_map(region: (Point2, Point2), k: f64, rotation: u8, origin: Point2) -> Transform2 {
    let (w, h) = ((region.1.x - region.0.x) * k, (region.1.y - region.0.y) * k);
    let (c, s, shx, shy) = match rotation % 4 {
        0 => (1.0, 0.0, 0.0, 0.0),
        1 => (0.0, 1.0, h, 0.0),
        2 => (-1.0, 0.0, w, h),
        _ => (0.0, -1.0, 0.0, w),
    };
    let (a, b, cc, d) = (c * k, -s * k, s * k, c * k);
    let tx = origin.x + shx - (a * region.0.x + b * region.0.y);
    let ty = origin.y + shy - (cc * region.0.x + d * region.0.y);
    Transform2::from_matrix_unchecked(Matrix3::new(a, b, tx, cc, d, ty, 0.0, 0.0, 1.0))
}

/// The scale factor of a similarity.
fn scale_of(t: &Transform2) -> f64 {
    let m = t.matrix();
    (m[(0, 0)] * m[(1, 1)] - m[(0, 1)] * m[(1, 0)]).abs().sqrt()
}

struct Context<'a> {
    snapshot: &'a Snapshot,
    display: &'a DisplayOptions,
    table: Option<PlotStyleTable>,
    options: &'a PlotOptions,
    names: BTreeMap<ObjectId, Option<String>>,
    patterns: BTreeMap<ObjectId, Option<Pattern>>,
}

impl Context<'_> {
    /// The plot style name of the drawing entity of `p`, else of its layer.
    fn style_name(&mut self, p: &Primitive) -> Option<String> {
        let id = p.path.last().copied().unwrap_or(p.source);
        let snapshot = self.snapshot;
        let read = |id: ObjectId, index: usize| -> Option<String> {
            let object = snapshot.get(snapshot.resolve(id).ok()?).ok()?;
            text_at(&object.values, index)
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
        };
        let own = self
            .names
            .entry(id)
            .or_insert_with(|| read(id, entity::PLOT_STYLE))
            .clone();
        own.or_else(|| {
            let layer_id = p.layers.first().copied()?;
            self.names
                .entry(layer_id)
                .or_insert_with(|| read(layer_id, layer::PLOT_STYLE))
                .clone()
        })
    }

    fn pattern(&mut self, linetype: ObjectId) -> Option<Pattern> {
        let snapshot = self.snapshot;
        self.patterns
            .entry(linetype)
            .or_insert_with(|| {
                let object = snapshot.get(snapshot.resolve(linetype).ok()?).ok()?;
                Pattern::from_values(&object.values).ok()
            })
            .clone()
    }

    fn pen(&mut self, p: &Primitive) -> Pen {
        let color = p
            .color
            .as_ref()
            .map_or_else(default_color, |c| c.value.clone());
        let name = if self.table.is_some() {
            self.style_name(p)
        } else {
            None
        };
        let appearance = Appearance {
            color: &color,
            lineweight: p.lineweight.as_ref().and_then(|l| l.value),
            linetype: p.linetype.as_ref().and_then(|l| l.value),
            transparency: p.transparency.as_ref().map_or(0.0, |t| t.value),
            style_name: name.as_deref(),
        };
        let mut pen =
            resolve_pen_on_paper(self.table.as_ref(), &appearance, self.options.paper_color);
        if !self.options.lineweights {
            pen.width = DEFAULT_LINEWEIGHT_MM;
        }
        pen
    }
}

/// The dashes of `curve` (on the page) with the pattern of the overriding linetype.
fn redash(
    cx: &mut Context<'_>,
    curve: &Curve2,
    pen: &Pen,
    p: &Primitive,
    scale: f64,
) -> Result<Option<Vec<Piece>>> {
    let Some(linetype) = pen.linetype else {
        return Ok(None);
    };
    let Some(pattern) = cx.pattern(linetype) else {
        return Ok(None);
    };
    let options = DashOptions {
        global_scale: 1.0,
        object_scale: p.linetype_scale * scale,
        continuous: p.continuous,
        max_dashes: cx.display.max_dashes,
    };
    match dashes(curve, &pattern, &options, &cx.display.tolerance)? {
        Dashes::Continuous => Ok(None),
        Dashes::Pieces(list) => Ok(Some(
            list.iter()
                .filter_map(|d| piece(curve, d.t0, d.t1).ok())
                .collect(),
        )),
    }
}

fn collect_viewports(
    snapshot: &Snapshot,
    lists: Lists<'_>,
    layout: ObjectId,
    display: &DisplayOptions,
    groups: &mut Vec<Group>,
    failed: &mut Vec<ObjectId>,
) {
    for vp in owned(snapshot, layout, VIEWPORT) {
        let listed = view(snapshot, vp)
            .and_then(|v| viewport_list(snapshot, lists, vp, display).map(|l| (v, l)));
        match listed {
            Ok((
                v,
                DisplayList {
                    primitives,
                    clip,
                    failed: f,
                },
            )) => {
                failed.extend(f);
                groups.push(Group {
                    clip: clip.or(Some(v.clip)),
                    pattern_scale: scale_of(&v.transform),
                    primitives,
                });
            }
            Err(_) => failed.push(vp),
        }
    }
}

/// Builds the stream from the groups, a plot area and a paper.
#[expect(clippy::too_many_lines, reason = "one pipeline from groups to items")]
fn build(
    snapshot: &Snapshot,
    mut groups: Vec<Group>,
    mut failed: Vec<ObjectId>,
    source_factor: f64,
    paper: &Paper,
    options: &PlotOptions,
    display: &DisplayOptions,
) -> Result<PlotStream> {
    let tol = &display.tolerance;
    if !positive(paper.width) || !positive(paper.height) {
        return Err(option_invalid("paper"));
    }
    if !paper.margins.iter().all(|m| m.is_finite() && *m >= 0.0)
        || paper.margins[0] + paper.margins[2] >= paper.width
        || paper.margins[1] + paper.margins[3] >= paper.height
    {
        return Err(option_invalid("margins"));
    }
    let printable = (
        Point2::new(paper.margins[0], paper.margins[1]),
        Point2::new(
            paper.width - paper.margins[2],
            paper.height - paper.margins[3],
        ),
    );
    let (pw, ph) = (printable.1.x - printable.0.x, printable.1.y - printable.0.y);

    // The plotted region in source coordinates.
    let extents = || {
        groups
            .iter()
            .flat_map(|g| g.primitives.iter())
            .filter_map(extent)
            .reduce(|a, b| a.union(&b))
    };
    let region = match options.area {
        PlotArea::Window { min, max } => {
            let finite = [min.x, min.y, max.x, max.y].iter().all(|v| v.is_finite());
            if !(finite && min.x < max.x && min.y < max.y) {
                return Err(option_invalid("area"));
            }
            (min, max)
        }
        PlotArea::Extents | PlotArea::Layout => {
            let b = extents().ok_or_else(|| option_invalid("area"))?;
            (b.min(), b.max())
        }
    };
    let (rw, rh) = (region.1.x - region.0.x, region.1.y - region.0.y);
    if !(positive(rw) && positive(rh)) {
        return Err(option_invalid("area"));
    }
    let k = match options.scale {
        PlotScale::Ratio(r) if positive(r) => r * source_factor,
        PlotScale::Ratio(_) => return Err(option_invalid("scale")),
        PlotScale::Fit => {
            let (tw, th) = if options.rotation % 2 == 1 {
                (rh, rw)
            } else {
                (rw, rh)
            };
            (pw / tw).min(ph / th)
        }
    };
    if !positive(k) {
        return Err(option_invalid("scale"));
    }
    let (sw, sh) = (rw * k, rh * k);
    let (tw, th) = if options.rotation % 2 == 1 {
        (sh, sw)
    } else {
        (sw, sh)
    };
    let origin = match options.offset {
        Offset::Center => Point2::new(
            printable.0.x + (pw - tw) / 2.0,
            printable.0.y + (ph - th) / 2.0,
        ),
        Offset::Origin(o) if o.x.is_finite() && o.y.is_finite() => {
            Point2::new(printable.0.x + o.x, printable.0.y + o.y)
        }
        Offset::Origin(_) => return Err(option_invalid("offset")),
    };
    let map = page_map(region, k, options.rotation, origin);
    let lo = Point2::new(origin.x.max(printable.0.x), origin.y.max(printable.0.y));
    let hi = Point2::new(
        (origin.x + tw).min(printable.1.x),
        (origin.y + th).min(printable.1.y),
    );
    let area = [lo, Point2::new(hi.x.max(lo.x), hi.y.max(lo.y))];

    // Unbounded curves outside viewports are cut to the plotted region.
    let bounds = rectangle(region.0, region.1)
        .and_then(|r| Loop2::new(vec![Curve2::Polyline(r)], tol))
        .map_err(|_| option_invalid("area"))?;
    for group in groups.iter_mut().filter(|g| g.clip.is_none()) {
        group.primitives.retain_mut(|p| {
            let Shape::Stroke { curve, drawn } = &mut p.shape else {
                return true;
            };
            if drawn.is_some() || curve.bounding_box().is_some() {
                return true;
            }
            match cut(curve, &bounds, true, tol) {
                Ok(pieces) if !pieces.is_empty() => {
                    *drawn = Some(pieces);
                    true
                }
                Ok(_) => false,
                Err(_) => {
                    failed.push(p.source);
                    false
                }
            }
        });
    }

    let table = if options.styles {
        options.style_table.as_deref().and_then(|name| {
            let mut tables: Vec<_> = snapshot
                .iter_type(PLOT_STYLE_TABLE)
                .ok()?
                .map(|(_, o)| o)
                .collect();
            tables.sort_by_key(|o| o.id);
            tables
                .into_iter()
                .find(|o| text_at(&o.values, table::NAME) == Some(name))
                .map(|o| PlotStyleTable::from_values(&o.values))
        })
    } else {
        None
    };
    let mut cx = Context {
        snapshot,
        display,
        table,
        options,
        names: BTreeMap::new(),
        patterns: BTreeMap::new(),
    };
    let overall = scale_of(&map);
    let mut stream = PlotStream {
        width: paper.width,
        height: paper.height,
        paper_color: options.paper_color,
        area,
        clips: Vec::new(),
        items: Vec::new(),
        failed: Vec::new(),
    };
    for group in groups {
        let clip = match &group.clip {
            Some(polyline) => match transform_polyline(polyline, &map, tol) {
                Ok(mapped) => {
                    stream.clips.push(mapped);
                    Some(stream.clips.len() - 1)
                }
                Err(_) => {
                    failed.extend(group.primitives.iter().map(|p| p.source));
                    continue;
                }
            },
            None => None,
        };
        for p in group.primitives {
            let pen = cx.pen(&p);
            let shape = match transform_shape(&p.shape, &map, tol) {
                Ok(shape) => shape,
                Err(_) => {
                    failed.push(p.source);
                    continue;
                }
            };
            let kind = match shape {
                Shape::Point(at) => ItemKind::Point { at, pen },
                Shape::Stroke { curve, drawn } => {
                    let pieces = if pen.linetype_overridden {
                        match redash(&mut cx, &curve, &pen, &p, group.pattern_scale * overall) {
                            Ok(pieces) => pieces,
                            Err(_) => {
                                failed.push(p.source);
                                continue;
                            }
                        }
                    } else {
                        drawn
                    };
                    ItemKind::Stroke { curve, pieces, pen }
                }
                Shape::Fill(region) => ItemKind::Fill { region, pen },
                Shape::Region(region) => ItemKind::RegionFill { region, pen },
                Shape::Mask(region) => ItemKind::Mask { region },
                Shape::Glyphs {
                    family,
                    index,
                    size,
                    glyphs,
                    placement,
                } => ItemKind::Glyphs {
                    family,
                    index,
                    size,
                    glyphs,
                    placement,
                    pen,
                },
            };
            if stream.items.len() >= options.max_items {
                return Err(Error::new(codes::PLOT_TOO_LARGE).with_arg(
                    "limit",
                    i64::try_from(options.max_items).unwrap_or(i64::MAX),
                ));
            }
            stream.items.push(Item {
                source: p.source,
                part: p.part,
                layer: p.layers.first().copied(),
                clip,
                kind,
            });
        }
    }
    failed.sort();
    failed.dedup();
    stream.failed = failed;
    Ok(stream)
}

/// The stream of the layout space `layout`: its viewports in ascending identifier order, then
/// its own entities in draw order.
///
/// The sheet is the paper of the layout's settings turned for its orientation, in the layout's
/// unit converted to millimeters; the margins are the settings'. Coordinates of the source are
/// layout coordinates; a [`PlotScale::Ratio`] is paper length per layout length.
///
/// # Errors
///
/// `draft.plot.source_invalid` when `layout` is not a layout space with settings;
/// `draft.plot.option_invalid` for an invalid option or an empty plot area;
/// `draft.plot.too_large`; the errors of [`space_list`] other than per-entity failures.
pub fn layout_stream(
    snapshot: &Snapshot,
    lists: Lists<'_>,
    layout: ObjectId,
    options: &PlotOptions,
    display: &DisplayOptions,
) -> Result<PlotStream> {
    let sheet = Sheet::read(snapshot, layout)?;
    let f = sheet.factor;
    let (w, h) = sheet_size(&sheet.values);
    let margin = |i| f64_at(&sheet.values, i).unwrap_or(0.0) * f;
    let paper = Paper {
        width: w * f,
        height: h * f,
        margins: [
            margin(settings::MARGIN_LEFT),
            margin(settings::MARGIN_BOTTOM),
            margin(settings::MARGIN_RIGHT),
            margin(settings::MARGIN_TOP),
        ],
    };
    let mut groups = Vec::new();
    let mut failed = Vec::new();
    collect_viewports(snapshot, lists, layout, display, &mut groups, &mut failed);
    let own = space_list(snapshot, lists, layout, display)?;
    failed.extend(own.failed);
    groups.push(Group {
        clip: None,
        pattern_scale: 1.0,
        primitives: own.primitives,
    });
    // `Layout` is the printable area of the sheet in layout coordinates.
    let mut options = options.clone();
    if options.area == PlotArea::Layout {
        let (l, b) = (
            margin(settings::MARGIN_LEFT) / f,
            margin(settings::MARGIN_BOTTOM) / f,
        );
        let (r, t) = (
            margin(settings::MARGIN_RIGHT) / f,
            margin(settings::MARGIN_TOP) / f,
        );
        options.area = PlotArea::Window {
            min: Point2::new(l, b),
            max: Point2::new(w - r, h - t),
        };
    }
    build(snapshot, groups, failed, f, &paper, &options, display)
}

/// The stream of the model space `space` on `paper`: the part chosen by `options.area` at the
/// scale of `options.scale`. A [`PlotScale::Ratio`] is paper length per model length, both in
/// one unit (an unitless space counts in millimeters).
///
/// # Errors
///
/// As [`layout_stream`], with `draft.plot.source_invalid` for an object that is not a space.
pub fn model_stream(
    snapshot: &Snapshot,
    lists: Lists<'_>,
    space: ObjectId,
    paper: &Paper,
    options: &PlotOptions,
    display: &DisplayOptions,
) -> Result<PlotStream> {
    if !type_is(snapshot, space, SPACE) {
        return Err(Error::new(codes::PLOT_SOURCE_INVALID).with_detail("id", space.to_string()));
    }
    let list = space_list(snapshot, lists, space, display)?;
    let groups = vec![Group {
        clip: None,
        pattern_scale: 1.0,
        primitives: list.primitives,
    }];
    let factor = unit_of(snapshot, space).factor_to(SpaceUnit::Millimeter);
    build(
        snapshot,
        groups,
        list.failed,
        factor,
        paper,
        options,
        display,
    )
}

// ----- canonical bytes -----

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u64(u64::try_from(n).unwrap_or(u64::MAX));
    }
    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_bits().to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.len(v.len());
        self.0.extend_from_slice(v);
    }
    fn point(&mut self, p: Point2) {
        self.f64(p.x);
        self.f64(p.y);
    }
    fn color(&mut self, c: Color) {
        for v in [c.r, c.g, c.b, c.a] {
            self.f32(v);
        }
    }
    fn id(&mut self, id: ObjectId) {
        self.0.extend_from_slice(&id.as_u128().to_le_bytes());
    }
    fn opt_id(&mut self, id: Option<ObjectId>) {
        match id {
            Some(id) => {
                self.u8(1);
                self.id(id);
            }
            None => self.u8(0),
        }
    }
    fn pen(&mut self, pen: &Pen) {
        self.color(pen.color);
        self.f64(pen.width);
        self.u8(pen.cap as u8);
        self.u8(pen.join as u8);
        self.opt_id(pen.linetype);
        self.u8(u8::from(pen.linetype_overridden));
    }
    fn matrix(&mut self, t: &Transform2) {
        let m = t.matrix();
        for r in 0..3 {
            for c in 0..3 {
                self.f64(m[(r, c)]);
            }
        }
    }
    fn piece(&mut self, piece: &Piece) {
        match piece {
            Piece::Point(p) => {
                self.u8(0);
                self.point(*p);
            }
            Piece::Curve(c) => {
                self.u8(1);
                self.bytes(&c.encode());
            }
        }
    }
}

impl PlotStream {
    /// The canonical bytes of the stream: a header, then every number as its bit pattern and
    /// every curve in the `arc.geom` encoding. Equal streams give equal bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Out(b"ARCP".to_vec());
        o.u8(1);
        o.f64(self.width);
        o.f64(self.height);
        o.color(self.paper_color);
        o.point(self.area[0]);
        o.point(self.area[1]);
        o.len(self.clips.len());
        for clip in &self.clips {
            o.bytes(&clip.encode());
        }
        o.len(self.failed.len());
        for id in &self.failed {
            o.id(*id);
        }
        o.len(self.items.len());
        for item in &self.items {
            o.id(item.source);
            o.u64(u64::from(item.part));
            o.opt_id(item.layer);
            match item.clip {
                Some(c) => {
                    o.u8(1);
                    o.len(c);
                }
                None => o.u8(0),
            }
            match &item.kind {
                ItemKind::Point { at, pen } => {
                    o.u8(0);
                    o.point(*at);
                    o.pen(pen);
                }
                ItemKind::Stroke { curve, pieces, pen } => {
                    o.u8(1);
                    o.bytes(&curve.encode());
                    match pieces {
                        Some(list) => {
                            o.u8(1);
                            o.len(list.len());
                            for p in list {
                                o.piece(p);
                            }
                        }
                        None => o.u8(0),
                    }
                    o.pen(pen);
                }
                ItemKind::Fill { region, pen } => {
                    o.u8(2);
                    o.bytes(&region.encode());
                    o.pen(pen);
                }
                ItemKind::RegionFill { region, pen } => {
                    o.u8(5);
                    o.bytes(&region.encode());
                    o.pen(pen);
                }
                ItemKind::Mask { region } => {
                    o.u8(3);
                    o.bytes(&region.encode());
                }
                ItemKind::Glyphs {
                    family,
                    index,
                    size,
                    glyphs,
                    placement,
                    pen,
                } => {
                    o.u8(4);
                    o.bytes(family.as_bytes());
                    o.u64(u64::from(*index));
                    o.f64(*size);
                    o.len(glyphs.len());
                    for g in glyphs {
                        o.u64(u64::from(g.id));
                        o.point(g.position);
                    }
                    o.matrix(placement);
                    o.pen(pen);
                }
            }
        }
        o.0
    }
}
