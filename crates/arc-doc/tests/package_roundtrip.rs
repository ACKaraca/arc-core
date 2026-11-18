//! The package round trip (CORE-DOC-040, CORE-DOC-044, CORE-DOC-051), model-based: random stored
//! documents (current, older minor and major versions, migrations, unknown types, corrupt and
//! unaddressable rows, newer minor versions) are written to a package, read back and compared
//! with an in-memory load of the same rows, then saved by the full rewrite with a random subset
//! of objects touched and compared object for object, held rows, type rows and last
//! transactions included. Also payload hash checks, previews, metadata and determinism.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::{DocumentId, Identifier};
use arc_base::{Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::package::{
    Contents, MetaValue, Metadata, Mode, PREVIEW, Package, Save, THUMBNAIL, save_as, write,
};
use arc_doc::{DiagnosticKind, Header, LoadOptions, WorkingSet, descriptor, load};
use package_support::{
    TempDir, blake3_of, cases, dump, held_rows, objects, preview, raw_rows, raw_types, registry,
    sorted, stored, types,
};
use proptest::prelude::*;

const NEW_TXN: [u8; 16] = [0xEE; 16];

fn metadata() -> Metadata {
    let mut other = BTreeMap::new();
    other.insert("ext_table:arc_ext_demo".to_owned(), MetaValue::Integer(1));
    other.insert("product.blob".to_owned(), MetaValue::Blob(vec![0, 1, 2]));
    Metadata {
        document_id: Some(DocumentId::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0001,
        )),
        created_by: Some("arc tests 1.0".into()),
        saved_by: Some("arc tests 1.1".into()),
        units: Some("mm".into()),
        coordinate_reference: Some("EPSG:25832".into()),
        save_counter: 4,
        history_retention: Some("all".into()),
        partitions: vec!["level 1".into(), "level 2".into()],
        thumbnail_missing: false,
        other,
    }
}

