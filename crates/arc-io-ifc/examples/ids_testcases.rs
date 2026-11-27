//! Runs the IDS checker over a directory of IDS/IFC pairs named `pass-*`, `fail-*` and `invalid-*`
//! (the layout of the buildingSMART IDS test cases) and reports the agreement with the expected
//! outcome: `cargo run -p arc-io-ifc --example ids_testcases <directory> [--verbose]`.
//! A `pass-` pair is expected to pass, a `fail-` pair to fail, and an `invalid-` pair to be
//! rejected by the reader or to fail. Not part of any shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool of the oracle"
)]

use std::path::{Path, PathBuf};

use arc_base::{Cancel, Limits};
use arc_io_ifc::ids;
use arc_io_ifc::{ReadOptions, read};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "ids") {
            out.push(path);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verbose = args.iter().any(|a| a == "--verbose");
    let Some(dir) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: ids_testcases <directory> [--verbose]");
        std::process::exit(2);
    };
    let mut files = Vec::new();
    collect(Path::new(dir), &mut files);
    files.sort();
    let (limits, cancel) = (Limits::DEFAULT, Cancel::new());
    let mut agree = 0usize;
    let mut disagree = Vec::new();
    let mut by_kind: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    for ids_path in &files {
        let stem = ids_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let ifc_path = ids_path.with_extension("ifc");
        let expected = stem.split('-').next().unwrap_or_default().to_owned();
        let outcome = (|| -> Result<String, String> {
            let ids_bytes = std::fs::read(ids_path).map_err(|e| e.to_string())?;
            let document = match ids::read(&ids_bytes, &limits, &cancel) {
                Ok(d) => d,
                Err(e) => return Ok(format!("rejected ({}) {e:?}", e.code())),
            };
            let ifc_bytes = std::fs::read(&ifc_path).map_err(|e| e.to_string())?;
            let file = read(&ifc_bytes, &mut ReadOptions::default())
                .map_err(|e| format!("ifc: {}", e.code()))?
                .file;
            let report = document
                .check(&file, &limits, &cancel)
                .map_err(|e| format!("check: {}", e.code()))?;
            let detail: Vec<String> = report
                .specifications
                .iter()
                .map(|s| {
                    format!(
                        "{:?} applicable={} failures={} {}",
                        s.status,
                        s.applicable,
                        s.failure_count,
                        s.failures
                            .first()
                            .map(|f| format!("[{} #{}: {}]", f.facet, f.instance, f.reason))
                            .or_else(|| s.reason.clone())
                            .unwrap_or_default()
                    )
                })
                .collect();
            Ok(format!(
                "{} {}",
                if report.passed() { "pass" } else { "fail" },
                detail.join("; ")
            ))
        })();
        let text = outcome.unwrap_or_else(|e| format!("error {e}"));
        let actual = text.split_whitespace().next().unwrap_or_default();
        let ok = match expected.as_str() {
            "pass" => actual == "pass",
            "fail" => actual == "fail",
            _ => actual == "fail" || actual == "rejected",
        };
        let kind = ids_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let entry = by_kind.entry(kind.clone()).or_default();
        entry.1 += 1;
        if ok {
            agree += 1;
            entry.0 += 1;
        } else {
            disagree.push(format!("{kind}/{stem}: expected {expected}, got {text}"));
        }
        if verbose {
            println!("{} {kind}/{stem}: {text}", if ok { "ok  " } else { "DIFF" });
        }
    }
    for (kind, (a, n)) in &by_kind {
        println!("{kind:16} {a:4} / {n:4}");
    }
    println!("agreement {agree} / {}", files.len());
    for d in &disagree {
        println!("DISAGREE {d}");
    }
}
