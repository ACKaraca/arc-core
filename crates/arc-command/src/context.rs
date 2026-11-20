//! The session, the command context and the predicate probe (CORE-CMD-003, CORE-CMD-005,
//! CORE-CMD-008).
//!
//! `run` reads everything it may depend on from its [`Context`]: the document state, the
//! selection, the session, the seeded generator of identifiers and random numbers, the clock,
//! the limits, the cancellation token and the progress sink. It opens no dialog and waits for no
//! input, so the same document state, parameters and seed give the same result.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use arc_base::id::{IdGenerator, Identifier};
use arc_base::{Cancel, Dimension, Error, Limits, ObjectId, Quantity, Result, Unit};
use arc_doc::{Snapshot, Txn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::registry::NameFlags;
use crate::roots::Roots;
use crate::selection::{LastHook, Selections};
use crate::spec::{Permission, Surface};

/// Progress is forwarded only once a command has run this long (CORE-CMD-008).
pub const PROGRESS_DELAY_MILLIS: u64 = 300;

/// The time source of a session. This layer makes no operating-system calls; the application
/// supplies its clock.
pub trait Clock: Send + Sync {
    /// Milliseconds since the Unix epoch.
    fn now_unix_millis(&self) -> u64;
}

/// A clock that stands still, for headless runs that must be reproducible.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixedClock(pub u64);

impl Clock for FixedClock {
    fn now_unix_millis(&self) -> u64 {
        self.0
    }
}

/// Receives the progress of a long-running command.
pub trait ProgressSink: Send + Sync {
    /// `fraction` is in `[0, 1]`; `message_id` names the current step.
    fn progress(&self, fraction: f64, message_id: &str);
}

/// A caller of commands: its surface, the permission classes granted to it, its limits, its
/// clock and the units bare numbers in quantity texts take.
#[derive(Clone)]
pub struct Session {
    /// The surface the session calls from.
    pub surface: Surface,
    /// The granted permission classes.
    pub permissions: BTreeSet<Permission>,
    /// The hostile-input limits of parameters.
    pub limits: Limits,
    /// The clock.
    pub clock: Arc<dyn Clock>,
    /// The context units: a bare number in a quantity text takes the unit of its dimension.
    pub units: Vec<Unit>,
    /// The file roots; commands reach files only through [`Context::open_read`] and
    /// [`Context::create_write`].
    pub roots: Roots,
    /// The current and previous selection of the session ([`crate::selection`]).
    pub(crate) selection: Selections,
    /// The product's answer for the keyword `last`, if it gave one.
    pub(crate) last: Option<LastHook>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("surface", &self.surface)
            .field("permissions", &self.permissions)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A session on `surface` with the `granted` classes, default limits, a clock standing at
    /// 0, no context units and no file roots.
    #[must_use]
    pub fn new(surface: Surface, granted: impl IntoIterator<Item = Permission>) -> Self {
        Self {
            surface,
            permissions: granted.into_iter().collect(),
            limits: Limits::DEFAULT,
            clock: Arc::new(FixedClock(0)),
            units: Vec::new(),
            roots: Roots::none(),
            selection: Selections::default(),
            last: None,
        }
    }

    /// Gives the keyword `last` its meaning: `hook` returns the objects the product names as the
    /// last one (for example the last created object), live or not. Without a hook, `last` fails
    /// with `cmd.selection.unavailable`.
    #[must_use]
    pub fn with_last_objects(
        mut self,
        hook: impl Fn(&Snapshot) -> Vec<ObjectId> + Send + Sync + 'static,
    ) -> Self {
        self.last = Some(Arc::new(hook));
        self
    }

    /// The current selection of the session, live objects only, in order. Clones of the session
    /// share it.
    #[must_use]
    pub fn current_selection(&self, snapshot: &Snapshot) -> Vec<ObjectId> {
        self.selection.current(snapshot)
    }

    /// The selection before the current one, live objects only.
    #[must_use]
    pub fn previous_selection(&self, snapshot: &Snapshot) -> Vec<ObjectId> {
        self.selection.previous(snapshot)
    }

    /// Makes `ids` the current selection and the current one the previous selection. The list is
    /// bounded by the session's `array_length` (the names of the same list kind in prompts) and
    /// holds each identifier once, in order.
    ///
    /// # Errors
    ///
    /// `cmd.params.limit_exceeded` (detail `limit`, `array_length`) when `ids` is longer.
    pub fn record_selection(&self, ids: &[ObjectId], snapshot: &Snapshot) -> Result<()> {
        if u64::try_from(ids.len()).unwrap_or(u64::MAX) > self.limits.array_length {
            return Err(
                Error::new(codes::PARAMS_LIMIT_EXCEEDED).with_detail("limit", "array_length")
            );
        }
        self.selection.record(ids, snapshot);
        Ok(())
    }
}

/// What an enabled predicate reads: the document state, the selection and the session.
#[derive(Debug, Clone, Copy)]
pub struct Probe<'a> {
    /// The document state the command would run on.
    pub snapshot: &'a Snapshot,
    /// The selection.
    pub selection: &'a [ObjectId],
    /// The session.
    pub session: &'a Session,
}

