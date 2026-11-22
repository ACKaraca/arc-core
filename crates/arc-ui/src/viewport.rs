//! The viewport widget (CORE-UI-005, CORE-UI-027 for the viewport).
//!
//! [`ViewportWidget::show`] allocates the available space, paints the texture the caller
//! registered for the current frame, turns egui input into [`ViewEvent`]s for the
//! [`Gestures`] state machine and returns a [`ViewportOutput`]: the pixel size it wants, the
//! camera to send to the `arc_gpu::Viewport`, the pre-selected object, the rubber-band
//! overlay, at most one selection intent per gesture and prompt answers. It never changes the
//! selection or runs a command.
//!
//! Registering the `arc_gpu::Frame` texture with the egui renderer belongs to the
//! application's integration, so this crate does not depend on `egui-wgpu`.
//!
//! Several viewports of one document each have a widget. Their cameras are independent, or
//! follow each other through a shared [`ViewLink`]: a camera change in one linked widget is
//! published and every other linked widget returns it as its camera on its next frame.

use std::sync::{Arc, Mutex, PoisonError};

use arc_base::error::Value;
use arc_base::math::Point3;
use arc_base::{Color, Error, ObjectId, Result, Tolerance};
use arc_command::prompt::{Prompt, PromptInput};
use arc_geom::view::ViewFrame;
use arc_gpu::{
    Anchor, Overlay, OverlayPrimitive, PickHit, PickRegion, RenderSettings, Renderer, Scene,
};
use egui::{
    Color32, Event, Modifiers, MouseWheelUnit, Pos2, Rect, Response, Sense, TextureId, Ui,
    WidgetInfo,
};

use crate::command::Intent;
use crate::command_line::prompt_text;
use crate::dynamic_input::{self, DynamicEvent, DynamicField};
use crate::gesture::{
    Band, BandKind, DRAG_THRESHOLD, Gestures, View, ViewEvent, ViewPicker, ViewportCommands,
};
use crate::hooks::{
    PointerEvent, PointerHandler, PointerKind, ProductOverlay, ViewProjection, WorkPlane,
    paint_overlays,
};
use crate::text::{Localize, messages, resolve};
use crate::theme::{Colors, UiScale};

/// Points per wheel line when the platform reports lines.
const POINTS_PER_LINE: f64 = 40.0;
/// Dash and gap lengths in points of the crossing band outline.
const DASH: f64 = 6.0;
const GAP: f64 = 4.0;
/// The most dashes per band edge.
const MAX_DASHES: f64 = 256.0;
/// The alpha of band fills.
const BAND_FILL_ALPHA: u8 = 40;

/// A camera shared by linked viewports. Clones share it.
#[derive(Debug, Clone, Default)]
pub struct ViewLink {
    inner: Arc<Mutex<(u64, Option<ViewFrame>)>>,
}

impl ViewLink {
    /// A link with no camera yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes `frame`; returns its version.
    pub fn publish(&self, frame: ViewFrame) -> u64 {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard.0 = guard.0.wrapping_add(1);
        guard.1 = Some(frame);
        guard.0
    }

    /// The latest camera and its version.
    #[must_use]
    pub fn latest(&self) -> Option<(u64, ViewFrame)> {
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        guard.1.map(|f| (guard.0, f))
    }
}

/// Picking through an `arc-gpu` renderer and the scene the viewport shows.
pub struct ScenePicker<'a> {
    /// The renderer.
    pub renderer: &'a mut Renderer,
    /// The scene.
    pub scene: &'a mut Scene,
    /// The render settings of the viewport (shading mode, back-face culling).
    pub settings: &'a RenderSettings,
}

impl ViewPicker for ScenePicker<'_> {
    fn pick(
        &mut self,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        region: &PickRegion,
    ) -> Result<Vec<PickHit>> {
        self.renderer
            .pick(self.scene, frame, width, height, self.settings, region)
    }

    fn bounds(&mut self) -> Option<(Point3, f64)> {
        self.scene.bounding_sphere(None)
    }
}

/// A dynamic input panel a product asks for beside the pointer.
#[derive(Debug, Clone, Copy)]
pub struct DynamicPanel<'a> {
    /// The prompt's message identifier, shown above the fields.
    pub prompt: &'a str,
    /// The fields, in order.
    pub fields: &'a [DynamicField],
}

