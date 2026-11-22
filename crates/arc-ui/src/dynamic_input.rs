//! Dynamic input near the cursor (CORE-UI-009).
//!
//! During point input the panel floats beside the pointer and shows the prompt, a distance
//! field in meters and an angle field in degrees. The fields are measured from the last point.
//!
//! Typing in a field locks it: a locked field emits what was typed. An unlocked field follows
//! the values the caller derives from the pointer ([`Relative`]), and emits those exact values
//! (not their displayed text). Tab moves between the fields, Enter emits the two values as
//! point input, and the panel stays inside the viewport rectangle: it flips to the other side of
//! the pointer near an edge and is clamped into the viewport when neither side fits.
//!
//! The widget emits values. It never computes geometry and never runs a command.

use egui::{Area, Context, Frame, Id, Key, Modifiers, Order, Pos2, Rect, Role, Vec2, WidgetInfo};

use crate::text::{Localize, messages, resolve};

/// The gap between the pointer and the panel, in logical points.
pub const POINTER_GAP: f32 = 16.0;

/// The size assumed for the panel before it has been drawn once.
const ESTIMATED_SIZE: Vec2 = Vec2::new(220.0, 96.0);

/// A point relative to the last point: a distance in meters and an angle in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relative {
    /// The distance from the last point, in meters.
    pub distance: f64,
    /// The angle from the x axis, in radians.
    pub angle: f64,
}

/// One field: its text and whether typing locked it.
#[derive(Debug, Clone, Default)]
struct Field {
    text: String,
    locked: bool,
}

/// The dynamic input's state: its two fields, the focused one and the panel's last size.
#[derive(Debug, Clone)]
pub struct DynamicInput {
    fields: [Field; 2],
    focus: usize,
    /// The distance field takes focus once, when the input starts.
    focus_pending: bool,
    size: Vec2,
}

impl Default for DynamicInput {
    fn default() -> Self {
        Self {
            fields: [Field::default(), Field::default()],
            focus: 0,
            focus_pending: true,
            size: ESTIMATED_SIZE,
        }
    }
}

/// The place of a panel of `size` next to `pointer`: right and below the pointer, flipped to the
/// left or upward when it would leave `viewport`, then clamped into `viewport`.
#[must_use]
pub fn place(pointer: Pos2, size: Vec2, viewport: Rect) -> Pos2 {
    let mut x = pointer.x + POINTER_GAP;
    if x + size.x > viewport.right() {
        x = pointer.x - POINTER_GAP - size.x;
    }
    let mut y = pointer.y + POINTER_GAP;
    if y + size.y > viewport.bottom() {
        y = pointer.y - POINTER_GAP - size.y;
    }
    Pos2::new(
        clamp_into(x, viewport.left(), viewport.right(), size.x),
        clamp_into(y, viewport.top(), viewport.bottom(), size.y),
    )
}

/// `value` moved into `[low, high - length]`, or to `low` when the span is shorter than `length`.
fn clamp_into(value: f32, low: f32, high: f32, length: f32) -> f32 {
    let top = (high - length).max(low);
    value.clamp(low, top)
}

/// Parses a number typed in a field: a decimal point or comma, finite values only.
#[must_use]
pub fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', ".");
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The text a derived value shows in an unlocked field.
fn shown(distance: f64, angle_degrees: f64) -> [String; 2] {
    [format!("{distance:.3}"), format!("{angle_degrees:.2}")]
}

