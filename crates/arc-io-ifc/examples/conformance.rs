//! Prints the conformance findings and rule counts of IFC files as JSON, for the oracle in
//! `tools/ifc-oracle/conformance_compare.py`: `cargo run -p arc-io-ifc --example conformance
//! <file>...`. Not part of any shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool of the oracle"
)]

use arc_base::{Cancel, Limits};
use arc_io_ifc::{ReadOptions, read};
use serde_json::{Value, json};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: conformance <file.ifc>...");
        std::process::exit(2);
    }
    let files: serde_json::Map<String, Value> = paths
        .iter()
        .map(|path| {
            (
                path.clone(),
                run(path).unwrap_or_else(|e| json!({ "error": e })),
            )
        })
        .collect();
    println!("{}", Value::Object(files));
}

fn run(path: &str) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let file = read(&bytes, &mut ReadOptions::default())
        .map_err(|e| e.to_string())?
        .file;
    let start = std::time::Instant::now();
    let result = file
        .conformance(&Limits::DEFAULT, &Cancel::new())
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "seconds": start.elapsed().as_secs_f64(),
        "schema": result.schema,
        "truncated": result.truncated,
        "findings": result.findings.iter().map(|f| json!({
            "instance": f.instance,
            "entity": f.entity,
            "attribute": f.attribute,
            "rule": f.rule.code,
        })).collect::<Vec<_>>(),
        "rules": result.rules.iter().map(|c| json!({
            "rule": c.rule.code,
            "passed": c.passed,
            "failed": c.failed,
            "not_evaluated": c.not_evaluated,
        })).collect::<Vec<_>>(),
        "where_rules": result.where_rules.iter().map(|c| json!({
            "rule": c.name(),
            "scope": format!("{:?}", c.scope),
            "passed": c.passed,
            "unknown": c.unknown,
            "failed": c.failed,
            "not_evaluated": c.not_evaluated,
            "reason": c.reason,
        })).collect::<Vec<_>>(),
    }))
}
