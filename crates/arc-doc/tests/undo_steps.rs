//! Undo and redo by example (CORE-DOC-019 to CORE-DOC-023): inversion of records with the
//! operations the deletion rules added, undo then redo, groups, failing group owners,
//! coalescing, limits, misuse, and the undo time of a large step.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId, Result, SessionId, TransactionId};
use arc_doc::undo::differences;
use arc_doc::{
    Cardinality, FieldDecl, FieldPath, Header, Journal, OnCopy, OnDelete, ReferenceSpec,
    RelationshipRule, Schema, SchemaVersion, Snapshot, SnapshotCell, Strength, Txn, TxnKind,
    TxnLimits, TxnMeta, TypeDecl, TypeKey, TypeRegistry, UndoLimits, UndoManager, Value, ValueType,
    WorkingSet, check_references,
};

const WHOLE: &str = "test.whole";
const PART: &str = "test.part";

/// A whole owns its parts through a list; deleting a whole deletes its parts, and deleting a
/// part removes it from the list.
fn registry() -> Arc<TypeRegistry> {
    let parts = ValueType::List(Box::new(ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new(PART).unwrap()],
        rule: RelationshipRule {
            strength: Strength::Owning,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::CopyTarget,
            cardinality: Cardinality::Many { max: 8 },
        },
    })));
    let mut registry = TypeRegistry::new();
    for (key, fields) in [
        (
            WHOLE,
            vec![
                FieldDecl::new("n", ValueType::I64),
                FieldDecl::new("parts", parts),
            ],
        ),
        (PART, vec![FieldDecl::new("n", ValueType::I64)]),
    ] {
        let schema = Schema::new(SchemaVersion::new(1, 0), fields);
        registry
            .register(TypeDecl::new(TypeKey::new(key).unwrap(), schema))
            .unwrap();
    }
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

struct Doc {
    cell: SnapshotCell,
    journal: Journal,
    undo: UndoManager,
    next: u128,
}

impl Doc {
    fn new(limits: UndoLimits) -> Self {
        Self {
            cell: SnapshotCell::new(Snapshot::empty(registry())),
            journal: Journal::default(),
            undo: UndoManager::new(limits),
            next: 1,
        }
    }

    fn snapshot(&self) -> Snapshot {
        self.cell.load()
    }

    fn meta(&mut self) -> TxnMeta {
        let mut meta = TxnMeta::new(TransactionId::from_u128(self.next), SessionId::from_u128(1));
        self.next += 1;
        meta.parent = self.journal.last().map(|entry| entry.record().txn_id);
        meta.group = self.undo.group().map(str::to_owned);
        meta
    }

    fn commit(&mut self, mut txn: Txn, coalesce: Option<&str>) -> Result<TransactionId> {
        let before = txn.base().clone();
        txn.apply_delete_rules(&Limits::DEFAULT)?;
        let parts = txn.into_parts();
        check_references(&parts.set)?;
        self.cell.publish(&parts.set)?;
        let after = self.cell.load();
        let id = parts.meta.id;
        self.journal
            .append(parts.meta, parts.ops, &before, &after)?;
        self.undo.committed(&self.journal, id, coalesce)?;
        Ok(id)
    }

    fn edit(&mut self, coalesce: Option<&str>, body: impl FnOnce(&mut Txn)) -> TransactionId {
        let meta = self.meta();
        let mut txn = Txn::new(&self.snapshot(), meta, TxnLimits::default());
        body(&mut txn);
        self.commit(txn, coalesce).unwrap()
    }

    fn undo(&mut self) -> bool {
        let meta = self.meta();
        let txn = self
            .undo
            .undo(&self.journal, &self.snapshot(), meta, TxnLimits::default())
            .unwrap();
        txn.map(|txn| self.commit(txn, None).unwrap()).is_some()
    }

    fn redo(&mut self) -> bool {
        let meta = self.meta();
        let txn = self
            .undo
            .redo(&self.journal, &self.snapshot(), meta, TxnLimits::default())
            .unwrap();
        txn.map(|txn| self.commit(txn, None).unwrap()).is_some()
    }

