//! The view cube (CORE-GPU-025 leaves it to the UI kit).
//!
//! [`ViewCube::show_over`] draws a small cube in the corner of a viewport, oriented like the
//! current camera, with the localized name of each visible face. The cube has 26 targets: six
//! faces, twelve edges and eight corners, each a direction from the view target toward the eye
//! with components in `{-1, 0, 1}` ([`CubeTarget`]). A click, or Enter or Space on a focused
//! target, returns a [`ViewIntent`]; the caller turns it into a camera with
//! [`ViewIntent::frame_from`] and animates with `arc_gpu::navigation::interpolate`. The widget
//! never moves the camera itself.
//!
//! # Conventions
//!
//! The world is Z-up. The face names are Top (+Z), Bottom (-Z), Front (-Y), Back (+Y), Right
//! (+X) and Left (-X). Looking at the top or bottom face the up direction is +Y; every other
//! target looks with +Z up.
//!
//! # Hit rule
//!
//! The cube is drawn in orthographic projection. The pointer position is mapped to a point on a
//! face facing the eye (a face is visible when its normal has a positive component toward the
//! eye) by solving the two-by-two linear system of the face's projected axes; faces edge-on to
//! the eye are never hit. On that face, a point within [`EDGE_BAND`] (as a fraction of the half
//! width) of one edge selects that edge's target, within the band of two edges the corner, and
//! anywhere else the face. [`hit`] is the pure form of this rule.
//!
//! # Accessibility
//!
//! The cube is a group named by `ui-view-cube`; each of the 26 targets is a focusable button
//! named from the face names (`ui-view-cube-edge`, `ui-view-cube-corner`). Hidden targets are
//! reachable by keyboard like visible ones. Only the pointer-hovered region is highlighted; a
//! focused target is shown by the focus indicator of the whole cube.

use arc_base::Result;
use arc_base::error::Value;
use arc_base::math::{Point3, Vector3};
use arc_geom::view::ViewFrame;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, TextStyle, Ui, WidgetInfo};

use crate::text::{Localize, Message, messages, resolve};
use crate::theme::Colors;

/// The part of a face, from each edge inward and as a fraction of the half width, that selects
/// the edge or corner instead of the face.
pub const EDGE_BAND: f64 = 0.3;
/// Faces whose normal points less than this toward the eye (as a cosine) get no label.
const LABEL_FACING: f64 = 0.35;
/// The distance of the cube from the viewport's top and right edges, in points.
const MARGIN: f32 = 8.0;

/// A face, edge or corner of the cube: the direction from the view target toward the eye in
/// world axes, each component -1, 0 or 1 and not all zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CubeTarget(pub [i8; 3]);

/// What kind of part a target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// One non-zero component.
    Face,
    /// Two non-zero components.
    Edge,
    /// Three non-zero components.
    Corner,
}

impl CubeTarget {
    /// The target for `direction`, or `None` when a component is outside -1..=1 or all are 0.
    #[must_use]
    pub fn new(direction: [i8; 3]) -> Option<Self> {
        let valid = direction.iter().all(|c| (-1..=1).contains(c)) && direction != [0; 3];
        valid.then_some(Self(direction))
    }

    /// All 26 targets: faces first, then edges, then corners.
    #[must_use]
    pub fn all() -> Vec<Self> {
        let mut all: Vec<Self> = (-1..=1)
            .flat_map(|x| (-1..=1).flat_map(move |y| (-1..=1).map(move |z| [x, y, z])))
            .filter_map(Self::new)
            .collect();
        all.sort_by_key(|t| (t.kind() as u8, *t));
        all
    }

    /// Face, edge or corner.
    #[must_use]
    pub fn kind(self) -> TargetKind {
        match self.0.iter().filter(|c| **c != 0).count() {
            1 => TargetKind::Face,
            2 => TargetKind::Edge,
            _ => TargetKind::Corner,
        }
    }

    /// The direction toward the eye as a unit vector.
    #[must_use]
    pub fn direction(self) -> Vector3 {
        let [x, y, z] = self.0.map(f64::from);
        Vector3::new(x, y, z).normalize()
    }

    /// The up direction the view uses: +Y when looking along the Z axis, else +Z.
    #[must_use]
    pub fn up(self) -> Vector3 {
        if self.0[0] == 0 && self.0[1] == 0 {
            Vector3::y()
        } else {
            Vector3::z()
        }
    }

