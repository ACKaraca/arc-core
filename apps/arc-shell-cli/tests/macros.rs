//! Macro recording and playback over the sample set (CORE-CMD-016, CORE-CMD-018): sessions
//! recorded through the protocol server replay on a copy of the starting package to the same
//! objects, identifiers included; a failing line stops playback with its line number and rolls
//! the group back; hostile macro text never panics.

#![allow(clippy::unwrap_used, reason = "tests")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_base::Limits;
use arc_command::macros::{parse_macro, play};
use arc_command::{
    FixedClock, NoLog, Permission, Recorder, Server, ServerConfig, Session, Surface,
};
use arc_doc::package::{LockOwner, Metadata, Mode, Package};
use arc_doc::{Document, DocumentConfig, LoadOptions, Object};
use proptest::prelude::*;
use serde_json::{Value, json};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "arc-shell-cli-macros-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
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
        application: "macros".into(),
        time_unix_ms: 0,
    }
}

fn product() -> arc_command::headless::Product {
    arc_shell_cli::product(Arc::new(FixedClock(0)), owner()).unwrap()
}

/// A starting package and an identical copy.
fn packages(dir: &Path) -> (PathBuf, PathBuf) {
    let a = dir.join("a.arc");
    drop(Package::create(&a, &Metadata::default(), &owner()).unwrap());
    let b = dir.join("b.arc");
    std::fs::copy(&a, &b).unwrap();
    (a, b)
}