    /// Prepares and commits a batch of `count` steps, declaring `declare` as its depth.
    fn batch(&mut self, undo: bool, count: usize, declare: Option<usize>) -> Result<TransactionId> {
        let meta = self.meta();
        let base = self.snapshot();
        let limits = TxnLimits::default();
        let txn = if undo {
            self.undo
                .undo_many(count, &self.journal, &base, meta, limits)
        } else {
            self.undo
                .redo_many(count, &self.journal, &base, meta, limits)
        }
        .unwrap();
        if let Some(depth) = declare {
            self.undo.expect_depth(depth);
        }
        self.commit(txn, None)
    }

    fn fail_group(&mut self) -> bool {
        let meta = self.meta();
        let txn = self
            .undo
            .fail_group(&self.journal, &self.snapshot(), meta, TxnLimits::default())
            .unwrap();
        txn.map(|txn| self.commit(txn, None).unwrap()).is_some()
    }

    fn steps(&self) -> (Vec<usize>, Vec<usize>) {
        (
            self.undo
                .undo_steps()
                .map(|s| s.transactions().len())
                .collect(),
            self.undo
                .redo_steps()
                .map(|s| s.transactions().len())
                .collect(),
        )
    }
}

fn whole(txn: &mut Txn, n: u128, parts: &[u128]) {
    for part in parts {
        txn.create(
            PART,
            id(*part),
            Header::default(),
            vec![Some(Value::I64(1))],
        )
        .unwrap();
    }
    let list = Value::List(parts.iter().map(|p| Value::Reference(id(*p))).collect());
    txn.create(
        WHOLE,
        id(n),
        Header::default(),
        vec![Some(Value::I64(0)), Some(list)],
    )
    .unwrap();
}

fn set(txn: &mut Txn, n: u128, value: i64) {
    txn.modify(id(n), FieldPath::new("n"), Some(Value::I64(value)))
        .unwrap();
}

#[test]
fn a_record_and_its_inverse_compare_empty_with_delete_rules() {
    let mut doc = Doc::new(UndoLimits::default());
    doc.edit(None, |txn| whole(txn, 1, &[10, 11, 12]));
    let original = doc.snapshot();
    // Deleting a part clears it from the list; deleting the whole cascades to its parts.
    let id = doc.edit(None, |txn| {
        txn.delete(id(11)).unwrap();
        set(txn, 1, 5);
        txn.delete(id(1)).unwrap();
    });
    let record = doc.journal.get(id).unwrap().record();
    assert!(record.ops.len() > 3, "the deletion rules add operations");
    let after = doc.snapshot();
    assert!(after.is_empty());
    let mut set = WorkingSet::new(&after);
    for op in record.ops.iter().rev() {
        op.inverse().apply(&mut set).unwrap();
    }
    assert!(differences(set.current(), &original).is_empty());
    let mut forward = WorkingSet::new(&original);
    for op in &record.ops {
        op.apply(&mut forward).unwrap();
    }
    assert!(differences(forward.current(), &after).is_empty());
    // Through undo and redo.
    assert!(doc.undo());
    assert!(differences(&doc.snapshot(), &original).is_empty());
    assert!(doc.redo());
    assert!(differences(&doc.snapshot(), &after).is_empty());
    assert_ne!(differences(&original, &after), Vec::<ObjectId>::new());
}

#[test]
fn undo_then_redo_compares_empty_after_unrelated_groups() {
    let mut doc = Doc::new(UndoLimits::default());
    doc.edit(None, |txn| whole(txn, 1, &[10]));
    doc.edit(None, |txn| whole(txn, 2, &[20]));
    doc.undo.begin_group("\"g\"");
    doc.edit(None, |txn| set(txn, 2, 7));
    doc.edit(None, |txn| txn.delete(id(20)).unwrap());
    doc.undo.end_group().unwrap();
    doc.edit(None, |txn| set(txn, 1, 3));
    let original = doc.snapshot();
    for _ in 0..3 {
        assert!(doc.undo());
    }
    assert_eq!(doc.snapshot().len(), 2);
    for _ in 0..3 {
        assert!(doc.redo());
    }
    assert!(differences(&doc.snapshot(), &original).is_empty());
    assert!(!doc.redo());
    let undos = doc
        .journal
        .iter()
        .filter(|e| matches!(e.record().kind, TxnKind::UndoOf(_)))
        .count();
    assert_eq!(undos, 3);
}

