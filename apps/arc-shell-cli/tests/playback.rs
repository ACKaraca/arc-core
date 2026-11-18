//! Through the built binary: recording with `serve --stdio --record` and `run --record`,
//! `play` and `batch` (CORE-CMD-016, CORE-CMD-018, CORE-CMD-031), and `render` to PNG on the
//! adapter the selection rules choose (a software adapter on CI).

#![allow(clippy::unwrap_used, reason = "tests")]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use arc_doc::package::{LockOwner, Metadata, Package};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_arc-shell-cli");

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "arc-shell-cli-playback-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The canonical form, so paths compare with the roots.
        Self(dir.canonicalize().unwrap())
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn create_package(path: &Path) {
    let owner = LockOwner {
        user: "test".into(),
        host: "test".into(),
        application: "playback tests".into(),
        time_unix_ms: 0,
    };
    drop(Package::create(path, &Metadata::default(), &owner).unwrap());
}

fn cli(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn inspected_objects(file: &str) -> Value {
    let out = cli(&["inspect", file]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["objects"].clone()
}

/// Runs a protocol session on `args` that creates a layer, a point on it and a line, moves the
/// line, fails once and deletes the point.
fn record_session(args: &[&str]) {
    let mut child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut from = BufReader::new(child.stdout.take().unwrap());
    let mut stdin = child.stdin.take().unwrap();
    let mut next = 0;
    let mut call = |method: &str, params: Value| -> Value {
        next += 1;
        let request = json!({"jsonrpc": "2.0", "id": next, "method": method, "params": params});
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        from.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    };
    call(
        "arc.initialize",
        json!({"clientName": "rec", "protocolVersions": {"min": "1.0", "max": "1.0"},
               "permissions": ["read", "edit"]}),
    );
    let mut run = |command: &str, params: Value, seed: u64| -> Value {
        call(
            "command.run",
            json!({"document": "pkg", "command": command, "params": params, "seed": seed}),
        )
    };
    let layer =
        run("shell.layer.create", json!({"name": "walls"}), 1)["result"]["value"]["id"].clone();
    let point = run(
        "shell.point.create",
        json!({"position": [1.5, 2.0, 0.0], "layer": layer}),
        2,
    )["result"]["value"]["id"]
        .clone();
    let line = run(
        "shell.line.create",
        json!({"start": [0.0, 0.0, 0.0], "end": [3.0, 0.0, 0.0]}),
        0,
    )["result"]["value"]["id"]
        .clone();
    run(
        "shell.object.move",
        json!({"id": line, "offset": [0.0, 1.0, 0.0]}),
        0,
    );
    let failed = run(
        "shell.object.delete",
        json!({"id": "00000000-0000-7000-8000-000000000000"}),
        0,
    );
    assert!(failed.get("error").is_some());
    run("shell.object.delete", json!({"id": point}), 0);
    call("arc.shutdown", json!({}));
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn recorded_sessions_replay_to_the_same_content() {
    let dir = TempDir::new("record");
    let (pkg, copy) = (dir.path("pkg.arc"), dir.path("copy.arc"));
    create_package(Path::new(&pkg));
    std::fs::copy(&pkg, &copy).unwrap();
    let record = dir.path("session.jsonl");
    let root = dir.0.to_str().unwrap();
    record_session(&[
        "serve",
        "--stdio",
        "--allow-edit",
        "--save",
        "--write-root",
        root,
        "--record",
        &record,
        &pkg,
    ]);
    let text = std::fs::read_to_string(&record).unwrap();
    // The header and five completed commands; the failed move is not recorded.
    assert_eq!(text.lines().count(), 6, "{text}");
    assert!(
        text.lines()
            .nth(4)
            .unwrap()
            .contains("\"line\":\"shell.object.move")
    );

    let out = cli(&[
        "play",
        &copy,
        "--macro",
        &record,
        "--read-root",
        root,
        "--allow-edit",
        "--save",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["results"].as_array().unwrap().len(), 5);
    let objects = inspected_objects(&pkg);
    assert_eq!(objects.as_array().unwrap().len(), 2);
    assert_eq!(inspected_objects(&copy), objects);

    // `run --record` records the one command; a run that is played again matches.
    let (one, other) = (dir.path("one.arc"), dir.path("other.arc"));
    create_package(Path::new(&one));
    std::fs::copy(&one, &other).unwrap();
    let single = dir.path("single.jsonl");
    let out = cli(&[
        "run",
        &one,
        "--command",
        "shell.point.create",
        "--params",
        r#"{"position": [0.1, 0.2, 0.3]}"#,
        "--seed",
        "99",
        "--allow-edit",
        "--save",
        "--write-root",
        root,
        "--record",
        &single,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let out = cli(&[
        "play",
        &other,
        "--macro",
        &single,
        "--read-root",
        root,
        "--allow-edit",
        "--save",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(inspected_objects(&other), inspected_objects(&one));

    // Macro and record files go through the roots.
    let out = cli(&["play", &other, "--macro", &single, "--allow-edit"]);
    assert_eq!(out.status.code(), Some(5), "{out:?}");
    let out = cli(&[
        "run",
        &one,
        "--command",
        "shell.layer.create",
        "--params",
        r#"{"name": "x"}"#,
        "--allow-edit",
        "--read-root",
        root,
        "--record",
        &single,
    ]);
    assert_eq!(out.status.code(), Some(5), "{out:?}");
    let out = cli(&["serve", "--stdio", "--record", &single, &one, &other]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn batch_reports_each_file_and_continues_after_a_failure() {
    let dir = TempDir::new("batch");
    let root = dir.0.to_str().unwrap();
    let files = [dir.path("a.arc"), dir.path("b.arc"), dir.path("c.arc")];
    create_package(Path::new(&files[0]));
    std::fs::write(&files[1], b"not a package").unwrap();
    create_package(Path::new(&files[2]));
    let list = dir.path("list.txt");
    std::fs::write(
        &list,
        "# two objects\nshell.layer.create walls\nshell.line.create start=0,0,0 end=1,0,0\n",
    )
    .unwrap();
    let mut args = vec!["batch"];
    args.extend(files.iter().map(String::as_str));
    args.extend([
        "--macro",
        &list,
        "--read-root",
        root,
        "--allow-edit",
        "--save",
    ]);
    let out = cli(&args);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let lines: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    for (line, file) in lines.iter().zip(&files) {
        assert_eq!(line["file"], json!(file));
    }
    assert_eq!(lines[0]["result"]["results"][1]["line"], 3);
    assert!(lines[1]["error"]["data"]["code"].is_string());
    assert!(lines[2].get("result").is_some());
    assert_eq!(inspected_objects(&files[0]).as_array().unwrap().len(), 2);
    assert_eq!(inspected_objects(&files[2]).as_array().unwrap().len(), 2);

    // A line that fails reports its number and nothing is saved.
    std::fs::write(
        &list,
        "shell.layer.create a\nshell.object.delete id=nonsense\n",
    )
    .unwrap();
    let out = cli(&[
        "batch",
        &files[0],
        "--macro",
        &list,
        "--read-root",
        root,
        "--allow-edit",
        "--save",
    ]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let line: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["error"]["data"]["details"]["line"], 2, "{line}");
    assert_eq!(inspected_objects(&files[0]).as_array().unwrap().len(), 2);
    // A macro that cannot be read fails before any file.
    std::fs::write(&list, "{not json").unwrap();
    let out = cli(&["batch", &files[0], "--macro", &list, "--read-root", root]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(out.stdout.is_empty());
}

#[test]
fn render_draws_the_sample_objects_to_a_png() {
    let dir = TempDir::new("render");
    let root = dir.0.to_str().unwrap();
    let pkg = dir.path("pkg.arc");
    create_package(Path::new(&pkg));
    let out = cli(&[
        "run",
        &pkg,
        "--command",
        "shell.line.create",
        "--params",
        r#"{"start": [-2.0, 0.0, 0.0], "end": [2.0, 0.0, 0.0]}"#,
        "--allow-edit",
        "--save",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let png = dir.path("view.png");
    let out = cli(&[
        "render",
        &pkg,
        "--output",
        &png,
        "--width",
        "80",
        "--height",
        "60",
        "--write-root",
        root,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["objects"], 1);
    assert_eq!(report["view"], "default");
    let image = image::open(&png).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (80, 60));
    // Zoomed to extents, the horizontal line crosses the middle row; the rest is background.
    let dark = |x, y| image.get_pixel(x, y).0[0] < 128;
    for x in [20, 40, 60] {
        let column: Vec<u8> = (0..60).map(|y| image.get_pixel(x, y).0[0]).collect();
        assert!(
            dark(x, 30) || dark(x, 29),
            "line missing at x {x}: {column:?}"
        );
    }
    for (x, y) in [(40, 5), (40, 55), (0, 0), (79, 59), (5, 30), (75, 30)] {
        assert_eq!(image.get_pixel(x, y).0, [255, 255, 255, 255], "at {x},{y}");
    }

    // Only the default view exists; the output goes through the write roots.
    let out = cli(&[
        "render",
        &pkg,
        "--output",
        &png,
        "--view",
        "top",
        "--write-root",
        root,
    ]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let out = cli(&["render", &pkg, "--output", &png]);
    assert_eq!(out.status.code(), Some(5), "{out:?}");
}
