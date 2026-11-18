//! Viewport interaction (CORE-UI-005, CORE-UI-029): a state machine from pointer and keyboard
//! events in viewport pixels to camera changes, pre-selection, rubber bands, selection
//! intents and prompt answers. It draws nothing and owns no selection, so tests drive it
//! without a window; [`crate::viewport::ViewportWidget`] feeds it from egui.
//!
//! # Rules
//!
//! Positions are image pixels of the viewport (x right, y down, from the top-left corner).
//! Distances below are logical points, multiplied by the scale (pixels per point times the
//! UI scale) given to [`Gestures::set_scale`]. Time comes from the caller's clock in seconds.
//!
//! - **Primary button.** A press and a release no farther apart than [`DRAG_THRESHOLD`] is a
//!   click. A click picks the *stack* under the pointer: the object visible at the pointer
//!   pixel first (lines within [`PICK_APERTURE`] count), then the other objects visible in the
//!   square of half side [`PICK_APERTURE`] around it. Another click within [`CYCLE_RADIUS`] of
//!   the previous click, with no camera change between them, takes the next object of the same
//!   stack, wrapping around. A click selects its object: replace, add with Shift, toggle with
//!   Ctrl (Cmd on macOS). A click on nothing emits nothing.
//! - **Rubber band.** Moving farther than [`DRAG_THRESHOLD`] with the primary button down
//!   starts a band. Dragged left to right it is a window: the objects with a visible pixel
//!   inside the rectangle and none outside it in the image. Right to left it is a crossing:
//!   the objects with a visible pixel inside. The modifiers of the press choose replace, add
//!   or toggle. A band with no objects emits a replace with no objects and nothing for add or
//!   toggle. Escape cancels a band.
//! - **One intent per gesture.** Each click or band emits at most one intent for
//!   [`ViewportCommands::select`] with `objects`, `mode` (`replace`, `add`, `toggle`) and
//!   `gesture` (`click`, `window`, `crossing`). Nothing else changes the selection: hover,
//!   navigation and prompt answers never emit a selection intent.
//! - **Prompts.** While a command prompts, the primary button answers it instead of
//!   selecting: a click on an object answers a pick prompt with the object or a selection
//!   prompt with a one-object set; otherwise a point prompt gets the point under the pointer
//!   on the view plane (through the camera target, facing the eye; no snapping yet). A band
//!   answers a selection prompt with its objects and is not started for other prompts. A
//!   prompt that accepts none of these ignores the button. Escape is left to the command line.
//! - **Pre-selection.** While no button is down, the hovered object is the one a click would
//!   select: the next object of the cycling stack when the pointer is within
//!   [`CYCLE_RADIUS`] of the last click, else the object at the pointer pixel. While a prompt
//!   that accepts no object is active, nothing is pre-selected.
//! - **Navigation.** Middle drag pans (the target follows the pointer); Shift with middle drag
//!   orbits about the target by [`ORBIT_RADIANS_PER_POINT`]: dragging right gives
//!   `yaw = −dx·k`, dragging down `pitch = −dy·k`. The wheel zooms by
//!   `exp(points · WHEEL_ZOOM_PER_POINT)` at the point under the pointer on the view plane;
//!   a pinch zooms by its factor there. A second middle press within
//!   [`DOUBLE_CLICK_SECONDS`] and [`DRAG_THRESHOLD`] of the first zooms to the extents of the
//!   scene. Keys (with focus): arrows orbit by [`KEY_ORBIT_RADIANS`], Shift with arrows pans
//!   by [`KEY_PAN_FRACTION`] of the image height, `+` and `-` zoom by [`KEY_ZOOM_FACTOR`] at
//!   the target, Home zooms to the extents. A clock running backwards counts as no time
//!   passed.
//! - **Escape** cancels a press or band in progress; otherwise, with no prompt and a
//!   non-empty selection, it emits [`ViewportCommands::clear_selection`].
//!
//! The pixel picks see only visible pixels: an object hidden behind others everywhere in the
//! pick square is not in the stack, and parts outside the image or hidden do not count against
//! a window.

use std::f64::consts::PI;

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, ObjectId, Result, Tolerance};
use arc_command::prompt::{Accept, Point, PointDims, Prompt, PromptInput};
use arc_geom::view::ViewFrame;
use arc_gpu::{PickHit, PickRegion, navigation};
use egui::{Key, Modifiers, PointerButton};

use crate::command::{Intent, object_list};

/// Movement in points beyond which a press becomes a drag.
pub const DRAG_THRESHOLD: f64 = 4.0;
/// Distance in points within which a repeated click cycles through the stack.
pub const CYCLE_RADIUS: f64 = 3.0;
/// Half side in points of the square picked for the stack; also the line pick tolerance.
pub const PICK_APERTURE: f64 = 4.0;
/// Seconds within which a second middle press zooms to the extents.
pub const DOUBLE_CLICK_SECONDS: f64 = 0.4;
/// Orbit radians per point of drag.
pub const ORBIT_RADIANS_PER_POINT: f64 = 0.01;
/// Wheel zoom: the factor is `exp(points · WHEEL_ZOOM_PER_POINT)`.
pub const WHEEL_ZOOM_PER_POINT: f64 = 0.002;
/// Orbit radians per arrow key press.
pub const KEY_ORBIT_RADIANS: f64 = PI / 12.0;
/// Pan per Shift+arrow key press, as a fraction of the image height.
pub const KEY_PAN_FRACTION: f64 = 0.1;
/// Zoom factor per `+` or `-` key press.
pub const KEY_ZOOM_FACTOR: f64 = 1.25;
/// The most objects one stack holds.
pub const MAX_STACK: usize = 64;

/// The default command of selection intents.
pub const DEFAULT_SELECT: &str = "core.select.set";
/// The default command that clears the selection.
pub const DEFAULT_CLEAR_SELECTION: &str = "core.select.clear";

