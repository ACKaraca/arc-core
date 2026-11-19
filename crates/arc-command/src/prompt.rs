//! Prompt machines (CORE-CMD-011).
//!
//! A prompt machine is a deterministic state machine owned by a command. Its state is the list
//! of answers collected so far; given that list (and the parameters typed inline), it yields the
//! next [`Prompt`] or the complete parameters ([`Step`]). A [`PromptSession`] drives one machine:
//! it validates every answer against the prompt's accepted inputs and the machine's own check,
//! keeps the answers for "back", and ends in an [`Invocation`] that runs through
//! [`crate::Invoker::invoke`] like every programmatic call, so a prompt can never bypass
//! validation, permissions or predicates.
//!
//! Simple commands describe their prompts declaratively with a [`PromptSequence`] (one step per
//! parameter, defaults taken from the parameter schema); others implement [`PromptMachine`].
//! A command without a machine gets one derived from its schema
//! ([`PromptSequence::from_schema`]).

use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::str::FromStr;
use std::sync::Arc;

use arc_base::{Dimension, Error, ObjectId, Quantity, Result};
use arc_doc::Snapshot;
use serde_json::{Map, Value};

use crate::context::Session;
use crate::error::codes;
use crate::invoke::Invocation;
use crate::line::{self, Kind};
use crate::spec::CommandSpec;

mod coordinates;

pub use coordinates::{MAX_POINT_BYTES, PlaneTransform, PointEntry};

/// The most answers one prompt session collects.
pub const MAX_ANSWERS: usize = 1024;

/// The longest keyword option identifier, in bytes.
pub const MAX_KEYWORD_BYTES: usize = 64;

/// A point in model coordinates, in meters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// The x coordinate.
    pub x: f64,
    /// The y coordinate.
    pub y: f64,
    /// The z coordinate of a 3D point.
    pub z: Option<f64>,
}

/// Which points a point prompt accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointDims {
    /// 2D points only.
    Two,
    /// 3D points only.
    Three,
    /// 2D and 3D points.
    Any,
}

/// One option of a keyword prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct KeywordOption {
    /// The identifier, typed on the command line and returned in the answer.
    pub id: String,
    /// The label message identifier.
    pub label: String,
    /// The shortcut letter, typed instead of the identifier (compared ignoring ASCII case).
    pub shortcut: Option<char>,
    /// The parameter value the option stands for; the identifier as a JSON string by default.
    pub value: Value,
}

impl KeywordOption {
    /// An option `id` with the label message identifier `label`, no shortcut, and `id` as value.
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            value: Value::String(id.clone()),
            id,
            label: label.into(),
            shortcut: None,
        }
    }

    /// Sets the shortcut letter.
    #[must_use]
    pub fn with_shortcut(mut self, shortcut: char) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Sets the parameter value.
    #[must_use]
    pub fn with_value(mut self, value: Value) -> Self {
        self.value = value;
        self
    }
}

/// An input kind a prompt accepts, with its constraints. Bounds are inclusive and in SI base
/// units (meters, radians).
#[derive(Debug, Clone, PartialEq)]
pub enum Accept {
    /// A point in model coordinates.
    Point {
        /// 2D, 3D or both.
        dims: PointDims,
    },
    /// A distance in meters.
    Distance {
        /// The smallest distance.
        min: Option<f64>,
        /// The largest distance.
        max: Option<f64>,
    },
    /// An angle in radians.
    Angle {
        /// The smallest angle.
        min: Option<f64>,
        /// The largest angle.
        max: Option<f64>,
    },
    /// One live object, optionally of the listed type keys.
    Pick {
        /// The accepted type keys; empty accepts every type.
        types: Vec<String>,
    },
    /// A set of distinct live objects.
    Selection {
        /// The fewest objects.
        min: usize,
        /// The most objects.
        max: Option<usize>,
        /// The accepted type keys; empty accepts every type.
        types: Vec<String>,
    },
    /// A text.
    Text {
        /// The fewest Unicode scalar values.
        min_chars: usize,
        /// The most Unicode scalar values.
        max_chars: Option<usize>,
    },
    /// A number with units of a stated dimension.
    Number {
        /// The dimension; [`Dimension::NONE`] for plain numbers.
        dimension: Dimension,
        /// The smallest value in SI base units.
        min: Option<f64>,
        /// The largest value in SI base units.
        max: Option<f64>,
        /// Whether only whole numbers are accepted.
        integer: bool,
    },
    /// An expression with units kept as text and evaluated by the command, for quantities whose
    /// dimension the prompt does not know.
    Expression,
    /// One of a closed list of keyword options.
    Keyword {
        /// The options.
        options: Vec<KeywordOption>,
    },
}

impl Accept {
    /// A distance without bounds.
    #[must_use]
    pub fn distance() -> Self {
        Self::Distance {
            min: None,
            max: None,
        }
    }

    /// An angle without bounds.
    #[must_use]
    pub fn angle() -> Self {
        Self::Angle {
            min: None,
            max: None,
        }
    }

    /// A number of `dimension` without bounds.
    #[must_use]
    pub fn number(dimension: Dimension) -> Self {
        Self::Number {
            dimension,
            min: None,
            max: None,
            integer: false,
        }
    }

    /// A text of any length.
    #[must_use]
    pub fn text() -> Self {
        Self::Text {
            min_chars: 0,
            max_chars: None,
        }
    }

    /// Keyword options.
    #[must_use]
    pub fn keywords(options: impl IntoIterator<Item = KeywordOption>) -> Self {
        Self::Keyword {
            options: options.into_iter().collect(),
        }
    }
}

/// A message identifier with arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The Fluent message identifier.
    pub id: String,
    /// The arguments by name.
    pub args: BTreeMap<String, String>,
}