/// A quantity parameter: a number in SI base units, or a text with units parsed by
/// `arc-solve` (`"2400 mm"`, `"3'6\""`), where a bare number takes the session's context unit
/// of the dimension. Its schema is `anyOf` number and string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum QuantityParam {
    /// A value in SI base units.
    Si(f64),
    /// An expression with units.
    Text(String),
}

/// An object parameter: an [`ObjectId`] written as a hyphenated UUID text. Its schema is
/// `{"type": "string", "format": "uuid"}`; a text that is no identifier fails decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdParam(pub ObjectId);

impl JsonSchema for IdParam {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ObjectId".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "format": "uuid"})
    }
}

enum Store<'a> {
    Txn(&'a mut Txn),
    Snapshot(&'a Snapshot),
}

/// The factor the snapshot revision is mixed into the seed with; playback of a macro on another
/// revision adjusts the seed by it (see `macros`).
pub(crate) const REVISION_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// SplitMix64, the random stream of a context.
pub(crate) fn split_mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The context of one `run`.
pub struct Context<'a> {
    store: Store<'a>,
    session: &'a Session,
    selection: &'a [ObjectId],
    ids: IdGenerator,
    random: u64,
    cancel: &'a Cancel,
    progress: Option<&'a dyn ProgressSink>,
    start: u64,
    flags: NameFlags,
    pub(crate) selection_after: Option<Vec<ObjectId>>,
    pub(crate) selection_request: Option<Vec<ObjectId>>,
}

impl fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context")
            .field("session", self.session)
            .field("selection", &self.selection)
            .finish_non_exhaustive()
    }
}

impl<'a> Context<'a> {
    pub(crate) fn new(
        txn: Option<&'a mut Txn>,
        snapshot: &'a Snapshot,
        session: &'a Session,
        selection: &'a [ObjectId],
        seed: u64,
        cancel: &'a Cancel,
        progress: Option<&'a dyn ProgressSink>,
    ) -> Self {
        // The revision makes each invocation's stream distinct within one document, so
        // repeated invocations with the same seed never generate the same identifiers.
        let mut stream = seed ^ snapshot.revision().wrapping_mul(REVISION_MIX);
        let stream = split_mix(&mut stream);
        Self {
            store: txn.map_or(Store::Snapshot(snapshot), Store::Txn),
            session,
            selection,
            ids: IdGenerator::seeded(stream),
            // A stream distinct from the identifiers'.
            random: stream ^ 0xA076_1D64_78BD_642F,
            cancel,
            progress,
            start: session.clock.now_unix_millis(),
            flags: NameFlags::default(),
            selection_after: None,
            selection_request: None,
        }
    }

