//! The Model Context Protocol endpoint (CORE-CMD-029).
//!
//! [`Server::serve_mcp`] speaks the Model Context Protocol over a byte stream pair with the same
//! newline-delimited framing as the automation protocol. It is an adapter over the server's
//! registry and documents: the tools are the commands exposed on [`Surface::Mcp`] plus six fixed
//! tools, and every call goes through [`Server::call`], so exposure, permission classes (the
//! server's ceiling decides whether edits are allowed), limits, schemas and predicates are those
//! of the automation protocol. The endpoint serves one document and answers one request at a
//! time. The rules are specified in `docs/crates/arc-command.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use arc_base::Error;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::framing::{Frame, next_frame};
use super::{
    Answer, ENVELOPE_DEPTH, Failure, LogLevel, Server, decode, errors, invalid_params,
    invalid_request, limit, panic_text, response,
};
use crate::error::codes;
use crate::schema::text_depth;
use crate::spec::{Permission, Surface};

/// The Model Context Protocol versions the endpoint speaks, newest first. A client that asks
/// for another version is answered with the first.
pub const MCP_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The fixed tools, besides one tool per command exposed on the MCP surface.
pub const MCP_FIXED_TOOLS: &[&str] = &[
    "describe_command",
    "get_diagnostics",
    "get_object",
    "list_commands",
    "query_objects",
    "render_view",
];

/// What the endpoint serves.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// The identifier of the document every tool works on.
    pub document: String,
    /// Texts of message identifiers (command descriptions and labels), from the active
    /// catalog; an identifier without text is used as it is.
    pub messages: BTreeMap<String, String>,
}

impl McpConfig {
    /// A configuration serving `document` with no message texts.
    #[must_use]
    pub fn new(document: impl Into<String>) -> Self {
        Self {
            document: document.into(),
            messages: BTreeMap::new(),
        }
    }
}

/// The tool name of a command: its identifier with the dots replaced by underscores.
#[must_use]
pub fn tool_name(command: &str) -> String {
    command.replace('.', "_")
}

fn object_schema(properties: &Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn fixed_tools() -> Vec<Value> {
    let page = json!({
        "limit": {"type": "integer", "minimum": 1, "description": "The page size."},
        "cursor": {"type": "string", "description": "The nextCursor of the previous page."},
    });
    let with_page = |mut properties: Value, extra: Value| {
        if let (Some(map), Some(page), Some(extra)) = (
            properties.as_object_mut(),
            page.as_object(),
            extra.as_object(),
        ) {
            map.extend(page.clone());
            map.extend(extra.clone());
        }
        properties
    };
    let tool = |name: &str, description: &str, schema: Value| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": schema,
            "annotations": {"readOnlyHint": true},
        })
    };
    vec![
        tool(
            "describe_command",
            "Describes one command: its parameters, result and permission classes.",
            object_schema(&json!({"command": {"type": "string"}}), &["command"]),
        ),
        tool(
            "get_diagnostics",
            "Lists the diagnostics of the document.",
            object_schema(
                &with_page(
                    json!({}),
                    json!({"severity": {"type": "string", "enum": ["error", "warning"]}}),
                ),
                &[],
            ),
        ),
        tool(
            "get_object",
            "Returns one object by identifier.",
            object_schema(
                &json!({
                    "id": {"type": "string", "format": "uuid"},
                    "fields": {"type": "array", "items": {"type": "string"}},
                }),
                &["id"],
            ),
        ),
        tool(
            "list_commands",
            "Lists the commands of the product with their enabled state.",
            object_schema(&json!({}), &[]),
        ),
        tool(
            "query_objects",
            "Lists objects of the document, filtered by type, identifier or owner.",
            object_schema(
                &with_page(
                    json!({}),
                    json!({
                        "types": {"type": "array", "items": {"type": "string"}},
                        "ids": {"type": "array", "items": {"type": "string", "format": "uuid"}},
                        "owner": {"type": "string", "format": "uuid"},
                        "fields": {"type": "array", "items": {"type": "string"}},
                    }),
                ),
                &[],
            ),
        ),
        tool(
            "render_view",
            "Renders a view to an image; answers cmd.render.unavailable when no renderer is attached.",
            object_schema(
                &json!({
                    "view": {"type": "string"},
                    "width": {"type": "integer", "minimum": 1},
                    "height": {"type": "integer", "minimum": 1},
                }),
                &[],
            ),
        ),
    ]
}

#[derive(Deserialize)]
struct CallParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

/// One MCP session.
struct Session<'s, 'a> {
    server: &'s Server<'a>,
    config: &'s McpConfig,
    initialized: bool,
}

fn parse_failure(reason: &'static str) -> Failure {
    Error::new(codes::PROTOCOL_PARSE)
        .with_detail("reason", reason)
        .into()
}

