//! The payload codec: the binary form of an object's field values (ADR-0005).
//!
//! A payload is the `postcard` encoding of the fields of one [`Schema`] in declared order; the
//! layout of every value type is specified in `docs/crates/arc-doc.md`. Encoding is canonical:
//! bit-identical values always give the same bytes, and decoding accepts only canonical bytes,
//! so a payload hash identifies a value.
//!
//! Decoding treats its input as hostile. It never panics, checks every length and the memory
//! budget against [`Limits`] before it allocates for them, and validates the result against the
//! declaration. Encoding applies the same limits, so that everything written can be read back.
//!
//! The schemas passed to this module are those of registered declarations (see
//! [`crate::TypeRegistry`]). The codec does not repeat every registration check, but it refuses
//! the declarations that would make it unsafe: empty records and types nested deeper than
//! [`MAX_TYPE_DEPTH`], failing with `doc.type.field_invalid`.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;
use std::mem::size_of;

use arc_base::id::Identifier;
use arc_base::math::{Point2, Point3, Transform3, Vector3};
use arc_base::{Color, Error, Limits, ObjectId, Quantity, Result};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{self, SerializeTuple, Serializer};
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::registry::MAX_TYPE_DEPTH;
use crate::schema::{
    AssetHash, ColorSource, ColorValue, ExternalRef, FieldDecl, GeometryBytes, Schema, Value,
    ValueType, check_value, check_values, transform_entries, transform_from_entries,
};

/// The most elements a decoder reserves memory for before it has decoded them.
const PREALLOCATE: usize = 1024;

/// How payload bytes are stored, recorded beside the payload (the `encoding` column of a
/// package).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// The `postcard` bytes as they are (code 0).
    Postcard,
    /// The `postcard` bytes compressed with `zstd` (code 1). This build neither writes nor reads
    /// it; decoding fails with `doc.payload.encoding_unsupported`.
    PostcardZstd,
}

impl Encoding {
    /// The stored code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Postcard => 0,
            Self::PostcardZstd => 1,
        }
    }

    /// The encoding of a stored code, or `None` for an unknown code.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Postcard),
            1 => Some(Self::PostcardZstd),
            _ => None,
        }
    }
}

/// An encoded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// How `bytes` are stored.
    pub encoding: Encoding,
    /// The stored bytes.
    pub bytes: Vec<u8>,
}

/// The result of decoding a payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// One entry per field of the schema, in declared order.
    pub values: Vec<Option<Value>>,
    /// The bytes after the known fields of a payload written by a newer minor version: the
    /// encoded fields this schema does not know, kept so that they can be written back
    /// unchanged. Empty for every other payload. These bytes are not validated: canonical form
    /// and hash identity cover the known fields only.
    pub trailing: Vec<u8>,
}

/// Encodes the field values of an object of `schema`.
///
/// The values are checked first (see [`check_values`]); failures carry the `doc.value.*` codes.
/// A payload that [`decode`] would reject under `limits` fails with
/// `doc.payload.limit_exceeded`.
pub fn encode(schema: &Schema, values: &[Option<Value>], limits: &Limits) -> Result<Payload> {
    check_values(&schema.fields, values)?;
    Ok(Payload {
        encoding: Encoding::Postcard,
        bytes: to_bytes(&schema.fields, values, limits)?,
    })
}

/// Decodes a payload that minor version `written_minor` of the major version of `schema` wrote.
///
/// - An older minor version holds a prefix of the fields; the fields appended since get their
///   default, or stay absent.
/// - A newer minor version holds the known fields followed by fields this schema does not know;
///   their bytes are returned in [`Decoded::trailing`]. The caller checks `written_minor`
///   against the versions the package declares.
///
/// Fails with `doc.payload.encoding_unsupported` for a compressed payload,
/// `doc.payload.limit_exceeded` when the payload, a string, a byte string, a collection, the
/// nesting depth or the memory budget exceeds `limits`, and `doc.payload.invalid` when the bytes
/// are malformed, not canonical, followed by unexpected bytes, or decode to values the
/// declaration rejects.
pub fn decode(
    schema: &Schema,
    written_minor: u32,
    encoding: Encoding,
    bytes: &[u8],
    limits: &Limits,
) -> Result<Decoded> {
    if encoding != Encoding::Postcard {
        return Err(Error::new(codes::PAYLOAD_ENCODING_UNSUPPORTED)
            .with_detail("encoding", i64::from(encoding.code())));
    }
    input_within(bytes.len(), limits)?;
    let known = schema.fields_at(written_minor);
    let context = Context::new(limits);
    let mut deserializer = postcard::Deserializer::from_bytes(bytes);
    let seed = FieldsSeed {
        fields: known,
        context: &context,
        depth: 0,
    };
    let mut values = seed
        .deserialize(&mut deserializer)
        .map_err(|cause| context.error(corrupt(cause)))?;
    let trailing = deserializer.finalize().map_err(corrupt)?;
    if written_minor <= schema.version.minor && !trailing.is_empty() {
        return Err(Error::new(codes::PAYLOAD_INVALID).with_detail("reason", "trailing bytes"));
    }
    check_values(known, &values).map_err(rejected)?;
    let consumed = bytes.get(..bytes.len().saturating_sub(trailing.len()));
    if consumed != Some(to_bytes(known, &values, limits)?.as_slice()) {
        return Err(not_canonical());
    }
    let appended = schema.fields.get(known.len()..).unwrap_or_default();
    values.extend(appended.iter().map(|field| field.default.clone()));
    Ok(Decoded {
        values,
        trailing: trailing.to_vec(),
    })
}

