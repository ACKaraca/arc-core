//! The command-line driver (CORE-CMD-013) and repeat of the last command (CORE-CMD-015).
//!
//! A [`CommandLine`] holds the active prompt session and the last completed command. A typed
//! line starts a command (parsed by [`crate::line::parse_line`]); a complete line runs at once,
//! a line with missing required parameters continues with the command's prompt machine. While a
//! prompt is active, a typed line answers it. The empty line accepts the prompt's default, or,
//! with no active command, repeats the last command. Every run goes through
//! [`Invoker::invoke`].

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::{Dimension, Error, ObjectId, Result};
use arc_doc::Document;
use serde_json::{Map, Value};

use crate::context::Session;
use crate::error::codes;
use crate::invoke::{Invocation, Invoker, Outcome};
use crate::line::{self, Completion};
use crate::prompt::{
    Accept, Feedback, PlaneTransform, Point, PointEntry, Prompt, PromptInput, PromptSession,
};
use crate::registry::Registry;
use crate::spec::UndoBehavior;

/// What a command line acts on.
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    /// The commands.
    pub registry: &'a Registry,
    /// The document commands run on.
    pub document: &'a Document,
    /// The caller's session.
    pub session: &'a Session,
    /// The selection invocations see.
    pub selection: &'a [ObjectId],
    /// Localized command names by identifier, from the active catalog.
    pub localized: &'a BTreeMap<String, String>,
    /// The seed of invocations.
    pub seed: u64,
}

/// The last completed command.
#[derive(Debug, Clone, PartialEq)]
pub struct LastCommand {
    /// The command identifier.
    pub command: String,
    /// The final parameters.
    pub params: Value,
}

/// What a submitted line or answer led to.
#[derive(Debug)]
pub enum Submitted {
    /// Nothing happened: an empty line with nothing to repeat.
    Idle,
    /// A prompt is asked.
    Prompt(Prompt),
    /// The answer was rejected; the prompt is asked again.
    Rejected {
        /// Why.
        error: Error,
        /// The prompt still asked.
        prompt: Prompt,
    },
    /// The command ran.
    Ran(Outcome),
    /// The active command was cancelled.
    Cancelled,
}

