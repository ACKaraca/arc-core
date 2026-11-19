//! Loss reports (CORE-IO-004): every read and write returns one, also when nothing was lost.
//!
//! The JSON form is described by `schemas/loss-report.schema.json`. Items are aggregated per
//! reason and record kind: the argument `count` says how many occurrences an item stands for and
//! `source_id` names the first one that has a source. The format crates re-export these types and
//! their aggregator; each crate owns its reason codes and catalog.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::Limits;
use serde::Serialize;

/// The most distinct items a bounded log keeps. Past it, a new record kind joins the overflow item
/// of its reason, so the report stays bounded and the counts stay exact.
pub const MAX_LOSS_ITEMS: usize = 1024;

/// The record kind of the overflow item of a reason. A file may name a record this way too; the
/// counts of the two then merge, which is harmless.
pub const OVERFLOW_KIND: &str = "(overflow)";

/// The most bytes of a record kind kept as a loss key. A kind comes from the file being read, so
/// a longer one is cut, on a character boundary, before it becomes a key or an item.
pub const MAX_KIND_BYTES: usize = 256;

/// `kind` cut to at most [`MAX_KIND_BYTES`] bytes, on a character boundary.
#[must_use]
pub fn clip_kind(kind: &str) -> &str {
    if kind.len() <= MAX_KIND_BYTES {
        return kind;
    }
    let mut end = MAX_KIND_BYTES;
    while end > 0 && !kind.is_char_boundary(end) {
        end -= 1;
    }
    kind.get(..end).unwrap_or_default()
}

/// The most property names one merged partial item lists; the names beyond are summarized as
/// "and more".
pub const MAX_MERGED_NAMES: usize = 32;

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

impl Outcome {
    /// The stable lowercase name used in the JSON form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mapped => "mapped",
            Self::Approximated => "approximated",
            Self::Preserved => "preserved",
            Self::Dropped => "dropped",
            Self::Failed => "failed",
        }
    }
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
    /// The source identifier of the first occurrence, if it has one.
    pub source_id: Option<String>,
    /// The record kind: an element, entity, section, table or property name.
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
    /// Records represented without change.
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

/// The header of a report. The format crate fills the format, version, direction and limits; the
/// caller fills file name, hash, application and time, which a format crate does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LossHeader {
    /// The format name, for example `dxf`.
    pub format: String,
    /// The format version read or written.
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

impl LossHeader {
    /// A header for a transfer of `format` (and `version`) in the given direction, with no
    /// caller-supplied fields.
    #[must_use]
    pub fn new(format: &str, version: &str, direction: Direction, limits: Option<Limits>) -> Self {
        Self {
            format: format.to_owned(),
            version: version.to_owned(),
            direction,
            file_name: None,
            file_hash: None,
            application: None,
            application_version: None,
            time: None,
            limits,
        }
    }
}

/// A loss report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LossReport {
    /// What was transferred.
    pub header: LossHeader,
    /// Counts by outcome. Exact, also when `truncated` is set.
    pub summary: LossSummary,
    /// The items, ordered by reason code and record kind.
    pub items: Vec<LossItem>,
    /// Whether some occurrences were folded into an overflow item, so the items do not name
    /// every record kind. The counts in `summary` and in the overflow items stay exact.
    pub truncated: bool,
}

impl LossReport {
    /// Whether any item says data was dropped or failed.
    #[must_use]
    pub fn has_loss(&self) -> bool {
        self.summary.dropped > 0 || self.summary.failed > 0
    }
}

/// The source identifier of an occurrence, with the form in which it is written to the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source<'a> {
    /// A number written in decimal (a line number, an instance name).
    Decimal(u64),
    /// A number written in uppercase hexadecimal (a DXF handle).
    Hex(u64),
    /// A text used as it is.
    Text(&'a str),
}

impl Source<'_> {
    fn into_text(self) -> String {
        match self {
            Self::Decimal(number) => number.to_string(),
            Self::Hex(number) => format!("{number:X}"),
            Self::Text(text) => text.to_owned(),
        }
    }
}

/// One aggregated item: reason, count, first source, arguments and merged property names.
#[derive(Debug)]
struct Entry {
    reason: Reason,
    count: u64,
    source: Option<String>,
    /// The arguments given with the first occurrence.
    arguments: Vec<(String, Argument)>,
    /// The property names merged over the occurrences of a partial record.
    names: BTreeSet<&'static str>,
    /// Whether names beyond [`MAX_MERGED_NAMES`] were seen.
    more_names: bool,
}

