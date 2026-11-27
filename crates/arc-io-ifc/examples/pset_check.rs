//! Compares the property and quantity sets of IFC files with the published templates and prints
//! the findings as JSON lines, one per file: `cargo run -p arc-io-ifc --example pset_check <files>`.
//! Exits 1 when a file has a finding or cannot be read. Not part of any shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool of the oracle"
)]

use arc_base::{Cancel, Limits};
use arc_io_ifc::{ReadOptions, read};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: pset_check <file.ifc>...");
        std::process::exit(2);
    }
    let mut failed = false;
    for path in &paths {
        match check(path) {
            Ok((line, clean)) => {
                println!("{line}");
                failed |= !clean;
            }
            Err(message) => {
                eprintln!("{path}: {message}");
                failed = true;
            }
        }
    }
    std::process::exit(i32::from(failed));
}

fn check(path: &str) -> Result<(String, bool), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let file = read(&bytes, &mut ReadOptions::default())
        .map_err(|e| e.to_string())?
        .file;
    let result = file
        .check_property_sets(&Limits::DEFAULT, &Cancel::new())
        .map_err(|e| e.to_string())?;
    let findings: Vec<String> = result
        .findings
        .iter()
        .map(|f| {
            format!(
                "{{\"code\":{:?},\"set\":{:?},\"property\":{:?},\"expected\":{:?},\"found\":{:?}}}",
                f.code.code, f.set_name, f.property, f.expected, f.found
            )
        })
        .collect();
    let line = format!(
        "{{\"file\":{path:?},\"templates\":{:?},\"sets_checked\":{},\"findings\":[{}]}}",
        result.templates.unwrap_or(""),
        result.sets_checked,
        findings.join(",")
    );
    Ok((line, result.findings.is_empty()))
}