/// One step of a command line, for [`CommandLine::drive`]; each variant matches the method of
/// the same name.
#[derive(Debug)]
pub enum LineStep<'a> {
    /// [`CommandLine::start`]: a command with preset parameters.
    Start {
        /// The command name.
        command: &'a str,
        /// The parameters already known.
        preset: Map<String, Value>,
    },
    /// [`CommandLine::submit`]: a typed line.
    Submit(&'a str),
    /// [`CommandLine::answer`]: an answer from the UI.
    Answer(PromptInput),
    /// [`CommandLine::back`].
    Back,
    /// [`CommandLine::repeat_last`].
    Repeat,
    /// [`CommandLine::transparent`]: a transparent command typed during a prompt.
    Transparent(&'a str),
}

/// The prefix that marks a typed line as a transparent command while a prompt is active. A
/// doubled prefix is an escape: the line answers the prompt with the text after the first one.
pub const TRANSPARENT_PREFIX: char = '\'';

/// Runs a completed invocation for [`CommandLine::drive`].
pub type Runner<'r> = &'r mut dyn FnMut(Invocation) -> Result<Outcome>;

/// The state of one command line: the active prompt session, the last completed command and the
/// point state of the line.
///
/// The point state (a [`PointEntry`]) is the last point of the previous prompt session, its
/// settings and the host's pointer direction. Each prompt session starts with it and hands its
/// state back when it ends. A transparent command does not change it.
#[derive(Debug, Default)]
pub struct CommandLine {
    active: Option<PromptSession>,
    last: Option<LastCommand>,
    entry: PointEntry,
}

impl CommandLine {
    /// An idle command line.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The active prompt session.
    #[must_use]
    pub fn active(&self) -> Option<&PromptSession> {
        self.active.as_ref()
    }

    /// The last completed command that repeat would run.
    #[must_use]
    pub fn last(&self) -> Option<&LastCommand> {
        self.last.as_ref()
    }

    /// Ends the active prompt session, if any, and keeps its point state: the last point it
    /// accepted (or, when it accepted none, the line's last point) and its settings. A cancelled
    /// command keeps the points it accepted.
    fn retire(&mut self) {
        if let Some(session) = self.active.take() {
            self.entry = session.carry();
        }
    }

    /// The point state of the active prompt session, or of the line when none is active.
    #[must_use]
    pub fn point_entry(&self) -> &PointEntry {
        self.active
            .as_ref()
            .map_or(&self.entry, PromptSession::point_entry)
    }

    /// The base point a rubber band measures from: the active session's last point, else the
    /// line's.
    #[must_use]
    pub fn base_point(&self) -> Option<Point> {
        self.active
            .as_ref()
            .map_or_else(|| self.entry.last_point(), PromptSession::last_point)
    }

    /// The direction a direct distance uses: a locked direction, else the pointer direction.
    #[must_use]
    pub fn direction(&self) -> Option<[f64; 2]> {
        self.point_entry().effective_direction()
    }

    /// Sets the pointer direction (a unit vector in the plane; normalized) for the active session
    /// and the line.
    ///
    /// # Errors
    ///
    /// `cmd.prompt.range` (reason `direction`) when the vector is not finite or has no length;
    /// nothing changes then.
    pub fn set_pointer_direction(&mut self, direction: Option<[f64; 2]>) -> Result<()> {
        self.entry.set_direction(direction)?;
        if let Some(active) = self.active.as_mut() {
            active.point_entry_mut().set_direction(direction)?;
        }
        Ok(())
    }

    /// Sets the last point the next point answer measures from, for the active session and the
    /// line. An active session that has accepted a point keeps its own last point.
    ///
    /// # Errors
    ///
    /// `cmd.prompt.range` (reason `not_finite`) when a coordinate is not finite; nothing changes.
    pub fn set_last_point(&mut self, point: Option<Point>) -> Result<()> {
        self.entry.set_last_point(point)?;
        if let Some(active) = self.active.as_mut() {
            active.point_entry_mut().set_last_point(point)?;
        }
        Ok(())
    }

    /// Sets whether a typed coordinate without a prefix is relative, for the active session and
    /// the line.
    pub fn set_relative_default(&mut self, relative: bool) {
        self.entry.set_relative_default(relative);
        if let Some(active) = self.active.as_mut() {
            active.point_entry_mut().set_relative_default(relative);
        }
    }

    /// Sets the user coordinate system transform, for the active session and the line.
    pub fn set_transform(&mut self, transform: PlaneTransform) {
        self.entry.set_transform(transform);
        if let Some(active) = self.active.as_mut() {
            active.point_entry_mut().set_transform(transform);
        }
    }

    /// Records a command completed elsewhere (a menu entry or a tool) as the last command;
    /// transparent and unknown commands are not recorded.
    pub fn record(&mut self, registry: &Registry, command: &str, params: &Value) {
        if let Ok(resolved) = registry.resolve(command, &BTreeMap::new())
            && !resolved.spec.is_transparent()
        {
            self.last = Some(LastCommand {
                command: resolved.spec.id().to_owned(),
                params: params.clone(),
            });
        }
    }

    fn run(
        &mut self,
        target: &Target<'_>,
        command: &str,
        params: Value,
        runner: Runner<'_>,
    ) -> Result<Submitted> {
        let mut invocation = Invocation::new(command, params.clone());
        invocation.selection = target.selection.to_vec();
        invocation.seed = target.seed;
        let outcome = runner(invocation)?;
        self.record(target.registry, &outcome.command, &params);
        Ok(Submitted::Ran(outcome))
    }

    fn follow(
        &mut self,
        target: &Target<'_>,
        feedback: Feedback,
        runner: Runner<'_>,
    ) -> Result<Submitted> {
        match feedback {
            Feedback::Ask(prompt) => Ok(Submitted::Prompt(prompt)),
            Feedback::Rejected { error, prompt } => Ok(Submitted::Rejected { error, prompt }),
            Feedback::Cancelled => {
                self.retire();
                Ok(Submitted::Cancelled)
            }
            Feedback::Failed(error) => {
                self.retire();
                Err(error)
            }
            Feedback::Done(invocation) => {
                self.retire();
                self.run(target, &invocation.command, invocation.params, runner)
            }
        }
    }

    /// Takes one [`LineStep`] like the method of the same name, except that every completed
    /// invocation goes to `runner` instead of [`Invoker::invoke`]. An application uses it to
    /// route command-line runs through its own entry point (which may adjust the session or
    /// handle shell commands); the runner's outcome or error is returned as the run's.
    ///
    /// # Errors
    ///
    /// Those of the step's method, with the runner's errors in place of the invoker's.
    pub fn drive(
        &mut self,
        target: &Target<'_>,
        step: LineStep<'_>,
        runner: Runner<'_>,
    ) -> Result<Submitted> {
        match step {
            LineStep::Start { command, preset } => {
                self.retire();
                let spec = Arc::clone(&target.registry.resolve(command, target.localized)?.spec);
                let snapshot = target.document.snapshot();
                let (session, feedback) = PromptSession::start_with_entry(
                    spec,
                    preset,
                    target.session,
                    &snapshot,
                    self.entry.clone(),
                );
                self.active = Some(session);
                self.follow(target, feedback, runner)
            }
            LineStep::Transparent(text) => self.run_transparent(target, text, runner),
            LineStep::Submit(text) => {
                if self.active.is_some() {
                    return match text.trim_start().strip_prefix(TRANSPARENT_PREFIX) {
                        Some(rest) if !rest.starts_with(TRANSPARENT_PREFIX) => {
                            self.run_transparent(target, rest, runner)
                        }
                        Some(escaped) => self.answer_typed(target, escaped, runner),
                        None => self.answer_typed(target, text, runner),
                    };
                }
                let Some(parsed) = line::parse_line(target.registry, target.localized, text)?
                else {
                    return if self.last.is_some() {
                        self.drive(target, LineStep::Repeat, runner)
                    } else {
                        Ok(Submitted::Idle)
                    };
                };
                if parsed.missing.is_empty() {
                    return self.run(
                        target,
                        &parsed.command,
                        Value::Object(parsed.params),
                        runner,
                    );
                }
                self.drive(
                    target,
                    LineStep::Start {
                        command: &parsed.command,
                        preset: parsed.params,
                    },
                    runner,
                )
            }
            LineStep::Answer(input) => {
                let Some(active) = self.active.as_mut() else {
                    return Err(Error::new(codes::PROMPT_ENDED));
                };
                let feedback = active.answer(input, &target.document.snapshot());
                self.follow(target, feedback, runner)
            }
            LineStep::Back => {
                let Some(active) = self.active.as_mut() else {
                    return Err(Error::new(codes::PROMPT_ENDED));
                };
                let feedback = active.back(&target.document.snapshot());
                self.follow(target, feedback, runner)
            }
            LineStep::Repeat => {
                let Some(last) = self.last.clone() else {
                    return Err(Error::new(codes::REPEAT_NONE));
                };
                self.run(target, &last.command, last.params, runner)
            }
        }
    }

    /// Answers the active prompt with a typed text and follows its feedback.
    fn answer_typed(
        &mut self,
        target: &Target<'_>,
        text: &str,
        runner: Runner<'_>,
    ) -> Result<Submitted> {
        let Some(active) = self.active.as_mut() else {
            return Err(Error::new(codes::PROMPT_ENDED));
        };
        let feedback = active.answer_text(text, &target.document.snapshot());
        self.follow(target, feedback, runner)
    }

    /// Runs a transparent command line beside the active prompt: the prompt session and the
    /// last command are not touched.
    fn run_transparent(
        &mut self,
        target: &Target<'_>,
        text: &str,
        runner: Runner<'_>,
    ) -> Result<Submitted> {
        let refused = |reason: &'static str| {
            Error::new(codes::TRANSPARENT_REFUSED).with_detail("reason", reason)
        };
        let Some(parsed) = line::parse_line(target.registry, target.localized, text)? else {
            return Err(refused("empty"));
        };
        let spec = Arc::clone(
            &target
                .registry
                .resolve(&parsed.command, target.localized)?
                .spec,
        );
        if !spec.is_transparent() {
            return Err(refused("not_transparent"));
        }
        if !parsed.missing.is_empty() {
            return Err(refused("missing_params"));
        }
        // Only a command that opens no transaction may run inside the outer command's group.
        let blocked = match spec.undo() {
            UndoBehavior::NotUndoable => false,
            UndoBehavior::Undoable => target.document.with_undo(|undo| undo.group().is_some()),
            UndoBehavior::UndoControl => true,
        };
        if blocked {
            return Err(refused("group_open"));
        }
        let mut invocation = Invocation::new(spec.id(), Value::Object(parsed.params));
        invocation.selection = target.selection.to_vec();
        invocation.seed = target.seed;
        Ok(Submitted::Ran(runner(invocation)?))
    }

    fn invoke(&mut self, target: &Target<'_>, step: LineStep<'_>) -> Result<Submitted> {
        let invoker = Invoker::new(target.registry);
        self.drive(target, step, &mut |invocation| {
            invoker.invoke(target.document, target.session, invocation)
        })
    }

    /// Starts `command` with the parameters `preset` (from a menu or tool), prompting for the
    /// missing ones; cancels the active command first.
    ///
    /// # Errors
    ///
    /// `cmd.unknown`, `cmd.name.ambiguous`, the machine's errors, and those of the run.
    pub fn start(
        &mut self,
        target: &Target<'_>,
        command: &str,
        preset: Map<String, Value>,
    ) -> Result<Submitted> {
        self.invoke(target, LineStep::Start { command, preset })
    }

    /// Submits a typed line: an answer while a prompt is active, otherwise a command line. A
    /// blank line accepts the default, or repeats the last command when no command is active
    /// ([`Submitted::Idle`] when there is none).
    ///
    /// # Errors
    ///
    /// Those of [`line::parse_line`], of the machine, and of the run.
    pub fn submit(&mut self, target: &Target<'_>, text: &str) -> Result<Submitted> {
        self.invoke(target, LineStep::Submit(text))
    }

    /// Runs the transparent command line `text` while a prompt is active, without cancelling
    /// the prompt or changing the last command (CORE-CMD-012). A typed line reaches it through
    /// [`submit`](Self::submit) when it starts with [`TRANSPARENT_PREFIX`].
    ///
    /// # Errors
    ///
    /// `cmd.transparent.refused` (detail `reason`: `empty`, `not_transparent`,
    /// `missing_params`, `group_open` for an undoable command while the document has an open
    /// undo group, `group_open` for an undo-control command); those of [`line::parse_line`]
    /// and of the run.
    pub fn transparent(&mut self, target: &Target<'_>, text: &str) -> Result<Submitted> {
        self.invoke(target, LineStep::Transparent(text))
    }

    /// Answers the active prompt with a typed input from the UI.
    ///
    /// # Errors
    ///
    /// `cmd.prompt.ended` without an active command; those of the machine and the run.
    pub fn answer(&mut self, target: &Target<'_>, input: PromptInput) -> Result<Submitted> {
        self.invoke(target, LineStep::Answer(input))
    }

    /// Steps back one answer in the active command.
    ///
    /// # Errors
    ///
    /// `cmd.prompt.ended` without an active command; the machine's errors.
    pub fn back(&mut self, target: &Target<'_>) -> Result<Submitted> {
        self.invoke(target, LineStep::Back)
    }

    /// Cancels the active command, keeping the points it accepted as the line's last point
    /// (a cancel before any point keeps the line's previous one); [`Submitted::Idle`] when there
    /// is none.
    pub fn cancel(&mut self) -> Submitted {
        if self.active.is_none() {
            return Submitted::Idle;
        }
        self.retire();
        Submitted::Cancelled
    }

    /// Runs the last completed command again with the same parameters through
    /// [`Invoker::invoke`].
    ///
    /// # Errors
    ///
    /// `cmd.repeat.none` when no command completed yet; those of the run.
    pub fn repeat_last(&mut self, target: &Target<'_>) -> Result<Submitted> {
        self.invoke(target, LineStep::Repeat)
    }

    /// Completion for a partial line: with an active prompt, its keyword options, else the
    /// names of objects (for a pick or selection prompt) or the units (for a distance, angle,
    /// number or expression prompt) of the text typed so far; otherwise [`line::complete_in`]
    /// on the document's snapshot.
    #[must_use]
    pub fn complete(&self, target: &Target<'_>, text: &str, cursor: usize) -> Completion {
        let snapshot = target.document.snapshot();
        let Some(active) = &self.active else {
            return line::complete_in(
                target.registry,
                target.localized,
                Some(&snapshot),
                text,
                cursor,
            );
        };
        let partial = text
            .get(..line::clamp_cursor(text, cursor))
            .unwrap_or_default();
        let lead = partial.len() - partial.trim_start().len();
        let typed = partial.trim_start();
        let at = |start: usize| line::Span {
            start,
            end: partial.len(),
        };
        let keywords: Vec<line::Candidate> = active
            .complete(partial)
            .into_iter()
            .map(|o| line::Candidate {
                text: o.id.clone(),
                kind: line::CandidateKind::Keyword,
                label: None,
            })
            .collect();
        if !keywords.is_empty() {
            return Completion {
                span: Some(at(lead)),
                candidates: keywords,
            };
        }
        for accept in active.prompt().iter().flat_map(|p| &p.accepts) {
            let (offset, candidates) = match accept {
                Accept::Pick { types } => {
                    (0, line::object_candidates(Some(&snapshot), typed, types))
                }
                Accept::Selection { types, .. } => {
                    let start = typed.rfind([',', ' ']).map_or(0, |i| i + 1);
                    let name = typed.get(start..).unwrap_or("");
                    (start, line::object_candidates(Some(&snapshot), name, types))
                }
                Accept::Distance { .. } => line::unit_candidates(typed, Some(Dimension::LENGTH)),
                Accept::Angle { .. } => line::unit_candidates(typed, Some(Dimension::ANGLE)),
                Accept::Number { dimension, .. } if *dimension != Dimension::NONE => {
                    line::unit_candidates(typed, Some(*dimension))
                }
                Accept::Expression => line::unit_candidates(typed, None),
                _ => continue,
            };
            if !candidates.is_empty() {
                return Completion {
                    span: Some(at(lead + offset)),
                    candidates,
                };
            }
        }
        Completion {
            span: Some(at(lead)),
            candidates: Vec::new(),
        }
    }
}