/// The commands viewport intents name, with their parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportCommands {
    /// `objects`, `mode` (`replace`, `add`, `toggle`) and `gesture` (`click`, `window`,
    /// `crossing`).
    pub select: String,
    /// No parameters.
    pub clear_selection: String,
}

impl Default for ViewportCommands {
    fn default() -> Self {
        Self {
            select: DEFAULT_SELECT.to_owned(),
            clear_selection: DEFAULT_CLEAR_SELECTION.to_owned(),
        }
    }
}

/// The picking a viewport needs: the identifier pass of `arc-gpu` over the scene the viewport
/// shows, and the scene's bounds. Tests implement it with a fake.
pub trait ViewPicker {
    /// The distinct hits visible in `region` of a `width × height` image seen through
    /// `frame`, as [`arc_gpu::Renderer::pick`] returns them.
    ///
    /// # Errors
    ///
    /// The picking errors of `arc-gpu`.
    fn pick(
        &mut self,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        region: &PickRegion,
    ) -> Result<Vec<PickHit>>;

    /// The bounding sphere of the whole scene, or `None` when it is empty.
    fn bounds(&mut self) -> Option<(Point3, f64)>;
}

/// An input event in viewport pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewEvent {
    /// The pointer moved to a position.
    Moved([f64; 2]),
    /// A button went down.
    Pressed {
        /// The position.
        at: [f64; 2],
        /// The button.
        button: PointerButton,
        /// The modifiers held.
        modifiers: Modifiers,
        /// The caller's clock in seconds.
        time: f64,
    },
    /// A button went up.
    Released {
        /// The position.
        at: [f64; 2],
        /// The button.
        button: PointerButton,
    },
    /// Wheel scrolling in points; positive zooms in.
    Wheel {
        /// The pointer position.
        at: [f64; 2],
        /// The scroll amount in points.
        points: f64,
    },
    /// A pinch zoom factor; greater than 1 zooms in.
    Zoom {
        /// The pointer position.
        at: [f64; 2],
        /// The factor.
        factor: f64,
    },
    /// A key press while the viewport has focus.
    Key {
        /// The key.
        key: Key,
        /// The modifiers held.
        modifiers: Modifiers,
    },
    /// The pointer left the viewport.
    Left,
}

/// The kind of a rubber band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandKind {
    /// Dragged left to right: objects fully inside.
    Window,
    /// Dragged right to left: objects inside or touching.
    Crossing,
}

/// A rubber band being dragged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    /// Where the drag started.
    pub start: [f64; 2],
    /// The current pointer position.
    pub end: [f64; 2],
}

impl Band {
    /// The kind, by drag direction.
    #[must_use]
    pub fn kind(&self) -> BandKind {
        if self.end[0] >= self.start[0] {
            BandKind::Window
        } else {
            BandKind::Crossing
        }
    }
}

/// What the state machine reads besides events.
pub struct View<'a> {
    /// The camera; navigation updates it.
    pub frame: ViewFrame,
    /// The image width in pixels.
    pub width: u32,
    /// The image height in pixels.
    pub height: u32,
    /// The active prompt, when a command is prompting.
    pub prompt: Option<&'a Prompt>,
    /// The number of selected objects.
    pub selected: usize,
    /// The document tolerance (orbit limits, zoom extents).
    pub tolerance: &'a Tolerance,
    /// The picking interface.
    pub picker: &'a mut dyn ViewPicker,
    /// The commands intents name.
    pub commands: &'a ViewportCommands,
}

/// What one event caused.
#[derive(Debug, Default)]
pub struct Reaction {
    /// The new camera, also stored in [`View::frame`].
    pub camera: Option<ViewFrame>,
    /// A selection intent.
    pub intent: Option<Intent>,
    /// An answer to the active prompt.
    pub answer: Option<PromptInput>,
    /// A picking or navigation error; the event had no other effect.
    pub error: Option<Error>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Press {
    Primary {
        start: [f64; 2],
        modifiers: Modifiers,
        band: Option<[f64; 2]>,
    },
    Pan([f64; 2]),
    Orbit([f64; 2]),
}

#[derive(Debug, Clone, PartialEq)]
struct LastClick {
    at: [f64; 2],
    stack: Vec<ObjectId>,
    index: usize,
}

/// The viewport interaction state; see the [rules](self).
#[derive(Debug, Clone, PartialEq)]
pub struct Gestures {
    scale: f64,
    press: Option<Press>,
    last_click: Option<LastClick>,
    last_middle: Option<([f64; 2], f64)>,
    hover: Option<ObjectId>,
}

impl Default for Gestures {
    fn default() -> Self {
        Self {
            scale: 1.0,
            press: None,
            last_click: None,
            last_middle: None,
            hover: None,
        }
    }
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn finite(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

/// `v` clamped to `0..=max` as a whole pixel index (`NaN` gives 0).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to the u32 range first"
)]
fn pixel(v: f64, max: u32) -> u32 {
    if v.is_nan() {
        0
    } else {
        v.floor().clamp(0.0, f64::from(max)) as u32
    }
}

fn region(x0: u32, y0: u32, x1: u32, y1: u32, tolerance_px: f32) -> Option<PickRegion> {
    (x1 > x0 && y1 > y0).then_some(PickRegion {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
        tolerance_px,
    })
}

fn objects(hits: &[PickHit], out: &mut Vec<ObjectId>) {
    for hit in hits {
        if out.len() < MAX_STACK && !out.contains(&hit.key.object) {
            out.push(hit.key.object);
        }
    }
}

fn accepts(prompt: &Prompt, test: impl Fn(&Accept) -> bool) -> bool {
    prompt.accepts.iter().any(test)
}

fn accepts_objects(prompt: &Prompt) -> bool {
    accepts(prompt, |a| {
        matches!(a, Accept::Pick { .. } | Accept::Selection { .. })
    })
}

impl Gestures {
    /// A machine at scale 1.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the pixels per point of the thresholds (pixels per point times the UI scale); a
    /// value that is not finite and positive gives 1.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
    }

