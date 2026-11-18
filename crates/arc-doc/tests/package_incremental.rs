//! Atomic incremental save (CORE-DOC-043): only the rows of touched objects are written; a chain
//! of incremental saves gives the same tables as a full rewrite of the final state; refusals
//! fall back to the full rewrite; a hot rollback journal is recovered on open for editing; and a
//! save process killed at random points leaves a file holding the old or the new state.
//!
//! The killed process is this test executable, re-run with `ARC_DOC_INCREMENTAL_TARGET` set so
//! that the `kill_child` entry performs one incremental save. Run the roadmap's 1,000 saves in a
//! release build:
//! `cargo nextest run -p arc-doc --all-features --release --run-ignored only incremental_kill_1000`.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::package::{
    Access, Contents, Metadata, Mode, Package, ReadOnlyCause, Save, SaveProgress,
    Strategy as SaveStrategy, THUMBNAIL, Touch, save_as, touched_between, write,
};
use arc_doc::{
    FieldDecl, Header, LoadOptions, Loaded, Object, Schema, SchemaVersion, Snapshot, TypeDecl,
    TypeKey, TypeRegistry, Value, ValueType, WorkingSet,
};
use package_support::{TempDir, cases, objects, owner, preview, registry, types};
use proptest::prelude::*;
use rusqlite::types::Value as SqlValue;

fn no_progress() -> impl FnMut(SaveProgress) -> arc_base::Result<()> {
    |_| Ok(())
}

/// Every row of every table of the package, tables by name, rows sorted.
fn tables(path: &Path) -> BTreeMap<String, Vec<Vec<String>>> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut statement = conn.prepare(&format!("SELECT * FROM {name}")).unwrap();
            let columns = statement.column_count();
            let mut rows: Vec<Vec<String>> = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|at| row.get::<_, SqlValue>(at).map(|v| format!("{v:?}")))
                        .collect()
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            rows.sort();
            (name, rows)
        })
        .collect()
}

fn t_a(id: u128, x: i64) -> Object {
    Object::new(
        ObjectId::from_u128(id),
        SchemaVersion::new(3, 1),
        Header::default(),
        vec![
            Some(Value::I64(x)),
            Some(Value::String(format!("tag {x}"))),
            None,
        ],
    )
}

#[derive(Debug, Clone)]
enum Edit {
    Create(u128, i64),
    Modify(usize, i64),
    Rename(usize),
    Delete(usize),
    Save(bool),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (100_u128..130, -5_i64..5).prop_map(|(id, x)| Edit::Create(id, x)),
        (any::<usize>(), -5_i64..5).prop_map(|(at, x)| Edit::Modify(at, x)),
        any::<usize>().prop_map(Edit::Rename),
        any::<usize>().prop_map(Edit::Delete),
        any::<bool>().prop_map(Edit::Save),
    ]
}

/// Applies `edit` to `set`, recording the transaction bytes `txn` of every changed object.
fn apply(set: &mut WorkingSet, edit: &Edit, txn: u8, touched: &mut BTreeMap<ObjectId, Vec<u8>>) {
    let live: Vec<_> = set
        .current()
        .iter()
        .map(|(handle, object)| {
            (
                handle,
                Arc::clone(object),
                set.current().type_key(handle).unwrap().as_str().to_owned(),
            )
        })
        .collect();
    let pick = |at: usize| live.get(at % live.len().max(1)).cloned();
    match *edit {
        Edit::Create(id, x) => {
            if set.current().resolve(ObjectId::from_u128(id)).is_err()
                && set.create("t.a", t_a(id, x)).is_ok()
            {
                touched.insert(ObjectId::from_u128(id), vec![txn; 16]);
            }
        }
        Edit::Modify(at, x) => {
            // A modification to the same value changes nothing, as no transaction records it.
            if let Some((handle, object, key)) = pick(at)
                && key == "t.a"
                && object.values[0] != Some(Value::I64(x))
            {
                let mut values = object.values.clone();
                values[0] = Some(Value::I64(x));
                set.replace_values(handle, values).unwrap();
                touched.insert(object.id, vec![txn; 16]);
            }
        }
        Edit::Rename(at) => {
            if let Some((handle, object, _)) = pick(at) {
                let header = Header {
                    name: Some(format!("renamed {txn}")),
                    ..object.header.clone()
                };
                if set.replace_header(handle, header).is_ok() {
                    touched.insert(object.id, vec![txn; 16]);
                }
            }
        }
        Edit::Delete(at) => {
            if let Some((handle, object, _)) = pick(at)
                && set.delete(handle).is_ok()
            {
                touched.remove(&object.id);
            }
        }
        Edit::Save(_) => {}
    }
}

