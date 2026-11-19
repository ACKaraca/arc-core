//! External drawing references (CORE-DRAFT-025): a placed reference shows the content of a
//! linked drawing, read-only, with an insertion transform and an optional clip boundary.
//!
//! A placed reference is an entity of type [`EXTERNAL_REF`]. After the common entity fields it
//! holds `link` (a `core.doc.link` object of the host document), `position`, `scale` and
//! `rotation` (as a block reference, with the origin of the linked drawing as base point),
//! `space` (the identifier of a space of the linked drawing; absent means its model space) and
//! `clip` (an optional closed polyline in the linked drawing's own coordinates, applied before the
//! insertion transform).
//!
//! The display list of a placed reference is built from the loaded [`Links`] (see
//! `display::space_list_with_links`): the primitives of the linked space, wipeouts applied, cut to
//! the clip boundary and transformed. Each primitive has `source` set to the placed reference and
//! `path` set to the link object followed by the linked entity and its nested path, so picking
//! can tell linked geometry from local geometry. An unresolved, missing or self-referencing link
//! draws nothing and is reported in `DisplayList::problems`; nothing is deleted or changed.

use arc_base::id::Identifier;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::math::{Point2, Vector3};
use arc_base::{Error, Limits, ObjectId, Result};
use arc_doc::link::{LINK_TYPE, LinkState, Links};
use arc_doc::{FieldDecl, Finding, Snapshot, Value, ValueType};
use arc_geom::Polyline2;
use arc_geom::encoding::GeomCodec;

use crate::appearance::{Source, Sourced};
use crate::display::{
    DisplayList, DisplayOptions, EntityList, Lists, Primitive, Reader, assemble, boundary_loop,
    cut_primitive, placement, transform_shape,
};
use crate::error::codes;
use crate::types::{self, SpaceUnit, entity, table};

/// Index of `link` in the entity values (the `kind` slot of the entity).
pub(crate) const LINK: usize = entity::KIND;
/// Index of `space` in the entity values.
pub(crate) const SPACE: usize = 20;
/// Index of `clip` in the entity values.
pub(crate) const CLIP: usize = 21;

/// The names of the fields after the common ones, in the order of [`fields`].
pub(crate) const FIELD_NAMES: [&str; 6] =
    ["link", "position", "scale", "rotation", "space", "clip"];

/// A problem with a placed reference: the reference, the link it names, and the stable code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceProblem {
    /// The placed reference, an entity of the host space or block definition.
    pub reference: ObjectId,
    /// The link object, when the reference names one.
    pub link: Option<ObjectId>,
    /// The code of the problem, as the error reports it.
    pub code: &'static str,
}

