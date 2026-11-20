//! Product hooks of the viewport (CORE-UI-005): pointer events in model coordinates, the
//! projection between pixels and model coordinates, and overlays painted in model coordinates.
//!
//! A product gives the viewport a [`PointerHandler`]. The widget offers it each pointer event
//! before its own gesture handling; a handler that returns `true` consumes the event, and the
//! widget does not answer it. The widget's navigation (pan, orbit, zoom) stays with the widget
//! unless the handler consumes the event. A press that the handler consumes belongs to the
//! handler until its release.
//!
//! [`ViewProjection`] is the same mapping the widget uses for its own picking, so a product
//! converts pixels to model coordinates and back for the view it was given. [`ProductOverlay`]
//! primitives are painted over the scene with the theme's overlay colors; a snap marker reuses
//! [`crate::snap::paint`].

use arc_base::math::Point3;
use arc_geom::view::ViewFrame;
use egui::{Color32, FontId, Modifiers, Pos2, Rect, Stroke, Ui};

use crate::gesture::{BandKind, plane_point, work_point};
use crate::snap::{self, Snap, SnapKind};
use crate::text::Localize;
use crate::theme::Colors;

/// Overlay primitives painted in one frame; more are not drawn.
pub const MAX_OVERLAYS: usize = 256;
/// Points of one overlay primitive; more are not drawn.
pub const MAX_POINTS: usize = 4096;
/// Dash and gap lengths of dashed lines, in logical points.
const DASH: f32 = 6.0;
const GAP: f32 = 4.0;
/// Dashes drawn per segment; a longer segment shows this many.
const MAX_DASHES: usize = 512;
/// The alpha of band fills.
const BAND_FILL_ALPHA: u8 = 40;
/// The opacity of the plate behind a text label, which keeps it readable over lines.
const LABEL_PLATE_ALPHA: u8 = 150;

/// The mapping between pixels of a `width × height` image and model coordinates, for one camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewProjection {
    frame: ViewFrame,
    width: u32,
    height: u32,
    work: Option<WorkPlane>,
}

/// The plane a product's point input lands on: a click answering a point prompt and
/// [`ViewProjection::pixel_to_model`] meet it instead of the plane through the camera target (a
/// storey's level plane, for example). The normal needs no unit length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkPlane {
    /// A point on the plane.
    pub origin: Point3,
    /// The plane's normal.
    pub normal: arc_base::math::Vector3,
}

impl ViewProjection {
    /// The projection of `frame` on an image of `width × height` pixels.
    #[must_use]
    pub fn new(frame: ViewFrame, width: u32, height: u32) -> Self {
        Self {
            frame,
            width,
            height,
            work: None,
        }
    }

    /// The same projection with the plane [`Self::pixel_to_model`] meets; `None` is the plane
    /// through the camera target that faces the eye.
    #[must_use]
    pub fn with_work_plane(mut self, plane: Option<WorkPlane>) -> Self {
        self.work = plane;
        self
    }

    /// The camera.
    #[must_use]
    pub fn frame(&self) -> &ViewFrame {
        &self.frame
    }

    /// The image size in pixels.
    #[must_use]
    pub fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    /// The model point under the pixel `at` (x right, y down, from the top-left corner): the
    /// point where the pick ray meets the work plane, or the plane through the target that faces
    /// the eye without one. `None` when the result is not finite (a ray parallel to the plane).
    #[must_use]
    pub fn pixel_to_model(&self, at: [f64; 2]) -> Option<Point3> {
        match &self.work {
            Some(plane) => work_point(&self.frame, self.width, self.height, at, plane).ok(),
            None => plane_point(&self.frame, self.width, self.height, at).ok(),
        }
    }

    /// The pixel of a model point, or `None` for a point behind the eye or outside the range of
    /// the camera.
    #[must_use]
    pub fn model_to_pixel(&self, p: &Point3) -> Option<[f64; 2]> {
        let (w, h) = (f64::from(self.width.max(1)), f64::from(self.height.max(1)));
        let ndc = self.frame.project(p, w / h)?;
        Some([(ndc.x + 1.0) * w / 2.0, (1.0 - ndc.y) * h / 2.0])
    }

