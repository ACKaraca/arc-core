//! Commands of the Arc suite.
//!
//! The parts that exist:
//!
//! - the command specification with identifier, labels, schemas, examples, enabled predicate,
//!   `run`, the prompt machine slot, transparency, undo behavior, permission classes and
//!   exposure (CORE-CMD-001, [`spec`]);
//! - parameter and result schemas generated from Rust types, bounded parameter parsing and
//!   validation with JSON-pointer violations (CORE-CMD-002, [`schema`]);
//! - the session, the command context with seeded identifiers, clock, cancellation and progress,
//!   and enabled predicates (CORE-CMD-003, CORE-CMD-005, CORE-CMD-008, [`context`]);
//! - guarded execution through `arc-doc` transactions with permission checks (CORE-CMD-004,
//!   CORE-CMD-007, [`invoke`]);
//! - the registry with deprecated and user aliases, name resolution and the command catalog
//!   (CORE-CMD-009, CORE-CMD-010, [`registry`]).
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

pub mod context;
pub mod error;
pub mod invoke;
pub mod registry;
pub mod schema;
pub mod spec;

pub use context::{
    Clock, Context, FixedClock, IdParam, PROGRESS_DELAY_MILLIS, Probe, ProgressSink, QuantityParam,
    Session,
};
pub use invoke::{Invocation, Invoker, Outcome};
pub use registry::{CATALOG_FORMAT, CATALOG_VERSION, Registry, Resolved, Via};
pub use schema::{Violation, ViolationKind, parse_params};
pub use spec::{
    CommandSpec, Disabled, Permission, Predicate, PromptMachine, Surface, UndoBehavior,
};