/// The field declarations of [`EXTERNAL_REF`] after the common fields.
pub(crate) fn fields() -> Vec<FieldDecl> {
    let mut clip = types::geometry();
    clip.name = "clip".to_owned();
    vec![
        FieldDecl::new(
            "link",
            types::reference(
                LINK_TYPE,
                arc_doc::Strength::Associative,
                arc_doc::OnDelete::Block,
                false,
            ),
        ),
        FieldDecl::new("position", ValueType::Point2).with_default(Value::Point2(Point2::origin())),
        FieldDecl::new("scale", ValueType::Vector3)
            .with_default(Value::Vector3(Vector3::new(1.0, 1.0, 1.0))),
        types::real("rotation", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("space", ValueType::Opaque).optional(),
        clip.optional(),
    ]
}

/// The most vertices a clip boundary may have. Creation, `set_clip` and display refuse a larger
/// boundary, which bounds the cost of each cut (see [`WorkBudget`]).
pub(crate) const CLIP_MAX_VERTICES: usize = 10_000;

/// Decodes a clip boundary. The decoder refuses more than [`CLIP_MAX_VERTICES`] vertices before
/// it allocates them.
pub(crate) fn clip_polyline(bytes: &[u8]) -> Result<Polyline2> {
    clip_polyline_within(bytes, CLIP_MAX_VERTICES)
}

/// As [`clip_polyline`], with at most `max_vertices` vertices.
fn clip_polyline_within(bytes: &[u8], max_vertices: usize) -> Result<Polyline2> {
    let limits = Limits {
        array_length: u64::try_from(max_vertices).unwrap_or(u64::MAX),
        ..Limits::DEFAULT
    };
    Polyline2::decode(bytes, &limits).map_err(|error| {
        Error::new(codes::EXTERNAL_CLIP_INVALID)
            .with_detail("reason", "vertex count or encoding")
            .with_source(error)
    })
}

/// The validator of [`EXTERNAL_REF`]: the common checks, a nonzero scale, finite placement values
/// and a clip that is a closed polyline within [`CLIP_MAX_VERTICES`].
pub(crate) fn findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut found = types::common_findings(values);
    if let Some(Some(Value::Vector3(s))) = values.get(entity::REF_SCALE)
        && (s.x == 0.0 || s.y == 0.0 || s.z == 0.0)
    {
        found.push(types::finding(Error::new(codes::BLOCK_SCALE_ZERO)));
    }
    let (position, scale, rotation) = placement_numbers(values);
    if let Some(field) = non_finite_placement(position, scale, rotation) {
        found.push(types::finding(
            Error::new(codes::FIELD_INVALID).with_arg("field", field),
        ));
    }
    if let Some(Some(Value::Geometry(bytes))) = values.get(CLIP) {
        let closed = clip_polyline(&bytes.0).is_ok_and(|p| p.is_closed());
        if !closed {
            found.push(types::finding(Error::new(codes::EXTERNAL_CLIP_INVALID)));
        }
    }
    found
}

/// The position, scale and rotation of a placed reference from its fields, with the defaults of
/// [`fields`] for absent values.
fn placement_numbers(values: &[Option<Value>]) -> ([f64; 2], [f64; 3], f64) {
    let position = match values.get(entity::REF_POSITION) {
        Some(Some(Value::Point2(p))) => [p.x, p.y],
        _ => [0.0, 0.0],
    };
    let scale = match values.get(entity::REF_SCALE) {
        Some(Some(Value::Vector3(v))) => [v.x, v.y, v.z],
        _ => [1.0; 3],
    };
    let rotation = match values.get(entity::REF_ROTATION) {
        Some(Some(Value::F64(r))) => *r,
        _ => 0.0,
    };
    (position, scale, rotation)
}

/// The name of the first placement value that is not finite: `position`, `scale` or `rotation`.
pub(crate) fn non_finite_placement(
    position: [f64; 2],
    scale: [f64; 3],
    rotation: f64,
) -> Option<&'static str> {
    if position.iter().any(|v| !v.is_finite()) {
        Some("position")
    } else if scale.iter().any(|v| !v.is_finite()) {
        Some("scale")
    } else if !rotation.is_finite() {
        Some("rotation")
    } else {
        None
    }
}

/// The view of the document that `link` links, for reading its objects. Fails with the code of
/// the link's state: `doc.link.invalid` (no such link), `doc.link.not_loaded`, the code of an
/// unresolved or refused link.
pub(crate) fn linked_view(links: &Links, link: ObjectId) -> Result<Links> {
    let invalid =
        || Error::new(arc_doc::error::codes::LINK_INVALID).with_arg("link", link.to_string());
    match links.state(link) {
        None => Err(invalid()),
        Some(LinkState::NotLoaded) => {
            Err(Error::new(arc_doc::error::codes::LINK_NOT_LOADED)
                .with_arg("link", link.to_string()))
        }
        Some(LinkState::Unresolved(problem) | LinkState::Refused(problem)) => {
            Err(Error::new(problem.code).with_arg("link", link.to_string()))
        }
        Some(LinkState::Resolved(_) | LinkState::Outdated(_)) => {
            links.linked(link).ok_or_else(invalid)
        }
    }
}

