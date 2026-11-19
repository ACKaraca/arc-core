//! Typed views over the generic model (CORE-IO-014): the spatial structure, the element types,
//! the property sets and the quantity sets.
//!
//! Attribute positions are those of IFC2X3, IFC4 and IFC4X3, which agree for every attribute the
//! views read, except the quantity and bounded-value layouts that the reader reads by position and
//! that the fixtures cover per schema. The schema is not consulted; the caller can read
//! [`IfcFile::schema`] to know which schema the values came from. A value of another kind than the
//! view expects is not read and is counted in the loss report as dropped.
//!
//! Each distinct instance is materialized once per [`TypedViews`]: the views share it through
//! [`Arc`], so a name listed many times in the file costs one entity. Every list of related
//! instances is deduplicated while it is read, and the views charge the bytes they allocate against
//! `memory_bytes` (the share left after the parser) and the entries they emit against
//! `record_count`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::{Cancel, Error, Limits, Result};

use crate::error::{codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog, LossReport, Reason, Source};
use crate::model::{IfcFile, Instance, Parameter, Record};
use crate::step::MAX_NESTING;

/// The entity types the typed views read. Records of any other type are kept as generic instances
/// and counted as preserved in the loss report of the read.
pub(crate) const UNDERSTOOD: &[&str] = &[
    "IFCPROJECT",
    "IFCSITE",
    "IFCBUILDING",
    "IFCBUILDINGSTOREY",
    "IFCSPACE",
    "IFCSPATIALZONE",
    "IFCFACILITY",
    "IFCFACILITYPART",
    "IFCROAD",
    "IFCBRIDGE",
    "IFCRAILWAY",
    "IFCMARINEFACILITY",
    "IFCWALL",
    "IFCWALLSTANDARDCASE",
    "IFCWALLTYPE",
    "IFCRELAGGREGATES",
    "IFCRELCONTAINEDINSPATIALSTRUCTURE",
    "IFCRELDEFINESBYTYPE",
    "IFCRELDEFINESBYPROPERTIES",
    "IFCPROPERTYSET",
    "IFCPROPERTYSINGLEVALUE",
    "IFCPROPERTYENUMERATEDVALUE",
    "IFCPROPERTYLISTVALUE",
    "IFCPROPERTYBOUNDEDVALUE",
    "IFCELEMENTQUANTITY",
    "IFCQUANTITYLENGTH",
    "IFCQUANTITYAREA",
    "IFCQUANTITYVOLUME",
    "IFCQUANTITYCOUNT",
    "IFCQUANTITYWEIGHT",
    "IFCQUANTITYTIME",
];

/// The spatial structure types, in the order of the spatial hierarchy.
const SPATIAL: &[&str] = &[
    "IFCPROJECT",
    "IFCSITE",
    "IFCBUILDING",
    "IFCBUILDINGSTOREY",
    "IFCSPACE",
    "IFCSPATIALZONE",
    "IFCFACILITY",
    "IFCFACILITYPART",
    "IFCROAD",
    "IFCBRIDGE",
    "IFCRAILWAY",
    "IFCMARINEFACILITY",
];

/// The property types the views read.
const PROPERTY_TYPES: &[&str] = &[
    "IFCPROPERTYSINGLEVALUE",
    "IFCPROPERTYENUMERATEDVALUE",
    "IFCPROPERTYLISTVALUE",
    "IFCPROPERTYBOUNDEDVALUE",
];

/// Estimated bytes the views allocate for an entity, besides its copied text.
const ENTITY_BYTES: u64 = 96;
/// Estimated bytes for a property, besides its copied values.
const PROPERTY_BYTES: u64 = 128;
/// Estimated bytes for a quantity.
const QUANTITY_BYTES: u64 = 128;
/// Estimated bytes for a property or quantity set.
const SET_BYTES: u64 = 96;
/// Estimated bytes for a spatial node.
const NODE_BYTES: u64 = 96;
/// Estimated bytes for a type assignment.
const ASSIGNMENT_BYTES: u64 = 32;
/// Estimated bytes for one shared reference in a list.
const LINK_BYTES: u64 = 8;
/// Estimated bytes for one copied value, such as a nominal value.
const NOMINAL_BYTES: u64 = 48;

