# arc-geom

Pure-Rust analytic geometry owned by the project: 2D and 3D curves, surfaces, NURBS, intersections,
planar regions, triangulation, TIN surfaces, spatial indexes and view frames.

## Owns

- Exact curve types (lines, arcs, circles, ellipses, bulge polylines, NURBS, transition spirals,
  composites) and their evaluation, queries, offsets, splitting and joining.
- Analytic and NURBS surfaces with evaluation and closest-point queries.
- Planar regions with booleans, offsets, containment and the hatch-boundary model.
- Constrained Delaunay triangulation, polygon triangulation for display and TIN surfaces.
- R*-tree and k-d tree spatial indexes with incremental updates.
- Transforms, coordinate frames and the view frame shared by views, scenes and the viewport.
- The documented, versioned binary encoding of every geometry value stored in documents.

## Stays in products

Domain geometry (alignment design rules, wall layer offsets, corridor templates), tolerance policies
for a product's documents, and any geometry whose only user is one product until a second user
appears.

## Key concepts and types

| Concept | Description |
|---|---|
| `Curve2`, `Curve3` | Closed sum of curve kinds with a common evaluation interface (point, derivatives, length, parameter range) |
| `Polyline2` | Vertices with bulge factors and optional widths; arcs stay exact |
| `Nurbs` | Degree, knots, weighted control points; rational and non-rational |
| `Spiral` | Clothoid and other transition curves defined by start and end curvature and length |
| `Surface` | Plane, cylinder, cone, sphere, torus, extrusion, revolution, NURBS |
| `Region` | Set of outer loops with holes, each loop a closed composite of exact curves, with orientation invariants |
| `Tin` | Triangulated surface with vertices, triangles, breaklines, boundary and holes |
| `SpatialIndex<T>` | R*-tree over bounding boxes keyed by caller identifiers |
| `ViewFrame` | Eye, target, up, projection, field of view or scale, clip planes |
| `GeomEncoding` | Encoding version and canonical byte form for each type |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base` |
| Third-party | `curvo`, `i_overlay`, `kurbo`, `robust`, `spade`, `rstar`, `nalgebra` (through `arc-base` aliases) |

## Test strategy

Property tests for every operation (intersection symmetry, offset-distance invariants, region boolean
algebra identities, area conservation under transforms); a geometry robustness corpus of degenerate
cases (tangent, coincident, collinear, near-degenerate within and just beyond tolerance) with
expected outcomes; fuzzing of the geometry decoder; benchmarks for intersections, booleans and
index queries; WebAssembly build in CI.

## Capabilities

### Curves

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-001 | Exact 2D primitives | Represents points, segments, rays, infinite lines, circles, circular arcs, ellipses and elliptical arcs exactly; arcs are never converted to splines or polylines in storage. | T1 | 0 |
| CORE-GEOM-002 | Bulge polylines | Represents polylines whose segments are lines or arcs defined by bulge, with optional start and end widths per vertex, open or closed. | T1 | 0 |
| CORE-GEOM-003 | Curve evaluation | Evaluates point, first and second derivatives, tangent, normal, curvature and length for every curve kind, and the parameter at a given length within the linear tolerance. | T1 | 0 |
| CORE-GEOM-004 | Closest point and projection | Returns the closest point and parameter on a curve to a given point, and all perpendicular foot points, for every curve kind. | T1 | 0 |
| CORE-GEOM-005 | Curve intersections | Computes all intersections of two curves with classification (crossing, tangent, overlap interval); overlapping collinear or concentric curves return intervals, not sampled points. | T1 | 0 |
| CORE-GEOM-006 | Split, trim, extend, join | Splits curves at parameters or points, trims and extends to boundary curves, and joins end-connected curves into composites within tolerance, reporting gaps that prevent joining. | T1 | 1 |
| CORE-GEOM-007 | Curve offsets | Offsets lines, arcs, circles, ellipses (as exact offset curves approximated by NURBS within tolerance), polylines and composites, with join styles and removal of self-intersecting loops. | T1 | 1 |
| CORE-GEOM-008 | NURBS curves | Evaluates, inserts and removes knots, elevates degree, splits, reverses and fits NURBS curves through points or control points with a stated tolerance. | T1 | 1 |
| CORE-GEOM-009 | Transition spirals | Represents clothoid spirals and other transition curves by length and end curvatures; evaluates position, heading and curvature at any distance to within 1e-9 m for lengths up to 10 km. | T2 | 3 |
| CORE-GEOM-010 | 3D curves | Provides 3D lines, arcs, polylines, helices and NURBS with the same evaluation and intersection interfaces as 2D, including curve-plane intersection. | T1 | 2 |
| CORE-GEOM-011 | Curve flattening | Converts any curve to a polyline that stays within a chord tolerance and an angle tolerance supplied by the caller; the vertex count is minimal for arcs given the tolerances. | T1 | 0 |

### Surfaces

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-012 | Analytic surfaces | Evaluates planes, cylinders, cones, spheres and tori: point, normal, curvature, closest point and parameter for a point. | T1 | 2 |
| CORE-GEOM-013 | NURBS surfaces | Evaluates, splits and reverses NURBS surfaces and extracts isoparametric curves. | T2 | 3 |
| CORE-GEOM-014 | Swept surfaces | Represents extrusion and revolution surfaces of any curve with evaluation and closest point. | T2 | 3 |
| CORE-GEOM-015 | Curve-surface intersection | Intersects curves with surfaces, returning points and parameters with tangency classification. | T2 | 3 |

### Regions

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-016 | Region construction | Builds regions from closed curves, detecting outer loops and holes by containment and normalizing orientation; open or self-intersecting input is reported with the offending curves. | T1 | 1 |
| CORE-GEOM-017 | Boundary tracing | Finds the smallest closed region containing a picked point from a set of arbitrary curves, including islands, for hatching and area tools. | T1 | 1 |
| CORE-GEOM-018 | Region booleans | Computes union, difference, intersection and exclusive-or of regions; output edges lying on input arcs within tolerance are restored as exact arcs. | T1 | 1 |
| CORE-GEOM-019 | Region offsets | Offsets regions inward and outward with round, miter (with limit) and square joins, merging or removing loops as required. | T1 | 1 |
| CORE-GEOM-020 | Region measurements | Computes area, perimeter, centroid and second moments of regions exactly for line and arc boundaries. | T1 | 1 |
| CORE-GEOM-021 | Point containment | Classifies points as inside, outside or on the boundary (within tolerance) of a region using robust predicates. | T1 | 0 |

### Triangulation and terrain

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-022 | Polygon triangulation | Triangulates regions with holes for display, after flattening at a supplied tolerance, without slivers below a minimum angle where the input allows. | T1 | 0 |
| CORE-GEOM-023 | Constrained Delaunay triangulation | Triangulates point sets with constraint edges; constraints are preserved exactly and intersecting constraints are split at their intersection. | T1 | 2 |
| CORE-GEOM-024 | TIN surfaces | Builds TIN surfaces from points, breaklines, boundaries and holes, supports incremental point and breakline edits, and rejects crossing breaklines with conflicting elevations with a report. | T1 | 3 |
| CORE-GEOM-025 | TIN queries | Returns elevation, slope and aspect at a point, profiles along curves and contours at intervals; queries outside the boundary return "no surface". | T1 | 3 |
| CORE-GEOM-026 | Surface volumes | Computes cut and fill volumes between two TIN surfaces or a TIN and a datum within a boundary, with prismoidal accuracy. | T2 | 3 |

### Spatial indexes, transforms and encoding

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-027 | R*-tree index | Indexes 2D and 3D bounding boxes with insert, remove and update; window, crossing, nearest-k and ray queries return results identical to a brute-force scan. | T1 | 0 |
| CORE-GEOM-028 | Point k-d tree | Indexes point sets for nearest-neighbor and radius queries, used for snapping to vertices and point clouds. | T1 | 1 |
| CORE-GEOM-029 | Transforms and frames | Applies rigid, similarity and affine transforms to every geometry kind; transforming an arc by a non-uniform scale produces an exact elliptical arc. | T1 | 0 |
| CORE-GEOM-030 | View frame | Defines eye, target, up, perspective or orthographic projection and clip planes, with world-to-view and view-to-world mappings in `f64`. | T1 | 0 |
| CORE-GEOM-031 | Robust predicates | Decides orientation and in-circle tests with adaptive exact arithmetic; results are consistent for permuted inputs. | T1 | 0 |
| CORE-GEOM-032 | Geometry encoding | Encodes every geometry value in a documented, versioned canonical byte form; decoding rejects malformed or oversized input with `geom.decode.invalid` and never panics. | T1 | 0 |

### Curve parameterization

Every 2D curve kind has a parameter range and evaluates the point at a parameter; evaluation
clamps the parameter to the range. Arcs store a start angle and a signed sweep: positive runs
counterclockwise, negative clockwise, and a magnitude of 2π is a full turn. A bulge is the tangent
of a quarter of the included angle of the segment that starts at the vertex: 0 is straight, a
positive bulge bends counterclockwise.

The start angle of an arc and the start parameter of an elliptical arc are normalized into
[0, 2π) on construction and on deserialization. A value already in that interval is kept bit for
bit, so stored values round-trip exactly; any other finite value is reduced through its sine and
cosine, which use an exact argument reduction, so the normalized angle names the same direction
as the given one however large it is. Reversal and transforms produce normalized values the same
way, and every later computation (evaluation, sweep membership, bounding boxes) works on the
normalized angle.

A bulged polyline segment is evaluated relative to its chord, from the vertices, the bulge and
sines and cosines of fractions of the included angle; no value is derived from the center. Its
end points are the vertices exactly, and its points and bounding box are accurate to a few units
in the last place of the larger of the chord length and the sagitta for every accepted bulge,
however close to 0. The segment is also available in center-radius form as an arc; for a nearly
straight segment that form is far from the chord, and its center, radius, start angle and sweep
reproduce the vertices only to the rounding of the radius. A bulge whose center-radius form
cannot be represented in `f64` (center or radius out of range for a bulge very close to 0, or a
sweep that rounds to a full turn for a bulge of about 1e16 or more in magnitude) is rejected with
`geom.precision.insufficient`. A closed polyline with exactly two vertices is valid, including
with both bulges 0: it is a degenerate loop whose second segment retraces the first, as some file
formats store it.

| Kind | Range | Parameter |
|---|---|---|
| Segment | [0, 1] | Linear fraction from start to end |
| Ray | [0, +∞) | Distance from the origin |
| Line | (−∞, +∞) | Signed distance from the reference point |
| Circle | [0, 2π] | Angle from the positive x axis, counterclockwise |
| Arc | [0, \|sweep\|] | Angle traveled from the start, in the sweep direction |
| Ellipse | [0, 2π] | Eccentric angle from the major axis, counterclockwise |
| Elliptical arc | [0, \|sweep\|] | Eccentric angle traveled from the start, in the sweep direction |
| Polyline | [0, n] for n segments | Segment index plus the fraction along it; arcs proportionally to angle |

Reversing a curve maps parameter `t` to `a + b − t` for range [a, b] (`−t` for a line); a circle
or ellipse reverses into a clockwise full arc, and a ray has no reverse. Rays and lines have no
bounding box; every other kind has an exact tight one that includes the axis extrema inside an
arc's sweep.

Values are validated on construction and on deserialization. Degeneracy is exact (a zero length,
radius, sweep or direction), so validity never depends on a document tolerance. Every finite
nonzero direction and every finite positive length is accepted, however large or small, and
lengths and unit directions are computed without squaring components, so every accepted value
evaluates to finite points.

### Transform rules

Rigid, similarity and affine transforms are all affine maps; the result depends on the map's
effect:

- Segments, rays and lines map point by point.
- A circle or circular arc stays one when its image's semi-axes differ by at most the linear
  tolerance, and otherwise becomes an exact ellipse or elliptical arc. Ellipses and elliptical
  arcs stay ellipses and elliptical arcs; the new axes are the principal axes of the transformed
  conjugate semi-diameters. A mirror reverses the sweep.
- Open arcs and elliptical arcs keep their parameter: the image of the point at `t` is the point at
  `t` of the result. A full circle or ellipse maps to one in its own canonical parameterization.
- A polyline maps vertex by vertex; bulges are negated under a mirror and widths scale by the mean
  scale factor. When the polyline has a nonzero bulge or width, the two principal scale factors
  must differ by at most the linear tolerance divided by its largest width or arc extent (half the
  chord times one plus the absolute bulge); otherwise the transform fails with
  `geom.transform.not_similarity`. A polyline without bulges or widths accepts any affine map.
- A transform is singular when its linear 2×2 block, divided by its largest entry magnitude, is
  zero or has an exactly zero determinant; it then fails with `geom.transform.singular`.
  Invertible transforms with very large or very small entries are accepted.
- A linear or angular tolerance that is NaN, infinite or negative fails with
  `geom.tolerance.invalid` before anything is computed.
- Lengths and principal axes are computed from values scaled by their largest component
  magnitude. When the image of a valid curve cannot be represented in `f64` (it overflows, its
  axes or segments underflow until they would degenerate, or an image ellipse would have a
  subnormal minor-to-major ratio), the transform fails with `geom.precision.insufficient`.

### Robust predicates

`orient2d` and `incircle` return the exact sign of their determinant. Before the adaptive exact
evaluation, all coordinates are multiplied by one common power of two chosen from the largest
coordinate magnitude, which is exact and keeps every intermediate value of the exact arithmetic
away from overflow and from subnormal values. With `E` the exponent of the largest coordinate
magnitude (2^E ≤ |x| < 2^(E+1)) and `L` the exponent of the lowest set bit of any nonzero
coordinate (−1074 for the smallest subnormal), the sign is exact when `E − L ≤ 957` for
`orient2d` and `E − L ≤ 477` for `incircle`; for coordinates with full 53-bit significands these
are magnitude ratios of up to about 1e272 and 1e128. A wider span fails with
`geom.precision.insufficient`. Both exponents depend only on the set of coordinate values, so
every argument order of the same points gives consistently permuted signs or fails alike.

### View frame

`ViewFrame` (CORE-GEOM-030) holds eye, target, an up hint, a projection (`Perspective` with a
vertical field of view in radians, or `Orthographic` with a view height in model units) and near
and far clip distances. View space is right-handed: x right, y up, the camera looks along negative
z, and the basis `(right, up, forward)` satisfies `right x up = -forward`. Clip space has x and y
in [-1, 1] and normalized depth 0 at near and 1 at far. The aspect ratio (width over height) is
not stored; every function that needs it takes it, because one frame serves views of different
sizes. World-to-view and view-to-world are rigid and computed from the basis without matrix
inversion. `project` returns `None` for a perspective point at or behind the eye plane. The eye
and target distance uses the linear tolerance and the up direction the angular tolerance passed
to the constructor; deserialization validates with the default tolerance. Orthographic frames may
have a zero or negative near distance.

### Error codes

The codes `arc-geom` owns. A code is never reused for a different meaning; messages are in
`locales/en-US/arc-geom.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `geom.input.not_finite` | `invalid_input` | A coordinate, length, angle, factor or transform entry is NaN or infinite |
| `geom.segment.zero_length` | `invalid_input` | A segment starts and ends at the same point |
| `geom.direction.zero` | `invalid_input` | The direction of a ray or line is the zero vector |
| `geom.radius.not_positive` | `invalid_input` | The radius of a circle or arc is zero or negative |
| `geom.sweep.invalid` | `invalid_input` | An arc sweep is zero or larger than a full turn |
| `geom.ellipse.invalid_axis` | `invalid_input` | An ellipse has a zero major axis or a minor-to-major ratio outside (0, 1] |
| `geom.polyline.too_few_vertices` | `invalid_input` | A polyline has fewer than two vertices |
| `geom.polyline.zero_length_segment` | `invalid_input` | Two consecutive polyline vertices are equal |
| `geom.polyline.negative_width` | `invalid_input` | A polyline width is negative |
| `geom.bbox.invalid` | `invalid_input` | A stored bounding box has a minimum above its maximum |
| `geom.transform.singular` | `invalid_input` | A transform is not invertible |
| `geom.transform.not_similarity` | `unsupported` | A polyline with arcs or widths is given a transform that is not a similarity within tolerance |
| `geom.precision.insufficient` | `failed_operation` | A computation exceeds the range or precision of `f64`: a transform image that is not representable, a bulge whose arc is not representable, predicate coordinates that span too many binary exponents |
| `geom.decode.invalid` | `corrupt_data` | Encoded geometry is malformed or oversized |
| `geom.tolerance.invalid` | `invalid_input` | A linear or angular tolerance is NaN, infinite or negative |
| `geom.view.eye_at_target` | `invalid_input` | The eye and target of a view frame are closer than the linear tolerance |
| `geom.view.up_invalid` | `invalid_input` | The up vector of a view frame is zero or within the angular tolerance of parallel to the view direction |
| `geom.view.fov_invalid` | `invalid_input` | A perspective field of view is outside (0, π) |
| `geom.view.scale_not_positive` | `invalid_input` | An orthographic view height is zero or negative |
| `geom.view.clip_invalid` | `invalid_input` | Near is not smaller than far, or near is not positive for a perspective frame |
| `geom.view.aspect_invalid` | `invalid_input` | An aspect ratio is not a finite positive number |

