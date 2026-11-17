//! Evaluation rules of the expression engine: dimensional arithmetic, bare numbers, the function
//! library, limits and the caller's hooks (CORE-SOLVE-016 to CORE-SOLVE-018, CORE-SOLVE-021).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test helpers fail by panicking"
)]

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use arc_base::{Cancel, Dimension, Error, Quantity, Unit};
use arc_solve::error::span_of;
use arc_solve::{Context, Expr, ExprLimits, Functions, Span, Value, evaluate};

fn unit(symbol: &str) -> Unit {
    *Unit::by_symbol(symbol).unwrap()
}

fn mm() -> Context<'static> {
    Context::new().with_bare_unit(unit("mm"))
}

fn number(text: &str, context: &Context<'_>) -> Quantity {
    match evaluate(text, context) {
        Ok(Value::Number(quantity) | Value::Absolute(quantity)) => quantity,
        other => panic!("{text}: {other:?}"),
    }
}

fn plain(text: &str) -> Quantity {
    number(text, &Context::default())
}

fn failure(text: &str, context: &Context<'_>) -> Error {
    match evaluate(text, context) {
        Err(error) => error,
        Ok(value) => panic!("{text}: {value:?}"),
    }
}

fn code(text: &str) -> &'static str {
    failure(text, &Context::default()).code()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * b.abs().max(1e-3)
}

#[track_caller]
fn check(text: &str, context: &Context<'_>, value: f64, dimension: Dimension) {
    let quantity = number(text, context);
    assert!(close(quantity.value(), value), "{text}: {quantity:?}");
    assert_eq!(quantity.dimension(), dimension, "{text}");
}

#[test]
fn a_length_plus_a_length_is_a_length() {
    check(
        "1200 mm + 3 in",
        &Context::default(),
        1.2762,
        Dimension::LENGTH,
    );
}

#[test]
fn a_length_plus_an_area_fails_with_the_offending_span() {
    let text = "2 * (1 m + 3 m^2) - 1 m";
    let error = failure(text, &Context::default());
    assert_eq!(error.code(), "solve.expr.dimension_mismatch");
    let span = span_of(&error).unwrap();
    // Parentheses belong to the sub-expression they enclose.
    assert_eq!(&text[span.start..span.end], "(1 m + 3 m^2)");
    assert_eq!(code("1 m < 1 s"), "solve.expr.dimension_mismatch");
    assert_eq!(code("1 m == 1 kg"), "solve.expr.dimension_mismatch");
}

#[test]
fn products_and_quotients_combine_dimensions() {
    let none = Context::default();
    check("2 m * 3 m", &none, 6.0, Dimension::AREA);
    check("6 m^2 / 2 m", &none, 3.0, Dimension::LENGTH);
    check("10 m / 2 s", &none, 5.0, Dimension::VELOCITY);
    check("2 * 3 m", &none, 6.0, Dimension::LENGTH);
    check("12 kN/m * 2 m", &none, 24_000.0, Dimension::FORCE);
    check("1 m / 1 m", &none, 1.0, Dimension::NONE);
    check("-(2 m) * +3", &none, -6.0, Dimension::LENGTH);
}

#[test]
fn powers_take_rational_exponents_that_keep_dimensions_whole() {
    let none = Context::default();
    check("(3 m)^2", &none, 9.0, Dimension::AREA);
    check("3 m ^ 2", &none, 9.0, Dimension::AREA);
    check("2 * 3 m^2", &none, 6.0, Dimension::AREA);
    check("(4 m^2)^0.5", &none, 2.0, Dimension::LENGTH);
    check("(8 m^3)^(1/3)", &none, 2.0, Dimension::LENGTH);
    check("(4 m^2)^1.5", &none, 8.0, Dimension::VOLUME);
    check(
        "(2 m)^-1",
        &none,
        0.5,
        Dimension::NONE.div(Dimension::LENGTH),
    );
    check("2^10", &none, 1024.0, Dimension::NONE);
    check("2^0.5", &none, 2.0_f64.sqrt(), Dimension::NONE);
    check("pow(2 m, 2)", &none, 4.0, Dimension::AREA);
    check("sqrt(16 m^2)", &none, 4.0, Dimension::LENGTH);
    assert_eq!(code("(2 m)^0.5"), "solve.expr.dimension_mismatch");
    assert_eq!(code("(2 m)^0.3"), "solve.expr.dimension_mismatch");
    assert_eq!(code("2^(1 m)"), "solve.expr.dimension_mismatch");
    assert_eq!(code("sqrt(2 m)"), "solve.expr.dimension_mismatch");
    assert_eq!(code("(2 m)^200"), "solve.expr.limit_exceeded");
    assert_eq!(code("(2 m^100) * (2 m^100)"), "solve.expr.limit_exceeded");
}