/// The space of `snapshot` that a placed reference shows: the space named by `space` (an
/// identifier of 16 bytes), or else the first model space in identifier order.
pub(crate) fn linked_space(snapshot: &Snapshot, space: Option<&Value>) -> Result<ObjectId> {
    let invalid = || Error::new(codes::EXTERNAL_SPACE_INVALID);
    match space {
        Some(Value::Opaque(bytes)) => {
            let raw = <[u8; 16]>::try_from(bytes.as_slice()).map_err(|_| invalid())?;
            let id = ObjectId::from_u128(u128::from_be_bytes(raw));
            let handle = snapshot.resolve(id).map_err(|_| invalid())?;
            match snapshot.type_key(handle).map(|k| k.as_str()) {
                Some(types::SPACE) => Ok(id),
                _ => Err(invalid()),
            }
        }
        None => model_space(snapshot).ok_or_else(invalid),
        Some(_) => Err(invalid()),
    }
}

fn model_space(snapshot: &Snapshot) -> Option<ObjectId> {
    snapshot
        .iter_type(types::SPACE)
        .ok()?
        .find(|(_, object)| {
            matches!(
                object.values.get(table::SPACE_KIND),
                Some(Some(Value::Enum(0)))
            )
        })
        .map(|(_, object)| object.id)
}

/// The unit of the space `space` of `snapshot`; unitless when it cannot be read.
pub(crate) fn unit_of(snapshot: &Snapshot, space: ObjectId) -> SpaceUnit {
    snapshot
        .resolve(space)
        .ok()
        .and_then(|h| snapshot.get(h).ok())
        .and_then(|object| match object.values.get(table::SPACE_UNIT) {
            Some(Some(Value::Enum(code))) => SpaceUnit::of(*code),
            _ => None,
        })
        .unwrap_or(SpaceUnit::Unitless)
}

/// The clip boundary of a placed reference, as a loop in the linked coordinates and its vertex
/// count.
fn clip_boundary(
    values: &[Option<Value>],
    options: &DisplayOptions,
) -> Result<Option<(arc_geom::Loop2, usize)>> {
    let Some(Some(Value::Geometry(bytes))) = values.get(CLIP) else {
        return Ok(None);
    };
    let polyline = clip_polyline(&bytes.0)?;
    if !polyline.is_closed() {
        return Err(Error::new(codes::EXTERNAL_CLIP_INVALID));
    }
    let vertices = polyline.vertices().len();
    Ok(Some((
        boundary_loop(&polyline, &options.tolerance)?,
        vertices,
    )))
}

/// The default number of linked-space expansions one top-level display build may make. An
/// expansion is counted when a linked space is built, not when a memoized list is reused.
pub const DEFAULT_MAX_EXPANSIONS: usize = 10_000;

/// The primitives one top-level build may materialize by default (see [`WorkBudget`]). A linked
/// space of up to 100,000 primitives fits one build of it and one placement.
const MAX_MATERIALIZED_PRIMITIVES: usize = 200_000;

/// The work units one top-level build may spend by default (see [`WorkBudget`]).
const MAX_WORK: usize = 1_000_000;

/// The bounds of one top-level display build that expands linked spaces, as
/// `display::space_list_with_work_budget` and the plot functions with a work budget take them.
/// [`WorkBudget::default`] holds the bounds of the functions without one.
///
/// * `expansions`: linked spaces built. Each (linked document, space, nesting depth) is built
///   once per build and then reused by every placement that shows it.
/// * `primitives`: primitives materialized. An expansion charges its primitives when it is built,
///   and each placement charges the copy it makes. A linked space of `n` primitives costs `2n` for
///   its first placement and `n` for each further placement.
/// * `work`: work units. One per entity of a linked space that is visited, visible or not, and
///   the primitives times the clip vertices of each clip cut. An expansion that runs out of work
///   is refused as a whole.
///
/// A placed reference that needs a bound that is used up draws nothing and reports
/// `draft.external_ref.budget_exceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkBudget {
    /// The linked spaces the build may expand.
    pub expansions: usize,
    /// The primitives the build may materialize.
    pub primitives: usize,
    /// The work units the build may spend.
    pub work: usize,
}

