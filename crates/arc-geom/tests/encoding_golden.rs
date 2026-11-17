//! Golden vectors of the geometry encoding (CORE-GEOM-032): the exact bytes of one value of each
//! kind, so that any change of the byte layout fails here. Changing a vector means a new encoding
//! version (docs/architecture/geometry-encoding.md).

#![allow(
    clippy::unwrap_used,
    reason = "the vectors are built from valid literal values"
)]

use std::f64::consts::{FRAC_PI_2, PI};
use std::fmt::Debug;

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Limits, Tolerance};
use arc_geom::{
    Aabb2, Aabb3, Arc2, Circle2, Curve2, Ellipse2, EllipticalArc2, GeomCodec, Line2, Loop2,
    Polyline2, PolylineVertex, Projection, Ray2, Region, RegionPart, Segment2, TriangleMesh2,
    ViewFrame,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn golden<T: GeomCodec + Debug + PartialEq>(value: &T, expected: &str) {
    let bytes = value.encode();
    assert_eq!(hex(&bytes), expected, "{value:?}");
    assert_eq!(&T::decode(&bytes, &Limits::STRICT).unwrap(), value);
}

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn square() -> Loop2 {
    let vertices = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]
        .iter()
        .map(|&(x, y)| PolylineVertex::new(p(x, y), 0.0))
        .collect();
    let polyline = Polyline2::new(vertices, true).unwrap();
    Loop2::new(vec![Curve2::Polyline(polyline)], &Tolerance::DEFAULT).unwrap()
}

#[test]
fn curves() {
    golden(
        &Segment2::new(p(1.0, 2.0), p(3.0, -4.0)).unwrap(),
        "41524347010010000000000000f03f0000000000000040000000000000084000\
        000000000010c0",
    );
    golden(
        &Ray2::new(p(0.0, 0.0), Vector2::new(1.0, 0.0)).unwrap(),
        "4152434701001100000000000000000000000000000000000000000000f03f00\
        00000000000000",
    );
    golden(
        &Line2::new(p(1.0, 1.0), Vector2::new(0.0, -1.0)).unwrap(),
        "41524347010012000000000000f03f000000000000f03f000000000000000000\
        0000000000f0bf",
    );
    golden(
        &Circle2::new(p(0.0, 0.0), 2.5).unwrap(),
        "41524347010013000000000000000000000000000000000000000000000440",
    );
    golden(
        &Arc2::new(p(1.0, 0.0), 2.0, FRAC_PI_2, -PI).unwrap(),
        "41524347010014000000000000f03f0000000000000000000000000000004018\
        2d4454fb21f93f182d4454fb2109c0",
    );
    let ellipse = Ellipse2::new(p(0.0, 0.0), Vector2::new(2.0, 0.0), 0.5).unwrap();
    golden(
        &ellipse,
        "4152434701001500000000000000000000000000000000000000000000004000\
        00000000000000000000000000e03f",
    );
    golden(
        &EllipticalArc2::new(ellipse, 0.0, PI).unwrap(),
        "4152434701001600000000000000000000000000000000000000000000004000\
        00000000000000000000000000e03f0000000000000000182d4454fb210940",
    );
    let polyline = Polyline2::new(
        vec![
            PolylineVertex {
                point: p(0.0, 0.0),
                bulge: 1.0,
                start_width: 0.5,
                end_width: 0.25,
            },
            PolylineVertex::new(p(2.0, 0.0), 0.0),
        ],
        false,
    )
    .unwrap();
    golden(
        &polyline,
        "4152434701001700020000000000000000000000000000000000000000000000\
        000000000000f03f000000000000e03f000000000000d03f0000000000000040\
        0000000000000000000000000000000000000000000000000000000000000000",
    );
    golden(
        &Curve2::Polyline(polyline.clone()),
        &hex(&polyline.encode()),
    );
}

#[test]
fn loops_and_regions() {
    golden(
        &square(),
        "4152434701002001000000000000001701040000000000000000000000000000\
        0000000000000000000000000000000000000000000000000000000000000000\
        0000000000000010400000000000000000000000000000000000000000000000\
        0000000000000000000000000000001040000000000000104000000000000000\
        0000000000000000000000000000000000000000000000000000000000000010\
        40000000000000000000000000000000000000000000000000",
    );
    let hole = Loop2::new(
        vec![Curve2::Circle(Circle2::new(p(2.0, 2.0), 1.0).unwrap())],
        &Tolerance::DEFAULT,
    )
    .unwrap();
    let region = Region::new(vec![RegionPart::new(square(), vec![hole])]).unwrap();
    golden(
        &region,
        "4152434701002101000000000000002001000000000000001701040000000000\
        0000000000000000000000000000000000000000000000000000000000000000\
        0000000000000000000000000000000010400000000000000000000000000000\
        0000000000000000000000000000000000000000000000001040000000000000\
        1040000000000000000000000000000000000000000000000000000000000000\
        0000000000000000104000000000000000000000000000000000000000000000\
        0000010000000000000020010000000000000014000000000000004000000000\
        00000040000000000000f03f0000000000000000182d4454fb2119c0",
    );
}

#[test]
fn boxes_frames_and_meshes() {
    golden(&Aabb2::EMPTY, "4152434701003000");
    golden(
        &Aabb2::from_corners(p(-1.0, 0.0), p(2.0, 3.0)),
        "4152434701003001000000000000f0bf00000000000000000000000000000040\
        0000000000000840",
    );
    golden(&Aabb3::EMPTY, "4152434701003100");
    golden(
        &Aabb3::from_corners(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 2.0, 3.0)),
        "4152434701003101000000000000000000000000000000000000000000000000\
        000000000000f03f00000000000000400000000000000840",
    );
    let frame = ViewFrame::new(
        Point3::new(0.0, -10.0, 5.0),
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Projection::Perspective { vertical_fov: 1.0 },
        0.5,
        100.0,
    )
    .unwrap();
    golden(
        &frame,
        "41524347010040000000000000000000000000000024c0000000000000144000\
        0000000000000000000000000000000000000000000000000000000000000000\
        00000000000000000000000000f03f00000000000000f03f000000000000e03f\
        0000000000005940",
    );
    let mesh =
        TriangleMesh2::new(vec![p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0)], vec![[0, 1, 2]]).unwrap();
    golden(
        &mesh,
        "4152434701005003000000000000000000000000000000000000000000000000\
        0000000000f03f00000000000000000000000000000000000000000000f03f01\
        00000000000000000000000100000002000000",
    );
}
