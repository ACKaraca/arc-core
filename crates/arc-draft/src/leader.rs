//! Leaders and multi-leaders (CORE-DRAFT-013).
//!
//! A leader is a line through its points with an arrowhead at the first point and a horizontal
//! landing after the last. A multi-leader has several leader lines that end at the start of one
//! landing, and content at the landing's end: text, a block reference or a tolerance frame. The
//! geometry functions are pure; the rules are stated in the crate specification, section
//! "Dimensions, leaders and annotation scaling as implemented".

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result};
use arc_doc::{Bound, FieldDecl, Finding, OnDelete, Strength, TypeValidator, Value, ValueType};
use arc_geom::{Polyline2, PolylineVertex};

use crate::annotation::{self, Arrow, Drawn, Part, arrowhead, unit};
use crate::error::codes;
use crate::style::{LEADER_STYLE, style_reference};
use crate::types::{
    BLOCK, TEXT_BYTES, common_findings, finding, flag, real, reference, text, variants, with_kind,
};

/// A leader entity.
pub const LEADER: &str = "core.draft.leader";
/// A multi-leader entity.
pub const MULTILEADER: &str = "core.draft.multileader";

/// The most points of a leader line, and leader lines of a multi-leader.
pub const MAX_POINTS: u64 = 4096;
/// The most rows of a tolerance frame, and cells of a row.
pub const MAX_CELLS: u64 = 64;

/// The arrow names in code order.
pub const ARROW_NAMES: [&str; 6] = ["closed_filled", "closed", "open", "dot", "tick", "none"];

/// Field indices of a leader.
pub mod line {
    use crate::types::entity::KIND as K;
    /// `points`: the arrow tip first.
    pub const POINTS: usize = K;
    /// `spline`: stored; the leader is drawn through its points as a polyline.
    pub const SPLINE: usize = K + 1;
    /// `arrow`: absent takes the style's.
    pub const ARROW: usize = K + 2;
    /// `style`: the leader style.
    pub const STYLE: usize = K + 3;
    /// `annotative`.
    pub const ANNOTATIVE: usize = K + 4;
    /// `scales`.
    pub const SCALES: usize = K + 5;
}

/// Field indices of a multi-leader.
pub mod multi {
    use crate::types::entity::KIND as K;
    /// `leaders`: lists of points, each starting at its arrow tip.
    pub const LEADERS: usize = K;
    /// `position`: the end of the landing, where the content attaches.
    pub const POSITION: usize = K + 1;
    /// `direction`: `right` (content to the right of the landing) or `left`.
    pub const DIRECTION: usize = K + 2;
    /// `content`: `text`, `block`, `tolerance` or `none`.
    pub const CONTENT: usize = K + 3;
    /// `text`: the text content, one paragraph per line.
    pub const TEXT: usize = K + 4;
    /// `block`: the block of block content.
    pub const BLOCK: usize = K + 5;
    /// `block_scale`: the scale of block content.
    pub const BLOCK_SCALE: usize = K + 6;
    /// `tolerance`: the rows of cell texts of a tolerance frame.
    pub const TOLERANCE: usize = K + 7;
    /// `attachment`: which part of the content lies on the landing.
    pub const ATTACHMENT: usize = K + 8;
    /// `style`: the leader style.
    pub const STYLE: usize = K + 9;
    /// `annotative`.
    pub const ANNOTATIVE: usize = K + 10;
    /// `scales`.
    pub const SCALES: usize = K + 11;
}

/// The vertical alignment of multi-leader content to its landing, with its enum code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Attachment {
    /// The top of the content lies on the landing.
    Top = 0,
    /// The middle of the content lies on the landing.
    #[default]
    Middle = 1,
    /// The bottom of the content lies on the landing.
    Bottom = 2,
}

impl Attachment {
    /// The attachment of an enum code; unknown codes read as [`Attachment::Middle`].
    #[must_use]
    pub fn of(code: u32) -> Self {
        match code {
            0 => Self::Top,
            2 => Self::Bottom,
            _ => Self::Middle,
        }
    }
}

fn points_list() -> ValueType {
    ValueType::List(Box::new(ValueType::Point2))
}

fn leader_fields() -> Vec<FieldDecl> {
    let mut fields = vec![
        FieldDecl::new("points", points_list()).with_bound(Bound::Length { max: MAX_POINTS }),
        flag("spline", false),
        FieldDecl::new("arrow", variants(&ARROW_NAMES)).optional(),
        FieldDecl::new("style", style_reference(LEADER_STYLE, true)).optional(),
    ];
    fields.extend(annotation::fields());
    with_kind(fields)
}

