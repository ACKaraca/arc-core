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

mod encode;
pub mod errors;
mod events;
mod framing;
mod mcp;
mod query;
mod run;
mod socket;
mod view;

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

pub use events::{EVENT_QUEUE, EVENTS, MAX_SELECTED};
pub use framing::{PipeReader, PipeWriter, pipe};
pub use mcp::{MCP_FIXED_TOOLS, MCP_VERSIONS, McpConfig, tool_name};
pub use query::MAX_PAGE;
#[cfg(windows)]
pub use socket::pipe_path;
pub use socket::random_token;
pub use socket::{
    ConnectionObserver, HANDSHAKE_TIMEOUT, LocalSocket, MAX_CLIENT_NAME_BYTES, MAX_CLIENTS,
    MAX_CONNECTIONS, MAX_HANDSHAKE_BYTES, MAX_HANDSHAKING, NoObserver, SOCKET_PATH_LIMIT,
    SocketConfig, SocketStop,
};
pub use view::{
    DEFAULT_DISPLAY_MODE, DEFAULT_SIZE, DEFAULT_VIEW, MAX_INLINE_BYTES, MAX_PIXELS, MAX_SECTIONS,
    MAX_SIDE, ViewRenderer, ViewRequest, base64,
};

use crate::context::{Clock, FixedClock, Probe, ProgressSink, Session};
use crate::error::codes;
use crate::history::Direction;
use crate::invoke::{Invocation, Invoker, Outcome, Preview};
use crate::macros::Recorder;
use crate::registry::Registry;
use crate::roots::Roots;
use crate::schema::text_depth;
use crate::spec::{CommandSpec, Permission, Surface};
use events::Hub;
use framing::{Frame, next_frame};
use run::{DocState, Target};

/// The protocol's largest message, in bytes, newline excluded.
pub const MAX_MESSAGE_BYTES: u64 = 64 << 20;

/// The protocol versions this server speaks, `(major, minor)`, oldest first.
pub const SUPPORTED_VERSIONS: &[(u16, u16)] = &[(1, 0)];

/// The most `command.run` requests a session holds, running or waiting.
pub const MAX_QUEUED_RUNS: usize = 64;

/// The most messages waiting for the writer.
const OUTPUT_QUEUE: usize = 256;

/// How long the event pump of a session sleeps when nothing happened.
const EVENT_POLL: std::time::Duration = std::time::Duration::from_millis(10);

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
    "event.subscribe",
    "event.unsubscribe",
    "history.redo",
    "history.undo",
    "query.diagnostics",
    "query.history",
    "query.objects",
    "query.references",
    "selection.get",
    "selection.set",
    "view.render",
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
    "query.object",
    "query.compare",
    "query.derived",
    "selection.add",
    "selection.remove",
    "view.list",
    "view.get",
    "view.setCamera",
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
    /// The surface the sessions run commands on: [`Surface::Automation`] for the protocol
    /// server, [`Surface::Mcp`] for the Model Context Protocol endpoint. It decides which
    /// commands `command.list`, `command.describe` and `command.run` reach.
    pub surface: Surface,
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
    /// limit, a clock standing at 0, no context units and the automation surface.
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
            surface: Surface::Automation,
        }
    }

    /// A command session of this configuration granted `permissions`.
    fn session(&self, permissions: BTreeSet<Permission>) -> Session {
        Session {
            surface: self.surface,
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
    /// What every session shares about each document.
    states: BTreeMap<String, Arc<DocState>>,
    config: ServerConfig,
    log: &'a dyn Log,
    recorder: Option<&'a Recorder>,
    renderer: Option<&'a dyn ViewRenderer>,
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

/// What a queued request does.
enum Work {
    /// A `command.run`: the invocation, and whether it is a preview.
    Run(Invocation, bool),
    /// A `history.undo` or `history.redo`, cancellable while it waits.
    Step(Direction, Cancel),
}

/// One queued `command.run`, `history.undo` or `history.redo`.
struct Job<'a> {
    id: Value,
    key: String,
    doc_id: String,
    document: &'a Document,
    state: Arc<DocState>,
    session: Session,
    work: Work,
}

/// The requests waiting or running, by id. A request whose entry is `None` can no longer be
/// cancelled: it is running and does not stop at a checkpoint (a history step).
type Pending = Mutex<BTreeMap<String, Option<Cancel>>>;

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

/// The most characters of a client's method name that reach the log.
const LOGGED_METHOD_CHARS: usize = 64;

/// A client's method name for the log: at most [`LOGGED_METHOD_CHARS`] characters, with control
/// characters replaced, so a client cannot fill the log or start new lines in it.
fn loggable_method(method: &str) -> String {
    method
        .chars()
        .take(LOGGED_METHOD_CHARS)
        .map(|c| {
            if c.is_control() {
                char::REPLACEMENT_CHARACTER
            } else {
                c
            }
        })
        .collect()
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

#[derive(Deserialize)]
struct DocumentParams {
    document: String,
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
        "preview": true,
        "notifications": ["event.notify"],
        "events": EVENTS,
    })
}

