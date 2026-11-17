//! No input makes the parser or the evaluator panic, overflow the stack or run unbounded
//! (CORE-SOLVE-021).

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test helpers fail by panicking"
)]

use arc_base::{Result, Unit};
use arc_solve::error::span_of;
use arc_solve::{Context, Expr, ExprLimits, Functions, Value, evaluate};
use proptest::prelude::*;

const PIECES: [&str; 60] = [
    "1", "2.5", "1e3", "1e999", ".5", "0", "12", "mm", "m", "in", "ft", "deg", "°", "°C", "kN/m",
    "m^2", "m2", "kg*m/s^2", "'", "\"", "1/2\"", "8'", "-", "+", "*", "/", "^", "(", ")", "[", "]",
    ",", ";", "==", "!=", "<", "<=", ">", ">=", "&&", "||", "!", "true", "false", "x", "max",
    "min", "sum", "if", "round", "sqrt", "format", "concat", "len", "\"a\"", "\\", " ", "é", "=",
    "sin",
];

fn mm() -> Context<'static> {
    Context::new().with_bare_unit(*Unit::by_symbol("mm").unwrap())
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
            ("!", "true", ""), ("abs(", "1", ")"), ("1+(", "1", ")"), ("2^", "2", ""), ("1 m*(", "1", ")")]),
    ) {
        let (open, core, close) = shape;
        let text = format!("{}{core}{}", open.repeat(depth), close.repeat(depth));
        let error = survive(&text, &Context::default()).unwrap_err();
        prop_assert_eq!(error.code(), "solve.expr.limit_exceeded");
    }
}

