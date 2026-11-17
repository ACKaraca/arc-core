//! Text written by `arc_base::DisplayFormat` reads back as the displayed value (CORE-SOLVE-022).

#![allow(clippy::unwrap_used, reason = "test helpers fail by panicking")]

use arc_base::{DisplayFormat, Unit};
use arc_solve::{Context, parse_quantity};
use proptest::prelude::*;

fn any_unit() -> impl Strategy<Value = Unit> {
    proptest::sample::select(Unit::all())
}

fn separator() -> impl Strategy<Value = char> {
    proptest::sample::select(vec!['.', ',', '٫'])
}

/// Either sign, log-uniform over 10^low to 10^high, or zero.
fn magnitude(low: f64, high: f64) -> impl Strategy<Value = f64> {
    prop_oneof![
        9 => (low..=high, any::<bool>()).prop_map(|(e, negative)| {
            let m = 10.0_f64.powf(e);
            if negative { -m } else { m }
        }),
        1 => Just(0.0),
    ]
}

fn format(
    unit: Unit,
    precision: u8,
    increment: Option<u8>,
    trailing_zeros: bool,
    show_symbol: bool,
) -> DisplayFormat {
    DisplayFormat {
        unit,
        precision,
        // A multiple of the last displayed decimal place.
        rounding_increment: increment.map(|m| f64::from(m) / 10.0_f64.powi(i32::from(precision))),
        trailing_zeros,
        show_symbol,
    }
}

fn field(unit: Unit) -> Context<'static> {
    Context::new().with_bare_unit(unit)
}

proptest! {
    #[test]
    fn formatted_text_parses_to_the_same_display_value(
        unit in any_unit(),
        value in magnitude(-7.0, 8.0),
        precision in 0_u8..=6,
        increment in proptest::option::of(1_u8..=50),
        trailing_zeros: bool,
        show_symbol: bool,
        separator in separator(),
    ) {
        let format = format(unit, precision, increment, trailing_zeros, show_symbol);
        let text = format.format(unit.quantity(value), separator).unwrap();
        let back = parse_quantity(&text, &field(unit), separator, unit.dimension);
        prop_assert!(back.is_ok(), "{text:?}: {back:?}");
        let again = format.format(back.unwrap(), separator).unwrap();
        prop_assert_eq!(&again, &text, "{} at {}", unit.name, value);
    }

    #[test]
    fn extreme_magnitudes_read_back_within_the_display_step(
        unit in any_unit(),
        value in magnitude(-300.0, 300.0),
        precision in 0_u8..=3,
        show_symbol: bool,
        separator in separator(),
    ) {
        let format = format(unit, precision, None, false, show_symbol);
        let quantity = unit.quantity(value);
        let text = format.format(quantity, separator).unwrap();
        let back = parse_quantity(&text, &field(unit), separator, unit.dimension);
        prop_assert!(back.is_ok(), "{text:?}: {back:?}");
        let step = 0.5 * 10.0_f64.powi(-i32::from(precision)) * unit.scale;
        let si = quantity.value().abs().max(unit.offset);
        let error = (back.unwrap().value() - quantity.value()).abs();
        prop_assert!(error <= step + 1e-14 * si, "{text:?}: off by {error}");
    }
}

#[test]
fn a_field_without_symbol_needs_the_context_unit() {
    let mm = *Unit::by_symbol("mm").unwrap();
    let format = DisplayFormat {
        show_symbol: false,
        ..DisplayFormat::new(mm)
    };
    let text = format.format(mm.quantity(-1234.5), ',').unwrap();
    assert_eq!(text, "-1234,5");
    let back = parse_quantity(&text, &field(mm), ',', mm.dimension).unwrap();
    assert_eq!(format.format(back, ',').unwrap(), text);
    let error = parse_quantity(&text, &Context::default(), ',', mm.dimension).unwrap_err();
    assert_eq!(error.code(), "solve.expr.dimension_mismatch");
    // A typed unit overrides the display unit, and expressions are field input too.
    let typed = parse_quantity("3 in + 0,5", &field(mm), ',', mm.dimension).unwrap();
    assert!((typed.value() - 0.0767).abs() < 1e-12);
}

#[test]
fn negative_readings_on_offset_scales_round_trip() {
    let celsius = *Unit::by_name("degree_celsius").unwrap();
    for show_symbol in [true, false] {
        let format = DisplayFormat {
            show_symbol,
            ..DisplayFormat::new(celsius)
        };
        let text = format.format(celsius.quantity(-40.0), '.').unwrap();
        let back = parse_quantity(&text, &field(celsius), '.', celsius.dimension).unwrap();
        assert!((back.value() - 233.15).abs() < 1e-9, "{text}");
    }
}
