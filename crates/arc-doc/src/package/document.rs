//! Opening a document from its package and saving it back: the changes since the last save come
//! from the document's journal, the save runs incrementally when they are known and by the full
//! rewrite in place otherwise, and the recovery journal is told about the saved state.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{ObjectId, Result, TransactionId};

use super::history::{Retention, retained};
use super::incremental::typed;
use super::recovery::{BaseFile, RecoveryJournal};
use super::{Package, Preview, Save, SaveProgress, Saved, Strategy, Touch};
use crate::document::{Document, DocumentConfig};
use crate::executor::lock;
use crate::graph::Evaluators;
use crate::journal::Journal;
use crate::load::{LoadOptions, StoredRecord};
use crate::op::Op;
use crate::registry::TypeRegistry;
use crate::snapshot::Snapshot;

/// The package state a document's journal is relative to: the file, its `save_counter` and the
/// snapshot revision the file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Baseline {
    path: PathBuf,
    save_counter: u64,
    revision: u64,
}

/// What a save of a document needs from it, read between two commits.
struct State {
    snapshot: Snapshot,
    /// Every object changed since load, with the identifier bytes of its last transaction.
    touched: BTreeMap<ObjectId, Vec<u8>>,
    /// The changes since `baseline`, when the journal holds every transaction since.
    changes: Option<BTreeMap<ObjectId, Touch>>,
    /// The last transaction of the saved state.
    last: Option<TransactionId>,
    /// The journal's records of revisions after the last save, not stored yet.
    fresh: Vec<StoredRecord>,
}

impl Package {
    /// Loads the package ([`Package::load`]) into a [`Document`] with `config` that remembers
    /// it was loaded from this package at its `save_counter`, so that
    /// [`Package::save_document`] can save it incrementally.
    pub fn open_document(
        &self,
        registry: Arc<TypeRegistry>,
        evaluators: Arc<Evaluators>,
        options: &LoadOptions,
        config: DocumentConfig,
    ) -> Result<Document> {
        let loaded = self.load(registry, options)?;
        let document = Document::from_loaded_with(loaded, evaluators, config)?;
        *lock(&document.saved) = Some(Baseline {
            path: self.path.clone(),
            save_counter: self.metadata.save_counter,
            revision: document.snapshot().revision(),
        });
        Ok(document)
    }

    /// Saves the latest snapshot of `document` into this package in place and returns what
    /// [`Package::save`] returned. The journal records the package holds
    /// ([`Document::stored_journal`]) and the journal's records since the last save are kept
    /// as the retention allows, and the document's diagnostics are recorded.
    ///
    /// The state saved and the journal are read together between two commits; the save then
    /// runs on the calling thread while editing continues. `Save::touched` maps every object
    /// a transaction of the journal changed to the 16 identifier bytes of the last such
    /// transaction (UUID byte order), and the metadata saved is the package's.
    ///
    /// The save is incremental ([`Strategy::Incremental`]) when the document was opened from or
    /// last saved to this package ([`Package::open_document`], this function), the package's
    /// `save_counter` is still the one of that time, and the journal holds every transaction
    /// since (one per revision): the map holds each object a transaction since changed,
    /// [`Touch::Created`] when the first operation on it created it, else [`Touch::Modified`]
    /// or [`Touch::Deleted`] by whether it is live now; an object created and deleted again has
    /// no entry. Otherwise it is the full rewrite in place ([`Strategy::Rewrite`]), and so is
    /// an incremental save that [`Package::save`] refuses.
    ///
    /// After a successful save the document remembers this package and the saved state, and
    /// `recovery`, when given, is told with [`RecoveryJournal::saved`] (the new file
    /// fingerprint and the last transaction saved), which removes the records up to it. Fails
    /// with the errors of [`Package::save`], with nothing remembered; `doc.executor.stopped`
    /// when the document is closed; and `doc.recovery.unavailable` when the recovery journal
    /// cannot be told, after the file was saved.
    pub fn save_document(
        &mut self,
        document: &Document,
        previews: &[Preview],
        recovery: Option<&RecoveryJournal>,
        progress: &mut dyn FnMut(SaveProgress) -> Result<()>,
    ) -> Result<Saved> {
        self.save_document_with(document, previews, recovery, progress, false)
    }