fn open(product: &arc_command::headless::Product, path: &Path) -> Document {
    let package = Package::open(path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    package
        .open_document(
            Arc::clone(&product.types),
            Arc::clone(&product.evaluators),
            &LoadOptions::default(),
            DocumentConfig::default(),
        )
        .unwrap()
}

fn objects(document: &Document) -> Vec<(String, Object)> {
    let snapshot = document.snapshot();
    let mut out: Vec<(String, Object)> = snapshot
        .iter()
        .map(|(handle, object)| {
            let key = snapshot.type_key(handle).unwrap().as_str().to_owned();
            (key, (**object).clone())
        })
        .collect();
    out.sort_by_key(|(_, o)| o.id);
    out
}

fn session() -> Session {
    Session::new(Surface::Automation, [Permission::Read, Permission::Edit])
}

/// One generated step: an operation, a coordinate, an object pick and a seed.
type Op = (u8, f64, usize, u64);

fn params(op: Op, ids: &[Value]) -> (&'static str, Value) {
    let (kind, x, pick, _) = op;
    let id = if ids.is_empty() {
        json!("00000000-0000-7000-8000-000000000000")
    } else {
        ids[pick % ids.len()].clone()
    };
    match kind % 6 {
        0 => ("shell.layer.create", json!({"name": format!("l{x}")})),
        1 if ids.is_empty() => ("shell.point.create", json!({"position": [x, 0.5, -x]})),
        1 => (
            "shell.point.create",
            json!({"position": [x, 0.5, -x], "layer": id}),
        ),
        2 => (
            "shell.line.create",
            json!({"start": [0.0, x, 1.0], "end": [x, 2.0, 0.25]}),
        ),
        3 => (
            "shell.object.move",
            json!({"id": id, "offset": [x, x, 0.0]}),
        ),
        4 => ("shell.object.delete", json!({"id": id})),
        _ => (
            "shell.object.set_field",
            json!({"objects": [id], "field": "position", "value": [x, x, x]}),
        ),
    }
}

fn coordinate() -> impl Strategy<Value = f64> {
    prop_oneof![
        -1e3..1e3f64,
        prop::num::f64::NORMAL,
        Just(0.0),
        Just(-0.0),
        Just(0.1),
        Just(1e-300),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Recorded through the protocol server, failures included, and played back on a copy of
    /// the starting package: the same objects with the same identifiers.
    #[test]
    fn recorded_sessions_replay_to_the_same_objects(
        ops in prop::collection::vec((any::<u8>(), coordinate(), any::<usize>(), any::<u64>()), 1..14),
    ) {
        let dir = TempDir::new("prop");
        let (a, b) = packages(&dir.0);
        let product = product();
        let mut documents = BTreeMap::new();
        documents.insert("a".to_owned(), open(&product, &a));
        let recorder = Recorder::new(&product.name, &product.version, &Limits::DEFAULT);
        let config = ServerConfig::new("p", "1", [Permission::Read, Permission::Edit]);
        let server = Server::new(&product.registry, &documents, config, &NoLog)
            .with_recorder(&recorder);
        let mut ids: Vec<Value> = Vec::new();
        let mut succeeded = 0;
        for op in ops {
            let (command, params) = params(op, &ids);
            let request = json!({"document": "a", "command": command, "params": params,
                                 "seed": op.3});
            if let Ok(result) = server.call("command.run", request) {
                succeeded += 1;
                if let Some(id) = result["value"].get("id") {
                    ids.push(id.clone());
                }
            }
        }
        prop_assert_eq!(recorder.len(), succeeded);
        let text = recorder.text();
        let parsed = parse_macro(&text, &Limits::DEFAULT).unwrap();
        let copy = open(&product, &b);
        play(&product.registry, &copy, &session(), &parsed.steps, "macro")
            .map_err(|e| TestCaseError::fail(format!("{e:?}\n{text}")))?;
        prop_assert_eq!(objects(&copy), objects(&documents["a"]));
    }

    /// Arbitrary text, and valid macros with mutated bytes, never panic the reader or playback.
    #[test]
    fn hostile_macro_text_never_panics(
        text in prop_oneof![
            ".{0,200}",
            prop::collection::vec(any::<u8>(), 0..300)
                .prop_map(|b| String::from_utf8_lossy(&b).into_owned()),
            (0usize..200, any::<char>()).prop_map(|(at, c)| {
                let mut t = String::from(concat!(
                    r#"{"format":"arc-macro","protocol":"1.0","product":{"name":"p","version":"1"},"version":1}"#,
                    "\n",
                    r#"{"line":"shell.point.create position=[1,2,3]","revision":0,"seed":3}"#,
                    "\n",
                    r#"{"line":"shell.layer.create name=a","seed":18446744073709551615}"#,
                ));
                let at = (0..=at.min(t.len())).rev().find(|i| t.is_char_boundary(*i)).unwrap();
                t.insert(at, c);
                t
            }),
        ],
    ) {
        let small = Limits { string_bytes: 128, record_count: 8, ..Limits::DEFAULT };
        for limits in [Limits::DEFAULT, small] {
            if let Ok(parsed) = parse_macro(&text, &limits) {
                let product = product();
                let document = Document::new(
                    Arc::clone(&product.types),
                    Arc::clone(&product.evaluators),
                ).unwrap();
                let _ = play(&product.registry, &document, &session(), &parsed.steps, "macro");
            }
        }
    }
}

#[test]
fn a_failing_line_stops_playback_with_its_number_and_rolls_the_group_back() {
    let product = product();
    let document =
        Document::new(Arc::clone(&product.types), Arc::clone(&product.evaluators)).unwrap();
    let text = "# a batch list\nshell.layer.create name=walls\n\nshell.point.create position=[1,2,3]\nshell.object.delete id=00000000-0000-7000-8000-000000000000\nshell.layer.create name=never\n";
    let steps = parse_macro(text, &Limits::DEFAULT).unwrap().steps;
    let before = objects(&document);
    let error = play(&product.registry, &document, &session(), &steps, "macro").unwrap_err();
    assert_eq!(
        error.details().get("line"),
        Some(&arc_base::error::Value::Int(5))
    );
    assert_eq!(error.category().as_str(), "not_found");
    assert_eq!(objects(&document), before);
    assert!(!document.can_redo());

    // A successful playback is one undo step.
    play(
        &product.registry,
        &document,
        &session(),
        &steps[..2],
        "macro",
    )
    .unwrap();
    assert_eq!(objects(&document).len(), 2);
    assert!(document.undo().wait().unwrap().value);
    assert_eq!(objects(&document), before);

    // Playback never bypasses the session's permissions.
    let read_only = Session::new(Surface::Automation, [Permission::Read]);
    let error = play(&product.registry, &document, &read_only, &steps, "m").unwrap_err();
    assert_eq!(error.code(), "cmd.permission_denied");
    assert_eq!(
        error.details().get("line"),
        Some(&arc_base::error::Value::Int(2))
    );
}
