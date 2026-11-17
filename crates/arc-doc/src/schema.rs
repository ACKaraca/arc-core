//! The value model of object fields and the declarations of object types (CORE-DOC-001).
//!
//! A [`TypeDecl`] names a type by its [`TypeKey`] and carries the [`Schema`] of its current
//! version, the schemas of its older major versions with the [`Migration`]s that lift their
//! values, and its [`TypeValidator`]s. A [`Schema`] is an ordered list of [`FieldDecl`]s; each
//! field has a [`ValueType`] and its values are [`Value`]s.
//!
//! The values of one object are a slice of `Option<Value>`, one entry per field in declared
//! order; `None` is an absent optional field.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;

use arc_base::math::{Point2, Point3, Transform3, Vector3};
use arc_base::{Color, Dimension, Error, ObjectId, Quantity, Result};
use serde::{Deserialize, Serialize};

use crate::error::codes;

/// The longest type key, in bytes.
pub const MAX_KEY_BYTES: usize = 128;
/// The longest field, enum variant or aspect name, in bytes.
pub const MAX_NAME_BYTES: usize = 64;

/// True when `text` is a name: a lowercase ASCII letter followed by lowercase letters, digits
/// and underscores, at most [`MAX_NAME_BYTES`] long.
#[must_use]
pub fn is_name(text: &str) -> bool {
    text.len() <= MAX_NAME_BYTES
        && text.starts_with(|c: char| c.is_ascii_lowercase())
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// True when `text` is a dotted key: two or more names separated by single dots, at most
/// [`MAX_KEY_BYTES`] long.
#[must_use]
pub fn is_key(text: &str) -> bool {
    text.len() <= MAX_KEY_BYTES && text.contains('.') && text.split('.').all(is_name)
}

/// The namespaced key of an object type, for example `core.doc.link` or
/// `archbuilder.wall`.
///
/// A key is two or more dot-separated segments; each segment starts with a lowercase ASCII
/// letter and continues with lowercase letters, digits and underscores. The first segment names
/// the owner (`core` for types of `arc-core`, the repository name for product types). The whole
/// key is at most [`MAX_KEY_BYTES`] long.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TypeKey(String);

impl TypeKey {
    /// Validates `key`; a malformed key fails with `doc.type.malformed_key`.
    pub fn new(key: impl Into<String>) -> Result<Self> {
        let key = key.into();
        if is_key(&key) {
            Ok(Self(key))
        } else {
            Err(Error::new(codes::TYPE_MALFORMED_KEY).with_arg("key", key))
        }
    }

    /// The key as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TypeKey {
    type Error = Error;

    fn try_from(key: String) -> Result<Self> {
        Self::new(key)
    }
}

impl From<TypeKey> for String {
    fn from(key: TypeKey) -> Self {
        key.0
    }
}

impl Borrow<str> for TypeKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TypeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A schema version `major.minor`. Major versions start at 1.
///
/// A minor increment only appends fields at the end of the field list; a major increment may
/// change anything and needs a [`Migration`] from the previous major.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaVersion {
    /// The major version.
    pub major: u32,
    /// The minor version.
    pub minor: u32,
}

impl SchemaVersion {
    /// Creates a version.
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The BLAKE3 hash of an asset's uncompressed bytes, which identifies the asset in the
/// document's asset store.
///
/// This unit stores and compares the 32 bytes only; computing hashes and storing assets belong
/// to the package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetHash(pub [u8; 32]);

/// A geometry value in the canonical byte form of the crate that owns its encoding
/// (`arc-geom`, `arc-mesh`, `arc-brep`).
///
/// The field's [`GeometryEncoding`] names the encoding and its version. The document store keeps
/// the bytes unchanged and does not interpret them; decoding them is the owning crate's task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GeometryBytes(pub Vec<u8>);

/// Where a color came from, kept beside its linear value so that formats with indexed or named
/// colors write them back unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSource {
    /// An index into an indexed palette.
    Indexed(u8),
    /// A named color.
    Named(String),
}

