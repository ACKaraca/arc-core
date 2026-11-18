//! The neutral path both writers draw: lines and exact elliptical arcs in page millimeters.

use std::f64::consts::PI;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result};
use arc_draft::clip::Piece;
use arc_geom::flatten::CurveFlatten;
use arc_geom::{Curve2, Polyline2, PolylineSegment, Region};

use crate::error::limit;

/// The largest magnitude of a written coordinate, in millimeters (1000 kilometers).
pub(crate) const MAX_COORD: f64 = 1e9;

/// An exact elliptical arc: the point at angle `t` is `c + a·cos t + b·sin t`; the arc runs from
/// `t0` through the signed sweep `dt`. `from` and `to` are the exact end points (they differ from
/// the formula by rounding only).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArcSeg {
    pub from: Point2,
    pub to: Point2,
    pub c: Point2,
    pub a: Vector2,
    pub b: Vector2,
    pub t0: f64,
    pub dt: f64,
}

impl ArcSeg {
    pub(crate) fn at(&self, t: f64) -> Point2 {
        let (s, c) = t.sin_cos();
        self.c + self.a * c + self.b * s
    }

    pub(crate) fn tangent(&self, t: f64) -> Vector2 {
        let (s, c) = t.sin_cos();
        self.b * c - self.a * s
    }

    /// The `i`-th of `n` equal pieces: start angle, end angle and exact end point.
    pub(crate) fn piece(&self, i: u32, n: u32) -> (f64, f64, Point2) {
        let f = |k: u32| self.t0 + self.dt * f64::from(k) / f64::from(n);
        let end = if i + 1 == n {
            self.to
        } else {
            self.at(f(i + 1))
        };
        (f(i), f(i + 1), end)
    }

    /// The larger semi-axis length.
    pub(crate) fn radius(&self) -> f64 {
        self.a.norm().max(self.b.norm())
    }
}

/// A segment after the current point.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Seg {
    Line(Point2),
    Arc(ArcSeg),
}

/// One connected run of segments.
#[derive(Debug, Clone)]
pub(crate) struct SubPath {
    pub start: Point2,
    pub segs: Vec<Seg>,
    pub closed: bool,
}

/// How overlapping loops of one filled path combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FillRule {
    /// A point is inside when the loops wind around it a nonzero number of times.
    NonZero,
    /// A point is inside when a ray from it crosses an odd number of loops (holes).
    EvenOdd,
}

/// What curve conversion counted, for the loss report.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Counts {
    pub nurbs: u64,
    pub widths: u64,
}

/// Curve conversion settings.
pub(crate) struct Cx {
    pub chord: f64,
    pub max_vertices: usize,
}

fn circular(arc: &arc_geom::Arc2) -> ArcSeg {
    let r = arc.radius();
    ArcSeg {
        from: arc.point_at(0.0),
        to: arc.point_at(arc.sweep().abs()),
        c: arc.center(),
        a: Vector2::new(r, 0.0),
        b: Vector2::new(0.0, r),
        t0: arc.start_angle(),
        dt: arc.sweep(),
    }
}

fn elliptical(arc: &arc_geom::EllipticalArc2) -> ArcSeg {
    let e = arc.ellipse();
    ArcSeg {
        from: arc.point_at(0.0),
        to: arc.point_at(arc.sweep().abs()),
        c: e.center(),
        a: e.major_axis(),
        b: e.minor_axis(),
        t0: arc.start_parameter(),
        dt: arc.sweep(),
    }
}

pub(crate) fn polyline_path(p: &Polyline2, cx: &Cx) -> Result<SubPath> {
    if u64::try_from(p.vertices().len()).map_or(true, |n| n > cx.max_vertices as u64) {
        return Err(limit("array_length"));
    }
    let mut segs = Vec::with_capacity(p.segment_count());
    let mut start = None;
    for segment in p.segments() {
        match segment {
            PolylineSegment::Line(s) => {
                start.get_or_insert(s.start());
                segs.push(Seg::Line(s.end()));
            }
            PolylineSegment::Arc(a) => {
                let arc = circular(&a);
                start.get_or_insert(arc.from);
                segs.push(Seg::Arc(arc));
            }
        }
    }
    Ok(SubPath {
        start: start.unwrap_or_default(),
        segs,
        closed: p.is_closed(),
    })
}