/// What a prompt asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    /// The key of the answer, unique among the prompts of one path (a parameter name for
    /// declarative machines).
    pub key: String,
    /// The prompt message.
    pub message: Message,
    /// The accepted inputs; never empty.
    pub accepts: Vec<Accept>,
    /// The answer Enter gives.
    pub default: Option<PromptInput>,
}

impl Prompt {
    /// A prompt `key` with the message identifier `message_id` accepting `accept`.
    #[must_use]
    pub fn new(key: impl Into<String>, message_id: impl Into<String>, accept: Accept) -> Self {
        Self {
            key: key.into(),
            message: Message {
                id: message_id.into(),
                args: BTreeMap::new(),
            },
            accepts: vec![accept],
            default: None,
        }
    }

    /// Accepts `accept` as well.
    #[must_use]
    pub fn or(mut self, accept: Accept) -> Self {
        self.accepts.push(accept);
        self
    }

    /// Sets the default answer.
    #[must_use]
    pub fn with_default(mut self, default: PromptInput) -> Self {
        self.default = Some(default);
        self
    }

    /// Adds a message argument.
    #[must_use]
    pub fn with_arg(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.message.args.insert(name.into(), value.into());
        self
    }

    /// Every keyword option of the prompt.
    pub fn keyword_options(&self) -> impl Iterator<Item = &KeywordOption> + '_ {
        self.accepts.iter().flat_map(|accept| match accept {
            Accept::Keyword { options } => options.as_slice(),
            _ => &[],
        })
    }
}

/// A typed answer, produced by the surface: the UI from pointer events and dynamic input, the
/// command line from typed text ([`PromptSession::answer_text`]).
#[derive(Debug, Clone, PartialEq)]
pub enum PromptInput {
    /// A point in meters.
    Point(Point),
    /// A distance in meters.
    Distance(f64),
    /// An angle in radians.
    Angle(f64),
    /// One object.
    Pick(ObjectId),
    /// A set of objects.
    Selection(Vec<ObjectId>),
    /// A text.
    Text(String),
    /// A number with units.
    Number(Quantity),
    /// An expression with units, evaluated by the command.
    Expression(String),
    /// A keyword option identifier.
    Keyword(String),
}

/// One collected answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The key of the prompt it answers.
    pub key: String,
    /// The input.
    pub input: PromptInput,
}

/// What a machine reads: the parameters typed inline, the answers so far and the current
/// document state.
#[derive(Debug, Clone, Copy)]
pub struct PromptState<'a> {
    /// The parameters given before prompting (inline on the command line).
    pub preset: &'a Map<String, Value>,
    /// The answers, oldest first.
    pub answers: &'a [Answer],
    /// The current document state; machines never change it.
    pub snapshot: &'a Snapshot,
    /// The last point: the latest accepted point answer, else the point the host set. Typed
    /// relative and direct forms measure from it; a host draws a rubber band from it.
    pub base: Option<Point>,
    /// The direction direct distances use, a unit vector in the plane: a direction locked by
    /// `<angle`, else the host's pointer direction.
    pub direction: Option<[f64; 2]>,
}

impl PromptState<'_> {
    /// The latest answer to the prompt `key`.
    #[must_use]
    pub fn answer(&self, key: &str) -> Option<&PromptInput> {
        self.answers
            .iter()
            .rev()
            .find(|a| a.key == key)
            .map(|a| &a.input)
    }
}

/// The next step of a machine.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Ask this prompt.
    Ask(Prompt),
    /// The parameters are complete. They are merged over the inline parameters.
    Done(Map<String, Value>),
}

/// The interactive state machine of a command (CORE-CMD-011).
///
/// The machine is a pure function of its [`PromptState`]: the same inline parameters and
/// answers give the same step. "Back" removes the last answer, so the machine needs no state of
/// its own. Keyword options change the path by being answers the machine reads.
pub trait PromptMachine: Send + Sync + fmt::Debug {
    /// The next prompt, or the complete parameters.
    ///
    /// # Errors
    ///
    /// Any error ends the prompt session.
    fn next(&self, state: &PromptState<'_>) -> Result<Step>;

    /// Checks an answer that already fits the prompt's accepted inputs; an error rejects it and
    /// the session stays at the prompt. The default accepts.
    ///
    /// # Errors
    ///
    /// The rejection, with a message identifier.
    fn check(&self, state: &PromptState<'_>, prompt: &Prompt, input: &PromptInput) -> Result<()> {
        let _ = (state, prompt, input);
        Ok(())
    }

    /// A preview of the pending result as data for the viewport (transient geometry in the
    /// product's terms), from the answers so far and the pointer's current input. This layer
    /// draws nothing; the default has no preview.
    fn preview(&self, state: &PromptState<'_>, pointer: Option<&PromptInput>) -> Option<Value> {
        let _ = (state, pointer);
        None
    }
}

type StepCheck = Arc<dyn Fn(&Snapshot, &PromptInput) -> Result<()> + Send + Sync>;

/// One step of a [`PromptSequence`]: the parameter it fills, its prompt and an optional check.
#[derive(Clone)]
pub struct PromptStep {
    prompt: Prompt,
    check: Option<StepCheck>,
}

impl fmt::Debug for PromptStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptStep")
            .field("prompt", &self.prompt)
            .finish_non_exhaustive()
    }
}

impl PromptStep {
    /// A step for the parameter `param` with the message identifier `message_id`, accepting
    /// `accept`.
    #[must_use]
    pub fn new(param: impl Into<String>, message_id: impl Into<String>, accept: Accept) -> Self {
        Self {
            prompt: Prompt::new(param, message_id, accept),
            check: None,
        }
    }

