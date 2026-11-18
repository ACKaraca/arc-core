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

use arc_base::{Error, ObjectId, Result};
use arc_doc::Document;
use serde_json::{Map, Value};

use crate::context::Session;
use crate::error::codes;
use crate::invoke::{Invocation, Invoker, Outcome};
use crate::line::{self, Completion};
use crate::prompt::{Feedback, Prompt, PromptInput, PromptSession};
use crate::registry::Registry;

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
}

/// Runs a completed invocation for [`CommandLine::drive`].
pub type Runner<'r> = &'r mut dyn FnMut(Invocation) -> Result<Outcome>;

/// The state of one command line: the active prompt session and the last completed command.
#[derive(Debug, Default)]
pub struct CommandLine {
    active: Option<PromptSession>,
    last: Option<LastCommand>,
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
                self.active = None;
                Ok(Submitted::Cancelled)
            }
            Feedback::Failed(error) => {
                self.active = None;
                Err(error)
            }
            Feedback::Done(invocation) => {
                self.active = None;
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
                self.active = None;
                let spec = Arc::clone(&target.registry.resolve(command, target.localized)?.spec);
                let snapshot = target.document.snapshot();
                let (session, feedback) =
                    PromptSession::start(spec, preset, target.session, &snapshot);
                self.active = Some(session);
                self.follow(target, feedback, runner)
            }
            LineStep::Submit(text) => {
                if let Some(active) = self.active.as_mut() {
                    let feedback = active.answer_text(text, &target.document.snapshot());
                    return self.follow(target, feedback, runner);
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

    /// Cancels the active command; [`Submitted::Idle`] when there is none.
    pub fn cancel(&mut self) -> Submitted {
        match self.active.take() {
            Some(_) => Submitted::Cancelled,
            None => Submitted::Idle,
        }
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

    /// Completion for a partial line: the keyword options of the active prompt, otherwise
    /// [`line::complete`].
    #[must_use]
    pub fn complete(&self, target: &Target<'_>, text: &str, cursor: usize) -> Completion {
        match &self.active {
            Some(active) => {
                let partial = text.get(..cursor.min(text.len())).unwrap_or(text);
                let start = partial.len() - partial.trim_start().len();
                Completion {
                    span: Some(line::Span {
                        start,
                        end: partial.len(),
                    }),
                    candidates: active
                        .complete(partial)
                        .into_iter()
                        .map(|o| line::Candidate {
                            text: o.id.clone(),
                            kind: line::CandidateKind::Keyword,
                        })
                        .collect(),
                }
            }
            None => line::complete(target.registry, target.localized, text, cursor),
        }
    }
}