#[test]
fn a_group_and_nested_groups_undo_as_one_step() {
    let mut doc = Doc::new(UndoLimits::default());
    doc.edit(None, |txn| whole(txn, 1, &[]));
    let before = doc.snapshot();
    doc.undo.begin_group("\"outer\"");
    doc.edit(None, |txn| set(txn, 1, 1));
    doc.undo.begin_group("\"inner\"");
    assert_eq!(doc.undo.group(), Some("\"outer\""));
    doc.edit(None, |txn| set(txn, 1, 2));
    doc.edit(None, |txn| whole(txn, 2, &[]));
    doc.undo.end_group().unwrap();
    doc.edit(None, |txn| set(txn, 1, 3));
    assert_eq!(doc.steps(), (vec![1], vec![]));
    doc.undo.end_group().unwrap();
    assert_eq!(doc.steps(), (vec![1, 4], vec![]));
    assert_eq!(
        doc.journal.last().unwrap().record().group.as_deref(),
        Some("\"outer\"")
    );
    assert!(doc.undo());
    assert!(differences(&doc.snapshot(), &before).is_empty());
    assert_eq!(doc.steps(), (vec![1], vec![4]));
    // An empty group forms no step.
    doc.undo.begin_group("\"empty\"");
    doc.undo.end_group().unwrap();
    assert_eq!(doc.steps(), (vec![1], vec![4]));
}

#[test]
fn a_failed_group_owner_rolls_back_the_whole_group_without_residue() {
    let mut doc = Doc::new(UndoLimits::default());
    doc.edit(None, |txn| whole(txn, 1, &[10]));
    doc.undo();
    doc.redo();
    let before = doc.snapshot();
    let stacks = doc.steps();
    doc.undo.begin_group("\"outer\"");
    doc.edit(None, |txn| set(txn, 1, 1));
    doc.undo.begin_group("\"inner\"");
    doc.edit(None, |txn| txn.delete(id(1)).unwrap());
    // The inner owner fails: its members are rolled back, the outer group continues.
    assert!(doc.fail_group());
    assert_eq!(doc.snapshot().len(), 2);
    doc.edit(None, |txn| whole(txn, 2, &[]));
    // The outer owner fails: everything since the outer group began is rolled back.
    assert!(doc.fail_group());
    assert!(differences(&doc.snapshot(), &before).is_empty());
    assert_eq!(doc.steps(), (stacks.0, vec![]));
    assert_eq!(doc.undo.group(), None);
    // A failed group without members needs no rollback.
    doc.undo.begin_group("\"empty\"");
    assert!(!doc.fail_group());
    let rollbacks = doc
        .journal
        .iter()
        .filter(|e| matches!(e.record().kind, TxnKind::UndoOf(_)))
        .count();
    assert_eq!(rollbacks, 3);
}

#[test]
fn coalescing_merges_within_one_gesture_and_keeps_every_record() {
    let mut doc = Doc::new(UndoLimits::default());
    doc.edit(None, |txn| whole(txn, 1, &[]));
    let before = doc.snapshot();
    doc.undo.begin_gesture();
    for n in 0..5 {
        doc.edit(Some("drag"), |txn| set(txn, 1, n));
    }
    doc.edit(Some("other"), |txn| set(txn, 1, 9));
    doc.edit(Some("other"), |txn| set(txn, 1, 8));
    doc.undo.end_gesture();
    doc.edit(Some("other"), |txn| set(txn, 1, 7));
    doc.undo.begin_gesture();
    doc.edit(Some("other"), |txn| set(txn, 1, 6));
    // Without a key, nothing coalesces.
    doc.edit(None, |txn| set(txn, 1, 5));
    doc.edit(None, |txn| set(txn, 1, 4));
    assert_eq!(doc.steps(), (vec![1, 5, 2, 1, 1, 1, 1], vec![]));
    assert_eq!(doc.journal.len(), 12);
    // After an undo, a commit with the gesture's key starts a new step.
    assert!(doc.undo());
    doc.edit(Some("other"), |txn| set(txn, 1, 3));
    doc.edit(Some("other"), |txn| set(txn, 1, 2));
    assert_eq!(doc.steps(), (vec![1, 5, 2, 1, 1, 1, 2], vec![]));
    for _ in 0..6 {
        assert!(doc.undo());
    }
    assert!(differences(&doc.snapshot(), &before).is_empty());
}

