//! Loading a document from its stored rows (CORE-DOC-037, CORE-DOC-038, CORE-DOC-039).
//!
//! The package reads the `arc_type` and `arc_object` tables into the neutral rows
//! [`StoredType`] and [`StoredObject`]; [`load`] turns them into a [`Snapshot`] of revision 0
//! through the bulk load path, applying the version policy ([`crate::version`]) and migrations
//! ([`crate::migrate`]) and keeping what it cannot use as held objects ([`crate::preserved`]).
//! Loading writes nothing and records nothing in the journal: migrated values exist in memory
//! only until a save writes them.
//!
//! The rules and the write-back contract are specified in `docs/crates/arc-doc.md`, section
//! "Loading, migration and held objects".

use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};

use crate::error::codes;
use crate::migrate::decode_current;
use crate::payload::{self, Decoded, Encoding};
use crate::preserved::{HeldKind, HeldObject};
use crate::registry::TypeRegistry;
use crate::relationship::references;
use crate::schema::{Schema, SchemaVersion, Severity, TypeDecl, TypeKey};
use crate::snapshot::{BulkLoad, Flags, Header, Object, Snapshot};
use crate::version::{self, NewerMajor};

/// The `flags` bit of [`Flags::hidden_from_ui`].
pub const FLAG_HIDDEN_FROM_UI: i64 = 1;
/// The `flags` bit of [`Flags::system`].
pub const FLAG_SYSTEM: i64 = 2;
/// The `flags` bit of [`Flags::read_only`].
pub const FLAG_READ_ONLY: i64 = 4;
const KNOWN_FLAGS: i64 = FLAG_HIDDEN_FROM_UI | FLAG_SYSTEM | FLAG_READ_ONLY;

/// What each object row is charged against `Limits::memory_bytes` besides its bytes.
const ROW_OVERHEAD: u64 = 256;
/// What each payload byte is charged for the values it may decode to: a value takes 32 bytes and
/// is encoded in at least one byte.
const DECODED_PER_BYTE: u64 = 32;
/// What a held or unaddressable row is charged besides its bytes: the held object, its object,
/// the reason and the diagnostic (about 1.9 KiB measured in all, row bytes included; see
/// `load_scale`).
const HELD_OVERHEAD: u64 = 2048;
/// How many times a held row's type key is charged: the row, the key of the held object and
/// the arguments of the reason and of its diagnostic.
const KEY_COPIES: u64 = 4;
/// What the original row of a read-only document is charged besides its bytes and trailing
/// bytes: the side-table entry (about 0.7 KiB measured beyond a loaded object).
const SIDE_OVERHEAD: u64 = 768;
/// What the `last_txn` entry of a loaded object is charged besides the bytes of its value: an
/// entry of 20 bytes, and each distinct value once with its map entry.
const TXN_OVERHEAD: u64 = 64;

/// One `arc_object` row as stored, every column in its stored form; the payload is still
/// encoded. The loader treats it as hostile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredObject {
    /// `id`: the 16 bytes of the `ObjectId` in UUID byte order.
    pub id: Vec<u8>,
    /// `type_key`.
    pub type_key: String,
    /// `schema_major`.
    pub major: i64,
    /// `schema_minor`.
    pub minor: i64,
    /// `owner`: 16 identifier bytes, or `None`.
    pub owner: Option<Vec<u8>>,
    /// `name`.
    pub name: Option<String>,
    /// `partition`.
    pub partition: Option<String>,
    /// `flags`: [`FLAG_HIDDEN_FROM_UI`], [`FLAG_SYSTEM`] and [`FLAG_READ_ONLY`]; no other bit.
    pub flags: i64,
    /// `encoding`: the [`Encoding`] code.
    pub encoding: i64,
    /// `payload`.
    pub payload: Vec<u8>,
    /// `payload_hash`, carried unchanged; the package verifies it.
    pub payload_hash: Vec<u8>,
    /// `last_txn`, carried unchanged.
    pub last_txn: Vec<u8>,
}

