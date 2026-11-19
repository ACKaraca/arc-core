//! The Model Context Protocol endpoint (CORE-CMD-029): sessions over in-memory pipes with the
//! server on its own thread.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::time::Duration;

use arc_base::Limits;
use arc_command::protocol::{
    MCP_FIXED_TOOLS, MCP_VERSIONS, McpConfig, PipeReader, PipeWriter, pipe,
};
use arc_command::{NoLog, Permission, Server, ServerConfig, Surface};
use arc_doc::Document;
use common::{document, registry};
use serde_json::{Value, json};

struct Client {
    to: PipeWriter,
    from: PipeReader,
    next: u64,
}

impl Client {
    fn send_raw(&mut self, bytes: &[u8]) {
        self.to.write_all(bytes).unwrap();
        self.to.write_all(b"\n").unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.from.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send_raw(
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
                .to_string()
                .as_bytes(),
        );
        let reply = self.read();
        assert_eq!(reply["id"], json!(id));
        reply
    }

    fn initialize(&mut self) -> Value {
        let reply = self.call(
            "initialize",
            json!({"protocolVersion": MCP_VERSIONS[0], "capabilities": {},
                   "clientInfo": {"name": "t", "version": "1"}}),
        );
        self.send_raw(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        reply
    }

    /// Calls a tool: the decoded text content and whether the call failed.
    fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        let reply = self.call("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        (
            serde_json::from_str(text).unwrap(),
            result["isError"] == json!(true),
        )
    }
}

/// Runs `body` against a session of a server whose ceiling is `ceiling`.
fn session<T>(
    documents: &BTreeMap<String, Document>,
    ceiling: &[Permission],
    max_message: u64,
    body: impl FnOnce(&mut Client) -> T,
) -> T {
    let mut config = ServerConfig::new("arc-test", "0.1.0", ceiling.iter().copied());
    config.surface = Surface::Mcp;
    config.max_message_bytes = max_message;
    config.limits = Limits::DEFAULT;
    let registry = registry();
    let server = Server::new(&registry, documents, config, &NoLog);
    let mut mcp = McpConfig::new("main");
    mcp.messages
        .insert("test-create-description".into(), "Creates an item.".into());
    let (to, server_in) = pipe();
    let (server_out, from) = pipe();
    let from = from.with_timeout(Duration::from_secs(30));
    std::thread::scope(|scope| {
        let served = scope.spawn(|| server.serve_mcp(&mcp, server_in, server_out));
        let mut client = Client { to, from, next: 0 };
        let out = body(&mut client);
        drop(client);
        served.join().unwrap().unwrap();
        out
    })
}

fn documents() -> BTreeMap<String, Document> {
    BTreeMap::from([("main".to_owned(), document())])
}

#[test]
fn the_tools_are_the_fixed_ones_and_the_mcp_exposed_commands_of_the_catalog() {
    let docs = documents();
    let reply = session(&docs, &[Permission::Read], 1 << 20, |c| {
        let init = c.initialize();
        assert_eq!(init["result"]["protocolVersion"], MCP_VERSIONS[0]);
        assert_eq!(init["result"]["serverInfo"]["name"], "arc-test");
        assert_eq!(c.call("ping", json!({}))["result"], json!({}));
        c.call("tools/list", json!({}))
    });
    let tools = reply["result"]["tools"].as_array().unwrap();
    let names: BTreeSet<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    let catalog = registry().catalog();
    let mut expected: BTreeSet<String> = MCP_FIXED_TOOLS.iter().map(|n| (*n).to_owned()).collect();
    let mut schemas = BTreeMap::new();
    for entry in catalog["commands"].as_array().unwrap() {
        if entry["exposure"]
            .as_array()
            .unwrap()
            .contains(&json!("mcp"))
        {
            let name = entry["id"].as_str().unwrap().replace('.', "_");
            schemas.insert(name.clone(), entry["params"].clone());
            expected.insert(name);
        }
    }
    assert_eq!(
        names,
        expected.iter().map(String::as_str).collect::<BTreeSet<_>>()
    );
    // A command exposed only on automation is not a tool.
    assert!(!names.contains("test_item_random"));
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        if let Some(schema) = schemas.get(name) {
            assert_eq!(&tool["inputSchema"], schema, "{name}");
        }
    }
    let by_name = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap();
    assert_eq!(
        by_name("test_item_summary")["annotations"]["readOnlyHint"],
        true
    );
    assert_eq!(
        by_name("test_item_create")["annotations"]["readOnlyHint"],
        false
    );
    assert_eq!(
        by_name("test_item_create")["description"],
        "Creates an item."
    );
}

