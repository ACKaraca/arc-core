//! Package compatibility fixtures (`fixtures/packages/`): every fixture of a released format
//! opens with the current code and its canonical dump equals the dump stored next to it; the
//! newer-major fixture is refused without changing it or creating a lock.
//!
//! Fixtures are never regenerated. `generate_fixtures` (ignored) writes only the files that do
//! not exist yet: `cargo nextest run -p arc-doc --features package --run-ignored only generate_fixtures`.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_base::id::{DocumentId, Identifier};
use arc_base::{Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::package::{
    Contents, Metadata, Mode, PREVIEW, Package, Save, THUMBNAIL, lock_path, save_as, write,
};
use arc_doc::{
    Cardinality, FieldDecl, Header, LoadOptions, Migration, Object, OnCopy, OnDelete,
    ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Snapshot, StoredObject, StoredType,
    Strength, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, WorkingSet, descriptor, payload,
};
use package_support::{TempDir, blake3_of, owner, preview, raw_types};
use serde_json::json;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/packages")
}

// The fixture types. They are part of the fixtures: never change them, add new types instead.

fn point() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("x", ValueType::F64),
            FieldDecl::new("y", ValueType::F64),
            FieldDecl::new("label", ValueType::String)
                .optional()
                .indexed(),
        ],
    )
}

fn point_reference() -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new("fx.point").unwrap()],
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Block,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::One,
        },
    })
}

fn line() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("start", point_reference()),
            FieldDecl::new("end", point_reference()),
        ],
    )
}

fn note_1() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("text", ValueType::String)],
    )
}

fn note_2() -> Schema {
    Schema::new(
        SchemaVersion::new(2, 0),
        vec![
            FieldDecl::new("text", ValueType::String),
            FieldDecl::new("size", ValueType::I64),
        ],
    )
}

fn note_upgrade(mut values: Vec<Option<Value>>) -> arc_base::Result<Vec<Option<Value>>> {
    values.push(Some(Value::I64(12)));
    Ok(values)
}

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    let key = |k: &str| TypeKey::new(k).unwrap();
    registry
        .register(TypeDecl::new(key("fx.point"), point()))
        .unwrap();
    registry
        .register(TypeDecl::new(key("fx.line"), line()))
        .unwrap();
    let mut note = TypeDecl::new(key("fx.note"), note_2());
    note.migrations = vec![Migration {
        source: note_1(),
        migrate: note_upgrade,
    }];
    registry.register(note).unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 | n)
}

fn metadata(name: &str) -> Metadata {
    Metadata {
        document_id: Some(DocumentId::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_00ff,
        )),
        created_by: Some("arc-doc fixtures 0.1.0".into()),
        saved_by: Some("arc-doc fixtures 0.1.0".into()),
        units: Some("m".into()),
        coordinate_reference: None,
        save_counter: 0,
        history_retention: Some("none".into()),
        partitions: vec![name.into()],
        thumbnail_missing: false,
        other: BTreeMap::new(),
    }
}

fn point_object(n: u128, x: f64, label: Option<&str>) -> Object {
    let values = vec![
        Some(Value::F64(x)),
        Some(Value::F64(-x)),
        label.map(|l| Value::String(l.into())),
    ];
    let header = Header {
        name: Some(format!("point {n}")),
        partition: Some("ground".into()),
        ..Header::default()
    };
    Object::new(id(n), SchemaVersion::new(1, 0), header, values)
}

/// `v1-minimal`: one point, saved as a new document.
fn write_minimal(path: &Path) {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    set.create("fx.point", point_object(1, 1.5, None)).unwrap();
    let touched: BTreeMap<_, _> = [(id(1), vec![0x11; 16])].into_iter().collect();
    let meta = metadata("ground");
    let save = Save {
        snapshot: set.current(),
        loaded: None,
        touched: &touched,
        metadata: &meta,
        previews: &[],
        journal: &[],
        diagnostics: &[],
        limits: Limits::DEFAULT,
    };
    save_as(path, &save, &mut |_| Ok(())).unwrap();
}