#[test]
fn domain_errors_carry_the_span() {
    for text in [
        "1 / 0",
        "1 m / (2 m - 2 m)",
        "sqrt(-1)",
        "(-8)^0.5",
        "asin(2)",
        "1e300 * 1e300",
    ] {
        assert_eq!(code(text), "solve.expr.domain", "{text}");
    }
    assert_eq!(code("round(5 mm, 0 mm)"), "solve.expr.domain");
    assert_eq!(code("first([])"), "solve.expr.domain");
    assert_eq!(code("max([])"), "solve.expr.domain");
    let error = failure("1 + 2 / 0", &Context::default());
    assert_eq!(span_of(&error), Some(Span { start: 4, end: 9 }));
}

#[test]
fn bare_numbers_take_the_context_unit_where_a_dimension_is_needed() {
    let field = mm();
    check("1200", &field, 1.2, Dimension::LENGTH);
    check("1200 + 300", &field, 1.5, Dimension::LENGTH);
    check("1200 / 2", &field, 0.6, Dimension::LENGTH);
    check("1200 + 3 in", &field, 1.2762, Dimension::LENGTH);
    check("3 in - 25.4", &field, 0.0508, Dimension::LENGTH);
    // A bare factor scales; it is not a length.
    check("2 * 600 mm", &field, 1.2, Dimension::LENGTH);
    check("max(1200, 2 m)", &field, 2.0, Dimension::LENGTH);
    check("round(1234, 50)", &field, 1.25, Dimension::LENGTH);
    check("round(1234 mm, 50)", &field, 1.25, Dimension::LENGTH);
    check("if(1200 > 1 m, 1, 2)", &field, 0.001, Dimension::LENGTH);
    // Without a context unit a bare number stays dimensionless.
    check("1200 + 300", &Context::default(), 1500.0, Dimension::NONE);
    assert_eq!(code("1200 + 3 in"), "solve.expr.dimension_mismatch");
    // The context unit only serves its own dimension.
    assert_eq!(
        failure("1200 + 3 s", &field).code(),
        "solve.expr.dimension_mismatch"
    );
    let celsius = Context::new().with_bare_unit(unit("°C"));
    check("20", &celsius, 293.15, Dimension::TEMPERATURE);
}

#[test]
fn evaluate_as_checks_the_expected_dimension() {
    let as_length = |text: &str, context: &Context<'_>| {
        Expr::parse(text, &ExprLimits::DEFAULT)
            .unwrap()
            .evaluate_as(context, Dimension::LENGTH)
    };
    assert!(close(
        as_length("2 ft", &Context::default()).unwrap().value(),
        0.6096
    ));
    assert!(close(as_length("1200", &mm()).unwrap().value(), 1.2));
    let error = as_length(" 2 s ", &mm()).unwrap_err();
    assert_eq!(error.code(), "solve.expr.dimension_mismatch");
    assert_eq!(span_of(&error), Some(Span { start: 1, end: 4 }));
    let bare = as_length("1200", &Context::default()).unwrap_err();
    assert_eq!(bare.code(), "solve.expr.dimension_mismatch");
    assert_eq!(
        as_length("true", &mm()).unwrap_err().code(),
        "solve.expr.type_mismatch"
    );
    let count = Expr::parse("3 * 4", &ExprLimits::DEFAULT)
        .unwrap()
        .evaluate_as(&mm(), Dimension::NONE)
        .unwrap();
    assert_eq!(count, Quantity::new(12.0, Dimension::NONE));
}

