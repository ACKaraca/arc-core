//! Scripted runs from nothing (`--create`), the document setup hook of the shared runner, the
//! undo and redo of a served session, and the refusal to record a session that stepped the
//! history (CORE-CMD-031).

#![allow(clippy::unwrap_used, reason = "tests")]

use std::ffi::OsString;
use std::io::{BufRead, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arc_base::Error;
use arc_base::error::{Category, ErrorCode};
use arc_command::FixedClock;
use arc_command::headless::{self, DocumentSetup, Exit};
use arc_command::protocol::{PipeReader, pipe};
use arc_doc::package::{LockOwner, Metadata, Package};
use arc_doc::{Document, Finding, Severity};
use serde_json::{Value, json};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "arc-cli-scripted-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn owner() -> LockOwner {
    LockOwner {
        user: "test".into(),
        host: "test".into(),
        application: "scripted".into(),
        time_unix_ms: 0,
    }
}

fn product() -> headless::Product {
    arc_shell_cli::product(Arc::new(FixedClock(0)), owner()).unwrap()
}

/// The words of a runner invocation: `head`, the paths, then `tail`.
fn words(head: &[&str], paths: &[&Path], tail: &[&str]) -> Vec<OsString> {
    let mut out: Vec<OsString> = head.iter().map(OsString::from).collect();
    out.extend(paths.iter().map(|p| p.as_os_str().to_owned()));
    out.extend(tail.iter().map(OsString::from));
    out
}

/// What a runner invocation wrote and returned.
struct Output {
    exit: Exit,
    stderr: String,
}

/// Runs a one-shot subcommand with no standard input.
fn run(args: &[OsString], setup: Option<&DocumentSetup>) -> Output {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = headless::run_with_setup(
        args.iter().cloned(),
        &product(),
        None,
        setup,
        Cursor::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    );
    Output {
        exit,
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

/// The stable code of the error the runner reported on standard error (its last line).
fn error_code_of(stderr: &str) -> String {
    let value: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    value["error"]["message"].as_str().unwrap().to_owned()
}

fn initialize(permissions: &[&str]) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "arc.initialize",
           "params": {"clientName": "test", "protocolVersions": {"min": "1.0", "max": "1.0"},
                      "permissions": permissions}})
}

fn run_command(id: u64, document: &str, command: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "command.run",
           "params": {"document": document, "command": command, "params": params}})
}

/// Serves the protocol on the runner's streams and sends `requests` one at a time, each after
/// the previous answer, so no request is still queued when the input ends. Returns the answers
/// and what the runner exited with.
struct Served {
    answers: Vec<Value>,
    output: Output,
}