fn meta() -> Metadata {
    Metadata {
        created_by: Some("tests".into()),
        save_counter: 3,
        ..Metadata::default()
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(16), ..ProptestConfig::default() })]

    /// A chain of incremental saves equals a full rewrite of the final state, table by table.
    #[test]
    fn incremental_saves_equal_a_full_rewrite(
        (_, input) in package_support::document().prop_filter("writable", |(newer, _)| !newer),
        edits in prop::collection::vec(edit(), 0..40),
    ) {
        let dir = TempDir::new("incremental eq ü");
        let path = dir.path("model ğ.arc");
        let declared = types(false);
        let base = meta();
        let contents = Contents {
            metadata: &base,
            types: &declared,
            objects: &input,
            previews: &[],
            snapshot: None,
        };
        // The base is a full rewrite of the loaded rows, so its derived indexes are complete.
        let raw = dir.path("raw.arc");
        write(&raw, &contents, &mut |_| Ok(())).unwrap();
        let first = Package::open(&raw, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
        let first = first.load(registry(true), &LoadOptions::default()).unwrap();
        let base_save = Save {
            snapshot: &first.snapshot,
            loaded: Some(&first),
            touched: &BTreeMap::new(),
            metadata: &base,
            previews: &[],
            limits: Limits::DEFAULT,
        };
        save_as(&path, &base_save, &mut |_| Ok(())).unwrap();
        let mut package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
        prop_assert!(package.is_editable());
        let loaded = package.load(registry(true), &LoadOptions::default()).unwrap();
        let mut saved = loaded.snapshot.clone();
        let mut set = WorkingSet::new(&loaded.snapshot);
        let mut touched = BTreeMap::new();
        let mut previews = Vec::new();
        let mut last_metadata = package.metadata().clone();
        let mut saves = 0;
        let mut since_save = std::collections::BTreeSet::new();
        let edits_and_final = edits.iter().chain([Edit::Save(true)].iter());
        for (n, edit) in edits_and_final.enumerate() {
            let before = touched.clone();
            apply(&mut set, edit, u8::try_from(n % 250).unwrap(), &mut touched);
            since_save.extend(touched.iter().filter(|(id, txn)| before.get(id) != Some(txn)).map(|(id, _)| *id));
            if let Edit::Save(thumbnail) = edit {
                let current = set.current().clone();
                previews = if *thumbnail { vec![preview(THUMBNAIL, 256, 256)] } else { Vec::new() };
                last_metadata = package.metadata().clone();
                let save = Save {
                    snapshot: &current,
                    loaded: Some(&loaded),
                    touched: &touched,
                    metadata: &last_metadata,
                    previews: &previews,
                    limits: Limits::DEFAULT,
                };
                // As the journal would: an object changed and changed back since the last
                // save has a new last transaction, so it is touched too.
                let mut changes = touched_between(&saved, &current);
                for id in since_save.iter() {
                    if saved.resolve(*id).is_ok() && current.resolve(*id).is_ok() {
                        changes.entry(*id).or_insert(Touch::Modified);
                    }
                }
                since_save.clear();
                let result = package
                    .save(&save, &SaveStrategy::Incremental(changes), &mut no_progress())
                    .unwrap();
                prop_assert!(result.incremental, "{:?}", result.refused);
                prop_assert_eq!(&result.metadata, package.metadata());
                saved = current;
                set = WorkingSet::new(&saved);
                saves += 1;
            }
        }
        prop_assert!(saves >= 1);
        let full = dir.path("full.arc");
        let save = Save {
            snapshot: &saved,
            loaded: Some(&loaded),
            touched: &touched,
            metadata: &last_metadata,
            previews: &previews,
            limits: Limits::DEFAULT,
        };
        save_as(&full, &save, &mut |_| Ok(())).unwrap();
        drop(package);
        prop_assert_eq!(tables(&path), tables(&full));
    }
}