    /// The faces this target touches, vertical axis first, then Y, then X.
    #[must_use]
    pub fn faces(self) -> Vec<Message> {
        let [x, y, z] = self.0;
        let mut faces = Vec::new();
        if z != 0 {
            faces.push(if z > 0 {
                messages::VIEW_TOP
            } else {
                messages::VIEW_BOTTOM
            });
        }
        if y != 0 {
            faces.push(if y > 0 {
                messages::VIEW_BACK
            } else {
                messages::VIEW_FRONT
            });
        }
        if x != 0 {
            faces.push(if x > 0 {
                messages::VIEW_RIGHT
            } else {
                messages::VIEW_LEFT
            });
        }
        faces
    }

    /// The localized accessible name: the face name, or the composite edge or corner name.
    #[must_use]
    pub fn name(self, localize: &dyn Localize) -> String {
        let names: Vec<String> = self.faces().iter().map(|f| f.text(localize)).collect();
        let arg = |i: usize, key: &'static str| {
            (key, Value::Text(names.get(i).cloned().unwrap_or_default()))
        };
        match self.kind() {
            TargetKind::Face => names.into_iter().next().unwrap_or_default(),
            TargetKind::Edge => resolve(
                localize,
                messages::VIEW_EDGE.message_id,
                &[arg(0, "first"), arg(1, "second")],
            ),
            TargetKind::Corner => resolve(
                localize,
                messages::VIEW_CORNER.message_id,
                &[arg(0, "first"), arg(1, "second"), arg(2, "third")],
            ),
        }
    }
}

/// The request to look at the model from a [`CubeTarget`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewIntent {
    /// The clicked or activated part.
    pub target: CubeTarget,
    /// The unit direction from the view target toward the eye.
    pub direction: Vector3,
    /// The up hint of the new view.
    pub up: Vector3,
}

impl ViewIntent {
    /// The intent for `target`.
    #[must_use]
    pub fn new(target: CubeTarget) -> Self {
        Self {
            target,
            direction: target.direction(),
            up: target.up(),
        }
    }

    /// The camera that looks from this direction: `current` with the eye moved around its target
    /// at the same distance and the up hint replaced; projection, near and far are kept. Pass it
    /// to `arc_gpu::navigation::interpolate` with `current` for an animated transition.
    ///
    /// # Errors
    ///
    /// The view-frame errors of `arc-geom` when the result cannot be represented.
    pub fn frame_from(&self, current: &ViewFrame) -> Result<ViewFrame> {
        let target = current.target();
        let distance = (current.eye() - target).norm();
        ViewFrame::new(
            Point3::from(target.coords + self.direction * distance),
            target,
            self.up,
            current.projection(),
            current.near(),
            current.far(),
        )
    }
}

/// Radians of orbit per point of cube drag.
pub const DRAG_RADIANS_PER_POINT: f64 = 0.01;
/// A view whose direction is this close to the Z axis (as a cosine) is a plan view.
const PLAN_COSINE: f64 = 0.999_9;

/// The camera after the cube is dragged by `drag` points (x right, y down): an orbit about the
/// view target, dragging right gives `yaw = -dx·k` and down `pitch = -dy·k` as in the viewport's
/// navigation. A plan view keeps its up direction (+Y) and only tilts toward the front
/// (dragging up), so the drawing does not turn over. Once the view
/// has left the plan the up direction is +Z, as for every non-plan target of the cube, so a
/// horizontal drag turns the model about the vertical axis. A plan view has no horizontal turn.
///
/// # Errors
///
/// The errors of `arc_gpu::navigation::orbit` and of the view frame.
pub fn orbit_by_drag(
    frame: &ViewFrame,
    drag: [f32; 2],
    tolerance: &arc_base::Tolerance,
) -> Result<ViewFrame> {
    let (yaw, pitch) = (
        -f64::from(drag[0]) * DRAG_RADIANS_PER_POINT,
        -f64::from(drag[1]) * DRAG_RADIANS_PER_POINT,
    );
    let rehint = |up: Vector3| {
        ViewFrame::new(
            frame.eye(),
            frame.target(),
            up,
            frame.projection(),
            frame.near(),
            frame.far(),
        )
    };
    let looking_down = frame.forward().z < 0.0;
    let (base, yaw, pitch) = if frame.forward().z.abs() >= PLAN_COSINE {
        // Dragging up (pitch > 0) lowers the eye on the -Y side of a top view, as dragging up
        // does in any view; dragging down leaves the plan where it is. A bottom view mirrors it.
        let lean = if looking_down {
            pitch.max(0.0)
        } else {
            pitch.min(0.0)
        };
        (rehint(Vector3::y())?, 0.0, lean)
    } else if frame.up_hint().z.abs() < 0.5 {
        (rehint(Vector3::z())?, yaw, pitch)
    } else {
        (*frame, yaw, pitch)
    };
    arc_gpu::navigation::orbit(&base, &base.target(), yaw, pitch, tolerance)
}