    /// The pixels that span one model unit on the plane through the target: a length in model
    /// units times this value is a length in pixels. `None` when the camera gives no finite
    /// scale.
    #[must_use]
    pub fn pixels_per_unit(&self) -> Option<f64> {
        let a = plane_point(&self.frame, self.width, self.height, [0.0, 0.0]).ok()?;
        let b = plane_point(&self.frame, self.width, self.height, [1.0, 0.0]).ok()?;
        let units = (b - a).norm();
        (units.is_finite() && units > 0.0).then(|| 1.0 / units)
    }
}

/// The kind of a pointer event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    /// The pointer moved.
    Move,
    /// A button went down.
    Press,
    /// A button went up.
    Release,
    /// A button went down and up without a drag beyond the drag threshold (primary button).
    Click,
}

/// A pointer event for a product, in model coordinates of the viewed space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerEvent {
    /// The kind.
    pub kind: PointerKind,
    /// The button; `None` for a move.
    pub button: Option<egui::PointerButton>,
    /// The modifiers held.
    pub modifiers: Modifiers,
    /// The pixel position (x right, y down, from the top-left corner of the image).
    pub pixel: [f64; 2],
    /// The model point under the pixel, `None` when it has none.
    pub model: Option<Point3>,
    /// The pixels that span one model unit, so a pixel aperture converts to model units by
    /// dividing by it; `None` when the camera gives no scale.
    pub pixels_per_unit: Option<f64>,
}

/// A product's handler of pointer events. Returns `true` to consume the event; a consumed event
/// is not handled by the widget.
pub trait PointerHandler {
    /// Handles one pointer event.
    fn pointer(&mut self, event: &PointerEvent) -> bool;
}

/// A primitive a product draws over the scene, in model coordinates. The widget projects each
/// point through the current camera; a point that the camera cannot project is skipped.
#[derive(Debug, Clone, PartialEq)]
pub enum ProductOverlay {
    /// A straight line, such as a rubber band.
    Line {
        /// One end.
        from: Point3,
        /// The other end.
        to: Point3,
    },
    /// A rectangle between two model points, drawn as the axis-aligned rectangle of their
    /// projections on the screen. A window band is a solid outline, a crossing band a dashed one.
    Band {
        /// One corner.
        start: Point3,
        /// The opposite corner.
        end: Point3,
        /// The kind of band; it selects the style.
        kind: BandKind,
    },
    /// A polyline, such as a tracking path; dashed when `dashed` is set.
    Path {
        /// The points in order.
        points: Vec<Point3>,
        /// Whether the line is dashed.
        dashed: bool,
    },
    /// A snap marker of `kind` at a model point, with the kind's name beside it.
    Snap {
        /// The kind; it selects the marker shape and the name.
        kind: SnapKind,
        /// The snapped model point.
        at: Point3,
    },
    /// A text label at a model point, shown as given (the product localizes it).
    Label {
        /// The anchor point.
        at: Point3,
        /// The text.
        text: String,
    },
}

/// The screen position of a model point in logical points, given the image rectangle.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a position in logical points, far inside the range of f32"
)]
fn screen(projection: &ViewProjection, rect: Rect, ppp: f64, p: &Point3) -> Option<Pos2> {
    let px = projection.model_to_pixel(p)?;
    let to_point = |v: f64| {
        let v = v / ppp;
        if v.is_finite() { v as f32 } else { 0.0 }
    };
    Some(Pos2::new(
        rect.min.x + to_point(px[0]),
        rect.min.y + to_point(px[1]),
    ))
}

