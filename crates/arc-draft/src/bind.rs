//! Copying a linked space into a block definition (CORE-DRAFT-025): the bind of a placed external
//! reference.
//!
//! Binding runs in two steps. [`plan`] reads the linked space and decides every object the copy
//! holds, without changing anything, and refuses a plan over the budget before anything is
//! created. [`write`] allocates the host identifiers of all copies first, and then creates them
//! with their references remapped: a reference to a copied object names its host copy, and any
//! other reference is cleared and counted. Creating an object does not resolve its references, so
//! a copy may name a copy created later. The validators run at commit, so a copy that fails them
//! aborts the whole bind.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::math::Point2;
use arc_base::{Error, Limits, ObjectId, Result};
use arc_command::Context;
use std::sync::Arc;

use arc_doc::{Header, Snapshot, TypeRegistry, Value};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Polyline2};

use crate::clip::{Piece, cut, keeps};
use crate::commands::values_of;
use crate::display::{DisplayOptions, boundary_loop, decode_curve, members};
use crate::error::codes;
use crate::external::WorkBudget;
use crate::types::{self, BLOCK, ENTITIES, entity};
use crate::{array, dimension, hatch, leader, style, table, text};

/// The index of the name: the first field of every named type.
const NAME: usize = 0;

/// The name of the default layer. The crate creates no layer; `"0"` is the layer every drawing
/// starts with in the interchange format, which the DXF bridge uses for its standard layer.
const DEFAULT_LAYER: &str = "0";

/// The types a bound entity refers to by reference: copied once, with their names made unique.
const DEFINITIONS: [&str; 7] = [
    types::LAYER,
    types::LINETYPE,
    style::TEXT_STYLE,
    style::DIM_STYLE,
    style::LEADER_STYLE,
    style::TABLE_STYLE,
    hatch::HATCH_PATTERN,
];

/// The curve entity types: a clip cuts them into pieces.
const CURVES: [&str; 9] = [
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

/// The longest name a named object may hold, in bytes.
fn name_limit() -> usize {
    usize::try_from(types::NAME_BYTES).unwrap_or(usize::MAX)
}

/// The longest prefix of `text` that is at most `max` bytes and ends on a character boundary.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `stem-name`, with the counter `-n` after it when given, and the whole within the name limit.
/// The stem is shortened first; the name is shortened only when the stem has no room left.
fn prefixed(stem: &str, name: &str, counter: Option<usize>) -> String {
    let tail = counter.map(|n| format!("-{n}")).unwrap_or_default();
    let room = name_limit().saturating_sub(tail.len());
    let fixed = name.len() + 1;
    let (stem, name) = if fixed <= room {
        (truncate(stem, room - fixed), name)
    } else {
        ("", truncate(name, room.saturating_sub(1)))
    };
    format!("{stem}-{name}{tail}")
}

/// The name of a copy for `name` that no object of its type holds. The first copy takes `name`,
/// then `stem-name`, then `stem-name-2`, and so on; the chosen name is added to `taken`.
pub(crate) fn unique_name(taken: &mut BTreeSet<String>, stem: &str, name: &str) -> String {
    let mut candidate = name.to_owned();
    if taken.contains(&candidate) {
        candidate = prefixed(stem, name, None);
    }
    let mut n = 2;
    while taken.contains(&candidate) {
        candidate = prefixed(stem, name, Some(n));
        n += 1;
    }
    taken.insert(candidate.clone());
    candidate
}

/// The names held by the objects of type `key` in `snapshot`.
pub(crate) fn names_of(snapshot: &Snapshot, key: &str) -> BTreeSet<String> {
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

/// The names the host holds, by type, and the copies that take them.
struct Names<'a> {
    host: &'a Snapshot,
    taken: BTreeMap<String, BTreeSet<String>>,
}

impl Names<'_> {
    /// A name for a copy of type `key` whose linked name is `name`.
    fn unique(&mut self, key: &str, stem: &str, name: &str) -> String {
        let host = self.host;
        let set = self
            .taken
            .entry(key.to_owned())
            .or_insert_with(|| names_of(host, key));
        unique_name(set, stem, name)
    }
}

