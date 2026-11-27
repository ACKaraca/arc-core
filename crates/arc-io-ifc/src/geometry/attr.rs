//! Reads of attributes by position, with presence and type checks. A wrong type is a skip of the
//! item that holds the attribute, reported as `io.ifc.value_unexpected`.

use std::collections::BTreeSet;

use arc_base::math::{Point3, Vector2, Vector3};

use crate::model::{IfcFile, Parameter, Record};

use super::budget::Budget;
use super::{Flow, Skip, Step};
use crate::error::reasons;

/// Bytes charged for one reference of a list: the entry of the vector and of the set of distinct
/// references.
const REFERENCE_BYTES: u64 = 32;

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

/// The one reference of a list element; any other element is a skip of the item.
fn reference_item(item: &Parameter, kind: &str) -> Step<u64> {
    match item {
        Parameter::Reference(name) => Ok(*name),
        _ => Err(wrong(kind)),
    }
}

/// Refuses a list longer than `max` before anything is built from it: longer than `array_length` is
/// a limit error, longer than the caller's own cap is a skip of the item.
fn check_length(budget: &Budget, len: usize, kind: &str, max: usize) -> Step<()> {
    budget.array(len)?;
    if len > max {
        Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind))
    } else {
        Ok(())
    }
}

/// The references of a list in their order, repeats kept. Use it where order and multiplicity are
/// the geometry: point lists of polylines and loops, indices and bounds of a face. The list is
/// refused when it is longer than `max` (`array_length` is checked as well) before anything is
/// charged or built, and the cancellation token is checked per element.
pub(crate) fn ordered_references(
    budget: &mut Budget,
    parameter: Option<&Parameter>,
    kind: &str,
    max: usize,
) -> Step<Vec<u64>> {
    let items = list(parameter, kind)?;
    check_length(budget, items.len(), kind, max)?;
    budget.charge(charge_of(items.len()))?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        budget.step()?;
        out.push(reference_item(item, kind)?);
    }
    Ok(out)
}

/// The references of a list with repeats removed, in the order of their first occurrence, and the
/// number of repeats. Use it where a repeat is meaningless: representation items, the faces of a
/// shell, the shells of a model, the representations of a product. Same checks as
/// [`ordered_references`].
pub(crate) fn distinct_references(
    budget: &mut Budget,
    parameter: Option<&Parameter>,
    kind: &str,
    max: usize,
) -> Step<(Vec<u64>, u64)> {
    let items = list(parameter, kind)?;
    check_length(budget, items.len(), kind, max)?;
    budget.charge(charge_of(items.len()))?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(items.len());
    let mut repeats = 0_u64;
    for item in items {
        budget.step()?;
        let name = reference_item(item, kind)?;
        if seen.insert(name) {
            out.push(name);
        } else {
            repeats = repeats.saturating_add(1);
        }
    }
    Ok((out, repeats))
}

/// Bytes charged for the references of a list of `len` elements: the vector and the set of
/// distinct references.
fn charge_of(len: usize) -> u64 {
    u64::try_from(len)
        .unwrap_or(u64::MAX)
        .saturating_mul(REFERENCE_BYTES)
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

/// The attribute `name` of a record by the schema tables, so that the differences between the
/// schemas are read as declared; at `position` (its place in IFC4) when the build has no tables.
pub(crate) fn named<'a>(
    file: &IfcFile,
    record: &'a Record,
    name: &str,
    position: usize,
) -> Option<&'a Parameter> {
    if file.tables().is_some() {
        file.attribute(record, name)
    } else {
        record.parameters.get(position)
    }
}

/// An optional boolean (`.T.` or `.F.`); `None` for `$`, `*` and any other value.
pub(crate) fn boolean(parameter: Option<&Parameter>) -> Option<bool> {
    match parameter {
        Some(Parameter::Enumeration(name)) => match name.as_str() {
            "T" => Some(true),
            "F" => Some(false),
            _ => None,
        },
        Some(Parameter::Typed { value, .. }) => boolean(Some(value)),
        _ => None,
    }
}
