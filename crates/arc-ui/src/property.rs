//! The property grid (CORE-UI-010).
//!
//! [`PropertyModel::build`] turns the field declarations and values of one or many selected
//! objects ([`PropertyObject`]) into categorized rows: the fields every object has with the
//! same type, in the first object's declared order, grouped by aspect in order of first
//! appearance. A row shows the common value, or the mixed state when the objects differ; it is
//! read-only when any object marks it so or the grid has no editor for its type, and carries an
//! override marker when any object marks it overridden. [`PropertyGrid::show`] draws the model;
//! a committed edit returns exactly one [`Intent`] for the edit command with the parameters
//! `objects` (identifiers), `field` (the field path text) and `value` (the canonical JSON form
//! of the value).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use arc_base::{Dimension, DisplayUnits, Error, ObjectId, Quantity, Result};
use arc_doc::schema::{Bound, FieldDecl, FieldPath, Value, ValueType};
use egui::{Checkbox, ComboBox, TextEdit, Ui};

use crate::command::{Intent, object_list};
use crate::error::codes;
use crate::quantity::{QuantityField, QuantityInput};
use crate::text::{Localize, messages};

/// The most objects times fields one model compares.
pub const MAX_COMPARISONS: usize = 1_000_000;
/// The most characters a read-only value shows.
pub const MAX_SHOWN_CHARS: usize = 64;

/// One selected object as the grid reads it.
#[derive(Debug, Clone, Copy)]
pub struct PropertyObject<'a> {
    /// The object.
    pub id: ObjectId,
    /// Its field declarations.
    pub fields: &'a [FieldDecl],
    /// Its values, in declared order.
    pub values: &'a [Option<Value>],
    /// The names of fields the user may not edit on this object.
    pub read_only: &'a [String],
    /// The names of fields whose value overrides an inherited one.
    pub overridden: &'a [String],
}

/// What a row shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    /// Every object has this value.
    Value(Value),
    /// The objects differ.
    Mixed,
    /// No object has a value.
    Absent,
}

/// One field of the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyRow {
    /// The field path.
    pub path: FieldPath,
    /// The field name.
    pub name: String,
    /// The value type.
    pub value_type: ValueType,
    /// The declared bound.
    pub bound: Option<Bound>,
    /// The common value or the mixed state.
    pub shown: Shown,
    /// The row cannot be edited.
    pub read_only: bool,
    /// Some object overrides the value.
    pub overridden: bool,
}

/// The rows of one aspect.
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    /// The aspect name.
    pub aspect: String,
    /// The rows.
    pub rows: Vec<PropertyRow>,
}

/// The grid's rows for a selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PropertyModel {
    /// The selected objects.
    pub objects: Vec<ObjectId>,
    /// The categories.
    pub categories: Vec<Category>,
}

fn editable(value_type: &ValueType) -> bool {
    matches!(
        value_type,
        ValueType::Bool
            | ValueType::I64
            | ValueType::F64
            | ValueType::String
            | ValueType::Quantity(_)
            | ValueType::Enum(_)
            | ValueType::Point3
    )
}

impl PropertyModel {
    /// The model of `objects`.
    ///
    /// # Errors
    ///
    /// `ui.property.too_large` when the objects times the first object's fields exceed
    /// [`MAX_COMPARISONS`].
    pub fn build(objects: &[PropertyObject<'_>]) -> Result<Self> {
        let Some(first) = objects.first() else {
            return Ok(Self::default());
        };
        if objects.len().saturating_mul(first.fields.len()) > MAX_COMPARISONS {
            return Err(Error::new(codes::PROPERTY_TOO_LARGE));
        }
        let lookups: Vec<HashMap<&str, usize>> = objects
            .iter()
            .map(|o| {
                o.fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| (f.name.as_str(), i))
                    .collect()
            })
            .collect();
        let mut categories: Vec<Category> = Vec::new();
        for decl in first.fields {
            let mut shown: Option<Shown> = None;
            let (mut read_only, mut overridden) = (!editable(&decl.value_type), false);
            let mut common = true;
            for (object, lookup) in objects.iter().zip(&lookups) {
                let Some(&at) = lookup.get(decl.name.as_str()) else {
                    common = false;
                    break;
                };
                if object.fields.get(at).map(|f| &f.value_type) != Some(&decl.value_type) {
                    common = false;
                    break;
                }
                read_only |= object.read_only.contains(&decl.name);
                overridden |= object.overridden.contains(&decl.name);
                let value = object.values.get(at).and_then(Option::as_ref);
                shown = Some(match (shown, value) {
                    (None, Some(v)) => Shown::Value(v.clone()),
                    (None, None) => Shown::Absent,
                    (Some(Shown::Value(a)), Some(b)) if a.bits_eq(b) => Shown::Value(a),
                    (Some(Shown::Absent), None) => Shown::Absent,
                    _ => Shown::Mixed,
                });
            }
            if !common {
                continue;
            }
            let row = PropertyRow {
                path: FieldPath::new(decl.name.clone()),
                name: decl.name.clone(),
                value_type: decl.value_type.clone(),
                bound: decl.bound,
                shown: shown.unwrap_or(Shown::Absent),
                read_only,
                overridden,
            };
            match categories.iter_mut().find(|c| c.aspect == decl.aspect) {
                Some(category) => category.rows.push(row),
                None => categories.push(Category {
                    aspect: decl.aspect.clone(),
                    rows: vec![row],
                }),
            }
        }
        Ok(Self {
            objects: objects.iter().map(|o| o.id).collect(),
            categories,
        })
    }