/// The host's own default object that a linked default maps to, when the linked object is a
/// default of the crate: the default layer and the `default` styles of each kind. The mapping
/// does not depend on the properties of either object, because defaults are the host's to define.
fn host_default(host: &Snapshot, key: &str, name: &str) -> Option<ObjectId> {
    let is_default = match key {
        types::LAYER => name == DEFAULT_LAYER,
        style::TEXT_STYLE | style::DIM_STYLE | style::LEADER_STYLE | style::TABLE_STYLE => {
            name == style::DEFAULT_NAME
        }
        _ => false,
    };
    if !is_default {
        return None;
    }
    host.iter_type(key)
        .ok()?
        .find(|(_, object)| types::text_at(&object.values, NAME) == Some(name))
        .map(|(_, object)| object.id)
}

/// One object the copy creates in the host.
#[derive(Debug)]
pub(crate) struct Object {
    /// The linked object it copies; `None` for the block that holds the copies and for a piece
    /// of a cut curve, which no reference can name.
    pub(crate) source: Option<ObjectId>,
    /// The type key.
    pub(crate) key: String,
    /// The linked owner: the linked space or block of an entity, the linked owner of a chain.
    pub(crate) owner: Option<ObjectId>,
    /// The header: the source's name, partition and flags. The write sets the owner to the host's.
    pub(crate) header: Header,
    /// The values, in the linked identifiers; the write remaps them.
    pub(crate) values: Vec<Option<Value>>,
}

/// What a clip did to the entities of the space.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Entities kept whole: points and kinds that cannot be cut, inside the boundary, and curves
    /// that lie wholly inside it.
    pub(crate) kept_whole: u64,
    /// Entities left out: outside the boundary, or without any part inside it.
    pub(crate) dropped: u64,
    /// Curves cut into pieces.
    pub(crate) cut: u64,
}

/// The number of copies of each kind of object.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Entities copied into the block definitions, the space's among them.
    pub(crate) entities: u64,
    /// Block definitions copied, the one that holds the space's entities among them.
    pub(crate) blocks: u64,
    /// Layers, linetypes, styles and hatch patterns copied.
    pub(crate) definitions: u64,
    /// Dimension chains copied.
    pub(crate) chains: u64,
}

/// The plan of a bind: every object to create, and the counts of the clip.
#[derive(Debug)]
pub(crate) struct Plan {
    /// The linked space.
    pub(crate) space: ObjectId,
    /// The objects to create, the block that holds the copies at `top`.
    pub(crate) objects: Vec<Object>,
    /// The index of the block that holds the space's entities.
    pub(crate) top: usize,
    /// What the clip did.
    pub(crate) counts: Counts,
    /// Placed references nested in the space that are not copied.
    pub(crate) external: u64,
    /// Linked defaults and the host's own defaults they map to: no object is created for them.
    pub(crate) mapped: Vec<(ObjectId, ObjectId)>,
}

impl Plan {
    /// The number of objects of each kind.
    pub(crate) fn tally(&self) -> Tally {
        let mut tally = Tally::default();
        for object in &self.objects {
            let key = object.key.as_str();
            if key == BLOCK {
                tally.blocks += 1;
            } else if key == dimension::CHAIN {
                tally.chains += 1;
            } else if DEFINITIONS.contains(&key) {
                tally.definitions += 1;
            } else if ENTITIES.contains(&key) {
                tally.entities += 1;
            }
        }
        tally
    }
}

/// The input of a plan that the caller reads from the placed reference and the link.
pub(crate) struct Input<'a> {
    /// The file stem of the link, which prefixes the names of copies that are taken.
    pub(crate) stem: &'a str,
    /// The values of the block that holds the space's entities, its name the space's name.
    pub(crate) block: Vec<Option<Value>>,
    /// The clip boundary of the placed reference, in the linked coordinates.
    pub(crate) clip: Option<Polyline2>,
    /// The work bounds: `primitives` for the objects created, `work` for objects and cut work.
    pub(crate) budget: WorkBudget,
    /// The most objects one bind creates, from the document limits.
    pub(crate) max_objects: u64,
}

