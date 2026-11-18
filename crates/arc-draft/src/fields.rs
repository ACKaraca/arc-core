//! Fields (CORE-DRAFT-007): placeholders inside text whose value comes from the document.
//!
//! A [`Field`] is stored beside the text it is inserted into: in the `fields` list of a
//! single-line text and in the `fields` list of a run of a rich text. Its `offset` is a byte
//! offset into that text (the run's `text`, the entity's `content`) at a character boundary;
//! the evaluated string is inserted there, so the text itself holds no placeholder syntax and
//! needs no escaping. Offsets are in ascending order.
//!
//! The source of a field is one of
//!
//! - a document property ([`DocumentProperty`]): the name of the object's layer, or of the
//!   space that contains the object;
//! - a property of an object ([`Source::Object`]): the object and a path, see [`property`];
//! - an `arc-solve` expression whose names are bound to such object properties
//!   ([`Source::Expression`]).
//!
//! The evaluated texts of an entity are the derived node [`FIELDS`]; the display list reads it,
//! so a field updates when one of the fields it read changes. A source that is missing shows
//! [`MISSING_MARKER`] and raises a warning, reported on every commit by [`diagnostics`].

use std::cell::Cell;
use std::sync::Arc;

use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_command::IdParam;
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, content_hash,
};
use arc_doc::{
    Bound, Cardinality, FieldDecl, Finding, Severity, Snapshot, TypeKey, Value, ValueType,
};
use arc_geom::{CurveEval, Loop2};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dimension::clearing_reference;
use crate::display::{Reader, decode_curve};
use crate::error::codes;
use crate::format::{NumberFormat, format_length};
use crate::text::{Paragraph, RICH_TEXT, TEXT};
use crate::types::{NAME_BYTES, SPACE, TEXT_BYTES, text, variants};

/// The derived key of the evaluated texts of a text entity.
pub const FIELDS: &str = "core.draft.fields";

/// What a field with a missing source or a failed expression shows.
pub const MISSING_MARKER: &str = "#?";

/// The most fields of one text or run.
pub const MAX_FIELDS: u64 = 64;
/// The most name bindings of an expression field.
pub const MAX_BINDINGS: usize = 16;
/// The most decimals of a number field.
pub const MAX_PRECISION: u32 = 8;

/// A property of the document the object that holds the field is part of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocumentProperty {
    /// The name of the object's layer; missing when it has no layer.
    LayerName,
    /// The name of the space that contains the object; missing in a block definition.
    SpaceName,
}

/// A name an expression can use and the object property it stands for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// The name, an identifier of the expression language.
    pub name: String,
    /// The object; absent when it was deleted.
    #[serde(default)]
    pub object: Option<IdParam>,
    /// The property path, see [`property`].
    pub path: String,
}

/// Where the value of a field comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// A document property.
    Document {
        /// The property.
        property: DocumentProperty,
    },
    /// A property of an object.
    Object {
        /// The object; absent when it was deleted.
        #[serde(default)]
        object: Option<IdParam>,
        /// The property path, see [`property`].
        path: String,
    },
    /// An expression over named properties.
    Expression {
        /// The expression text.
        expression: String,
        /// The names the expression may use.
        #[serde(default)]
        bindings: Vec<Binding>,
    },
}

fn two() -> u32 {
    2
}

/// A field: a source and the place its value takes in the text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// The byte offset in the text where the value is inserted.
    pub offset: usize,
    /// The source.
    pub source: Source,
    /// The decimals of a number value, 0 to 8; default 2.
    #[serde(default = "two")]
    pub precision: u32,
}

impl Field {
    /// A field at `offset` with the default precision.
    #[must_use]
    pub fn new(offset: usize, source: Source) -> Self {
        Self {
            offset,
            source,
            precision: two(),
        }
    }
}

fn binding_type() -> ValueType {
    ValueType::Record(vec![
        FieldDecl::new("name", ValueType::String).with_bound(Bound::Length { max: NAME_BYTES }),
        FieldDecl::new("object", clearing_reference(&[], Cardinality::Optional)).optional(),
        FieldDecl::new("path", ValueType::String).with_bound(Bound::Length { max: NAME_BYTES }),
    ])
}