fn no_progress() -> impl FnMut(arc_doc::package::SaveStep) -> arc_base::Result<()> {
    |_| Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(24), ..ProptestConfig::default() })]

    #[test]
    fn a_full_rewrite_keeps_every_object_row_and_transaction(
        (newer_minor, input) in package_support::document(),
        with_b in any::<bool>(),
        touch in prop::collection::vec(any::<bool>(), 40),
    ) {
        let dir = TempDir::new("roundtrip");
        let first = dir.path("first.arc");
        let declared = types(newer_minor);
        let meta = metadata();
        let contents = Contents {
            metadata: &meta,
            types: &declared,
            objects: &input,
            previews: &[],
            snapshot: None,
        };
        write(&first, &contents, &mut no_progress()).unwrap();
        prop_assert_eq!(raw_rows(&first), sorted(input.clone()));

        // Reading the package equals loading the same rows in memory.
        let options = LoadOptions::default();
        let package = Package::open(&first, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
        prop_assert_eq!(package.metadata(), &Metadata { thumbnail_missing: true, ..meta.clone() });
        let loaded = package.load(registry(with_b), &options).unwrap();
        let in_memory = load(registry(with_b), &declared, input.clone(), &options).unwrap();
        prop_assert_eq!(dump(&loaded), dump(&in_memory));
        let read_only = !loaded.snapshot.read_only().is_empty();
        prop_assert_eq!(read_only, newer_minor);

        // Touch a random subset of the typed objects of a writable document.
        let mut set = WorkingSet::new(&loaded.snapshot);
        let mut touched = BTreeMap::new();
        if !read_only {
            let live: Vec<_> = loaded.snapshot.iter().map(|(h, o)| (h, Arc::clone(o))).collect();
            for (at, (handle, object)) in live.iter().enumerate() {
                if touch[at % touch.len()] {
                    let header = Header { name: Some("edited".into()), ..object.header.clone() };
                    set.replace_header(*handle, header).unwrap();
                    touched.insert(object.id, NEW_TXN.to_vec());
                }
            }
        }
        let snapshot = set.current();
        let second = dir.path("second.arc");
        let save = Save {
            snapshot,
            loaded: Some(&loaded),
            touched: &touched,
            metadata: &meta,
            previews: &[],
            limits: Limits::DEFAULT,
        };
        let written = save_as(&second, &save, &mut no_progress()).unwrap();
        prop_assert_eq!(written.save_counter, meta.save_counter + 1);
        prop_assert!(written.thumbnail_missing);

        let again = Package::open(&second, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
        prop_assert_eq!(again.metadata(), &written);
        let reloaded = again.load(registry(with_b), &options).unwrap();
        if read_only {
            // A copy of a read-only document is every row and type row as stored.
            prop_assert_eq!(raw_rows(&second), sorted(input.clone()));
            prop_assert_eq!(raw_types(&second), raw_types(&first));
            prop_assert_eq!(dump(&reloaded), dump(&loaded));
        } else {
            prop_assert_eq!(objects(&reloaded.snapshot), objects(snapshot));
            prop_assert_eq!(held_rows(&reloaded.snapshot), held_rows(&loaded.snapshot));
            prop_assert_eq!(
                sorted(reloaded.rows.unaddressable().to_vec()),
                sorted(loaded.rows.unaddressable().to_vec())
            );
            for id in objects(snapshot).keys() {
                let expected = touched
                    .get(id)
                    .map(Vec::as_slice)
                    .or_else(|| loaded.rows.last_txn(*id));
                prop_assert_eq!(reloaded.rows.last_txn(*id), expected);
            }
            prop_assert_eq!(reloaded.counts.migrated + reloaded.counts.older_minor, 0);
            // Every written payload carries its BLAKE3 hash; no diagnostic reports a mismatch.
            for row in raw_rows(&second) {
                let held = held_rows(&loaded.snapshot).contains(&row)
                    || loaded.rows.unaddressable().contains(&row);
                prop_assert!(held || row.payload_hash == blake3_of(&row.payload));
            }
            // Type rows: the current version of each written registered type with the
            // registry's descriptor, and the stored type row of every row written as stored.
            let saved_types = raw_types(&second);
            let reg = registry(with_b);
            for row in &saved_types {
                let current = reg.get(&row.type_key).filter(|decl| {
                    (i64::from(decl.schema.version.major), i64::from(decl.schema.version.minor))
                        == (row.major, row.minor)
                });
                let typed = objects(snapshot).values().any(|(key, ..)| *key == row.type_key);
                match current {
                    // Held rows alone at the current version keep their stored type row.
                    Some(decl) if typed => prop_assert_eq!(
                        Some(row.descriptor.clone()),
                        descriptor(decl, decl.schema.version)
                    ),
                    _ => prop_assert!(declared.contains(row)),
                }
            }
            for row in raw_rows(&second) {
                let was_declared = declared
                    .iter()
                    .any(|t| (&t.type_key, t.major, t.minor) == (&row.type_key, row.major, row.minor));
                let is_declared = saved_types
                    .iter()
                    .any(|t| (&t.type_key, t.major, t.minor) == (&row.type_key, row.major, row.minor));
                prop_assert!(!was_declared || is_declared, "type row of {:?} dropped", row);
            }
        }
        prop_assert!(!reloaded
            .diagnostics
            .iter()
            .any(|d| d.error.code() == codes::PACKAGE_HASH_MISMATCH.code));
        // Only the target remains: no temporary file, no lock.
        prop_assert_eq!(dir.files(), vec!["first.arc".to_owned(), "second.arc".to_owned()]);
    }
}

#[test]
fn a_payload_hash_mismatch_is_reported() {
    let dir = TempDir::new("hash");
    let path = dir.path("hash.arc");
    let mut good = stored(3, 5, 1, None, 1);
    let mut bad = stored(3, 6, 1, None, 1);
    bad.payload_hash = vec![0; 32];
    good.name = Some("good".into());
    let declared = types(false);
    let meta = Metadata::default();
    let rows = [good, bad];
    let contents = Contents {
        metadata: &meta,
        types: &declared,
        objects: &rows,
        previews: &[],
        snapshot: None,
    };
    write(&path, &contents, &mut no_progress()).unwrap();
    let package = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let loaded = package
        .load(registry(true), &LoadOptions::default())
        .unwrap();
    let mismatches: Vec<_> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.error.code() == codes::PACKAGE_HASH_MISMATCH.code)
        .collect();
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0].kind, DiagnosticKind::Warning);
    assert_eq!(mismatches[0].object, Some(ObjectId::from_u128(6)));
}