/// The objects reached from the space, by container, with their type keys.
#[derive(Debug, Default)]
struct Reach {
    /// The members of the space and of each reached block, in the order of identifiers.
    containers: BTreeMap<ObjectId, Vec<ObjectId>>,
    /// The type key of each reached object: entities, blocks, definitions and chains.
    keys: BTreeMap<ObjectId, String>,
    /// The dimension chains with a member that is copied.
    chains: BTreeSet<ObjectId>,
    /// Placed references nested in the space, which are not copied.
    external: Vec<ObjectId>,
}

enum Visit {
    /// The members of a container (the space or a block), at a block nesting depth.
    Container(ObjectId, u32),
    /// The references of an object, at a block nesting depth.
    Object(ObjectId, u32),
}

fn key_of(snapshot: &Snapshot, id: ObjectId) -> Option<String> {
    snapshot
        .resolve(id)
        .ok()
        .and_then(|handle| snapshot.type_key(handle))
        .map(|key| key.as_str().to_owned())
}

/// Every identifier a value holds, found without recursion.
fn references(values: &[Option<Value>]) -> Vec<ObjectId> {
    let mut found = Vec::new();
    let mut stack: Vec<&Value> = values.iter().flatten().collect();
    while let Some(value) = stack.pop() {
        match value {
            Value::Reference(id) => found.push(*id),
            Value::List(items) => stack.extend(items),
            Value::Record(fields) => stack.extend(fields.iter().flatten()),
            Value::Map(entries) => stack.extend(entries.values()),
            _ => {}
        }
    }
    found
}

/// Records a reached object, refusing the one that passes `max_objects`.
fn admit(reach: &mut Reach, id: ObjectId, key: String, max_objects: u64) -> Result<()> {
    if reach.keys.len() as u64 >= max_objects {
        return Err(Error::new(codes::EXTERNAL_BUDGET_EXCEEDED));
    }
    reach.keys.insert(id, key);
    Ok(())
}

/// Reaches the members of `space`, the blocks they place and the definitions they use. A
/// worklist holds the visits, so no nesting of blocks recurses on the call stack.
///
/// A block is reached at the deepest nesting it is placed at, because a copy holds every
/// placement: a block placed at a deep level and again at a shallow one is visited again from its
/// deeper placement, so the members below it are checked at the depth they reach. Each block is
/// visited at most `max_nesting` times, since each visit is deeper than the last. The walk refuses
/// as soon as a placement is deeper than `max_nesting`, or the reached objects pass `max_objects`.
fn reach(linked: &Snapshot, space: ObjectId, max_nesting: u32, max_objects: u64) -> Result<Reach> {
    let mut reach = Reach::default();
    let mut deepest: BTreeMap<ObjectId, u32> = BTreeMap::new();
    let mut stack = vec![Visit::Container(space, 0)];
    while let Some(visit) = stack.pop() {
        match visit {
            Visit::Container(container, depth) => {
                if !reach.containers.contains_key(&container) {
                    let mut kept = Vec::new();
                    for member in members(linked, container) {
                        let Some(key) = key_of(linked, member) else {
                            continue;
                        };
                        if key == types::EXTERNAL_REF {
                            reach.external.push(member);
                            continue;
                        }
                        admit(&mut reach, member, key, max_objects)?;
                        kept.push(member);
                    }
                    reach.containers.insert(container, kept);
                }
                let kept = reach
                    .containers
                    .get(&container)
                    .cloned()
                    .unwrap_or_default();
                stack.extend(kept.into_iter().map(|member| Visit::Object(member, depth)));
            }
            Visit::Object(object, depth) => {
                for target in references(&values_of(linked, object)?) {
                    let Some(key) = key_of(linked, target) else {
                        continue;
                    };
                    if key == BLOCK {
                        let placed_at = depth + 1;
                        if placed_at > max_nesting {
                            return Err(Error::new(codes::BLOCK_NESTING_TOO_DEEP));
                        }
                        if deepest
                            .get(&target)
                            .is_some_and(|known| *known >= placed_at)
                        {
                            continue;
                        }
                        deepest.insert(target, placed_at);
                        if !reach.keys.contains_key(&target) {
                            admit(&mut reach, target, key, max_objects)?;
                        }
                        stack.push(Visit::Container(target, placed_at));
                    } else if DEFINITIONS.contains(&key.as_str())
                        && !reach.keys.contains_key(&target)
                    {
                        admit(&mut reach, target, key, max_objects)?;
                        stack.push(Visit::Object(target, depth));
                    }
                    // Other targets are entities (copied only as members of a copied container,
                    // so a reference to one is cleared when its container is not copied) or
                    // objects of the linked document that a copy cannot hold.
                }
            }
        }
    }

    let copied: BTreeSet<ObjectId> = reach.containers.values().flatten().copied().collect();
    for chain in linked.iter_type(dimension::CHAIN)? {
        let (_, object) = chain;
        if references(&object.values)
            .iter()
            .any(|member| copied.contains(member))
        {
            reach.chains.insert(object.id);
            reach.keys.insert(object.id, dimension::CHAIN.to_owned());
        }
    }
    Ok(reach)
}