fn serve(args: Vec<OsString>, setup: Option<&DocumentSetup>, requests: &[Value]) -> Served {
    let (mut to_server, server_in) = pipe();
    let (server_out, from_server) = pipe();
    let mut from_server: PipeReader = from_server.with_timeout(Duration::from_secs(30));
    let mut stderr = Vec::new();
    let (exit, stderr) = std::thread::scope(|scope| {
        let served = scope.spawn(move || {
            let exit = headless::run_with_setup(
                args,
                &product(),
                None,
                setup,
                server_in,
                server_out,
                &mut stderr,
            );
            (exit, stderr)
        });
        let mut answers = Vec::new();
        for request in requests {
            writeln!(to_server, "{request}").unwrap();
            let mut line = String::new();
            from_server.read_line(&mut line).unwrap();
            answers.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        drop(to_server);
        let (exit, stderr) = served.join().unwrap();
        (exit, (stderr, answers))
    });
    let (stderr_bytes, answers) = stderr;
    Served {
        answers,
        output: Output {
            exit,
            stderr: String::from_utf8(stderr_bytes).unwrap(),
        },
    }
}

#[test]
fn create_starts_an_empty_package_inside_a_write_root() {
    let dir = TempDir::new("create");
    let file = dir.path("scratch.arc");
    let root = dir.0.to_str().unwrap().to_owned();

    let created = run(
        &words(
            &["run"],
            &[&file],
            &[
                "--command",
                "shell.layer.create",
                "--params",
                r#"{"name":"walls"}"#,
                "--save",
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
            ],
        ),
        None,
    );
    assert_eq!(created.exit, Exit::Success, "{}", created.stderr);
    assert!(file.exists());

    // The created package opens like any other. A second run needs its own seed: the identifiers
    // of a run repeat for the same seed, as they do for any package.
    let again = run(
        &words(
            &["run"],
            &[&file],
            &[
                "--command",
                "shell.layer.create",
                "--params",
                r#"{"name":"roof"}"#,
                "--seed",
                "1",
                "--allow-edit",
                "--save",
            ],
        ),
        None,
    );
    assert_eq!(again.exit, Exit::Success, "{}", again.stderr);
}

#[test]
fn create_refuses_an_existing_file_a_missing_parent_and_paths_outside_the_write_roots() {
    let dir = TempDir::new("refuse");
    let taken = dir.path("taken.arc");
    drop(Package::create(&taken, &Metadata::default(), &owner()).unwrap());
    let root = dir.0.to_str().unwrap().to_owned();
    let command = [
        "--command",
        "shell.layer.create",
        "--params",
        r#"{"name":"a"}"#,
    ];

    let exists = run(
        &words(
            &["run"],
            &[&taken],
            &[
                &command[..],
                &["--create", "--allow-edit", "--write-root", &root],
            ]
            .concat(),
        ),
        None,
    );
    assert_eq!(exists.exit, Exit::Failed);
    assert_eq!(error_code_of(&exists.stderr), "cmd.path.io");

    let orphan = dir.0.join("missing-dir").join("new.arc");
    let no_parent = run(
        &words(
            &["run"],
            &[&orphan],
            &[
                &command[..],
                &["--create", "--allow-edit", "--write-root", &root],
            ]
            .concat(),
        ),
        None,
    );
    assert_eq!(no_parent.exit, Exit::Failed);
    assert_eq!(error_code_of(&no_parent.stderr), "cmd.path.not_found");

    // No write root: the path lies outside the roots, and nothing is created.
    let unrooted = dir.path("unrooted.arc");
    let no_roots = run(
        &words(
            &["run"],
            &[&unrooted],
            &[&command[..], &["--create", "--allow-edit"]].concat(),
        ),
        None,
    );
    assert_eq!(no_roots.exit, Exit::Denied);
    assert_eq!(error_code_of(&no_roots.stderr), "cmd.path.outside_roots");
    assert!(!unrooted.exists());
}

const LAYER_LIMIT: ErrorCode = ErrorCode {
    code: "test.layer.limit",
    category: Category::Conflict,
    message_id: "test-layer-limit",
};

static SETUP_CALLS: AtomicUsize = AtomicUsize::new(0);

/// The setup a product would install: a document validator that allows two layers.
fn limit_layers(document: &Document) -> arc_base::Result<()> {
    SETUP_CALLS.fetch_add(1, Ordering::Relaxed);
    document.add_validator(Arc::new(|_, after, _| {
        let layers = after
            .iter_type(arc_shell_cli::LAYER)
            .map_or(0, Iterator::count);
        if layers <= 2 {
            return Vec::new();
        }
        vec![Finding {
            severity: Severity::Error,
            error: Error::new(LAYER_LIMIT),
        }]
    }));
    Ok(())
}

#[test]
fn the_setup_hook_validator_rejects_an_invalid_command_through_the_protocol() {
    let dir = TempDir::new("hook");
    let file = dir.path("sheet.arc");
    let root = dir.0.to_str().unwrap().to_owned();
    let before = SETUP_CALLS.load(Ordering::Relaxed);
    let requests: Vec<Value> = std::iter::once(initialize(&["read", "edit"]))
        .chain(["a", "b", "c"].iter().enumerate().map(|(i, name)| {
            run_command(
                2 + i as u64,
                "sheet",
                "shell.layer.create",
                json!({"name": name}),
            )
        }))
        .collect();
    let served = serve(
        words(
            &["serve", "--stdio"],
            &[&file],
            &["--create", "--allow-edit", "--write-root", &root],
        ),
        Some(&limit_layers),
        &requests,
    );
    assert_eq!(
        served.output.exit,
        Exit::Success,
        "{}",
        served.output.stderr
    );
    assert_eq!(SETUP_CALLS.load(Ordering::Relaxed) - before, 1);
    assert!(served.answers[1]["result"]["revision"].is_u64());
    assert!(served.answers[2]["result"]["revision"].is_u64());
    assert_eq!(served.answers[3]["error"]["message"], "test.layer.limit");
}

#[test]
fn the_setup_hook_runs_for_every_document_a_subcommand_opens() {
    let dir = TempDir::new("hook-many");
    let a = dir.path("a.arc");
    let b = dir.path("b.arc");
    drop(Package::create(&a, &Metadata::default(), &owner()).unwrap());
    drop(Package::create(&b, &Metadata::default(), &owner()).unwrap());
    let before = SETUP_CALLS.load(Ordering::Relaxed);
    let served = serve(
        words(&["serve", "--stdio"], &[&a, &b], &[]),
        Some(&limit_layers),
        &[],
    );
    assert_eq!(
        served.output.exit,
        Exit::Success,
        "{}",
        served.output.stderr
    );
    assert_eq!(SETUP_CALLS.load(Ordering::Relaxed) - before, 2);
}

#[test]
fn a_session_that_stepped_the_history_is_not_recorded_as_a_macro() {
    let dir = TempDir::new("record");
    let root = dir.0.to_str().unwrap().to_owned();
    let record = dir.path("session.jsonl");
    let record_flag = format!("--record={}", record.display());
    let file = dir.path("doc.arc");
    let undo = json!({"jsonrpc": "2.0", "id": 3, "method": "history.undo",
                      "params": {"document": "doc"}});
    let requests = [
        initialize(&["read", "edit"]),
        run_command(2, "doc", "shell.layer.create", json!({"name": "a"})),
        undo,
    ];
    let served = serve(
        words(
            &["serve", "--stdio"],
            &[&file],
            &[
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
                &record_flag,
            ],
        ),
        None,
        &requests,
    );
    assert_eq!(served.answers[2]["result"]["command"], "shell.layer.create");
    assert_eq!(
        served.output.exit,
        Exit::Unsupported,
        "{}",
        served.output.stderr
    );
    assert_eq!(
        error_code_of(&served.output.stderr),
        "cmd.macro.unrecordable"
    );
    assert!(
        !record.exists(),
        "no macro is written for an unrecordable session"
    );

    // The same session without the undo records its run as before.
    let plain = dir.path("plain.arc");
    let recorded = dir.path("plain.jsonl");
    let recorded_flag = format!("--record={}", recorded.display());
    let served = serve(
        words(
            &["serve", "--stdio"],
            &[&plain],
            &[
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
                &recorded_flag,
            ],
        ),
        None,
        &[
            initialize(&["read", "edit"]),
            run_command(2, "plain", "shell.layer.create", json!({"name": "a"})),
        ],
    );
    assert_eq!(
        served.output.exit,
        Exit::Success,
        "{}",
        served.output.stderr
    );
    assert!(
        std::fs::read_to_string(&recorded)
            .unwrap()
            .contains("shell.layer.create")
    );
}

/// A setup that refuses the document, as a product's hook may.
fn refuse_document(_: &Document) -> arc_base::Result<()> {
    Err(Error::new(LAYER_LIMIT))
}

#[test]
fn a_failed_creation_run_leaves_no_file_behind() {
    let dir = TempDir::new("cleanup");
    let root = dir.0.to_str().unwrap().to_owned();

    // The setup hook refuses the document after the package was written.
    let hooked = dir.path("hooked.arc");
    let output = run(
        &words(
            &["run"],
            &[&hooked],
            &[
                "--command",
                "shell.layer.create",
                "--params",
                r#"{"name":"a"}"#,
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
            ],
        ),
        Some(&refuse_document),
    );
    assert_eq!(output.exit, Exit::Failed, "{}", output.stderr);
    assert!(!hooked.exists());

    // A command that fails without --save removes the package it created.
    let invalid = dir.path("invalid.arc");
    let output = run(
        &words(
            &["run"],
            &[&invalid],
            &[
                "--command",
                "shell.layer.create",
                "--params",
                "{}",
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
            ],
        ),
        None,
    );
    assert_eq!(output.exit, Exit::Usage, "{}", output.stderr);
    assert!(!invalid.exists());
}

#[test]
fn a_session_that_records_nothing_leaves_an_existing_macro_file_alone() {
    let dir = TempDir::new("record-keep");
    let root = dir.0.to_str().unwrap().to_owned();
    let record = dir.path("kept.jsonl");
    std::fs::write(&record, "keep me").unwrap();
    let record_flag = format!("--record={}", record.display());
    let file = dir.path("empty.arc");
    let served = serve(
        words(
            &["serve", "--stdio"],
            &[&file],
            &[
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
                &record_flag,
            ],
        ),
        None,
        &[initialize(&["read", "edit"])],
    );
    assert_eq!(
        served.output.exit,
        Exit::Success,
        "{}",
        served.output.stderr
    );
    assert_eq!(std::fs::read_to_string(&record).unwrap(), "keep me");

    // A session that stepped the history is refused, and the existing file is not truncated.
    let file = dir.path("stepped.arc");
    let served = serve(
        words(
            &["serve", "--stdio"],
            &[&file],
            &[
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
                &record_flag,
            ],
        ),
        None,
        &[
            initialize(&["read", "edit"]),
            run_command(2, "stepped", "shell.layer.create", json!({"name": "a"})),
            json!({"jsonrpc": "2.0", "id": 3, "method": "history.undo",
                   "params": {"document": "stepped"}}),
        ],
    );
    assert_eq!(
        served.output.exit,
        Exit::Unsupported,
        "{}",
        served.output.stderr
    );
    assert_eq!(std::fs::read_to_string(&record).unwrap(), "keep me");
}

#[test]
fn creation_refuses_a_missing_directory_outside_the_roots_without_saying_so() {
    let dir = TempDir::new("outside-missing");
    let root = dir.0.to_str().unwrap().to_owned();
    let elsewhere =
        std::env::temp_dir().join(format!("arc-cli-scripted-absent-{}", std::process::id()));
    let target = elsewhere.join("sub").join("new.arc");
    let output = run(
        &words(
            &["run"],
            &[&target],
            &[
                "--command",
                "shell.layer.create",
                "--params",
                r#"{"name":"a"}"#,
                "--create",
                "--allow-edit",
                "--write-root",
                &root,
            ],
        ),
        None,
    );
    assert_eq!(output.exit, Exit::Denied);
    assert_eq!(error_code_of(&output.stderr), "cmd.path.outside_roots");
}
