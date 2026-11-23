//! Models, a source and an SVG writer shared by the tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::type_complexity,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    reason = "test code"
)]
#![allow(dead_code, reason = "each test file uses a part")]

use std::fmt::Write as _;

use arc_base::id::Identifier;
use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Color, Limits, ObjectId, Result, Tolerance};
use arc_draft::display::Shape;
use arc_geom::{Curve2, PlaneFrame, Polyhedron, Region, extrude, prism};
use arc_view::{
    ClassStyles, CutFill, Geometry, LineStyle, PrimitiveKind, SourceObject, ViewDef, ViewResult,
    ViewSource, VisibilityClass, crop_polygon,
};

pub const TOL: Tolerance = Tolerance::DEFAULT;

pub fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 | n)
}

pub fn polygon(points: &[(f64, f64)]) -> Region {
    let points: Vec<Point2> = points.iter().map(|&(x, y)| Point2::new(x, y)).collect();
    crop_polygon(&points, &TOL).expect("polygon")
}

pub fn cuboid(x: (f64, f64), y: (f64, f64), z: (f64, f64)) -> Polyhedron {
    let footprint = polygon(&[(x.0, y.0), (x.1, y.0), (x.1, y.1), (x.0, y.1)]);
    prism(&footprint, z.0, z.1 - z.0, 1e-3, &TOL, &Limits::DEFAULT).expect("prism")
}

/// A solid extruded from a profile in the world xz plane, from y = 0 toward negative y.
pub fn profile_solid(points: &[(f64, f64)], depth: f64) -> Polyhedron {
    let frame = PlaneFrame::new(Point3::origin(), Vector3::x(), Vector3::z(), &TOL).expect("frame");
    extrude(
        &polygon(points),
        &frame,
        depth,
        1e-3,
        &TOL,
        &Limits::DEFAULT,
    )
    .expect("extrude")
}

pub fn styles() -> ClassStyles {
    ClassStyles::uniform(&LineStyle::solid(Color::BLACK))
}

pub fn object(geometry: Polyhedron, category: &str, join: Option<u64>) -> SourceObject {
    SourceObject::single(
        Geometry::Solid(geometry),
        category,
        styles(),
        Some(CutFill::solid(Color::new(0.5, 0.5, 0.5, 1.0))),
        join,
    )
}

/// A source over a fixed list of objects.
#[derive(Default)]
pub struct Model {
    pub objects: Vec<(ObjectId, SourceObject)>,
}

impl Model {
    pub fn add(&mut self, n: u128, object: SourceObject) -> ObjectId {
        let key = id(n);
        self.objects.push((key, object));
        key
    }
}

impl ViewSource for Model {
    fn objects(&self, _: &ViewDef) -> Result<Vec<ObjectId>> {
        Ok(self.objects.iter().map(|o| o.0).collect())
    }

    fn object(&self, id: ObjectId, _: &ViewDef) -> Result<Option<SourceObject>> {
        Ok(self.objects.iter().find(|o| o.0 == id).map(|o| o.1.clone()))
    }
}

/// The four walls of a 5 m by 4 m room, 0.2 m thick and 3 m high, joined.
pub fn room(model: &mut Model) {
    let walls = [
        ((0.0, 5.0), (0.0, 0.2)),
        ((0.0, 5.0), (3.8, 4.0)),
        ((0.0, 0.2), (0.2, 3.8)),
        ((4.8, 5.0), (0.2, 3.8)),
    ];
    for (n, (x, y)) in (1..).zip(walls) {
        model.add(n, object(cuboid(x, y, (0.0, 3.0)), "wall", Some(1)));
    }
}