    /// Accepts `accept` as well.
    #[must_use]
    pub fn or(mut self, accept: Accept) -> Self {
        self.prompt = self.prompt.or(accept);
        self
    }

    /// Sets the default answer, replacing the one taken from the schema.
    #[must_use]
    pub fn with_default(mut self, default: PromptInput) -> Self {
        self.prompt.default = Some(default);
        self
    }

    /// Adds a predicate on answers (for example on the picked object's fields); an error
    /// rejects the answer.
    #[must_use]
    pub fn with_check(
        mut self,
        check: impl Fn(&Snapshot, &PromptInput) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.check = Some(Arc::new(check));
        self
    }

    /// The prompt.
    #[must_use]
    pub fn prompt(&self) -> &Prompt {
        &self.prompt
    }
}

/// A declarative prompt machine: one step per parameter, asked in order, skipping parameters
/// given inline. An answer becomes the parameter's JSON value ([`input_to_json`]).
#[derive(Debug, Clone, Default)]
pub struct PromptSequence {
    steps: Vec<PromptStep>,
}

impl PromptSequence {
    /// An empty sequence.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a step.
    #[must_use]
    pub fn step(mut self, step: PromptStep) -> Self {
        self.steps.push(step);
        self
    }

    /// The steps.
    #[must_use]
    pub fn steps(&self) -> &[PromptStep] {
        &self.steps
    }

    /// Gives every step without a default the parameter's schema `default`, when one of the
    /// step's accepted inputs can represent it ([`input_from_json`]); others stay without.
    #[must_use]
    pub fn with_schema_defaults(mut self, schema: &Value) -> Self {
        for step in &mut self.steps {
            if step.prompt.default.is_some() {
                continue;
            }
            let default = schema
                .get("properties")
                .and_then(|p| p.get(&step.prompt.key))
                .and_then(|p| p.get("default"));
            if let Some(value) = default {
                step.prompt.default = input_from_json(&step.prompt.accepts, value);
            }
        }
        self
    }

    /// The machine of a command without one: a step for each required parameter, in the order
    /// of `required`, accepting what the parameter's schema kind maps to (see the
    /// specification), with the message `cmd-prompt-param` and the argument `name`.
    #[must_use]
    pub fn from_schema(schema: &Value) -> Self {
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let steps = required
            .iter()
            .filter_map(Value::as_str)
            .map(|name| {
                let kind = line::param_kind(schema, name);
                let prompt =
                    Prompt::new(name, PARAM.message_id, accept_for(&kind)).with_arg("name", name);
                PromptStep {
                    prompt,
                    check: None,
                }
            })
            .collect();
        Self { steps }.with_schema_defaults(schema)
    }

    fn open_steps<'s>(
        &'s self,
        preset: &'s Map<String, Value>,
    ) -> impl Iterator<Item = &'s PromptStep> {
        self.steps
            .iter()
            .filter(move |s| !preset.contains_key(&s.prompt.key))
    }
}

/// A message identifier of this module, written so the catalog gate finds it.
struct Msg {
    message_id: &'static str,
}

/// The prompt of a derived step; argument `name`.
const PARAM: Msg = Msg {
    message_id: "cmd-prompt-param",
};
const YES: Msg = Msg {
    message_id: "cmd-keyword-yes",
};
const NO: Msg = Msg {
    message_id: "cmd-keyword-no",
};
/// The label of a derived keyword option; the surface passes the option identifier as `value`.
const VALUE: Msg = Msg {
    message_id: "cmd-keyword-value",
};

fn yes_no() -> Accept {
    Accept::keywords([
        KeywordOption::new("true", YES.message_id)
            .with_shortcut('y')
            .with_value(Value::Bool(true)),
        KeywordOption::new("false", NO.message_id)
            .with_shortcut('n')
            .with_value(Value::Bool(false)),
    ])
}

fn accept_for(kind: &Kind) -> Accept {
    match kind {
        Kind::Quantity => Accept::Expression,
        Kind::Number => Accept::number(Dimension::NONE),
        Kind::Integer => Accept::Number {
            dimension: Dimension::NONE,
            min: None,
            max: None,
            integer: true,
        },
        Kind::Boolean => yes_no(),
        Kind::Id => Accept::Pick { types: Vec::new() },
        Kind::Enum(values) => Accept::keywords(values.iter().map(|v| {
            let id = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
            KeywordOption::new(id, VALUE.message_id).with_value(v.clone())
        })),
        Kind::Array(item) if **item == Kind::Id => Accept::Selection {
            min: 0,
            max: None,
            types: Vec::new(),
        },
        Kind::Array(item) if matches!(**item, Kind::Number | Kind::Quantity) => Accept::Point {
            dims: PointDims::Any,
        },
        Kind::Text | Kind::Array(_) | Kind::Json => Accept::text(),
    }
}

/// The JSON value of an answer: a point as `[x, y]` or `[x, y, z]`, distances, angles and
/// numbers in SI base units (a whole number for an integer prompt), an object as its identifier
/// text, a selection as an array of identifier texts, a text and an expression as strings, a
/// keyword as its option's value.
#[must_use]
pub fn input_to_json(accepts: &[Accept], input: &PromptInput) -> Value {
    match input {
        PromptInput::Point(p) => {
            let mut out = vec![Value::from(p.x), Value::from(p.y)];
            out.extend(p.z.map(Value::from));
            Value::Array(out)
        }
        PromptInput::Distance(v) | PromptInput::Angle(v) => Value::from(*v),
        PromptInput::Pick(id) => Value::String(id.to_string()),
        PromptInput::Selection(ids) => {
            Value::Array(ids.iter().map(|id| Value::String(id.to_string())).collect())
        }
        PromptInput::Text(text) | PromptInput::Expression(text) => Value::String(text.clone()),
        PromptInput::Number(q) => {
            let integer = accepts.iter().any(|a| {
                matches!(a, Accept::Number { dimension, integer: true, .. } if *dimension == q.dimension())
            });
            match whole(q.value()) {
                Some(n) if integer => Value::from(n),
                _ => Value::from(q.value()),
            }
        }
        PromptInput::Keyword(id) => accepts
            .iter()
            .flat_map(|a| match a {
                Accept::Keyword { options } => options.as_slice(),
                _ => &[],
            })
            .find(|o| o.id == *id)
            .map_or_else(|| Value::String(id.clone()), |o| o.value.clone()),
    }
}

