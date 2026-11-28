//! Classifications, owner histories and georeferencing.

use arc_base::Result;

use crate::error::reasons;

use super::super::{Reader, Value};
use super::{
    ClassificationRef, ClassificationSystem, CrsView, GeoReference, GeoSource, ObjectView, Origin,
    OwnerHistory, ResolvedPropertySet,
};

/// How far `ReferencedSource` is followed from a classification reference to its system.
const SOURCE_DEPTH: u32 = 8;

impl Reader<'_> {
    /// The classification associations of an object.
    pub(super) fn classifications_of(
        &mut self,
        object: u64,
        origin: Origin,
    ) -> Result<Vec<ClassificationRef>> {
        let mut out = Vec::new();
        for class in self.imp.class_of.get(&object).cloned().unwrap_or_default() {
            self.cancel.check()?;
            let Some(record) = self.rec(class) else {
                continue;
            };
            let reference = match record.type_name.as_str() {
                "IFCCLASSIFICATIONREFERENCE" => ClassificationRef {
                    instance: class,
                    identification: self
                        .text(record, "Identification", class)
                        .or_else(|| self.text(record, "ItemReference", class)),
                    name: self.text(record, "Name", class),
                    location: self.text(record, "Location", class),
                    system: self.classification_system(class),
                    origin,
                },
                "IFCCLASSIFICATION" => ClassificationRef {
                    instance: class,
                    identification: None,
                    name: self.text(record, "Name", class),
                    location: None,
                    system: self.classification_system(class),
                    origin,
                },
                other => {
                    self.lose(reasons::NOT_MAPPED, other, class, 1);
                    continue;
                }
            };
            self.link()?;
            out.push(reference);
        }
        Ok(out)
    }

    /// The `IfcClassification` a reference belongs to, following `ReferencedSource`.
    fn classification_system(&mut self, start: u64) -> Option<ClassificationSystem> {
        let mut current = start;
        for _ in 0..SOURCE_DEPTH {
            let record = self.rec(current)?;
            if record.type_name == "IFCCLASSIFICATION" {
                return Some(ClassificationSystem {
                    instance: current,
                    name: self.text(record, "Name", current),
                    source: self.text(record, "Source", current),
                    edition: self.text(record, "Edition", current),
                });
            }
            current = self.reference(record, "ReferencedSource", current)?;
        }
        None
    }

    /// The owner histories the objects refer to, in ascending instance name.
    pub(super) fn read_owner_histories(
        &mut self,
        objects: &[ObjectView],
    ) -> Result<Vec<OwnerHistory>> {
        let mut names: Vec<u64> = objects.iter().filter_map(|o| o.owner_history).collect();
        names.sort_unstable();
        names.dedup();
        let mut out = Vec::new();
        for name in names {
            self.cancel.check()?;
            let Some(record) = self.rec(name) else {
                self.lose(reasons::TARGET_MISSING, "IFCOWNERHISTORY", name, 1);
                continue;
            };
            let user = self.reference(record, "OwningUser", name);
            let (person, organization) = match user.and_then(|u| self.rec(u)) {
                Some(pao) => (
                    self.reference(pao, "ThePerson", name),
                    self.reference(pao, "TheOrganization", name),
                ),
                None => (None, None),
            };
            let person = person.and_then(|p| self.person_name(p));
            let organization = organization
                .and_then(|o| self.rec(o))
                .and_then(|o| self.text(o, "Name", name));
            let application = self
                .reference(record, "OwningApplication", name)
                .and_then(|a| self.rec(a));
            let (application, application_version) = match application {
                Some(a) => (
                    self.text(a, "ApplicationFullName", name),
                    self.text(a, "Version", name),
                ),
                None => (None, None),
            };
            self.fixed(160)?;
            out.push(OwnerHistory {
                instance: name,
                state: self.enumeration(record, "State", name),
                change_action: self.enumeration(record, "ChangeAction", name),
                last_modified: self.integer(record, "LastModifiedDate", name),
                created: self.integer(record, "CreationDate", name),
                user: person,
                organization,
                application,
                application_version,
            });
        }
        Ok(out)
    }

    /// The given and family names of an `IfcPerson`, or its identification.
    fn person_name(&mut self, person: u64) -> Option<String> {
        let record = self.rec(person)?;
        let family = self.text(record, "FamilyName", person);
        let given = self.text(record, "GivenName", person);
        let joined = [given, family]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if joined.is_empty() {
            self.text(record, "Identification", person)
                .or_else(|| self.text(record, "Id", person))
        } else {
            Some(joined)
        }
    }

    /// The georeference: an `IfcMapConversion` (IFC4 and IFC4X3), else the `ePSet_MapConversion`
    /// property set of the project or a site (the IFC2X3 convention).
    pub(super) fn read_georeference(
        &mut self,
        objects: &[ObjectView],
    ) -> Result<Option<GeoReference>> {
        let file = self.file;
        let found = ["IFCMAPCONVERSION", "IFCMAPCONVERSIONSCALED"]
            .iter()
            .find_map(|t| file.instances_of(t).next().map(|i| i.name));
        if let Some(name) = found {
            return Ok(self.map_conversion(name));
        }
        Ok(Self::pset_georeference(objects))
    }

    fn map_conversion(&mut self, name: u64) -> Option<GeoReference> {
        let record = self.rec(name)?;
        let crs = self
            .reference(record, "TargetCRS", name)
            .and_then(|c| self.crs(c));
        let mut number =
            |attr: &str, default: f64| self.real(record, attr, name).unwrap_or(default);
        Some(GeoReference {
            source: GeoSource::MapConversion,
            instance: Some(name),
            eastings: number("Eastings", 0.0),
            northings: number("Northings", 0.0),
            orthogonal_height: number("OrthogonalHeight", 0.0),
            x_axis_abscissa: number("XAxisAbscissa", 1.0),
            x_axis_ordinate: number("XAxisOrdinate", 0.0),
            scale: number("Scale", 1.0),
            crs,
        })
    }

    fn crs(&mut self, instance: u64) -> Option<CrsView> {
        let record = self.rec(instance)?;
        let map_unit = self.reference(record, "MapUnit", instance);
        let map_unit_to_meters = map_unit.and_then(|u| self.unit_to_si(u)).unwrap_or(1.0);
        Some(CrsView {
            name: self.text(record, "Name", instance),
            description: self.text(record, "Description", instance),
            geodetic_datum: self.text(record, "GeodeticDatum", instance),
            vertical_datum: self.text(record, "VerticalDatum", instance),
            map_projection: self.text(record, "MapProjection", instance),
            map_zone: self.text(record, "MapZone", instance),
            map_unit,
            map_unit_to_meters,
        })
    }

    fn pset_georeference(objects: &[ObjectView]) -> Option<GeoReference> {
        let sets = |name: &str| -> Option<&ResolvedPropertySet> {
            // The project first, then the sites, in file order.
            ["IFCPROJECT", "IFCSITE"].iter().find_map(|t| {
                objects.iter().filter(|o| o.type_name == *t).find_map(|o| {
                    o.property_sets.iter().find(|s| {
                        s.name
                            .as_deref()
                            .is_some_and(|n| n.eq_ignore_ascii_case(name))
                    })
                })
            })
        };
        let conversion = sets("ePSet_MapConversion")?;
        let number = |set: &ResolvedPropertySet, name: &str, default: f64| {
            let nominal = value_of(set, name);
            match nominal {
                Some(Value::Real(r)) => r,
                #[allow(clippy::cast_precision_loss, reason = "a map coordinate")]
                Some(Value::Integer(i)) => i as f64,
                _ => default,
            }
        };
        let text = |set: &ResolvedPropertySet, name: &str| match value_of(set, name) {
            Some(Value::Text(t)) => Some(t),
            _ => None,
        };
        let crs = sets("ePSet_ProjectedCRS").map(|set| CrsView {
            name: text(set, "Name"),
            description: text(set, "Description"),
            geodetic_datum: text(set, "GeodeticDatum"),
            vertical_datum: text(set, "VerticalDatum"),
            map_projection: text(set, "MapProjection"),
            map_zone: text(set, "MapZone"),
            map_unit: None,
            map_unit_to_meters: 1.0,
        });
        Some(GeoReference {
            source: GeoSource::PropertySet,
            instance: Some(conversion.instance),
            eastings: number(conversion, "Eastings", 0.0),
            northings: number(conversion, "Northings", 0.0),
            orthogonal_height: number(conversion, "OrthogonalHeight", 0.0),
            x_axis_abscissa: number(conversion, "XAxisAbscissa", 1.0),
            x_axis_ordinate: number(conversion, "XAxisOrdinate", 0.0),
            scale: number(conversion, "Scale", 1.0),
            crs,
        })
    }
}

/// The nominal value of the single value property `name` of a set.
fn value_of(set: &ResolvedPropertySet, name: &str) -> Option<Value> {
    set.properties
        .iter()
        .find(|p| {
            p.property
                .name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|p| match &p.property.value {
            super::super::PropertyValue::Single { nominal, .. } => {
                nominal.as_ref().map(|n| n.value.clone())
            }
            _ => None,
        })
}
