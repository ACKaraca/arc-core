//! The document package (feature `package`): one SQLite file with the documented schema
//! (CORE-DOC-040), written by the atomic full rewrite (CORE-DOC-044), opened for editing under a
//! sidecar edit lock (CORE-DOC-046) and carrying the previews the application supplies
//! (CORE-DOC-051).
//!
//! The format is specified in `docs/architecture/document-package.md`; this module's rules in
//! `docs/crates/arc-doc.md`, section "Package format, full rewrite, edit locks and previews".
//! Every value read from a file is treated as hostile: failures are errors with stable codes,
//! never panics. No `rusqlite` type appears in a public signature.

mod asset;
mod background;
mod incremental;
mod lock;
mod preview;
mod read;
pub mod recovery;
mod schema;
mod write;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arc_base::id::DocumentId;
use arc_base::{Error, Limits, Result};

use crate::error::codes;
use crate::load::{LoadOptions, Loaded};
use crate::registry::TypeRegistry;
use crate::schema::AssetHash;

pub use asset::{AssetReader, CHUNK_AVG, CHUNK_MAX, CHUNK_MIN, WHOLE_MAX, asset_values};
pub use background::{BackgroundSave, Finished, SaveRequest};
pub use incremental::{Saved, Strategy, Touch, touched_between};
pub use lock::{LockHolder, LockOwner, lock_path};
pub use preview::{MAX_PREVIEW_BYTES, PNG, PREVIEW, Preview, SHEET_PREFIX, THUMBNAIL};
pub use write::{Contents, ROWS_PER_TICK, Save, SaveProgress, SaveStep, save_as, write};

/// `PRAGMA application_id` of every package: ASCII `ARC1`.
pub const APPLICATION_ID: u32 = 0x4152_4331;

/// The package format this version writes.
pub const FORMAT: FormatVersion = FormatVersion { major: 1, minor: 0 };

/// A package format version `major.minor`. `PRAGMA user_version` holds the major version and
/// `arc_meta` key `format_version` the text form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormatVersion {
    /// Incremented by changes older readers cannot read.
    pub major: u32,
    /// Incremented by additions older readers open read-only.
    pub minor: u32,
}

impl fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl FormatVersion {
    /// Parses `major.minor` (decimal, no sign, no leading zeros beyond `0`).
    fn parse(text: &str) -> Option<Self> {
        let (major, minor) = text.split_once('.')?;
        let number = |part: &str| {
            let canonical = !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part == "0" || !part.starts_with('0'));
            canonical.then(|| part.parse().ok()).flatten()
        };
        Some(Self {
            major: number(major)?,
            minor: number(minor)?,
        })
    }
}

/// A value of `arc_meta` with a key this version does not interpret, in its SQLite storage
/// class; written back unchanged.
#[derive(Debug, Clone, PartialEq)]
pub enum MetaValue {
    /// `NULL`.
    Null,
    /// `INTEGER`.
    Integer(i64),
    /// `REAL`.
    Real(f64),
    /// `TEXT`.
    Text(String),
    /// `BLOB`.
    Blob(Vec<u8>),
}

/// The document metadata of `arc_meta`. The package manages the keys `format_version`,
/// `save_counter` and `thumbnail`; the other documented keys are the fields below; every other
/// key is kept in [`Metadata::other`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    /// `document_id`: the document's identifier.
    pub document_id: Option<DocumentId>,
    /// `created_by`: the creating application and its version.
    pub created_by: Option<String>,
    /// `saved_by`: the last saving application and its version.
    pub saved_by: Option<String>,
    /// `units`: the document's display units.
    pub units: Option<String>,
    /// `coordinate_reference`: the coordinate reference of the model.
    pub coordinate_reference: Option<String>,
    /// `save_counter`: the number of saves; a save writes the next number.
    pub save_counter: u64,
    /// `history_retention`: how much of the journal a save keeps.
    pub history_retention: Option<String>,
    /// `partitions`: the product-defined load units, in order (a JSON array of strings).
    pub partitions: Vec<String>,
    /// True when the last save wrote no thumbnail (`thumbnail` = `none`).
    pub thumbnail_missing: bool,
    /// Every other key (`ext_table:<name>` registrations, product keys), written back unchanged.
    /// An entry with one of the keys above is ignored on write.
    pub other: BTreeMap<String, MetaValue>,
}

