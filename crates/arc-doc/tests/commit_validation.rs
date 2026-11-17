//! Commit validation (CORE-DOC-015): the stages run in the order field, reference, type,
//! document; the first error of any stage publishes nothing; warnings become diagnostics; the
//! deletion rules become operations; read-only objects and documents are refused.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use arc_base::id::Identifier;
use arc_base::{Category, Error, ErrorCode, Limits, ObjectId, SessionId, TransactionId};
use arc_doc::{
    Bound, Cardinality, CommitHooks, Committed, DocumentValidator, FieldDecl, FieldPath, Finding,
    Flags, Header, JournalSink, OnCopy, OnDelete, Op, Origin, Properties, ReferenceSpec,
    RelationshipRule, Schema, SchemaVersion, Severity, Snapshot, SnapshotCell, Strength, Txn,
    TxnLimits, TxnMeta, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, commit,
};

const THIRTEEN: ErrorCode = ErrorCode {
    code: "test.item.thirteen",
    category: Category::InvalidInput,
    message_id: "test-item-thirteen",
};
const DOCUMENT: ErrorCode = ErrorCode {
    code: "test.document.rejected",
    category: Category::InvalidInput,
    message_id: "test-document-rejected",
};

thread_local! {
    static LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

fn log() -> Vec<&'static str> {
    LOG.with(|log| std::mem::take(&mut *log.borrow_mut()))
}

/// Errors on 13, warns on 7.
fn validator(values: &[Option<Value>]) -> Vec<Finding> {
    LOG.with(|log| log.borrow_mut().push("type"));
    match values.first() {
        Some(Some(Value::I64(13))) => vec![Finding {
            severity: Severity::Error,
            error: Error::new(THIRTEEN),
        }],
        Some(Some(Value::I64(7))) => vec![Finding {
            severity: Severity::Warning,
            error: Error::new(THIRTEEN),
        }],
        _ => Vec::new(),
    }
}

fn registry() -> Arc<TypeRegistry> {
    let link = ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new("test.item").unwrap()],
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    });
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("n", ValueType::I64).with_bound(Bound::Int { min: 0, max: 100 }),
            FieldDecl::new("link", link).optional(),
        ],
    );
    let mut decl = TypeDecl::new(TypeKey::new("test.item").unwrap(), schema);
    decl.validators.push(validator);
    let mut registry = TypeRegistry::new();
    registry.register(decl).unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn txn(cell: &SnapshotCell) -> Txn {
    let meta = TxnMeta::new(TransactionId::from_u128(1), SessionId::from_u128(1));
    Txn::new(&cell.load(), meta, TxnLimits::default())
}

fn create(txn: &mut Txn, n: u128, value: i64, link: Option<u128>) {
    let values = vec![
        Some(Value::I64(value)),
        link.map(|target| Value::Reference(id(target))),
    ];
    txn.create("test.item", id(n), Header::default(), values)
        .unwrap();
}

/// A document validator that logs its call and rejects when `reject` holds.
fn document_validator(reject: bool) -> DocumentValidator {
    Arc::new(move |_base, _candidate, _touched| {
        LOG.with(|log| log.borrow_mut().push("document"));
        let severity = if reject {
            Severity::Error
        } else {
            Severity::Warning
        };
        vec![Finding {
            severity,
            error: Error::new(DOCUMENT),
        }]
    })
}

/// Commits a transaction with up to one defect per stage; returns the error code (if any),
/// the stages that ran and whether the published snapshot stayed the same.
fn run(
    field: bool,
    reference: bool,
    type_error: bool,
    document: bool,
) -> (Option<&'static str>, Vec<&'static str>, bool) {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let before = cell.load();
    let mut t = txn(&cell);
    create(&mut t, 1, if field { 200 } else { 1 }, None);
    create(&mut t, 2, 2, Some(if reference { 99 } else { 1 }));
    create(&mut t, 3, if type_error { 13 } else { 3 }, None);
    let validators = [document_validator(document)];
    let limits = Limits::DEFAULT;
    let hooks = CommitHooks {
        validators: &validators,
        ..CommitHooks::new(&cell, &limits)
    };
    log();
    let code = commit(t, &hooks).err().map(|error| error.code());
    (code, log(), cell.load().ptr_eq(&before))
}

#[test]
fn stages_run_in_order_and_the_first_error_wins() {
    assert_eq!(
        run(true, true, true, true),
        (Some("doc.value.out_of_range"), vec![], true)
    );
    assert_eq!(
        run(false, true, true, true),
        (Some("doc.reference.dangling"), vec![], true)
    );
    // Type validators run per object in identifier order and stop at the first error.
    assert_eq!(
        run(false, false, true, true),
        (
            Some("test.item.thirteen"),
            vec!["type", "type", "type"],
            true
        )
    );
    assert_eq!(
        run(false, false, false, true),
        (
            Some("test.document.rejected"),
            vec!["type", "type", "type", "document"],
            true
        )
    );
    assert_eq!(
        run(false, false, false, false),
        (None, vec!["type", "type", "type", "document"], false)
    );
}

