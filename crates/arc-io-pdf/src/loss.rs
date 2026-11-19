//! Loss reports (CORE-IO-004) of the SVG and PDF writers.
//!
//! The types and the aggregator are shared through `arc_io_common::loss`; this module re-exports
//! them under the crate's own path. The reason codes stay in [`crate::error`]. The PDF log is
//! unbounded, so every distinct plot item kind of a stream keeps its own item.

pub use arc_io_common::loss::{
    Argument, Direction, LossHeader, LossItem, LossReport, LossSummary, Outcome, Reason, Severity,
};
pub(crate) use arc_io_common::loss::{LossLog, Source};

#[cfg(test)]
mod tests {
    use super::*;

    const MAPPED: Reason = Reason {
        code: "t.mapped",
        outcome: Outcome::Mapped,
        severity: Severity::Info,
        message_id: "m",
    };
    const DROPPED: Reason = Reason {
        code: "t.drop",
        outcome: Outcome::Dropped,
        severity: Severity::Error,
        message_id: "m",
    };
    const PRESERVED: Reason = Reason {
        code: "t.pres",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "m",
    };
    const FAILED: Reason = Reason {
        code: "t.fail",
        outcome: Outcome::Failed,
        severity: Severity::Error,
        message_id: "m",
    };

    /// The report of the PDF log before the types moved to `arc_io_common`, with the same records,
    /// so the serialized form is pinned to the one the crate produced then.
    #[test]
    fn serialized_report_keeps_the_pre_move_shape() {
        let mut log = LossLog::unbounded();
        log.add(MAPPED, "stroke", Some(Source::Text("42")), 3);
        log.add(DROPPED, "glyphs", None, 1);
        log.add_with(
            DROPPED,
            "glyphs",
            Some(Source::Text("obj-7")),
            2,
            vec![
                ("name".to_owned(), Argument::Text("x".to_owned())),
                ("n".to_owned(), Argument::Int(7)),
            ],
        );
        log.add(PRESERVED, "", None, 2);
        log.add(FAILED, "bad", Some(Source::Text("z")), 1);
        log.add(DROPPED, "none", None, 0);
        let report = log.finish(LossHeader::new("pdf", "1.7", Direction::Export, None));
        assert_eq!(
            serde_json::to_string(&report).ok().as_deref(),
            Some(PRE_MOVE_JSON)
        );
    }

    #[test]
    fn unbounded_log_keeps_every_record_kind() {
        let mut log = LossLog::unbounded();
        for i in 0..1100u64 {
            log.add(
                DROPPED,
                &format!("K{i:04}"),
                Some(Source::Text(&i.to_string())),
                1,
            );
        }
        let report = log.finish(LossHeader::new("pdf", "1.7", Direction::Export, None));
        assert_eq!((report.items.len(), report.summary.dropped), (1100, 1100));
    }

    const PRE_MOVE_JSON: &str = r#"{"header":{"format":"pdf","version":"1.7","direction":"export","file_name":null,"file_hash":null,"application":null,"application_version":null,"time":null,"limits":null},"summary":{"mapped":3,"approximated":0,"preserved":2,"dropped":3,"failed":1},"items":[{"outcome":"dropped","severity":"error","source_id":"obj-7","record_kind":"glyphs","object_id":null,"reason":"t.drop","message_id":"m","arguments":{"count":3}},{"outcome":"failed","severity":"error","source_id":"z","record_kind":"bad","object_id":null,"reason":"t.fail","message_id":"m","arguments":{"count":1}},{"outcome":"mapped","severity":"info","source_id":"42","record_kind":"stroke","object_id":null,"reason":"t.mapped","message_id":"m","arguments":{"count":3}},{"outcome":"preserved","severity":"info","source_id":null,"record_kind":null,"object_id":null,"reason":"t.pres","message_id":"m","arguments":{"count":2}}]}"#;
}