/// How [`Package::open`] opens a package.
#[derive(Debug, Clone, Copy)]
pub enum Mode<'a> {
    /// Read-only: no lock, nothing written to the file or its directory.
    ReadOnly,
    /// For editing under the edit lock of this owner, or read-only when that is not possible.
    Edit(&'a LockOwner),
}

/// Why a package opened read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOnlyCause {
    /// The caller asked for [`Mode::ReadOnly`].
    Requested,
    /// A live edit lock exists: its holder, or `None` when the lock file is unreadable.
    Locked(Option<LockHolder>),
    /// A stale edit lock exists (its host is this one and no process holds it): it can be taken
    /// over with [`Package::take_over`] after the user confirms.
    StaleLock(LockHolder),
    /// The package has a newer format minor version.
    NewerFormat(FormatVersion),
}

/// What an open package allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Access {
    /// Open for editing under this process's edit lock.
    Edit,
    /// Open read-only.
    ReadOnly(ReadOnlyCause),
}

/// An open package: its path, format, metadata and access, and the edit lock when it is open
/// for editing. It keeps no file open besides the lock file; reading reopens the file.
/// Dropping it releases the lock.
#[derive(Debug)]
pub struct Package {
    path: PathBuf,
    format: FormatVersion,
    metadata: Metadata,
    limits: Limits,
    access: Access,
    lock: Option<lock::Held>,
}

impl Package {
    /// Creates an empty package at `path` with `metadata` by a full rewrite and opens it for
    /// editing. Fails with `doc.package.exists` when `path` exists.
    pub fn create(path: &Path, metadata: &Metadata, owner: &LockOwner) -> Result<Self> {
        if path.exists() {
            return Err(Error::new(codes::PACKAGE_EXISTS).with_arg("path", display(path)));
        }
        let contents = Contents {
            metadata,
            types: &[],
            objects: &[],
            previews: &[],
            snapshot: None,
        };
        write(path, &contents, &mut |_| Ok(()))?;
        Self::open(path, Mode::Edit(owner), &Limits::DEFAULT)
    }

    /// Opens the package at `path`.
    ///
    /// The file's header is read with plain file reads first: a file that is not SQLite or has
    /// another application id fails with `doc.package.not_a_package`, and a newer format major
    /// version fails with `doc.package.newer_format` before SQLite opens the file and before
    /// any lock is taken, so the file and its directory stay untouched. Then the package opens
    /// read-only in SQLite, its schema and format version are checked
    /// (`doc.package.schema_invalid`) and its metadata is read. A newer format minor version
    /// opens read-only ([`ReadOnlyCause::NewerFormat`]) and takes no lock. With [`Mode::Edit`]
    /// the edit lock is taken; a live or stale lock of another opener makes the package
    /// read-only and names the holder.
    pub fn open(path: &Path, mode: Mode<'_>, limits: &Limits) -> Result<Self> {
        if let Mode::Edit(_) = mode {
            incremental::recover(path, limits)?;
        }
        let (format, metadata) = read::open_checked(path, limits)?;
        let mut package = Self {
            path: path.to_path_buf(),
            format,
            metadata,
            limits: *limits,
            access: Access::ReadOnly(ReadOnlyCause::Requested),
            lock: None,
        };
        if format.minor > FORMAT.minor {
            package.access = Access::ReadOnly(ReadOnlyCause::NewerFormat(format));
            return Ok(package);
        }
        if let Mode::Edit(owner) = mode {
            match lock::acquire(path, owner)? {
                Ok(held) => {
                    package.lock = Some(held);
                    package.access = Access::Edit;
                }
                Err(cause) => package.access = Access::ReadOnly(cause),
            }
        }
        Ok(package)
    }