#[test]
fn trigonometry_is_angle_aware() {
    let none = Context::default();
    check("sin(30 deg)", &none, 0.5, Dimension::NONE);
    check("cos(60°)", &none, 0.5, Dimension::NONE);
    check("tan(45 deg)", &none, 1.0, Dimension::NONE);
    check("sin(0.5 rad)", &none, 0.5_f64.sin(), Dimension::NONE);
    check("sin(0.5)", &none, 0.5_f64.sin(), Dimension::NONE);
    check("sin(100 gon)", &none, 1.0, Dimension::NONE);
    check("asin(1)", &none, FRAC_PI_2, Dimension::ANGLE);
    check("acos(-1)", &none, PI, Dimension::ANGLE);
    check("atan(1)", &none, FRAC_PI_4, Dimension::ANGLE);
    check("atan2(1 m, 1000 mm)", &none, FRAC_PI_4, Dimension::ANGLE);
    check("atan2(1, 1) + 45 deg", &none, FRAC_PI_2, Dimension::ANGLE);
    assert_eq!(code("sin(1 m)"), "solve.expr.dimension_mismatch");
    assert_eq!(code("asin(1 rad)"), "solve.expr.dimension_mismatch");
    assert_eq!(code("atan2(1 m, 1 s)"), "solve.expr.dimension_mismatch");
}

#[test]
fn rounding_minimum_maximum_and_absolute_value() {
    let none = Context::default();
    check("round(1234 mm, 50 mm)", &none, 1.25, Dimension::LENGTH);
    check("floor(1234 mm, 50 mm)", &none, 1.2, Dimension::LENGTH);
    check("ceil(1201 mm, 50 mm)", &none, 1.25, Dimension::LENGTH);
    check("round(-2.5)", &none, -3.0, Dimension::NONE);
    check("floor(2.7)", &none, 2.0, Dimension::NONE);
    check("ceil(2.1)", &none, 3.0, Dimension::NONE);
    check("round(7 ft, 1 m)", &none, 2.0, Dimension::LENGTH);
    check("abs(-3 m)", &none, 3.0, Dimension::LENGTH);
    check("min(3 m, 200 cm, 2500 mm)", &none, 2.0, Dimension::LENGTH);
    check("max([3 m, 200 cm])", &none, 3.0, Dimension::LENGTH);
    check("max(4)", &none, 4.0, Dimension::NONE);
    assert_eq!(code("round(1234 mm)"), "solve.expr.dimension_mismatch");
    assert_eq!(code("round(1 m, 1 s)"), "solve.expr.dimension_mismatch");
    assert_eq!(code("min(1 m, 1 s)"), "solve.expr.dimension_mismatch");
    assert_eq!(code("abs(true)"), "solve.expr.type_mismatch");
}

#[test]
fn conditionals_logic_and_comparisons() {
    let none = Context::default();
    let truth = |text: &str| evaluate(text, &none).unwrap();
    assert_eq!(truth("1 m > 50 cm && !(2 < 1)"), Value::Bool(true));
    assert_eq!(truth("1 m == 100 cm || 1 / 0 > 1"), Value::Bool(true));
    assert_eq!(truth("false && 1 / 0 > 1"), Value::Bool(false));
    assert_eq!(truth("\"a\" != \"b\" && true == true"), Value::Bool(true));
    check(
        "if(2 m > 1 m, 10 mm, 1 / 0)",
        &none,
        0.01,
        Dimension::LENGTH,
    );
    check("if(false, 1 / 0, 7)", &none, 7.0, Dimension::NONE);
    assert_eq!(code("if(1, 2, 3)"), "solve.expr.type_mismatch");
    assert_eq!(code("1 && true"), "solve.expr.type_mismatch");
    assert_eq!(code("!1"), "solve.expr.type_mismatch");
    assert_eq!(code("-true"), "solve.expr.type_mismatch");
    assert_eq!(code("\"a\" < \"b\""), "solve.expr.type_mismatch");
    assert_eq!(code("\"a\" + 1"), "solve.expr.type_mismatch");
}

