# Messages of the errors owned by arc-mesh. Identifiers are registered in
# crates/arc-mesh/src/error.rs.

mesh-index-out-of-range = The mesh has a triangle that refers to a vertex that does not exist.
mesh-value-not-finite = A position or attribute value of the mesh is not a finite number.
mesh-channel-length-mismatch = An attribute of the mesh does not have one value for every vertex.
mesh-channel-invalid = An attribute of the mesh has the wrong type, an invalid name or invalid values, or the mesh has too many attributes.
mesh-channel-missing = The operation needs an attribute the mesh does not have.
mesh-group-invalid = A face group of the mesh is empty, lies outside its triangles or overlaps another group.
mesh-limit-exceeded = The mesh has more vertices or triangles than the limit allows.
mesh-transform-singular = The transform cannot be inverted.
mesh-precision-insufficient = The result cannot be computed precisely enough.
mesh-merge-incompatible = The meshes have different attributes and cannot be merged.
mesh-tolerance-invalid = A tolerance is negative or not a finite number.
mesh-crease-invalid = The crease angle must be between zero and a half turn.
mesh-quantization-invalid = The quantization error bound must be a positive number.
mesh-decode-invalid = The stored mesh is damaged or too large.
mesh-transform-not-affine = The transform is not affine: its last row must be 0, 0, 0, 1.
mesh-measure-not-closed = The mesh is not closed and consistently oriented, so it has no volume.
mesh-measure-zero-volume = The mesh encloses no volume, so it has no centroid.
mesh-topology-non-manifold = The mesh has an edge shared by more than two triangles or a vertex that is pinched, so its genus is undefined.
mesh-query-invalid = The ray, point or distance limit is not a finite number, or the ray has no direction.
mesh-query-empty = The mesh has no triangles to query.
mesh-repair-invalid = A repair option is out of range.
mesh-simplify-invalid = Give a target triangle count or an error bound that is a finite, non-negative number.
mesh-lod-invalid = A level-of-detail setting is out of range, or the chain has no levels.
