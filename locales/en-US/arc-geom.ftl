# Messages of the errors owned by arc-geom. Identifiers are registered in
# crates/arc-geom/src/error.rs.

geom-input-not-finite = A coordinate, length, angle, factor or transform entry is not a finite number.
geom-segment-zero-length = The segment starts and ends at the same point.
geom-direction-zero = The direction has no length.
geom-radius-not-positive = The radius must be a positive number.
geom-sweep-invalid = The arc must sweep more than zero and at most one full turn.
geom-ellipse-invalid-axis = The ellipse needs a major axis with a positive length and a minor-to-major ratio above zero and at most one.
geom-polyline-too-few-vertices = A polyline needs at least two vertices.
geom-polyline-zero-length-segment = Two consecutive polyline vertices are at the same point.
geom-polyline-negative-width = A polyline width cannot be negative.
geom-bbox-invalid = The bounding box minimum lies above its maximum.
geom-transform-singular = The transform cannot be inverted.
geom-transform-not-similarity = A polyline with arcs or widths can only be moved, rotated, mirrored or scaled uniformly.
geom-precision-insufficient = The result cannot be computed precisely enough.
geom-decode-invalid = The stored geometry is damaged or too large.
geom-tolerance-invalid = A tolerance is negative, not a finite number, or zero where a positive value is needed.
geom-view-eye-at-target = The eye and the target of the view are at the same point.
geom-view-up-invalid = The up direction must not be zero or parallel to the viewing direction.
geom-view-fov-invalid = The field of view must be greater than zero and less than a half turn.
geom-view-scale-not-positive = The view height must be a positive number.
geom-view-clip-invalid = The near clip distance must be smaller than the far one, and positive for a perspective view.
geom-view-aspect-invalid = The aspect ratio must be a positive number.
geom-length-out-of-range = The length lies outside the curve.
geom-curve-unbounded = The operation needs a curve with two ends; rays and lines have no end.
geom-flatten-too-many-vertices = Flattening the curve needs more vertices than the limit allows.