    /// [`Package::save_document`] with [`Strategy::Compact`] when `compact` is set, which
    /// rewrites the whole file and keeps only the referenced assets (see [`Package::compact_document`]).
    pub(super) fn save_document_with(
        &mut self,
        document: &Document,
        previews: &[Preview],
        recovery: Option<&RecoveryJournal>,
        progress: &mut dyn FnMut(SaveProgress) -> Result<()>,
        compact: bool,
    ) -> Result<Saved> {
        let baseline = lock(&document.saved)
            .clone()
            .filter(|b| b.path == self.path && b.save_counter == self.metadata.save_counter);
        let (stored, stored_revision) = lock(&document.stored).clone();
        let state = document.read_state(move |snapshot, journal| {
            state(
                snapshot,
                journal,
                baseline.as_ref().map(|b| b.revision),
                stored_revision,
            )
        })?;
        let mut records = stored;
        records.extend(state.fresh.iter().cloned());
        let diagnostics = document.diagnostics_snapshot().diagnostics;
        let strategy = match (compact, state.changes) {
            (true, _) => Strategy::Compact,
            (false, changes) => changes.map_or(Strategy::Rewrite, Strategy::Incremental),
        };
        let metadata = self.metadata.clone();
        let save = Save {
            snapshot: &state.snapshot,
            loaded: Some(document.loaded()),
            touched: &state.touched,
            metadata: &metadata,
            previews,
            journal: &records,
            diagnostics: &diagnostics,
            limits: self.limits,
        };
        let saved = self.save(&save, &strategy, progress)?;
        let retention = Retention::parse(metadata.history_retention.as_deref());
        *lock(&document.stored) = (
            retained(
                &records,
                retention,
                saved.metadata.save_counter,
                &self.limits,
            ),
            state.snapshot.revision(),
        );
        *lock(&document.saved) = Some(Baseline {
            path: self.path.clone(),
            save_counter: saved.metadata.save_counter,
            revision: state.snapshot.revision(),
        });
        if let Some(recovery) = recovery {
            let base = BaseFile::of(&self.path, saved.metadata.save_counter)?;
            recovery.saved(Some(base), state.last)?;
        }
        Ok(saved)
    }
}

/// Reads the save state of `snapshot` from `journal`; `since` is the revision of the last save.
fn state(snapshot: &Snapshot, journal: &Journal, since: Option<u64>, stored: u64) -> State {
    let fresh = journal
        .iter()
        .filter(|entry| entry.revision() > stored)
        .map(|entry| StoredRecord {
            seq: None,
            txn_id: entry.record().txn_id.as_u128().to_be_bytes().to_vec(),
            record: entry.bytes().to_vec(),
        })
        .collect();
    let mut touched = BTreeMap::new();
    for entry in journal.iter() {
        let bytes = entry.record().txn_id.as_u128().to_be_bytes().to_vec();
        for id in entry.record().ops.iter().filter_map(Op::object_id) {
            touched.insert(id, bytes.clone());
        }
    }
    let last = journal
        .last()
        .filter(|entry| entry.revision() == snapshot.revision())
        .map(|entry| entry.record().txn_id);
    let changes = since.and_then(|since| {
        let window: Vec<_> = journal.iter().filter(|e| e.revision() > since).collect();
        // Every revision since the save has its transaction in the journal, or a cut journal
        // lost some and the changes are unknown.
        let complete = window.iter().enumerate().all(|(at, entry)| {
            u64::try_from(at)
                .ok()
                .and_then(|at| since.checked_add(at)?.checked_add(1))
                == Some(entry.revision())
        }) && u64::try_from(window.len()).ok()
            == snapshot.revision().checked_sub(since);
        if !complete {
            return None;
        }
        let mut existed: BTreeMap<ObjectId, bool> = BTreeMap::new();
        for op in window.iter().flat_map(|entry| &entry.record().ops) {
            if let Some(id) = op.object_id() {
                existed
                    .entry(id)
                    .or_insert(!matches!(op, Op::Create { .. }));
            }
        }
        Some(
            existed
                .into_iter()
                .filter_map(
                    |(id, existed)| match (existed, typed(snapshot, id).is_some()) {
                        (true, true) => Some((id, Touch::Modified)),
                        (true, false) => Some((id, Touch::Deleted)),
                        (false, true) => Some((id, Touch::Created)),
                        (false, false) => None,
                    },
                )
                .collect(),
        )
    });
    State {
        snapshot: snapshot.clone(),
        touched,
        changes,
        last,
        fresh,
    }
}
