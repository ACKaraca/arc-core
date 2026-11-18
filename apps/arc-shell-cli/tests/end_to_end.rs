//! End to end through the built binary (CORE-APP-026, CORE-CMD-031): a package is created,
//! edited by sample commands over the stdio protocol and with `run`, saved, reopened and
//! inspected; the inspected objects equal the expected ones.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use arc_doc::package::{LockOwner, Metadata, Package};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_arc-shell-cli");

/// A directory removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("arc-shell-cli-{name}-{}", std::process::id()));
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

fn create_package(path: &Path) {
    let owner = LockOwner {
        user: "test".into(),
        host: "test".into(),
        application: "arc-shell-cli tests".into(),
        time_unix_ms: 0,
    };
    drop(Package::create(path, &Metadata::default(), &owner).unwrap());
}

fn cli(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A protocol session with the binary.
struct Session {
    child: std::process::Child,
    from: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Session {
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(BIN)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let from = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child,
            from,
            next: 0,
        };
        let r = session.call(
            "arc.initialize",
            json!({"clientName": "e2e", "protocolVersions": {"min": "1.0", "max": "1.0"},
                   "permissions": ["read", "edit"]}),
        );
        assert_eq!(r["result"]["permissions"], json!(["read", "edit"]));
        session
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let request =
            json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(self.from.read_line(&mut line).unwrap() > 0, "session ended");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == json!(self.next) {
                return message;
            }
        }
    }

    /// Runs `command` on document `pkg` and returns the created identifier, if any.
    fn run(&mut self, command: &str, params: Value) -> Value {
        let r = self.call(
            "command.run",
            json!({"document": "pkg", "command": command, "params": params}),
        );
        assert!(r.get("result").is_some(), "{r}");
        r["result"]["value"]["id"].clone()
    }

    fn finish(mut self) {
        let r = self.call("arc.shutdown", json!({}));
        assert_eq!(r["result"], Value::Null);
        drop(self.child.stdin.take());
        let status = self.child.wait().unwrap();
        let mut stderr = String::new();
        std::io::Read::read_to_string(self.child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
        assert!(status.success(), "{status}: {stderr}");
    }
}

/// The inspected objects as `(type_key, values)`, with identifiers replaced by names.
fn objects(dump: &Value, names: &[(&Value, &str)]) -> Vec<(String, Value)> {
    let rename = |value: &Value| -> Value {
        names
            .iter()
            .find(|(id, _)| *id == value)
            .map_or_else(|| value.clone(), |(_, name)| json!(name))
    };
    let mut out: Vec<(String, Value)> = dump["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| {
            let values: serde_json::Map<String, Value> = o["values"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), rename(v)))
                .collect();
            (
                o["type_key"].as_str().unwrap().to_owned(),
                Value::Object(values),
            )
        })
        .collect();
    out.sort_by_key(|(key, values)| format!("{key}{values}"));
    out
}

