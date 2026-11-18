//! Shared helpers of the `package_*` tests: temporary directories, PNG images, the test types
//! of the write-back tests, random stored documents and canonical dumps.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::type_complexity,
    reason = "test helpers; each test file uses a part of them"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::package::{LockOwner, PNG, Preview};
use arc_doc::{
    Cardinality, FieldDecl, Finding, Header, Loaded, Migration, OnCopy, OnDelete, ReferenceSpec,
    RelationshipRule, Schema, SchemaVersion, Severity, Snapshot, StoredObject, StoredType,
    Strength, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, payload,
};
use proptest::prelude::*;

/// A directory removed when dropped.
pub struct TempDir(pub PathBuf);

static DIRS: AtomicU64 = AtomicU64::new(0);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let n = DIRS.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("arc-doc-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// The names of the files in the directory, sorted.
    pub fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn owner(user: &str, host: &str) -> LockOwner {
    LockOwner {
        user: user.into(),
        host: host.into(),
        application: "arc-doc tests 0".into(),
        time_unix_ms: 1_700_000_000_000,
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
    let mut typed = kind.to_vec();
    typed.extend(data);
    out.extend(&typed);
    out.extend(crc32(&typed).to_be_bytes());
}

/// A valid white 1-bit grayscale PNG of the size, stored without compression.
pub fn png(width: u32, height: u32) -> Vec<u8> {
    let row = 1 + width.div_ceil(8) as usize;
    let mut raw = Vec::new();
    for _ in 0..height {
        raw.push(0);
        raw.extend(std::iter::repeat_n(0xFF, row - 1));
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (at, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(at + 1 == blocks.len()));
        let len = u16::try_from(block.len()).unwrap();
        zlib.extend(len.to_le_bytes());
        zlib.extend((!len).to_le_bytes());
        zlib.extend(*block);
    }
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([1, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn preview(kind: &str, width: u32, height: u32) -> Preview {
    Preview {
        kind: kind.into(),
        media_type: PNG.into(),
        width,
        height,
        bytes: png(width, height),
    }
}

// The types of the write-back tests: `t.a` at 3.1 with migrations from 1.x and 2.0, `t.b` with
// a reference, and `x.u`, never registered.

pub fn any_reference() -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: Vec::new(),
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    })
}

pub fn a1() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 1),
        vec![
            FieldDecl::new("x", ValueType::I64),
            FieldDecl::new("y", ValueType::I64)
                .optional()
                .since_minor(1),
        ],
    )
}

pub fn a2() -> Schema {
    Schema::new(
        SchemaVersion::new(2, 0),
        vec![
            FieldDecl::new("x", ValueType::I64),
            FieldDecl::new("tag", ValueType::String),
        ],
    )
}

pub fn a3() -> Schema {
    Schema::new(
        SchemaVersion::new(3, 1),
        vec![
            FieldDecl::new("x", ValueType::I64).indexed(),
            FieldDecl::new("tag", ValueType::String),
            FieldDecl::new("z", ValueType::I64)
                .optional()
                .since_minor(1),
        ],
    )
}

pub fn a3_2() -> Schema {
    let mut fields = a3().fields;
    fields.push(
        FieldDecl::new("w", ValueType::I64)
            .optional()
            .since_minor(2),
    );
    Schema::new(SchemaVersion::new(3, 2), fields)
}

pub fn b1() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("r", any_reference()).optional()],
    )
}

fn one_to_two(values: Vec<Option<Value>>) -> arc_base::Result<Vec<Option<Value>>> {
    match values.first() {
        Some(Some(Value::I64(13))) => Err(Error::new(codes::VALUE_OUT_OF_RANGE)),
        Some(Some(Value::I64(x))) => {
            Ok(vec![Some(Value::I64(*x)), Some(Value::String("m".into()))])
        }
        _ => Err(Error::new(codes::VALUE_TYPE_MISMATCH)),
    }
}

fn two_to_three(mut values: Vec<Option<Value>>) -> arc_base::Result<Vec<Option<Value>>> {
    values.push(None);
    Ok(values)
}

fn validator(values: &[Option<Value>]) -> Vec<Finding> {
    match values.first() {
        Some(Some(Value::I64(99))) => vec![Finding {
            severity: Severity::Error,
            error: Error::new(codes::VALUE_OUT_OF_RANGE),
        }],
        _ => Vec::new(),
    }
}