/// The value of a `color` field: linear RGBA and an optional source.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorValue {
    /// The linear color; every component is finite.
    pub rgba: Color,
    /// The indexed or named source, if the color has one.
    pub source: Option<ColorSource>,
}

/// A reference into a linked document: the link object and the target in the linked
/// document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExternalRef {
    /// The `core.doc.link` object of this document.
    pub link: ObjectId,
    /// The target object in the linked document.
    pub target: ObjectId,
}

/// The dynamically typed value of a field.
///
/// Every variant corresponds to the [`ValueType`] of the same name. Floating-point values are
/// finite. Serializing a value with `serde` yields its canonical JSON form, used in field
/// descriptors: numbers, strings, arrays of coordinates, identifiers as hyphenated text, byte
/// strings and hashes as lowercase hexadecimal; payloads use [`crate::payload`] instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A truth value.
    Bool(bool),
    /// A signed 64-bit integer.
    I64(i64),
    /// A 64-bit floating-point number.
    F64(f64),
    /// UTF-8 text.
    String(String),
    /// A value in SI base units with the dimension declared by the field.
    Quantity(Quantity),
    /// The numeric code of an enum variant.
    Enum(u32),
    /// A point in the plane.
    Point2(Point2),
    /// A point in space.
    Point3(Point3),
    /// A vector in space.
    Vector3(Vector3),
    /// An affine transform of space.
    Transform(Transform3),
    /// A color.
    Color(ColorValue),
    /// The identifier of the target object.
    Reference(ObjectId),
    /// A reference into a linked document.
    ExternalReference(ExternalRef),
    /// The hash of an asset.
    Asset(AssetHash),
    /// A geometry value in its owning crate's encoding.
    Geometry(GeometryBytes),
    /// The elements of a list.
    List(Vec<Value>),
    /// The entries of a map, ordered by key.
    Map(BTreeMap<String, Value>),
    /// The field values of a nested record, in declared order.
    Record(Vec<Option<Value>>),
    /// Bytes kept for a foreign or newer schema and never interpreted.
    Opaque(Vec<u8>),
}

/// A variant of an enum value type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnumVariant {
    /// The variant name, a [name](is_name).
    pub name: String,
    /// The stable numeric code stored in payloads.
    pub code: u32,
}

impl EnumVariant {
    /// Creates a variant.
    #[must_use]
    pub fn new(name: impl Into<String>, code: u32) -> Self {
        Self {
            name: name.into(),
            code,
        }
    }
}

/// The named and versioned encoding of a geometry field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GeometryEncoding {
    /// The encoding name in the [key](is_key) grammar, owned by the crate that defines it.
    pub name: String,
    /// The encoding version.
    pub version: u32,
}

/// The strength of a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    /// The target's life ends with the source: deleting the source deletes the target.
    Owning,
    /// The source depends on the target.
    Associative,
    /// Navigation only.
    Informational,
}

/// What happens to the source when the target of a reference is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnDelete {
    /// The source is deleted too.
    Cascade,
    /// The reference is removed: an optional field becomes absent, a list loses the element.
    Clear,
    /// The deleting transaction is refused.
    Block,
}

/// What a copy of the source does with the target of a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnCopy {
    /// The target is copied and the copy refers to the new target.
    CopyTarget,
    /// The copy refers to the same target.
    ShareTarget,
    /// The copy drops the reference.
    Clear,
}

/// How many targets a reference field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cardinality {
    /// Exactly one: a required reference field.
    One,
    /// Zero or one: an optional reference field.
    Optional,
    /// A list of references with at most `max` elements.
    Many {
        /// The largest number of references; at least 1.
        max: u32,
    },
}

/// The relationship rule every reference declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelationshipRule {
    /// The strength.
    pub strength: Strength,
    /// The effect of deleting the target.
    pub on_delete: OnDelete,
    /// The effect of copying the source.
    pub on_copy: OnCopy,
    /// How many targets the field holds.
    pub cardinality: Cardinality,
}

