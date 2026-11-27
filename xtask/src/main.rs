//! Repository tasks of `arc-core`, run as `cargo xtask <task>`:
//!
//! - `layers` checks the workspace against `xtask/layers.toml` (ADR-0001).
//! - `wasm` builds the WebAssembly set for `wasm32-unknown-unknown`.
//! - `features`, `catalogs` and `schemas` run one gate of `ci` alone.
//! - `ifc-schema --schema-dir <dir> [--check]` generates the IFC schema tables of `arc-io-ifc` from
//!   the published EXPRESS schemas in `<dir>` (ADR-0018); see [`ifc_schema`].
//! - `ifc-psets --psd-dir <dir> [--check]` generates the property and quantity set templates of
//!   `arc-io-ifc` from the published property set definitions in `<dir>`; see [`ifc_pset`].
//! - `ci` runs every gate in order; it is the single command CI runs: format, clippy, layers,
//!   wasm, tests (nextest and documentation tests), the feature matrix, `cargo deny`,
//!   `cargo audit`, the localization catalog check and the schema check.

// A command-line tool reports on the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod catalog;
mod features;
mod ifc_pset;
mod ifc_schema;
mod layers;
mod schemas;
mod workspace;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use workspace::Failure;

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "layers" => run_layers(),
        "wasm" => run_wasm(),
        "features" => run_features(),
        "catalogs" => run_catalogs(),
        "schemas" => run_schemas(),
        "ci" => run_ci(),
        "ifc-schema" => ifc_schema::run(&std::env::args().skip(2).collect::<Vec<_>>()),
        "ifc-psets" => ifc_pset::run(&std::env::args().skip(2).collect::<Vec<_>>()),
        _ => Err(
            "usage: cargo xtask <layers|wasm|features|catalogs|schemas|ci|ifc-schema|ifc-psets>"
                .to_owned(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("xtask {task}: {failure}");
            ExitCode::FAILURE
        }
    }
}

fn run_layers() -> Result<(), Failure> {
    let table = workspace::load_table()?;
    let facts = workspace::load_facts()?;
    let violations = layers::check(&table, &facts);
    for violation in &violations {
        eprintln!("{violation}");
    }
    if violations.is_empty() {
        println!(
            "layers: {} crates respect the layer table",
            facts.workspace.len()
        );
        Ok(())
    } else {
        Err(format!("{} layering violation(s)", violations.len()))
    }
}

fn run_wasm() -> Result<(), Failure> {
    let table = workspace::load_table()?;
    let facts = workspace::load_facts()?;
    // The table lists the target set; only crates that already exist are built.
    let existing = |name: &&String| facts.workspace.iter().any(|c| &c.name == *name);
    let mut args = vec!["build", "--target", "wasm32-unknown-unknown"];
    for name in table.wasm.iter().filter(existing) {
        args.extend(["--package", name]);
    }
    cargo(&args)
}

fn run_ci() -> Result<(), Failure> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ])?;
    run_layers()?;
    run_wasm()?;
    // Feature-gated code (the `package` feature of arc-doc) is linted and tested too; the
    // WebAssembly step builds without features.
    cargo(&["nextest", "run", "--workspace", "--all-features"])?;
    // nextest does not run documentation tests.
    cargo(&["test", "--workspace", "--doc"])?;
    run_features()?;
    cargo(&["deny", "check"])?;
    cargo(&["audit"])?;
    run_catalogs()?;
    run_schemas()?;
    println!("ci: all gates passed");
    Ok(())
}

fn run_features() -> Result<(), Failure> {
    let mut ran = false;
    for member in workspace::load_members()? {
        for invocation in features::invocations(&member.name, &member.features) {
            let mut args = vec!["check"];
            args.extend(invocation.iter().map(String::as_str));
            cargo(&args)?;
            ran = true;
        }
    }
    if !ran {
        println!("features: no crate declares features, nothing to check");
    }
    Ok(())
}

fn run_catalogs() -> Result<(), Failure> {
    let members = workspace::load_members()?;
    let dir = workspace::root().join("locales").join("en-US");
    let mut problems = Vec::new();
    let mut checked = 0;
    for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if path.extension().is_some_and(|e| e == "ftl") && !members.iter().any(|m| m.name == stem) {
            problems.push(format!("{}: catalog without a crate", path.display()));
        }
    }
    for member in &members {
        let path = dir.join(format!("{}.ftl", member.name));
        let text = fs::read_to_string(&path).ok();
        let mut ids = Vec::new();
        for file in rust_files(&member.dir.join("src"))? {
            let source =
                fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            ids.extend(catalog::referenced(&source));
        }
        checked += usize::from(text.is_some());
        problems.extend(catalog::check(&member.name, text.as_deref(), &ids));
    }
    problems.extend(check_locales(&dir)?);
    report(
        "catalogs",
        checked,
        "catalog(s) match their crates",
        problems,
    )
}

/// Checks every locale directory next to `en_us` against it.
fn check_locales(en_us: &Path) -> Result<Vec<String>, Failure> {
    let Some(locales) = en_us.parent() else {
        return Ok(Vec::new());
    };
    let list = |dir: &Path| -> Result<Vec<String>, Failure> {
        let mut names = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    };
    let english = list(en_us)?;
    let mut problems = Vec::new();
    for locale in list(locales)? {
        let dir = locales.join(&locale);
        if locale == "en-US" || !dir.is_dir() {
            continue;
        }
        let present = list(&dir)?;
        for file in english.iter().filter(|f| !present.contains(f)) {
            problems.push(format!("{locale}/{file}: file is missing"));
        }
        for file in present.iter().filter(|f| !english.contains(f)) {
            problems.push(format!("{locale}/{file}: file does not exist in en-US"));
        }
        for file in english.iter().filter(|f| present.contains(f)) {
            let read = |d: &Path| {
                fs::read_to_string(d.join(file)).map_err(|e| format!("{locale}/{file}: {e}"))
            };
            problems.extend(catalog::check_translation(
                &format!("{locale}/{file}"),
                &read(en_us)?,
                &read(&dir)?,
            ));
        }
    }
    Ok(problems)
}

fn run_schemas() -> Result<(), Failure> {
    let dir = workspace::root().join("schemas");
    if !dir.is_dir() {
        println!("schemas: no schemas/ directory, nothing to check");
        return Ok(());
    }
    let mut problems = Vec::new();
    let mut checked = 0;
    let mut pending = vec![dir];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                let text =
                    fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                checked += 1;
                problems.extend(schemas::check(&path.display().to_string(), &text));
            }
        }
    }
    report("schemas", checked, "schema(s) are well formed", problems)
}

fn report(task: &str, checked: usize, ok: &str, problems: Vec<String>) -> Result<(), Failure> {
    for problem in &problems {
        eprintln!("{problem}");
    }
    if problems.is_empty() {
        println!("{task}: {checked} {ok}");
        Ok(())
    } else {
        Err(format!("{} problem(s) in {task}", problems.len()))
    }
}

/// Every `.rs` file below `dir`; a missing directory has none.
fn rust_files(dir: &Path) -> Result<Vec<PathBuf>, Failure> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(out);
    };
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.is_dir() {
            out.extend(rust_files(&path)?);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(out)
}

fn cargo(args: &[&str]) -> Result<(), Failure> {
    let line = format!("cargo {}", args.join(" "));
    println!("==> {line}");
    let status = Command::new(workspace::cargo())
        .args(args)
        .current_dir(workspace::root())
        .status()
        .map_err(|e| format!("{line}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{line}` failed ({status})"))
    }
}