impl Default for WorkBudget {
    fn default() -> Self {
        Self {
            expansions: DEFAULT_MAX_EXPANSIONS,
            primitives: MAX_MATERIALIZED_PRIMITIVES,
            work: MAX_WORK,
        }
    }
}

/// What one top-level display build used of its [`WorkBudget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkUse {
    /// The linked spaces expanded.
    pub expansions: usize,
    /// The primitives materialized.
    pub primitives: usize,
    /// The work units spent.
    pub work: usize,
}

/// The untransformed display list of a linked space, as an expansion at one nesting depth.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Expanded {
    pub(crate) list: DisplayList,
    pub(crate) deepest: u32,
}

/// An expansion, or the code of the error that stopped it.
type Outcome = std::result::Result<Arc<Expanded>, &'static str>;

/// The state of one top-level display build that expands linked spaces: the expansions made so
/// far, keyed by linked document, space and nesting depth, the usage of the budget, and which
/// block definitions hold a placed reference.
#[derive(Debug)]
pub(crate) struct Memo {
    budget: WorkBudget,
    used: Cell<WorkUse>,
    /// Set when a work charge does not fit: the expansion in progress is then refused as a whole.
    overrun: Cell<bool>,
    built: RefCell<BTreeMap<(ObjectId, ObjectId, u32), Outcome>>,
    holds: RefCell<BTreeMap<ObjectId, bool>>,
}

impl Memo {
    pub(crate) fn new(budget: WorkBudget) -> Self {
        Self {
            budget,
            used: Cell::new(WorkUse::default()),
            overrun: Cell::new(false),
            built: RefCell::new(BTreeMap::new()),
            holds: RefCell::new(BTreeMap::new()),
        }
    }

    /// What the build has used so far.
    pub(crate) fn usage(&self) -> WorkUse {
        self.used.get()
    }

    /// The primitives the build may still materialize.
    pub(crate) fn primitives_left(&self) -> usize {
        self.budget
            .primitives
            .saturating_sub(self.used.get().primitives)
    }

    /// Charges `count` materialized primitives; `false` when they do not fit the bound.
    fn charge_primitives(&self, count: usize) -> bool {
        let used = self.used.get();
        if count > self.budget.primitives.saturating_sub(used.primitives) {
            return false;
        }
        self.used.set(WorkUse {
            primitives: used.primitives + count,
            ..used
        });
        true
    }

    /// Charges `count` work units; `false` when they do not fit, which marks the build overrun.
    pub(crate) fn charge_work(&self, count: usize) -> bool {
        let used = self.used.get();
        if count > self.budget.work.saturating_sub(used.work) {
            self.overrun.set(true);
            return false;
        }
        self.used.set(WorkUse {
            work: used.work + count,
            ..used
        });
        true
    }

    /// Whether a work charge has failed in this build.
    fn overran(&self) -> bool {
        self.overrun.get()
    }

    /// Takes one expansion from the budget; `false` when none is left.
    fn spend_expansion(&self) -> bool {
        let used = self.used.get();
        if used.expansions >= self.budget.expansions {
            return false;
        }
        self.used.set(WorkUse {
            expansions: used.expansions + 1,
            ..used
        });
        true
    }

    fn lookup(&self, key: (ObjectId, ObjectId, u32)) -> Option<Outcome> {
        self.built.borrow().get(&key).cloned()
    }

    fn store(&self, key: (ObjectId, ObjectId, u32), outcome: Outcome) {
        self.built.borrow_mut().insert(key, outcome);
    }

    /// Whether the block definition `block` holds a placed reference, when it was answered.
    pub(crate) fn holds_of(&self, block: ObjectId) -> Option<bool> {
        self.holds.borrow().get(&block).copied()
    }

