//! The view source a product implements, the styles it supplies and graphic overrides
//! (CORE-VIEW-005, CORE-VIEW-006, CORE-VIEW-012, CORE-VIEW-015, CORE-VIEW-016, CORE-VIEW-018).
//!
//! The structs and enums a product builds are `#[non_exhaustive]`: build them with their
//! constructors and `with_*` methods, or start from `Default` and set public fields, so fields
//! can be added later without breaking products.

use std::collections::BTreeMap;

use arc_base::{Color, Error, ObjectId, Result};
use arc_doc::ColorValue;
use arc_draft::hatch_pattern::PatternLine;
use arc_geom::{Curve2, Polyhedron};
use arc_mesh::TriMesh;

use crate::def::ViewDef;
use crate::error::codes;
use crate::result::VisibilityClass;

/// The geometry of one part, in world coordinates.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Geometry {
    /// A closed planar solid.
    Solid(Polyhedron),
    /// A triangle mesh.
    Mesh(TriMesh),
}

/// The appearance of the lines of one visibility class.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LineStyle {
    /// The color.
    pub color: ColorValue,
    /// The linetype object; `None` is continuous. Dashes are not generated: the primitive
    /// carries the linetype for the consumer.
    pub linetype: Option<ObjectId>,
    /// The lineweight in millimeters, finite and not negative; `None` is the default.
    pub lineweight: Option<f64>,
    /// The layer the primitives are placed on, if any.
    pub layer: Option<ObjectId>,
}

impl LineStyle {
    /// A continuous line of `color` with the default lineweight on no layer.
    #[must_use]
    pub fn solid(color: Color) -> Self {
        Self {
            color: ColorValue {
                rgba: color,
                source: None,
            },
            linetype: None,
            lineweight: None,
            layer: None,
        }
    }

    /// The style with `lineweight` in millimeters.
    #[must_use]
    pub fn with_lineweight(mut self, lineweight: Option<f64>) -> Self {
        self.lineweight = lineweight;
        self
    }

    /// The style with `linetype`.
    #[must_use]
    pub fn with_linetype(mut self, linetype: Option<ObjectId>) -> Self {
        self.linetype = linetype;
        self
    }

    /// The style on `layer`.
    #[must_use]
    pub fn with_layer(mut self, layer: Option<ObjectId>) -> Self {
        self.layer = layer;
        self
    }
}

/// The line style of every visibility class; a class without a style is not drawn.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct ClassStyles {
    /// Edges below the cut of a part that is cut.
    pub visible: Option<LineStyle>,
    /// Hidden edge segments; usually `None` in plans.
    pub hidden: Option<LineStyle>,
    /// Boundaries of cut regions and open section edges.
    pub cut: Option<LineStyle>,
    /// Visible edges of parts entirely below the cut.
    pub beyond: Option<LineStyle>,
    /// Edges of parts in the overhead range.
    pub overhead: Option<LineStyle>,
    /// Silhouette edges (not produced from geometry yet; symbols may use the class).
    pub silhouette: Option<LineStyle>,
}

impl ClassStyles {
    /// The same style for every class but hidden.
    #[must_use]
    pub fn uniform(style: &LineStyle) -> Self {
        Self {
            visible: Some(style.clone()),
            hidden: None,
            cut: Some(style.clone()),
            beyond: Some(style.clone()),
            overhead: Some(style.clone()),
            silhouette: Some(style.clone()),
        }
    }

    /// The styles with `style` for `class`.
    #[must_use]
    pub fn with(mut self, class: VisibilityClass, style: Option<LineStyle>) -> Self {
        *self.slot(class) = style;
        self
    }

    /// The style of `class`.
    #[must_use]
    pub fn get(&self, class: VisibilityClass) -> Option<&LineStyle> {
        match class {
            VisibilityClass::Visible => self.visible.as_ref(),
            VisibilityClass::Hidden => self.hidden.as_ref(),
            VisibilityClass::Cut => self.cut.as_ref(),
            VisibilityClass::Beyond => self.beyond.as_ref(),
            VisibilityClass::Overhead => self.overhead.as_ref(),
            VisibilityClass::Silhouette => self.silhouette.as_ref(),
        }
    }

