//! View rendering over the protocol (CORE-CMD-026): `view.render` and the MCP `render_view` tool
//! with a test renderer that records its calls. The sessions run over in-memory streams.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Mutex;

use arc_base::Error;
use arc_command::protocol::{McpConfig, ViewRenderer, ViewRequest};
use arc_command::{NoLog, Permission, Roots, Server, ServerConfig};
use arc_doc::Document;
use common::{document, registry};
use serde_json::{Value, json};

/// The first bytes of a PNG with the given size: the signature and the IHDR chunk, then `pad`
/// zero bytes.
fn png(width: u32, height: u32, pad: usize) -> Vec<u8> {
    let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13];
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    bytes.resize(bytes.len() + pad, 0);
    bytes
}

/// The size of a PNG from its IHDR chunk; only the header is read.
fn png_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10], "signature");
    assert_eq!(&bytes[12..16], b"IHDR", "IHDR");
    let word = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (word(16), word(20))
}

/// The bytes of a base64 text with padding.
#[allow(
    clippy::cast_possible_truncation,
    reason = "each shifted value is one byte"
)]
fn unbase64(text: &str) -> Vec<u8> {
    let digit = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => 0,
    };
    let mut out = Vec::new();
    for quad in text.as_bytes().chunks(4) {
        let n = quad.iter().enumerate().fold(0_u32, |acc, (i, c)| {
            acc | (u32::from(digit(*c)) << (18 - 6 * i))
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

/// A renderer that records each request and answers a PNG of the requested size, padded by
/// `pad` bytes.
struct Recording {
    calls: Mutex<Vec<(u32, u32, String, String)>>,
    pad: usize,
}

impl Recording {
    fn new(pad: usize) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            pad,
        }
    }

    fn calls(&self) -> Vec<(u32, u32, String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl ViewRenderer for Recording {
    fn render(&self, request: &ViewRequest<'_>) -> Result<Vec<u8>, Error> {
        self.calls.lock().unwrap().push((
            request.width,
            request.height,
            request.view.to_owned(),
            request.display_mode.to_owned(),
        ));
        Ok(png(request.width, request.height, self.pad))
    }
}

fn docs() -> BTreeMap<String, Document> {
    BTreeMap::from([("main".to_owned(), document())])
}

/// Runs the lines as one session with `ceiling` and the optional write root `write`, returning
/// the answers in order.
fn session(
    renderer: Option<&dyn ViewRenderer>,
    ceiling: &[Permission],
    write: Option<&std::path::Path>,
    lines: &[Value],
) -> Vec<Value> {
    let registry = registry();
    let documents = docs();
    let mut config = ServerConfig::new("arc-test", "0.1.0", ceiling.iter().copied());
    if let Some(dir) = write {
        config.roots = Roots::new(Vec::<&std::path::Path>::new(), vec![dir]).unwrap();
    }
    let mut server = Server::new(&registry, &documents, config, &NoLog);
    if let Some(renderer) = renderer {
        server = server.with_view_renderer(renderer);
    }
    let mut input = String::new();
    for line in lines {
        input.push_str(&line.to_string());
        input.push('\n');
    }
    let mut output = Vec::new();
    server
        .serve(Cursor::new(input.into_bytes()), &mut output)
        .unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn initialize(permissions: &[&str]) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "arc.initialize", "params": {
        "clientName": "test",
        "protocolVersions": {"min": "1.0", "max": "1.0"},
        "permissions": permissions,
    }})
}

fn render(id: u64, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "view.render", "params": params})
}

fn error_code(answer: &Value) -> String {
    answer["error"]["data"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn inline_image_matches_the_request_and_the_renderer_gets_the_defaults() {
    let renderer = Recording::new(16);
    let answers = session(
        Some(&renderer),
        &[Permission::Read],
        None,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main", "width": 320, "height": 200})),
        ],
    );
    let result = &answers[1]["result"];
    assert_eq!(result["mimeType"], "image/png");
    assert_eq!(
        (result["width"].as_u64(), result["height"].as_u64()),
        (Some(320), Some(200))
    );
    let bytes = unbase64(result["data"].as_str().unwrap());
    assert_eq!(png_size(&bytes), (320, 200));
    assert_eq!(bytes.len() as u64, result["bytes"].as_u64().unwrap());
    assert_eq!(
        renderer.calls(),
        vec![(320, 200, "default".into(), "shaded".into())]
    );

    // Without a size the request takes 800 by 600.
    let answers = session(
        Some(&renderer),
        &[Permission::Read],
        None,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main"})),
        ],
    );
    assert_eq!(
        png_size(&unbase64(answers[1]["result"]["data"].as_str().unwrap())),
        (800, 600)
    );
}

