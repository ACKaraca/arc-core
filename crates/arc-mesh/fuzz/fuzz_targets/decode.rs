//! Decodes arbitrary bytes as a mesh with the strict limits profile. A decoded mesh must encode
//! and decode again to itself, and input without quantized sections must encode to exactly the
//! input.

#![no_main]

use arc_base::Limits;
use arc_mesh::{Quantization, TriMesh};
use libfuzzer_sys::fuzz_target;

/// Whether the input stores positions or normals quantized. Offset 6 is the position mode; with
/// exact positions the channels follow the vertices and triangles, and a normal channel (kind 1)
/// is always the first, with its mode right after the kind.
fn quantized(data: &[u8], mesh: &TriMesh) -> bool {
    if data.get(6) != Some(&0) {
        return true;
    }
    let channels = 7 + 8 + mesh.vertex_count() * 24 + 8 + mesh.triangle_count() * 12;
    let first = channels + 8;
    mesh.normals().is_some() && data.get(first + 1) == Some(&1)
}

fuzz_target!(|data: &[u8]| {
    match TriMesh::decode(data, &Limits::STRICT) {
        Ok(mesh) => {
            let bytes = mesh.encode(&Quantization::default()).expect("no bounds to reject");
            let again = TriMesh::decode(&bytes, &Limits::STRICT).expect("encoder output decodes");
            assert_eq!(again, mesh);
            if !quantized(data, &mesh) {
                assert_eq!(bytes, data);
            }
        }
        Err(error) => assert_eq!(error.code(), "mesh.decode.invalid"),
    }
});