#[test]
fn lists_and_text() {
    let none = Context::default();
    check("len([1, 2, 3])", &none, 3.0, Dimension::NONE);
    check("len(\"añb\")", &none, 3.0, Dimension::NONE);
    check("sum([1 m, 20 cm, 3 mm])", &none, 1.203, Dimension::LENGTH);
    check("sum([])", &none, 0.0, Dimension::NONE);
    check("first([4 m, 5 m])", &none, 4.0, Dimension::LENGTH);
    check("last([4 m, 5 m])", &none, 5.0, Dimension::LENGTH);
    check("sum([1200, 1 m])", &mm(), 2.2, Dimension::LENGTH);
    let text = |source: &str| match evaluate(source, &none).unwrap() {
        Value::Text(text) => text.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(text("format(1.2345 m, \"mm\", 1)"), "1234.5 mm");
    assert_eq!(text("format(1.2 m, \"mm\")"), "1200 mm");
    assert_eq!(text("format(90 deg, \"deg\", 0)"), "90°");
    assert_eq!(
        text("concat(\"w = \", format(2 ft, \"in\", 0), \"\\n\")"),
        "w = 24 in\n"
    );
    assert_eq!(text("concat()"), "");
    let list = evaluate("[1, 2 m, true, \"x\", [1]]", &mm()).unwrap();
    let Value::List(items) = list else { panic!() };
    assert_eq!(
        items[0],
        Value::Number(Quantity::new(0.001, Dimension::LENGTH))
    );
    assert_eq!(items[2], Value::Bool(true));
    assert_eq!(code("format(1 m, \"s\")"), "solve.expr.dimension_mismatch");
    assert_eq!(code("format(1 m, \"cubit\")"), "solve.expr.unknown_unit");
    assert_eq!(code("format(1 m, \"mm\", 1.5)"), "solve.expr.domain");
    assert_eq!(code("format(1 m, \"mm\", 99)"), "solve.expr.domain");
    assert_eq!(code("concat(\"a\", 1)"), "solve.expr.type_mismatch");
    assert_eq!(code("sum(1, 2)"), "solve.expr.argument_count");
    assert_eq!(code("sum(1)"), "solve.expr.type_mismatch");
    assert_eq!(code("sum([1, true])"), "solve.expr.type_mismatch");
}

#[test]
fn calls_are_checked() {
    assert_eq!(code("abs()"), "solve.expr.argument_count");
    assert_eq!(code("abs(1, 2)"), "solve.expr.argument_count");
    assert_eq!(code("if(true, 1)"), "solve.expr.argument_count");
    assert_eq!(code("frobnicate(1)"), "solve.expr.unknown_function");
    assert_eq!(code("true()"), "solve.expr.unknown_function");
    let error = failure("1 + width", &Context::default());
    assert_eq!(error.code(), "solve.expr.unknown_name");
    assert_eq!(span_of(&error), Some(Span { start: 4, end: 9 }));
}

#[test]
fn names_come_from_the_callers_lookup() {
    let names = |name: &str| {
        Ok(match name {
            "width" => Some(Value::Number(Quantity::new(2.0, Dimension::LENGTH))),
            "count" => Some(Value::Bare(3.0)),
            "bad" => Some(Value::Bare(f64::NAN)),
            "long" => Some(Value::list(vec![Value::Bool(true); 20_000])),
            // A parameter whose own expression failed passes its error on.
            "broken" => return Err(Error::new(arc_solve::error::codes::EXPR_DOMAIN)),
            _ => None,
        })
    };
    let context = mm().with_names(&names);
    check("width * count + 500", &context, 6.5, Dimension::LENGTH);
    assert_eq!(
        failure("height", &context).code(),
        "solve.expr.unknown_name"
    );
    assert_eq!(failure("bad", &context).code(), "solve.expr.domain");
    assert_eq!(
        failure("long", &context).code(),
        "solve.expr.limit_exceeded"
    );
    let error = failure("1 + broken", &context);
    assert_eq!(error.code(), "solve.expr.domain");
    assert_eq!(span_of(&error), Some(Span { start: 4, end: 10 }));
}

#[test]
fn hosts_register_pure_functions_under_the_same_limits() {
    let mut functions = Functions::new();
    functions
        .register("double", 1, 1, |args, _| match args {
            [Value::Number(quantity)] => Ok(Value::Number(*quantity * 2.0)),
            _ => Err(Error::new(arc_solve::error::codes::EXPR_TYPE_MISMATCH)),
        })
        .unwrap();
    functions
        .register("flood", 0, 0, |_, _| {
            Ok(Value::list(vec![Value::Bare(0.0); 20_000]))
        })
        .unwrap();
    functions
        .register("fail", 0, 0, |_, _| {
            Err(Error::new(arc_base::error::codes::UNIT_UNKNOWN))
        })
        .unwrap();
    for (name, min, max) in [
        ("max", 1, 1),
        ("true", 0, 0),
        ("double", 1, 1),
        ("1x", 0, 0),
        ("a b", 0, 0),
        ("", 0, 0),
        ("f", 2, 1),
    ] {
        let error = functions
            .register(name, min, max, |_, _| Ok(Value::Bool(true)))
            .unwrap_err();
        assert_eq!(error.code(), "solve.function.invalid", "{name}");
    }
    // A host function receives the context unit and decides what a bare argument means.
    functions
        .register("width", 1, 1, |args, unit| match (args, unit) {
            ([Value::Bare(x)], Some(unit)) => Ok(Value::Number(unit.quantity(*x))),
            _ => Err(Error::new(arc_solve::error::codes::EXPR_TYPE_MISMATCH)),
        })
        .unwrap();
    let context = Context::new().with_functions(&functions);
    check("double(2 m) + 1 m", &context, 5.0, Dimension::LENGTH);
    check(
        "width(1200)",
        &mm().with_functions(&functions),
        1.2,
        Dimension::LENGTH,
    );
    assert_eq!(
        failure("width(1200)", &context).code(),
        "solve.expr.type_mismatch"
    );
    assert_eq!(
        failure("double()", &context).code(),
        "solve.expr.argument_count"
    );
    assert_eq!(
        failure("flood()", &context).code(),
        "solve.expr.limit_exceeded"
    );
    let error = failure("1 + fail()", &context);
    assert_eq!(error.code(), "base.unit.unknown");
    assert_eq!(span_of(&error), Some(Span { start: 4, end: 10 }));
    assert_eq!(code("double(1)"), "solve.expr.unknown_function");
}

#[test]
fn evaluation_is_bounded() {
    let limited = |limits: ExprLimits| Context::new().with_limits(limits);
    let limit_of = |text: &str, context: &Context<'_>| {
        let error = failure(text, context);
        assert_eq!(error.code(), "solve.expr.limit_exceeded", "{text}");
        assert_eq!(error.category(), arc_base::Category::LimitExceeded);
        error.details().get("limit").cloned().unwrap()
    };
    let steps = limited(ExprLimits::DEFAULT.with_steps(5));
    assert_eq!(limit_of("1+1+1", &steps), "steps".into());
    assert_eq!(limit_of("sum([1,1,1])", &steps), "steps".into());
    assert!(evaluate("1+1", &steps).is_ok());
    let lists = limited(ExprLimits::DEFAULT.with_list_length(3));
    assert_eq!(limit_of("[1,2,3,4]", &lists), "list_length".into());
    assert_eq!(limit_of("max(1,2,3,4)", &lists), "list_length".into());
    let depth = limited(ExprLimits::DEFAULT.with_depth(3));
    assert_eq!(limit_of("[[[[1]]]]", &depth), "depth".into());
    assert_eq!(limit_of("----1", &depth), "depth".into());
    assert_eq!(limit_of("2^2^2^2", &depth), "depth".into());
    let text = limited(ExprLimits::DEFAULT.with_text_bytes(12));
    assert_eq!(limit_of("1 + 1 + 1 + 1", &text), "text_bytes".into());
    let long = |_: &str| Ok(Some(Value::text("x".repeat(20))));
    let texts = Context::new()
        .with_names(&long)
        .with_limits(ExprLimits::DEFAULT.with_text_bytes(30));
    assert_eq!(limit_of("concat(t, t)", &texts), "text_bytes".into());

    // An expression parsed under looser limits is checked again when it is evaluated.
    let deep = Expr::parse("-(-(-(-1)))", &ExprLimits::DEFAULT).unwrap();
    assert_eq!(
        deep.evaluate(&depth).unwrap_err().code(),
        "solve.expr.limit_exceeded"
    );

    let cancel = Cancel::new();
    cancel.cancel();
    let cancelled = Context::new().with_cancel(&cancel);
    assert_eq!(failure("1", &cancelled).code(), "base.cancelled");
}

