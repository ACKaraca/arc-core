//! The objects of the import views: attributes by name, sets, relations and the merge of the
//! type object's definitions into its occurrences.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::Result;

use crate::error::reasons;
use crate::model::{Parameter, Record};

use super::super::{
    PropertySet, PropertyValue, QuantityKind, QuantitySet, Reader, TypeAssignment, got_number,
    got_reference, got_text,
};
use super::units::{to_f64, unit_type_of_measure};
use super::{
    GroupView, ObjectKind, ObjectView, Origin, ResolvedProperty, ResolvedPropertySet,
    ResolvedQuantity, ResolvedQuantitySet,
};

/// Estimated bytes of an object view.
const OBJECT_BYTES: u64 = 512;
/// Estimated bytes of a resolved property or quantity.
const RESOLVED_BYTES: u64 = 48;

/// The sets of the objects, built once.
#[derive(Default)]
pub(super) struct Sets {
    /// Definition to its resolved property set.
    by_def_p: BTreeMap<u64, ResolvedPropertySet>,
    /// Definition to its resolved quantity set.
    by_def_q: BTreeMap<u64, ResolvedQuantitySet>,
    /// Object to the definitions assigned to it by `IfcRelDefinesByProperties`.
    defs_of: BTreeMap<u64, Vec<u64>>,
    /// Element to the structure that contains it.
    container_of: BTreeMap<u64, u64>,
    /// Occurrence to its type object.
    type_of: BTreeMap<u64, u64>,
}