### Spatial index (CORE-GEOM-027)

`SpatialIndex2<K>` (over `Aabb2`) and `SpatialIndex3<K>` (over `Aabb3`) are R*-trees keyed by a
caller identifier `K: Copy + Eq + Hash + Ord`. The tree library is private; no type of it appears
in a public signature. A key-to-box map beside the tree makes `remove`, `update` and `get` work by
key.

- `insert(key, box)` replaces the box of an existing key and returns the previous one. `update`
  replaces only an existing key and returns `false` when the key is absent. `remove` returns the
  removed box. `from_entries` bulk loads; the last pair of a repeated key wins.
- Boxes are closed. `window` returns the keys of entries entirely inside the query box;
  `crossing` returns the keys of entries that intersect it, touching included. Both are sorted by
  key.
- `nearest(point, k)` returns the `k` entries with the smallest Euclidean distance from the point
  to their box (zero inside or on the box), sorted by distance then key; entries tied with the
  k-th distance are resolved by key.
- `ray(origin, direction, max_distance)` returns entries the ray meets at a distance in
  `0..=max_distance` (the direction is normalized), with the entry distance, sorted by distance
  then key. The slab test treats a zero direction component as a hit only when the origin lies in
  that slab; an origin inside a box gives distance 0; a negative `max_distance` gives no hits.
- An empty box is rejected with `geom.bbox.invalid`; a non-finite coordinate, point or
  `max_distance` with `geom.input.not_finite`; a zero ray direction with `geom.direction.zero`.
  A rejected call leaves the index unchanged.

The property tests compare every query with a brute-force scan after random sequences of insert,
remove and update, in 2D and 3D, on integer-grid boxes that force touching and exact ties.