/// Collects items while reading or writing.
///
/// Items are keyed by reason code and then by record kind, so a lookup of an existing item borrows
/// the kind and allocates nothing. The log keeps at most [`MAX_LOSS_ITEMS`] distinct items; a
/// record kind beyond that is counted in the overflow item of its reason, and the report is marked
/// truncated. Every format crate uses this log, so no file can grow a report without bound.
#[derive(Debug, Default)]
pub struct LossLog {
    items: BTreeMap<&'static str, BTreeMap<String, Entry>>,
    distinct: usize,
    folded: bool,
    mapped: u64,
}

impl LossLog {
    /// An empty log.
    #[must_use]
    pub fn bounded() -> Self {
        Self::default()
    }

    /// Counts `count` records carried into the target without a loss item of their own.
    pub fn add_mapped(&mut self, count: u64) {
        self.mapped = self.mapped.saturating_add(count);
    }

    /// Adds `count` occurrences of `reason` for the record kind `kind`.
    pub fn add(&mut self, reason: Reason, kind: &str, source: Option<Source<'_>>, count: u64) {
        self.add_with(reason, kind, source, count, Vec::new());
    }

    /// Adds `count` occurrences of `reason` for the record kind `kind`, with arguments that the
    /// first occurrence of the item carries.
    pub fn add_with(
        &mut self,
        reason: Reason,
        kind: &str,
        source: Option<Source<'_>>,
        count: u64,
        arguments: Vec<(String, Argument)>,
    ) {
        if count == 0 {
            return;
        }
        if let Some(entry) = self.touch(reason, kind, arguments) {
            entry.count = entry.count.saturating_add(count);
            if entry.source.is_none() {
                entry.source = source.map(Source::into_text);
            }
        }
    }

