//! Commands of the Arc suite.
//!
//! The parts that exist:
//!
//! - the command specification with identifier, labels, schemas, examples, enabled predicate,
//!   `run`, the prompt machine, transparency, undo behavior, permission classes and
//!   exposure (CORE-CMD-001, [`spec`]);
//! - parameter and result schemas generated from Rust types, bounded parameter parsing and
//!   validation with JSON-pointer violations (CORE-CMD-002, [`schema`]);
//! - the session, the command context with seeded identifiers, clock, cancellation and progress,
//!   and enabled predicates (CORE-CMD-003, CORE-CMD-005, CORE-CMD-008, [`context`]);
//! - guarded execution through `arc-doc` transactions with permission checks (CORE-CMD-004,
//!   CORE-CMD-007, [`invoke`]);
//! - the registry with deprecated and user aliases, name resolution and the command catalog
//!   (CORE-CMD-009, CORE-CMD-010, [`registry`]);
//! - prompt machines with typed inputs, defaults, keyword options, back, cancel and previews
//!   (CORE-CMD-011, [`prompt`]);
//! - command-line parsing with inline positional and named parameters, the canonical line form
//!   and completion data (CORE-CMD-013, [`line`]);
//! - the command-line driver that continues with prompts and repeats the last command
//!   (CORE-CMD-013, CORE-CMD-015, [`command_line`]);
//! - the Arc Automation Protocol server: framing, session negotiation, the command methods and
//!   the error model (CORE-CMD-019, CORE-CMD-021, CORE-CMD-022, CORE-CMD-027, [`protocol`]);
//! - read and write roots with the check bound to the opened handle (CORE-CMD-032, [`roots`]);
//! - macro recording, reading with hostile-input bounds and playback as one undo group
//!   (CORE-CMD-016, CORE-CMD-018, [`macros`]);
//! - the headless runner with the shared CLI subcommands, flags and exit codes (CORE-CMD-031,
//!   `headless`, feature `headless`).
//!
//! A command is a typed Rust function over its parameters and a [`Context`]:
//!
//! ```
//! use std::sync::Arc;
//! use arc_command::{CommandSpec, Invocation, Invoker, Permission, Registry, Session, Surface};
//! use arc_doc::{Document, TypeRegistry, graph::Evaluators};
//!
//! #[derive(serde::Deserialize, schemars::JsonSchema)]
//! #[serde(deny_unknown_fields)]
//! struct Params { n: u32 }
//!
//! let spec = CommandSpec::new("demo.math.double", "demo-double", "demo-double-description",
//!     |_context, params: Params| Ok(u64::from(params.n) * 2))
//!     .with_undo(arc_command::UndoBehavior::NotUndoable)
//!     .with_permissions([Permission::Read])
//!     .with_example(serde_json::json!({"n": 2}));
//! let mut registry = Registry::new();
//! registry.register(spec).unwrap();
//!
//! let document = Document::new(Arc::new(TypeRegistry::new()), Arc::new(Evaluators::new())).unwrap();
//! let session = Session::new(Surface::Automation, [Permission::Read]);
//! let outcome = Invoker::new(&registry)
//!     .invoke(&document, &session, Invocation::new("demo.math.double", serde_json::json!({"n": 21})))
//!     .unwrap();
//! assert_eq!(outcome.value, 42);
//! ```
//!
//! No function in this crate panics.

pub mod command_line;
pub mod context;
pub mod error;
#[cfg(feature = "headless")]
pub mod headless;
pub mod invoke;
pub mod line;
pub mod macros;
pub mod prompt;
pub mod protocol;
pub mod registry;
pub mod roots;
pub mod schema;
pub mod spec;

pub use command_line::{CommandLine, LastCommand, LineStep, Runner, Submitted, Target};
pub use context::{
    Clock, Context, FixedClock, IdParam, PROGRESS_DELAY_MILLIS, Probe, ProgressSink, QuantityParam,
    Session,
};
pub use invoke::{Invocation, Invoker, Outcome, Preview};
pub use line::{
    Candidate, CandidateKind, Completion, ParsedLine, Span, complete, format_line, parse_line,
};
pub use macros::{Macro, MacroHeader, MacroStep, Recorder, parse_macro, play};
pub use prompt::{
    Accept, Answer, Feedback, KeywordOption, Message, Point, PointDims, Prompt, PromptInput,
    PromptMachine, PromptSequence, PromptSession, PromptState, PromptStep, Step,
};
pub use protocol::{Log, LogLevel, NoLog, Server, ServerConfig};
pub use registry::{CATALOG_FORMAT, CATALOG_VERSION, Registry, Resolved, Via};
pub use roots::Roots;
pub use schema::{Violation, ViolationKind, parse_params};
pub use spec::{CommandSpec, Disabled, Permission, Predicate, Surface, UndoBehavior};
