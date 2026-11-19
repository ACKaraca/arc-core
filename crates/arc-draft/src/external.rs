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

/// The validator of [`EXTERNAL_REF`]: the common checks, a nonzero scale and a clip that is a
/// closed polyline.
pub(crate) fn findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut found = types::common_findings(values);
    if let Some(Some(Value::Vector3(s))) = values.get(entity::REF_SCALE)
        && (s.x == 0.0 || s.y == 0.0 || s.z == 0.0)
    {
        found.push(types::finding(Error::new(codes::BLOCK_SCALE_ZERO)));
    }
    if let Some(Some(Value::Geometry(bytes))) = values.get(CLIP) {
        let closed = Polyline2::decode(&bytes.0, &Limits::DEFAULT).is_ok_and(|p| p.is_closed());
        if !closed {
            found.push(types::finding(Error::new(codes::EXTERNAL_CLIP_INVALID)));
        }
    }
    found
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

/// The clip boundary of a placed reference, as a loop in the linked coordinates.
fn clip_boundary(
    values: &[Option<Value>],
    options: &DisplayOptions,
) -> Result<Option<arc_geom::Loop2>> {
    let Some(Some(Value::Geometry(bytes))) = values.get(CLIP) else {
        return Ok(None);
    };
    let polyline = Polyline2::decode(&bytes.0, &Limits::DEFAULT)
        .map_err(|_| Error::new(codes::EXTERNAL_CLIP_INVALID))?;
    if !polyline.is_closed() {
        return Err(Error::new(codes::EXTERNAL_CLIP_INVALID));
    }
    boundary_loop(&polyline, &options.tolerance).map(Some)
}

/// The default number of linked-space expansions one top-level display build may make. An
/// expansion is counted when a linked space is built, not when a memoized list is reused.
pub const DEFAULT_MAX_EXPANSIONS: usize = 10_000;

/// The primitives one top-level display build may hold in expansions and copy into placed
/// references. Expansions and copies are charged, so the work of a build stays bounded whatever
/// the number of references and the depth.
const MAX_MATERIALIZED_PRIMITIVES: usize = 100_000;

/// The untransformed display list of a linked space, as an expansion at one nesting depth.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Expanded {
    pub(crate) list: DisplayList,
    pub(crate) deepest: u32,
}

/// An expansion, or the code of the error that stopped it.
type Outcome = std::result::Result<Arc<Expanded>, &'static str>;

/// The state of one top-level display build that expands linked spaces: the expansions made so
/// far, keyed by linked document, space and nesting depth, and the budget that is left.
#[derive(Debug)]
pub(crate) struct Memo {
    budget: Cell<usize>,
    primitives: Cell<usize>,
    built: RefCell<BTreeMap<(ObjectId, ObjectId, u32), Outcome>>,
}

impl Memo {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            budget: Cell::new(budget),
            primitives: Cell::new(MAX_MATERIALIZED_PRIMITIVES),
            built: RefCell::new(BTreeMap::new()),
        }
    }

    /// Charges `count` primitives to the build; `false` when they do not fit the bound.
    fn charge(&self, count: usize) -> bool {
        let left = self.primitives.get();
        if count > left {
            return false;
        }
        self.primitives.set(left - count);
        true
    }

    fn lookup(&self, key: (ObjectId, ObjectId, u32)) -> Option<Outcome> {
        self.built.borrow().get(&key).cloned()
    }

    fn store(&self, key: (ObjectId, ObjectId, u32), outcome: Outcome) {
        self.built.borrow_mut().insert(key, outcome);
    }

    /// Takes one expansion from the budget; `false` when none is left.
    fn spend(&self) -> bool {
        let left = self.budget.get();
        if left == 0 {
            return false;
        }
        self.budget.set(left - 1);
        true
    }
}

