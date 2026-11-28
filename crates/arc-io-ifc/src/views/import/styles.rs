//! Surface styles of representation items (`IfcStyledItem`) and of materials
//! (`IfcMaterialDefinitionRepresentation`), read through the inverse index.
//!
//! The items of the product's representations are followed through `IfcMappedItem` to the mapped
//! representation, at most [`MAP_DEPTH`] levels deep. Styles attached deeper (an operand of a
//! boolean result, say) are not read; the item's instance stays in the coverage as preserved.

use std::collections::BTreeSet;

use arc_base::Result;

use crate::model::Record;
use crate::schema::Referrer;

use super::super::Reader;
use super::StyleView;

/// How many mapped representations are followed from a product's representation.
const MAP_DEPTH: u32 = 3;

impl Reader<'_> {
    /// The surface styles of the items of a product's representations.
    pub(super) fn object_styles(&mut self, record: &Record, name: u64) -> Result<Vec<StyleView>> {
        let mut styles = Vec::new();
        let mut visited = BTreeSet::new();
        let Some(shape) = self.reference(record, "Representation", name) else {
            return Ok(styles);
        };
        let mut representations = vec![(shape, 0_u32)];
        while let Some((current, depth)) = representations.pop() {
            self.cancel.check()?;
            if !visited.insert(current) {
                continue;
            }
            let Some(rec) = self.rec(current) else {
                continue;
            };
            // A product definition shape lists representations; a representation lists items.
            let nested = self.references_of(rec, "Representations", current);
            representations.extend(nested.into_iter().map(|r| (r, depth)));
            for item in self.references_of(rec, "Items", current) {
                self.item_styles(item, depth, &mut styles, &mut representations)?;
            }
        }
        Ok(styles)
    }

    fn item_styles(
        &mut self,
        item: u64,
        depth: u32,
        styles: &mut Vec<StyleView>,
        representations: &mut Vec<(u64, u32)>,
    ) -> Result<()> {
        for styled in self.referrers_through(item, "IFCSTYLEDITEM", "Item") {
            let found = self.styled_item_styles(styled);
            for style in found {
                self.fixed(96)?;
                styles.push(style);
            }
        }
        if depth < MAP_DEPTH && self.is_a(item, "IFCMAPPEDITEM") {
            let source = self
                .rec(item)
                .and_then(|r| self.reference(r, "MappingSource", item));
            let mapped = source
                .and_then(|s| self.rec(s))
                .and_then(|s| self.reference(s, "MappedRepresentation", item));
            if let Some(mapped) = mapped {
                representations.push((mapped, depth.saturating_add(1)));
            }
        }
        Ok(())
    }

    /// The instances of `entity` (or a subtype) that refer to `target` through `attr`.
    pub(super) fn referrers_through(&self, target: u64, entity: &str, attr: &str) -> Vec<u64> {
        let Some(tables) = self.file.tables() else {
            return Vec::new();
        };
        let Some(ancestor) = tables.entity_index(entity) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for &Referrer {
            instance,
            record,
            position,
        } in self.imp.index.referrers(target)
        {
            let Some(r) = self
                .file
                .instance(instance)
                .and_then(|i| i.records.get(usize::try_from(record).unwrap_or(usize::MAX)))
            else {
                continue;
            };
            let Some(e) = tables.entity_index(&r.type_name) else {
                continue;
            };
            let at = tables.attribute_position(e, attr).map(|(p, _)| p);
            if tables.is_subtype_of(e, ancestor)
                && at == Some(usize::try_from(position).unwrap_or(usize::MAX))
                && !out.contains(&instance)
            {
                out.push(instance);
            }
        }
        out
    }

    /// The surface styles of an `IfcStyledItem`.
    fn styled_item_styles(&mut self, styled: u64) -> Vec<StyleView> {
        let Some(record) = self.rec(styled) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for style in self.references_of(record, "Styles", styled) {
            let Some(sr) = self.rec(style) else { continue };
            // IFC2X3 and IFC4 wrap the styles in an assignment; IFC4 also allows them directly.
            let direct = if sr.type_name == "IFCPRESENTATIONSTYLEASSIGNMENT" {
                self.references_of(sr, "Styles", style)
            } else {
                vec![style]
            };
            for surface in direct {
                if let Some(view) = self.surface_style(surface, styled) {
                    out.push(view);
                }
            }
        }
        out
    }

    /// An `IfcSurfaceStyle`: the first shading or rendering element with a color.
    fn surface_style(&mut self, style: u64, owner: u64) -> Option<StyleView> {
        let record = self.rec(style)?;
        if record.type_name != "IFCSURFACESTYLE" {
            return None;
        }
        let name = self.text(record, "Name", style);
        let mut color = None;
        let mut transparency = None;
        for element in self.references_of(record, "Styles", style) {
            let Some(er) = self.rec(element) else {
                continue;
            };
            if !self.is_a(element, "IFCSURFACESTYLESHADING") {
                continue;
            }
            color = self
                .reference(er, "SurfaceColour", element)
                .and_then(|c| self.rec(c))
                .and_then(|c| {
                    Some([
                        self.real(c, "Red", style)?,
                        self.real(c, "Green", style)?,
                        self.real(c, "Blue", style)?,
                    ])
                });
            transparency = self.real(er, "Transparency", element);
            if color.is_some() {
                break;
            }
        }
        Some(StyleView {
            instance: owner,
            style,
            name,
            color,
            transparency,
        })
    }

    /// The styles of the material's `IfcMaterialDefinitionRepresentation`.
    pub(super) fn material_styles(&mut self, material: u64) -> Vec<StyleView> {
        let mut out = Vec::new();
        for definition in self.referrers_through(
            material,
            "IFCMATERIALDEFINITIONREPRESENTATION",
            "RepresentedMaterial",
        ) {
            let Some(dr) = self.rec(definition) else {
                continue;
            };
            for representation in self.references_of(dr, "Representations", definition) {
                let Some(rr) = self.rec(representation) else {
                    continue;
                };
                for item in self.references_of(rr, "Items", representation) {
                    out.extend(self.styled_item_styles(item));
                }
            }
        }
        out
    }
}