/// An object with the rooted attributes of IFC: its instance name, its type, and `GlobalId`,
/// `Name` and `Description` where the file gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootedEntity {
    /// The instance name in the file.
    pub instance: u64,
    /// The entity type, in upper case.
    pub type_name: String,
    /// The `GlobalId`.
    pub global_id: Option<String>,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Description`.
    pub description: Option<String>,
}

/// A spatial structure element with its children and the elements it contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpatialNode {
    /// The element.
    pub entity: Arc<RootedEntity>,
    /// The aggregated spatial children, in file order.
    pub children: Vec<SpatialNode>,
    /// The elements contained in this structure, in file order.
    pub contained: Vec<Arc<RootedEntity>>,
}

/// An element and the type object it is defined by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAssignment {
    /// The element.
    pub element: Arc<RootedEntity>,
    /// The type object.
    pub element_type: Arc<RootedEntity>,
}

/// A value of a typed nominal value: the text, number, or enumeration (booleans and logicals
/// are enumerations `T`, `F` and `U`).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A text value.
    Text(String),
    /// A real value.
    Real(f64),
    /// An integer value.
    Integer(i64),
    /// An enumeration value, without its dots.
    Enumeration(String),
}

/// A nominal value with the type it is wrapped in, such as `IFCLABEL`, when the file wraps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Nominal {
    /// The wrapping type, in upper case.
    pub type_name: Option<String>,
    /// The value.
    pub value: Value,
}

/// The value of a property, by the property's type.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// `IFCPROPERTYSINGLEVALUE`.
    Single {
        /// The nominal value.
        nominal: Option<Nominal>,
        /// The unit, as the instance name of the unit.
        unit: Option<u64>,
    },
    /// `IFCPROPERTYENUMERATEDVALUE`.
    Enumerated {
        /// The enumeration values.
        values: Vec<Nominal>,
        /// The instance name of the enumeration reference, if any.
        reference: Option<u64>,
    },
    /// `IFCPROPERTYLISTVALUE`.
    List {
        /// The values in order.
        values: Vec<Nominal>,
        /// The unit, as the instance name of the unit.
        unit: Option<u64>,
    },
    /// `IFCPROPERTYBOUNDEDVALUE`.
    Bounded {
        /// The upper bound.
        upper: Option<Nominal>,
        /// The lower bound.
        lower: Option<Nominal>,
        /// The unit, as the instance name of the unit.
        unit: Option<u64>,
        /// The set point; absent in IFC2X3, which has no `SetPointValue`.
        set_point: Option<Nominal>,
    },
}

/// A property of a property set. Each property is built once per instance and shared.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    /// The instance name of the property.
    pub instance: u64,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Description`.
    pub description: Option<String>,
    /// The value.
    pub value: PropertyValue,
}

/// A property set with the objects it is assigned to.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertySet {
    /// The property set.
    pub definition: Arc<RootedEntity>,
    /// The properties in file order, without repeats.
    pub properties: Vec<Arc<Property>>,
    /// The objects the set is assigned to, in file order, without repeats.
    pub related: Vec<Arc<RootedEntity>>,
}

/// A numeric quantity value: an integer for counts, a real otherwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// An integer value.
    Integer(i64),
    /// A real value.
    Real(f64),
}

/// The kind of a quantity, by its IFC type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantityKind {
    /// `IFCQUANTITYLENGTH`.
    Length,
    /// `IFCQUANTITYAREA`.
    Area,
    /// `IFCQUANTITYVOLUME`.
    Volume,
    /// `IFCQUANTITYCOUNT`.
    Count,
    /// `IFCQUANTITYWEIGHT`.
    Weight,
    /// `IFCQUANTITYTIME`.
    Time,
}

/// A quantity of a quantity set. Each quantity is built once per instance and shared.
#[derive(Debug, Clone, PartialEq)]
pub struct Quantity {
    /// The instance name of the quantity.
    pub instance: u64,
    /// The kind of the quantity.
    pub kind: QuantityKind,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Description`.
    pub description: Option<String>,
    /// The unit, as the instance name of the unit.
    pub unit: Option<u64>,
    /// The value.
    pub value: Option<Number>,
    /// The `Formula`; absent in IFC2X3, which has none.
    pub formula: Option<String>,
}

/// A quantity set with the objects it is assigned to.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantitySet {
    /// The quantity set.
    pub definition: Arc<RootedEntity>,
    /// The quantities in file order, without repeats.
    pub quantities: Vec<Arc<Quantity>>,
    /// The objects the set is assigned to, in file order, without repeats.
    pub related: Vec<Arc<RootedEntity>>,
}

/// All typed views of a file and the loss report of building them.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedViews {
    /// The roots of the spatial structure, in file order (normally one `IFCPROJECT`).
    pub spatial: Vec<SpatialNode>,
    /// The type assignments, in file order.
    pub types: Vec<TypeAssignment>,
    /// The property sets, in order of first assignment.
    pub property_sets: Vec<PropertySet>,
    /// The quantity sets, in order of first assignment.
    pub quantity_sets: Vec<QuantitySet>,
    /// The loss report of the views.
    pub report: LossReport,
}

/// A value of the wrong kind for its attribute.
#[derive(Debug, Clone, Copy)]
struct Bad;

type Got<T> = std::result::Result<T, Bad>;

/// A list of references or values: the items kept, and the counts of repeated and unusable entries.
#[derive(Debug)]
struct Listed<T> {
    items: Vec<T>,
    repeated: u64,
    bad: u64,
}

impl<T> Listed<T> {
    fn empty() -> Self {
        Self {
            items: Vec::new(),
            repeated: 0,
            bad: 0,
        }
    }

    fn single(item: T) -> Self {
        Self {
            items: vec![item],
            repeated: 0,
            bad: 0,
        }
    }
}

impl IfcFile {
    /// Builds the typed views. Cancellation is checked for every instance and every node; the
    /// views charge their allocations against `memory_bytes` (the share left after the parser) and
    /// their entries against `record_count`.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` for the nesting depth of the spatial structure, for `memory_bytes` and
    /// for `record_count`; `base.cancelled` when the token was cancelled.
    pub fn typed_views(&self, limits: &Limits, cancel: &Cancel) -> Result<TypedViews> {
        cancel.check()?;
        let mut reader = Reader::new(self, limits, cancel);
        reader.scan()?;
        let spatial = reader.build_spatial()?;
        let types = reader.type_assignments()?;
        let property_sets = reader.property_sets()?;
        let quantity_sets = reader.quantity_sets()?;
        let report = reader.log.finish(LossHeader::new(
            "ifc",
            self.schema.identifier(),
            Direction::Import,
            Some(*limits),
        ));
        Ok(TypedViews {
            spatial,
            types,
            property_sets,
            quantity_sets,
            report,
        })
    }
}

