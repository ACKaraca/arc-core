//! Commands of placed external drawing references (CORE-DRAFT-025): creating one, changing its
//! clip boundary, and binding it into a block definition.
//!
//! `core.draft.external_ref.create` and `core.draft.external_ref.set_clip` are registered by
//! [`crate::register_commands`]. Bind needs the loaded links of the document, which a command
//! cannot reach, so it is not registered there: [`bind_command`] returns its specification with
//! the links the product loaded, and the product registers it.

use arc_base::id::Identifier;
use arc_base::math::Vector3;
use arc_base::{Error, ObjectId, Result};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::link::{LINK_TYPE, Links};
use arc_doc::{FieldPath, GeometryBytes, Header, Value};
use arc_geom::Curve2;
use arc_geom::encoding::GeomCodec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::bind::{self, Input};
use crate::commands::{Created, create_entity, expect, field_invalid, point2, spec, values_of};
use crate::error::codes;
use crate::external::{self, WorkBudget, unit_of};
use crate::types::{self, BLOCK_REF, CONTAINERS, EXTERNAL_REF, SpaceUnit, entity, table};

/// Parameters of `core.draft.external_ref.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateExternal {
    /// The space or block definition the reference belongs to.
    pub owner: IdParam,
    /// The `core.doc.link` object of this document that names the linked drawing.
    pub link: IdParam,
    /// The space of the linked drawing (its identifier there); the model space when absent.
    #[serde(default)]
    pub space: Option<IdParam>,
    /// The insertion point, in the host's coordinates.
    pub position: [f64; 2],
    /// The per-axis scale; 1 when absent.
    #[serde(default)]
    pub scale: Option<[f64; 3]>,
    /// The rotation in radians.
    #[serde(default)]
    pub rotation: f64,
    /// The clip boundary: a closed polyline in the coordinates of the linked drawing.
    #[serde(default)]
    pub clip: Option<serde_json::Value>,
    /// The layer of the reference.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// Parameters of `core.draft.external_ref.set_clip`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetClip {
    /// The placed references to change.
    pub objects: Vec<IdParam>,
    /// The new clip boundary, a closed polyline in the linked drawing's coordinates; `null`
    /// removes the clip.
    pub clip: Option<serde_json::Value>,
}

/// Parameters of the bind command.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bind {
    /// The placed reference to bind.
    pub reference: IdParam,
}

/// The encoded clip boundary of a JSON closed polyline; `None` for `null`. A boundary with more
/// than [`external::CLIP_MAX_VERTICES`] vertices is refused.
fn clip_value(json: Option<serde_json::Value>) -> Result<Option<Value>> {
    clip_value_within(json, external::CLIP_MAX_VERTICES)
}

/// As [`clip_value`], with at most `max_vertices` vertices in the boundary.
fn clip_value_within(
    json: Option<serde_json::Value>,
    max_vertices: usize,
) -> Result<Option<Value>> {
    let Some(json) = json else {
        return Ok(None);
    };
    let invalid = || Error::new(codes::EXTERNAL_CLIP_INVALID);
    let curve: Curve2 = serde_json::from_value(json).map_err(|_| invalid())?;
    match &curve {
        Curve2::Polyline(polyline) if polyline.is_closed() => {
            if polyline.vertices().len() > max_vertices {
                return Err(invalid().with_detail("reason", "vertex count"));
            }
            Ok(Some(Value::Geometry(GeometryBytes(curve.encode()))))
        }
        _ => Err(invalid()),
    }
}

/// Creates a placed external reference in `owner`.
pub(crate) fn create_reference(ctx: &mut Context<'_>, p: CreateExternal) -> Result<Created> {
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, p.owner.0, CONTAINERS)?;
    expect(&snapshot, p.link.0, &[LINK_TYPE])?;
    let [sx, sy, sz] = p.scale.unwrap_or([1.0; 3]);
    if let Some(field) = external::non_finite_placement(p.position, [sx, sy, sz], p.rotation) {
        return Err(field_invalid(field));
    }
    let mut kind = vec![
        (external::LINK, Value::Reference(p.link.0)),
        (entity::REF_POSITION, point2(p.position)),
        (entity::REF_SCALE, Value::Vector3(Vector3::new(sx, sy, sz))),
        (entity::REF_ROTATION, Value::F64(p.rotation)),
    ];
    if let Some(IdParam(space)) = p.space {
        kind.push((
            external::SPACE,
            Value::Opaque(space.as_u128().to_be_bytes().to_vec()),
        ));
    }
    if let Some(clip) = clip_value(p.clip)? {
        kind.push((external::CLIP, clip));
    }
    create_entity(ctx, EXTERNAL_REF, p.owner.0, p.layer, kind)
}

/// Replaces the clip boundary of the placed references `p.objects`.
pub(crate) fn set_clip(ctx: &mut Context<'_>, p: SetClip) -> Result<()> {
    let clip = clip_value(p.clip)?;
    let txn = ctx.txn()?;
    for IdParam(id) in p.objects {
        expect(txn.snapshot(), id, &[EXTERNAL_REF])?;
        txn.modify(id, FieldPath::new("clip"), clip.clone())?;
    }
    Ok(())
}