fn field_type() -> ValueType {
    ValueType::Record(vec![
        FieldDecl::new("offset", ValueType::I64).with_bound(Bound::Int {
            min: 0,
            max: i64::try_from(TEXT_BYTES).unwrap_or(i64::MAX),
        }),
        FieldDecl::new("kind", variants(&["document", "object", "expression"])),
        FieldDecl::new("property", variants(&["layer_name", "space_name"]))
            .with_default(Value::Enum(0)),
        FieldDecl::new("object", clearing_reference(&[], Cardinality::Optional)).optional(),
        text("path", NAME_BYTES),
        text("expression", TEXT_BYTES),
        FieldDecl::new("bindings", ValueType::List(Box::new(binding_type()))).with_bound(
            Bound::Length {
                max: MAX_BINDINGS as u64,
            },
        ),
        FieldDecl::new("precision", ValueType::I64)
            .with_default(Value::I64(2))
            .with_bound(Bound::Int { min: 0, max: 8 }),
    ])
}

/// The declaration of a `fields` list of a text or run.
pub(crate) fn list_decl() -> FieldDecl {
    FieldDecl::new("fields", ValueType::List(Box::new(field_type())))
        .with_bound(Bound::Length { max: MAX_FIELDS })
        .with_default(Value::List(Vec::new()))
}

fn object_value(object: Option<IdParam>) -> Option<Value> {
    object.map(|IdParam(id)| Value::Reference(id))
}

impl Field {
    /// The stored form.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let (kind, property, object, path, expression, bindings) = match &self.source {
            Source::Document { property } => (0, *property as u32, None, None, None, Vec::new()),
            Source::Object { object, path } => (
                1,
                0,
                object_value(*object),
                Some(path.clone()),
                None,
                Vec::new(),
            ),
            Source::Expression {
                expression,
                bindings,
            } => (
                2,
                0,
                None,
                None,
                Some(expression.clone()),
                bindings
                    .iter()
                    .map(|b| {
                        Value::Record(vec![
                            Some(Value::String(b.name.clone())),
                            object_value(b.object),
                            Some(Value::String(b.path.clone())),
                        ])
                    })
                    .collect(),
            ),
        };
        Value::Record(vec![
            Some(Value::I64(i64::try_from(self.offset).unwrap_or(i64::MAX))),
            Some(Value::Enum(kind)),
            Some(Value::Enum(property)),
            object,
            path.map(Value::String),
            expression.map(Value::String),
            Some(Value::List(bindings)),
            Some(Value::I64(i64::from(self.precision))),
        ])
    }

    /// The field of a stored value; `None` when it is not a field record.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let Value::Record(f) = value else {
            return None;
        };
        let string = |v: Option<&Option<Value>>| match v {
            Some(Some(Value::String(s))) => Some(s.clone()),
            _ => None,
        };
        let object = |v: Option<&Option<Value>>| match v {
            Some(Some(Value::Reference(id))) => Some(IdParam(*id)),
            _ => None,
        };
        let source = match f.get(1) {
            Some(Some(Value::Enum(0))) => Source::Document {
                property: match f.get(2) {
                    Some(Some(Value::Enum(1))) => DocumentProperty::SpaceName,
                    _ => DocumentProperty::LayerName,
                },
            },
            Some(Some(Value::Enum(1))) => Source::Object {
                object: object(f.get(3)),
                path: string(f.get(4)).unwrap_or_default(),
            },
            Some(Some(Value::Enum(2))) => Source::Expression {
                expression: string(f.get(5)).unwrap_or_default(),
                bindings: match f.get(6) {
                    Some(Some(Value::List(items))) => items
                        .iter()
                        .filter_map(|item| {
                            let Value::Record(b) = item else {
                                return None;
                            };
                            Some(Binding {
                                name: string(b.first())?,
                                object: object(b.get(1)),
                                path: string(b.get(2))?,
                            })
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            },
            _ => return None,
        };
        Some(Self {
            offset: match f.first() {
                Some(Some(Value::I64(n))) => usize::try_from(*n).ok()?,
                _ => return None,
            },
            source,
            precision: match f.get(7) {
                Some(Some(Value::I64(n))) => u32::try_from(*n).unwrap_or(two()),
                _ => two(),
            },
        })
    }
}

/// The stored `fields` list.
#[must_use]
pub fn list_value(fields: &[Field]) -> Value {
    Value::List(fields.iter().map(Field::to_value).collect())
}

/// The fields of a stored `fields` list.
#[must_use]
pub fn fields_of(value: Option<&Value>) -> Vec<Field> {
    match value {
        Some(Value::List(items)) => items.iter().filter_map(Field::from_value).collect(),
        _ => Vec::new(),
    }
}

/// Checks the fields of `text` against the rules.
///
/// # Errors
///
/// `draft.field.placeholder_invalid` (argument `reason`) for more than [`MAX_FIELDS`] fields,
/// offsets that are not ascending, beyond the text or inside a character, a precision above
/// [`MAX_PRECISION`], a malformed source or an expression longer than the string limit.
pub fn validate(text: &str, fields: &[Field], limits: &Limits) -> Result<()> {
    let invalid =
        |reason: &str| Error::new(codes::FIELD_PLACEHOLDER_INVALID).with_arg("reason", reason);
    if u64::try_from(fields.len()).is_ok_and(|n| n > MAX_FIELDS) {
        return Err(invalid("count"));
    }
    let mut previous = 0;
    for f in fields {
        if f.offset < previous || !text.is_char_boundary(f.offset) {
            return Err(invalid("offset"));
        }
        previous = f.offset;
        if f.precision > MAX_PRECISION {
            return Err(invalid("precision"));
        }
        let long =
            |s: &str| u64::try_from(s.len()).is_ok_and(|n| n > limits.string_bytes.min(TEXT_BYTES));
        match &f.source {
            Source::Document { .. } => {}
            Source::Object { path, .. } => {
                if long(path) {
                    return Err(invalid("path"));
                }
            }
            Source::Expression {
                expression,
                bindings,
            } => {
                if long(expression) || bindings.len() > MAX_BINDINGS {
                    return Err(invalid("expression"));
                }
                if bindings.iter().any(|b| b.name.is_empty() || long(&b.path)) {
                    return Err(invalid("binding"));
                }
            }
        }
    }
    Ok(())
}

/// Why a field shows the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    /// The object or property does not exist.
    SourceMissing,
    /// The expression does not evaluate.
    ExpressionInvalid,
}