/// The target types and the relationship rule of a reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReferenceSpec {
    /// The type keys a target may have; empty allows any type.
    pub targets: Vec<TypeKey>,
    /// The relationship rule.
    pub rule: RelationshipRule,
}

/// The value type of a field.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueType {
    /// A truth value.
    Bool,
    /// A signed 64-bit integer.
    I64,
    /// A finite 64-bit floating-point number.
    F64,
    /// UTF-8 text.
    String,
    /// A finite value in SI base units of the given dimension.
    Quantity(Dimension),
    /// One of a closed set of variants, stored by code.
    Enum(Vec<EnumVariant>),
    /// A point in the plane.
    Point2,
    /// A point in space.
    Point3,
    /// A vector in space.
    Vector3,
    /// An affine transform of space.
    Transform,
    /// Linear RGBA with an optional source.
    Color,
    /// The identifier of an object of this document.
    Reference(ReferenceSpec),
    /// A link and an object of the linked document.
    ExternalReference,
    /// The hash of an asset.
    Asset,
    /// A geometry value in the named encoding.
    Geometry(GeometryEncoding),
    /// A list of values of one type.
    List(Box<ValueType>),
    /// A map from text keys to values of one type.
    Map(Box<ValueType>),
    /// A nested record with its own fields; it has at least one field.
    Record(Vec<FieldDecl>),
    /// Bytes that are never interpreted.
    Opaque,
}

/// The valid range or length bound of a field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bound {
    /// The inclusive range of an `i64` field.
    Int {
        /// The smallest valid value.
        min: i64,
        /// The largest valid value.
        max: i64,
    },
    /// The inclusive range of an `f64` or `quantity` field, in SI base units; both ends are
    /// finite.
    Real {
        /// The smallest valid value.
        min: f64,
        /// The largest valid value.
        max: f64,
    },
    /// The largest length of a `string` (in bytes), `list` or `map` (in entries) or `opaque`
    /// (in bytes) field.
    Length {
        /// The largest valid length.
        max: u64,
    },
}

/// The aspect of fields that belong to no other aspect.
pub const DEFAULT_ASPECT: &str = "data";

/// The declaration of one field.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    /// The field name, a [name](is_name) unique among its siblings.
    pub name: String,
    /// The value type.
    pub value_type: ValueType,
    /// The value a new object gets; it satisfies the type and the bound.
    pub default: Option<Value>,
    /// The valid range or length bound.
    pub bound: Option<Bound>,
    /// Whether the field may be absent.
    pub optional: bool,
    /// Whether the field is copied to the package property index for queries.
    pub indexed: bool,
    /// The aspect, the unit of dependency tracking: `geometry`, `appearance`, `identity`,
    /// `data` or another [name](is_name).
    pub aspect: String,
    /// The minor version of the type's major version that appended the field; 0 for fields of
    /// the first minor version and for the fields of nested records.
    pub since_minor: u32,
}

impl FieldDecl {
    /// A required, unindexed field of the `data` aspect without default or bound, present
    /// since minor version 0.
    #[must_use]
    pub fn new(name: impl Into<String>, value_type: ValueType) -> Self {
        Self {
            name: name.into(),
            value_type,
            default: None,
            bound: None,
            optional: false,
            indexed: false,
            aspect: DEFAULT_ASPECT.to_owned(),
            since_minor: 0,
        }
    }

    /// Makes the field optional.
    #[must_use]
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// Marks the field as indexed.
    #[must_use]
    pub fn indexed(mut self) -> Self {
        self.indexed = true;
        self
    }

    /// Sets the default value.
    #[must_use]
    pub fn with_default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }

    /// Sets the bound.
    #[must_use]
    pub fn with_bound(mut self, bound: Bound) -> Self {
        self.bound = Some(bound);
        self
    }

    /// Sets the aspect.
    #[must_use]
    pub fn in_aspect(mut self, aspect: impl Into<String>) -> Self {
        self.aspect = aspect.into();
        self
    }

    /// Sets the minor version that appended the field.
    #[must_use]
    pub fn since_minor(mut self, minor: u32) -> Self {
        self.since_minor = minor;
        self
    }
}

