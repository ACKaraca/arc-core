//! Numeric and unit input (CORE-UI-011).
//!
//! A [`QuantityField`] reads typed text as an expression with units through `arc-solve`, where a
//! bare number takes the display unit, and shows values in display units through
//! [`DisplayFormat`]. Invalid text stays in the field with the error's localized message below
//! it, and nothing is returned for it, so the value is unchanged. A drag handle next to the
//! field scrubs the value and returns it once when the drag ends; with the handle focused the
//! Up and Down keys step it.

use arc_base::error::Value;
use arc_base::{Dimension, DisplayFormat, Error, Quantity, Result};
use egui::{Button, Id, Key, Modifiers, Role, Sense, TextEdit, Ui, WidgetInfo};

use crate::error::codes;
use crate::text::{Localize, messages, resolve};

/// How a numeric field reads and shows its value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantityField {
    /// The dimension of the value; [`Dimension::NONE`] for plain numbers.
    pub dimension: Dimension,
    /// The display format; `None` shows SI base units without a symbol and leaves bare
    /// numbers dimensionless.
    pub format: Option<DisplayFormat>,
    /// The decimal separator of the user's locale.
    pub decimal_separator: char,
    /// The inclusive range in SI base units.
    pub range: Option<(f64, f64)>,
    /// Only whole numbers (in SI base units) are accepted.
    pub integer: bool,
    /// The scrub step per logical point of drag and per arrow key, in the display unit.
    pub step: f64,
}

impl QuantityField {
    /// A field for `dimension` shown in `format`, with a step of one display unit.
    #[must_use]
    pub fn new(dimension: Dimension, format: Option<DisplayFormat>) -> Self {
        Self {
            dimension,
            format,
            decimal_separator: '.',
            range: None,
            integer: false,
            step: 1.0,
        }
    }

    /// Reads `text`.
    ///
    /// # Errors
    ///
    /// The errors of `arc_solve::parse_quantity`; `ui.quantity.out_of_range` outside the range;
    /// `ui.quantity.not_integer` for a fraction in an integer field.
    pub fn evaluate(&self, text: &str) -> Result<Quantity> {
        let mut context = arc_solve::Context::new();
        if let Some(format) = &self.format {
            context = context.with_bare_unit(format.unit);
        }
        let q = arc_solve::parse_quantity(text, &context, self.decimal_separator, self.dimension)?;
        self.admit(q)
    }

    fn admit(&self, q: Quantity) -> Result<Quantity> {
        if let Some((min, max)) = self.range
            && !(min..=max).contains(&q.value())
        {
            let min = self.display(Quantity::new(min, self.dimension));
            let max = self.display(Quantity::new(max, self.dimension));
            return Err(Error::new(codes::QUANTITY_OUT_OF_RANGE)
                .with_arg("min", min)
                .with_arg("max", max));
        }
        if self.integer && q.value().fract() != 0.0 {
            return Err(Error::new(codes::QUANTITY_NOT_INTEGER));
        }
        Ok(q)
    }

    /// The text of `q` in the display format, or its SI value when there is none or it does not
    /// fit.
    #[must_use]
    pub fn display(&self, q: Quantity) -> String {
        self.format
            .and_then(|f| f.format(q, self.decimal_separator).ok())
            .unwrap_or_else(|| {
                let text = q.value().to_string();
                if self.decimal_separator == '.' {
                    text
                } else {
                    text.replace('.', self.decimal_separator.encode_utf8(&mut [0; 4]))
                }
            })
    }

    /// `q` moved by `steps` scrub steps, rounded to a whole number in an integer field and
    /// clamped into the range; `None` when the result is not finite.
    #[must_use]
    pub fn scrub(&self, q: Quantity, steps: f64) -> Option<Quantity> {
        let unit = self.format.map(|f| f.unit);
        let per_step = unit.map_or(self.step, |u| u.to_si(self.step) - u.to_si(0.0));
        let mut value = q.value() + steps * per_step;
        if self.integer {
            value = value.round();
        }
        if let Some((min, max)) = self.range {
            value = value.clamp(min, max);
        }
        value
            .is_finite()
            .then(|| Quantity::new(value, self.dimension))
    }
}