    pub(crate) fn remember_holds(&self, block: ObjectId, holds: bool) {
        self.holds.borrow_mut().insert(block, holds);
    }
}

/// What a placed reference reads its linked drawings through: the loaded links of the document
/// that holds the reference, and the memo of the build.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scope<'a> {
    pub(crate) links: &'a Links,
    pub(crate) memo: &'a Memo,
    /// Whether the entities read belong to a linked drawing; their visits are charged.
    pub(crate) linked: bool,
}

/// The expansion of the linked space `space` of `view` at `depth`: memoized in `scope`, and
/// built only while the budget covers it.
fn expansion(
    scope: Scope<'_>,
    view: &Links,
    space: ObjectId,
    options: &DisplayOptions,
    depth: u32,
) -> Outcome {
    let key = (view.document(), space, depth);
    if let Some(found) = scope.memo.lookup(key) {
        return found;
    }
    // The frames of this function lie on the recursion path of nested drawings: keep them small.
    let built = match view.snapshot() {
        Some(snapshot) if scope.memo.spend_expansion() => {
            // The build stops at the primitives left: the build's own list gather is bounded by
            // them through the scope (see `gather`), so a linked space larger than the budget is
            // refused before it is built in full.
            let left = scope.memo.primitives_left();
            let inner = Scope {
                links: view,
                memo: scope.memo,
                linked: true,
            };
            match assemble(snapshot, Lists::Direct, Some(inner), space, options, depth) {
                _ if scope.memo.overran() => Err(codes::EXTERNAL_BUDGET_EXCEEDED.code),
                Ok((list, deepest)) if scope.memo.charge_primitives(list.primitives.len()) => {
                    Ok(Arc::new(Expanded { list, deepest }))
                }
                Ok(_) => Err(codes::EXTERNAL_BUDGET_EXCEEDED.code),
                // The limit that stopped the build was the budget's when it was lower than the bound.
                Err(error)
                    if left < options.max_primitives
                        && error.code() == codes::DISPLAY_TOO_LARGE.code =>
                {
                    Err(codes::EXTERNAL_BUDGET_EXCEEDED.code)
                }
                Err(error) => Err(error.code()),
            }
        }
        Some(_) => Err(codes::EXTERNAL_BUDGET_EXCEEDED.code),
        None => Err(arc_doc::error::codes::LINK_INVALID.code),
    };
    scope.memo.store(key, built.clone());
    built
}

/// The appearance of a linked primitive is the object's own: its layer values were resolved in
/// the linked drawing, so a viewport override of a host layer does not restyle it.
fn detach_layer<T>(value: &mut Option<Sourced<T>>) {
    if let Some(sourced) = value
        && matches!(sourced.source, Source::Layer(_))
    {
        sourced.source = Source::Object;
    }
}

/// The list of a placed reference that draws nothing: its one problem.
fn refusal(base: &Primitive, link: Option<ObjectId>, code: &'static str) -> EntityList {
    EntityList {
        order: base.draw_order,
        depth: 0,
        primitives: Vec::new(),
        problems: vec![ReferenceProblem {
            reference: base.source,
            link,
            code,
        }],
    }
}

/// The expansion of the linked space that the placed reference with `link` and `values` shows,
/// and the unit of that space. On failure, the code of the problem.
fn expand_reference(
    reader: &dyn Reader,
    link: ObjectId,
    values: &[Option<Value>],
    options: &DisplayOptions,
) -> std::result::Result<(Arc<Expanded>, SpaceUnit), &'static str> {
    let Some(scope) = reader.scope() else {
        return Err(codes::EXTERNAL_NO_LINKS.code);
    };
    let view = linked_view(scope.links, link).map_err(|error| error.code())?;
    let nested = reader.depth().saturating_add(1);
    if nested > options.max_nesting {
        return Err(codes::BLOCK_NESTING_TOO_DEEP.code);
    }
    let snapshot = view
        .snapshot()
        .ok_or(arc_doc::error::codes::LINK_INVALID.code)?;
    let space = linked_space(snapshot, values.get(SPACE).and_then(Option::as_ref))
        .map_err(|error| error.code())?;
    let expanded = expansion(scope, &view, space, options, nested)?;
    Ok((expanded, unit_of(snapshot, space)))
}

