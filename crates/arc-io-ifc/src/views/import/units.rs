//! The unit assignment of the project: SI units, conversion based units (with offset) and derived
//! units, with the factor of each to SI base units.
//!
//! An SI prefix scales the unit once, as in SI naming: `KILO` with `SQUARE_METRE` is 1000 square
//! meters. `GRAM` is 0.001 of the base unit kilogram. The conversion of a unit is bounded in depth,
//! so a conversion that refers to itself ends with no factor.

use arc_base::Result;

use crate::error::reasons;
use crate::model::Record;

use super::super::{Number, Reader, got_number};
use super::{UnitKind, UnitView};

/// The deepest chain of units a conversion may follow.
const UNIT_DEPTH: u32 = 8;

/// The factor of an SI prefix.
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

/// The factor of an `IfcSIUnit` name to the SI base unit of its type.
fn base_factor(name: &str) -> f64 {
    if name == "GRAM" { 1e-3 } else { 1.0 }
}

/// The unit type a measure type is expressed in: `IFCLENGTHMEASURE` is `LENGTHUNIT`,
/// `IFCPOSITIVELENGTHMEASURE` too, `IFCMASSDENSITYMEASURE` is `MASSDENSITYUNIT`.
pub(super) fn unit_type_of_measure(measure: &str) -> Option<String> {
    let core = measure.strip_prefix("IFC")?.strip_suffix("MEASURE")?;
    let core = core
        .strip_prefix("POSITIVE")
        .or_else(|| core.strip_prefix("NONNEGATIVE"))
        .unwrap_or(core);
    (!core.is_empty()).then(|| format!("{core}UNIT"))
}

pub(super) fn to_f64(number: Number) -> f64 {
    match number {
        Number::Real(real) => real,
        #[allow(clippy::cast_precision_loss, reason = "a measure, not an id")]
        Number::Integer(integer) => integer as f64,
    }
}