#[test]
fn evaluation_is_repeatable_bit_for_bit() {
    let text = "round((1200 mm + 3 in) * 7 / 3, 1 mm) + 8' 6 1/2\" ^ 1";
    let first = plain(text).value().to_bits();
    for _ in 0..3 {
        assert_eq!(plain(text).value().to_bits(), first);
    }
    assert_eq!(plain("3^5").value(), 243.0);
    assert_eq!(plain("2^-2").value(), 0.25);
    assert_eq!(plain("0.1 + 0.2").value(), 0.1 + 0.2);
}

fn truth(text: &str, context: &Context<'_>) -> bool {
    match evaluate(text, context) {
        Ok(Value::Bool(truth)) => truth,
        other => panic!("{text}: {other:?}"),
    }
}

#[test]
fn rounding_to_increments_is_exact_on_multiples_and_ties() {
    let none = Context::default();
    // Multiples written in the increment's unit stay exactly as written.
    for text in [
        "floor(6 in, 1 in) == 6 in",
        "ceil(6 in, 1 in) == 6 in",
        "round(6 in, 1 in) == 6 in",
        "floor(0.3 m, 0.1 m) == 0.3 m",
        "ceil(0.3 m, 0.1 m) == 0.3 m",
        "floor(700 mm, 100 mm) == 700 mm",
        "ceil(1100 mm, 100 mm) == 1100 mm",
        "floor(5 ft, 6 in) == 5 ft",
        "ceil(1 in, 1/8\") == 1 in",
        "floor(-0.3 m, 0.1 m) == -0.3 m",
        "floor(90°, 15°) == 90°",
        // Ties go away from zero.
        "round(1.5 in, 1 in) == 2 in",
        "round(-1.5 in, 1 in) == -2 in",
        "round(2 1/4\", 1/2\") == 2 1/2\"",
        "round(15 mm, 10 mm) == 20 mm",
        "round(-15 mm, 10 mm) == -20 mm",
        "round(0.15, 0.1) == 0.2",
        // Decimal increments give the nearest decimal.
        "round(0.74 m, 0.1 m) == 0.7 m",
        "round(1234 mm, 50 mm) == 1.25 m",
        "floor(0.79, 0.1) == 0.7",
        "ceil(0.71, 0.1) == 0.8",
    ] {
        assert!(truth(text, &none), "{text}");
    }
    check("round(1225, 50)", &mm(), 1.25, Dimension::LENGTH);
    check("floor(7.5, 0.5)", &none, 7.5, Dimension::NONE);
}

