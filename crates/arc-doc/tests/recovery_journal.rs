//! The recovery journal (CORE-DOC-041) and crash recovery (CORE-DOC-042) in one process:
//! replay onto an unchanged and a changed base, a never-saved document, truncation at a save,
//! deletion on a clean close, recovery after recovery, undo after recovery and hostile recovery
//! databases (byte mutations, truncations, foreign files), which never panic and recover a
//! prefix of the records.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;
mod recovery_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_base::id::Identifier;
use arc_base::{Limits, SessionId};
use arc_doc::Document;
use arc_doc::graph::Evaluators;
use arc_doc::package::recovery::{self, BaseFile, Orphan, RecoverOptions, Recovered, scan};
use package_support::TempDir;
use recovery_support::{State, open, protect, registry, set, state, write_base};

const WAIT: Duration = Duration::from_secs(10);

fn orphans(root: &Path) -> Vec<Orphan> {
    scan(root, &Limits::DEFAULT).unwrap()
}

fn recover_one(root: &Path, session: u128) -> Recovered {
    let found = orphans(root);
    assert_eq!(found.len(), 1, "{found:?}");
    let options = RecoverOptions::new(root, SessionId::from_u128(session));
    recovery::recover(&found[0], registry(), Arc::new(Evaluators::new()), options).unwrap()
}

fn initial() -> State {
    (1..=5).map(|i| (i, 0)).collect()
}

/// A base package, a session that edits it and leaves its journal behind; returns the
/// expected recovered state.
fn session_with_edits(dir: &TempDir, root: &Path) -> (PathBuf, State) {
    let base_path = dir.path("model.arc");
    write_base(&base_path, &initial(), 1);
    let (document, base) = open(&base_path);
    let journal = protect(&document, root, 1, Some(base));
    let mut expected = initial();
    for (i, v) in [(1, 10), (2, 20), (6, 60), (1, 11)] {
        set(&document, i, v);
        expected.insert(i, v);
    }
    assert!(journal.flush(WAIT));
    assert_eq!(journal.written(), 4);
    drop(document);
    drop(journal);
    (base_path, expected)
}

#[test]
fn an_unchanged_base_is_replayed_and_undo_works_after_recovery() {
    let dir = TempDir::new("rec-unchanged");
    let root = dir.path("app");
    let (_, expected) = session_with_edits(&dir, &root);
    let found = orphans(&root);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].records, 4);
    assert!(found[0].problem.is_none() && found[0].base.is_some());
    let recovered = recover_one(&root, 2);
    assert!(!recovered.base_changed);
    assert_eq!(recovered.replayed, 4);
    assert!(recovered.stopped.is_none());
    assert_eq!(state(&recovered.document.snapshot()), expected);
    assert_eq!(recovered.document.with_journal(arc_doc::Journal::len), 4);
    // The undo history starts at the recovered state; new commits undo normally.
    assert!(!recovered.document.can_undo());
    set(&recovered.document, 3, 33);
    assert!(recovered.document.undo().wait().unwrap().value);
    assert_eq!(state(&recovered.document.snapshot()), expected);
    // The old orphan is gone; the new session's journal is live, so nothing is listed.
    assert!(orphans(&root).is_empty());
}

#[test]
fn a_changed_base_recovers_from_the_copy_as_a_new_document() {
    let dir = TempDir::new("rec-changed");
    let root = dir.path("app");
    let (base_path, expected) = session_with_edits(&dir, &root);
    // Another save replaced the file.
    write_base(&base_path, &State::from([(9, 9)]), 7);
    let recovered = recover_one(&root, 2);
    assert!(recovered.base_changed);
    assert_eq!(recovered.replayed, 4);
    assert_eq!(state(&recovered.document.snapshot()), expected);
}

