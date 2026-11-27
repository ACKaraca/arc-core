//! Material associations: a material, a list, a layer set (or usage), a profile set (or usage) and
//! a constituent set, with the layers' thicknesses in meters.

use arc_base::Result;

use crate::error::reasons;
use crate::model::Record;

use super::super::Reader;
use super::{
    ConstituentView, LayerUsage, LayerView, MaterialAssociation, MaterialRef, ProfileView,
};

impl Reader<'_> {
    /// The association `instance` names (`RelatingMaterial`); `None` when it is not a material
    /// definition the views read, which is logged.
    pub(super) fn material_association(
        &mut self,
        instance: u64,
    ) -> Result<Option<MaterialAssociation>> {
        self.cancel.check()?;
        if !self.imp.associations.contains_key(&instance) {
            let read = self.read_association(instance)?;
            self.imp.associations.insert(instance, read);
        }
        let found = self.imp.associations.get(&instance).cloned().flatten();
        // Each object that uses the association holds its own copy.
        if let Some(found) = &found {
            self.fixed(found.estimated_bytes())?;
        }
        Ok(found)
    }

    fn read_association(&mut self, instance: u64) -> Result<Option<MaterialAssociation>> {
        let Some(record) = self.rec(instance) else {
            return Ok(None);
        };
        let kind = record.type_name.as_str();
        let found = match kind {
            "IFCMATERIAL" => self.material(instance).map(MaterialAssociation::Single),
            "IFCMATERIALLIST" => {
                let mut list = Vec::new();
                for m in self.references_of(record, "Materials", instance) {
                    list.extend(self.material(m));
                }
                Some(MaterialAssociation::List(list))
            }
            "IFCMATERIALLAYERSET" => self.layer_set(instance, None),
            "IFCMATERIALLAYERSETUSAGE" => {
                let set = self.reference(record, "ForLayerSet", instance);
                let usage = LayerUsage {
                    instance,
                    direction: self.enumeration(record, "LayerSetDirection", instance),
                    sense: self.enumeration(record, "DirectionSense", instance),
                    offset: self
                        .real(record, "OffsetFromReferenceLine", instance)
                        .map(|v| v * self.imp.units.length_to_meters),
                    reference_extent: self
                        .real(record, "ReferenceExtent", instance)
                        .map(|v| v * self.imp.units.length_to_meters),
                };
                set.and_then(|s| self.layer_set(s, Some(usage)))
            }
            "IFCMATERIALPROFILESET" => self.profile_set(instance, None, None),
            "IFCMATERIALPROFILESETUSAGE" | "IFCMATERIALPROFILESETUSAGETAPERING" => {
                let set = self.reference(record, "ForProfileSet", instance);
                let cardinal = self.integer(record, "CardinalPoint", instance);
                let extent = self
                    .real(record, "ReferenceExtent", instance)
                    .map(|v| v * self.imp.units.length_to_meters);
                set.and_then(|s| self.profile_set(s, cardinal, extent))
            }
            "IFCMATERIALCONSTITUENTSET" => self.constituent_set(instance),
            other => {
                self.lose(reasons::NOT_MAPPED, other, instance, 1);
                None
            }
        };
        Ok(found)
    }

    /// An `IfcMaterial`, with the styles of its definition representation.
    pub(super) fn material(&mut self, instance: u64) -> Option<MaterialRef> {
        let record = self.rec(instance)?;
        if record.type_name != "IFCMATERIAL" {
            self.lose(reasons::NOT_MAPPED, record.type_name.as_str(), instance, 1);
            return None;
        }
        Some(MaterialRef {
            instance,
            name: self.text(record, "Name", instance),
            description: self.text(record, "Description", instance),
            category: self.text(record, "Category", instance),
            styles: self.material_styles(instance),
        })
    }

    fn material_of(&mut self, record: &Record, instance: u64) -> Option<MaterialRef> {
        let m = self.reference(record, "Material", instance)?;
        self.material(m)
    }

    fn layer_set(
        &mut self,
        instance: u64,
        usage: Option<LayerUsage>,
    ) -> Option<MaterialAssociation> {
        let record = self.rec(instance)?;
        let name = self.text(record, "LayerSetName", instance);
        let mut layers = Vec::new();
        for layer in self.references_of(record, "MaterialLayers", instance) {
            let Some(lr) = self.rec(layer) else {
                self.lose(reasons::TARGET_MISSING, "IFCMATERIALLAYERSET", instance, 1);
                continue;
            };
            let scale = self.imp.units.length_to_meters;
            let thickness = self.real(lr, "LayerThickness", layer).map(|t| t * scale);
            let layer_view = LayerView {
                instance: layer,
                material: self.material_of(lr, layer),
                thickness,
                ventilated: self.flag(lr, "IsVentilated", layer),
                name: self.text(lr, "Name", layer),
                category: self.text(lr, "Category", layer),
                priority: self.integer(lr, "Priority", layer),
            };
            layers.push(layer_view);
        }
        Some(MaterialAssociation::LayerSet {
            instance,
            name,
            layers,
            usage,
        })
    }

    fn profile_set(
        &mut self,
        instance: u64,
        cardinal_point: Option<i64>,
        reference_extent: Option<f64>,
    ) -> Option<MaterialAssociation> {
        let record = self.rec(instance)?;
        let name = self.text(record, "Name", instance);
        let mut profiles = Vec::new();
        for profile in self.references_of(record, "MaterialProfiles", instance) {
            let Some(pr) = self.rec(profile) else {
                self.lose(
                    reasons::TARGET_MISSING,
                    "IFCMATERIALPROFILESET",
                    instance,
                    1,
                );
                continue;
            };
            let view = ProfileView {
                instance: profile,
                name: self.text(pr, "Name", profile),
                material: self.material_of(pr, profile),
                profile: self.reference(pr, "Profile", profile),
                priority: self.integer(pr, "Priority", profile),
                category: self.text(pr, "Category", profile),
            };
            profiles.push(view);
        }
        Some(MaterialAssociation::ProfileSet {
            instance,
            name,
            profiles,
            cardinal_point,
            reference_extent,
        })
    }

    fn constituent_set(&mut self, instance: u64) -> Option<MaterialAssociation> {
        let record = self.rec(instance)?;
        let name = self.text(record, "Name", instance);
        let mut constituents = Vec::new();
        for constituent in self.references_of(record, "MaterialConstituents", instance) {
            let Some(cr) = self.rec(constituent) else {
                self.lose(
                    reasons::TARGET_MISSING,
                    "IFCMATERIALCONSTITUENTSET",
                    instance,
                    1,
                );
                continue;
            };
            let view = ConstituentView {
                instance: constituent,
                name: self.text(cr, "Name", constituent),
                material: self.material_of(cr, constituent),
                fraction: self.real(cr, "Fraction", constituent),
                category: self.text(cr, "Category", constituent),
            };
            constituents.push(view);
        }
        Some(MaterialAssociation::ConstituentSet {
            instance,
            name,
            constituents,
        })
    }
}
