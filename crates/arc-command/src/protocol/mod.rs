//! The Arc Automation Protocol server (CORE-CMD-019, CORE-CMD-021, CORE-CMD-022,
//! CORE-CMD-027).
//!
//! [`Server::serve`] runs one session over a byte stream pair: newline-delimited JSON-RPC 2.0
//! read from any [`BufRead`] and written to any [`Write`]. The process that owns stdin and
//! stdout passes them; tests pass an in-memory [`pipe`]. Nothing but protocol messages is
//! written to the output; diagnostics go to the caller's [`Log`].
//!
//! Three threads serve a session: the caller's thread reads and answers requests, a worker runs
//! `command.run` requests one at a time in arrival order, and a writer owns the output. A
//! request that arrives while a command runs is answered at once (`command.cancel` among them);
//! a further `command.run` waits its turn. The rules are specified in
//! `docs/crates/arc-command.md`.

pub mod errors;
mod framing;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use arc_base::{Cancel, Error, Limits, Unit};
use arc_doc::{Document, Snapshot};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

pub use framing::{PipeReader, PipeWriter, pipe};

use crate::context::{Clock, FixedClock, Probe, ProgressSink, Session};
use crate::error::codes;
use crate::invoke::{Invocation, Invoker, Outcome};
use crate::registry::Registry;
use crate::roots::Roots;
use crate::schema::text_depth;
use crate::spec::{CommandSpec, Permission, Surface};
use framing::{Frame, next_frame};

/// The protocol's largest message, in bytes, newline excluded.
pub const MAX_MESSAGE_BYTES: u64 = 64 << 20;

/// The protocol versions this server speaks, `(major, minor)`, oldest first.
pub const SUPPORTED_VERSIONS: &[(u16, u16)] = &[(1, 0)];

/// The most `command.run` requests a session holds, running or waiting.
pub const MAX_QUEUED_RUNS: usize = 64;

/// The most messages waiting for the writer.
const OUTPUT_QUEUE: usize = 256;

/// The nesting the message envelope adds above command parameters (the message and its
/// `params`).
const ENVELOPE_DEPTH: u32 = 2;

/// The methods this server provides, in name order.
pub const METHODS: &[&str] = &[
    "arc.capabilities",
    "arc.initialize",
    "arc.ping",
    "arc.shutdown",
    "command.cancel",
    "command.describe",
    "command.list",
    "command.run",
    "command.validate",
];

/// The protocol methods of later capabilities, answered with code -32001.
pub const LATER_METHODS: &[&str] = &[
    "document.list",
    "document.new",
    "document.open",
    "document.save",
    "document.saveAs",
    "document.close",
    "document.info",
    "query.objects",
    "query.object",
    "query.references",
    "query.diagnostics",
    "query.history",
    "query.compare",
    "query.derived",
    "event.subscribe",
    "event.unsubscribe",
    "selection.get",
    "selection.set",
    "selection.add",
    "selection.remove",
    "view.list",
    "view.get",
    "view.setCamera",
    "view.render",
];

/// How much a log record matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Detail for diagnosing the protocol.
    Debug,
    /// A session event.
    Info,
    /// Something a client did wrong.
    Warning,
    /// A fault of the server.
    Error,
}

/// Where a session's diagnostics go; the process maps it to stderr.
pub trait Log: Send + Sync {
    /// Records `event` (a stable name such as `protocol.panic`) with free-form `detail`.
    fn log(&self, level: LogLevel, event: &str, detail: &str);
}

/// A log that drops everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLog;

impl Log for NoLog {
    fn log(&self, _: LogLevel, _: &str, _: &str) {}
}

/// What the host grants and reports in every session.
#[derive(Clone)]
pub struct ServerConfig {
    /// The product name reported by `arc.initialize`.
    pub product_name: String,
    /// The product version reported by `arc.initialize`.
    pub product_version: String,
    /// The most a session may be granted; requested classes beyond it are not granted.
    pub ceiling: BTreeSet<Permission>,
    /// The file roots of every session.
    pub roots: Roots,
    /// The hostile-input limits of messages and command parameters.
    pub limits: Limits,
    /// The largest accepted message, in bytes; the effective limit is also at most
    /// `limits.input_bytes`.
    pub max_message_bytes: u64,
    /// The clock of command sessions.
    pub clock: Arc<dyn Clock>,
    /// The context units of quantity parameters.
    pub units: Vec<Unit>,
}