impl IssueKind {
    /// The error of the warning.
    #[must_use]
    pub fn error(self) -> Error {
        Error::new(match self {
            Self::SourceMissing => codes::FIELD_SOURCE_MISSING,
            Self::ExpressionInvalid => codes::FIELD_EXPRESSION_INVALID,
        })
    }
}

/// A field that shows the marker: its position among the fields of the entity in reading
/// order, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Issue {
    /// The position among the fields of the entity, from 0.
    pub field: usize,
    /// The reason.
    pub kind: IssueKind,
}

/// The paragraphs of a text with every field replaced by its value, and the fields that show
/// the marker; the result of the [`FIELDS`] node.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluated {
    /// The paragraphs; their runs hold no fields.
    pub paragraphs: Vec<Paragraph>,
    /// The fields with a missing source or a failed expression.
    pub issues: Vec<Issue>,
}

impl Derived for Evaluated {
    fn content_hash(&self) -> u128 {
        // shortcut: hashes the exact `Debug` text, as the display lists do.
        content_hash(&format!("{self:?}"))
    }
}

/// A property value.
enum Prop {
    Number(f64),
    Text(String),
}

/// The value of the property `path` of `object`, `None` when the object or the property does
/// not exist or has no value of that kind. Paths:
///
/// - `length`: the length of a curve entity;
/// - `area`: the area a closed curve entity encloses;
/// - `field.<name>`: the stored field `<name>`: a number, an integer (as a number), a text or
///   a flag (as `true` or `false`).
///
/// Lengths and areas are in the drawing units of the object's container.
fn property(reader: &dyn Reader, object: ObjectId, path: &str, tol: &Tolerance) -> Option<Prop> {
    reader.type_key(object)?;
    let finite = |x: f64| x.is_finite().then_some(Prop::Number(x));
    match path {
        "length" => {
            let curve = decode_curve(Some(&reader.field(object, "geometry").ok()?)).ok()?;
            finite(curve.length(tol).ok()?)
        }
        "area" => {
            let curve = decode_curve(Some(&reader.field(object, "geometry").ok()?)).ok()?;
            finite(Loop2::new(vec![curve], tol).ok()?.measure(tol).ok()?.area)
        }
        _ => match reader.field(object, path.strip_prefix("field.")?).ok()?? {
            Value::F64(x) => finite(x),
            #[expect(clippy::cast_precision_loss, reason = "a display value")]
            Value::I64(n) => finite(n as f64),
            Value::String(s) => Some(Prop::Text(s)),
            Value::Bool(b) => Some(Prop::Text(b.to_string())),
            _ => None,
        },
    }
}

fn number_text(value: f64, precision: u32) -> String {
    let format = NumberFormat {
        precision: precision.min(MAX_PRECISION),
        ..NumberFormat::default()
    };
    format_length(value, &format, 1.0)
}