impl<'f> Reader<'f> {
    /// The first record of an instance, marking it as read.
    pub(super) fn rec(&mut self, name: u64) -> Option<&'f Record> {
        let file = self.file;
        let record = file.instance(name)?.records.first()?;
        self.imp.touched.insert(name);
        Some(record)
    }

    /// Whether any record of the instance is of the entity `ancestor` or a subtype of it.
    pub(in crate::views) fn is_a(&self, name: u64, ancestor: &str) -> bool {
        let Some(tables) = self.file.tables() else {
            return false;
        };
        let (Some(instance), Some(ancestor)) =
            (self.file.instance(name), tables.entity_index(ancestor))
        else {
            return false;
        };
        instance.records.iter().any(|r| {
            tables
                .entity_index(&r.type_name)
                .is_some_and(|e| tables.is_subtype_of(e, ancestor))
        })
    }

    /// `lose`, and marks the instance as read in part.
    pub(super) fn text(&mut self, record: &Record, attr: &str, source: u64) -> Option<String> {
        let found = got_text(self.file.attribute(record, attr));
        self.opt(found, source, record.type_name.as_str())
    }

    pub(super) fn reference(&mut self, record: &Record, attr: &str, source: u64) -> Option<u64> {
        let found = got_reference(self.file.attribute(record, attr));
        self.opt(found, source, record.type_name.as_str())
    }

    pub(super) fn references_of(&mut self, record: &Record, attr: &str, source: u64) -> Vec<u64> {
        let parameter = self.file.attribute(record, attr);
        self.references(parameter, source, record.type_name.as_str())
            .items
    }

    pub(super) fn real(&mut self, record: &Record, attr: &str, source: u64) -> Option<f64> {
        let found = got_number(self.file.attribute(record, attr));
        self.opt(found, source, record.type_name.as_str())
            .map(to_f64)
    }

    pub(super) fn integer(&mut self, record: &Record, attr: &str, source: u64) -> Option<i64> {
        match self.file.attribute(record, attr) {
            Some(Parameter::Integer(i)) => Some(*i),
            Some(Parameter::Typed { value, .. }) => match value.as_ref() {
                Parameter::Integer(i) => Some(*i),
                _ => {
                    self.bad(source, record.type_name.as_str());
                    None
                }
            },
            None | Some(Parameter::Omitted | Parameter::Derived) => None,
            Some(_) => {
                self.bad(source, record.type_name.as_str());
                None
            }
        }
    }

    /// An enumeration value without its dots (`.T.` is `T`).
    pub(super) fn enumeration(
        &mut self,
        record: &Record,
        attr: &str,
        source: u64,
    ) -> Option<String> {
        match self.file.attribute(record, attr) {
            Some(Parameter::Enumeration(text)) => Some(text.clone()),
            Some(Parameter::Typed { value, .. }) => match value.as_ref() {
                Parameter::Enumeration(text) => Some(text.clone()),
                _ => {
                    self.bad(source, record.type_name.as_str());
                    None
                }
            },
            None | Some(Parameter::Omitted | Parameter::Derived) => None,
            Some(_) => {
                self.bad(source, record.type_name.as_str());
                None
            }
        }
    }

    /// A `BOOLEAN` or `LOGICAL` attribute: `None` for unknown or unset.
    pub(super) fn flag(&mut self, record: &Record, attr: &str, source: u64) -> Option<bool> {
        match self.enumeration(record, attr, source)?.as_str() {
            "T" => Some(true),
            "F" => Some(false),
            _ => None,
        }
    }

    /// The sets of every object: resolved once per definition, and the inverted relations.
    pub(super) fn set_index(
        &mut self,
        property_sets: &[PropertySet],
        quantity_sets: &[QuantitySet],
        types: &[TypeAssignment],
    ) -> Sets {
        let mut sets = Sets::default();
        for set in property_sets {
            let resolved = self.resolve_property_set(set);
            for object in &set.related {
                sets.defs_of
                    .entry(object.instance)
                    .or_default()
                    .push(set.definition.instance);
            }
            sets.by_def_p.insert(set.definition.instance, resolved);
        }
        for set in quantity_sets {
            let resolved = self.resolve_quantity_set(set);
            for object in &set.related {
                sets.defs_of
                    .entry(object.instance)
                    .or_default()
                    .push(set.definition.instance);
            }
            sets.by_def_q.insert(set.definition.instance, resolved);
        }
        for (structure, elements) in &self.contained {
            for element in elements {
                sets.container_of.entry(*element).or_insert(*structure);
            }
        }
        for assignment in types {
            sets.type_of
                .entry(assignment.element.instance)
                .or_insert(assignment.element_type.instance);
        }
        sets
    }

    fn resolve_property_set(&mut self, set: &PropertySet) -> ResolvedPropertySet {
        let properties = set
            .properties
            .iter()
            .map(|p| self.resolve_property(p, Origin::Occurrence))
            .collect();
        ResolvedPropertySet {
            instance: set.definition.instance,
            name: set.definition.name.clone(),
            properties,
        }
    }

    fn resolve_quantity_set(&mut self, set: &QuantitySet) -> ResolvedQuantitySet {
        let quantities = set
            .quantities
            .iter()
            .map(|q| {
                let (unit, factor) = self.unit_for(q.unit, quantity_unit_type(q.kind));
                let si_value = q.value.map(|n| {
                    let value = to_f64(n);
                    if q.kind == QuantityKind::Count {
                        value
                    } else {
                        factor.map_or(value, |f| value * f)
                    }
                });
                let applied = q.kind == QuantityKind::Count || factor.is_some();
                ResolvedQuantity {
                    quantity: Arc::clone(q),
                    origin: Origin::Occurrence,
                    unit,
                    si_value: si_value.filter(|_| applied),
                }
            })
            .collect();
        ResolvedQuantitySet {
            instance: set.definition.instance,
            name: set.definition.name.clone(),
            quantities,
        }
    }

    /// The unit and its factor to SI: the explicit unit, else the project's of `unit_type`. A unit
    /// with an offset (a temperature scale) has no factor.
    fn unit_for(
        &mut self,
        explicit: Option<u64>,
        unit_type: Option<String>,
    ) -> (Option<usize>, Option<f64>) {
        let index = explicit
            .and_then(|u| self.unit_index(u))
            .or_else(|| unit_type.and_then(|t| self.project_unit(&t)));
        let unit = index.and_then(|i| self.imp.units.units.get(i));
        let factor = unit
            .filter(|u| u.offset == 0.0 && u.unit_type != "THERMODYNAMICTEMPERATUREUNIT")
            .and_then(|u| u.to_si);
        (index, factor)
    }

    fn resolve_property(
        &mut self,
        property: &Arc<crate::views::Property>,
        origin: Origin,
    ) -> ResolvedProperty {
        let (explicit, measure) = match &property.value {
            PropertyValue::Single { nominal, unit } => {
                (*unit, nominal.as_ref().and_then(|n| n.type_name.clone()))
            }
            PropertyValue::List { values, unit } => {
                (*unit, values.first().and_then(|n| n.type_name.clone()))
            }
            PropertyValue::Bounded {
                upper, lower, unit, ..
            } => (
                *unit,
                upper
                    .as_ref()
                    .or(lower.as_ref())
                    .and_then(|n| n.type_name.clone()),
            ),
            PropertyValue::Enumerated { .. } => (None, None),
        };
        let (unit, scale_to_si) =
            self.unit_for(explicit, measure.as_deref().and_then(unit_type_of_measure));
        ResolvedProperty {
            property: Arc::clone(property),
            origin,
            unit,
            scale_to_si,
        }
    }

    /// Every object of the file in ascending instance name, with its own definitions; the type
    /// object's are merged by [`Reader::merge_types`].
    pub(super) fn read_objects(&mut self, sets: &mut Sets) -> Result<Vec<ObjectView>> {
        let file = self.file;
        let mut objects = Vec::new();
        for instance in file.instances() {
            self.cancel.check()?;
            let name = instance.name;
            if !self.is_a(name, "IFCOBJECTDEFINITION") || self.is_a(name, "IFCGROUP") {
                continue;
            }
            let kind = if self.is_a(name, "IFCTYPEOBJECT") {
                ObjectKind::Type
            } else if self.is_a(name, "IFCPRODUCT") {
                ObjectKind::Product
            } else {
                ObjectKind::Other
            };
            if let Some(view) = self.object(name, kind, sets)? {
                objects.push(view);
            }
        }
        objects.sort_by_key(|o| o.instance);
        Ok(objects)
    }

    fn object(
        &mut self,
        name: u64,
        kind: ObjectKind,
        sets: &mut Sets,
    ) -> Result<Option<ObjectView>> {
        let (Some(rooted), Some(record)) = (self.entity(name)?, self.rec(name)) else {
            return Ok(None);
        };
        self.fixed(OBJECT_BYTES)?;
        let is_type = kind == ObjectKind::Type;
        let object_type = self.text(
            record,
            if is_type { "ElementType" } else { "ObjectType" },
            name,
        );
        let tag = self.text(record, "Tag", name);
        let predefined_type = self.enumeration(record, "PredefinedType", name);
        let (property_sets, quantity_sets) = self.own_sets(name, record, kind, sets)?;
        let placement = match kind {
            ObjectKind::Product => self
                .reference(record, "ObjectPlacement", name)
                .and_then(|p| self.placement(p)),
            _ => None,
        };
        let (elevation, latitude_longitude) = self.site_attributes(record, name);
        let classifications = self.classifications_of(name, Origin::Occurrence)?;
        let material = match self.imp.material_of.get(&name).copied() {
            Some(material) => self
                .material_association(material)?
                .map(|m| (m, Origin::Occurrence)),
            None => None,
        };
        let styles = match kind {
            ObjectKind::Product => self.object_styles(record, name)?,
            _ => Vec::new(),
        };
        let (bodies, mapped) = match kind {
            ObjectKind::Product => self.representation_items(record, name)?,
            _ => (Vec::new(), Vec::new()),
        };
        let representation_maps = match kind {
            ObjectKind::Type => self.representation_maps(record, name),
            _ => Vec::new(),
        };
        let owner_history = self.reference(record, "OwnerHistory", name);
        let imp = &self.imp;
        let get = |map: &BTreeMap<u64, Vec<u64>>| map.get(&name).cloned().unwrap_or_default();
        Ok(Some(ObjectView {
            instance: name,
            type_name: rooted.type_name.clone(),
            kind,
            global_id: rooted.global_id.clone(),
            name: rooted.name.clone(),
            description: rooted.description.clone(),
            object_type: object_type.clone(),
            tag,
            effective_predefined_type: predefined_type.clone(),
            user_defined_type: (predefined_type.as_deref() == Some("USERDEFINED"))
                .then(|| object_type.clone())
                .flatten(),
            predefined_type,
            type_object: sets.type_of.get(&name).copied(),
            container: sets.container_of.get(&name).copied(),
            aggregate_parent: imp.aggregate_parent.get(&name).copied(),
            parts: get(&imp.aggregate_parts),
            nest_parent: imp.nest_parent.get(&name).copied(),
            nested: get(&imp.nest_children),
            placement,
            elevation,
            latitude_longitude,
            property_sets,
            quantity_sets,
            classifications,
            material,
            styles,
            bodies,
            mapped,
            representation_maps,
            openings: get(&imp.openings_of),
            voids: imp.host_of.get(&name).copied(),
            filled_by: get(&imp.fillers_of),
            fills: imp.opening_of.get(&name).copied(),
            connections: Vec::new(),
            space_boundaries: Vec::new(),
            groups: get(&imp.groups_of),
            owner_history,
        }))
    }

    /// The property and quantity sets assigned to the object: by relationship for an occurrence,
    /// through `HasPropertySets` for a type object.
    fn own_sets(
        &mut self,
        name: u64,
        record: &Record,
        kind: ObjectKind,
        sets: &mut Sets,
    ) -> Result<(Vec<ResolvedPropertySet>, Vec<ResolvedQuantitySet>)> {
        let mut definitions = sets.defs_of.get(&name).cloned().unwrap_or_default();
        if kind == ObjectKind::Type {
            for definition in self.references_of(record, "HasPropertySets", name) {
                self.cancel.check()?;
                self.type_definition(definition, sets);
                definitions.push(definition);
            }
        }
        let mut property_sets = Vec::new();
        let mut quantity_sets = Vec::new();
        for definition in definitions {
            if let Some(set) = sets.by_def_p.get(&definition) {
                let bytes = RESOLVED_BYTES
                    .saturating_mul(u64::try_from(set.properties.len()).unwrap_or(u64::MAX));
                self.charge(bytes)?;
                self.emit()?;
                property_sets.push(set.clone());
            } else if let Some(set) = sets.by_def_q.get(&definition) {
                let bytes = RESOLVED_BYTES
                    .saturating_mul(u64::try_from(set.quantities.len()).unwrap_or(u64::MAX));
                self.charge(bytes)?;
                self.emit()?;
                quantity_sets.push(set.clone());
            }
        }
        Ok((property_sets, quantity_sets))
    }

    /// Builds a set a type object lists in `HasPropertySets` that no relationship assigned.
    fn type_definition(&mut self, definition: u64, sets: &mut Sets) {
        if sets.by_def_p.contains_key(&definition) || sets.by_def_q.contains_key(&definition) {
            return;
        }
        let file = self.file;
        let Some(instance) = file.instance(definition) else {
            self.lose(reasons::TARGET_MISSING, "IFCTYPEOBJECT", definition, 1);
            return;
        };
        let Some(record) = instance.records.first() else {
            return;
        };
        match record.type_name.as_str() {
            "IFCPROPERTYSET" => {
                if let Ok(Some(set)) = self.property_set(definition, record, &[]) {
                    let resolved = self.resolve_property_set(&set);
                    sets.by_def_p.insert(definition, resolved);
                }
            }
            "IFCELEMENTQUANTITY" => {
                if let Ok(Some(set)) = self.quantity_set(definition, record, &[]) {
                    let resolved = self.resolve_quantity_set(&set);
                    sets.by_def_q.insert(definition, resolved);
                }
            }
            other => self.lose(reasons::NOT_MAPPED, other, definition, 1),
        }
    }

    /// Merges the definitions of the type object into every occurrence: the predefined type, the
    /// property and quantity sets (the occurrence's values win), the classification and, when the
    /// occurrence has none, the material.
    pub(super) fn merge_types(&mut self, objects: &mut [ObjectView]) -> Result<()> {
        for i in 0..objects.len() {
            self.cancel.check()?;
            let Some(type_instance) = objects.get(i).and_then(|o| o.type_object) else {
                continue;
            };
            let Some(t) = pos(objects, type_instance)
                .and_then(|p| objects.get(p))
                .cloned()
            else {
                continue;
            };
            let Some(o) = objects.get_mut(i) else {
                continue;
            };
            // The type object defines the predefined type; the occurrence's own value stays where
            // the type says `NOTDEFINED`, or says `USERDEFINED` without naming its kind.
            match t.predefined_type.as_deref() {
                None | Some("NOTDEFINED") => {}
                Some("USERDEFINED") => {
                    if t.object_type.is_some() {
                        o.effective_predefined_type = t.predefined_type.clone();
                        o.user_defined_type = t.object_type.clone();
                    }
                }
                Some(_) => {
                    o.effective_predefined_type = t.predefined_type.clone();
                    o.user_defined_type = None;
                }
            }
            if o.effective_predefined_type.is_none() {
                o.effective_predefined_type = t.predefined_type.clone();
            }
            let members: usize = t
                .property_sets
                .iter()
                .map(|s| s.properties.len())
                .sum::<usize>()
                + t.quantity_sets
                    .iter()
                    .map(|s| s.quantities.len())
                    .sum::<usize>();
            self.charge(RESOLVED_BYTES.saturating_mul(u64::try_from(members).unwrap_or(u64::MAX)))?;
            merge_property_sets(&mut o.property_sets, &t.property_sets);
            merge_quantity_sets(&mut o.quantity_sets, &t.quantity_sets);
            // A reference of the type is inherited unless the occurrence has one of the same system.
            for mut class in t.classifications {
                let own = |c: &super::ClassificationRef| {
                    c.system
                        .as_ref()
                        .zip(class.system.as_ref())
                        .is_some_and(|(a, b)| a.instance == b.instance)
                };
                if !o.classifications.iter().any(own) {
                    class.origin = Origin::Type;
                    o.classifications.push(class);
                }
            }
            if o.material.is_none() {
                o.material = t.material.map(|(m, _)| (m, Origin::Type));
            }
        }
        Ok(())
    }

    /// The connection and space boundary indices of the objects.
    pub(super) fn link_relations(&mut self, objects: &mut [ObjectView]) -> Result<()> {
        let connections = std::mem::take(&mut self.imp.connections);
        for (i, c) in connections.iter().enumerate() {
            self.cancel.check()?;
            for end in [c.relating, c.related] {
                if let Some(o) = pos(objects, end).and_then(|p| objects.get_mut(p)) {
                    self.link()?;
                    o.connections.push(i);
                }
            }
        }
        self.imp.connections = connections;
        let boundaries = std::mem::take(&mut self.imp.boundaries);
        for (i, b) in boundaries.iter().enumerate() {
            self.cancel.check()?;
            for end in std::iter::once(b.space).chain(b.element) {
                if let Some(o) = pos(objects, end).and_then(|p| objects.get_mut(p)) {
                    self.link()?;
                    o.space_boundaries.push(i);
                }
            }
        }
        self.imp.boundaries = boundaries;
        Ok(())
    }

    /// The groups, systems and zones.
    pub(super) fn read_groups(&mut self) -> Result<Vec<GroupView>> {
        let file = self.file;
        let mut groups = Vec::new();
        for instance in file.instances() {
            self.cancel.check()?;
            let name = instance.name;
            if !self.is_a(name, "IFCGROUP") {
                continue;
            }
            let (Some(rooted), Some(record)) = (self.entity(name)?, self.rec(name)) else {
                continue;
            };
            self.fixed(OBJECT_BYTES)?;
            groups.push(GroupView {
                instance: name,
                type_name: rooted.type_name.clone(),
                global_id: rooted.global_id.clone(),
                name: rooted.name.clone(),
                description: rooted.description.clone(),
                object_type: self.text(record, "ObjectType", name),
                predefined_type: self.enumeration(record, "PredefinedType", name),
                long_name: self.text(record, "LongName", name),
                members: self.imp.members_of.get(&name).cloned().unwrap_or_default(),
            });
        }
        Ok(groups)
    }

    /// The elevation (meters) and the reference latitude and longitude (decimal degrees) of the
    /// spatial elements that have them.
    fn site_attributes(&mut self, record: &Record, name: u64) -> (Option<f64>, Option<(f64, f64)>) {
        let attr = match record.type_name.as_str() {
            "IFCBUILDINGSTOREY" => "Elevation",
            "IFCBUILDING" => "ElevationOfRefHeight",
            "IFCSITE" => "RefElevation",
            _ => return (None, None),
        };
        let scale = self.imp.units.length_to_meters;
        let elevation = self.real(record, attr, name).map(|e| e * scale);
        let angle = |this: &mut Self, attr: &str| match this.file.attribute(record, attr) {
            Some(Parameter::List(items)) => dms(items),
            _ => None,
        };
        let coordinates = angle(self, "RefLatitude").zip(angle(self, "RefLongitude"));
        (elevation, coordinates)
    }
}

