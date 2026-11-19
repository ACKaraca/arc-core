//! Macro recording, inputs and playback (CORE-CMD-016, CORE-CMD-017, CORE-CMD-018).
//!
//! A macro is JSON Lines text: a header line, then one line per completed invocation holding
//! the [canonical command line](crate::line::format_line), the invocation seed, the revision the
//! command ran on and the selection. A [`Recorder`] collects the invocations a session completes
//! (failed and cancelled ones never reach it); [`parse_macro`] reads a macro, or a plain list
//! of command lines, with hostile-input bounds; [`play`] runs it through [`Invoker::invoke`]
//! as one undo group, stopping at the first failure and rolling the group back. The format is
//! specified in `docs/crates/arc-command.md`, section "Macros and batch as implemented".

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};
use arc_doc::Document;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::context::{REVISION_MIX, Session, split_mix};
use crate::error::codes;
use crate::invoke::{Invocation, Invoker, Outcome};
use crate::line::{format_line, parse_line};
use crate::registry::Registry;

/// The `format` of a macro header.
pub const MACRO_FORMAT: &str = "arc-macro";
/// The macro format version this crate writes and reads.
pub const MACRO_VERSION: u32 = 2;
/// The oldest macro format version this crate still reads.
pub const MACRO_MIN_VERSION: u32 = 1;
/// The most inputs one step declares.
pub const MAX_STEP_INPUTS: usize = 64;

/// The product a macro was recorded with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroProduct {
    /// The product name.
    pub name: String,
    /// The product version.
    pub version: String,
}

/// The first line of a macro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroHeader {
    /// Always [`MACRO_FORMAT`].
    pub format: String,
    /// The format version, [`MACRO_MIN_VERSION`] to [`MACRO_VERSION`].
    pub version: u32,
    /// The automation protocol version of the recording session's crate.
    pub protocol: String,
    /// The recording product.
    pub product: MacroProduct,
}

/// One invocation of a macro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MacroStep {
    /// The command line, in canonical form when recorded.
    pub line: String,
    /// The invocation seed.
    #[serde(default)]
    pub seed: u64,
    /// The revision the command ran on when recorded; playback on another revision adjusts the
    /// seed so the command generates the recorded identifiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// The selection the command saw.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selection: Vec<ObjectId>,
    /// The inputs of the step (version 2): parameter name to input name. At playback the
    /// parameter takes the value supplied for the input; the recorded value in `line` is the
    /// default a surface offers when it prompts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// The 1-based line number in the text the step was read from; not serialized.
    #[serde(skip)]
    pub line_number: usize,
}

/// A parsed macro: the header (none for a plain list of command lines) and the steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    /// The header of a JSON Lines macro.
    pub header: Option<MacroHeader>,
    /// The invocations in order.
    pub steps: Vec<MacroStep>,
}

fn malformed(line: usize, reason: &'static str) -> Error {
    let line = i64::try_from(line).unwrap_or(i64::MAX);
    Error::new(codes::MACRO_MALFORMED)
        .with_arg("line", line)
        .with_detail("line", line)
        .with_detail("reason", reason)
}

