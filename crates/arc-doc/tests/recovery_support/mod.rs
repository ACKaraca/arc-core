//! Shared helpers of the `recovery_*` tests: a one-type registry, base packages, documents with
//! a recovery journal and their states.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers; each test file uses a part of them"
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use arc_base::id::{DocumentId, Identifier};
use arc_base::{Limits, ObjectId, SessionId};
use arc_doc::graph::Evaluators;
use arc_doc::package::recovery::{BaseFile, RecoveryJournal, Session};
use arc_doc::package::{Metadata, Mode, Package, Save, save_as};
use arc_doc::{
    Document, FieldDecl, FieldPath, Header, JournalSink, LoadOptions, Object, Schema,
    SchemaVersion, Snapshot, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, WorkingSet,
};

pub const KEY: &str = "k.n";
pub const DOCUMENT: u128 = 0x0190_0000_0000_7000_8000_0000_0000_0001;

pub fn registry() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("v", ValueType::I64)],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new(KEY).unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

/// The values of the objects of `snapshot` by identifier.
pub type State = BTreeMap<u128, i64>;

pub fn state(snapshot: &Snapshot) -> State {
    snapshot
        .iter()
        .map(|(_, object)| {
            let v = match object.values.first() {
                Some(Some(Value::I64(v))) => *v,
                _ => i64::MIN,
            };
            (object.id.as_u128(), v)
        })
        .collect()
}

/// Writes a package at `path` holding `state`, with save counter `counter - 1` before the save.
pub fn write_base(path: &Path, state: &State, counter: u64) {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    for (id, v) in state {
        let object = Object::new(
            ObjectId::from_u128(*id),
            SchemaVersion::new(1, 0),
            Header::default(),
            vec![Some(Value::I64(*v))],
        );
        set.create(KEY, object).unwrap();
    }
    let snapshot = set.current().clone();
    let touched = state
        .keys()
        .map(|id| (ObjectId::from_u128(*id), vec![0_u8; 16]))
        .collect();
    let metadata = Metadata {
        document_id: Some(DocumentId::from_u128(DOCUMENT)),
        save_counter: counter.saturating_sub(1),
        ..Metadata::default()
    };
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
}

/// Opens the package at `path` as a document and its base fingerprint.
pub fn open(path: &Path) -> (Document, BaseFile) {
    let package = Package::open(path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let base = BaseFile::of(path, package.metadata().save_counter).unwrap();
    let loaded = package.load(registry(), &LoadOptions::default()).unwrap();
    let document = Document::from_loaded(loaded, Arc::new(Evaluators::new())).unwrap();
    (document, base)
}

/// Starts the recovery journal of session `session` under `root` and attaches it.
pub fn protect(
    document: &Document,
    root: &Path,
    session: u128,
    base: Option<BaseFile>,
) -> Arc<RecoveryJournal> {
    let session = Session::new(
        root,
        DocumentId::from_u128(DOCUMENT),
        SessionId::from_u128(session),
        base,
    );
    let journal = RecoveryJournal::start(session).unwrap();
    document.set_journal(Arc::clone(&journal) as Arc<dyn JournalSink>);
    journal
}

/// Sets `v` of object `id`, creating it when it does not exist.
pub fn set(document: &Document, id: u128, v: i64) {
    document
        .submit(move |txn| {
            let object = ObjectId::from_u128(id);
            if txn.resolve(object).is_ok() {
                txn.modify(object, FieldPath::new("v"), Some(Value::I64(v)))
            } else {
                txn.create(KEY, object, Header::default(), vec![Some(Value::I64(v))])
                    .map(drop)
            }
        })
        .wait()
        .unwrap();
}