struct Reader<'f> {
    file: &'f IfcFile,
    cancel: Cancel,
    depth_limit: u32,
    /// The bytes the views may allocate, or `None` when `memory_bytes` is unlimited.
    memory: Option<u64>,
    used: u64,
    record_limit: u64,
    emitted: u64,
    log: LossLog,
    spatial: BTreeSet<u64>,
    children: BTreeMap<u64, Vec<u64>>,
    child_set: BTreeSet<u64>,
    aggregated: BTreeSet<(u64, u64)>,
    contained: BTreeMap<u64, Vec<u64>>,
    contained_seen: BTreeSet<(u64, u64)>,
    typed: Vec<(u64, u64)>,
    typed_seen: BTreeSet<(u64, u64)>,
    /// The definitions of property and quantity sets, in order of first assignment.
    order: Vec<u64>,
    objects: BTreeMap<u64, Vec<u64>>,
    object_seen: BTreeSet<(u64, u64)>,
    entities: BTreeMap<u64, Arc<RootedEntity>>,
    properties: BTreeMap<u64, Arc<Property>>,
    quantities: BTreeMap<u64, Arc<Quantity>>,
    placed: BTreeSet<u64>,
    path: BTreeSet<u64>,
}

impl<'f> Reader<'f> {
    fn new(file: &'f IfcFile, limits: &Limits, cancel: &Cancel) -> Self {
        Self {
            file,
            cancel: cancel.clone(),
            depth_limit: limits.nesting_depth.min(MAX_NESTING),
            memory: limits
                .memory_bytes
                .map(|max| max.saturating_sub(file.parsed_bytes())),
            used: 0,
            record_limit: limits.record_count,
            emitted: 0,
            log: LossLog::default(),
            spatial: BTreeSet::new(),
            children: BTreeMap::new(),
            child_set: BTreeSet::new(),
            aggregated: BTreeSet::new(),
            contained: BTreeMap::new(),
            contained_seen: BTreeSet::new(),
            typed: Vec::new(),
            typed_seen: BTreeSet::new(),
            order: Vec::new(),
            objects: BTreeMap::new(),
            object_seen: BTreeSet::new(),
            entities: BTreeMap::new(),
            properties: BTreeMap::new(),
            quantities: BTreeMap::new(),
            placed: BTreeSet::new(),
            path: BTreeSet::new(),
        }
    }

    /// Charges `bytes` before they are allocated; refuses when the views would exceed their share.
    fn charge(&mut self, bytes: u64) -> Result<()> {
        self.used = self.used.saturating_add(bytes);
        match self.memory {
            Some(max) if self.used > max => Err(limit("memory_bytes", self.used)),
            _ => Ok(()),
        }
    }

    /// Counts one emitted entry against `record_count`.
    fn emit(&mut self) -> Result<()> {
        self.emitted = self.emitted.saturating_add(1);
        if self.emitted > self.record_limit {
            Err(limit("record_count", self.emitted))
        } else {
            Ok(())
        }
    }

    /// Counts an emitted struct and charges its fixed size.
    fn fixed(&mut self, bytes: u64) -> Result<()> {
        self.emit()?;
        self.charge(bytes)
    }

    /// Counts one shared reference in a list.
    fn link(&mut self) -> Result<()> {
        self.emit()?;
        self.charge(LINK_BYTES)
    }

    fn lose(&mut self, reason: Reason, kind: &str, source: u64, count: u64) {
        self.log
            .add(reason, kind, Some(Source::Decimal(source)), count);
    }

    fn bad(&mut self, source: u64, kind: &str) {
        self.lose(reasons::VALUE_UNEXPECTED, kind, source, 1);
    }

    /// Logs the repeated and unusable entries of a list as one aggregated count each.
    fn note_list<T>(&mut self, listed: &Listed<T>, source: u64, kind: &str) {
        self.lose(reasons::DUPLICATE_PLACEMENT, kind, source, listed.repeated);
        self.lose(reasons::VALUE_UNEXPECTED, kind, source, listed.bad);
    }