    /// Sets the flags of the name the invocation was called by ([`NameFlags`]).
    pub(crate) fn with_flags(mut self, flags: NameFlags) -> Self {
        self.flags = flags;
        self
    }

    /// The flags of the name the invocation was called by: the ones a product's name resolver
    /// set, or the defaults for an identifier, an alias or a localized name. The crate does not
    /// interpret them.
    #[must_use]
    pub fn name_flags(&self) -> NameFlags {
        self.flags
    }

    /// The document state: the transaction's candidate, or the snapshot a query reads.
    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        match &self.store {
            Store::Txn(txn) => txn.snapshot(),
            Store::Snapshot(snapshot) => snapshot,
        }
    }

    /// The open transaction.
    ///
    /// # Errors
    ///
    /// `cmd.no_transaction` for commands that are not `undoable`.
    pub fn txn(&mut self) -> Result<&mut Txn> {
        match &mut self.store {
            Store::Txn(txn) => Ok(txn),
            Store::Snapshot(_) => Err(Error::new(codes::NO_TRANSACTION)),
        }
    }

    /// The selection at invocation.
    #[must_use]
    pub fn selection(&self) -> &[ObjectId] {
        self.selection
    }

    /// Sets the selection to restore after the command, recorded in the transaction.
    pub fn set_selection_after(&mut self, ids: Vec<ObjectId>) {
        self.selection_after = Some(ids);
    }

    /// Asks for `ids` to become the session's current selection, and the current one the
    /// previous selection, once the command succeeds (not on a dry run). It works for any command:
    /// a read-only one, or one with a transaction. The change is session state: it creates no
    /// journal record and no undo step, and the outcome reports it ([`crate::Outcome::selection`]).
    ///
    /// # Errors
    ///
    /// `cmd.params.limit_exceeded` (detail `limit`, `array_length`) when `ids` is longer than the
    /// session's limit; nothing is requested then.
    pub fn set_selection(&mut self, ids: Vec<ObjectId>) -> Result<()> {
        if u64::try_from(ids.len()).unwrap_or(u64::MAX) > self.session.limits.array_length {
            return Err(
                Error::new(codes::PARAMS_LIMIT_EXCEEDED).with_detail("limit", "array_length")
            );
        }
        self.selection_request = Some(ids);
        Ok(())
    }

    /// The session.
    #[must_use]
    pub fn session(&self) -> &Session {
        self.session
    }

    /// The limits of the session.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.session.limits
    }

    /// The next identifier of the invocation's generator, seeded from the invocation's seed and
    /// the revision of the snapshot the command runs on.
    pub fn new_id<I: Identifier>(&mut self) -> I {
        self.ids.generate()
    }

    /// The next 64 random bits of the invocation's stream, seeded like the identifiers.
    pub fn random_u64(&mut self) -> u64 {
        split_mix(&mut self.random)
    }

    /// The session clock's time.
    #[must_use]
    pub fn now_unix_millis(&self) -> u64 {
        self.session.clock.now_unix_millis()
    }

    /// Fails with `cmd.cancelled` once the invocation was cancelled; long-running commands call
    /// it between steps.
    ///
    /// # Errors
    ///
    /// `cmd.cancelled`.
    pub fn checkpoint(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::new(codes::CANCELLED))
        } else {
            Ok(())
        }
    }

    /// Reports progress. It reaches the sink only after [`PROGRESS_DELAY_MILLIS`] on the
    /// session clock since the command started; a non-finite fraction counts as 0, others are
    /// clamped to `[0, 1]`.
    pub fn progress(&self, fraction: f64, message_id: &str) {
        let Some(sink) = self.progress else { return };
        let elapsed = self.now_unix_millis().saturating_sub(self.start);
        if elapsed >= PROGRESS_DELAY_MILLIS {
            let fraction = if fraction.is_finite() {
                fraction.clamp(0.0, 1.0)
            } else {
                0.0
            };
            sink.progress(fraction, message_id);
        }
    }

    /// Opens the regular file at `path` for reading, inside the session's roots
    /// ([`Roots::open_read`]); the session needs the class `file_read` or `file_write`.
    ///
    /// # Errors
    ///
    /// `cmd.permission_denied` (detail `permission`) and the errors of [`Roots::open_read`].
    pub fn open_read(&self, path: &std::path::Path) -> Result<std::fs::File> {
        let granted = &self.session.permissions;
        if !granted.contains(&Permission::FileRead) && !granted.contains(&Permission::FileWrite) {
            return Err(Error::new(codes::PERMISSION_DENIED).with_detail("permission", "file_read"));
        }
        self.session.roots.open_read(path)
    }

    /// Creates or truncates the file at `path` for writing, inside the session's write roots
    /// ([`Roots::create_write`]); the session needs the class `file_write`.
    ///
    /// # Errors
    ///
    /// `cmd.permission_denied` (detail `permission`) and the errors of [`Roots::create_write`].
    pub fn create_write(&self, path: &std::path::Path) -> Result<std::fs::File> {
        if !self.session.permissions.contains(&Permission::FileWrite) {
            return Err(
                Error::new(codes::PERMISSION_DENIED).with_detail("permission", "file_write")
            );
        }
        self.session.roots.create_write(path)
    }

    /// Creates `path` exclusively inside the session's write roots and returns it open for
    /// writing, without truncation or a reopen by path ([`Roots::create_new`]); the session
    /// needs the class `file_write`. An existing name, a link included, is an error.
    ///
    /// # Errors
    ///
    /// `cmd.permission_denied` (detail `permission`) and the errors of [`Roots::create_new`].
    pub fn create_new(&self, path: &std::path::Path) -> Result<std::fs::File> {
        if !self.session.permissions.contains(&Permission::FileWrite) {
            return Err(
                Error::new(codes::PERMISSION_DENIED).with_detail("permission", "file_write")
            );
        }
        self.session.roots.create_new(path)
    }

    /// Replaces the file at `target` with the regular file at `temporary`, both in one
    /// directory inside the session's write roots ([`Roots::replace_file`]); the session needs
    /// the class `file_write`. Together with [`Context::create_new`] this is an atomic save.
    ///
    /// # Errors
    ///
    /// `cmd.permission_denied` (detail `permission`) and the errors of [`Roots::replace_file`].
    pub fn replace_file(
        &self,
        temporary: &std::path::Path,
        target: &std::path::Path,
    ) -> Result<()> {
        if !self.session.permissions.contains(&Permission::FileWrite) {
            return Err(
                Error::new(codes::PERMISSION_DENIED).with_detail("permission", "file_write")
            );
        }
        self.session.roots.replace_file(temporary, target)
    }

    /// The quantity of `param` in `dimension`. A number is taken in SI base units; a text is
    /// parsed by [`arc_solve::parse_quantity`] with `.` as the decimal separator and the
    /// session's context unit of `dimension` for bare numbers.
    ///
    /// # Errors
    ///
    /// The `solve.*` codes of the parser and evaluator; `cmd.params.invalid` for a
    /// non-finite number.
    pub fn quantity(&self, param: &QuantityParam, dimension: Dimension) -> Result<Quantity> {
        match param {
            QuantityParam::Si(value) if value.is_finite() => Ok(Quantity::new(*value, dimension)),
            QuantityParam::Si(_) => Err(Error::new(codes::PARAMS_INVALID)),
            QuantityParam::Text(text) => {
                let mut context = arc_solve::Context::new().with_cancel(self.cancel);
                if let Some(unit) = self.session.units.iter().find(|u| u.dimension == dimension) {
                    context = context.with_bare_unit(*unit);
                }
                arc_solve::parse_quantity(text, &context, '.', dimension)
            }
        }
    }
}