impl Session<'_, '_> {
    fn message(&self, id: &str) -> String {
        self.config
            .messages
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_owned())
    }

    /// Handles one line; the response to write, if any.
    fn line(&mut self, bytes: &[u8]) -> Option<String> {
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return None;
        }
        catch_unwind(AssertUnwindSafe(|| self.message_of(bytes))).unwrap_or_else(|payload| {
            self.server
                .log
                .log(LogLevel::Error, "mcp.panic", &panic_text(payload.as_ref()));
            Some(response(
                &Value::Null,
                Err(Error::new(codes::PROTOCOL_INTERNAL).into()),
            ))
        })
    }

    fn message_of(&mut self, bytes: &[u8]) -> Option<String> {
        let null = Value::Null;
        let fail = |id: &Value, failure: Failure| Some(response(id, Err(failure)));
        let Ok(text) = std::str::from_utf8(bytes) else {
            return fail(&null, parse_failure("utf8"));
        };
        let depth = self
            .server
            .config
            .limits
            .nesting_depth
            .saturating_add(ENVELOPE_DEPTH);
        if text_depth(text) > depth {
            return fail(&null, limit("nesting_depth"));
        }
        let message = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(message)) => message,
            Ok(Value::Array(_)) => return fail(&null, invalid_request("batch")),
            Ok(_) => return fail(&null, invalid_request("not_object")),
            Err(_) => return fail(&null, parse_failure("json")),
        };
        let id = match message.get("id") {
            None => None,
            Some(v) if v.is_string() || v.is_i64() || v.is_u64() => Some(v.clone()),
            Some(_) => return fail(&null, invalid_request("id")),
        };
        let to = id.clone().unwrap_or(Value::Null);
        // A response from the client answers no request of ours.
        if message.get("method").is_none()
            && (message.contains_key("result") || message.contains_key("error"))
        {
            return None;
        }
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return fail(&to, invalid_request("jsonrpc"));
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return fail(&to, invalid_request("method"));
        };
        let Some(id) = id else {
            // Notifications (`notifications/initialized`, `notifications/cancelled`) need no
            // answer and change nothing: requests are answered one at a time.
            self.server
                .log
                .log(LogLevel::Debug, "mcp.notification_ignored", method);
            return None;
        };
        let params = match message.get("params") {
            None => Map::new(),
            Some(Value::Object(params)) => params.clone(),
            Some(_) => return fail(&id, invalid_params(method, "params")),
        };
        Some(response(&id, self.dispatch(method, params)))
    }

    fn dispatch(&mut self, method: &str, params: Map<String, Value>) -> Answer {
        match method {
            "ping" => Ok(json!({})),
            "initialize" => self.initialize(params),
            "tools/list" | "tools/call" if !self.initialized => {
                Err(Error::new(codes::PROTOCOL_NOT_INITIALIZED).into())
            }
            "tools/list" => {
                if params.get("cursor").is_some_and(|c| !c.is_null()) {
                    return Err(invalid_params(method, "cursor"));
                }
                Ok(json!({"tools": self.tools()}))
            }
            "tools/call" => decode::<CallParams>(method, params).and_then(|p| self.call_tool(p)),
            _ => Err(Error::new(codes::PROTOCOL_METHOD_UNKNOWN)
                .with_arg("method", method)
                .into()),
        }
    }

    fn initialize(&mut self, params: Map<String, Value>) -> Answer {
        const METHOD: &str = "initialize";
        if self.initialized {
            return Err(Error::new(codes::PROTOCOL_ALREADY_INITIALIZED).into());
        }
        let Some(asked) = params.get("protocolVersion").and_then(Value::as_str) else {
            return Err(invalid_params(METHOD, "protocol_version"));
        };
        let version = MCP_VERSIONS
            .iter()
            .find(|v| **v == asked)
            .or_else(|| MCP_VERSIONS.first())
            .copied()
            .unwrap_or_default();
        self.initialized = true;
        let config = &self.server.config;
        Ok(json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": config.product_name, "version": config.product_version},
        }))
    }

    /// The tools: the fixed ones, then one per command exposed on the MCP surface in identifier
    /// order. When two identifiers give the same tool name (`a.b_c.d` and `a.b.c_d`), the
    /// first in identifier order keeps it and the other is not offered.
    fn tools(&self) -> Vec<Value> {
        let mut tools = fixed_tools();
        let mut names: BTreeSet<String> = MCP_FIXED_TOOLS.iter().map(|n| (*n).to_owned()).collect();
        for spec in self
            .server
            .registry
            .iter()
            .filter(|s| s.exposure().contains(&Surface::Mcp))
        {
            let name = tool_name(spec.id());
            if !names.insert(name.clone()) {
                self.server
                    .log
                    .log(LogLevel::Warning, "mcp.tool_name_taken", spec.id());
                continue;
            }
            let read_only = spec.permissions().iter().all(|p| *p == Permission::Read);
            tools.push(json!({
                "name": name,
                "description": self.message(spec.description()),
                "inputSchema": spec.params_schema(),
                "annotations": {
                    "title": self.message(spec.label()),
                    "readOnlyHint": read_only,
                },
            }));
        }
        tools
    }

    fn call_tool(&self, call: CallParams) -> Answer {
        const METHOD: &str = "tools/call";
        let arguments = match call.arguments {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return Err(invalid_params(METHOD, "arguments")),
        };
        let document = &self.config.document;
        // The tools work on the served document only: a `document` argument is replaced.
        let with_document = |mut arguments: Map<String, Value>| {
            arguments.insert("document".into(), json!(document));
            Value::Object(arguments)
        };
        let server = self.server;
        let outcome =
            match call.name.as_str() {
                "list_commands" => server.call("command.list", with_document(Map::new())),
                "describe_command" => server.call("command.describe", with_document(arguments)),
                "query_objects" => server.call("query.objects", with_document(arguments)),
                "get_diagnostics" => server.call("query.diagnostics", with_document(arguments)),
                "get_object" => self.get_object(arguments),
                "render_view" => Err(errors::error_object(
                    &Error::new(codes::RENDER_UNAVAILABLE),
                    Map::new(),
                )),
                name => {
                    let Some(spec) = server.registry.iter().find(|s| {
                        s.exposure().contains(&Surface::Mcp) && tool_name(s.id()) == name
                    }) else {
                        return Err(invalid_params(METHOD, "tool"));
                    };
                    server.call(
                        "command.run",
                        json!({"document": document, "command": spec.id(), "params": arguments}),
                    )
                }
            };
        let (value, is_error) = match outcome {
            Ok(value) => (value, false),
            Err(error) => (error.get("data").cloned().unwrap_or(error), true),
        };
        Ok(json!({
            "content": [{"type": "text", "text": value.to_string()}],
            "isError": is_error,
        }))
    }

    /// `get_object`: `query.objects` with one identifier, answering the object itself.
    fn get_object(&self, arguments: Map<String, Value>) -> Result<Value, Value> {
        let Some(id) = arguments.get("id").cloned() else {
            return Err(errors::error_object(
                &Error::new(codes::PROTOCOL_INVALID_PARAMS)
                    .with_arg("method", "get_object")
                    .with_detail("reason", "id"),
                Map::new(),
            ));
        };
        let mut params = Map::new();
        params.insert("document".into(), json!(self.config.document));
        params.insert("ids".into(), json!([id]));
        if let Some(fields) = arguments.get("fields") {
            params.insert("fields".into(), fields.clone());
        }
        let found = self.server.call("query.objects", Value::Object(params))?;
        match found
            .get("objects")
            .and_then(Value::as_array)
            .and_then(|objects| objects.first())
        {
            Some(object) => Ok(object.clone()),
            None => Err(errors::error_object(
                &Error::new(arc_doc::error::codes::OBJECT_NOT_FOUND),
                Map::new(),
            )),
        }
    }
}