/// Encodes one value of `value_type` on its own, as it appears inside a payload: the form of
/// field-level before and after values in journal records (ADR-0003).
///
/// The value is checked against the type (without a field's bound, which the caller checks);
/// failures carry the `doc.value.*` codes, and a value [`decode_value`] would reject under
/// `limits` fails with `doc.payload.limit_exceeded`.
pub fn encode_value(value_type: &ValueType, value: &Value, limits: &Limits) -> Result<Vec<u8>> {
    check_value(value_type, None, value)?;
    let context = Context::new(limits);
    let typed = TypedSer {
        value_type,
        value,
        context: &context,
        depth: 0,
    };
    let bytes = postcard::to_allocvec(&typed).map_err(|cause| context.error(mismatch(cause)))?;
    input_within(bytes.len(), limits)?;
    Ok(bytes)
}

/// Decodes one value of `value_type` that [`encode_value`] wrote, with the checks and failures
/// of [`decode`]; every byte must belong to the value.
pub fn decode_value(value_type: &ValueType, bytes: &[u8], limits: &Limits) -> Result<Value> {
    input_within(bytes.len(), limits)?;
    let context = Context::new(limits);
    let mut deserializer = postcard::Deserializer::from_bytes(bytes);
    let seed = TypedSeed {
        value_type,
        context: &context,
        depth: 0,
    };
    let value = seed
        .deserialize(&mut deserializer)
        .map_err(|cause| context.error(corrupt(cause)))?;
    if !deserializer.finalize().map_err(corrupt)?.is_empty() {
        return Err(Error::new(codes::PAYLOAD_INVALID).with_detail("reason", "trailing bytes"));
    }
    check_value(value_type, None, &value).map_err(rejected)?;
    if encode_value(value_type, &value, limits)? != bytes {
        return Err(not_canonical());
    }
    Ok(value)
}

fn input_within(length: usize, limits: &Limits) -> Result<()> {
    if u64::try_from(length).map_or(true, |length| length > limits.input_bytes) {
        return Err(Error::new(codes::PAYLOAD_LIMIT_EXCEEDED).with_detail("limit", "input_bytes"));
    }
    Ok(())
}

fn corrupt(cause: postcard::Error) -> Error {
    Error::new(codes::PAYLOAD_INVALID).with_source(cause)
}

fn mismatch(cause: postcard::Error) -> Error {
    Error::new(codes::VALUE_TYPE_MISMATCH).with_source(cause)
}

fn not_canonical() -> Error {
    Error::new(codes::PAYLOAD_INVALID).with_detail("reason", "not canonical")
}

/// A decoded value the declaration rejects: `doc.payload.invalid` with the value error as its
/// source and its `field` argument.
fn rejected(cause: Error) -> Error {
    let field = cause.arguments().get("field").cloned();
    let error = Error::new(codes::PAYLOAD_INVALID).with_source(cause);
    match field {
        Some(field) => error.with_arg("field", field),
        None => error,
    }
}

/// Encodes checked values.
fn to_bytes(fields: &[FieldDecl], values: &[Option<Value>], limits: &Limits) -> Result<Vec<u8>> {
    let context = Context::new(limits);
    let tuple = FieldsSer {
        fields,
        values,
        context: &context,
        depth: 0,
    };
    let bytes = postcard::to_allocvec(&tuple).map_err(|cause| context.error(mismatch(cause)))?;
    input_within(bytes.len(), limits)?;
    Ok(bytes)
}

fn id_bytes(id: ObjectId) -> [u8; 16] {
    id.as_u128().to_be_bytes()
}

fn id_from(bytes: [u8; 16]) -> ObjectId {
    ObjectId::from_u128(u128::from_be_bytes(bytes))
}

fn limit(name: &'static str) -> Error {
    Error::new(codes::PAYLOAD_LIMIT_EXCEEDED).with_detail("limit", name)
}

fn bad_type(reason: &'static str) -> Error {
    Error::new(codes::TYPE_FIELD_INVALID).with_detail("reason", reason)
}

/// The state shared by one encode or decode: the limits, the memory charged so far and the
/// first Arc error found, which replaces the generic `postcard` error it travels inside.
///
/// Encoding and decoding make the same checks at the same places of a value, so a value encodes
/// under some limits exactly when its encoding decodes under them.
struct Context<'a> {
    limits: &'a Limits,
    used: Cell<u64>,
    failure: Cell<Option<Error>>,
}

impl<'a> Context<'a> {
    fn new(limits: &'a Limits) -> Self {
        Self {
            limits,
            used: Cell::new(0),
            failure: Cell::new(None),
        }
    }

    /// The Arc error behind a failed `postcard` call, or `fallback` when there is none.
    fn error(&self, fallback: Error) -> Error {
        self.failure.take().unwrap_or(fallback)
    }

    fn keep(&self, error: Error) {
        let first = self.failure.take().unwrap_or(error);
        self.failure.set(Some(first));
    }

    /// Converts a check into a deserializer error, keeping the Arc error.
    fn de<E: de::Error>(&self, checked: Result<()>) -> std::result::Result<(), E> {
        checked.map_err(|error| {
            self.keep(error);
            E::custom("payload rejected")
        })
    }

    /// Converts a check into a serializer error, keeping the Arc error.
    fn ser<E: ser::Error>(&self, checked: Result<()>) -> std::result::Result<(), E> {
        checked.map_err(|error| {
            self.keep(error);
            E::custom("payload rejected")
        })
    }

