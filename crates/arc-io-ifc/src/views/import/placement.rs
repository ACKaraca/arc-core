//! Object placements resolved to world transforms.
//!
//! An `IfcLocalPlacement` chain is walked iteratively, bounded by the nesting limit, with a visited
//! set; the result of every placement is kept, so a shared parent is resolved once. A chain that
//! meets an `IfcGridPlacement` or an `IfcLinearPlacement` stops there: the transform is the part up
//! to it and the state names the entity that was not evaluated. Axis placements follow the
//! schema: an absent `Axis` is the z axis, an absent `RefDirection` is the first of the x and y
//! axes not parallel to `Axis`, orthogonalized against it.

use std::collections::BTreeSet;

use crate::error::reasons;
use crate::model::{Parameter, Record};

use super::super::Reader;
use super::{Matrix, PlacementState, PlacementView};

const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// The product of two row-major transforms.
pub(super) fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for col in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a.get(row * 4 + k).copied().unwrap_or(0.0)
                    * b.get(k * 4 + col).copied().unwrap_or(0.0);
            }
            if let Some(slot) = out.get_mut(row * 4 + col) {
                *slot = sum;
            }
        }
    }
    out
}

type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The unit vector, or `None` for a zero or non-finite vector.
fn unit(v: V3) -> Option<V3> {
    let n = dot(v, v).sqrt();
    (n > 1e-12 && n.is_finite()).then(|| [v[0] / n, v[1] / n, v[2] / n])
}

/// `v` made orthogonal to the unit vector `z`, as a unit vector.
fn orthogonal(v: V3, z: V3) -> Option<V3> {
    let d = dot(v, z);
    unit([v[0] - z[0] * d, v[1] - z[1] * d, v[2] - z[2] * d])
}

/// The frame with the axes `x_hint` and `z` (either may be absent) at `origin`.
fn frame(origin: V3, z: Option<V3>, x_hint: Option<V3>) -> Matrix {
    let z = z.and_then(unit).unwrap_or([0.0, 0.0, 1.0]);
    let x = x_hint
        .and_then(|x| orthogonal(x, z))
        .or_else(|| orthogonal([1.0, 0.0, 0.0], z))
        .or_else(|| orthogonal([0.0, 1.0, 0.0], z))
        .unwrap_or([1.0, 0.0, 0.0]);
    let y = cross(z, x);
    [
        x[0], y[0], z[0], origin[0], //
        x[1], y[1], z[1], origin[1], //
        x[2], y[2], z[2], origin[2], //
        0.0, 0.0, 0.0, 1.0,
    ]
}

/// The row-major transform of an operator: `x` made orthogonal to `z`, `y` made orthogonal to both
/// (the z axis times x when absent), each axis scaled, and the origin. `None` for a degenerate
/// axis; the geometry evaluator substitutes a perpendicular there, which is not repeated.
fn operator_matrix(origin: V3, x: V3, y: Option<V3>, z: V3, scales: [f64; 3]) -> Option<Matrix> {
    let z = unit(z)?;
    let x = orthogonal(x, z)?;
    let y = match y {
        None => cross(z, x),
        Some(v) => {
            let (dz, dx) = (dot(v, z), dot(v, x));
            unit([
                v[0] - z[0] * dz - x[0] * dx,
                v[1] - z[1] * dz - x[1] * dx,
                v[2] - z[2] * dz - x[2] * dx,
            ])?
        }
    };
    let [sx, sy, sz] = scales;
    Some([
        x[0] * sx,
        y[0] * sy,
        z[0] * sz,
        origin[0], //
        x[1] * sx,
        y[1] * sy,
        z[1] * sz,
        origin[1], //
        x[2] * sx,
        y[2] * sy,
        z[2] * sz,
        origin[2], //
        0.0,
        0.0,
        0.0,
        1.0,
    ])
}

