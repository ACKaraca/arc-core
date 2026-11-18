//! The loss report of a mapping: the items of the format crate plus the items of the mapping,
//! one per occurrence up to a bound and aggregated per reason and kind beyond it.

use std::collections::BTreeMap;

use arc_io_dxf::loss::Reason;
use arc_io_dxf::loss::{
    Argument, Direction, LossHeader, LossItem, LossReport, LossSummary, Outcome,
};

/// The most items of one report that name a single occurrence; further occurrences of a reason
/// and kind are counted in one aggregated item.
pub const MAX_ITEMS: usize = 4096;

/// Collects the items of one import or export.
#[derive(Debug, Default)]
pub struct Reporter {
    items: Vec<LossItem>,
    overflow: BTreeMap<(&'static str, String), (Reason, u64, Option<String>)>,
    summary: LossSummary,
    mapped: u64,
}

impl Reporter {
    /// An empty reporter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Notes one occurrence of `reason` on a record of `kind` with the source identifier
    /// `source` (a DXF handle in hexadecimal) and message arguments.
    pub fn note(
        &mut self,
        reason: Reason,
        kind: &str,
        source: Option<String>,
        arguments: &[(&str, Argument)],
    ) {
        self.note_n(reason, kind, source, 1, arguments);
    }

    /// Notes `count` occurrences at once as one item.
    pub fn note_n(
        &mut self,
        reason: Reason,
        kind: &str,
        source: Option<String>,
        count: u64,
        arguments: &[(&str, Argument)],
    ) {
        if self.items.len() >= MAX_ITEMS {
            let entry = self
                .overflow
                .entry((reason.code, kind.to_owned()))
                .or_insert((reason, 0, source));
            entry.1 = entry.1.saturating_add(count);
            return;
        }
        let mut args: BTreeMap<String, Argument> = arguments
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();
        args.insert(
            "count".to_owned(),
            Argument::Int(i64::try_from(count).unwrap_or(i64::MAX)),
        );
        args.insert("kind".to_owned(), Argument::Text(kind.to_owned()));
        self.items.push(LossItem {
            outcome: reason.outcome,
            severity: reason.severity,
            source_id: source,
            record_kind: (!kind.is_empty()).then(|| kind.to_owned()),
            object_id: None,
            reason: reason.code.to_owned(),
            message_id: reason.message_id.to_owned(),
            arguments: args,
        });
    }

    /// Counts `n` records that were mapped without loss.
    pub fn mapped(&mut self, n: u64) {
        self.mapped = self.mapped.saturating_add(n);
    }

    /// Adds the items and counts of a report from the format crate.
    pub fn absorb(&mut self, report: &LossReport) {
        self.items.extend(report.items.iter().cloned());
        let s = &report.summary;
        self.summary.mapped = self.summary.mapped.saturating_add(s.mapped);
        self.summary.approximated = self.summary.approximated.saturating_add(s.approximated);
        self.summary.preserved = self.summary.preserved.saturating_add(s.preserved);
        self.summary.dropped = self.summary.dropped.saturating_add(s.dropped);
        self.summary.failed = self.summary.failed.saturating_add(s.failed);
    }

    /// The report: aggregated items of the overflow, then the summary over every item.
    #[must_use]
    pub fn finish(mut self, format_version: &str, direction: Direction) -> LossReport {
        for ((_, kind), (reason, count, source)) in std::mem::take(&mut self.overflow) {
            let mut arguments = BTreeMap::new();
            arguments.insert(
                "count".to_owned(),
                Argument::Int(i64::try_from(count).unwrap_or(i64::MAX)),
            );
            arguments.insert("kind".to_owned(), Argument::Text(kind.clone()));
            self.items.push(LossItem {
                outcome: reason.outcome,
                severity: reason.severity,
                source_id: source,
                record_kind: (!kind.is_empty()).then_some(kind),
                object_id: None,
                reason: reason.code.to_owned(),
                message_id: reason.message_id.to_owned(),
                arguments,
            });
        }
        let mut summary = self.summary;
        summary.mapped = summary.mapped.saturating_add(self.mapped);
        for item in &self.items {
            // Items absorbed from the format crate are counted in the absorbed summary.
            if item.reason.starts_with("draft.dxf.") {
                let count = match item.arguments.get("count") {
                    Some(Argument::Int(n)) => u64::try_from(*n).unwrap_or(0),
                    _ => 1,
                };
                let slot = match item.outcome {
                    Outcome::Mapped => &mut summary.mapped,
                    Outcome::Approximated => &mut summary.approximated,
                    Outcome::Preserved => &mut summary.preserved,
                    Outcome::Dropped => &mut summary.dropped,
                    Outcome::Failed => &mut summary.failed,
                };
                *slot = slot.saturating_add(count);
            }
        }
        LossReport {
            header: LossHeader {
                format: "dxf".to_owned(),
                version: format_version.to_owned(),
                direction,
                file_name: None,
                file_hash: None,
                application: None,
                application_version: None,
                time: None,
                limits: None,
            },
            summary,
            items: self.items,
        }
    }
}
