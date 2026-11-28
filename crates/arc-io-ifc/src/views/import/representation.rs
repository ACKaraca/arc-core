//! The representation references of the objects: the items of a product's own representations,
//! split into the bodies and the `IfcMappedItem`s, and the representation maps of a type object.
//!
//! The geometry is evaluated by [`crate::IfcFile::geometry`]. The views give the references and,
//! for a mapped item, the transform from the map's coordinates to the product's placement
//! (`MappingTarget` composed with `MappingOrigin`), so that an importer can place a type's shared
//! representation without evaluating it. Only the product's own representations are followed; the
//! items of a map are not walked here, and a map's own mapped items are left to the geometry
//! evaluator.

use arc_base::Result;

use crate::error::reasons;
use crate::model::Record;

use super::super::Reader;
use super::MappedView;
use super::placement::mul;

/// Estimated bytes of a mapped item view.
const MAPPED_BYTES: u64 = 128;

impl Reader<'_> {
    /// The bodies and the mapped items of the product's own representations, in file order.
    pub(super) fn representation_items(
        &mut self,
        record: &Record,
        name: u64,
    ) -> Result<(Vec<u64>, Vec<MappedView>)> {
        let mut bodies = Vec::new();
        let mut mapped = Vec::new();
        let Some(shape) = self.reference(record, "Representation", name) else {
            return Ok((bodies, mapped));
        };
        let Some(shape_record) = self.rec(shape) else {
            return Ok((bodies, mapped));
        };
        for representation in self.references_of(shape_record, "Representations", shape) {
            self.cancel.check()?;
            let Some(rep) = self.rec(representation) else {
                continue;
            };
            for item in self.references_of(rep, "Items", representation) {
                if !self.file.contains(item) {
                    self.lose(
                        reasons::TARGET_MISSING,
                        rep.type_name.as_str(),
                        representation,
                        1,
                    );
                    continue;
                }
                self.link()?;
                if self.is_a(item, "IFCMAPPEDITEM") {
                    self.fixed(MAPPED_BYTES)?;
                    mapped.push(self.mapped_item(item));
                } else {
                    bodies.push(item);
                }
            }
        }
        Ok((bodies, mapped))
    }

    /// An `IfcMappedItem`: its map, the map's representation and type object, and the transform.
    fn mapped_item(&mut self, item: u64) -> MappedView {
        let mut view = MappedView {
            instance: item,
            map: None,
            type_object: None,
            representation: None,
            transform: None,
        };
        let Some(record) = self.rec(item) else {
            self.lose(reasons::TARGET_MISSING, "IFCMAPPEDITEM", item, 1);
            return view;
        };
        let Some(map) = self.reference(record, "MappingSource", item) else {
            self.lose(reasons::TARGET_MISSING, "IFCMAPPEDITEM", item, 1);
            return view;
        };
        view.map = Some(map);
        view.type_object = self
            .referrers_through(map, "IFCTYPEPRODUCT", "RepresentationMaps")
            .first()
            .copied();
        let Some(map_record) = self.rec(map) else {
            self.lose(reasons::TARGET_MISSING, "IFCMAPPEDITEM", item, 1);
            return view;
        };
        view.representation = self.reference(map_record, "MappedRepresentation", map);
        let origin = self.reference(map_record, "MappingOrigin", map);
        let target = self.reference(record, "MappingTarget", item);
        let (Some(origin), Some(target)) = (origin, target) else {
            self.lose(reasons::TARGET_MISSING, "IFCMAPPEDITEM", item, 1);
            return view;
        };
        let frame = self.axis_frame(origin, map, "IFCREPRESENTATIONMAP");
        let operator = self.operator(target, item);
        view.transform = frame
            .zip(operator)
            .map(|(frame, operator)| mul(&operator, &frame));
        view
    }

    /// The `IfcRepresentationMap`s of a type object's `RepresentationMaps`, in file order.
    pub(super) fn representation_maps(&mut self, record: &Record, name: u64) -> Vec<u64> {
        self.references_of(record, "RepresentationMaps", name)
    }
}