/// The specification of `core.draft.external_ref.create`.
pub(crate) fn create_spec() -> CommandSpec {
    spec("core.draft.external_ref.create", create_reference)
}

/// The specification of `core.draft.external_ref.set_clip`.
pub(crate) fn set_clip_spec() -> CommandSpec {
    spec("core.draft.external_ref.set_clip", set_clip)
}

/// The specification of `core.draft.external_ref.bind`, with the loaded links of the document.
///
/// The command runs in one transaction, so one undo restores the placed reference and removes
/// everything it created. It copies the linked space into a new block definition, named after the
/// space (or `<file name>-<space name>`, with a counter, when the name is taken), and replaces the
/// placed reference with a block reference that has its insertion transform, its common fields and
/// its header. The link object is deleted when no other placed reference uses it.
///
/// Every entity type of the space is copied with its values and header. The definitions an entity
/// refers to (layers, linetypes, text, dimension, leader and table styles, hatch patterns) and the
/// blocks it places are copied once each, recursively; a name the host holds is taken under the
/// `<file name>-<name>` prefix. A linked default (the layer `0` and the `default` styles) maps to the
/// host's own default of the same kind and is not copied. Each reference of a copy names its host
/// copy; a reference to an object that is not copied is cleared and reported. A clip boundary cuts curves and keeps the
/// other entities whose insertion or anchor point is inside it. Placed references nested in the
/// space are not copied, and are reported. The copy is bounded by `budget` and the document's
/// record limit; an over-large space is refused before anything is created.
pub fn bind_command(links: Links) -> CommandSpec {
    bind_command_with_budget(links, WorkBudget::default())
}

/// As [`bind_command`], with the work bounds of the copy given by `budget`.
pub fn bind_command_with_budget(links: Links, budget: WorkBudget) -> CommandSpec {
    spec("core.draft.external_ref.bind", move |ctx, p: Bind| {
        bind(ctx, &links, budget, p)
    })
}

/// A problem a bind reports without refusing: a count of one kind of loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Problem {
    /// The stable code, as the error catalog names it.
    pub code: String,
    /// The number of occurrences.
    pub count: u64,
}

/// The result of `core.draft.external_ref.bind`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Bound {
    /// The block reference that replaces the placed reference.
    pub id: IdParam,
    /// The block definition that holds the copies.
    pub block: IdParam,
    /// Entities copied into the block definitions, after the clip.
    pub entities: u64,
    /// Block definitions copied, the bound one not among them.
    pub blocks: u64,
    /// Layers, linetypes, styles and hatch patterns copied.
    pub definitions: u64,
    /// Dimension chains copied.
    pub chains: u64,
    /// Entities kept whole by the clip (zero without a clip).
    pub clip_kept_whole: u64,
    /// Entities left out by the clip (zero without a clip).
    pub clip_dropped: u64,
    /// Curves cut by the clip (zero without a clip).
    pub clip_cut: u64,
    /// Placed references nested in the space that were not copied.
    pub external_references_not_copied: u64,
    /// The losses that did not refuse the bind.
    pub problems: Vec<Problem>,
}