    fn within(length: usize, max: u64, name: &'static str) -> Result<()> {
        if u64::try_from(length).is_ok_and(|length| length <= max) {
            Ok(())
        } else {
            Err(limit(name))
        }
    }

    /// Charges `bytes` against the memory budget, when the limits set one.
    fn charge(&self, bytes: usize) -> Result<()> {
        let Some(budget) = self.limits.memory_bytes else {
            return Ok(());
        };
        let used =
            u64::try_from(bytes).map_or(u64::MAX, |bytes| self.used.get().saturating_add(bytes));
        if used > budget {
            return Err(limit("memory_bytes"));
        }
        self.used.set(used);
        Ok(())
    }

    /// Charges one value slot plus the boxed or owned data of `value_type` that is not a string
    /// or byte string, which [`Self::text`] and [`Self::bytes`] charge.
    fn value(&self, value_type: &ValueType) -> Result<()> {
        let boxed = match value_type {
            ValueType::Transform => size_of::<Transform3>(),
            ValueType::Color => size_of::<ColorValue>(),
            ValueType::ExternalReference => size_of::<ExternalRef>(),
            ValueType::Asset => size_of::<AssetHash>(),
            _ => 0,
        };
        self.charge(size_of::<Value>().saturating_add(boxed))
    }

    /// Charges an absent optional field.
    fn absent(&self) -> Result<()> {
        self.charge(size_of::<Value>())
    }

    fn text(&self, length: usize) -> Result<()> {
        Self::within(length, self.limits.string_bytes, "string_bytes")?;
        self.charge(length)
    }

    fn bytes(&self, length: usize) -> Result<()> {
        Self::within(length, self.limits.array_length, "array_length")?;
        self.charge(length)
    }

    fn entries(&self, length: usize) -> Result<()> {
        Self::within(length, self.limits.array_length, "array_length")
    }

    /// The depth of the elements of a collection or the fields of a record at `depth`.
    fn nested(&self, depth: u32) -> Result<u32> {
        let depth = depth.saturating_add(1);
        if depth > MAX_TYPE_DEPTH {
            return Err(bad_type("nested too deeply"));
        }
        if depth > self.limits.nesting_depth {
            return Err(limit("nesting_depth"));
        }
        Ok(depth)
    }
}

fn record_fields(fields: &[FieldDecl]) -> Result<()> {
    if fields.is_empty() {
        Err(bad_type("a record needs a field"))
    } else {
        Ok(())
    }
}

/// The fields of a schema or record as a `postcard` tuple: an optional field as an option, a
/// required one as its value.
struct FieldsSer<'a> {
    fields: &'a [FieldDecl],
    values: &'a [Option<Value>],
    context: &'a Context<'a>,
    depth: u32,
}

impl Serialize for FieldsSer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if self.fields.len() != self.values.len() {
            return Err(ser::Error::custom("wrong number of field values"));
        }
        let mut tuple = serializer.serialize_tuple(self.fields.len())?;
        for (field, value) in self.fields.iter().zip(self.values) {
            let typed = value.as_ref().map(|value| TypedSer {
                value_type: &field.value_type,
                value,
                context: self.context,
                depth: self.depth,
            });
            match (field.optional, typed) {
                (true, None) => {
                    self.context.ser(self.context.absent())?;
                    tuple.serialize_element(&None::<TypedSer<'_>>)?;
                }
                (true, typed) => tuple.serialize_element(&typed)?,
                (false, Some(typed)) => tuple.serialize_element(&typed)?,
                (false, None) => return Err(ser::Error::custom("missing required field")),
            }
        }
        tuple.end()
    }
}

/// One value; the type supplies the element types of collections and the fields of records.
struct TypedSer<'a> {
    value_type: &'a ValueType,
    value: &'a Value,
    context: &'a Context<'a>,
    depth: u32,
}

impl Serialize for TypedSer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let context = self.context;
        context.ser(context.value(self.value_type))?;
        match (self.value_type, self.value) {
            (_, Value::Bool(v)) => serializer.serialize_bool(*v),
            (_, Value::I64(v)) => serializer.serialize_i64(*v),
            (_, Value::F64(v)) => serializer.serialize_f64(*v),
            (_, Value::String(v)) => {
                context.ser(context.text(v.len()))?;
                serializer.serialize_str(v)
            }
            (_, Value::Quantity(q)) => serializer.serialize_f64(q.value()),
            (_, Value::Enum(code)) => serializer.serialize_u32(*code),
            (_, Value::Point2(p)) => [p.x, p.y].serialize(serializer),
            (_, Value::Point3(p)) => [p.x, p.y, p.z].serialize(serializer),
            (_, Value::Vector3(v)) => [v.x, v.y, v.z].serialize(serializer),
            (_, Value::Transform(t)) => transform_entries(t).serialize(serializer),
            (_, Value::Color(c)) => {
                if let Some(ColorSource::Named(name)) = &c.source {
                    context.ser(context.text(name.len()))?;
                }
                (c.rgba.r, c.rgba.g, c.rgba.b, c.rgba.a, &c.source).serialize(serializer)
            }
            (_, Value::Reference(id)) => id_bytes(*id).serialize(serializer),
            (_, Value::ExternalReference(r)) => {
                (id_bytes(r.link), id_bytes(r.target)).serialize(serializer)
            }
            (_, Value::Asset(hash)) => hash.0.serialize(serializer),
            (_, Value::Geometry(bytes)) => {
                context.ser(context.bytes(bytes.0.len()))?;
                serializer.serialize_bytes(&bytes.0)
            }
            (_, Value::Opaque(bytes)) => {
                context.ser(context.bytes(bytes.len()))?;
                serializer.serialize_bytes(bytes)
            }
            (ValueType::List(element), Value::List(items)) => {
                let depth = nested_ser::<S>(context, self.depth)?;
                context.ser(context.entries(items.len()))?;
                serializer.collect_seq(items.iter().map(|value| TypedSer {
                    value_type: element,
                    value,
                    context,
                    depth,
                }))
            }
            (ValueType::Map(element), Value::Map(entries)) => {
                let depth = nested_ser::<S>(context, self.depth)?;
                context.ser(context.entries(entries.len()))?;
                for key in entries.keys() {
                    context.ser(context.text(key.len()))?;
                }
                serializer.collect_map(entries.iter().map(|(key, value)| {
                    let typed = TypedSer {
                        value_type: element,
                        value,
                        context,
                        depth,
                    };
                    (key, typed)
                }))
            }
            (ValueType::Record(fields), Value::Record(values)) => {
                let depth = nested_ser::<S>(context, self.depth)?;
                context.ser(record_fields(fields))?;
                FieldsSer {
                    fields,
                    values,
                    context,
                    depth,
                }
                .serialize(serializer)
            }
            _ => Err(ser::Error::custom("value does not match its type")),
        }
    }
}

