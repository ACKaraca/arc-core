//! `IfcMappedItem` (CORE-IO-015): a representation map placed by a cartesian transformation
//! operator.
//!
//! Attributes: `MappingSource` 0 (an `IfcRepresentationMap`) and `MappingTarget` 1 (an
//! `IfcCartesianTransformationOperator3D`, or its non-uniform form). The map has `MappingOrigin` 0
//! (an axis placement) and `MappedRepresentation` 1; the items of that representation are in the
//! map's coordinates. An item of the mapped representation maps to the representation of the
//! mapped item by `MappingTarget` composed with `MappingOrigin`.
//!
//! The pieces of the mapped representation's items are not copied: each piece keeps its shape,
//! shared through `Arc`, and receives the composed transform. A map that reaches its own mapped
//! item again is cut by the cycle check of [`Evaluator::item`]; nesting shares the depth limit.

use crate::schema::access::known;
use std::sync::Arc;

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, skip};
use super::place::expect;
use super::{Evaluator, Piece, Step};

impl Evaluator<'_> {
    /// The pieces of the mapped item `name`: the pieces of the mapped representation's items,
    /// transformed by the target operator composed with the mapping origin.
    pub(crate) fn mapped(&mut self, name: u64, record: &Record, depth: u32) -> Step<Vec<Piece>> {
        let kind = "IFCMAPPEDITEM";
        let file = self.file;
        let source = attr::reference(file.attr(record, known::MAPPING_SOURCE), kind)?;
        let target = attr::reference(file.attr(record, known::MAPPING_TARGET), kind)?;
        let map = expect(file, source, "IFCREPRESENTATIONMAP")?;
        let origin = self.axis_any(attr::opt_reference(
            file.attr(map, known::MAPPING_ORIGIN),
            "IFCREPRESENTATIONMAP",
        )?)?;
        let representation = attr::reference(
            file.attr(map, known::MAPPED_REPRESENTATION),
            "IFCREPRESENTATIONMAP",
        )?;
        let rep = file
            .instance(representation)
            .and_then(|i| i.records.first())
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCREPRESENTATION"))?;
        let (items, _) = attr::distinct_references(
            &mut self.budget,
            file.attr(rep, known::REPRESENTATION_ITEMS),
            "IFCREPRESENTATION",
            usize::MAX,
        )?;
        let (operator, approximated) = self.operator(target)?;
        if approximated {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        let placement = operator * origin;
        let mut out = Vec::new();
        for item in items {
            self.budget.step()?;
            let Some(pieces) = self.item(item, depth.saturating_add(1))? else {
                continue;
            };
            for piece in pieces.iter() {
                self.charge_piece()?;
                out.push(Piece {
                    shape: Arc::clone(&piece.shape),
                    transform: placement * piece.transform,
                });
            }
        }
        Ok(out)
    }
}