#[test]
fn absolute_temperatures_follow_the_rules_of_readings() {
    let none = Context::default();
    let celsius = Context::new().with_bare_unit(unit("°C"));
    let absolute = |text: &str, context: &Context<'_>| match evaluate(text, context) {
        Ok(Value::Absolute(q)) => q.value(),
        other => panic!("{text}: {other:?}"),
    };
    let difference = |text: &str, context: &Context<'_>| match evaluate(text, context) {
        Ok(Value::Number(q)) if q.dimension() == Dimension::TEMPERATURE => q.value(),
        other => panic!("{text}: {other:?}"),
    };
    assert!(close(absolute("20 °C", &none), 293.15));
    assert!(close(absolute("-40 °C", &none), 233.15));
    assert!(close(absolute("20 °C + 5 K", &none), 298.15));
    assert!(close(absolute("5 K + 20 °C", &none), 298.15));
    assert!(close(absolute("20 °C - 5 K", &none), 288.15));
    assert!(close(difference("20 °C - 5 °C", &none), 15.0));
    assert!(close(difference("68 °F - 20 °C", &none), 0.0));
    // In a field on a scale with an offset, a bare number meeting a temperature in a sum is a
    // difference, and a bare result is a reading.
    assert!(close(absolute("20 °C + 10", &celsius), 303.15));
    assert!(close(absolute("20 °C - 10", &celsius), 283.15));
    assert!(close(absolute("20 + 10", &celsius), 303.15));
    assert!(close(difference("5 K + 3", &celsius), 8.0));
    let fahrenheit = Context::new().with_bare_unit(unit("°F"));
    assert!(close(absolute("0 °C + 9", &fahrenheit), 278.15));
    // Comparisons, `min` and `max` read a bare number as a reading.
    assert!(truth("20 °C < 25", &celsius));
    assert!(close(absolute("max(20 °C, 25)", &celsius), 298.15));
    assert!(close(absolute("min(25, 20 °C)", &celsius), 293.15));
    assert!(close(absolute("sum([20 °C, 5 K])", &none), 298.15));
    assert_eq!(
        evaluate("format(20 °C, \"°F\")", &none).unwrap(),
        Value::text("68 °F")
    );
    for text in [
        "20 °C + 5 °C",
        "-(20 °C)",
        "2 °C * 3 °C",
        "20 °C / 10 °C",
        "(20 °C)^2",
        "sqrt(20 °C)",
        "5 K - 20 °C",
        "sum([20 °C, 5 °C])",
        "round(21.3 °C, 0.5 K)",
        "round(20 °C)",
    ] {
        assert_eq!(code(text), "solve.expr.offset_unit", "{text}");
    }
    let text = "1 + (20 °C + 5 °C)";
    let span = span_of(&failure(text, &none)).unwrap();
    assert_eq!(&text[span.start..span.end], "(20 °C + 5 °C)");
    assert_eq!(
        failure("25 - 20 °C", &celsius).code(),
        "solve.expr.offset_unit"
    );
    // A reading times a plain number is a plain quantity in kelvin.
    assert!(close(difference("2 * 20 °C", &none), 586.3));
}