/// What a product gives the widget for one frame; see [`crate::hooks`].
pub struct ProductInput<'a> {
    /// The handler offered each pointer event before the widget's own handling.
    pub pointer: Option<&'a mut dyn PointerHandler>,
    /// Overlays painted over the scene in model coordinates, in order.
    pub overlays: &'a [ProductOverlay],
    /// The dynamic input panel, when the product asks for point input.
    pub dynamic: Option<DynamicPanel<'a>>,
    /// The plane clicks answer point prompts on and [`PointerEvent::model`] lies on; `None` is
    /// the plane through the camera target that faces the eye.
    pub work_plane: Option<WorkPlane>,
}

/// What one frame of the widget reads.
pub struct ViewportInput<'a> {
    /// The texture of the latest rendered frame, registered by the caller; `None` paints the
    /// viewport background only.
    pub texture: Option<TextureId>,
    /// The current camera.
    pub frame: &'a ViewFrame,
    /// The active prompt, when a command is prompting.
    pub prompt: Option<&'a Prompt>,
    /// The number of selected objects.
    pub selected: usize,
    /// The picking interface.
    pub picker: &'a mut dyn ViewPicker,
    /// The commands intents name.
    pub commands: &'a ViewportCommands,
    /// The message source.
    pub localize: &'a dyn Localize,
    /// The theme colors (viewport background and band colors).
    pub colors: &'a Colors,
    /// The UI scale; with the pixels-per-point factor it scales the gesture thresholds.
    pub scale: UiScale,
    /// The document tolerance.
    pub tolerance: &'a Tolerance,
    /// The view's name (user data, such as a named view), part of the accessible name.
    pub name: &'a str,
    /// The caller's clock in seconds.
    pub time: f64,
    /// The product's pointer handler, overlays and dynamic input; `None` for none of them.
    pub product: Option<ProductInput<'a>>,
}

/// What one frame of the widget produced.
#[derive(Debug)]
pub struct ViewportOutput {
    /// The response of the viewport area.
    pub response: Response,
    /// The image size in pixels the viewport wants; resize the `arc_gpu::Viewport` to it.
    pub size: [u32; 2],
    /// The new camera for the `arc_gpu::Viewport`, when it changed.
    pub camera: Option<ViewFrame>,
    /// The pre-selected object, for `arc_gpu::Viewport::set_highlight`.
    pub hover: Option<ObjectId>,
    /// The overlay of the rubber band (empty without one), for `set_overlay`.
    pub overlay: Overlay,
    /// The selection intent of a finished gesture.
    pub intent: Option<Intent>,
    /// An answer for the active prompt (`CommandLine::answer`).
    pub answer: Option<PromptInput>,
    /// Picking and navigation errors of this frame.
    pub errors: Vec<Error>,
    /// The projection of the camera after this frame, for the product's next frame.
    pub projection: ViewProjection,
    /// What the user did in the dynamic input panel this frame.
    pub dynamic: Vec<DynamicEvent>,
    /// The model point under the pointer, when it is over the viewport.
    pub pointer_model: Option<Point3>,
    /// Escape ended a press the product had claimed this frame; the product cancels its gesture.
    pub claim_cancelled: bool,
}

/// What the viewport draws under the scene and over it: the grid, the axes with the origin
/// marker, and the crosshair that replaces the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportDisplay {
    /// Draws the grid on the plane `z = 0`.
    pub grid: bool,
    /// Draws the X and Y axes and the origin marker.
    pub axes: bool,
    /// The grid spacing in model units; `None` picks a spacing that keeps lines about
    /// [`MIN_GRID_PIXELS`] points apart.
    pub spacing: Option<f64>,
}

impl Default for ViewportDisplay {
    fn default() -> Self {
        Self {
            grid: true,
            axes: true,
            spacing: None,
        }
    }
}

/// The least distance between neighboring grid lines of the automatic spacing, in points.
pub const MIN_GRID_PIXELS: f64 = 14.0;
/// The most grid lines drawn per direction.
const MAX_GRID_LINES: i64 = 400;
/// Every this many grid lines is a stronger one.
const MAJOR_EVERY: i64 = 5;