/// Decimal degrees of `IfcCompoundPlaneAngleMeasure`: degrees, minutes, seconds, millionths.
fn dms(items: &[Parameter]) -> Option<f64> {
    let mut parts = [0.0_f64; 4];
    let mut sign = 0.0_f64;
    for (slot, item) in parts.iter_mut().zip(items) {
        let Parameter::Integer(i) = item else {
            return None;
        };
        #[allow(clippy::cast_precision_loss, reason = "an angle component")]
        let value = *i as f64;
        if sign == 0.0 && value != 0.0 {
            sign = value.signum();
        }
        *slot = value.abs();
    }
    let sign = if sign == 0.0 { 1.0 } else { sign };
    Some(sign * (parts[0] + parts[1] / 60.0 + parts[2] / 3600.0 + parts[3] / 3.6e9))
}

/// The position of an object in the sorted list.
pub(super) fn pos(objects: &[ObjectView], instance: u64) -> Option<usize> {
    objects.binary_search_by_key(&instance, |o| o.instance).ok()
}

fn quantity_unit_type(kind: QuantityKind) -> Option<String> {
    let unit = match kind {
        QuantityKind::Length => "LENGTHUNIT",
        QuantityKind::Area => "AREAUNIT",
        QuantityKind::Volume => "VOLUMEUNIT",
        QuantityKind::Weight => "MASSUNIT",
        QuantityKind::Time => "TIMEUNIT",
        QuantityKind::Count => return None,
    };
    Some(unit.to_owned())
}

