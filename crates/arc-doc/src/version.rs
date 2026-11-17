//! The version policy of loading (CORE-DOC-038): what the schema versions a document was written
//! with mean for opening it with the registry at hand.
//!
//! For each stored type row (type key and version) of a registered type:
//!
//! | Stored version | Effect |
//! |---|---|
//! | The current one, or an older minor of the current major | Objects decode; appended fields take their default or stay absent |
//! | An older major | Objects migrate in memory ([`crate::migrate`]) |
//! | A newer minor of the current major | Objects decode the known fields and keep the trailing bytes; the document is read-only ([`ReadOnlyReason::NewerMinor`]) |
//! | A newer major | The load is refused with `doc.load.newer_major`, or, with [`NewerMajor::ReadOnlyView`], the objects of that version are preserved and the document is read-only ([`ReadOnlyReason::NewerMajor`]) |
//!
//! Rows of unregistered types are preserved whatever their version. Type rows whose version
//! numbers are out of range (major below 1, either part beyond `u32`) describe nothing loadable
//! and are ignored, apart from refusing a newer major; objects that name such versions are
//! quarantined.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Error, Result};

use crate::error::codes;
use crate::load::StoredType;
use crate::registry::TypeRegistry;
use crate::schema::{SchemaVersion, TypeKey};

/// How to open a document that contains a type of a newer major version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum NewerMajor {
    /// Refuse to open it with `doc.load.newer_major`; nothing is created.
    #[default]
    Refuse,
    /// Open a read-only view that shows the objects of known versions; the objects of the newer
    /// major version are preserved. The user chooses this explicitly.
    ReadOnlyView,
}

/// Why a loaded document is read-only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReadOnlyReason {
    /// The document contains a newer minor version of a type: its unknown trailing bytes are kept
    /// but cannot be edited consistently.
    NewerMinor {
        /// The type.
        type_key: TypeKey,
        /// The version that wrote it.
        written: SchemaVersion,
        /// The version of the registry.
        current: SchemaVersion,
    },
    /// The user opened the read-only view of a document with a newer major version of a type.
    NewerMajor {
        /// The type.
        type_key: TypeKey,
        /// The version that wrote it.
        written: SchemaVersion,
        /// The version of the registry.
        current: SchemaVersion,
    },
}

impl ReadOnlyReason {
    /// The type the reason names.
    #[must_use]
    pub fn type_key(&self) -> &TypeKey {
        match self {
            Self::NewerMinor { type_key, .. } | Self::NewerMajor { type_key, .. } => type_key,
        }
    }

    /// The version that wrote the type.
    #[must_use]
    pub fn written(&self) -> SchemaVersion {
        match self {
            Self::NewerMinor { written, .. } | Self::NewerMajor { written, .. } => *written,
        }
    }

    /// The `doc.document.read_only` error naming the type (argument `type`) and the writing
    /// version (argument `version`), with the current version as detail.
    #[must_use]
    pub fn error(&self) -> Error {
        let current = match self {
            Self::NewerMinor { current, .. } | Self::NewerMajor { current, .. } => *current,
        };
        Error::new(codes::DOCUMENT_READ_ONLY)
            .with_arg("type", self.type_key().as_str())
            .with_arg("version", self.written().to_string())
            .with_detail("current", current.to_string())
    }
}

/// The versions a document declares and what they mean, from its type rows.
#[derive(Debug, Default)]
pub(crate) struct Plan {
    /// The declared `(major, minor)` versions of each type key.
    declared: BTreeMap<String, BTreeSet<(i64, i64)>>,
    /// Why the document is read-only, in type-key and version order.
    pub(crate) read_only: Vec<ReadOnlyReason>,
}

impl Plan {
    /// True when the type rows declare `version` of `type_key`.
    pub(crate) fn declares(&self, type_key: &str, version: SchemaVersion) -> bool {
        self.declared.get(type_key).is_some_and(|versions| {
            versions.contains(&(i64::from(version.major), i64::from(version.minor)))
        })
    }
}

/// Plans the load of a document with type rows `types`. A newer major version of a registered
/// type fails with `doc.load.newer_major` (arguments `type`, `version` and `current`) unless
/// `mode` is [`NewerMajor::ReadOnlyView`].
pub(crate) fn plan(
    registry: &TypeRegistry,
    types: &[StoredType],
    mode: NewerMajor,
) -> Result<Plan> {
    let mut plan = Plan::default();
    for row in types {
        plan.declared
            .entry(row.type_key.clone())
            .or_default()
            .insert((row.major, row.minor));
    }
    for (key, versions) in &plan.declared {
        let Some(decl) = registry.get(key) else {
            continue;
        };
        let current = decl.schema.version;
        for &(major, minor) in versions {
            if major > i64::from(current.major) && mode == NewerMajor::Refuse {
                return Err(Error::new(codes::LOAD_NEWER_MAJOR)
                    .with_arg("type", key.as_str())
                    .with_arg("version", format!("{major}.{minor}"))
                    .with_arg("current", current.to_string()));
            }
            let (Ok(major), Ok(minor)) = (u32::try_from(major), u32::try_from(minor)) else {
                continue;
            };
            let written = SchemaVersion::new(major, minor);
            let type_key = decl.key.clone();
            if major > current.major {
                plan.read_only.push(ReadOnlyReason::NewerMajor {
                    type_key,
                    written,
                    current,
                });
            } else if major == current.major && minor > current.minor {
                plan.read_only.push(ReadOnlyReason::NewerMinor {
                    type_key,
                    written,
                    current,
                });
            }
        }
    }
    Ok(plan)
}