/// Queues the progress of one run for its session's `event.notify` notifications.
struct Progress {
    hub: Arc<Hub>,
    id: Value,
}

impl ProgressSink for Progress {
    fn progress(&self, fraction: f64, message_id: &str) {
        self.hub.progress(&self.id, fraction, message_id);
    }
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
            states: documents
                .keys()
                .map(|id| (id.clone(), Arc::default()))
                .collect(),
            config,
            log,
            recorder: None,
            renderer: None,
        }
    }

    /// Attaches the renderer of `view.render` and the MCP `render_view` tool. Without one both
    /// answer `cmd.render.unavailable`.
    #[must_use]
    pub fn with_view_renderer(mut self, renderer: &'a dyn ViewRenderer) -> Self {
        self.renderer = Some(renderer);
        self
    }

    /// Records every `command.run` that succeeds, of every session, into `recorder`
    /// (CORE-CMD-016). A step the recorder refuses is logged as `macro.record` and the run is
    /// answered as usual.
    #[must_use]
    pub fn with_recorder(mut self, recorder: &'a Recorder) -> Self {
        self.recorder = Some(recorder);
        self
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
    pub fn serve<R: BufRead, W: Write + Send>(&self, input: R, output: W) -> io::Result<()> {
        self.serve_with_ceiling(
            self.config.ceiling.clone(),
            input,
            output,
            &AtomicBool::new(false),
        )
    }

    /// [`serve`](Self::serve) with the session granted at most `ceiling` instead of the
    /// configured ceiling. The local socket uses it to give each client its own grant.
    ///
    /// Once `revoked` is set the session reads no further request, starts no queued run that has
    /// not begun, and ends; a running command is cancelled by the end of its reader.
    pub(super) fn serve_with_ceiling<R: BufRead, W: Write + Send>(
        &self,
        ceiling: BTreeSet<Permission>,
        mut input: R,
        mut output: W,
        revoked: &AtomicBool,
    ) -> io::Result<()> {
        let pending = Pending::default();
        let closed_flag = AtomicBool::new(false);
        let closed = &closed_flag;
        let stop_flag = AtomicBool::new(false);
        let stop = &stop_flag;
        // Bounded, so a client that stops reading slows the session down instead of growing it.
        let (out, lines) = sync_channel::<String>(OUTPUT_QUEUE);
        let (jobs, queue) = channel::<Job<'_>>();
        thread::scope(|scope| {
            let writer = thread::Builder::new()
                .name("arc-protocol-writer".into())
                .spawn_scoped(scope, move || write_lines(&lines, &mut output, closed))?;
            let hub = Arc::new(Hub::new(out.clone()));
            let worker_out = out.clone();
            let pending_ref = &pending;
            let worker_hub = Arc::clone(&hub);
            let worker = thread::Builder::new()
                .name("arc-protocol-worker".into())
                .spawn_scoped(scope, move || {
                    self.work(&queue, &worker_out, pending_ref, &worker_hub, revoked);
                });
            let pump_hub = Arc::clone(&hub);
            let pump = worker.is_ok().then(|| {
                thread::Builder::new()
                    .name("arc-protocol-events".into())
                    .spawn_scoped(scope, move || {
                        while !stop.load(Ordering::Acquire) {
                            if !pump_hub.pump(false) {
                                thread::park_timeout(EVENT_POLL);
                            }
                        }
                    })
            });
            let mut reader = Reader {
                server: self,
                out,
                hub: Arc::clone(&hub),
                jobs: Some(jobs),
                pending: &pending,
                closed,
                revoked,
                ceiling,
                session: None,
                current: None,
            };
            let result = match (worker, pump) {
                (Ok(worker), Some(Ok(pump))) => {
                    let result = reader.run(&mut input);
                    reader.finish();
                    let _ = worker.join();
                    // The notifications still queued precede the shutdown response.
                    hub.pump(true);
                    stop.store(true, Ordering::Release);
                    pump.thread().unpark();
                    let _ = pump.join();
                    reader.shutdown_reply();
                    result
                }
                (worker, pump) => {
                    stop.store(true, Ordering::Release);
                    reader.finish();
                    if let Ok(worker) = worker {
                        let _ = worker.join();
                    }
                    if let Some(Ok(pump)) = pump {
                        let _ = pump.join();
                    }
                    Err(io::Error::other("session threads"))
                }
            };
            drop(reader);
            drop(hub);
            let written = writer
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("writer fault")));
            result.and(written)
        })
    }

    fn work(
        &self,
        queue: &Receiver<Job<'_>>,
        out: &SyncSender<String>,
        pending: &Pending,
        hub: &Hub,
        revoked: &AtomicBool,
    ) {
        for job in queue {
            let Job {
                id,
                key,
                doc_id,
                document,
                state,
                session,
                work,
            } = job;
            let target = Target {
                id: &doc_id,
                document,
                state: &state,
            };
            // A request queued before the revocation never starts: it is answered as cancelled.
            let answer = if revoked.load(Ordering::Acquire) {
                Err(Error::new(codes::CANCELLED).into())
            } else {
                catch_unwind(AssertUnwindSafe(|| match work {
                    Work::Run(invocation, preview) => {
                        self.run_job(Some(hub), &target, &session, invocation, preview)
                    }
                    Work::Step(direction, cancel) => {
                        self.step_job(&target, &session, direction, &cancel, pending, &key)
                    }
                }))
                .unwrap_or_else(|payload| {
                    self.log.log(
                        LogLevel::Error,
                        "protocol.panic",
                        &panic_text(payload.as_ref()),
                    );
                    Err(Error::new(codes::PROTOCOL_INTERNAL).into())
                })
            };
            pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&key);
            // The events of the run, its progress among them, precede its response.
            hub.pump(true);
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
                    let (document, state) = self.document_state(&p.document)?;
                    let mut invocation = Invocation::new(p.command, p.params);
                    invocation.seed = p.seed;
                    invocation.expected_revision = p.expected_revision;
                    self.run_job(
                        None,
                        &Target {
                            id: &p.document,
                            document,
                            state: &state,
                        },
                        &session,
                        invocation,
                        p.preview,
                    )
                }),
                _ => self.query(&session, method, params, &AtomicBool::new(false)),
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
    fn query(
        &self,
        session: &Session,
        method: &str,
        params: Map<String, Value>,
        revoked: &AtomicBool,
    ) -> Answer {
        match method {
            "command.list" => decode::<ListParams>(method, params).and_then(|p| {
                let snapshot = p
                    .document
                    .map(|d| self.document(&d).map(Document::snapshot))
                    .transpose()?;
                let commands: Vec<Value> = self
                    .registry
                    .iter()
                    .filter(|spec| spec.exposure().contains(&self.config.surface))
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
            "query.objects" | "query.references" | "query.diagnostics" | "query.history" => {
                self.read_method(session, method, params)
            }
            "view.render" => self.view_render(session, params, revoked),
            _ => Err(Error::new(codes::PROTOCOL_METHOD_UNKNOWN)
                .with_arg("method", method)
                .into()),
        }
    }

    /// Runs `invocation` through the invoker, records it when `record` is set (a preview is
    /// not recorded) and logs the panic of a faulted command.
    fn invoke(
        &self,
        document: &Document,
        session: &Session,
        invocation: Invocation,
        record: bool,
    ) -> Result<Outcome, Failure> {
        // Runs are one at a time per session, so this is the revision the command runs on.
        let recorded = self
            .recorder
            .filter(|_| record)
            .map(|recorder| (recorder, invocation.clone(), document.snapshot().revision()));
        match Invoker::new(self.registry).invoke(document, session, invocation) {
            Ok(outcome) => {
                if let Some((recorder, invocation, revision)) = recorded
                    && let Err(error) = recorder.record(self.registry, &invocation, revision)
                {
                    self.log
                        .log(LogLevel::Warning, "macro.record", error.code());
                }
                Ok(outcome)
            }
            Err(error) => Err(self.failed(error)),
        }
    }

    /// Runs `invocation` as a dry run: nothing is recorded and the document is unchanged.
    fn dry_run(
        &self,
        document: &Document,
        session: &Session,
        invocation: Invocation,
    ) -> Result<Preview, Failure> {
        Invoker::new(self.registry)
            .dry_run(document, session, invocation)
            .map_err(|error| self.failed(error))
    }

    /// Logs the panic of a faulted command and turns `error` into a failure.
    fn failed(&self, error: Error) -> Failure {
        if let Some(panic) = error.details().get("panic") {
            self.log
                .log(LogLevel::Error, "command.fault", &format!("{panic:?}"));
        }
        Failure::from(error)
    }

    fn document_state(&self, id: &str) -> Result<(&'a Document, Arc<DocState>), Failure> {
        let document = self.document(id)?;
        let state = self.states.get(id).map(Arc::clone).unwrap_or_default();
        Ok((document, state))
    }

    fn document(&self, id: &str) -> Result<&'a Document, Failure> {
        self.documents.get(id).ok_or_else(|| {
            Error::new(codes::DOCUMENT_UNKNOWN)
                .with_arg("document", id)
                .into()
        })
    }

    /// The command named `name`, which must be exposed on the configured surface.
    fn command(&self, name: &str) -> Result<Arc<CommandSpec>, Failure> {
        let spec = self.registry.resolve(name, &BTreeMap::new())?.spec;
        if spec.exposure().contains(&self.config.surface) {
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
    hub: Arc<Hub>,
    jobs: Option<std::sync::mpsc::Sender<Job<'a>>>,
    pending: &'s Pending,
    closed: &'s AtomicBool,
    /// Set when the session's client is revoked: no further request is read.
    revoked: &'s AtomicBool,
    /// The most this session may be granted.
    ceiling: BTreeSet<Permission>,
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
        // Checked before each request: a request already buffered from the stream is not handled
        // once the session is revoked.
        while !self.closed.load(Ordering::Acquire) && !self.revoked.load(Ordering::Acquire) {
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
            self.server.log.log(
                LogLevel::Debug,
                "protocol.notification_ignored",
                &loggable_method(method),
            );
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
            "command.list" | "command.describe" | "command.validate" | "query.objects"
            | "query.references" | "query.diagnostics" | "query.history" | "view.render" => {
                server.query(&session, method, params, self.revoked)
            }
            "event.subscribe" | "event.unsubscribe" => self.hub.subscribe(
                server,
                &session.permissions,
                method,
                params,
                method == "event.subscribe",
            ),
            "selection.get" => self.hub.selection_get(server, &session.permissions, params),
            "selection.set" => {
                let answer = self.hub.selection_set(server, &session.permissions, params);
                // Like a run's events, the selection event precedes the answer.
                self.hub.pump(true);
                answer
            }
            "command.run" => match decode::<RunParams>(method, params) {
                Ok(p) => return self.run_command(id, session, p),
                Err(failure) => Err(failure),
            },
            "history.undo" | "history.redo" => {
                let direction = if method == "history.undo" {
                    Direction::Undo
                } else {
                    Direction::Redo
                };
                match decode::<DocumentParams>(method, params) {
                    Ok(p) => return self.history(id, session, p, direction),
                    Err(failure) => Err(failure),
                }
            }
            "command.cancel" => decode::<CancelParams>(method, params).map(|p| {
                let key = text(&p.id);
                let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                let cancelled = match pending.get(&key) {
                    Some(Some(cancel)) => {
                        cancel.cancel();
                        true
                    }
                    _ => false,
                };
                json!({"cancelled": cancelled})
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
            requested.intersection(&self.ceiling).copied().collect();
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
                "pageSize": query::max_page(&config.limits),
                "eventQueue": EVENT_QUEUE,
            },
        });
        self.session = Some(config.session(granted));
        Ok(result)
    }

    fn run_command(&mut self, id: &Value, session: Session, p: RunParams) -> Reply {
        let (document, state) = match self.server.document_state(&p.document) {
            Ok(found) => found,
            Err(failure) => return Reply::Now(Err(failure)),
        };
        let key = text(id);
        let cancel = Cancel::new();
        if let Err(failure) = self.reserve(&key, &cancel) {
            return Reply::Now(Err(failure));
        }
        let mut invocation = Invocation::new(p.command, p.params);
        invocation.seed = p.seed;
        invocation.cancel = cancel;
        invocation.expected_revision = p.expected_revision;
        invocation.progress = Some(Arc::new(Progress {
            hub: Arc::clone(&self.hub),
            id: id.clone(),
        }));
        let work = Work::Run(invocation, p.preview);
        self.enqueue(Job {
            id: id.clone(),
            key,
            doc_id: p.document,
            document,
            state,
            session,
            work,
        })
    }

    /// Queues `history.undo` or `history.redo` behind the requests of the session, so it steps
    /// over the change the session's earlier requests made. It is cancellable while it waits.
    fn history(
        &mut self,
        id: &Value,
        session: Session,
        p: DocumentParams,
        direction: Direction,
    ) -> Reply {
        let (document, state) = match self.server.document_state(&p.document) {
            Ok(found) => found,
            Err(failure) => return Reply::Now(Err(failure)),
        };
        let key = text(id);
        let cancel = Cancel::new();
        if let Err(failure) = self.reserve(&key, &cancel) {
            return Reply::Now(Err(failure));
        }
        self.enqueue(Job {
            id: id.clone(),
            key,
            doc_id: p.document,
            document,
            state,
            session,
            work: Work::Step(direction, cancel),
        })
    }

    /// Records a pending request under `key` with its cancellation token, or refuses it: a
    /// duplicate identifier, or the queue full.
    fn reserve(&self, key: &str, cancel: &Cancel) -> Result<(), Failure> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.contains_key(key) {
            return Err(invalid_request("duplicate_id"));
        }
        if pending.len() >= MAX_QUEUED_RUNS {
            return Err(limit("queued_runs"));
        }
        pending.insert(key.to_owned(), Some(cancel.clone()));
        Ok(())
    }

    /// Sends a reserved request to the worker; `Later` when it is queued.
    fn enqueue(&self, job: Job<'a>) -> Reply {
        let key = job.key.clone();
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
            .flatten()
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
    fn logged_method_names_are_bounded_and_plain() {
        let long = "m".repeat(200);
        assert_eq!(loggable_method(&long).chars().count(), LOGGED_METHOD_CHARS);
        assert_eq!(
            loggable_method("notifications/cancelled"),
            "notifications/cancelled"
        );
        let hostile = loggable_method("a\nb\u{7}\u{1b}[2J");
        assert!(!hostile.chars().any(char::is_control));
        assert_eq!(hostile.chars().count(), 8);
    }

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

    /// Input that yields `chunk` at once, then the chunks sent on `gate`. An empty chunk, or a
    /// dropped gate, is the end of the stream, so the reader never ends before the test says so.
    struct Gated {
        chunk: Vec<u8>,
        pos: usize,
        gate: std::sync::mpsc::Receiver<Vec<u8>>,
    }

    impl io::Read for Gated {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let available = self.fill_buf()?;
            let n = available.len().min(out.len());
            out[..n].copy_from_slice(&available[..n]);
            self.consume(n);
            Ok(n)
        }
    }

    impl BufRead for Gated {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.pos >= self.chunk.len() {
                self.chunk = self.gate.recv().unwrap_or_default();
                self.pos = 0;
            }
            Ok(&self.chunk[self.pos..])
        }

        fn consume(&mut self, amt: usize) {
            self.pos += amt;
        }
    }

    /// Output that sends every complete line to a channel.
    struct Lines {
        sent: std::sync::mpsc::Sender<String>,
        partial: Vec<u8>,
    }

    impl Write for Lines {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            for &byte in bytes {
                if byte == b'\n' {
                    let line = String::from_utf8_lossy(&self.partial).into_owned();
                    self.partial.clear();
                    let _ = self.sent.send(line);
                } else {
                    self.partial.push(byte);
                }
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The line answering request `id`, waiting for it.
    fn answer(lines: &std::sync::mpsc::Receiver<String>, id: u64) -> String {
        loop {
            let line = lines.recv().unwrap();
            if serde_json::from_str::<Value>(&line).unwrap()["id"] == json!(id) {
                return line;
            }
        }
    }

    /// A history step waiting in the queue is cancelled by its token; once it starts it is no
    /// longer cancellable, so its entry is `None` and `command.cancel` answers `cancelled: false`.
    #[test]
    fn a_started_history_step_is_no_longer_cancellable() {
        let registry = Registry::new();
        let document = Document::new(
            Arc::new(arc_doc::TypeRegistry::new()),
            Arc::new(arc_doc::graph::Evaluators::new()),
        )
        .unwrap();
        let docs = BTreeMap::from([("main".to_owned(), document)]);
        let config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read, Permission::Edit]);
        let server = Server::new(&registry, &docs, config, &NoLog);
        let session = server.config.session(server.config.ceiling.clone());
        let state = DocState::default();
        let target = Target {
            id: "main",
            document: &docs["main"],
            state: &state,
        };
        let pending: Pending = Mutex::new(BTreeMap::new());
        let token = Cancel::new();
        pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert("step".to_owned(), Some(token.clone()));

        // Cancelled while waiting: it does not step.
        token.cancel();
        let answer = server.step_job(&target, &session, Direction::Undo, &token, &pending, "step");
        assert!(answer.is_err());

        // Not cancelled: the step starts, and from then on the request has no cancel token.
        let token = Cancel::new();
        pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert("step".to_owned(), Some(token.clone()));
        let _ = server.step_job(&target, &session, Direction::Undo, &token, &pending, "step");
        let entry = pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get("step")
            .map(Option::is_none);
        assert_eq!(entry, Some(true));
    }

    #[test]
    fn a_revoked_session_starts_no_run_it_had_queued() {
        use std::sync::mpsc::{Sender, channel};

        #[derive(Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct Nothing {}

        // `test.item.block` (Read) announces its start and waits for its release; `test.item.mark`
        // (Edit) announces its start. Both change nothing.
        let (started_tx, started_rx) = channel::<()>();
        let (release_tx, release_rx) = channel::<()>();
        let (marked_tx, marked_rx) = channel::<()>();
        let started_tx = Mutex::new(started_tx);
        let release_rx = Mutex::new(release_rx);
        let marked_tx: Mutex<Sender<()>> = Mutex::new(marked_tx);
        let mut registry = Registry::new();
        registry
            .register(
                CommandSpec::new(
                    "test.item.block",
                    "test-block",
                    "test-block-description",
                    move |_ctx, _: Nothing| -> Result<String, arc_base::Error> {
                        let _ = started_tx.lock().unwrap().send(());
                        let _ = release_rx.lock().unwrap().recv();
                        Ok(String::from("released"))
                    },
                )
                .with_undo(crate::spec::UndoBehavior::NotUndoable)
                .with_permissions([Permission::Read]),
            )
            .unwrap();
        registry
            .register(
                CommandSpec::new(
                    "test.item.mark",
                    "test-mark",
                    "test-mark-description",
                    move |_ctx, _: Nothing| -> Result<String, arc_base::Error> {
                        let _ = marked_tx.lock().unwrap().send(());
                        Ok(String::from("marked"))
                    },
                )
                .with_undo(crate::spec::UndoBehavior::NotUndoable)
                .with_permissions([Permission::Edit]),
            )
            .unwrap();
        let document = Document::new(
            Arc::new(arc_doc::TypeRegistry::new()),
            Arc::new(arc_doc::graph::Evaluators::new()),
        )
        .unwrap();
        let docs = BTreeMap::from([("main".to_owned(), document)]);
        let config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read, Permission::Edit]);
        let server = Server::new(&registry, &docs, config, &NoLog);
        let run = |id: u64, command: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"command.run","params":{{"document":"main","command":"{command}","params":{{}}}}}}"#
            )
        };
        // The handshake, the blocking run, the mark, and a ping: the ping is answered by the
        // reader itself, so its answer proves that both runs were queued before the revocation.
        let chunk = format!(
            "{}\n{}\n{}\n{}\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"arc.initialize","params":{"clientName":"t","protocolVersions":{"min":"1.0","max":"1.0"},"permissions":["read","edit"]}}"#,
            run(2, "test.item.block"),
            run(3, "test.item.mark"),
            r#"{"jsonrpc":"2.0","id":4,"method":"arc.ping","params":{}}"#,
        );
        let (gate_tx, gate_rx) = channel::<Vec<u8>>();
        let input = Gated {
            chunk: chunk.into_bytes(),
            pos: 0,
            gate: gate_rx,
        };
        let (sent_tx, sent_rx) = channel::<String>();
        let output = Lines {
            sent: sent_tx,
            partial: Vec::new(),
        };
        let revoked = AtomicBool::new(false);
        let ceiling = BTreeSet::from([Permission::Read, Permission::Edit]);
        std::thread::scope(|scope| {
            let served =
                scope.spawn(|| server.serve_with_ceiling(ceiling, input, output, &revoked));
            started_rx.recv().unwrap();
            answer(&sent_rx, 4);
            revoked.store(true, Ordering::Release);
            release_tx.send(()).unwrap();
            // Answers come in order, so the answer of the mark follows whatever it did.
            answer(&sent_rx, 3);
            gate_tx.send(Vec::new()).unwrap();
            served.join().unwrap().unwrap();
        });
        assert!(
            marked_rx.try_recv().is_err(),
            "a run queued before the revocation started"
        );
    }
}