/// One `arc_type` row: a type and schema version the document was written with, and its JSON
/// field descriptor (carried unchanged; building a schema from it is not provided yet).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredType {
    /// `type_key`.
    pub type_key: String,
    /// `major`.
    pub major: i64,
    /// `minor`.
    pub minor: i64,
    /// `descriptor`.
    pub descriptor: String,
}

impl StoredObject {
    /// The row of `object` of type `type_key` at the version of `schema`, with its payload
    /// encoded by [`payload::encode`]; `payload_hash` and `last_txn` are left empty for the
    /// package to fill. Fails like [`payload::encode`].
    pub fn of(
        type_key: &TypeKey,
        schema: &Schema,
        object: &Object,
        limits: &Limits,
    ) -> Result<Self> {
        let encoded = payload::encode(schema, &object.values, limits)?;
        let flags = &object.header.flags;
        Ok(Self {
            id: object.id.as_u128().to_be_bytes().to_vec(),
            type_key: type_key.as_str().to_owned(),
            major: i64::from(schema.version.major),
            minor: i64::from(schema.version.minor),
            owner: object
                .header
                .owner
                .map(|owner| owner.as_u128().to_be_bytes().to_vec()),
            name: object.header.name.clone(),
            partition: object.header.partition.clone(),
            flags: i64::from(flags.hidden_from_ui) * FLAG_HIDDEN_FROM_UI
                + i64::from(flags.system) * FLAG_SYSTEM
                + i64::from(flags.read_only) * FLAG_READ_ONLY,
            encoding: i64::from(encoded.encoding.code()),
            payload: encoded.bytes,
            payload_hash: Vec::new(),
            last_txn: Vec::new(),
        })
    }
}

/// The options of [`load`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadOptions {
    /// The hostile-input limits: `record_count` bounds the rows of each table,
    /// `memory_bytes` the estimate of what the loaded document retains, and the others every
    /// payload ([`payload::decode`]) and every name and partition (`string_bytes`).
    pub limits: Limits,
    /// What to do with a newer major version of a registered type.
    pub newer_major: NewerMajor,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            limits: Limits::DEFAULT,
            newer_major: NewerMajor::Refuse,
        }
    }
}

/// What a [`LoadDiagnostic`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    /// An object was preserved: its type is not registered, or is of a newer major version.
    Preserved,
    /// An object was quarantined.
    Quarantined,
    /// A row has no usable identifier (malformed, nil or a duplicate); it is kept for write-back
    /// only.
    Unaddressable,
    /// A type validator returned a warning for a loaded object, migrated or not.
    Warning,
    /// A reference of a loaded object names no loaded object, or one of a type it does not allow.
    Dangling,
}

/// A finding of loading.
#[derive(Debug)]
pub struct LoadDiagnostic {
    /// What it reports.
    pub kind: DiagnosticKind,
    /// The object concerned, when the row has a usable identifier.
    pub object: Option<ObjectId>,
    /// The code, message and arguments.
    pub error: Error,
}

/// The original row of a loaded object of a read-only document whose stored form differs from
/// the encoding of its loaded values: an older version (minor or major) or a newer minor
/// version.
#[derive(Debug)]
pub struct LoadedRow {
    /// The version that wrote the row.
    pub written: SchemaVersion,
    /// The row as stored.
    pub stored: StoredObject,
    /// The bytes after the known fields of a newer minor version; empty otherwise. The stored
    /// payload is the encoding of the loaded values at the current version followed by them.
    pub trailing: Vec<u8>,
}

/// The `last_txn` column of every loaded object, interned: one entry of an identifier and an
/// index per object, and each distinct value once.
#[derive(Debug, Default)]
struct LastTxns {
    /// Identifier bytes in UUID byte order and the index of the value, sorted by identifier.
    entries: Vec<([u8; 16], u32)>,
    values: Vec<Box<[u8]>>,
}