    /// The references of a list attribute, with the list's own repeats and unusable entries logged.
    fn references(
        &mut self,
        parameter: Option<&Parameter>,
        source: u64,
        kind: &str,
    ) -> Listed<u64> {
        match got_references(parameter) {
            Ok(listed) => {
                self.note_list(&listed, source, kind);
                listed
            }
            Err(Bad) => {
                self.bad(source, kind);
                Listed::empty()
            }
        }
    }

    /// The value of an optional attribute, or `None` when it is absent or of the wrong kind.
    fn opt<T>(&mut self, value: Got<Option<T>>, source: u64, kind: &str) -> Option<T> {
        match value {
            Ok(value) => value,
            Err(Bad) => {
                self.bad(source, kind);
                None
            }
        }
    }

    /// Two passes: the spatial elements first, so that relationships declared before their
    /// elements (forward references) resolve.
    fn scan(&mut self) -> Result<()> {
        let file = self.file;
        for instance in file.instances() {
            self.cancel.check()?;
            if instance
                .records
                .iter()
                .any(|r| SPATIAL.contains(&r.type_name.as_str()))
            {
                self.spatial.insert(instance.name);
            }
        }
        for instance in file.instances() {
            self.cancel.check()?;
            for record in &instance.records {
                self.relationship(instance.name, record);
            }
        }
        Ok(())
    }

    fn relationship(&mut self, name: u64, record: &Record) {
        match record.type_name.as_str() {
            "IFCRELAGGREGATES" => self.aggregates(name, record),
            "IFCRELCONTAINEDINSPATIALSTRUCTURE" => self.containment(name, record),
            "IFCRELDEFINESBYTYPE" => self.typing(name, record),
            "IFCRELDEFINESBYPROPERTIES" => self.definitions_of(name, record),
            _ => {}
        }
    }

    /// `IfcRelAggregates`: `RelatingObject` 4, `RelatedObjects` 5.
    fn aggregates(&mut self, name: u64, record: &Record) {
        let kind = record.type_name.as_str();
        let parts = (
            got_reference(record.parameters.get(4)),
            got_references(record.parameters.get(5)),
        );
        let (Ok(Some(parent)), Ok(children)) = parts else {
            return self.bad(name, kind);
        };
        self.note_list(&children, name, kind);
        if !self.file.contains(parent) {
            let count = to_u64(children.items.len());
            return self.lose(reasons::TARGET_MISSING, kind, name, count);
        }
        let parent_spatial = self.spatial.contains(&parent);
        for child in children.items {
            if !self.file.contains(child) {
                self.lose(reasons::TARGET_MISSING, kind, name, 1);
            } else if !parent_spatial || !self.spatial.contains(&child) {
                self.lose(reasons::NOT_MAPPED, kind, name, 1);
            } else if !self.aggregated.insert((parent, child)) {
                self.lose(reasons::DUPLICATE_PLACEMENT, kind, name, 1);
            } else {
                self.children.entry(parent).or_default().push(child);
                self.child_set.insert(child);
            }
        }
    }

    /// `IfcRelContainedInSpatialStructure`: `RelatedElements` 4, `RelatingStructure` 5.
    fn containment(&mut self, name: u64, record: &Record) {
        let kind = record.type_name.as_str();
        let parts = (
            got_references(record.parameters.get(4)),
            got_reference(record.parameters.get(5)),
        );
        let (Ok(related), Ok(Some(structure))) = parts else {
            return self.bad(name, kind);
        };
        self.note_list(&related, name, kind);
        if !self.file.contains(structure) {
            let count = to_u64(related.items.len());
            return self.lose(reasons::TARGET_MISSING, kind, name, count);
        }
        let spatial = self.spatial.contains(&structure);
        for element in related.items {
            if !self.file.contains(element) {
                self.lose(reasons::TARGET_MISSING, kind, name, 1);
            } else if !spatial {
                self.lose(reasons::NOT_MAPPED, kind, name, 1);
            } else if !self.contained_seen.insert((structure, element)) {
                self.lose(reasons::DUPLICATE_PLACEMENT, kind, name, 1);
            } else {
                self.contained.entry(structure).or_default().push(element);
            }
        }
    }

    /// `IfcRelDefinesByType`: `RelatedObjects` 4, `RelatingType` 5.
    fn typing(&mut self, name: u64, record: &Record) {
        let kind = record.type_name.as_str();
        let parts = (
            got_references(record.parameters.get(4)),
            got_reference(record.parameters.get(5)),
        );
        let (Ok(related), Ok(Some(element_type))) = parts else {
            return self.bad(name, kind);
        };
        self.note_list(&related, name, kind);
        if !self.file.contains(element_type) {
            let count = to_u64(related.items.len());
            return self.lose(reasons::TARGET_MISSING, kind, name, count);
        }
        for element in related.items {
            if !self.file.contains(element) {
                self.lose(reasons::TARGET_MISSING, kind, name, 1);
            } else if !self.typed_seen.insert((element, element_type)) {
                self.lose(reasons::DUPLICATE_PLACEMENT, kind, name, 1);
            } else {
                self.typed.push((element, element_type));
            }
        }
    }

