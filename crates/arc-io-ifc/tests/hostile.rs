//! Hostile input: arbitrary bytes, token soup and mutations of a valid file return `Ok` or an
//! `io.*` / `base.*` error and never panic; and a parameter tree printed by the test printer reads
//! back to the same parameters.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

#[path = "support/mod.rs"]
mod support;

use arc_base::{Cancel, Limits};
use arc_io_ifc::{Parameter, ReadOptions, read};
use proptest::prelude::*;

/// Prints a parameter in the physical file's syntax. Text is written with its apostrophes and
/// backslashes doubled; reals always carry a point and use `E` for the exponent.
fn print(parameter: &Parameter) -> String {
    match parameter {
        Parameter::Integer(value) => value.to_string(),
        Parameter::Real(value) => {
            let text = format!("{value:?}");
            match text.split_once('e') {
                Some((mantissa, exponent)) if mantissa.contains('.') => {
                    format!("{mantissa}E{exponent}")
                }
                Some((mantissa, exponent)) => format!("{mantissa}.0E{exponent}"),
                None => text,
            }
        }
        Parameter::Text(text) => {
            format!("'{}'", text.replace('\\', "\\\\").replace('\'', "''"))
        }
        Parameter::Enumeration(name) => format!(".{name}."),
        Parameter::Binary(digits) => format!("\"{digits}\""),
        Parameter::Reference(name) => format!("#{name}"),
        Parameter::Omitted => "$".to_owned(),
        Parameter::Derived => "*".to_owned(),
        Parameter::Typed { type_name, value } => format!("{type_name}({})", print(value)),
        Parameter::List(items) => {
            let inner: Vec<String> = items.iter().map(print).collect();
            format!("({})", inner.join(","))
        }
    }
}

fn parameter() -> impl Strategy<Value = Parameter> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(Parameter::Integer),
        any::<f64>()
            .prop_filter("finite", |v| v.is_finite())
            .prop_map(Parameter::Real),
        "[ -~é]{0,24}".prop_map(Parameter::Text),
        "[A-Z][A-Z0-9_]{0,8}".prop_map(Parameter::Enumeration),
        "0[0-9A-F]{0,8}".prop_map(Parameter::Binary),
        (1u64..100_000).prop_map(Parameter::Reference),
        Just(Parameter::Omitted),
        Just(Parameter::Derived),
    ];
    leaf.prop_recursive(4, 40, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Parameter::List),
            ("[A-Z][A-Z0-9]{0,6}", inner).prop_map(|(type_name, value)| Parameter::Typed {
                type_name,
                value: Box::new(value),
            }),
        ]
    })
}

/// Each outcome is `Ok` or an error of this family. A file that reads writes back byte for byte,
/// and its first instance, rewritten in the canonical form, reads back to the same records.
fn assert_clean(bytes: &[u8]) {
    match read(bytes, &mut ReadOptions::default()) {
        Ok(output) => {
            let _ = output
                .file
                .typed_views(&Limits::CONSTRAINED, &Cancel::new());
            let mut file = output.file;
            if !bytes.starts_with(b"PK") {
                assert_eq!(file.write_step(&Limits::LARGE_MODELS).unwrap(), bytes);
            }
            let Some(first) = file.instances().first().cloned() else {
                return;
            };
            // A record the reader accepted may hold a reference it cannot resolve; the edit
            // refuses that with a typed error.
            match file.replace(first.name, first.records.clone()) {
                Ok(()) => {
                    let written = file.write_step(&Limits::LARGE_MODELS).unwrap();
                    let again = read(&written, &mut ReadOptions::default()).unwrap();
                    assert_eq!(again.file.instance(first.name), Some(&first));
                }
                Err(error) => assert!(error.code().starts_with("io.ifc.edit_"), "{error:?}"),
            }
        }
        Err(error) => {
            let code = error.code();
            assert!(
                code.starts_with("io.") || code.starts_with("base."),
                "unexpected code {code}"
            );
        }
    }
}

const TOKENS: &[&str] = &[
    "#", "#1", "=", "(", ")", ",", ";", "'", "''", "\\", "\\X2\\", "\\X0\\", "\\S\\", "\\PA\\",
    "/*", "*/", "$", "*", ".", ".T.", "IFCX", "IFCWALL", "\"0A\"", "1", "-", "+", "2.5E3", "E",
    " ", "\n", "é",
];

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        assert_clean(&bytes);
    }

    #[test]
    fn token_soup_never_panics(
        tokens in proptest::collection::vec(proptest::sample::select(TOKENS), 0..80)
    ) {
        let body: String = tokens.concat();
        let text = format!(
            "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n{body}\nENDSEC;\nEND-ISO-10303-21;\n"
        );
        assert_clean(text.as_bytes());
    }

    #[test]
    fn mutations_of_the_valid_file_never_panic(
        edits in proptest::collection::vec((any::<usize>(), any::<u8>(), 0u8..3), 1..8)
    ) {
        let mut bytes = support::project("IFC4").into_bytes();
        for (index, value, kind) in edits {
            if bytes.is_empty() {
                break;
            }
            let at = index % bytes.len();
            match kind {
                0 => bytes[at] = value,
                1 => {
                    bytes.remove(at);
                }
                _ => bytes.insert(at, value),
            }
        }
        assert_clean(&bytes);
    }

    #[test]
    fn archive_signature_followed_by_noise_never_panics(
        tail in proptest::collection::vec(any::<u8>(), 0..256)
    ) {
        let mut bytes = b"PK\x03\x04".to_vec();
        bytes.extend(tail);
        assert_clean(&bytes);
    }

    #[test]
    fn printed_parameters_read_back_equal(
        parameters in proptest::collection::vec(parameter(), 0..6)
    ) {
        let printed: Vec<String> = parameters.iter().map(print).collect();
        let text = format!(
            "ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n#1=IFCTEST({});\nENDSEC;\nEND-ISO-10303-21;\n",
            printed.join(",")
        );
        let output = read(text.as_bytes(), &mut ReadOptions::default()).unwrap();
        let read_back = &output.file.instance(1).unwrap().records[0].parameters;
        prop_assert_eq!(read_back, &parameters);
    }
}