    /// The edit intent for setting `row` to `value` on every selected object.
    #[must_use]
    pub fn edit(&self, command: &str, row: &PropertyRow, value: &Value) -> Intent {
        Intent::new(command)
            .with("objects", object_list(&self.objects))
            .with("field", row.path.to_string())
            .with("value", serde_json::to_value(value).unwrap_or_default())
    }
}

/// What the grid reads besides the model.
#[derive(Clone, Copy)]
pub struct GridContext<'a> {
    /// The message source.
    pub localize: &'a dyn Localize,
    /// The localized label of a field or aspect name.
    pub label: &'a dyn Fn(&str) -> String,
    /// The display units of quantity fields.
    pub units: &'a DisplayUnits,
    /// The decimal separator of the user's locale.
    pub decimal_separator: char,
    /// The command an edit invokes.
    pub edit_command: &'a str,
}

/// The editing state of the grid's fields.
#[derive(Debug, Default)]
pub struct PropertyGrid {
    numbers: BTreeMap<String, QuantityInput>,
    texts: BTreeMap<String, (String, bool)>,
}

fn numeric_field(row: &PropertyRow, context: &GridContext<'_>) -> QuantityField {
    let dimension = match row.value_type {
        ValueType::Quantity(d) => d,
        _ => Dimension::NONE,
    };
    let format = (dimension != Dimension::NONE)
        .then(|| context.units.get(dimension).copied())
        .flatten();
    let mut field = QuantityField::new(dimension, format);
    field.decimal_separator = context.decimal_separator;
    field.integer = matches!(row.value_type, ValueType::I64);
    field.range = match row.bound {
        // shortcut: i64 bounds beyond 2^53 are compared as f64; exact enough for UI ranges.
        #[allow(clippy::cast_precision_loss, reason = "UI range")]
        Some(Bound::Int { min, max }) => Some((min as f64, max as f64)),
        Some(Bound::Real { min, max }) => Some((min, max)),
        _ => None,
    };
    field
}

fn number_of(shown: &Shown, dimension: Dimension) -> Option<Quantity> {
    match shown {
        Shown::Value(Value::Quantity(q)) => Some(*q),
        Shown::Value(Value::F64(x)) => Some(Quantity::new(*x, dimension)),
        // shortcut: i64 values beyond 2^53 show rounded; exact entry still parses as a number.
        #[allow(clippy::cast_precision_loss, reason = "display")]
        Shown::Value(Value::I64(x)) => Some(Quantity::new(*x as f64, dimension)),
        _ => None,
    }
}

fn shown_text(shown: &Shown, localize: &dyn Localize) -> String {
    match shown {
        Shown::Value(Value::String(s)) => s.clone(),
        Shown::Value(v) => serde_json::to_string(v)
            .unwrap_or_default()
            .chars()
            .take(MAX_SHOWN_CHARS)
            .collect(),
        Shown::Mixed => messages::PROPERTY_MIXED.text(localize),
        Shown::Absent => String::new(),
    }
}

/// What a read-only row shows: a number in the display unit and the user's decimal separator, as
/// an editable field would show it; any other value as text.
fn read_only_text(row: &PropertyRow, context: &GridContext<'_>) -> String {
    let field = numeric_field(row, context);
    match (&row.value_type, number_of(&row.shown, field.dimension)) {
        (ValueType::I64 | ValueType::F64 | ValueType::Quantity(_), Some(q)) => field.display(q),
        _ => shown_text(&row.shown, context.localize),
    }
}

