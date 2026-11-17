//! Journal records (CORE-DOC-019): the golden encoding of format 1, records built from commits
//! with the types of their operations, and the journal's append, lookup and parent chain.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId, SessionId, TransactionId};
use arc_doc::{
    FieldDecl, FieldPath, Flags, Header, Journal, Properties, Schema, SchemaVersion, Snapshot,
    SnapshotCell, Txn, TxnKind, TxnLimits, TxnMeta, TxnRecord, TypeDecl, TypeKey, TypeRegistry,
    Value, ValueType,
};

const KEY: &str = "test.golden";

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("n", ValueType::I64),
            FieldDecl::new("tags", ValueType::Map(Box::new(ValueType::String))).optional(),
        ],
    );
    registry
        .register(TypeDecl::new(TypeKey::new(KEY).unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn meta(n: u128, parent: Option<u128>) -> TxnMeta {
    let mut meta = TxnMeta::new(TransactionId::from_u128(n), SessionId::from_u128(0xab));
    meta.parent = parent.map(TransactionId::from_u128);
    meta
}

/// Commits one transaction that creates an object, then one that changes it in every way, and
/// returns the journal and the published snapshots.
fn journal() -> (Journal, Vec<Snapshot>) {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let mut journal = Journal::default();
    let mut snapshots = vec![cell.load()];
    let mut first = meta(1, None);
    first.author = "a".to_owned();
    first.time_unix_millis = 1_700_000_000_000;
    first.command = "{\"c\":1}".to_owned();
    first.label = "l".to_owned();
    let mut txn = Txn::new(&cell.load(), first, TxnLimits::default());
    let tags = BTreeMap::from([("k".to_owned(), Value::String("v".to_owned()))]);
    txn.create(
        KEY,
        ObjectId::from_u128(2),
        Header {
            name: Some("x".to_owned()),
            ..Header::default()
        },
        vec![Some(Value::I64(-1)), Some(Value::Map(tags))],
    )
    .unwrap();
    commit(&cell, &mut journal, txn, &mut snapshots);
    let mut second = meta(3, Some(1));
    second.kind = TxnKind::UndoOf(TransactionId::from_u128(1));
    second.group = Some("\"g\"".to_owned());
    second.selection_after = Some("[]".to_owned());
    let mut txn = Txn::new(&cell.load(), second, TxnLimits::default());
    let id = ObjectId::from_u128(2);
    txn.modify(id, FieldPath::new("tags").key("k"), None)
        .unwrap();
    txn.modify(id, FieldPath::new("n"), Some(Value::I64(64)))
        .unwrap();
    txn.set_properties(
        id,
        Properties {
            name: None,
            partition: Some("p".to_owned()),
            flags: Flags {
                read_only: true,
                ..Flags::default()
            },
        },
    )
    .unwrap();
    txn.relink(id, Some(ObjectId::from_u128(4))).unwrap();
    txn.relink(id, None).unwrap();
    txn.delete(id).unwrap();
    commit(&cell, &mut journal, txn, &mut snapshots);
    (journal, snapshots)
}

fn commit(cell: &SnapshotCell, journal: &mut Journal, txn: Txn, snapshots: &mut Vec<Snapshot>) {
    let before = txn.base().clone();
    let parts = txn.into_parts();
    cell.publish(&parts.set).unwrap();
    snapshots.push(cell.load());
    journal
        .append(parts.meta, parts.ops, &before, &cell.load())
        .unwrap();
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Format 1 is frozen: these bytes must never change.
const GOLDEN: [&str; 2] = [
    concat!(
        "010000000000000000000000000000000100000000000000000000000000000000ab016180d095ff",
        "bc3100077b2263223a317d016c0000010b746573742e676f6c64656e010000000000000000000000",
        "00000000000002010000010178000007010101016b017600"
    ),
    concat!(
        "01000000000000000000000000000000030100000000000000000000000000000001000000000000",
        "000000000000000000ab000001000000000000000000000000000000010000010322672201025b5d",
        "010b746573742e676f6c64656e060100000000000000000000000000000000020100020004746167",
        "7302016b010201760001000000000000000000000000000000000201000100016e01010101028001",
        "02000000000000000000000000000000000201017800000001017004040000000000000000000000",
        "00000000000200010000000000000000000000000000000404000000000000000000000000000000",
        "00020100000000000000000000000000000004000300000000000000000000000000000000020100",
        "0000010170040480010100"
    ),
];

#[test]
fn records_encode_to_the_golden_bytes_and_decode_back() {
    let (journal, _) = journal();
    let registry = registry();
    for (entry, golden) in journal.iter().zip(GOLDEN) {
        assert_eq!(hex(entry.bytes()), golden);
        let decoded = TxnRecord::decode(entry.bytes(), &registry, &Limits::STRICT).unwrap();
        assert_eq!(&decoded, entry.record());
        assert_eq!(decoded.meta().id, entry.record().txn_id);
    }
}

#[test]
fn records_name_the_types_of_their_operations() {
    let (journal, snapshots) = journal();
    let second = journal.get(TransactionId::from_u128(3)).unwrap().record();
    assert_eq!(second.ops.len(), 6);
    assert!(second.type_keys().iter().all(|key| key.as_str() == KEY));
    // An operation on an object the base does not hold has no type.
    let mut txn = Txn::new(&snapshots[1], meta(9, None), TxnLimits::default());
    txn.relink(ObjectId::from_u128(2), None).unwrap();
    let ops = txn.into_parts().ops;
    let error = TxnRecord::from_commit(meta(9, None), ops, &snapshots[0]).unwrap_err();
    assert_eq!(error.code(), "doc.journal.record_invalid");
}

#[test]
fn the_journal_finds_records_and_follows_the_parent_chain() {
    let (journal, snapshots) = journal();
    assert_eq!(journal.len(), 2);
    assert!(!journal.is_empty());
    let last = journal.last().unwrap();
    assert_eq!(last.revision(), snapshots[2].revision());
    assert!(last.memory_bytes() > u64::try_from(last.bytes().len()).unwrap());
    let chain: Vec<u128> = journal
        .ancestors(TransactionId::from_u128(3))
        .map(|entry| entry.record().txn_id.as_u128())
        .collect();
    assert_eq!(chain, [3, 1]);
    assert!(journal.get(TransactionId::from_u128(2)).is_none());
}

#[test]
fn records_of_unregistered_versions_do_not_encode_or_decode() {
    let (journal, _) = journal();
    let entry = journal.iter().next().unwrap();
    let mut other = TypeRegistry::new();
    let schema = Schema::new(
        SchemaVersion::new(1, 1),
        vec![
            FieldDecl::new("n", ValueType::I64),
            FieldDecl::new("tags", ValueType::Map(Box::new(ValueType::String))).optional(),
            FieldDecl::new("m", ValueType::I64)
                .optional()
                .since_minor(1),
        ],
    );
    other
        .register(TypeDecl::new(TypeKey::new(KEY).unwrap(), schema))
        .unwrap();
    let error = TxnRecord::decode(entry.bytes(), &other, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), "doc.journal.record_invalid");
    let error = entry.record().encode(&other, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), "doc.journal.record_invalid");
    let error =
        TxnRecord::decode(entry.bytes(), &TypeRegistry::new(), &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), "doc.journal.record_invalid");
}