fn multileader_fields() -> Vec<FieldDecl> {
    let strings = ValueType::List(Box::new(ValueType::String));
    let mut fields = vec![
        FieldDecl::new("leaders", ValueType::List(Box::new(points_list())))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length { max: MAX_POINTS }),
        FieldDecl::new("position", ValueType::Point2),
        FieldDecl::new("direction", variants(&["right", "left"])).with_default(Value::Enum(0)),
        FieldDecl::new("content", variants(&["text", "block", "tolerance", "none"]))
            .with_default(Value::Enum(0)),
        text("text", TEXT_BYTES),
        FieldDecl::new(
            "block",
            reference(BLOCK, Strength::Associative, OnDelete::Block, true),
        )
        .optional(),
        real("block_scale", 1.0, f64::MIN_POSITIVE, f64::MAX),
        FieldDecl::new("tolerance", ValueType::List(Box::new(strings)))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length { max: MAX_CELLS }),
        FieldDecl::new("attachment", variants(&["top", "middle", "bottom"]))
            .with_default(Value::Enum(1)),
        FieldDecl::new("style", style_reference(LEADER_STYLE, true)).optional(),
    ];
    fields.extend(annotation::fields());
    with_kind(fields)
}

fn count(values: &[Option<Value>], index: usize) -> Option<Vec<usize>> {
    match values.get(index) {
        Some(Some(Value::List(items))) => Some(
            items
                .iter()
                .map(|i| match i {
                    Value::List(inner) => inner.len(),
                    _ => 0,
                })
                .collect(),
        ),
        _ => None,
    }
}

fn leader_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = common_findings(values);
    let points = match values.get(line::POINTS) {
        Some(Some(Value::List(items))) => items.len(),
        _ => 0,
    };
    if points < 2 {
        findings.push(finding(Error::new(codes::LEADER_POINTS_INVALID)));
    }
    findings.extend(annotation::scale_findings(values, line::SCALES));
    findings
}

fn multileader_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = common_findings(values);
    if count(values, multi::LEADERS).is_some_and(|c| c.contains(&0)) {
        findings.push(finding(Error::new(codes::LEADER_POINTS_INVALID)));
    }
    if count(values, multi::TOLERANCE).is_some_and(|c| {
        c.iter()
            .any(|n| u64::try_from(*n).unwrap_or(u64::MAX) > MAX_CELLS)
    }) {
        findings.push(finding(Error::new(codes::LEADER_POINTS_INVALID)));
    }
    if let Some(Some(Value::String(s))) = values.get(multi::TEXT)
        && s.chars()
            .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        findings.push(finding(Error::new(codes::TEXT_CONTENT_INVALID)));
    }
    findings.extend(annotation::scale_findings(values, multi::SCALES));
    findings
}

/// The leader and multi-leader declarations.
pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![
        (LEADER, leader_fields(), vec![leader_findings]),
        (
            MULTILEADER,
            multileader_fields(),
            vec![multileader_findings],
        ),
    ]
}

/// The arrowhead at the first of `points` pointing from the next distinct point to it.
fn tip(d: &mut Drawn, points: &[Point2], arrow: Arrow, size: f64) -> Result<()> {
    let Some(first) = points.first() else {
        return Ok(());
    };
    if let Some(e) = points.iter().find_map(|p| unit(*first - *p)) {
        arrowhead(d, arrow, *first, e, size)?;
    }
    Ok(())
}

/// The drawn geometry of a leader: the line through `points` (the tip first), the arrowhead at
/// the tip, and a landing of `landing` length from the last point, horizontal, continuing the
/// last segment's horizontal direction (right when it is vertical).
///
/// # Errors
///
/// `draft.leader.points_invalid` for fewer than two distinct points; the `arc-geom` errors of
/// shapes that cannot be represented.
pub fn leader_geometry(
    points: &[Point2],
    arrow: Arrow,
    arrow_size: f64,
    landing: f64,
) -> Result<Drawn> {
    let mut d = Drawn::default();
    d.polyline(Part::Line, points)?;
    if d.strokes.is_empty() {
        return Err(Error::new(codes::LEADER_POINTS_INVALID));
    }
    tip(&mut d, points, arrow, arrow_size)?;
    if let [.., prev, last] = points
        && landing > 0.0
    {
        let sign = if last.x < prev.x { -1.0 } else { 1.0 };
        d.line(
            Part::Landing,
            *last,
            *last + Vector2::new(sign * landing, 0.0),
        )?;
    }
    Ok(d)
}

