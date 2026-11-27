//! Units of the project (CORE-IO-015): the length unit gives the scale of every length to meters,
//! the plane angle unit the radians of an angle. Both are read from the unit assignment of the
//! project, or the first unit assignment of the file.
//!
//! Supported units are `IfcSIUnit` with a prefix (`LENGTHUNIT` `METRE`, `PLANEANGLEUNIT` `RADIAN`)
//! and `IfcConversionBasedUnit` whose `IfcMeasureWithUnit` refers to an `IfcSIUnit` (for example
//! a foot of 0.3048 metre). Other units are ignored, and so is a conversion whose factor is zero,
//! negative or not finite. When no usable length unit exists, meters are assumed and reported as
//! `io.ifc.unit_assumed`.

use arc_base::Result;

use crate::error::reasons;
use crate::model::{IfcFile, Parameter, Record};
use crate::schema::access::known;

use super::Evaluator;
use super::attr;

/// The units the geometry was converted with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Units {
    /// Meters per length unit of the file: 0.001 for millimeters.
    pub meters_per_length_unit: f64,
    /// Radians per plane angle unit of the file: 1.0 for radians, pi / 180 for degrees.
    pub radians_per_angle_unit: f64,
    /// Whether no length unit was found and meters were assumed.
    pub length_assumed: bool,
}

/// The scale of an SI prefix, or `None` for an unknown prefix.
fn prefix_factor(prefix: &str) -> Option<f64> {
    Some(match prefix {
        "EXA" => 1e18,
        "PETA" => 1e15,
        "TERA" => 1e12,
        "GIGA" => 1e9,
        "MEGA" => 1e6,
        "KILO" => 1e3,
        "HECTO" => 1e2,
        "DECA" => 1e1,
        "DECI" => 1e-1,
        "CENTI" => 1e-2,
        "MILLI" => 1e-3,
        "MICRO" => 1e-6,
        "NANO" => 1e-9,
        "PICO" => 1e-12,
        "FEMTO" => 1e-15,
        "ATTO" => 1e-18,
        _ => return None,
    })
}

/// The factor of an SI unit record (its prefix); `None` when the record is not an SI unit or its
/// prefix is unknown.
fn si_factor(file: &IfcFile, record: &Record) -> Option<f64> {
    if record.type_name != "IFCSIUNIT" {
        return None;
    }
    match file.attr(record, known::SI_PREFIX) {
        None | Some(Parameter::Omitted | Parameter::Derived) => Some(1.0),
        Some(Parameter::Enumeration(prefix)) => prefix_factor(prefix),
        _ => None,
    }
}

/// The name of an SI unit (`METRE`, `RADIAN`, ...).
fn si_name<'a>(file: &IfcFile, record: &'a Record) -> Option<&'a str> {
    attr::enumeration(file.attr(record, known::SI_NAME))
}

impl Evaluator<'_> {
    /// Reads the length and plane angle units, sets the length scale of the evaluation and logs
    /// the assumption when no length unit is found.
    pub(crate) fn read_units(&mut self) -> Result<Units> {
        let mut length = None;
        let mut angle = None;
        for unit in self.unit_names() {
            let Some(record) = self.file.instance(unit).and_then(|i| i.records.first()) else {
                continue;
            };
            let unit_type = attr::enumeration(self.file.attr(record, known::UNIT_TYPE));
            match record.type_name.as_str() {
                "IFCSIUNIT" => {
                    let factor = si_factor(self.file, record);
                    match (unit_type, si_name(self.file, record), factor) {
                        (Some("LENGTHUNIT"), Some("METRE"), Some(f)) if length.is_none() => {
                            length = Some(f);
                        }
                        (Some("PLANEANGLEUNIT"), Some("RADIAN"), Some(f)) if angle.is_none() => {
                            angle = Some(f);
                        }
                        _ => {}
                    }
                }
                "IFCCONVERSIONBASEDUNIT" => {
                    let Some(value) = self.conversion_factor(record) else {
                        continue;
                    };
                    match unit_type {
                        Some("LENGTHUNIT") if length.is_none() => length = Some(value),
                        Some("PLANEANGLEUNIT") if angle.is_none() => angle = Some(value),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let length_assumed = length.is_none();
        if length_assumed {
            self.log
                .add(reasons::UNIT_ASSUMED, "IFCUNITASSIGNMENT", None, 1);
        }
        let meters_per_length_unit = length.unwrap_or(1.0);
        self.scale = meters_per_length_unit;
        Ok(Units {
            meters_per_length_unit,
            radians_per_angle_unit: angle.unwrap_or(1.0),
            length_assumed,
        })
    }

    /// The names of the units of the project's unit assignment, or of the first unit assignment of
    /// the file when the project has none.
    fn unit_names(&self) -> Vec<u64> {
        let file = self.file;
        let project = file
            .instances_of("IFCPROJECT")
            .next()
            .and_then(|i| i.records.first())
            .and_then(|r| match file.attr(r, known::PROJECT_UNITS) {
                Some(Parameter::Reference(name)) => Some(*name),
                _ => None,
            });
        let assignment = project.or_else(|| {
            file.instances_of("IFCUNITASSIGNMENT")
                .next()
                .map(|i| i.name)
        });
        let Some(assignment) = assignment else {
            return Vec::new();
        };
        let Some(record) = attr::typed(file, assignment, "IFCUNITASSIGNMENT") else {
            return Vec::new();
        };
        match file.attr(record, known::UNIT_ASSIGNMENT_UNITS) {
            Some(Parameter::List(items)) => items
                .iter()
                .filter_map(|item| match item {
                    Parameter::Reference(name) => Some(*name),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The factor of an `IfcConversionBasedUnit`: the value of its `IfcMeasureWithUnit` times the
    /// factor of the SI unit that measure refers to.
    fn conversion_factor(&self, record: &Record) -> Option<f64> {
        let Some(Parameter::Reference(measure)) = self.file.attr(record, known::CONVERSION_FACTOR)
        else {
            return None;
        };
        self.measure_with_unit(*measure)
    }

    fn measure_with_unit(&self, name: u64) -> Option<f64> {
        let measure = attr::typed(self.file, name, "IFCMEASUREWITHUNIT")?;
        let value = match self.file.attr(measure, known::MEASURE_VALUE)? {
            Parameter::Typed { value, .. } => match value.as_ref() {
                Parameter::Real(real) => *real,
                Parameter::Integer(integer) => *integer as f64,
                _ => return None,
            },
            Parameter::Real(real) => *real,
            Parameter::Integer(integer) => *integer as f64,
            _ => return None,
        };
        let Some(Parameter::Reference(component)) = self.file.attr(measure, known::MEASURE_UNIT)
        else {
            return None;
        };
        let component = attr::typed(self.file, *component, "IFCSIUNIT")?;
        let factor = value * si_factor(self.file, component)?;
        // A factor that is zero, negative or not finite makes the unit unusable: meters are assumed.
        (factor.is_finite() && factor > 0.0).then_some(factor)
    }
}
