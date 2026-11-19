//! `view.render` and the MCP `render_view` tool through the built binary, with the reference
//! renderer of the sample set (CORE-CMD-026): the PNG header and size match the request, the
//! file variant writes inside the write root, and the image is the same over both surfaces.

#![allow(clippy::unwrap_used, reason = "tests")]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use arc_doc::package::{LockOwner, Metadata, Package};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_arc-shell-cli");

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("arc-shell-cli-view-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A package holding one line from (-2, 0, 0) to (2, 0, 0), saved.
fn package_with_a_line(dir: &Path) -> PathBuf {
    let owner = LockOwner {
        user: "test".into(),
        host: "test".into(),
        application: "view render tests".into(),
        time_unix_ms: 0,
    };
    let file = dir.join("pkg.arc");
    drop(Package::create(&file, &Metadata::default(), &owner).unwrap());
    let out = Command::new(BIN)
        .args([
            "run",
            file.to_str().unwrap(),
            "--command",
            "shell.line.create",
            "--params",
            r#"{"start": [-2.0, 0.0, 0.0], "end": [2.0, 0.0, 0.0]}"#,
            "--allow-edit",
            "--save",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    file
}

/// Runs a session of `subcommand` over `stdio` with the lines as input.
fn session(subcommand: &[&str], lines: &[Value]) -> Output {
    let mut child = Command::new(BIN)
        .args(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for line in lines {
            writeln!(stdin, "{line}").unwrap();
        }
    }
    child.wait_with_output().unwrap()
}

/// The answers of a session, one per line, by identifier.
fn answers(out: &Output) -> Vec<Value> {
    String::from_utf8(out.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn size_of_png(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    assert_eq!(&bytes[12..16], b"IHDR");
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (word(16), word(20))
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "each shifted value is one byte"
)]
fn unbase64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    for quad in text.as_bytes().chunks(4) {
        let n = quad.iter().enumerate().fold(0_u32, |acc, (i, c)| {
            let digit = ALPHABET.iter().position(|a| a == c).unwrap_or(0) as u32;
            acc | (digit << (18 - 6 * i))
        });
        let kept = quad
            .iter()
            .filter(|c| **c != b'=')
            .count()
            .saturating_sub(1);
        for i in 0..kept {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    out
}

fn initialize(permissions: &[&str]) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "arc.initialize", "params": {
        "clientName": "test",
        "protocolVersions": {"min": "1.0", "max": "1.0"},
        "permissions": permissions,
    }})
}

#[test]
#[cfg(unix)]
fn view_render_answers_a_png_of_the_requested_size_inline_and_in_a_write_root() {
    let dir = TempDir::new("protocol");
    let file = package_with_a_line(&dir.0);
    let root = dir.0.to_str().unwrap();
    let target = dir.0.join("view.png");
    let out = session(
        &[
            "serve",
            "--stdio",
            "--write-root",
            root,
            file.to_str().unwrap(),
        ],
        &[
            initialize(&["read", "file_write"]),
            json!({"jsonrpc": "2.0", "id": 2, "method": "view.render",
                   "params": {"document": "pkg", "width": 120, "height": 90}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "view.render",
                   "params": {"document": "pkg", "width": 64, "height": 48,
                              "path": target.to_str().unwrap()}}),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let answers = answers(&out);
    let inline = unbase64(answers[1]["result"]["data"].as_str().unwrap());
    assert_eq!(size_of_png(&inline), (120, 90));
    assert_eq!(answers[1]["result"]["width"], 120);

    let written = std::fs::read(&target).unwrap();
    assert_eq!(size_of_png(&written), (64, 48));
    let decoded = image::open(&target).unwrap().to_rgba8();
    assert_eq!(decoded.dimensions(), (64, 48));
    // Zoomed to the extents, the line crosses the middle row of the image.
    assert!(decoded.get_pixel(32, 24).0[0] < 128 || decoded.get_pixel(32, 23).0[0] < 128);
}

#[test]
fn the_mcp_tool_returns_the_image_the_method_renders() {
    let dir = TempDir::new("mcp");
    let file = package_with_a_line(&dir.0);
    let file = file.to_str().unwrap();
    let method = answers(&session(
        &["serve", "--stdio", file],
        &[
            initialize(&["read"]),
            json!({"jsonrpc": "2.0", "id": 2, "method": "view.render",
                   "params": {"document": "pkg", "width": 40, "height": 30}}),
        ],
    ));
    let inline = unbase64(method[1]["result"]["data"].as_str().unwrap());

    let out = session(
        &["mcp", file],
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                              "clientInfo": {"name": "t", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "render_view", "arguments": {"width": 40, "height": 30}}}),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let tool = answers(&out);
    let content = &tool[1]["result"]["content"];
    assert_eq!(tool[1]["result"]["isError"], false);
    assert_eq!(content[0]["type"], "image");
    let image = unbase64(content[0]["data"].as_str().unwrap());
    assert_eq!(size_of_png(&image), (40, 30));
    assert_eq!(
        image, inline,
        "the tool and the method render the same image"
    );
}