/// The largest magnitude below which every whole `f64` is exact.
const EXACT_INTEGER: f64 = 9_007_199_254_740_992.0;

/// `value` as an `i64` when it is a whole number of exact magnitude.
fn whole(value: f64) -> Option<i64> {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the value is whole and below 2^53"
    )]
    (value.fract() == 0.0 && value.abs() <= EXACT_INTEGER).then_some(value as i64)
}

/// The answer that represents `value` under the first of `accepts` that can, the inverse of
/// [`input_to_json`]; `None` when none can.
#[must_use]
pub fn input_from_json(accepts: &[Accept], value: &Value) -> Option<PromptInput> {
    accepts.iter().find_map(|accept| match (accept, value) {
        (Accept::Keyword { options }, _) => options
            .iter()
            .find(|o| o.value == *value)
            .map(|o| PromptInput::Keyword(o.id.clone())),
        (Accept::Point { .. }, Value::Array(items)) => {
            let n: Option<Vec<f64>> = items.iter().map(Value::as_f64).collect();
            match n.as_deref() {
                Some(&[x, y]) => Some(PromptInput::Point(Point { x, y, z: None })),
                Some(&[x, y, z]) => Some(PromptInput::Point(Point { x, y, z: Some(z) })),
                _ => None,
            }
        }
        (Accept::Distance { .. }, Value::Number(n)) => n.as_f64().map(PromptInput::Distance),
        (Accept::Angle { .. }, Value::Number(n)) => n.as_f64().map(PromptInput::Angle),
        (Accept::Number { dimension, .. }, Value::Number(n)) => n
            .as_f64()
            .map(|v| PromptInput::Number(Quantity::new(v, *dimension))),
        (Accept::Expression, Value::String(s)) => Some(PromptInput::Expression(s.clone())),
        (Accept::Text { .. }, Value::String(s)) => Some(PromptInput::Text(s.clone())),
        (Accept::Pick { .. }, Value::String(s)) => {
            ObjectId::from_str(s).ok().map(PromptInput::Pick)
        }
        (Accept::Selection { .. }, Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().and_then(|s| ObjectId::from_str(s).ok()))
            .collect::<Option<Vec<_>>>()
            .map(PromptInput::Selection),
        _ => None,
    })
}

impl PromptMachine for PromptSequence {
    fn next(&self, state: &PromptState<'_>) -> Result<Step> {
        let mut params = Map::new();
        let mut answers = state.answers.iter();
        for step in self.open_steps(state.preset) {
            match answers.next() {
                Some(answer) => {
                    params.insert(
                        step.prompt.key.clone(),
                        input_to_json(&step.prompt.accepts, &answer.input),
                    );
                }
                None => return Ok(Step::Ask(step.prompt.clone())),
            }
        }
        Ok(Step::Done(params))
    }

    fn check(&self, state: &PromptState<'_>, prompt: &Prompt, input: &PromptInput) -> Result<()> {
        self.steps
            .iter()
            .find(|s| s.prompt.key == prompt.key)
            .and_then(|s| s.check.as_ref())
            .map_or(Ok(()), |check| check(state.snapshot, input))
    }
}

/// The result of driving a prompt session one step.
#[derive(Debug)]
pub enum Feedback {
    /// The session asks this prompt.
    Ask(Prompt),
    /// The answer was rejected; the session stays at the prompt.
    Rejected {
        /// Why, with a message identifier.
        error: Error,
        /// The prompt still asked.
        prompt: Prompt,
    },
    /// The parameters are complete: run this invocation through [`crate::Invoker::invoke`].
    Done(Invocation),
    /// The session was cancelled.
    Cancelled,
    /// The machine failed or misbehaved; the session is over.
    Failed(Error),
}

fn range_error(reason: &'static str) -> Error {
    Error::new(codes::PROMPT_RANGE).with_detail("reason", reason)
}

fn within(value: f64, min: Option<f64>, max: Option<f64>) -> Result<()> {
    if !value.is_finite() {
        return Err(range_error("not_finite"));
    }
    if min.is_some_and(|m| value < m) {
        return Err(range_error("minimum"));
    }
    if max.is_some_and(|m| value > m) {
        return Err(range_error("maximum"));
    }
    Ok(())
}

fn live_of_type(snapshot: &Snapshot, id: ObjectId, types: &[String]) -> Result<()> {
    let handle = snapshot.resolve(id).map_err(|_| {
        Error::new(codes::PROMPT_OBJECT)
            .with_detail("reason", "not_found")
            .with_detail("id", id.to_string())
    })?;
    let fits = types.is_empty()
        || snapshot
            .type_key(handle)
            .is_some_and(|key| types.iter().any(|t| t == key.as_str()));
    if fits {
        Ok(())
    } else {
        Err(Error::new(codes::PROMPT_OBJECT)
            .with_detail("reason", "type")
            .with_detail("id", id.to_string()))
    }
}