#[test]
fn limits_drop_the_oldest_steps_and_keep_the_journal() {
    let mut doc = Doc::new(UndoLimits {
        steps: 3,
        ..UndoLimits::default()
    });
    doc.edit(None, |txn| whole(txn, 1, &[]));
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(doc.edit(None, |txn| set(txn, 1, n)));
    }
    let kept: Vec<TransactionId> = doc
        .undo
        .undo_steps()
        .flat_map(|s| s.transactions().to_vec())
        .collect();
    assert_eq!(kept, ids[2..]);
    assert_eq!(doc.journal.len(), 6);
    let one = doc.undo.undo_steps().next().unwrap().memory_bytes();
    assert_eq!(doc.undo.undo_memory_bytes(), 3 * one);
    // A budget of two steps' memory keeps two.
    doc.undo.set_limits(UndoLimits {
        steps: 1_000,
        memory_bytes: 2 * one + one / 2,
    });
    assert_eq!(doc.steps().0, vec![1, 1]);
    while doc.undo() {}
    let n = doc.snapshot().iter().next().unwrap().1.values[0].clone();
    assert_eq!(n, Some(Value::I64(2)));
    assert_eq!(doc.journal.len(), 8);
    // A step larger than the budget does not stay.
    doc.undo.set_limits(UndoLimits {
        steps: 1_000,
        memory_bytes: 0,
    });
    assert_eq!(doc.steps().0, Vec::<usize>::new());
}

#[test]
fn misuse_and_changed_states_fail_with_stable_codes() {
    let mut doc = Doc::new(UndoLimits::default());
    assert_eq!(
        doc.undo.end_group().unwrap_err().code(),
        "doc.undo.sequence_invalid"
    );
    let unknown = TransactionId::from_u128(999);
    assert_eq!(
        doc.undo
            .committed(&doc.journal, unknown, None)
            .unwrap_err()
            .code(),
        "doc.undo.sequence_invalid"
    );
    assert!(!doc.undo());
    let first = doc.edit(None, |txn| whole(txn, 1, &[]));
    // A member of a group must carry the group's identifier.
    doc.undo.begin_group("\"g\"");
    let mut meta = doc.meta();
    meta.group = None;
    let mut txn = Txn::new(&doc.snapshot(), meta, TxnLimits::default());
    set(&mut txn, 1, 1);
    assert_eq!(
        doc.commit(txn, None).unwrap_err().code(),
        "doc.undo.sequence_invalid"
    );
    let meta = doc.meta();
    let base = doc.snapshot();
    assert_eq!(
        doc.undo
            .undo(&doc.journal, &base, meta, TxnLimits::default())
            .unwrap_err()
            .code(),
        "doc.undo.sequence_invalid"
    );
    doc.undo.end_group().unwrap();
    // A merge changes the field behind the undo manager's back: the undo no longer fits.
    let mut meta = doc.meta();
    meta.kind = TxnKind::Merge;
    let mut txn = Txn::new(&doc.snapshot(), meta, TxnLimits::default());
    set(&mut txn, 1, 42);
    doc.commit(txn, None).unwrap();
    assert_eq!(doc.steps(), (vec![1], vec![]));
    let meta = doc.meta();
    let base = doc.snapshot();
    let error = doc
        .undo
        .undo(&doc.journal, &base, meta, TxnLimits::default())
        .unwrap_err();
    assert_eq!(error.code(), "doc.undo.state_mismatch");
    // The journal refuses a duplicate identifier and a parent that is not its last record.
    let mut meta = doc.meta();
    meta.id = first;
    let txn = Txn::new(&doc.snapshot(), meta, TxnLimits::default());
    assert_eq!(
        doc.commit(txn, None).unwrap_err().code(),
        "doc.journal.duplicate_txn"
    );
    let mut meta = doc.meta();
    meta.parent = Some(first);
    let before = doc.snapshot();
    assert_eq!(
        doc.journal
            .append(meta, Vec::new(), &before, &before)
            .unwrap_err()
            .code(),
        "doc.journal.parent_mismatch"
    );
}

