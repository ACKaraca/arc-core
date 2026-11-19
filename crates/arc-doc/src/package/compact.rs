//! Compaction (CORE-DOC-049): a full rewrite of the package that keeps only what is still
//! referenced, with a report of what was removed.
//!
//! The rewrite is [`Strategy::Compact`]: assets are kept only when a typed object, a held row,
//! a journal record or a foreign data row refers to them (see [`super::Package::save`]), and the
//! journal is cut to the history retention of the package's metadata
//! ([`super::history`]). Nothing a save keeps is removed; the report measures the file before and
//! after, so `reclaimed_bytes` is the real difference of the file sizes.

use std::path::Path;

use arc_base::{Limits, Result};

use super::Package;
use super::read;
use super::recovery::RecoveryJournal;
use super::{Preview, SaveProgress, Saved};
use crate::document::Document;

/// The result of [`Package::compact_document`].
#[derive(Debug)]
pub struct Compaction {
    /// What the rewrite saved ([`Package::save`]).
    pub saved: Saved,
    /// The size of the package file before the rewrite, in bytes.
    pub file_bytes_before: u64,
    /// The size of the package file after the rewrite, in bytes.
    pub file_bytes_after: u64,
    /// The bytes the file shrank by; zero when it did not shrink.
    pub reclaimed_bytes: u64,
    /// The asset rows removed.
    pub assets_removed: u64,
    /// The chunk rows removed.
    pub chunks_removed: u64,
    /// The journal rows removed. Records committed since the last save are not counted: they
    /// are added to the journal by the rewrite, so this is the rows dropped net of them.
    pub journal_records_removed: u64,
}

/// The counts of a package file that a compaction changes.
#[derive(Debug, Clone, Copy)]
struct Footprint {
    file_bytes: u64,
    assets: u64,
    chunks: u64,
    journal: u64,
}

fn footprint(path: &Path, limits: &Limits) -> Result<Footprint> {
    let (conn, _, _) = read::open(path, limits)?;
    let name = path.display().to_string();
    let count = |sql: &str| -> Result<u64> {
        let value: i64 = conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(|e| read::sql_error(e, &name))?;
        Ok(u64::try_from(value).unwrap_or(0))
    };
    Ok(Footprint {
        file_bytes: std::fs::metadata(path)
            .map_err(|e| read::io_error(e, path))?
            .len(),
        assets: count("SELECT count(*) FROM arc_asset")?,
        chunks: count("SELECT count(*) FROM arc_chunk")?,
        journal: count("SELECT count(*) FROM arc_journal")?,
    })
}

impl Package {
    /// Compacts the package in place from the latest state of `document`: saves it by
    /// [`Strategy::Compact`] exactly as [`Package::save_document`] does, but always as a full
    /// rewrite, and reports the file sizes and the rows removed.
    ///
    /// The document keeps its state and its journal; reopening the package gives the same
    /// document. Fails with the errors of [`Package::save_document`], and with
    /// `doc.package.io_failed` or `doc.package.corrupt` when the file's footprint cannot be read
    /// before or after the rewrite.
    pub fn compact_document(
        &mut self,
        document: &Document,
        previews: &[Preview],
        recovery: Option<&RecoveryJournal>,
        progress: &mut dyn FnMut(SaveProgress) -> Result<()>,
    ) -> Result<Compaction> {
        let limits = self.limits;
        let before = footprint(&self.path, &limits)?;
        let saved = self.save_document_with(document, previews, recovery, progress, true)?;
        let after = footprint(&self.path, &limits)?;
        Ok(Compaction {
            saved,
            file_bytes_before: before.file_bytes,
            file_bytes_after: after.file_bytes,
            reclaimed_bytes: before.file_bytes.saturating_sub(after.file_bytes),
            assets_removed: before.assets.saturating_sub(after.assets),
            chunks_removed: before.chunks.saturating_sub(after.chunks),
            journal_records_removed: before.journal.saturating_sub(after.journal),
        })
    }
}
