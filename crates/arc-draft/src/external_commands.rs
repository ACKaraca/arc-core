//! Commands of placed external drawing references (CORE-DRAFT-025): creating one, changing its
//! clip boundary, and binding it into a block definition.
//!
//! `core.draft.external_ref.create` and `core.draft.external_ref.set_clip` are registered by
//! [`crate::register_commands`]. Bind needs the loaded links of the document, which a command
//! cannot reach, so it is not registered there: [`bind_command`] returns its specification with
//! the links the product loaded, and the product registers it.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::id::Identifier;
use arc_base::math::Vector3;
use arc_base::{Error, Limits, ObjectId, Result};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::link::{LINK_TYPE, Links};
use arc_doc::{FieldPath, GeometryBytes, Snapshot, Value};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Polyline2};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::clip::{Piece, cut, keeps};
use crate::commands::{
    Created, create, create_entity, expect, field_invalid, point2, spec, values_of,
};
use crate::display::{DisplayOptions, boundary_loop, decode_curve, members};
use crate::error::codes;
use crate::external::{self, unit_of};
use crate::types::{
    self, BLOCK, BLOCK_REF, CONTAINERS, EXTERNAL_REF, LAYER, LINETYPE, SpaceUnit, entity, layer,
    table,
};

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

/// The encoded clip boundary of a JSON closed polyline; `None` for `null`.
fn clip_value(json: Option<serde_json::Value>) -> Result<Option<Value>> {
    let Some(json) = json else {
        return Ok(None);
    };
    let invalid = || Error::new(codes::EXTERNAL_CLIP_INVALID);
    let curve: Curve2 = serde_json::from_value(json).map_err(|_| invalid())?;
    match &curve {
        Curve2::Polyline(polyline) if polyline.is_closed() => {
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
/// everything it created. It copies the point and curve entities of the linked space into a new
/// block definition, named after the space (renamed when the name is taken), together with the
/// layers and linetypes they use (renamed with the link's file name as prefix when their names
/// exist in the host). It replaces the placed reference with a block reference that has the same
/// insertion transform and common fields, and deletes the link object when no other placed
/// reference uses it.
///
/// A clip boundary is applied to the copied geometry (the block references of this crate do not
/// clip), so the copies are the parts inside it. Other entity types are refused, with
/// `draft.external_ref.bind_unsupported`.
pub fn bind_command(links: Links) -> CommandSpec {
    spec("core.draft.external_ref.bind", move |ctx, p: Bind| {
        bind(ctx, &links, p)
    })
}

/// The entity types that bind copies.
const BIND_KINDS: &[&str] = &[
    types::POINT,
    types::LINE,
    types::RAY,
    types::CONSTRUCTION_LINE,
    types::CIRCLE,
    types::ARC,
    types::ELLIPSE,
    types::ELLIPTICAL_ARC,
    types::POLYLINE,
    types::SPLINE,
];

/// A name that is not in `taken`: `name`, else `stem-name`, else `stem-name-2`, and so on.
fn unique_name(taken: &mut BTreeSet<String>, stem: &str, name: &str) -> String {
    let mut candidate = name.to_owned();
    let mut n = 2;
    if taken.contains(&candidate) {
        candidate = format!("{stem}-{name}");
    }
    while taken.contains(&candidate) {
        candidate = format!("{stem}-{name}-{n}");
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
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

/// The names of the objects of type `key` in `snapshot`.
fn names_of(snapshot: &Snapshot, key: &str) -> BTreeSet<String> {
    snapshot
        .iter_type(key)
        .map(|objects| {
            objects
                .filter_map(|(_, object)| match object.values.first() {
                    Some(Some(Value::String(name))) => Some(name.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The copies of the layers and linetypes the bound entities use, by their linked identifiers.
struct Styles<'a> {
    linked: &'a Snapshot,
    stem: String,
    layers: BTreeMap<ObjectId, ObjectId>,
    linetypes: BTreeMap<ObjectId, ObjectId>,
    taken_layers: BTreeSet<String>,
    taken_linetypes: BTreeSet<String>,
}

impl Styles<'_> {
    /// The host copy of the linked layer `id`, created on first use.
    fn layer(&mut self, ctx: &mut Context<'_>, id: ObjectId) -> Result<ObjectId> {
        if let Some(copy) = self.layers.get(&id) {
            return Ok(*copy);
        }
        let mut values = values_of(self.linked, id)?;
        let name = match values.get(layer::NAME) {
            Some(Some(Value::String(name))) => name.clone(),
            _ => return Err(Error::new(codes::FIELD_INVALID).with_arg("field", "name")),
        };
        let stem = self.stem.clone();
        values[layer::NAME] = Some(Value::String(unique_name(
            &mut self.taken_layers,
            &stem,
            &name,
        )));
        if let Some(linetype) = types::reference_at(&values, layer::LINETYPE) {
            let copy = self.linetype(ctx, linetype)?;
            values[layer::LINETYPE] = Some(Value::Reference(copy));
        }
        let copy = create(ctx, LAYER, None, values)?;
        self.layers.insert(id, copy);
        Ok(copy)
    }

    /// The host copy of the linked linetype `id`, created on first use.
    fn linetype(&mut self, ctx: &mut Context<'_>, id: ObjectId) -> Result<ObjectId> {
        if let Some(copy) = self.linetypes.get(&id) {
            return Ok(*copy);
        }
        let mut values = values_of(self.linked, id)?;
        let name = match values.first() {
            Some(Some(Value::String(name))) => name.clone(),
            _ => return Err(Error::new(codes::FIELD_INVALID).with_arg("field", "name")),
        };
        let stem = self.stem.clone();
        values[0] = Some(Value::String(unique_name(
            &mut self.taken_linetypes,
            &stem,
            &name,
        )));
        let copy = create(ctx, LINETYPE, None, values)?;
        self.linetypes.insert(id, copy);
        Ok(copy)
    }

    /// The values of a linked entity with its layer and linetype replaced by host copies, and
    /// the common fields kept.
    fn common(
        &mut self,
        ctx: &mut Context<'_>,
        values: &[Option<Value>],
    ) -> Result<Vec<Option<Value>>> {
        let mut common: Vec<Option<Value>> = values.iter().take(entity::KIND).cloned().collect();
        if let Some(Some(Value::Reference(id))) = common.get(entity::LAYER).cloned() {
            let copy = self.layer(ctx, id)?;
            common[entity::LAYER] = Some(Value::Reference(copy));
        }
        if let Some(Some(Value::Reference(id))) = common.get(entity::LINETYPE).cloned() {
            let copy = self.linetype(ctx, id)?;
            common[entity::LINETYPE] = Some(Value::Reference(copy));
        }
        Ok(common)
    }
}

/// The entity type a curve becomes, and the values of its geometry.
fn curve_entity(curve: &Curve2) -> (&'static str, Vec<(usize, Value)>) {
    match curve {
        Curve2::Segment(_) => (types::LINE, vec![]),
        Curve2::Ray(_) => (types::RAY, vec![]),
        Curve2::Line(_) => (types::CONSTRUCTION_LINE, vec![]),
        Curve2::Circle(_) => (types::CIRCLE, vec![]),
        Curve2::Arc(_) => (types::ARC, vec![]),
        Curve2::Ellipse(_) => (types::ELLIPSE, vec![]),
        Curve2::EllipticalArc(_) => (types::ELLIPTICAL_ARC, vec![]),
        Curve2::Polyline(_) => (types::POLYLINE, vec![]),
        Curve2::Nurbs(nurbs) => (types::SPLINE, crate::spline::values(nurbs, &[])),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one transaction: the styles, the block, its entities and the replaced reference"
)]
fn bind(ctx: &mut Context<'_>, links: &Links, p: Bind) -> Result<Created> {
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
    let placed_owner = snapshot
        .get(snapshot.resolve(reference)?)?
        .header
        .owner
        .ok_or_else(|| Error::new(codes::EXTERNAL_LINK_INVALID))?;
    let space_values = values_of(linked, space)?;
    let space_name = types::text_at(&space_values, table::NAME)
        .filter(|name| !name.is_empty())
        .unwrap_or("model")
        .to_owned();
    let unit: SpaceUnit = unit_of(linked, space);

    // Refuse before changing anything: every entity must be a kind bind copies.
    let tolerance = DisplayOptions::default().tolerance;
    let mut entities: Vec<(ObjectId, i64, &'static str)> = Vec::new();
    for member in members(linked, space) {
        let key = linked
            .resolve(member)
            .ok()
            .and_then(|h| linked.type_key(h))
            .map(|k| k.as_str().to_owned())
            .unwrap_or_default();
        let Some(kind) = BIND_KINDS.iter().copied().find(|known| *known == key) else {
            return Err(Error::new(codes::EXTERNAL_BIND_UNSUPPORTED).with_arg("type", key));
        };
        let values = values_of(linked, member)?;
        let order = match values.get(entity::DRAW_ORDER) {
            Some(Some(Value::I64(order))) => *order,
            _ => 0,
        };
        entities.push((member, order, kind));
    }
    entities.sort_by_key(|(id, order, _)| (*order, *id));
    let clip = match placed.get(external::CLIP) {
        Some(Some(Value::Geometry(bytes))) => {
            let polyline = Polyline2::decode(&bytes.0, &Limits::DEFAULT)
                .map_err(|_| Error::new(codes::EXTERNAL_CLIP_INVALID))?;
            Some(
                boundary_loop(&polyline, &tolerance)
                    .map_err(|_| Error::new(codes::EXTERNAL_CLIP_INVALID))?,
            )
        }
        _ => None,
    };

    let block_names = names_of(&snapshot, BLOCK);
    let block_name = {
        let mut taken = block_names;
        unique_name(&mut taken, &stem, &space_name)
    };
    let mut styles = Styles {
        linked,
        stem: stem.clone(),
        layers: BTreeMap::new(),
        linetypes: BTreeMap::new(),
        taken_layers: names_of(&snapshot, LAYER),
        taken_linetypes: names_of(&snapshot, LINETYPE),
    };
    let block_values = vec![
        Some(Value::String(block_name)),
        Some(point2([0.0, 0.0])),
        Some(Value::Enum(unit.code())),
        None,
        Some(Value::List(Vec::new())),
    ];
    let block = create(ctx, BLOCK, None, block_values)?;

    for (member, _, kind) in entities {
        let values = values_of(linked, member)?;
        // Each copy: the entity type and its values after the common fields.
        let copies: Vec<(&'static str, Vec<(usize, Value)>)> = match &clip {
            None => {
                let all = values
                    .iter()
                    .enumerate()
                    .skip(entity::KIND)
                    .filter_map(|(i, v)| v.clone().map(|v| (i, v)))
                    .collect();
                vec![(kind, all)]
            }
            Some(boundary) if kind == types::POINT => match values.get(entity::KIND) {
                Some(Some(Value::Point2(p))) if keeps(*p, boundary, true, &tolerance)? => {
                    vec![(types::POINT, vec![(entity::KIND, Value::Point2(*p))])]
                }
                _ => Vec::new(),
            },
            Some(boundary) => {
                let curve = decode_curve(values.get(entity::KIND))?;
                let mut out = Vec::new();
                for piece in cut(&curve, boundary, true, &tolerance)? {
                    if let Piece::Curve(c) = piece {
                        let (copy_key, mut kind_values) = curve_entity(&c);
                        if copy_key == types::POLYLINE
                            && let Some(Some(flag)) = values.get(entity::CONTINUOUS_PATTERN)
                        {
                            kind_values.push((entity::CONTINUOUS_PATTERN, flag.clone()));
                        }
                        out.push((copy_key, kind_values));
                    }
                }
                out
            }
        };
        if copies.is_empty() {
            continue;
        }
        let common = styles.common(ctx, &values)?;
        for (copy_key, kind_values) in copies {
            let mut vals = types::defaults(snapshot.registry(), copy_key);
            for (i, value) in common.iter().enumerate() {
                if let Some(slot) = vals.get_mut(i) {
                    *slot = value.clone();
                }
            }
            for (i, value) in kind_values {
                if let Some(slot) = vals.get_mut(i) {
                    *slot = Some(value);
                }
            }
            create(ctx, copy_key, Some(block), vals)?;
        }
    }

    // The placed reference becomes a block reference with the same transform and common fields.
    let mut block_ref = types::defaults(snapshot.registry(), BLOCK_REF);
    for (i, value) in placed.iter().enumerate().take(entity::KIND) {
        block_ref[i] = value.clone();
    }
    block_ref[entity::KIND] = Some(Value::Reference(block));
    block_ref[entity::REF_POSITION] = placed.get(entity::REF_POSITION).cloned().flatten();
    block_ref[entity::REF_SCALE] = placed.get(entity::REF_SCALE).cloned().flatten();
    block_ref[entity::REF_ROTATION] = placed.get(entity::REF_ROTATION).cloned().flatten();
    block_ref[entity::REF_ATTRIBUTES] = Some(Value::Map(BTreeMap::new()));
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
    let inserted = create(ctx, BLOCK_REF, Some(placed_owner), block_ref)?;
    ctx.txn()?.apply_delete_rules(&limits)?;
    ctx.set_selection_after(vec![inserted]);
    Ok(Created {
        id: IdParam(inserted),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn clip_values_must_be_closed_polylines() {
        assert!(clip_value(None).unwrap().is_none());
        let open = serde_json::json!({"kind": "segment", "start": [0.0, 0.0], "end": [1.0, 0.0]});
        assert_eq!(
            clip_value(Some(open)).unwrap_err().code(),
            codes::EXTERNAL_CLIP_INVALID.code
        );
    }
}
