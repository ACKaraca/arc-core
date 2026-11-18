//! Insertion units: the `$INSUNITS` codes and the drafting space units.

use arc_draft::types::SpaceUnit;

/// Meters per unit of an `$INSUNITS` code; `None` for 0 (unitless) and unknown codes.
#[must_use]
pub fn meters_of_code(code: i64) -> Option<f64> {
    Some(match code {
        1 => 0.0254,
        2 => 0.3048,
        3 => 1609.344,
        4 => 0.001,
        5 => 0.01,
        6 => 1.0,
        7 => 1000.0,
        8 => 2.54e-8,
        9 => 2.54e-5,
        10 => 0.9144,
        11 => 1e-10,
        12 => 1e-9,
        13 => 1e-6,
        14 => 0.1,
        15 => 10.0,
        16 => 100.0,
        17 => 1e9,
        18 => 1.495_978_707e11,
        19 => 9.460_730_472_580_8e15,
        20 => 3.085_677_581_491_4e16,
        21 => 1200.0 / 3937.0,
        _ => return None,
    })
}

/// The space unit of an `$INSUNITS` code, when the drafting model has one.
#[must_use]
pub fn unit_of_code(code: i64) -> Option<SpaceUnit> {
    Some(match code {
        1 => SpaceUnit::Inch,
        2 => SpaceUnit::Foot,
        3 => SpaceUnit::Mile,
        4 => SpaceUnit::Millimeter,
        5 => SpaceUnit::Centimeter,
        6 => SpaceUnit::Meter,
        7 => SpaceUnit::Kilometer,
        10 => SpaceUnit::Yard,
        21 => SpaceUnit::UsSurveyFoot,
        _ => return None,
    })
}

/// The `$INSUNITS` code of a space unit.
#[must_use]
pub fn code_of_unit(unit: SpaceUnit) -> i64 {
    match unit {
        SpaceUnit::Unitless => 0,
        SpaceUnit::Inch => 1,
        SpaceUnit::Foot => 2,
        SpaceUnit::Mile => 3,
        SpaceUnit::Millimeter => 4,
        SpaceUnit::Centimeter => 5,
        SpaceUnit::Meter => 6,
        SpaceUnit::Kilometer => 7,
        SpaceUnit::Yard => 10,
        SpaceUnit::UsSurveyFoot => 21,
    }
}

/// The factor that converts a length of the file's unit `code` to `target`; 1 when either is
/// unitless or the code is unknown.
#[must_use]
pub fn factor(code: i64, target: SpaceUnit) -> f64 {
    match (meters_of_code(code), target.unit()) {
        (Some(from), Some(to)) if to.scale > 0.0 => from / to.scale,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inches_convert_to_millimeters() {
        assert!((factor(1, SpaceUnit::Millimeter) - 25.4).abs() < 1e-12);
        assert!((factor(4, SpaceUnit::Meter) - 0.001).abs() < 1e-15);
        assert!((factor(0, SpaceUnit::Meter) - 1.0).abs() < f64::EPSILON);
        assert!((factor(1, SpaceUnit::Unitless) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn codes_round_trip_for_every_space_unit() {
        for unit in [
            SpaceUnit::Unitless,
            SpaceUnit::Millimeter,
            SpaceUnit::Centimeter,
            SpaceUnit::Meter,
            SpaceUnit::Kilometer,
            SpaceUnit::Inch,
            SpaceUnit::Foot,
            SpaceUnit::Yard,
            SpaceUnit::Mile,
            SpaceUnit::UsSurveyFoot,
        ] {
            let code = code_of_unit(unit);
            assert_eq!(unit_of_code(code).unwrap_or(SpaceUnit::Unitless), unit);
        }
    }
}