/// The header of a linked object.
fn header_of(snapshot: &Snapshot, id: ObjectId) -> Result<Header> {
    Ok(snapshot.get(snapshot.resolve(id)?)?.header.clone())
}

/// The entity values of a clipped curve piece: the common fields of `values`, the kind values of
/// the piece's own type.
fn piece(
    values: &[Option<Value>],
    curve: &Curve2,
    registry: &TypeRegistry,
    header: Header,
) -> Object {
    let (key, kind) = curve_entity(curve);
    let mut out = types::defaults(registry, key);
    for (i, value) in values.iter().enumerate().take(entity::KIND) {
        out[i] = value.clone();
    }
    let mut kind = kind;
    if key == types::POLYLINE
        && let Some(Some(flag)) = values.get(entity::CONTINUOUS_PATTERN)
    {
        kind.push((entity::CONTINUOUS_PATTERN, flag.clone()));
    }
    for (i, value) in kind {
        out[i] = Some(value);
    }
    Object {
        source: None,
        key: key.to_owned(),
        owner: None,
        header,
        values: out,
    }
}

/// The type a curve becomes, and the values of its geometry. Every entity holds its geometry in
/// the kind slot, as the commands that create entities do.
fn curve_entity(curve: &Curve2) -> (&'static str, Vec<(usize, Value)>) {
    let geometry = || {
        vec![(
            entity::KIND,
            Value::Geometry(arc_doc::GeometryBytes(curve.encode())),
        )]
    };
    match curve {
        Curve2::Segment(_) => (types::LINE, geometry()),
        Curve2::Ray(_) => (types::RAY, geometry()),
        Curve2::Line(_) => (types::CONSTRUCTION_LINE, geometry()),
        Curve2::Circle(_) => (types::CIRCLE, geometry()),
        Curve2::Arc(_) => (types::ARC, geometry()),
        Curve2::Ellipse(_) => (types::ELLIPSE, geometry()),
        Curve2::EllipticalArc(_) => (types::ELLIPTICAL_ARC, geometry()),
        Curve2::Polyline(_) => (types::POLYLINE, geometry()),
        Curve2::Nurbs(nurbs) => (types::SPLINE, crate::spline::values(nurbs, &[])),
    }
}

/// The point that decides whether a clip keeps a non-curve entity: its insertion or anchor point.
/// `None` when the kind has none, which keeps the entity.
fn anchor(key: &str, values: &[Option<Value>]) -> Result<Option<Point2>> {
    let point = |index: usize| match values.get(index) {
        Some(Some(Value::Point2(p))) => Some(*p),
        _ => None,
    };
    let first_point = |index: usize| match values.get(index) {
        Some(Some(Value::List(items))) => items.iter().find_map(|v| match v {
            Value::Point2(p) => Some(*p),
            _ => None,
        }),
        _ => None,
    };
    let lower_corner = |curve: Option<Curve2>| {
        curve
            .and_then(|c| c.bounding_box())
            .map(|bounds| bounds.min())
    };
    Ok(match key {
        text::TEXT | text::RICH_TEXT => point(text::field::POSITION),
        types::BLOCK_REF => point(entity::REF_POSITION),
        array::ARRAY => point(array::field::POSITION),
        table::TABLE => point(table::field::POSITION),
        leader::MULTILEADER => point(leader::multi::POSITION),
        dimension::DIMENSION => point(dimension::field::LINE_POSITION),
        leader::LEADER => first_point(leader::line::POINTS),
        hatch::HATCH => match point(hatch::field::PICK) {
            Some(p) => Some(p),
            None => lower_corner(match values.get(hatch::field::LOOPS) {
                Some(Some(Value::List(items))) => match items.first() {
                    Some(Value::Geometry(bytes)) => Curve2::decode(&bytes.0, &Limits::DEFAULT).ok(),
                    _ => None,
                },
                _ => None,
            }),
        },
        types::WIPEOUT => lower_corner(decode_curve(values.get(entity::KIND)).ok()),
        _ => None,
    })
}

