//! The command specification (CORE-CMD-001): identifier, labels, schemas, examples, enabled
//! predicate, `run`, the prompt machine, the positional order, transparency, undo behavior, permission classes and
//! exposure.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use arc_base::{Error, Limits, Result};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::{Context, Probe};
use crate::error::codes;
use crate::prompt::{PromptMachine, PromptSequence};
use crate::schema::{self, Violation, ViolationKind};

/// The longest command identifier, in bytes.
pub const MAX_ID_BYTES: usize = 128;

/// A surface on which a command can appear and be called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// Menus, toolbars, the palette and key bindings.
    Ui,
    /// The command line.
    CommandLine,
    /// The Arc Automation Protocol, scripts and headless CLIs.
    Automation,
    /// The Model Context Protocol endpoint.
    Mcp,
    /// WebAssembly plug-ins.
    Plugin,
}

impl Surface {
    /// Every surface, in declaration order.
    pub const ALL: [Self; 5] = [
        Self::Ui,
        Self::CommandLine,
        Self::Automation,
        Self::Mcp,
        Self::Plugin,
    ];
}

/// A permission class a command needs and a session or surface is granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Read documents.
    Read,
    /// Change documents.
    Edit,
    /// Read files.
    FileRead,
    /// Write files.
    FileWrite,
    /// Use the network.
    Network,
    /// Start processes.
    Process,
    /// Change settings.
    Settings,
}

/// How a command appears in undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndoBehavior {
    /// The command changes the document in a transaction that forms, joins or is coalesced into
    /// an undo step.
    Undoable,
    /// The command does not change the document (view, query, export) and opens no transaction.
    NotUndoable,
    /// Undo, redo and group control. In this unit such commands run like `not_undoable` ones,
    /// on a snapshot without a transaction; their own commit path comes with the core commands.
    UndoControl,
}

/// Why a command is disabled: a message identifier with arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Disabled {
    /// The Fluent message identifier of the reason.
    pub message_id: &'static str,
    /// The message arguments by name.
    pub arguments: BTreeMap<&'static str, arc_base::error::Value>,
}

impl Disabled {
    /// A reason without arguments.
    #[must_use]
    pub fn new(message_id: &'static str) -> Self {
        Self {
            message_id,
            arguments: BTreeMap::new(),
        }
    }

    /// Adds an argument.
    #[must_use]
    pub fn with_arg(
        mut self,
        name: &'static str,
        value: impl Into<arc_base::error::Value>,
    ) -> Self {
        self.arguments.insert(name, value.into());
        self
    }
}

/// An enabled predicate: enabled, or disabled with a reason.
pub type Predicate = Arc<dyn Fn(&Probe<'_>) -> std::result::Result<(), Disabled> + Send + Sync>;

type RunFn = Arc<dyn Fn(&mut Context<'_>, Value) -> Result<Value> + Send + Sync>;

/// A registered command.
///
/// Build one with [`CommandSpec::new`] from a typed `run` function and the `with_*` setters;
/// the registry checks it at registration ([`CommandSpec::check`]). `run` is reachable only
/// through [`crate::Invoker::invoke`].
#[derive(Clone)]
pub struct CommandSpec {
    id: String,
    label: String,
    description: String,
    category: Option<String>,
    keywords: Option<String>,
    params: Value,
    result: Value,
    examples: Vec<Value>,
    enabled: Option<Predicate>,
    run: RunFn,
    prompt: Option<Arc<dyn PromptMachine>>,
    positional: Vec<String>,
    transparent: bool,
    undo: UndoBehavior,
    coalesce: Option<String>,
    permissions: BTreeSet<Permission>,
    exposure: BTreeSet<Surface>,
}

impl fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandSpec")
            .field("id", &self.id)
            .field("undo", &self.undo)
            .field("permissions", &self.permissions)
            .field("exposure", &self.exposure)
            .field("positional", &self.positional)
            .finish_non_exhaustive()
    }
}