/// Paints `overlays` over the viewport image at `rect`, whose logical points are `ppp` pixels
/// each. At most [`MAX_OVERLAYS`] primitives and [`MAX_POINTS`] points per primitive are drawn.
pub fn paint_overlays(
    ui: &Ui,
    localize: &dyn Localize,
    colors: &Colors,
    rect: Rect,
    ppp: f64,
    projection: &ViewProjection,
    overlays: &[ProductOverlay],
) {
    let painter = ui.painter_at(rect);
    let at = |p: &Point3| screen(projection, rect, ppp, p);
    let style = |kind: BandKind| match kind {
        BandKind::Window => colors.viewport_selection,
        BandKind::Crossing => colors.viewport_preselection,
    };
    let mut placed: Vec<Rect> = Vec::new();
    for overlay in overlays.iter().take(MAX_OVERLAYS) {
        match overlay {
            ProductOverlay::Line { from, to } => {
                if let (Some(a), Some(b)) = (at(from), at(to)) {
                    let stroke = Stroke::new(1.0, colors.viewport_preselection);
                    painter.line_segment([a, b], stroke);
                }
            }
            ProductOverlay::Band { start, end, kind } => {
                let (Some(a), Some(b)) = (at(start), at(end)) else {
                    continue;
                };
                let rect = Rect::from_two_pos(a, b);
                let color = style(*kind);
                let fill = Color32::from_rgba_unmultiplied(
                    color.r(),
                    color.g(),
                    color.b(),
                    BAND_FILL_ALPHA,
                );
                painter.rect_filled(rect, 0.0, fill);
                let stroke = Stroke::new(1.0, color);
                let corners = [
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                ];
                for (p, q) in corners.iter().zip(corners.iter().cycle().skip(1)) {
                    let (p, q) = (*p, *q);
                    match kind {
                        BandKind::Window => {
                            painter.line_segment([p, q], stroke);
                        }
                        BandKind::Crossing => dashed(&painter, [p, q], stroke),
                    }
                }
            }
            ProductOverlay::Path {
                points,
                dashed: is_dashed,
            } => {
                let stroke = Stroke::new(1.0, colors.viewport_preselection);
                let mut screen_points = points.iter().take(MAX_POINTS).filter_map(at);
                let mut previous = screen_points.next();
                for next in screen_points {
                    if let Some(p) = previous {
                        if *is_dashed {
                            dashed(&painter, [p, next], stroke);
                        } else {
                            painter.line_segment([p, next], stroke);
                        }
                    }
                    previous = Some(next);
                }
            }
            ProductOverlay::Snap { kind, at: p } => {
                if let Some(position) = at(p) {
                    snap::paint(
                        ui,
                        localize,
                        Some(&Snap {
                            kind: *kind,
                            position,
                        }),
                        &[],
                    );
                }
            }
            ProductOverlay::Label { at: p, text } => {
                if let Some(position) = at(p) {
                    paint_label(&painter, colors, position, text, &mut placed);
                }
            }
        }
    }
}

/// Paints a text label beside `position`, on a plate that keeps it readable over lines. A label
/// that would cover one of `placed` steps down until it does not, so labels of anchors that
/// project close together (levels seen from above) stay readable, in the order they were given.
fn paint_label(
    painter: &egui::Painter,
    colors: &Colors,
    position: Pos2,
    text: &str,
    placed: &mut Vec<Rect>,
) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::proportional(12.0),
        colors.viewport_selection,
    );
    let size = galley.size();
    let mut rect = Rect::from_min_size(position + egui::vec2(8.0, -8.0 - size.y), size);
    for _ in 0..MAX_OVERLAYS {
        if !placed.iter().any(|r| r.expand(1.0).intersects(rect)) {
            break;
        }
        rect = rect.translate(egui::vec2(0.0, size.y + 2.0));
    }
    placed.push(rect);
    painter.rect_filled(
        rect.expand(2.0),
        2.0,
        Color32::from_black_alpha(LABEL_PLATE_ALPHA),
    );
    painter.galley(rect.min, galley, colors.viewport_selection);
}