/// Plans the copy of the linked `space` into the host, as a block definition. Reads both
/// snapshots and changes nothing; refuses a plan over the budget.
///
/// # Errors
///
/// `draft.block.nesting_too_deep` for block nesting above the display limit,
/// `draft.external_ref.clip_invalid` for a clip that is not a closed polyline of the crate's
/// clip kind, and `draft.external_ref.budget_exceeded` when the plan needs more objects or work
/// than the budget or the limits allow. The walk and the planning loop stop at the first excess.
#[expect(
    clippy::too_many_lines,
    reason = "the entities of each container, the definitions and chains, then the budget"
)]
pub(crate) fn plan(
    linked: &Snapshot,
    host: &Snapshot,
    space: ObjectId,
    input: Input<'_>,
) -> Result<Plan> {
    let options = DisplayOptions::default();
    let work_bound = u64::try_from(input.budget.work).unwrap_or(u64::MAX);
    let object_bound = input
        .max_objects
        .min(u64::try_from(input.budget.primitives).unwrap_or(u64::MAX));
    let reach = reach(linked, space, options.max_nesting, object_bound)?;
    let boundary = match &input.clip {
        Some(polyline) => Some(
            boundary_loop(polyline, &options.tolerance)
                .map_err(|_| Error::new(codes::EXTERNAL_CLIP_INVALID))?,
        ),
        None => None,
    };
    let clip_vertices = input.clip.as_ref().map_or(0, |p| p.vertices().len() as u64);
    let registry = Arc::clone(linked.registry());
    let mut names = Names {
        host,
        taken: BTreeMap::new(),
    };
    let mut counts = Counts::default();
    let mut cut_work = 0_u64;
    let mut mapped = Vec::new();
    // Whether the copies so far, and the cut work so far, are over a bound.
    let over = |objects: usize, work: u64| {
        (objects as u64) > object_bound || (objects as u64).saturating_add(work) > work_bound
    };
    let refused = || Error::new(codes::EXTERNAL_BUDGET_EXCEEDED);

    let mut block = input.block;
    let space_name = crate::types::text_at(&block, NAME)
        .unwrap_or_default()
        .to_owned();
    block[NAME] = Some(Value::String(names.unique(BLOCK, input.stem, &space_name)));
    let mut objects = vec![Object {
        source: None,
        key: BLOCK.to_owned(),
        owner: None,
        header: Header::default(),
        values: block,
    }];
    let top = 0;

    // The entities of each container: the space's are clipped, the blocks' are copied whole.
    for (container, members) in &reach.containers {
        let mut sorted = Vec::with_capacity(members.len());
        for member in members {
            let key = reach.keys.get(member).cloned().unwrap_or_default();
            let values = values_of(linked, *member)?;
            let order = match values.get(entity::DRAW_ORDER) {
                Some(Some(Value::I64(order))) => *order,
                _ => 0,
            };
            sorted.push((order, *member, key, values));
        }
        sorted.sort_by_key(|(order, id, _, _)| (*order, *id));
        for (_, source, key, values) in sorted {
            if over(objects.len(), cut_work) {
                return Err(refused());
            }
            let header = header_of(linked, source)?;
            let Some(clip) = boundary.as_ref().filter(|_| *container == space) else {
                objects.push(Object {
                    source: Some(source),
                    key,
                    owner: Some(*container),
                    header,
                    values,
                });
                continue;
            };
            if key == types::POINT {
                let inside = match values.get(entity::KIND) {
                    Some(Some(Value::Point2(p))) => keeps(*p, clip, true, &options.tolerance)?,
                    _ => true,
                };
                if inside {
                    counts.kept_whole += 1;
                    objects.push(Object {
                        source: Some(source),
                        key,
                        owner: Some(*container),
                        header,
                        values,
                    });
                } else {
                    counts.dropped += 1;
                }
            } else if CURVES.contains(&key.as_str()) {
                let curve = decode_curve(values.get(entity::KIND))?;
                if over(objects.len(), cut_work.saturating_add(clip_vertices)) {
                    return Err(refused());
                }
                let pieces = cut(&curve, clip, true, &options.tolerance)?;
                cut_work = cut_work.saturating_add(clip_vertices);
                // A curve the clip returns unchanged keeps its object, so references to it hold.
                let unchanged = matches!(pieces.as_slice(), [Piece::Curve(c)] if *c == curve);
                if unchanged {
                    counts.kept_whole += 1;
                    objects.push(Object {
                        source: Some(source),
                        key,
                        owner: Some(*container),
                        header,
                        values,
                    });
                    continue;
                }
                counts.cut += 1;
                let before = objects.len();
                for piece_of in pieces {
                    if let Piece::Curve(c) = piece_of {
                        let mut copy = piece(&values, &c, &registry, header.clone());
                        copy.owner = Some(*container);
                        objects.push(copy);
                    }
                }
                if objects.len() == before {
                    counts.dropped += 1;
                }
            } else {
                let inside = match anchor(&key, &values)? {
                    Some(p) => keeps(p, clip, true, &options.tolerance)?,
                    None => true,
                };
                if inside {
                    counts.kept_whole += 1;
                    objects.push(Object {
                        source: Some(source),
                        key,
                        owner: Some(*container),
                        header,
                        values,
                    });
                } else {
                    counts.dropped += 1;
                }
            }
        }
    }

    // The blocks the space places, the definitions they use, and the chains of copied members.
    for (id, key) in &reach.keys {
        if over(objects.len(), cut_work) {
            return Err(refused());
        }
        let is_block = key == BLOCK;
        let is_definition = DEFINITIONS.contains(&key.as_str());
        if !is_block && !is_definition && key != dimension::CHAIN {
            continue;
        }
        if key == dimension::CHAIN && !reach.chains.contains(id) {
            continue;
        }
        let mut values = values_of(linked, *id)?;
        let owner = if key == dimension::CHAIN {
            linked
                .resolve(*id)
                .ok()
                .and_then(|h| linked.get(h).ok())
                .and_then(|object| object.header.owner)
        } else {
            None
        };
        if is_block || is_definition {
            let name = types::text_at(&values, NAME)
                .ok_or_else(|| Error::new(codes::FIELD_INVALID).with_arg("field", "name"))?
                .to_owned();
            if is_definition && let Some(host_default) = host_default(host, key, &name) {
                mapped.push((*id, host_default));
                continue;
            }
            values[NAME] = Some(Value::String(names.unique(key, input.stem, &name)));
        }
        objects.push(Object {
            source: Some(*id),
            key: key.clone(),
            owner,
            header: header_of(linked, *id)?,
            values,
        });
    }

    let total = objects.len() as u64;
    let budget_exceeded = total > object_bound || total.saturating_add(cut_work) > work_bound;
    if budget_exceeded {
        return Err(refused());
    }
    Ok(Plan {
        space,
        objects,
        top,
        counts,
        external: reach.external.len() as u64,
        mapped,
    })
}

