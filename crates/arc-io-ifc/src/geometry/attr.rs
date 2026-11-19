//! Reads of attributes by position, with presence and type checks. A wrong type is a skip of the
//! item that holds the attribute, reported as `io.ifc.value_unexpected`.

use arc_base::math::{Point3, Vector2, Vector3};

use crate::model::{IfcFile, Parameter, Record};

use super::{Flow, Skip, Step};
use crate::error::reasons;

/// A skip of the item named by `kind` because of a value of the wrong type.
pub(crate) fn wrong(kind: &str) -> Flow {
    Flow::Skip(Skip {
        reason: reasons::VALUE_UNEXPECTED,
        kind: kind.to_owned(),
    })
}

/// A skip of the item named by `kind` for a reason in the loss catalog.
pub(crate) fn skip(reason: crate::loss::Reason, kind: &str) -> Flow {
    Flow::Skip(Skip {
        reason,
        kind: kind.to_owned(),
    })
}

/// The first record of an instance of the given type, or `None` when the instance is absent or
/// holds no record of that type.
pub(crate) fn typed<'a>(file: &'a IfcFile, name: u64, type_name: &str) -> Option<&'a Record> {
    file.instance(name)?
        .records
        .iter()
        .find(|record| record.type_name == type_name)
}

/// The first record of an instance, whatever its type.
pub(crate) fn typed_first(file: &IfcFile, name: u64) -> Option<&Record> {
    file.instance(name)?.records.first()
}

/// Whether a parameter holds no value (`$` or `*`).
fn absent(parameter: Option<&Parameter>) -> bool {
    matches!(
        parameter,
        None | Some(Parameter::Omitted | Parameter::Derived)
    )
}

/// The number of a real or integer, bare or wrapped in a measure type such as `IFCLENGTHMEASURE`.
fn number(parameter: &Parameter) -> Option<f64> {
    match parameter {
        Parameter::Real(real) => Some(*real),
        Parameter::Integer(integer) => {
            // Integers beyond 2^53 round to the nearest real, as in other IFC readers.
            Some(*integer as f64)
        }
        Parameter::Typed { value, .. } => number(value),
        _ => None,
    }
}

/// A required real value, scaled by `scale`; a non-finite result is a skip.
pub(crate) fn real(parameter: Option<&Parameter>, scale: f64, kind: &str) -> Step<f64> {
    let value = parameter.and_then(number).ok_or_else(|| wrong(kind))?;
    finite_scaled(value, scale, kind)
}

/// An optional real value, scaled by `scale`.
pub(crate) fn opt_real(parameter: Option<&Parameter>, scale: f64, kind: &str) -> Step<Option<f64>> {
    if absent(parameter) {
        return Ok(None);
    }
    real(parameter, scale, kind).map(Some)
}

/// A real value that is multiplied by `scale`; the product must be finite.
pub(crate) fn finite_scaled(value: f64, scale: f64, kind: &str) -> Step<f64> {
    let scaled = value * scale;
    if scaled.is_finite() {
        Ok(scaled)
    } else {
        Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
    }
}

/// A required reference to an instance.
pub(crate) fn reference(parameter: Option<&Parameter>, kind: &str) -> Step<u64> {
    match parameter {
        Some(Parameter::Reference(name)) => Ok(*name),
        _ => Err(wrong(kind)),
    }
}

/// An optional reference; `$` and `*` are `None`.
pub(crate) fn opt_reference(parameter: Option<&Parameter>, kind: &str) -> Step<Option<u64>> {
    if absent(parameter) {
        return Ok(None);
    }
    reference(parameter, kind).map(Some)
}

/// The elements of a list parameter.
pub(crate) fn list<'a>(parameter: Option<&'a Parameter>, kind: &str) -> Step<&'a [Parameter]> {
    match parameter {
        Some(Parameter::List(items)) => Ok(items),
        _ => Err(wrong(kind)),
    }
}

/// The references of a list, with repeats removed; returns the references in order and the number
/// of repeats.
pub(crate) fn references(parameter: Option<&Parameter>, kind: &str) -> Step<(Vec<u64>, u64)> {
    let mut out = Vec::new();
    let mut repeats = 0_u64;
    for item in list(parameter, kind)? {
        match item {
            Parameter::Reference(name) => {
                if out.contains(name) {
                    repeats = repeats.saturating_add(1);
                } else {
                    out.push(*name);
                }
            }
            _ => return Err(wrong(kind)),
        }
    }
    Ok((out, repeats))
}

/// The integers of a list. A value that is not an integer is a skip.
pub(crate) fn integers(parameter: Option<&Parameter>, kind: &str) -> Step<Vec<i64>> {
    list(parameter, kind)?
        .iter()
        .map(|item| match item {
            Parameter::Integer(value) => Ok(*value),
            _ => Err(skip(reasons::INDEX_INVALID, kind)),
        })
        .collect()
}

/// An optional enumeration without its dots.
pub(crate) fn enumeration(parameter: Option<&Parameter>) -> Option<&str> {
    match parameter {
        Some(Parameter::Enumeration(name)) => Some(name.as_str()),
        _ => None,
    }
}

/// An optional text: a string, bare or wrapped in a type; `None` for `$`.
pub(crate) fn text(parameter: Option<&Parameter>, kind: &str) -> Step<Option<String>> {
    match parameter {
        None | Some(Parameter::Omitted | Parameter::Derived) => Ok(None),
        Some(Parameter::Text(text)) => Ok(Some(text.clone())),
        Some(Parameter::Typed { value, .. }) => match value.as_ref() {
            Parameter::Text(text) => Ok(Some(text.clone())),
            _ => Err(wrong(kind)),
        },
        Some(_) => Err(wrong(kind)),
    }
}

/// The coordinates of a point or direction of `dimension` 2 or 3 components, as a 3D vector with
/// a zero third component for 2D input. Values are scaled by `scale` (points) or not (directions).
pub(crate) fn coordinates3(list: &[Parameter], scale: f64, kind: &str) -> Step<Vector3> {
    let values: Vec<f64> = list
        .iter()
        .map(|item| real(Some(item), scale, kind))
        .collect::<Step<_>>()?;
    match values.as_slice() {
        [x, y] => Ok(Vector3::new(*x, *y, 0.0)),
        [x, y, z] => Ok(Vector3::new(*x, *y, *z)),
        _ => Err(wrong(kind)),
    }
}

/// The coordinates of a 2D point or direction, scaled by `scale`.
pub(crate) fn coordinates2(list: &[Parameter], scale: f64, kind: &str) -> Step<Vector2> {
    let values: Vec<f64> = list
        .iter()
        .map(|item| real(Some(item), scale, kind))
        .collect::<Step<_>>()?;
    match values.as_slice() {
        [x, y] => Ok(Vector2::new(*x, *y)),
        _ => Err(wrong(kind)),
    }
}

/// A 3D point from a 2D or 3D coordinate list.
pub(crate) fn point3(list: &[Parameter], scale: f64, kind: &str) -> Step<Point3> {
    coordinates3(list, scale, kind).map(|v| Point3::new(v.x, v.y, v.z))
}