/// The state of one viewport widget.
#[derive(Debug, Clone, Default)]
pub struct ViewportWidget {
    /// What is drawn besides the scene.
    pub display: ViewportDisplay,
    /// A rectangle (the view cube) whose pointer presses and wheel turns belong to another
    /// widget: the viewport ignores them there.
    pub blocked: Option<Rect>,
    /// The widget identifier of the viewport as of its last frame, so a host can tell whether
    /// the viewport holds keyboard focus.
    pub id: Option<egui::Id>,
    gestures: Gestures,
    link: Option<ViewLink>,
    seen: u64,
    /// The last press: its button and pixel position, for click detection.
    press_start: Option<(egui::PointerButton, [f64; 2])>,
    /// The button a product consumed the press of; its events are the product's until release.
    product_press: Option<egui::PointerButton>,
    /// Escape ended a claimed press since the last frame's output.
    claim_cancelled: bool,
}

impl ViewportWidget {
    /// A widget with an independent camera.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A widget whose camera follows `link`.
    #[must_use]
    pub fn linked(link: ViewLink) -> Self {
        Self {
            link: Some(link),
            ..Self::default()
        }
    }

    /// Whether the product holds a claimed press (a drag it owns) right now.
    #[must_use]
    pub fn holds_product_press(&self) -> bool {
        self.product_press.is_some()
    }

    /// The gesture state.
    #[must_use]
    pub fn gestures(&self) -> &Gestures {
        &self.gestures
    }

    /// Draws the viewport in the available space and handles its input.
    ///
    /// Each pointer event first goes to the product's [`PointerHandler`] (see [`crate::hooks`]).
    /// A consumed event is not handled by the widget. A press the product consumes belongs to the
    /// product until its release. A primary click the product consumes ends the widget's press
    /// without an answer.
    #[allow(clippy::too_many_lines, reason = "one frame of the widget")]
    pub fn show(&mut self, ui: &mut Ui, input: ViewportInput<'_>) -> ViewportOutput {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.id = Some(response.id);
        let ppp = f64::from(ui.ctx().pixels_per_point());
        let scale = ppp * f64::from(input.scale.factor());
        let size = pixel_size(rect, ppp);
        self.gestures.set_scale(scale);

        let mut frame = *input.frame;
        let mut camera = None;
        if let Some((version, linked)) = self.link.as_ref().and_then(ViewLink::latest)
            && version != self.seen
        {
            self.seen = version;
            if linked != frame {
                frame = linked;
                camera = Some(linked);
            }
        }

        let events = collect_events(
            ui,
            &response,
            rect,
            self.blocked,
            ppp,
            input.time,
            self.gestures.is_pressed(),
        );
        if events
            .iter()
            .any(|e| matches!(e, ViewEvent::Pressed { .. }))
        {
            response.request_focus();
        }

        let (prompt, selected, tolerance, name) =
            (input.prompt, input.selected, input.tolerance, input.name);
        let product = input.product;
        let (overlays, dynamic) = match &product {
            Some(p) => (p.overlays, p.dynamic),
            None => (&[][..], None),
        };
        let work_plane = product.as_ref().and_then(|p| p.work_plane);
        let mut handler = product.and_then(|p| p.pointer);
        let modifiers = ui.input(|i| i.modifiers);
        let mut view = View {
            frame,
            width: size[0],
            height: size[1],
            prompt,
            selected,
            tolerance,
            picker: input.picker,
            commands: input.commands,
            work_plane,
        };
        let mut out = ViewportOutput {
            response: response.clone(),
            size,
            camera,
            hover: None,
            overlay: Overlay::new(),
            intent: None,
            answer: None,
            errors: Vec::new(),
            projection: ViewProjection::new(frame, size[0], size[1]).with_work_plane(work_plane),
            dynamic: Vec::new(),
            pointer_model: None,
            claim_cancelled: false,
        };
        let moved_here = self.route(&mut view, &events, &mut handler, modifiers, scale, &mut out);
        if moved_here && let Some(link) = &self.link {
            self.seen = link.publish(view.frame);
        }
        out.claim_cancelled = std::mem::take(&mut self.claim_cancelled);
        out.hover = self.gestures.hover();
        out.projection =
            ViewProjection::new(view.frame, size[0], size[1]).with_work_plane(work_plane);
        if let Some(band) = self.gestures.band() {
            out.overlay = band_overlay(&band, input.colors, scale);
        }

        out.pointer_model = pointer_model(ui, &response, rect, ppp, &out.projection);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, input.colors.viewport);
        paint_grid(
            &painter,
            rect,
            ppp,
            &out.projection,
            input.colors,
            &self.display,
        );
        paint_texture(&painter, rect, input.texture);
        if !overlays.is_empty() {
            paint_overlays(
                ui,
                input.localize,
                input.colors,
                rect,
                ppp,
                &out.projection,
                overlays,
            );
        }
        if let Some(panel) = dynamic {
            let pointer = ui
                .input(|i| i.pointer.latest_pos())
                .unwrap_or(rect.center());
            out.dynamic = dynamic_input::show_fields(
                ui.ctx(),
                input.localize,
                panel.prompt,
                rect,
                pointer,
                panel.fields,
            );
        }