/// What the write reports besides the objects it created.
#[derive(Debug, Default)]
pub(crate) struct Written {
    /// The host identifiers of the plan's objects, in the order of the plan.
    pub(crate) ids: Vec<ObjectId>,
    /// The references that were cleared, because their target is not copied.
    pub(crate) cleared: u64,
}

/// The value with every reference remapped to its host copy. A reference without a copy is
/// cleared and counted. Its place is kept where a record holds it: the field is absent, the state
/// a reader of the record treats as not associated, and the record keeps its position in its list.
/// A list element that is itself a reference is dropped, as a deleted member is removed from the
/// list that holds it.
fn remap(value: &Value, map: &BTreeMap<ObjectId, ObjectId>, cleared: &mut u64) -> Option<Value> {
    Some(match value {
        Value::Reference(id) => match map.get(id) {
            Some(host) => Value::Reference(*host),
            None => {
                *cleared += 1;
                return None;
            }
        },
        Value::ExternalReference(_) => {
            *cleared += 1;
            return None;
        }
        Value::List(items) => Value::List(
            items
                .iter()
                .filter_map(|item| remap(item, map, cleared))
                .collect(),
        ),
        Value::Record(fields) => Value::Record(
            fields
                .iter()
                .map(|field| field.as_ref().and_then(|v| remap(v, map, cleared)))
                .collect(),
        ),
        Value::Map(entries) => Value::Map(
            entries
                .iter()
                .filter_map(|(key, v)| remap(v, map, cleared).map(|v| (key.clone(), v)))
                .collect(),
        ),
        other => other.clone(),
    })
}