/// The editing state of one field.
#[derive(Debug, Default)]
pub struct QuantityInput {
    /// The text in the field.
    pub text: String,
    editing: bool,
    scrubbed: Option<Quantity>,
    /// The error of the last rejected text.
    pub error: Option<Error>,
    /// The readable name of the field, which the error line names; empty for none.
    pub label: String,
}

/// The localized text of an error.
#[must_use]
pub fn error_text(error: &Error, localize: &dyn Localize) -> String {
    let args: Vec<(&str, Value)> = error
        .arguments()
        .iter()
        .map(|(k, v)| (*k, v.clone()))
        .collect();
    resolve(localize, error.message_id(), &args)
}

impl QuantityInput {
    /// What the line under the field says about `error`: for a value outside the range, the
    /// range and the field's name; for any text that does not read as a value of the field, an
    /// example of what is accepted, in the decimal separator and the display unit of the field.
    /// A whole-number error is its message.
    fn error_line(&self, error: &Error, field: &QuantityField, localize: &dyn Localize) -> String {
        let named = !self.label.is_empty();
        let mut args: Vec<(&str, Value)> = error
            .arguments()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        if named {
            args.push(("field", Value::Text(self.label.clone())));
        }
        let id = match (error.code(), named) {
            (c, true) if c == codes::QUANTITY_OUT_OF_RANGE.code => {
                messages::QUANTITY_RANGE_FIELD.message_id
            }
            (c, _) if c == codes::QUANTITY_OUT_OF_RANGE.code => error.message_id(),
            (c, _) if c == codes::QUANTITY_NOT_INTEGER.code => error.message_id(),
            (_, named) => {
                // A leading 3.5 shown in the field's own unit and separator is the example.
                let example = field.display(Quantity::new(3.5, field.dimension));
                args = vec![("example", Value::Text(example))];
                if named {
                    args.push(("field", Value::Text(self.label.clone())));
                    messages::QUANTITY_EXPECTED_FIELD.message_id
                } else {
                    messages::QUANTITY_EXPECTED.message_id
                }
            }
        };
        resolve(localize, id, &args)
    }