#[test]
fn edits_are_refused_without_the_edit_class_and_applied_with_it() {
    let create = json!({"name": "wall", "length": "2400 mm"});
    let docs = documents();
    session(&docs, &[Permission::Read], 1 << 20, |c| {
        c.initialize();
        let (error, failed) = c.tool("test_item_create", create.clone());
        assert!(failed);
        assert_eq!(error["code"], "cmd.permission_denied");
        let (found, failed) = c.tool("query_objects", json!({}));
        assert!(!failed);
        assert_eq!(found["objects"], json!([]));
        // Reads run in a read-only session.
        let (summary, failed) = c.tool("test_item_summary", json!({}));
        assert!(!failed);
        assert_eq!(summary["value"]["count"], 0);
    });
    assert_eq!(docs["main"].snapshot().iter().count(), 0);
    session(&docs, &[Permission::Read, Permission::Edit], 1 << 20, |c| {
        c.initialize();
        let (created, failed) = c.tool("test_item_create", create.clone());
        assert!(!failed, "{created}");
        let id = created["value"]["id"].as_str().unwrap().to_owned();
        let (found, _) = c.tool("query_objects", json!({"types": ["test.item"]}));
        assert_eq!(found["objects"].as_array().unwrap().len(), 1);
        let (object, failed) = c.tool("get_object", json!({"id": id}));
        assert!(!failed);
        assert_eq!(object["id"], json!(id));
        let (missing, failed) = c.tool(
            "get_object",
            json!({"id": "0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c"}),
        );
        assert!(failed);
        assert_eq!(missing["code"], "doc.object.not_found");
        let (list, failed) = c.tool("list_commands", json!({}));
        assert!(!failed);
        assert!(
            list["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == "test.item.create")
        );
        let (described, _) = c.tool("describe_command", json!({"command": "test.item.create"}));
        assert_eq!(described["id"], "test.item.create");
        let (diagnostics, failed) = c.tool("get_diagnostics", json!({}));
        assert!(!failed);
        assert!(diagnostics["diagnostics"].is_array());
        // Invalid parameters are a tool error, not a protocol error.
        let (invalid, failed) = c.tool("test_item_create", json!({"name": 3}));
        assert!(failed);
        assert_eq!(invalid["code"], "cmd.params.invalid");
    });
    assert_eq!(docs["main"].snapshot().iter().count(), 1);
}

#[test]
fn render_view_answers_not_available_without_a_renderer() {
    session(&documents(), &[Permission::Read], 1 << 20, |c| {
        c.initialize();
        let (error, failed) = c.tool("render_view", json!({"view": "plan"}));
        assert!(failed);
        assert_eq!(error["code"], "cmd.render.unavailable");
    });
}

#[test]
fn the_session_rules_and_bounds_hold() {
    session(&documents(), &[Permission::Read], 512, |c| {
        // Nothing but ping before initialize.
        let early = c.call("tools/list", json!({}));
        assert_eq!(early["error"]["message"], "cmd.protocol.not_initialized");
        assert_eq!(c.call("ping", json!({}))["result"], json!({}));
        let init = c.call("initialize", json!({"protocolVersion": "1999-01-01"}));
        assert_eq!(init["result"]["protocolVersion"], MCP_VERSIONS[0]);
        assert_eq!(
            c.call("initialize", json!({"protocolVersion": MCP_VERSIONS[1]}))["error"]["message"],
            "cmd.protocol.already_initialized"
        );
        assert_eq!(c.call("nope", json!({}))["error"]["code"], -32601);
        assert_eq!(
            c.call("tools/call", json!({"name": "no_such_tool"}))["error"]["code"],
            -32602
        );
        assert_eq!(
            c.call("tools/call", json!({"name": "get_object", "arguments": 3}))["error"]["code"],
            -32602
        );
        // A message over the limit is refused, and the next one is read normally.
        let big = json!({"jsonrpc": "2.0", "id": 99, "method": "ping",
                         "params": {"pad": "x".repeat(600)}});
        c.send_raw(big.to_string().as_bytes());
        let refused = c.read();
        assert_eq!(refused["error"]["code"], -32008);
        assert_eq!(
            refused["error"]["data"]["details"]["limit"],
            "message_bytes"
        );
        assert_eq!(refused["id"], Value::Null);
        assert_eq!(c.call("ping", json!({}))["result"], json!({}));
        // Hostile lines never end the session.
        for bad in [
            "not json",
            "[1]",
            "{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"ping\"}",
            "{\"id\":{}}",
        ] {
            c.send_raw(bad.as_bytes());
            assert!(c.read()["error"].is_object(), "{bad}");
        }
        assert_eq!(c.call("ping", json!({}))["result"], json!({}));
    });
}