#[test]
fn bare_trigonometric_arguments_take_an_angle_context_unit() {
    let degrees = Context::new().with_bare_unit(unit("°"));
    check("sin(90)", &degrees, 1.0, Dimension::NONE);
    check("cos(180)", &degrees, -1.0, Dimension::NONE);
    check("sin(90 + 0°)", &degrees, 1.0, Dimension::NONE);
    check("sin(0.5)", &mm(), 0.5_f64.sin(), Dimension::NONE);
    check(
        "sin(0.5)",
        &Context::default(),
        0.5_f64.sin(),
        Dimension::NONE,
    );
    // The branches of `if` are independent: each is read on its own.
    check(
        "if(true, 5, 3 mm)",
        &degrees,
        5.0_f64.to_radians(),
        Dimension::ANGLE,
    );
    check("if(false, 5, 3 mm)", &degrees, 0.003, Dimension::LENGTH);
}

#[test]
fn prefix_plus_takes_numbers_only() {
    let none = Context::default();
    check("+3 m", &none, 3.0, Dimension::LENGTH);
    check("+-+2", &none, -2.0, Dimension::NONE);
    assert!(matches!(evaluate("+20 °C", &none), Ok(Value::Absolute(_))));
    for text in ["+true", "+\"x\"", "+[1, 2]"] {
        let error = failure(text, &none);
        assert_eq!(error.code(), "solve.expr.type_mismatch", "{text}");
        assert_eq!(
            span_of(&error),
            Some(Span {
                start: 0,
                end: text.len()
            })
        );
    }
}

