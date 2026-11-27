//! Reading curves from the instances of a file.

use std::collections::BTreeMap;

use arc_base::{Cancel, Error, Limits, Result};

use super::curve::{Cant, CantKind, CantPiece, Curve, Gradient, Plan, Seg};
use super::parent::{Parent, Spiral};
use crate::error::codes;
use crate::model::{IfcFile, Parameter, Record};

/// The most references the evaluator follows from one curve to its base curves.
const MAX_BASE_DEPTH: u32 = 4;

pub(crate) fn invalid() -> Error {
    Error::new(codes::IFC_CURVE_INVALID)
}

pub(crate) fn unsupported(kind: &str) -> Error {
    Error::new(codes::IFC_CURVE_UNSUPPORTED).with_arg("kind", kind.to_owned())
}

/// The number of a real or integer, bare or wrapped in a measure type.
pub(crate) fn number(parameter: &Parameter) -> Option<f64> {
    match parameter {
        Parameter::Real(real) => Some(*real).filter(|r| r.is_finite()),
        Parameter::Integer(integer) => Some(*integer as f64),
        Parameter::Typed { value, .. } => number(value),
        _ => None,
    }
}

fn present(parameter: Option<&Parameter>) -> Option<&Parameter> {
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => None,
        other => other,
    }
}

/// Reads the instances of one file for the evaluator.
pub(crate) struct Reader<'f> {
    pub(crate) file: &'f IfcFile,
    pub(crate) limits: Limits,
    pub(crate) cancel: Cancel,
    /// Meters per length unit.
    pub(crate) scale: f64,
    /// Radians per plane angle unit.
    pub(crate) angle: f64,
    nests: Option<BTreeMap<u64, Vec<u64>>>,
    aggregates: Option<BTreeMap<u64, Vec<u64>>>,
}

impl<'f> Reader<'f> {
    pub(crate) fn new(
        file: &'f IfcFile,
        limits: &Limits,
        cancel: &Cancel,
        scale: f64,
        angle: f64,
    ) -> Self {
        Self {
            file,
            limits: *limits,
            cancel: cancel.clone(),
            scale,
            angle,
            nests: None,
            aggregates: None,
        }
    }