/// The drawn leader lines and landing of a multi-leader, and the anchor of its content.
#[derive(Debug, Clone, PartialEq)]
pub struct MultiGeometry {
    /// The leader lines, arrowheads and landing.
    pub drawn: Drawn,
    /// The point the content's near side is aligned to: `gap` beyond the landing's end.
    pub anchor: Point2,
    /// 1 when content lies to the right of the anchor, −1 to the left.
    pub direction: f64,
}

/// The geometry of a multi-leader: the landing from `landing` before `position` to `position`
/// along `direction` (±1), each leader line through its points to the landing's start with an
/// arrowhead at its first point, and the content anchor `gap` beyond `position`.
///
/// # Errors
///
/// The `arc-geom` errors of shapes that cannot be represented.
pub fn multileader_geometry(
    leaders: &[Vec<Point2>],
    position: Point2,
    direction: f64,
    landing: f64,
    gap: f64,
    arrow: Arrow,
    arrow_size: f64,
) -> Result<MultiGeometry> {
    let mut d = Drawn::default();
    let along = Vector2::new(direction, 0.0);
    let start = position - along * landing.max(0.0);
    d.line(Part::Landing, start, position)?;
    for points in leaders {
        let mut line = points.clone();
        line.push(start);
        d.polyline(Part::Line, &line)?;
        tip(&mut d, &line, arrow, arrow_size)?;
    }
    Ok(MultiGeometry {
        drawn: d,
        anchor: position + along * gap,
        direction,
    })
}

/// The boxes of a tolerance frame and the centers of their texts. `widths` holds the text width
/// of every cell by row; each box is its text plus `gap` on every side, rows stack downward,
/// and the frame's near side lies on `anchor` with the row edge or middle the attachment names
/// at the anchor's height.
///
/// # Errors
///
/// The `arc-geom` errors of boxes that cannot be represented.
pub fn frame_geometry(
    widths: &[Vec<f64>],
    height: f64,
    gap: f64,
    anchor: Point2,
    direction: f64,
    attachment: Attachment,
) -> Result<(Vec<Polyline2>, Vec<Point2>)> {
    let row = height + 2.0 * gap;
    #[expect(clippy::cast_precision_loss, reason = "rows are at most 64")]
    let total = widths.len() as f64 * row;
    let top = match attachment {
        Attachment::Top => anchor.y,
        Attachment::Middle => anchor.y + total / 2.0,
        Attachment::Bottom => anchor.y + total,
    };
    let mut boxes = Vec::new();
    let mut centers = Vec::new();
    let mut y = top;
    for cells in widths {
        let full: f64 = cells.iter().map(|w| w + 2.0 * gap).sum();
        let mut x = if direction < 0.0 {
            anchor.x - full
        } else {
            anchor.x
        };
        for w in cells {
            let cw = w + 2.0 * gap;
            let corner = |px: f64, py: f64| PolylineVertex::new(Point2::new(px, py), 0.0);
            boxes.push(Polyline2::new(
                vec![
                    corner(x, y - row),
                    corner(x + cw, y - row),
                    corner(x + cw, y),
                    corner(x, y),
                ],
                true,
            )?);
            centers.push(Point2::new(x + cw / 2.0, y - row / 2.0));
            x += cw;
        }
        y -= row;
    }
    Ok((boxes, centers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_stack_rows_from_the_attachment() {
        let (boxes, centers) = frame_geometry(
            &[vec![2.0, 4.0], vec![1.0]],
            2.0,
            0.5,
            Point2::new(10.0, 0.0),
            1.0,
            Attachment::Top,
        )
        .unwrap();
        assert_eq!(boxes.len(), 3);
        assert_eq!(centers[0], Point2::new(11.5, -1.5));
        assert_eq!(centers[1], Point2::new(15.5, -1.5));
        assert_eq!(centers[2], Point2::new(11.0, -4.5));
        let (_, left) = frame_geometry(
            &[vec![2.0]],
            2.0,
            0.5,
            Point2::new(10.0, 0.0),
            -1.0,
            Attachment::Bottom,
        )
        .unwrap();
        assert_eq!(left[0], Point2::new(8.5, 1.5));
    }
}