fn name_of(reader: &dyn Reader, object: ObjectId) -> Option<String> {
    match reader.field(object, "name").ok()? {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

fn evaluate_one(
    reader: &dyn Reader,
    host: ObjectId,
    field: &Field,
    tol: &Tolerance,
) -> std::result::Result<String, IssueKind> {
    let missing = IssueKind::SourceMissing;
    match &field.source {
        Source::Document { property } => match property {
            DocumentProperty::LayerName => match reader.field(host, "layer").ok().flatten() {
                Some(Value::Reference(layer)) => name_of(reader, layer).ok_or(missing),
                _ => Err(missing),
            },
            DocumentProperty::SpaceName => {
                let space = reader.owner(host).ok().flatten().ok_or(missing)?;
                if reader.type_key(space).as_deref() == Some(SPACE) {
                    name_of(reader, space).ok_or(missing)
                } else {
                    Err(missing)
                }
            }
        },
        Source::Object { object, path } => {
            match object.and_then(|IdParam(o)| property(reader, o, path, tol)) {
                Some(Prop::Number(x)) => Ok(number_text(x, field.precision)),
                Some(Prop::Text(s)) => Ok(s),
                None => Err(missing),
            }
        }
        Source::Expression {
            expression,
            bindings,
        } => {
            let absent = Cell::new(false);
            let lookup = |name: &str| -> Result<Option<arc_solve::Value>> {
                let Some(b) = bindings.iter().find(|b| b.name == name) else {
                    return Ok(None);
                };
                match b
                    .object
                    .and_then(|IdParam(o)| property(reader, o, &b.path, tol))
                {
                    Some(Prop::Number(x)) => Ok(Some(arc_solve::Value::Bare(x))),
                    Some(Prop::Text(s)) => Ok(Some(arc_solve::Value::text(s))),
                    None => {
                        absent.set(true);
                        Err(Error::new(codes::FIELD_SOURCE_MISSING))
                    }
                }
            };
            let context = arc_solve::Context::new()
                .with_limits(arc_solve::ExprLimits::DEFAULT.with_steps(100_000))
                .with_names(&lookup);
            match arc_solve::evaluate(expression, &context) {
                Ok(value) => {
                    value_text(&value, field.precision).ok_or(IssueKind::ExpressionInvalid)
                }
                Err(_) if absent.get() => Err(missing),
                Err(_) => Err(IssueKind::ExpressionInvalid),
            }
        }
    }
}

/// The text of an expression result: a number (its value in SI base units), a text or a truth
/// value; `None` for a list.
pub(crate) fn value_text(value: &arc_solve::Value, precision: u32) -> Option<String> {
    match value {
        arc_solve::Value::Text(t) => Some(t.to_string()),
        arc_solve::Value::Bool(b) => Some(b.to_string()),
        other => Some(number_text(other.quantity()?.value(), precision)),
    }
}

/// `text` with the values of `fields` inserted at their offsets; a field that cannot be
/// evaluated inserts [`MISSING_MARKER`] and is listed in `issues` with its position `first`
/// plus its index.
pub(crate) fn substitute(
    reader: &dyn Reader,
    host: ObjectId,
    text: &str,
    fields: &[Field],
    first: usize,
    tol: &Tolerance,
    issues: &mut Vec<Issue>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cut = 0;
    for (i, field) in fields.iter().enumerate() {
        let mut at = field.offset.clamp(cut, text.len());
        while !text.is_char_boundary(at) {
            at += 1;
        }
        out.push_str(text.get(cut..at).unwrap_or(""));
        cut = at;
        match evaluate_one(reader, host, field, tol) {
            Ok(s) => out.push_str(&s),
            Err(kind) => {
                out.push_str(MISSING_MARKER);
                issues.push(Issue {
                    field: first + i,
                    kind,
                });
            }
        }
    }
    out.push_str(text.get(cut..).unwrap_or(""));
    out
}

/// The evaluated paragraphs of the text or rich text `id`, evaluated through `reader`.
pub(crate) fn evaluate(reader: &dyn Reader, id: ObjectId, tol: &Tolerance) -> Result<Evaluated> {
    let mut issues = Vec::new();
    let paragraphs = if reader.type_key(id).as_deref() == Some(TEXT) {
        let content = match reader.field(id, "content")? {
            Some(Value::String(s)) => s,
            _ => String::new(),
        };
        let fields = fields_of(reader.field(id, "fields")?.as_ref());
        let text = substitute(reader, id, &content, &fields, 0, tol, &mut issues);
        vec![Paragraph {
            runs: vec![crate::text::Run {
                text,
                ..crate::text::Run::default()
            }],
            ..Paragraph::default()
        }]
    } else {
        let mut paragraphs = match reader.field(id, "paragraphs")? {
            Some(Value::List(items)) => items.iter().filter_map(Paragraph::from_value).collect(),
            _ => Vec::new(),
        };
        let mut counted = 0;
        for run in paragraphs
            .iter_mut()
            .flat_map(|p: &mut Paragraph| p.runs.iter_mut())
        {
            let fields = std::mem::take(&mut run.fields);
            run.text = substitute(reader, id, &run.text, &fields, counted, tol, &mut issues);
            counted += fields.len();
        }
        paragraphs
    };
    Ok(Evaluated { paragraphs, issues })
}

/// The node of the evaluated texts of `entity`.
///
/// # Errors
///
/// None for the constant key; the key check of `arc-doc` is passed on.
pub fn node(entity: ObjectId) -> Result<NodeKey> {
    Ok(NodeKey::object(entity, DerivedKey::new(FIELDS)?))
}

/// The evaluator of [`FIELDS`] with the tolerance of `options`.
pub fn evaluator(
    options: &crate::display::DisplayOptions,
) -> impl Fn(&EvalContext<'_>) -> Result<DerivedValue> + Send + Sync + Clone + 'static {
    let tol = options.tolerance;
    move |ctx| {
        let id = ctx
            .object_id()
            .ok_or_else(|| Error::new(codes::OBJECT_WRONG_TYPE))?;
        let evaluated = evaluate(&crate::display::GraphReader { ctx }, id, &tol)?;
        Ok(Arc::new(evaluated) as DerivedValue)
    }
}

/// Registers the [`FIELDS`] evaluator of single-line and rich text with the tolerance of
/// `options`.
///
/// # Errors
///
/// `doc.graph.duplicate_key` when it is registered already.
pub fn register_evaluators(
    evaluators: &mut Evaluators,
    options: &crate::display::DisplayOptions,
) -> Result<()> {
    for key in [TEXT, RICH_TEXT] {
        evaluators.register_object(
            TypeKey::new(key)?,
            DerivedKey::new(FIELDS)?,
            evaluator(options),
        )?;
    }
    Ok(())
}

/// The evaluated texts of the text or rich text `entity` of `snapshot`, without the graph.
///
/// # Errors
///
/// `doc.object.not_found`; `draft.object.wrong_type` for an object that is not a text.
pub fn resolve(
    snapshot: &Snapshot,
    entity: ObjectId,
    options: &crate::display::DisplayOptions,
) -> Result<Evaluated> {
    crate::display::with_snapshot(snapshot, options, |reader| {
        match reader.type_key(entity).as_deref() {
            Some(TEXT | RICH_TEXT) => evaluate(reader, entity, &options.tolerance),
            Some(_) => {
                Err(Error::new(codes::OBJECT_WRONG_TYPE).with_detail("id", entity.to_string()))
            }
            None => Err(Error::new(arc_doc::error::codes::OBJECT_NOT_FOUND)),
        }
    })
}

/// Whether the stored values of a text type hold a field.
fn has_fields(values: &[Option<Value>], key: &str) -> bool {
    if key == TEXT {
        return matches!(values.get(crate::text::field::FIELDS), Some(Some(Value::List(l))) if !l.is_empty());
    }
    crate::text::paragraphs_of(values)
        .iter()
        .any(|p| p.runs.iter().any(|r| !r.fields.is_empty()))
}

/// A warning for every field of `snapshot` that shows the marker (`draft.field.source_missing`
/// or `draft.field.expression_invalid`, details `id` and `field`).
pub(crate) fn diagnostics(snapshot: &Snapshot) -> Vec<Finding> {
    let options = crate::dimension::resolution_options();
    let mut out = Vec::new();
    for key in [TEXT, RICH_TEXT] {
        let Ok(all) = snapshot.iter_type(key) else {
            continue;
        };
        for (_, object) in all.filter(|(_, o)| has_fields(&o.values, key)) {
            let issues = crate::display::with_snapshot(snapshot, &options, |reader| {
                evaluate(reader, object.id, &options.tolerance)
            })
            .map(|e| e.issues)
            .unwrap_or_default();
            out.extend(issues.into_iter().map(|issue| {
                Finding {
                    severity: Severity::Warning,
                    error: issue
                        .kind
                        .error()
                        .with_detail("id", object.id.to_string())
                        .with_detail("field", i64::try_from(issue.field).unwrap_or(i64::MAX)),
                }
            }));
        }
    }
    out
}