/// What a placed reference reads its linked drawings through: the loaded links of the document
/// that holds the reference, and the memo of the build.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scope<'a> {
    pub(crate) links: &'a Links,
    pub(crate) memo: &'a Memo,
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
    let built = match view.snapshot() {
        Some(snapshot) if scope.memo.spend() => {
            let inner = Scope {
                links: view,
                memo: scope.memo,
            };
            assemble(snapshot, Lists::Direct, Some(inner), space, options, depth)
                .map_err(|error| error.code())
                .and_then(|(list, deepest)| {
                    if scope.memo.charge(list.primitives.len()) {
                        Ok(Arc::new(Expanded { list, deepest }))
                    } else {
                        Err(codes::EXTERNAL_BUDGET_EXCEEDED.code)
                    }
                })
        }
        Some(_) => Err(codes::EXTERNAL_BUDGET_EXCEEDED.code),
        None => Err(arc_doc::error::codes::LINK_INVALID.code),
    };
    scope.memo.store(key, built.clone());
    built
}

/// The list of the placed reference `base.source`, with `values` holding its fields after the
/// common ones (see [`FIELD_NAMES`]). A reference whose link or space cannot be read, that is
/// nested too deep, or that the budget does not cover draws nothing and reports one problem.
///
/// The untransformed list of a linked space is built once per build, document, space and depth,
/// and reused by every reference that shows it; the transform and the clip are applied per
/// reference.
pub(crate) fn list(
    reader: &dyn Reader,
    base: Primitive,
    values: &[Option<Value>],
    options: &DisplayOptions,
) -> Result<EntityList> {
    let id = base.source;
    let refused = |link: Option<ObjectId>, code: &'static str| {
        Ok(EntityList {
            order: base.draw_order,
            depth: 0,
            primitives: Vec::new(),
            problems: vec![ReferenceProblem {
                reference: id,
                link,
                code,
            }],
        })
    };
    let link = match values.get(LINK) {
        Some(Some(Value::Reference(link))) => *link,
        _ => return refused(None, codes::EXTERNAL_LINK_INVALID.code),
    };
    let Some(scope) = reader.scope() else {
        return refused(Some(link), codes::EXTERNAL_NO_LINKS.code);
    };
    let view = match linked_view(scope.links, link) {
        Ok(view) => view,
        Err(error) => return refused(Some(link), error.code()),
    };
    let nested = reader.depth().saturating_add(1);
    if nested > options.max_nesting {
        return refused(Some(link), codes::BLOCK_NESTING_TOO_DEEP.code);
    }
    let Some(snapshot) = view.snapshot() else {
        return refused(Some(link), arc_doc::error::codes::LINK_INVALID.code);
    };
    let space = match linked_space(snapshot, values.get(SPACE).and_then(Option::as_ref)) {
        Ok(space) => space,
        Err(error) => return refused(Some(link), error.code()),
    };
    let outcome = expansion(scope, &view, space, options, nested);
    let expanded = match outcome {
        Ok(expanded) => expanded,
        Err(code) => return refused(Some(link), code),
    };
    if expanded.list.primitives.len() > options.max_primitives {
        return refused(Some(link), codes::DISPLAY_TOO_LARGE.code);
    }
    if !scope.memo.charge(expanded.list.primitives.len()) {
        return refused(Some(link), codes::EXTERNAL_BUDGET_EXCEEDED.code);
    }
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
        Err(error) => return refused(Some(link), error.code()),
    };
    let tolerance = &options.tolerance;
    let mut primitives = expanded.list.primitives.clone();
    if let Some(boundary) = &clip {
        let mut kept = Vec::with_capacity(primitives.len());
        for mut p in primitives {
            if cut_primitive(&mut p, boundary, true, tolerance)? {
                kept.push(p);
            }
        }
        primitives = kept;
    }
    let from = unit_of(snapshot, space);
    let transform = placement(reader, values, Point2::origin(), from, reader.owner(id)?)?;
    let mut out = Vec::with_capacity(primitives.len());
    for mut p in primitives {
        p.shape = transform_shape(&p.shape, &transform, tolerance)?;
        // [link, linked entity, nested path...]: the link object, then the object in the linked
        // drawing, as the sub-object key of the primitive.
        p.path.insert(0, p.source);
        p.path.insert(0, link);
        p.source = id;
        p.draw_order = base.draw_order;
        p.layers.extend(base.layers.iter().copied());
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
mod budget_tests {
    use super::*;

    #[test]
    fn the_budget_is_spent_once_per_expansion() {
        let memo = Memo::new(2);
        assert!(memo.spend());
        assert!(memo.spend());
        assert!(!memo.spend());
    }
}
