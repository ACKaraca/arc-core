//! Content-addressed assets (CORE-DOC-047): round trips around the 4 MiB chunking threshold,
//! deduplication of whole assets and of shared chunks, detection of damaged chunks in the file,
//! hostile asset tables, the compaction of unreferenced assets and the survival of assets across
//! every save strategy.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::package::{
    CHUNK_MAX, Metadata, Mode, Package, Save, SaveProgress, Strategy, WHOLE_MAX, asset_values,
};
use arc_doc::{
    AssetHash, FieldDecl, Header, LoadOptions, Object, Schema, SchemaVersion, Snapshot, TypeDecl,
    TypeKey, TypeRegistry, Value, ValueType, WorkingSet,
};
use package_support::{TempDir, owner};

/// Deterministic pseudo-random bytes.
fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[3]
        })
        .collect()
}

fn create(dir: &TempDir, name: &str) -> Package {
    Package::create(&dir.path(name), &Metadata::default(), &owner("u", "h")).unwrap()
}

fn count(path: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn chunks_of(path: &Path, hash: &AssetHash) -> Vec<Vec<u8>> {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.prepare("SELECT chunk FROM arc_asset_chunk WHERE asset = ?1 ORDER BY ordinal")
        .unwrap()
        .query_map([hash.0], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn assets_round_trip_around_the_chunking_threshold() {
    let dir = TempDir::new("asset round trip ü");
    let package = create(&dir, "assets ğ.arc");
    for (seed, len) in [
        (1, 0),
        (2, 1),
        (3, 1000),
        (4, WHOLE_MAX - 1),
        (5, WHOLE_MAX),
        (6, WHOLE_MAX + 1),
        (7, 11 * 1024 * 1024 + 7),
    ] {
        let content = bytes(seed, len);
        let hash = package
            .put_asset("application/octet-stream", &mut content.as_slice())
            .unwrap();
        assert_eq!(hash.0, *blake3::hash(&content).as_bytes());
        assert_eq!(package.asset_bytes(&hash).unwrap(), content, "{len}");
        let mut reader = package.asset(&hash).unwrap();
        assert_eq!(reader.len(), len as u64);
        assert_eq!(reader.media_type(), "application/octet-stream");
        let mut streamed = Vec::new();
        reader.read_to_end(&mut streamed).unwrap();
        assert_eq!(streamed, content);
        let chunks = chunks_of(package.path(), &hash);
        if len <= WHOLE_MAX {
            assert!(chunks.is_empty(), "{len}");
        } else {
            assert!(chunks.len() >= 2, "{len}");
        }
    }
}

#[test]
fn identical_content_is_stored_once_and_shared_runs_share_chunks() {
    let dir = TempDir::new("asset dedup");
    let package = create(&dir, "dedup.arc");
    let path = package.path().to_path_buf();
    let small = bytes(10, 5000);
    let a = package
        .put_asset("image/png", &mut small.as_slice())
        .unwrap();
    let b = package
        .put_asset("image/png", &mut small.as_slice())
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(
        (count(&path, "arc_asset"), count(&path, "arc_chunk")),
        (1, 1)
    );

    let run = bytes(11, 8 * 1024 * 1024);
    let mut first = run.clone();
    first.extend(bytes(12, 2 * 1024 * 1024));
    let mut second = bytes(13, 1024 * 1024 + 333);
    second.extend(&run);
    let x = package.put_asset("model/x", &mut first.as_slice()).unwrap();
    let chunks_after_first = count(&path, "arc_chunk");
    let again = package.put_asset("model/x", &mut first.as_slice()).unwrap();
    assert_eq!(x, again);
    assert_eq!(count(&path, "arc_chunk"), chunks_after_first);
    let y = package
        .put_asset("model/x", &mut second.as_slice())
        .unwrap();
    let (cx, cy) = (chunks_of(&path, &x), chunks_of(&path, &y));
    let shared = cx.iter().filter(|chunk| cy.contains(chunk)).count();
    assert!(
        shared >= 3,
        "shared {shared} of {} and {}",
        cx.len(),
        cy.len()
    );
    let distinct = count(&path, "arc_chunk") - 1;
    assert_eq!(
        distinct,
        i64::try_from(cx.len() + cy.len() - shared).unwrap()
    );
    assert_eq!(package.asset_bytes(&y).unwrap(), second);
}

/// Flips one byte of `needle`'s first occurrence in the file.
fn flip_in_file(path: &Path, needle: &[u8]) {
    let mut file = std::fs::read(path).unwrap();
    let at = file
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap();
    file[at + needle.len() / 2] ^= 0x40;
    std::fs::write(path, file).unwrap();
}

#[test]
fn a_damaged_chunk_is_reported_and_never_returned() {
    let dir = TempDir::new("asset damage");
    let package = create(&dir, "damage.arc");
    let path = package.path().to_path_buf();
    let whole = bytes(20, 3000);
    let small = package
        .put_asset("text/plain", &mut whole.as_slice())
        .unwrap();
    let large_content = bytes(21, 9 * 1024 * 1024);
    let large = package
        .put_asset("text/plain", &mut large_content.as_slice())
        .unwrap();
    let other = bytes(22, 100);
    let fine = package
        .put_asset("text/plain", &mut other.as_slice())
        .unwrap();
    flip_in_file(&path, &whole[1000..1064]);
    // A chunk past the first of the large asset.
    let list = chunks_of(&path, &large);
    let first_len = {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let len: i64 = conn
            .query_row(
                "SELECT length(bytes) FROM arc_chunk WHERE hash = ?1",
                [&list[0]],
                |row| row.get(0),
            )
            .unwrap();
        usize::try_from(len).unwrap()
    };
    flip_in_file(&path, &large_content[first_len + 5000..first_len + 5064]);

    let error = package.asset_bytes(&small).unwrap_err();
    assert_eq!(error.code(), codes::PACKAGE_ASSET_CORRUPT.code);
    let mut reader = package.asset(&small).unwrap();
    let mut out = Vec::new();
    let failure = reader.read_to_end(&mut out).unwrap_err();
    assert_eq!(failure.kind(), std::io::ErrorKind::InvalidData);
    assert!(out.is_empty());

    assert_eq!(
        package.asset_bytes(&large).unwrap_err().code(),
        codes::PACKAGE_ASSET_CORRUPT.code
    );
    let mut reader = package.asset(&large).unwrap();
    assert_eq!(reader.next_chunk().unwrap().unwrap().len(), first_len);
    assert_eq!(
        reader.next_chunk().unwrap_err().code(),
        codes::PACKAGE_ASSET_CORRUPT.code
    );
    // The damage is confined to the damaged assets.
    assert_eq!(package.asset_bytes(&fine).unwrap(), other);
}

/// Copies the package at `path`, changes the copy with `statement` and returns the code of
/// reading `hash` from it (`ok` when the asset reads back).
fn tampered(dir: &TempDir, path: &Path, hash: &AssetHash, statement: &str) -> String {
    let case = dir.path("case.arc");
    std::fs::copy(path, &case).unwrap();
    let conn = rusqlite::Connection::open(&case).unwrap();
    conn.execute_batch(statement).unwrap();
    drop(conn);
    let package = Package::open(&case, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    package
        .asset_bytes(hash)
        .and_then(|_| {
            let mut reader = package.asset(hash)?;
            while reader.next_chunk()?.is_some() {}
            Ok(())
        })
        .map_or_else(|e| e.code().to_owned(), |()| "ok".to_owned())
}

#[test]
fn hostile_asset_tables_fail_with_codes() {
    let dir = TempDir::new("asset hostile");
    let package = create(&dir, "hostile.arc");
    let path = package.path().to_path_buf();
    let content = bytes(30, 6 * 1024 * 1024);
    let hash = package.put_asset("a/b", &mut content.as_slice()).unwrap();
    let corrupt = codes::PACKAGE_ASSET_CORRUPT.code;
    let missing = AssetHash([7; 32]);
    assert_eq!(
        package.asset_bytes(&missing).unwrap_err().code(),
        codes::PACKAGE_ASSET_NOT_FOUND.code
    );
    for (statement, code) in [
        ("UPDATE arc_asset SET byte_length = -1", corrupt),
        ("UPDATE arc_asset SET byte_length = 'x'", corrupt),
        (
            "UPDATE arc_asset SET byte_length = byte_length + 1",
            corrupt,
        ),
        ("UPDATE arc_asset SET byte_length = 1", corrupt),
        ("UPDATE arc_asset SET chunked = 2", corrupt),
        ("UPDATE arc_asset SET chunked = 0", corrupt),
        ("UPDATE arc_asset SET media_type = x'00'", corrupt),
        (
            "UPDATE arc_asset SET encoding = 3",
            codes::PAYLOAD_ENCODING_UNSUPPORTED.code,
        ),
        (
            "UPDATE arc_chunk SET encoding = 9",
            codes::PAYLOAD_ENCODING_UNSUPPORTED.code,
        ),
        ("UPDATE arc_chunk SET encoding = 'z'", corrupt),
        ("UPDATE arc_chunk SET bytes = 'text'", corrupt),
        ("UPDATE arc_chunk SET bytes = zeroblob(5000000)", corrupt),
        ("DELETE FROM arc_chunk", corrupt),
        (
            "UPDATE arc_asset_chunk SET ordinal = ordinal + 100",
            corrupt,
        ),
        ("UPDATE arc_asset_chunk SET chunk = x'01'", corrupt),
        ("UPDATE arc_asset_chunk SET chunk = 5", corrupt),
        ("DELETE FROM arc_asset_chunk WHERE ordinal = 1", corrupt),
        (
            "UPDATE arc_asset_chunk SET chunk = \
             (SELECT chunk FROM arc_asset_chunk WHERE ordinal = 1) WHERE ordinal = 0",
            corrupt,
        ),
        (
            "UPDATE arc_asset SET byte_length = byte_length - 1",
            corrupt,
        ),
        ("DELETE FROM arc_asset", codes::PACKAGE_ASSET_NOT_FOUND.code),
    ] {
        assert_eq!(tampered(&dir, &path, &hash, statement), code, "{statement}");
    }
    assert_eq!(package.asset_bytes(&hash).unwrap(), content);
    let tight = Limits {
        input_bytes: 1024,
        ..Limits::DEFAULT
    };
    let small = Package::open(&path, Mode::ReadOnly, &tight).unwrap();
    assert_eq!(
        small.asset_bytes(&hash).unwrap_err().code(),
        codes::LOAD_LIMIT_EXCEEDED.code
    );
    // A read-only package stores nothing.
    let error = small.put_asset("a/b", &mut [1_u8].as_slice()).unwrap_err();
    assert_eq!(error.code(), codes::DOCUMENT_READ_ONLY.code);
    assert_eq!(
        package
            .put_asset("", &mut [1_u8].as_slice())
            .unwrap_err()
            .code(),
        codes::VALUE_OUT_OF_RANGE.code
    );
}

fn asset_registry() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("main", ValueType::Asset).optional(),
            FieldDecl::new("more", ValueType::List(Box::new(ValueType::Asset))).optional(),
        ],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("k.asset").unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn holder(id: u128, main: Option<AssetHash>, more: Vec<AssetHash>) -> Object {
    Object::new(
        ObjectId::from_u128(id),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![
            main.map(|hash| Value::Asset(Box::new(hash))),
            Some(Value::List(
                more.into_iter()
                    .map(|hash| Value::Asset(Box::new(hash)))
                    .collect(),
            )),
        ],
    )
}

fn save_with(package: &mut Package, snapshot: &Snapshot, strategy: &Strategy) {
    let touched: BTreeMap<_, _> = snapshot
        .iter()
        .map(|(_, object)| (object.id, vec![1; 16]))
        .collect();
    let metadata = package.metadata().clone();
    let save = Save {
        snapshot,
        loaded: None,
        touched: &touched,
        metadata: &metadata,
        previews: &[],
        journal: &[],
        diagnostics: &[],
        limits: Limits::DEFAULT,
    };
    package
        .save(&save, strategy, &mut |_: SaveProgress| Ok(()))
        .unwrap();
}

#[test]
fn saves_keep_assets_and_compaction_removes_unreferenced_ones() {
    let dir = TempDir::new("asset gc");
    let mut package = create(&dir, "gc model.arc");
    let path = package.path().to_path_buf();
    let contents: Vec<Vec<u8>> = vec![
        bytes(40, 2000),
        bytes(41, 5 * 1024 * 1024),
        bytes(42, 3000),
        bytes(43, 6 * 1024 * 1024),
    ];
    let hashes: Vec<AssetHash> = contents
        .iter()
        .map(|c| package.put_asset("x/y", &mut c.as_slice()).unwrap())
        .collect();
    let mut set = WorkingSet::new(&Snapshot::empty(asset_registry()));
    set.create("k.asset", holder(1, Some(hashes[0]), vec![hashes[1]]))
        .unwrap();
    let snapshot = set.current().clone();
    assert_eq!(asset_values(&snapshot).len(), 2);
    let changes = arc_doc::package::touched_between(&Snapshot::empty(asset_registry()), &snapshot);
    for strategy in [Strategy::Incremental(changes), Strategy::Rewrite] {
        save_with(&mut package, &snapshot, &strategy);
        assert_eq!(count(&path, "arc_asset"), 4, "{strategy:?}");
    }
    let copy = dir.path("copy.arc");
    save_with(&mut package, &snapshot, &Strategy::Copy(copy.clone()));
    assert_eq!(count(&copy, "arc_asset"), 4);
    let chunks = count(&path, "arc_chunk");
    save_with(&mut package, &snapshot, &Strategy::Compact);
    assert_eq!(count(&path, "arc_asset"), 2);
    assert!(count(&path, "arc_chunk") < chunks);
    for (at, hash) in hashes.iter().enumerate() {
        let read = package.asset_bytes(hash);
        if at < 2 {
            assert_eq!(read.unwrap(), contents[at]);
        } else {
            assert_eq!(
                read.unwrap_err().code(),
                codes::PACKAGE_ASSET_NOT_FOUND.code
            );
        }
    }
    // The compacted package still loads the referencing object.
    let loaded = package
        .load(asset_registry(), &LoadOptions::default())
        .unwrap();
    assert_eq!(asset_values(&loaded.snapshot), asset_values(&snapshot));
}

/// A reader of `len` pseudo-random bytes generated on the fly.
struct Generated {
    left: u64,
    state: u64,
}

impl Read for Generated {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = usize::try_from(self.left.min(out.len() as u64)).unwrap();
        for byte in &mut out[..n] {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            *byte = self.state.to_le_bytes()[3];
        }
        self.left -= n as u64;
        Ok(n)
    }
}

#[test]
#[ignore = "streams a 1 GiB asset; run in a release build"]
fn a_large_asset_streams_with_bounded_memory() {
    let dir = TempDir::new("asset large");
    let package = create(&dir, "large.arc");
    let len = 1024 * 1024 * 1024;
    let mut expected = blake3::Hasher::new();
    let mut source = Generated {
        left: len,
        state: 99,
    };
    let mut copy = Vec::new();
    let mut check = Generated {
        left: len,
        state: 99,
    };
    loop {
        copy.resize(CHUNK_MAX, 0);
        let n = check.read(&mut copy).unwrap();
        if n == 0 {
            break;
        }
        expected.update(&copy[..n]);
    }
    let hash = package.put_asset("model/big", &mut source).unwrap();
    assert_eq!(hash.0, *expected.finalize().as_bytes());
    let mut reader = package.asset(&hash).unwrap();
    let mut streamed = blake3::Hasher::new();
    let mut total = 0;
    while let Some(chunk) = reader.next_chunk().unwrap() {
        assert!(chunk.len() <= CHUNK_MAX);
        total += chunk.len() as u64;
        streamed.update(&chunk);
    }
    assert_eq!(total, len);
    assert_eq!(streamed.finalize().as_bytes(), &hash.0);
}
