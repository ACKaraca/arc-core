//! Curves as polylines (CORE-IO-015): `IfcPolyline` and `IfcIndexedPolyCurve` with line segments,
//! in 2D or 3D. The points are in meters. A polyline is one output shape with at least two points.
//!
//! `IfcIndexedPolyCurve`: `Points` 0 (an `IfcCartesianPointList2D` or `IfcCartesianPointList3D`) and
//! `Segments` 1, an optional list of `IfcLineIndex` (a list of one-based integers). Each segment
//! that is a line continues the polyline; a repeated join point is dropped. An `IfcArcIndex` is not
//! evaluated, so the curve is skipped and reported.

use arc_base::math::Point3;

use crate::error::reasons;
use crate::model::{Parameter, Record};

use super::attr::{self, skip};
use super::{Evaluator, Piece, Shape, Step};

impl Evaluator<'_> {
    /// The pieces of a polyline or indexed polyline curve.
    pub(crate) fn curve(&mut self, record: &Record) -> Step<Vec<Piece>> {
        let points = self.curve_points_of(record)?;
        if points.len() < 2 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, &record.type_name));
        }
        self.budget.array(points.len())?;
        self.budget.charge(
            u64::try_from(points.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(super::VERTEX_BYTES),
        )?;
        self.charge_shape()?;
        Ok(Self::own(Shape::Polylines(vec![points])))
    }

    /// The points of the curve named `name`, which may be a polyline or an indexed polyline.
    pub(crate) fn curve_points(&mut self, name: u64) -> Step<Vec<Point3>> {
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            return Err(skip(reasons::TARGET_MISSING, "IFCPOLYLINE"));
        };
        let Some(record) = instance.records.first() else {
            return Err(skip(reasons::TARGET_MISSING, "IFCPOLYLINE"));
        };
        self.curve_points_of(record)
    }

    fn curve_points_of(&mut self, record: &Record) -> Step<Vec<Point3>> {
        match record.type_name.as_str() {
            "IFCPOLYLINE" => {
                let kind = "IFCPOLYLINE";
                let (points, _) = attr::references(record.parameters.first(), kind)?;
                points.into_iter().map(|p| self.point(p)).collect()
            }
            "IFCINDEXEDPOLYCURVE" => self.indexed_polycurve(record),
            other => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
    }

    fn indexed_polycurve(&mut self, record: &Record) -> Step<Vec<Point3>> {
        let kind = "IFCINDEXEDPOLYCURVE";
        let list = attr::reference(record.parameters.first(), kind)?;
        let coordinates = self.point_list(list)?;
        let count = coordinates.len();
        let order = match record.parameters.get(1) {
            None | Some(Parameter::Omitted | Parameter::Derived) => (0..count).collect(),
            Some(Parameter::List(segments)) => self.segment_order(segments, count)?,
            Some(_) => return Err(attr::wrong(kind)),
        };
        order
            .into_iter()
            .map(|k| {
                coordinates
                    .get(k)
                    .copied()
                    .ok_or_else(|| skip(reasons::INDEX_INVALID, kind))
            })
            .collect()
    }

    /// The zero-based point indices of the segments in order, joined at repeated points.
    fn segment_order(&self, segments: &[Parameter], count: usize) -> Step<Vec<usize>> {
        let kind = "IFCINDEXEDPOLYCURVE";
        let mut order: Vec<usize> = Vec::new();
        for segment in segments {
            let (type_name, value) = match segment {
                Parameter::Typed { type_name, value } => (type_name.as_str(), value.as_ref()),
                Parameter::List(_) => ("IFCLINEINDEX", segment),
                _ => return Err(attr::wrong(kind)),
            };
            if type_name != "IFCLINEINDEX" {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, type_name));
            }
            let indices = attr::integers(Some(value), kind)?;
            for (position, one_based) in indices.into_iter().enumerate() {
                let zero_based = one_based
                    .checked_sub(1)
                    .and_then(|k| usize::try_from(k).ok())
                    .filter(|&k| k < count)
                    .ok_or_else(|| skip(reasons::INDEX_INVALID, kind))?;
                if position == 0 && order.last() == Some(&zero_based) {
                    continue;
                }
                order.push(zero_based);
            }
        }
        Ok(order)
    }
}

impl Evaluator<'_> {
    /// The points of an `IfcCartesianPointList2D` or `IfcCartesianPointList3D`, scaled to meters.
    /// The row count is checked against `array_length` and charged before the points are built.
    pub(crate) fn point_list(&mut self, name: u64) -> Step<Vec<Point3>> {
        let kind = "IFCCARTESIANPOINTLIST";
        let file = self.file;
        let record = file
            .instance(name)
            .and_then(|i| i.records.first())
            .ok_or_else(|| skip(reasons::TARGET_MISSING, kind))?;
        if record.type_name != "IFCCARTESIANPOINTLIST2D"
            && record.type_name != "IFCCARTESIANPOINTLIST3D"
        {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
        }
        let rows = attr::list(record.parameters.first(), kind)?;
        self.budget.array(rows.len())?;
        self.budget.charge(
            u64::try_from(rows.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(super::VERTEX_BYTES),
        )?;
        rows.iter()
            .map(|row| match row {
                Parameter::List(values) => attr::point3(values, self.scale, kind),
                _ => Err(attr::wrong(kind)),
            })
            .collect()
    }
}