fn nested_ser<S: Serializer>(
    context: &Context<'_>,
    depth: u32,
) -> std::result::Result<u32, S::Error> {
    context.nested(depth).map_err(|error| {
        context.keep(error);
        ser::Error::custom("payload rejected")
    })
}

fn nested_de<'de, D: Deserializer<'de>>(
    context: &Context<'_>,
    depth: u32,
) -> std::result::Result<u32, D::Error> {
    context.nested(depth).map_err(|error| {
        context.keep(error);
        de::Error::custom("payload rejected")
    })
}

/// Decodes the fields of a schema or record.
#[derive(Clone, Copy)]
struct FieldsSeed<'a> {
    fields: &'a [FieldDecl],
    context: &'a Context<'a>,
    depth: u32,
}

impl<'de> DeserializeSeed<'de> for FieldsSeed<'_> {
    type Value = Vec<Option<Value>>;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_tuple(self.fields.len(), self)
    }
}

impl<'de> Visitor<'de> for FieldsSeed<'_> {
    type Value = Vec<Option<Value>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the fields of a record")
    }

    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut values = Vec::with_capacity(self.fields.len());
        for field in self.fields {
            let seed = TypedSeed {
                value_type: &field.value_type,
                context: self.context,
                depth: self.depth,
            };
            let value = if field.optional {
                seq.next_element_seed(OptionSeed(seed))?
            } else {
                seq.next_element_seed(seed)?.map(Some)
            };
            match value {
                Some(value) => values.push(value),
                None => return Err(de::Error::invalid_length(values.len(), &self)),
            }
        }
        Ok(values)
    }
}

/// Decodes an optional field.
struct OptionSeed<'a>(TypedSeed<'a>);

impl<'de> DeserializeSeed<'de> for OptionSeed<'_> {
    type Value = Option<Value>;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        deserializer.deserialize_option(self)
    }
}

impl<'de> Visitor<'de> for OptionSeed<'_> {
    type Value = Option<Value>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an optional field")
    }

    fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
        let context = self.0.context;
        context.de(context.absent())?;
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        self.0.deserialize(deserializer).map(Some)
    }
}

/// The source of a color as stored, with a borrowed name so that its length is checked before it
/// is copied. The variants are in the order of [`ColorSource`].
#[derive(Deserialize)]
enum SourceRef<'a> {
    Indexed(u8),
    Named(&'a str),
}

/// Decodes one value of a known type.
#[derive(Clone, Copy)]
struct TypedSeed<'a> {
    value_type: &'a ValueType,
    context: &'a Context<'a>,
    depth: u32,
}

