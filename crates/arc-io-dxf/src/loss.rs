//! Loss reports (CORE-IO-004) of the DXF reader and writer.
//!
//! The types and the aggregator are shared through `arc_io_common::loss`; this module re-exports
//! them under the crate's own path. The reason codes stay in [`crate::error`]. The DXF log is
//! unbounded, so every distinct record kind of a file keeps its own item.

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

    /// The report of the DXF log before the types moved to `arc_io_common`, with the same records,
    /// so the serialized form (handles in hexadecimal) is pinned to the one the crate produced then.
    #[test]
    fn serialized_report_keeps_the_pre_move_shape() {
        let mut log = LossLog::unbounded();
        log.add_mapped(2);
        log.add(MAPPED, "ENTITY", Some(Source::Hex(0x1F)), 3);
        log.add(APPROXIMATED, "LINE", None, 1);
        log.add(APPROXIMATED, "LINE", Some(Source::Hex(255)), 2);
        log.add_with(
            DROPPED,
            "HATCH",
            Some(Source::Hex(16)),
            1,
            vec![
                ("name".to_owned(), Argument::Text("x".to_owned())),
                ("n".to_owned(), Argument::Int(7)),
            ],
        );
        log.add(DROPPED, "HATCH", Some(Source::Hex(32)), 4);
        log.add(PRESERVED, "", None, 2);
        log.add(FAILED, "BAD", Some(Source::Hex(1)), 1);
        log.add(DROPPED, "NONE", None, 0);
        let mut header = LossHeader::new(
            "dxf",
            "AC1015",
            Direction::Import,
            Some(arc_base::Limits::default()),
        );
        header.file_name = Some("a.dxf".to_owned());
        let report = log.finish(header);
        assert_eq!(
            serde_json::to_string(&report).ok().as_deref(),
            Some(PRE_MOVE_JSON)
        );
    }

    #[test]
    fn unbounded_log_keeps_every_record_kind() {
        let mut log = LossLog::unbounded();
        for i in 0..1100u64 {
            log.add(DROPPED, &format!("K{i:04}"), Some(Source::Hex(i)), 1);
        }
        let report = log.finish(LossHeader::new("dxf", "", Direction::Import, None));
        assert_eq!((report.items.len(), report.summary.dropped), (1100, 1100));
    }

    const PRE_MOVE_JSON: &str = r#"{"header":{"format":"dxf","version":"AC1015","direction":"import","file_name":"a.dxf","file_hash":null,"application":null,"application_version":null,"time":null,"limits":{"input_bytes":4294967296,"decompressed_bytes":17179869184,"decompression_ratio":200,"record_count":50000000,"nesting_depth":64,"string_bytes":16777216,"array_length":50000000,"memory_bytes":17179869184,"wall_clock_seconds":null}},"summary":{"mapped":5,"approximated":3,"preserved":2,"dropped":5,"failed":1},"items":[{"outcome":"approximated","severity":"warning","source_id":"FF","record_kind":"LINE","object_id":null,"reason":"t.approx","message_id":"m","arguments":{"count":3}},{"outcome":"dropped","severity":"error","source_id":"10","record_kind":"HATCH","object_id":null,"reason":"t.drop","message_id":"m","arguments":{"count":5,"n":7,"name":"x"}},{"outcome":"failed","severity":"error","source_id":"1","record_kind":"BAD","object_id":null,"reason":"t.fail","message_id":"m","arguments":{"count":1}},{"outcome":"mapped","severity":"info","source_id":"1F","record_kind":"ENTITY","object_id":null,"reason":"t.mapped","message_id":"m","arguments":{"count":3}},{"outcome":"preserved","severity":"info","source_id":null,"record_kind":null,"object_id":null,"reason":"t.pres","message_id":"m","arguments":{"count":2}}]}"#;
}
