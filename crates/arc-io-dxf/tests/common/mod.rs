//! Helpers shared by the integration tests.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::panic,
    clippy::collapsible_if,
    reason = "test helpers"
)]

use arc_io_dxf::{ValueKind, value_kind};
use serde_json::Value as Json;

/// The bytes of a golden file under `tests/data`.
pub fn golden(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(path).unwrap()
}

/// Rewrites an ASCII file as binary DXF, with one-byte group codes (`short`) or two-byte ones.
/// Text keeps its raw bytes, so the encoding of the file is unchanged.
pub fn to_binary(ascii: &[u8], short: bool) -> Vec<u8> {
    let mut lines = ascii
        .split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    let mut out = b"AutoCAD Binary DXF\r\n\x1a\0".to_vec();
    while let Some(code_line) = lines.next() {
        let code_text = std::str::from_utf8(code_line).unwrap().trim();
        if code_text.is_empty() {
            break;
        }
        let code: i16 = code_text.parse().unwrap();
        let value = lines.next().unwrap();
        if short {
            if (0..255).contains(&code) {
                out.push(u8::try_from(code).unwrap());
            } else {
                out.push(255);
                out.extend_from_slice(&code.to_le_bytes());
            }
        } else {
            out.extend_from_slice(&code.to_le_bytes());
        }
        let number = || std::str::from_utf8(value).unwrap().trim();
        match value_kind(code).unwrap() {
            ValueKind::Text => {
                out.extend_from_slice(value);
                out.push(0);
            }
            ValueKind::Double => {
                out.extend_from_slice(&number().parse::<f64>().unwrap().to_le_bytes())
            }
            ValueKind::Int8 => out.push(number().parse::<i16>().unwrap().to_le_bytes()[0]),
            ValueKind::Int16 => {
                out.extend_from_slice(&number().parse::<i16>().unwrap().to_le_bytes())
            }
            ValueKind::Int32 => {
                out.extend_from_slice(&number().parse::<i32>().unwrap().to_le_bytes())
            }
            ValueKind::Int64 => {
                out.extend_from_slice(&number().parse::<i64>().unwrap().to_le_bytes())
            }
            ValueKind::Bool => out.push(u8::from(number() != "0")),
            ValueKind::Bytes => {
                let hex = number();
                let bytes: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect();
                out.push(u8::try_from(bytes.len()).unwrap());
                out.extend_from_slice(&bytes);
            }
        }
    }
    out
}

/// Checks `value` against a JSON Schema, supporting the keywords the repository's schemas use:
/// `$ref` to `#/$defs`, `type`, `enum`, `const`, `required`, `properties`,
/// `additionalProperties`, `items`, `oneOf`, `minimum` and `pattern` (prefix anchors only checked
/// for non-emptiness). Returns the paths that fail.
pub fn validate(schema: &Json, root: &Json, value: &Json, path: &str, errors: &mut Vec<String>) {
    if let Some(reference) = schema.get("$ref").and_then(Json::as_str) {
        let name = reference.strip_prefix("#/$defs/").unwrap();
        return validate(&root["$defs"][name], root, value, path, errors);
    }
    if let Some(types) = schema.get("type") {
        let names: Vec<&str> = match types {
            Json::String(s) => vec![s.as_str()],
            Json::Array(a) => a.iter().map(|t| t.as_str().unwrap()).collect(),
            _ => panic!("bad type keyword"),
        };
        let ok = names.iter().any(|t| match *t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            other => panic!("unknown type {other}"),
        });
        if !ok {
            errors.push(format!("{path}: type {types}"));
            return;
        }
    }
    if let Some(options) = schema.get("enum").and_then(Json::as_array) {
        if !options.contains(value) {
            errors.push(format!("{path}: not in enum"));
        }
    }
    if let Some(expected) = schema.get("const") {
        if expected != value {
            errors.push(format!("{path}: not the constant"));
        }
    }
    if let Some(minimum) = schema.get("minimum").and_then(Json::as_f64) {
        if value.as_f64().is_some_and(|v| v < minimum) {
            errors.push(format!("{path}: below minimum"));
        }
    }
    if schema.get("pattern").is_some() && value.as_str().is_some_and(str::is_empty) {
        errors.push(format!("{path}: empty string for a pattern"));
    }
    if let Some(options) = schema.get("oneOf").and_then(Json::as_array) {
        let matching = options
            .iter()
            .filter(|option| {
                let mut e = Vec::new();
                validate(option, root, value, path, &mut e);
                e.is_empty()
            })
            .count();
        if matching != 1 {
            errors.push(format!("{path}: {matching} oneOf branches match"));
        }
    }
    if let Some(object) = value.as_object() {
        for key in schema
            .get("required")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if !object.contains_key(key.as_str().unwrap()) {
                errors.push(format!("{path}: missing {key}"));
            }
        }
        let properties = schema.get("properties").and_then(Json::as_object);
        for (key, member) in object {
            let member_path = format!("{path}/{key}");
            match properties.and_then(|p| p.get(key)) {
                Some(sub) => validate(sub, root, member, &member_path, errors),
                None => match schema.get("additionalProperties") {
                    Some(Json::Bool(false)) => errors.push(format!("{member_path}: not allowed")),
                    Some(sub @ Json::Object(_)) => {
                        validate(sub, root, member, &member_path, errors)
                    }
                    _ => {}
                },
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, item) in array.iter().enumerate() {
            validate(items, root, item, &format!("{path}/{i}"), errors);
        }
    }
}

/// Validates a loss report against `schemas/loss-report.schema.json`.
pub fn assert_report_valid(report: &arc_io_dxf::LossReport) {
    let text = std::fs::read_to_string(format!(
        "{}/../../schemas/loss-report.schema.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let schema: Json = serde_json::from_str(&text).unwrap();
    let value = serde_json::to_value(report).unwrap();
    let mut errors = Vec::new();
    validate(&schema, &schema, &value, "", &mut errors);
    assert!(errors.is_empty(), "{errors:?}\n{value:#}");
}