const N: u128 = 200;

fn simple_registry() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("v", ValueType::I64).indexed(),
            FieldDecl::new("s", ValueType::String),
        ],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("k.n").unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn k_n(id: u128, v: i64, pad: usize) -> Object {
    Object::new(
        ObjectId::from_u128(id),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![
            Some(Value::I64(v)),
            Some(Value::String(format!("object {id} {}", "x".repeat(pad)))),
        ],
    )
}

/// State `n` of the kill and change-count tests: object `i` (1 to `N`) is absent when
/// `i % 7 == n % 7`, and its value is the last state up to `n` whose number is `i` modulo 5.
fn state(n: u64) -> Snapshot {
    let mut set = WorkingSet::new(&Snapshot::empty(simple_registry()));
    for i in 1..=N {
        let i64_i = i64::try_from(i).unwrap();
        let n = i64::try_from(n).unwrap();
        if i64_i % 7 == n % 7 {
            continue;
        }
        let v = n - ((n % 5 + 5 - i64_i % 5) % 5);
        set.create("k.n", k_n(i, v, 60)).unwrap();
    }
    set.current().clone()
}

/// Writes state `n` with `save_counter` `n + 1` by the full rewrite.
fn write_state(path: &Path, n: u64) {
    let snapshot = state(n);
    let touched = snapshot
        .iter()
        .map(|(_, object)| (object.id, vec![0; 16]))
        .collect();
    let metadata = Metadata {
        save_counter: n,
        ..Metadata::default()
    };
    let save = Save {
        snapshot: &snapshot,
        loaded: None,
        touched: &touched,
        metadata: &metadata,
        previews: &[],
        limits: Limits::DEFAULT,
    };
    save_as(path, &save, &mut |_| Ok(())).unwrap();
}

/// Opens `path` for editing, taking over a stale lock left by a killed process.
fn open_edit(path: &Path) -> Package {
    let me = owner("kill", "localhost-test");
    let mut package = Package::open(path, Mode::Edit(&me), &Limits::DEFAULT).unwrap();
    if let Access::ReadOnly(ReadOnlyCause::StaleLock(_)) = package.access() {
        package.take_over(&me).unwrap();
    }
    package
}

/// Brings `loaded` to state `n` in a working set; the changed objects get transaction `n`.
fn edit_to(loaded: &Loaded, n: u64) -> (Snapshot, BTreeMap<ObjectId, Vec<u8>>) {
    let target = state(n);
    let mut set = WorkingSet::new(&loaded.snapshot);
    for (handle, object) in loaded.snapshot.iter() {
        match target.resolve(object.id) {
            Err(_) => {
                set.delete(handle).unwrap();
            }
            Ok(new) => {
                let new = target.get(new).unwrap();
                if new.values != object.values {
                    set.replace_values(handle, new.values.clone()).unwrap();
                }
            }
        }
    }
    for (_, object) in target.iter() {
        if loaded.snapshot.resolve(object.id).is_err() {
            set.create("k.n", (**object).clone()).unwrap();
        }
    }
    let snapshot = set.current().clone();
    let touched = touched_between(&loaded.snapshot, &snapshot)
        .into_keys()
        .map(|id| (id, n.to_be_bytes().to_vec()))
        .collect();
    (snapshot, touched)
}