impl Server<'_> {
    /// Serves one Model Context Protocol session over `input` and `output`: newline-delimited
    /// JSON-RPC like the automation protocol, until the end of the input. Requests are
    /// answered one at a time on the calling thread. The tools are the commands exposed on
    /// [`Surface::Mcp`] (so the server's [`ServerConfig::surface`](super::ServerConfig::surface)
    /// must be `Mcp`) and the fixed tools [`MCP_FIXED_TOOLS`]; the server's permission ceiling
    /// decides what a call may do, so a ceiling without `edit` makes every edit tool fail with
    /// `cmd.permission_denied`.
    ///
    /// # Errors
    ///
    /// A read or write error of the streams.
    pub fn serve_mcp<R: BufRead, W: Write>(
        &self,
        config: &McpConfig,
        mut input: R,
        mut output: W,
    ) -> io::Result<()> {
        let mut session = Session {
            server: self,
            config,
            initialized: false,
        };
        let max = self.config.max_line();
        let mut buf = Vec::new();
        loop {
            let reply = match next_frame(&mut input, max, &mut buf)? {
                Frame::End => return Ok(()),
                Frame::TooLong => {
                    self.log.log(LogLevel::Warning, "mcp.too_long", "");
                    Some(response(&Value::Null, Err(limit("message_bytes"))))
                }
                Frame::Line => session.line(&buf),
            };
            if let Some(line) = reply {
                output.write_all(line.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
    }
}