/// Draws the segment `from`–`to` as dashes, at most [`MAX_DASHES`] of them.
fn dashed(painter: &egui::Painter, [from, to]: [Pos2; 2], stroke: Stroke) {
    let delta = to - from;
    let len = delta.length();
    if !len.is_finite() || len <= 0.0 {
        return;
    }
    let period = (DASH + GAP).max(len / MAX_DASHES as f32);
    let dash = period * DASH / (DASH + GAP);
    let unit = delta / len;
    let mut s = 0.0_f32;
    while s < len {
        let e = (s + dash).min(len);
        painter.line_segment([from + unit * s, from + unit * e], stroke);
        s += period;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamic_input::{DynamicEvent, DynamicField};
    use crate::gesture::ViewportCommands;
    use crate::gesture::tests::{Fake, top};
    use crate::theme::ThemeId;
    use crate::theme::UiScale;
    use crate::viewport::{
        DynamicPanel, ProductInput, ViewportInput, ViewportOutput, ViewportWidget,
    };
    use arc_base::Tolerance;
    use arc_base::error::Value;
    use arc_base::math::Vector3;
    use arc_geom::view::{Projection, ViewFrame};
    use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Shape, vec2};

    struct NoText;

    impl Localize for NoText {
        fn message(&self, _: &str, _: &[(&str, Value)]) -> Option<String> {
            None
        }
    }

    /// Records the events it is offered and consumes them when `consume` is set.
    struct Recorder {
        consume: bool,
        seen: Vec<PointerEvent>,
    }

    impl PointerHandler for Recorder {
        fn pointer(&mut self, event: &PointerEvent) -> bool {
            self.seen.push(*event);
            self.consume
        }
    }

    fn screen(events: Vec<Event>) -> RawInput {
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0))),
            events,
            ..RawInput::default()
        }
    }

    /// Runs one frame of a 100 × 100 viewport looking down at the origin, one unit per pixel.
    fn frame_with(
        ctx: &egui::Context,
        events: Vec<Event>,
        widget: &mut ViewportWidget,
        picker: &mut Fake,
        mut handler: Option<&mut dyn PointerHandler>,
        overlays: &[ProductOverlay],
        dynamic: Option<DynamicPanel<'_>>,
    ) -> (ViewportOutput, Vec<egui::epaint::ClippedShape>) {
        let mut out = None;
        let mut full = ctx.run_ui(screen(events), |ui| {
            let frame = top();
            let commands = ViewportCommands::default();
            let colors = ThemeId::Light.theme().colors;
            let tolerance = Tolerance::DEFAULT;
            out = Some(widget.show(
                ui,
                ViewportInput {
                    texture: None,
                    frame: &frame,
                    prompt: None,
                    selected: 0,
                    picker,
                    commands: &commands,
                    localize: &NoText,
                    colors: &colors,
                    scale: UiScale::default(),
                    tolerance: &tolerance,
                    name: "",
                    time: 0.0,
                    product: Some(ProductInput {
                        pointer: match handler.as_mut() {
                            Some(h) => Some(&mut **h as &mut dyn PointerHandler),
                            None => None,
                        },
                        overlays,
                        dynamic,
                        work_plane: None,
                    }),
                },
            ));
        });
        full.textures_delta.clear();
        (out.expect("the frame ran"), full.shapes)
    }

    fn click_at(x: f32, y: f32) -> Vec<Event> {
        let at = Pos2::new(x, y);
        vec![
            Event::PointerMoved(at),
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ]
    }

    fn object_at_center() -> Fake {
        Fake::new(&[(7, [40, 40, 60, 60], 0)])
    }

    #[test]
    fn a_consumed_click_is_not_answered_by_the_widget() {
        let ctx = egui::Context::default();
        let mut widget = ViewportWidget::new();
        let mut picker = object_at_center();
        // Without a handler the click selects the object under it.
        let (plain, _) = frame_with(
            &ctx,
            click_at(50.0, 50.0),
            &mut widget,
            &mut picker,
            None,
            &[],
            None,
        );
        assert!(plain.intent.is_some());

        let mut widget = ViewportWidget::new();
        let mut picker = object_at_center();
        let mut recorder = Recorder {
            consume: true,
            seen: Vec::new(),
        };
        let (consumed, _) = frame_with(
            &ctx,
            click_at(50.0, 50.0),
            &mut widget,
            &mut picker,
            Some(&mut recorder),
            &[],
            None,
        );
        assert!(consumed.intent.is_none());
        assert!(consumed.answer.is_none());
        let kinds: Vec<PointerKind> = recorder.seen.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            [
                PointerKind::Move,
                PointerKind::Press,
                PointerKind::Release,
                PointerKind::Click
            ]
        );
        let click = recorder.seen.last().copied().expect("a click");
        let model = click.model.expect("the pixel has a model point");
        assert!(model.coords.norm() < 1e-9, "the center is the origin");
        assert_eq!(click.button, Some(PointerButton::Primary));
        assert!((click.pixels_per_unit.expect("a scale") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn an_unconsumed_click_reaches_the_widget() {
        let ctx = egui::Context::default();
        let mut widget = ViewportWidget::new();
        let mut picker = object_at_center();
        let mut recorder = Recorder {
            consume: false,
            seen: Vec::new(),
        };
        let (out, _) = frame_with(
            &ctx,
            click_at(50.0, 50.0),
            &mut widget,
            &mut picker,
            Some(&mut recorder),
            &[],
            None,
        );
        assert!(out.intent.is_some());
        assert!(recorder.seen.iter().any(|e| e.kind == PointerKind::Click));
    }

    #[test]
    fn a_consumed_press_keeps_the_band_from_starting() {
        let ctx = egui::Context::default();
        let mut widget = ViewportWidget::new();
        let mut picker = Fake::new(&[]);
        let mut recorder = Recorder {
            consume: true,
            seen: Vec::new(),
        };
        let at = Pos2::new(10.0, 10.0);
        let drag = vec![
            Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            Event::PointerMoved(Pos2::new(90.0, 90.0)),
            Event::PointerButton {
                pos: Pos2::new(90.0, 90.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ];
        let (out, _) = frame_with(
            &ctx,
            drag,
            &mut widget,
            &mut picker,
            Some(&mut recorder),
            &[],
            None,
        );
        assert!(out.intent.is_none());
        assert!(widget.gestures().band().is_none());
        assert!(!widget.gestures().is_pressed());
        // The product owns the press: the moves and the release reach it.
        assert!(recorder.seen.iter().any(|e| e.kind == PointerKind::Release));
    }

    #[test]
    fn a_consumed_move_does_not_preselect() {
        let ctx = egui::Context::default();
        let mut widget = ViewportWidget::new();
        let mut picker = object_at_center();
        let mut recorder = Recorder {
            consume: true,
            seen: Vec::new(),
        };
        let moved = vec![Event::PointerMoved(Pos2::new(50.0, 50.0))];
        let (out, _) = frame_with(
            &ctx,
            moved,
            &mut widget,
            &mut picker,
            Some(&mut recorder),
            &[],
            None,
        );
        assert!(out.hover.is_none());
        assert_eq!(picker.picks, 0);
    }

    #[test]
    fn pixels_and_model_points_round_trip() {
        for frame in [top(), perspective()] {
            let projection = ViewProjection::new(frame, 120, 80);
            for at in [[0.5, 0.5], [10.0, 20.0], [60.0, 40.0], [119.0, 79.0]] {
                let model = projection.pixel_to_model(at).expect("a model point");
                let back = projection.model_to_pixel(&model).expect("a pixel");
                assert!((back[0] - at[0]).abs() < 1e-6, "{back:?} vs {at:?}");
                assert!((back[1] - at[1]).abs() < 1e-6, "{back:?} vs {at:?}");
            }
            assert!(projection.pixels_per_unit().is_some_and(|v| v > 0.0));
        }
    }

    #[test]
    fn a_work_plane_moves_the_model_point_onto_it() {
        let plane = WorkPlane {
            origin: Point3::new(0.0, 0.0, 6.0),
            normal: Vector3::z(),
        };
        let projection = ViewProjection::new(perspective(), 120, 80).with_work_plane(Some(plane));
        for at in [[10.0, 20.0], [60.0, 40.0]] {
            let p = projection.pixel_to_model(at).expect("a point on the plane");
            assert!((p.z - 6.0).abs() < 1e-9, "{p:?}");
            let back = projection.model_to_pixel(&p).expect("a pixel");
            assert!((back[0] - at[0]).abs() < 1e-6 && (back[1] - at[1]).abs() < 1e-6);
        }
        // A plane the rays run parallel to gives no point.
        let edge = WorkPlane {
            origin: Point3::origin(),
            normal: Vector3::x(),
        };
        let sideways = ViewProjection::new(top(), 100, 100).with_work_plane(Some(edge));
        assert!(sideways.pixel_to_model([50.0, 50.0]).is_none());
    }

    fn perspective() -> ViewFrame {
        ViewFrame::new(
            arc_base::math::Point3::new(3.0, -2.0, 40.0),
            arc_base::math::Point3::origin(),
            Vector3::y(),
            Projection::Perspective { vertical_fov: 0.7 },
            0.5,
            500.0,
        )
        .expect("a valid camera")
    }

    #[test]
    fn the_output_carries_the_projection_of_the_camera() {
        let ctx = egui::Context::default();
        let mut widget = ViewportWidget::new();
        let mut picker = Fake::new(&[]);
        let (out, _) = frame_with(&ctx, Vec::new(), &mut widget, &mut picker, None, &[], None);
        assert_eq!(out.projection, ViewProjection::new(top(), 100, 100));
        let p = out.projection.pixel_to_model([50.0, 50.0]).expect("center");
        assert!(p.coords.norm() < 1e-9);
    }

    fn colored(shapes: &[egui::epaint::ClippedShape]) -> (usize, usize, usize, usize) {
        let (mut lines, mut rects, mut texts, mut fills) = (0, 0, 0, 0);
        for clipped in shapes {
            match &clipped.shape {
                Shape::LineSegment { .. } => lines += 1,
                Shape::Rect(rect) => {
                    rects += 1;
                    if rect.fill.a() == BAND_FILL_ALPHA {
                        fills += 1;
                    }
                }
                Shape::Text(_) => texts += 1,
                _ => {}
            }
        }
        (lines, rects, texts, fills)
    }

    #[test]
    fn overlay_primitives_are_painted_in_model_coordinates() {
        let ctx = egui::Context::default();
        let mut widget = ViewportWidget::new();
        let mut picker = Fake::new(&[]);
        let p = |x: f64, y: f64| arc_base::math::Point3::new(x, y, 0.0);
        let overlays = [
            ProductOverlay::Line {
                from: p(-20.0, -20.0),
                to: p(20.0, 20.0),
            },
            ProductOverlay::Band {
                start: p(-30.0, -30.0),
                end: p(-10.0, -10.0),
                kind: BandKind::Window,
            },
            ProductOverlay::Band {
                start: p(10.0, 10.0),
                end: p(30.0, 30.0),
                kind: BandKind::Crossing,
            },
            ProductOverlay::Path {
                points: vec![p(0.0, 0.0), p(0.0, 40.0)],
                dashed: true,
            },
            ProductOverlay::Snap {
                kind: SnapKind::Midpoint,
                at: p(0.0, 0.0),
            },
            ProductOverlay::Label {
                at: p(5.0, 5.0),
                text: "5".into(),
            },
        ];
        let (_, shapes) = frame_with(
            &ctx,
            Vec::new(),
            &mut widget,
            &mut picker,
            None,
            &overlays,
            None,
        );
        let (lines, rects, texts, fills) = colored(&shapes);
        assert!(lines > 1 + 4, "line, window outline, dashes: {lines}");
        assert!(rects >= 2, "two band fills: {rects}");
        assert_eq!(fills, 2, "both bands are filled with the overlay tint");
        assert!(texts >= 1, "the label");
        // Without overlays the viewport paints nothing of the overlay style.
        let mut widget = ViewportWidget::new();
        let (_, bare) = frame_with(&ctx, Vec::new(), &mut widget, &mut picker, None, &[], None);
        assert_eq!(colored(&bare).3, 0);
    }

    #[test]
    fn dynamic_input_edits_reach_the_product() {
        let ctx = egui::Context::default();
        let fields = [DynamicField {
            label: "label-distance".into(),
            text: String::new(),
            locked: false,
            focused: true,
        }];
        let prompt = "prompt-point";
        let viewport = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        let pointer = Pos2::new(50.0, 50.0);
        // The first frame gives the field the focus; the second types into it.
        let mut first = ctx.run_ui(screen(Vec::new()), |ui| {
            let _ = crate::dynamic_input::show_fields(
                ui.ctx(),
                &NoText,
                prompt,
                viewport,
                pointer,
                &fields,
            );
        });
        first.textures_delta.clear();
        let typed = vec![
            Event::Text("5".into()),
            Event::Key {
                key: Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ];
        let mut events = Vec::new();
        let mut second = ctx.run_ui(screen(typed), |ui| {
            events = crate::dynamic_input::show_fields(
                ui.ctx(),
                &NoText,
                prompt,
                viewport,
                pointer,
                &fields,
            );
        });
        second.textures_delta.clear();
        assert!(events.contains(&DynamicEvent::Edited {
            index: 0,
            text: "5".into()
        }));
        assert!(events.contains(&DynamicEvent::Tab { index: 0 }));
    }

    #[test]
    fn the_viewport_passes_dynamic_input_through_its_output() {
        let ctx = egui::Context::default();
        let mut widget = ViewportWidget::new();
        let mut picker = Fake::new(&[]);
        let fields = [DynamicField {
            label: "label-distance".into(),
            text: "2".into(),
            locked: true,
            focused: true,
        }];
        let panel = DynamicPanel {
            prompt: "prompt-point",
            fields: &fields,
        };
        let _ = frame_with(
            &ctx,
            Vec::new(),
            &mut widget,
            &mut picker,
            None,
            &[],
            Some(panel),
        );
        let typed = vec![
            Event::Text("3".into()),
            Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            },
        ];
        let (out, _) = frame_with(
            &ctx,
            typed,
            &mut widget,
            &mut picker,
            None,
            &[],
            Some(panel),
        );
        assert!(
            out.dynamic
                .iter()
                .any(|e| matches!(e, DynamicEvent::Enter { index: 0 }))
        );
        assert!(
            out.dynamic
                .iter()
                .any(|e| matches!(e, DynamicEvent::Edited { index: 0, .. }))
        );
    }
}