#[test]
fn previews_are_stored_and_read_back_and_a_save_without_them_succeeds() {
    let dir = TempDir::new("previews");
    let reg = registry(true);
    let loaded = load(
        Arc::clone(&reg),
        &types(false),
        vec![stored(3, 1, 1, None, 1)],
        &LoadOptions::default(),
    )
    .unwrap();
    let previews = vec![
        preview(THUMBNAIL, 256, 256),
        preview(PREVIEW, 1024, 16),
        preview("sheet:A-101", 64, 32),
    ];
    let meta = Metadata::default();
    let touched = BTreeMap::new();
    let mut save = Save {
        snapshot: &loaded.snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: &meta,
        previews: &previews,
        limits: Limits::DEFAULT,
    };
    let with = dir.path("with.arc");
    let written = save_as(&with, &save, &mut no_progress()).unwrap();
    assert!(!written.thumbnail_missing);
    let package = Package::open(&with, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let mut expected = previews.clone();
    expected.sort_by(|a, b| a.kind.cmp(&b.kind));
    assert_eq!(package.previews().unwrap(), expected);
    assert!(!package.metadata().thumbnail_missing);

    save.previews = &[];
    let without = dir.path("without.arc");
    let written = save_as(&without, &save, &mut no_progress()).unwrap();
    assert!(written.thumbnail_missing);
    let package = Package::open(&without, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    assert!(package.previews().unwrap().is_empty());
    assert!(package.metadata().thumbnail_missing);

    // Invalid and repeated previews fail before anything is written.
    let target = dir.path("invalid.arc");
    for bad in [
        vec![preview(THUMBNAIL, 128, 128)],
        vec![preview(PREVIEW, 1024, 16), preview(PREVIEW, 1024, 16)],
    ] {
        let save = Save {
            previews: &bad,
            ..save
        };
        let error = save_as(&target, &save, &mut no_progress()).unwrap_err();
        assert_eq!(error.code(), codes::PACKAGE_PREVIEW_INVALID.code);
        assert!(!target.exists());
    }
}

#[test]
fn a_document_never_loaded_needs_the_transaction_of_every_object() {
    let dir = TempDir::new("fresh");
    let reg = registry(true);
    let loaded = load(
        Arc::clone(&reg),
        &types(false),
        vec![stored(3, 1, 1, None, 1)],
        &LoadOptions::default(),
    )
    .unwrap();
    let meta = Metadata::default();
    let touched = BTreeMap::new();
    let mut save = Save {
        snapshot: &loaded.snapshot,
        loaded: None,
        touched: &touched,
        metadata: &meta,
        previews: &[],
        limits: Limits::DEFAULT,
    };
    let target = dir.path("fresh.arc");
    let error = save_as(&target, &save, &mut no_progress()).unwrap_err();
    assert_eq!(error.code(), codes::PACKAGE_LAST_TXN_MISSING.code);
    assert!(!target.exists());
    let mut touched = BTreeMap::new();
    touched.insert(ObjectId::from_u128(1), NEW_TXN.to_vec());
    save.touched = &touched;
    save_as(&target, &save, &mut no_progress()).unwrap();
    let package = Package::open(&target, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let again = package.load(reg, &LoadOptions::default()).unwrap();
    assert_eq!(
        again.rows.last_txn(ObjectId::from_u128(1)),
        Some(NEW_TXN.as_slice())
    );
}

#[test]
fn equal_saves_are_byte_identical_and_derived_indexes_are_filled() {
    let dir = TempDir::new("determinism");
    let reg = registry(true);
    let rows = vec![
        stored(3, 1, 4, None, 1),
        stored(5, 2, 0, Some(1), 1),
        stored(0, 3, 2, Some(2), 2),
    ];
    let loaded = load(
        Arc::clone(&reg),
        &types(false),
        rows,
        &LoadOptions::default(),
    )
    .unwrap();
    let meta = metadata();
    let touched = BTreeMap::new();
    let previews = vec![preview(THUMBNAIL, 256, 256)];
    let save = Save {
        snapshot: &loaded.snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: &meta,
        previews: &previews,
        limits: Limits::DEFAULT,
    };
    let (a, b) = (dir.path("a.arc"), dir.path("b.arc"));
    save_as(&a, &save, &mut no_progress()).unwrap();
    save_as(&b, &save, &mut no_progress()).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());

    let conn =
        rusqlite::Connection::open_with_flags(&a, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let references: Vec<(Vec<u8>, String, Vec<u8>)> = conn
        .prepare("SELECT source, field, target FROM arc_reference ORDER BY 1, 2, 3")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let id = |n: u128| n.to_be_bytes().to_vec();
    assert_eq!(
        references,
        vec![
            (id(2), "(owner)".to_owned(), id(1)),
            (id(2), "r".to_owned(), id(1)),
            (id(3), "(owner)".to_owned(), id(2)),
        ]
    );
    let properties: Vec<(Vec<u8>, String, i64)> = conn
        .prepare("SELECT object, key, value FROM arc_property ORDER BY 1, 2")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        properties,
        vec![(id(1), "x".to_owned(), 4), (id(3), "x".to_owned(), 2)]
    );
}