    /// Draws the scrub handle and the field for `value` (`None` shows the mixed-value text) and
    /// returns a new value when the user commits a valid, changed text (Enter or leaving the
    /// field) or ends a scrub. Escape restores the shown value. `label` names the field for
    /// assistive technology.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        id: Id,
        field: &QuantityField,
        value: Option<Quantity>,
        localize: &dyn Localize,
        label: Option<Id>,
    ) -> Option<Quantity> {
        let shown = match (self.scrubbed, value) {
            (Some(q), _) | (None, Some(q)) => field.display(q),
            (None, None) => messages::PROPERTY_MIXED.text(localize),
        };
        if !self.editing {
            self.text.clone_from(&shown);
        }
        let mut result = None;
        ui.horizontal(|ui| {
            let scrub_name = messages::QUANTITY_SCRUB.text(localize);
            let handle = ui.add(Button::new("↔").sense(Sense::click_and_drag()));
            handle.widget_info(|| WidgetInfo::labeled(Role::SpinButton, true, scrub_name.as_str()));
            if let Some(base) = self.scrubbed.or(value) {
                let factor = ui.input(|i| {
                    if i.modifiers.shift {
                        0.1
                    } else if i.modifiers.command {
                        10.0
                    } else {
                        1.0
                    }
                });
                if handle.dragged() {
                    let dx = f64::from(handle.drag_delta().x);
                    self.scrubbed = field.scrub(base, dx * factor).or(self.scrubbed);
                }
                if handle.has_focus() {
                    let (up, down) = ui.input_mut(|i| {
                        (
                            i.consume_key(Modifiers::NONE, Key::ArrowUp),
                            i.consume_key(Modifiers::NONE, Key::ArrowDown),
                        )
                    });
                    let steps = f64::from(u8::from(up)) - f64::from(u8::from(down));
                    if steps != 0.0 {
                        result = field.scrub(base, steps);
                    }
                }
            }
            if handle.drag_stopped() {
                result = self.scrubbed.take();
            }
            let mut edit = ui.add(TextEdit::singleline(&mut self.text).id(id));
            if let Some(label) = label {
                edit = edit.labelled_by(label);
            }
            if edit.gained_focus() {
                self.editing = true;
            }
            if edit.lost_focus() {
                // The Enter that commits the field must not also reach the command line.
                let entered = ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                // Escape ends the edit and is not also the command line's (which would clear the
                // selection).
                if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                    self.editing = false;
                    self.error = None;
                } else if self.text == shown && self.error.is_none() {
                    self.editing = false;
                } else {
                    match field.evaluate(&self.text) {
                        Ok(q) => {
                            self.editing = false;
                            self.error = None;
                            result = Some(q);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            // After Enter the text stays in the field, which keeps the focus
                            // for the correction: typing goes on here, not to the command line.
                            if entered {
                                edit.request_focus();
                            }
                        }
                    }
                }
            }
        });
        if let Some(error) = &self.error {
            let text = self.error_line(error, field, localize);
            ui.colored_label(ui.visuals().error_fg_color, text);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_base::Unit;
    use proptest::prelude::*;

    fn mm() -> QuantityField {
        let unit = *Unit::by_symbol("mm").unwrap();
        QuantityField::new(Dimension::LENGTH, Some(DisplayFormat::new(unit)))
    }

    #[test]
    fn expressions_with_units_evaluate_in_display_units() {
        let f = mm();
        let q = f.evaluate("1200 + 3 in").unwrap();
        assert!((q.value() - 1.2762).abs() < 1e-12);
        assert_eq!(f.display(q), "1276.2 mm");
        let mut comma = mm();
        comma.decimal_separator = ',';
        assert!((comma.evaluate("2,5 m").unwrap().value() - 2.5).abs() < 1e-12);
        assert_eq!(
            comma.display(Quantity::new(0.0015, Dimension::LENGTH)),
            "1,5 mm"
        );
    }

    #[test]
    fn invalid_input_reports_message_ids() {
        let mut f = mm();
        assert_eq!(
            f.evaluate("2 kg").unwrap_err().code(),
            "solve.expr.dimension_mismatch"
        );
        assert!(f.evaluate("2 +").unwrap_err().code().starts_with("solve."));
        assert!(f.evaluate("").is_err());
        f.range = Some((0.0, 1.0));
        let e = f.evaluate("2 m").unwrap_err();
        assert_eq!(e.code(), "ui.quantity.out_of_range");
        assert_eq!(e.message_id(), "ui-quantity-out-of-range");
        let mut n = QuantityField::new(Dimension::NONE, None);
        n.integer = true;
        assert_eq!(
            n.evaluate("7 / 2").unwrap_err().code(),
            "ui.quantity.not_integer"
        );
        assert_eq!(n.evaluate("7 * 2").unwrap().value(), 14.0);
    }

    #[test]
    fn scrub_steps_in_display_units_and_clamps() {
        let mut f = mm();
        f.range = Some((0.0, 0.01));
        let q = Quantity::new(0.005, Dimension::LENGTH);
        assert!((f.scrub(q, 2.0).unwrap().value() - 0.007).abs() < 1e-12);
        assert_eq!(f.scrub(q, 1e9).unwrap().value(), 0.01);
        assert_eq!(f.scrub(q, f64::NAN), None);
    }

    proptest! {
        #[test]
        fn displayed_text_reads_back_and_extremes_never_panic(
            mantissa in -9.999f64..9.999, exponent in -300i32..300,
        ) {
            let f = QuantityField::new(Dimension::NONE, None);
            let q = Quantity::new(mantissa * 10f64.powi(exponent), Dimension::NONE);
            let text = f.display(q);
            if let Ok(back) = f.evaluate(&text) {
                prop_assert_eq!(back.value(), q.value());
            }
            let _ = f.scrub(q, 1e300);
            let _ = mm().evaluate(&format!("{text} m * 1e300"));
        }
    }
}