#[test]
fn a_million_levels_do_not_overflow_the_stack_under_any_limits() {
    let unbounded = ExprLimits::DEFAULT
        .with_text_bytes(usize::MAX)
        .with_depth(u32::MAX)
        .with_list_length(usize::MAX)
        .with_steps(u64::MAX)
        .with_value_bytes(u64::MAX);
    let context = Context::new().with_limits(unbounded);
    for (open, core, close) in [
        ("(", "1", ")"),
        ("[", "", "]"),
        ("-", "1", ""),
        ("2^", "1", ""),
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
    let few_steps = Context::new().with_limits(ExprLimits::DEFAULT.with_steps(15_000));
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

/// The stack the deepest allowed expressions must fit: the documented 256 KiB in optimized
/// builds, 1 MiB in unoptimized ones.
const STACK: usize = if cfg!(debug_assertions) {
    1 << 20
} else {
    256 << 10
};

/// Every recursive construct of the grammar, as an opening, a core and a closing that nest.
const SHAPES: &[(&str, &str, &str)] = &[
    ("(", "1", ")"),
    ("[", "1", "]"),
    ("-", "1", ""),
    ("+", "1", ""),
    ("!", "true", ""),
    ("+-", "1", ""),
    ("abs(", "1", ")"),
    ("min(1,", "1", ")"),
    ("max(", "1", ",1)"),
    ("if(true,", "1", ",2)"),
    ("if(false,1,", "1", ")"),
    ("if(", "true", ",true,false)"),
    ("1+(", "1", ")"),
    ("2*(", "1", ")"),
    ("1^", "1", ""),
    ("true&&(", "true", ")"),
    ("false||(", "true", ")"),
    ("first([", "1", "])"),
    ("concat(\"a\",", "\"b\"", ")"),
    ("len([", "1", "])"),
    ("-(", "1", ")"),
    ("abs(-(", "1", "))"),
    ("-(+(", "1", "))"),
    ("pow(1,", "1", ")"),
    ("sqrt(", "1", ")"),
    ("round(", "1", ",1)"),
    ("h(", "1", ")"),
    ("x+(", "x", ")"),
    ("1 < (", "1", ")"),
    ("-40 °C+(", "1 K", ")"),
    ("(1 m)*(", "1", ")"),
];

fn host() -> Functions {
    let mut functions = Functions::new();
    functions
        .register("h", 0, 10, |args, _| {
            Ok(args.first().cloned().unwrap_or(Value::Bare(0.0)))
        })
        .unwrap();
    functions
}

fn names(name: &str) -> Result<Option<Value>> {
    Ok((name == "x").then_some(Value::Bare(1.0)))
}

#[test]
fn the_deepest_allowed_expression_of_every_shape_fits_the_documented_stack() {
    let worker = std::thread::Builder::new().stack_size(STACK).spawn(|| {
        let functions = host();
        let context = Context::new()
            .with_limits(ExprLimits::DEFAULT.with_depth(u32::MAX))
            .with_functions(&functions)
            .with_names(&names);
        for (open, core, close) in SHAPES {
            let text =
                |levels: usize| format!("{}{core}{}", open.repeat(levels), close.repeat(levels));
            let levels = (1..=ExprLimits::MAX_DEPTH as usize)
                .rev()
                .find(|levels| Expr::parse(&text(*levels), &context.limits).is_ok())
                .unwrap_or_else(|| panic!("{open}: nothing parses"));
            // One level more exceeds the limit: this is the deepest expression of the shape.
            let error = Expr::parse(&text(levels + 1), &context.limits).unwrap_err();
            assert_eq!(error.code(), "solve.expr.limit_exceeded", "{open}");
            let deepest = text(levels);
            // Some shapes fail on a value at the top (`1 < (1 < ...)` compares a truth value),
            // after evaluating every level; none may fail on a limit.
            if let Err(error) = survive(&deepest, &context) {
                assert_ne!(error.code(), "solve.expr.limit_exceeded", "{open}");
            }
        }
        // A sum of thousands of terms nests inside the deepest parentheses.
        let terms = format!("1{}", "+1".repeat(5_000));
        let levels = ExprLimits::MAX_DEPTH as usize - 2;
        let text = format!("{}{terms}{}", "(".repeat(levels), ")".repeat(levels));
        assert!(survive(&text, &context).is_ok());
    });
    worker.unwrap().join().unwrap();
}

#[test]
fn values_from_names_and_hosts_are_bounded_in_total_size() {
    let big: Value = Value::text("a".repeat(60_000));
    let lookup = move |name: &str| Ok((name == "x").then(|| big.clone()));
    let context = Context::new().with_names(&lookup);
    // Each reference copies 60 KB; 10,000 of them would hold 600 MB.
    let text = format!("len([{}])", vec!["x"; 10_000].join(","));
    let error = survive(&text, &context).unwrap_err();
    assert_eq!(error.details().get("limit"), Some(&"value_bytes".into()));
    let inner = format!("[{}]", vec!["x"; 3_000].join(","));
    let nested = format!("len([{}])", [inner.as_str(); 7].join(","));
    let error = survive(&nested, &context).unwrap_err();
    assert_eq!(error.details().get("limit"), Some(&"value_bytes".into()));
    // A few references are fine.
    assert!(survive("len(concat(x)) + len([x, x])", &context).is_ok());

    let mut functions = Functions::new();
    functions
        .register("big", 0, 0, |_, _| {
            Ok(Value::list(vec![Value::text("b".repeat(65_536)); 10_000]))
        })
        .unwrap();
    functions
        .register("rows", 0, 0, |_, _| {
            Ok(Value::list(vec![
                Value::list(vec![Value::Bare(1.0); 10_000]);
                60
            ]))
        })
        .unwrap();
    let context = Context::new().with_functions(&functions);
    for text in ["len(big())", "len(rows())"] {
        let error = survive(text, &context).unwrap_err();
        assert_eq!(
            error.details().get("limit"),
            Some(&"value_bytes".into()),
            "{text}"
        );
    }
    // Copies of parts are charged too.
    let rows = format!("len([{}])", vec!["first([[1,2,3]])"; 3_000].join(","));
    let small = Context::new().with_limits(ExprLimits::DEFAULT.with_value_bytes(100_000));
    let error = survive(&rows, &small).unwrap_err();
    assert_eq!(error.details().get("limit"), Some(&"value_bytes".into()));
    let error = survive(
        "[1, 2, 3, 4]",
        &small.with_limits(ExprLimits::DEFAULT.with_value_bytes(100)),
    );
    assert_eq!(
        error.unwrap_err().details().get("limit"),
        Some(&"value_bytes".into())
    );
}

/// A list nested `levels` deep, built without recursion.
fn nested(levels: usize) -> Value {
    let mut value = Value::Bare(1.0);
    for _ in 0..levels {
        value = Value::list(vec![value]);
    }
    value
}

#[test]
fn deeply_nested_values_from_hosts_and_names_are_rejected_and_dropped_without_recursion() {
    let worker = std::thread::Builder::new().stack_size(STACK).spawn(|| {
        let mut functions = Functions::new();
        functions
            .register("deep", 0, 0, |_, _| Ok(nested(200_000)))
            .unwrap();
        let lookup = |name: &str| Ok((name == "d").then(|| nested(200_000)));
        let context = Context::new()
            .with_functions(&functions)
            .with_names(&lookup);
        for text in ["deep()", "d", "len([d])"] {
            let error = survive(text, &context).unwrap_err();
            assert_eq!(
                error.details().get("limit"),
                Some(&"depth".into()),
                "{text}"
            );
        }
        // At the depth limit a value is admitted.
        let lookup = |_: &str| Ok(Some(nested(63)));
        assert!(survive("first(x)", &Context::new().with_names(&lookup)).is_ok());
    });
    worker.unwrap().join().unwrap();
}

/// A small deterministic generator of random numbers for the expression generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }

    fn pick<'t>(&mut self, items: &[&'t str]) -> &'t str {
        items[self.below(items.len())]
    }
}

