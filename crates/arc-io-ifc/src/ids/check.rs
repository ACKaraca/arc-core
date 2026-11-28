//! Checking a file against an IDS document.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Cancel, Error, Limits, Result};

use super::Report;
use super::model::{
    AttributeFacet, Cardinality, ClassificationFacet, EntityFacet, Facet, FacetCardinality, Ids,
    MaterialFacet, PartOfFacet, PropertyFacet, Specification, Value,
};
use super::value::{self, Val, has_range, name_matches};
use crate::error::codes;
use crate::model::{IfcFile, Parameter, Record};
use crate::schema::SchemaTables;

/// The result of one specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The specification is satisfied.
    Pass,
    /// It is not.
    Fail,
}

/// A requirement an instance does not satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The instance name.
    pub instance: u64,
    /// The index of the requirement in the specification.
    pub requirement: usize,
    /// The facet kind (`attribute`, `property`, ...).
    pub facet: &'static str,
    /// Why the instance fails.
    pub reason: String,
}

/// The result of checking one specification.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecificationResult {
    /// The name of the specification.
    pub name: String,
    /// Its identifier.
    pub identifier: Option<String>,
    /// Whether it passed.
    pub status: Status,
    /// The number of applicable instances.
    pub applicable: usize,
    /// The names of the applicable instances, up to `array_length`.
    pub applicable_instances: Vec<u64>,
    /// The failing instances with the requirement and the reason, up to `array_length`.
    pub failures: Vec<Failure>,
    /// The number of failures, which may exceed `failures.len()`.
    pub failure_count: usize,
    /// A reason that does not belong to an instance: no applicable instance for a required
    /// specification, or applicable instances of a prohibited one.
    pub reason: Option<String>,
}

/// The outcome of one facet on one instance.
struct Eval {
    /// The described data exists on the instance.
    exists: bool,
    /// It satisfies the facet.
    passes: bool,
    reason: String,
}

impl Eval {
    fn new(exists: bool, passes: bool, reason: impl Into<String>) -> Self {
        Self {
            exists,
            passes,
            reason: reason.into(),
        }
    }
}

/// The values of a property with their data types, and whether they are the bounds of a range.
type TypedValues = (Vec<(Val, Option<String>)>, bool);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Relation {
    Aggregates,
    Group,
    Contained,
    Nests,
    Opening,
}

/// Relationships of a file, read once.
struct Index {
    type_of: BTreeMap<u64, u64>,
    defined_by: BTreeMap<u64, Vec<u64>>,
    materials: BTreeMap<u64, Vec<u64>>,
    classifications: BTreeMap<u64, Vec<u64>>,
    material_props: BTreeMap<u64, Vec<u64>>,
    up: BTreeMap<u64, Vec<(Relation, u64)>>,
}

struct Ctx<'f> {
    file: &'f IfcFile,
    tables: &'static SchemaTables,
    index: Index,
    /// The factor to SI units of each unit type of the project.
    units: BTreeMap<String, f64>,
    cancel: &'f Cancel,
    limits: Limits,
}

fn refs(parameter: Option<&Parameter>) -> Vec<u64> {
    match parameter {
        Some(Parameter::List(items)) => items
            .iter()
            .filter_map(|i| {
                if let Parameter::Reference(n) = i {
                    Some(*n)
                } else {
                    None
                }
            })
            .collect(),
        Some(Parameter::Reference(n)) => vec![*n],
        _ => Vec::new(),
    }
}

fn reference(parameter: Option<&Parameter>) -> Option<u64> {
    match parameter {
        Some(Parameter::Reference(n)) => Some(*n),
        _ => None,
    }
}

impl<'f> Ctx<'f> {
    fn new(file: &'f IfcFile, limits: &Limits, cancel: &'f Cancel) -> Result<Self> {
        let tables = SchemaTables::of(&file.schema)
            .ok_or_else(|| Error::new(codes::IDS_SCHEMA_UNAVAILABLE))?;
        let mut ctx = Self {
            file,
            tables,
            index: Index {
                type_of: BTreeMap::new(),
                defined_by: BTreeMap::new(),
                materials: BTreeMap::new(),
                classifications: BTreeMap::new(),
                material_props: BTreeMap::new(),
                up: BTreeMap::new(),
            },
            units: BTreeMap::new(),
            cancel,
            limits: *limits,
        };
        ctx.build_index()?;
        ctx.read_units();
        Ok(ctx)
    }

    fn attr(&self, record: &'f Record, name: &str) -> Option<&'f Parameter> {
        self.file.attribute(record, name)
    }

    fn record(&self, name: u64) -> Option<&'f Record> {
        self.file.instance(name).and_then(|i| i.records.first())
    }