#[test]
fn edit_save_reopen_and_inspect_through_the_binary() {
    let dir = TempDir::new("e2e");
    let path = dir.0.join("pkg.arc");
    create_package(&path);
    let file = path.to_str().unwrap();

    let mut s = Session::start(&["serve", "--stdio", "--allow-edit", "--save", file]);
    let walls = s.run("shell.layer.create", json!({"name": "walls"}));
    let doors = s.run("shell.layer.create", json!({"name": "doors"}));
    let point = s.run(
        "shell.point.create",
        json!({"position": [1.0, 2.0, 3.0], "layer": walls}),
    );
    let line = s.run(
        "shell.line.create",
        json!({"start": [0.0, 0.0, 0.0], "end": [1.0, 0.0, 0.0], "layer": doors}),
    );
    s.run(
        "shell.object.move",
        json!({"id": line, "offset": [0.0, 0.0, 0.5]}),
    );
    s.run("shell.object.delete", json!({"id": doors}));
    let gone = s.run("shell.point.create", json!({"position": [9.0, 9.0, 9.0]}));
    s.run("shell.object.delete", json!({"id": gone}));
    // An edit the store refuses (a coordinate overflowing to infinity) changes nothing.
    let far = s.run("shell.point.create", json!({"position": [1e308, 0.0, 0.0]}));
    let r = s.call(
        "command.run",
        json!({"document": "pkg", "command": "shell.object.move",
               "params": {"id": far, "offset": [1e308, 0.0, 0.0]}}),
    );
    assert!(r.get("error").is_some(), "{r}");
    s.run("shell.object.delete", json!({"id": far}));
    s.finish();

    // `run` edits the saved package once more and saves it.
    let out = cli(&[
        "run",
        file,
        "--command",
        "shell.object.move",
        "--params",
        &json!({"id": point, "offset": [1.0, 1.0, 1.0]}).to_string(),
        "--allow-edit",
        "--save",
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(stdout_json(&out)["command"], "shell.object.move");

    let out = cli(&["inspect", file, "--check"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let dump = stdout_json(&out);
    assert_eq!(dump["findings"], json!([]));
    let names = [(&walls, "walls"), (&point, "point"), (&line, "line")];
    assert_eq!(
        objects(&dump, &names),
        vec![
            ("shell.layer".to_owned(), json!({"name": "walls"})),
            (
                "shell.line".to_owned(),
                json!({"start": [0.0, 0.0, 0.5], "end": [1.0, 0.0, 0.5], "layer": null})
            ),
            (
                "shell.point".to_owned(),
                json!({"position": [2.0, 3.0, 4.0], "layer": "walls"})
            ),
        ]
    );
    let out = cli(&["validate", file]);
    assert_eq!(
        (out.status.code(), stdout_json(&out)),
        (Some(0), json!({"findings": []}))
    );
}

#[test]
fn edits_need_the_flag_and_failures_have_their_exit_codes() {
    let dir = TempDir::new("exit");
    let path = dir.0.join("pkg.arc");
    create_package(&path);
    let file = path.to_str().unwrap();
    let run = |params: &str, extra: &[&str]| {
        let mut args = vec![
            "run",
            file,
            "--command",
            "shell.layer.create",
            "--params",
            params,
        ];
        args.extend(extra);
        cli(&args)
    };
    assert_eq!(run(r#"{"name": "a"}"#, &[]).status.code(), Some(5));
    assert_eq!(run(r#"{}"#, &["--allow-edit"]).status.code(), Some(2));
    assert_eq!(
        run(r#"{"name": 1}"#, &["--allow-edit"]).status.code(),
        Some(3)
    );
    assert_eq!(run("{", &["--allow-edit"]).status.code(), Some(3));
    let ok = run(r#"{"name": "a"}"#, &["--allow-edit"]);
    assert_eq!(ok.status.code(), Some(0));
    assert!(ok.stderr.is_empty());
    // Without --save nothing was written.
    let dump = stdout_json(&cli(&["inspect", file]));
    assert_eq!(dump["objects"], json!([]));
    let out = cli(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["data"]["code"], "cmd.cli.usage");
    let missing = dir.0.join("missing.arc");
    assert_eq!(
        cli(&["inspect", missing.to_str().unwrap()]).status.code(),
        Some(1)
    );
    std::fs::write(dir.0.join("junk.arc"), b"not a package").unwrap();
    let junk = dir.0.join("junk.arc");
    assert_ne!(
        cli(&["validate", junk.to_str().unwrap()]).status.code(),
        Some(0)
    );
    let out = cli(&["commands"]);
    let listed = stdout_json(&out);
    let ids: Vec<&str> = listed["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "shell.layer.create",
            "shell.line.create",
            "shell.object.delete",
            "shell.object.move",
            "shell.object.set_field",
            "shell.point.create"
        ]
    );
    let out = cli(&[
        "validate-params",
        "shell.point.create",
        "--params",
        r#"{"position": [1]}"#,
    ]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(stdout_json(&out)["valid"], false);
    let out = cli(&["describe", "shell.point.create"]);
    assert_eq!(stdout_json(&out)["permitted"], false);
}
