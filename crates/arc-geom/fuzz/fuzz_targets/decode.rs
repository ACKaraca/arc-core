//! Decodes arbitrary bytes as every geometry kind with two limits profiles; a decoded value
//! must encode to exactly the input, and a decoded prefix to exactly the bytes it consumed.

#![no_main]

use std::fmt::Debug;

use arc_base::Limits;
use arc_geom::{
    Aabb2, Aabb3, Arc2, Circle2, Curve2, Ellipse2, EllipticalArc2, GeomCodec, Line2, Loop2,
    Nurbs2, Polyline2, Ray2, Region, Segment2, TriangleMesh2, ViewFrame,
};
use libfuzzer_sys::fuzz_target;

/// Small enough for the fuzzer to reach every limit.
const TIGHT: Limits = Limits {
    input_bytes: 96,
    array_length: 3,
    nesting_depth: 2,
    ..Limits::STRICT
};

fn check<T: GeomCodec + Debug + PartialEq>(data: &[u8]) {
    for limits in [&Limits::STRICT, &TIGHT] {
        let whole = T::decode(data, limits);
        match &whole {
            Ok(value) => assert_eq!(value.encode(), data, "{value:?}"),
            Err(error) => assert_eq!(error.code(), "geom.decode.invalid"),
        }
        match T::decode_prefix(data, limits) {
            Ok((value, rest)) => {
                let consumed = &data[..data.len() - rest.len()];
                assert_eq!(value.encode(), consumed, "{value:?}");
                assert!(consumed.len() as u64 <= limits.input_bytes);
                // The whole input decodes exactly when nothing follows the prefix.
                assert_eq!(whole.as_ref().ok(), rest.is_empty().then_some(&value));
            }
            Err(error) => {
                assert_eq!(error.code(), "geom.decode.invalid");
                assert!(whole.is_err());
            }
        }
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
    check::<Nurbs2>(data);
    check::<Loop2>(data);
    check::<Region>(data);
    check::<Aabb2>(data);
    check::<Aabb3>(data);
    check::<ViewFrame>(data);
    check::<TriangleMesh2>(data);
});
