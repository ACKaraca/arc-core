//! Affine transforms of triangle meshes.
//!
//! [`TriMesh::transformed`] maps positions by the transform, normals by the inverse transpose of
//! its linear part and tangent directions by the linear part, both normalized afterwards (a zero
//! normal or tangent stays zero). Under a mirror (a linear part with a negative determinant) the
//! handedness of every tangent is negated and the winding of every triangle is reversed by
//! swapping its second and third index, so face normals from the winding keep agreeing with the
//! transformed vertex normals. Other channels and the face groups are unchanged.
//!
//! A matrix whose bottom row is not `[0, 0, 0, 1]` (possible only through an unchecked
//! constructor) is rejected rather than having that row ignored.
//!
//! The linear part is singular when it has a non-finite entry, all entries zero, or, divided
//! by its largest entry magnitude, an exactly zero determinant; scaling first keeps invertible
//! transforms with very large or very small entries usable.

use arc_base::math::{Matrix3, Matrix4, Transform3, Vector3};
use arc_base::{Error, Result};

use crate::error::codes;
use crate::mesh::{AttributeKey, Channel, TriMesh};
use crate::unit;

fn to_f64(v: &[f32]) -> Vector3 {
    let get = |i: usize| f64::from(v.get(i).copied().unwrap_or(0.0));
    Vector3::new(get(0), get(1), get(2))
}

fn direction(v: &Vector3) -> [f32; 3] {
    unit(v).map_or([0.0; 3], |u| arc_base::math::vector3_to_gpu(&u).to_array())
}

