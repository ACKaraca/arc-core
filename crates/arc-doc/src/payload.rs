//! The payload codec: the binary form of an object's field values (ADR-0005).
//!
//! A payload is the `postcard` encoding of the fields of one [`Schema`] in declared order; the
//! layout of every value type is specified in `docs/crates/arc-doc.md`. Encoding is canonical:
//! the same values always give the same bytes, and decoding accepts only canonical bytes, so a
//! payload hash identifies a value.
//!
//! Decoding treats its input as hostile. It never panics, checks every length against
//! [`Limits`] before it allocates for it, and validates the result against the declaration.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;

use arc_base::id::Identifier;
use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Color, Error, Limits, ObjectId, Quantity, Result};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{self, SerializeTuple, Serializer};
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::schema::{
    AssetHash, ColorSource, ColorValue, ExternalRef, FieldDecl, GeometryBytes, Schema, Value,
    ValueType, check_values, transform_entries, transform_from_entries,
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
    /// unchanged. Empty for every other payload.
    pub trailing: Vec<u8>,
}

/// Encodes the field values of an object of `schema`.
///
/// The values are checked first (see [`check_values`]); failures carry the `doc.value.*` codes.
pub fn encode(schema: &Schema, values: &[Option<Value>]) -> Result<Payload> {
    check_values(&schema.fields, values)?;
    Ok(Payload {
        encoding: Encoding::Postcard,
        bytes: to_bytes(&schema.fields, values)?,
    })
}

/// Decodes a payload that minor version `written_minor` of the major version of `schema` wrote.
///
/// - An older minor version holds a prefix of the fields; the fields appended since get their
///   default, or stay absent.
/// - A newer minor version holds the known fields followed by fields this schema does not know;
///   their bytes are returned in [`Decoded::trailing`].
///
/// Fails with `doc.payload.encoding_unsupported` for a compressed payload,
/// `doc.payload.limit_exceeded` when the payload, a string, a byte string, a collection or the
/// nesting depth exceeds `limits`, and `doc.payload.invalid` when the bytes are malformed, not
/// canonical, followed by unexpected bytes, or decode to values the declaration rejects.
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
    if u64::try_from(bytes.len()).map_or(true, |length| length > limits.input_bytes) {
        return Err(Error::new(codes::PAYLOAD_LIMIT_EXCEEDED).with_detail("limit", "input_bytes"));
    }
    let known = schema.fields_at(written_minor);
    let context = Context {
        limits,
        failure: Cell::new(None),
    };
    let mut deserializer = postcard::Deserializer::from_bytes(bytes);
    let seed = FieldsSeed {
        fields: known,
        context: &context,
        depth: 0,
    };
    let mut values = match seed.deserialize(&mut deserializer) {
        Ok(values) => values,
        Err(cause) => return Err(context.failure.take().unwrap_or_else(|| corrupt(cause))),
    };
    let trailing = deserializer.finalize().map_err(corrupt)?;
    if written_minor <= schema.version.minor && !trailing.is_empty() {
        return Err(Error::new(codes::PAYLOAD_INVALID).with_detail("reason", "trailing bytes"));
    }
    check_values(known, &values).map_err(|cause| {
        let field = cause.arguments().get("field").cloned();
        let error = Error::new(codes::PAYLOAD_INVALID).with_source(cause);
        match field {
            Some(field) => error.with_arg("field", field),
            None => error,
        }
    })?;
    let consumed = bytes.get(..bytes.len().saturating_sub(trailing.len()));
    if consumed != Some(to_bytes(known, &values)?.as_slice()) {
        return Err(Error::new(codes::PAYLOAD_INVALID).with_detail("reason", "not canonical"));
    }
    let appended = schema.fields.get(known.len()..).unwrap_or_default();
    values.extend(appended.iter().map(|field| field.default.clone()));
    Ok(Decoded {
        values,
        trailing: trailing.to_vec(),
    })
}

