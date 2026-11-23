# Messages of the errors owned by arc-view. Identifiers are registered in
# crates/arc-view/src/error.rs.

view-definition-invalid = A parameter of the view definition is out of range or not a finite number.
view-limit-exceeded = The view needs more objects, triangles, edges, occlusion tests or primitives than the limit allows.
view-geometry-out-of-range = The object has coordinates outside the range a view accepts.
view-section-open = The section of the object at the cut plane is not closed, so it is drawn without a fill.
view-geometry-degenerate = The object is too small to draw: none of its triangles is wider than the tolerance.
view-style-invalid = A line style, fill or symbol of the object is invalid and was replaced by a default.