/// Checks `input` against one accepted kind: `None` when the kinds differ.
fn admit(
    accept: &Accept,
    input: &PromptInput,
    snapshot: &Snapshot,
    session: &Session,
) -> Option<Result<()>> {
    let limits = &session.limits;
    let too_long = |bytes: usize| u64::try_from(bytes).unwrap_or(u64::MAX) > limits.string_bytes;
    Some(match (accept, input) {
        (Accept::Point { dims }, PromptInput::Point(p)) => {
            let fits = match dims {
                PointDims::Two => p.z.is_none(),
                PointDims::Three => p.z.is_some(),
                PointDims::Any => true,
            };
            if fits {
                [p.x, p.y, p.z.unwrap_or(0.0)]
                    .into_iter()
                    .try_for_each(|c| within(c, None, None))
            } else {
                Err(range_error("dimensions"))
            }
        }
        (Accept::Distance { min, max }, PromptInput::Distance(v))
        | (Accept::Angle { min, max }, PromptInput::Angle(v)) => within(*v, *min, *max),
        (Accept::Pick { types }, PromptInput::Pick(id)) => live_of_type(snapshot, *id, types),
        (Accept::Selection { min, max, types }, PromptInput::Selection(ids)) => {
            let mut sorted = ids.clone();
            sorted.sort_unstable();
            sorted.dedup();
            if u64::try_from(ids.len()).unwrap_or(u64::MAX) > limits.array_length {
                Err(Error::new(codes::PROMPT_LIMIT_EXCEEDED).with_detail("limit", "array_length"))
            } else if sorted.len() != ids.len() {
                Err(range_error("duplicate"))
            } else if ids.len() < *min || max.is_some_and(|m| ids.len() > m) {
                Err(range_error("count"))
            } else {
                ids.iter()
                    .try_for_each(|id| live_of_type(snapshot, *id, types))
            }
        }
        (
            Accept::Text {
                min_chars,
                max_chars,
            },
            PromptInput::Text(text),
        ) => {
            let chars = text.chars().count();
            if too_long(text.len()) {
                Err(Error::new(codes::PROMPT_LIMIT_EXCEEDED).with_detail("limit", "string_bytes"))
            } else if chars < *min_chars || max_chars.is_some_and(|m| chars > m) {
                Err(range_error("length"))
            } else {
                Ok(())
            }
        }
        (
            Accept::Number {
                dimension,
                min,
                max,
                integer,
            },
            PromptInput::Number(q),
        ) => {
            if q.dimension() != *dimension {
                return None;
            }
            within(q.value(), *min, *max).and_then(|()| {
                if *integer && whole(q.value()).is_none() {
                    Err(range_error("integer"))
                } else {
                    Ok(())
                }
            })
        }
        (Accept::Expression, PromptInput::Expression(text)) => {
            if too_long(text.len()) {
                Err(Error::new(codes::PROMPT_LIMIT_EXCEEDED).with_detail("limit", "string_bytes"))
            } else {
                arc_solve::Expr::parse(text, &arc_solve::ExprLimits::DEFAULT).map(drop)
            }
        }
        (Accept::Keyword { options }, PromptInput::Keyword(id)) => {
            if options.iter().any(|o| o.id == *id) {
                Ok(())
            } else {
                Err(Error::new(codes::PROMPT_KIND).with_detail("reason", "keyword"))
            }
        }
        _ => return None,
    })
}

/// Checks a prompt a machine produced: at least one accepted kind, keyword identifiers that are
/// unique, non-empty and at most [`MAX_KEYWORD_BYTES`] long, unique shortcut letters, and a
/// default of an accepted kind.
fn prompt_problem(prompt: &Prompt) -> Option<&'static str> {
    if prompt.accepts.is_empty() {
        return Some("accepts_nothing");
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut shortcuts = std::collections::BTreeSet::new();
    for option in prompt.keyword_options() {
        if option.id.is_empty()
            || option.id.len() > MAX_KEYWORD_BYTES
            || option.id.chars().any(char::is_whitespace)
            || !ids.insert(option.id.to_ascii_lowercase())
        {
            return Some("keyword_id");
        }
        if let Some(c) = option.shortcut
            && !shortcuts.insert(c.to_ascii_lowercase())
        {
            return Some("keyword_shortcut");
        }
    }
    None
}

/// One command's prompt session: the inline parameters, the collected answers and the current
/// prompt. It is transient UI state and is not serialized. Every method contains panics of the
/// machine (`cmd.fault`).
pub struct PromptSession {
    spec: Arc<CommandSpec>,
    machine: Arc<dyn PromptMachine>,
    preset: Map<String, Value>,
    answers: Vec<Answer>,
    current: Option<Prompt>,
    session: Session,
    entry: PointEntry,
}

impl fmt::Debug for PromptSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptSession")
            .field("command", &self.spec.id())
            .field("answers", &self.answers)
            .field("current", &self.current)
            .finish_non_exhaustive()
    }
}

fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Err(Error::new(codes::FAULT).with_detail("panic", message))
    })
}

impl PromptSession {
    /// Starts prompting for `spec` with the inline parameters `preset`, using the command's
    /// machine or, without one, [`PromptSequence::from_schema`]. The first feedback is the first
    /// prompt, or `Done` when nothing is missing.
    pub fn start(
        spec: Arc<CommandSpec>,
        preset: Map<String, Value>,
        session: &Session,
        snapshot: &Snapshot,
    ) -> (Self, Feedback) {
        Self::start_with_entry(spec, preset, session, snapshot, PointEntry::new())
    }

    /// Like [`PromptSession::start`], with the point state `entry` (the last point, the host's
    /// direction and the typed-input settings, usually [`PromptSession::carry`] of the previous
    /// session). A locked direction does not carry over and is released.
    pub fn start_with_entry(
        spec: Arc<CommandSpec>,
        preset: Map<String, Value>,
        session: &Session,
        snapshot: &Snapshot,
        mut entry: PointEntry,
    ) -> (Self, Feedback) {
        entry.unlock();
        let machine = spec.prompt().cloned().unwrap_or_else(|| {
            Arc::new(PromptSequence::from_schema(spec.params_schema())) as Arc<dyn PromptMachine>
        });
        let mut this = Self {
            spec,
            machine,
            preset,
            answers: Vec::new(),
            current: None,
            session: session.clone(),
            entry,
        };
        let feedback = this.advance(snapshot, None);
        (this, feedback)
    }