    /// The pre-selected object.
    #[must_use]
    pub fn hover(&self) -> Option<ObjectId> {
        self.hover
    }

    /// Whether a button press is being tracked (a click, band, pan or orbit in progress).
    #[must_use]
    pub fn is_pressed(&self) -> bool {
        self.press.is_some()
    }

    /// The rubber band being dragged.
    #[must_use]
    pub fn band(&self) -> Option<Band> {
        match self.press {
            Some(Press::Primary {
                start,
                band: Some(end),
                ..
            }) => Some(Band { start, end }),
            _ => None,
        }
    }

    /// Handles one event.
    pub fn handle(&mut self, view: &mut View<'_>, event: &ViewEvent) -> Reaction {
        let mut out = Reaction::default();
        match *event {
            ViewEvent::Moved(at) if finite(at) => self.moved(view, at, &mut out),
            ViewEvent::Pressed {
                at,
                button,
                modifiers,
                time,
            } if finite(at) => self.pressed(view, at, button, modifiers, time, &mut out),
            ViewEvent::Released { at, button } if finite(at) => {
                self.released(view, at, button, &mut out);
            }
            ViewEvent::Wheel { at, points } if finite(at) && points.is_finite() => {
                let point = self.view_plane_point(view, at);
                let factor = (points * WHEEL_ZOOM_PER_POINT).exp();
                self.navigate(view, &mut out, |f| navigation::zoom_at(f, factor, &point?));
            }
            ViewEvent::Zoom { at, factor } if finite(at) && factor.is_finite() => {
                let point = self.view_plane_point(view, at);
                self.navigate(view, &mut out, |f| navigation::zoom_at(f, factor, &point?));
            }
            ViewEvent::Key { key, modifiers } => self.key(view, key, modifiers, &mut out),
            ViewEvent::Left => {
                self.hover = None;
            }
            _ => {}
        }
        out
    }

    fn moved(&mut self, view: &mut View<'_>, at: [f64; 2], out: &mut Reaction) {
        match &mut self.press {
            Some(Press::Primary { start, band, .. }) => {
                let allowed = view
                    .prompt
                    .is_none_or(|p| accepts(p, |a| matches!(a, Accept::Selection { .. })));
                if band.is_some() || (allowed && distance(*start, at) > DRAG_THRESHOLD * self.scale)
                {
                    *band = Some(at);
                    self.hover = None;
                }
            }
            Some(Press::Pan(last)) => {
                let delta = [at[0] - last[0], at[1] - last[1]];
                *last = at;
                let (w, h) = (view.width, view.height);
                self.navigate(view, out, |f| navigation::pan(f, delta, w, h, &f.target()));
            }
            Some(Press::Orbit(last)) => {
                let k = ORBIT_RADIANS_PER_POINT / self.scale;
                let (yaw, pitch) = (-(at[0] - last[0]) * k, -(at[1] - last[1]) * k);
                *last = at;
                let tolerance = *view.tolerance;
                self.navigate(view, out, |f| {
                    navigation::orbit(f, &f.target(), yaw, pitch, &tolerance)
                });
            }
            None => self.hover = self.preselect(view, at, out),
        }
    }

    fn pressed(
        &mut self,
        view: &mut View<'_>,
        at: [f64; 2],
        button: PointerButton,
        modifiers: Modifiers,
        time: f64,
        out: &mut Reaction,
    ) {
        if self.press.is_some() {
            return;
        }
        match button {
            PointerButton::Primary => {
                self.press = Some(Press::Primary {
                    start: at,
                    modifiers,
                    band: None,
                });
            }
            PointerButton::Middle => {
                let double = self.last_middle.is_some_and(|(then, t)| {
                    (time - t).max(0.0) <= DOUBLE_CLICK_SECONDS
                        && distance(then, at) <= DRAG_THRESHOLD * self.scale
                });
                if double {
                    self.last_middle = None;
                    self.zoom_extents(view, out);
                    return;
                }
                self.last_middle = time.is_finite().then_some((at, time));
                self.press = Some(if modifiers.shift {
                    Press::Orbit(at)
                } else {
                    Press::Pan(at)
                });
                self.hover = None;
            }
            _ => {}
        }
    }

    fn released(
        &mut self,
        view: &mut View<'_>,
        at: [f64; 2],
        button: PointerButton,
        out: &mut Reaction,
    ) {
        match (self.press, button) {
            (
                Some(Press::Primary {
                    start,
                    modifiers,
                    band,
                }),
                PointerButton::Primary,
            ) => {
                self.press = None;
                if band.is_some() {
                    self.finish_band(view, Band { start, end: at }, modifiers, out);
                } else {
                    self.click(view, start, modifiers, out);
                }
            }
            (Some(Press::Pan(_) | Press::Orbit(_)), PointerButton::Middle) => self.press = None,
            _ => {}
        }
    }

    fn key(&mut self, view: &mut View<'_>, key: Key, modifiers: Modifiers, out: &mut Reaction) {
        let tolerance = *view.tolerance;
        let orbit = |yaw: f64, pitch: f64| {
            move |f: &ViewFrame| navigation::orbit(f, &f.target(), yaw, pitch, &tolerance)
        };
        let step = f64::from(view.height) * KEY_PAN_FRACTION;
        let (w, h) = (view.width, view.height);
        let pan =
            |dx: f64, dy: f64| move |f: &ViewFrame| navigation::pan(f, [dx, dy], w, h, &f.target());
        let zoom = |factor: f64| move |f: &ViewFrame| navigation::zoom_at(f, factor, &f.target());
        let a = KEY_ORBIT_RADIANS;
        match (key, modifiers.shift) {
            (Key::ArrowLeft, false) => self.navigate(view, out, orbit(a, 0.0)),
            (Key::ArrowRight, false) => self.navigate(view, out, orbit(-a, 0.0)),
            (Key::ArrowUp, false) => self.navigate(view, out, orbit(0.0, a)),
            (Key::ArrowDown, false) => self.navigate(view, out, orbit(0.0, -a)),
            (Key::ArrowLeft, true) => self.navigate(view, out, pan(step, 0.0)),
            (Key::ArrowRight, true) => self.navigate(view, out, pan(-step, 0.0)),
            (Key::ArrowUp, true) => self.navigate(view, out, pan(0.0, step)),
            (Key::ArrowDown, true) => self.navigate(view, out, pan(0.0, -step)),
            (Key::Plus | Key::Equals, _) => self.navigate(view, out, zoom(KEY_ZOOM_FACTOR)),
            (Key::Minus, _) => self.navigate(view, out, zoom(1.0 / KEY_ZOOM_FACTOR)),
            (Key::Home, _) => self.zoom_extents(view, out),
            (Key::Escape, _) => {
                let idle = self.press.take().is_none();
                if idle && view.prompt.is_none() && view.selected > 0 {
                    out.intent = Some(Intent::new(view.commands.clear_selection.clone()));
                }
            }
            _ => {}
        }
    }