/// The part under a point and the face it lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// The face, edge or corner.
    pub target: CubeTarget,
    /// The visible face the point lies on: its axis (0 to 2).
    pub axis: usize,
    /// The sign of that face's normal.
    pub sign: i8,
}

/// Right, up and toward-the-eye directions of `frame` as arrays.
fn basis(frame: &ViewFrame) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let arr = |v: Vector3| [v.x, v.y, v.z];
    (arr(frame.right()), arr(frame.up()), arr(-frame.forward()))
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The tangent axes of the face with normal axis `axis`.
fn tangents(axis: usize) -> (usize, usize) {
    ((axis + 1) % 3, (axis + 2) % 3)
}

/// The point on the unit-half-size cube face (`axis`, `sign`) at face coordinates `(u, v)`.
fn face_point(axis: usize, sign: i8, u: f64, v: f64) -> [f64; 3] {
    let (a, b) = tangents(axis);
    let mut p = [0.0; 3];
    p[axis] = f64::from(sign);
    p[a] = u;
    p[b] = v;
    p
}

/// The view-plane position (x right, y up, in cube half-widths) of a cube point.
#[must_use]
pub fn project(frame: &ViewFrame, p: [f64; 3]) -> [f64; 2] {
    let (right, up, _) = basis(frame);
    [dot(right, p), dot(up, p)]
}

/// The cube part drawn at the view-plane position `q` (x right, y up, in cube half-widths from
/// the cube's center) for the camera orientation of `frame`; `None` outside the silhouette.
#[must_use]
pub fn hit(frame: &ViewFrame, q: [f64; 2]) -> Option<Hit> {
    let (right, up, eye) = basis(frame);
    for axis in 0..3 {
        for sign in [-1_i8, 1] {
            let mut normal = [0.0; 3];
            normal[axis] = f64::from(sign);
            if dot(eye, normal) <= 1e-9 {
                continue;
            }
            let (a, b) = tangents(axis);
            let proj = |p: [f64; 3]| [dot(right, p), dot(up, p)];
            let n = proj(face_point(axis, sign, 0.0, 0.0));
            let au = proj(face_point(axis, 0, 1.0, 0.0));
            let bv = proj(face_point(axis, 0, 0.0, 1.0));
            let det = au[0] * bv[1] - au[1] * bv[0];
            if det.abs() < 1e-9 {
                continue;
            }
            let d = [q[0] - n[0], q[1] - n[1]];
            let u = (d[0] * bv[1] - d[1] * bv[0]) / det;
            let v = (au[0] * d[1] - au[1] * d[0]) / det;
            if u.abs() <= 1.0 && v.abs() <= 1.0 {
                let mut dir = [0_i8; 3];
                dir[axis] = sign;
                let band = 1.0 - EDGE_BAND;
                if u.abs() > band {
                    dir[a] = if u > 0.0 { 1 } else { -1 };
                }
                if v.abs() > band {
                    dir[b] = if v > 0.0 { 1 } else { -1 };
                }
                return CubeTarget::new(dir).map(|target| Hit { target, axis, sign });
            }
        }
    }
    None
}

/// The four view-plane corners of the region `hit` lies in, on its face.
fn region(frame: &ViewFrame, hit: Hit) -> [[f64; 2]; 4] {
    let (a, b) = tangents(hit.axis);
    let band = 1.0 - EDGE_BAND;
    let range = |component: i8| match component {
        0 => (-band, band),
        1 => (band, 1.0),
        _ => (-1.0, -band),
    };
    let (u0, u1) = range(hit.target.0[a]);
    let (v0, v1) = range(hit.target.0[b]);
    [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
        .map(|(u, v)| project(frame, face_point(hit.axis, hit.sign, u, v)))
}

/// What one frame of the view cube reads.
pub struct ViewCubeInput<'a> {
    /// The current camera.
    pub frame: &'a ViewFrame,
    /// The message source.
    pub localize: &'a dyn Localize,
    /// The theme colors.
    pub colors: &'a Colors,
    /// The edge length of the square the cube occupies, in points; scale it with the UI scale
    /// (about four times the row height is a good size).
    pub size: f32,
}

