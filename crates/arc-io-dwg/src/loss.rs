//! The loss report of a read: items aggregated per reason and record kind, in the form of
//! `schemas/loss-report.schema.json`. The aggregation mirrors the one of `arc-io-dxf`, whose log is
//! not public.

use std::collections::{BTreeMap, BTreeSet};

use arc_io_dxf::loss::{Argument, LossHeader, LossItem, LossReport, LossSummary, Outcome, Reason};

/// The most property names one partial item lists; the names beyond are summarized as "and more".
const MAX_NAMES: usize = 32;

/// One aggregated item.
struct Entry {
    reason: Reason,
    count: u64,
    /// The arguments given with the first occurrence.
    arguments: Vec<(String, Argument)>,
    /// The names of the data not carried, merged over the occurrences of the group.
    names: BTreeSet<&'static str>,
    /// Whether names beyond [`MAX_NAMES`] were seen.
    more: bool,
}

/// Collects items while mapping.
#[derive(Default)]
pub(crate) struct LossLog {
    items: BTreeMap<(&'static str, String), Entry>,
    /// Records carried into the drawing model.
    pub(crate) mapped: u64,
}

impl LossLog {
    fn entry(&mut self, reason: Reason, kind: &str) -> &mut Entry {
        self.items
            .entry((reason.code, kind.to_owned()))
            .or_insert_with(|| Entry {
                reason,
                count: 0,
                arguments: Vec::new(),
                names: BTreeSet::new(),
                more: false,
            })
    }

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
        let entry = self.entry(reason, kind);
        if entry.count == 0 {
            entry.arguments = arguments;
        }
        entry.count = entry.count.saturating_add(count);
    }

    /// Adds one occurrence of a partial record, with the names of the data it does not carry.
    /// The names of all occurrences of the record kind are merged.
    pub(crate) fn add_partial(&mut self, reason: Reason, kind: &str, names: &[&'static str]) {
        if names.is_empty() {
            return;
        }
        let entry = self.entry(reason, kind);
        entry.count = entry.count.saturating_add(1);
        for &name in names {
            if entry.names.contains(name) {
                continue;
            }
            if entry.names.len() < MAX_NAMES {
                entry.names.insert(name);
            } else {
                entry.more = true;
            }
        }
    }

    pub(crate) fn finish(self, header: LossHeader) -> LossReport {
        let mut summary = LossSummary {
            mapped: self.mapped,
            ..LossSummary::default()
        };
        let items = self
            .items
            .into_iter()
            .map(|((_, kind), entry)| {
                let count = entry.count;
                let slot = match entry.reason.outcome {
                    Outcome::Mapped => &mut summary.mapped,
                    Outcome::Approximated => &mut summary.approximated,
                    Outcome::Preserved => &mut summary.preserved,
                    Outcome::Dropped => &mut summary.dropped,
                    Outcome::Failed => &mut summary.failed,
                };
                *slot = slot.saturating_add(count);
                let mut arguments: BTreeMap<String, Argument> =
                    entry.arguments.into_iter().collect();
                if !entry.names.is_empty() {
                    let mut names = entry.names.into_iter().collect::<Vec<_>>().join(", ");
                    if entry.more {
                        names.push_str(", and more");
                    }
                    arguments.insert("properties".to_owned(), Argument::Text(names));
                }
                arguments.insert(
                    "count".to_owned(),
                    Argument::Int(i64::try_from(count).unwrap_or(i64::MAX)),
                );
                LossItem {
                    outcome: entry.reason.outcome,
                    severity: entry.reason.severity,
                    source_id: None,
                    record_kind: (!kind.is_empty()).then_some(kind),
                    object_id: None,
                    reason: entry.reason.code.to_owned(),
                    message_id: entry.reason.message_id.to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::reasons;

    fn header() -> LossHeader {
        LossHeader {
            format: "dwg".to_owned(),
            version: "R2018".to_owned(),
            direction: arc_io_dxf::loss::Direction::Import,
            file_name: None,
            file_hash: None,
            application: None,
            application_version: None,
            time: None,
            limits: None,
        }
    }

    #[test]
    fn partial_names_of_a_kind_are_merged_and_bounded() {
        let mut log = LossLog::default();
        log.add_partial(reasons::ENTITY_PARTIAL, "LWPOLYLINE", &["color"]);
        log.add_partial(
            reasons::ENTITY_PARTIAL,
            "LWPOLYLINE",
            &["transparency", "color"],
        );
        let report = log.finish(header());
        let item = report.items.first();
        let properties = item.and_then(|i| i.arguments.get("properties"));
        assert_eq!(
            properties,
            Some(&Argument::Text("color, transparency".to_owned()))
        );
        assert_eq!(
            item.map(|i| i.arguments.get("count").cloned()),
            Some(Some(Argument::Int(2)))
        );
    }

    #[test]
    fn more_than_the_bound_of_names_is_summarized() {
        const NAMES: [&str; 40] = [
            "n00", "n01", "n02", "n03", "n04", "n05", "n06", "n07", "n08", "n09", "n10", "n11",
            "n12", "n13", "n14", "n15", "n16", "n17", "n18", "n19", "n20", "n21", "n22", "n23",
            "n24", "n25", "n26", "n27", "n28", "n29", "n30", "n31", "n32", "n33", "n34", "n35",
            "n36", "n37", "n38", "n39",
        ];
        let mut log = LossLog::default();
        log.add_partial(reasons::ENTITY_PARTIAL, "HATCH", &NAMES);
        let report = log.finish(header());
        let Some(Argument::Text(text)) = report
            .items
            .first()
            .and_then(|i| i.arguments.get("properties"))
        else {
            panic!("the names are listed");
        };
        assert_eq!(text.split(", ").count(), MAX_NAMES + 1);
        assert!(text.ends_with(", and more"));
    }
}
