//! View results, back-links and selection (CORE-VIEW-012, CORE-VIEW-019, CORE-VIEW-020).

use arc_base::{Error, ObjectId};
use arc_draft::display::DisplayList;

/// The visibility class of a derived primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum VisibilityClass {
    /// A visible edge below the cut of an object that is cut.
    Visible,
    /// An edge segment hidden by a cut region or a nearer surface.
    Hidden,
    /// A cut region, its boundary or its fill, or an edge of an open section.
    Cut,
    /// A visible edge of an object entirely below the cut, within the view depth.
    Beyond,
    /// An edge of an object above the cut, within the overhead range.
    Overhead,
    /// A silhouette edge (not produced yet).
    Silhouette,
}

/// What a primitive draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PrimitiveKind {
    /// A line segment.
    Line,
    /// A solid fill of a cut region.
    Fill,
    /// A mark of a cut region's fill pattern.
    Pattern,
    /// A product-supplied symbol curve.
    Symbol,
}

/// The source of one primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackLink {
    /// The source object.
    pub source: ObjectId,
    /// The product's stable sub-object key: the key of the part the primitive comes from, or of
    /// the symbol it draws. Use this key to find the primitive again after regeneration.
    pub sub_key: u32,
    /// The geometric sub-key: the face index of a solid or the face-group key of a mesh, of the
    /// lowest triangle of an edge or of the triangle a section segment crosses (for a merged
    /// line, of its first piece); `None` for fills, symbols and triangles outside a group.
    /// Face indices change when the geometry is regenerated and are not stable keys.
    pub face: Option<u32>,
    /// The visibility class.
    pub class: VisibilityClass,
    /// What the primitive draws.
    pub kind: PrimitiveKind,
    /// The other parts, as (object, part key), of a joined cut region, for its fill and pattern.
    pub joined: Vec<(ObjectId, u32)>,
}

/// A problem with one object that did not stop the view.
#[derive(Debug)]
pub struct Diagnostic {
    /// The object.
    pub object: ObjectId,
    /// What happened: `view.section.open`, `view.geometry.out_of_range`, a geometry error of its
    /// section or an error of the source.
    pub error: Error,
}

/// A built view: `arc-draft` display content in view coordinates, one back-link per primitive
/// (same index) and the diagnostics.
#[derive(Debug, Default)]
pub struct ViewResult {
    /// The primitives in draw order.
    pub list: DisplayList,
    /// The back-link of each primitive.
    pub links: Vec<BackLink>,
    /// The diagnostics, by object in scope order.
    pub diagnostics: Vec<Diagnostic>,
}

impl ViewResult {
    /// The back-link of primitive `index`.
    #[must_use]
    pub fn link(&self, index: usize) -> Option<&BackLink> {
        self.links.get(index)
    }

    /// The indices of every primitive of `source`, joined fills included.
    #[must_use]
    pub fn primitives_of(&self, source: ObjectId) -> Vec<usize> {
        self.links
            .iter()
            .enumerate()
            .filter(|(_, l)| l.source == source || l.joined.iter().any(|j| j.0 == source))
            .map(|(i, _)| i)
            .collect()
    }

    /// Resolves picked primitive `index` to its source object and all of that object's
    /// primitives, for highlighting.
    #[must_use]
    pub fn select(&self, index: usize) -> Option<(ObjectId, Vec<usize>)> {
        let source = self.link(index)?.source;
        Some((source, self.primitives_of(source)))
    }

    /// The number of line primitives of `class`.
    #[must_use]
    pub fn count(&self, class: VisibilityClass) -> usize {
        self.links
            .iter()
            .filter(|l| l.class == class && l.kind == PrimitiveKind::Line)
            .count()
    }
}