        announce(ui, &response, input.localize, name, selected, prompt);
        out
    }

    /// Hands each event to the product first and the rest to the gestures, which may move the
    /// camera and produce an intent, an answer or errors. Returns whether the camera moved.
    fn route(
        &mut self,
        view: &mut View<'_>,
        events: &[ViewEvent],
        handler: &mut Option<&mut dyn PointerHandler>,
        modifiers: Modifiers,
        scale: f64,
        out: &mut ViewportOutput,
    ) -> bool {
        let mut moved_here = false;
        for event in events {
            let projection = ViewProjection::new(view.frame, view.width, view.height)
                .with_work_plane(view.work_plane);
            if self.offer(handler, &projection, modifiers, event, scale) {
                if matches!(event, ViewEvent::Moved(_)) {
                    // A consumed move still clears the widget's pre-selection.
                    self.gestures.handle(view, &ViewEvent::Left);
                }
                continue;
            }
            let r = self.gestures.handle(view, event);
            if let Some(c) = r.camera {
                out.camera = Some(c);
                moved_here = true;
            }
            // Gestures end on a release, so two intents in one frame come only from two
            // gestures; the later one wins.
            out.intent = r.intent.or(out.intent.take());
            out.answer = r.answer.or(out.answer.take());
            out.errors.extend(r.error);
        }
        moved_here
    }

    /// Offers one event to the product's handler, and tracks the product's and the widget's
    /// presses. Returns `true` when the event is consumed: the widget then does not handle it.
    fn offer(
        &mut self,
        handler: &mut Option<&mut dyn PointerHandler>,
        projection: &ViewProjection,
        modifiers: Modifiers,
        event: &ViewEvent,
        scale: f64,
    ) -> bool {
        let press = self.press_start;
        let mut send = |kind: PointerKind, button: Option<egui::PointerButton>, at: [f64; 2]| {
            let Some(h) = handler.as_mut() else {
                return false;
            };
            h.pointer(&PointerEvent {
                kind,
                button,
                modifiers,
                pixel: at,
                model: projection.pixel_to_model(at),
                pixels_per_unit: projection.pixels_per_unit(),
            })
        };
        match *event {
            ViewEvent::Moved(at) if !self.gestures.is_pressed() => {
                send(PointerKind::Move, None, at)
            }
            ViewEvent::Pressed { at, button, .. } => {
                self.press_start = Some((button, at));
                if self.gestures.is_pressed() {
                    return false;
                }
                let consumed = send(PointerKind::Press, Some(button), at);
                if consumed {
                    self.product_press = Some(button);
                }
                consumed
            }
            ViewEvent::Released { at, button } if self.product_press == Some(button) => {
                self.product_press = None;
                send(PointerKind::Release, Some(button), at);
                if is_click(press, button, at, DRAG_THRESHOLD * scale) {
                    send(PointerKind::Click, Some(button), at);
                }
                true
            }
            ViewEvent::Key {
                key: egui::Key::Escape,
                ..
            } if self.product_press.is_some() => {
                // Escape ends the product's press: the widget neither clears the selection nor
                // answers the release, and the product learns of it as the shell's cancel.
                self.product_press = None;
                self.claim_cancelled = true;
                true
            }
            ViewEvent::Released { at, button } if self.gestures.is_pressed() => {
                // The widget owns this press: the product sees the release, and may consume the
                // click, which ends the press without the widget's answer.
                send(PointerKind::Release, Some(button), at);
                let clicked = self.gestures.band().is_none()
                    && is_click(press, button, at, DRAG_THRESHOLD * scale);
                if clicked && send(PointerKind::Click, Some(button), at) {
                    self.gestures.cancel_press();
                    return true;
                }
                false
            }
            _ => false,
        }
    }
}

