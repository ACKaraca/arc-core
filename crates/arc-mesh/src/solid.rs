//! Triangle meshes of polyhedral solids for display and picking.
//!
//! [`TriMesh::from_polyhedron`] triangulates every face of an [`arc_geom::Polyhedron`] with
//! [`Polyhedron::triangulate_face`], so faces with holes use the constrained triangulation of
//! `arc-geom`. The mesh is flat shaded: each face has its own vertices, each carrying the face's
//! outward unit normal, so no normal is averaged across an edge.
//!
//! The mapping from triangles to source faces is the face groups: face `i` of the solid is the
//! group whose [`SubObjectKey`] has `local == i` and `object == None`, a contiguous triangle
//! range, in face order. [`TriMesh::group_of`] therefore turns a picked triangle into a face
//! index. Every face yields at least one triangle, so every face has a group. The role of the
//! face a triangle belongs to is `solid.face_tag(group.key.local)`; it survives clipping and
//! subtraction of openings, while the face indices do not.

use std::collections::HashMap;

use arc_base::math::Point3;
use arc_base::{Error, Limits, Result, Tolerance};
use arc_geom::Polyhedron;

use crate::error::codes;
use crate::mesh::{AttributeKey, Channel, FaceGroup, SubObjectKey, TriMesh, TriMeshBuilder};

