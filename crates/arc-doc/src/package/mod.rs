//! The document package (feature `package`): one SQLite file with the documented schema
//! (CORE-DOC-040), written by the atomic full rewrite (CORE-DOC-044), opened for editing under a
//! sidecar edit lock (CORE-DOC-046) and carrying the previews the application supplies
//! (CORE-DOC-051).
//!
//! The format is specified in `docs/architecture/document-package.md`; this module's rules in
//! `docs/crates/arc-doc.md`, section "Package format, full rewrite, edit locks and previews".
//! Every value read from a file is treated as hostile: failures are errors with stable codes,
//! never panics. No `rusqlite` type appears in a public signature.

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

pub use lock::{LockHolder, LockOwner, lock_path};
pub use preview::{MAX_PREVIEW_BYTES, PNG, PREVIEW, Preview, SHEET_PREFIX, THUMBNAIL};
pub use write::{Contents, Save, SaveStep, save_as, write};

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