impl LastTxns {
    fn get(&self, id: ObjectId) -> Option<&[u8]> {
        let key = id.as_u128().to_be_bytes();
        let at = self
            .entries
            .binary_search_by(|(probe, _)| probe.cmp(&key))
            .ok()?;
        let (_, value) = self.entries.get(at)?;
        self.values
            .get(usize::try_from(*value).ok()?)
            .map(|value| &**value)
    }
}

/// The side table of a load: what a save needs beyond the snapshot to write rows back. The
/// write-back contract is specified in `docs/crates/arc-doc.md` ("Write-back contract"):
///
/// - Rows without a usable identifier ([`LoadedRows::unaddressable`]) and the rows of held
///   objects ([`crate::Snapshot::held`]) are written back unchanged.
/// - [`LoadedRows::last_txn`] gives the stored `last_txn` of every object loaded into the typed
///   stores, for objects no transaction changed since load.
/// - Only in a read-only document, [`LoadedRows::get`] gives the original row of each loaded
///   object whose stored row is not the encoding of its loaded values; a copy of the read-only
///   document writes those rows unchanged.
#[derive(Debug, Default)]
pub struct LoadedRows {
    rows: BTreeMap<ObjectId, LoadedRow>,
    unaddressable: Vec<StoredObject>,
    last_txn: LastTxns,
}

impl LoadedRows {
    /// The original row of `id` in a read-only document, when its stored form differs from its
    /// loaded values; always `None` in a writable document.
    #[must_use]
    pub fn get(&self, id: ObjectId) -> Option<&LoadedRow> {
        self.rows.get(&id)
    }

    /// The stored `last_txn` of the object `id` loaded into the typed stores; `None` for held
    /// objects (their rows carry it) and unknown identifiers.
    #[must_use]
    pub fn last_txn(&self, id: ObjectId) -> Option<&[u8]> {
        self.last_txn.get(id)
    }

    /// Every original row, in identifier order.
    pub fn iter(&self) -> impl Iterator<Item = (&ObjectId, &LoadedRow)> + '_ {
        self.rows.iter()
    }

    /// The number of original rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// True when no original row is kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows without a usable identifier, in load order.
    #[must_use]
    pub fn unaddressable(&self) -> &[StoredObject] {
        &self.unaddressable
    }
}

/// How many rows ended up where.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct LoadCounts {
    /// Object rows read.
    pub rows: u64,
    /// Objects in the typed stores.
    pub objects: u64,
    /// Of those, objects decoded from an older minor version of the current major version.
    pub older_minor: u64,
    /// Of those, objects migrated from an older major version.
    pub migrated: u64,
    /// Of those, objects of a newer minor version, with trailing bytes kept.
    pub newer_minor: u64,
    /// Preserved objects.
    pub preserved: u64,
    /// Quarantined objects.
    pub quarantined: u64,
    /// Rows without a usable identifier.
    pub unaddressable: u64,
    /// Validator warnings.
    pub warnings: u64,
    /// Dangling references.
    pub dangling: u64,
}

/// A loaded document.
#[derive(Debug)]
pub struct Loaded {
    /// The snapshot of revision 0. It is read-only when [`Snapshot::read_only`] lists reasons.
    pub snapshot: Snapshot,
    /// The findings, in row order, then the dangling references in handle order.
    pub diagnostics: Vec<LoadDiagnostic>,
    /// The type rows as read, for a save to write back those of held and as-stored rows.
    pub types: Vec<StoredType>,
    /// The side table a save needs.
    pub rows: LoadedRows,
    /// The counts.
    pub counts: LoadCounts,
}

