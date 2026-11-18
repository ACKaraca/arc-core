//! Saving a document to its package: random edits, a save, more edits and a second save give
//! a file that reopens to the in-memory document; the second save writes only the rows of the
//! objects changed since the first; after a save the recovery journal holds nothing to replay.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;
mod recovery_support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::graph::Evaluators;
use arc_doc::package::{Mode, Package};
use arc_doc::{Document, DocumentConfig, LoadOptions};
use package_support::{TempDir, cases, owner};
use proptest::prelude::*;
use recovery_support::{State, protect, registry, set, state, write_base};

/// An edit: set `v` of object `id` (creating it), or delete it.
#[derive(Debug, Clone)]
enum Edit {
    Set(u128, i64),
    Delete(u128),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        3 => (1_u128..16, -9_i64..9).prop_map(|(id, v)| Edit::Set(id, v)),
        1 => (1_u128..16).prop_map(Edit::Delete),
    ]
}

/// Applies `edit` to the document and the model; returns the object it changed.
fn apply(document: &Document, model: &mut State, edit: &Edit) -> Option<u128> {
    match *edit {
        Edit::Set(id, v) => {
            if model.get(&id) == Some(&v) {
                return None;
            }
            set(document, id, v);
            model.insert(id, v);
            Some(id)
        }
        Edit::Delete(id) => {
            model.remove(&id)?;
            document
                .submit(move |txn| txn.delete(ObjectId::from_u128(id)))
                .wait()
                .unwrap();
            Some(id)
        }
    }
}

/// The `last_txn` of every object row of the package at `path`.
fn last_txns(path: &std::path::Path) -> BTreeMap<u128, Vec<u8>> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut statement = conn.prepare("SELECT id, last_txn FROM arc_object").unwrap();
    statement
        .query_map([], |row| {
            let id: Vec<u8> = row.get(0)?;
            Ok((u128::from_be_bytes(id.try_into().unwrap()), row.get(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(12), ..ProptestConfig::default() })]

    #[test]
    fn two_saves_reopen_to_the_document_and_the_second_writes_only_changed_rows(
        initial in prop::collection::btree_map(1_u128..16, -9_i64..9, 0..8),
        first in prop::collection::vec(edit(), 0..12),
        second in prop::collection::vec(edit(), 0..12),
    ) {
        let dir = TempDir::new("document save");
        let path = dir.path("model.arc");
        write_base(&path, &initial, 1);
        let mut package =
            Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
        let config = DocumentConfig { workers: 1, ..DocumentConfig::default() };
        let document = package
            .open_document(registry(), Arc::new(Evaluators::new()), &LoadOptions::default(), config)
            .unwrap();
        let base = arc_doc::package::recovery::BaseFile::of(&path, 1).unwrap();
        let journal = protect(&document, &dir.path("app"), 7, Some(base));
        let mut model = initial.clone();
        for edit in &first {
            apply(&document, &mut model, edit);
        }
        let saved = package.save_document(&document, &[], Some(&journal), &mut |_| Ok(())).unwrap();
        prop_assert!(saved.incremental, "{:?}", saved.refused);
        prop_assert_eq!(saved.metadata.save_counter, 2);

        // Mark every row: a row the second save writes loses the marker.
        let marker = vec![0xAB_u8; 16];
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("UPDATE arc_object SET last_txn = ?1", [&marker])
            .unwrap();
        let mut changed = BTreeSet::new();
        for edit in &second {
            changed.extend(apply(&document, &mut model, edit));
        }
        let saved = package.save_document(&document, &[], Some(&journal), &mut |_| Ok(())).unwrap();
        prop_assert!(saved.incremental, "{:?}", saved.refused);
        prop_assert_eq!(saved.metadata.save_counter, 3);
        let rows = last_txns(&path);
        prop_assert_eq!(rows.keys().copied().collect::<BTreeSet<_>>(), model.keys().copied().collect());
        for (id, txn) in &rows {
            prop_assert_eq!(txn == &marker, !changed.contains(id), "object {}", id);
        }

        let reopened = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT)
            .unwrap()
            .load(registry(), &LoadOptions::default())
            .unwrap();
        prop_assert!(reopened.diagnostics.is_empty());
        prop_assert_eq!(state(&reopened.snapshot), state(&document.snapshot()));
        prop_assert_eq!(state(&reopened.snapshot), model);

        // The completed save left no record to replay: a clean close removes the database.
        prop_assert!(journal.flush(Duration::from_secs(10)));
        let database = journal.path().to_path_buf();
        journal.close();
        prop_assert!(!database.exists());
    }
}

#[test]
fn a_document_not_opened_from_the_package_is_saved_in_full() {
    let dir = TempDir::new("document full");
    let path = dir.path("model.arc");
    write_base(&path, &BTreeMap::from([(1, 1)]), 1);
    let mut package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
    // A new document: no baseline, so the first save rewrites the file.
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    set(&document, 2, 5);
    let saved = package
        .save_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();
    assert!(!saved.incremental);
    set(&document, 3, 6);
    let saved = package
        .save_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();
    assert!(saved.incremental, "{:?}", saved.refused);
    let reopened = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT)
        .unwrap()
        .load(registry(), &LoadOptions::default())
        .unwrap();
    assert_eq!(state(&reopened.snapshot), BTreeMap::from([(2, 5), (3, 6)]));
}