fn corrupt(cause: postcard::Error) -> Error {
    Error::new(codes::PAYLOAD_INVALID).with_source(cause)
}

/// Encodes checked values.
fn to_bytes(fields: &[FieldDecl], values: &[Option<Value>]) -> Result<Vec<u8>> {
    postcard::to_allocvec(&FieldsSer { fields, values })
        .map_err(|cause| Error::new(codes::VALUE_TYPE_MISMATCH).with_source(cause))
}

/// The fields of a schema or record as a `postcard` tuple: an optional field as an option, a
/// required one as its value.
struct FieldsSer<'a> {
    fields: &'a [FieldDecl],
    values: &'a [Option<Value>],
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
            });
            match (field.optional, typed) {
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
}

impl Serialize for TypedSer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match (self.value_type, self.value) {
            (_, Value::Bool(v)) => serializer.serialize_bool(*v),
            (_, Value::I64(v)) => serializer.serialize_i64(*v),
            (_, Value::F64(v)) => serializer.serialize_f64(*v),
            (_, Value::String(v)) => serializer.serialize_str(v),
            (_, Value::Quantity(q)) => serializer.serialize_f64(q.value()),
            (_, Value::Enum(code)) => serializer.serialize_u32(*code),
            (_, Value::Point2(p)) => [p.x, p.y].serialize(serializer),
            (_, Value::Point3(p)) => [p.x, p.y, p.z].serialize(serializer),
            (_, Value::Vector3(v)) => [v.x, v.y, v.z].serialize(serializer),
            (_, Value::Transform(t)) => transform_entries(t).serialize(serializer),
            (_, Value::Color(c)) => {
                (c.rgba.r, c.rgba.g, c.rgba.b, c.rgba.a, &c.source).serialize(serializer)
            }
            (_, Value::Reference(id)) => id_bytes(*id).serialize(serializer),
            (_, Value::ExternalReference(r)) => {
                (id_bytes(r.link), id_bytes(r.target)).serialize(serializer)
            }
            (_, Value::Asset(hash)) => hash.0.serialize(serializer),
            (_, Value::Geometry(bytes)) => serializer.serialize_bytes(&bytes.0),
            (_, Value::Opaque(bytes)) => serializer.serialize_bytes(bytes),
            (ValueType::List(element), Value::List(items)) => {
                serializer.collect_seq(items.iter().map(|value| TypedSer {
                    value_type: element,
                    value,
                }))
            }
            (ValueType::Map(element), Value::Map(entries)) => {
                serializer.collect_map(entries.iter().map(|(key, value)| {
                    let typed = TypedSer {
                        value_type: element,
                        value,
                    };
                    (key, typed)
                }))
            }
            (ValueType::Record(fields), Value::Record(values)) => {
                FieldsSer { fields, values }.serialize(serializer)
            }
            _ => Err(ser::Error::custom("value does not match its type")),
        }
    }
}

fn id_bytes(id: ObjectId) -> [u8; 16] {
    id.as_u128().to_be_bytes()
}

fn id_from(bytes: [u8; 16]) -> ObjectId {
    ObjectId::from_u128(u128::from_be_bytes(bytes))
}

/// The state shared by one decode: the limits and the first Arc error found, which replaces
/// the generic `postcard` error it travels inside.
struct Context<'a> {
    limits: &'a Limits,
    failure: Cell<Option<Error>>,
}

impl Context<'_> {
    fn fail<E: de::Error>(&self, error: Error) -> E {
        let first = self.failure.take().unwrap_or(error);
        self.failure.set(Some(first));
        E::custom("payload rejected")
    }

    fn within<E: de::Error>(
        &self,
        length: usize,
        max: u64,
        limit: &'static str,
    ) -> std::result::Result<(), E> {
        if u64::try_from(length).is_ok_and(|length| length <= max) {
            Ok(())
        } else {
            Err(self.fail(Error::new(codes::PAYLOAD_LIMIT_EXCEEDED).with_detail("limit", limit)))
        }
    }

    fn text<E: de::Error>(&self, text: &str) -> std::result::Result<String, E> {
        self.within(text.len(), self.limits.string_bytes, "string_bytes")?;
        Ok(text.to_owned())
    }

    fn bytes<E: de::Error>(&self, bytes: &[u8]) -> std::result::Result<Vec<u8>, E> {
        self.within(bytes.len(), self.limits.array_length, "array_length")?;
        Ok(bytes.to_vec())
    }
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
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        self.0.deserialize(deserializer).map(Some)
    }
}