    fn slot(&mut self, class: VisibilityClass) -> &mut Option<LineStyle> {
        match class {
            VisibilityClass::Visible => &mut self.visible,
            VisibilityClass::Hidden => &mut self.hidden,
            VisibilityClass::Cut => &mut self.cut,
            VisibilityClass::Beyond => &mut self.beyond,
            VisibilityClass::Overhead => &mut self.overhead,
            VisibilityClass::Silhouette => &mut self.silhouette,
        }
    }

    fn each_mut(&mut self) -> [&mut Option<LineStyle>; 6] {
        [
            &mut self.visible,
            &mut self.hidden,
            &mut self.cut,
            &mut self.beyond,
            &mut self.overhead,
            &mut self.silhouette,
        ]
    }
}

/// The fill of a cut region.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct CutFill {
    /// The pattern line families; empty is a solid fill.
    pub lines: Vec<PatternLine>,
    /// Pattern units to view units; finite and positive.
    pub scale: f64,
    /// The rotation of the pattern in radians; finite.
    pub angle: f64,
    /// The color of the fill or the pattern lines.
    pub color: ColorValue,
    /// The layer of the fill primitives, if any.
    pub layer: Option<ObjectId>,
}

impl CutFill {
    /// A solid fill of `color`.
    #[must_use]
    pub fn solid(color: Color) -> Self {
        Self::pattern(Vec::new(), 1.0, 0.0, color)
    }

    /// A pattern fill of `lines` at `scale` and `angle` in `color`.
    #[must_use]
    pub fn pattern(lines: Vec<PatternLine>, scale: f64, angle: f64, color: Color) -> Self {
        Self {
            lines,
            scale,
            angle,
            color: ColorValue {
                rgba: color,
                source: None,
            },
            layer: None,
        }
    }

    /// The fill on `layer`.
    #[must_use]
    pub fn with_layer(mut self, layer: Option<ObjectId>) -> Self {
        self.layer = layer;
        self
    }
}

/// One part of an object: a layer of a compound wall, a slab or a whole simple object.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Part {
    /// The product's stable sub-object key of the part, reported in back-links.
    pub key: u32,
    /// The geometry at the view's detail level.
    pub geometry: Geometry,
    /// The fill of the part's cut region; `None` leaves it unfilled.
    pub cut_fill: Option<CutFill>,
    /// Parts with the same join key, of any objects, have their cut regions merged without
    /// seams (for example one key per material); `None` never joins. A joined region takes the
    /// fill of its first part in scope order.
    pub join: Option<u64>,
}

impl Part {
    /// A part with sub-object key `key`, no fill and no join key.
    #[must_use]
    pub fn new(key: u32, geometry: Geometry) -> Self {
        Self {
            key,
            geometry,
            cut_fill: None,
            join: None,
        }
    }

    /// The part with `fill`.
    #[must_use]
    pub fn with_fill(mut self, fill: Option<CutFill>) -> Self {
        self.cut_fill = fill;
        self
    }

    /// The part with join key `join`.
    #[must_use]
    pub fn with_join(mut self, join: Option<u64>) -> Self {
        self.join = join;
        self
    }
}

/// A 2D curve a product draws in a view for an object (a door swing, a stair arrow, the frame of
/// a tag), in view coordinates. It goes through the same overrides, crop, draw order and
/// back-links as derived lines, but not through hidden-line removal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Symbol {
    /// The curve; bounded.
    pub curve: Curve2,
    /// The class whose draw order it takes, and whose style it uses without its own.
    pub class: VisibilityClass,
    /// The product's stable sub-object key, reported in back-links.
    pub key: u32,
    /// Its own style; `None` uses the object's style of `class`.
    pub style: Option<LineStyle>,
}

