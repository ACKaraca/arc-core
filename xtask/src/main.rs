//! Repository tasks of `arc-core`, run as `cargo xtask <task>`:
//!
//! - `layers` checks the workspace against `xtask/layers.toml` (ADR-0001).
//! - `wasm` builds the WebAssembly set for `wasm32-unknown-unknown`.
//! - `ci` runs every gate in order; it is the single command CI runs.

// A command-line tool reports on the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod layers;
mod workspace;

use std::process::{Command, ExitCode};

use workspace::Failure;

fn main() -> ExitCode {
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "layers" => run_layers(),
        "wasm" => run_wasm(),
        "ci" => run_ci(),
        _ => Err("usage: cargo xtask <layers|wasm|ci>".to_owned()),
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
        "--",
        "-D",
        "warnings",
    ])?;
    run_layers()?;
    run_wasm()?;
    cargo(&["nextest", "run", "--workspace"])?;
    // nextest does not run documentation tests.
    cargo(&["test", "--workspace", "--doc"])?;
    cargo(&["deny", "check"])?;
    println!("ci: all gates passed");
    Ok(())
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