impl DynamicInput {
    /// Draws the panel for `prompt` (a message identifier) beside `pointer`, inside `viewport`,
    /// with `derived` as the pointer's distance and angle. Returns the point input when Enter is
    /// pressed in a field: each locked field's typed value and each unlocked field's derived
    /// value. Returns `None` when Enter gives no complete value.
    pub fn show(
        &mut self,
        ctx: &Context,
        localize: &dyn Localize,
        prompt: &str,
        viewport: Rect,
        pointer: Pos2,
        derived: Option<Relative>,
    ) -> Option<Relative> {
        let ids = [field_id(0), field_id(1)];
        let anything_focused = ctx.memory(|m| m.focused().is_some());
        let ours = ctx.memory(|m| ids.iter().any(|id| m.has_focus(*id)));
        let focused_id = ids.get(self.focus).copied().unwrap_or(ids[0]);
        // Tab moves between the fields through egui's own focus order, which needs the fields'
        // focus to be set by egui; requesting focus on every frame would break that order.
        if self.focus_pending && !anything_focused {
            ctx.memory_mut(|m| m.request_focus(focused_id));
        }
        // Enter is read before the fields draw, so a value typed in the same frame is not lost.
        let enter = ours && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));

        // The values are those of the fields before this frame draws, the state Enter refers to.
        let emitted = if enter { self.values(derived) } else { None };
        let mut focus_index = self.focus;
        let position = place(pointer, self.size, viewport);
        let response = Area::new(Id::unique("arc-dynamic-input"))
            .order(Order::Foreground)
            .fixed_pos(position)
            .show(ctx, |ui| {
                Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(resolve(localize, prompt, &[]));
                    let texts = derived.map(|d| shown(d.distance, d.angle.to_degrees()));
                    let labels = [messages::DYNAMIC_DISTANCE, messages::DYNAMIC_ANGLE];
                    for (index, field) in self.fields.iter_mut().enumerate() {
                        let Some(label_message) = labels.get(index) else {
                            continue;
                        };
                        if !field.locked
                            && let Some(text) = texts.as_ref().and_then(|t| t.get(index))
                        {
                            field.text.clone_from(text);
                        }
                        let id = ids.get(index).copied().unwrap_or(focused_id);
                        ui.horizontal(|ui| {
                            let label = ui.label(label_message.text(localize));
                            let edit = ui
                                .add(
                                    egui::TextEdit::singleline(&mut field.text)
                                        .id(id)
                                        .desired_width(90.0),
                                )
                                .labelled_by(label.id);
                            if edit.has_focus() {
                                focus_index = index;
                            }
                            if edit.changed() {
                                field.locked = !field.text.trim().is_empty();
                            }
                        });
                    }
                });
            })
            .response;
        self.focus = focus_index;
        if ours || ctx.memory(|m| ids.iter().any(|id| m.has_focus(*id))) {
            self.focus_pending = false;
        }
        self.size = response.rect.size();
        let name = messages::DYNAMIC_INPUT.text(localize);
        response.widget_info(|| WidgetInfo::labeled(Role::Group, true, name.as_str()));
        emitted
    }

    /// The point input from the fields: a locked field gives its typed value, an unlocked one the
    /// derived value. `None` when a needed value is missing or not a number.
    fn values(&self, derived: Option<Relative>) -> Option<Relative> {
        let [distance_field, angle_field] = &self.fields;
        let distance = if distance_field.locked {
            parse_number(&distance_field.text)?
        } else {
            derived?.distance
        };
        let angle = if angle_field.locked {
            parse_number(&angle_field.text)?.to_radians()
        } else {
            derived?.angle
        };
        Some(Relative { distance, angle })
    }

    /// The index of the focused field: 0 for distance, 1 for angle.
    #[must_use]
    pub fn focused_field(&self) -> usize {
        self.focus
    }

    /// Whether field `index` (0 distance, 1 angle) is locked by typing.
    #[must_use]
    pub fn is_locked(&self, index: usize) -> bool {
        self.fields.get(index).is_some_and(|f| f.locked)
    }
}

/// Most fields a product's dynamic input shows; more are not drawn.
pub const MAX_FIELDS: usize = 8;

/// A field a product supplies to the dynamic input. The product owns the text and the lock: the
/// widget shows what it is given and reports what the user does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DynamicField {
    /// The label's message identifier.
    pub label: String,
    /// The text the field shows.
    pub text: String,
    /// Whether the product holds the field's value against the pointer (a typed value).
    pub locked: bool,
    /// Whether the field takes the keyboard focus this frame.
    pub focused: bool,
    /// Whether the text is selected when the field gains the focus, so typing replaces it.
    pub select_all: bool,
    /// Whether Escape in the field is consumed and reported as [`DynamicEvent::Cancel`]. Off,
    /// Escape is left to the shell.
    pub escape_cancels: bool,
}

