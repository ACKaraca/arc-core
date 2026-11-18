//! Access to drafting object values by field name, so the mapping does not depend on field
//! positions.

use arc_base::{Error, ObjectId, Result};
use arc_doc::{Snapshot, TypeRegistry, Value};

use crate::error::codes;

/// The field names of `key`, in declared order.
fn names_of(registry: &TypeRegistry, key: &str) -> Vec<String> {
    registry
        .get(key)
        .map(|decl| decl.schema.fields.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default()
}

/// A new object's values: the declared defaults, set by field name.
pub struct Builder {
    names: Vec<String>,
    /// The values in declared order.
    pub values: Vec<Option<Value>>,
}

impl Builder {
    /// A builder for a new object of type `key`.
    #[must_use]
    pub fn new(registry: &TypeRegistry, key: &str) -> Self {
        Self {
            names: names_of(registry, key),
            values: arc_draft::types::defaults(registry, key),
        }
    }

    /// Sets the field `name`.
    ///
    /// # Errors
    ///
    /// `draft.dxf.document_invalid` when the type has no such field.
    pub fn set(&mut self, name: &str, value: Value) -> Result<&mut Self> {
        self.put(name, Some(value))
    }

    /// Sets the field `name` when `value` is present.
    ///
    /// # Errors
    ///
    /// As [`Builder::set`].
    pub fn set_opt(&mut self, name: &str, value: Option<Value>) -> Result<&mut Self> {
        self.put(name, value)
    }

    fn put(&mut self, name: &str, value: Option<Value>) -> Result<&mut Self> {
        let slot = self
            .names
            .iter()
            .position(|n| n == name)
            .and_then(|i| self.values.get_mut(i))
            .ok_or_else(|| Error::new(codes::DOCUMENT_INVALID).with_detail("field", name))?;
        if value.is_some() {
            *slot = value;
        }
        Ok(self)
    }
}

/// The values of one stored object, read by field name.
pub struct Reader<'a> {
    names: Vec<String>,
    values: &'a [Option<Value>],
}

impl<'a> Reader<'a> {
    /// A reader of `values`, the values of an object of type `key`.
    #[must_use]
    pub fn new(registry: &TypeRegistry, key: &str, values: &'a [Option<Value>]) -> Self {
        Self {
            names: names_of(registry, key),
            values,
        }
    }

    /// The value of the field `name`, absent when the field has none.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'a Value> {
        let index = self.names.iter().position(|n| n == name)?;
        self.values.get(index)?.as_ref()
    }

    /// A real field.
    #[must_use]
    pub fn f64(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            Value::F64(v) => Some(*v),
            _ => None,
        }
    }

    /// A real field with a default.
    #[must_use]
    pub fn f64_or(&self, name: &str, default: f64) -> f64 {
        self.f64(name).unwrap_or(default)
    }

    /// A boolean field with a default.
    #[must_use]
    pub fn bool_or(&self, name: &str, default: bool) -> bool {
        match self.get(name) {
            Some(Value::Bool(b)) => *b,
            _ => default,
        }
    }

    /// An enum field's code.
    #[must_use]
    pub fn enum_code(&self, name: &str) -> Option<u32> {
        match self.get(name)? {
            Value::Enum(c) => Some(*c),
            _ => None,
        }
    }

    /// A text field.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&'a str> {
        match self.get(name)? {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// A reference field.
    #[must_use]
    pub fn reference(&self, name: &str) -> Option<ObjectId> {
        match self.get(name)? {
            Value::Reference(id) => Some(*id),
            _ => None,
        }
    }
}

/// The name of the object `id` (its `name` field), when it has one.
#[must_use]
pub fn name_of(snapshot: &Snapshot, id: Option<ObjectId>) -> Option<String> {
    let id = id?;
    let handle = snapshot.resolve(id).ok()?;
    let key = snapshot.type_key(handle)?.as_str().to_owned();
    let object = snapshot.get(handle).ok()?;
    Reader::new(snapshot.registry(), &key, &object.values)
        .text("name")
        .map(str::to_owned)
}