impl Symbol {
    /// A symbol curve of `class` with sub-object key `key` and the class style.
    #[must_use]
    pub fn new(curve: Curve2, class: VisibilityClass, key: u32) -> Self {
        Self {
            curve,
            class,
            key,
            style: None,
        }
    }

    /// The symbol with its own `style`.
    #[must_use]
    pub fn with_style(mut self, style: Option<LineStyle>) -> Self {
        self.style = style;
        self
    }
}

/// What the source supplies for one object.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SourceObject {
    /// The parts.
    pub parts: Vec<Part>,
    /// The product's 2D symbol curves.
    pub symbols: Vec<Symbol>,
    /// The category, matched by category overrides.
    pub category: String,
    /// The line style per visibility class.
    pub styles: ClassStyles,
}

impl SourceObject {
    /// An object of `category` with `styles` and no parts or symbols.
    #[must_use]
    pub fn new(category: impl Into<String>, styles: ClassStyles) -> Self {
        Self {
            parts: Vec::new(),
            symbols: Vec::new(),
            category: category.into(),
            styles,
        }
    }

    /// An object of one part with sub-object key 0.
    #[must_use]
    pub fn single(
        geometry: Geometry,
        category: impl Into<String>,
        styles: ClassStyles,
        cut_fill: Option<CutFill>,
        join: Option<u64>,
    ) -> Self {
        Self::new(category, styles)
            .with_part(Part::new(0, geometry).with_fill(cut_fill).with_join(join))
    }

    /// The object with `part` added.
    #[must_use]
    pub fn with_part(mut self, part: Part) -> Self {
        self.parts.push(part);
        self
    }

    /// The object with `symbol` added.
    #[must_use]
    pub fn with_symbol(mut self, symbol: Symbol) -> Self {
        self.symbols.push(symbol);
        self
    }
}

/// The interface through which a product supplies a view's content. `arc-view` applies what it
/// supplies and decides no styling.
pub trait ViewSource {
    /// The objects in the view's scope, in drawing order; the product's scope filter. A repeated
    /// identifier is used once.
    ///
    /// # Errors
    ///
    /// Any error of the product; it fails the build.
    fn objects(&self, view: &ViewDef) -> Result<Vec<ObjectId>>;

    /// The content of `id` at `view.detail`, tessellated within [`ViewDef::chord`] for curved
    /// geometry; `None` draws nothing for it.
    ///
    /// # Errors
    ///
    /// Any error of the product; the object is left out and the error becomes a diagnostic.
    fn object(&self, id: ObjectId, view: &ViewDef) -> Result<Option<SourceObject>>;
}

/// A graphic override; each `None` field keeps the source's value.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct GraphicOverride {
    /// Whether the object is drawn at all.
    pub visible: Option<bool>,
    /// The color of every line class, symbol and cut fill.
    pub color: Option<ColorValue>,
    /// The linetype of every line class and symbol.
    pub linetype: Option<Option<ObjectId>>,
    /// The lineweight of every line class and symbol.
    pub lineweight: Option<Option<f64>>,
    /// The cut fill of every part.
    pub fill: Option<Option<CutFill>>,
    /// Whether colors are drawn halfway toward white.
    pub halftone: Option<bool>,
}

impl GraphicOverride {
    /// The fields of `self`, falling back to those of `other` where `self` has none.
    #[must_use]
    pub fn or(&self, other: &Self) -> Self {
        Self {
            visible: self.visible.or(other.visible),
            color: self.color.clone().or_else(|| other.color.clone()),
            linetype: self.linetype.or(other.linetype),
            lineweight: self.lineweight.or(other.lineweight),
            fill: self.fill.clone().or_else(|| other.fill.clone()),
            halftone: self.halftone.or(other.halftone),
        }
    }
}

/// The overrides of one view: by object, then by category. An object override field wins over
/// the same field of its category's override. Rule-based overrides are not implemented.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct Overrides {
    /// By object.
    pub by_object: BTreeMap<ObjectId, GraphicOverride>,
    /// By category name.
    pub by_category: BTreeMap<String, GraphicOverride>,
}