/// What the user did in the dynamic input, for the product to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DynamicEvent {
    /// The text of field `index` changed to `text`.
    Edited {
        /// The field.
        index: usize,
        /// The new text.
        text: String,
    },
    /// Escape in field `index`, which asked for it with [`DynamicField::escape_cancels`].
    Cancel {
        /// The field the key was pressed in.
        index: usize,
    },
    /// Tab in field `index`: the product moves the focus.
    Tab {
        /// The field the key was pressed in.
        index: usize,
    },
    /// Enter in field `index`: the product takes the point input.
    Enter {
        /// The field the key was pressed in.
        index: usize,
    },
}

/// Draws the product's dynamic input `fields` in a panel beside `pointer`, inside `viewport`,
/// under the prompt message `prompt`, and returns what the user did this frame. The panel keeps
/// the size it last had, so it flips to the other side of the pointer at an edge of `viewport`
/// using that size. Tab and Enter are consumed while a field has the focus; Tab does not move the
/// focus here, the product sets it through [`DynamicField::focused`]. At most [`MAX_FIELDS`]
/// fields are drawn.
pub fn show_fields(
    ctx: &Context,
    localize: &dyn Localize,
    prompt: &str,
    viewport: Rect,
    pointer: Pos2,
    fields: &[DynamicField],
) -> Vec<DynamicEvent> {
    let fields = &fields[..fields.len().min(MAX_FIELDS)];
    let ids: Vec<Id> = (0..fields.len()).map(field_id).collect();
    let size_id = Id::unique("arc-dynamic-input-fields-size");
    let size = ctx
        .data(|d| d.get_temp::<Vec2>(size_id))
        .unwrap_or(ESTIMATED_SIZE);
    let mut events = Vec::new();
    let ours = ctx.memory(|m| ids.iter().any(|id| m.has_focus(*id)));
    let mut keys = Vec::new();
    if ours {
        let focused = ctx.memory(|m| ids.iter().position(|id| m.has_focus(*id)));
        if let Some(index) = focused {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) {
                keys.push(DynamicEvent::Tab { index });
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                keys.push(DynamicEvent::Enter { index });
            }
        }
    }
    // egui drops the focus at the start of a pass in which Escape is pressed, so the field that
    // had it in the previous frame is the one Escape was pressed in.
    let focus_id = Id::unique("arc-dynamic-input-fields-focus");
    let was = ctx.data(|d| d.get_temp::<usize>(focus_id));
    if let Some(index) = was
        && fields.get(index).is_some_and(|f| f.escape_cancels)
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))
    {
        keys.push(DynamicEvent::Cancel { index });
    }
    let position = place(pointer, size, viewport);
    let response = Area::new(Id::unique("arc-dynamic-input-fields"))
        .order(Order::Foreground)
        .fixed_pos(position)
        .show(ctx, |ui| {
            Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(resolve(localize, prompt, &[]));
                for (index, field) in fields.iter().enumerate() {
                    let mut text = field.text.clone();
                    let label_text = resolve(localize, &field.label, &[]);
                    ui.horizontal(|ui| {
                        let label = ui.label(label_text);
                        let edit = ui
                            .add(
                                egui::TextEdit::singleline(&mut text)
                                    .id(field_id(index))
                                    .desired_width(90.0),
                            )
                            .labelled_by(label.id);
                        if field.focused && !edit.has_focus() {
                            edit.request_focus();
                        }
                        if field.select_all && edit.gained_focus() {
                            let mut state = egui::TextEdit::load_state(ui.ctx(), field_id(index))
                                .unwrap_or_default();
                            let end = egui::text::CCursor::new(text.chars().count());
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    end,
                                )));
                            state.store(ui.ctx(), field_id(index));
                        }
                        if edit.changed() {
                            events.push(DynamicEvent::Edited { index, text });
                        }
                    });
                }
            });
        })
        .response;
    ctx.data_mut(|d| d.insert_temp(size_id, response.rect.size()));
    let now = ctx.memory(|m| ids.iter().position(|id| m.has_focus(*id)));
    ctx.data_mut(|d| {
        if let Some(index) = now {
            d.insert_temp(focus_id, index);
        } else {
            d.remove::<usize>(focus_id);
        }
    });
    let name = messages::DYNAMIC_INPUT.text(localize);
    response.widget_info(|| WidgetInfo::labeled(Role::Group, true, name.as_str()));
    keys.extend(events);
    keys
}

fn field_id(index: usize) -> Id {
    Id::unique(("arc-dynamic-input-field", index))
}
