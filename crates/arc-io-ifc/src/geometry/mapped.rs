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

use std::sync::Arc;

use crate::error::reasons;
use crate::model::Record;

use super::attr::{self, skip};
use super::place::expect;
use super::{Evaluator, Piece, Step};

impl Evaluator<'_> {
    /// The pieces of a mapped item: the pieces of the mapped representation's items, transformed.
    pub(crate) fn mapped(&mut self, record: &Record, depth: u32) -> Step<Vec<Piece>> {
        let kind = "IFCMAPPEDITEM";
        let file = self.file;
        let source = attr::reference(record.parameters.first(), kind)?;
        let target = attr::reference(record.parameters.get(1), kind)?;
        let map = expect(file, source, "IFCREPRESENTATIONMAP")?;
        let origin = self.axis_any(attr::opt_reference(
            map.parameters.first(),
            "IFCREPRESENTATIONMAP",
        )?)?;
        let representation = attr::reference(map.parameters.get(1), "IFCREPRESENTATIONMAP")?;
        let rep = file
            .instance(representation)
            .and_then(|i| i.records.first())
            .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCREPRESENTATION"))?;
        let (items, _) = attr::references(rep.parameters.get(3), "IFCREPRESENTATION")?;
        let placement = self.operator(target)? * origin;
        let mut out = Vec::new();
        for item in items {
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