/// Loads a document of `registry` from its type rows and object rows.
///
/// Fails, creating nothing and changing nothing, with:
/// - `doc.load.newer_major` when a type row declares a newer major version of a registered type
///   and `options.newer_major` is [`NewerMajor::Refuse`];
/// - `doc.load.limit_exceeded` (argument `limit`: `record_count` or `memory_bytes`) when either
///   table has more rows than `record_count`, or the estimated memory the loaded document
///   retains exceeds `memory_bytes`;
/// - `doc.store.capacity_exceeded` when a type has more objects than a store holds.
///
/// Everything else (unregistered types, invalid rows, failed or panicking migrations and
/// validators, dangling references) is kept and reported in the diagnostics. A panic of a
/// registered migration or validator is contained only where panics unwind.
pub fn load(
    registry: Arc<TypeRegistry>,
    types: &[StoredType],
    rows: impl IntoIterator<Item = StoredObject>,
    options: &LoadOptions,
) -> Result<Loaded> {
    let limits = &options.limits;
    if u64::try_from(types.len()).unwrap_or(u64::MAX) > limits.record_count {
        return Err(limit("record_count"));
    }
    let plan = version::plan(&registry, types, options.newer_major)?;
    // The type rows are kept and their keys copied into the plan.
    let mut memory = types.iter().fold(0_u64, |sum, row| {
        let bytes = len(row.type_key.len())
            .saturating_mul(2)
            .saturating_add(len(row.descriptor.len()));
        sum.saturating_add(ROW_OVERHEAD).saturating_add(bytes)
    });
    let mut loader = Loader {
        bulk: BulkLoad::new(registry),
        keep_rows: !plan.read_only.is_empty(),
        plan: &plan,
        limits,
        diagnostics: Vec::new(),
        rows: LoadedRows::default(),
        txns: HashMap::new(),
        counts: LoadCounts::default(),
    };
    let charge = |memory: &mut u64, cost: u64| {
        *memory = memory.saturating_add(cost);
        match limits.memory_bytes {
            Some(budget) if *memory > budget => Err(limit("memory_bytes")),
            _ => Ok(()),
        }
    };
    charge(&mut memory, 0)?;
    for row in rows {
        loader.counts.rows += 1;
        if loader.counts.rows > limits.record_count {
            return Err(limit("record_count"));
        }
        charge(&mut memory, cost(&row))?;
        let extra = loader.row(row)?;
        charge(&mut memory, extra)?;
    }
    let Loader {
        bulk,
        mut diagnostics,
        mut rows,
        txns,
        mut counts,
        ..
    } = loader;
    rows.last_txn.entries.sort_unstable_by_key(|entry| entry.0);
    let mut values: Vec<(u32, Box<[u8]>)> =
        txns.into_iter().map(|(value, at)| (at, value)).collect();
    values.sort_unstable_by_key(|(at, _)| *at);
    rows.last_txn.values = values.into_iter().map(|(_, value)| value).collect();
    let snapshot = bulk.finish(plan.read_only);
    dangling(&snapshot, &mut diagnostics, &mut counts);
    Ok(Loaded {
        snapshot,
        diagnostics,
        types: types.to_vec(),
        rows,
        counts,
    })
}

fn len(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn limit(name: &'static str) -> Error {
    Error::new(codes::LOAD_LIMIT_EXCEEDED).with_arg("limit", name)
}

/// An upper estimate of what a row and its decoded values retain.
fn cost(row: &StoredObject) -> u64 {
    let bytes = [
        row.id.len(),
        row.type_key.len(),
        row.owner.as_ref().map_or(0, Vec::len),
        row.name.as_ref().map_or(0, String::len),
        row.partition.as_ref().map_or(0, String::len),
        row.payload.len(),
        row.payload_hash.len(),
        row.last_txn.len(),
    ]
    .into_iter()
    .fold(ROW_OVERHEAD, |sum, n| sum.saturating_add(len(n)));
    bytes.saturating_add(len(row.payload.len()).saturating_mul(DECODED_PER_BYTE))
}

struct Loader<'a> {
    bulk: BulkLoad,
    /// True for a read-only document, whose original rows are kept for a copy.
    keep_rows: bool,
    plan: &'a version::Plan,
    limits: &'a Limits,
    diagnostics: Vec<LoadDiagnostic>,
    rows: LoadedRows,
    /// The distinct `last_txn` values and their indexes.
    txns: HashMap<Box<[u8]>, u32>,
    counts: LoadCounts,
}

