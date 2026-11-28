//! The import views: the objects of a file with everything an importer needs to map them, read by
//! name through the schema tables so IFC2X3, IFC4 and IFC4X3 share one code path.
//!
//! The pass has two steps. While the relationships are scanned (together with the existing
//! views), the pairs of the relationships read here are collected in [`State`]. After the property
//! and quantity sets are built, [`Reader::import`] reads the objects and merges the type objects'
//! definitions into the occurrences. Every instance the views read is marked, so the coverage
//! report can say which classes were interpreted, which only in part and which were kept as they
//! are. Without schema tables (`IfcFile::tables` is `None`) the import views are empty and the
//! coverage lists every instance as preserved.
//!
//! Lengths are in meters: the project's length unit is applied to every length the views report
//! (placements, layer thicknesses, elevations). Property and quantity values stay as the file
//! states them; [`ResolvedProperty::scale_to_si`] and [`ResolvedQuantity::si_value`] carry the
//! conversion.

mod cover;
mod materials;
mod misc;
mod objects;
mod placement;
mod representation;
mod styles;
mod types;
mod units;

use std::collections::{BTreeMap, BTreeSet};

use arc_base::Result;

use crate::error::reasons;
use crate::model::Record;
use crate::schema::InverseIndex;

use super::{Reader, got_reference};

pub use types::{
    ClassCoverage, ClassificationRef, ClassificationSystem, Connection, ConstituentView, CrsView,
    GeoReference, GeoSource, GroupView, ImportViews, LayerUsage, LayerView, MappedView,
    MaterialAssociation, MaterialRef, Matrix, ObjectKind, ObjectView, Origin, OwnerHistory,
    PlacementState, PlacementView, ProfileView, ResolvedProperty, ResolvedPropertySet,
    ResolvedQuantity, ResolvedQuantitySet, SpaceBoundary, StyleView, UnitKind, UnitSystem,
    UnitView,
};

/// The entity types the import views read, besides those of `UNDERSTOOD` in the parent module.
pub(crate) const UNDERSTOOD_IMPORT: &[&str] = &[
    "IFCRELASSOCIATESMATERIAL",
    "IFCRELASSOCIATESCLASSIFICATION",
    "IFCRELVOIDSELEMENT",
    "IFCRELFILLSELEMENT",
    "IFCRELNESTS",
    "IFCRELCONNECTSELEMENTS",
    "IFCRELCONNECTSPATHELEMENTS",
    "IFCRELCONNECTSWITHREALIZINGELEMENTS",
    "IFCRELSPACEBOUNDARY",
    "IFCRELSPACEBOUNDARY1STLEVEL",
    "IFCRELSPACEBOUNDARY2NDLEVEL",
    "IFCRELASSIGNSTOGROUP",
    "IFCRELASSIGNSTOGROUPBYFACTOR",
];

/// The relationships collected by the scan and the marks of the coverage.
#[derive(Debug, Default)]
pub(super) struct State {
    /// The inverse index of the file.
    pub index: InverseIndex,
    /// Instances the views read.
    pub touched: BTreeSet<u64>,
    /// Instances the views read in part: a value was unusable or a target missing.
    pub flagged: BTreeSet<u64>,
    /// Object to the instance of its material association, the first one.
    pub material_of: BTreeMap<u64, u64>,
    /// Object to its classification instances.
    pub class_of: BTreeMap<u64, Vec<u64>>,
    /// Host to the opening elements that void it.
    pub openings_of: BTreeMap<u64, Vec<u64>>,
    /// Opening element to the host it voids.
    pub host_of: BTreeMap<u64, u64>,
    /// Opening element to the elements that fill it.
    pub fillers_of: BTreeMap<u64, Vec<u64>>,
    /// Filling element to its opening element.
    pub opening_of: BTreeMap<u64, u64>,
    /// Object to its `IfcRelAggregates` parent.
    pub aggregate_parent: BTreeMap<u64, u64>,
    /// Object to its `IfcRelAggregates` parts.
    pub aggregate_parts: BTreeMap<u64, Vec<u64>>,
    /// Object to its `IfcRelNests` parent.
    pub nest_parent: BTreeMap<u64, u64>,
    /// Object to its `IfcRelNests` children.
    pub nest_children: BTreeMap<u64, Vec<u64>>,
    /// The element connections.
    pub connections: Vec<Connection>,
    /// The space boundaries.
    pub boundaries: Vec<SpaceBoundary>,
    /// Group to its members.
    pub members_of: BTreeMap<u64, Vec<u64>>,
    /// Member to its groups.
    pub groups_of: BTreeMap<u64, Vec<u64>>,
    /// The pairs already recorded, by relationship kind.
    pub seen: BTreeSet<(u8, u64, u64)>,
    /// The units, as far as they are read.
    pub units: UnitSystem,
    /// The placements resolved so far.
    pub placed: BTreeMap<u64, (Matrix, PlacementState)>,
    /// The material associations read so far, by the instance of the association.
    pub associations: BTreeMap<u64, Option<MaterialAssociation>>,
}

