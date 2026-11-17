//! Decodes arbitrary bytes as every geometry kind with the strict limits profile; a decoded value
//! must encode to exactly the input.

#![no_main]

use std::fmt::Debug;

use arc_base::Limits;
use arc_geom::{
    Aabb2, Aabb3, Arc2, Circle2, Curve2, Ellipse2, EllipticalArc2, GeomCodec, Line2, Loop2,
    Polyline2, Ray2, Region, Segment2, TriangleMesh2, ViewFrame,
};
use libfuzzer_sys::fuzz_target;

fn check<T: GeomCodec + Debug>(data: &[u8]) {
    match T::decode(data, &Limits::STRICT) {
        Ok(value) => assert_eq!(value.encode(), data, "{value:?}"),
        Err(error) => assert_eq!(error.code(), "geom.decode.invalid"),
    }
}

fuzz_target!(|data: &[u8]| {
    check::<Curve2>(data);
    check::<Segment2>(data);
    check::<Ray2>(data);
    check::<Line2>(data);
    check::<Circle2>(data);
    check::<Arc2>(data);
    check::<Ellipse2>(data);
    check::<EllipticalArc2>(data);
    check::<Polyline2>(data);
    check::<Loop2>(data);
    check::<Region>(data);
    check::<Aabb2>(data);
    check::<Aabb3>(data);
    check::<ViewFrame>(data);
    check::<TriangleMesh2>(data);
});