/// Creates the objects of `plan` in the host transaction. Every identifier is allocated first,
/// so the remapping sees all copies.
///
/// # Errors
///
/// The transaction's errors: a value the type does not accept fails the bind before the commit.
pub(crate) fn write(ctx: &mut Context<'_>, plan: &Plan) -> Result<Written> {
    let ids: Vec<ObjectId> = plan.objects.iter().map(|_| ctx.new_id()).collect();
    let mut map: BTreeMap<ObjectId, ObjectId> = plan
        .objects
        .iter()
        .zip(&ids)
        .filter_map(|(object, host)| object.source.map(|source| (source, *host)))
        .collect();
    map.extend(plan.mapped.iter().copied());
    let top_host = ids
        .get(plan.top)
        .copied()
        .ok_or_else(|| Error::new(codes::FIELD_INVALID).with_arg("field", "block"))?;
    let mut cleared = 0_u64;
    for (object, host) in plan.objects.iter().zip(&ids) {
        let values: Vec<Option<Value>> = object
            .values
            .iter()
            .map(|v| v.as_ref().and_then(|v| remap(v, &map, &mut cleared)))
            .collect();
        let owner = match object.owner {
            None => None,
            Some(owner) if owner == plan.space => Some(top_host),
            Some(owner) => map.get(&owner).copied(),
        };
        let mut header = object.header.clone();
        header.owner = owner;
        ctx.txn()?.create(&object.key, *host, header, values)?;
    }
    Ok(Written { ids, cleared })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_base::id::Identifier;

    fn id(n: u128) -> ObjectId {
        ObjectId::from_u128(n)
    }

    #[test]
    fn a_record_whose_reference_is_not_copied_keeps_its_place_in_a_list() {
        // The path of a dimension point is a list of one-field records: a cleared member keeps its
        // position, so the members after it do not shift.
        let map = BTreeMap::from([(id(1), id(3)), (id(4), id(6))]);
        let mut cleared = 0;
        let path = Value::List(vec![
            Value::Record(vec![Some(Value::Reference(id(1)))]),
            Value::Record(vec![Some(Value::Reference(id(2)))]),
            Value::Record(vec![Some(Value::Reference(id(4)))]),
        ]);
        let out = remap(&path, &map, &mut cleared);
        assert_eq!(cleared, 1);
        assert_eq!(
            out,
            Some(Value::List(vec![
                Value::Record(vec![Some(Value::Reference(id(3)))]),
                Value::Record(vec![None]),
                Value::Record(vec![Some(Value::Reference(id(6)))]),
            ]))
        );
    }

    #[test]
    fn a_reference_list_drops_a_member_without_a_copy() {
        let map = BTreeMap::from([(id(1), id(3))]);
        let mut cleared = 0;
        let members = Value::List(vec![Value::Reference(id(1)), Value::Reference(id(2))]);
        assert_eq!(
            remap(&members, &map, &mut cleared),
            Some(Value::List(vec![Value::Reference(id(3))]))
        );
        assert_eq!(cleared, 1);
    }
}