/// Undo of one step of 1,000 single-field modifications against the 50 ms budget of
/// `docs/testing.md`. Run in a release build: `cargo test --release -p arc-doc --test
/// undo_steps -- --ignored --nocapture`.
#[test]
#[ignore = "release-mode measurement"]
#[allow(clippy::print_stderr, reason = "the measurement is reported")]
fn undo_of_a_thousand_modifications_stays_within_budget() {
    let mut doc = Doc::new(UndoLimits::default());
    doc.edit(None, |txn| {
        for n in 0..1_000 {
            whole(txn, n + 1, &[]);
        }
    });
    doc.edit(None, |txn| {
        for n in 0..1_000 {
            set(txn, n + 1, 1);
        }
    });
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..20 {
        let start = std::time::Instant::now();
        assert!(doc.undo());
        worst = worst.max(start.elapsed());
        assert!(doc.redo());
    }
    eprintln!("undo of 1,000 modifications, worst of 20: {worst:?}");
    assert!(worst < std::time::Duration::from_millis(50));
}

/// Three edits, the middle one two transactions in a group, the last two coalesced in a gesture.
fn mixed() -> Doc {
    let mut doc = Doc::new(UndoLimits::default());
    doc.edit(None, |txn| whole(txn, 1, &[10]));
    doc.undo.begin_group("\"g\"");
    doc.edit(None, |txn| set(txn, 1, 1));
    doc.edit(None, |txn| whole(txn, 2, &[20]));
    doc.undo.end_group().unwrap();
    doc.undo.begin_gesture();
    doc.edit(Some("k"), |txn| set(txn, 1, 2));
    doc.edit(Some("k"), |txn| set(txn, 1, 3));
    doc.undo.end_gesture();
    doc
}

#[test]
fn a_batch_over_groups_and_coalesced_steps_equals_single_undos_and_redo_returns() {
    let (mut single, mut batch) = (mixed(), mixed());
    let start = batch.snapshot();
    assert_eq!(batch.steps().0, vec![1, 2, 2]);
    for _ in 0..3 {
        assert!(single.undo());
    }
    batch.batch(true, 3, Some(3)).unwrap();
    assert!(differences(&single.snapshot(), &batch.snapshot()).is_empty());
    assert!(single.snapshot().is_empty() && batch.steps() == (vec![], vec![2, 2, 1]));
    for _ in 0..3 {
        assert!(single.redo());
    }
    batch.batch(false, 3, Some(3)).unwrap();
    assert!(differences(&single.snapshot(), &start).is_empty());
    assert!(differences(&batch.snapshot(), &start).is_empty());
    assert_eq!(batch.steps(), (vec![1], vec![]));
    // The batch redo is one step; its undo returns to the empty document.
    assert!(batch.undo());
    assert!(batch.snapshot().is_empty());
}

#[test]
fn a_report_that_does_not_match_the_prepared_batch_is_refused_and_changes_nothing() {
    for declare in [None, Some(1), Some(3)] {
        let mut doc = mixed();
        let steps = doc.steps();
        let error = doc.batch(true, 2, declare).unwrap_err();
        assert_eq!(error.code(), "doc.undo.sequence_invalid", "{declare:?}");
        assert_eq!(doc.steps(), steps);
    }
    // A redo report of the wrong depth, and an undo report of a deeper step than the top.
    let mut doc = mixed();
    doc.batch(true, 3, Some(3)).unwrap();
    let steps = doc.steps();
    assert!(doc.batch(false, 2, Some(1)).is_err());
    assert_eq!(doc.steps(), steps);
    // The declaration is consumed by one report.
    let mut doc = mixed();
    doc.batch(true, 2, Some(2)).unwrap();
    assert!(doc.undo());
}

#[test]
fn a_redo_batch_larger_than_the_budget_costs_older_steps_not_itself() {
    let mut doc = Doc::new(UndoLimits::default());
    for n in 1..=4 {
        doc.edit(None, |txn| whole(txn, n, &[100 + n]));
    }
    let total = doc.undo.undo_memory_bytes();
    doc.batch(true, 3, Some(3)).unwrap();
    doc.undo.set_limits(UndoLimits {
        steps: 1_000,
        memory_bytes: total / 2,
    });
    assert_eq!(doc.steps().0, vec![1]);
    doc.batch(false, 3, Some(3)).unwrap();
    // The older step went; the batch step stays and can be undone.
    assert_eq!(doc.steps().0, vec![1]);
    assert!(doc.undo());
    // Only the first edit (a whole and its part) remains.
    assert_eq!(doc.snapshot().iter().count(), 2);
}
