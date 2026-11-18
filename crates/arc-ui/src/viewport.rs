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
    Color32, Event, MouseWheelUnit, Pos2, Rect, Response, Sense, TextureId, Ui, WidgetInfo,
};

use crate::command::Intent;
use crate::command_line::prompt_text;
use crate::gesture::{Band, BandKind, Gestures, View, ViewEvent, ViewPicker, ViewportCommands};
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
}

/// The state of one viewport widget.
#[derive(Debug, Clone, Default)]
pub struct ViewportWidget {
    gestures: Gestures,
    link: Option<ViewLink>,
    seen: u64,
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

    /// The gesture state.
    #[must_use]
    pub fn gestures(&self) -> &Gestures {
        &self.gestures
    }

    /// Draws the viewport in the available space and handles its input.
    pub fn show(&mut self, ui: &mut Ui, input: ViewportInput<'_>) -> ViewportOutput {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let ppp = f64::from(ui.ctx().pixels_per_point());
        let size = pixel_size(rect, ppp);
        self.gestures
            .set_scale(ppp * f64::from(input.scale.factor()));

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

        let mut view = View {
            frame,
            width: size[0],
            height: size[1],
            prompt: input.prompt,
            selected: input.selected,
            tolerance: input.tolerance,
            picker: input.picker,
            commands: input.commands,
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
        };
        let mut moved_here = false;
        for event in &events {
            let r = self.gestures.handle(&mut view, event);
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
        if moved_here && let Some(link) = &self.link {
            self.seen = link.publish(view.frame);
        }
        out.hover = self.gestures.hover();
        if let Some(band) = self.gestures.band() {
            out.overlay = band_overlay(&band, input.colors, ppp * f64::from(input.scale.factor()));
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, input.colors.viewport);
        if let Some(texture) = input.texture {
            painter.image(
                texture,
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        let name = resolve(
            input.localize,
            messages::VIEWPORT.message_id,
            &[("name", Value::Text(input.name.to_owned()))],
        );
        let description = describe(input.localize, input.selected, input.prompt);
        response.widget_info(|| WidgetInfo::labeled(egui::accesskit::Role::Canvas, true, &name));
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_description(description);
        });
        out
    }
}

/// The egui events of this frame that concern the viewport, in viewport pixels.
fn collect_events(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    ppp: f64,
    time: f64,
    pressed: bool,
) -> Vec<ViewEvent> {
    let to_px = |p: Pos2| {
        [
            f64::from(p.x - rect.min.x) * ppp,
            f64::from(p.y - rect.min.y) * ppp,
        ]
    };
    let focused = response.has_focus();
    let hovered = response.hovered() || response.contains_pointer();
    let pointer = ui.input(|i| i.pointer.latest_pos());
    ui.input(|i| {
        i.events
            .iter()
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
                } if rect.contains(pos) => Some(ViewEvent::Pressed {
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
                Event::MouseWheel { unit, delta, .. } if hovered => {
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
    })
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