pub fn decl_a() -> TypeDecl {
    let mut decl = TypeDecl::new(TypeKey::new("t.a").unwrap(), a3());
    decl.migrations = vec![
        Migration {
            source: a1(),
            migrate: one_to_two,
        },
        Migration {
            source: a2(),
            migrate: two_to_three,
        },
    ];
    decl.validators = vec![validator];
    decl
}

pub fn decl_b() -> TypeDecl {
    TypeDecl::new(TypeKey::new("t.b").unwrap(), b1())
}

/// The registry of `t.a`, and of `t.b` when `with_b`.
pub fn registry(with_b: bool) -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    registry.register(decl_a()).unwrap();
    if with_b {
        registry.register(decl_b()).unwrap();
    }
    Arc::new(registry)
}

pub fn stored_type(key: &str, major: i64, minor: i64) -> StoredType {
    StoredType {
        type_key: key.into(),
        major,
        minor,
        descriptor: format!("{{\"stand-in\":\"{key} {major}.{minor}\"}}"),
    }
}

pub fn types(newer_minor: bool) -> Vec<StoredType> {
    let mut types: Vec<StoredType> = [
        ("t.a", 1, 0),
        ("t.a", 1, 1),
        ("t.a", 2, 0),
        ("t.a", 3, 0),
        ("t.a", 3, 1),
        ("t.b", 1, 0),
        ("x.u", 4, 2),
    ]
    .into_iter()
    .map(|(key, major, minor)| stored_type(key, major, minor))
    .collect();
    if newer_minor {
        types.push(stored_type("t.a", 3, 2));
    }
    types
}

pub fn blake3_of(bytes: &[u8]) -> Vec<u8> {
    blake3::hash(bytes).as_bytes().to_vec()
}

fn encode(schema: &Schema, values: &[Option<Value>]) -> Vec<u8> {
    payload::encode(schema, values, &Limits::DEFAULT)
        .unwrap()
        .bytes
}

pub const KINDS: u8 = 13;

/// A stored row of case `kind` with identifier `n`: current, older minor and major versions,
/// failing migrations and validators, unknown types, corrupt payloads, an unaddressable
/// identifier, invalid header columns, an unsupported encoding and (kind 12) the newer minor
/// version 3.2.
pub fn stored(kind: u8, n: u128, x: i64, owner: Option<u128>, txn: u8) -> StoredObject {
    let id = |n: u128| n.to_be_bytes().to_vec();
    let mut row = StoredObject {
        id: id(n),
        owner: owner.map(id),
        name: Some(format!("n{n}")),
        partition: n.is_multiple_of(3).then(|| "p".into()),
        flags: i64::try_from(n % 8).unwrap(),
        last_txn: vec![txn; 16],
        ..StoredObject::default()
    };
    let x = || Some(Value::I64(x));
    let tag = || Some(Value::String("t".into()));
    let (key, major, minor, payload) = match kind % KINDS {
        0 => {
            let v1_0 = Schema::new(SchemaVersion::new(1, 0), a1().fields_at(0).to_vec());
            ("t.a", 1, 0, encode(&v1_0, &[x()]))
        }
        1 => ("t.a", 1, 1, encode(&a1(), &[x(), Some(Value::I64(1))])),
        2 => ("t.a", 2, 0, encode(&a2(), &[x(), tag()])),
        3 | 4 => (
            "t.a",
            3,
            1,
            encode(&a3(), &[x(), tag(), Some(Value::I64(2))]),
        ),
        5 => {
            let reference = owner.map(|o| Value::Reference(ObjectId::from_u128(o)));
            ("t.b", 1, 0, encode(&b1(), &[reference]))
        }
        6 => ("x.u", 4, 2, vec![1, 2, 3, n.to_be_bytes()[15]]),
        7 => ("t.a", 2, 5, vec![0]),
        8 => ("t.a", 3, 1, vec![0xFF; 3]),
        9 => {
            row.id = vec![1, 2, u8::try_from(n % 200).unwrap()];
            ("t.b", 1, 0, vec![0])
        }
        10 => {
            row.flags = 64;
            ("t.b", 1, 0, encode(&b1(), &[None]))
        }
        11 => {
            row.encoding = 7;
            ("t.b", 1, 0, encode(&b1(), &[None]))
        }
        _ => {
            let values = [x(), tag(), None, Some(Value::I64(4))];
            ("t.a", 3, 2, encode(&a3_2(), &values))
        }
    };
    row.type_key = key.into();
    row.major = major;
    row.minor = minor;
    row.payload_hash = blake3_of(&payload);
    row.payload = payload;
    row
}