/// Relationship kinds in [`State::seen`].
const FILLS: u8 = 1;
const VOIDS: u8 = 2;
const NESTS: u8 = 3;
const AGGREGATES: u8 = 4;
const GROUPS: u8 = 5;

impl Reader<'_> {
    /// Reads the relationships of the import views; called for every relationship record.
    pub(super) fn import_relationship(&mut self, name: u64, record: &Record) {
        let handled = match record.type_name.as_str() {
            "IFCRELASSOCIATESMATERIAL" => self.assoc_material(name, record),
            "IFCRELASSOCIATESCLASSIFICATION" => self.assoc_classification(name, record),
            "IFCRELVOIDSELEMENT" => self.voids(name, record),
            "IFCRELFILLSELEMENT" => self.fills(name, record),
            "IFCRELNESTS" => self.decomposition(name, record, NESTS),
            "IFCRELAGGREGATES" => self.decomposition(name, record, AGGREGATES),
            "IFCRELASSIGNSTOGROUP" | "IFCRELASSIGNSTOGROUPBYFACTOR" => {
                self.assign_group(name, record)
            }
            "IFCRELCONNECTSELEMENTS"
            | "IFCRELCONNECTSPATHELEMENTS"
            | "IFCRELCONNECTSWITHREALIZINGELEMENTS" => self.connect(name, record),
            "IFCRELSPACEBOUNDARY"
            | "IFCRELSPACEBOUNDARY1STLEVEL"
            | "IFCRELSPACEBOUNDARY2NDLEVEL" => self.boundary(name, record),
            _ => false,
        };
        if handled {
            self.imp.touched.insert(name);
        }
    }

    /// Records a pair once; `false` when the pair was recorded before.
    fn once(&mut self, kind: u8, a: u64, b: u64) -> bool {
        self.imp.seen.insert((kind, a, b))
    }

    /// The reference attribute `attr` of a relationship that must name an instance of the file;
    /// a missing target is counted.
    fn target(&mut self, record: &Record, attr: &str, name: u64) -> Option<u64> {
        let found = got_reference(self.file.attribute(record, attr));
        let target = self.opt(found, name, record.type_name.as_str())?;
        if self.file.contains(target) {
            Some(target)
        } else {
            self.lose(reasons::TARGET_MISSING, record.type_name.as_str(), name, 1);
            None
        }
    }

    /// The existing members of the reference list `attr` of a relationship.
    fn targets(&mut self, record: &Record, attr: &str, name: u64) -> Vec<u64> {
        let listed = self.references(
            self.file.attribute(record, attr),
            name,
            record.type_name.as_str(),
        );
        let mut out = Vec::new();
        for item in listed.items {
            if self.file.contains(item) {
                out.push(item);
            } else {
                self.lose(reasons::TARGET_MISSING, record.type_name.as_str(), name, 1);
            }
        }
        out
    }

    fn assoc_material(&mut self, name: u64, record: &Record) -> bool {
        let objects = self.targets(record, "RelatedObjects", name);
        let relating = self.target(record, "RelatingMaterial", name);
        if let Some(material) = relating {
            for object in objects {
                if self.imp.material_of.insert(object, material).is_some() {
                    self.lose(
                        reasons::DUPLICATE_PLACEMENT,
                        "IFCRELASSOCIATESMATERIAL",
                        name,
                        1,
                    );
                }
            }
        }
        true
    }

    fn assoc_classification(&mut self, name: u64, record: &Record) -> bool {
        let objects = self.targets(record, "RelatedObjects", name);
        if let Some(class) = self.target(record, "RelatingClassification", name) {
            for object in objects {
                let list = self.imp.class_of.entry(object).or_default();
                if !list.contains(&class) {
                    list.push(class);
                }
            }
        }
        true
    }

    fn voids(&mut self, name: u64, record: &Record) -> bool {
        let host = self.target(record, "RelatingBuildingElement", name);
        let opening = self.target(record, "RelatedOpeningElement", name);
        if let (Some(host), Some(opening)) = (host, opening) {
            if self.once(VOIDS, host, opening) {
                self.imp.openings_of.entry(host).or_default().push(opening);
                self.imp.host_of.entry(opening).or_insert(host);
            } else {
                self.lose(reasons::DUPLICATE_PLACEMENT, "IFCRELVOIDSELEMENT", name, 1);
            }
        }
        true
    }

    fn fills(&mut self, name: u64, record: &Record) -> bool {
        let opening = self.target(record, "RelatingOpeningElement", name);
        let filler = self.target(record, "RelatedBuildingElement", name);
        if let (Some(opening), Some(filler)) = (opening, filler) {
            if self.once(FILLS, opening, filler) {
                self.imp.fillers_of.entry(opening).or_default().push(filler);
                self.imp.opening_of.entry(filler).or_insert(opening);
            } else {
                self.lose(reasons::DUPLICATE_PLACEMENT, "IFCRELFILLSELEMENT", name, 1);
            }
        }
        true
    }

    /// `IfcRelAggregates` (all objects, not only the spatial structure) and `IfcRelNests`.
    fn decomposition(&mut self, name: u64, record: &Record, kind: u8) -> bool {
        let parent = self.target(record, "RelatingObject", name);
        let children = self.targets(record, "RelatedObjects", name);
        let Some(parent) = parent else { return true };
        for child in children {
            if !self.once(kind, parent, child) {
                continue;
            }
            let imp = &mut self.imp;
            let (parents, parts) = if kind == NESTS {
                (&mut imp.nest_parent, &mut imp.nest_children)
            } else {
                (&mut imp.aggregate_parent, &mut imp.aggregate_parts)
            };
            let second_parent = parents.insert(child, parent).is_some_and(|p| p != parent);
            parts.entry(parent).or_default().push(child);
            // The spatial reader already counts a second parent between spatial elements.
            let counted = kind == AGGREGATES
                && self.spatial.contains(&parent)
                && self.spatial.contains(&child);
            if second_parent && !counted {
                self.lose(
                    reasons::DUPLICATE_PLACEMENT,
                    record.type_name.as_str(),
                    name,
                    1,
                );
            }
        }
        true
    }

    fn assign_group(&mut self, name: u64, record: &Record) -> bool {
        let group = self.target(record, "RelatingGroup", name);
        let members = self.targets(record, "RelatedObjects", name);
        let Some(group) = group else { return true };
        for member in members {
            if self.once(GROUPS, group, member) {
                self.imp.members_of.entry(group).or_default().push(member);
                self.imp.groups_of.entry(member).or_default().push(group);
            }
        }
        true
    }

    fn connect(&mut self, name: u64, record: &Record) -> bool {
        let relating = self.target(record, "RelatingElement", name);
        let related = self.target(record, "RelatedElement", name);
        let (Some(relating), Some(related)) = (relating, related) else {
            return true;
        };
        let relating_type = self.enumeration(record, "RelatingConnectionType", name);
        let related_type = self.enumeration(record, "RelatedConnectionType", name);
        let description = self.text(record, "Description", name);
        let realizing = self.targets(record, "RealizingElements", name);
        self.imp.connections.push(Connection {
            instance: name,
            kind: record.type_name.clone(),
            relating,
            related,
            relating_type,
            related_type,
            realizing,
            description,
        });
        true
    }

    fn boundary(&mut self, name: u64, record: &Record) -> bool {
        let Some(space) = self.target(record, "RelatingSpace", name) else {
            return true;
        };
        let element = self.optional_target(record, "RelatedBuildingElement", name);
        let geometry = self.optional_target(record, "ConnectionGeometry", name);
        let level = match record.type_name.as_str() {
            "IFCRELSPACEBOUNDARY1STLEVEL" => 1,
            "IFCRELSPACEBOUNDARY2NDLEVEL" => 2,
            _ => 0,
        };
        let parent = self.optional_target(record, "ParentBoundary", name);
        let corresponding = self.optional_target(record, "CorrespondingBoundary", name);
        let boundary = SpaceBoundary {
            instance: name,
            level,
            space,
            element,
            physical: self.enumeration(record, "PhysicalOrVirtualBoundary", name),
            internal: self.enumeration(record, "InternalOrExternalBoundary", name),
            geometry,
            parent,
            corresponding,
        };
        self.imp.boundaries.push(boundary);
        true
    }

    /// Like [`Reader::target`], but an omitted value is not a loss.
    fn optional_target(&mut self, record: &Record, attr: &str, name: u64) -> Option<u64> {
        match self.file.attribute(record, attr) {
            None | Some(crate::model::Parameter::Omitted | crate::model::Parameter::Derived) => {
                None
            }
            Some(_) => self.target(record, attr, name),
        }
    }

    /// Builds the import views. `property_sets` and `quantity_sets` are the views built by the
    /// parent pass; the objects' sets are taken from them.
    pub(super) fn import(
        &mut self,
        property_sets: &[super::PropertySet],
        quantity_sets: &[super::QuantitySet],
        types: &[super::TypeAssignment],
    ) -> Result<ImportViews> {
        if self.file.tables().is_none() {
            return self.import_without_tables();
        }
        self.read_units()?;
        let mut sets = self.set_index(property_sets, quantity_sets, types);
        let mut objects = self.read_objects(&mut sets)?;
        let groups = self.read_groups()?;
        self.merge_types(&mut objects)?;
        self.link_relations(&mut objects)?;
        let owner_histories = self.read_owner_histories(&objects)?;
        let georeference = self.read_georeference(&objects)?;
        let connections = std::mem::take(&mut self.imp.connections);
        let space_boundaries = std::mem::take(&mut self.imp.boundaries);
        let coverage = self.coverage()?;
        Ok(ImportViews {
            units: std::mem::take(&mut self.imp.units),
            objects,
            groups,
            connections,
            space_boundaries,
            owner_histories,
            georeference,
            coverage,
        })
    }

    /// The views of a file whose schema has no tables here: nothing is read, everything is kept.
    fn import_without_tables(&mut self) -> Result<ImportViews> {
        let coverage = self.coverage()?;
        Ok(ImportViews {
            coverage,
            ..ImportViews::default()
        })
    }
}