#[test]
fn logic_and_condition_errors_point_at_the_operand() {
    let none = Context::default();
    let span = |text: &str| {
        let error = failure(text, &none);
        assert_eq!(error.code(), "solve.expr.type_mismatch", "{text}");
        let span = span_of(&error).unwrap();
        text[span.start..span.end].to_owned()
    };
    assert_eq!(span("true && 1"), "1");
    assert_eq!(span("1 || true"), "1");
    assert_eq!(span("true && true && 2 m"), "2 m");
    assert_eq!(span("if(1, 2, 3)"), "1");
    // Operators of a run carry the span from the first operand to their right operand.
    let error = failure("1 m + 2 s + 3 m", &none);
    assert_eq!(span_of(&error), Some(Span { start: 0, end: 9 }));
    // `&&` and `||` runs stop at the operand that decides them.
    assert!(!truth("false && x && y", &none));
    assert!(truth("true || x || y", &none));
}

#[test]
fn long_runs_of_operators_evaluate_within_the_step_limit() {
    let none = Context::default();
    let sum = format!("1{}", " + 1".repeat(10_000));
    check(&sum, &none, 10_001.0, Dimension::NONE);
    let mixed = format!("0 m{}", " + 1 mm - 2 mm".repeat(2_000));
    check(&mixed, &none, -2.0, Dimension::LENGTH);
    let product = format!("1{}", " * 1.001".repeat(1_000));
    check(&product, &none, 1.001_f64.powi(1_000), Dimension::NONE);
    let logic = format!("true{}", " && 1 < 2".repeat(3_000));
    assert!(truth(&logic, &none));
    let few = Context::new().with_limits(ExprLimits::DEFAULT.with_steps(10_000));
    assert_eq!(
        failure(&sum, &few).details().get("limit"),
        Some(&"steps".into())
    );
}

#[test]
fn context_and_limits_are_built_without_struct_literals() {
    let limits = ExprLimits::default()
        .with_text_bytes(100)
        .with_depth(10)
        .with_list_length(5)
        .with_steps(50)
        .with_value_bytes(1_000);
    assert_eq!(
        (limits.text_bytes, limits.depth, limits.list_length),
        (100, 10, 5)
    );
    assert_eq!((limits.steps, limits.value_bytes), (50, 1_000));
    let context = Context::new()
        .with_limits(limits)
        .with_bare_unit(unit("mm"));
    assert_eq!(context.bare_unit, Some(unit("mm")));
    assert_eq!(context.limits, limits);
}

proptest::proptest! {
    #[test]
    fn multiples_and_ties_of_an_increment_round_exactly(
        n in -100_000_i64..100_000,
        k in 1_i64..=12,
        symbol in proptest::sample::select(vec!["mm", "cm", "m", "in", "ft", "yd", "°", "gon", "lb", "min"]),
    ) {
        let none = Context::default();
        let (value, increment) = (n * k, format!("{k} {symbol}"));
        for function in ["floor", "ceil", "round"] {
            let text = format!("{function}({value} {symbol}, {increment}) == {value} {symbol}");
            proptest::prop_assert!(truth(&text, &none), "{}", text);
        }
        // A half increment above a multiple is a tie, rounded away from zero.
        // Exactly representable, and printed without an exponent.
        #[allow(clippy::cast_precision_loss, reason = "small integers")]
        let tie = (2 * n * k + k) as f64 / 2.0;
        let away = if n >= 0 { (n + 1) * k } else { n * k };
        let text = format!("round({tie} {symbol}, {increment}) == {away} {symbol}");
        proptest::prop_assert!(truth(&text, &none), "{}", text);
    }
}