    /// The command.
    #[must_use]
    pub fn command(&self) -> &Arc<CommandSpec> {
        &self.spec
    }

    /// The current prompt; `None` once the session is over.
    #[must_use]
    pub fn prompt(&self) -> Option<&Prompt> {
        self.current.as_ref()
    }

    /// The answers, oldest first.
    #[must_use]
    pub fn answers(&self) -> &[Answer] {
        &self.answers
    }

    /// The inline parameters.
    #[must_use]
    pub fn preset(&self) -> &Map<String, Value> {
        &self.preset
    }

    /// The point state: the host's settings, direction and lock.
    #[must_use]
    pub fn point_entry(&self) -> &PointEntry {
        &self.entry
    }

    /// The point state, for the host to set the direction, the defaults or the transform before
    /// answering. The last point the host sets here is used until a point is answered.
    pub fn point_entry_mut(&mut self) -> &mut PointEntry {
        &mut self.entry
    }

    /// The last point: the latest accepted point answer of this session, else the point the host
    /// set on the entry.
    #[must_use]
    pub fn last_point(&self) -> Option<Point> {
        self.answers
            .iter()
            .rev()
            .find_map(|a| match &a.input {
                PromptInput::Point(p) => Some(*p),
                _ => None,
            })
            .or_else(|| self.entry.last_point())
    }

    /// The point state a following session starts with (see
    /// [`PromptSession::start_with_entry`]): this session's settings and direction, its last
    /// point, and no locked direction.
    #[must_use]
    pub fn carry(&self) -> PointEntry {
        let mut entry = self.entry.clone().with_last_point(self.last_point());
        entry.unlock();
        entry
    }

    fn state<'s>(&'s self, snapshot: &'s Snapshot) -> PromptState<'s> {
        PromptState {
            preset: &self.preset,
            answers: &self.answers,
            snapshot,
            base: self.last_point(),
            direction: self.entry.effective_direction(),
        }
    }

    fn fail(&mut self, error: Error) -> Feedback {
        self.current = None;
        Feedback::Failed(error)
    }

    /// Asks the machine for the next step. `rejected` is the error to report with the prompt.
    fn advance(&mut self, snapshot: &Snapshot, mut rejected: Option<Error>) -> Feedback {
        loop {
            let machine = Arc::clone(&self.machine);
            let state = self.state(snapshot);
            let step = match guarded(|| machine.next(&state)) {
                Ok(step) => step,
                Err(error) => return self.fail(error),
            };
            match step {
                Step::Ask(prompt) => {
                    if let Some(reason) = prompt_problem(&prompt) {
                        let error = Error::new(codes::PROMPT_INVALID).with_detail("reason", reason);
                        return self.fail(error);
                    }
                    self.current = Some(prompt.clone());
                    return match rejected {
                        Some(error) => Feedback::Rejected { error, prompt },
                        None => Feedback::Ask(prompt),
                    };
                }
                Step::Done(params) => {
                    let mut all = self.preset.clone();
                    all.extend(params);
                    let all = Value::Object(all);
                    let error = match self.spec.validate(&all, &self.session.limits) {
                        Ok(violations) if violations.is_empty() => {
                            self.current = None;
                            return Feedback::Done(Invocation::new(self.spec.id(), all));
                        }
                        Ok(violations) => crate::schema::invalid(&violations),
                        Err(error) => error,
                    };
                    // The last answer completed invalid parameters: reject it and ask again.
                    if self.answers.pop().is_none() {
                        return self.fail(error);
                    }
                    rejected = Some(error);
                }
            }
        }
    }

    fn rejected(&self, error: Error) -> Feedback {
        match &self.current {
            Some(prompt) => Feedback::Rejected {
                error,
                prompt: prompt.clone(),
            },
            None => Feedback::Failed(error),
        }
    }