impl std::fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerConfig")
            .field("product_name", &self.product_name)
            .field("ceiling", &self.ceiling)
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

impl ServerConfig {
    /// A configuration with the `ceiling`, no roots, the default limits, a 64 MiB message
    /// limit, a clock standing at 0 and no context units.
    #[must_use]
    pub fn new(
        product_name: impl Into<String>,
        product_version: impl Into<String>,
        ceiling: impl IntoIterator<Item = Permission>,
    ) -> Self {
        Self {
            product_name: product_name.into(),
            product_version: product_version.into(),
            ceiling: ceiling.into_iter().collect(),
            roots: Roots::none(),
            limits: Limits::DEFAULT,
            max_message_bytes: MAX_MESSAGE_BYTES,
            clock: Arc::new(FixedClock(0)),
            units: Vec::new(),
        }
    }

    /// A command session of this configuration granted `permissions`.
    fn session(&self, permissions: BTreeSet<Permission>) -> Session {
        Session {
            surface: Surface::Automation,
            permissions,
            limits: self.limits,
            clock: Arc::clone(&self.clock),
            units: self.units.clone(),
            roots: self.roots.clone(),
        }
    }

    fn max_line(&self) -> usize {
        let max = self.max_message_bytes.min(self.limits.input_bytes);
        usize::try_from(max).unwrap_or(usize::MAX)
    }
}

/// The protocol server of a registry and the documents a host has open, keyed by the
/// identifier clients name them with.
pub struct Server<'a> {
    registry: &'a Registry,
    documents: &'a BTreeMap<String, Document>,
    config: ServerConfig,
    log: &'a dyn Log,
}

impl std::fmt::Debug for Server<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// A protocol failure: the error and details that are not scalars.
struct Failure(Box<(Error, Map<String, Value>)>);

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self(Box::new((error, Map::new())))
    }
}

type Answer = Result<Value, Failure>;

enum Reply {
    Now(Answer),
    Later,
    Shutdown,
}

/// One queued `command.run`.
struct Job<'a> {
    id: Value,
    key: String,
    document: &'a Document,
    session: Session,
    invocation: Invocation,
}

type Pending = Mutex<BTreeMap<String, Cancel>>;

fn text(message: &Value) -> String {
    serde_json::to_string(message).unwrap_or_else(|_| {
        r#"{"error":{"code":-32603,"message":"cmd.protocol.internal"},"id":null,"jsonrpc":"2.0"}"#
            .to_owned()
    })
}

fn response(id: &Value, answer: Answer) -> String {
    text(&match answer {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(failure) => {
            let (error, extra) = *failure.0;
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": errors::error_object(&error, extra),
            })
        }
    })
}

fn invalid_request(reason: &'static str) -> Failure {
    Error::new(codes::PROTOCOL_INVALID_REQUEST)
        .with_detail("reason", reason)
        .into()
}

fn invalid_params(method: &str, reason: impl Into<String>) -> Failure {
    Error::new(codes::PROTOCOL_INVALID_PARAMS)
        .with_arg("method", method)
        .with_detail("reason", reason.into())
        .into()
}

fn limit(name: &'static str) -> Failure {
    Error::new(codes::PROTOCOL_LIMIT_EXCEEDED)
        .with_detail("limit", name)
        .into()
}