const ATOMS: &[&str] = &[
    "1",
    "0",
    "2.5",
    ".5",
    "1e3",
    "1e308",
    "1e-308",
    "1e999",
    "007",
    "-40 °C",
    "20°C",
    "3 in",
    "1200 mm",
    "8' 6\"",
    "8'-6\"",
    "8' 6 1/2\"",
    "1/2\"",
    "1/0\"",
    "8' 13\"",
    "2 m^2",
    "2 m2",
    "2 m^-2",
    "2 m^+2",
    "1 kN/m",
    "1 kg*m/s^2",
    "1 m/s/s",
    "90°",
    "45 deg",
    "30′",
    "1 rad",
    "1 min",
    "1 h",
    "1 ft·lbf",
    "1 °F",
    "1 K",
    "2 μm",
    "true",
    "false",
    "\"a\"",
    "\"\\n\"",
    "\"µ°³\"",
    "x",
    "y",
    "[]",
    "[1]",
    "[1 mm, 2]",
    "2 m^127",
    "1 in^-128",
    "1e-320",
    "1 L",
];
const OPERATORS: &[&str] = &[
    "+", "-", "*", "/", "^", "==", "!=", "<", "<=", ">", ">=", "&&", "||",
];
const FUNCTIONS: &[&str] = &[
    "abs", "min", "max", "sqrt", "pow", "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
    "round", "floor", "ceil", "if", "len", "sum", "first", "last", "format", "concat", "h", "dup",
    "nope",
];

/// Appends a random expression of at most `depth` levels, with deep runs of one construct.
fn generate(rng: &mut Rng, depth: u32, out: &mut String) {
    if depth == 0 || rng.below(4) == 0 {
        out.push_str(rng.pick(ATOMS));
        return;
    }
    match rng.below(7) {
        0 => {
            out.push('(');
            generate(rng, depth - 1, out);
            out.push(')');
        }
        1 => {
            out.push_str(rng.pick(&["-", "!", "+", "- "]));
            generate(rng, depth - 1, out);
        }
        2 | 3 => {
            generate(rng, depth - 1, out);
            out.push_str(rng.pick(&[" ", ""]));
            out.push_str(rng.pick(OPERATORS));
            out.push_str(rng.pick(&[" ", ""]));
            generate(rng, depth - 1, out);
        }
        4 => {
            out.push_str(rng.pick(FUNCTIONS));
            out.push('(');
            for i in 0..rng.below(4) {
                if i > 0 {
                    out.push_str(rng.pick(&[",", ";", ", "]));
                }
                generate(rng, depth - 1, out);
            }
            out.push(')');
        }
        5 => {
            out.push('[');
            for i in 0..rng.below(4) {
                if i > 0 {
                    out.push(',');
                }
                generate(rng, depth - 1, out);
            }
            out.push(']');
        }
        _ => {
            let (open, close) = [
                ("(", ")"),
                ("-", ""),
                ("2^", ""),
                ("[", "]"),
                ("abs(", ")"),
                ("if(true,", ",1)"),
                ("1+", ""),
            ][rng.below(7)];
            let levels = rng.below(200);
            out.push_str(&open.repeat(levels));
            generate(rng, depth - 1, out);
            out.push_str(&close.repeat(levels));
        }
    }
}

/// Generated expressions, optionally with a few bytes mutated.
fn fuzz_case(seed: u64, mutate: bool) -> String {
    let mut rng = Rng(seed | 1);
    let mut text = String::new();
    let depth = 1 + u32::try_from(rng.below(12)).unwrap();
    generate(&mut rng, depth, &mut text);
    if !mutate {
        return text;
    }
    let mut bytes = text.into_bytes();
    for _ in 0..1 + rng.below(4) {
        if bytes.is_empty() {
            break;
        }
        let at = rng.below(bytes.len());
        match rng.below(4) {
            0 => bytes[at] = u8::try_from(rng.next() % 256).unwrap(),
            1 => {
                bytes.remove(at);
            }
            2 => {
                let copy = bytes[rng.below(bytes.len())];
                bytes.insert(at, copy);
            }
            _ => {
                let other = rng.below(bytes.len());
                bytes.swap(at, other);
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

proptest! {
    #[test]
    fn generated_and_mutated_expressions_never_panic(seed: u64, mutate: bool, profile in 0_usize..6) {
        let text = fuzz_case(seed, mutate);
        let worker = std::thread::Builder::new().stack_size(STACK).spawn(move || {
            let mut functions = host();
            functions
                .register("dup", 1, 1, |args, _| {
                    Ok(Value::list(vec![args[0].clone(), args[0].clone()]))
                })
                .unwrap();
            let lookup = |name: &str| {
                Ok(match name {
                    "x" => Some(Value::Bare(2.0)),
                    "y" => Some(Value::text("yy")),
                    _ => None,
                })
            };
            let unit = [None, Some("mm"), Some("°C")][profile % 3];
            let depth = [64, u32::MAX][profile / 3];
            let mut context = Context::new()
                .with_limits(ExprLimits::DEFAULT.with_depth(depth))
                .with_functions(&functions)
                .with_names(&lookup);
            if let Some(symbol) = unit {
                context = context.with_bare_unit(*Unit::by_symbol(symbol).unwrap());
            }
            let _ = survive(&text, &context);
            for separator in [',', '٫'] {
                let _ = Expr::parse_localized(&text, &context.limits, separator)
                    .and_then(|e| e.evaluate(&context));
            }
        });
        worker.unwrap().join().unwrap();
    }
}