    /// Takes over a stale edit lock after the user confirmed it: only when the package opened
    /// read-only with [`ReadOnlyCause::StaleLock`] and the lock is still stale. The package is
    /// then open for editing. Fails with `doc.package.locked` when the lock is live, belongs to
    /// another host or was taken in the meantime, leaving the package read-only.
    pub fn take_over(&mut self, owner: &LockOwner) -> Result<()> {
        let Access::ReadOnly(ReadOnlyCause::StaleLock(holder)) = &self.access else {
            let error = Error::new(codes::PACKAGE_LOCKED)
                .with_arg("user", "")
                .with_arg("host", "")
                .with_detail("reason", "not stale");
            return Err(error);
        };
        let held = lock::take_over(&self.path, holder, owner)?;
        self.lock = Some(held);
        self.access = Access::Edit;
        Ok(())
    }

    /// The package's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The format version of the file.
    #[must_use]
    pub fn format(&self) -> FormatVersion {
        self.format
    }

    /// The metadata read at open.
    #[must_use]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// What the package allows.
    #[must_use]
    pub fn access(&self) -> &Access {
        &self.access
    }

    /// True when the package is open for editing.
    #[must_use]
    pub fn is_editable(&self) -> bool {
        self.access == Access::Edit
    }

    /// Reads the type and object rows and loads them with [`crate::load::load`].
    ///
    /// Each row's `payload_hash` is checked against the BLAKE3 hash of its payload when its
    /// encoding is 0 (other encodings cannot be hashed by this version); a row that does not
    /// match is quarantined ([`crate::load::load_checked`]) with `doc.package.hash_mismatch`
    /// (argument `id`) and written back unchanged.
    /// Values of an unexpected storage class or text encoding fail with
    /// `doc.package.value_invalid` (arguments `table`, `column`); values longer than
    /// `input_bytes` fail with `doc.load.limit_exceeded`; the load's own limits and failures
    /// apply as documented there.
    pub fn load(&self, registry: Arc<TypeRegistry>, options: &LoadOptions) -> Result<Loaded> {
        read::load(&self.path, registry, options)
    }

    /// Reads the stored previews, in kind order. Each is checked as at save; an invalid one
    /// fails with `doc.package.preview_invalid`.
    pub fn previews(&self) -> Result<Vec<Preview>> {
        read::previews(&self.path, &self.limits)
    }
}

