//! Loss reports (CORE-IO-004): every write returns one, also when nothing was lost.
//!
//! The shape is that of `arc-io-dxf::loss`, written against the same published schema
//! `schemas/loss-report.schema.json`; the types are duplicated because the layer table does not
//! let one format crate depend on another. Items are aggregated per reason and record kind: the
//! argument `count` says how many occurrences an item stands for.

use std::collections::BTreeMap;

use arc_base::Limits;
use serde::Serialize;

/// What happened to a piece of data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Represented in the target without change.
    Mapped,
    /// Represented with a stated approximation.
    Approximated,
    /// Kept as preserved data that the same crate writes back.
    Preserved,
    /// Not represented.
    Dropped,
    /// Could not be processed.
    Failed,
}

/// How important an item is to a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// For information.
    Info,
    /// Data changed or may not be what the user expects.
    Warning,
    /// Data is missing.
    Error,
}

/// The direction of the transfer a report describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Reading a file into the neutral model.
    Import,
    /// Writing the neutral model into a file.
    Export,
}

/// A registered loss reason: its stable code, outcome, severity and message identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reason {
    /// The stable dotted reason code.
    pub code: &'static str,
    /// The outcome of every item with this reason.
    pub outcome: Outcome,
    /// The severity of every item with this reason.
    pub severity: Severity,
    /// The Fluent message identifier that describes the item.
    pub message_id: &'static str,
}

/// A message argument of a loss item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Argument {
    /// A whole number.
    Int(i64),
    /// A text.
    Text(String),
}

/// One loss item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LossItem {
    /// What happened.
    pub outcome: Outcome,
    /// How important it is.
    pub severity: Severity,
    /// The source identifier of the first occurrence: the plot item's source object.
    pub source_id: Option<String>,
    /// The record kind: the plot item kind (`stroke`, `fill`, `glyphs`, ...).
    pub record_kind: Option<String>,
    /// The Arc object identifier; format crates leave it empty and products fill it.
    pub object_id: Option<String>,
    /// The stable reason code.
    pub reason: String,
    /// The Fluent message identifier.
    pub message_id: String,
    /// The message arguments; `count` is always present.
    pub arguments: BTreeMap<String, Argument>,
}

/// Counts by outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct LossSummary {
    /// Occurrences represented without change.
    pub mapped: u64,
    /// Occurrences represented with an approximation.
    pub approximated: u64,
    /// Occurrences kept as preserved data.
    pub preserved: u64,
    /// Occurrences not represented.
    pub dropped: u64,
    /// Occurrences that could not be processed.
    pub failed: u64,
}

/// The header of a report. The crate fills the format, version, direction and limits; the
/// caller fills file name, hash, application and time, which a format crate does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LossHeader {
    /// The format name, `svg` or `pdf`.
    pub format: String,
    /// The format version written: `1.1` for SVG, `1.7` for PDF.
    pub version: String,
    /// Import or export.
    pub direction: Direction,
    /// The file name, if the caller sets it.
    pub file_name: Option<String>,
    /// The file hash, if the caller sets it.
    pub file_hash: Option<String>,
    /// The application name, if the caller sets it.
    pub application: Option<String>,
    /// The application version, if the caller sets it.
    pub application_version: Option<String>,
    /// The time of the transfer, if the caller sets it.
    pub time: Option<String>,
    /// The limits profile of an import; empty for an export.
    pub limits: Option<Limits>,
}

/// A loss report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LossReport {
    /// What was transferred.
    pub header: LossHeader,
    /// Counts by outcome.
    pub summary: LossSummary,
    /// The items, ordered by reason code and record kind.
    pub items: Vec<LossItem>,
}

impl LossReport {
    /// Whether any item says data was dropped or failed.
    #[must_use]
    pub fn has_loss(&self) -> bool {
        self.summary.dropped > 0 || self.summary.failed > 0
    }
}

/// One aggregated item: reason, count, first source and arguments.
type Entry = (Reason, u64, Option<String>, Vec<(String, Argument)>);

/// Collects items while writing.
#[derive(Debug, Default)]
pub(crate) struct LossLog {
    items: BTreeMap<(&'static str, String), Entry>,
}

impl LossLog {
    pub(crate) fn add(&mut self, reason: Reason, kind: &str, source: Option<String>, count: u64) {
        self.add_with(reason, kind, source, count, Vec::new());
    }

    pub(crate) fn add_with(
        &mut self,
        reason: Reason,
        kind: &str,
        source: Option<String>,
        count: u64,
        arguments: Vec<(String, Argument)>,
    ) {
        if count == 0 {
            return;
        }
        let entry = self
            .items
            .entry((reason.code, kind.to_owned()))
            .or_insert((reason, 0, None, arguments));
        entry.1 = entry.1.saturating_add(count);
        if entry.2.is_none() {
            entry.2 = source;
        }
    }

    pub(crate) fn finish(self, format: &str, version: &str) -> LossReport {
        let mut summary = LossSummary::default();
        let items = self
            .items
            .into_iter()
            .map(|((_, kind), (reason, count, source, arguments))| {
                let slot = match reason.outcome {
                    Outcome::Mapped => &mut summary.mapped,
                    Outcome::Approximated => &mut summary.approximated,
                    Outcome::Preserved => &mut summary.preserved,
                    Outcome::Dropped => &mut summary.dropped,
                    Outcome::Failed => &mut summary.failed,
                };
                *slot = slot.saturating_add(count);
                let mut arguments: BTreeMap<String, Argument> = arguments.into_iter().collect();
                arguments.insert(
                    "count".to_owned(),
                    Argument::Int(i64::try_from(count).unwrap_or(i64::MAX)),
                );
                LossItem {
                    outcome: reason.outcome,
                    severity: reason.severity,
                    source_id: source,
                    record_kind: (!kind.is_empty()).then_some(kind),
                    object_id: None,
                    reason: reason.code.to_owned(),
                    message_id: reason.message_id.to_owned(),
                    arguments,
                }
            })
            .collect();
        LossReport {
            header: LossHeader {
                format: format.to_owned(),
                version: version.to_owned(),
                direction: Direction::Export,
                file_name: None,
                file_hash: None,
                application: None,
                application_version: None,
                time: None,
                limits: None,
            },
            summary,
            items,
        }
    }
}