impl<'de> DeserializeSeed<'de> for TypedSeed<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        let context = self.context;
        context.de(context.value(self.value_type))?;
        Ok(match self.value_type {
            ValueType::Bool => Value::Bool(bool::deserialize(deserializer)?),
            ValueType::I64 => Value::I64(i64::deserialize(deserializer)?),
            ValueType::F64 => Value::F64(f64::deserialize(deserializer)?),
            ValueType::String => {
                let text = <&str>::deserialize(deserializer)?;
                context.de(context.text(text.len()))?;
                Value::String(text.to_owned())
            }
            ValueType::Quantity(dimension) => {
                Value::Quantity(Quantity::new(f64::deserialize(deserializer)?, *dimension))
            }
            ValueType::Enum(_) => Value::Enum(u32::deserialize(deserializer)?),
            ValueType::Point2 => {
                let [x, y] = <[f64; 2]>::deserialize(deserializer)?;
                Value::Point2(Point2::new(x, y))
            }
            ValueType::Point3 => {
                let [x, y, z] = <[f64; 3]>::deserialize(deserializer)?;
                Value::Point3(Point3::new(x, y, z))
            }
            ValueType::Vector3 => {
                let [x, y, z] = <[f64; 3]>::deserialize(deserializer)?;
                Value::Vector3(Vector3::new(x, y, z))
            }
            ValueType::Transform => Value::Transform(Box::new(transform_from_entries(
                &<[f64; 12]>::deserialize(deserializer)?,
            ))),
            ValueType::Color => {
                let (r, g, b, a, source) =
                    <(f32, f32, f32, f32, Option<SourceRef<'de>>)>::deserialize(deserializer)?;
                let source = match source {
                    None => None,
                    Some(SourceRef::Indexed(index)) => Some(ColorSource::Indexed(index)),
                    Some(SourceRef::Named(name)) => {
                        context.de(context.text(name.len()))?;
                        Some(ColorSource::Named(name.to_owned()))
                    }
                };
                Value::Color(Box::new(ColorValue {
                    rgba: Color::new(r, g, b, a),
                    source,
                }))
            }
            ValueType::Reference(_) => {
                Value::Reference(id_from(<[u8; 16]>::deserialize(deserializer)?))
            }
            ValueType::ExternalReference => {
                let (link, target) = <([u8; 16], [u8; 16])>::deserialize(deserializer)?;
                Value::ExternalReference(Box::new(ExternalRef {
                    link: id_from(link),
                    target: id_from(target),
                }))
            }
            ValueType::Asset => {
                Value::Asset(Box::new(AssetHash(<[u8; 32]>::deserialize(deserializer)?)))
            }
            ValueType::Geometry(_) => {
                let bytes = <&[u8]>::deserialize(deserializer)?;
                context.de(context.bytes(bytes.len()))?;
                Value::Geometry(GeometryBytes(bytes.to_vec()))
            }
            ValueType::Opaque => {
                let bytes = <&[u8]>::deserialize(deserializer)?;
                context.de(context.bytes(bytes.len()))?;
                Value::Opaque(bytes.to_vec())
            }
            ValueType::List(element) => {
                let seed = Self {
                    value_type: element,
                    context,
                    depth: nested_de::<D>(context, self.depth)?,
                };
                deserializer.deserialize_seq(ListVisitor(seed))?
            }
            ValueType::Map(element) => {
                let seed = Self {
                    value_type: element,
                    context,
                    depth: nested_de::<D>(context, self.depth)?,
                };
                deserializer.deserialize_map(MapVisitor(seed))?
            }
            ValueType::Record(fields) => {
                let depth = nested_de::<D>(context, self.depth)?;
                context.de(record_fields(fields))?;
                let seed = FieldsSeed {
                    fields,
                    context,
                    depth,
                };
                Value::Record(seed.deserialize(deserializer)?)
            }
        })
    }
}

/// Decodes the elements of a list with the element seed.
struct ListVisitor<'a>(TypedSeed<'a>);

impl<'de> Visitor<'de> for ListVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let seed = self.0;
        let context = seed.context;
        let hint = seq.size_hint().unwrap_or(0);
        context.de(context.entries(hint))?;
        let mut items = Vec::with_capacity(hint.min(PREALLOCATE));
        while let Some(item) = seq.next_element_seed(seed)? {
            items.push(item);
            context.de(context.entries(items.len()))?;
        }
        Ok(Value::List(items))
    }
}

/// Decodes the entries of a map with the value seed.
struct MapVisitor<'a>(TypedSeed<'a>);