/// Whether a release at `at` of `button` ends a click that started at `start` within `threshold`
/// pixels.
fn is_click(
    start: Option<(egui::PointerButton, [f64; 2])>,
    button: egui::PointerButton,
    at: [f64; 2],
    threshold: f64,
) -> bool {
    match start {
        Some((pressed, from)) if pressed == button && button == egui::PointerButton::Primary => {
            (from[0] - at[0]).hypot(from[1] - at[1]) <= threshold
        }
        _ => false,
    }
}

/// The model point under the pointer, when it is over the viewport `rect`.
fn pointer_model(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    ppp: f64,
    projection: &ViewProjection,
) -> Option<Point3> {
    let p = ui
        .input(|i| i.pointer.latest_pos())
        .filter(|p| rect.contains(*p) && response.contains_pointer())?;
    projection.pixel_to_model([
        f64::from(p.x - rect.min.x) * ppp,
        f64::from(p.y - rect.min.y) * ppp,
    ])
}

/// The spacing of the automatic grid: 1, 2 or 5 times a power of ten of model units, the least
/// one that is at least [`MIN_GRID_PIXELS`] points wide at `points_per_unit`.
#[must_use]
pub fn grid_spacing(points_per_unit: f64) -> Option<f64> {
    if !points_per_unit.is_finite() || points_per_unit <= 0.0 {
        return None;
    }
    let least = MIN_GRID_PIXELS / points_per_unit;
    let decade = 10f64.powf(least.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * decade)
        .find(|s| *s >= least)
}

/// A color with its alpha set.
fn faint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// Paints the grid on the plane `z = 0`, the axes and the origin marker under the scene.
#[allow(
    clippy::cast_possible_truncation,
    reason = "grid indices are bounded by MAX_GRID_LINES"
)]
fn paint_grid(
    painter: &egui::Painter,
    rect: Rect,
    ppp: f64,
    projection: &ViewProjection,
    colors: &Colors,
    display: &ViewportDisplay,
) {
    if !display.grid && !display.axes {
        return;
    }
    let [w, h] = projection.size().map(f64::from);
    let corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|c| projection.pixel_to_model(c));
    let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for c in corners {
        let Some(c) = c else {
            return;
        };
        low = [low[0].min(c.x), low[1].min(c.y)];
        high = [high[0].max(c.x), high[1].max(c.y)];
    }
    let Some(ppu) = projection.pixels_per_unit().map(|p| p / ppp) else {
        return;
    };
    let to_screen = |x: f64, y: f64| {
        projection.model_to_pixel(&Point3::new(x, y, 0.0)).map(|p| {
            Pos2::new(
                rect.min.x + (p[0] / ppp) as f32,
                rect.min.y + (p[1] / ppp) as f32,
            )
        })
    };
    let line = |a: (f64, f64), b: (f64, f64), stroke: egui::Stroke| {
        if let (Some(a), Some(b)) = (to_screen(a.0, a.1), to_screen(b.0, b.1)) {
            painter.line_segment([a, b], stroke);
        }
    };
    if display.grid
        && let Some(step) = display
            .spacing
            .filter(|s| *s > 0.0 && *s * ppu >= MIN_GRID_PIXELS / 4.0)
            .or_else(|| grid_spacing(ppu))
    {
        let minor = egui::Stroke::new(1.0, faint(colors.text_muted, 38));
        let major = egui::Stroke::new(1.0, faint(colors.text_muted, 80));
        for axis in 0..2 {
            let first = (low[axis] / step).floor() as i64;
            let last = (high[axis] / step).ceil() as i64;
            if last.saturating_sub(first) > MAX_GRID_LINES {
                continue;
            }
            for n in first..=last {
                let at = n as f64 * step;
                let stroke = if n % MAJOR_EVERY == 0 { major } else { minor };
                if axis == 0 {
                    line((at, low[1]), (at, high[1]), stroke);
                } else {
                    line((low[0], at), (high[0], at), stroke);
                }
            }
        }
    }
    if display.axes {
        line(
            (low[0], 0.0),
            (high[0], 0.0),
            egui::Stroke::new(1.5, faint(colors.error, 210)),
        );
        line(
            (0.0, low[1]),
            (0.0, high[1]),
            egui::Stroke::new(1.5, faint(colors.success, 210)),
        );
        if let Some(origin) = to_screen(0.0, 0.0) {
            painter.circle_stroke(origin, 6.0, egui::Stroke::new(1.5, colors.text));
            painter.circle_filled(origin, 1.5, colors.text);
        }
    }
}