/// Saves state `n + 1` incrementally into the package at `path`, which holds state `n`.
fn save_next(path: &Path, n: u64) -> arc_doc::package::Saved {
    let mut package = open_edit(path);
    let loaded = package
        .load(simple_registry(), &LoadOptions::default())
        .unwrap();
    let (snapshot, touched) = edit_to(&loaded, n + 1);
    let metadata = package.metadata().clone();
    let save = Save {
        snapshot: &snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: &metadata,
        previews: &[],
        limits: Limits::DEFAULT,
    };
    let changes = touched_between(&loaded.snapshot, &snapshot);
    package
        .save(
            &save,
            &SaveStrategy::Incremental(changes),
            &mut no_progress(),
        )
        .unwrap()
}

/// Which state the package at `path` holds: opened for editing (which recovers a hot journal),
/// checked for integrity and loaded. Panics when it holds neither candidate.
fn stored_state(path: &Path, candidates: [u64; 2]) -> u64 {
    let package = Package::open(
        path,
        Mode::Edit(&owner("check", "elsewhere")),
        &Limits::DEFAULT,
    )
    .unwrap();
    let loaded = package
        .load(simple_registry(), &LoadOptions::default())
        .unwrap();
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    let found = objects(&loaded.snapshot);
    let counter = package.metadata().save_counter;
    drop(package);
    let conn = rusqlite::Connection::open(path).unwrap();
    let check: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    for n in candidates {
        if found == objects(&state(n)) && counter == n + 1 {
            return n;
        }
    }
    panic!("{} holds neither state {candidates:?}", path.display());
}