impl TriMesh {
    /// The flat-shaded triangle mesh of `solid`, with one face group per face (material slot 0)
    /// and a normal channel, as the [module documentation](self) describes.
    ///
    /// # Errors
    ///
    /// The errors of [`Polyhedron::triangulate_face`]; `mesh.limit.exceeded` when the vertex or
    /// triangle count exceeds `u32::MAX` or `limits.array_length`.
    #[allow(clippy::cast_possible_truncation, reason = "normals are stored as f32")]
    pub fn from_polyhedron(
        solid: &Polyhedron,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Self> {
        let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
        let mut positions: Vec<Point3> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        let mut groups: Vec<FaceGroup> = Vec::with_capacity(solid.faces().len());
        for index in 0..solid.faces().len() {
            let face_triangles = solid.triangulate_face(index, tolerance)?;
            let normal = solid
                .face_normal(index)
                .map_or([0.0; 3], |n| [n.x as f32, n.y as f32, n.z as f32]);
            let start =
                u32::try_from(triangles.len()).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
            let mut local: HashMap<u32, u32> = HashMap::new();
            for triangle in face_triangles {
                let mut mapped = [0_u32; 3];
                for (slot, &vertex) in mapped.iter_mut().zip(&triangle) {
                    *slot = match local.get(&vertex) {
                        Some(&id) => id,
                        None => {
                            let point = usize::try_from(vertex)
                                .ok()
                                .and_then(|v| solid.vertices().get(v))
                                .ok_or_else(|| Error::new(codes::INDEX_OUT_OF_RANGE))?;
                            let id = u32::try_from(positions.len())
                                .map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
                            positions.push(*point);
                            normals.push(normal);
                            local.insert(vertex, id);
                            id
                        }
                    };
                }
                triangles.push(mapped);
            }
            if positions.len() > limit || triangles.len() > limit {
                return Err(Error::new(codes::LIMIT_EXCEEDED));
            }
            let end =
                u32::try_from(triangles.len()).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
            groups.push(FaceGroup {
                start,
                end,
                material: 0,
                key: SubObjectKey {
                    object: None,
                    local: u32::try_from(index).map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?,
                },
            });
        }
        let mut builder = TriMeshBuilder::new(positions, triangles)
            .channel(AttributeKey::Normal, Channel::Vec3(normals));
        for group in groups {
            builder = builder.group(group);
        }
        builder.build(limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_base::math::Point2;
    use arc_geom::{Curve2, Loop2, PolyFace, Region, RegionPart, Segment2, prism};

    const TOL: Tolerance = Tolerance::DEFAULT;

    fn square(a: f64, b: f64) -> Loop2 {
        let pts = [(a, a), (b, a), (b, b), (a, b)].map(|(x, y)| Point2::new(x, y));
        let curves = (0..4)
            .map(|k| Curve2::Segment(Segment2::new(pts[k], pts[(k + 1) % 4]).expect("segment")))
            .collect();
        Loop2::new(curves, &TOL).expect("loop")
    }

    #[test]
    fn a_cube_gives_twelve_flat_triangles_and_six_groups() {
        let region = Region::new(vec![RegionPart::new(square(0.0, 1.0), vec![])]).expect("region");
        let solid = prism(&region, 0.0, 1.0, 1e-3, &TOL, &Limits::DEFAULT).expect("cube");
        let mesh = TriMesh::from_polyhedron(&solid, &TOL, &Limits::DEFAULT).expect("mesh");
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.groups().len(), 6);
        // Each triangle's winding agrees with the normal of the face its group names.
        for (t, triangle) in mesh.triangles().iter().enumerate() {
            let group = mesh
                .group_of(u32::try_from(t).expect("small"))
                .expect("group");
            let normal = solid.face_normal(group.key.local as usize).expect("face");
            let p = |k: u32| mesh.positions()[k as usize];
            let n = (p(triangle[1]) - p(triangle[0])).cross(&(p(triangle[2]) - p(triangle[0])));
            assert!(n.dot(&normal) > 0.0);
            let stored = mesh.normals().expect("normals")[triangle[0] as usize];
            assert!((f64::from(stored[2]) - normal.z).abs() < 1e-6);
        }
        let area: f64 = mesh
            .triangles()
            .iter()
            .map(|t| {
                let p = |k: u32| mesh.positions()[k as usize];
                (p(t[1]) - p(t[0])).cross(&(p(t[2]) - p(t[0]))).norm() / 2.0
            })
            .sum();
        assert!((area - solid.surface_area()).abs() < 1e-12);
    }

    #[test]
    fn faces_with_holes_triangulate_to_their_area_and_limits_apply() {
        let region = Region::new(vec![RegionPart::new(
            square(0.0, 10.0),
            vec![square(2.0, 4.0)],
        )])
        .expect("region");
        let solid = prism(&region, 0.0, 2.0, 1e-3, &TOL, &Limits::DEFAULT).expect("solid");
        let mesh = TriMesh::from_polyhedron(&solid, &TOL, &Limits::DEFAULT).expect("mesh");
        // Cap with a hole: 8 vertices need 8 triangles; sides: 8 quads = 16 triangles.
        assert_eq!(mesh.groups().len(), solid.faces().len());
        let cap_group = mesh
            .groups()
            .iter()
            .find(|g| {
                solid
                    .faces()
                    .get(g.key.local as usize)
                    .is_some_and(|f: &PolyFace| !f.holes.is_empty())
            })
            .expect("cap");
        assert_eq!(cap_group.end - cap_group.start, 8);
        let small = Limits {
            array_length: 20,
            ..Limits::DEFAULT
        };
        assert_eq!(
            TriMesh::from_polyhedron(&solid, &TOL, &small)
                .err()
                .map(|e| e.code()),
            Some("mesh.limit.exceeded")
        );
    }

    #[test]
    fn a_picked_triangle_names_the_role_of_its_face_after_a_cut() {
        use arc_geom::{ClipSide, FaceTag, Plane3};
        let region = Region::new(vec![RegionPart::new(square(0.0, 2.0), vec![])]).expect("region");
        let solid = prism(&region, 0.0, 2.0, 1e-3, &TOL, &Limits::DEFAULT).expect("block");
        let cut =
            Plane3::new(Point3::new(0.0, 0.0, 1.0), arc_base::math::Vector3::z()).expect("plane");
        let lower = solid
            .clip(&cut, ClipSide::Negative, 0, &TOL, &Limits::DEFAULT)
            .expect("clip")
            .expect("some");
        let mesh = TriMesh::from_polyhedron(&lower, &TOL, &Limits::DEFAULT).expect("mesh");
        let mut tags = std::collections::BTreeSet::new();
        for t in 0..mesh.triangle_count() {
            let group = mesh
                .group_of(u32::try_from(t).expect("small"))
                .expect("group");
            let tag = lower.face_tag(group.key.local as usize).expect("face");
            // The normal of the triangle's face agrees with its role.
            let normal = lower.face_normal(group.key.local as usize).expect("normal");
            match tag {
                FaceTag::Cap { .. } => assert!(normal.z > 0.99),
                FaceTag::Bottom => assert!(normal.z < -0.99),
                FaceTag::Side { .. } => assert!(normal.z.abs() < 1e-9),
                other => panic!("unexpected tag {other:?}"),
            }
            tags.insert(format!("{tag:?}"));
        }
        assert_eq!(tags.len(), 6, "the cap, the bottom and the four sides");
    }
}
