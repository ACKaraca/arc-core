//! Background save (CORE-DOC-045): a save of a snapshot runs on a storage thread with progress
//! while commits continue, and saves exactly that snapshot; cancellation and a panicking
//! progress callback leave the file as it was and hand the package back.

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
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::package::{
    BackgroundSave, Metadata, Mode, Package, Save, SaveProgress, SaveRequest, SaveStep, Strategy,
    save_as, touched_between,
};
use arc_doc::{
    FieldDecl, Header, LoadOptions, Loaded, Object, Schema, SchemaVersion, Snapshot, SnapshotCell,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType, WorkingSet,
};
use package_support::{TempDir, objects, owner};

const N: u128 = 3000;

fn registry() -> Arc<TypeRegistry> {
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

fn object(id: u128, v: i64) -> Object {
    Object::new(
        ObjectId::from_u128(id),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![
            Some(Value::I64(v)),
            Some(Value::String(format!("{id} {}", "y".repeat(100)))),
        ],
    )
}

/// A package of `N` objects at `path`, opened for editing and loaded.
fn prepare(path: &Path) -> (Package, Arc<Loaded>) {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    for id in 1..=N {
        set.create("k.n", object(id, 0)).unwrap();
    }
    let snapshot = set.current().clone();
    let touched = snapshot
        .iter()
        .map(|(_, object)| (object.id, vec![0; 16]))
        .collect();
    let metadata = Metadata::default();
    let save = Save {
        snapshot: &snapshot,
        loaded: None,
        touched: &touched,
        metadata: &metadata,
        previews: &[],
        journal: &[],
        diagnostics: &[],
        limits: Limits::DEFAULT,
    };
    save_as(path, &save, &mut |_| Ok(())).unwrap();
    let package = Package::open(path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
    let loaded = package.load(registry(), &LoadOptions::default()).unwrap();
    (package, Arc::new(loaded))
}

/// Sets `v` of every object whose identifier is a multiple of `step` to `v`, as one commit.
fn commit(cell: &SnapshotCell, step: u128, v: i64, touched: &mut BTreeMap<ObjectId, Vec<u8>>) {
    let base = cell.load();
    let mut set = WorkingSet::new(&base);
    for id in (step..=N).step_by(usize::try_from(step).unwrap()) {
        let handle = base.resolve(ObjectId::from_u128(id)).unwrap();
        let mut values = base.get(handle).unwrap().values.clone();
        values[0] = Some(Value::I64(v));
        set.replace_values(handle, values).unwrap();
        touched.insert(ObjectId::from_u128(id), v.to_be_bytes().to_vec());
    }
    cell.publish(&set).unwrap();
}

fn request(
    loaded: &Arc<Loaded>,
    snapshot: Snapshot,
    touched: BTreeMap<ObjectId, Vec<u8>>,
    metadata: Metadata,
    strategy: Strategy,
) -> SaveRequest {
    SaveRequest {
        snapshot,
        loaded: Some(Arc::clone(loaded)),
        touched,
        metadata,
        previews: Vec::new(),
        journal: Vec::new(),
        diagnostics: Vec::new(),
        limits: Limits::DEFAULT,
        strategy,
    }
}

#[test]
fn a_background_save_writes_its_snapshot_while_commits_continue() {
    let dir = TempDir::new("background commits ü");
    let path = dir.path("model ğ.arc");
    let (mut package, loaded) = prepare(&path);
    let cell = SnapshotCell::new(loaded.snapshot.clone());
    let mut touched = BTreeMap::new();
    for (step, strategy) in [(2, "incremental"), (3, "rewrite")] {
        commit(&cell, step, 1, &mut touched);
        let saving = cell.load();
        let strategy = if strategy == "incremental" {
            Strategy::Incremental(touched_between(&loaded.snapshot, &saving))
        } else {
            Strategy::Rewrite
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let metadata = package.metadata().clone();
        let (gate, wait) = mpsc::channel::<()>();
        let mut gate_once = Some(wait);
        let background = BackgroundSave::start(
            package,
            request(&loaded, saving.clone(), touched.clone(), metadata, strategy),
            move |event| {
                // The save waits at its object step until the commits below are published.
                if event == SaveProgress::Step(SaveStep::Objects)
                    && let Some(wait) = gate_once.take()
                {
                    let _ = wait.recv();
                }
                sink.lock().unwrap().push(event);
            },
        );
        for v in 2..20 {
            commit(&cell, 5, v, &mut touched);
        }
        gate.send(()).unwrap();
        let finished = background.join().unwrap();
        let saved = finished.result.unwrap();
        package = finished.package;
        assert_eq!(saved.incremental, step == 2);
        assert_eq!(package.metadata(), &saved.metadata);
        let events = events.lock().unwrap().clone();
        assert!(
            events.contains(&SaveProgress::Step(SaveStep::Commit)),
            "{events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(e, SaveProgress::Rows(_))),
            "{events:?}"
        );
        let reread = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
        assert_eq!(reread.metadata(), &saved.metadata);
        let stored = reread.load(registry(), &LoadOptions::default()).unwrap();
        assert_eq!(objects(&stored.snapshot), objects(&saving));
        assert_ne!(objects(&stored.snapshot), objects(&cell.load()));
    }
    drop(package);
}

#[test]
fn a_cancelled_or_failed_save_leaves_the_file_unchanged() {
    let dir = TempDir::new("background cancel");
    let path = dir.path("cancel.arc");
    let (mut package, loaded) = prepare(&path);
    let before = std::fs::read(&path).unwrap();
    let cell = SnapshotCell::new(loaded.snapshot.clone());
    let mut touched = BTreeMap::new();
    commit(&cell, 1, 7, &mut touched);
    let snapshot = cell.load();
    let strategies = [
        Strategy::Incremental(touched_between(&loaded.snapshot, &snapshot)),
        Strategy::Rewrite,
        Strategy::Compact,
    ];
    for strategy in strategies {
        for at in [SaveStep::Prepare, SaveStep::Objects, SaveStep::Commit] {
            let metadata = package.metadata().clone();
            let (reached, on_reached) = mpsc::channel::<()>();
            let (resume, wait) = mpsc::channel::<()>();
            let background = BackgroundSave::start(
                package,
                request(
                    &loaded,
                    snapshot.clone(),
                    touched.clone(),
                    metadata.clone(),
                    strategy.clone(),
                ),
                move |event| {
                    if event == SaveProgress::Step(at) {
                        reached.send(()).unwrap();
                        wait.recv().unwrap();
                    }
                },
            );
            on_reached.recv().unwrap();
            background.cancel();
            resume.send(()).unwrap();
            let finished = background.join().unwrap();
            assert_eq!(
                finished.result.unwrap_err().code(),
                arc_base::error::codes::CANCELLED.code,
                "{strategy:?} {at:?}"
            );
            package = finished.package;
            assert_eq!(package.metadata(), &metadata);
            assert!(package.is_editable());
            assert_eq!(std::fs::read(&path).unwrap(), before, "{strategy:?} {at:?}");
            let mut files = dir.files();
            files.retain(|name| !name.ends_with(".lock"));
            assert_eq!(files, vec!["cancel.arc".to_owned()]);
        }
    }
    // A save whose progress callback panics fails with its own code and hands the package back.
    let metadata = package.metadata().clone();
    let background = BackgroundSave::start(
        package,
        request(&loaded, snapshot, touched, metadata, Strategy::Rewrite),
        |event| {
            if event == SaveProgress::Step(SaveStep::Verify) {
                panic!("injected");
            }
        },
    );
    let finished = background.join().unwrap();
    assert_eq!(
        finished.result.unwrap_err().code(),
        codes::PACKAGE_SAVE_PANICKED.code
    );
    assert!(finished.package.is_editable());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut files = dir.files();
    files.retain(|name| !name.ends_with(".lock"));
    assert_eq!(files, vec!["cancel.arc".to_owned()]);
}