impl<'de> Visitor<'de> for MapVisitor<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let seed = self.0;
        let context = seed.context;
        let hint = map.size_hint().unwrap_or(0);
        context.de(context.entries(hint))?;
        let mut entries = BTreeMap::new();
        while let Some(key) = map.next_key::<&str>()? {
            context.de(context.text(key.len()))?;
            let value = map.next_value_seed(seed)?;
            // A repeated key shrinks the map, which the canonical check rejects.
            entries.insert(key.to_owned(), value);
            context.de(context.entries(entries.len()))?;
        }
        Ok(Value::Map(entries))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::schema::{Bound, SchemaVersion};
    use crate::testing::{nested_type, object, value};

    fn fields_v1(fields: Vec<FieldDecl>) -> Schema {
        Schema::new(SchemaVersion::new(1, 0), fields)
    }

    fn decode_all(schema: &Schema, bytes: &[u8]) -> Result<Decoded> {
        decode(schema, 0, Encoding::Postcard, bytes, &Limits::DEFAULT)
    }

    fn code(result: Result<Decoded>) -> &'static str {
        result.map(|_| "ok").unwrap_or_else(|error| error.code())
    }

    fn list_of_strings() -> Schema {
        fields_v1(vec![FieldDecl::new(
            "names",
            ValueType::List(Box::new(ValueType::String)),
        )])
    }

    proptest! {
        #[test]
        fn decode_inverts_encode_and_bytes_are_canonical((fields, values) in object()) {
            let schema = fields_v1(fields);
            let payload = encode(&schema, &values, &Limits::DEFAULT).unwrap();
            prop_assert_eq!(payload.encoding, Encoding::Postcard);
            let decoded = decode_all(&schema, &payload.bytes).unwrap();
            prop_assert_eq!(&decoded.values, &values);
            prop_assert!(decoded.trailing.is_empty());
            prop_assert_eq!(encode(&schema, &decoded.values, &Limits::DEFAULT).unwrap().bytes, payload.bytes);
        }

        #[test]
        fn arbitrary_bytes_never_panic_and_accepted_bytes_are_canonical(
            (fields, _) in object(),
            bytes in prop::collection::vec(any::<u8>(), 0..64),
        ) {
            let schema = fields_v1(fields);
            if let Ok(decoded) = decode_all(&schema, &bytes) {
                prop_assert_eq!(encode(&schema, &decoded.values, &Limits::DEFAULT).unwrap().bytes, bytes);
            }
        }

        #[test]
        fn mutated_encodings_never_panic_and_accepted_bytes_are_canonical(
            (fields, values) in object(),
            position in any::<prop::sample::Index>(),
            byte in any::<u8>(),
            kind in 0..3_u8,
        ) {
            let schema = fields_v1(fields);
            let mut bytes = encode(&schema, &values, &Limits::DEFAULT).unwrap().bytes;
            let at = position.index(bytes.len() + 1);
            match kind {
                0 if at < bytes.len() => bytes[at] = byte,
                1 => bytes.truncate(at),
                _ => bytes.insert(at, byte),
            }
            if let Ok(decoded) = decode_all(&schema, &bytes) {
                prop_assert_eq!(encode(&schema, &decoded.values, &Limits::DEFAULT).unwrap().bytes, bytes);
            }
        }

        #[test]
        fn tight_limits_reject_with_a_limit_error_and_never_panic(
            value_type in nested_type(),
            depth in 0..3_u32,
            length in 0..3_u64,
        ) {
            let schema = fields_v1(vec![FieldDecl::new("v", value_type.clone())]);
            let limits = Limits {
                nesting_depth: depth,
                string_bytes: length,
                array_length: length,
                ..Limits::DEFAULT
            };
            let mut runner = proptest::test_runner::TestRunner::deterministic();
            let sample = value(&value_type).new_tree(&mut runner).unwrap().current();
            let bytes = encode(&schema, &[Some(sample)], &Limits::DEFAULT).unwrap().bytes;
            let result = decode(&schema, 0, Encoding::Postcard, &bytes, &limits);
            if let Err(error) = result {
                prop_assert_eq!(error.code(), "doc.payload.limit_exceeded");
            }
        }
    }

    #[test]
    fn the_layout_is_the_documented_postcard_form() {
        let schema = fields_v1(vec![
            FieldDecl::new("flag", ValueType::Bool),
            FieldDecl::new("count", ValueType::I64).optional(),
            FieldDecl::new("name", ValueType::String),
        ]);
        let values = [
            Some(Value::Bool(true)),
            Some(Value::I64(-2)),
            Some(Value::String("ab".to_owned())),
        ];
        let bytes = encode(&schema, &values, &Limits::DEFAULT).unwrap().bytes;
        assert_eq!(bytes, [1, 1, 3, 2, b'a', b'b']);
        let absent = [values[0].clone(), None, values[2].clone()];
        assert_eq!(
            encode(&schema, &absent, &Limits::DEFAULT).unwrap().bytes,
            [1, 0, 2, b'a', b'b']
        );
    }

    #[test]
    fn encode_checks_values_first() {
        let schema = fields_v1(vec![
            FieldDecl::new("n", ValueType::I64).with_bound(Bound::Int { min: 0, max: 9 }),
        ]);
        let encoded = |value| {
            encode(&schema, &[value], &Limits::DEFAULT)
                .map(|_| ())
                .unwrap_err()
                .code()
        };
        assert_eq!(encoded(None), "doc.value.missing");
        assert_eq!(encoded(Some(Value::Bool(true))), "doc.value.type_mismatch");
        assert_eq!(encoded(Some(Value::I64(10))), "doc.value.out_of_range");
        let reals = fields_v1(vec![FieldDecl::new("x", ValueType::F64)]);
        let error = encode(&reals, &[Some(Value::F64(f64::NAN))], &Limits::DEFAULT).unwrap_err();
        assert_eq!(error.code(), "doc.value.not_finite");
        assert_eq!(
            error.arguments().get("field"),
            Some(&arc_base::error::Value::Text("x".to_owned()))
        );
    }

    #[test]
    fn decoded_values_outside_the_declaration_are_invalid() {
        let bounded = fields_v1(vec![
            FieldDecl::new("n", ValueType::I64).with_bound(Bound::Int { min: 0, max: 9 }),
        ]);
        // 20 is zigzag 10.
        assert_eq!(code(decode_all(&bounded, &[20])), "doc.payload.invalid");
        let reals = fields_v1(vec![FieldDecl::new("x", ValueType::F64)]);
        let nan = f64::NAN.to_le_bytes();
        assert_eq!(code(decode_all(&reals, &nan)), "doc.payload.invalid");
    }

    #[test]
    fn non_canonical_and_trailing_bytes_are_invalid() {
        let numbers = fields_v1(vec![FieldDecl::new("n", ValueType::I64)]);
        assert_eq!(code(decode_all(&numbers, &[2])), "ok");
        // An overlong varint for 1.
        assert_eq!(
            code(decode_all(&numbers, &[0x82, 0x00])),
            "doc.payload.invalid"
        );
        assert_eq!(code(decode_all(&numbers, &[2, 0])), "doc.payload.invalid");
        // A map with its keys out of order.
        let map = fields_v1(vec![FieldDecl::new(
            "m",
            ValueType::Map(Box::new(ValueType::Bool)),
        )]);
        let sorted = [2, 1, b'a', 1, 1, b'b', 0];
        assert_eq!(code(decode_all(&map, &sorted)), "ok");
        let unsorted = [2, 1, b'b', 0, 1, b'a', 1];
        assert_eq!(code(decode_all(&map, &unsorted)), "doc.payload.invalid");
        let repeated = [2, 1, b'a', 1, 1, b'a', 1];
        assert_eq!(code(decode_all(&map, &repeated)), "doc.payload.invalid");
    }

    #[test]
    fn hostile_lengths_fail_without_allocating() {
        let strings = list_of_strings();
        // A list that claims 2^63 elements, and one string that claims 2^63 bytes: both claim
        // more than the input holds, so nothing is reserved and decoding fails at the end.
        let huge = [0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        assert_eq!(code(decode_all(&strings, &huge)), "doc.payload.invalid");
        let mut one = vec![1];
        one.extend_from_slice(&huge);
        assert_eq!(code(decode_all(&strings, &one)), "doc.payload.invalid");
    }

    #[test]
    fn limits_bound_lengths_depth_and_input_size() {
        let strings = list_of_strings();
        let values = [Some(Value::List(vec![Value::String("abcd".to_owned()); 3]))];
        let bytes = encode(&strings, &values, &Limits::DEFAULT).unwrap().bytes;
        let decode_with =
            |limits: Limits| code(decode(&strings, 0, Encoding::Postcard, &bytes, &limits));
        assert_eq!(decode_with(Limits::DEFAULT), "ok");
        let short = Limits {
            string_bytes: 3,
            ..Limits::DEFAULT
        };
        assert_eq!(decode_with(short), "doc.payload.limit_exceeded");
        let few = Limits {
            array_length: 2,
            ..Limits::DEFAULT
        };
        assert_eq!(decode_with(few), "doc.payload.limit_exceeded");
        let flat = Limits {
            nesting_depth: 0,
            ..Limits::DEFAULT
        };
        assert_eq!(decode_with(flat), "doc.payload.limit_exceeded");
        let small = Limits {
            input_bytes: 4,
            ..Limits::DEFAULT
        };
        assert_eq!(decode_with(small), "doc.payload.limit_exceeded");
    }

    #[test]
    fn minor_versions_fill_appended_fields_and_keep_unknown_bytes() {
        let v1_0 = vec![FieldDecl::new("a", ValueType::I64)];
        let mut v1_1 = v1_0.clone();
        v1_1.push(
            FieldDecl::new("b", ValueType::Bool)
                .since_minor(1)
                .with_default(Value::Bool(true)),
        );
        let old = Schema::new(SchemaVersion::new(1, 0), v1_0);
        let new = Schema::new(SchemaVersion::new(1, 1), v1_1);

        let old_bytes = encode(&old, &[Some(Value::I64(7))], &Limits::DEFAULT)
            .unwrap()
            .bytes;
        let read_by_new =
            decode(&new, 0, Encoding::Postcard, &old_bytes, &Limits::DEFAULT).unwrap();
        assert_eq!(
            read_by_new.values,
            [Some(Value::I64(7)), Some(Value::Bool(true))]
        );

        let new_bytes = encode(
            &new,
            &[Some(Value::I64(7)), Some(Value::Bool(false))],
            &Limits::DEFAULT,
        )
        .unwrap()
        .bytes;
        let read_by_old =
            decode(&old, 1, Encoding::Postcard, &new_bytes, &Limits::DEFAULT).unwrap();
        assert_eq!(read_by_old.values, [Some(Value::I64(7))]);
        assert_eq!(read_by_old.trailing, [0]);
        let mut written_back = encode(&old, &read_by_old.values, &Limits::DEFAULT)
            .unwrap()
            .bytes;
        written_back.extend_from_slice(&read_by_old.trailing);
        assert_eq!(written_back, new_bytes);
    }

    #[test]
    fn compressed_payloads_are_unsupported() {
        let result = decode(
            &list_of_strings(),
            0,
            Encoding::PostcardZstd,
            &[0],
            &Limits::DEFAULT,
        );
        assert_eq!(code(result), "doc.payload.encoding_unsupported");
        assert_eq!(Encoding::from_code(1), Some(Encoding::PostcardZstd));
        assert_eq!(
            Encoding::from_code(Encoding::Postcard.code()),
            Some(Encoding::Postcard)
        );
        assert_eq!(Encoding::from_code(2), None);
    }

    fn limits_from(depth: u32, length: u64, memory: Option<u64>) -> Limits {
        Limits {
            nesting_depth: depth,
            string_bytes: length,
            array_length: length,
            memory_bytes: memory,
            ..Limits::DEFAULT
        }
    }

    proptest! {
        #[test]
        fn encode_fails_under_limits_exactly_when_decode_does(
            (fields, values) in object(),
            depth in 0..4_u32,
            length in 0..4_u64,
            memory in prop::option::of(0..2_000_u64),
        ) {
            let schema = fields_v1(fields);
            let limits = limits_from(depth, length, memory);
            let bytes = encode(&schema, &values, &Limits::DEFAULT).unwrap().bytes;
            let decoded = decode(&schema, 0, Encoding::Postcard, &bytes, &limits).map(|_| ());
            let encoded = encode(&schema, &values, &limits).map(|_| ());
            prop_assert_eq!(
                decoded.map_err(|error| error.code()),
                encoded.map_err(|error| error.code())
            );
        }

        #[test]
        fn single_values_round_trip_canonically(
            (value_type, sample) in nested_type()
                .prop_flat_map(|value_type| (Just(value_type.clone()), value(&value_type))),
            extra in any::<u8>(),
        ) {
            let bytes = encode_value(&value_type, &sample, &Limits::DEFAULT).unwrap();
            let decoded = decode_value(&value_type, &bytes, &Limits::DEFAULT).unwrap();
            prop_assert_eq!(&decoded, &sample);
            let mut longer = bytes.clone();
            longer.push(extra);
            let code = decode_value(&value_type, &longer, &Limits::DEFAULT)
                .map(|_| "ok")
                .unwrap_or_else(|error| error.code());
            prop_assert_eq!(code, "doc.payload.invalid");
        }
    }

    #[test]
    fn values_are_small_and_memory_is_budgeted() {
        assert_eq!(std::mem::size_of::<Value>(), 32);
        assert_eq!(std::mem::size_of::<Option<Value>>(), 32);
        // 100,000 empty lists: one byte each, one 32-byte value each.
        let lists = fields_v1(vec![FieldDecl::new(
            "l",
            ValueType::List(Box::new(ValueType::List(Box::new(ValueType::Bool)))),
        )]);
        let mut bytes = vec![0xa0, 0x8d, 0x06];
        bytes.resize(bytes.len() + 100_000, 0);
        let unbudgeted = limits_from(64, u64::MAX, None);
        assert_eq!(
            code(decode(&lists, 0, Encoding::Postcard, &bytes, &unbudgeted)),
            "ok"
        );
        let budget = limits_from(64, u64::MAX, Some(1 << 20));
        assert_eq!(
            code(decode(&lists, 0, Encoding::Postcard, &bytes, &budget)),
            "doc.payload.limit_exceeded"
        );
        let values = decode(&lists, 0, Encoding::Postcard, &bytes, &unbudgeted)
            .unwrap()
            .values;
        let encoded = encode(&lists, &values, &budget).unwrap_err();
        assert_eq!(encoded.code(), "doc.payload.limit_exceeded");
        // Strings and byte strings count with their length.
        let text = fields_v1(vec![FieldDecl::new("s", ValueType::String)]);
        let long = [Some(Value::String("x".repeat(2_000)))];
        let small = limits_from(64, u64::MAX, Some(1_000));
        assert_eq!(
            encode(&text, &long, &small).unwrap_err().code(),
            "doc.payload.limit_exceeded"
        );
    }

    #[test]
    fn schemas_registration_rejects_are_refused() {
        // A list of empty records would decode millions of values from three bytes.
        let empty = fields_v1(vec![FieldDecl::new(
            "l",
            ValueType::List(Box::new(ValueType::Record(Vec::new()))),
        )]);
        assert_eq!(
            code(decode_all(&empty, &[0x80, 0x89, 0x7a])),
            "doc.type.field_invalid"
        );
        let values = [Some(Value::List(vec![Value::Record(Vec::new())]))];
        assert_eq!(
            encode(&empty, &values, &Limits::DEFAULT)
                .unwrap_err()
                .code(),
            "doc.type.field_invalid"
        );
        let mut deep = ValueType::Bool;
        for _ in 0..=MAX_TYPE_DEPTH {
            deep = ValueType::List(Box::new(deep));
        }
        let deep = fields_v1(vec![FieldDecl::new("d", deep)]);
        let mut nested = vec![1; 17];
        nested.push(0);
        assert_eq!(code(decode_all(&deep, &nested)), "doc.type.field_invalid");
    }

    #[test]
    fn singular_transforms_are_rejected() {
        let schema = fields_v1(vec![FieldDecl::new("t", ValueType::Transform)]);
        assert_eq!(code(decode_all(&schema, &[0; 96])), "doc.payload.invalid");
        let mut entries = [0.0; 12];
        entries[0] = 1.0;
        entries[4] = 1.0;
        let flat = Value::Transform(Box::new(transform_from_entries(&entries)));
        let error = encode(&schema, &[Some(flat)], &Limits::DEFAULT).unwrap_err();
        assert_eq!(error.code(), "doc.value.out_of_range");
        // A uniformly tiny scale is invertible: the check is relative to the largest entry.
        entries[8] = 1.0;
        let tiny = entries.map(|entry| entry * 1e-200);
        let scaled = Value::Transform(Box::new(transform_from_entries(&tiny)));
        let bytes = encode(&schema, &[Some(scaled)], &Limits::DEFAULT)
            .unwrap()
            .bytes;
        assert_eq!(code(decode_all(&schema, &bytes)), "ok");
    }

    #[test]
    fn color_names_are_bounded_and_zero_signs_kept() {
        let schema = fields_v1(vec![FieldDecl::new("c", ValueType::Color)]);
        let named = Value::Color(Box::new(ColorValue {
            rgba: Color::new(0.0, 0.0, 0.0, 1.0),
            source: Some(ColorSource::Named("abcd".to_owned())),
        }));
        let bytes = encode(&schema, &[Some(named)], &Limits::DEFAULT)
            .unwrap()
            .bytes;
        let short = limits_from(64, 3, None);
        assert_eq!(
            code(decode(&schema, 0, Encoding::Postcard, &bytes, &short)),
            "doc.payload.limit_exceeded"
        );
        let reals = fields_v1(vec![FieldDecl::new("x", ValueType::F64)]);
        let negative = encode(&reals, &[Some(Value::F64(-0.0))], &Limits::DEFAULT)
            .unwrap()
            .bytes;
        let decoded = decode_all(&reals, &negative).unwrap().values;
        assert!(matches!(decoded[0], Some(Value::F64(x)) if x.to_bits() == (-0.0_f64).to_bits()));
    }

    #[test]
    fn a_wrong_value_count_names_a_field() {
        let schema = fields_v1(vec![
            FieldDecl::new("a", ValueType::Bool),
            FieldDecl::new("b", ValueType::Bool),
        ]);
        let field = |values: &[Option<Value>]| {
            let error = encode(&schema, values, &Limits::DEFAULT).unwrap_err();
            assert_eq!(error.code(), "doc.value.type_mismatch");
            error.arguments().get("field").cloned()
        };
        let text = |s: &str| Some(arc_base::error::Value::Text(s.to_owned()));
        assert_eq!(field(&[Some(Value::Bool(true))]), text("b"));
        assert_eq!(field(&vec![Some(Value::Bool(true)); 3]), text("2"));
    }
}
