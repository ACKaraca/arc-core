//! Snapping feedback (CORE-UI-030).
//!
//! [`paint`] draws the feedback of a snap the caller computed: a marker at the snap position
//! whose shape depends on the [`SnapKind`], a label with the kind's localized name beside it, and
//! alignment tracking lines from the points the user acquired. The module computes no snap; it
//! paints the data it is given. With no snap, nothing is drawn, tracking lines included.

use egui::{FontId, Pos2, Shape, Stroke, Ui, Vec2, pos2, vec2};

use crate::text::{Localize, Message, messages, resolve};

/// The most tracking segments drawn in one frame.
pub const MAX_TRACKING: usize = 16;

/// Half the size of a marker, in logical points.
pub const MARKER_RADIUS: f32 = 6.0;

/// The kind of a snap: what the snapped point is on the geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapKind {
    /// An end of a curve or a line.
    Endpoint,
    /// The middle of a curve or a line.
    Midpoint,
    /// The center of a circle, an arc or an ellipse.
    Center,
    /// Where two curves cross.
    Intersection,
    /// The foot of a perpendicular from the last point.
    Perpendicular,
    /// A point where a curve touches the line from the last point.
    Tangent,
    /// The nearest point of a curve.
    Nearest,
    /// A point object such as a node.
    Node,
    /// A quarter point of a circle or an arc.
    Quadrant,
}

/// The shape of a snap marker. Each kind has its own shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerShape {
    /// A square outline.
    Square,
    /// A triangle outline.
    Triangle,
    /// A circle outline.
    Circle,
    /// A filled circle.
    Dot,
    /// A cross of two diagonals.
    Cross,
    /// A right-angle corner.
    Corner,
    /// A half circle.
    Arc,
    /// Two triangles meeting at a point.
    Hourglass,
    /// A diamond outline.
    Diamond,
}

impl SnapKind {
    /// Every kind, in the order of the specification's list.
    pub const ALL: [Self; 9] = [
        Self::Endpoint,
        Self::Midpoint,
        Self::Center,
        Self::Intersection,
        Self::Perpendicular,
        Self::Tangent,
        Self::Nearest,
        Self::Node,
        Self::Quadrant,
    ];

    /// The marker shape of the kind.
    #[must_use]
    pub fn marker(self) -> MarkerShape {
        match self {
            Self::Endpoint => MarkerShape::Square,
            Self::Midpoint => MarkerShape::Triangle,
            Self::Center => MarkerShape::Circle,
            Self::Node => MarkerShape::Dot,
            Self::Intersection => MarkerShape::Cross,
            Self::Perpendicular => MarkerShape::Corner,
            Self::Tangent => MarkerShape::Arc,
            Self::Nearest => MarkerShape::Hourglass,
            Self::Quadrant => MarkerShape::Diamond,
        }
    }

    /// The message that names the kind.
    #[must_use]
    pub fn message(self) -> Message {
        match self {
            Self::Endpoint => messages::SNAP_ENDPOINT,
            Self::Midpoint => messages::SNAP_MIDPOINT,
            Self::Center => messages::SNAP_CENTER,
            Self::Intersection => messages::SNAP_INTERSECTION,
            Self::Perpendicular => messages::SNAP_PERPENDICULAR,
            Self::Tangent => messages::SNAP_TANGENT,
            Self::Nearest => messages::SNAP_NEAREST,
            Self::Node => messages::SNAP_NODE,
            Self::Quadrant => messages::SNAP_QUADRANT,
        }
    }
}

/// A snap the caller found: its kind and the screen position of the snapped point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snap {
    /// The kind.
    pub kind: SnapKind,
    /// The position in screen coordinates.
    pub position: Pos2,
}

/// Paints the feedback of `snap` with its label and `tracking` segments in screen coordinates.
/// Draws nothing when `snap` is `None`. At most [`MAX_TRACKING`] segments are drawn.
pub fn paint(ui: &Ui, localize: &dyn Localize, snap: Option<&Snap>, tracking: &[[Pos2; 2]]) {
    let Some(snap) = snap else {
        return;
    };
    let painter = ui.painter();
    let stroke = ui.visuals().selection.stroke;
    let line = Stroke::new(1.0, stroke.color);
    for segment in tracking.iter().take(MAX_TRACKING) {
        painter.line_segment(*segment, line);
    }
    draw_marker(painter, snap.kind.marker(), snap.position, stroke);

    let name = resolve(localize, snap.kind.message().message_id, &[]);
    let font: FontId = egui::TextStyle::Small.resolve(ui.style());
    let galley = painter.layout_no_wrap(name, font, stroke.color);
    let at = snap.position + vec2(MARKER_RADIUS + 4.0, -MARKER_RADIUS - 4.0);
    let size: Vec2 = galley.size() + vec2(8.0, 4.0);
    let rect = egui::Rect::from_min_size(pos2(at.x, at.y - size.y), size);
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    painter.galley(rect.min + vec2(4.0, 2.0), galley, stroke.color);
}

/// Draws one marker centered at `center`.
fn draw_marker(painter: &egui::Painter, shape: MarkerShape, center: Pos2, stroke: Stroke) {
    let r = MARKER_RADIUS;
    let c = center;
    match shape {
        MarkerShape::Square => {
            let rect = egui::Rect::from_center_size(c, vec2(2.0 * r, 2.0 * r));
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Middle);
        }
        MarkerShape::Triangle => {
            let points = vec![
                pos2(c.x, c.y - r),
                pos2(c.x + r, c.y + r),
                pos2(c.x - r, c.y + r),
            ];
            painter.add(Shape::closed_line(points, stroke));
        }
        MarkerShape::Circle => {
            painter.circle_stroke(c, r, stroke);
        }
        MarkerShape::Dot => {
            painter.circle_filled(c, r / 2.0, stroke.color);
        }
        MarkerShape::Cross => {
            painter.line_segment([pos2(c.x - r, c.y - r), pos2(c.x + r, c.y + r)], stroke);
            painter.line_segment([pos2(c.x + r, c.y - r), pos2(c.x - r, c.y + r)], stroke);
        }
        MarkerShape::Corner => {
            let points = vec![
                pos2(c.x - r, c.y - r),
                pos2(c.x - r, c.y + r),
                pos2(c.x + r, c.y + r),
            ];
            painter.add(Shape::line(points, stroke));
        }
        MarkerShape::Arc => {
            let steps: u8 = 8;
            let points: Vec<Pos2> = (0..=steps)
                .map(|i| {
                    let t = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::PI * (f32::from(i) / f32::from(steps));
                    pos2(c.x + r * t.cos(), c.y + r * t.sin())
                })
                .collect();
            painter.add(Shape::line(points, stroke));
        }
        MarkerShape::Hourglass => {
            let points = vec![
                pos2(c.x - r, c.y - r),
                pos2(c.x + r, c.y - r),
                pos2(c.x - r, c.y + r),
                pos2(c.x + r, c.y + r),
            ];
            painter.add(Shape::closed_line(points, stroke));
        }
        MarkerShape::Diamond => {
            let points = vec![
                pos2(c.x, c.y - r),
                pos2(c.x + r, c.y),
                pos2(c.x, c.y + r),
                pos2(c.x - r, c.y),
            ];
            painter.add(Shape::closed_line(points, stroke));
        }
    }
}