impl TriMesh {
    /// This mesh moved by `transform`; see the module documentation.
    ///
    /// # Errors
    ///
    /// `mesh.transform.singular` when the transform is not finite or not invertible;
    /// `mesh.transform.not_affine` when its matrix (built unchecked) does not end in the row
    /// `[0, 0, 0, 1]`; `mesh.precision.insufficient` when a transformed position overflows.
    pub fn transformed(&self, transform: &Transform3) -> Result<TriMesh> {
        let matrix = transform.matrix();
        if !matrix.iter().all(|c| c.is_finite()) {
            return Err(Error::new(codes::TRANSFORM_SINGULAR));
        }
        // An affine transform ignores this row, so a projective one would be applied wrongly.
        if matrix.row(3) != Matrix4::identity().row(3) {
            return Err(Error::new(codes::TRANSFORM_NOT_AFFINE));
        }
        let linear: Matrix3 = matrix.fixed_view::<3, 3>(0, 0).into_owned();
        let largest = linear.amax();
        let scaled = if largest > 0.0 {
            linear / largest
        } else {
            linear
        };
        let determinant = scaled.determinant();
        if determinant == 0.0 || !determinant.is_finite() {
            return Err(Error::new(codes::TRANSFORM_SINGULAR));
        }
        let mirror = determinant < 0.0;

        let positions = self
            .positions
            .iter()
            .map(|p| {
                let image = transform.transform_point(p);
                if image.iter().all(|c| c.is_finite()) {
                    Ok(image)
                } else {
                    Err(Error::new(codes::PRECISION_INSUFFICIENT))
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let triangles = if mirror {
            self.triangles.iter().map(|&[a, b, c]| [a, c, b]).collect()
        } else {
            self.triangles.clone()
        };

        // The cofactor matrix is the inverse transpose times the determinant; its columns are
        // cross products of the columns of the matrix. The sign of the determinant is restored.
        let [c0, c1, c2] = [0, 1, 2].map(|i| scaled.column(i).into_owned());
        let cofactor = Matrix3::from_columns(&[c1.cross(&c2), c2.cross(&c0), c0.cross(&c1)])
            * determinant.signum();
        let mut channels = self.channels.clone();
        if let Some(Channel::Vec3(normals)) = channels.get_mut(&AttributeKey::Normal) {
            for n in normals.iter_mut() {
                *n = direction(&(cofactor * to_f64(n)));
            }
        }
        if let Some(Channel::Vec4(tangents)) = channels.get_mut(&AttributeKey::Tangent) {
            for t in tangents.iter_mut() {
                let [x, y, z] = direction(&(scaled * to_f64(t)));
                *t = [x, y, z, if mirror { -t[3] } else { t[3] }];
            }
        }
        Ok(TriMesh {
            positions,
            triangles,
            channels,
            groups: self.groups.clone(),
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test values are small and exact enough"
)]
mod tests {
    use arc_base::math::Point3;
    use arc_base::{Limits, Tolerance};
    use proptest::prelude::*;

    use super::*;
    use crate::normals::face;
    use crate::testing::cube;
    use crate::{NormalWeighting, angle_between};

    fn affine(m: Matrix4) -> Transform3 {
        Transform3::from_matrix_unchecked(m)
    }

    fn framed(mesh: &TriMesh) -> TriMesh {
        let uvs = mesh
            .positions()
            .iter()
            .map(|p| [(p.x + 0.3 * p.y) as f32, (p.z - 0.2 * p.x + p.y) as f32])
            .collect();
        mesh.clone()
            .into_builder()
            .channel(AttributeKey::Uv(0), Channel::Vec2(uvs))
            .build(&Limits::STRICT)
            .unwrap()
            .with_normals(
                NormalWeighting::Flat,
                0.0,
                &Tolerance::DEFAULT,
                &Limits::STRICT,
            )
            .unwrap()
            .with_tangents(0, &Tolerance::DEFAULT)
            .unwrap()
    }

    #[test]
    fn singular_and_non_finite_transforms_are_rejected() {
        let mesh = cube();
        let mut flat = Matrix4::identity();
        flat[(2, 2)] = 0.0;
        let mut nan = Matrix4::identity();
        nan[(0, 3)] = f64::NAN;
        let mut zero = Matrix4::zeros();
        zero[(3, 3)] = 1.0;
        for m in [flat, nan, zero] {
            assert_eq!(
                mesh.transformed(&affine(m)).unwrap_err().code(),
                "mesh.transform.singular"
            );
        }
        let huge = affine(Matrix4::new_scaling(1e300));
        let far = TriMesh::new(vec![Point3::new(1e10, 0.0, 0.0)], vec![], &Limits::STRICT).unwrap();
        assert_eq!(
            far.transformed(&huge).unwrap_err().code(),
            "mesh.precision.insufficient"
        );
        // A projective bottom row is not ignored but rejected.
        let mut projective = Matrix4::identity();
        projective[(3, 0)] = 1.0;
        projective[(3, 3)] = 0.0;
        let mut shifted = Matrix4::identity();
        shifted[(3, 3)] = 2.0;
        for m in [projective, shifted] {
            assert_eq!(
                mesh.transformed(&affine(m)).unwrap_err().code(),
                "mesh.transform.not_affine"
            );
        }
        // Invertible with tiny entries.
        assert!(
            mesh.transformed(&affine(Matrix4::new_scaling(1e-200)))
                .is_ok()
        );
    }

    #[test]
    fn a_mirror_reverses_winding_and_handedness() {
        let mesh = framed(&cube());
        let mirror = affine(Matrix4::new_nonuniform_scaling(&Vector3::new(
            -1.0, 1.0, 1.0,
        )));
        let image = mesh.transformed(&mirror).unwrap();
        assert_eq!(image.triangles()[0], {
            let [a, b, c] = mesh.triangles()[0];
            [a, c, b]
        });
        for (t, u) in image
            .tangents()
            .unwrap()
            .iter()
            .zip(mesh.tangents().unwrap())
        {
            assert_eq!(t[3], -u[3]);
        }
    }

    proptest! {
        /// After any well-conditioned affine transform, vertex normals still agree with the
        /// face normals of the transformed winding, tangents stay orthogonal to the normals and
        /// the frame's bitangent `w · (n × t)` is the image of the original bitangent.
        #[test]
        fn frames_follow_any_affine_transform(
            entries in prop::array::uniform9(-3.0..3.0_f64),
            offset in prop::array::uniform3(-1e3..1e3_f64),
            exponent in -100.0..100.0_f64,
        ) {
            let linear = Matrix3::from_row_slice(&entries) * 10.0_f64.powf(exponent);
            let scaled = linear / linear.amax();
            prop_assume!(scaled.determinant().abs() > 0.2);
            let mut m = linear.to_homogeneous();
            m.fixed_view_mut::<3, 1>(0, 3)
                .copy_from(&(Vector3::from(offset) * 10.0_f64.powf(exponent)));
            let mesh = framed(&cube());
            let image = mesh.transformed(&affine(m)).unwrap();
            let normals = image.normals().unwrap();
            let exact = Tolerance { linear: 0.0, ..Tolerance::DEFAULT };
            for t in image.triangles() {
                let geometry = face(image.positions(), *t, exact.linear).unwrap();
                for &i in t {
                    let n = to_f64(&normals[i as usize]);
                    prop_assert!(angle_between(&n, &geometry.normal) < 1e-5);
                }
            }
            for (i, (t, u)) in image.tangents().unwrap().iter().zip(mesh.tangents().unwrap()).enumerate() {
                let n = to_f64(&normals[i]);
                let tangent = to_f64(t);
                prop_assert!(n.dot(&tangent).abs() < 1e-5);
                let before = to_f64(&mesh.normals().unwrap()[i]).cross(&to_f64(u)) * f64::from(u[3]);
                let after = n.cross(&tangent) * f64::from(t[3]);
                prop_assert!((scaled * before).dot(&after) > 0.0);
            }
        }
    }
}