/// The path of a bounded curve; `None` when the curve cannot be drawn (unbounded, or the
/// flattening fails for a reason other than the vertex limit).
pub(crate) fn curve_path(curve: &Curve2, cx: &Cx, counts: &mut Counts) -> Result<Option<SubPath>> {
    let arc = |a: ArcSeg, closed| SubPath {
        start: a.from,
        segs: vec![Seg::Arc(a)],
        closed,
    };
    Ok(Some(match curve {
        Curve2::Ray(_) | Curve2::Line(_) => return Ok(None),
        Curve2::Segment(s) => SubPath {
            start: s.start(),
            segs: vec![Seg::Line(s.end())],
            closed: false,
        },
        Curve2::Circle(c) => arc(circular(&c.to_arc()), true),
        Curve2::Arc(a) => arc(circular(a), false),
        Curve2::Ellipse(e) => arc(elliptical(&e.to_arc()), true),
        Curve2::EllipticalArc(a) => arc(elliptical(a), false),
        Curve2::Polyline(p) => polyline_path(p, cx)?,
        Curve2::Nurbs(n) => {
            let points = match n.flatten(cx.chord, PI / 12.0, cx.max_vertices) {
                Ok(points) => points,
                Err(e) if e.code() == "geom.flatten.too_many_vertices" => {
                    return Err(limit("array_length"));
                }
                Err(_) => return Ok(None),
            };
            let mut iter = points.into_iter();
            let Some(start) = iter.next() else {
                return Ok(None);
            };
            counts.nurbs += 1;
            SubPath {
                start,
                segs: iter.map(Seg::Line).collect(),
                closed: false,
            }
        }
    }))
}

/// Every loop of a region (outer loops and holes) as one closed sub-path each; `None` when a
/// curve cannot be drawn.
pub(crate) fn region_loops(
    region: &Region,
    cx: &Cx,
    counts: &mut Counts,
) -> Result<Option<Vec<SubPath>>> {
    let mut out = Vec::new();
    for part in region.parts() {
        for lp in std::iter::once(part.outer()).chain(part.holes()) {
            let mut joined: Option<SubPath> = None;
            for curve in lp.curves() {
                let Some(sub) = curve_path(curve, cx, counts)? else {
                    return Ok(None);
                };
                match &mut joined {
                    None => joined = Some(sub),
                    Some(path) => {
                        // Gaps the loop closed within its tolerance become a straight segment.
                        let end = match path.segs.last() {
                            Some(Seg::Line(q)) => *q,
                            Some(Seg::Arc(a)) => a.to,
                            None => path.start,
                        };
                        if end != sub.start {
                            path.segs.push(Seg::Line(sub.start));
                        }
                        path.segs.extend(sub.segs);
                    }
                }
            }
            let Some(mut path) = joined else {
                return Ok(None);
            };
            path.closed = true;
            out.push(path);
        }
    }
    Ok(Some(out))
}

/// The drawn paths and dots of a stroke; `None` when a part could not be drawn.
pub(crate) fn stroke_paths(
    curve: &Curve2,
    pieces: Option<&[Piece]>,
    cx: &Cx,
    counts: &mut Counts,
) -> Result<Option<(Vec<SubPath>, Vec<Point2>)>> {
    let (mut paths, mut dots) = (Vec::new(), Vec::new());
    match pieces {
        None => match curve_path(curve, cx, counts)? {
            Some(p) => paths.push(p),
            None => return Ok(None),
        },
        Some(list) => {
            if list.len() as u64 > cx.max_vertices as u64 {
                return Err(limit("array_length"));
            }
            for piece in list {
                match piece {
                    Piece::Point(p) => dots.push(*p),
                    Piece::Curve(c) => match curve_path(c, cx, counts)? {
                        Some(p) => paths.push(p),
                        None => return Ok(None),
                    },
                }
            }
        }
    }
    Ok(Some((paths, dots)))
}

/// The width of a stroke: the pen's, or the mean width of a polyline with widths when larger.
/// The second value says whether a width was approximated.
pub(crate) fn stroke_width(curve: &Curve2, pen_width: f64) -> (f64, bool) {
    // shortcut: a polyline with widths is stroked with its mean vertex width, as the raster
    // output does, instead of tapering per segment; draw it as a filled outline to taper.
    if let Curve2::Polyline(p) = curve {
        let n = p.vertices().len().max(1) as f64;
        let mean = p
            .vertices()
            .iter()
            .map(|v| 0.5 * (v.start_width + v.end_width))
            .sum::<f64>()
            / n;
        if mean > 0.0 {
            return (pen_width.max(mean), true);
        }
    }
    (pen_width, false)
}

/// An error for a value the writers cannot represent.
pub(crate) fn range() -> Error {
    crate::error::stream("coordinate")
}