    /// `IfcRelDefinesByProperties`: `RelatedObjects` 4, `RelatingPropertyDefinition` 5, which may
    /// be `IFCPROPERTYSETDEFINITIONSET((#a,#b))` in IFC4.
    fn definitions_of(&mut self, name: u64, record: &Record) {
        let kind = record.type_name.as_str();
        let parts = (
            got_references(record.parameters.get(4)),
            got_definitions(record.parameters.get(5)),
        );
        let (Ok(related), Ok(definitions)) = parts else {
            return self.bad(name, kind);
        };
        self.note_list(&related, name, kind);
        self.note_list(&definitions, name, kind);
        for definition in definitions.items {
            if !self.objects.contains_key(&definition) {
                self.order.push(definition);
                self.objects.insert(definition, Vec::new());
            }
            let mut fresh = Vec::new();
            for &object in &related.items {
                if !self.file.contains(object) {
                    self.lose(reasons::TARGET_MISSING, kind, name, 1);
                } else if !self.object_seen.insert((definition, object)) {
                    self.lose(reasons::DUPLICATE_PLACEMENT, kind, name, 1);
                } else {
                    fresh.push(object);
                }
            }
            if let Some(list) = self.objects.get_mut(&definition) {
                list.extend(fresh);
            }
        }
    }

    /// The spatial tree from the roots, in file order. A spatial element that is never placed lies
    /// on a cycle or below one and is reported as cut.
    fn build_spatial(&mut self) -> Result<Vec<SpatialNode>> {
        let file = self.file;
        let roots: Vec<u64> = file
            .instances()
            .iter()
            .map(|i| i.name)
            .filter(|n| self.spatial.contains(n) && !self.child_set.contains(n))
            .collect();
        let mut nodes = Vec::new();
        for root in roots {
            self.placed.insert(root);
            nodes.push(self.node(root, 1)?);
        }
        let unplaced: Vec<u64> = self
            .spatial
            .iter()
            .copied()
            .filter(|n| !self.placed.contains(n))
            .collect();
        for name in unplaced {
            self.lose(reasons::CYCLE_CUT, "IFCRELAGGREGATES", name, 1);
        }
        Ok(nodes)
    }

    /// One node of the spatial tree. A child on the path from the root is a cycle; a child placed
    /// before under another parent is a duplicate placement.
    fn node(&mut self, name: u64, depth: u32) -> Result<SpatialNode> {
        self.cancel.check()?;
        if depth > self.depth_limit {
            return Err(limit("nesting_depth", u64::from(depth)));
        }
        let entity = self
            .entity(name)?
            .ok_or_else(|| Error::new(codes::IFC_STRUCTURE_INVALID))?;
        self.fixed(NODE_BYTES)?;
        self.path.insert(name);
        let mut children = Vec::new();
        for child in self.children.get(&name).cloned().unwrap_or_default() {
            self.cancel.check()?;
            if self.path.contains(&child) {
                self.lose(reasons::CYCLE_CUT, "IFCRELAGGREGATES", name, 1);
            } else if self.placed.contains(&child) {
                self.lose(reasons::DUPLICATE_PLACEMENT, "IFCRELAGGREGATES", name, 1);
            } else {
                self.placed.insert(child);
                children.push(self.node(child, depth.saturating_add(1))?);
            }
        }
        self.path.remove(&name);
        let mut contained = Vec::new();
        for element in self.contained.get(&name).cloned().unwrap_or_default() {
            if let Some(entity) = self.entity(element)? {
                self.link()?;
                contained.push(entity);
            }
        }
        Ok(SpatialNode {
            entity,
            children,
            contained,
        })
    }