#[test]
fn recovery_after_recovery_keeps_every_record() {
    let dir = TempDir::new("rec-twice");
    let root = dir.path("app");
    let (base_path, mut expected) = session_with_edits(&dir, &root);
    let recovered = recover_one(&root, 2);
    set(&recovered.document, 2, 22);
    expected.insert(2, 22);
    assert!(recovered.journal.flush(WAIT));
    drop(recovered);
    // The second recovery replays the first session's and the second session's records,
    // also onto the copy once the base changed.
    write_base(&base_path, &State::new(), 9);
    let again = recover_one(&root, 3);
    assert!(again.base_changed);
    assert_eq!(again.replayed, 5);
    assert_eq!(state(&again.document.snapshot()), expected);
}

#[test]
fn a_document_never_saved_recovers_onto_an_empty_document() {
    let dir = TempDir::new("rec-new");
    let root = dir.path("app");
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    let journal = protect(&document, &root, 1, None);
    set(&document, 1, 1);
    set(&document, 1, 2);
    assert!(journal.flush(WAIT));
    drop((document, journal));
    let recovered = recover_one(&root, 2);
    assert!(!recovered.base_changed);
    assert_eq!(state(&recovered.document.snapshot()), State::from([(1, 2)]));
}

#[test]
fn a_save_truncates_and_a_clean_close_deletes_the_journal() {
    let dir = TempDir::new("rec-save");
    let root = dir.path("app");
    let base_path = dir.path("model.arc");
    write_base(&base_path, &initial(), 1);
    let (document, base) = open(&base_path);
    let journal = protect(&document, &root, 1, Some(base));
    set(&document, 1, 5);
    set(&document, 2, 5);
    // The application saves the state; the journal is truncated to the new base.
    let saved_state = state(&document.snapshot());
    let last = document.with_journal(|j| j.last().map(|e| e.record().txn_id));
    write_base(&base_path, &saved_state, 2);
    let base = BaseFile::of(&base_path, 1).unwrap();
    journal.saved(Some(base), last).unwrap();
    assert!(journal.is_protected());
    set(&document, 3, 7);
    assert!(journal.flush(WAIT));
    let directory = journal.path().parent().unwrap().to_path_buf();
    drop(document);
    drop(journal);
    // One record after the save is left; recovery replays it onto the saved file.
    let recovered = recover_one(&root, 2);
    assert_eq!(recovered.replayed, 1);
    let mut expected = saved_state;
    expected.insert(3, 7);
    assert_eq!(state(&recovered.document.snapshot()), expected);
    // Saving again and closing cleanly removes every file of the session.
    let last = recovered
        .document
        .with_journal(|j| j.last().map(|e| e.record().txn_id));
    write_base(&base_path, &expected, 3);
    let base = BaseFile::of(&base_path, 2).unwrap();
    recovered.journal.saved(Some(base), last).unwrap();
    recovered.journal.close();
    drop(recovered);
    let left: Vec<_> = std::fs::read_dir(&directory).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(orphans(&root).is_empty());
}

#[test]
fn a_live_journal_is_not_listed_and_discard_removes_an_orphan() {
    let dir = TempDir::new("rec-live");
    let root = dir.path("app");
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    let journal = protect(&document, &root, 1, None);
    set(&document, 1, 1);
    assert!(journal.flush(WAIT));
    assert!(orphans(&root).is_empty());
    drop((document, journal));
    let found = orphans(&root);
    assert_eq!(found.len(), 1);
    recovery::discard(&found[0]).unwrap();
    assert!(orphans(&root).is_empty());
}

/// The database and the state after each record of a session of 8 records on a never-saved
/// document.
fn sample(root: &Path) -> (PathBuf, Vec<State>) {
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    let journal = protect(&document, root, 1, None);
    let mut states = vec![State::new()];
    for i in 0..8_i64 {
        set(&document, u128::try_from(i % 3).unwrap() + 1, i);
        states.push(state(&document.snapshot()));
    }
    assert!(journal.flush(WAIT));
    let path = journal.path().to_path_buf();
    drop((document, journal));
    (path, states)
}