    /// The relations that make an instance part of another: instance to `(relation, whole)`.
    fn part_of_edges(&self) -> Result<BTreeMap<u64, Vec<(Relation, u64)>>> {
        let file = self.file;
        let each = |relation: &str, f: &mut dyn FnMut(&'f Record)| -> Result<()> {
            for instance in file.instances_of(relation) {
                self.cancel.check()?;
                if let Some(record) = instance.records.first() {
                    f(record);
                }
            }
            Ok(())
        };
        let mut up: BTreeMap<u64, Vec<(Relation, u64)>> = BTreeMap::new();
        let mut edge = |part: u64, kind: Relation, whole: u64| {
            up.entry(part).or_default().push((kind, whole));
        };
        each("IFCRELAGGREGATES", &mut |r| {
            if let Some(w) = reference(file.attribute(r, "RelatingObject")) {
                for p in refs(file.attribute(r, "RelatedObjects")) {
                    edge(p, Relation::Aggregates, w);
                }
            }
        })?;
        each("IFCRELNESTS", &mut |r| {
            if let Some(w) = reference(file.attribute(r, "RelatingObject")) {
                for p in refs(file.attribute(r, "RelatedObjects")) {
                    edge(p, Relation::Nests, w);
                }
            }
        })?;
        each("IFCRELCONTAINEDINSPATIALSTRUCTURE", &mut |r| {
            if let Some(w) = reference(file.attribute(r, "RelatingStructure")) {
                for p in refs(file.attribute(r, "RelatedElements")) {
                    edge(p, Relation::Contained, w);
                }
            }
        })?;
        each("IFCRELASSIGNSTOGROUP", &mut |r| {
            if let Some(w) = reference(file.attribute(r, "RelatingGroup")) {
                for p in refs(file.attribute(r, "RelatedObjects")) {
                    edge(p, Relation::Group, w);
                }
            }
        })?;
        each("IFCRELVOIDSELEMENT", &mut |r| {
            if let (Some(w), Some(p)) = (
                reference(file.attribute(r, "RelatingBuildingElement")),
                reference(file.attribute(r, "RelatedOpeningElement")),
            ) {
                edge(p, Relation::Opening, w);
            }
        })?;
        each("IFCRELFILLSELEMENT", &mut |r| {
            if let (Some(w), Some(p)) = (
                reference(file.attribute(r, "RelatingOpeningElement")),
                reference(file.attribute(r, "RelatedBuildingElement")),
            ) {
                edge(p, Relation::Opening, w);
            }
        })?;
        Ok(up)
    }

    fn build_index(&mut self) -> Result<()> {
        let file = self.file;
        let each = |relation: &str, f: &mut dyn FnMut(&'f Record)| -> Result<()> {
            for instance in file.instances_of(relation) {
                self.cancel.check()?;
                if let Some(record) = instance.records.first() {
                    f(record);
                }
            }
            Ok(())
        };
        let mut type_of = BTreeMap::new();
        each("IFCRELDEFINESBYTYPE", &mut |r| {
            if let Some(t) = reference(file.attribute(r, "RelatingType")) {
                for o in refs(file.attribute(r, "RelatedObjects")) {
                    type_of.insert(o, t);
                }
            }
        })?;
        let mut defined_by: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        each("IFCRELDEFINESBYPROPERTIES", &mut |r| {
            let defs = refs(file.attribute(r, "RelatingPropertyDefinition"));
            for o in refs(file.attribute(r, "RelatedObjects")) {
                defined_by
                    .entry(o)
                    .or_default()
                    .extend(defs.iter().copied());
            }
        })?;
        let mut materials: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        each("IFCRELASSOCIATESMATERIAL", &mut |r| {
            let m = refs(file.attribute(r, "RelatingMaterial"));
            for o in refs(file.attribute(r, "RelatedObjects")) {
                materials.entry(o).or_default().extend(m.iter().copied());
            }
        })?;
        let mut classifications: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        each("IFCRELASSOCIATESCLASSIFICATION", &mut |r| {
            let c = refs(file.attribute(r, "RelatingClassification"));
            for o in refs(file.attribute(r, "RelatedObjects")) {
                classifications
                    .entry(o)
                    .or_default()
                    .extend(c.iter().copied());
            }
        })?;
        each("IFCEXTERNALREFERENCERELATIONSHIP", &mut |r| {
            if let Some(c) = reference(file.attribute(r, "RelatingReference")) {
                for o in refs(file.attribute(r, "RelatedResourceObjects")) {
                    classifications.entry(o).or_default().push(c);
                }
            }
        })?;
        let up = self.part_of_edges()?;
        let mut material_props: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for kind in ["IFCMATERIALPROPERTIES", "IFCEXTENDEDMATERIALPROPERTIES"] {
            for instance in file.instances_of(kind) {
                if let Some(record) = instance.records.first()
                    && let Some(m) = reference(file.attribute(record, "Material"))
                {
                    material_props.entry(m).or_default().push(instance.name);
                }
            }
        }
        self.index = Index {
            type_of,
            defined_by,
            materials,
            classifications,
            material_props,
            up,
        };
        Ok(())
    }