    /// The rooted entity of an instance, built once and shared; `None` when the instance is absent.
    fn entity(&mut self, name: u64) -> Result<Option<Arc<RootedEntity>>> {
        if let Some(found) = self.entities.get(&name) {
            return Ok(Some(Arc::clone(found)));
        }
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            return Ok(None);
        };
        let Some(record) = instance.records.first() else {
            return Ok(None);
        };
        self.cancel.check()?;
        let parameters = &record.parameters;
        let base = ENTITY_BYTES.saturating_add(to_u64(record.type_name.len()));
        let bytes = [0usize, 2, 3].iter().fold(base, |sum, &i| {
            sum.saturating_add(copied_bytes(parameters.get(i)))
        });
        self.charge(bytes)?;
        let kind = record.type_name.as_str();
        let global_id = self.opt(got_text(parameters.first()), name, kind);
        let label = self.opt(got_text(parameters.get(2)), name, kind);
        let description = self.opt(got_text(parameters.get(3)), name, kind);
        let entity = Arc::new(RootedEntity {
            instance: name,
            type_name: record.type_name.clone(),
            global_id,
            name: label,
            description,
        });
        self.entities.insert(name, Arc::clone(&entity));
        Ok(Some(entity))
    }

    /// The shared entities of a list of objects, in order.
    fn related(&mut self, objects: &[u64]) -> Result<Vec<Arc<RootedEntity>>> {
        let mut out = Vec::new();
        for &object in objects {
            if let Some(entity) = self.entity(object)? {
                self.link()?;
                out.push(entity);
            }
        }
        Ok(out)
    }

    fn type_assignments(&mut self) -> Result<Vec<TypeAssignment>> {
        let pairs = std::mem::take(&mut self.typed);
        let mut out = Vec::new();
        for (element, element_type) in pairs {
            self.cancel.check()?;
            let (Some(element), Some(element_type)) =
                (self.entity(element)?, self.entity(element_type)?)
            else {
                continue;
            };
            self.fixed(ASSIGNMENT_BYTES)?;
            out.push(TypeAssignment {
                element,
                element_type,
            });
        }
        Ok(out)
    }

    fn property_sets(&mut self) -> Result<Vec<PropertySet>> {
        let file = self.file;
        let mut out = Vec::new();
        for definition in self.order.clone() {
            self.cancel.check()?;
            let objects = self.objects.get(&definition).cloned().unwrap_or_default();
            let Some(instance) = file.instance(definition) else {
                self.lose(
                    reasons::TARGET_MISSING,
                    "IFCRELDEFINESBYPROPERTIES",
                    definition,
                    1,
                );
                continue;
            };
            if let Some(record) = first_of(instance, "IFCPROPERTYSET") {
                out.extend(self.property_set(definition, record, &objects)?);
            } else if !has_type(instance, "IFCELEMENTQUANTITY") {
                self.lose(
                    reasons::TARGET_MISSING,
                    "IFCRELDEFINESBYPROPERTIES",
                    definition,
                    1,
                );
            }
        }
        Ok(out)
    }

    fn property_set(
        &mut self,
        definition: u64,
        record: &Record,
        objects: &[u64],
    ) -> Result<Option<PropertySet>> {
        let Some(head) = self.entity(definition)? else {
            return Ok(None);
        };
        let names = self.references(record.parameters.get(4), definition, "IFCPROPERTYSET");
        let mut properties = Vec::new();
        for name in names.items {
            if let Some(property) = self.property(name, definition)? {
                self.link()?;
                properties.push(property);
            }
        }
        let related = self.related(objects)?;
        self.fixed(SET_BYTES)?;
        Ok(Some(PropertySet {
            definition: head,
            properties,
            related,
        }))
    }

    /// A property of a set, built once per instance; `None` when it is missing or not read.
    fn property(&mut self, name: u64, owner: u64) -> Result<Option<Arc<Property>>> {
        if let Some(found) = self.properties.get(&name) {
            return Ok(Some(Arc::clone(found)));
        }
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            self.lose(reasons::TARGET_MISSING, "IFCPROPERTYSET", owner, 1);
            return Ok(None);
        };
        let [record] = instance.records.as_slice() else {
            let kind = instance
                .records
                .first()
                .map_or("", |r| r.type_name.as_str());
            self.lose(reasons::NOT_MAPPED, kind, name, 1);
            return Ok(None);
        };
        let kind = record.type_name.as_str();
        if !PROPERTY_TYPES.contains(&kind) {
            if kind.starts_with("IFCPROPERTY") {
                self.lose(reasons::NOT_MAPPED, kind, name, 1);
            } else {
                self.lose(reasons::TARGET_MISSING, kind, owner, 1);
            }
            return Ok(None);
        }
        self.cancel.check()?;
        let parameters = &record.parameters;
        let bytes = (0..6).fold(PROPERTY_BYTES, |sum, i| {
            sum.saturating_add(copied_bytes(parameters.get(i)))
        });
        self.fixed(bytes)?;
        let label = self.opt(got_text(parameters.first()), name, kind);
        let description = self.opt(got_text(parameters.get(1)), name, kind);
        let value = self.property_value(kind, record, name);
        let property = Arc::new(Property {
            instance: name,
            name: label,
            description,
            value,
        });
        self.properties.insert(name, Arc::clone(&property));
        Ok(Some(property))
    }

    fn property_value(&mut self, kind: &str, record: &Record, source: u64) -> PropertyValue {
        let p = |i: usize| record.parameters.get(i);
        match kind {
            "IFCPROPERTYSINGLEVALUE" => PropertyValue::Single {
                nominal: self.opt(got_nominal(p(2)), source, kind),
                unit: self.opt(got_reference(p(3)), source, kind),
            },
            "IFCPROPERTYENUMERATEDVALUE" => {
                let values = self.nominals(p(2), source, kind);
                PropertyValue::Enumerated {
                    values,
                    reference: self.opt(got_reference(p(3)), source, kind),
                }
            }
            "IFCPROPERTYLISTVALUE" => {
                let values = self.nominals(p(2), source, kind);
                PropertyValue::List {
                    values,
                    unit: self.opt(got_reference(p(3)), source, kind),
                }
            }
            _ => PropertyValue::Bounded {
                upper: self.opt(got_nominal(p(2)), source, kind),
                lower: self.opt(got_nominal(p(3)), source, kind),
                unit: self.opt(got_reference(p(4)), source, kind),
                set_point: self.opt(got_nominal(p(5)), source, kind),
            },
        }
    }

    /// A list of nominal values; an unusable entry is dropped and counted, the others are kept.
    fn nominals(&mut self, parameter: Option<&Parameter>, source: u64, kind: &str) -> Vec<Nominal> {
        match got_nominals(parameter) {
            Ok(listed) => {
                self.lose(reasons::VALUE_UNEXPECTED, kind, source, listed.bad);
                listed.items
            }
            Err(Bad) => {
                self.bad(source, kind);
                Vec::new()
            }
        }
    }

    fn quantity_sets(&mut self) -> Result<Vec<QuantitySet>> {
        let file = self.file;
        let mut out = Vec::new();
        for definition in self.order.clone() {
            self.cancel.check()?;
            let objects = self.objects.get(&definition).cloned().unwrap_or_default();
            let Some(instance) = file.instance(definition) else {
                continue;
            };
            if let Some(record) = first_of(instance, "IFCELEMENTQUANTITY") {
                out.extend(self.quantity_set(definition, record, &objects)?);
            }
        }
        Ok(out)
    }

    fn quantity_set(
        &mut self,
        definition: u64,
        record: &Record,
        objects: &[u64],
    ) -> Result<Option<QuantitySet>> {
        let Some(head) = self.entity(definition)? else {
            return Ok(None);
        };
        let names = self.references(record.parameters.get(5), definition, "IFCELEMENTQUANTITY");
        let mut quantities = Vec::new();
        for name in names.items {
            if let Some(quantity) = self.quantity(name, definition)? {
                self.link()?;
                quantities.push(quantity);
            }
        }
        let related = self.related(objects)?;
        self.fixed(SET_BYTES)?;
        Ok(Some(QuantitySet {
            definition: head,
            quantities,
            related,
        }))
    }

    /// A quantity of a set, built once per instance; `None` when it is missing or not read.
    fn quantity(&mut self, name: u64, owner: u64) -> Result<Option<Arc<Quantity>>> {
        if let Some(found) = self.quantities.get(&name) {
            return Ok(Some(Arc::clone(found)));
        }
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            self.lose(reasons::TARGET_MISSING, "IFCELEMENTQUANTITY", owner, 1);
            return Ok(None);
        };
        let [record] = instance.records.as_slice() else {
            let kind = instance
                .records
                .first()
                .map_or("", |r| r.type_name.as_str());
            self.lose(reasons::NOT_MAPPED, kind, name, 1);
            return Ok(None);
        };
        let kind = record.type_name.as_str();
        let Some(quantity_kind) = quantity_kind(kind) else {
            if kind.starts_with("IFCQUANTITY") {
                self.lose(reasons::NOT_MAPPED, kind, name, 1);
            } else {
                self.lose(reasons::TARGET_MISSING, kind, owner, 1);
            }
            return Ok(None);
        };
        self.cancel.check()?;
        let parameters = &record.parameters;
        let bytes = (0..5).fold(QUANTITY_BYTES, |sum, i| {
            sum.saturating_add(copied_bytes(parameters.get(i)))
        });
        self.fixed(bytes)?;
        let label = self.opt(got_text(parameters.first()), name, kind);
        let description = self.opt(got_text(parameters.get(1)), name, kind);
        let unit = self.opt(got_reference(parameters.get(2)), name, kind);
        let value = self.opt(got_number(parameters.get(3)), name, kind);
        let formula = self.opt(got_text(parameters.get(4)), name, kind);
        let quantity = Arc::new(Quantity {
            instance: name,
            kind: quantity_kind,
            name: label,
            description,
            unit,
            value,
            formula,
        });
        self.quantities.insert(name, Arc::clone(&quantity));
        Ok(Some(quantity))
    }
}

