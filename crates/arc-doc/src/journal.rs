//! The operation journal of a session (CORE-DOC-019).
//!
//! A [`Journal`] is the append-only list of the records of the transactions committed in one
//! session, in commit order. Appending costs constant time besides encoding the record; records
//! are found by [`TransactionId`] in constant time. Each entry keeps the decoded record, which
//! undo and redo read, and its canonical encoding, which the package persists in its journal
//! table (`docs/architecture/journal-record.md`). Retention across saves belongs to the package.

use std::collections::HashMap;
use std::mem::size_of;

use arc_base::{Error, Limits, Result, TransactionId};

use crate::error::codes;
use crate::op::Op;
use crate::record::TxnRecord;
use crate::snapshot::Snapshot;
use crate::txn::TxnMeta;

/// One committed transaction in a [`Journal`].
#[derive(Debug, Clone)]
pub struct JournalEntry {
    record: TxnRecord,
    bytes: Vec<u8>,
    revision: u64,
}

impl JournalEntry {
    /// The record.
    #[must_use]
    pub fn record(&self) -> &TxnRecord {
        &self.record
    }

    /// The canonical encoding of the record, as the package stores it.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The revision of the snapshot the transaction published.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The estimated memory the entry retains for undo: the length of its encoding plus the
    /// size of one in-memory operation per operation. Objects of creates and deletes are often
    /// shared with snapshots, so the estimate counts their encoded size, not their decoded size.
    #[must_use]
    pub fn memory_bytes(&self) -> u64 {
        let ops = self.record.ops.len().saturating_mul(size_of::<Op>());
        u64::try_from(self.bytes.len().saturating_add(ops)).unwrap_or(u64::MAX)
    }
}

/// The append-only journal of one session.
#[derive(Debug, Clone)]
pub struct Journal {
    entries: Vec<JournalEntry>,
    by_id: HashMap<TransactionId, usize>,
    limits: Limits,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new(Limits::DEFAULT)
    }
}

impl Journal {
    /// An empty journal whose records must encode within `limits`, so that they decode under
    /// the same limits.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            entries: Vec::new(),
            by_id: HashMap::new(),
            limits,
        }
    }

    /// Appends the record of a committed transaction: its metadata `meta`, its final
    /// operations `ops` (including those the deletion rules added), the snapshot `before` it
    /// was applied on (which names the types of the objects its operations change) and the
    /// snapshot `after` it published (whose registry encodes its values). This is the commit
    /// hook of the journal.
    ///
    /// Fails with `doc.journal.duplicate_txn` when the journal holds `meta.id`,
    /// `doc.journal.parent_mismatch` when the journal is not empty and `meta.parent` is not its
    /// last transaction, and with the codes of [`TxnRecord::from_commit`] and
    /// [`TxnRecord::encode`]. A failure appends nothing.
    pub fn append(
        &mut self,
        meta: TxnMeta,
        ops: Vec<Op>,
        before: &Snapshot,
        after: &Snapshot,
    ) -> Result<&JournalEntry> {
        if self.by_id.contains_key(&meta.id) {
            return Err(
                Error::new(codes::JOURNAL_DUPLICATE_TXN).with_arg("id", meta.id.to_string())
            );
        }
        if let Some(last) = self.entries.last()
            && meta.parent != Some(last.record.txn_id)
        {
            return Err(Error::new(codes::JOURNAL_PARENT_MISMATCH)
                .with_arg("id", meta.id.to_string())
                .with_detail("last", last.record.txn_id.to_string()));
        }
        let record = TxnRecord::from_commit(meta, ops, before)?;
        let bytes = record.encode(after.registry(), &self.limits)?;
        self.by_id.insert(record.txn_id, self.entries.len());
        self.entries.push(JournalEntry {
            record,
            bytes,
            revision: after.revision(),
        });
        self.entries
            .last()
            .ok_or_else(|| Error::new(codes::JOURNAL_RECORD_INVALID))
    }

    /// The entry of transaction `id`, in constant time.
    #[must_use]
    pub fn get(&self, id: TransactionId) -> Option<&JournalEntry> {
        self.entries.get(*self.by_id.get(&id)?)
    }

    /// The last entry.
    #[must_use]
    pub fn last(&self) -> Option<&JournalEntry> {
        self.entries.last()
    }

    /// Every entry in commit order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &JournalEntry> + '_ {
        self.entries.iter()
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the journal holds no entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The parent chain from transaction `id`: its entry, the entry of its parent, and so on
    /// while the parent is in this journal. The first record of a session names as parent the
    /// last transaction of the document it opened, which is not in the session's journal.
    pub fn ancestors(&self, id: TransactionId) -> impl Iterator<Item = &JournalEntry> + '_ {
        // Parents precede their children in a journal, so the walk ends after `len` steps.
        std::iter::successors(self.get(id), |entry| self.get(entry.record.parent?))
            .take(self.entries.len())
    }
}