#[test]
fn field_errors_name_the_object_and_type() {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let mut t = txn(&cell);
    t.create("test.item", id(1), Header::default(), vec![None, None])
        .unwrap();
    let error = commit(t, &CommitHooks::new(&cell, &Limits::DEFAULT)).unwrap_err();
    assert_eq!(error.code(), "doc.value.missing");
    assert_eq!(
        error.details().get("object"),
        Some(&arc_base::error::Value::Text(id(1).to_string()))
    );
    assert_eq!(cell.load().revision(), 0);
}

#[test]
fn warnings_become_diagnostics_and_do_not_block() {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let mut t = txn(&cell);
    create(&mut t, 1, 7, None);
    let validators = [document_validator(false)];
    let sink = arc_doc::CollectingSink::new(10);
    let limits = Limits::DEFAULT;
    let hooks = CommitHooks {
        validators: &validators,
        diagnostics: Some(&sink),
        ..CommitHooks::new(&cell, &limits)
    };
    let outcome = commit(t, &hooks).unwrap();
    assert_eq!(outcome.snapshot.revision(), 1);
    assert!(outcome.snapshot.ptr_eq(&cell.load()));
    let codes: Vec<_> = outcome.diagnostics.iter().map(|d| d.code()).collect();
    assert_eq!(codes, ["test.item.thirteen", "test.document.rejected"]);
    assert_eq!(outcome.diagnostics[0].objects, vec![id(1)]);
    assert_eq!(outcome.diagnostics[0].origin, Origin::Validator);
    assert_eq!(sink.take().0.len(), 2);
}

#[derive(Default)]
struct Journal(Mutex<Vec<(usize, u64, u64)>>);

impl JournalSink for Journal {
    fn record(&self, committed: &Committed) {
        let clears = committed
            .ops
            .iter()
            .filter(|op| matches!(op, Op::Modify { .. }))
            .count();
        self.0.lock().unwrap().push((
            clears,
            committed.before.revision(),
            committed.after.revision(),
        ));
    }
}

#[test]
fn deletion_rules_become_operations_the_journal_receives() {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let limits = Limits::DEFAULT;
    let journal = Journal::default();
    let hooks = CommitHooks {
        journal: Some(&journal),
        ..CommitHooks::new(&cell, &limits)
    };
    let mut t = txn(&cell);
    create(&mut t, 1, 1, None);
    create(&mut t, 2, 2, Some(1));
    commit(t, &hooks).unwrap();
    let mut t = txn(&cell);
    t.delete(id(1)).unwrap();
    let outcome = commit(t, &hooks).unwrap();
    let snapshot = outcome.snapshot;
    let two = snapshot.get(snapshot.resolve(id(2)).unwrap()).unwrap();
    assert_eq!(two.values[1], None);
    assert_eq!(*journal.0.lock().unwrap(), vec![(0, 0, 1), (1, 1, 2)]);
}

#[test]
fn read_only_objects_keep_values_owner_and_existence() {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let hooks_limits = Limits::DEFAULT;
    let hooks = CommitHooks::new(&cell, &hooks_limits);
    let mut t = txn(&cell);
    let header = Header {
        flags: Flags {
            read_only: true,
            ..Flags::default()
        },
        ..Header::default()
    };
    t.create("test.item", id(1), header, vec![Some(Value::I64(1)), None])
        .unwrap();
    create(&mut t, 2, 2, None);
    commit(t, &hooks).unwrap();

    let refused = |change: &dyn Fn(&mut Txn)| {
        let mut t = txn(&cell);
        change(&mut t);
        let before = cell.load();
        let error = commit(t, &hooks).unwrap_err();
        assert!(cell.load().ptr_eq(&before));
        error.code()
    };
    let modify = |t: &mut Txn| {
        t.modify(id(1), FieldPath::new("n"), Some(Value::I64(5)))
            .unwrap()
    };
    assert_eq!(refused(&modify), "doc.object.read_only");
    assert_eq!(
        refused(&|t| t.delete(id(1)).unwrap()),
        "doc.object.read_only"
    );
    assert_eq!(
        refused(&|t| t.relink(id(1), Some(id(2))).unwrap()),
        "doc.object.read_only"
    );
    // Clearing the flag is allowed, and the object can be changed afterwards.
    let mut t = txn(&cell);
    t.set_properties(id(1), Properties::default()).unwrap();
    commit(t, &hooks).unwrap();
    let mut t = txn(&cell);
    modify(&mut t);
    assert_eq!(commit(t, &hooks).unwrap().snapshot.revision(), 3);
}

#[test]
fn a_transaction_without_operations_publishes_nothing() {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let before = cell.load();
    let outcome = commit(txn(&cell), &CommitHooks::new(&cell, &Limits::DEFAULT)).unwrap();
    assert!(outcome.txn_id.is_none());
    assert!(outcome.snapshot.ptr_eq(&before));
    assert!(cell.load().ptr_eq(&before));
}

#[test]
fn a_stale_base_publishes_nothing() {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let hooks_limits = Limits::DEFAULT;
    let hooks = CommitHooks::new(&cell, &hooks_limits);
    let mut first = txn(&cell);
    let mut second = txn(&cell);
    create(&mut first, 1, 1, None);
    create(&mut second, 2, 2, None);
    commit(first, &hooks).unwrap();
    let published = cell.load();
    assert_eq!(
        commit(second, &hooks).unwrap_err().code(),
        "doc.snapshot.stale"
    );
    assert!(cell.load().ptr_eq(&published));
}