fn row(
    n: u128,
    key: &str,
    schema: &Schema,
    values: &[Option<Value>],
    owner: Option<u128>,
) -> StoredObject {
    let bytes = payload::encode(schema, values, &Limits::DEFAULT)
        .unwrap()
        .bytes;
    StoredObject {
        id: id(n).as_u128().to_be_bytes().to_vec(),
        type_key: key.into(),
        major: schema.version.major.into(),
        minor: schema.version.minor.into(),
        owner: owner.map(|o| id(o).as_u128().to_be_bytes().to_vec()),
        name: Some(format!("{key} {n}")),
        partition: Some("ground".into()),
        flags: 0,
        encoding: 0,
        payload_hash: blake3_of(&bytes),
        payload: bytes,
        last_txn: vec![u8::try_from(n).unwrap(); 16],
    }
}

/// `v1-typical`: points, a line, a note written at version 1.0 (migrated), an object of an
/// unknown type (preserved) and a corrupt point (quarantined), loaded and saved by the full
/// rewrite with a thumbnail and a preview.
fn write_typical(path: &Path, scratch: &Path) {
    let reg = registry();
    let decl = |k: &str| reg.get(k).unwrap();
    let f = |x: f64| Some(Value::F64(x));
    let reference = |n| Some(Value::Reference(id(n)));
    let rows = vec![
        row(
            1,
            "fx.point",
            &point(),
            &[f(0.0), f(0.0), Some(Value::String("origin".into()))],
            None,
        ),
        row(2, "fx.point", &point(), &[f(3.0), f(4.0), None], None),
        row(3, "fx.line", &line(), &[reference(1), reference(2)], None),
        row(
            4,
            "fx.note",
            &note_1(),
            &[Some(Value::String("a note".into()))],
            Some(3),
        ),
        StoredObject {
            major: 3,
            payload: vec![7, 7, 7],
            payload_hash: blake3_of(&[7, 7, 7]),
            ..row(
                5,
                "fx.legacy",
                &note_1(),
                &[Some(Value::String(String::new()))],
                None,
            )
        },
        StoredObject {
            payload: vec![0xFF],
            payload_hash: blake3_of(&[0xFF]),
            ..row(6, "fx.point", &point(), &[f(0.0), f(0.0), None], None)
        },
    ];
    let mut types: Vec<StoredType> = [
        ("fx.point", point().version),
        ("fx.line", line().version),
        ("fx.note", note_1().version),
    ]
    .into_iter()
    .map(|(key, version)| StoredType {
        type_key: key.into(),
        major: version.major.into(),
        minor: version.minor.into(),
        descriptor: descriptor(decl(key), version).unwrap(),
    })
    .collect();
    types.push(StoredType {
        type_key: "fx.legacy".into(),
        major: 3,
        minor: 0,
        descriptor: "{\"descriptor_format\": 1, \"type_key\": \"fx.legacy\", \"major\": 3, \"minor\": 0, \"fields\": []}".into(),
    });
    let meta = metadata("ground");
    let contents = Contents {
        metadata: &meta,
        types: &types,
        objects: &rows,
        previews: &[],
        snapshot: None,
    };
    write(scratch, &contents, &mut |_| Ok(())).unwrap();
    let loaded = Package::open(scratch, Mode::ReadOnly, &Limits::DEFAULT)
        .unwrap()
        .load(Arc::clone(&reg), &LoadOptions::default())
        .unwrap();
    let previews = [preview(THUMBNAIL, 256, 256), preview(PREVIEW, 1024, 8)];
    let touched = BTreeMap::new();
    let save = Save {
        snapshot: &loaded.snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: &meta,
        previews: &previews,
        journal: &[],
        diagnostics: &[],
        limits: Limits::DEFAULT,
    };
    save_as(path, &save, &mut |_| Ok(())).unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The canonical dump of a package: metadata, type rows, typed objects with their values
/// encoded at the current version, held rows, previews and the load's findings.
fn dump(path: &Path) -> String {
    let reg = registry();
    let package = Package::open(path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let loaded = package
        .load(Arc::clone(&reg), &LoadOptions::default())
        .unwrap();
    let meta = package.metadata();
    let snapshot = &loaded.snapshot;
    let objects: Vec<_> = snapshot
        .iter()
        .map(|(handle, object)| {
            let key = snapshot.type_key(handle).unwrap().as_str();
            let schema = &reg.get(key).unwrap().schema;
            let bytes = payload::encode(schema, &object.values, &Limits::DEFAULT)
                .unwrap()
                .bytes;
            let flags = &object.header.flags;
            json!({
                "id": object.id.to_string(),
                "type": key,
                "version": object.version.to_string(),
                "owner": object.header.owner.map(|o| o.to_string()),
                "name": object.header.name,
                "partition": object.header.partition,
                "flags": [flags.hidden_from_ui, flags.system, flags.read_only],
                "payload": hex(&bytes),
                "last_txn": hex(loaded.rows.last_txn(object.id).unwrap()),
            })
        })
        .collect();
    let held: Vec<_> = snapshot
        .held()
        .map(|(_, held)| {
            let row = held.row();
            json!({
                "id": hex(&row.id),
                "kind": format!("{:?}", held.kind()),
                "reason": held.reason().code(),
                "type": row.type_key,
                "version": format!("{}.{}", row.major, row.minor),
                "payload": hex(&row.payload),
                "last_txn": hex(&row.last_txn),
            })
        })
        .collect();
    let types: Vec<_> = raw_types(path)
        .into_iter()
        .map(|t| json!({"type": t.type_key, "version": format!("{}.{}", t.major, t.minor), "descriptor_blake3": hex(&blake3_of(t.descriptor.as_bytes()))}))
        .collect();
    let previews: Vec<_> = package
        .previews()
        .unwrap()
        .into_iter()
        .map(|p| json!({"kind": p.kind, "media_type": p.media_type, "width": p.width, "height": p.height, "blake3": hex(&blake3_of(&p.bytes))}))
        .collect();
    let diagnostics: Vec<_> = loaded
        .diagnostics
        .iter()
        .map(|d| json!({"kind": format!("{:?}", d.kind), "object": d.object.map(|o| o.to_string()), "code": d.error.code()}))
        .collect();
    let dump = json!({
        "format": package.format().to_string(),
        "metadata": {
            "document_id": meta.document_id.map(|d| d.to_string()),
            "created_by": meta.created_by,
            "saved_by": meta.saved_by,
            "units": meta.units,
            "coordinate_reference": meta.coordinate_reference,
            "save_counter": meta.save_counter,
            "history_retention": meta.history_retention,
            "partitions": meta.partitions,
            "thumbnail_missing": meta.thumbnail_missing,
        },
        "types": types,
        "objects": objects,
        "held": held,
        "previews": previews,
        "diagnostics": diagnostics,
    });
    serde_json::to_string_pretty(&dump).unwrap() + "\n"
}

#[test]
#[ignore = "writes missing fixtures; fixtures are never regenerated"]
fn generate_fixtures() {
    let dir = fixtures();
    std::fs::create_dir_all(&dir).unwrap();
    let scratch = TempDir::new("fixtures");
    let minimal = dir.join("v1-minimal.arc");
    if !minimal.exists() {
        write_minimal(&minimal);
    }
    let typical = dir.join("v1-typical.arc");
    if !typical.exists() {
        write_typical(&typical, &scratch.path("typical-input.arc"));
    }
    for name in ["v1-minimal", "v1-typical"] {
        let expected = dir.join(format!("{name}.json"));
        if !expected.exists() {
            std::fs::write(&expected, dump(&dir.join(format!("{name}.arc")))).unwrap();
        }
    }
    let newer = dir.join("newer-major.arc");
    if !newer.exists() {
        let path = scratch.path("newer.arc");
        write_minimal(&path);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(
                "PRAGMA user_version = 2; \
                 UPDATE arc_meta SET value = '2.0' WHERE key = 'format_version';",
            )
            .unwrap();
        std::fs::copy(&path, &newer).unwrap();
    }
}

#[test]
fn the_v1_fixtures_open_and_equal_their_dumps() {
    for name in ["v1-minimal", "v1-typical"] {
        let path = fixtures().join(format!("{name}.arc"));
        let expected = std::fs::read_to_string(fixtures().join(format!("{name}.json")))
            .unwrap_or_else(|_| panic!("{name}.json is missing: run generate_fixtures"));
        assert_eq!(dump(&path), expected.replace("\r\n", "\n"), "{name}");
    }
}

#[test]
fn the_typical_fixture_survives_another_save() {
    let dir = TempDir::new("fixture-resave");
    let source = fixtures().join("v1-typical.arc");
    let package = Package::open(&source, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let loaded = package.load(registry(), &LoadOptions::default()).unwrap();
    let previews = package.previews().unwrap();
    let touched = BTreeMap::new();
    let save = Save {
        snapshot: &loaded.snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: package.metadata(),
        previews: &previews,
        journal: &[],
        diagnostics: &[],
        limits: Limits::DEFAULT,
    };
    let copy = dir.path("copy.arc");
    save_as(&copy, &save, &mut |_| Ok(())).unwrap();
    // The same document, one save later.
    let counter = |text: String| text.replace("\"save_counter\": 2", "\"save_counter\": 1");
    assert_eq!(counter(dump(&copy)), dump(&source));
}

#[test]
fn the_newer_major_fixture_is_refused_untouched() {
    let original = fixtures().join("newer-major.arc");
    let error = Package::open(&original, Mode::ReadOnly, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), codes::PACKAGE_NEWER_FORMAT.code);
    // Opening a copy for editing changes neither its bytes nor its time and creates no lock.
    let dir = TempDir::new("fixture-refusal");
    let path = dir.path("newer-major.arc");
    std::fs::copy(&original, &path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let owner = package_support::owner("ann", "here");
    let error = Package::open(&path, Mode::Edit(&owner), &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), codes::PACKAGE_NEWER_FORMAT.code);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert!(!lock_path(&path).exists());
    assert_eq!(dir.files(), vec!["newer-major.arc".to_owned()]);
}

#[test]
fn a_fixture_written_before_the_stored_history_opens_and_gains_one() {
    let dir = TempDir::new("fixture-history");
    let path = dir.path("model.arc");
    std::fs::copy(fixtures().join("v1-typical.arc"), &path).unwrap();
    // The fixture keeps no history; keep all of it from now on.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE arc_meta SET value = 'full' WHERE key = 'history_retention'",
            [],
        )
        .unwrap();
    let mut package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
    let document = package
        .open_document(
            registry(),
            Arc::new(arc_doc::graph::Evaluators::new()),
            &LoadOptions::default(),
            arc_doc::DocumentConfig::default(),
        )
        .unwrap();
    assert!(document.stored_journal().is_empty());
    assert!(document.loaded_rows().foreign().is_empty());
    assert!(document.loaded_rows().diagnostics().is_empty());
    let point = point_object(900, 2.0, None);
    document
        .submit(move |txn| {
            txn.create(
                "fx.point",
                point.id,
                point.header.clone(),
                point.values.clone(),
            )
            .map(drop)
        })
        .wait()
        .unwrap();
    let saved = package
        .save_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();
    assert!(saved.incremental, "{:?}", saved.refused);
    assert_eq!(document.stored_journal().len(), 1);
    // The fixture's own findings (it holds invalid rows on purpose) stay the only ones.
    let codes_of = |path: &Path| -> Vec<&'static str> {
        arc_doc::package::check(path, &Limits::DEFAULT)
            .unwrap()
            .iter()
            .map(arc_base::Error::code)
            .collect()
    };
    assert_eq!(
        codes_of(&path),
        codes_of(&fixtures().join("v1-typical.arc"))
    );
    drop(document);
    drop(package);
    let loaded = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT)
        .unwrap()
        .load(registry(), &LoadOptions::default())
        .unwrap();
    assert_eq!(loaded.rows.journal().len(), 1);
}
