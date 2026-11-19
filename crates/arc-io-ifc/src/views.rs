//! Typed views over the generic model (CORE-IO-014): the spatial structure, the element types,
//! the property sets and the quantity sets.
//!
//! Attribute positions are those of IFC2X3, IFC4 and IFC4X3, which agree for every attribute the
//! views read: the rooted attributes `GlobalId`, `Name` and `Description` (positions 0, 2, 3), the
//! relationship attributes, and the value attributes of the property and quantity types. The
//! schema is therefore not consulted; the caller can read [`IfcFile::schema`] to know which schema
//! the values came from. A value of another kind than the view expects is not read and is counted
//! in the loss report as dropped.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Cancel, Error, Limits, Result};

use crate::error::{codes, limit, reasons};
use crate::loss::{LossHeader, LossLog, LossReport};
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
];

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
    pub entity: RootedEntity,
    /// The aggregated spatial children, in file order.
    pub children: Vec<SpatialNode>,
    /// The elements contained in this structure, in file order.
    pub contained: Vec<RootedEntity>,
}

/// An element and the type object it is defined by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAssignment {
    /// The element.
    pub element: RootedEntity,
    /// The type object.
    pub element_type: RootedEntity,
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
        /// The set point.
        set_point: Option<Nominal>,
    },
}

/// A property of a property set.
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
    pub definition: RootedEntity,
    /// The properties in file order.
    pub properties: Vec<Property>,
    /// The objects the set is assigned to, in file order.
    pub related: Vec<RootedEntity>,
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

/// A quantity of a quantity set.
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
    /// The `Formula`.
    pub formula: Option<String>,
}

/// A quantity set with the objects it is assigned to.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantitySet {
    /// The quantity set.
    pub definition: RootedEntity,
    /// The quantities in file order.
    pub quantities: Vec<Quantity>,
    /// The objects the set is assigned to, in file order.
    pub related: Vec<RootedEntity>,
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

impl IfcFile {
    /// Builds the typed views. Cancellation is checked for every instance; a spatial structure
    /// nested deeper than the limit fails with `io.limit_exceeded`.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` for the nesting depth of the spatial structure; `base.cancelled` when
    /// the token was cancelled.
    pub fn typed_views(&self, limits: &Limits, cancel: &Cancel) -> Result<TypedViews> {
        let mut reader = Reader::new(self, limits, cancel);
        reader.scan()?;
        let spatial = reader.build_spatial()?;
        let types = reader.type_assignments();
        let property_sets = reader.property_sets();
        let quantity_sets = reader.quantity_sets();
        let report = reader
            .log
            .finish(LossHeader::import(self.schema.identifier(), *limits));
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
    cancel: &'f Cancel,
    depth_limit: u32,
    log: LossLog,
    spatial: BTreeSet<u64>,
    children: BTreeMap<u64, Vec<u64>>,
    child_set: BTreeSet<u64>,
    contained: BTreeMap<u64, Vec<u64>>,
    type_pairs: Vec<(Vec<u64>, u64)>,
    definitions: Vec<(u64, Vec<u64>)>,
    definition_index: BTreeMap<u64, usize>,
    placed: BTreeSet<u64>,
}

impl<'f> Reader<'f> {
    fn new(file: &'f IfcFile, limits: &Limits, cancel: &'f Cancel) -> Self {
        Self {
            file,
            cancel,
            depth_limit: limits.nesting_depth.min(MAX_NESTING),
            log: LossLog::default(),
            spatial: BTreeSet::new(),
            children: BTreeMap::new(),
            child_set: BTreeSet::new(),
            contained: BTreeMap::new(),
            type_pairs: Vec::new(),
            definitions: Vec::new(),
            definition_index: BTreeMap::new(),
            placed: BTreeSet::new(),
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
        let kind = record.type_name.as_str();
        match kind {
            "IFCRELAGGREGATES" => {
                let parts = (
                    got_reference(record.parameters.get(4)),
                    got_references(record.parameters.get(5)),
                );
                let (Ok(Some(parent)), Ok(related)) = parts else {
                    return self.bad(name, kind);
                };
                if !self.spatial.contains(&parent) {
                    return;
                }
                for child in related.into_iter().filter(|c| self.spatial.contains(c)) {
                    self.children.entry(parent).or_default().push(child);
                    self.child_set.insert(child);
                }
            }
            "IFCRELCONTAINEDINSPATIALSTRUCTURE" => {
                let parts = (
                    got_references(record.parameters.get(4)),
                    got_reference(record.parameters.get(5)),
                );
                let (Ok(related), Ok(Some(structure))) = parts else {
                    return self.bad(name, kind);
                };
                if self.spatial.contains(&structure) {
                    self.contained.entry(structure).or_default().extend(related);
                }
            }
            "IFCRELDEFINESBYTYPE" => {
                let parts = (
                    got_references(record.parameters.get(4)),
                    got_reference(record.parameters.get(5)),
                );
                match parts {
                    (Ok(related), Ok(Some(element_type))) => {
                        self.type_pairs.push((related, element_type));
                    }
                    _ => self.bad(name, kind),
                }
            }
            "IFCRELDEFINESBYPROPERTIES" => {
                let parts = (
                    got_references(record.parameters.get(4)),
                    got_reference(record.parameters.get(5)),
                );
                match parts {
                    (Ok(related), Ok(Some(definition))) => {
                        let slot = match self.definition_index.get(&definition) {
                            Some(&slot) => slot,
                            None => {
                                self.definitions.push((definition, Vec::new()));
                                let slot = self.definitions.len() - 1;
                                self.definition_index.insert(definition, slot);
                                slot
                            }
                        };
                        if let Some((_, objects)) = self.definitions.get_mut(slot) {
                            objects.extend(related);
                        }
                    }
                    _ => self.bad(name, kind),
                }
            }
            _ => {}
        }
    }

    fn bad(&mut self, source: u64, kind: &str) {
        self.log.add(
            reasons::VALUE_UNEXPECTED,
            kind,
            Some(&source.to_string()),
            1,
        );
    }

    /// The spatial tree from the roots, in file order; every spatial element that is never placed
    /// lies on a cycle and is reported as cut.
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
            self.log.add(
                reasons::CYCLE_CUT,
                "IFCRELAGGREGATES",
                Some(&name.to_string()),
                1,
            );
        }
        Ok(nodes)
    }