#[test]
#[cfg(unix)]
fn file_variant_writes_inside_a_write_root_and_refuses_a_path_outside() {
    let dir = std::env::temp_dir().join(format!("arc-view-render-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let renderer = Recording::new(0);
    let inside = dir.join("view.png");
    let answers = session(
        Some(&renderer),
        &[Permission::Read, Permission::FileWrite],
        Some(&dir),
        &[
            initialize(&["read", "file_write"]),
            render(
                2,
                json!({"document": "main", "width": 64, "height": 48, "path": inside}),
            ),
            render(
                3,
                json!({"document": "main", "width": 8, "height": 8,
                       "path": dir.join("..").join("arc-outside.png")}),
            ),
        ],
    );
    let written = std::fs::read(&inside).unwrap();
    assert_eq!(png_size(&written), (64, 48));
    assert_eq!(
        answers[1]["result"]["bytes"].as_u64(),
        Some(written.len() as u64)
    );
    assert!(
        answers[1]["result"].get("data").is_none(),
        "a path answers no image data"
    );
    assert_eq!(error_code(&answers[2]), "cmd.path.outside_roots");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_variant_needs_the_file_write_permission() {
    let renderer = Recording::new(0);
    let answers = session(
        Some(&renderer),
        &[Permission::Read],
        None,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main", "path": "view.png"})),
        ],
    );
    assert_eq!(error_code(&answers[1]), "cmd.permission_denied");
    assert!(renderer.calls().is_empty());
}

#[test]
fn oversize_requests_are_refused_before_the_renderer_runs() {
    let renderer = Recording::new(0);
    let answers = session(
        Some(&renderer),
        &[Permission::Read],
        None,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main", "width": 5000, "height": 10})),
            render(
                3,
                json!({"document": "main", "width": 4096, "height": 4096}),
            ),
            render(4, json!({"document": "main", "width": 0, "height": 10})),
            render(5, json!({"document": "main", "camera": [1, 2, 3]})),
            render(6, json!({"document": "main", "sections": [1]})),
            render(7, json!({"document": "main", "view": ""})),
        ],
    );
    assert_eq!(error_code(&answers[1]), "cmd.render.size_exceeded");
    assert_eq!(answers[1]["error"]["data"]["details"]["limit"], "side");
    assert_eq!(error_code(&answers[2]), "cmd.render.size_exceeded");
    assert_eq!(answers[2]["error"]["data"]["details"]["limit"], "pixels");
    assert_eq!(error_code(&answers[3]), "cmd.protocol.invalid_params");
    assert_eq!(error_code(&answers[4]), "cmd.protocol.invalid_params");
    assert_eq!(error_code(&answers[5]), "cmd.protocol.invalid_params");
    assert_eq!(error_code(&answers[6]), "cmd.protocol.invalid_params");
    assert!(renderer.calls().is_empty(), "no request above was rendered");
}

#[test]
fn an_image_too_large_to_return_inline_is_refused() {
    struct Big;
    impl ViewRenderer for Big {
        fn render(&self, request: &ViewRequest<'_>) -> Result<Vec<u8>, Error> {
            Ok(png(request.width, request.height, 13 << 20))
        }
    }
    let answers = session(
        Some(&Big),
        &[Permission::Read],
        None,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main", "width": 8, "height": 8})),
        ],
    );
    assert_eq!(error_code(&answers[1]), "cmd.render.too_large");
    assert_eq!(
        answers[1]["error"]["data"]["details"]["limit"],
        "inline_bytes"
    );
}

#[test]
fn without_a_renderer_the_answer_is_unavailable() {
    let answers = session(
        None,
        &[Permission::Read],
        None,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main"})),
        ],
    );
    assert_eq!(error_code(&answers[1]), "cmd.render.unavailable");
}