/// The line segments of `class` as `((x0, y0), (x1, y1))`, endpoints sorted.
pub fn lines(view: &ViewResult, class: VisibilityClass) -> Vec<((f64, f64), (f64, f64))> {
    let mut out = Vec::new();
    for (p, l) in view.list.primitives.iter().zip(&view.links) {
        if l.class != class || l.kind != PrimitiveKind::Line {
            continue;
        }
        if let Shape::Stroke {
            curve: Curve2::Segment(s),
            ..
        } = &p.shape
        {
            let (a, b) = ((s.start().x, s.start().y), (s.end().x, s.end().y));
            out.push(if a <= b { (a, b) } else { (b, a) });
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    out
}

/// Whether `lines` holds a segment within twice the linear tolerance of `a`–`b`, either way.
pub fn has(lines: &[((f64, f64), (f64, f64))], a: (f64, f64), b: (f64, f64)) -> bool {
    let near = |p: (f64, f64), q: (f64, f64)| {
        (p.0 - q.0).abs() <= 2.0 * TOL.linear && (p.1 - q.1).abs() <= 2.0 * TOL.linear
    };
    lines
        .iter()
        .any(|&(p, q)| (near(p, a) && near(q, b)) || (near(p, b) && near(q, a)))
}

pub fn region_area(region: &Region) -> f64 {
    region
        .parts()
        .iter()
        .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
        .map(arc_geom::Loop2::signed_area)
        .sum()
}

fn color(class: VisibilityClass) -> &'static str {
    match class {
        VisibilityClass::Cut => "#000000",
        VisibilityClass::Visible => "#1f5fbf",
        VisibilityClass::Beyond => "#2a9d4b",
        VisibilityClass::Hidden => "#bbbbbb",
        VisibilityClass::Overhead => "#c0392b",
        _ => "#8e44ad",
    }
}

/// Writes the view as SVG to `$ARC_VIEW_SVG_DIR/<name>.svg` when that variable is set.
pub fn write_svg(name: &str, view: &ViewResult) {
    let Ok(dir) = std::env::var("ARC_VIEW_SVG_DIR") else {
        return;
    };
    let mut body = String::new();
    let (mut lo, mut hi) = ((f64::MAX, f64::MAX), (f64::MIN, f64::MIN));
    let mut grow = |p: Point2| {
        lo = (lo.0.min(p.x), lo.1.min(p.y));
        hi = (hi.0.max(p.x), hi.1.max(p.y));
    };
    for (p, l) in view.list.primitives.iter().zip(&view.links) {
        match &p.shape {
            Shape::Stroke {
                curve: Curve2::Segment(s),
                ..
            } => {
                grow(s.start());
                grow(s.end());
                let (dash, width) = match l.class {
                    VisibilityClass::Overhead | VisibilityClass::Hidden => {
                        (" stroke-dasharray=\"0.1 0.05\"", 0.02)
                    }
                    VisibilityClass::Cut if l.kind == PrimitiveKind::Line => ("", 0.04),
                    _ => ("", 0.015),
                };
                let _ = writeln!(
                    body,
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{width}\"{dash}/>",
                    s.start().x,
                    -s.start().y,
                    s.end().x,
                    -s.end().y,
                    color(l.class)
                );
            }
            Shape::Region(region) => {
                let mut d = String::new();
                for part in region.parts() {
                    for lp in std::iter::once(part.outer()).chain(part.holes()) {
                        for (i, c) in lp.curves().iter().enumerate() {
                            if let Curve2::Segment(s) = c {
                                grow(s.start());
                                let _ = write!(
                                    d,
                                    "{}{} {} ",
                                    if i == 0 { "M" } else { "L" },
                                    s.start().x,
                                    -s.start().y
                                );
                            }
                        }
                        d.push_str("Z ");
                    }
                }
                let _ = writeln!(
                    body,
                    "<path d=\"{d}\" fill=\"#d0d0d0\" fill-rule=\"evenodd\"/>"
                );
            }
            _ => {}
        }
    }
    let margin = 0.5;
    let (w, h) = (hi.0 - lo.0 + 2.0 * margin, hi.1 - lo.1 + 2.0 * margin);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"800\" height=\"{}\" viewBox=\"{} {} {w} {h}\">\n<rect x=\"{}\" y=\"{}\" width=\"{w}\" height=\"{h}\" fill=\"white\"/>\n{body}</svg>\n",
        (800.0 * h / w).round(),
        lo.0 - margin,
        -hi.1 - margin,
        lo.0 - margin,
        -hi.1 - margin,
    );
    std::fs::write(format!("{dir}/{name}.svg"), svg).expect("write svg");
}
