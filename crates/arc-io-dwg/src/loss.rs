//! The loss log of the DWG reader.
//!
//! The types and the aggregator are shared through `arc_io_common::loss`. The DWG reader reports
//! with the DXF crate's types, which re-export the shared ones, so its items name the same types.
//! The log is bounded like the DXF log (`arc_io_common::loss::MAX_LOSS_ITEMS` distinct items), and
//! merges the property names of partial records.

pub(crate) use arc_io_common::loss::LossLog;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::reasons;
    use arc_io_common::loss::MAX_MERGED_NAMES;
    use arc_io_dxf::loss::{Argument, Direction, LossHeader, Outcome, Reason, Severity};

    fn header() -> LossHeader {
        LossHeader {
            format: "dwg".to_owned(),
            version: "R2018".to_owned(),
            direction: Direction::Import,
            file_name: None,
            file_hash: None,
            application: None,
            application_version: None,
            time: None,
            limits: None,
        }
    }

    const DROPPED: Reason = Reason {
        code: "t.drop",
        outcome: Outcome::Dropped,
        severity: Severity::Error,
        message_id: "m",
    };
    const APPROXIMATED: Reason = Reason {
        code: "t.approx",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "m",
    };
    const PARTIAL: Reason = Reason {
        code: "t.partial",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "m",
    };

    /// The report of the DWG log before the types moved to `arc_io_common`, with the same records,
    /// so the serialized form is pinned to the one the crate produced then.
    #[test]
    fn serialized_report_keeps_the_pre_move_shape() {
        let mut log = LossLog::bounded();
        log.add_mapped(3);
        log.add_with(
            DROPPED,
            "ENTITY",
            None,
            2,
            vec![("name".to_owned(), Argument::Text("x".to_owned()))],
        );
        log.add_with(
            DROPPED,
            "ENTITY",
            None,
            5,
            vec![("name".to_owned(), Argument::Text("y".to_owned()))],
        );
        log.add(APPROXIMATED, "", None, 1);
        log.add_partial(PARTIAL, "LWPOLYLINE", &["color"]);
        log.add_partial(PARTIAL, "LWPOLYLINE", &["transparency", "color"]);
        log.add_partial(PARTIAL, "HATCH", &[]);
        log.add(DROPPED, "none", None, 0);
        let report = log.finish(header());
        assert_eq!(
            serde_json::to_string(&report).ok().as_deref(),
            Some(PRE_MOVE_JSON)
        );
    }

    const PRE_MOVE_JSON: &str = r#"{"header":{"format":"dwg","version":"R2018","direction":"import","file_name":null,"file_hash":null,"application":null,"application_version":null,"time":null,"limits":null},"summary":{"mapped":3,"approximated":3,"preserved":0,"dropped":7,"failed":0},"items":[{"outcome":"approximated","severity":"warning","source_id":null,"record_kind":null,"object_id":null,"reason":"t.approx","message_id":"m","arguments":{"count":1}},{"outcome":"dropped","severity":"error","source_id":null,"record_kind":"ENTITY","object_id":null,"reason":"t.drop","message_id":"m","arguments":{"count":7,"name":"x"}},{"outcome":"approximated","severity":"warning","source_id":null,"record_kind":"LWPOLYLINE","object_id":null,"reason":"t.partial","message_id":"m","arguments":{"count":2,"properties":"color, transparency"}}],"truncated":false}"#;

    #[test]
    fn partial_names_of_a_kind_are_merged_and_bounded() {
        let mut log = LossLog::bounded();
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
        let mut log = LossLog::bounded();
        log.add_partial(reasons::ENTITY_PARTIAL, "HATCH", &NAMES);
        let report = log.finish(header());
        let Some(Argument::Text(text)) = report
            .items
            .first()
            .and_then(|i| i.arguments.get("properties"))
        else {
            panic!("the names are listed");
        };
        assert_eq!(text.split(", ").count(), MAX_MERGED_NAMES + 1);
        assert!(text.ends_with(", and more"));
    }
}
