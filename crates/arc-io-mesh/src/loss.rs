//! Loss reports (CORE-IO-004) of the mesh readers and writers.
//!
//! The types and the aggregator are shared through `arc_io_common::loss`; this module re-exports
//! them under the crate's own path. The reason codes stay in [`crate::error`].

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

    /// The report of the OBJ reader's log before the types moved to `arc_io_common`, with the same
    /// records, so the serialized form is pinned to the one the crate produced then.
    #[test]
    fn serialized_report_keeps_the_pre_move_shape() {
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
        log.add(PRESERVED, "", None, 2);
        log.add(FAILED, "bad", Some(Source::Decimal(1)), 1);
        log.add(DROPPED, "none", None, 0);
        let mut header = LossHeader::new(
            "obj",
            "",
            Direction::Import,
            Some(arc_base::Limits::default()),
        );
        header.file_name = Some("a.obj".to_owned());
        let report = log.finish(header);
        assert_eq!(
            serde_json::to_string(&report).ok().as_deref(),
            Some(PRE_MOVE_JSON)
        );
    }

    const PRE_MOVE_JSON: &str = r#"{"header":{"format":"obj","version":"","direction":"import","file_name":"a.obj","file_hash":null,"application":null,"application_version":null,"time":null,"limits":{"input_bytes":4294967296,"decompressed_bytes":17179869184,"decompression_ratio":200,"record_count":50000000,"nesting_depth":64,"string_bytes":16777216,"array_length":50000000,"memory_bytes":17179869184,"wall_clock_seconds":null}},"summary":{"mapped":5,"approximated":3,"preserved":2,"dropped":6,"failed":1},"items":[{"outcome":"approximated","severity":"warning","source_id":"9","record_kind":"normal","object_id":null,"reason":"t.approx","message_id":"m","arguments":{"count":3}},{"outcome":"dropped","severity":"error","source_id":null,"record_kind":"color","object_id":null,"reason":"t.drop","message_id":"m","arguments":{"attribute":"color","count":5}},{"outcome":"dropped","severity":"error","source_id":"4","record_kind":"usemtl","object_id":null,"reason":"t.drop","message_id":"m","arguments":{"count":1,"n":7,"name":"x"}},{"outcome":"failed","severity":"error","source_id":"1","record_kind":"bad","object_id":null,"reason":"t.fail","message_id":"m","arguments":{"count":1}},{"outcome":"mapped","severity":"info","source_id":"12","record_kind":"element","object_id":null,"reason":"t.mapped","message_id":"m","arguments":{"count":3}},{"outcome":"preserved","severity":"info","source_id":null,"record_kind":null,"object_id":null,"reason":"t.pres","message_id":"m","arguments":{"count":2}}]}"#;
}