impl Package {
    /// Saves `save`, the state descending from the last save of this package, by `strategy`,
    /// and returns what was written. `progress` is called before each [`SaveStep`] and every
    /// [`ROWS_PER_TICK`] rows; an error from it cancels the save, which leaves the file as it
    /// was. A save in place makes the metadata written the package's metadata: its
    /// `save_counter` is the baseline of the next incremental save.
    ///
    /// - [`Strategy::Incremental`] writes, in one SQLite transaction with the rollback journal
    ///   and `synchronous = FULL`, only the rows of the objects in the map and of objects still
    ///   stored at an older version of their registered type, their derived index rows, the type
    ///   rows of their types, the changed metadata entries and the changed previews. It is
    ///   refused with `doc.package.incremental_refused` (argument `reason`: `read_only_package`,
    ///   `format_upgrade`, `read_only_document`, `base_changed` when the file's `save_counter`
    ///   is not the one of `save.metadata`, `changes` when the map does not fit the file and the
    ///   snapshot), and the full rewrite in place runs instead; [`Saved::refused`] says why.
    /// - [`Strategy::Rewrite`] and [`Strategy::Compact`] write the package anew by the full
    ///   rewrite and replace it; `Compact` keeps only the referenced assets.
    /// - [`Strategy::Copy`] writes the package to another file by the full rewrite; this
    ///   package is unchanged.
    ///
    /// A save in place of a package not open for editing fails with `doc.document.read_only`.
    /// Other failures are those of [`save_as`] and of SQLite (`doc.package.io_failed`,
    /// `doc.package.corrupt`).
    pub fn save(
        &mut self,
        save: &Save<'_>,
        strategy: &Strategy,
        progress: &mut dyn FnMut(SaveProgress) -> Result<()>,
    ) -> Result<Saved> {
        let path = self.path.clone();
        if let Strategy::Copy(target) = strategy {
            let carry = asset::Carry {
                from: &path,
                only_referenced: None,
            };
            let metadata = write::save_full(target, save, Some(&carry), progress)?;
            return Ok(Saved {
                metadata,
                incremental: false,
                refused: None,
            });
        }
        if !self.is_editable() {
            return Err(Error::new(codes::DOCUMENT_READ_ONLY)
                .with_arg("type", "")
                .with_arg("version", "")
                .with_detail("reason", "package"));
        }
        let mut refused = None;
        if let Strategy::Incremental(changes) = strategy {
            match incremental::save(self, save, changes, progress) {
                Ok(metadata) => {
                    self.metadata = metadata.clone();
                    return Ok(Saved {
                        metadata,
                        incremental: true,
                        refused: None,
                    });
                }
                Err(error) if error.code() == codes::PACKAGE_INCREMENTAL_REFUSED.code => {
                    refused = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        let carry = asset::Carry {
            from: &path,
            only_referenced: matches!(strategy, Strategy::Compact).then_some(save),
        };
        let metadata = write::save_full(&path, save, Some(&carry), progress)?;
        self.metadata = metadata.clone();
        Ok(Saved {
            metadata,
            incremental: false,
            refused,
        })
    }

    /// Stores the bytes `reader` yields as an asset of `media_type` and returns its hash
    /// (BLAKE3 of the bytes). An asset already stored is stored once; up to [`WHOLE_MAX`] bytes
    /// are one chunk, larger content is split into content-defined chunks shared by every asset
    /// that contains them. The asset is written into the package in its own transaction: no
    /// object refers to it until a save writes one that does, so the document the file holds
    /// is unchanged, and an asset no save came to refer to is removed by [`Strategy::Compact`].
    /// Memory stays bounded by a few chunks. Fails with `doc.document.read_only` when the
    /// package is not open for editing, `doc.value.out_of_range` for an empty media type or one
    /// longer than `string_bytes`, `doc.load.limit_exceeded` beyond `input_bytes` bytes or
    /// `record_count` chunks, and `doc.package.io_failed` when reading fails; nothing is stored
    /// then.
    pub fn put_asset(&self, media_type: &str, reader: &mut dyn std::io::Read) -> Result<AssetHash> {
        if !self.is_editable() {
            return Err(Error::new(codes::DOCUMENT_READ_ONLY)
                .with_arg("type", "")
                .with_arg("version", "")
                .with_detail("reason", "package"));
        }
        asset::put(&self.path, media_type, reader, &self.limits)
    }

    /// A reader of the asset `hash` that verifies every chunk before returning its bytes and the
    /// whole at its end (see [`AssetReader`]). Fails with `doc.package.asset_not_found` when no
    /// such asset is stored and `doc.package.asset_corrupt` when its rows are malformed.
    pub fn asset(&self, hash: &AssetHash) -> Result<AssetReader> {
        asset::open(&self.path, hash, &self.limits)
    }

    /// The bytes of the asset `hash`, returned only after every chunk and the whole verified;
    /// a damaged asset fails with `doc.package.asset_corrupt` and none of its bytes are
    /// returned. An asset longer than `input_bytes` fails with `doc.load.limit_exceeded`.
    pub fn asset_bytes(&self, hash: &AssetHash) -> Result<Vec<u8>> {
        asset::read_all(&self.path, hash, &self.limits)
    }
}

/// The text form of a path for error arguments.
fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_versions_parse_only_in_canonical_form() {
        assert_eq!(
            FormatVersion::parse("1.0"),
            Some(FormatVersion { major: 1, minor: 0 })
        );
        assert_eq!(
            FormatVersion::parse("12.30"),
            Some(FormatVersion {
                major: 12,
                minor: 30
            })
        );
        for bad in [
            "1",
            "1.",
            ".0",
            "01.0",
            "1.00",
            "+1.0",
            "1.0.0",
            "a.b",
            "",
            "99999999999.0",
        ] {
            assert_eq!(FormatVersion::parse(bad), None, "{bad}");
        }
        assert_eq!(FORMAT.to_string(), "1.0");
    }
}