impl Overrides {
    /// The effective override of an object.
    #[must_use]
    pub fn of(&self, id: ObjectId, category: &str) -> GraphicOverride {
        let none = GraphicOverride::default();
        let by_object = self.by_object.get(&id).unwrap_or(&none);
        by_object.or(self.by_category.get(category).unwrap_or(&none))
    }
}

/// `color` halfway toward white.
fn halftone(color: &ColorValue) -> ColorValue {
    let c = color.rgba;
    ColorValue {
        rgba: Color::new(0.5 + 0.5 * c.r, 0.5 + 0.5 * c.g, 0.5 + 0.5 * c.b, c.a),
        source: None,
    }
}

fn styles_mut(object: &mut SourceObject) -> impl Iterator<Item = &mut LineStyle> {
    object
        .styles
        .each_mut()
        .into_iter()
        .flatten()
        .chain(object.symbols.iter_mut().filter_map(|s| s.style.as_mut()))
}

/// Applies `over` to the styles and fills of `object`; returns `false` when it hides the object.
pub(crate) fn apply(over: &GraphicOverride, object: &mut SourceObject) -> bool {
    if over.visible == Some(false) {
        return false;
    }
    let half = over.halftone == Some(true);
    for part in &mut object.parts {
        if let Some(fill) = &over.fill {
            part.cut_fill.clone_from(fill);
        }
        if let Some(fill) = &mut part.cut_fill {
            if let Some(color) = &over.color {
                fill.color = color.clone();
            }
            if half {
                fill.color = halftone(&fill.color);
            }
        }
    }
    for style in styles_mut(object) {
        if let Some(color) = &over.color {
            style.color = color.clone();
        }
        if let Some(linetype) = over.linetype {
            style.linetype = linetype;
        }
        if let Some(lineweight) = over.lineweight {
            style.lineweight = lineweight;
        }
        if half {
            style.color = halftone(&style.color);
        }
    }
    true
}

fn finite_color(color: &ColorValue) -> bool {
    let c = color.rgba;
    [c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite())
}

fn invalid(what: &'static str) -> Error {
    Error::new(codes::STYLE_INVALID).with_detail("what", what)
}

/// Replaces invalid values of `object` with defaults and returns one `view.style.invalid` error
/// per replacement: a non-finite color becomes opaque black, a lineweight that is not finite
/// and not negative the default, a fill with an invalid scale, angle or pattern family a solid
/// fill, and a symbol with an unbounded curve is removed.
pub(crate) fn sanitize(object: &mut SourceObject) -> Vec<Error> {
    let mut errors = Vec::new();
    let black = ColorValue {
        rgba: Color::BLACK,
        source: None,
    };
    for style in styles_mut(object) {
        if !finite_color(&style.color) {
            style.color = black.clone();
            errors.push(invalid("color"));
        }
        if style
            .lineweight
            .is_some_and(|w| w.is_nan() || w.is_infinite() || w < 0.0)
        {
            style.lineweight = None;
            errors.push(invalid("lineweight"));
        }
    }
    for fill in object.parts.iter_mut().filter_map(|p| p.cut_fill.as_mut()) {
        if !finite_color(&fill.color) {
            fill.color = black.clone();
            errors.push(invalid("fill_color"));
        }
        let placed = fill.scale.is_finite() && fill.scale > 0.0 && fill.angle.is_finite();
        if !placed || fill.lines.iter().any(|l| l.validate().is_err()) {
            fill.lines.clear();
            fill.scale = 1.0;
            fill.angle = 0.0;
            errors.push(invalid("fill"));
        }
    }
    let before = object.symbols.len();
    object
        .symbols
        .retain(|s| s.curve.bounding_box().is_some_and(|b| !b.is_empty()));
    if object.symbols.len() < before {
        errors.push(invalid("symbol"));
    }
    errors
}