/// The fields of one major version of a type, at its last minor version.
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    /// The version.
    pub version: SchemaVersion,
    /// The fields in declared order, which is also the payload order.
    pub fields: Vec<FieldDecl>,
}

impl Schema {
    /// Creates a schema.
    #[must_use]
    pub fn new(version: SchemaVersion, fields: Vec<FieldDecl>) -> Self {
        Self { version, fields }
    }

    /// The fields present at minor version `minor` of this major version: the prefix of fields
    /// appended at or before it.
    #[must_use]
    pub fn fields_at(&self, minor: u32) -> &[FieldDecl] {
        let count = self
            .fields
            .iter()
            .take_while(|field| field.since_minor <= minor)
            .count();
        self.fields.get(..count).unwrap_or_default()
    }
}

/// How serious a validation finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// The transaction is rejected.
    Error,
    /// The finding becomes a diagnostic and does not block.
    Warning,
}

/// A finding of a [`TypeValidator`].
#[derive(Debug)]
pub struct Finding {
    /// Whether the finding rejects the transaction.
    pub severity: Severity,
    /// The code, message and arguments of the finding.
    pub error: Error,
}

/// A type validator: receives the field values of one object in declared order and returns its
/// findings. It runs at commit on touched objects, after field and reference checks, and must be
/// fast and free of side effects.
pub type TypeValidator = fn(&[Option<Value>]) -> Vec<Finding>;

/// A migration: a pure function from the field values of one major version (at the minor
/// version of its [`Migration::source`]) to those of the next major version (at the minor
/// version of the next schema in the chain).
pub type MigrateFn = fn(Vec<Option<Value>>) -> Result<Vec<Option<Value>>>;

/// The schema of an older major version and the function that lifts its values to the next
/// major version.
#[derive(Debug, Clone)]
pub struct Migration {
    /// The schema of the older major version; its fields decode payloads of that version.
    pub source: Schema,
    /// The function from values of `source` to values of the next major version.
    pub migrate: MigrateFn,
}

/// The declaration of an object type.
#[derive(Debug, Clone)]
pub struct TypeDecl {
    /// The type key.
    pub key: TypeKey,
    /// The schema of the current version.
    pub schema: Schema,
    /// The type validators, run in this order.
    pub validators: Vec<TypeValidator>,
    /// One migration for every major version from 1 up to the current one, exclusive.
    pub migrations: Vec<Migration>,
}

impl TypeDecl {
    /// A type with no validators and no migrations.
    #[must_use]
    pub fn new(key: TypeKey, schema: Schema) -> Self {
        Self {
            key,
            schema,
            validators: Vec::new(),
            migrations: Vec::new(),
        }
    }

    /// The schema of major version `major`: the current one or the source of a migration.
    #[must_use]
    pub fn schema(&self, major: u32) -> Option<&Schema> {
        std::iter::once(&self.schema)
            .chain(self.migrations.iter().map(|migration| &migration.source))
            .find(|schema| schema.version.major == major)
    }

    /// Every version the declaration describes, ascending: each minor version of each major
    /// version.
    #[must_use]
    pub fn versions(&self) -> Vec<SchemaVersion> {
        let mut versions: Vec<SchemaVersion> = std::iter::once(&self.schema)
            .chain(self.migrations.iter().map(|migration| &migration.source))
            .flat_map(|schema| {
                let major = schema.version.major;
                (0..=schema.version.minor).map(move |minor| SchemaVersion::new(major, minor))
            })
            .collect();
        versions.sort();
        versions
    }
}