#[test]
fn a_session_without_read_is_refused_first() {
    let renderer = Recording::new(0);
    let answers = session(
        Some(&renderer),
        &[Permission::Edit],
        None,
        &[
            initialize(&["edit"]),
            render(2, json!({"document": "main"})),
        ],
    );
    assert_eq!(error_code(&answers[1]), "cmd.permission_denied");
    assert!(renderer.calls().is_empty());
}

#[test]
fn the_mcp_tool_returns_the_same_image_as_the_method() {
    let renderer = Recording::new(4);
    let registry = registry();
    let documents = docs();
    let config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read]);
    let server = Server::new(&registry, &documents, config, &NoLog).with_view_renderer(&renderer);
    let lines = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "t", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "render_view", "arguments": {"width": 30, "height": 20}}}),
    ];
    let mut input = String::new();
    for line in &lines {
        input.push_str(&line.to_string());
        input.push('\n');
    }
    let mut output = Vec::new();
    server
        .serve_mcp(
            &McpConfig::new("main"),
            Cursor::new(input.into_bytes()),
            &mut output,
        )
        .unwrap();
    let answer: Value = String::from_utf8(output)
        .unwrap()
        .lines()
        .nth(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .unwrap();
    let content = &answer["result"]["content"];
    assert_eq!(answer["result"]["isError"], false);
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/png");
    let image = unbase64(content[0]["data"].as_str().unwrap());
    assert_eq!(png_size(&image), (30, 20));

    let method = session(
        Some(&renderer),
        &[Permission::Read],
        None,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main", "width": 30, "height": 20})),
        ],
    );
    let inline = unbase64(method[1]["result"]["data"].as_str().unwrap());
    assert_eq!(
        image, inline,
        "the tool and the method render the same image"
    );
}

#[test]
fn the_mcp_tool_without_a_renderer_answers_unavailable() {
    let registry = registry();
    let documents = docs();
    let config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read]);
    let server = Server::new(&registry, &documents, config, &NoLog);
    let input = format!(
        "{}\n{}\n",
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "t", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "render_view", "arguments": {}}}),
    );
    let mut output = Vec::new();
    server
        .serve_mcp(
            &McpConfig::new("main"),
            Cursor::new(input.into_bytes()),
            &mut output,
        )
        .unwrap();
    let text = String::from_utf8(output).unwrap();
    let answer: Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
    assert_eq!(answer["result"]["isError"], true);
    assert!(
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("cmd.render.unavailable")
    );
}

/// [`session`] with the server's message limit set to `message_bytes`, over the renderer given.
fn session_with_message_limit(
    renderer: &dyn ViewRenderer,
    message_bytes: u64,
    lines: &[Value],
) -> Vec<Value> {
    let registry = registry();
    let documents = docs();
    let mut config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read]);
    config.max_message_bytes = message_bytes;
    let server = Server::new(&registry, &documents, config, &NoLog).with_view_renderer(renderer);
    let mut input = String::new();
    for line in lines {
        input.push_str(&line.to_string());
        input.push('\n');
    }
    let mut output = Vec::new();
    server
        .serve(Cursor::new(input.into_bytes()), &mut output)
        .unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn an_inline_image_whose_estimate_does_not_fit_is_refused_before_rendering() {
    let renderer = Recording::new(0);
    // A 64 by 64 image is estimated at 512 bytes, which is 684 bytes of base64 plus the envelope:
    // more than the 1,200-byte message limit, so the renderer is not called.
    let answers = session_with_message_limit(
        &renderer,
        1200,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main", "width": 64, "height": 64})),
        ],
    );
    assert_eq!(error_code(&answers[1]), "cmd.render.too_large");
    assert_eq!(
        answers[1]["error"]["data"]["details"]["limit"],
        "message_bytes"
    );
    assert!(renderer.calls().is_empty(), "the renderer is not called");
}

#[test]
fn an_inline_image_too_large_for_the_message_is_refused_after_rendering() {
    // The estimate of an 8 by 8 image fits; the image the renderer returns does not.
    let renderer = Recording::new(1000);
    let answers = session_with_message_limit(
        &renderer,
        2000,
        &[
            initialize(&["read"]),
            render(2, json!({"document": "main", "width": 8, "height": 8})),
        ],
    );
    assert_eq!(error_code(&answers[1]), "cmd.render.too_large");
    assert_eq!(
        answers[1]["error"]["data"]["details"]["limit"],
        "message_bytes"
    );
    assert_eq!(renderer.calls().len(), 1, "the renderer ran once");
}