/// The quantity kind of an IFC quantity type.
fn quantity_kind(type_name: &str) -> Option<QuantityKind> {
    Some(match type_name {
        "IFCQUANTITYLENGTH" => QuantityKind::Length,
        "IFCQUANTITYAREA" => QuantityKind::Area,
        "IFCQUANTITYVOLUME" => QuantityKind::Volume,
        "IFCQUANTITYCOUNT" => QuantityKind::Count,
        "IFCQUANTITYWEIGHT" => QuantityKind::Weight,
        "IFCQUANTITYTIME" => QuantityKind::Time,
        _ => return None,
    })
}

fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn has_type(instance: &Instance, type_name: &str) -> bool {
    instance.records.iter().any(|r| r.type_name == type_name)
}

fn first_of<'a>(instance: &'a Instance, type_name: &str) -> Option<&'a Record> {
    instance.records.iter().find(|r| r.type_name == type_name)
}

/// The bytes a view allocates for a parameter it copies: its strings, type names and values.
fn copied_bytes(parameter: Option<&Parameter>) -> u64 {
    fn walk(parameter: &Parameter) -> u64 {
        match parameter {
            Parameter::Text(text) | Parameter::Enumeration(text) | Parameter::Binary(text) => {
                to_u64(text.len())
            }
            Parameter::Integer(_) | Parameter::Real(_) => NOMINAL_BYTES,
            Parameter::Typed { type_name, value } => NOMINAL_BYTES
                .saturating_add(to_u64(type_name.len()))
                .saturating_add(walk(value)),
            Parameter::List(items) => items
                .iter()
                .fold(0, |sum, item| sum.saturating_add(walk(item))),
            Parameter::Reference(_) | Parameter::Omitted | Parameter::Derived => 0,
        }
    }
    parameter.map_or(0, walk)
}