    /// Applies a navigation function to the camera.
    fn navigate(
        &mut self,
        view: &mut View<'_>,
        out: &mut Reaction,
        f: impl FnOnce(&ViewFrame) -> Result<ViewFrame>,
    ) {
        match f(&view.frame) {
            Ok(frame) => {
                if frame != view.frame {
                    view.frame = frame;
                    out.camera = Some(frame);
                    // The stack under the pointer is stale once the view moved.
                    self.last_click = None;
                }
            }
            Err(e) => out.error = Some(e),
        }
    }

    fn zoom_extents(&mut self, view: &mut View<'_>, out: &mut Reaction) {
        let Some((center, radius)) = view.picker.bounds() else {
            return;
        };
        let aspect = f64::from(view.width) / f64::from(view.height.max(1));
        let tolerance = *view.tolerance;
        self.navigate(view, out, |f| {
            navigation::zoom_to_sphere(f, &center, radius, aspect, &tolerance)
        });
    }

    /// The pick ray through `at` meets the plane through the target facing the eye.
    fn view_plane_point(&self, view: &View<'_>, at: [f64; 2]) -> Result<Point3> {
        let (w, h) = (f64::from(view.width.max(1)), f64::from(view.height.max(1)));
        let f = &view.frame;
        let (origin, dir) = f.ray_through(2.0 * at[0] / w - 1.0, 1.0 - 2.0 * at[1] / h, w / h)?;
        let forward: Vector3 = f.forward();
        let along = dir.dot(&forward);
        let t = (f.target() - origin).dot(&forward) / along;
        let p = origin + dir * t;
        if p.iter().all(|c| c.is_finite()) {
            Ok(p)
        } else {
            Err(Error::new(arc_gpu::error::codes::NAVIGATION_INVALID))
        }
    }

    fn pick(&mut self, view: &mut View<'_>, region: Option<PickRegion>) -> Result<Vec<PickHit>> {
        match region {
            Some(r) => view.picker.pick(&view.frame, view.width, view.height, &r),
            None => Ok(Vec::new()),
        }
    }

    /// The objects under `at`, the pixel's object first.
    fn stack(&mut self, view: &mut View<'_>, at: [f64; 2]) -> Result<Vec<ObjectId>> {
        let (w, h) = (view.width, view.height);
        let a = PICK_APERTURE * self.scale;
        #[allow(clippy::cast_possible_truncation, reason = "a few pixels")]
        let tolerance = a as f32;
        let (x, y) = (
            pixel(at[0], w.saturating_sub(1)),
            pixel(at[1], h.saturating_sub(1)),
        );
        let mut out = Vec::new();
        let center = self.pick(view, region(x, y, x + 1, y + 1, tolerance))?;
        objects(&center, &mut out);
        let around = region(
            pixel(at[0] - a, w),
            pixel(at[1] - a, h),
            pixel(at[0] + a + 1.0, w),
            pixel(at[1] + a + 1.0, h),
            0.0,
        );
        let around = self.pick(view, around)?;
        objects(&around, &mut out);
        Ok(out)
    }

    /// The cycled stack for a click at `at`: the next object of the last click's stack when
    /// `at` is within the cycle radius, else a fresh stack.
    fn cycle(&mut self, view: &mut View<'_>, at: [f64; 2]) -> Result<LastClick> {
        if let Some(last) = &self.last_click
            && distance(last.at, at) <= CYCLE_RADIUS * self.scale
            && !last.stack.is_empty()
        {
            return Ok(LastClick {
                at: last.at,
                stack: last.stack.clone(),
                index: (last.index + 1) % last.stack.len(),
            });
        }
        Ok(LastClick {
            at,
            stack: self.stack(view, at)?,
            index: 0,
        })
    }

    fn preselect(
        &mut self,
        view: &mut View<'_>,
        at: [f64; 2],
        out: &mut Reaction,
    ) -> Option<ObjectId> {
        if view.prompt.is_some_and(|p| !accepts_objects(p)) {
            return None;
        }
        if let Some(last) = &self.last_click
            && distance(last.at, at) <= CYCLE_RADIUS * self.scale
            && !last.stack.is_empty()
        {
            return last.stack.get((last.index + 1) % last.stack.len()).copied();
        }
        let (w, h) = (view.width, view.height);
        let (x, y) = (
            pixel(at[0], w.saturating_sub(1)),
            pixel(at[1], h.saturating_sub(1)),
        );
        #[allow(clippy::cast_possible_truncation, reason = "a few pixels")]
        let tolerance = (PICK_APERTURE * self.scale) as f32;
        match self.pick(view, region(x, y, x + 1, y + 1, tolerance)) {
            Ok(hits) => hits.first().map(|h| h.key.object),
            Err(e) => {
                out.error = Some(e);
                None
            }
        }
    }