/// Checks the values of a field list: one entry per field, every required field present, every
/// value of its field's type, finite and within its bound and enum codes. This is the field
/// schema check of commit validation.
///
/// Fails with `doc.value.type_mismatch` (also for a wrong number of entries),
/// `doc.value.missing`, `doc.value.out_of_range` or `doc.value.not_finite`; the `field` argument
/// names the offending field as a dotted path through nested records.
pub fn check_values(fields: &[FieldDecl], values: &[Option<Value>]) -> Result<()> {
    if fields.len() != values.len() {
        return Err(Error::new(codes::VALUE_TYPE_MISMATCH)
            .with_detail("expected_fields", count(fields.len()))
            .with_detail("actual_fields", count(values.len())));
    }
    for (field, value) in fields.iter().zip(values) {
        let checked = match value {
            None if field.optional => Ok(()),
            None => Err(Error::new(codes::VALUE_MISSING)),
            Some(value) => check_value(&field.value_type, field.bound.as_ref(), value),
        };
        checked.map_err(|error| prefix_field(error, &field.name))?;
    }
    Ok(())
}

/// Saturates a length to `i64` for an error detail.
pub(crate) fn count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Sets the `field` argument of `error` to `name`, followed by the path already recorded.
pub(crate) fn prefix_field(error: Error, name: &str) -> Error {
    let path = match error.arguments().get("field") {
        Some(arc_base::error::Value::Text(inner)) => format!("{name}.{inner}"),
        _ => name.to_owned(),
    };
    error.with_arg("field", path)
}

fn finite(values: &[f64]) -> Result<()> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(Error::new(codes::VALUE_NOT_FINITE))
    }
}

fn within(ok: bool) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::new(codes::VALUE_OUT_OF_RANGE))
    }
}

fn length_within(bound: Option<&Bound>, length: usize) -> Result<()> {
    match bound {
        Some(Bound::Length { max }) => within(u64::try_from(length).is_ok_and(|n| n <= *max)),
        _ => Ok(()),
    }
}

/// The twelve stored entries of an affine transform: the top three rows of its 4×4 matrix in
/// column-major order.
pub(crate) fn transform_entries(transform: &Transform3) -> [f64; 12] {
    let mut entries = [0.0; 12];
    // The matrix iterates column by column; row 3 of each column is the fixed `0 0 0 1`.
    let stored = transform
        .matrix()
        .iter()
        .enumerate()
        .filter(|(index, _)| index % 4 != 3)
        .map(|(_, entry)| *entry);
    for (slot, entry) in entries.iter_mut().zip(stored) {
        *slot = entry;
    }
    entries
}

/// The affine transform of twelve entries in the order of [`transform_entries`].
pub(crate) fn transform_from_entries(entries: &[f64; 12]) -> Transform3 {
    let matrix = arc_base::math::Matrix4::from_fn(|row, column| match row {
        3 if column == 3 => 1.0,
        3 => 0.0,
        _ => entries.get(column * 3 + row).copied().unwrap_or(0.0),
    });
    Transform3::from_matrix_unchecked(matrix)
}