/// A text attribute: `None` for an omitted or derived value, the text when it is a string, possibly
/// wrapped in a type such as `IFCLABEL`.
fn got_text(parameter: Option<&Parameter>) -> Got<Option<String>> {
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => Ok(None),
        Some(Parameter::Text(text)) => Ok(Some(text.clone())),
        Some(Parameter::Typed { value, .. }) => match value.as_ref() {
            Parameter::Text(text) => Ok(Some(text.clone())),
            _ => Err(Bad),
        },
        Some(_) => Err(Bad),
    }
}

fn got_reference(parameter: Option<&Parameter>) -> Got<Option<u64>> {
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => Ok(None),
        Some(Parameter::Reference(name)) => Ok(Some(*name)),
        Some(_) => Err(Bad),
    }
}

/// A list of references. Repeated names are kept once; a non-reference entry is counted as bad.
fn got_references(parameter: Option<&Parameter>) -> Got<Listed<u64>> {
    let mut listed = Listed::empty();
    let mut seen = BTreeSet::new();
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => {}
        Some(Parameter::List(items)) => {
            for item in items {
                match item {
                    Parameter::Reference(name) => {
                        if seen.insert(*name) {
                            listed.items.push(*name);
                        } else {
                            listed.repeated = listed.repeated.saturating_add(1);
                        }
                    }
                    _ => listed.bad = listed.bad.saturating_add(1),
                }
            }
        }
        Some(_) => return Err(Bad),
    }
    Ok(listed)
}

/// The definitions of `IfcRelDefinesByProperties`: one reference, or the members of
/// `IFCPROPERTYSETDEFINITIONSET((#a,#b))`.
fn got_definitions(parameter: Option<&Parameter>) -> Got<Listed<u64>> {
    match parameter {
        Some(Parameter::Reference(name)) => Ok(Listed::single(*name)),
        Some(Parameter::Typed { type_name, value })
            if type_name == "IFCPROPERTYSETDEFINITIONSET" =>
        {
            got_references(Some(value.as_ref()))
        }
        _ => Err(Bad),
    }
}

fn got_value(parameter: &Parameter) -> Got<Value> {
    match parameter {
        Parameter::Text(text) => Ok(Value::Text(text.clone())),
        Parameter::Real(real) => Ok(Value::Real(*real)),
        Parameter::Integer(integer) => Ok(Value::Integer(*integer)),
        Parameter::Enumeration(name) => Ok(Value::Enumeration(name.clone())),
        _ => Err(Bad),
    }
}

fn got_nominal(parameter: Option<&Parameter>) -> Got<Option<Nominal>> {
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => Ok(None),
        Some(Parameter::Typed { type_name, value }) => Ok(Some(Nominal {
            type_name: Some(type_name.clone()),
            value: got_value(value)?,
        })),
        Some(bare) => Ok(Some(Nominal {
            type_name: None,
            value: got_value(bare)?,
        })),
    }
}

/// A list of nominal values; an entry that is not a value is counted as bad.
fn got_nominals(parameter: Option<&Parameter>) -> Got<Listed<Nominal>> {
    let mut listed = Listed::empty();
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => {}
        Some(Parameter::List(items)) => {
            for item in items {
                match got_nominal(Some(item)) {
                    Ok(Some(nominal)) => listed.items.push(nominal),
                    _ => listed.bad = listed.bad.saturating_add(1),
                }
            }
        }
        Some(_) => return Err(Bad),
    }
    Ok(listed)
}

/// A numeric value, possibly wrapped in a measure type such as `IFCLENGTHMEASURE`.
fn got_number(parameter: Option<&Parameter>) -> Got<Option<Number>> {
    let number = |value: &Parameter| match value {
        Parameter::Integer(integer) => Ok(Number::Integer(*integer)),
        Parameter::Real(real) => Ok(Number::Real(*real)),
        _ => Err(Bad),
    };
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => Ok(None),
        Some(Parameter::Typed { value, .. }) => number(value).map(Some),
        Some(bare) => number(bare).map(Some),
    }
}