/// True when `text` is a Fluent message identifier.
fn is_message_id(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_alphabetic())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True when `text` is a command identifier: three or more names (a lowercase ASCII letter
/// followed by lowercase letters, digits and underscores) separated by single dots,
/// `<owner>.<area>.<verb>`, at most [`MAX_ID_BYTES`] long.
#[must_use]
pub fn is_command_id(text: &str) -> bool {
    text.len() <= MAX_ID_BYTES && arc_doc::schema::is_key(text) && text.split('.').count() >= 3
}

fn invalid(reason: &'static str) -> Error {
    Error::new(codes::SPEC_INVALID).with_detail("reason", reason)
}

impl CommandSpec {
    /// A command `id` with the label and description message identifiers and a typed `run`.
    /// The parameter and result schemas are generated from `P` and `R`.
    ///
    /// Defaults: undoable, permission class `edit`, exposed on every surface, always enabled,
    /// not transparent, no examples, no prompt machine, and the required parameters in
    /// declaration order as the positional order.
    pub fn new<P, R, F>(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        run: F,
    ) -> Self
    where
        P: DeserializeOwned + JsonSchema,
        R: Serialize + JsonSchema,
        F: Fn(&mut Context<'_>, P) -> Result<R> + Send + Sync + 'static,
    {
        let run: RunFn = Arc::new(move |context: &mut Context<'_>, params: Value| {
            let params: P = serde_json::from_value(params).map_err(|_| {
                schema::invalid(&[Violation {
                    pointer: String::new(),
                    kind: ViolationKind::Decode,
                }])
            })?;
            let value = run(context, params)?;
            serde_json::to_value(value).map_err(|_| Error::new(codes::RESULT_INVALID))
        });
        let params = schema::schema_for::<P>();
        let positional = params
            .get("required")
            .and_then(Value::as_array)
            .map(|r| {
                r.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            category: None,
            keywords: None,
            params,
            result: schema::schema_for::<R>(),
            examples: Vec::new(),
            enabled: None,
            run,
            prompt: None,
            positional,
            transparent: false,
            undo: UndoBehavior::Undoable,
            coalesce: None,
            permissions: BTreeSet::from([Permission::Edit]),
            exposure: Surface::ALL.into_iter().collect(),
        }
    }

    /// Sets the category message identifier.
    #[must_use]
    pub fn with_category(mut self, message_id: impl Into<String>) -> Self {
        self.category = Some(message_id.into());
        self
    }

    /// Sets the message identifier of the localized keywords.
    #[must_use]
    pub fn with_keywords(mut self, message_id: impl Into<String>) -> Self {
        self.keywords = Some(message_id.into());
        self
    }

    /// Adds an example parameter set; it must validate against the parameter schema.
    #[must_use]
    pub fn with_example(mut self, params: Value) -> Self {
        self.examples.push(params);
        self
    }

    /// Sets the enabled predicate.
    #[must_use]
    pub fn with_enabled(
        mut self,
        predicate: impl Fn(&Probe<'_>) -> std::result::Result<(), Disabled> + Send + Sync + 'static,
    ) -> Self {
        self.enabled = Some(Arc::new(predicate));
        self
    }

    /// Sets the prompt machine.
    #[must_use]
    pub fn with_prompt(mut self, machine: Arc<dyn PromptMachine>) -> Self {
        self.prompt = Some(machine);
        self
    }

    /// Sets a declarative prompt machine; steps without a default take the parameter's schema
    /// `default` ([`PromptSequence::with_schema_defaults`]).
    #[must_use]
    pub fn with_prompts(mut self, sequence: PromptSequence) -> Self {
        self.prompt = Some(Arc::new(sequence.with_schema_defaults(&self.params)));
        self
    }

    /// Replaces the positional order: the parameters that inline values without a name fill,
    /// in order. The default is the required parameters in declaration order; optional
    /// parameters are named-only unless listed here.
    #[must_use]
    pub fn with_positional<S: Into<String>>(mut self, names: impl IntoIterator<Item = S>) -> Self {
        self.positional = names.into_iter().map(Into::into).collect();
        self
    }

    /// The positional order of inline parameters.
    #[must_use]
    pub fn positional(&self) -> &[String] {
        &self.positional
    }

    /// Marks the command transparent.
    #[must_use]
    pub fn transparent(mut self) -> Self {
        self.transparent = true;
        self
    }

    /// Sets the undo behavior; anything but `undoable` drops the coalesce key.
    #[must_use]
    pub fn with_undo(mut self, undo: UndoBehavior) -> Self {
        self.undo = undo;
        if undo != UndoBehavior::Undoable {
            self.coalesce = None;
        }
        self
    }

    /// Makes the command undoable with a coalesce key: within a gesture, consecutive
    /// invocations with the same key form one undo step.
    #[must_use]
    pub fn coalesced(mut self, key: impl Into<String>) -> Self {
        self.undo = UndoBehavior::Undoable;
        self.coalesce = Some(key.into());
        self
    }

    /// Replaces the permission classes.
    #[must_use]
    pub fn with_permissions(mut self, classes: impl IntoIterator<Item = Permission>) -> Self {
        self.permissions = classes.into_iter().collect();
        self
    }

    /// Replaces the surfaces; an empty set hides the command from every surface and the catalog.
    #[must_use]
    pub fn with_exposure(mut self, surfaces: impl IntoIterator<Item = Surface>) -> Self {
        self.exposure = surfaces.into_iter().collect();
        self
    }

    /// The identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The label message identifier.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The description message identifier.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The category message identifier.
    #[must_use]
    pub fn category(&self) -> Option<&str> {
        self.category.as_deref()
    }

    /// The keywords message identifier.
    #[must_use]
    pub fn keywords(&self) -> Option<&str> {
        self.keywords.as_deref()
    }

    /// The parameter schema.
    #[must_use]
    pub fn params_schema(&self) -> &Value {
        &self.params
    }

    /// The result schema.
    #[must_use]
    pub fn result_schema(&self) -> &Value {
        &self.result
    }

    /// The example parameter sets.
    #[must_use]
    pub fn examples(&self) -> &[Value] {
        &self.examples
    }

    /// The prompt machine.
    #[must_use]
    pub fn prompt(&self) -> Option<&Arc<dyn PromptMachine>> {
        self.prompt.as_ref()
    }

    /// Whether the command is transparent.
    #[must_use]
    pub fn is_transparent(&self) -> bool {
        self.transparent
    }

    /// The undo behavior.
    #[must_use]
    pub fn undo(&self) -> UndoBehavior {
        self.undo
    }

    /// The coalesce key of an undoable command.
    #[must_use]
    pub fn coalesce(&self) -> Option<&str> {
        self.coalesce.as_deref()
    }

    /// The permission classes.
    #[must_use]
    pub fn permissions(&self) -> &BTreeSet<Permission> {
        &self.permissions
    }

    /// The surfaces.
    #[must_use]
    pub fn exposure(&self) -> &BTreeSet<Surface> {
        &self.exposure
    }

    /// Checks `params` against [`Limits`] and the parameter schema; returns the violations.
    ///
    /// # Errors
    ///
    /// `cmd.params.limit_exceeded`.
    pub fn validate(&self, params: &Value, limits: &Limits) -> Result<Vec<Violation>> {
        schema::check_limits(params, limits)?;
        schema::validate(&self.params, params)
    }

    /// Checks every example against the parameter schema.
    ///
    /// # Errors
    ///
    /// `cmd.spec.invalid` with the details `example` (its index) and `violations`.
    pub fn validate_examples(&self) -> Result<()> {
        for (index, example) in self.examples.iter().enumerate() {
            let violations = self.validate(example, &Limits::DEFAULT)?;
            if !violations.is_empty() {
                return Err(invalid("example_invalid")
                    .with_detail("example", i64::try_from(index).unwrap_or(i64::MAX))
                    .with_detail(
                        "violations",
                        serde_json::to_string(&violations).unwrap_or_default(),
                    ));
            }
        }
        Ok(())
    }

    /// The registration check: the identifier grammar ([`is_command_id`]), Fluent message
    /// identifiers for label, description, category and keywords, at least one permission
    /// class, schemas within the supported subset ([`schema::check_schema`]), a parameter schema
    /// that is an object rejecting unknown properties (`additionalProperties: false`), and
    /// valid examples, and a positional order of distinct parameter names.
    ///
    /// # Errors
    ///
    /// `cmd.spec.invalid` with the detail `reason`.
    pub fn check(&self) -> Result<()> {
        if !is_command_id(&self.id) {
            return Err(invalid("malformed_id"));
        }
        let messages = [Some(&self.label), Some(&self.description)]
            .into_iter()
            .chain([self.category.as_ref(), self.keywords.as_ref()]);
        if !messages.flatten().all(|m| is_message_id(m)) {
            return Err(invalid("malformed_message_id"));
        }
        if self.permissions.is_empty() {
            return Err(invalid("no_permission_class"));
        }
        schema::check_schema(&self.params)?;
        schema::check_schema(&self.result)?;
        let object = self.params.get("type").and_then(Value::as_str) == Some("object")
            && self.params.get("additionalProperties") == Some(&Value::Bool(false));
        if !object {
            return Err(invalid("params_not_closed_object"));
        }
        let properties = self.params.get("properties").and_then(Value::as_object);
        let mut seen = BTreeSet::new();
        let positional_ok = self.positional.iter().all(|name| {
            seen.insert(name.as_str()) && properties.is_some_and(|p| p.contains_key(name))
        });
        if !positional_ok {
            return Err(invalid("positional_invalid"));
        }
        self.validate_examples()
    }

    /// The enabled predicate's verdict.
    pub(crate) fn enabled(&self, probe: &Probe<'_>) -> std::result::Result<(), Disabled> {
        self.enabled.as_ref().map_or(Ok(()), |p| p(probe))
    }

    /// Decodes `params`, runs the command and encodes and checks its result.
    pub(crate) fn call(&self, context: &mut Context<'_>, params: Value) -> Result<Value> {
        let value = (self.run)(context, params)?;
        let violations = schema::validate(&self.result, &value)?;
        if violations.is_empty() {
            Ok(value)
        } else {
            Err(Error::new(codes::RESULT_INVALID).with_detail(
                "violations",
                serde_json::to_string(&violations).unwrap_or_default(),
            ))
        }
    }
}