/// The list of the placed reference `base.source`, with `values` holding its fields after the
/// common ones (see [`FIELD_NAMES`]). A reference whose link or space cannot be read, whose
/// placement is not finite, that is nested too deep, or that the budget does not cover draws
/// nothing and reports one problem.
///
/// The untransformed list of a linked space is built once per build, document, space and depth,
/// and reused by every reference that shows it. A placement charges the copy it makes, and a clip
/// charges the primitives times the clip's vertices; the transform and the clip are applied per
/// reference.
///
/// The primitives of a placed reference carry the layer of the reference itself, not the linked
/// drawing's layers: those were applied inside the expansion, and their identifiers belong to
/// another document.
pub(crate) fn list(
    reader: &dyn Reader,
    base: Primitive,
    values: &[Option<Value>],
    options: &DisplayOptions,
) -> Result<EntityList> {
    let link = match values.get(LINK) {
        Some(Some(Value::Reference(link))) => *link,
        _ => return Ok(refusal(&base, None, codes::EXTERNAL_LINK_INVALID.code)),
    };
    let (position, scale, rotation) = placement_numbers(values);
    if non_finite_placement(position, scale, rotation).is_some() {
        return Ok(refusal(&base, Some(link), codes::FIELD_INVALID.code));
    }
    match expand_reference(reader, link, values, options) {
        Ok((expanded, unit)) => place_copy(reader, base, link, values, options, &expanded, unit),
        Err(code) => Ok(refusal(&base, Some(link), code)),
    }
}

