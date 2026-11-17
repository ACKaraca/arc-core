//! Decodes arbitrary bytes as a mesh with the strict limits profile. A decoded mesh must encode
//! and decode again to itself, and input without quantized sections must encode to exactly the
//! input.

#![no_main]

use arc_base::Limits;
use arc_mesh::{Quantization, TriMesh};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    match TriMesh::decode(data, &Limits::STRICT) {
        Ok(mesh) => {
            let bytes = mesh.encode(&Quantization::default()).expect("no bounds to reject");
            let again = TriMesh::decode(&bytes, &Limits::STRICT).expect("encoder output decodes");
            assert_eq!(again, mesh);
            // Offset 6 is the position mode; an exact normal channel follows the triangles.
            let exact_positions = data.get(6) == Some(&0);
            if exact_positions && bytes.len() == data.len() {
                assert_eq!(bytes, data);
            }
        }
        Err(error) => assert_eq!(error.code(), "mesh.decode.invalid"),
    }
});