/// A stored document: whether it declares the newer minor version, and rows with distinct
/// identifiers.
pub fn document() -> impl Strategy<Value = (bool, Vec<StoredObject>)> {
    let row = (
        0..KINDS,
        1_u128..40,
        prop_oneof![Just(13_i64), Just(99), -5_i64..5],
        prop::option::of(1_u128..40),
        0_u8..3,
    );
    (any::<bool>(), prop::collection::vec(row, 0..40)).prop_map(|(newer_minor, rows)| {
        let mut seen = std::collections::BTreeSet::new();
        let rows = rows
            .into_iter()
            .filter(|(kind, ..)| newer_minor || *kind != KINDS - 1)
            .map(|(kind, n, x, owner, txn)| stored(kind, n, x, owner, txn))
            .filter(|row| seen.insert(row.id.clone()))
            .collect();
        (newer_minor, rows)
    })
}

/// The objects of the typed stores by identifier: type key, header, values.
pub fn objects(snapshot: &Snapshot) -> BTreeMap<ObjectId, (String, Header, Vec<Option<Value>>)> {
    snapshot
        .iter()
        .map(|(handle, object)| {
            let key = snapshot.type_key(handle).unwrap().as_str().to_owned();
            (
                object.id,
                (key, object.header.clone(), object.values.clone()),
            )
        })
        .collect()
}

pub fn sorted(mut rows: Vec<StoredObject>) -> Vec<StoredObject> {
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// The held rows of a snapshot, by identifier bytes.
pub fn held_rows(snapshot: &Snapshot) -> Vec<StoredObject> {
    sorted(
        snapshot
            .held()
            .map(|(_, held)| held.row().clone())
            .collect(),
    )
}

/// Everything a load yields that a save must keep: typed objects with their `last_txn`, held
/// rows, unaddressable rows and the read-only reasons.
#[derive(Debug, PartialEq)]
pub struct Dump {
    pub objects: BTreeMap<ObjectId, (String, Header, Vec<Option<Value>>, Vec<u8>)>,
    pub held: Vec<StoredObject>,
    pub unaddressable: Vec<StoredObject>,
    pub read_only: usize,
}

pub fn dump(loaded: &Loaded) -> Dump {
    let snapshot = &loaded.snapshot;
    Dump {
        objects: objects(snapshot)
            .into_iter()
            .map(|(id, (key, header, values))| {
                let txn = loaded.rows.last_txn(id).unwrap_or_default().to_vec();
                (id, (key, header, values, txn))
            })
            .collect(),
        held: held_rows(snapshot),
        unaddressable: sorted(loaded.rows.unaddressable().to_vec()),
        read_only: snapshot.read_only().len(),
    }
}

/// The raw rows of `arc_object` of a package file.
pub fn raw_rows(path: &Path) -> Vec<StoredObject> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = conn
        .prepare(
            "SELECT id, type_key, schema_major, schema_minor, owner, name, partition, flags, \
             encoding, payload, payload_hash, last_txn FROM arc_object ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok(StoredObject {
                id: row.get(0)?,
                type_key: row.get(1)?,
                major: row.get(2)?,
                minor: row.get(3)?,
                owner: row.get(4)?,
                name: row.get(5)?,
                partition: row.get(6)?,
                flags: row.get(7)?,
                encoding: row.get(8)?,
                payload: row.get(9)?,
                payload_hash: row.get(10)?,
                last_txn: row.get(11)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// The raw rows of `arc_type` of a package file.
pub fn raw_types(path: &Path) -> Vec<StoredType> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = conn
        .prepare("SELECT type_key, major, minor, descriptor FROM arc_type ORDER BY 1, 2, 3")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok(StoredType {
                type_key: row.get(0)?,
                major: row.get(1)?,
                minor: row.get(2)?,
                descriptor: row.get(3)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

pub fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(default)
}