/// Decodes one value of a known type.
#[derive(Clone, Copy)]
struct TypedSeed<'a> {
    value_type: &'a ValueType,
    context: &'a Context<'a>,
    depth: u32,
}

impl<'a> TypedSeed<'a> {
    /// The seed of an element one level deeper, or a limit failure.
    fn nested<E: de::Error>(self, value_type: &'a ValueType) -> std::result::Result<Self, E> {
        let depth = self.depth.saturating_add(1);
        if depth > self.context.limits.nesting_depth {
            return Err(self.context.fail(
                Error::new(codes::PAYLOAD_LIMIT_EXCEEDED).with_detail("limit", "nesting_depth"),
            ));
        }
        Ok(Self {
            value_type,
            context: self.context,
            depth,
        })
    }
}

impl<'de> DeserializeSeed<'de> for TypedSeed<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Value, D::Error> {
        let context = self.context;
        Ok(match self.value_type {
            ValueType::Bool => Value::Bool(bool::deserialize(deserializer)?),
            ValueType::I64 => Value::I64(i64::deserialize(deserializer)?),
            ValueType::F64 => Value::F64(f64::deserialize(deserializer)?),
            ValueType::String => Value::String(context.text(<&str>::deserialize(deserializer)?)?),
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
            ValueType::Transform => Value::Transform(transform_from_entries(
                &<[f64; 12]>::deserialize(deserializer)?,
            )),
            ValueType::Color => {
                let (r, g, b, a, source) =
                    <(f32, f32, f32, f32, Option<ColorSource>)>::deserialize(deserializer)?;
                if let Some(ColorSource::Named(name)) = &source {
                    context.within(name.len(), context.limits.string_bytes, "string_bytes")?;
                }
                Value::Color(ColorValue {
                    rgba: Color::new(r, g, b, a),
                    source,
                })
            }
            ValueType::Reference(_) => {
                Value::Reference(id_from(<[u8; 16]>::deserialize(deserializer)?))
            }
            ValueType::ExternalReference => {
                let (link, target) = <([u8; 16], [u8; 16])>::deserialize(deserializer)?;
                Value::ExternalReference(ExternalRef {
                    link: id_from(link),
                    target: id_from(target),
                })
            }
            ValueType::Asset => Value::Asset(AssetHash(<[u8; 32]>::deserialize(deserializer)?)),
            ValueType::Geometry(_) => Value::Geometry(GeometryBytes(
                context.bytes(<&[u8]>::deserialize(deserializer)?)?,
            )),
            ValueType::Opaque => Value::Opaque(context.bytes(<&[u8]>::deserialize(deserializer)?)?),
            ValueType::List(element) => {
                deserializer.deserialize_seq(ListVisitor(self.nested(element)?))?
            }
            ValueType::Map(element) => {
                deserializer.deserialize_map(MapVisitor(self.nested(element)?))?
            }
            ValueType::Record(fields) => {
                let nested = self.nested(self.value_type)?;
                let seed = FieldsSeed {
                    fields,
                    context,
                    depth: nested.depth,
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
        let limit = seed.context.limits.array_length;
        let hint = seq.size_hint().unwrap_or(0);
        seed.context.within(hint, limit, "array_length")?;
        let mut items = Vec::with_capacity(hint.min(PREALLOCATE));
        while let Some(item) = seq.next_element_seed(seed)? {
            items.push(item);
            seed.context.within(items.len(), limit, "array_length")?;
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
        context.within(hint, context.limits.array_length, "array_length")?;
        let mut entries = BTreeMap::new();
        while let Some(key) = map.next_key::<&str>()? {
            let key = context.text(key)?;
            let value = map.next_value_seed(seed)?;
            // A repeated key shrinks the map, which the canonical check rejects.
            entries.insert(key, value);
            context.within(entries.len(), context.limits.array_length, "array_length")?;
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
            let payload = encode(&schema, &values).unwrap();
            prop_assert_eq!(payload.encoding, Encoding::Postcard);
            let decoded = decode_all(&schema, &payload.bytes).unwrap();
            prop_assert_eq!(&decoded.values, &values);
            prop_assert!(decoded.trailing.is_empty());
            prop_assert_eq!(encode(&schema, &decoded.values).unwrap().bytes, payload.bytes);
        }

        #[test]
        fn arbitrary_bytes_never_panic_and_accepted_bytes_are_canonical(
            (fields, _) in object(),
            bytes in prop::collection::vec(any::<u8>(), 0..64),
        ) {
            let schema = fields_v1(fields);
            if let Ok(decoded) = decode_all(&schema, &bytes) {
                prop_assert_eq!(encode(&schema, &decoded.values).unwrap().bytes, bytes);
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
            let mut bytes = encode(&schema, &values).unwrap().bytes;
            let at = position.index(bytes.len() + 1);
            match kind {
                0 if at < bytes.len() => bytes[at] = byte,
                1 => bytes.truncate(at),
                _ => bytes.insert(at, byte),
            }
            if let Ok(decoded) = decode_all(&schema, &bytes) {
                prop_assert_eq!(encode(&schema, &decoded.values).unwrap().bytes, bytes);
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
            let bytes = encode(&schema, &[Some(sample)]).unwrap().bytes;
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
        let bytes = encode(&schema, &values).unwrap().bytes;
        assert_eq!(bytes, [1, 1, 3, 2, b'a', b'b']);
        let absent = [values[0].clone(), None, values[2].clone()];
        assert_eq!(
            encode(&schema, &absent).unwrap().bytes,
            [1, 0, 2, b'a', b'b']
        );
    }

    #[test]
    fn encode_checks_values_first() {
        let schema = fields_v1(vec![
            FieldDecl::new("n", ValueType::I64).with_bound(Bound::Int { min: 0, max: 9 }),
        ]);
        let encoded = |value| encode(&schema, &[value]).map(|_| ()).unwrap_err().code();
        assert_eq!(encoded(None), "doc.value.missing");
        assert_eq!(encoded(Some(Value::Bool(true))), "doc.value.type_mismatch");
        assert_eq!(encoded(Some(Value::I64(10))), "doc.value.out_of_range");
        let reals = fields_v1(vec![FieldDecl::new("x", ValueType::F64)]);
        let error = encode(&reals, &[Some(Value::F64(f64::NAN))]).unwrap_err();
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
        let bytes = encode(&strings, &values).unwrap().bytes;
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

        let old_bytes = encode(&old, &[Some(Value::I64(7))]).unwrap().bytes;
        let read_by_new =
            decode(&new, 0, Encoding::Postcard, &old_bytes, &Limits::DEFAULT).unwrap();
        assert_eq!(
            read_by_new.values,
            [Some(Value::I64(7)), Some(Value::Bool(true))]
        );

        let new_bytes = encode(&new, &[Some(Value::I64(7)), Some(Value::Bool(false))])
            .unwrap()
            .bytes;
        let read_by_old =
            decode(&old, 1, Encoding::Postcard, &new_bytes, &Limits::DEFAULT).unwrap();
        assert_eq!(read_by_old.values, [Some(Value::I64(7))]);
        assert_eq!(read_by_old.trailing, [0]);
        let mut written_back = encode(&old, &read_by_old.values).unwrap().bytes;
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
}