impl Loader<'_> {
    /// Loads one row and returns what it retains beyond [`cost`].
    fn row(&mut self, row: StoredObject) -> Result<u64> {
        let Some(id) = parse_id(&row.id) else {
            self.unaddressable(None, header_invalid("id"), row);
            return Ok(HELD_OVERHEAD);
        };
        if self.bulk.contains(id) {
            let error = Error::new(codes::OBJECT_DUPLICATE_ID).with_arg("id", id.to_string());
            self.unaddressable(Some(id), error, row);
            return Ok(HELD_OVERHEAD);
        }
        let registry = Arc::clone(self.bulk.registry());
        let held_cost =
            HELD_OVERHEAD.saturating_add(len(row.type_key.len()).saturating_mul(KEY_COPIES));
        let decl = if len(row.type_key.len()) > self.limits.string_bytes {
            None
        } else {
            registry.get(row.type_key.as_str())
        };
        // A registered key is valid; only other keys need parsing.
        let key = match decl {
            Some(decl) => Ok(decl.key.clone()),
            None if len(row.type_key.len()) > self.limits.string_bytes => {
                Err(header_invalid("type_key"))
            }
            None => TypeKey::new(row.type_key.as_str()).map_err(|_| header_invalid("type_key")),
        };
        let version = parse_version(&row);
        let (header, header_error) = parse_header(&row, self.limits);
        let (key, version) = match (key, version, header_error) {
            (Ok(key), Ok(version), None) => (key, version),
            (key, version, header_error) => {
                // Keep what is valid of the object; the row itself is written back unchanged.
                let error = [
                    key.as_ref().err().map(copy),
                    version.as_ref().err().map(copy),
                    header_error,
                ]
                .into_iter()
                .flatten()
                .next()
                .unwrap_or_else(|| header_invalid("header"));
                let held = Object::new(
                    id,
                    version.unwrap_or(SchemaVersion::new(0, 0)),
                    header,
                    Vec::new(),
                );
                self.hold(HeldKind::Quarantined, key.ok(), held, row, error)?;
                return Ok(held_cost);
            }
        };
        let object = |values| Object::new(id, version, header.clone(), values);
        let Some(decl) = decl else {
            let error = Error::new(codes::TYPE_UNKNOWN).with_arg("type", key.as_str());
            self.hold(
                HeldKind::Preserved,
                Some(key),
                object(Vec::new()),
                row,
                error,
            )?;
            return Ok(held_cost);
        };
        let (decoded, warnings) = match self.decode(decl, version, &row) {
            Ok(decoded) => decoded,
            Err((kind, error)) => {
                self.hold(kind, Some(key), object(Vec::new()), row, error)?;
                return Ok(held_cost);
            }
        };
        let current = decl.schema.version;
        for error in warnings {
            self.counts.warnings += 1;
            self.diagnose(DiagnosticKind::Warning, Some(id), error);
        }
        self.bulk.insert(
            decl.key.as_str(),
            Object::new(id, current, header, decoded.values),
        )?;
        self.counts.objects += 1;
        let mut extra = self.last_txn(id, &row.last_txn);
        if version != current {
            if version.major < current.major {
                self.counts.migrated += 1;
            } else if version.minor < current.minor {
                self.counts.older_minor += 1;
            } else {
                self.counts.newer_minor += 1;
            }
            if self.keep_rows {
                extra = extra
                    .saturating_add(SIDE_OVERHEAD)
                    .saturating_add(len(decoded.trailing.len()));
                let entry = LoadedRow {
                    written: version,
                    stored: row,
                    trailing: decoded.trailing,
                };
                self.rows.rows.insert(id, entry);
            }
        }
        Ok(extra)
    }

    /// Records the `last_txn` of a loaded object and returns what it retains.
    fn last_txn(&mut self, id: ObjectId, value: &[u8]) -> u64 {
        let next = u32::try_from(self.txns.len()).unwrap_or(u32::MAX);
        let (at, cost) = match self.txns.get(value) {
            Some(&at) => (at, TXN_OVERHEAD),
            None => {
                self.txns.insert(Box::from(value), next);
                (next, TXN_OVERHEAD.saturating_add(len(value.len())))
            }
        };
        self.rows
            .last_txn
            .entries
            .push((id.as_u128().to_be_bytes(), at));
        cost
    }

    /// Decodes and validates a row of the registered type `decl` written at `version`: the
    /// values at the current version with the validator warnings, or how and why the object is
    /// held.
    fn decode(
        &self,
        decl: &TypeDecl,
        version: SchemaVersion,
        row: &StoredObject,
    ) -> std::result::Result<(Decoded, Vec<Error>), (HeldKind, Error)> {
        let key = decl.key.as_str();
        let current = decl.schema.version;
        if !self.plan.declares(key, version) {
            let error = Error::new(codes::LOAD_VERSION_UNDECLARED)
                .with_arg("type", key)
                .with_arg("version", version.to_string());
            return Err((HeldKind::Quarantined, error));
        }
        if version.major > current.major {
            // Only the read-only view gets here: the plan refuses newer majors otherwise.
            let error = Error::new(codes::LOAD_NEWER_MAJOR)
                .with_arg("type", key)
                .with_arg("version", version.to_string())
                .with_arg("current", current.to_string());
            return Err((HeldKind::Preserved, error));
        }
        let quarantine = |error| (HeldKind::Quarantined, error);
        let encoding = u8::try_from(row.encoding)
            .ok()
            .and_then(Encoding::from_code)
            .ok_or_else(|| {
                Error::new(codes::PAYLOAD_ENCODING_UNSUPPORTED)
                    .with_detail("encoding", row.encoding)
            })
            .map_err(quarantine)?;
        let decoded = decode_current(decl, version, encoding, &row.payload, self.limits)
            .map_err(quarantine)?;
        let mut warnings = Vec::new();
        for validator in &decl.validators {
            // A panicking validator is a defect of its type; it quarantines the object only.
            let findings =
                catch_unwind(AssertUnwindSafe(|| validator(&decoded.values))).map_err(|_| {
                    quarantine(Error::new(codes::LOAD_VALIDATOR_PANICKED).with_arg("type", key))
                })?;
            for finding in findings {
                match finding.severity {
                    Severity::Error => return Err(quarantine(finding.error)),
                    Severity::Warning => warnings.push(finding.error),
                }
            }
        }
        Ok((decoded, warnings))
    }

    fn hold(
        &mut self,
        kind: HeldKind,
        key: Option<TypeKey>,
        object: Object,
        row: StoredObject,
        reason: Error,
    ) -> Result<()> {
        let id = object.id;
        let (diagnostic, count) = match kind {
            HeldKind::Preserved => (DiagnosticKind::Preserved, &mut self.counts.preserved),
            HeldKind::Quarantined => (DiagnosticKind::Quarantined, &mut self.counts.quarantined),
        };
        *count += 1;
        self.diagnose(diagnostic, Some(id), copy(&reason));
        self.bulk
            .hold(HeldObject::new(kind, key, object, row, reason))
    }

    fn unaddressable(&mut self, id: Option<ObjectId>, error: Error, row: StoredObject) {
        self.counts.unaddressable += 1;
        self.diagnose(DiagnosticKind::Unaddressable, id, error);
        self.rows.unaddressable.push(row);
    }

    fn diagnose(&mut self, kind: DiagnosticKind, object: Option<ObjectId>, error: Error) {
        self.diagnostics.push(LoadDiagnostic {
            kind,
            object,
            error,
        });
    }
}