fn limit(name: &'static str, line: usize) -> Error {
    Error::new(codes::MACRO_LIMIT_EXCEEDED)
        .with_detail("limit", name)
        .with_detail("line", i64::try_from(line).unwrap_or(i64::MAX))
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// A parameter name: an ASCII letter or `_`, then letters, digits and `_`, at most 128 bytes.
fn param_ok(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 128
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// An input name: a lowercase ASCII letter, then lowercase letters, digits and `_`, at most 64
/// bytes.
fn input_ok(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 64
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// An input of a macro, as a surface needs it to prompt for a value.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroInput {
    /// The input name.
    pub name: String,
    /// The command of the first step that uses the input.
    pub command: String,
    /// The parameter of that step the input replaces.
    pub param: String,
    /// The value recorded for that parameter.
    pub default: Value,
}

impl MacroStep {
    /// Makes the value of `param` an input called `name`: at playback the parameter takes the
    /// value supplied for `name`. The recorded value stays in the line as the default.
    ///
    /// # Errors
    ///
    /// `cmd.macro.malformed` (reason `input`) when a name is not well formed, the step has
    /// [`MAX_STEP_INPUTS`] inputs already, or the line gives no such parameter; the errors of
    /// [`parse_line`] for the line.
    pub fn set_input(&mut self, registry: &Registry, param: &str, name: &str) -> Result<()> {
        let parsed = parse_line(registry, &BTreeMap::new(), &self.line)?;
        let known = parsed.is_some_and(|p| p.params.contains_key(param));
        let full = self.inputs.len() >= MAX_STEP_INPUTS && !self.inputs.contains_key(param);
        if !known || full || !param_ok(param) || !input_ok(name) {
            return Err(malformed(self.line_number, "input"));
        }
        self.inputs.insert(param.to_owned(), name.to_owned());
        Ok(())
    }
}

impl Macro {
    /// The inputs of the macro, each name once, in the order of first use.
    ///
    /// # Errors
    ///
    /// The errors of [`parse_line`] for a step's line.
    pub fn inputs(&self, registry: &Registry) -> Result<Vec<MacroInput>> {
        let mut found: Vec<MacroInput> = Vec::new();
        for step in &self.steps {
            if step.inputs.is_empty() {
                continue;
            }
            let Some(parsed) = parse_line(registry, &BTreeMap::new(), &step.line)? else {
                continue;
            };
            for (param, name) in &step.inputs {
                if found.iter().any(|i| &i.name == name) {
                    continue;
                }
                found.push(MacroInput {
                    name: name.clone(),
                    command: parsed.command.clone(),
                    param: param.clone(),
                    default: parsed.params.get(param).cloned().unwrap_or(Value::Null),
                });
            }
        }
        Ok(found)
    }
}

/// Reads a macro or a batch list. Text whose first non-blank line starts with `{` is a JSON
/// Lines macro: a header, then one [`MacroStep`] object per line. Any other text is a list of
/// command lines, one per line, each with seed 0. In both, blank lines are skipped, and in a
/// list so are lines starting with `#`.
///
/// # Errors
///
/// `cmd.macro.limit_exceeded` (detail `limit`: `input_bytes` for the whole text over
/// `limits.input_bytes`, `line_bytes` for a line over `limits.string_bytes`, `line_count` for
/// more than `limits.record_count` steps); `cmd.macro.malformed` (details `line`, `reason`:
/// `header`, `step`); `cmd.macro.version_unsupported` (detail `version`).
pub fn parse_macro(text: &str, limits: &Limits) -> Result<Macro> {
    if as_u64(text.len()) > limits.input_bytes {
        return Err(limit("input_bytes", 0));
    }
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index.saturating_add(1), line));
    let json = text.trim_start().starts_with('{');
    let mut header = None;
    let mut steps = Vec::new();
    for (number, line) in lines.by_ref() {
        if as_u64(line.len()) > limits.string_bytes {
            return Err(limit("line_bytes", number));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || (!json && trimmed.starts_with('#')) {
            continue;
        }
        if json && header.is_none() {
            let parsed: MacroHeader =
                serde_json::from_str(trimmed).map_err(|_| malformed(number, "header"))?;
            if parsed.format != MACRO_FORMAT {
                return Err(malformed(number, "header"));
            }
            if !(MACRO_MIN_VERSION..=MACRO_VERSION).contains(&parsed.version) {
                return Err(Error::new(codes::MACRO_VERSION_UNSUPPORTED)
                    .with_arg("version", i64::from(parsed.version))
                    .with_detail("version", i64::from(parsed.version)));
            }
            header = Some(parsed);
            continue;
        }
        if as_u64(steps.len()) >= limits.record_count {
            return Err(limit("line_count", number));
        }
        let mut step = if json {
            serde_json::from_str::<MacroStep>(trimmed).map_err(|_| malformed(number, "step"))?
        } else {
            MacroStep {
                line: trimmed.to_owned(),
                seed: 0,
                revision: None,
                selection: Vec::new(),
                inputs: BTreeMap::new(),
                line_number: 0,
            }
        };
        step.line_number = number;
        let version = header.as_ref().map_or(MACRO_VERSION, |h| h.version);
        if (version < 2 && !step.inputs.is_empty())
            || step.inputs.len() > MAX_STEP_INPUTS
            || step
                .inputs
                .iter()
                .any(|(p, n)| !param_ok(p) || !input_ok(n))
        {
            return Err(malformed(number, "input"));
        }
        steps.push(step);
    }
    Ok(Macro { header, steps })
}

/// Collects the invocations a session completes as a macro. Shared between threads.
#[derive(Debug)]
pub struct Recorder {
    header: MacroHeader,
    max_steps: u64,
    steps: Mutex<Vec<MacroStep>>,
    /// Set when the session ran something a macro cannot express (an undo or redo).
    unrecordable: AtomicBool,
}

impl Recorder {
    /// A recorder for `product_name` and `product_version` that keeps at most
    /// `limits.record_count` steps.
    #[must_use]
    pub fn new(
        product_name: impl Into<String>,
        product_version: impl Into<String>,
        limits: &Limits,
    ) -> Self {
        Self {
            header: MacroHeader {
                format: MACRO_FORMAT.to_owned(),
                version: MACRO_VERSION,
                protocol: crate::protocol::SUPPORTED_VERSIONS
                    .last()
                    .map(|(major, minor)| format!("{major}.{minor}"))
                    .unwrap_or_default(),
                product: MacroProduct {
                    name: product_name.into(),
                    version: product_version.into(),
                },
            },
            max_steps: limits.record_count,
            steps: Mutex::new(Vec::new()),
            unrecordable: AtomicBool::new(false),
        }
    }

    /// Records `invocation`, which completed on a snapshot at `revision`, in canonical form.
    /// Call it only for invocations that succeeded.
    ///
    /// # Errors
    ///
    /// The errors of [`format_line`]; `cmd.macro.limit_exceeded` (limit `line_count`) when the
    /// recorder is full. Nothing is recorded then.
    pub fn record(
        &self,
        registry: &Registry,
        invocation: &Invocation,
        revision: u64,
    ) -> Result<()> {
        let line = format_line(registry, invocation)?;
        let mut steps = self.steps.lock().unwrap_or_else(PoisonError::into_inner);
        if as_u64(steps.len()) >= self.max_steps {
            return Err(limit("line_count", steps.len().saturating_add(2)));
        }
        let line_number = steps.len().saturating_add(2);
        steps.push(MacroStep {
            line,
            seed: invocation.seed,
            revision: Some(revision),
            selection: invocation.selection.clone(),
            inputs: BTreeMap::new(),
            line_number,
        });
        Ok(())
    }

    /// Makes the value of `param` in recorded step `index` (0-based) an input called `name`
    /// ([`MacroStep::set_input`]).
    ///
    /// # Errors
    ///
    /// Those of [`MacroStep::set_input`]; `cmd.macro.malformed` (reason `step`) when there is
    /// no such step.
    pub fn set_input(
        &self,
        registry: &Registry,
        index: usize,
        param: &str,
        name: &str,
    ) -> Result<()> {
        let mut steps = self.steps.lock().unwrap_or_else(PoisonError::into_inner);
        let step = steps
            .get_mut(index)
            .ok_or_else(|| malformed(index.saturating_add(2), "step"))?;
        step.set_input(registry, param, name)
    }

    /// Marks the recording as not faithful: the session did something a macro cannot express,
    /// so the steps recorded so far do not reproduce it. [`Recorder::is_unrecordable`] tells.
    pub fn mark_unrecordable(&self) {
        self.unrecordable.store(true, Ordering::Release);
    }

    /// True once [`Recorder::mark_unrecordable`] was called.
    #[must_use]
    pub fn is_unrecordable(&self) -> bool {
        self.unrecordable.load(Ordering::Acquire)
    }

    /// The number of recorded steps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The macro text: the header line and one line per step, each ending with a newline,
    /// object keys sorted.
    #[must_use]
    pub fn text(&self) -> String {
        let steps = self.steps.lock().unwrap_or_else(PoisonError::into_inner);
        let mut text = String::new();
        let header = serde_json::to_value(&self.header).unwrap_or_default();
        text.push_str(&header.to_string());
        text.push('\n');
        for step in steps.iter() {
            // `Value` keeps keys sorted, so the same steps give the same bytes.
            let value = serde_json::to_value(step).unwrap_or_default();
            text.push_str(&value.to_string());
            text.push('\n');
        }
        text
    }
}

/// The seed of a step on a snapshot at `now`. A recorded step generates the recorded
/// identifiers: the context mixes the revision into the seed, so the mix is swapped. A step
/// without a recorded revision mixes in `content`, the digest of the starting objects, because
/// a reopened package starts again at revision 0 and would otherwise repeat identifiers.
fn adjusted_seed(seed: u64, recorded: Option<u64>, now: u64, content: u64) -> u64 {
    recorded.map_or(seed ^ content, |recorded| {
        seed ^ recorded.wrapping_mul(REVISION_MIX) ^ now.wrapping_mul(REVISION_MIX)
    })
}

/// An order-independent digest of the identifiers of `document`'s objects; 0 when empty.
fn content_digest(document: &Document) -> u64 {
    document
        .snapshot()
        .iter()
        .fold(0u64, |digest, (_, object)| {
            let raw = object.id.as_u128();
            // Truncation keeps the low half; the high half is folded in first.
            #[allow(clippy::cast_possible_truncation, reason = "folding 128 bits into 64")]
            let mut state = (raw >> 64) as u64 ^ raw as u64;
            digest.wrapping_add(split_mix(&mut state))
        })
}

/// Plays `steps` on `document` for `session`: each line is parsed with [`parse_line`] and run
/// through [`Invoker::invoke`], so exposure, permissions, limits, the schema and the
/// predicate are checked as for every surface. The steps run inside the undo group `group`;
/// on success the group ends and forms one undo step, and the outcome of each step is returned
/// in order. A step whose line is blank is `cmd.macro.malformed` (reason `empty`).
///
/// # Errors
///
/// The first failing step's error (of the line parser or the invoker) with the detail `line`,
/// its line number; the group is rolled back first, so the document content is as before
/// playback. The group's own errors (`doc.undo.*`) without a line.
pub fn play(
    registry: &Registry,
    document: &Document,
    session: &Session,
    steps: &[MacroStep],
    group: &str,
) -> Result<Vec<Outcome>> {
    play_with(registry, document, session, steps, group, &BTreeMap::new())
}

/// [`play`] with values for the inputs of the steps: a parameter that a step declared as an
/// input takes the value supplied under the input's name (checked by the invoker like any
/// parameter). Values for names no step declares are ignored.
///
/// # Errors
///
/// Those of [`play`]; `cmd.macro.input_missing` (argument and detail `input`) for the first
/// input without a value, before anything changes.
pub fn play_with(
    registry: &Registry,
    document: &Document,
    session: &Session,
    steps: &[MacroStep],
    group: &str,
    values: &BTreeMap<String, Value>,
) -> Result<Vec<Outcome>> {
    for step in steps {
        if let Some(name) = step.inputs.values().find(|n| !values.contains_key(*n)) {
            return Err(Error::new(codes::MACRO_INPUT_MISSING)
                .with_arg("input", name.as_str())
                .with_detail("input", name.as_str())
                .with_detail("line", i64::try_from(step.line_number).unwrap_or(i64::MAX)));
        }
    }
    document.begin_group(group).wait()?;
    let invoker = Invoker::new(registry);
    let content = content_digest(document);
    let mut outcomes = Vec::with_capacity(steps.len());
    for step in steps {
        match run_step(&invoker, registry, document, session, step, content, values) {
            Ok(outcome) => outcomes.push(outcome),
            Err(error) => {
                document.fail_group().wait()?;
                let line = i64::try_from(step.line_number).unwrap_or(i64::MAX);
                return Err(error.with_detail("line", line));
            }
        }
    }
    document.end_group().wait()?;
    Ok(outcomes)
}

fn run_step(
    invoker: &Invoker<'_>,
    registry: &Registry,
    document: &Document,
    session: &Session,
    step: &MacroStep,
    content: u64,
    values: &BTreeMap<String, Value>,
) -> Result<Outcome> {
    let Some(parsed) = parse_line(registry, &BTreeMap::new(), &step.line)? else {
        return Err(malformed(step.line_number, "empty"));
    };
    let mut params = parsed.params;
    for (param, name) in &step.inputs {
        if let Some(value) = values.get(name) {
            params.insert(param.clone(), value.clone());
        }
    }
    let mut invocation = Invocation::new(parsed.command, Value::Object(params));
    let now = document.snapshot().revision();
    invocation.seed = adjusted_seed(step.seed, step.revision, now, content);
    invocation.selection.clone_from(&step.selection);
    invoker.invoke(document, session, invocation)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use super::*;

    #[test]
    fn seeds_are_unchanged_on_the_recorded_revision_and_swap_the_mix_otherwise() {
        assert_eq!(adjusted_seed(7, Some(3), 3, 5), 7);
        assert_eq!(adjusted_seed(7, None, 9, 0), 7);
        assert_eq!(adjusted_seed(7, None, 9, 5), 2);
        let seed = adjusted_seed(7, Some(3), 9, 5);
        assert_eq!(
            seed ^ 9u64.wrapping_mul(REVISION_MIX),
            7 ^ 3u64.wrapping_mul(REVISION_MIX)
        );
    }

    #[test]
    fn lists_and_macros_parse_with_line_numbers() {
        let list = parse_macro("# comment\n\na.b.c x=1\n  d.e.f\n", &Limits::DEFAULT).unwrap();
        assert!(list.header.is_none());
        let lines: Vec<(&str, usize)> = list
            .steps
            .iter()
            .map(|s| (s.line.as_str(), s.line_number))
            .collect();
        assert_eq!(lines, [("a.b.c x=1", 3), ("d.e.f", 4)]);
        let recorder = Recorder::new("p", "1", &Limits::DEFAULT);
        let text = recorder.text();
        assert!(text.starts_with(r#"{"format":"arc-macro","product":{"name":"p","version":"1"}"#));
        let text = format!("{text}\n{{\"line\":\"a.b.c\",\"seed\":5,\"revision\":2}}\n");
        let parsed = parse_macro(&text, &Limits::DEFAULT).unwrap();
        assert_eq!(parsed.header.unwrap().product.name, "p");
        assert_eq!(parsed.steps[0].seed, 5);
        assert_eq!(parsed.steps[0].line_number, 3);
    }

    #[test]
    fn malformed_and_oversized_macros_are_refused() {
        let code = |text: &str, limits: &Limits| parse_macro(text, limits).unwrap_err();
        let d = Limits::DEFAULT;
        assert_eq!(code("{", &d).code(), "cmd.macro.malformed");
        assert_eq!(
            code(
                r#"{"format":"x","version":1,"protocol":"1.0","product":{"name":"","version":""}}"#,
                &d
            )
            .code(),
            "cmd.macro.malformed"
        );
        let future = r#"{"format":"arc-macro","version":3,"protocol":"1.0","product":{"name":"","version":""}}"#;
        assert_eq!(code(future, &d).code(), "cmd.macro.version_unsupported");
        let header = r#"{"format":"arc-macro","version":1,"protocol":"1.0","product":{"name":"","version":""}}"#;
        let e = code(&format!("{header}\n{{\"line\":1}}"), &d);
        assert_eq!(
            e.details().get("reason"),
            Some(&arc_base::error::Value::Text("step".into()))
        );
        let small = Limits {
            string_bytes: 4,
            record_count: 1,
            input_bytes: 64,
            ..Limits::DEFAULT
        };
        assert_eq!(code("abcde", &small).code(), "cmd.macro.limit_exceeded");
        assert_eq!(code("a\nb", &small).code(), "cmd.macro.limit_exceeded");
        assert_eq!(
            code(&"a\n".repeat(40), &small).code(),
            "cmd.macro.limit_exceeded"
        );
    }
}