    fn node(&mut self, name: u64, depth: u32) -> Result<SpatialNode> {
        self.cancel.check()?;
        if depth > self.depth_limit {
            return Err(limit("nesting_depth", u64::from(depth)));
        }
        let entity = self
            .rooted_of(name)
            .ok_or_else(|| Error::new(codes::IFC_STRUCTURE_INVALID))?;
        let mut children = Vec::new();
        for child in self.children.get(&name).cloned().unwrap_or_default() {
            if self.placed.contains(&child) {
                self.log.add(
                    reasons::CYCLE_CUT,
                    "IFCRELAGGREGATES",
                    Some(&name.to_string()),
                    1,
                );
                continue;
            }
            self.placed.insert(child);
            children.push(self.node(child, depth.saturating_add(1))?);
        }
        let contained = self
            .contained
            .get(&name)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|element| self.rooted_of(element))
            .collect();
        Ok(SpatialNode {
            entity,
            children,
            contained,
        })
    }

    fn type_assignments(&mut self) -> Vec<TypeAssignment> {
        let pairs = std::mem::take(&mut self.type_pairs);
        let mut out = Vec::new();
        for (elements, element_type) in pairs {
            let Some(element_type) = self.rooted_of(element_type) else {
                continue;
            };
            for element in elements {
                if let Some(element) = self.rooted_of(element) {
                    out.push(TypeAssignment {
                        element,
                        element_type: element_type.clone(),
                    });
                }
            }
        }
        out
    }

    fn property_sets(&mut self) -> Vec<PropertySet> {
        let mut out = Vec::new();
        for (definition, related) in self.definitions.clone() {
            let Some(instance) = self.file.instance(definition) else {
                continue;
            };
            if !has_type(instance, "IFCPROPERTYSET") {
                continue;
            }
            let Some(record) = first_of(instance, "IFCPROPERTYSET") else {
                continue;
            };
            let Some(head) = self.rooted_of(definition) else {
                continue;
            };
            let names = self.got(got_references(record.parameters.get(4)), definition, record);
            let properties = names
                .unwrap_or_default()
                .into_iter()
                .filter_map(|p| self.property(p))
                .collect();
            let related = related
                .into_iter()
                .filter_map(|o| self.rooted_of(o))
                .collect();
            out.push(PropertySet {
                definition: head,
                properties,
                related,
            });
        }
        out
    }

    fn property(&mut self, name: u64) -> Option<Property> {
        let instance = self.file.instance(name)?;
        let record = instance.records.iter().find(|r| {
            matches!(
                r.type_name.as_str(),
                "IFCPROPERTYSINGLEVALUE"
                    | "IFCPROPERTYENUMERATEDVALUE"
                    | "IFCPROPERTYLISTVALUE"
                    | "IFCPROPERTYBOUNDEDVALUE"
            )
        })?;
        let param = |i: usize| record.parameters.get(i);
        let text = |i: usize| got_text(param(i));
        let label = self.opt(text(0), name, record);
        let description = self.opt(text(1), name, record);
        let value = match record.type_name.as_str() {
            "IFCPROPERTYSINGLEVALUE" => PropertyValue::Single {
                nominal: self.opt(got_nominal(param(2)), name, record),
                unit: self.opt(got_reference(param(3)), name, record),
            },
            "IFCPROPERTYENUMERATEDVALUE" => PropertyValue::Enumerated {
                values: self
                    .got(got_nominals(param(2)), name, record)
                    .unwrap_or_default(),
                reference: self.opt(got_reference(param(3)), name, record),
            },
            "IFCPROPERTYLISTVALUE" => PropertyValue::List {
                values: self
                    .got(got_nominals(param(2)), name, record)
                    .unwrap_or_default(),
                unit: self.opt(got_reference(param(3)), name, record),
            },
            _ => PropertyValue::Bounded {
                upper: self.opt(got_nominal(param(2)), name, record),
                lower: self.opt(got_nominal(param(3)), name, record),
                unit: self.opt(got_reference(param(4)), name, record),
                set_point: self.opt(got_nominal(param(5)), name, record),
            },
        };
        Some(Property {
            instance: name,
            name: label,
            description,
            value,
        })
    }

    fn quantity_sets(&mut self) -> Vec<QuantitySet> {
        let mut out = Vec::new();
        for (definition, related) in self.definitions.clone() {
            let Some(instance) = self.file.instance(definition) else {
                continue;
            };
            let Some(record) = first_of(instance, "IFCELEMENTQUANTITY") else {
                continue;
            };
            let Some(head) = self.rooted_of(definition) else {
                continue;
            };
            let names = self.got(got_references(record.parameters.get(5)), definition, record);
            let quantities = names
                .unwrap_or_default()
                .into_iter()
                .filter_map(|q| self.quantity(q))
                .collect();
            let related = related
                .into_iter()
                .filter_map(|o| self.rooted_of(o))
                .collect();
            out.push(QuantitySet {
                definition: head,
                quantities,
                related,
            });
        }
        out
    }

    fn quantity(&mut self, name: u64) -> Option<Quantity> {
        let instance = self.file.instance(name)?;
        let record = instance.records.iter().find(|r| {
            matches!(
                r.type_name.as_str(),
                "IFCQUANTITYLENGTH"
                    | "IFCQUANTITYAREA"
                    | "IFCQUANTITYVOLUME"
                    | "IFCQUANTITYCOUNT"
                    | "IFCQUANTITYWEIGHT"
                    | "IFCQUANTITYTIME"
            )
        })?;
        let kind = match record.type_name.as_str() {
            "IFCQUANTITYLENGTH" => QuantityKind::Length,
            "IFCQUANTITYAREA" => QuantityKind::Area,
            "IFCQUANTITYVOLUME" => QuantityKind::Volume,
            "IFCQUANTITYCOUNT" => QuantityKind::Count,
            "IFCQUANTITYWEIGHT" => QuantityKind::Weight,
            _ => QuantityKind::Time,
        };
        let param = |i: usize| record.parameters.get(i);
        let label = self.opt(got_text(param(0)), name, record);
        let description = self.opt(got_text(param(1)), name, record);
        let unit = self.opt(got_reference(param(2)), name, record);
        let value = self.opt(got_number(param(3)), name, record);
        let formula = self.opt(got_text(param(4)), name, record);
        Some(Quantity {
            instance: name,
            kind,
            name: label,
            description,
            unit,
            value,
            formula,
        })
    }

    /// The value of an attribute, or `None` with a dropped item in the report when it is of the
    /// wrong kind.
    fn got<T>(&mut self, value: Got<T>, source: u64, record: &Record) -> Option<T> {
        match value {
            Ok(value) => Some(value),
            Err(Bad) => {
                self.bad(source, &record.type_name);
                None
            }
        }
    }

    /// The value of an optional attribute, or `None` when it is absent or of the wrong kind.
    fn opt<T>(&mut self, value: Got<Option<T>>, source: u64, record: &Record) -> Option<T> {
        self.got(value, source, record).flatten()
    }

    fn rooted_of(&mut self, name: u64) -> Option<RootedEntity> {
        let file = self.file;
        let instance = file.instance(name)?;
        let record = instance.records.first()?;
        Some(self.rooted(name, record))
    }

    fn rooted(&mut self, name: u64, record: &Record) -> RootedEntity {
        let global_id = self.opt(got_text(record.parameters.first()), name, record);
        let label = self.opt(got_text(record.parameters.get(2)), name, record);
        let description = self.opt(got_text(record.parameters.get(3)), name, record);
        RootedEntity {
            instance: name,
            type_name: record.type_name.clone(),
            global_id,
            name: label,
            description,
        }
    }
}

fn has_type(instance: &Instance, type_name: &str) -> bool {
    instance.records.iter().any(|r| r.type_name == type_name)
}

fn first_of<'a>(instance: &'a Instance, type_name: &str) -> Option<&'a Record> {
    instance.records.iter().find(|r| r.type_name == type_name)
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

fn got_references(parameter: Option<&Parameter>) -> Got<Vec<u64>> {
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => Ok(Vec::new()),
        Some(Parameter::List(items)) => items
            .iter()
            .map(|item| match item {
                Parameter::Reference(name) => Ok(*name),
                _ => Err(Bad),
            })
            .collect(),
        Some(_) => Err(Bad),
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

fn got_nominals(parameter: Option<&Parameter>) -> Got<Vec<Nominal>> {
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => Ok(Vec::new()),
        Some(Parameter::List(items)) => items
            .iter()
            .map(|item| got_nominal(Some(item)).and_then(|n| n.ok_or(Bad)))
            .collect(),
        Some(_) => Err(Bad),
    }
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