fn merge_property_sets(own: &mut Vec<ResolvedPropertySet>, from_type: &[ResolvedPropertySet]) {
    for set in from_type {
        let inherited = |p: &ResolvedProperty| ResolvedProperty {
            origin: Origin::Type,
            ..p.clone()
        };
        match own
            .iter_mut()
            .find(|s| s.name.is_some() && s.name == set.name)
        {
            Some(target) => {
                for property in &set.properties {
                    if !target
                        .properties
                        .iter()
                        .any(|p| p.property.name == property.property.name)
                    {
                        target.properties.push(inherited(property));
                    }
                }
            }
            None => own.push(ResolvedPropertySet {
                instance: set.instance,
                name: set.name.clone(),
                properties: set.properties.iter().map(inherited).collect(),
            }),
        }
    }
}

fn merge_quantity_sets(own: &mut Vec<ResolvedQuantitySet>, from_type: &[ResolvedQuantitySet]) {
    for set in from_type {
        let inherited = |q: &ResolvedQuantity| ResolvedQuantity {
            origin: Origin::Type,
            ..q.clone()
        };
        match own
            .iter_mut()
            .find(|s| s.name.is_some() && s.name == set.name)
        {
            Some(target) => {
                for quantity in &set.quantities {
                    if !target
                        .quantities
                        .iter()
                        .any(|q| q.quantity.name == quantity.quantity.name)
                    {
                        target.quantities.push(inherited(quantity));
                    }
                }
            }
            None => own.push(ResolvedQuantitySet {
                instance: set.instance,
                name: set.name.clone(),
                quantities: set.quantities.iter().map(inherited).collect(),
            }),
        }
    }
}