/// A copy of `error` without its source chain.
fn copy(error: &Error) -> Error {
    let mut copied = Error::new(arc_base::ErrorCode {
        code: error.code(),
        category: error.category(),
        message_id: error.message_id(),
    });
    for (name, value) in error.arguments() {
        copied = copied.with_arg(name, value.clone());
    }
    for (name, value) in error.details() {
        copied = copied.with_detail(name, value.clone());
    }
    copied
}

fn header_invalid(column: &'static str) -> Error {
    Error::new(codes::LOAD_HEADER_INVALID).with_arg("column", column)
}

/// The identifier of 16 bytes in UUID byte order; the nil identifier is not one.
fn parse_id(bytes: &[u8]) -> Option<ObjectId> {
    let raw = u128::from_be_bytes(bytes.try_into().ok()?);
    (raw != 0).then(|| ObjectId::from_u128(raw))
}

fn parse_version(row: &StoredObject) -> Result<SchemaVersion> {
    let major = u32::try_from(row.major)
        .ok()
        .filter(|major| *major >= 1)
        .ok_or_else(|| header_invalid("schema_major"))?;
    let minor = u32::try_from(row.minor).map_err(|_| header_invalid("schema_minor"))?;
    Ok(SchemaVersion::new(major, minor))
}

/// The header of `row` with every valid column, and the error of the first invalid one (in the
/// order owner, flags, name, partition); an invalid column takes its default.
fn parse_header(row: &StoredObject, limits: &Limits) -> (Header, Option<Error>) {
    let mut first = None;
    let mut keep = |error: Error| {
        first.get_or_insert(error);
    };
    let owner = match &row.owner {
        None => None,
        Some(bytes) => parse_id(bytes).or_else(|| {
            keep(header_invalid("owner"));
            None
        }),
    };
    let flags = if row.flags & !KNOWN_FLAGS == 0 {
        Flags {
            hidden_from_ui: row.flags & FLAG_HIDDEN_FROM_UI != 0,
            system: row.flags & FLAG_SYSTEM != 0,
            read_only: row.flags & FLAG_READ_ONLY != 0,
        }
    } else {
        keep(header_invalid("flags"));
        Flags::default()
    };
    let mut text = |text: &Option<String>, column| match text {
        Some(text) if len(text.len()) > limits.string_bytes => {
            keep(header_invalid(column));
            None
        }
        _ => text.clone(),
    };
    let name = text(&row.name, "name");
    let partition = text(&row.partition, "partition");
    let header = Header {
        owner,
        name,
        partition,
        flags,
    };
    (header, first)
}

/// Lists every reference of a loaded object, held objects' owners included, that names no
/// loaded object or one of a type its field does not allow.
fn dangling(snapshot: &Snapshot, diagnostics: &mut Vec<LoadDiagnostic>, counts: &mut LoadCounts) {
    let registry = snapshot.registry();
    for (handle, object) in snapshot.iter().chain(
        snapshot
            .held()
            .map(|(handle, held)| (handle, held.object())),
    ) {
        let key = snapshot.type_key(handle).map_or("", TypeKey::as_str);
        for reference in references(registry, key, object) {
            let allowed = snapshot.resolve(reference.target).is_ok_and(|target| {
                reference.spec.targets.is_empty()
                    || snapshot
                        .type_key(target)
                        .is_some_and(|key| reference.spec.targets.contains(key))
            });
            if !allowed {
                counts.dangling += 1;
                diagnostics.push(LoadDiagnostic {
                    kind: DiagnosticKind::Dangling,
                    object: Some(object.id),
                    error: Error::new(codes::REFERENCE_DANGLING)
                        .with_arg("field", reference.field.to_string())
                        .with_detail("source", object.id.to_string())
                        .with_detail("target", reference.target.to_string()),
                });
            }
        }
    }
}