/// What one frame of the view cube produced.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CubeOutput {
    /// The request of a click or keyboard activation.
    pub intent: Option<ViewIntent>,
    /// The movement of the pointer in points, while the cube is dragged: x right, y down.
    pub drag: Option<[f32; 2]>,
    /// The pointer is over the cube.
    pub over: bool,
}

/// The view cube widget.
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewCube;

impl ViewCube {
    /// The square the cube occupies in `viewport` when its edge is `size` points: in the top
    /// right corner, shrunk to fit a small viewport.
    #[must_use]
    pub fn rect(viewport: Rect, size: f32) -> Rect {
        let size = size
            .min(viewport.width() - MARGIN)
            .min(viewport.height() - MARGIN)
            .max(0.0);
        Rect::from_min_size(
            Pos2::new(viewport.right() - MARGIN - size, viewport.top() + MARGIN),
            egui::vec2(size, size),
        )
    }

    /// Draws the cube in the top right corner of `viewport` and returns the intent of a click or
    /// keyboard activation. The cube shrinks to fit a small viewport. Call it after the viewport
    /// widget so it is drawn over it.
    pub fn show_over(
        &mut self,
        ui: &mut Ui,
        viewport: Rect,
        input: &ViewCubeInput<'_>,
    ) -> Option<ViewIntent> {
        self.show_over_dragged(ui, viewport, input).intent
    }