    fn click(
        &mut self,
        view: &mut View<'_>,
        at: [f64; 2],
        modifiers: Modifiers,
        out: &mut Reaction,
    ) {
        if let Some(prompt) = view.prompt {
            let wants_object = accepts_objects(prompt);
            let wants_point = accepts(prompt, |a| matches!(a, Accept::Point { .. }));
            if !wants_object && !wants_point {
                return;
            }
            let hit = if wants_object {
                match self.cycle(view, at) {
                    Ok(c) => {
                        let hit = c.stack.get(c.index).copied();
                        self.last_click = Some(c);
                        hit
                    }
                    Err(e) => {
                        out.error = Some(e);
                        return;
                    }
                }
            } else {
                None
            };
            out.answer = match hit {
                Some(id) if accepts(prompt, |a| matches!(a, Accept::Pick { .. })) => {
                    Some(PromptInput::Pick(id))
                }
                Some(id) => Some(PromptInput::Selection(vec![id])),
                None if wants_point => match self.view_plane_point(view, at) {
                    Ok(p) => Some(point_answer(prompt, p)),
                    Err(e) => {
                        out.error = Some(e);
                        None
                    }
                },
                None => None,
            };
            return;
        }
        let cycled = match self.cycle(view, at) {
            Ok(c) => c,
            Err(e) => {
                out.error = Some(e);
                return;
            }
        };
        let hit = cycled.stack.get(cycled.index).copied();
        self.last_click = Some(cycled);
        if let Some(id) = hit {
            out.intent = Some(select(view.commands, &[id], modifiers, "click"));
        }
    }

    fn finish_band(
        &mut self,
        view: &mut View<'_>,
        band: Band,
        modifiers: Modifiers,
        out: &mut Reaction,
    ) {
        self.last_click = None;
        let found = match self.area(view, band) {
            Ok(found) => found,
            Err(e) => {
                out.error = Some(e);
                return;
            }
        };
        if view.prompt.is_some() {
            out.answer = Some(PromptInput::Selection(found));
            return;
        }
        if found.is_empty() && (modifiers.shift || modifiers.command) {
            return;
        }
        let gesture = match band.kind() {
            BandKind::Window => "window",
            BandKind::Crossing => "crossing",
        };
        out.intent = Some(select(view.commands, &found, modifiers, gesture));
    }

    /// The objects a band selects.
    fn area(&mut self, view: &mut View<'_>, band: Band) -> Result<Vec<ObjectId>> {
        let (w, h) = (view.width, view.height);
        let lo = |i: usize| band.start[i].min(band.end[i]);
        let hi = |i: usize| band.start[i].max(band.end[i]);
        let (x0, y0) = (pixel(lo(0), w), pixel(lo(1), h));
        let (x1, y1) = (pixel(hi(0).ceil(), w), pixel(hi(1).ceil(), h));
        let mut inside = Vec::new();
        let hits = self.pick(
            view,
            region(x0, y0, x1.max(x0 + 1).min(w), y1.max(y0 + 1).min(h), 0.0),
        )?;
        for hit in &hits {
            if !inside.contains(&hit.key.object) {
                inside.push(hit.key.object);
            }
        }
        if band.kind() == BandKind::Crossing {
            return Ok(inside);
        }
        // Window: drop every object also seen in the four strips around the rectangle.
        for strip in [
            region(0, 0, w, y0, 0.0),
            region(0, y1, w, h, 0.0),
            region(0, y0, x0, y1, 0.0),
            region(x1, y0, w, y1, 0.0),
        ] {
            let outside = self.pick(view, strip)?;
            inside.retain(|id| !outside.iter().any(|h| h.key.object == *id));
        }
        Ok(inside)
    }
}

fn select(
    commands: &ViewportCommands,
    ids: &[ObjectId],
    modifiers: Modifiers,
    gesture: &str,
) -> Intent {
    let mode = if modifiers.shift {
        "add"
    } else if modifiers.command {
        "toggle"
    } else {
        "replace"
    };
    Intent::new(commands.select.clone())
        .with("objects", object_list(ids))
        .with("mode", mode)
        .with("gesture", gesture)
}