    /// The factor to SI of each unit type of the project's unit assignment.
    fn read_units(&mut self) {
        let file = self.file;
        let Some(project) = file
            .instances_of("IFCPROJECT")
            .next()
            .and_then(|i| i.records.first())
        else {
            return;
        };
        let Some(assignment) = reference(self.attr(project, "UnitsInContext")) else {
            return;
        };
        let Some(record) = self.record(assignment) else {
            return;
        };
        for unit in refs(self.attr(record, "Units")) {
            let Some(u) = self.record(unit) else { continue };
            let unit_type = match self.attr(u, "UnitType") {
                Some(Parameter::Enumeration(t)) => t.clone(),
                _ => continue,
            };
            if let Some(f) = self.unit_factor(u) {
                self.units.insert(unit_type, f);
            }
        }
    }

    fn unit_factor(&self, record: &'f Record) -> Option<f64> {
        match record.type_name.as_str() {
            "IFCSIUNIT" => Some(match self.attr(record, "Prefix") {
                Some(Parameter::Enumeration(p)) => prefix(p)?,
                _ => 1.0,
            }),
            "IFCCONVERSIONBASEDUNIT" => {
                let measure = self.record(reference(self.attr(record, "ConversionFactor"))?)?;
                let value = match self.attr(measure, "ValueComponent")? {
                    Parameter::Typed { value, .. } => match value.as_ref() {
                        Parameter::Real(r) => *r,
                        Parameter::Integer(i) => *i as f64,
                        _ => return None,
                    },
                    Parameter::Real(r) => *r,
                    _ => return None,
                };
                let base = self.record(reference(self.attr(measure, "UnitComponent"))?)?;
                Some(value * self.unit_factor(base)?)
            }
            _ => None,
        }
    }

    // ---------------------------------------------------------------- schema helpers

    /// The index of the entity of an instance in the tables.
    fn entity_of(&self, record: &Record) -> Option<u16> {
        self.tables.entity_index(&record.type_name)
    }