/// Paints the crosshair at `pointer`: two lines across `rect` with a small gap at the pointer,
/// in the text color. The pointer itself is hidden while it is over the viewport.
pub fn paint_crosshair(ui: &Ui, rect: Rect, colors: &Colors, pointer: Pos2) {
    let painter = ui.painter_at(rect);
    let stroke = egui::Stroke::new(1.0, faint(colors.text, 170));
    let gap = 5.0;
    painter.line_segment(
        [
            Pos2::new(rect.left(), pointer.y),
            Pos2::new(pointer.x - gap, pointer.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(pointer.x + gap, pointer.y),
            Pos2::new(rect.right(), pointer.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(pointer.x, rect.top()),
            Pos2::new(pointer.x, pointer.y - gap),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(pointer.x, pointer.y + gap),
            Pos2::new(pointer.x, rect.bottom()),
        ],
        stroke,
    );
    painter.rect_stroke(
        Rect::from_center_size(pointer, egui::vec2(2.0 * gap, 2.0 * gap)),
        0.0,
        stroke,
        egui::StrokeKind::Inside,
    );
    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
}

/// Paints the image of the registered texture over the background, when there is one.
fn paint_texture(painter: &egui::Painter, rect: Rect, texture: Option<TextureId>) {
    if let Some(texture) = texture {
        painter.image(
            texture,
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

/// Names the viewport for accessibility: its label, the selection count and the prompt.
fn announce(
    ui: &Ui,
    response: &Response,
    localize: &dyn Localize,
    name: &str,
    selected: usize,
    prompt: Option<&Prompt>,
) {
    let name = resolve(
        localize,
        messages::VIEWPORT.message_id,
        &[("name", Value::Text(name.to_owned()))],
    );
    let description = describe(localize, selected, prompt);
    response.widget_info(|| WidgetInfo::labeled(egui::accesskit::Role::Canvas, true, &name));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_description(description);
    });
}

/// The egui events of this frame that concern the viewport, in viewport pixels.
fn collect_events(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    blocked: Option<Rect>,
    ppp: f64,
    time: f64,
    pressed: bool,
) -> Vec<ViewEvent> {
    // A menu, dialog or popup above the viewport takes the click: the viewport sees a press only
    // where its own layer is the topmost.
    let layer = ui.layer_id();
    let on_top = |p: Pos2| ui.ctx().layer_id_at(p).is_none_or(|top| top == layer);
    let free = |p: Pos2| rect.contains(p) && !blocked.is_some_and(|b| b.contains(p)) && on_top(p);
    let to_px = |p: Pos2| {
        [
            f64::from(p.x - rect.min.x) * ppp,
            f64::from(p.y - rect.min.y) * ppp,
        ]
    };
    let focused = response.has_focus();
    let hovered = response.hovered() || response.contains_pointer();
    let pointer = ui.input(|i| i.pointer.latest_pos());
    // The events are copied out first: asking the context for the layer under a point while the
    // input is borrowed would wait on itself.
    let raw = ui.input(|i| i.events.clone());
    {
        raw.iter()
            .filter_map(|e| match *e {
                Event::PointerMoved(p) if pressed || rect.contains(p) => {
                    Some(ViewEvent::Moved(to_px(p)))
                }
                Event::PointerMoved(_) => Some(ViewEvent::Left),
                Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers,
                } if free(pos) => Some(ViewEvent::Pressed {
                    at: to_px(pos),
                    button,
                    modifiers,
                    time,
                }),
                Event::PointerButton {
                    pos,
                    button,
                    pressed: false,
                    ..
                } => Some(ViewEvent::Released {
                    at: to_px(pos),
                    button,
                }),
                Event::PointerGone => Some(ViewEvent::Left),
                Event::MouseWheel { unit, delta, .. } if hovered && pointer.is_none_or(free) => {
                    let lines = f64::from(delta.y);
                    let points = match unit {
                        MouseWheelUnit::Point => lines,
                        MouseWheelUnit::Line => lines * POINTS_PER_LINE,
                        MouseWheelUnit::Page => lines * f64::from(rect.height()),
                    };
                    Some(ViewEvent::Wheel {
                        at: to_px(pointer.unwrap_or(rect.center())),
                        points,
                    })
                }
                Event::Zoom(factor) if hovered => Some(ViewEvent::Zoom {
                    at: to_px(pointer.unwrap_or(rect.center())),
                    factor: f64::from(factor),
                }),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if focused => Some(ViewEvent::Key { key, modifiers }),
                _ => None,
            })
            .collect()
    }
}

/// The accessible description: the selection count, then the prompt text when a command is
/// prompting.
fn describe(localize: &dyn Localize, selected: usize, prompt: Option<&Prompt>) -> String {
    let count = i64::try_from(selected).unwrap_or(i64::MAX);
    let selection = resolve(
        localize,
        messages::VIEWPORT_SELECTION.message_id,
        &[("count", Value::Int(count))],
    );
    match prompt {
        None => selection,
        Some(p) => resolve(
            localize,
            messages::VIEWPORT_STATUS.message_id,
            &[
                ("selection", Value::Text(selection)),
                ("prompt", Value::Text(prompt_text(p, localize))),
            ],
        ),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 1..=u16::MAX first"
)]
fn pixel_size(rect: Rect, ppp: f64) -> [u32; 2] {
    let side = |points: f32| {
        let px = (f64::from(points) * ppp).round();
        if px.is_finite() {
            px.clamp(1.0, f64::from(u16::MAX)) as u32
        } else {
            1
        }
    };
    [side(rect.width()), side(rect.height())]
}

fn color(c: Color32, alpha: Option<u8>) -> Color {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color::from_srgb8([r, g, b, alpha.unwrap_or(a)])
}

/// The overlay of a rubber band at `scale` pixels per point: a window is a solid outline in
/// the selection color, a crossing a dashed outline in the pre-selection color, each over a
/// faint fill of its color, so the kinds differ in more than color.
#[must_use]
pub fn band_overlay(band: &Band, colors: &Colors, scale: f64) -> Overlay {
    let (x0, x1) = (
        band.start[0].min(band.end[0]),
        band.start[0].max(band.end[0]),
    );
    let (y0, y1) = (
        band.start[1].min(band.end[1]),
        band.start[1].max(band.end[1]),
    );
    let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    let (token, dashed) = match band.kind() {
        BandKind::Window => (colors.viewport_selection, false),
        BandKind::Crossing => (colors.viewport_preselection, true),
    };
    #[allow(clippy::cast_possible_truncation, reason = "a stroke width")]
    let width_px = scale.clamp(1.0, 8.0) as f32;
    let line = color(token, None);
    let mut primitives = Vec::new();
    if x1 - x0 >= 1.0 && y1 - y0 >= 1.0 {
        primitives.push(OverlayPrimitive::Fill {
            points: corners.iter().map(|c| Anchor::Screen(*c)).collect(),
            color: color(token, Some(BAND_FILL_ALPHA)),
        });
    }
    if dashed {
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            let period = ((DASH + GAP) * scale).max(len / MAX_DASHES).max(1.0);
            let dash = period * DASH / (DASH + GAP);
            let mut s = 0.0;
            while s < len {
                let e = (s + dash).min(len);
                let at = |t: f64| {
                    Anchor::Screen([
                        a[0] + (b[0] - a[0]) * t / len,
                        a[1] + (b[1] - a[1]) * t / len,
                    ])
                };
                primitives.push(OverlayPrimitive::Polyline {
                    points: vec![at(s), at(e)],
                    closed: false,
                    width_px,
                    color: line,
                });
                s += period;
            }
        }
    } else {
        primitives.push(OverlayPrimitive::Polyline {
            points: corners.iter().map(|c| Anchor::Screen(*c)).collect(),
            closed: true,
            width_px,
            color: line,
        });
    }
    Overlay { primitives }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeId;

    struct Claim;

    impl PointerHandler for Claim {
        fn pointer(&mut self, _: &PointerEvent) -> bool {
            true
        }
    }

    #[test]
    fn escape_ends_a_claimed_press_and_its_release_belongs_to_nobody() {
        let frame = crate::gesture::tests::top();
        let projection = ViewProjection::new(frame, 100, 100);
        let mut widget = ViewportWidget::new();
        let mut claim = Claim;
        let mut handler: Option<&mut dyn PointerHandler> = Some(&mut claim);
        let mut offer = |event: ViewEvent| {
            widget.offer(&mut handler, &projection, Modifiers::NONE, &event, 1.0)
        };
        let button = egui::PointerButton::Primary;
        assert!(offer(ViewEvent::Pressed {
            at: [10.0, 10.0],
            button,
            modifiers: Modifiers::NONE,
            time: 0.0,
        }));
        assert!(offer(ViewEvent::Key {
            key: egui::Key::Escape,
            modifiers: Modifiers::NONE,
        }));
        assert!(!offer(ViewEvent::Released {
            at: [50.0, 50.0],
            button,
        }));
        assert!(widget.claim_cancelled);
        assert!(!widget.holds_product_press());
    }

    #[test]
    fn grid_spacing_steps_in_one_two_five_and_keeps_lines_apart() {
        for ppu in [0.001, 0.37, 1.0, 4.5, 80.0, 1.0e6] {
            let step = grid_spacing(ppu).unwrap();
            assert!(step * ppu >= MIN_GRID_PIXELS * 0.999, "{ppu} {step}");
            assert!(step * ppu < MIN_GRID_PIXELS * 2.6, "{ppu} {step}");
            let decade = 10f64.powf(step.log10().floor());
            let mantissa = (step / decade).round();
            assert!([1.0, 2.0, 5.0].contains(&mantissa), "{step}");
        }
        assert_eq!(grid_spacing(1.0), Some(20.0));
        assert_eq!(grid_spacing(0.0), None);
        assert_eq!(grid_spacing(f64::NAN), None);
    }

    #[test]
    fn window_and_crossing_bands_differ_in_outline_and_color() {
        let colors = ThemeId::Light.theme().colors;
        let lines = |o: &Overlay| {
            o.primitives
                .iter()
                .filter(|p| matches!(p, OverlayPrimitive::Polyline { .. }))
                .count()
        };
        let window = band_overlay(
            &Band {
                start: [10.0, 10.0],
                end: [110.0, 60.0],
            },
            &colors,
            1.0,
        );
        let crossing = band_overlay(
            &Band {
                start: [110.0, 60.0],
                end: [10.0, 10.0],
            },
            &colors,
            1.0,
        );
        assert_eq!(lines(&window), 1, "one closed solid outline");
        assert!(lines(&crossing) > 20, "dashes");
        let fill = |o: &Overlay| match o.primitives.first() {
            Some(OverlayPrimitive::Fill { color, .. }) => *color,
            other => panic!("{other:?}"),
        };
        assert_ne!(fill(&window), fill(&crossing));
        // Huge bands keep a bounded number of dashes; a zero-size band has no fill.
        let huge = band_overlay(
            &Band {
                start: [1e9, 0.0],
                end: [0.0, 1e9],
            },
            &colors,
            1.0,
        );
        assert!(lines(&huge) <= 4 * 257);
        let dot = band_overlay(
            &Band {
                start: [5.0, 5.0],
                end: [5.0, 5.0],
            },
            &colors,
            1.0,
        );
        assert!(
            dot.primitives
                .iter()
                .all(|p| !matches!(p, OverlayPrimitive::Fill { .. }))
        );
    }
}