/// The numbers of a list parameter, padded with zeros to three.
fn numbers(parameter: Option<&Parameter>) -> Option<V3> {
    let Some(Parameter::List(items)) = parameter else {
        return None;
    };
    let mut out = [0.0; 3];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = match item {
            Parameter::Real(r) => *r,
            #[allow(clippy::cast_precision_loss, reason = "a coordinate")]
            Parameter::Integer(i) => *i as f64,
            _ => return None,
        };
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

impl Reader<'_> {
    /// The world placement of an `IfcObjectPlacement`; `None` when the instance is missing.
    pub(super) fn placement(&mut self, start: u64) -> Option<PlacementView> {
        let view = |(world, state): &(Matrix, PlacementState)| PlacementView {
            instance: start,
            world: *world,
            state: state.clone(),
        };
        if let Some(found) = self.imp.placed.get(&start) {
            return Some(view(found));
        }
        if !self.file.contains(start) {
            self.lose(reasons::TARGET_MISSING, "IFCLOCALPLACEMENT", start, 1);
            return None;
        }
        let mut chain: Vec<(u64, &Record)> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut base: (Matrix, PlacementState) = (IDENTITY, PlacementState::Resolved);
        let mut current = Some(start);
        while let Some(p) = current.take() {
            if let Some(found) = self.imp.placed.get(&p) {
                base = found.clone();
                break;
            }
            let too_deep = u32::try_from(chain.len()).map_or(true, |n| n >= self.depth_limit);
            if too_deep || !seen.insert(p) {
                self.lose(reasons::CYCLE_CUT, "IFCLOCALPLACEMENT", p, 1);
                base = (IDENTITY, PlacementState::Cut);
                break;
            }
            let Some(record) = self.rec(p) else {
                self.lose(reasons::TARGET_MISSING, "IFCLOCALPLACEMENT", start, 1);
                break;
            };
            if record.type_name != "IFCLOCALPLACEMENT" {
                self.lose(reasons::NOT_MAPPED, record.type_name.as_str(), p, 1);
                base = (
                    IDENTITY,
                    PlacementState::NotResolved(record.type_name.clone()),
                );
                break;
            }
            chain.push((p, record));
            current = self.reference(record, "PlacementRelTo", p);
        }
        for (p, record) in chain.into_iter().rev() {
            let local = self.relative_placement(record, p);
            base.0 = mul(&base.0, &local);
            self.imp.placed.insert(p, base.clone());
        }
        self.imp.placed.insert(start, base.clone());
        Some(view(&base))
    }

    /// The `RelativePlacement` of a local placement, translation in meters.
    fn relative_placement(&mut self, record: &Record, source: u64) -> Matrix {
        let Some(axis) = self.reference(record, "RelativePlacement", source) else {
            return IDENTITY;
        };
        self.axis_frame(axis, source, "IFCLOCALPLACEMENT")
            .unwrap_or(IDENTITY)
    }

    /// The frame of an `IfcAxis2Placement2D` or `IfcAxis2Placement3D`, translation in meters. An
    /// instance that is missing is reported under `kind`, the entity that refers to it.
    pub(super) fn axis_frame(&mut self, axis: u64, source: u64, kind: &str) -> Option<Matrix> {
        let Some(axis_record) = self.rec(axis) else {
            self.lose(reasons::TARGET_MISSING, kind, source, 1);
            return None;
        };
        let scale = self.imp.units.length_to_meters;
        let origin = self
            .reference(axis_record, "Location", axis)
            .and_then(|p| self.rec(p))
            .and_then(|p| numbers(self.file.attribute(p, "Coordinates")))
            .map_or([0.0; 3], |c| [c[0] * scale, c[1] * scale, c[2] * scale]);
        let z = match axis_record.type_name.as_str() {
            "IFCAXIS2PLACEMENT3D" => self.direction(axis_record, "Axis", axis),
            _ => None,
        };
        let x = self.direction(axis_record, "RefDirection", axis);
        Some(frame(origin, z, x))
    }

    fn direction(&mut self, record: &Record, attr: &str, source: u64) -> Option<V3> {
        let target = self.reference(record, attr, source)?;
        let direction = self.rec(target)?;
        numbers(self.file.attribute(direction, "DirectionRatios"))
    }

    /// `Some(None)` when the attribute is absent, `Some(Some(name))` for a reference, and `None`
    /// when it is present but not a reference (logged by [`Reader::reference`]).
    fn optional_reference(
        &mut self,
        record: &Record,
        attr: &str,
        source: u64,
    ) -> Option<Option<u64>> {
        match self.file.attribute(record, attr) {
            None | Some(Parameter::Omitted | Parameter::Derived) => Some(None),
            Some(_) => self.reference(record, attr, source).map(Some),
        }
    }

    /// An optional direction attribute, with the same three states as [`Self::optional_reference`].
    fn optional_direction(
        &mut self,
        record: &Record,
        attr: &str,
        source: u64,
        kind: &str,
    ) -> Option<Option<V3>> {
        let Some(target) = self.optional_reference(record, attr, source)? else {
            return Some(None);
        };
        let Some(direction) = self.rec(target) else {
            self.lose(reasons::TARGET_MISSING, kind, source, 1);
            return None;
        };
        match numbers(self.file.attribute(direction, "DirectionRatios")).and_then(unit) {
            Some(v) => Some(Some(v)),
            None => {
                self.lose(reasons::VALUE_UNEXPECTED, kind, source, 1);
                None
            }
        }
    }

    /// An optional real attribute; the default when it is absent.
    fn optional_real(
        &mut self,
        record: &Record,
        attr: &str,
        source: u64,
        default: f64,
    ) -> Option<f64> {
        match self.file.attribute(record, attr) {
            None | Some(Parameter::Omitted | Parameter::Derived) => Some(default),
            Some(_) => self.real(record, attr, source),
        }
    }

    /// The transform of an `IfcCartesianTransformationOperator3D` or of its non-uniform form, in
    /// meters, as `IfcFile::geometry` evaluates it: the axes orthogonalized in the same order, the
    /// scale of each axis, and the local origin. `None` when an attribute is unusable; the loss
    /// report names the reason.
    pub(super) fn operator(&mut self, name: u64, source: u64) -> Option<Matrix> {
        let Some(record) = self.rec(name) else {
            self.lose(
                reasons::TARGET_MISSING,
                "IFCCARTESIANTRANSFORMATIONOPERATOR3D",
                source,
                1,
            );
            return None;
        };
        let kind = record.type_name.as_str();
        let non_uniform = match kind {
            "IFCCARTESIANTRANSFORMATIONOPERATOR3D" => false,
            "IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM" => true,
            _ => {
                self.lose(reasons::NOT_MAPPED, kind, source, 1);
                return None;
            }
        };
        let z = self
            .optional_direction(record, "Axis3", name, kind)?
            .unwrap_or([0.0, 0.0, 1.0]);
        let x = self
            .optional_direction(record, "Axis1", name, kind)?
            .unwrap_or([1.0, 0.0, 0.0]);
        let y = self.optional_direction(record, "Axis2", name, kind)?;
        let origin = match self.optional_reference(record, "LocalOrigin", name)? {
            None => [0.0; 3],
            Some(point) => {
                let scale = self.imp.units.length_to_meters;
                let coordinates = self
                    .rec(point)
                    .and_then(|p| numbers(self.file.attribute(p, "Coordinates")));
                match coordinates {
                    Some(c) => [c[0] * scale, c[1] * scale, c[2] * scale],
                    None => {
                        self.lose(reasons::TARGET_MISSING, kind, name, 1);
                        return None;
                    }
                }
            }
        };
        let uniform = self.optional_real(record, "Scale", name, 1.0)?;
        let (second, third) = if non_uniform {
            (
                self.optional_real(record, "Scale2", name, uniform)?,
                self.optional_real(record, "Scale3", name, uniform)?,
            )
        } else {
            (uniform, uniform)
        };
        let scales = [uniform, second, third];
        match operator_matrix(origin, x, y, z, scales) {
            Some(matrix) if scales.iter().all(|s| *s > 0.0 && s.is_finite()) => Some(matrix),
            _ => {
                self.lose(reasons::VALUE_UNEXPECTED, kind, name, 1);
                None
            }
        }
    }
}