impl PropertyGrid {
    /// Draws `model` by category and returns the intent of a committed edit. Text and numbers
    /// commit on Enter or when the field loses focus with a changed, valid text; a check box or
    /// a choice commits at once.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        model: &PropertyModel,
        context: &GridContext<'_>,
    ) -> Option<Intent> {
        let keys: BTreeSet<String> = model
            .categories
            .iter()
            .flat_map(|c| c.rows.iter().map(|r| r.path.to_string()))
            .collect();
        // Point fields keep one input per axis under `<path>#<axis>`.
        self.numbers
            .retain(|k, _| keys.contains(k.split('#').next().unwrap_or_default()));
        self.texts.retain(|k, _| keys.contains(k));
        let mut intent = None;
        for category in &model.categories {
            ui.strong((context.label)(&category.aspect));
            for row in &category.rows {
                if let Some(value) = self.row(ui, row, context) {
                    intent = Some(model.edit(context.edit_command, row, &value));
                }
            }
        }
        intent
    }

    fn row(&mut self, ui: &mut Ui, row: &PropertyRow, context: &GridContext<'_>) -> Option<Value> {
        let key = row.path.to_string();
        let id = ui.make_persistent_id(("arc-property", &key));
        let mut name = (context.label)(&row.name);
        let name_for_errors = name.clone();
        if row.overridden {
            name = format!(
                "{name} {}",
                messages::PROPERTY_OVERRIDDEN.text(context.localize)
            );
        }
        ui.horizontal(|ui| {
            let label = ui.label(name.as_str());
            if row.read_only {
                let text = read_only_text(row, context);
                ui.add_enabled(false, TextEdit::singleline(&mut text.as_str()))
                    .labelled_by(label.id);
                return None;
            }
            match &row.value_type {
                ValueType::Bool => {
                    let mixed = row.shown == Shown::Mixed;
                    let mut on = row.shown == Shown::Value(Value::Bool(true));
                    let response = ui.add(Checkbox::new(&mut on, "").indeterminate(mixed));
                    let response = response.labelled_by(label.id);
                    response.clicked().then_some(Value::Bool(on))
                }
                ValueType::Enum(variants) => {
                    let current = match &row.shown {
                        Shown::Value(Value::Enum(code)) => variants
                            .iter()
                            .find(|v| v.code == *code)
                            .map(|v| (context.label)(&v.name)),
                        Shown::Mixed => Some(messages::PROPERTY_MIXED.text(context.localize)),
                        _ => None,
                    };
                    let mut chosen = None;
                    let combo = ComboBox::from_id_salt(id)
                        .selected_text(current.unwrap_or_default())
                        .show_ui(ui, |ui| {
                            for variant in variants {
                                let selected = row.shown == Shown::Value(Value::Enum(variant.code));
                                if ui
                                    .selectable_label(selected, (context.label)(&variant.name))
                                    .clicked()
                                    && !selected
                                {
                                    chosen = Some(Value::Enum(variant.code));
                                }
                            }
                        });
                    let _ = combo.response.labelled_by(label.id);
                    chosen
                }
                ValueType::String => {
                    let shown = shown_text(&row.shown, context.localize);
                    let (text, editing) = self.texts.entry(key).or_default();
                    if !*editing {
                        text.clone_from(&shown);
                    }
                    let edit = ui
                        .add(TextEdit::singleline(text).id(id))
                        .labelled_by(label.id);
                    if edit.gained_focus() {
                        *editing = true;
                    }
                    if edit.lost_focus() {
                        // The committing Enter must not also reach the command line.
                        ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                        *editing = false;
                        let escaped = ui
                            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
                        if !escaped && *text != shown {
                            return Some(Value::String(text.clone()));
                        }
                    }
                    None
                }
                ValueType::I64 | ValueType::F64 | ValueType::Quantity(_) => {
                    let field = numeric_field(row, context);
                    let input = self.numbers.entry(key).or_default();
                    input.label.clone_from(&name_for_errors);
                    let value = number_of(&row.shown, field.dimension);
                    // The field and its error line stack beside the label, so the error shows
                    // under the field and is never clipped at the right edge.
                    let q = ui
                        .vertical(|ui| {
                            input.show(ui, id, &field, value, context.localize, Some(label.id))
                        })
                        .inner?;
                    Some(match row.value_type {
                        // The field accepted only whole numbers within the i64 range checks.
                        #[allow(clippy::cast_possible_truncation, reason = "checked whole number")]
                        ValueType::I64 => Value::I64(q.value() as i64),
                        ValueType::F64 => Value::F64(q.value()),
                        _ => Value::Quantity(q),
                    })
                }
                ValueType::Point3 => self.point(ui, &key, id, row, context, label.id),
                _ => None,
            }
        })
        .inner
    }

    /// A point as three stacked length fields in the display unit of lengths; committing one axis
    /// returns the point with that axis replaced. A mixed or absent point is shown read-only.
    fn point(
        &mut self,
        ui: &mut Ui,
        key: &str,
        id: egui::Id,
        row: &PropertyRow,
        context: &GridContext<'_>,
        label: egui::Id,
    ) -> Option<Value> {
        let Shown::Value(Value::Point3(at)) = &row.shown else {
            let text = shown_text(&row.shown, context.localize);
            ui.add_enabled(false, TextEdit::singleline(&mut text.as_str()))
                .labelled_by(label);
            return None;
        };
        let mut field = QuantityField::new(
            Dimension::LENGTH,
            context.units.get(Dimension::LENGTH).copied(),
        );
        field.decimal_separator = context.decimal_separator;
        let mut coordinates = [at.x, at.y, at.z];
        // The axes are stacked, so a narrow grid still shows each field whole.
        let changed = ui
            .vertical(|ui| {
                let mut changed = false;
                for (axis, value) in ["x", "y", "z"].into_iter().zip(&mut coordinates) {
                    let input = self.numbers.entry(format!("{key}#{axis}")).or_default();
                    let shown = Quantity::new(*value, Dimension::LENGTH);
                    if let Some(q) = input.show(
                        ui,
                        id.with(axis),
                        &field,
                        Some(shown),
                        context.localize,
                        Some(label),
                    ) {
                        *value = q.value();
                        changed = true;
                    }
                }
                changed
            })
            .inner;
        let [x, y, z] = coordinates;
        changed.then_some(Value::Point3(arc_base::math::Point3::new(x, y, z)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_base::id::Identifier;
    use arc_doc::schema::EnumVariant;

    fn decls() -> Vec<FieldDecl> {
        vec![
            FieldDecl::new("width", ValueType::Quantity(Dimension::LENGTH)).in_aspect("geometry"),
            FieldDecl::new("name", ValueType::String).in_aspect("identity"),
            FieldDecl::new("visible", ValueType::Bool),
            FieldDecl::new(
                "kind",
                ValueType::Enum(vec![EnumVariant::new("a", 1), EnumVariant::new("b", 2)]),
            ),
            FieldDecl::new("owner", ValueType::Opaque),
        ]
    }

    #[test]
    fn mixed_values_markers_and_common_fields() {
        let d = decls();
        let mut fewer = decls();
        fewer.pop();
        let a = [
            Some(Value::Quantity(Quantity::new(1.0, Dimension::LENGTH))),
            Some(Value::String("x".into())),
            Some(Value::Bool(true)),
            None,
            Some(Value::Opaque(vec![1])),
        ];
        let mut b = a.clone();
        b[1] = Some(Value::String("y".into()));
        let ro = ["visible".to_owned()];
        let ov = ["width".to_owned()];
        let objects = [
            PropertyObject {
                id: ObjectId::from_u128(1),
                fields: &d,
                values: &a,
                read_only: &[],
                overridden: &ov,
            },
            PropertyObject {
                id: ObjectId::from_u128(2),
                fields: &fewer,
                values: &b,
                read_only: &ro,
                overridden: &[],
            },
        ];
        let model = PropertyModel::build(&objects).unwrap();
        let aspects: Vec<&str> = model.categories.iter().map(|c| c.aspect.as_str()).collect();
        assert_eq!(aspects, ["geometry", "identity", "data"]);
        let rows: Vec<&PropertyRow> = model.categories.iter().flat_map(|c| &c.rows).collect();
        assert_eq!(rows.len(), 4, "owner is not common");
        assert!(rows[0].overridden && !rows[0].read_only);
        assert_eq!(rows[1].shown, Shown::Mixed);
        assert!(rows[2].read_only);
        assert_eq!(rows[3].shown, Shown::Absent);
        let one = PropertyModel::build(&objects[..1]).unwrap();
        let owner = one
            .categories
            .iter()
            .flat_map(|c| &c.rows)
            .find(|r| r.name == "owner")
            .unwrap();
        assert!(owner.read_only, "no editor for opaque values");
        let intent = model.edit(
            "core.object.set_field",
            rows[0],
            &Value::Quantity(Quantity::new(2.0, Dimension::LENGTH)),
        );
        assert_eq!(intent.params["field"], "width");
        assert_eq!(intent.params["value"], 2.0);
        assert_eq!(intent.params["objects"].as_array().map(Vec::len), Some(2));
        assert_eq!(PropertyModel::build(&[]).unwrap(), PropertyModel::default());
    }

    #[test]
    fn oversized_selection_is_rejected() {
        let d = decls();
        let v: Vec<Option<Value>> = vec![None; d.len()];
        let one = PropertyObject {
            id: ObjectId::from_u128(1),
            fields: &d,
            values: &v,
            read_only: &[],
            overridden: &[],
        };
        let many = vec![one; MAX_COMPARISONS / d.len() + 1];
        assert_eq!(
            PropertyModel::build(&many).unwrap_err().code(),
            "ui.property.too_large"
        );
    }
}