    /// The explicit attributes of a record's entity: `(name, position, derived)`.
    fn attributes_of(&self, record: &Record) -> Vec<(&'static str, usize, bool)> {
        self.entity_of(record)
            .map(|e| {
                self.tables
                    .positions(e)
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (p.attribute.name, i, p.derived))
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- entity

    fn class_matches(&self, facet: &EntityFacet, name: u64) -> bool {
        let Some(record) = self.record(name) else {
            return false;
        };
        let class_ok = match self.occurrence_of(&facet.name) {
            Some((occurrence, type_class)) => {
                record.type_name == occurrence
                    && self
                        .index
                        .type_of
                        .get(&name)
                        .and_then(|&t| self.record(t))
                        .is_some_and(|t| t.type_name == type_class)
            }
            None => name_matches(&facet.name, &record.type_name),
        };
        if !class_ok {
            return false;
        }
        match &facet.predefined_type {
            None => true,
            Some(want) => [self.predefined_type(name), self.predefined_item(name)]
                .into_iter()
                .flatten()
                .any(|have| value::matches(want, &Val::Str(have))),
        }
    }

    /// IFC2X3 has no class for some products, only a type class: the product is an instance of the
    /// supertype of the type class without the `Type` suffix, defined by that type. For a name
    /// that IFC2X3 does not declare and whose `...Type` class it does, the occurrence class and
    /// the type class.
    fn occurrence_of(&self, name: &Value) -> Option<(String, String)> {
        let Value::Simple(n) = name else { return None };
        if self.file.schema != crate::model::Schema::Ifc2x3 || self.tables.entity_index(n).is_some()
        {
            return None;
        }
        let type_class = format!("{n}TYPE");
        let type_entity = self.tables.entity(&type_class)?;
        let parent = self
            .tables
            .entities
            .get(usize::from(type_entity.supertype?))?;
        let occurrence = parent.name.to_ascii_uppercase();
        occurrence
            .strip_suffix("TYPE")
            .map(|o| (o.to_owned(), type_class))
    }

    /// The predefined type of an instance: from its type object when that has one, else its own.
    fn predefined_type(&self, name: u64) -> Option<String> {
        if let Some(&t) = self.index.type_of.get(&name)
            && let Some(p) = self.predefined_of(t)
            && p != "NOTDEFINED"
        {
            return Some(p);
        }
        self.predefined_of(name)
    }

    /// The raw `PredefinedType` item of an instance or its type.
    fn predefined_item(&self, name: u64) -> Option<String> {
        let own = |n: u64| match self.record(n).and_then(|r| self.attr(r, "PredefinedType")) {
            Some(Parameter::Enumeration(p)) => Some(p.clone()),
            _ => None,
        };
        self.index
            .type_of
            .get(&name)
            .and_then(|&t| own(t))
            .filter(|p| p != "NOTDEFINED")
            .or_else(|| own(name))
    }

    fn predefined_of(&self, name: u64) -> Option<String> {
        let record = self.record(name)?;
        let Some(Parameter::Enumeration(p)) = self.attr(record, "PredefinedType") else {
            return None;
        };
        if p == "USERDEFINED" {
            for user in ["ElementType", "ObjectType", "ProcessType", "ResourceType"] {
                if let Some(Parameter::Text(t)) = self.attr(record, user) {
                    return Some(t.clone());
                }
            }
            return Some(String::new());
        }
        Some(p.clone())
    }

    // ---------------------------------------------------------------- attribute

    fn attribute_eval(&self, facet: &AttributeFacet, name: u64) -> Eval {
        let Some(record) = self.record(name) else {
            return Eval::new(false, false, "the instance does not exist");
        };
        let matched: Vec<_> = self
            .attributes_of(record)
            .into_iter()
            .filter(|(n, _, _)| name_matches(&facet.name, n))
            .collect();
        if matched.is_empty() {
            return Eval::new(false, false, "the entity has no such attribute");
        }
        // A name restriction may match several attributes; one of them suffices.
        let mut exists = false;
        let mut any = false;
        let mut reason = String::new();
        for (attr, position, derived) in matched {
            if derived {
                reason = format!("the attribute {attr} is derived");
                continue;
            }
            let v = record.parameters.get(position).map_or(Val::Null, Val::of);
            if matches!(v, Val::Null) {
                reason = format!("the attribute {attr} has no value");
                continue;
            }
            exists = true;
            if !v.usable() {
                reason = format!("the attribute {attr} is empty");
                continue;
            }
            if let Some(want) = &facet.value
                && (matches!(v, Val::Object(_)) || !value::matches(want, &v))
            {
                reason = format!("the attribute {attr} has another value");
                continue;
            }
            any = true;
        }
        Eval::new(exists, any, reason)
    }

    // ---------------------------------------------------------------- classification

    /// The classifications of an instance as `(system, identifications)`, the occurrence's
    /// overriding those of its type per system.
    fn classification_entries(&self, name: u64) -> Vec<(String, Vec<String>)> {
        let own = self.classifications_of(name);
        let mut entries = own;
        if let Some(&t) = self.index.type_of.get(&name) {
            for (system, values) in self.classifications_of(t) {
                if !entries.iter().any(|(s, _)| *s == system) {
                    entries.push((system, values));
                }
            }
        }
        entries
    }

    fn classifications_of(&self, name: u64) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for &c in self
            .index
            .classifications
            .get(&name)
            .map_or(&[][..], Vec::as_slice)
        {
            let mut values = Vec::new();
            let mut current = c;
            let mut system = None;
            for _ in 0..32 {
                let Some(r) = self.record(current) else { break };
                if r.type_name == "IFCCLASSIFICATIONREFERENCE" {
                    for key in ["Identification", "ItemReference"] {
                        if let Some(Parameter::Text(t)) = self.attr(r, key) {
                            values.push(t.clone());
                        }
                    }
                    match reference(self.attr(r, "ReferencedSource")) {
                        Some(next) => current = next,
                        None => break,
                    }
                } else {
                    if let Some(Parameter::Text(t)) = self.attr(r, "Name") {
                        system = Some(t.clone());
                    }
                    break;
                }
            }
            if let Some(system) = system {
                match out.iter_mut().find(|(s, _)| *s == system) {
                    Some((_, v)) => v.extend(values),
                    None => out.push((system, values)),
                }
            }
        }
        out
    }

    fn classification_eval(&self, facet: &ClassificationFacet, name: u64) -> Eval {
        let entries = self.classification_entries(name);
        let in_system: Vec<_> = entries
            .iter()
            .filter(|(s, _)| name_matches(&facet.system, s))
            .collect();
        if in_system.is_empty() {
            return Eval::new(
                !entries.is_empty(),
                false,
                "no classification of the system",
            );
        }
        match &facet.value {
            None => Eval::new(true, true, ""),
            Some(want) => {
                let ok = in_system.iter().any(|(_, values)| {
                    values
                        .iter()
                        .any(|v| value::matches(want, &Val::Str(v.clone())))
                });
                Eval::new(true, ok, "no classification reference has the value")
            }
        }
    }

    // ---------------------------------------------------------------- material

    /// Names and categories of the materials of an instance, from the occurrence or else its type.
    fn material_strings(&self, name: u64) -> (bool, Vec<String>) {
        let mut sources = vec![name];
        if let Some(&t) = self.index.type_of.get(&name) {
            sources.push(t);
        }
        for source in sources {
            if let Some(list) = self.index.materials.get(&source)
                && !list.is_empty()
            {
                let mut out = Vec::new();
                for &m in list {
                    self.collect_material(m, &mut out, 0);
                }
                return (true, out);
            }
        }
        (false, Vec::new())
    }

    fn collect_material(&self, name: u64, out: &mut Vec<String>, depth: u32) {
        if depth > 8 {
            return;
        }
        let Some(r) = self.record(name) else { return };
        for key in ["Name", "Category", "LayerSetName"] {
            if let Some(Parameter::Text(t)) = self.attr(r, key) {
                out.push(t.clone());
            }
        }
        for key in [
            "Material",
            "Materials",
            "ForLayerSet",
            "MaterialLayers",
            "ForProfileSet",
            "MaterialProfiles",
            "MaterialConstituents",
        ] {
            for child in refs(self.attr(r, key)) {
                self.collect_material(child, out, depth + 1);
            }
        }
    }

    fn material_eval(&self, facet: &MaterialFacet, name: u64) -> Eval {
        let (has, strings) = self.material_strings(name);
        if !has {
            return Eval::new(false, false, "the instance has no material");
        }
        match &facet.value {
            None => Eval::new(true, true, ""),
            Some(want) => Eval::new(
                true,
                strings
                    .iter()
                    .any(|s| value::matches(want, &Val::Str(s.clone()))),
                "no material has the name or category",
            ),
        }
    }

    // ---------------------------------------------------------------- part of

    fn part_of_eval(&self, facet: &PartOfFacet, name: u64) -> Eval {
        let wanted: Option<&[Relation]> = match facet.relation.as_deref() {
            None => None,
            Some("IFCRELAGGREGATES") => Some(&[Relation::Aggregates]),
            Some("IFCRELASSIGNSTOGROUP") => Some(&[Relation::Group]),
            Some("IFCRELCONTAINEDINSPATIALSTRUCTURE") => Some(&[Relation::Contained]),
            Some("IFCRELNESTS") => Some(&[Relation::Nests]),
            Some(_) => Some(&[Relation::Opening]),
        };
        let mut seen = BTreeSet::new();
        let mut stack = vec![name];
        while let Some(current) = stack.pop() {
            for &(kind, whole) in self.index.up.get(&current).map_or(&[][..], Vec::as_slice) {
                if wanted.is_some_and(|w| !w.contains(&kind)) || !seen.insert(whole) {
                    continue;
                }
                if self.class_matches(&facet.entity, whole) {
                    return Eval::new(true, true, "");
                }
                stack.push(whole);
            }
        }
        Eval::new(
            false,
            false,
            "the instance is not part of a matching entity",
        )
    }

    // ---------------------------------------------------------------- property

    /// Property sets and quantity sets of an instance: `(name of the set, instance of the set)`.
    /// The sets of the occurrence override those of its type of the same name.
    fn property_sets(&self, name: u64) -> Vec<(String, u64)> {
        let mut sets: Vec<(String, u64)> = Vec::new();
        let add = |ids: Vec<u64>, sets: &mut Vec<(String, u64)>, over: bool| {
            for id in ids {
                let Some(r) = self.record(id) else { continue };
                let n = match self.attr(r, "Name") {
                    Some(Parameter::Text(t)) => t.clone(),
                    _ => r.type_name.clone(),
                };
                if sets.iter().any(|(s, _)| *s == n) && !over {
                    continue;
                }
                sets.push((n, id));
            }
        };
        add(
            self.index
                .defined_by
                .get(&name)
                .cloned()
                .unwrap_or_default(),
            &mut sets,
            true,
        );
        if let Some(r) = self.record(name) {
            add(refs(self.attr(r, "HasPropertySets")), &mut sets, false);
        }
        add(
            self.index
                .material_props
                .get(&name)
                .cloned()
                .unwrap_or_default(),
            &mut sets,
            false,
        );
        if let Some(&t) = self.index.type_of.get(&name)
            && let Some(r) = self.record(t)
        {
            add(refs(self.attr(r, "HasPropertySets")), &mut sets, false);
        }
        // Material properties.
        let (_, _) = (0, 0);
        let mut material_sources = vec![name];
        if let Some(&t) = self.index.type_of.get(&name) {
            material_sources.push(t);
        }
        for source in material_sources {
            for &m in self
                .index
                .materials
                .get(&source)
                .map_or(&[][..], Vec::as_slice)
            {
                for &props in self
                    .index
                    .material_props
                    .get(&m)
                    .map_or(&[][..], Vec::as_slice)
                {
                    add(vec![props], &mut sets, false);
                }
            }
        }
        sets
    }

    /// The properties of a set: `(name, instance of the property)`.
    fn properties_of(&self, set: u64) -> Vec<(String, u64)> {
        let Some(r) = self.record(set) else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for key in [
            "HasProperties",
            "Quantities",
            "Properties",
            "ExtendedProperties",
        ] {
            ids.extend(refs(self.attr(r, key)));
        }
        let mut out: Vec<(String, u64)> = ids
            .into_iter()
            .filter_map(|id| {
                let p = self.record(id)?;
                match self.attr(p, "Name") {
                    Some(Parameter::Text(t)) => Some((t.clone(), id)),
                    _ => None,
                }
            })
            .collect();
        // Predefined property sets: the attributes of the entity are the properties.
        if r.type_name.starts_with("IFC") && r.type_name.ends_with("PROPERTIES") && out.is_empty() {
            for (n, position, derived) in self.attributes_of(r) {
                if !derived
                    && !matches!(
                        r.parameters.get(position),
                        Some(Parameter::Omitted | Parameter::Derived) | None
                    )
                {
                    out.push((n.to_owned(), set));
                }
            }
        }
        out
    }

    /// The conversion factor for a measure type, 1 for a type without a unit.
    fn factor(&self, data_type: &str) -> f64 {
        let unit = match data_type {
            "IFCLENGTHMEASURE" | "IFCPOSITIVELENGTHMEASURE" | "IFCNONNEGATIVELENGTHMEASURE" => {
                "LENGTHUNIT"
            }
            "IFCAREAMEASURE" => "AREAUNIT",
            "IFCVOLUMEMEASURE" => "VOLUMEUNIT",
            "IFCMASSMEASURE" => "MASSUNIT",
            "IFCTIMEMEASURE" => "TIMEUNIT",
            "IFCPLANEANGLEMEASURE" | "IFCPOSITIVEPLANEANGLEMEASURE" => "PLANEANGLEUNIT",
            _ => return 1.0,
        };
        self.units.get(unit).copied().unwrap_or(1.0)
    }

    /// The values of a property with their data type, `None` when the property has no value
    /// that can be checked (complex, reference).
    fn property_values(&self, prop: u64, set: u64, name: &str) -> Option<TypedValues> {
        let r = self.record(prop)?;
        let wrap = |p: &Parameter| -> (Val, Option<String>) {
            match p {
                Parameter::Typed { type_name, value } => (Val::of(value), Some(type_name.clone())),
                other => (Val::of(other), None),
            }
        };
        let typed_list = |p: Option<&Parameter>| -> Vec<(Val, Option<String>)> {
            match p {
                Some(Parameter::List(items)) => items.iter().map(wrap).collect(),
                _ => Vec::new(),
            }
        };
        let (values, is_range): (Vec<(Val, Option<String>)>, bool) = match r.type_name.as_str() {
            "IFCPROPERTYSINGLEVALUE" => (
                vec![self.attr(r, "NominalValue").map_or((Val::Null, None), wrap)],
                false,
            ),
            "IFCPROPERTYENUMERATEDVALUE" => (typed_list(self.attr(r, "EnumerationValues")), false),
            "IFCPROPERTYLISTVALUE" => (typed_list(self.attr(r, "ListValues")), false),
            "IFCPROPERTYTABLEVALUE" => {
                let mut v = typed_list(self.attr(r, "DefiningValues"));
                v.extend(typed_list(self.attr(r, "DefinedValues")));
                (v, false)
            }
            "IFCPROPERTYBOUNDEDVALUE" => {
                let bounds: Vec<_> = ["UpperBoundValue", "LowerBoundValue", "SetPointValue"]
                    .iter()
                    .filter_map(|k| self.attr(r, k).map(wrap))
                    .collect();
                (bounds, true)
            }
            "IFCQUANTITYLENGTH" | "IFCQUANTITYAREA" | "IFCQUANTITYVOLUME" | "IFCQUANTITYCOUNT"
            | "IFCQUANTITYWEIGHT" | "IFCQUANTITYTIME" => {
                let (key, ty) = match r.type_name.as_str() {
                    "IFCQUANTITYLENGTH" => ("LengthValue", "IFCLENGTHMEASURE"),
                    "IFCQUANTITYAREA" => ("AreaValue", "IFCAREAMEASURE"),
                    "IFCQUANTITYVOLUME" => ("VolumeValue", "IFCVOLUMEMEASURE"),
                    "IFCQUANTITYCOUNT" => ("CountValue", "IFCCOUNTMEASURE"),
                    "IFCQUANTITYWEIGHT" => ("WeightValue", "IFCMASSMEASURE"),
                    _ => ("TimeValue", "IFCTIMEMEASURE"),
                };
                let v = self.attr(r, key).map_or(Val::Null, Val::of);
                (vec![(v, Some(ty.to_owned()))], false)
            }
            _ if set == prop => {
                // A predefined property set: the attribute with the property's name.
                let rec = self.record(set)?;
                let position = self
                    .attributes_of(rec)
                    .into_iter()
                    .find(|(n, _, _)| *n == name)?
                    .1;
                let p = rec.parameters.get(position)?;
                let (v, ty) = wrap(p);
                let declared = self.entity_of(rec).and_then(|e| {
                    let position = self
                        .tables
                        .positions(e)
                        .into_iter()
                        .find(|q| q.attribute.name == name)?;
                    match position.attribute.ty {
                        crate::schema::TypeRef::Named(crate::schema::Named::Enumeration(i)) => self
                            .tables
                            .enumerations
                            .get(usize::from(i))
                            .map(|t| t.name.to_ascii_uppercase()),
                        crate::schema::TypeRef::Named(crate::schema::Named::Defined(i)) => self
                            .tables
                            .defined_types
                            .get(usize::from(i))
                            .map(|t| t.name.to_ascii_uppercase()),
                        _ => None,
                    }
                });
                (vec![(v, ty.or(declared))], false)
            }
            _ => return None,
        };
        Some((values, is_range))
    }

    fn property_eval(&self, facet: &PropertyFacet, name: u64) -> Eval {
        let mut found = false;
        let mut all_ok = true;
        let mut reason = String::new();
        for (set_name, set) in self.property_sets(name) {
            if !name_matches(&facet.property_set, &set_name) {
                continue;
            }
            let mut in_set = false;
            for (prop_name, prop) in self.properties_of(set) {
                if !name_matches(&facet.base_name, &prop_name) {
                    continue;
                }
                found = true;
                in_set = true;
                let Some((typed, is_bound)) = self.property_values(prop, set, &prop_name) else {
                    all_ok = false;
                    reason = format!("the property {prop_name} is of a kind that is not checked");
                    continue;
                };
                if typed.is_empty() || !typed.iter().any(|(v, _)| v.usable()) {
                    all_ok = false;
                    reason = format!("the property {prop_name} has no value");
                    continue;
                }
                // A table has values of two types; the facet's data type selects among them.
                let typed: Vec<_> = match &facet.data_type {
                    Some(want) => typed
                        .into_iter()
                        .filter(|(_, t)| t.as_deref() == Some(want.as_str()))
                        .collect(),
                    None => typed,
                };
                if typed.is_empty() {
                    all_ok = false;
                    reason = format!("the property {prop_name} has another data type");
                    continue;
                }
                let Some(want) = &facet.value else { continue };
                let scaled: Vec<Val> = typed
                    .iter()
                    .map(|(v, t)| {
                        let factor = t.as_deref().map_or(1.0, |t| self.factor(t));
                        match v {
                            Val::Real(r) => Val::Real(r * factor),
                            Val::Int(i) if factor != 1.0 => Val::Real(*i as f64 * factor),
                            other => other.clone(),
                        }
                    })
                    .collect();
                let ok = if is_bound && has_range(want) {
                    // Both bounds must be given and lie inside the range.
                    scaled.len() >= 2
                        && scaled
                            .iter()
                            .take(2)
                            .all(|v| v.usable() && value::matches(want, v))
                } else if has_range(want) {
                    scaled
                        .iter()
                        .filter(|v| v.usable())
                        .all(|v| value::matches(want, v))
                } else {
                    scaled.iter().any(|v| v.usable() && value::matches(want, v))
                };
                if !ok {
                    all_ok = false;
                    reason = format!("the property {prop_name} has another value");
                }
            }
            if !in_set {
                all_ok = false;
                reason = "a matching property set has no matching property".to_owned();
            }
        }
        if !found {
            return Eval::new(false, false, "no such property");
        }
        Eval::new(true, all_ok, reason)
    }

    // ---------------------------------------------------------------- facets

    fn eval(&self, facet: &Facet, name: u64) -> Eval {
        match facet {
            Facet::Entity(e) => {
                let ok = self.class_matches(e, name);
                Eval::new(ok, ok, "the entity or predefined type is different")
            }
            Facet::Attribute(a) => self.attribute_eval(a, name),
            Facet::Classification(c) => self.classification_eval(c, name),
            Facet::Property(p) => self.property_eval(p, name),
            Facet::Material(m) => self.material_eval(m, name),
            Facet::PartOf(p) => self.part_of_eval(p, name),
        }
    }

    /// The instances an applicability can select from: those of the entity facet's classes, or
    /// every instance.
    fn candidates(&self, applicability: &[Facet]) -> Vec<u64> {
        for facet in applicability {
            if let Facet::Entity(e) = facet {
                return match &e.name {
                    Value::Simple(n) => {
                        let class = self
                            .occurrence_of(&e.name)
                            .map_or_else(|| n.clone(), |(o, _)| o);
                        self.file.instances_of(&class).map(|i| i.name).collect()
                    }
                    Value::Restriction(_) => self
                        .file
                        .by_type
                        .iter()
                        .filter(|(t, _)| name_matches(&e.name, t))
                        .flat_map(|(_, positions)| positions.iter())
                        .filter_map(|&p| self.file.instances.get(p).map(|i| i.name))
                        .collect(),
                };
            }
        }
        self.file.instances().iter().map(|i| i.name).collect()
    }

    fn check_specification(&self, spec: &Specification) -> Result<SpecificationResult> {
        let cap = usize::try_from(self.limits.array_length).unwrap_or(usize::MAX);
        let mut applicable = Vec::new();
        let mut count = 0usize;
        for name in self.candidates(&spec.applicability) {
            self.cancel.check()?;
            if spec.applicability.iter().all(|f| self.eval(f, name).passes) {
                count += 1;
                if applicable.len() < cap {
                    applicable.push(name);
                }
            }
        }
        let mut failures = Vec::new();
        let mut failure_count = 0usize;
        let mut reason = None;
        match spec.cardinality {
            Cardinality::Prohibited => {
                if count > 0 {
                    reason = Some(format!(
                        "{count} instances are applicable to a prohibited specification"
                    ));
                }
            }
            cardinality => {
                if count == 0 && cardinality == Cardinality::Required {
                    reason = Some("no instance is applicable".to_owned());
                }
                for &name in &applicable {
                    self.cancel.check()?;
                    for (index, requirement) in spec.requirements.iter().enumerate() {
                        let e = self.eval(&requirement.facet, name);
                        let ok = match requirement.cardinality {
                            FacetCardinality::Required => e.passes,
                            FacetCardinality::Optional => !e.exists || e.passes,
                            FacetCardinality::Prohibited => !e.passes,
                        };
                        if !ok {
                            failure_count += 1;
                            if failures.len() < cap {
                                failures.push(Failure {
                                    instance: name,
                                    requirement: index,
                                    facet: requirement.facet.kind(),
                                    reason: match requirement.cardinality {
                                        FacetCardinality::Prohibited => {
                                            "the prohibited facet is satisfied".to_owned()
                                        }
                                        _ => e.reason,
                                    },
                                });
                            }
                        }
                    }
                }
            }
        }
        let status = if reason.is_none() && failure_count == 0 {
            Status::Pass
        } else {
            Status::Fail
        };
        Ok(SpecificationResult {
            name: spec.name.clone(),
            identifier: spec.identifier.clone(),
            status,
            applicable: count,
            applicable_instances: applicable,
            failures,
            failure_count,
            reason,
        })
    }
}

fn prefix(name: &str) -> Option<f64> {
    Some(match name {
        "EXA" => 1e18,
        "PETA" => 1e15,
        "TERA" => 1e12,
        "GIGA" => 1e9,
        "MEGA" => 1e6,
        "KILO" => 1e3,
        "HECTO" => 1e2,
        "DECA" => 1e1,
        "DECI" => 1e-1,
        "CENTI" => 1e-2,
        "MILLI" => 1e-3,
        "MICRO" => 1e-6,
        "NANO" => 1e-9,
        "PICO" => 1e-12,
        "FEMTO" => 1e-15,
        "ATTO" => 1e-18,
        _ => return None,
    })
}

impl Ids {
    /// Checks a file against every specification.
    ///
    /// # Errors
    ///
    /// `io.ids.schema_unavailable` when the tables of the schema of the file are not part of this
    /// build; `base.cancelled`.
    pub fn check(&self, file: &IfcFile, limits: &Limits, cancel: &Cancel) -> Result<Report> {
        let ctx = Ctx::new(file, limits, cancel)?;
        let specifications = self
            .specifications
            .iter()
            .map(|s| ctx.check_specification(s))
            .collect::<Result<Vec<_>>>()?;
        Ok(Report { specifications })
    }
}