    /// The first record of an instance whose type is one of `types`.
    pub(crate) fn record(&self, name: u64, types: &[&str]) -> Result<&'f Record> {
        let record = self
            .file
            .instance(name)
            .and_then(|i| i.records.first())
            .ok_or_else(invalid)?;
        if types.is_empty() || types.contains(&record.type_name.as_str()) {
            Ok(record)
        } else {
            Err(unsupported(&record.type_name))
        }
    }

    /// The value of the attribute `name` of a record; `None` when the record does not have it or
    /// holds no value.
    pub(crate) fn opt(&self, record: &'f Record, name: &str) -> Option<&'f Parameter> {
        present(self.file.attribute(record, name))
    }

    pub(crate) fn req(&self, record: &'f Record, name: &str) -> Result<&'f Parameter> {
        self.opt(record, name).ok_or_else(invalid)
    }

    pub(crate) fn real(&self, record: &'f Record, name: &str) -> Result<f64> {
        number(self.req(record, name)?).ok_or_else(invalid)
    }

    pub(crate) fn opt_real(&self, record: &'f Record, name: &str) -> Result<Option<f64>> {
        match self.opt(record, name) {
            None => Ok(None),
            Some(p) => number(p).map(Some).ok_or_else(invalid),
        }
    }

    pub(crate) fn reference(&self, record: &'f Record, name: &str) -> Result<u64> {
        match self.req(record, name)? {
            Parameter::Reference(n) => Ok(*n),
            _ => Err(invalid()),
        }
    }

    pub(crate) fn opt_reference(&self, record: &'f Record, name: &str) -> Result<Option<u64>> {
        match self.opt(record, name) {
            None => Ok(None),
            Some(Parameter::Reference(n)) => Ok(Some(*n)),
            Some(_) => Err(invalid()),
        }
    }

    /// The references of a list, bounded by `array_length`.
    pub(crate) fn references(&self, record: &'f Record, name: &str) -> Result<Vec<u64>> {
        let Parameter::List(items) = self.req(record, name)? else {
            return Err(invalid());
        };
        if u64::try_from(items.len()).unwrap_or(u64::MAX) > self.limits.array_length {
            return Err(crate::error::limit("array_length", items.len() as u64));
        }
        items
            .iter()
            .map(|p| match p {
                Parameter::Reference(n) => Ok(*n),
                _ => Err(invalid()),
            })
            .collect()
    }

    /// The reals of a list; absent gives an empty list.
    fn reals(&self, record: &'f Record, name: &str) -> Result<Vec<f64>> {
        match self.opt(record, name) {
            None => Ok(Vec::new()),
            Some(Parameter::List(items)) => {
                if u64::try_from(items.len()).unwrap_or(u64::MAX) > self.limits.array_length {
                    return Err(crate::error::limit("array_length", items.len() as u64));
                }
                items
                    .iter()
                    .map(|p| number(p).ok_or_else(invalid))
                    .collect()
            }
            Some(_) => Err(invalid()),
        }
    }

    /// The coordinates of a cartesian point or the ratios of a direction.
    pub(crate) fn coordinates(&self, name: u64) -> Result<Vec<f64>> {
        let record = self.record(name, &["IFCCARTESIANPOINT", "IFCDIRECTION"])?;
        let key = if record.type_name == "IFCDIRECTION" {
            "DirectionRatios"
        } else {
            "Coordinates"
        };
        self.reals(record, key)
    }

    /// The location, the heading of the reference direction and the axis of a 2D or 3D placement.
    pub(crate) fn placement(&self, name: u64) -> Result<([f64; 3], f64, [f64; 3])> {
        let record = self.record(name, &["IFCAXIS2PLACEMENT2D", "IFCAXIS2PLACEMENT3D"])?;
        let loc = self.coordinates(self.reference(record, "Location")?)?;
        let location = [
            *loc.first().ok_or_else(invalid)?,
            *loc.get(1).ok_or_else(invalid)?,
            loc.get(2).copied().unwrap_or(0.0),
        ];
        let phi = match self.opt_reference(record, "RefDirection")? {
            Some(r) => {
                let d = self.coordinates(r)?;
                let (x, y) = (
                    *d.first().ok_or_else(invalid)?,
                    *d.get(1).ok_or_else(invalid)?,
                );
                if x == 0.0 && y == 0.0 {
                    return Err(invalid());
                }
                y.atan2(x)
            }
            None => 0.0,
        };
        let axis = if record.type_name == "IFCAXIS2PLACEMENT3D" {
            match self.opt_reference(record, "Axis")? {
                Some(r) => {
                    let d = self.coordinates(r)?;
                    [
                        d.first().copied().unwrap_or(0.0),
                        d.get(1).copied().unwrap_or(0.0),
                        d.get(2).copied().unwrap_or(0.0),
                    ]
                }
                None => [0.0, 0.0, 1.0],
            }
        } else {
            [0.0, 0.0, 1.0]
        };
        Ok((location, phi, axis))
    }

    /// The related objects of the `IfcRelNests` or `IfcRelAggregates` of `relating`.
    pub(crate) fn related(&mut self, relation: &'static str, relating: u64) -> Vec<u64> {
        let map = if relation == "IFCRELNESTS" {
            &mut self.nests
        } else {
            &mut self.aggregates
        };
        if map.is_none() {
            let mut built: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
            for instance in self.file.instances_of(relation) {
                let Some(record) = instance.records.first() else {
                    continue;
                };
                let (Some(Parameter::Reference(parent)), Some(Parameter::List(items))) = (
                    self.file.attribute(record, "RelatingObject"),
                    self.file.attribute(record, "RelatedObjects"),
                ) else {
                    continue;
                };
                let entry = built.entry(*parent).or_default();
                for item in items {
                    if let Parameter::Reference(n) = item {
                        entry.push(*n);
                    }
                }
            }
            *map = Some(built);
        }
        map.as_ref()
            .and_then(|m| m.get(&relating))
            .cloned()
            .unwrap_or_default()
    }

    /// The parent curve of a segment of the length `len`.
    fn parent(&self, name: u64, len: f64) -> Result<Parent> {
        let record = self.record(name, &[])?;
        let term = |attr: &str| -> Result<Option<f64>> {
            Ok(self.opt_real(record, attr)?.filter(|a| *a != 0.0))
        };
        // Coefficient of s^k of the curvature for the term A_k of a polynomial spiral.
        let coefficient = |k: u32, a: Option<f64>| -> f64 {
            a.map_or(0.0, |a| {
                let power = i32::try_from(k + 1).unwrap_or(1);
                if k.is_multiple_of(2) {
                    a.powi(power).recip()
                } else {
                    a.signum() / a.abs().powi(power)
                }
            })
        };
        let polynomial = |names: &[&str]| -> Result<Parent> {
            let mut c = Vec::with_capacity(names.len());
            for (k, attr) in names.iter().rev().enumerate() {
                c.push(coefficient(u32::try_from(k).unwrap_or(0), term(attr)?));
            }
            Ok(Parent::Spiral(Spiral::Polynomial(c)))
        };
        let seg_len = if len == 0.0 { 1.0 } else { len.abs() };
        match record.type_name.as_str() {
            "IFCLINE" => Ok(Parent::Line),
            "IFCCIRCLE" => {
                let r = self.real(record, "Radius")?;
                if r > 0.0 {
                    Ok(Parent::Circle(r))
                } else {
                    Err(invalid())
                }
            }
            "IFCCLOTHOID" => {
                let a = self.real(record, "ClothoidConstant")?;
                if a == 0.0 {
                    return Err(invalid());
                }
                Ok(Parent::Spiral(Spiral::Polynomial(vec![
                    0.0,
                    a.signum() / (a * a),
                ])))
            }
            "IFCPOLYNOMIALCURVE" => {
                let x = self.reals(record, "CoefficientsX")?;
                let y = self.reals(record, "CoefficientsY")?;
                let z = self.reals(record, "CoefficientsZ")?;
                if x.is_empty() || !z.is_empty() {
                    return Err(unsupported("IFCPOLYNOMIALCURVE"));
                }
                // The segment start and length are lengths along the curve.
                Ok(Parent::Polynomial {
                    x,
                    y,
                    by_length: true,
                })
            }
            "IFCSECONDORDERPOLYNOMIALSPIRAL" => {
                polynomial(&["QuadraticTerm", "LinearTerm", "ConstantTerm"])
            }
            "IFCTHIRDORDERPOLYNOMIALSPIRAL" => {
                polynomial(&["CubicTerm", "QuadraticTerm", "LinearTerm", "ConstantTerm"])
            }
            "IFCSEVENTHORDERPOLYNOMIALSPIRAL" => polynomial(&[
                "SepticTerm",
                "SexticTerm",
                "QuinticTerm",
                "QuarticTerm",
                "CubicTerm",
                "QuadraticTerm",
                "LinearTerm",
                "ConstantTerm",
            ]),
            "IFCCOSINESPIRAL" => Ok(Parent::Spiral(Spiral::Cosine {
                c0: term("ConstantTerm")?.map_or(0.0, f64::recip),
                c1: term("CosineTerm")?.map_or(0.0, f64::recip),
                len: seg_len,
            })),
            "IFCSINESPIRAL" => {
                let a1 = term("LinearTerm")?;
                Ok(Parent::Spiral(Spiral::Sine {
                    c0: term("ConstantTerm")?.map_or(0.0, f64::recip),
                    t2: a1.map_or(0.0, |a| 0.5 * a.signum() / (a * a)),
                    t3: term("SineTerm")?
                        .map_or(0.0, |a| seg_len / (2.0 * std::f64::consts::PI * a)),
                    len: seg_len,
                }))
            }
            other => Err(unsupported(other)),
        }
    }

    /// A curve segment.
    pub(crate) fn segment(&self, name: u64) -> Result<Seg> {
        let record = self.record(name, &["IFCCURVESEGMENT"])?;
        let (loc, phi, axis) = self.placement(self.reference(record, "Placement")?)?;
        let s0 = self.real(record, "SegmentStart")?;
        let len = self.real(record, "SegmentLength")?;
        let parent_name = self.reference(record, "ParentCurve")?;
        let kind = self.record(parent_name, &[])?.type_name.clone();
        let parent = self.parent(parent_name, len)?;
        Ok(Seg {
            kind,
            parent,
            s0,
            len,
            loc,
            phi,
            axis,
        })
    }

    fn segments(&self, record: &'f Record) -> Result<Vec<Seg>> {
        let names = self.references(record, "Segments")?;
        if names.is_empty() {
            return Err(invalid());
        }
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            self.cancel.check()?;
            out.push(self.segment(name)?);
        }
        Ok(out)
    }

    /// The curve of an `IfcCompositeCurve`, `IfcGradientCurve` or `IfcSegmentedReferenceCurve`.
    pub(crate) fn curve(&self, name: u64) -> Result<Curve> {
        self.curve_at(name, 0)
    }

    fn curve_at(&self, name: u64, depth: u32) -> Result<Curve> {
        if depth > MAX_BASE_DEPTH {
            return Err(invalid());
        }
        let record = self.record(
            name,
            &[
                "IFCCOMPOSITECURVE",
                "IFCGRADIENTCURVE",
                "IFCSEGMENTEDREFERENCECURVE",
            ],
        )?;
        let scale = self.scale;
        match record.type_name.as_str() {
            "IFCCOMPOSITECURVE" => Ok(Curve {
                plan: Plan::new(self.segments(record)?),
                gradient: None,
                cant: None,
                scale,
            }),
            "IFCGRADIENTCURVE" => {
                let mut base = self.curve_at(self.reference(record, "BaseCurve")?, depth + 1)?;
                if base.gradient.is_some() {
                    return Err(invalid());
                }
                let segs = self.segments(record)?;
                let mut pieces = Vec::new();
                for (i, seg) in segs.iter().enumerate() {
                    if seg.extent() == 0.0 {
                        continue;
                    }
                    let x1 = match segs.get(i + 1) {
                        Some(next) => next.loc[0],
                        None => seg.local(seg.extent()).0[0],
                    };
                    pieces.push((seg.loc[0], x1, seg.clone()));
                }
                if pieces.is_empty() {
                    return Err(invalid());
                }
                base.gradient = Some(Gradient { pieces });
                Ok(base)
            }
            _ => {
                let mut base = self.curve_at(self.reference(record, "BaseCurve")?, depth + 1)?;
                if base.cant.is_some() {
                    return Err(invalid());
                }
                let segs = self.segments(record)?;
                let end = match self.opt_reference(record, "EndPoint")? {
                    Some(n) => Some(self.placement(n)?),
                    None => None,
                };
                let tilt_of = |axis: [f64; 3]| axis[1].atan2(axis[2]);
                let mut pieces = Vec::new();
                for (i, seg) in segs.iter().enumerate() {
                    if seg.extent() == 0.0 {
                        continue;
                    }
                    if matches!(seg.parent, Parent::Circle(_)) {
                        return Err(unsupported("cant segment parent curve"));
                    }
                    let (x1, dev_end, end_axis) = match (segs.get(i + 1), &end) {
                        (Some(next), _) => (next.loc[0], next.loc[1], next.axis),
                        (None, Some((loc, _, axis))) => (loc[0], loc[1], *axis),
                        (None, None) => (seg.loc[0] + seg.extent(), seg.loc[1], seg.axis),
                    };
                    pieces.push(CantPiece {
                        x0: seg.loc[0],
                        x1,
                        kind: CantKind::Reference {
                            seg: seg.clone(),
                            tilt: [tilt_of(seg.axis), tilt_of(end_axis)],
                            dev_end,
                        },
                    });
                }
                if pieces.is_empty() {
                    return Err(invalid());
                }
                base.cant = Some(Cant { pieces });
                Ok(base)
            }
        }
    }

    /// The curve a linear placement or an alignment is measured along: an alignment gives its
    /// axis representation, a curve itself.
    pub(crate) fn any_curve(&mut self, name: u64) -> Result<Curve> {
        let record = self.record(name, &[])?;
        if record.type_name == "IFCALIGNMENT" {
            return self.alignment_curve(name);
        }
        self.curve(name)
    }

    /// The curve of the axis representation of an alignment, or, when it has none, its layout.
    pub(crate) fn alignment_curve(&mut self, name: u64) -> Result<Curve> {
        if let Some(item) = self.axis_item(name)? {
            return self.curve(item);
        }
        self.layout_curve(name)
    }

    /// The most capable curve item of the axis representation of an alignment.
    pub(crate) fn axis_item(&self, name: u64) -> Result<Option<u64>> {
        let record = self.record(name, &["IFCALIGNMENT"])?;
        let Some(shape) = self.opt_reference(record, "Representation")? else {
            return Ok(None);
        };
        let shape = self.record(shape, &["IFCPRODUCTDEFINITIONSHAPE"])?;
        let mut best: Option<(u8, u64)> = None;
        for rep in self.references(shape, "Representations")? {
            let rep = self.record(rep, &["IFCSHAPEREPRESENTATION"])?;
            for item in self.references(rep, "Items")? {
                let rank = match self
                    .file
                    .instance(item)
                    .and_then(|i| i.records.first())
                    .map(|r| r.type_name.as_str())
                {
                    Some("IFCSEGMENTEDREFERENCECURVE") => 3,
                    Some("IFCGRADIENTCURVE") => 2,
                    Some("IFCCOMPOSITECURVE") => 1,
                    _ => 0,
                };
                if rank > best.map_or(0, |b| b.0) {
                    best = Some((rank, item));
                }
            }
        }
        Ok(best.map(|b| b.1))
    }
}