#[test]
fn only_the_rows_of_touched_objects_are_written() {
    let dir = TempDir::new("incremental rows");
    let path = dir.path("rows.arc");
    write_state(&path, 0);
    let marker = vec![0xAB; 16];
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("UPDATE arc_object SET last_txn = ?1", [&marker])
        .unwrap();
    conn.execute("UPDATE arc_property SET unit = 'marker'", [])
        .unwrap();
    drop(conn);
    let saved = save_next(&path, 0);
    assert!(saved.incremental);
    assert_eq!(saved.metadata.save_counter, 2);
    let changed = touched_between(&state(0), &state(1));
    assert!(changed.values().any(|t| *t == Touch::Created));
    assert!(changed.values().any(|t| *t == Touch::Deleted));
    assert!(changed.values().any(|t| *t == Touch::Modified));
    let conn = rusqlite::Connection::open(&path).unwrap();
    let rows: Vec<(Vec<u8>, Vec<u8>, Option<String>)> = conn
        .prepare(
            "SELECT o.id, o.last_txn, p.unit FROM arc_object o \
             JOIN arc_property p ON p.object = o.id ORDER BY o.id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(rows.len(), state(1).len());
    for (id, txn, unit) in rows {
        let id = ObjectId::from_u128(u128::from_be_bytes(id.try_into().unwrap()));
        if changed.contains_key(&id) {
            assert_eq!(txn, 1_u64.to_be_bytes().to_vec(), "{id}");
            assert_eq!(unit, None, "{id}");
        } else {
            // Untouched rows still carry the markers: they were not rewritten.
            assert_eq!(txn, marker, "{id}");
            assert_eq!(unit.as_deref(), Some("marker"), "{id}");
        }
    }
    drop(conn);
    assert_eq!(stored_state(&path, [0, 1]), 1);
}

#[test]
fn refusals_fall_back_to_the_full_rewrite() {
    let dir = TempDir::new("incremental refusals");
    let path = dir.path("refuse.arc");
    write_state(&path, 0);
    let mut package = open_edit(&path);
    let loaded = package
        .load(simple_registry(), &LoadOptions::default())
        .unwrap();
    let (snapshot, touched) = edit_to(&loaded, 1);
    let good = touched_between(&loaded.snapshot, &snapshot);
    let stale = Metadata {
        save_counter: 7,
        ..package.metadata().clone()
    };
    let mut wrong_kind = good.clone();
    let first_modified = *good.iter().find(|(_, t)| **t == Touch::Modified).unwrap().0;
    wrong_kind.insert(first_modified, Touch::Created);
    let mut wrong_delete = good.clone();
    wrong_delete.insert(first_modified, Touch::Deleted);
    let current = package.metadata().clone();
    for (metadata, changes, reason) in [
        (&stale, &good, "base_changed"),
        (&current, &wrong_kind, "changes"),
        (&current, &wrong_delete, "changes"),
    ] {
        write_state(&path, 0);
        let before = tables(&path);
        let save = Save {
            snapshot: &snapshot,
            loaded: Some(&loaded),
            touched: &touched,
            metadata,
            previews: &[],
            limits: Limits::DEFAULT,
        };
        // A refusal leaves the file as it was before the fallback writes it.
        let mut seen = Vec::new();
        let saved = package
            .save(
                &save,
                &SaveStrategy::Incremental(changes.clone()),
                &mut |event| {
                    if let SaveProgress::Step(step) = event {
                        seen.push(step);
                    }
                    if event == SaveProgress::Step(arc_doc::package::SaveStep::Schema) {
                        assert_eq!(tables(&path), before);
                    }
                    Ok(())
                },
            )
            .unwrap();
        assert!(!saved.incremental);
        let refused = saved.refused.unwrap();
        assert_eq!(refused.code(), codes::PACKAGE_INCREMENTAL_REFUSED.code);
        assert_eq!(
            refused.arguments().get("reason"),
            Some(&arc_base::error::Value::from(reason))
        );
        assert!(seen.contains(&arc_doc::package::SaveStep::Replace));
    }
    drop(package);
    // A package opened read-only is never saved in place.
    let mut read_only = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let metadata = read_only.metadata().clone();
    let save = Save {
        snapshot: &snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: &metadata,
        previews: &[],
        limits: Limits::DEFAULT,
    };
    let error = read_only
        .save(&save, &SaveStrategy::Incremental(good), &mut no_progress())
        .unwrap_err();
    assert_eq!(error.code(), codes::DOCUMENT_READ_ONLY.code);
}

#[test]
fn an_error_at_any_step_leaves_the_file_unchanged() {
    let dir = TempDir::new("incremental fault");
    let path = dir.path("fault.arc");
    write_state(&path, 0);
    let before = std::fs::read(&path).unwrap();
    let mut package = open_edit(&path);
    let loaded = package
        .load(simple_registry(), &LoadOptions::default())
        .unwrap();
    let (snapshot, touched) = edit_to(&loaded, 1);
    let changes = touched_between(&loaded.snapshot, &snapshot);
    let metadata = package.metadata().clone();
    let save = Save {
        snapshot: &snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: &metadata,
        previews: &[],
        limits: Limits::DEFAULT,
    };
    let mut events = Vec::new();
    package
        .save(
            &save,
            &SaveStrategy::Incremental(changes.clone()),
            &mut |event| {
                events.push(event);
                Err(arc_base::Error::new(codes::PACKAGE_IO_FAILED))
            },
        )
        .unwrap_err();
    // Record every event of a successful run on a copy, then fail at each one in turn.
    let copy = dir.path("copy.arc");
    std::fs::copy(&path, &copy).unwrap();
    let mut events = Vec::new();
    let mut copy_package = open_edit(&copy);
    copy_package
        .save(
            &save,
            &SaveStrategy::Incremental(changes.clone()),
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();
    drop(copy_package);
    assert!(events.len() >= 6, "{events:?}");
    for failing in 0..events.len() {
        let mut at = 0;
        let error = package
            .save(
                &save,
                &SaveStrategy::Incremental(changes.clone()),
                &mut |_| {
                    at += 1;
                    if at > failing {
                        Err(arc_base::Error::new(codes::PACKAGE_IO_FAILED))
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
        assert_eq!(error.code(), codes::PACKAGE_IO_FAILED.code);
        assert_eq!(std::fs::read(&path).unwrap(), before, "event {failing}");
        assert_eq!(package.metadata(), &metadata);
    }
}

/// A copy of `path` and its rollback journal taken in the middle of a write transaction whose
/// changes were already written to the database file: a hot journal.
fn hot_copy(path: &Path, into: &Path) {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA cache_size = 1; BEGIN;")
        .unwrap();
    conn.execute(
        "UPDATE arc_object SET payload = zeroblob(length(payload)), name = 'half'",
        [],
    )
    .unwrap();
    conn.execute("DELETE FROM arc_meta", []).unwrap();
    let mut journal = path.as_os_str().to_owned();
    journal.push("-journal");
    let mut into_journal = into.as_os_str().to_owned();
    into_journal.push("-journal");
    std::fs::copy(path, into).unwrap();
    std::fs::copy(&journal, &into_journal).unwrap();
    conn.execute_batch("ROLLBACK").unwrap();
    assert_ne!(std::fs::read(path).unwrap(), std::fs::read(into).unwrap());
}

#[test]
fn a_hot_journal_is_rolled_back_by_an_open_for_editing() {
    let dir = TempDir::new("incremental hot ü");
    let path = dir.path("base ğ.arc");
    write_state(&path, 3);
    let hot = dir.path("hot copy ç.arc");
    hot_copy(&path, &hot);
    let mut journal = hot.as_os_str().to_owned();
    journal.push("-journal");
    let journal = std::path::PathBuf::from(journal);
    // A read-only open never writes, so it cannot roll back: it fails rather than read a
    // half-written state, and leaves the journal in place.
    assert!(Package::open(&hot, Mode::ReadOnly, &Limits::DEFAULT).is_err());
    assert!(journal.exists());
    // Opening for editing rolls the journal back: the file holds the previous state again.
    assert_eq!(stored_state(&hot, [3, 3]), 3);
    assert!(!journal.exists());
    let package = Package::open(&hot, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    assert_eq!(package.metadata().save_counter, 4);
}

const TARGET: &str = "ARC_DOC_INCREMENTAL_TARGET";
const STATE: &str = "ARC_DOC_INCREMENTAL_STATE";

/// The child entry: one incremental save when started by the kill harness, nothing otherwise.
#[test]
fn kill_child() {
    if let (Ok(target), Ok(n)) = (std::env::var(TARGET), std::env::var(STATE)) {
        let saved = save_next(Path::new(&target), n.parse().unwrap());
        assert!(saved.incremental, "{:?}", saved.refused);
    }
}

fn child(target: &Path, n: u64) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["kill_child", "--exact", "--test-threads", "1", "--quiet"])
        .env(TARGET, target)
        .env(STATE, n.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

fn incremental_kill(iterations: u64) {
    let dir = TempDir::new("incremental kill ü");
    let target = dir.path("model ğ.arc");
    write_state(&target, 0);
    let started = Instant::now();
    assert!(child(&target, 0).status().unwrap().success());
    let full = started.elapsed();
    let mut current = stored_state(&target, [0, 1]);
    assert_eq!(current, 1);
    let mut seed = 0x1ce;
    let (mut old, mut new) = (0, 0);
    for _ in 0..iterations {
        let delay =
            full.mul_f64(f64::from(u32::try_from(next(&mut seed) % 1000).unwrap()) / 1000.0);
        let mut process = child(&target, current).spawn().unwrap();
        std::thread::sleep(delay);
        let _ = process.kill();
        process.wait().unwrap();
        let found = stored_state(&target, [current, current + 1]);
        if found == current {
            old += 1;
        } else {
            new += 1;
        }
        current = found;
    }
    assert!(
        // Which outcome a kill gives depends on the speed of the machine; the property is that
        // every outcome is one of the two states, which `stored_state` asserts above.
        old + new == iterations,
        "old {old}, new {new} in {iterations} runs of {full:?}"
    );
}

#[test]
fn incremental_kill_50() {
    incremental_kill(50);
}

#[test]
#[ignore = "the roadmap's 1,000 killed saves; run in a release build"]
fn incremental_kill_1000() {
    incremental_kill(1000);
}