/// The placed copy of an expansion: its problems, its clip, its transform and its layer. The copy
/// is charged to the budget, and so is the clip, per primitive and clip vertex.
fn place_copy(
    reader: &dyn Reader,
    base: Primitive,
    link: ObjectId,
    values: &[Option<Value>],
    options: &DisplayOptions,
    expanded: &Expanded,
    unit: SpaceUnit,
) -> Result<EntityList> {
    let id = base.source;
    let Some(scope) = reader.scope() else {
        return Ok(refusal(&base, Some(link), codes::EXTERNAL_NO_LINKS.code));
    };
    // One problem per reference: the first one below, or the partial drawing.
    let mut problems = Vec::new();
    if let Some(first) = expanded.list.problems.first() {
        problems.push(ReferenceProblem {
            reference: id,
            link: Some(link),
            code: first.code,
        });
    } else if !expanded.list.failed.is_empty() {
        problems.push(ReferenceProblem {
            reference: id,
            link: Some(link),
            code: codes::EXTERNAL_PARTIAL.code,
        });
    }
    let clip = match clip_boundary(values, options) {
        Ok(clip) => clip,
        Err(error) => return Ok(refusal(&base, Some(link), error.code())),
    };
    if !scope.memo.charge_primitives(expanded.list.primitives.len()) {
        return Ok(refusal(
            &base,
            Some(link),
            codes::EXTERNAL_BUDGET_EXCEEDED.code,
        ));
    }
    let tolerance = &options.tolerance;
    let mut primitives = expanded.list.primitives.clone();
    if let Some((boundary, vertices)) = &clip {
        if !scope
            .memo
            .charge_work(primitives.len().saturating_mul(*vertices))
        {
            return Ok(refusal(
                &base,
                Some(link),
                codes::EXTERNAL_BUDGET_EXCEEDED.code,
            ));
        }
        let mut kept = Vec::with_capacity(primitives.len());
        for mut p in primitives {
            if cut_primitive(&mut p, boundary, true, tolerance)? {
                kept.push(p);
            }
        }
        primitives = kept;
    }
    let transform = placement(reader, values, Point2::origin(), unit, reader.owner(id)?)?;
    let mut out = Vec::with_capacity(primitives.len());
    for mut p in primitives {
        p.shape = transform_shape(&p.shape, &transform, tolerance)?;
        // [link, linked entity, nested path...]: the link object, then the object in the linked
        // drawing, as the sub-object key of the primitive.
        p.path.insert(0, p.source);
        p.path.insert(0, link);
        p.source = id;
        p.draw_order = base.draw_order;
        p.layers = base.layers.clone();
        detach_layer(&mut p.color);
        detach_layer(&mut p.linetype);
        detach_layer(&mut p.lineweight);
        detach_layer(&mut p.transparency);
        out.push(p);
    }
    Ok(EntityList {
        order: base.draw_order,
        depth: expanded.deepest.saturating_add(1),
        primitives: out,
        problems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_names_follow_the_entity_slots() {
        let declared: Vec<String> = fields().into_iter().map(|f| f.name).collect();
        assert_eq!(declared, FIELD_NAMES.map(str::to_owned));
        assert_eq!(LINK, entity::KIND);
        assert_eq!(SPACE, entity::REF_ROTATION + 1);
        assert_eq!(CLIP, SPACE + 1);
    }
}

#[cfg(test)]
mod finding_tests {
    use super::*;

    #[test]
    fn a_non_finite_placement_value_is_a_finding() {
        let mut values = vec![None; CLIP + 1];
        values[entity::REF_ROTATION] = Some(Value::F64(f64::NAN));
        let found = findings(&values);
        assert!(
            found
                .iter()
                .any(|f| f.error.code() == codes::FIELD_INVALID.code),
            "{found:?}"
        );
        values[entity::REF_ROTATION] = Some(Value::F64(0.5));
        values[entity::REF_POSITION] = Some(Value::Point2(Point2::new(f64::INFINITY, 0.0)));
        assert!(
            findings(&values)
                .iter()
                .any(|f| f.error.code() == codes::FIELD_INVALID.code)
        );
        values[entity::REF_POSITION] = Some(Value::Point2(Point2::origin()));
        values[entity::REF_SCALE] = Some(Value::Vector3(Vector3::new(1.0, f64::NAN, 1.0)));
        assert!(
            findings(&values)
                .iter()
                .any(|f| f.error.code() == codes::FIELD_INVALID.code)
        );
        values[entity::REF_SCALE] = Some(Value::Vector3(Vector3::new(1.0, 1.0, 1.0)));
        assert!(findings(&values).is_empty());
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use arc_geom::PolylineVertex;

    #[test]
    fn a_clip_boundary_over_the_vertex_limit_is_refused_when_decoded() {
        let vertices = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .map(|(x, y)| PolylineVertex::new(Point2::new(x, y), 0.0))
            .to_vec();
        let square = Polyline2::new(vertices, true).unwrap();
        let bytes = square.encode();
        assert!(clip_polyline_within(&bytes, 4).is_ok());
        let err = clip_polyline_within(&bytes, 3).unwrap_err();
        assert_eq!(err.code(), codes::EXTERNAL_CLIP_INVALID.code);
    }

    #[test]
    fn the_budget_is_spent_once_per_expansion() {
        let memo = Memo::new(WorkBudget {
            expansions: 2,
            ..WorkBudget::default()
        });
        assert!(memo.spend_expansion());
        assert!(memo.spend_expansion());
        assert!(!memo.spend_expansion());
    }

    #[test]
    fn a_placement_charges_only_what_it_copies_and_work_is_bounded() {
        let memo = Memo::new(WorkBudget {
            primitives: 5,
            work: 3,
            ..WorkBudget::default()
        });
        assert!(memo.charge_primitives(5));
        assert!(!memo.charge_primitives(1));
        assert!(memo.charge_work(3));
        assert!(!memo.charge_work(1));
        assert!(memo.overran());
        assert_eq!(
            memo.usage(),
            WorkUse {
                expansions: 0,
                primitives: 5,
                work: 3,
            }
        );
    }
}