    /// Adds one occurrence of a partial record: the record is carried, but the named properties
    /// are not. The names of all occurrences of the record kind are merged, up to
    /// [`MAX_MERGED_NAMES`], and listed in the `properties` argument.
    pub fn add_partial(&mut self, reason: Reason, kind: &str, names: &[&'static str]) {
        if names.is_empty() {
            return;
        }
        if let Some(entry) = self.touch(reason, kind, Vec::new()) {
            entry.count = entry.count.saturating_add(1);
            for &name in names {
                if entry.names.contains(name) {
                    continue;
                }
                if entry.names.len() < MAX_MERGED_NAMES {
                    entry.names.insert(name);
                } else {
                    entry.more_names = true;
                }
            }
        }
    }

    /// Adds `count` occurrences of an attribute-related reason; the attribute names the record kind
    /// and is also the `attribute` argument.
    pub fn attribute(&mut self, reason: Reason, attribute: &str, count: u64) {
        let arguments = vec![("attribute".to_owned(), Argument::Text(attribute.to_owned()))];
        self.add_with(reason, attribute, None, count, arguments);
    }

    /// The entry of a reason and record kind, created when it is new. Past the cap a new kind is
    /// filed under the overflow item of its reason. A lookup of an existing item allocates nothing.
    fn touch(
        &mut self,
        reason: Reason,
        kind: &str,
        arguments: Vec<(String, Argument)>,
    ) -> Option<&mut Entry> {
        let kind = clip_kind(kind);
        let known = self
            .items
            .get(reason.code)
            .is_some_and(|by_kind| by_kind.contains_key(kind));
        let within = known || self.distinct < MAX_LOSS_ITEMS;
        let (key, arguments) = if within {
            (kind, arguments)
        } else {
            // The overflow item keeps the arguments of the first occurrence folded into it.
            self.folded = true;
            (OVERFLOW_KIND, arguments)
        };
        let by_kind = self.items.entry(reason.code).or_default();
        if !by_kind.contains_key(key) {
            by_kind.insert(
                key.to_owned(),
                Entry {
                    reason,
                    count: 0,
                    source: None,
                    arguments,
                    names: BTreeSet::new(),
                    more_names: false,
                },
            );
            self.distinct = self.distinct.saturating_add(1);
        }
        by_kind.get_mut(key)
    }

    /// Finishes the report: summary counts and items, ordered by reason code and record kind.
    #[must_use]
    pub fn finish(self, header: LossHeader) -> LossReport {
        let mut summary = LossSummary {
            mapped: self.mapped,
            ..LossSummary::default()
        };
        let items = self
            .items
            .into_values()
            .flat_map(BTreeMap::into_iter)
            .map(|(kind, entry)| {
                let Entry {
                    reason,
                    count,
                    source,
                    arguments,
                    names,
                    more_names,
                } = entry;
                let slot = match reason.outcome {
                    Outcome::Mapped => &mut summary.mapped,
                    Outcome::Approximated => &mut summary.approximated,
                    Outcome::Preserved => &mut summary.preserved,
                    Outcome::Dropped => &mut summary.dropped,
                    Outcome::Failed => &mut summary.failed,
                };
                *slot = slot.saturating_add(count);
                let mut arguments: BTreeMap<String, Argument> = arguments.into_iter().collect();
                if !names.is_empty() {
                    let mut listed = names.into_iter().collect::<Vec<_>>().join(", ");
                    if more_names {
                        listed.push_str(", and more");
                    }
                    arguments.insert("properties".to_owned(), Argument::Text(listed));
                }
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
            header,
            summary,
            items,
            truncated: self.folded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The JSON of the sample report, captured from the OBJ copy before the move.
    const MESH_SAMPLE: &str = r#"{"header":{"format":"obj","version":"","direction":"import","file_name":"a.obj","file_hash":null,"application":null,"application_version":null,"time":null,"limits":{"input_bytes":4294967296,"decompressed_bytes":17179869184,"decompression_ratio":200,"record_count":50000000,"nesting_depth":64,"string_bytes":16777216,"array_length":50000000,"memory_bytes":17179869184,"wall_clock_seconds":null}},"summary":{"mapped":5,"approximated":3,"preserved":2,"dropped":6,"failed":1},"items":[{"outcome":"approximated","severity":"warning","source_id":"9","record_kind":"normal","object_id":null,"reason":"t.approx","message_id":"m","arguments":{"count":3}},{"outcome":"dropped","severity":"error","source_id":null,"record_kind":"color","object_id":null,"reason":"t.drop","message_id":"m","arguments":{"attribute":"color","count":5}},{"outcome":"dropped","severity":"error","source_id":"4","record_kind":"usemtl","object_id":null,"reason":"t.drop","message_id":"m","arguments":{"count":1,"n":7,"name":"x"}},{"outcome":"failed","severity":"error","source_id":"1","record_kind":"bad","object_id":null,"reason":"t.fail","message_id":"m","arguments":{"count":1}},{"outcome":"mapped","severity":"info","source_id":"12","record_kind":"element","object_id":null,"reason":"t.mapped","message_id":"m","arguments":{"count":3}},{"outcome":"preserved","severity":"info","source_id":null,"record_kind":null,"object_id":null,"reason":"t.pres","message_id":"m","arguments":{"count":2}}],"truncated":false}"#;

    const MAPPED: Reason = Reason {
        code: "t.mapped",
        outcome: Outcome::Mapped,
        severity: Severity::Info,
        message_id: "m",
    };
    const APPROXIMATED: Reason = Reason {
        code: "t.approx",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "m",
    };
    const DROPPED: Reason = Reason {
        code: "t.drop",
        outcome: Outcome::Dropped,
        severity: Severity::Error,
        message_id: "m",
    };

    fn header(file_name: &str) -> LossHeader {
        let mut header = LossHeader::new("obj", "", Direction::Import, Some(Limits::default()));
        header.file_name = Some(file_name.to_owned());
        header
    }

    /// The report of the OBJ reader before the types moved here, so the shape is pinned.
    #[test]
    fn serialized_report_keeps_the_shape_of_the_format_copies() {
        let mut log = LossLog::default();
        log.add_mapped(2);
        log.add(MAPPED, "element", Some(Source::Decimal(12)), 3);
        log.add(APPROXIMATED, "normal", None, 1);
        log.add(APPROXIMATED, "normal", Some(Source::Decimal(9)), 2);
        log.attribute(DROPPED, "color", 5);
        log.add_with(
            DROPPED,
            "usemtl",
            Some(Source::Decimal(4)),
            1,
            vec![
                ("name".to_owned(), Argument::Text("x".to_owned())),
                ("n".to_owned(), Argument::Int(7)),
            ],
        );
        log.add(
            Reason {
                code: "t.pres",
                outcome: Outcome::Preserved,
                severity: Severity::Info,
                message_id: "m",
            },
            "",
            None,
            2,
        );
        log.add(
            Reason {
                code: "t.fail",
                outcome: Outcome::Failed,
                severity: Severity::Error,
                message_id: "m",
            },
            "bad",
            Some(Source::Decimal(1)),
            1,
        );
        log.add(DROPPED, "none", None, 0);
        let report = log.finish(header("a.obj"));
        assert_eq!(
            serde_json::to_string(&report).ok().as_deref(),
            Some(MESH_SAMPLE)
        );
    }

    #[test]
    fn hex_and_text_sources_keep_their_written_form() {
        let mut log = LossLog::default();
        log.add(DROPPED, "ENTITY", Some(Source::Hex(0x1F)), 1);
        log.add(DROPPED, "ENTITY", Some(Source::Decimal(9)), 1);
        log.add(DROPPED, "glyphs", Some(Source::Text("obj-7")), 1);
        let report = log.finish(header("a.dxf"));
        let sources: Vec<_> = report
            .items
            .iter()
            .map(|i| i.source_id.as_deref())
            .collect();
        assert_eq!(sources, [Some("1F"), Some("obj-7")]);
    }

    #[test]
    fn bounded_log_folds_new_kinds_into_one_overflow_item_per_reason() {
        let mut log = LossLog::bounded();
        for i in 0..1100u64 {
            log.add(DROPPED, &format!("K{i:04}"), Some(Source::Decimal(i)), 1);
        }
        let report = log.finish(header("a.obj"));
        assert_eq!(report.items.len(), MAX_LOSS_ITEMS + 1);
        assert_eq!(report.summary.dropped, 1100);
        let overflow = report
            .items
            .iter()
            .find(|i| i.record_kind.as_deref() == Some(OVERFLOW_KIND));
        assert_eq!(
            overflow.and_then(|i| i.arguments.get("count")),
            Some(&Argument::Int(76))
        );
        assert_eq!(overflow.and_then(|i| i.source_id.as_deref()), Some("1024"));
    }

    #[test]
    fn report_is_not_truncated_below_the_bound() {
        let mut log = LossLog::bounded();
        for i in 0..MAX_LOSS_ITEMS as u64 {
            log.add(DROPPED, &format!("K{i:04}"), None, 1);
        }
        let report = log.finish(header("a.obj"));
        assert!(!report.truncated);
        assert_eq!(report.items.len(), MAX_LOSS_ITEMS);
    }

    #[test]
    fn truncated_report_keeps_exact_counts_for_known_kinds() {
        let mut log = LossLog::bounded();
        for i in 0..MAX_LOSS_ITEMS as u64 {
            log.add(DROPPED, &format!("K{i:04}"), None, 1);
        }
        // A kind already in the log keeps its own item even after the bound is reached.
        log.add(DROPPED, "K0000", None, 2);
        log.add(DROPPED, "late", None, 5);
        let report = log.finish(header("a.obj"));
        assert!(report.truncated);
        assert_eq!(report.summary.dropped, MAX_LOSS_ITEMS as u64 + 7);
        assert_eq!(report.items.len(), MAX_LOSS_ITEMS + 1);
    }

    #[test]
    fn overflow_item_keeps_the_arguments_of_the_first_folded_occurrence() {
        let mut log = LossLog::bounded();
        for i in 0..MAX_LOSS_ITEMS as u64 {
            log.add(DROPPED, &format!("K{i:04}"), None, 1);
        }
        log.add_with(
            DROPPED,
            "late",
            None,
            1,
            vec![("limit".to_owned(), Argument::Text("max_items".to_owned()))],
        );
        log.add_with(
            DROPPED,
            "later",
            None,
            1,
            vec![("limit".to_owned(), Argument::Text("other".to_owned()))],
        );
        let report = log.finish(header("a.obj"));
        let overflow = report
            .items
            .iter()
            .find(|i| i.record_kind.as_deref() == Some(OVERFLOW_KIND));
        assert_eq!(
            overflow.and_then(|i| i.arguments.get("limit")),
            Some(&Argument::Text("max_items".to_owned()))
        );
        assert_eq!(
            overflow.and_then(|i| i.arguments.get("count")),
            Some(&Argument::Int(2))
        );
    }

    #[test]
    fn a_long_record_kind_is_cut_on_a_character_boundary() {
        let long = "a".repeat(MAX_KIND_BYTES - 1) + "\u{e9}tail";
        let clipped = clip_kind(&long);
        assert_eq!(clipped.len(), MAX_KIND_BYTES - 1);
        assert_eq!(clip_kind("short"), "short");
        let mut log = LossLog::bounded();
        log.add(DROPPED, &long, None, 1);
        let report = log.finish(header("a.obj"));
        assert_eq!(report.items[0].record_kind.as_deref(), Some(clipped));
    }
}