fn point_answer(prompt: &Prompt, p: Point3) -> PromptInput {
    let two = accepts(prompt, |a| {
        matches!(
            a,
            Accept::Point {
                dims: PointDims::Two
            }
        )
    });
    PromptInput::Point(Point {
        x: p.x,
        y: p.y,
        z: (!two).then_some(p.z),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use arc_base::id::Identifier;
    use arc_geom::view::Projection;
    use arc_gpu::ItemKey;
    use proptest::prelude::*;

    /// Objects as pixel rectangles `[x0, y0, x1)` × `[y0, y1)` with a depth; smaller is in
    /// front. Picking returns the front object of each pixel, sorted and distinct, like the
    /// identifier pass.
    pub struct Fake {
        pub objects: Vec<(ObjectId, [u32; 4], u32)>,
        pub picks: usize,
    }

    impl Fake {
        pub fn new(objects: &[(u128, [u32; 4], u32)]) -> Self {
            Self {
                objects: objects
                    .iter()
                    .map(|(id, r, d)| (ObjectId::from_u128(*id), *r, *d))
                    .collect(),
                picks: 0,
            }
        }
    }

    impl ViewPicker for Fake {
        fn pick(&mut self, _: &ViewFrame, w: u32, h: u32, r: &PickRegion) -> Result<Vec<PickHit>> {
            self.picks += 1;
            assert!(r.width > 0 && r.height > 0 && r.x + r.width <= w && r.y + r.height <= h);
            let mut out: Vec<PickHit> = Vec::new();
            for y in r.y..r.y + r.height {
                for x in r.x..r.x + r.width {
                    let front = self
                        .objects
                        .iter()
                        .filter(|(_, b, _)| x >= b[0] && x < b[2] && y >= b[1] && y < b[3])
                        .min_by_key(|(_, _, d)| *d);
                    if let Some((id, _, _)) = front {
                        let hit = PickHit {
                            key: ItemKey {
                                object: *id,
                                sub: 0,
                            },
                            element: 0,
                        };
                        if !out.contains(&hit) {
                            out.push(hit);
                        }
                    }
                }
            }
            out.sort_by_key(|h| h.key);
            Ok(out)
        }
        fn bounds(&mut self) -> Option<(Point3, f64)> {
            Some((Point3::new(10.0, 10.0, 0.0), 5.0))
        }
    }

    /// Looks down −z at the origin, 100 units tall: one unit per pixel of a 100 × 100 image.
    pub fn top() -> ViewFrame {
        ViewFrame::new(
            Point3::new(0.0, 0.0, 50.0),
            Point3::origin(),
            Vector3::y(),
            Projection::Orthographic { height: 100.0 },
            1.0,
            1000.0,
        )
        .unwrap()
    }

    fn id(n: u128) -> ObjectId {
        ObjectId::from_u128(n)
    }

    struct Rig {
        g: Gestures,
        fake: Fake,
        frame: ViewFrame,
        prompt: Option<Prompt>,
        selected: usize,
        commands: ViewportCommands,
    }

    impl Rig {
        fn new(objects: &[(u128, [u32; 4], u32)]) -> Self {
            Self {
                g: Gestures::new(),
                fake: Fake::new(objects),
                frame: top(),
                prompt: None,
                selected: 0,
                commands: ViewportCommands::default(),
            }
        }
        fn send(&mut self, e: ViewEvent) -> Reaction {
            let tolerance = Tolerance::DEFAULT;
            let mut view = View {
                frame: self.frame,
                width: 100,
                height: 100,
                prompt: self.prompt.as_ref(),
                selected: self.selected,
                tolerance: &tolerance,
                picker: &mut self.fake,
                commands: &self.commands,
            };
            let r = self.g.handle(&mut view, &e);
            self.frame = view.frame;
            r
        }
        /// Press, optionally move, release; returns every reaction.
        fn gesture(
            &mut self,
            button: PointerButton,
            m: Modifiers,
            path: &[[f64; 2]],
            time: f64,
        ) -> Vec<Reaction> {
            let mut out = vec![self.send(ViewEvent::Pressed {
                at: path[0],
                button,
                modifiers: m,
                time,
            })];
            for p in &path[1..] {
                out.push(self.send(ViewEvent::Moved(*p)));
            }
            out.push(self.send(ViewEvent::Released {
                at: *path.last().unwrap(),
                button,
            }));
            out
        }
        fn click(&mut self, at: [f64; 2], m: Modifiers) -> Reaction {
            self.gesture(PointerButton::Primary, m, &[at], 0.0)
                .pop()
                .unwrap()
        }
    }

    fn intent(ids: &[u128], mode: &str, gesture: &str) -> Intent {
        let ids: Vec<ObjectId> = ids.iter().map(|n| id(*n)).collect();
        Intent::new(DEFAULT_SELECT)
            .with("objects", object_list(&ids))
            .with("mode", mode)
            .with("gesture", gesture)
    }

    fn only_intent(rs: &[Reaction]) -> Option<Intent> {
        let found: Vec<&Intent> = rs.iter().filter_map(|r| r.intent.as_ref()).collect();
        assert!(found.len() <= 1, "at most one intent per gesture");
        found.first().map(|i| (*i).clone())
    }

    #[test]
    fn click_replaces_shift_adds_ctrl_toggles_and_empty_does_nothing() {
        let mut rig = Rig::new(&[(1, [10, 10, 20, 20], 0)]);
        assert_eq!(
            rig.click([15.0, 15.0], Modifiers::NONE).intent,
            Some(intent(&[1], "replace", "click"))
        );
        rig.g.last_click = None;
        assert_eq!(
            rig.click([15.0, 15.0], Modifiers::SHIFT).intent,
            Some(intent(&[1], "add", "click"))
        );
        rig.g.last_click = None;
        assert_eq!(
            rig.click([15.0, 15.0], Modifiers::COMMAND).intent,
            Some(intent(&[1], "toggle", "click"))
        );
        assert_eq!(rig.click([60.0, 60.0], Modifiers::NONE).intent, None);
        // A small wobble below the drag threshold is still a click.
        rig.g.last_click = None;
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::NONE,
            &[[15.0, 15.0], [17.0, 17.0]],
            0.0,
        );
        assert_eq!(only_intent(&rs), Some(intent(&[1], "replace", "click")));
    }

    #[test]
    fn repeated_clicks_cycle_through_the_stack_and_hover_shows_the_next() {
        // A small object 1 in front of a large object 2, and object 3 nearby.
        let mut rig = Rig::new(&[
            (1, [49, 49, 51, 51], 0),
            (2, [40, 40, 60, 60], 1),
            (3, [52, 45, 54, 47], 0),
        ]);
        let at = [50.0, 50.0];
        let picked: Vec<Option<Intent>> = (0..4)
            .map(|_| rig.click(at, Modifiers::NONE).intent)
            .collect();
        assert_eq!(
            picked,
            [
                Some(intent(&[1], "replace", "click")),
                Some(intent(&[2], "replace", "click")),
                Some(intent(&[3], "replace", "click")),
                Some(intent(&[1], "replace", "click")),
            ]
        );
        rig.send(ViewEvent::Moved([51.0, 50.0]));
        assert_eq!(
            rig.g.hover(),
            Some(id(2)),
            "the object the next click selects"
        );
        // Away from the last click a fresh stack starts.
        rig.send(ViewEvent::Moved([45.0, 58.0]));
        assert_eq!(rig.g.hover(), Some(id(2)));
        assert_eq!(
            rig.click([45.0, 58.0], Modifiers::NONE).intent,
            Some(intent(&[2], "replace", "click"))
        );
        // A camera change forgets the stack.
        rig.click(at, Modifiers::NONE);
        rig.send(ViewEvent::Key {
            key: Key::Plus,
            modifiers: Modifiers::NONE,
        });
        rig.frame = top();
        assert_eq!(
            rig.click(at, Modifiers::NONE).intent,
            Some(intent(&[1], "replace", "click"))
        );
        rig.send(ViewEvent::Left);
        assert_eq!(rig.g.hover(), None);
    }

    #[test]
    fn left_to_right_is_window_and_right_to_left_is_crossing() {
        // 1 fully inside 20..60, 2 straddles the right edge, 3 is outside.
        let objects = [
            (1, [25, 25, 35, 35], 0),
            (2, [50, 30, 70, 40], 0),
            (3, [80, 80, 90, 90], 0),
        ];
        let mut rig = Rig::new(&objects);
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::NONE,
            &[[20.0, 20.0], [40.0, 40.0], [60.0, 60.0]],
            0.0,
        );
        assert_eq!(only_intent(&rs), Some(intent(&[1], "replace", "window")));
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::SHIFT,
            &[[60.0, 60.0], [40.0, 40.0], [20.0, 20.0]],
            0.0,
        );
        assert_eq!(only_intent(&rs), Some(intent(&[1, 2], "add", "crossing")));
        // An empty band replaces with nothing; with Ctrl it does nothing.
        let empty = [[1.0, 90.0], [10.0, 99.0]];
        let rs = rig.gesture(PointerButton::Primary, Modifiers::NONE, &empty, 0.0);
        assert_eq!(only_intent(&rs), Some(intent(&[], "replace", "window")));
        let rs = rig.gesture(PointerButton::Primary, Modifiers::COMMAND, &empty, 0.0);
        assert_eq!(only_intent(&rs), None);
        // The band is reported while dragging, with its kind; Escape cancels it.
        rig.send(ViewEvent::Pressed {
            at: [60.0, 60.0],
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            time: 0.0,
        });
        rig.send(ViewEvent::Moved([30.0, 30.0]));
        assert_eq!(rig.g.band().map(|b| b.kind()), Some(BandKind::Crossing));
        rig.send(ViewEvent::Key {
            key: Key::Escape,
            modifiers: Modifiers::NONE,
        });
        assert_eq!(rig.g.band(), None);
        let r = rig.send(ViewEvent::Released {
            at: [30.0, 30.0],
            button: PointerButton::Primary,
        });
        assert!(r.intent.is_none());
    }

    #[test]
    fn thresholds_scale_with_the_ui_scale() {
        let mut rig = Rig::new(&[(1, [10, 10, 20, 20], 0)]);
        rig.g.set_scale(2.0);
        // Six pixels is a drag at scale 1 but a click at scale 2.
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::NONE,
            &[[12.0, 12.0], [18.0, 12.0]],
            0.0,
        );
        assert_eq!(only_intent(&rs), Some(intent(&[1], "replace", "click")));
        rig.g.set_scale(f64::NAN);
        assert_eq!(rig.g.scale, 1.0);
        rig.g.last_click = None;
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::NONE,
            &[[12.0, 12.0], [18.0, 12.0]],
            0.0,
        );
        assert_eq!(only_intent(&rs).unwrap().params["gesture"], "window");
    }

    #[test]
    fn prompts_take_clicks_and_bands_as_answers() {
        let mut rig = Rig::new(&[(1, [10, 10, 20, 20], 0)]);
        rig.prompt = Some(Prompt::new(
            "p",
            "m",
            Accept::Point {
                dims: PointDims::Three,
            },
        ));
        let r = rig.click([75.0, 25.0], Modifiers::NONE);
        assert!(r.intent.is_none());
        let Some(PromptInput::Point(p)) = r.answer else {
            panic!("{r:?}")
        };
        assert!((p.x - 25.0).abs() < 1e-9 && (p.y - 25.0).abs() < 1e-9 && p.z == Some(0.0));
        // A point prompt sees no objects: clicking one still gives a point; no pre-selection.
        rig.send(ViewEvent::Moved([15.0, 15.0]));
        assert_eq!(rig.g.hover(), None);
        rig.prompt = Some(Prompt::new(
            "p",
            "m",
            Accept::Point {
                dims: PointDims::Two,
            },
        ));
        let r = rig.click([15.0, 15.0], Modifiers::NONE);
        assert!(matches!(
            r.answer,
            Some(PromptInput::Point(Point { z: None, .. }))
        ));
        // A band is not started for a point prompt.
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::NONE,
            &[[5.0, 5.0], [30.0, 30.0]],
            0.0,
        );
        assert!(rs.iter().all(|r| r.intent.is_none()));

        rig.prompt = Some(Prompt::new("p", "m", Accept::Pick { types: vec![] }).or(
            Accept::Point {
                dims: PointDims::Any,
            },
        ));
        rig.g.last_click = None;
        assert_eq!(
            rig.click([15.0, 15.0], Modifiers::NONE).answer,
            Some(PromptInput::Pick(id(1)))
        );
        assert!(matches!(
            rig.click([80.0, 80.0], Modifiers::NONE).answer,
            Some(PromptInput::Point(_))
        ));

        rig.prompt = Some(Prompt::new(
            "p",
            "m",
            Accept::Selection {
                min: 1,
                max: None,
                types: vec![],
            },
        ));
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::NONE,
            &[[5.0, 5.0], [30.0, 30.0]],
            0.0,
        );
        assert_eq!(
            rs.last().unwrap().answer,
            Some(PromptInput::Selection(vec![id(1)]))
        );
        assert!(rs.iter().all(|r| r.intent.is_none()));

        rig.prompt = Some(Prompt::new(
            "p",
            "m",
            Accept::Distance {
                min: None,
                max: None,
            },
        ));
        let r = rig.click([15.0, 15.0], Modifiers::NONE);
        assert!(r.answer.is_none() && r.intent.is_none());
        // Escape is the command line's while a command prompts.
        rig.selected = 3;
        assert!(
            rig.send(ViewEvent::Key {
                key: Key::Escape,
                modifiers: Modifiers::NONE
            })
            .intent
            .is_none()
        );
    }

    #[test]
    fn navigation_moves_the_camera_and_never_selects() {
        let mut rig = Rig::new(&[(1, [10, 10, 20, 20], 0)]);
        // Middle drag pans: the target follows the pointer, 10 px right moves it 10 units left
        // in view terms, so the camera moves −10 in x.
        let rs = rig.gesture(
            PointerButton::Middle,
            Modifiers::NONE,
            &[[50.0, 50.0], [60.0, 50.0]],
            0.0,
        );
        assert!(rs.iter().all(|r| r.intent.is_none() && r.answer.is_none()));
        assert!((rig.frame.target().x + 10.0).abs() < 1e-9);
        // Shift+middle drag orbits about the target, keeping the distance.
        let before = rig.frame;
        rig.gesture(
            PointerButton::Middle,
            Modifiers::SHIFT,
            &[[50.0, 50.0], [50.0, 70.0]],
            5.0,
        );
        assert_ne!(rig.frame, before);
        assert!(((rig.frame.eye() - rig.frame.target()).norm() - 50.0).abs() < 1e-9);
        // The wheel zooms at the pointer: the model point under it stays under it.
        rig.frame = top();
        let r = rig.send(ViewEvent::Wheel {
            at: [80.0, 30.0],
            points: 200.0,
        });
        assert!(r.camera.is_some());
        let Projection::Orthographic { height } = rig.frame.projection() else {
            panic!()
        };
        assert!(height < 100.0);
        let ndc = rig
            .frame
            .project(&Point3::new(30.0, 20.0, 0.0), 1.0)
            .unwrap();
        assert!((ndc.x - 0.6).abs() < 1e-9 && (ndc.y - 0.4).abs() < 1e-9);
        // A double middle click zooms to the extents; a slow one does not.
        rig.frame = top();
        rig.gesture(PointerButton::Middle, Modifiers::NONE, &[[5.0, 5.0]], 10.0);
        rig.gesture(PointerButton::Middle, Modifiers::NONE, &[[5.0, 5.0]], 11.0);
        assert_eq!(rig.frame, top());
        rig.gesture(PointerButton::Middle, Modifiers::NONE, &[[5.0, 5.0]], 11.2);
        assert!((rig.frame.target() - Point3::new(10.0, 10.0, 0.0)).norm() < 1e-9);
        // Keys: Home, arrows and zoom; none selects.
        for (key, modifiers) in [
            (Key::ArrowLeft, Modifiers::NONE),
            (Key::ArrowUp, Modifiers::NONE),
            (Key::ArrowRight, Modifiers::SHIFT),
            (Key::Minus, Modifiers::NONE),
            (Key::Home, Modifiers::NONE),
        ] {
            let before = rig.frame;
            let r = rig.send(ViewEvent::Key { key, modifiers });
            assert!(
                r.intent.is_none() && r.camera.is_some() && rig.frame != before,
                "{key:?}"
            );
        }
        // Escape with a selection and no prompt clears it, else does nothing.
        assert!(
            rig.send(ViewEvent::Key {
                key: Key::Escape,
                modifiers: Modifiers::NONE
            })
            .intent
            .is_none()
        );
        rig.selected = 2;
        assert_eq!(
            rig.send(ViewEvent::Key {
                key: Key::Escape,
                modifiers: Modifiers::NONE
            })
            .intent,
            Some(Intent::new(DEFAULT_CLEAR_SELECTION))
        );
    }

    #[test]
    fn hostile_positions_are_ignored() {
        let mut rig = Rig::new(&[(1, [10, 10, 20, 20], 0)]);
        for at in [[f64::NAN, 1.0], [f64::INFINITY, 0.0]] {
            let r = rig.send(ViewEvent::Pressed {
                at,
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
                time: 0.0,
            });
            assert!(r.intent.is_none() && !rig.g.is_pressed());
            assert!(
                rig.send(ViewEvent::Wheel {
                    at: [1.0, 1.0],
                    points: f64::NAN
                })
                .camera
                .is_none()
            );
        }
        // Far outside the image the picks are clamped to it.
        let rs = rig.gesture(
            PointerButton::Primary,
            Modifiers::NONE,
            &[[-1e300, -1e300], [1e300, 1e300]],
            0.0,
        );
        assert_eq!(only_intent(&rs), Some(intent(&[1], "replace", "window")));
        let r = rig.click([1e300, -1e300], Modifiers::NONE);
        assert!(r.intent.is_none());
    }

    proptest! {
        /// Any event stream yields at most one selection intent per release or Escape, and
        /// none from moves, wheels, pinches, presses or other keys.
        #[test]
        fn selection_changes_only_through_one_intent_per_gesture(
            events in proptest::collection::vec((0u8..7, -20.0f64..120.0, -20.0f64..120.0, 0u8..4), 0..60),
        ) {
            let mut rig = Rig::new(&[(1, [10, 10, 30, 30], 0), (2, [20, 20, 60, 60], 1)]);
            rig.selected = 1;
            for (kind, x, y, m) in events {
                let modifiers = [Modifiers::NONE, Modifiers::SHIFT, Modifiers::COMMAND, Modifiers::ALT][usize::from(m)];
                let button = if m == 3 { PointerButton::Middle } else { PointerButton::Primary };
                let e = match kind {
                    0 => ViewEvent::Moved([x, y]),
                    1 => ViewEvent::Pressed { at: [x, y], button, modifiers, time: x },
                    2 => ViewEvent::Released { at: [x, y], button },
                    3 => ViewEvent::Wheel { at: [x, y], points: y },
                    4 => ViewEvent::Zoom { at: [x, y], factor: 1.0 + x / 200.0 },
                    5 => ViewEvent::Key { key: [Key::Escape, Key::Home, Key::ArrowUp, Key::A][usize::from(m)], modifiers },
                    _ => ViewEvent::Left,
                };
                let r = rig.send(e);
                let ends = matches!(e, ViewEvent::Released { .. } | ViewEvent::Key { key: Key::Escape, .. });
                prop_assert!(r.intent.is_none() || ends);
                if let Some(i) = r.intent {
                    prop_assert!(i.command == DEFAULT_SELECT || i.command == DEFAULT_CLEAR_SELECTION);
                }
            }
        }
    }
}