#[expect(
    clippy::too_many_lines,
    reason = "one transaction: the plan, its writes, the block reference and the report"
)]
fn bind(ctx: &mut Context<'_>, links: &Links, budget: WorkBudget, p: Bind) -> Result<Bound> {
    let limits = *ctx.limits();
    let snapshot = ctx.txn()?.snapshot().clone();
    let reference = p.reference.0;
    expect(&snapshot, reference, &[EXTERNAL_REF])?;
    let placed = values_of(&snapshot, reference)?;
    let link = types::reference_at(&placed, external::LINK).ok_or_else(|| field_invalid("link"))?;
    let view = external::linked_view(links, link)?;
    let linked = view
        .snapshot()
        .ok_or_else(|| Error::new(arc_doc::error::codes::LINK_INVALID))?;
    let space =
        external::linked_space(linked, placed.get(external::SPACE).and_then(Option::as_ref))?;
    let link_values = values_of(&snapshot, link)?;
    let stem = stem_of(types::text_at(&link_values, 1).unwrap_or_default());
    let placed_header = snapshot.get(snapshot.resolve(reference)?)?.header.clone();
    let placed_owner = placed_header
        .owner
        .ok_or_else(|| Error::new(codes::EXTERNAL_LINK_INVALID))?;
    let space_values = values_of(linked, space)?;
    let space_name = types::text_at(&space_values, table::NAME)
        .filter(|name| !name.is_empty())
        .unwrap_or("model")
        .to_owned();
    let unit: SpaceUnit = unit_of(linked, space);
    let clip = match placed.get(external::CLIP) {
        Some(Some(Value::Geometry(bytes))) => Some(external::clip_polyline(&bytes.0)?),
        _ => None,
    };
    let block_values = vec![
        Some(Value::String(space_name)),
        Some(point2([0.0, 0.0])),
        Some(Value::Enum(unit.code())),
        None,
        Some(Value::List(Vec::new())),
    ];
    let plan = bind::plan(
        linked,
        &snapshot,
        space,
        Input {
            stem: &stem,
            block: block_values,
            clip,
            budget,
            max_objects: limits.record_count,
        },
    )?;
    let counts = plan.counts;
    let tally = plan.tally();
    let external = plan.external;
    let written = bind::write(ctx, &plan)?;
    let block = written.ids[plan.top];

    // The placed reference becomes a block reference with the same transform and common fields.
    let mut block_ref = types::defaults(snapshot.registry(), BLOCK_REF);
    for (i, value) in placed.iter().enumerate().take(entity::KIND) {
        block_ref[i] = value.clone();
    }
    block_ref[entity::KIND] = Some(Value::Reference(block));
    block_ref[entity::REF_POSITION] = placed.get(entity::REF_POSITION).cloned().flatten();
    block_ref[entity::REF_SCALE] = placed.get(entity::REF_SCALE).cloned().flatten();
    block_ref[entity::REF_ROTATION] = placed.get(entity::REF_ROTATION).cloned().flatten();
    block_ref[entity::REF_ATTRIBUTES] = Some(Value::Map(std::collections::BTreeMap::new()));
    let txn = ctx.txn()?;
    txn.delete(reference)?;
    let others = txn.snapshot().referrers(link).any(|r| {
        txn.snapshot()
            .resolve(r.source)
            .ok()
            .and_then(|h| txn.snapshot().type_key(h))
            .is_some_and(|k| k.as_str() == EXTERNAL_REF)
    });
    if !others {
        txn.delete(link)?;
    }
    let inserted: ObjectId = ctx.new_id();
    ctx.txn()?.create(
        BLOCK_REF,
        inserted,
        Header {
            owner: Some(placed_owner),
            ..placed_header
        },
        block_ref,
    )?;
    ctx.txn()?.apply_delete_rules(&limits)?;
    ctx.set_selection_after(vec![inserted]);

    let mut problems = Vec::new();
    if written.cleared > 0 {
        problems.push(Problem {
            code: codes::EXTERNAL_BIND_REFERENCE_CLEARED.code.to_owned(),
            count: written.cleared,
        });
    }
    if external > 0 {
        problems.push(Problem {
            code: codes::EXTERNAL_BIND_NOT_COPIED.code.to_owned(),
            count: external,
        });
    }
    Ok(Bound {
        id: IdParam(inserted),
        block: IdParam(block),
        entities: tally.entities,
        blocks: tally.blocks.saturating_sub(1),
        definitions: tally.definitions,
        chains: tally.chains,
        clip_kept_whole: counts.kept_whole,
        clip_dropped: counts.dropped,
        clip_cut: counts.cut,
        external_references_not_copied: external,
        problems,
    })
}

/// The file name of a link's path without its directory and extension.
fn stem_of(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    if stem.is_empty() {
        "linked".to_owned()
    } else {
        stem.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bind::unique_name;
    use arc_geom::{Polyline2, PolylineVertex};
    use std::collections::BTreeSet;

    #[test]
    fn stems_and_unique_names() {
        assert_eq!(stem_of("../drawings/site plan.arc"), "site plan");
        assert_eq!(stem_of("C:\\work\\block"), "block");
        assert_eq!(stem_of(""), "linked");
        let mut taken: BTreeSet<String> = ["0".to_owned()].into_iter().collect();
        assert_eq!(unique_name(&mut taken, "site", "0"), "site-0");
        assert_eq!(unique_name(&mut taken, "site", "0"), "site-0-2");
        assert_eq!(unique_name(&mut taken, "site", "1"), "1");
    }

    #[test]
    fn a_clip_over_the_vertex_cap_is_refused_by_create_and_set_clip() {
        // Both commands encode their clip with the same function.
        let vertices = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .map(|(x, y)| PolylineVertex::new(arc_base::math::Point2::new(x, y), 0.0))
            .to_vec();
        let square =
            serde_json::to_value(Curve2::Polyline(Polyline2::new(vertices, true).unwrap()))
                .unwrap();
        assert!(
            clip_value_within(Some(square.clone()), 4)
                .unwrap()
                .is_some()
        );
        let err = clip_value_within(Some(square), 3).unwrap_err();
        assert_eq!(err.code(), codes::EXTERNAL_CLIP_INVALID.code);
        assert!(err.details().contains_key("reason"), "{err:?}");
    }

    #[test]
    fn clip_values_must_be_closed_polylines() {
        assert!(clip_value(None).unwrap().is_none());
        let open = serde_json::json!({"kind": "segment", "start": [0.0, 0.0], "end": [1.0, 0.0]});
        assert_eq!(
            clip_value(Some(open)).unwrap_err().code(),
            codes::EXTERNAL_CLIP_INVALID.code
        );
    }
}