impl Reader<'_> {
    /// Reads the unit assignment of the first project and sets the length and angle scale.
    pub(super) fn read_units(&mut self) -> Result<()> {
        let file = self.file;
        let Some(first) = file.instances_of("IFCPROJECT").next() else {
            return Ok(());
        };
        let name = first.name;
        let Some(project) = self.rec(name) else {
            return Ok(());
        };
        let Some(assignment) = self.reference(project, "UnitsInContext", name) else {
            return Ok(());
        };
        let Some(assignment_record) = self.rec(assignment) else {
            self.lose(reasons::TARGET_MISSING, "IFCPROJECT", name, 1);
            return Ok(());
        };
        for unit in self.references_of(assignment_record, "Units", assignment) {
            self.cancel.check()?;
            self.unit_index(unit);
        }
        self.imp.units.assigned = self.imp.units.units.len();
        let find = |units: &[UnitView], kind: &str| {
            units
                .iter()
                .find(|u| u.unit_type == kind)
                .and_then(|u| u.to_si)
        };
        let units = &self.imp.units.units;
        let length = find(units, "LENGTHUNIT");
        let angle = find(units, "PLANEANGLEUNIT");
        self.imp.units.length_assumed = length.is_none();
        self.imp.units.length_to_meters = length.unwrap_or(1.0);
        self.imp.units.angle_to_radians = angle.unwrap_or(1.0);
        if length.is_none() {
            self.lose(reasons::UNIT_ASSUMED, "IFCPROJECT", name, 1);
        }
        Ok(())
    }

    /// The index in the unit list of the unit `instance`, reading and adding it when it is not
    /// listed yet; `None` when it is not a unit.
    pub(super) fn unit_index(&mut self, instance: u64) -> Option<usize> {
        if let Some(i) = self
            .imp
            .units
            .units
            .iter()
            .position(|u| u.instance == instance)
        {
            return Some(i);
        }
        let unit = self.read_unit(instance, 0)?;
        self.imp.units.units.push(unit);
        Some(self.imp.units.units.len().saturating_sub(1))
    }

    /// The factor to SI of the unit `instance`, read without adding it to the unit list.
    pub(super) fn unit_to_si(&mut self, instance: u64) -> Option<f64> {
        self.read_unit(instance, 0)?.to_si
    }

    /// The index of the project's unit of `unit_type`, the first one listed.
    pub(super) fn project_unit(&self, unit_type: &str) -> Option<usize> {
        self.imp
            .units
            .units
            .iter()
            .position(|u| u.unit_type == unit_type)
    }

    fn read_unit(&mut self, instance: u64, depth: u32) -> Option<UnitView> {
        if depth > UNIT_DEPTH {
            return None;
        }
        let record = self.rec(instance)?;
        let (kind, unit_type, name, to_si, offset) = match record.type_name.as_str() {
            "IFCSIUNIT" => self.si_unit(record, instance),
            "IFCCONVERSIONBASEDUNIT" | "IFCCONVERSIONBASEDUNITWITHOFFSET" => {
                self.conversion_unit(record, instance, depth)
            }
            "IFCDERIVEDUNIT" => self.derived_unit(record, instance, depth),
            "IFCMONETARYUNIT" => {
                let currency = self
                    .enumeration(record, "Currency", instance)
                    .or_else(|| self.text(record, "Currency", instance));
                (
                    UnitKind::Monetary,
                    "MONETARYUNIT".to_owned(),
                    currency,
                    None,
                    0.0,
                )
            }
            "IFCCONTEXTDEPENDENTUNIT" => {
                let unit_type = self.enumeration(record, "UnitType", instance);
                let name = self.text(record, "Name", instance);
                (
                    UnitKind::ContextDependent,
                    unit_type.unwrap_or_default(),
                    name,
                    None,
                    0.0,
                )
            }
            _ => {
                self.lose(reasons::NOT_MAPPED, record.type_name.as_str(), instance, 1);
                return None;
            }
        };
        Some(UnitView {
            instance,
            kind,
            unit_type,
            name: name.unwrap_or_default(),
            to_si,
            offset,
        })
    }

    #[allow(
        clippy::type_complexity,
        reason = "a private tuple of the parts of a unit"
    )]
    fn si_unit(
        &mut self,
        record: &Record,
        instance: u64,
    ) -> (UnitKind, String, Option<String>, Option<f64>, f64) {
        let unit_type = self
            .enumeration(record, "UnitType", instance)
            .unwrap_or_default();
        let name = self.enumeration(record, "Name", instance);
        let prefix = self.enumeration(record, "Prefix", instance);
        let scale = match prefix.as_deref() {
            None => Some(1.0),
            Some(p) => prefix_factor(p),
        };
        let to_si = name.as_deref().zip(scale).map(|(n, s)| s * base_factor(n));
        (UnitKind::Si, unit_type, name, to_si, 0.0)
    }

    #[allow(
        clippy::type_complexity,
        reason = "a private tuple of the parts of a unit"
    )]
    fn conversion_unit(
        &mut self,
        record: &Record,
        instance: u64,
        depth: u32,
    ) -> (UnitKind, String, Option<String>, Option<f64>, f64) {
        let unit_type = self
            .enumeration(record, "UnitType", instance)
            .unwrap_or_default();
        let name = self.text(record, "Name", instance);
        let offset = self
            .real(record, "ConversionOffset", instance)
            .unwrap_or(0.0);
        let to_si = self
            .reference(record, "ConversionFactor", instance)
            .and_then(|measure| {
                let measure_record = self.rec(measure)?;
                let value = got_number(self.file.attribute(measure_record, "ValueComponent"))
                    .ok()
                    .flatten()?;
                let component = self.reference(measure_record, "UnitComponent", measure)?;
                let base = self.read_unit(component, depth.saturating_add(1))?.to_si?;
                Some(to_f64(value) * base)
            });
        let factor_ok = to_si.is_some_and(|f| f.is_finite() && f > 0.0);
        (
            UnitKind::ConversionBased,
            unit_type,
            name,
            if factor_ok { to_si } else { None },
            offset,
        )
    }

    #[allow(
        clippy::type_complexity,
        reason = "a private tuple of the parts of a unit"
    )]
    fn derived_unit(
        &mut self,
        record: &Record,
        instance: u64,
        depth: u32,
    ) -> (UnitKind, String, Option<String>, Option<f64>, f64) {
        let unit_type = self
            .enumeration(record, "UnitType", instance)
            .unwrap_or_default();
        let user = self.text(record, "UserDefinedType", instance);
        let name = self.text(record, "Name", instance);
        let mut to_si = Some(1.0_f64);
        for element in self.references_of(record, "Elements", instance) {
            let factor = self.rec(element).and_then(|e| {
                let unit = self.reference(e, "Unit", element)?;
                let exponent = self.integer(e, "Exponent", element)?;
                let base = self.read_unit(unit, depth.saturating_add(1))?.to_si?;
                Some(base.powi(i32::try_from(exponent).ok()?))
            });
            to_si = to_si.zip(factor).map(|(a, b)| a * b);
        }
        let unit_type = if unit_type == "USERDEFINED" {
            user.unwrap_or(unit_type)
        } else {
            unit_type
        };
        (UnitKind::Derived, unit_type, name, to_si, 0.0)
    }
}