fn decode<T: DeserializeOwned>(method: &str, params: Map<String, Value>) -> Result<T, Failure> {
    serde_json::from_value(Value::Object(params)).map_err(|e| invalid_params(method, e.to_string()))
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeParams {
    client_name: String,
    protocol_versions: VersionRange,
    #[serde(default)]
    permissions: Vec<Value>,
}

#[derive(Deserialize)]
struct VersionRange {
    min: String,
    max: String,
}

#[derive(Deserialize)]
struct ListParams {
    #[serde(default)]
    document: Option<String>,
}

#[derive(Deserialize)]
struct DescribeParams {
    command: String,
    #[serde(default)]
    document: Option<String>,
}

#[derive(Deserialize)]
struct ValidateParams {
    command: String,
    #[serde(default = "empty_object")]
    params: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunParams {
    document: String,
    command: String,
    #[serde(default = "empty_object")]
    params: Value,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    preview: bool,
    #[serde(default)]
    seed: u64,
}

#[derive(Deserialize)]
struct CancelParams {
    id: Value,
}

/// Parses `major.minor`, each one to five digits.
fn parse_version(text: &str) -> Option<(u16, u16)> {
    let (major, minor) = text.split_once('.')?;
    let part = |p: &str| {
        (!p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
            .then(|| p.parse().ok())
            .flatten()
    };
    Some((part(major)?, part(minor)?))
}

fn version_text((major, minor): (u16, u16)) -> String {
    format!("{major}.{minor}")
}

/// The protocol version both sides speak: the newest supported one in `[min, max]`.
fn negotiate(min: (u16, u16), max: (u16, u16)) -> Option<(u16, u16)> {
    SUPPORTED_VERSIONS
        .iter()
        .rev()
        .copied()
        .find(|v| min <= *v && *v <= max)
}

fn capabilities() -> Value {
    json!({
        "methods": METHODS,
        "batch": false,
        "preview": false,
        "notifications": ["event.notify"],
    })
}

/// Sends progress of one run as `event.notify` notifications.
struct Progress {
    out: SyncSender<String>,
    id: Value,
}

impl ProgressSink for Progress {
    fn progress(&self, fraction: f64, message_id: &str) {
        let note = json!({
            "jsonrpc": "2.0",
            "method": "event.notify",
            "params": {
                "event": "command.progress",
                "requestId": self.id,
                "fraction": fraction,
                "messageId": message_id,
            },
        });
        // A closed output ends the session anyway.
        let _ = self.out.send(text(&note));
    }
}

fn run_result(outcome: &Outcome) -> Value {
    let diagnostics: Vec<Value> = outcome
        .diagnostics
        .iter()
        .map(|d| {
            let mut entry = errors::error_data(&d.error, Map::new());
            if let Some(map) = entry.as_object_mut() {
                let severity = match d.severity {
                    arc_doc::Severity::Error => "error",
                    arc_doc::Severity::Warning => "warning",
                };
                map.insert("severity".into(), json!(severity));
                let objects: Vec<String> = d.objects.iter().map(ToString::to_string).collect();
                map.insert("objects".into(), json!(objects));
            }
            entry
        })
        .collect();
    json!({
        "command": outcome.command,
        "revision": outcome.revision,
        "value": outcome.value,
        "diagnostics": diagnostics,
    })
}

fn write_lines<W: Write>(
    lines: &Receiver<String>,
    output: &mut W,
    closed: &AtomicBool,
) -> io::Result<()> {
    let mut failed = None;
    for line in lines {
        if failed.is_none() {
            let written = output
                .write_all(line.as_bytes())
                .and_then(|()| output.write_all(b"\n"))
                .and_then(|()| output.flush());
            if let Err(e) = written {
                closed.store(true, Ordering::Release);
                failed = Some(e);
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

impl<'a> Server<'a> {
    /// A server of `registry`'s commands over `documents`, logging to `log`.
    #[must_use]
    pub fn new(
        registry: &'a Registry,
        documents: &'a BTreeMap<String, Document>,
        config: ServerConfig,
        log: &'a dyn Log,
    ) -> Self {
        Self {
            registry,
            documents,
            config,
            log,
        }
    }

    /// Serves one session: reads requests from `input` until `arc.shutdown` or the end of the
    /// stream and writes responses and notifications to `output`, one message per line. At the
    /// end, running and waiting commands are cancelled (they roll back and are answered) before
    /// the `arc.shutdown` response.
    ///
    /// # Errors
    ///
    /// A read or write error of the streams, or `Unsupported` when the session threads cannot
    /// be started (WebAssembly without threads).
    pub fn serve<R: BufRead, W: Write + Send>(
        &self,
        mut input: R,
        mut output: W,
    ) -> io::Result<()> {
        let pending = Pending::default();
        let closed_flag = AtomicBool::new(false);
        let closed = &closed_flag;
        // Bounded, so a client that stops reading slows the session down instead of growing it.
        let (out, lines) = sync_channel::<String>(OUTPUT_QUEUE);
        let (jobs, queue) = channel::<Job<'_>>();
        thread::scope(|scope| {
            let writer = thread::Builder::new()
                .name("arc-protocol-writer".into())
                .spawn_scoped(scope, move || write_lines(&lines, &mut output, closed))?;
            let worker_out = out.clone();
            let pending_ref = &pending;
            let worker = thread::Builder::new()
                .name("arc-protocol-worker".into())
                .spawn_scoped(scope, move || self.work(&queue, &worker_out, pending_ref));
            let mut reader = Reader {
                server: self,
                out,
                jobs: Some(jobs),
                pending: &pending,
                closed,
                session: None,
                current: None,
            };
            let result = match worker {
                Ok(worker) => {
                    let result = reader.run(&mut input);
                    reader.finish();
                    let _ = worker.join();
                    reader.shutdown_reply();
                    result
                }
                Err(e) => Err(e),
            };
            drop(reader);
            let written = writer
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("writer fault")));
            result.and(written)
        })
    }

    fn work(&self, queue: &Receiver<Job<'_>>, out: &SyncSender<String>, pending: &Pending) {
        for job in queue {
            let Job {
                id,
                key,
                document,
                session,
                invocation,
            } = job;
            let answer = catch_unwind(AssertUnwindSafe(|| {
                if invocation.cancel.is_cancelled() {
                    return Err(Error::new(codes::CANCELLED).into());
                }
                self.invoke(document, &session, invocation)
            }))
            .unwrap_or_else(|payload| {
                self.log.log(
                    LogLevel::Error,
                    "protocol.panic",
                    &panic_text(payload.as_ref()),
                );
                Err(Error::new(codes::PROTOCOL_INTERNAL).into())
            });
            pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&key);
            let _ = out.send(response(&id, answer));
        }
    }

    /// Answers one command method (`command.list`, `command.describe`, `command.validate` or
    /// `command.run`) with `params` as a request would, without framing or threads: in a
    /// session granted the whole `ceiling`, with `command.run` executed on the calling thread.
    /// The headless runner answers its one-shot subcommands with it.
    ///
    /// # Errors
    ///
    /// The JSON-RPC error object the request would be answered with.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, Value> {
        let session = self.config.session(self.config.ceiling.clone());
        let answer = catch_unwind(AssertUnwindSafe(|| {
            let Value::Object(params) = params else {
                return Err(invalid_request("params"));
            };
            match method {
                "command.run" => decode::<RunParams>(method, params).and_then(|p| {
                    if p.preview {
                        return Err(Error::new(codes::PREVIEW_UNSUPPORTED).into());
                    }
                    let document = self.document(&p.document)?;
                    let mut invocation = Invocation::new(p.command, p.params);
                    invocation.seed = p.seed;
                    invocation.expected_revision = p.expected_revision;
                    self.invoke(document, &session, invocation)
                }),
                _ => self.query(&session, method, params),
            }
        }))
        .unwrap_or_else(|payload| {
            self.log.log(
                LogLevel::Error,
                "protocol.panic",
                &panic_text(payload.as_ref()),
            );
            Err(Error::new(codes::PROTOCOL_INTERNAL).into())
        });
        answer.map_err(|failure| {
            let (error, extra) = *failure.0;
            errors::error_object(&error, extra)
        })
    }

    /// Answers the command methods that do not run a command; any other method is unknown.
    fn query(&self, session: &Session, method: &str, params: Map<String, Value>) -> Answer {
        match method {
            "command.list" => decode::<ListParams>(method, params).and_then(|p| {
                let snapshot = p
                    .document
                    .map(|d| self.document(&d).map(Document::snapshot))
                    .transpose()?;
                let commands: Vec<Value> = self
                    .registry
                    .iter()
                    .filter(|spec| spec.exposure().contains(&Surface::Automation))
                    .map(|spec| self.entry(spec, session, snapshot.as_ref()))
                    .collect();
                Ok(json!({"commands": commands}))
            }),
            "command.describe" => decode::<DescribeParams>(method, params).and_then(|p| {
                let spec = self.command(&p.command)?;
                let snapshot = p
                    .document
                    .map(|d| self.document(&d).map(Document::snapshot))
                    .transpose()?;
                Ok(self.entry(&spec, session, snapshot.as_ref()))
            }),
            "command.validate" => decode::<ValidateParams>(method, params).and_then(|p| {
                let spec = self.command(&p.command)?;
                let violations = spec.validate(&p.params, &session.limits)?;
                Ok(json!({"valid": violations.is_empty(), "violations": violations}))
            }),
            _ => Err(Error::new(codes::PROTOCOL_METHOD_UNKNOWN)
                .with_arg("method", method)
                .into()),
        }
    }

    /// Runs `invocation` through the invoker and logs the panic of a faulted command.
    fn invoke(&self, document: &Document, session: &Session, invocation: Invocation) -> Answer {
        match Invoker::new(self.registry).invoke(document, session, invocation) {
            Ok(outcome) => Ok(run_result(&outcome)),
            Err(error) => {
                if let Some(panic) = error.details().get("panic") {
                    self.log
                        .log(LogLevel::Error, "command.fault", &format!("{panic:?}"));
                }
                Err(Failure::from(error))
            }
        }
    }

    fn document(&self, id: &str) -> Result<&'a Document, Failure> {
        self.documents.get(id).ok_or_else(|| {
            Error::new(codes::DOCUMENT_UNKNOWN)
                .with_arg("document", id)
                .into()
        })
    }

    /// The command named `name`, which must be exposed on the automation surface.
    fn command(&self, name: &str) -> Result<Arc<CommandSpec>, Failure> {
        let spec = self.registry.resolve(name, &BTreeMap::new())?.spec;
        if spec.exposure().contains(&Surface::Automation) {
            Ok(spec)
        } else {
            Err(Error::new(codes::NOT_EXPOSED)
                .with_arg("command", spec.id())
                .into())
        }
    }

    /// The catalog entry of `spec` with `permitted`, and with `enabled` (and
    /// `disabledReason`) when a snapshot is given.
    fn entry(&self, spec: &CommandSpec, session: &Session, snapshot: Option<&Snapshot>) -> Value {
        let mut entry = self.registry.entry(spec);
        let Some(map) = entry.as_object_mut() else {
            return entry;
        };
        map.insert(
            "permitted".into(),
            json!(spec.permissions().is_subset(&session.permissions)),
        );
        if let Some(snapshot) = snapshot {
            let probe = Probe {
                snapshot,
                selection: &[],
                session,
            };
            let verdict = catch_unwind(AssertUnwindSafe(|| spec.enabled(&probe)));
            let reason = match verdict {
                Ok(Ok(())) => None,
                Ok(Err(disabled)) => {
                    let arguments: Map<String, Value> = disabled
                        .arguments
                        .iter()
                        .map(|(k, v)| {
                            ((*k).to_owned(), serde_json::to_value(v).unwrap_or_default())
                        })
                        .collect();
                    Some(json!({"messageId": disabled.message_id, "arguments": arguments}))
                }
                Err(_) => Some(json!({"messageId": codes::FAULT.message_id, "arguments": {}})),
            };
            map.insert("enabled".into(), json!(reason.is_none()));
            if let Some(reason) = reason {
                map.insert("disabledReason".into(), reason);
            }
        }
        entry
    }
}

/// The reading side of one session.
struct Reader<'s, 'a> {
    server: &'s Server<'a>,
    out: SyncSender<String>,
    jobs: Option<std::sync::mpsc::Sender<Job<'a>>>,
    pending: &'s Pending,
    closed: &'s AtomicBool,
    session: Option<Session>,
    /// The id of a pending `arc.shutdown`.
    current: Option<Value>,
}

impl<'a> Reader<'_, 'a> {
    fn send(&self, line: String) {
        let _ = self.out.send(line);
    }

    fn run<R: BufRead>(&mut self, input: &mut R) -> io::Result<()> {
        let max = self.server.config.max_line();
        let mut buf = Vec::new();
        while !self.closed.load(Ordering::Acquire) {
            match next_frame(input, max, &mut buf)? {
                Frame::End => return Ok(()),
                Frame::TooLong => {
                    self.server
                        .log
                        .log(LogLevel::Warning, "protocol.too_long", "");
                    self.send(response(&Value::Null, Err(limit("message_bytes"))));
                }
                Frame::Line => {
                    if self.line(&buf) {
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    /// Handles one line; true when the session shuts down.
    fn line(&mut self, bytes: &[u8]) -> bool {
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return false;
        }
        let mut id = None;
        let handled = catch_unwind(AssertUnwindSafe(|| self.message(bytes, &mut id)));
        match handled {
            Ok(shutdown) => shutdown,
            Err(payload) => {
                self.server.log.log(
                    LogLevel::Error,
                    "protocol.panic",
                    &panic_text(payload.as_ref()),
                );
                if let Some(id) = id {
                    let failure = Error::new(codes::PROTOCOL_INTERNAL).into();
                    self.send(response(&id, Err(failure)));
                }
                false
            }
        }
    }

    /// Handles one message; `id` receives the request id once known. True on shutdown.
    fn message(&mut self, bytes: &[u8], id: &mut Option<Value>) -> bool {
        let null = Value::Null;
        let fail = |this: &Self, to: &Value, failure: Failure| {
            this.send(response(to, Err(failure)));
            false
        };
        let parse =
            |reason| Failure::from(Error::new(codes::PROTOCOL_PARSE).with_detail("reason", reason));
        let Ok(text) = std::str::from_utf8(bytes) else {
            return fail(self, &null, parse("utf8"));
        };
        let depth = self
            .server
            .config
            .limits
            .nesting_depth
            .saturating_add(ENVELOPE_DEPTH);
        if text_depth(text) > depth {
            return fail(self, &null, limit("nesting_depth"));
        }
        let message = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(message)) => message,
            Ok(Value::Array(_)) => return fail(self, &null, invalid_request("batch")),
            Ok(_) => return fail(self, &null, invalid_request("not_object")),
            Err(_) => return fail(self, &null, parse("json")),
        };
        let request_id = match message.get("id") {
            None => None,
            Some(v) if v.is_string() || v.is_i64() || v.is_u64() => Some(v.clone()),
            Some(_) => return fail(self, &null, invalid_request("id")),
        };
        id.clone_from(&request_id);
        let to = request_id.clone().unwrap_or(Value::Null);
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return fail(self, &to, invalid_request("jsonrpc"));
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return fail(self, &to, invalid_request("method"));
        };
        let Some(request_id) = request_id else {
            self.server
                .log
                .log(LogLevel::Debug, "protocol.notification_ignored", method);
            return false;
        };
        let params = match message.get("params") {
            None => Map::new(),
            Some(Value::Object(params)) => params.clone(),
            Some(Value::Array(_)) => {
                return fail(self, &to, invalid_params(method, "by_position"));
            }
            Some(_) => return fail(self, &to, invalid_request("params")),
        };
        match self.dispatch(&request_id, method, params) {
            Reply::Now(answer) => {
                self.send(response(&request_id, answer));
                false
            }
            Reply::Later => false,
            Reply::Shutdown => {
                self.current = Some(request_id);
                true
            }
        }
    }

    fn dispatch(&mut self, id: &Value, method: &str, params: Map<String, Value>) -> Reply {
        if !METHODS.contains(&method) && !LATER_METHODS.contains(&method) {
            return Reply::Now(Err(Error::new(codes::PROTOCOL_METHOD_UNKNOWN)
                .with_arg("method", method)
                .into()));
        }
        let Some(session) = self.session.clone() else {
            return Reply::Now(if method == "arc.initialize" {
                self.initialize(params)
            } else {
                Err(Error::new(codes::PROTOCOL_NOT_INITIALIZED).into())
            });
        };
        let server = self.server;
        Reply::Now(match method {
            "arc.initialize" => Err(Error::new(codes::PROTOCOL_ALREADY_INITIALIZED).into()),
            "arc.shutdown" => return Reply::Shutdown,
            "arc.ping" => Ok(json!({})),
            "arc.capabilities" => Ok(capabilities()),
            "command.list" | "command.describe" | "command.validate" => {
                server.query(&session, method, params)
            }
            "command.run" => match decode::<RunParams>(method, params) {
                Ok(p) => return self.run_command(id, session, p),
                Err(failure) => Err(failure),
            },
            "command.cancel" => decode::<CancelParams>(method, params).map(|p| {
                let key = text(&p.id);
                let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                let cancel = pending.get(&key);
                if let Some(cancel) = cancel {
                    cancel.cancel();
                }
                json!({"cancelled": cancel.is_some()})
            }),
            _ => Err(Error::new(codes::PROTOCOL_METHOD_UNSUPPORTED)
                .with_arg("method", method)
                .into()),
        })
    }

    fn initialize(&mut self, params: Map<String, Value>) -> Answer {
        const METHOD: &str = "arc.initialize";
        let p: InitializeParams = decode(METHOD, params)?;
        if p.client_name.is_empty() || p.client_name.len() > 256 {
            return Err(invalid_params(METHOD, "client_name"));
        }
        let (Some(min), Some(max)) = (
            parse_version(&p.protocol_versions.min),
            parse_version(&p.protocol_versions.max),
        ) else {
            return Err(invalid_params(METHOD, "version"));
        };
        let Some(version) = negotiate(min, max) else {
            let supported: Vec<String> = SUPPORTED_VERSIONS
                .iter()
                .copied()
                .map(version_text)
                .collect();
            let mut extra = Map::new();
            extra.insert("supported".into(), json!(supported));
            return Err(Failure(Box::new((
                Error::new(codes::PROTOCOL_VERSION_UNSUPPORTED),
                extra,
            ))));
        };
        let mut requested = BTreeSet::new();
        for class in p.permissions {
            let class: Permission =
                serde_json::from_value(class).map_err(|_| invalid_params(METHOD, "permission"))?;
            requested.insert(class);
        }
        let config = &self.server.config;
        let granted: BTreeSet<Permission> =
            requested.intersection(&config.ceiling).copied().collect();
        self.server
            .log
            .log(LogLevel::Info, "protocol.initialized", &p.client_name);
        let paths = |roots: &[std::path::PathBuf], shown: bool| -> Vec<String> {
            if shown {
                roots
                    .iter()
                    .map(|r| r.to_string_lossy().into_owned())
                    .collect()
            } else {
                Vec::new()
            }
        };
        let can_write = granted.contains(&Permission::FileWrite);
        let can_read = can_write || granted.contains(&Permission::FileRead);
        let result = json!({
            "protocolVersion": version_text(version),
            "server": {
                "name": config.product_name,
                "version": config.product_version,
                "arcCore": env!("CARGO_PKG_VERSION"),
            },
            "permissions": granted,
            "roots": {
                "read": paths(config.roots.read_roots(), can_read),
                "write": paths(config.roots.write_roots(), can_write),
            },
            "capabilities": capabilities(),
            "limits": {
                "messageBytes": config.max_line(),
                "nestingDepth": config.limits.nesting_depth,
                "queuedRuns": MAX_QUEUED_RUNS,
            },
        });
        self.session = Some(config.session(granted));
        Ok(result)
    }

    fn run_command(&mut self, id: &Value, session: Session, p: RunParams) -> Reply {
        if p.preview {
            return Reply::Now(Err(Error::new(codes::PREVIEW_UNSUPPORTED).into()));
        }
        let document = match self.server.document(&p.document) {
            Ok(document) => document,
            Err(failure) => return Reply::Now(Err(failure)),
        };
        let key = text(id);
        let cancel = Cancel::new();
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.contains_key(&key) {
                return Reply::Now(Err(invalid_request("duplicate_id")));
            }
            if pending.len() >= MAX_QUEUED_RUNS {
                return Reply::Now(Err(limit("queued_runs")));
            }
            pending.insert(key.clone(), cancel.clone());
        }
        let mut invocation = Invocation::new(p.command, p.params);
        invocation.seed = p.seed;
        invocation.cancel = cancel;
        invocation.expected_revision = p.expected_revision;
        invocation.progress = Some(Arc::new(Progress {
            out: self.out.clone(),
            id: id.clone(),
        }));
        let job = Job {
            id: id.clone(),
            key: key.clone(),
            document,
            session,
            invocation,
        };
        let sent = self
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok());
        if sent {
            Reply::Later
        } else {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&key);
            Reply::Now(Err(Error::new(codes::PROTOCOL_INTERNAL).into()))
        }
    }

    /// Cancels running and waiting commands and closes the queue.
    fn finish(&mut self) {
        for cancel in self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            cancel.cancel();
        }
        self.jobs = None;
    }

    /// Answers a pending `arc.shutdown`, after the worker finished.
    fn shutdown_reply(&mut self) {
        if let Some(id) = self.current.take() {
            self.send(response(&id, Ok(Value::Null)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_negotiate() {
        assert_eq!(parse_version("1.0"), Some((1, 0)));
        assert_eq!(parse_version("12.345"), Some((12, 345)));
        for bad in [
            "1", "1.", ".1", "1.0.0", "a.b", "+1.0", "1.-0", "123456.0", "99999.0",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert_eq!(negotiate((1, 0), (1, 9)), Some((1, 0)));
        assert_eq!(negotiate((0, 1), (0, 9)), None);
        assert_eq!(negotiate((2, 0), (1, 0)), None);
    }
}
