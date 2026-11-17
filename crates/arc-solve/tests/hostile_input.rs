//! No input makes the parser or the evaluator panic, overflow the stack or run unbounded
//! (CORE-SOLVE-021).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test helpers fail by panicking"
)]

use arc_base::{Result, Unit};
use arc_solve::error::span_of;
use arc_solve::{Context, Expr, ExprLimits, Value, evaluate};
use proptest::prelude::*;

const PIECES: [&str; 60] = [
    "1", "2.5", "1e3", "1e999", ".5", "0", "12", "mm", "m", "in", "ft", "deg", "°", "°C", "kN/m",
    "m^2", "m2", "kg*m/s^2", "'", "\"", "1/2\"", "8'", "-", "+", "*", "/", "^", "(", ")", "[", "]",
    ",", ";", "==", "!=", "<", "<=", ">", ">=", "&&", "||", "!", "true", "false", "x", "max",
    "min", "sum", "if", "round", "sqrt", "format", "concat", "len", "\"a\"", "\\", " ", "é", "=",
    "sin",
];

fn mm() -> Context<'static> {
    Context {
        bare_unit: Some(*Unit::by_symbol("mm").unwrap()),
        ..Context::default()
    }
}

/// Evaluates `text` and checks that a failure is a registered error with a span inside the text.
fn survive(text: &str, context: &Context<'_>) -> Result<Value> {
    let result = evaluate(text, context);
    if let Err(error) = &result {
        assert!(error.code().starts_with("solve."), "{text:?}: {error}");
        let span = span_of(error).unwrap_or_else(|| panic!("{text:?}: no span on {error}"));
        assert!(
            span.start <= span.end && span.end <= text.len(),
            "{text:?}: {span:?}"
        );
        assert!(
            text.get(span.start..span.end).is_some(),
            "{text:?}: {span:?}"
        );
    }
    result
}

proptest! {
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = survive(&text, &Context::default());
        let _ = survive(&text, &mm());
        let _ = Expr::parse_localized(&text, &ExprLimits::DEFAULT, ',');
    }

    #[test]
    fn random_text_never_panics(text in "\\PC{0,80}") {
        let _ = survive(&text, &mm());
    }

    #[test]
    fn token_soups_never_panic(
        pieces in proptest::collection::vec((proptest::sample::select(&PIECES[..]), any::<bool>()), 0..40),
    ) {
        let mut text = String::new();
        for (piece, space) in pieces {
            text.push_str(piece);
            if space {
                text.push(' ');
            }
        }
        let _ = survive(&text, &Context::default());
        let _ = survive(&text, &mm());
    }

    #[test]
    fn arithmetic_on_extreme_magnitudes_is_finite_or_fails(
        a in -300_i32..=300, b in -300_i32..=300, op in proptest::sample::select(vec!["+", "-", "*", "/", "^"]),
    ) {
        let text = format!("1e{a} m {op} 3e{b}");
        match survive(&text, &mm()) {
            Ok(Value::Number(quantity)) => prop_assert!(quantity.value().is_finite(), "{text}"),
            Ok(other) => prop_assert!(false, "{text}: {other:?}"),
            Err(error) => prop_assert!(
                ["solve.expr.domain", "solve.expr.dimension_mismatch", "solve.expr.limit_exceeded"]
                    .contains(&error.code()),
                "{text}: {error}"
            ),
        }
    }

    #[test]
    fn deep_nesting_fails_with_the_limit(
        depth in 65_usize..20_000,
        shape in proptest::sample::select(vec![("(", "1", ")"), ("[", "1", "]"), ("-", "1", ""),
            ("!", "true", ""), ("abs(", "1", ")"), ("1+", "1", ""), ("2^", "2", ""), ("1 m*(", "1", ")")]),
    ) {
        let (open, core, close) = shape;
        let text = format!("{}{core}{}", open.repeat(depth), close.repeat(depth));
        let error = survive(&text, &Context::default()).unwrap_err();
        prop_assert_eq!(error.code(), "solve.expr.limit_exceeded");
    }
}

#[test]
fn a_million_levels_do_not_overflow_the_stack_under_any_limits() {
    let unbounded = ExprLimits {
        text_bytes: usize::MAX,
        depth: u32::MAX,
        list_length: usize::MAX,
        steps: u64::MAX,
    };
    let context = Context {
        limits: unbounded,
        ..Context::default()
    };
    for (open, core, close) in [
        ("(", "1", ")"),
        ("[", "", "]"),
        ("-", "1", ""),
        ("1+", "1", ""),
        ("f(", "", ")"),
    ] {
        let text = format!(
            "{}{core}{}",
            open.repeat(1_000_000),
            close.repeat(1_000_000)
        );
        let error = survive(&text, &context).unwrap_err();
        assert_eq!(error.code(), "solve.expr.limit_exceeded", "{open}");
    }
    // The cap leaves ordinary nesting alone.
    let text = format!("{}1{}", "(".repeat(120), ")".repeat(120));
    assert!(survive(&text, &context).is_ok());
}

#[test]
fn long_inputs_fail_with_the_limit_and_long_lists_are_counted() {
    let long = "1+".repeat(40_000) + "1";
    let error = survive(&long, &Context::default()).unwrap_err();
    assert_eq!(error.code(), "solve.expr.limit_exceeded");
    assert_eq!(error.details().get("limit"), Some(&"text_bytes".into()));

    let list = format!("sum([{}])", vec!["1"; 10_001].join(","));
    let error = survive(&list, &Context::default()).unwrap_err();
    assert_eq!(error.details().get("limit"), Some(&"list_length".into()));
    let list = format!("sum([{}])", vec!["1 mm"; 10_000].join(","));
    assert!(survive(&list, &Context::default()).is_ok());
    let few_steps = Context {
        limits: ExprLimits {
            steps: 15_000,
            ..ExprLimits::DEFAULT
        },
        ..Context::default()
    };
    let error = survive(&list, &few_steps).unwrap_err();
    assert_eq!(error.details().get("limit"), Some(&"steps".into()));

    let digits = "9".repeat(60_000);
    assert_eq!(
        survive(&digits, &Context::default()).unwrap_err().code(),
        "solve.expr.domain"
    );
    let small = format!("0.{}1", "0".repeat(60_000));
    assert!(survive(&small, &Context::default()).is_ok());
}

#[test]
fn the_deepest_allowed_nesting_fits_a_one_mebibyte_stack() {
    let worker = std::thread::Builder::new().stack_size(1 << 20).spawn(|| {
        let context = Context {
            limits: ExprLimits {
                depth: u32::MAX,
                ..ExprLimits::DEFAULT
            },
            ..Context::default()
        };
        let levels = ExprLimits::MAX_DEPTH as usize - 2;
        for (open, core, close) in [
            ("(", "1", ")"),
            ("[", "1", "]"),
            ("-", "1", ""),
            ("abs(", "1", ")"),
            ("if(true,", "1", ",2)"),
            ("1+", "1", ""),
            ("1^", "1", ""),
            ("true&&", "true", ""),
        ] {
            let text = format!("{}{core}{}", open.repeat(levels), close.repeat(levels));
            assert!(evaluate(&text, &context).is_ok(), "{open}");
        }
    });
    worker.unwrap().join().unwrap();
}
