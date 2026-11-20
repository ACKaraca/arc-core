//! A silent record made through the invoker is journaled and rebuilt by crash recovery.

#![cfg(feature = "headless")]
#![allow(clippy::unwrap_used, reason = "tests")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use arc_base::id::Identifier;
use arc_base::{DocumentId, SessionId};
use arc_command::{CommandSpec, Context, Registry};
use arc_doc::StoredForeign;
use arc_doc::graph::Evaluators;
use arc_doc::package::recovery::{RecoverOptions, RecoveryJournal, Session, recover, scan};
use common::{document, invoke, registry, session, types};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Note {}

fn note(ctx: &mut Context<'_>, _: Note) -> arc_base::Result<()> {
    ctx.commit_silently()?;
    ctx.txn()?
        .add_foreign(StoredForeign {
            source_format: "test".to_owned(),
            record_kind: "note".to_owned(),
            ..StoredForeign::default()
        })
        .map(drop)
}

#[test]
fn a_silent_record_from_the_invoker_is_recovered_without_a_step() {
    let root = std::env::temp_dir().join(format!("arc-silent-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut r: Registry = registry();
    r.register(CommandSpec::new("test.row.note", "l", "d", note))
        .unwrap();
    let (doc, s) = (document(), session());
    let journal = RecoveryJournal::start(Session::new(
        &root,
        DocumentId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001),
        SessionId::from_u128(1),
        None,
    ))
    .unwrap();
    doc.set_journal(Arc::clone(&journal) as Arc<dyn arc_doc::JournalSink>);
    let args = json!({"name": "a", "length": 1});
    invoke(&r, &doc, &s, "test.item.create", args).unwrap();
    invoke(&r, &doc, &s, "test.row.note", json!({})).unwrap();
    assert_eq!(doc.undo_step_list(10).len(), 1);
    assert!(journal.flush(Duration::from_secs(10)));
    assert_eq!(journal.written(), 2);
    drop(doc);
    drop(journal);
    let found = scan(&root, &arc_base::Limits::DEFAULT).unwrap();
    assert_eq!(found.len(), 1);
    let recovered = recover(
        &found[0],
        types(),
        Arc::new(Evaluators::new()),
        RecoverOptions::new(&root, SessionId::from_u128(2)),
    )
    .unwrap();
    assert_eq!(recovered.replayed, 2);
    let snapshot = recovered.document.snapshot();
    assert_eq!(snapshot.iter().count(), 1);
    assert_eq!(snapshot.foreign_rows().count(), 1);
    drop(recovered);
    let _ = std::fs::remove_dir_all(&root);
}
