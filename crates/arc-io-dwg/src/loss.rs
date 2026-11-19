//! The loss report of a read: items aggregated per reason and record kind, in the form of
//! `schemas/loss-report.schema.json`. The aggregation mirrors the one of `arc-io-dxf`, whose log is
//! not public.

use std::collections::BTreeMap;

use arc_io_dxf::loss::{Argument, LossHeader, LossItem, LossReport, LossSummary, Outcome, Reason};

/// One aggregated item: reason, count and arguments.
type Entry = (Reason, u64, Vec<(String, Argument)>);

/// Collects items while mapping.
#[derive(Default)]
pub(crate) struct LossLog {
    items: BTreeMap<(&'static str, String), Entry>,
    /// Records carried into the drawing model.
    pub(crate) mapped: u64,
}

impl LossLog {
    /// Adds `count` occurrences of a reason for a record kind. The first arguments given are kept.
    pub(crate) fn add(
        &mut self,
        reason: Reason,
        kind: &str,
        count: u64,
        arguments: Vec<(String, Argument)>,
    ) {
        if count == 0 {
            return;
        }
        let entry = self
            .items
            .entry((reason.code, kind.to_owned()))
            .or_insert((reason, 0, arguments));
        entry.1 = entry.1.saturating_add(count);
    }

    pub(crate) fn finish(self, header: LossHeader) -> LossReport {
        let mut summary = LossSummary {
            mapped: self.mapped,
            ..LossSummary::default()
        };
        let items = self
            .items
            .into_iter()
            .map(|((_, kind), (reason, count, arguments))| {
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
                    source_id: None,
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
        }
    }
}
