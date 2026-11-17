//! Schema migration on load (CORE-DOC-037).
//!
//! A payload of an older major version `M` of a type at major version `N` is decoded with the
//! source schema of `M` (the schema of its [`crate::Migration`]), then lifted one major version
//! at a time: the migration whose source is `M` turns the values into values of `M + 1`, which
//! must pass the field schema check of the schema of `M + 1` (the next migration's source, or the
//! current schema), and so on up to `N`. Migrations are pure and run in memory only; the stored
//! row stays as it was until a save writes the object at the current version.
//!
//! An object is not migrated, and fails with `doc.migration.failed`, when its payload holds
//! trailing bytes of a newer minor version of its old major version (a migration sees decoded
//! values only and would drop them), when a migration function fails, or when its result fails
//! the field schema check. Failures of decoding keep their own codes. The loader quarantines such
//! objects.

use arc_base::{Error, Limits, Result};

use crate::error::codes;
use crate::payload::{self, Decoded, Encoding};
use crate::schema::{SchemaVersion, TypeDecl, check_values};

/// Decodes a payload of `decl` written at version `written` and, when that is an older major
/// version, migrates its values to the current version. The result holds one value per field
/// of the current schema; its trailing bytes are those of a newer minor version of the current
/// major version, and empty otherwise.
///
/// A newer major version fails with `doc.load.newer_major`; see the module documentation for
/// the other failures.
pub fn decode_current(
    decl: &TypeDecl,
    written: SchemaVersion,
    encoding: Encoding,
    bytes: &[u8],
    limits: &Limits,
) -> Result<Decoded> {
    let current = decl.schema.version;
    if written.major > current.major {
        return Err(Error::new(codes::LOAD_NEWER_MAJOR)
            .with_arg("type", decl.key.as_str())
            .with_arg("version", written.to_string())
            .with_arg("current", current.to_string()));
    }
    if written.major == current.major {
        return payload::decode(&decl.schema, written.minor, encoding, bytes, limits);
    }
    let failed = |from: u32, reason: &'static str| {
        Error::new(codes::MIGRATION_FAILED)
            .with_arg("type", decl.key.as_str())
            .with_arg("from", written.to_string())
            .with_detail("step", i64::from(from))
            .with_detail("reason", reason)
    };
    let source = decl
        .schema(written.major)
        .ok_or_else(|| failed(written.major, "no migration"))?;
    let decoded = payload::decode(source, written.minor, encoding, bytes, limits)?;
    if !decoded.trailing.is_empty() {
        return Err(failed(written.major, "trailing bytes"));
    }
    let mut values = decoded.values;
    // One step per major version; each step needs its own migration, so the registry's chain
    // bounds the loop.
    for major in written.major..current.major {
        let migration = decl
            .migrations
            .iter()
            .find(|migration| migration.source.version.major == major)
            .ok_or_else(|| failed(major, "no migration"))?;
        let next = decl
            .schema(major.saturating_add(1))
            .ok_or_else(|| failed(major, "no migration"))?;
        values = (migration.migrate)(values)
            .map_err(|cause| failed(major, "migration failed").with_source(cause))?;
        check_values(&next.fields, &values)
            .map_err(|cause| failed(major, "invalid result").with_source(cause))?;
    }
    Ok(Decoded {
        values,
        trailing: Vec::new(),
    })
}