/// Recovers a database holding `bytes`; it must not panic, and what it recovers is a prefix.
fn recover_bytes(dir: &TempDir, n: usize, bytes: &[u8], states: &[State]) {
    let root = dir.path(&format!("hostile-{n}"));
    let directory = root
        .join("recovery")
        .join(arc_base::id::DocumentId::from_u128(recovery_support::DOCUMENT).to_string());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("x.arcrec"), bytes).unwrap();
    let found = orphans(&root);
    assert_eq!(found.len(), 1);
    let options = RecoverOptions::new(&root, SessionId::from_u128(100 + n as u128));
    if let Ok(recovered) =
        recovery::recover(&found[0], registry(), Arc::new(Evaluators::new()), options)
    {
        let replayed = usize::try_from(recovered.replayed).unwrap();
        assert_eq!(
            state(&recovered.document.snapshot()),
            states[replayed],
            "mutation {n}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hostile_recovery_databases_never_panic_and_recover_a_prefix() {
    let dir = TempDir::new("rec-hostile");
    let (path, states) = sample(&dir.path("app"));
    let original = std::fs::read(&path).unwrap();
    let mut seed = 0x1234_5678_u64;
    let mut next = |bound: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(seed >> 33).unwrap() % bound
    };
    let mut n = 0;
    // Truncations at many lengths.
    for cut in (0..original.len()).step_by(original.len() / 24 + 1) {
        recover_bytes(&dir, n, &original[..cut], &states);
        n += 1;
    }
    // Random byte mutations.
    for _ in 0..60 {
        let mut bytes = original.clone();
        for _ in 0..1 + next(4) {
            let at = next(bytes.len());
            bytes[at] ^= u8::try_from(1 + next(255)).unwrap();
        }
        recover_bytes(&dir, n, &bytes, &states);
        n += 1;
    }
    // Garbage and a package are not recovery databases.
    let garbage = TempDir::new("rec-garbage");
    recover_bytes(&garbage, 0, b"not a database at all", &states);
    let package = dir.path("p.arc");
    write_base(&package, &State::new(), 1);
    let error = {
        let root = dir.path("foreign");
        let directory = root.join("recovery").join("d");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::copy(&package, directory.join("p.arcrec")).unwrap();
        let found = orphans(&root);
        assert_eq!(
            found[0].problem.as_ref().unwrap().code(),
            "doc.recovery.invalid"
        );
        let options = RecoverOptions::new(&root, SessionId::from_u128(9));
        recovery::recover(&found[0], registry(), Arc::new(Evaluators::new()), options).unwrap_err()
    };
    assert_eq!(error.code(), "doc.recovery.invalid");
}

#[test]
fn a_record_out_of_order_ends_the_replay_at_the_last_good_record() {
    let dir = TempDir::new("rec-order");
    let (path, states) = sample(&dir.path("app"));
    // Record 4 is replaced by a copy of record 2: valid bytes, but not the next transaction.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let (record, hash): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT record, hash FROM arc_record WHERE seq = 2",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    conn.execute(
        "UPDATE arc_record SET record = ?1, hash = ?2 WHERE seq = 4",
        rusqlite::params![record, hash],
    )
    .unwrap();
    drop(conn);
    let found = orphans(&dir.path("app"));
    let options = RecoverOptions::new(dir.path("app"), SessionId::from_u128(5));
    let recovered =
        recovery::recover(&found[0], registry(), Arc::new(Evaluators::new()), options).unwrap();
    assert_eq!(recovered.replayed, 3);
    assert_eq!(state(&recovered.document.snapshot()), states[3]);
    assert_eq!(
        recovered.stopped.as_ref().unwrap().code(),
        "doc.recovery.truncated"
    );
    // The new session holds the valid prefix only.
    drop(recovered);
    let again = recover_one(&dir.path("app"), 6);
    assert_eq!(again.replayed, 3);
    assert!(again.stopped.is_none());
}