    /// Answers the current prompt. The input must fit one of the prompt's accepted kinds and
    /// its constraints (`cmd.prompt.kind`, `cmd.prompt.range`, `cmd.prompt.object`,
    /// `cmd.prompt.limit_exceeded`, or the expression parser's error) and pass the machine's
    /// check; a rejected input leaves the session at the prompt.
    pub fn answer(&mut self, input: PromptInput, snapshot: &Snapshot) -> Feedback {
        let Some(prompt) = self.current.clone() else {
            return Feedback::Failed(Error::new(codes::PROMPT_ENDED));
        };
        if self.answers.len() >= MAX_ANSWERS {
            return self.rejected(
                Error::new(codes::PROMPT_LIMIT_EXCEEDED).with_detail("limit", "answers"),
            );
        }
        let mut first_error = None;
        let mut fits = false;
        for accept in &prompt.accepts {
            match admit(accept, &input, snapshot, &self.session) {
                Some(Ok(())) => {
                    fits = true;
                    break;
                }
                Some(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                None => {}
            }
        }
        if !fits {
            let error = first_error
                .unwrap_or_else(|| Error::new(codes::PROMPT_KIND).with_detail("reason", "kind"));
            return self.rejected(error);
        }
        let machine = Arc::clone(&self.machine);
        let state = self.state(snapshot);
        if let Err(error) = guarded(|| machine.check(&state, &prompt, &input)) {
            return self.rejected(error);
        }
        if matches!(input, PromptInput::Point(_)) {
            // An accepted point ends a direction lock: the next distance measures from it.
            self.entry.unlock();
        }
        self.answers.push(Answer {
            key: prompt.key,
            input,
        });
        self.advance(snapshot, None)
    }

    /// Answers with the prompt's default (Enter); `cmd.prompt.no_default` when it has none.
    pub fn accept_default(&mut self, snapshot: &Snapshot) -> Feedback {
        match self.current.as_ref().map(|p| p.default.clone()) {
            None => Feedback::Failed(Error::new(codes::PROMPT_ENDED)),
            Some(None) => self.rejected(Error::new(codes::PROMPT_NO_DEFAULT)),
            Some(Some(default)) => self.answer(default, snapshot),
        }
    }

    /// Answers with typed text, read as the prompt's accepted kinds (see the specification):
    /// empty text accepts the default; keyword options (identifier or shortcut letter) are tried
    /// first, then the accepted kinds in order. Text that no kind reads is
    /// `cmd.prompt.unreadable`; text a kind reads but rejects reports that kind's error.
    pub fn answer_text(&mut self, text: &str, snapshot: &Snapshot) -> Feedback {
        let Some(prompt) = self.current.clone() else {
            return Feedback::Failed(Error::new(codes::PROMPT_ENDED));
        };
        let text = text.trim();
        if text.is_empty() {
            return self.accept_default(snapshot);
        }
        if text.len() > line::MAX_LINE_BYTES {
            return self.rejected(
                Error::new(codes::PROMPT_LIMIT_EXCEEDED).with_detail("limit", "line_bytes"),
            );
        }
        if let Some(option) = keyword_of(&prompt, text) {
            return self.answer(PromptInput::Keyword(option), snapshot);
        }
        let mut first_error = None;
        let last = self.last_point();
        for accept in &prompt.accepts {
            let read_result = match accept {
                Accept::Point { .. } => {
                    match coordinates::read(text, &self.session, &self.entry, last) {
                        // `<angle` locks a direction and keeps the prompt.
                        Some(Ok(coordinates::Read::Lock(direction))) => {
                            self.entry.lock(direction);
                            return Feedback::Ask(prompt);
                        }
                        Some(Ok(coordinates::Read::Point(point))) => {
                            Some(Ok(PromptInput::Point(point)))
                        }
                        Some(Err(error)) => Some(Err(error)),
                        None => None,
                    }
                }
                _ => read(accept, text, &self.session),
            };
            match read_result {
                Some(Ok(input)) => match admit(accept, &input, snapshot, &self.session) {
                    Some(Ok(())) => return self.answer(input, snapshot),
                    Some(Err(error)) => {
                        first_error.get_or_insert(error);
                    }
                    None => {}
                },
                Some(Err(error)) => {
                    first_error.get_or_insert(error);
                }
                None => {}
            }
        }
        self.rejected(first_error.unwrap_or_else(|| Error::new(codes::PROMPT_UNREADABLE)))
    }

    /// Removes the last answer and asks its prompt again; at the first prompt it asks it again.
    pub fn back(&mut self, snapshot: &Snapshot) -> Feedback {
        if self.current.is_none() {
            return Feedback::Failed(Error::new(codes::PROMPT_ENDED));
        }
        self.answers.pop();
        self.advance(snapshot, None)
    }

    /// Cancels the session.
    pub fn cancel(&mut self) -> Feedback {
        self.current = None;
        Feedback::Cancelled
    }

    /// The machine's preview for the answers so far and the pointer's input; `None` without a
    /// preview, after the session ended, or when the machine panics.
    #[must_use]
    pub fn preview(&self, snapshot: &Snapshot, pointer: Option<&PromptInput>) -> Option<Value> {
        self.current.as_ref()?;
        let state = self.state(snapshot);
        guarded(|| Ok(self.machine.preview(&state, pointer)))
            .ok()
            .flatten()
    }

    /// The keyword options of the current prompt whose identifier starts with `partial`
    /// (ignoring ASCII case), in prompt order.
    #[must_use]
    pub fn complete(&self, partial: &str) -> Vec<&KeywordOption> {
        let partial = partial.trim().to_ascii_lowercase();
        self.current
            .iter()
            .flat_map(Prompt::keyword_options)
            .filter(|o| o.id.to_ascii_lowercase().starts_with(&partial))
            .collect()
    }
}

/// The keyword option `text` names by identifier or shortcut letter, ignoring ASCII case.
fn keyword_of(prompt: &Prompt, text: &str) -> Option<String> {
    let mut chars = text.chars();
    let single = match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c.to_ascii_lowercase()),
        _ => None,
    };
    prompt
        .keyword_options()
        .find(|o| {
            o.id.eq_ignore_ascii_case(text)
                || (single.is_some() && o.shortcut.map(|c| c.to_ascii_lowercase()) == single)
        })
        .map(|o| o.id.clone())
}

fn quantity(text: &str, session: &Session, dimension: Dimension) -> Result<Quantity> {
    let mut context = arc_solve::Context::new();
    if let Some(unit) = session.units.iter().find(|u| u.dimension == dimension) {
        context = context.with_bare_unit(*unit);
    }
    arc_solve::parse_quantity(text, &context, '.', dimension)
}