    /// Like [`ViewCube::show_over`], and also reports a drag of the cube as the pointer's
    /// movement in points, which the caller turns into an orbit, and whether the pointer is over
    /// the cube.
    pub fn show_over_dragged(
        &mut self,
        ui: &mut Ui,
        viewport: Rect,
        input: &ViewCubeInput<'_>,
    ) -> CubeOutput {
        let rect = Self::rect(viewport, input.size);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        self.show(&mut child, rect, input)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "drawing and interaction of one small widget"
    )]
    fn show(&mut self, ui: &mut Ui, rect: Rect, input: &ViewCubeInput<'_>) -> CubeOutput {
        let c = input.colors;
        // The cube's silhouette is at most sqrt(3) half-widths from its center.
        let scale = f64::from(rect.width().min(rect.height())) / 2.0 / 3.0_f64.sqrt();
        let center = rect.center();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "screen coordinates are f32"
        )]
        let to_screen = |q: [f64; 2]| {
            Pos2::new(
                center.x + (q[0] * scale) as f32,
                center.y - (q[1] * scale) as f32,
            )
        };
        let to_plane = |p: Pos2| {
            [
                f64::from(p.x - center.x) / scale.max(f64::EPSILON),
                f64::from(center.y - p.y) / scale.max(f64::EPSILON),
            ]
        };

        let group = ui.interact(rect, ui.make_persistent_id("arc-view-cube"), Sense::hover());
        let name = resolve(input.localize, messages::VIEW_CUBE.message_id, &[]);
        group.widget_info(|| WidgetInfo::labeled(egui::accesskit::Role::Group, true, &name));

        let hover = ui
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p))
            .and_then(|p| hit(input.frame, to_plane(p)));

        let painter = ui.painter_at(rect.expand(1.0));
        let (_, _, eye) = basis(input.frame);
        let font = FontId::new(
            ui.style()
                .text_styles
                .get(&TextStyle::Small)
                .map_or(11.0, |f| f.size),
            egui::FontFamily::Proportional,
        );
        for axis in 0..3 {
            for sign in [-1_i8, 1] {
                let mut normal = [0.0; 3];
                normal[axis] = f64::from(sign);
                let facing = dot(eye, normal);
                if facing <= 1e-9 {
                    continue;
                }
                let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                    .map(|(u, v)| to_screen(project(input.frame, face_point(axis, sign, u, v))));
                let hovered = hover.is_some_and(|h| h.axis == axis && h.sign == sign);
                let fill = if hovered { c.control_hover } else { c.control };
                painter.add(Shape::convex_polygon(
                    corners.to_vec(),
                    fill,
                    Stroke::new(1.0, c.border),
                ));
                if facing >= LABEL_FACING {
                    let mut dir = [0_i8; 3];
                    dir[axis] = sign;
                    if let Some(target) = CubeTarget::new(dir) {
                        painter.text(
                            to_screen(project(input.frame, face_point(axis, sign, 0.0, 0.0))),
                            Align2::CENTER_CENTER,
                            target.name(input.localize),
                            font.clone(),
                            c.text,
                        );
                    }
                }
            }
        }
        if let Some(h) = hover {
            let points = region(input.frame, h).map(to_screen).to_vec();
            painter.add(Shape::convex_polygon(
                points,
                Color32::TRANSPARENT,
                Stroke::new(2.0, c.accent),
            ));
        }

        let mut intent = None;
        let mut drag = None;
        for target in CubeTarget::all() {
            let response = ui.interact(
                rect,
                ui.make_persistent_id(("arc-view-cube", target.0)),
                Sense::click_and_drag(),
            );
            if response.dragged() {
                let d = response.drag_delta();
                drag = Some([d.x, d.y]);
            }
            let label = target.name(input.localize);
            response
                .widget_info(|| WidgetInfo::labeled(egui::accesskit::Role::Button, true, &label));
            if response.has_focus() {
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(2.0, c.focus),
                    egui::StrokeKind::Inside,
                );
            }
            if response.clicked() && intent.is_none() {
                // A pointer click names the part under the pointer; a key press the focused one.
                let chosen = response
                    .interact_pointer_pos()
                    .and_then(|p| hit(input.frame, to_plane(p)))
                    .map(|h| h.target)
                    .or_else(|| response.interact_pointer_pos().is_none().then_some(target));
                intent = chosen.map(ViewIntent::new);
            }
        }
        let over = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| rect.contains(p));
        if drag.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if over {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        CubeOutput { intent, drag, over }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_geom::view::Projection;
    use proptest::prelude::*;

    fn looking_from(eye: [f64; 3], up: Vector3) -> ViewFrame {
        ViewFrame::new(
            Point3::new(eye[0], eye[1], eye[2]),
            Point3::origin(),
            up,
            Projection::Orthographic { height: 10.0 },
            0.1,
            1000.0,
        )
        .unwrap()
    }

    fn top() -> ViewFrame {
        looking_from([0.0, 0.0, 5.0], Vector3::y())
    }

    #[test]
    fn dragging_the_cube_tilts_a_plan_view_and_then_turns_it_about_the_vertical() {
        let tolerance = arc_base::Tolerance::DEFAULT;
        let plan = top();
        // Dragging up (negative y) tilts the plan view; the camera leaves the plan.
        let tilted = orbit_by_drag(&plan, [0.0, -40.0], &tolerance).unwrap();
        // Dragging down from a plan view changes nothing.
        let down = orbit_by_drag(&plan, [0.0, 40.0], &tolerance).unwrap();
        assert_eq!(down.eye(), plan.eye());
        // Dragging down again after a tilt brings the eye back toward the top.
        let back = orbit_by_drag(&tilted, [0.0, 40.0], &tolerance).unwrap();
        assert!(back.eye().z > tilted.eye().z);
        assert!(tilted.forward().z.abs() < 0.999, "{:?}", tilted.forward());
        assert_eq!(tilted.target(), plan.target());
        let distance = |f: &ViewFrame| (f.eye() - f.target()).norm();
        assert!((distance(&tilted) - distance(&plan)).abs() < 1e-9);
        // A horizontal drag now turns about the vertical axis: the eye keeps its height.
        let turned = orbit_by_drag(&tilted, [30.0, 0.0], &tolerance).unwrap();
        assert!((turned.eye().z - tilted.eye().z).abs() < 1e-9);
        assert!(
            (turned.eye().x - tilted.eye().x).abs() > 1e-6
                || (turned.eye().y - tilted.eye().y).abs() > 1e-6
        );
        // A plan view has no horizontal turn.
        let still = orbit_by_drag(&plan, [30.0, 0.0], &tolerance).unwrap();
        assert_eq!(still.eye(), plan.eye());
    }

    #[test]
    fn there_are_26_distinct_targets_and_invalid_ones_are_refused() {
        let all = CubeTarget::all();
        assert_eq!(all.len(), 26);
        let kinds = |k| all.iter().filter(|t| t.kind() == k).count();
        assert_eq!(
            (
                kinds(TargetKind::Face),
                kinds(TargetKind::Edge),
                kinds(TargetKind::Corner)
            ),
            (6, 12, 8)
        );
        assert!(CubeTarget::new([0, 0, 0]).is_none());
        assert!(CubeTarget::new([2, 0, 0]).is_none());
    }

    #[test]
    fn top_view_hits_face_edge_corner_and_nothing() {
        let frame = top();
        let target = |q| hit(&frame, q).map(|h| h.target.0);
        assert_eq!(target([0.0, 0.0]), Some([0, 0, 1]));
        assert_eq!(target([0.5, -0.5]), Some([0, 0, 1]));
        assert_eq!(target([0.95, 0.0]), Some([1, 0, 1]));
        assert_eq!(target([0.0, -0.95]), Some([0, -1, 1]));
        assert_eq!(target([-0.95, 0.95]), Some([-1, 1, 1]));
        assert_eq!(target([1.2, 0.0]), None);
        assert_eq!(target([f64::NAN, 0.0]), None);
    }

    #[test]
    fn isometric_view_hits_the_three_visible_faces_at_their_centers() {
        let frame = looking_from([-1.0, -1.0, 1.0], Vector3::z());
        for dir in [[0, 0, 1], [-1, 0, 0], [0, -1, 0]] {
            let (axis, sign) = (0..3)
                .find_map(|a| (dir[a] != 0).then_some((a, dir[a])))
                .unwrap();
            let q = project(&frame, face_point(axis, sign, 0.0, 0.0));
            assert_eq!(hit(&frame, q).map(|h| h.target.0), Some(dir));
        }
        // The point of the silhouette on the hidden side is not part of any visible face.
        assert_eq!(hit(&frame, [3.0, 3.0]), None);
    }

    #[test]
    fn intent_looks_from_the_target_at_the_same_distance() {
        let current = looking_from([0.0, -10.0, 0.0], Vector3::z());
        let intent = ViewIntent::new(CubeTarget::new([1, 1, 1]).unwrap());
        let frame = intent.frame_from(&current).unwrap();
        assert!(((frame.eye() - frame.target()).norm() - 10.0).abs() < 1e-9);
        assert!((-frame.forward() - intent.direction).norm() < 1e-9);
        let top = ViewIntent::new(CubeTarget::new([0, 0, -1]).unwrap());
        assert!(top.frame_from(&current).is_ok());
    }

    #[test]
    fn names_compose_from_the_face_names() {
        struct Names;
        impl Localize for Names {
            fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
                let text = |k: &str| match args.iter().find(|(n, _)| *n == k) {
                    Some((_, Value::Text(t))) => t.clone(),
                    _ => String::new(),
                };
                Some(match id {
                    "ui-view-cube-top" => "Top".into(),
                    "ui-view-cube-front" => "Front".into(),
                    "ui-view-cube-right" => "Right".into(),
                    "ui-view-cube-edge" => format!("{}-{} edge", text("first"), text("second")),
                    "ui-view-cube-corner" => format!(
                        "{}, {}, {} corner",
                        text("first"),
                        text("second"),
                        text("third")
                    ),
                    _ => return None,
                })
            }
        }
        let name = |d| CubeTarget::new(d).unwrap().name(&Names);
        assert_eq!(name([0, 0, 1]), "Top");
        assert_eq!(name([0, -1, 1]), "Top-Front edge");
        assert_eq!(name([1, -1, 1]), "Top, Front, Right corner");
    }

    proptest! {
        #[test]
        fn the_center_of_every_visible_face_hits_that_face(
            x in -1.0_f64..1.0, y in -1.0_f64..1.0, z in -1.0_f64..1.0,
        ) {
            let eye = Vector3::new(x, y, z);
            prop_assume!(eye.norm() > 0.1);
            // An up hint that is never parallel to the direction.
            let up = if eye.normalize().z.abs() > 0.9 { Vector3::y() } else { Vector3::z() };
            let frame = looking_from([x * 10.0, y * 10.0, z * 10.0], up);
            for axis in 0..3 {
                for sign in [-1_i8, 1] {
                    let facing = dot(basis(&frame).2, face_point(axis, sign, 0.0, 0.0));
                    if facing > 0.05 {
                        let q = project(&frame, face_point(axis, sign, 0.0, 0.0));
                        let mut dir = [0_i8; 3];
                        dir[axis] = sign;
                        prop_assert_eq!(hit(&frame, q).map(|h| h.target.0), Some(dir));
                    }
                }
            }
        }

        #[test]
        fn hit_never_panics(qx in proptest::num::f64::ANY, qy in proptest::num::f64::ANY) {
            let _ = hit(&top(), [qx, qy]);
        }
    }
}