pub(crate) fn check_value(
    value_type: &ValueType,
    bound: Option<&Bound>,
    value: &Value,
) -> Result<()> {
    match (value_type, value) {
        (ValueType::Bool, Value::Bool(_))
        | (ValueType::Reference(_), Value::Reference(_))
        | (ValueType::ExternalReference, Value::ExternalReference(_))
        | (ValueType::Asset, Value::Asset(_))
        | (ValueType::Geometry(_), Value::Geometry(_)) => Ok(()),
        (ValueType::I64, Value::I64(v)) => match bound {
            Some(Bound::Int { min, max }) => within(min <= v && v <= max),
            _ => Ok(()),
        },
        (ValueType::F64, Value::F64(v)) => check_real(*v, bound),
        (ValueType::Quantity(dimension), Value::Quantity(q)) => {
            if q.dimension() == *dimension {
                check_real(q.value(), bound)
            } else {
                Err(Error::new(codes::VALUE_TYPE_MISMATCH))
            }
        }
        (ValueType::String, Value::String(s)) => length_within(bound, s.len()),
        (ValueType::Opaque, Value::Opaque(bytes)) => length_within(bound, bytes.len()),
        (ValueType::Enum(variants), Value::Enum(code)) => {
            within(variants.iter().any(|variant| variant.code == *code))
        }
        (ValueType::Point2, Value::Point2(p)) => finite(&[p.x, p.y]),
        (ValueType::Point3, Value::Point3(p)) => finite(&[p.x, p.y, p.z]),
        (ValueType::Vector3, Value::Vector3(v)) => finite(&[v.x, v.y, v.z]),
        (ValueType::Transform, Value::Transform(t)) => {
            finite(&transform_entries(t))?;
            let bottom = t.matrix().row(3);
            if bottom.iter().eq([0.0, 0.0, 0.0, 1.0].iter()) {
                Ok(())
            } else {
                Err(Error::new(codes::VALUE_TYPE_MISMATCH))
            }
        }
        (ValueType::Color, Value::Color(c)) => {
            let rgba = [c.rgba.r, c.rgba.g, c.rgba.b, c.rgba.a];
            finite(&rgba.map(f64::from))
        }
        (ValueType::List(element), Value::List(items)) => {
            length_within(bound, items.len())?;
            if let ValueType::Reference(ReferenceSpec {
                rule:
                    RelationshipRule {
                        cardinality: Cardinality::Many { max },
                        ..
                    },
                ..
            }) = element.as_ref()
            {
                within(u32::try_from(items.len()).is_ok_and(|n| n <= *max))?;
            }
            for (index, item) in items.iter().enumerate() {
                check_value(element, None, item)
                    .map_err(|e| prefix_field(e, &index.to_string()))?;
            }
            Ok(())
        }
        (ValueType::Map(element), Value::Map(entries)) => {
            length_within(bound, entries.len())?;
            for (key, item) in entries {
                check_value(element, None, item).map_err(|e| prefix_field(e, key))?;
            }
            Ok(())
        }
        (ValueType::Record(fields), Value::Record(values)) => check_values(fields, values),
        _ => Err(Error::new(codes::VALUE_TYPE_MISMATCH)),
    }
}

fn check_real(value: f64, bound: Option<&Bound>) -> Result<()> {
    finite(&[value])?;
    match bound {
        Some(Bound::Real { min, max }) => within(*min <= value && value <= *max),
        _ => Ok(()),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

impl Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Bool(v) => s.serialize_bool(*v),
            Self::I64(v) => s.serialize_i64(*v),
            Self::F64(v) => s.serialize_f64(*v),
            Self::String(v) => s.serialize_str(v),
            Self::Quantity(q) => s.serialize_f64(q.value()),
            Self::Enum(code) => s.serialize_u32(*code),
            Self::Point2(p) => [p.x, p.y].serialize(s),
            Self::Point3(p) => [p.x, p.y, p.z].serialize(s),
            Self::Vector3(v) => [v.x, v.y, v.z].serialize(s),
            Self::Transform(t) => transform_entries(t).serialize(s),
            Self::Color(c) => {
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry("rgba", &[c.rgba.r, c.rgba.g, c.rgba.b, c.rgba.a])?;
                map.serialize_entry("source", &c.source)?;
                map.end()
            }
            Self::Reference(id) => s.serialize_str(&id.to_string()),
            Self::ExternalReference(r) => {
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry("link", &r.link.to_string())?;
                map.serialize_entry("target", &r.target.to_string())?;
                map.end()
            }
            Self::Asset(hash) => s.serialize_str(&hex(&hash.0)),
            Self::Geometry(bytes) => s.serialize_str(&hex(&bytes.0)),
            Self::Opaque(bytes) => s.serialize_str(&hex(bytes)),
            Self::List(items) => items.serialize(s),
            Self::Map(entries) => entries.serialize(s),
            Self::Record(values) => values.serialize(s),
        }
    }
}