/// Reads typed text as one accepted kind: `None` when the text does not have that kind's form.
fn read(accept: &Accept, text: &str, session: &Session) -> Option<Result<PromptInput>> {
    Some(match accept {
        // Points are read by `coordinates::read`, which needs the session's point entry.
        Accept::Point { .. } => return None,
        Accept::Distance { .. } => coordinates::length(text, session).map(PromptInput::Distance),
        Accept::Angle { .. } => coordinates::angle(text, session).map(PromptInput::Angle),
        Accept::Number { dimension, .. } => {
            quantity(text, session, *dimension).map(PromptInput::Number)
        }
        Accept::Expression => Ok(PromptInput::Expression(text.to_owned())),
        Accept::Pick { .. } => ObjectId::from_str(text)
            .ok()
            .map(PromptInput::Pick)
            .map(Ok)?,
        Accept::Selection { .. } => text
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .map(|s| ObjectId::from_str(s).ok())
            .collect::<Option<Vec<_>>>()
            .map(|ids| Ok(PromptInput::Selection(ids)))?,
        Accept::Text { .. } => Ok(PromptInput::Text(text.to_owned())),
        Accept::Keyword { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

    use std::sync::Arc;

    use arc_doc::TypeRegistry;
    use serde_json::json;

    use super::*;
    use crate::spec::Surface;

    #[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Params {
        #[serde(default = "five")]
        count: u32,
        flag: bool,
        mode: Mode,
    }

    fn five() -> u32 {
        5
    }

    #[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "snake_case")]
    enum Mode {
        Fast,
        Slow,
    }

    fn spec() -> CommandSpec {
        CommandSpec::new("test.unit.run", "l", "d", |_, _: Params| Ok(()))
    }

    fn snapshot() -> Snapshot {
        Snapshot::empty(Arc::new(TypeRegistry::new()))
    }

    fn session() -> Session {
        Session::new(Surface::CommandLine, [])
    }

    #[test]
    fn declarative_steps_take_defaults_from_the_schema() {
        let spec = spec().with_prompts(
            PromptSequence::new()
                .step(PromptStep::new(
                    "count",
                    "m",
                    Accept::Number {
                        dimension: Dimension::NONE,
                        min: None,
                        max: None,
                        integer: true,
                    },
                ))
                .step(PromptStep::new("flag", "m", yes_no())),
        );
        let snapshot = snapshot();
        let mut preset = Map::new();
        preset.insert("mode".into(), json!("slow"));
        let (mut s, first) = PromptSession::start(Arc::new(spec), preset, &session(), &snapshot);
        let Feedback::Ask(prompt) = first else {
            panic!("{first:?}")
        };
        assert_eq!(
            prompt.default,
            Some(PromptInput::Number(Quantity::new(5.0, Dimension::NONE)))
        );
        assert!(matches!(s.answer_text("", &snapshot), Feedback::Ask(_)));
        let Feedback::Done(invocation) = s.answer_text("Y", &snapshot) else {
            panic!()
        };
        assert_eq!(
            invocation.params,
            json!({"count": 5, "flag": true, "mode": "slow"})
        );
    }

    #[test]
    fn the_derived_machine_follows_the_schema_kinds() {
        let sequence = PromptSequence::from_schema(spec().params_schema());
        let accepts: Vec<&Accept> = sequence
            .steps()
            .iter()
            .map(|s| &s.prompt().accepts[0])
            .collect();
        assert_eq!(accepts.len(), 2, "only required parameters");
        assert_eq!(*accepts[0], yes_no());
        let Accept::Keyword { options } = accepts[1] else {
            panic!()
        };
        let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["fast", "slow"]);
    }

    #[derive(Debug)]
    struct Broken(u8);

    impl PromptMachine for Broken {
        fn next(&self, _: &PromptState<'_>) -> Result<Step> {
            match self.0 {
                0 => panic!("machine fault"),
                1 => Ok(Step::Ask(Prompt {
                    key: "k".into(),
                    message: Message {
                        id: "m".into(),
                        args: BTreeMap::new(),
                    },
                    accepts: Vec::new(),
                    default: None,
                })),
                _ => Ok(Step::Ask(
                    Prompt::new("k", "m", Accept::keywords([KeywordOption::new("a", "m")]))
                        .or(Accept::keywords([KeywordOption::new("A", "m")])),
                )),
            }
        }
    }

    #[test]
    fn faulty_machines_end_the_session() {
        for (n, code) in [
            (0, "cmd.fault"),
            (1, "cmd.prompt.invalid"),
            (2, "cmd.prompt.invalid"),
        ] {
            let spec = spec().with_prompt(Arc::new(Broken(n)));
            let (mut s, first) =
                PromptSession::start(Arc::new(spec), Map::new(), &session(), &snapshot());
            let Feedback::Failed(error) = first else {
                panic!("{first:?}")
            };
            assert_eq!(error.code(), code);
            assert!(s.prompt().is_none());
            assert!(matches!(
                s.answer_text("x", &snapshot()),
                Feedback::Failed(e) if e.code() == "cmd.prompt.ended"
            ));
        }
    }

    #[test]
    fn answers_convert_to_json_and_back() {
        let id = ObjectId::from_str("0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c").unwrap();
        let cases = [
            (
                Accept::Point {
                    dims: PointDims::Any,
                },
                PromptInput::Point(Point {
                    x: 1.0,
                    y: 2.0,
                    z: Some(3.0),
                }),
            ),
            (Accept::distance(), PromptInput::Distance(2.5)),
            (Accept::angle(), PromptInput::Angle(-1.0)),
            (Accept::Pick { types: Vec::new() }, PromptInput::Pick(id)),
            (
                Accept::Selection {
                    min: 0,
                    max: None,
                    types: Vec::new(),
                },
                PromptInput::Selection(vec![id]),
            ),
            (Accept::text(), PromptInput::Text("t".into())),
            (
                Accept::number(Dimension::LENGTH),
                PromptInput::Number(Quantity::new(0.25, Dimension::LENGTH)),
            ),
            (Accept::Expression, PromptInput::Expression("1 m".into())),
            (yes_no(), PromptInput::Keyword("false".into())),
        ];
        for (accept, input) in cases {
            let accepts = [accept];
            let value = input_to_json(&accepts, &input);
            assert_eq!(input_from_json(&accepts, &value), Some(input));
        }
    }
}
