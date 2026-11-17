//! Math aliases (CORE-BASE-022).
//!
//! Model math is `f64`; GPU data is `f32`. Public Arc APIs name these aliases, never the
//! third-party math crates behind them. Conversions from model to GPU precision lose precision
//! and are therefore explicit function calls.

/// A point in the plane, in model precision.
pub type Point2 = nalgebra::Point2<f64>;
/// A point in space, in model precision.
pub type Point3 = nalgebra::Point3<f64>;
/// A vector in the plane, in model precision.
pub type Vector2 = nalgebra::Vector2<f64>;
/// A vector in space, in model precision.
pub type Vector3 = nalgebra::Vector3<f64>;
/// A 3×3 matrix, in model precision.
pub type Matrix3 = nalgebra::Matrix3<f64>;
/// A 4×4 matrix, in model precision.
pub type Matrix4 = nalgebra::Matrix4<f64>;
/// An affine transform of the plane, in model precision.
pub type Transform2 = nalgebra::Affine2<f64>;
/// An affine transform of space, in model precision.
pub type Transform3 = nalgebra::Affine3<f64>;

/// A two-component vector of GPU data.
pub type Vec2f = glam::Vec2;
/// A three-component vector of GPU data.
pub type Vec3f = glam::Vec3;
/// A four-component vector of GPU data.
pub type Vec4f = glam::Vec4;
/// A 4×4 matrix of GPU data.
pub type Mat4f = glam::Mat4;

#[expect(
    clippy::cast_possible_truncation,
    reason = "narrowing to GPU precision is the purpose"
)]
fn narrow(value: f64) -> f32 {
    value as f32
}

/// Narrows a model vector to GPU precision.
#[must_use]
pub fn vector3_to_gpu(vector: &Vector3) -> Vec3f {
    Vec3f::new(narrow(vector.x), narrow(vector.y), narrow(vector.z))
}

/// Narrows a model point to GPU precision. Subtract the camera origin first: coordinates far
/// from the origin lose their fine detail in `f32`.
#[must_use]
pub fn point3_to_gpu(point: &Point3) -> Vec3f {
    vector3_to_gpu(&point.coords)
}

/// Narrows a model matrix to GPU precision.
#[must_use]
pub fn matrix4_to_gpu(matrix: &Matrix4) -> Mat4f {
    let mut columns = [0.0; 16];
    // Both crates store matrices column by column.
    for (out, value) in columns.iter_mut().zip(matrix.iter()) {
        *out = narrow(*value);
    }
    Mat4f::from_cols_array(&columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors_and_points_narrow_component_wise() {
        assert_eq!(
            vector3_to_gpu(&Vector3::new(1.5, -2.0, 0.25)),
            Vec3f::new(1.5, -2.0, 0.25)
        );
        assert_eq!(
            point3_to_gpu(&Point3::new(1.0, 2.0, 3.0)),
            Vec3f::new(1.0, 2.0, 3.0)
        );
    }

    #[test]
    fn matrices_keep_their_meaning_across_the_conversion() {
        let translation = Matrix4::new_translation(&Vector3::new(1.0, 2.0, 3.0));
        let gpu = matrix4_to_gpu(&translation);
        assert_eq!(gpu, Mat4f::from_translation(Vec3f::new(1.0, 2.0, 3.0)));
        assert_eq!(gpu.transform_point3(Vec3f::ZERO), Vec3f::new(1.0, 2.0, 3.0));
    }
}
