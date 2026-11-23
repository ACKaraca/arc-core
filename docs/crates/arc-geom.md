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
| `Nurbs2` | Degree, knots, weighted control points; rational and non-rational |
| `Spiral` | Clothoid and other transition curves defined by start and end curvature and length |
| `Surface` | Plane, cylinder, cone, sphere, torus, extrusion, revolution, NURBS |
| `Plane3`, `PlaneFrame` | A plane as a point and unit normal, and its right-handed 2D frame for mapping between 3D and plane coordinates |
| `Polyhedron` | Closed, oriented solid with planar faces (outer loop and holes), built by extrusion, clipped by planes and triangulated per face |
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
| NURBS | [u_p, u_n], the first and last knots | The knot parameter |

Reversing a curve maps parameter `t` to `a + b − t` for range [a, b] (`−t` for a line, and for a
NURBS curve, whose reversal negates its knots so that they stay exact); a circle
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
- A NURBS curve maps control point by control point with the same knots and weights, which is
  exact for every affine map.
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
inversion. `project` returns `None` for a perspective point at or behind the eye plane.
Orthographic frames may have a zero or negative near distance.

Degeneracy is decided exactly, without a tolerance, so construction and deserialization accept
the same frames: the eye must differ from the target, and the up hint must be nonzero and not
parallel to the view direction (its cross product with the computed unit view direction, taken
with accurate differences of products, is not exactly zero). Lengths and unit vectors are
computed from components scaled by the largest one, so every finite, non-degenerate frame is
accepted whatever its magnitude. A frame whose matrices cannot be represented in `f64` (a field
of view or height whose reciprocal overflows, a depth span or perspective depth coefficient that
overflows, or a world-to-view translation that overflows) is rejected with
`geom.precision.insufficient`. Afterwards no function returns a non-finite value as a success:
`projection_matrix`, `inverse_projection_matrix`, `unproject` and `ray_through` fail with
`geom.precision.insufficient` when their result would not be finite (for example an aspect so
extreme that a matrix entry overflows), and `project` returns `None`. The depth is computed in a
form that cannot overflow when the plain one does, and stays exactly 0 at near and 1 at far.

### Evaluation, projection and flattening

`CurveEval`, `CurveProject` and `CurveFlatten` are implemented by every 2D curve type and by
`Curve2` (CORE-GEOM-003, CORE-GEOM-004, CORE-GEOM-011).

- Derivatives are taken with respect to the parameter in the table above, at the clamped
  parameter. The unit normal is the unit tangent turned a quarter turn counterclockwise;
  curvature is signed, positive where the curve turns counterclockwise. At an interior polyline
  vertex the derivatives are those of the segment that starts there. A NaN parameter gives NaN
  points and derivatives, except for segments, rays and lines, whose derivatives are constant.
- A bulged polyline segment is differentiated and projected relative to its chord, like its
  points: the tangent at angle fraction `f` is the chord direction turned by `(2f − 1)·α`, with
  `α` the signed half included angle, and nothing is derived from the center. Its derivatives,
  curvature and foot points stay accurate for every accepted bulge.
- Lengths, speeds, distances and unit vectors are computed from values scaled by their largest
  component, and ellipse computations from the axes divided by the major semi-axis length, so
  every curve a constructor accepts evaluates without overflow or underflow. A query so far
  from a curve, relative to its size, that its coordinates in the curve's frame overflow fails
  with `geom.precision.insufficient`.
- The length of a ray or line is infinite. The parameter at a length is measured from the start
  of the range (from parameter 0 for a line, negative lengths backwards); a length outside
  [0, length] by more than the linear tolerance fails with `geom.length.out_of_range`.
- Elliptical arc lengths are integrated with an adaptive 8-point Gauss–Legendre rule split at
  the quadrant angles, with an estimated error below 1/64 of the linear tolerance (or 64
  units in the last place of the length scale, when larger) and a bounded number of intervals;
  the parameter at a length is found by safeguarded Newton iteration, so its point is within
  the linear tolerance of the true point where `f64` resolves that.
- Foot points are the parameters where the vector to the query point is perpendicular to the
  tangent within the linear tolerance. When every point of an arc or circle is one (the query
  is its center) the result is an interval covering it. Ellipse foot points are found by
  splitting the turn into at most six intervals on which the perpendicularity function has at
  most one zero, then refining each sign change to convergence; the quadrant angles use exact
  sines and cosines. A root is accepted when the function is within its rounding error or the
  bracket cannot shrink further in `f64`; far from a large curve, where `f64` cannot resolve
  the linear tolerance, the tangential residual is then of the order of
  `ε·(a + |q − center|)·a / |C′(θ)|` for semi-major axis `a` and machine epsilon `ε`. Foot
  points closer than the linear tolerance, or than the rounding of their coordinates, are
  merged. The closest point is the nearest foot point or end, chosen among the foot points
  before merging so that a merged foot cannot move it by more than the linear tolerance;
  candidates within the linear tolerance tie and the smallest parameter wins, so the result is
  within the linear tolerance of the true minimum distance where `f64` resolves that.
- Flattening takes a chord tolerance, an angle tolerance (turning per segment) and a vertex
  limit; non-positive or non-finite tolerances fail with `geom.tolerance.invalid`, rays and
  lines with `geom.curve.unbounded`, and a result above the limit with
  `geom.flatten.too_many_vertices` (a result of exactly the limit is accepted). End points and
  polyline vertices are exact. Circular arcs use the minimal number of equal segments; ellipses
  are divided greedily, each segment as long as both tolerances allow, turning at most a
  quarter turn and spanning at most a half turn of eccentric angle. When a circular or
  elliptical part needs interior vertices and the chord tolerance is below 16 units in the last
  place of the largest coordinate magnitude of its bounding box, flattening fails with
  `geom.precision.insufficient`.

### Curve intersections

`intersect(a, b, tolerance)` returns every intersection of two curves of any kind (CORE-GEOM-005),
sorted by the parameter on `a`, then on `b`; `intersect_with_limits(a, b, tolerance, limits)` is
the same with a caller's `Limits`, and `intersect` uses `Limits::DEFAULT`. A result is either a
point, with its parameter on each curve and a contact of `Crossing` or `Tangent`, or an overlap: an
ascending parameter interval on each curve and an orientation, `Same` when the starts of the
intervals meet and `Opposite` when the start on `a` meets the end on `b`. Intervals of rays and
lines may be unbounded. Parameters follow the curve parameterization above.

Each curve is split into pieces: a polyline into its straight and arc segments, every other kind
into one piece. Pieces are linear (segment, ray, line) or conic (circle, arc, ellipse, elliptical
arc). A bulged polyline segment whose sagitta (half the chord times the absolute bulge) is at most
half the linear tolerance is intersected as its chord, at the parameter of the chord fraction,
which differs from the angle fraction by a relative O(bulge²); its points are then within the
sagitta of the segment. A bulged segment whose center-radius form would round by more than a
sixteenth of the linear tolerance (a center or radius far beyond its vertices) is intersected as
the fewest chords, between points of the arc at equal angle fractions, whose sagittas are at most
half the linear tolerance, up to 64 of them; such chords follow the linear rules, so a line that
touches the segment is reported where it meets the chords. Every other bulged segment is a conic.
With `linear` and `angular` the tolerances:

- Two linear pieces are parallel when the sine of their angle is at most `angular`, and collinear
  when, in addition, each one's reference point lies within `linear` of the other's line. Collinear
  pieces overlap where their ranges share more than `linear`, touch at one tangent point where
  they meet within `linear`, and are otherwise disjoint. Pieces that are not parallel meet in at
  most one crossing point.
- A linear piece and a conic: with `h` the distance from the center to the line and `w` the
  half-width of the conic across the line (the radius of a circle), the line is tangent when
  `|h − w| ≤ linear`, crosses twice when `h < w − linear`, and misses otherwise.
- Two circles with center distance `d` and radii `r1`, `r2`, in this order: the same circle when
  `d + |r1 − r2| ≤ linear`; externally tangent when `|d − (r1 + r2)| ≤ linear`; internally
  tangent when `d > 0` and `|d − |r1 − r2|| ≤ linear`; two crossings when
  `|r1 − r2| < d < r1 + r2`; otherwise disjoint.
- Two conics, at least one not a circle: the rounder one is written implicitly and the other's
  eccentric-angle parameterization is substituted, which gives a trigonometric polynomial of
  degree 2. Its critical points, found by subdivision with Taylor bounds on the derivatives,
  split the period into monotone stretches. A critical point whose distance estimate (the
  polynomial divided by the length of its gradient) is within `linear` is a tangent point, and
  the sign changes next to it belong to that tangency; every other sign change is a crossing,
  found by bisection. The conics coincide when the distance estimate is within `linear` at every
  critical point.
- Pieces on the same circle or ellipse overlap where their angular ranges share a stretch whose
  chord exceeds `linear`, and touch at one tangent point where the shared stretch or the gap
  between them has a chord within `linear`.
- An end point of one piece within `linear` of the other piece is an intersection, tangent when
  the directions there are parallel within `angular`, unless a found point is already within
  `linear` of it.

Every rule is symmetric in the two curves: the curves are put into a fixed canonical order (by
kind, then by their defining numbers) before computing, and the other argument order returns the
same results with the curves' parameters swapped. A point within `linear` of an end point of a
piece, or of the seam of a full circle or ellipse, is reported at the parameter of that end point
or seam, and a point at the seam of a closed curve at the start of its range. Two parameters name
the same place of a curve when they are equal or lie on one piece with at most `linear` of curve
between them, also across the seam of a closed curve; a curve that passes one location twice
visits two places. Points that several pieces find at the same place of both curves within
`linear` of each other, such as at a shared polyline vertex or at the seam, are merged into one,
which keeps the parameters of one of them and is tangent when any of them is. A point inside an
overlap, or at the same place as one of its ends, on both curves is dropped. Overlaps that
continue each other at the same place of both curves are joined, whichever other results lie
between them; an overlap that runs across the seam of a closed curve is reported as two overlaps,
one ending at the end of the range and one starting at its start. Each reported point is the
midpoint of the two curve points at its parameters. `intersect(a, a)` returns the overlap of the
curve with itself, so points where a curve crosses itself are not reported.

Constructions are carried out in `f64` with an explicit error bound: the rounding error of the
inputs, relative to the size of the pieces, divided by the sine of the crossing angle (taken at
the computed point, before it is moved to an end or seam) for a crossing, plus the rounding of
the point's coordinates and half the sagittas of chords standing for bulged segments. When a
coordinate is so large that `f64` cannot resolve `linear` there, or the bound of a result exceeds
`linear`, the operation fails with `geom.precision.insufficient`. Only pieces whose bounding
boxes, one grown by `linear`, meet are computed, so a curve far from the other never causes this
error. Consequences of the bound, with the default tolerance:

- Pieces whose coordinates reach about 5.6e8 (`linear` divided by eight units of rounding) fail;
  georeferenced coordinates must be brought to a local origin first.
- Crossings at a very small angle fail when the bound divided by the sine of the angle exceeds
  `linear`: for example a circle of radius 1e6 cut 1 m inside its edge, or two ellipses with a
  minor-to-major ratio of 1e-6 lying along each other.
- Two conics that are not both circles carry the major-to-minor ratio of the rounder one in their
  bound, so ellipses with a ratio near 1e-9 fail against any conic but a circle.
- A bulged segment that needs more than 64 chords and whose center-radius form rounds by more than
  `linear` fails.

A tolerance whose `linear` is not positive and finite, or whose `angular` is negative, not finite
or above a quarter turn, fails with `geom.tolerance.invalid`. Intersections with a NURBS curve
are not implemented yet and fail with `geom.curve.kind_unsupported`. Pairs of pieces are found with a
spatial index when the curves have many pieces. More than `limits.array_length` results fail
with `geom.intersect.too_many_results`; the work before that check is bounded by four found
points or overlaps per allowed result.

### Regions and point containment

`Loop2` is a closed chain of bounded curves (segments, arcs, elliptical arcs, polylines, or a
single full circle, full ellipse or closed polyline); `Region` is one or more `RegionPart`s, each
an outer loop with holes (CORE-GEOM-021).

- A loop holds no NURBS curve yet (its area and containment are not implemented for them);
  `Loop2::new` fails with `geom.curve.kind_unsupported`.
- `Loop2::new(curves, tolerance)` accepts consecutive end and start points that differ by at most
  `linear` (the last curve joins the first) and closes each nonzero gap with a straight segment
  stored in the chain. Reversing or transforming a loop can leave gaps of a few units in the last
  place between recomputed arc ends; every algorithm treats a remaining gap as a straight joint,
  so the boundary is watertight. Deserialization accepts gaps up to the default `linear` and keeps
  the curves unchanged.
- The signed area is computed in closed form: the chord triangle of each piece with a reference
  point, plus `½·r²·(θ − sin θ)` for each circular arc of sweep `θ` (with the radius taken from
  the exact chord below a half turn) and `½·a·b·(θ − sin θ)` for each elliptical arc of eccentric
  sweep `θ`. A loop of exactly zero area is rejected with `geom.region.loop_zero_area`; when the
  loop is so small that the square of its size underflows (a circle of radius 1e-300), the zero
  says nothing and the error is `geom.precision.insufficient`, as for an area that overflows.
  Positive area is counterclockwise.
- `Loop2::reversed` reverses every curve; where the recomputed end points of two reversed arcs
  come apart by more than the default linear tolerance it inserts a segment, so the reversed loop
  is valid in its stored form, and the arc of a polyline segment becomes the arc of the reversed
  segment.
- `Region::new(parts)` turns outer loops counterclockwise and holes clockwise, and checks that
  there is a part and that each hole's bounding box lies inside its outer loop's. That loops
  neither cross nor touch and that holes lie inside their outer loops is the caller's
  precondition; building regions from arbitrary curves is CORE-GEOM-016.
- `classify(point, tolerance)` on a loop or a region returns `Inside`, `Outside` or
  `OnBoundary`. A point within `linear` of a curve, by the closest-point query, is on the
  boundary; a circular arc whose sagitta is at most `linear` is measured to its chord instead,
  which differs from it by at most the sagitta, so a nearly straight arc of huge radius (a bulge
  of 1e-10) needs no projection onto its far-away circle. Otherwise the crossings of the boundary
  with the ray from the point toward increasing x are counted, an end point at the ray's height
  counting as below it: straight pieces are decided exactly with `orient2d`; arcs and elliptical
  arcs are split at their extrema in y into monotone stretches, decided by their ends and
  located by bisection. Consecutive pieces share exact end points, so vertices and tangencies
  count once. A point in a hole is outside; a part inside another part's hole is a separate
  island.

The property tests compare the classification with a densely flattened reference for points
more than 0.01 from the boundary, at random points and at points snapped to the heights of
vertices and of extrema in y, both near the origin and with the region moved by 1e6; they
classify points sampled on every curve as on the boundary, and check invariance under rigid
transforms of region and point together. Unit tests cover loops with gap joints at the heights
of their ends, both orientations, the tolerance band on either side, magnitudes of 1e±150 and
the nearly straight arcs above.

### Polygon triangulation

`triangulate(region, chord, angle, max_vertices)` and `triangulate_with(region, tolerance,
limits)` return a `TriangleMesh2`: the flattened vertices, loop by loop in region order, and
counterclockwise triangles of `u32` indices (CORE-GEOM-022).

- Every curve is flattened with `CurveFlatten` at `chord` and `angle`; each curve after the first
  continues from the previous curve's end, and exactly equal consecutive vertices are merged. A
  segment no longer than `chord` whose neighbors are not such segments, such as a gap joint, is
  merged into the next curve, which moves the boundary by at most its length and keeps a joint
  of a few units in the last place from folding back; a chain of two or more such segments is
  kept vertex by vertex. A flattened loop with fewer than three vertices or whose shoelace area
  is zero or of the opposite sign fails with `geom.triangulate.loop_collapsed`; more than
  `max_vertices` vertices in all fail with `geom.flatten.too_many_vertices`.
- The vertices and loop edges form a constrained Delaunay triangulation (`spade`, behind the API),
  whose predicates are exact. Vertices are inserted in output order and each loop edge is added
  as soon as both of its ends are present. An edge that would cross an earlier one, equal
  vertices, and a vertex on an edge or an edge through a vertex (touching away from a shared
  vertex, overlap, folding back) fail with `geom.triangulate.edges_cross`, which also catches
  loops whose chords cross only after coarse flattening. Among the triangulations of the
  flattened vertices that contain the loop edges it maximizes the smallest angle; no vertices
  are added, so a thin triangle remains only where the flattened boundary forces it. This is the
  meaning of "where the input allows".
- Each edge is added by walking the triangles it crosses; added right after its end vertices, it
  crosses few, so typical input costs close to `O(n log n)` for `n` vertices (in an optimized
  build a 314,000-vertex circle takes under 1 s, a 20,000-vertex comb of long slanted teeth under
  0.1 s). The worst case is `O(n²)`: many long edges of later loops that each cross the
  triangles of many earlier vertices, such as 2,500 long thin holes stacked inside a loop whose
  long sides have 5,000 vertices each (20,000 vertices), take about 12 s.
- Triangles are kept by parity, flooding from the outside and toggling at every loop edge. Each
  loop edge must then have a kept triangle on its region side and none on the other; otherwise
  the call fails with `geom.triangulate.nesting_invalid`. Coordinates must be zero or of
  magnitude between about 1.8e-43 and 3.2e60, the range of the triangulation's exact arithmetic;
  others fail with `geom.precision.insufficient`.
- `TriangleMesh2::new` and deserialization reject a non-finite vertex, an index outside the
  vertices, and a triangle that is not strictly counterclockwise by `orient2d` (clockwise,
  degenerate, with a repeated index, or undecidable) with `geom.mesh.invalid`.

The property tests check that the triangle areas sum to the flattened region's area, that
triangles are counterclockwise and non-degenerate, that every loop edge borders exactly one
triangle on its region side and every other edge two triangles (so no triangle lies outside the
region or in a hole), the constrained Delaunay condition on interior edges with `incircle`,
that every flattened vertex and edge midpoint lies within `chord` of the region by projection
onto its curves, that meshes round-trip through serde, and determinism. On regions of a small
integer grid, where collinear and coincident vertices and touching loops are common,
`geom.triangulate.edges_cross` is compared with an exact pairwise test of all flattened edges.
Unit tests cover loops of short segment curves, slanted combs of 1,000 and 20,002 vertices (the
larger under a generous time bound), and hostile meshes.

### Geometry encoding

`GeomCodec` encodes and decodes every stored value (`Segment2`, `Ray2`, `Line2`, `Circle2`,
`Arc2`, `Ellipse2`, `EllipticalArc2`, `Polyline2`, `Curve2`, `Loop2`, `Region`, `Aabb2`, `Aabb3`,
`ViewFrame`, `TriangleMesh2`, `Nurbs2`) in the byte form `arc.geom` specified in [geometry-encoding.md](../architecture/geometry-encoding.md) (CORE-GEOM-032).

- A value is written in version 1 (`GeomEncoding::CURRENT`) unless it holds a NURBS curve, which
  is written in version 2 (`GeomEncoding::VERSION_2`); `encoding` names the version of a value,
  for the descriptor of the field that stores it. The decoder reads both versions.
- `encode` returns the canonical bytes and `encoded_len` their length; equal values give equal
  bytes, and −0.0 is written as +0.0. Decoding an encoding returns the value. The one value that
  encodes and does not decode is a bounding box with an infinite corner, which `from_corners`
  accepts and no stored form holds.
- `decode(bytes, limits)` reads a value that fills the input; `decode_prefix` returns the bytes
  after it. A `Curve2` has the bytes of the curve kind it holds, so `Curve2::decode` reads any
  curve and `Segment2::decode` only a segment.
- An `Arc2` built from a polyline segment is stored as a chord arc (start, end, bulge) and read
  back with its chord, so its end points stay the vertices; a plain arc is stored by center,
  radius and angles. `Arc2::decode` reads both, and they are different values.
- Values are built through their validating constructors, and input that is not the canonical
  encoding of the value it decodes to is rejected, so the byte form of a value is unique. Counts
  are checked against `Limits::array_length` and the remaining input before memory is reserved,
  the reservation is fallible, the bytes of the value are checked against `input_bytes`, and the
  nesting of values, counted from 1 in each call, against `nesting_depth`. Every failure is
  `geom.decode.invalid` with the details `offset` and `reason`.

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
| `geom.precision.insufficient` | `failed_operation` | A computation exceeds the range or precision of `f64`: a transform image that is not representable, a bulge whose arc is not representable, predicate coordinates that span too many binary exponents, an intersection whose error bound exceeds the linear tolerance |
| `geom.decode.invalid` | `corrupt_data` | Encoded geometry is malformed or oversized |
| `geom.tolerance.invalid` | `invalid_input` | A tolerance is NaN, infinite or negative, or zero where the operation needs a positive value |
| `geom.view.eye_at_target` | `invalid_input` | The eye and target of a view frame are the same point |
| `geom.view.up_invalid` | `invalid_input` | The up vector of a view frame is zero or parallel to the view direction |
| `geom.view.fov_invalid` | `invalid_input` | A perspective field of view is outside (0, π) |
| `geom.view.scale_not_positive` | `invalid_input` | An orthographic view height is zero or negative |
| `geom.view.clip_invalid` | `invalid_input` | Near is not smaller than far, or near is not positive for a perspective frame |
| `geom.view.aspect_invalid` | `invalid_input` | An aspect ratio is not a finite positive number |
| `geom.region.loop_empty` | `invalid_input` | A loop has no curves |
| `geom.region.loop_open` | `invalid_input` | The end of a curve of a loop is farther than the linear tolerance from the start of the next |
| `geom.region.loop_zero_area` | `invalid_input` | A loop encloses an area of exactly zero (not one that underflows) |
| `geom.region.empty` | `invalid_input` | A region has no parts |
| `geom.region.hole_outside` | `invalid_input` | The bounding box of a hole is not inside that of its outer loop |
| `geom.triangulate.edges_cross` | `invalid_input` | Edges of the flattened loops of a region cross, touch away from a shared vertex, or fold back |
| `geom.triangulate.loop_collapsed` | `invalid_input` | A flattened loop has fewer than three vertices, no area, or the opposite orientation |
| `geom.triangulate.nesting_invalid` | `invalid_input` | The flattened loops are not nested as the parts of the region say: a hole lies outside its outer loop or parts overlap |
| `geom.mesh.invalid` | `invalid_input` | A triangle mesh has a non-finite vertex, a triangle index outside its vertex list, or a triangle that is not strictly counterclockwise |
| `geom.length.out_of_range` | `invalid_input` | A requested arc length is negative or exceeds the curve's length by more than the linear tolerance |
| `geom.curve.unbounded` | `unsupported` | A ray or line is given to an operation that needs a bounded curve, such as flattening |
| `geom.flatten.too_many_vertices` | `limit_exceeded` | Flattening a curve needs more vertices than the caller's limit |
| `geom.intersect.too_many_results` | `limit_exceeded` | Two curves have more intersections than the caller's limit |
| `geom.nurbs.degree_invalid` | `invalid_input` | The degree of a NURBS curve is 0, above the largest supported degree, or not below the number of control points |
| `geom.nurbs.knots_invalid` | `invalid_input` | The knot vector of a NURBS curve has the wrong length, decreases, is not clamped, repeats a value too often or has an empty range |
| `geom.nurbs.weights_invalid` | `invalid_input` | The weights of a NURBS curve do not match its control points in number, or one is not positive |
| `geom.nurbs.degenerate` | `invalid_input` | All control points of a NURBS curve are equal |
| `geom.nurbs.too_many_control_points` | `limit_exceeded` | A NURBS curve has more control points than the limit allows |
| `geom.nurbs.knot_not_removable` | `failed_operation` | A knot cannot be removed as often as asked without moving the curve by more than the tolerance |
| `geom.fit.points_invalid` | `invalid_input` | The points to fit are too few for the degree, or two consecutive ones are equal |
| `geom.parameter.out_of_range` | `invalid_input` | A parameter lies outside the range an operation accepts, or is NaN |
| `geom.curve.kind_unsupported` | `unsupported` | An operation does not support the kind of a curve it was given yet |
| `geom.edit.point_off_curve` | `invalid_input` | A point given to split a curve is farther than the linear tolerance from it |
| `geom.edit.no_intersection` | `failed_operation` | The boundary curves do not meet the curve, or its extension, where a trim or extension needs them |
| `geom.edit.curve_closed` | `invalid_input` | A closed curve was given to an operation that needs an open one |
| `geom.composite.empty` | `invalid_input` | A composite curve has no curves |
| `geom.composite.not_connected` | `invalid_input` | The end of a curve of a composite is farther than the linear tolerance from the start of the next |
| `geom.offset.collapsed` | `failed_operation` | An offset of a circle or arc reaches or passes its center, or nothing of an offset remains |
| `geom.offset.degenerate` | `invalid_input` | A curve to offset has a point without a tangent direction |
| `geom.offset.join_invalid` | `invalid_input` | A miter limit is not a finite number of at least 1 |
| `geom.offset.too_many_pieces` | `limit_exceeded` | An offset needs more pieces or samples than the limit allows |
| `geom.index.too_many_points` | `limit_exceeded` | A point index is built from more points than the limit allows |
| `geom.region.self_intersecting` | `invalid_input` | A closed curve given to build a region crosses, touches or overlaps itself away from the joints of its consecutive curves |
| `geom.region.curves_intersect` | `invalid_input` | Two closed curves given to build a region cross, touch or overlap |
| `geom.arrangement.too_large` | `limit_exceeded` | Building a planar arrangement of curves needs more elements, candidate pairs, intersections, vertices or edges than the limit allows |
| `geom.trace.not_enclosed` | `failed_operation` | No closed boundary of the curves encloses the picked point |
| `geom.trace.on_boundary` | `invalid_input` | The picked point lies within the linear tolerance of a boundary curve |
| `geom.corner.radius_invalid` | `invalid_input` | A fillet radius is NaN, infinite or negative |
| `geom.corner.distance_invalid` | `invalid_input` | A chamfer distance is NaN, infinite or negative, or a chamfer angle is outside (0, π) |
| `geom.corner.pick_off_curve` | `invalid_input` | A pick point of a corner construction is farther than the linear tolerance from its curve |
| `geom.corner.pick_no_solution` | `failed_operation` | No corner solution keeps the pick points on the parts that remain, such as a pick inside the removed stretch |
| `geom.corner.radius_too_large` | `failed_operation` | No fillet circle of the radius is tangent to both supports; reached only when no candidate center exists |
| `geom.corner.no_intersection` | `failed_operation` | The supports of the two curves do not meet, so they form no corner |
| `geom.corner.parallel` | `failed_operation` | Two lines are parallel and a chamfer is requested, or a polyline vertex has collinear segments |
| `geom.corner.coincident` | `invalid_input` | Two curves of a corner construction lie on the same line or circle |
| `geom.corner.curve_too_short` | `failed_operation` | A part of a curve that a corner construction keeps would be empty or outside the curve |
| `geom.corner.unsupported_pair` | `unsupported` | A corner construction is not defined for the kinds of its curves |
| `geom.solid.limit_exceeded` | `limit_exceeded` | A solid has more vertices, faces or face-loop entries than the limit allows |
| `geom.solid.topology_invalid` | `invalid_input` | A face of a solid has a loop with fewer than three vertices, a repeated vertex, an index outside the vertex list, a hole that is not inside its outer loop, a zero area or an edge not longer than the linear tolerance, or a vertex belongs to no face |
| `geom.solid.not_planar` | `invalid_input` | A face of a solid has a vertex farther than the linear tolerance from the plane of the face |
| `geom.solid.not_closed` | `invalid_input` | An edge of a face of a solid has no matching edge in the opposite direction |
| `geom.solid.non_manifold` | `invalid_input` | An edge of a solid is used by more than two faces, or a diagonal of a face's triangulation is also an edge of another face |
| `geom.solid.orientation_inconsistent` | `invalid_input` | Two faces of a solid that share an edge traverse it in the same direction |
| `geom.solid.volume_not_positive` | `invalid_input` | The enclosed volume of a solid is not larger than the cube of the linear tolerance, because its faces point inward or it is flat |
| `geom.solid.extrusion_invalid` | `invalid_input` | An extrusion height is zero or not finite, or the target plane is not parallel to the plane of the region |
| `geom.solid.clip_degenerate` | `failed_operation` | The cutting plane meets the solid in a way that does not give a valid result, such as a section that touches itself at a vertex or a plane nearly parallel to a face it crosses |
| `geom.solid.opening_unsupported` | `unsupported` | A prism to subtract does not pass fully through two parallel faces of the solid with its whole outline inside both faces and clear of every other face, or its region has holes |
| `geom.solid.opening_splits` | `invalid_input` | A prism to subtract would leave the solid in more connected shells than it had, and `PrismTool::allow_split` is false |
| `geom.skeleton.polygon_invalid` | `invalid_input` | The polygon of a weighted straight skeleton is not simple, has fewer than 3 or more than 256 edges, an edge shorter than `tolerance.linear`, or a speed that is negative or not finite |
| `geom.skeleton.no_slope` | `invalid_input` | Every edge of a weighted straight skeleton has speed zero |
| `geom.skeleton.collinear_speeds` | `invalid_input` | Two adjacent edges of a weighted straight skeleton lie on one line with different speeds; the vertex is in `x` and `y` |
| `geom.skeleton.unclosed` | `invalid_input` | No continuous surface covers a part of the polygon: it is enclosed only by edges of speed zero, or an edge of speed zero meets a reflex corner (its vertical plane would run into the polygon); a point of it is in `x` and `y` |
| `geom.skeleton.degenerate` | `failed_operation` | The skeleton needs too many events, a vertex velocity is not finite, or the cells do not cover the polygon; a nearby point is in `x` and `y` |
| `geom.corner.segment_invalid` | `invalid_input` | A polyline segment index is outside the polyline or the two indices are equal |
| `geom.construct.coincident_points` | `invalid_input` | Two points that a construction needs distinct are closer than the linear tolerance, or a center and a start point are |
| `geom.construct.collinear_points` | `invalid_input` | The three points of a circle or of an arc through three points lie on one line |
| `geom.construct.radius_too_small` | `failed_operation` | The radius of an arc through two points is less than half the distance between them |
| `geom.construct.sides_invalid` | `invalid_input` | A regular polygon has fewer than three sides or more than `MAX_POLYGON_SIDES` |
| `geom.construct.degenerate` | `invalid_input` | A tangent construction has infinitely many solutions: coincident lines, parallel lines at distance twice the radius, or concentric circles with a common distance |

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
  k-th distance are resolved by key. The ranking scales every gap by a power of two before
  squaring, so it stays correct where the squared distance would overflow or underflow and equals
  the plain sum of squares where that is a normal number; a distance above the largest `f64` is
  reported as infinity but ranked by its true size.
- `ray(origin, direction, max_distance)` returns entries the ray meets at a distance in
  `0..=max_distance` (the direction is normalized), with the entry distance, sorted by distance
  then key. The slab test treats a zero direction component as a hit only when the origin lies in
  that slab; an origin inside a box gives distance 0; a negative `max_distance` gives no hits.
  The ray tested is the normalized one: a direction component smaller than about 1e-300 times
  the largest becomes zero or loses precision when normalized, which can decide hits that only
  graze a box face and distances measured through that component.
- An empty box is rejected with `geom.bbox.invalid` (a box built from a NaN corner is empty); a
  non-finite coordinate, point or `max_distance` with `geom.input.not_finite`; a zero ray
  direction with `geom.direction.zero`. A rejected call leaves the index unchanged.

The property tests compare every query with a brute-force scan after random sequences of insert,
remove and update, in 2D and 3D, on integer-grid boxes that force touching and exact ties. The
nearest and ray results are also compared with exact integer and rational references that share
no code with the index, on the grid scaled by powers of two from 2^-1000 to 2^1020.

### NURBS curves (CORE-GEOM-008)

`Nurbs2` is a rational B-spline curve of degree `p` with `n` control points, `n` positive weights
and a clamped knot vector of `n + p + 1` values: non-decreasing, its first and last values exactly
`p + 1` times, every interior value at most `p` times, and a nonempty range. Its parameter range
is `[u_p, u_n]`; it starts at the first control point and ends at the last. It is the `Nurbs` kind
of `Curve2`.

- `Nurbs2::new` checks the control point count against `Limits::DEFAULT.array_length` and
  `with_limits` against a caller's limits, before anything else; the degree is at most
  `Nurbs2::MAX_DEGREE` (24). Invalid input fails with `geom.nurbs.too_many_control_points`,
  `geom.nurbs.degree_invalid`, `geom.nurbs.weights_invalid`, `geom.nurbs.knots_invalid`,
  `geom.input.not_finite` or `geom.nurbs.degenerate` (all control points equal). A curve whose
  weights span more than the normal range of `f64`, or for which a bound on the first and second
  derivatives (control point extent times `2p · w_max / w_min` over the shortest nonempty knot
  span, and its square) overflows, fails with `geom.precision.insufficient`, so every accepted
  curve evaluates to finite points and derivatives.
- Points are evaluated with the de Boor algorithm on homogeneous points whose weights are divided
  by the largest weight of the span, so every step is a convex combination; the ends are the
  first and last control points exactly. First and second derivatives come from the derivative
  control points of the homogeneous points taken relative to the evaluated point, which avoids
  cancellation far from the origin. The property tests compare evaluation with the recursive
  Cox–de Boor definition, with coordinates up to 1e±100.
- `insert_knot(u, times)` and `split_at(t)` use Boehm's insertion: each new control point is a
  convex combination of two old ones and the others are kept exactly. The parameter must lie strictly inside the range
  (`geom.parameter.out_of_range`) and a multiplicity above `p` fails with
  `geom.nurbs.knots_invalid`. The two pieces of a split keep their parameters and meet exactly.
- `remove_knot(u, times, tolerance)` solves the insertion equations from both ends; the change of
  the curve from one removal is at most the inconsistency left in the middle equation, converted
  to a distance with the smallest weight and the extent of the affected control points. Removals
  are accepted while the sum of these bounds stays within `tolerance.linear`; otherwise the call
  fails with `geom.nurbs.knot_not_removable`, whose detail `removable` is the number of removals
  that would have stayed within it.
- `elevate_degree(times, tolerance)` elevates the Bézier segments with the binomial formula and
  then removes each interior knot as often as the original continuity allows, within
  `tolerance.linear`; a knot that rounding keeps from being removed stays, which changes the
  representation and not the curve.
- `reversed` negates and reverses the knots, so the range becomes `[−u_n, −u_p]`, the reversed
  point at `−t` is the point at `t` exactly, and reversing twice gives the original curve.
- `from_control_points(degree, points, weights)` uses the clamped uniform knot vector on [0, 1].
  `interpolate(points, degree, tolerance)` passes a non-rational curve through the points at
  chord-length parameters on [0, 1], with knots averaged from the parameters, by solving the
  banded system without pivoting (stable for this totally positive matrix). Every point lies
  within `tolerance.linear` of the curve at its parameter, or the fit fails with
  `geom.precision.insufficient`; fewer points than `degree + 1` or two consecutive equal points
  fail with `geom.fit.points_invalid`.
- The bounding box consists of the ends of the Bézier segments and their extrema in x and y,
  which are the roots of the numerators of the derivatives in Bernstein form. Foot points are
  the roots of `W·(A·A′) − W′·|A|²` per Bézier segment (`A` the homogeneous points relative to
  the query, `W` the weight). Roots are isolated by subdivision with the variation-diminishing
  property of the Bernstein form, with a rounding allowance per coefficient of 64 machine epsilons
  times the degree and the magnitude of the terms of that coefficient (subdivided with it), and refined by bisection; a stretch where the
  polynomial vanishes within the allowance is an interval foot point. The closest point also
  considers every distinct knot, where a corner can be nearest. More than 4096 subdivision nodes
  per degree fail with `geom.precision.insufficient`.
- Lengths are integrated per knot span with the adaptive Gauss–Legendre rule of elliptical arcs.
  Flattening subdivides each Bézier segment until its control polygon lies within half the
  chord tolerance of its chord and its legs turn by at most the angle tolerance, since the curve lies
  in the hull of its control points and its tangents in the cone of the legs; the vertex count is
  not minimal.

Through `Curve2`, NURBS curves support evaluation, length, closest point and foot points,
flattening, bounding boxes, transforms, reversal, splitting and the geometry encoding. Not yet
supported, with `geom.curve.kind_unsupported`: intersections, loops and regions, and extension.

### Split, trim, extend and join (CORE-GEOM-006)

- `split_at_parameters(curve, parameters, tolerance)` returns the pieces between the parameters
  in the curve's direction; a piece keeps the kind of its curve where the kind allows (a piece of
  a circle is an arc, of a ray a segment or ray, of a polyline a polyline with the bulges and
  widths of its parts). The first piece of a split line is the ray from the first cut backward.
  Cuts closer than `tolerance.linear` of curve length to the previous cut or to an end are
  dropped. On a closed curve the piece across the seam is one curve, except for a NURBS curve,
  where it is the two pieces on either side of the seam. A parameter outside the range or NaN
  fails with `geom.parameter.out_of_range`.
- `split_at_points` projects each point and fails with `geom.edit.point_off_curve` when a point
  is farther than `tolerance.linear` from the curve.
- `trim(curve, boundaries, pick, tolerance)` removes the stretch around the parameter `pick`
  between the nearest intersections with the boundaries on either side, or to the curve's end
  where an open curve has none on that side, and returns the remaining parts. A closed curve
  needs two distinct intersections. No intersection fails with `geom.edit.no_intersection`.
- `extend(curve, end, boundaries, tolerance)` lengthens one end of an open curve to the nearest
  intersection beyond it farther than `tolerance.linear`: a segment along its line, an arc along
  its circle, an elliptical arc along its ellipse (at most to a full turn) and a polyline by its
  end segment. Closed curves fail with `geom.edit.curve_closed`, rays and lines with
  `geom.curve.unbounded`, NURBS curves with `geom.curve.kind_unsupported`.
- `join(curves, tolerance)` grows chains greedily from the lowest-indexed unused curve, forward
  and then backward, continuing at each end with the lowest-indexed unused curve that has an end
  within `tolerance.linear`, reversed when needed. It returns each chain as a `Composite2` with
  the input indices, and for every free end the nearest other free end and their distance
  (`JoinGap`), each pair once and sorted by distance. Where more than two ends meet, the curves
  left over start chains of their own.
- `Composite2::new(curves, tolerance)` holds bounded curves whose consecutive ends are within
  `tolerance.linear` (`geom.composite.empty`, `geom.composite.not_connected`,
  `geom.curve.unbounded` otherwise); it is closed when its last end is that close to its first
  start. `to_polyline` converts a chain of segments, arcs and open polylines into one polyline.
  A composite is not a `Curve2` kind and has no stored form yet.

The property tests split curves of every kind, including bulges from 1e-300 to 1e15 and
ellipses with ratios down to 1e-12, check that every point of the curve is within the tolerance
of a piece and every piece on the curve, that consecutive pieces meet and that joining the pieces
gives one chain, closed exactly when the curve is; separated pieces are reported as a gap.

### Curve offsets (CORE-GEOM-007)

`offset(curve, distance, join, tolerance, limits)` and `offset_composite` return the curves at a
signed distance, positive to the left of the direction of travel, as a list of composites
(removing loops can leave several chains). `Line2::offset` and `Ray2::offset` give the parallel
line and ray; `offset` rejects rays and lines with `geom.curve.unbounded`.

- The curve is split into smooth elements (segments, arcs, elliptical arcs, NURBS pieces between
  knots of multiplicity `p`). A segment offsets to the parallel segment and an arc to the
  concentric arc (a polyline arc to the chord arc with the same bulge between its offset ends);
  an arc whose offset passes its center has the arc turned a half turn about the center, which is removed with the loops but cuts its neighbors, and an offset exactly at the center has none. An elliptical arc or NURBS piece offsets to a
  cubic NURBS curve interpolating `P(t) + d·N(t)` at uniformly spaced parameters, the sample
  count doubled from 16 until the fit is within a quarter of `tolerance.linear` of the exact
  offset at the midpoints and quarter points of every sample interval. A circle offsets to a
  circle.
- Where consecutive offsets do not meet within `tolerance.linear`, a join is inserted: at a
  convex corner (the offset side outside the turn) a circular arc of radius `|d|` about the
  corner, elsewhere a straight connector, which is removed with the loops.
- Loops are removed by splitting the raw chain at its intersections with itself and with the
  rest of the boundary of the region within `|d|` of the curve (the offsets of the elements by
  `−d` and the circles of radius `|d|` about the vertices and ends) (exact
  intersections between line and conic pieces; crossings of flattened polylines refined by
  Newton iteration where a NURBS piece is involved) and keeping the parts whose samples (ends and
  seven interior points) are at least `|d| − tolerance.linear` from the curve; a part whose
  samples disagree is split where the validity changes, found by bisection; parts at most twice
  `tolerance.linear` long are dropped. Kept parts are chained end to start within four times
  `tolerance.linear`.
- What remains of each round join is then shaped by the join style: kept (`JoinStyle::Round`),
  replaced by its chord (`JoinStyle::Bevel`), or by the tangent lines at its ends up to their
  intersection when that lies within `limit·|d|` of the corner and by its chord otherwise
  (`JoinStyle::Miter { limit }`, `limit ≥ 1`), or by the tangent lines at its ends up to the
  line tangent to it at its middle, at distance `|d|` from the corner, and that line between them
  (`JoinStyle::Square`; for a join of sweep `φ` the tangent lines are `|d|·tan(φ/4)` long). A
  chord lies inside the circle of radius `|d|` about the corner, which no kept part enters, so it
  crosses nothing; the lines of a miter or square join lie outside that circle and are not
  checked against other parts.
- `d = 0` returns the curve. A collapsed circle or arc, or an offset of which nothing remains,
  fails with `geom.offset.collapsed`; a NURBS point of zero derivative with
  `geom.offset.degenerate`; a miter limit below 1 or not finite with `geom.offset.join_invalid`;
  more pieces than `limits.array_length` or a fit above 65,536 samples with
  `geom.offset.too_many_pieces`.

The property tests sample every result curve of offsets of open and closed bulge polylines, of
ellipses and of random NURBS curves, at distances from 1e-3 to 20, and check with round joins
that every point lies at the offset distance from the curve within 1e-5 (miter and bevel joins:
at least the distance and at most `limit` times it), which also shows that no loop remains.

### Corner constructions (CORE-GEOM-033)

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-033 | Fillets and chamfers | Joins two curves with a fillet arc tangent to both, or a chamfer segment, between a segment, ray, line, arc or circle, a pair of polyline segments, or every vertex of a polyline; trims or extends the inputs to the joint, with a pick point per curve selecting the solution and the part kept. | T1 | 1 |

`fillet(first, first_hint, second, second_hint, radius, mode, tolerance)` returns a
`CornerResult`: the joint (`Curve2::Arc` for a fillet, `Curve2::Segment` for a chamfer, `None` for a
sharp corner) and, for each input, a `Replacement`, which is `Trimmed` with the new curve or
`Unchanged` in `CornerMode::Keep`. A caller replaces its entities atomically with these values.

- The corner point is the intersection of the two supports: the infinite line of a segment, ray or
  line, or the full circle of an arc or circle. When the supports meet at two points (two circles,
  or a line and a circle), the point nearest the midpoint of the joint's cuts is used. Supports that
  do not meet are refused with `geom.corner.no_intersection`; no fillet is built without a corner.
- A fillet candidate is a circle of the radius tangent to both supports: one for each combination of
  sides (outside or inside a circle, left or right of a line) that has a center. Its cuts are the
  tangent points. The candidate whose cuts are nearest to the pick points is chosen among those that
  keep both pick points on their remaining parts.
- Each input is cut at its joint point and at the corner point. The remaining part is the part
  beyond the joint point on the side away from the corner, extended to the corner when the input
  does not reach it, and it must contain the pick point. A pick point in the removed stretch selects
  no solution (`geom.corner.pick_no_solution`); a remaining part with no length is refused with
  `geom.corner.curve_too_short`.
- The fillet arc is the arc between the cuts whose middle faces the corner point.
- A full circle keeps the arc between its two cuts that contains the pick point. A cut that
  coincides with the corner (radius zero, or a zero distance) leaves a full circle unchanged.
- A radius of zero, or distances of zero on both curves, gives the sharp corner with no joint: each
  input is trimmed or extended to the corner point, on the side of its pick point.
- Chamfers take distances `d1` and `d2` from the corner along the first and the second curve, or a
  distance `d` along the first curve and an angle between the chamfer and that curve, measured
  inside the corner; the second distance is then determined. The chamfer ends are on the sides of
  the pick points. Chamfers are defined for segments, rays and lines only; a circular curve is
  refused with `geom.corner.unsupported_pair`.
- Parallel, distinct lines (segments, rays or lines, including anti-parallel directions) receive a
  semicircular end in `fillet`. The joint is a semicircle whose diameter joins the end of the first
  curve nearest its pick point to the foot of that end on the second line; its radius is half the
  distance between the lines. The radius argument is ignored for this case, including zero, but a
  non-finite or negative radius is still refused. The first curve keeps its geometry (a segment
  keeps its length; a ray keeps its origin and direction). The second curve is trimmed or extended
  to the foot, keeping the part in the body direction of the first curve, which must contain its
  pick point. The semicircle bulges away from the kept parts. A line as the first curve has no end
  and is refused with `geom.corner.unsupported_pair`. Coincident lines are refused with
  `geom.corner.coincident`. Chamfers of parallel lines are refused with `geom.corner.parallel`, as
  are collinear adjacent segments at a polyline vertex.
- `fillet_polyline_segments` and `chamfer_polyline_segments` apply the pair constructions to two
  segments of one polyline and return each segment's replacement; the caller rebuilds the polyline.
- `fillet_polyline_vertices` and `chamfer_polyline_vertices` apply one construction at every
  corner vertex (every vertex of a closed polyline, the inner vertices of an open one) and return a
  `CornerPolyline`: the rebuilt polyline and the list of `SkippedVertex { index, reason }`.
  A vertex with a bulged neighbour is left unchanged (`SkipReason::BulgedSegment`), and so is a
  vertex whose segments are too short for the radius or distance, or whose cut would leave a
  neighbouring segment with no part (`SkipReason::TooShort`). Corners are applied in vertex order,
  and each is applied only if both segments it touches keep a part. Bulges of untouched segments are
  kept. A radius or distance of zero returns the polyline with no vertices skipped. Two collinear
  adjacent segments refuse the whole call with `geom.corner.coincident` (same line) or
  `geom.corner.parallel`. Polylines with any width are refused with `geom.corner.unsupported_pair`:
  widths are not carried through the cuts. Fillet arcs are stored as bulges.
- Tolerances come from `Tolerance::linear` for distances and `Tolerance::angular` for the parallel
  test. Every algorithm is closed-form: no iteration is bounded by input size.

Not covered: ellipses, elliptical arcs and NURBS curves (`geom.corner.unsupported_pair`), chamfers of
arcs, corners of supports that do not meet (`geom.corner.no_intersection`), and polyline widths.

### Analytic constructions (CORE-GEOM-034)

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-034 | Analytic constructions | Builds circles through two or three points, arcs through three points, from a start point, center and sweep, or from two ends and a signed radius, circles tangent to two lines or circles with all solutions, regular polygons inscribed in, circumscribed about or built on an edge, and ellipses from an axis and the other semi-axis, as exact primitives. | T1 | 1 |

The functions of the module `construct` take points, curves and lengths and return `Circle2`,
`Arc2`, `Ellipse2`, `TangentCircle` values or vertex lists. Each checks its inputs before it builds
anything, and none changes a document. Distances are compared with `Tolerance::linear`, and the
tangent tests compare directions with `Tolerance::angular`.

- `circle_through_points(a, b, c, tolerance)` is the circumcircle of three points. Two points closer
  than the linear tolerance are `geom.construct.coincident_points`; a third point within the
  tolerance of the line through the other two is `geom.construct.collinear_points`.
- `circle_on_diameter(a, b, tolerance)` has the midpoint of the segment as its center and half the
  length as its radius.
- `arc_through_points(start, middle, end, tolerance)` is the arc of the circumcircle from `start`
  to `end` that passes through `middle`. The sweep is counterclockwise when `middle` is reached
  first going counterclockwise from `start`, and clockwise otherwise.
- `arc_from_start_center(start, center, sweep, tolerance)` takes the radius from the distance of
  `start` from `center`. A positive sweep runs counterclockwise. The sweep rules of `Arc2` apply:
  zero or a magnitude above a full turn is `geom.sweep.invalid`.
- `arc_from_ends_and_radius(start, end, radius, tolerance)` returns the minor arc (sweep of
  magnitude at most π). A positive radius puts the center on the left of the direction from `start`
  to `end`, so the arc runs counterclockwise; a negative radius puts it on the right, and the arc
  runs clockwise. A zero radius is `geom.radius.not_positive`; a magnitude below half the chord is
  `geom.construct.radius_too_small`. A chord of exactly twice the radius gives a semicircle.
- `tangent_circles(first, second, radius, tolerance)` returns every circle of the radius tangent to
  both curves, as `TangentCircle { circle, first, second }` where `first` and `second` are the
  tangent points. Each input is a `Curve2::Line` (infinite) or a `Curve2::Circle`; other kinds are
  `geom.curve.kind_unsupported`. A line gives two offset lines, one on each side at the radius. A
  circle of radius `R` gives the distances `R + radius` (outside) and `|R - radius|` (inside); the
  inside distance is omitted when it is within the tolerance, because the center would then be the
  circle's own center and the solution would coincide with the circle. Solutions are the
  intersections of the loci of the two inputs, deduplicated within the linear tolerance and sorted
  by the center's x and then y coordinate. No solution is an empty list, not an error.
- The families of solutions are refused with `geom.construct.degenerate`: two lines that coincide
  (their offset lines coincide), two parallel distinct lines at distance `2 · radius`, and two
  concentric circles with equal distances from the common center.
- `polygon_inscribed(circle, sides, start_angle)` returns the vertices counterclockwise, the first at
  `start_angle`, on the circle. `polygon_circumscribed(circle, sides, start_angle)` returns the
  vertices at the same angles at distance `radius / cos(π / sides)`, so that every edge is tangent
  to the circle at its midpoint. Fewer than three sides, or more than `MAX_POLYGON_SIDES` (65 536),
  is `geom.construct.sides_invalid`.
- `polygon_from_edge(first, second, sides, side)` returns a regular polygon with the segment from
  `first` to `second` as its first edge. The polygon lies on the chosen `EdgeSide`: its vertices run
  counterclockwise for `Left` and clockwise for `Right`, starting with `first` and then `second`.
  Equal points are `geom.segment.zero_length`.
- `ellipse_from_axis(first, second, other)` takes the segment as one axis, centered at its midpoint.
  When `other` is at most half the length of the segment, the segment is the major axis and the
  ratio is `other / half_length`; otherwise the segment is the minor axis, the major axis is
  perpendicular to it with length `other`, and the ratio is `half_length / other`. A length equal to
  the half length gives a circle. A non-positive `other` is `geom.ellipse.invalid_axis`.

Each construction is closed form; the work is bounded by the number of inputs, except the number of
sides, which is bounded by `MAX_POLYGON_SIDES`.

### Point k-d tree (CORE-GEOM-028)

`KdTree2<K>` and `KdTree3<K>` index keyed points (`K: Copy + Ord`, keys need not be distinct) in
one array arranged as an implicit balanced tree, built once in `O(n log n)` time without
recursion; at most `limits.array_length` points (`geom.index.too_many_points`), all finite
(`geom.input.not_finite`).

- `nearest(query, k)` returns the `k` nearest points and `within(query, radius)` every point in
  the closed ball, each with its distance, sorted by distance, then key, then input order.
- Distances are ranked with the extended exponent range of the R*-tree, so the ranking is exact
  where squares overflow or underflow. A subtree is skipped only when the distance to its
  splitting planes is strictly larger than the distance to beat, so ties are always seen and the
  results equal a brute-force scan. A query costs `O(log n)` for well-spread points and at most
  `O(n)`. A negative radius finds nothing; a non-finite query or radius fails with
  `geom.input.not_finite`.

The property tests compare both queries with a brute-force scan on integer-grid points with many
exact ties, in 2D scaled by powers of two from 2^-1000 to 2^1000 and in 3D, and on random points.

### Planar arrangements

Region construction, boundary tracing, region booleans and region offsets share one planar
arrangement of exact curves (crate-private).

- The curves are split into smooth elements: segments, circular arcs (a polyline arc keeps its
  chord form) and elliptical arcs; a circle or ellipse is its full arc and a polyline its
  segments. Rays and lines fail with `geom.curve.unbounded` and NURBS curves, whose
  intersections are not implemented, with `geom.curve.kind_unsupported`, each with the detail
  `curve`.
- Candidate pairs are the pairs of elements whose bounding boxes, expanded by
  `tolerance.linear`, meet in an R*-tree; each pair is intersected with `intersect`. The work is
  near-linear in the number of elements plus candidate pairs. More than `limits.array_length`
  elements, candidate pairs, intersections, cut points or edges fail with
  `geom.arrangement.too_large`.
- Each element is cut at its intersection parameters (both ends of an overlap) and a closed
  element also at its thirds. The cut points, element ends first, are snapped: points within
  twice `tolerance.linear` are one vertex, represented by its first point (so input vertices stay
  exact). A sub-edge whose ends snap together is dropped when it is at most four times
  `tolerance.linear` long and split at its middle otherwise. A segment is rebuilt between its end
  vertices and a circular arc as the arc with the same sweep on the chord between them (moving
  it by at most the snap distance); an elliptical arc keeps its own ends, and a loop built from
  it closes the remaining gap with a segment. Sub-edges with the same end vertices whose middles
  lie within twice `tolerance.linear` of each other are one edge, with the geometry of the first
  in input order and the sum of their winding weights.
- Around each vertex the half-edges are ordered by the angle of their tangents measured from a
  direction 1 radian above the x axis (so that axis-aligned tangents do not sit on the cut of
  the angle range) and, where the angles differ by at most `max(tolerance.angular, 1e-12)`, by
  signed curvature: of two curves leaving with the same tangent, the one turning more to the
  left lies counterclockwise. The next half-edge around a face is the first one clockwise from
  the reverse of the current one at its end, so every cycle has its face on the left. In each
  connected component the cycle of least signed area is the component's outer boundary; every
  other cycle bounds a face.
- Coordinates whose grid cell (in units of twice the linear tolerance) exceeds 2^52 fail with
  `geom.precision.insufficient`, as does a set of half-edges that does not close into cycles.

### Region construction (CORE-GEOM-016)

`build_region(curves, tolerance, limits)` builds the region bounded by closed composites; a
single closed curve (circle, ellipse, closed polyline) is a composite of one curve.

- Every composite must end within `tolerance.linear` of its start: otherwise
  `geom.region.loop_open` with the detail `curves` listing all open ones (such as `"1,2"`). A
  NURBS curve fails with `geom.curve.kind_unsupported` and the detail `curves`. No composites
  is `geom.region.empty`.
- The elements of all composites are intersected pairwise as in the arrangement. Two
  consecutive elements of one composite may meet only within twice `tolerance.linear` of their
  shared joint; any other point, and any overlap, within one composite fails with
  `geom.region.self_intersecting`. Any contact between two composites (crossing, touching from
  either side, or overlapping) fails with `geom.region.curves_intersect`. Both carry the detail
  `curves` with every offending composite. Nothing is repaired or guessed.
- Each composite becomes a loop (`Loop2::new`; a loop of zero area fails with
  `geom.region.loop_zero_area` and the detail `curve`). The containment depth of a loop is the
  number of other loops that contain it: candidates come from an R*-tree of the loop boxes, and
  each is decided by classifying points of the loop (the middle of each curve, then each start)
  until one is not on the candidate's boundary. Loops of even depth are outer loops; a loop of
  odd depth is a hole of the containing loop of least area. `Region::new` turns outer loops
  counterclockwise and holes clockwise, whatever the input orientation.

The property tests build groups of concentric circles and squares of random orientation at
scales from 1e-100 to 1e100, compare region membership at random points with an even-odd test
on the densely flattened input, check the loop count and orientation, and add a circle crossing
a random curve, which must be reported together with that curve. Unit tests cover open,
self-intersecting (crossing and pinched), touching, NURBS and empty input and the limit.

### Boundary tracing (CORE-GEOM-017)

`trace_region(curves, pick, islands, tolerance, limits)` returns the region around `pick`
bounded by arbitrary bounded curves (open or closed, crossing each other), for hatching and area
tools.

- The arrangement of all curves is built with zero winding weights. The traced face is the
  bounded face whose boundary cycle has the least area among those containing `pick`.
  Dangling ends and curves joining two otherwise separate groups have the same face on both
  sides and never bound the result. Gaps are not bridged: an end that misses another curve by
  more than twice `tolerance.linear` leaves the boundary open.
- An island is a connected group of curves inside the traced face that does not touch its
  outer boundary. The face that contains each such group is the bounded face of least area,
  among the other groups, containing one of its vertices. The depth of a face is the length of
  the shortest path of faces from the traced face, crossing one curve per step, without leaving
  the traced face's outer boundary. `IslandMode::Outermost` fills depth 0 only (the face itself:
  islands are holes), `IslandMode::Alternating` every even depth (islands are holes, faces inside
  them two curves deep are filled again, and so on), and `IslandMode::Ignore` every depth (the
  whole area inside the outer boundary). The boundary of the filled faces against the unfilled
  ones becomes the region, so adjacent filled faces merge.
- A non-finite pick fails with `geom.input.not_finite`; a pick within `tolerance.linear` of a
  curve bounding a face that contains it with `geom.trace.on_boundary`; a pick outside every
  closed boundary with `geom.trace.not_enclosed`.

The property tests trace random arrangements of two to five segments and arcs, half of them
inside a closing box, from a random pick: the result contains the pick, every sampled boundary
point lies within 1e-5 of an input curve, and the result agrees with a flood fill of a 48 × 48
raster whose walls are the cells within 0.71 cells of a curve, at every cell at least two cells
from a curve; a box always encloses the pick. Arrangements with near misses the raster cannot
resolve (curves that do not meet, or free ends and curves they do not reach, within three cells)
are skipped. Unit tests cover crossing and overshooting lines, a dangling segment, an open gap
of 1e-3, a circle cut by a chord, nested squares and a double-circle island in all three modes,
and hostile input.

### Region booleans (CORE-GEOM-018)

`boolean(a, b, op, tolerance, limits)` returns `a op b` for `BooleanOp::Union`, `Difference`,
`Intersection` and `ExclusiveOr`, or `None` when the result is empty.

- All loops of both regions go into one arrangement; each curve carries the winding weight 1
  for its region in its direction (outer loops counterclockwise, holes clockwise, so a region's
  winding number is 1 inside and 0 outside). The winding numbers of the faces of a component
  follow from the weights of the edges crossed from its outer face; the winding numbers of the
  outer face are the sums, over the loops of the other components, of ±1 for each loop (by its
  orientation) that contains a vertex of the component, the first vertex not on that loop's
  boundary deciding. A face is in the result when the operation holds for its two memberships;
  the edges between a member face and a non-member face, oriented with the member on their
  left, are walked into cycles. Cycles of positive area are outer loops; each other cycle is a
  hole of the outer loop of least area that contains it.
- No curve is flattened: every output edge is a piece of an input segment, circular arc or
  elliptical arc, so an output edge on an input arc is an arc with its radius (rebuilt on the
  chord between its snapped end vertices, so within the snap distance) and an output edge on an
  input elliptical arc is a piece of that elliptical arc.
- Coincidence is resolved in the arrangement: overlapping curves of the two regions are cut at
  the ends of the overlap, and their pieces, which share their end vertices and lie within twice
  `tolerance.linear` of each other, become one edge with the geometry of `a`'s curve (the first
  in input order) and the sum of the weights. Shared edges, touching vertices and coincident or
  tangent arcs are therefore decided once for both regions.
- The result loops neither cross nor overlap; loops may touch at isolated vertices (two squares
  sharing only a corner give two parts that touch there), where the walk separates them.

The property tests check `|A ∪ B| = |A| + |B| − |A ∩ B|`, `|A △ B| = |A ∪ B| − |A ∩ B|` and
`|A − B| = |A| − |A ∩ B|` within 1e-9 relative, and point membership at random points farther
than 1e-3 from both boundaries, for random regions with line, arc and elliptical boundaries
moved and rotated against each other, and for unions of rectangles and circles on a small
integer grid (shared edges, vertices and tangencies) scaled from 1e-100 to 1e100 together with
the tolerance. Unit tests check that the union of two overlapping circles has only arc edges
with the input radii, shared edges, corner contact, identical regions, holes and islands,
internal and external tangency, disjoint regions and hostile input.

### Region offsets (CORE-GEOM-019)

`offset_region(region, distance, join, tolerance, limits)` offsets a region outward for a
positive distance and inward for a negative one, or returns `None` when nothing remains.

- Every loop is offset with `offset_composite` by `−distance`: to its right, which is outside
  the region for counterclockwise outer loops and clockwise holes. The join styles are those of
  curve offsets: `JoinStyle::Round`, `Miter { limit }`, `Bevel` and `Square`. A loop whose offset
  collapses (`geom.offset.collapsed`) contributes nothing.
- Offset chains close within four times `tolerance.linear` (the chaining gap of curve
  offsets); a chain whose box diagonal is at most eight times `tolerance.linear` is a sliver and
  is dropped. Where the loop removal of a curve offset leaves a closed loop as open chains whose
  ends overshoot their crossing (the tolerance band along a shallow crossing), each chain is
  followed by the chain whose start is nearest to its end, at most `|distance|` away (otherwise
  `geom.precision.insufficient`), and the joint between them makes a small reversed lobe.
- The loops are combined in an arrangement by winding number, keeping the points with a
  positive winding number: offsets of different loops that meet merge, a grown hole cuts away
  what it overlaps, reversed lobes vanish, and loops that vanish are removed. An inward offset
  larger than the inradius gives `None`, not an error. `distance = 0` returns the region.
- Elliptical arcs offset to NURBS curves, which loops do not hold yet, so a region with an
  elliptical boundary fails with `geom.curve.kind_unsupported`.

The property tests offset star-shaped bulge polygons, with or without a circular hole, scaled
from 1e-50 to 1e50 with the tolerance, by three sorted distances from −2.5 to 2.5 times the
scale with round joins: every sampled point of every result curve lies at the offset distance
from the source boundary within 1e-5 times the scale, and the area does not decrease as the
distance grows. Unit tests check a square with every join style against closed-form areas,
inward offsets past the inradius, merging squares, holes that close, grown holes that cut the
outer loop, an annulus that vanishes, and rejected input.

### Region measurements (CORE-GEOM-020)

`Loop2::measure(tolerance)` and `Region::measure(tolerance)` return `Measurements`: the area,
the perimeter (holes included), the centroid, and the second moments `SecondMoments { xx, yy,
xy }` (`∫x² dA`, `∫y² dA`, `∫x·y dA`) about axes through the centroid parallel to the x and y
axes. `Measurements::about_axes(origin, x_axis)` gives the second moments in any frame by the
parallel-axis rule and rotation (`geom.direction.zero` for a zero axis). A loop is measured
whatever its orientation.

- The integrals follow from Green's theorem as sums over the boundary pieces, relative to the
  center of the bounding box. A piece from `a` to `b` contributes the triangle of the reference
  point with `a` and `b` (closed forms: area `½·a×b`, first moment `A·(a + b)/3`, second moment
  `A/12·(2·a aᵀ + a bᵀ + b aᵀ + 2·b bᵀ)`) and, for an arc, the segment between its chord and
  the arc.
- A circular arc is the image of an arc of the unit circle under `r·I`, an elliptical arc under
  the linear map `[major, minor]` of its eccentric angle; the segment's moments are those of the
  unit-circle segment of the same half-sweep `β`, mapped linearly (area by the determinant) and
  placed at the middle of the chord. The unit segment, in the frame of its chord, has `∫u` and
  `∫uv` zero and `a₀ = β − sin β cos β`, `∫v`, `∫u²`, `∫v²` in closed form as the sector minus
  the triangle with the center, moved to the chord, for `β ≥ 1`. Below `β = 1` these closed forms
  cancel (a bulge of 1e-10 loses every digit), and the same integrals are evaluated as boundary
  integrals over the arc with `v = 2·sin((β + t)/2)·sin((β − t)/2)`, which does not cancel, by
  8-point Gauss–Legendre quadrature on panels of at most ¼ radian; the integrands are
  trigonometric polynomials of degree at most 4, so the quadrature error is below 1e-20
  relative, far below the rounding of `f64`. A polyline arc takes its radius and bisector from
  its exact chord below a half turn; a plain arc uses its exact radius and angles.
- The perimeter is the sum of the curve lengths: exact for segments and circular arcs, by the
  adaptive quadrature of `CurveEval::length` within `tolerance.linear` for elliptical arcs.
- Results that are not finite (second moments overflow above about 1e75 times the unit) fail
  with `geom.precision.insufficient`.

The property tests compare area, centroid and second moments of random regions with line, arc
and elliptical boundaries with the moments of their flattening (chord 1e-4, turning 0.002) within
1e-5 relative, and check that scaling by 10^±70 scales area, perimeter and second moments by
`s²`, `s` and `s⁴`. Unit tests compare rectangles, disks, an annulus (also about a tangent line),
a rotated ellipse, circular segments of sweeps from 1e-6 to 6, a rounded rectangle and nearly
straight polyline arcs with closed forms, and the quadrature with the closed forms where both are
accurate.

### Planes and polyhedral solids (parts of CORE-GEOM-010 and CORE-GEOM-012)

Implemented: the plane case of CORE-GEOM-012 and the solids the planar generators of the two-tier
geometry decision (ADR-0003) cover. Not implemented: 3D curves (CORE-GEOM-010 beyond the plane
intersections below), the curved analytic surfaces and NURBS surfaces, and the exact solid tier.
The details are in the module documentation of `plane3`, `solid`, `solid_gen`, `solid_clip` and
`solid_opening`; this section states the contract.

- **Planes.** `Plane3` (origin and unit normal), `side`, `signed_distance`, `project`,
  `intersect_line`, `intersect_segment` and `intersect_plane` take `Tolerance`; parallel, contained
  and coincident cases are results, not errors. `PlaneFrame` maps points to plane coordinates and
  back; `Plane3::frame` uses a horizontal first axis and an upward second axis (the world x and y
  axes for a horizontal plane), `frame_with_x` takes the first axis from a hint.
- **`Polyhedron`.** Vertices and faces; a `PolyFace` is an outer loop (counterclockwise seen from
  outside) and hole loops (clockwise) of vertex indices lying in one plane, with a `FaceTag`.
  `Polyhedron::new` validates counts against `Limits::array_length`, finite coordinates, loop shape,
  planarity within `PLANARITY_FACTOR` times `tolerance.linear`, edges used once in each direction
  (`geom.solid.not_closed`, `geom.solid.non_manifold`, `geom.solid.orientation_inconsistent`),
  positive volume, and that every face triangulates: the exact edge tests of the triangulation find
  no edge that crosses, touches or folds back, so no operation returns a solid that cannot be
  meshed. The triangulation is kept; triangles and quadrilaterals cost constant time, other faces
  `O(n log n)` in their vertices. Coordinates too large for the arithmetic give
  `geom.precision.insufficient`. Volume, area, centroid and bounding box are exact sums over the
  faces; `transformed` accepts any invertible `Transform3` and reverses loops for a mirror. Not
  checked: faces crossing each other, distinct coincident vertices, connectedness and
  manifoldness at vertices. The specification does not name this type; it is the smallest
  representation that keeps walls, floors and roofs exact.
- **Face tags.** `FaceTag` is a plain enum (`Untagged`, `Side { loop_index, curve, facet }`, `Top`,
  `Bottom`, `OpeningWall { opening, curve, facet }`, `Cap { cut }`) carried by every operation:
  `extrude` and `prism` set `Side`, `Top` and `Bottom`; `subtract_prism` sets `OpeningWall` with the
  caller's opening number; `clip` sets `Cap` on new faces with the cut number the caller passes to
  it, so the caps of a plan cut and a section cut can be told apart after both; every other output
  face keeps the tag of its source face, and a face that is split yields several faces with the
  same tag. `face_tag(i)` reads it. A side face is named by the input it comes from, not by the
  faceting: loops are numbered from 0 in the order of the region (the outer loop of each part,
  then its holes in the order given to `RegionPart::new`, parts in order); `curve` is the index of
  the curve in the loop as the region stores it (outer loops counterclockwise, holes clockwise);
  tags follow the region as stored, not the caller's drawing direction, so a clockwise outline
  that `Region::new` reversed has curve 0 = the reverse of the last curve drawn, and a saved and
  loaded or equal region numbers identically. A mirrored `Region::transformed` is a different
  region and is renumbered as it stores it. `facet` is the index of the chord within a curved
  edge, counted in the stored direction of the curve, and 0 for a straight edge.
  The face of a straight edge therefore keeps its tag when the chord or the radius of a neighboring
  arc changes. A segment of at most `tolerance.linear` between two
  curves (a joint that closed a gap) is not an edge and is not counted. `OpeningWall` numbers the
  curves of the profile's outer loop the same way. `Polyhedron::transformed` keeps every tag,
  including under a mirror (the loops are reversed so the normals still point outward).
- **Generators.** `extrude` (a `Region` in a `PlaneFrame`, along the frame normal, by a signed
  height), `extrude_between` (to a parallel plane) and `prism` (a footprint in the x-y plane from a
  base elevation). Curved edges become chords whose end points lie on the curve, at the caller's
  chord tolerance with no angle limit. The solid then differs from the exact one by at most chord
  times the length of the curved edges times the height: it is smaller along a convex arc (a
  column) and larger along a concave one (a round hole, whose facets are inside the circle). The
  chord applies to curved edges only: a straight edge is kept exactly whatever its length, so a
  jog or a notch shorter than the chord stays in the solid. The region is also run through the
  exact triangulation checks, so crossing loops are refused.
- **Clipping.** `Polyhedron::clip` keeps one side of a plane and caps the cut with faces built from
  the section loops. A vertex within `tolerance.linear` of the plane is on it. The cut is made as
  if the plane were moved infinitesimally against its normal for both sides: `Positive` keeps the
  vertices on the plane, `Negative` drops them, so the two results are exact complements and their
  volumes sum to the solid's. Edges in the plane (the head, sill or jamb of an opening, the edge
  of a step) are handled by cancelling edges that meet their reverse, so zero-width bridges vanish
  and faces and caps split into separate faces. `section` returns the cap as a `Region` in the
  plane's frame: where the solid has material on both sides of the plane. Refused with
  `geom.solid.clip_degenerate`: a hole whose tip touches the cut line at one vertex, a plane
  almost parallel to a crossed face, crossing points that cannot be paired, a result that fails
  validation, and a cut that leaves two parts meeting along an edge that lies inside the cap (the
  `Negative` side of a plane through the apex of a V-shaped notch). A result may have several
  shells (`Polyhedron::shell_count`) when the parts do not meet. The cases and results are
  tabulated in the `solid_clip` module documentation.
- **Triangles.** `Polyhedron::triangulate_face` returns the triangles of a face (indices of the
  solid's vertices, no vertices added); `arc_mesh::TriMesh::from_polyhedron` builds the flat-shaded
  mesh with one face group per face (the group's local key is the face index), so a picked
  triangle names its face and, through `face_tag`, its role.
- **Openings.** `Polyhedron::subtract_prism` subtracts a prism (a `PrismTool` with its opening
  number) that runs between two parallel faces. The outline may lie inside the faces or touch or
  cross their boundary (a door at floor level, a full-height opening, an opening at a wall end),
  in which case the faces parallel to the axis that it meets are cut and the walls trimmed to the
  solid, and an outline may contain a corner of a face. A vertex of the outline within
  `tolerance.linear` of the plane of a face parallel to the axis (a floor, a ceiling, a wall end,
  the jamb of another opening) lies on it; a strip beyond the tolerance stays as a strip. An
  opening that leaves the solid in more shells than it had (a profile used with the axis along the
  wall, or a full-length opening) fails with `geom.solid.opening_splits` unless
  `PrismTool::allow_split` is set. Anything else fails with `geom.solid.opening_unsupported`; the
  reason is a detail. The
  work for an opening is linear in the face loop entries (bounding boxes prefilter every face
  test), plus the validation; openings are applied one after the other. General booleans are not
  provided.

The tests check that every generated, clipped and perforated solid validates and meshes, that
volume and area survive rigid transforms, that the two clipped parts sum to the original volume,
and that an extruded volume is area times height, with exact cases (unit cube, an L-shaped prism,
a prism with a hole, cuts through vertices, edges and faces, the head, sill and jambs of the
openings of a wall at 5 x 3 m, a plane in the step of an L-shaped solid, cuts within a tolerance of
a vertex, and a slanted wall 500 km from the origin carried through extrusion, opening and cut).

### Weighted straight skeleton (CORE-GEOM-035)

| ID | Capability | Description | Tier | Phase |
|---|---|---|---|---|
| CORE-GEOM-035 | Weighted straight skeleton | Computes the surface that rises from every edge of a simple polygon at its own rate (a roof over its outline), with gable edges that do not move, as convex planar cells per edge and the ridge, hip and valley lines; degenerate input is refused with a located error. | T1 | 2 |

`weighted_skeleton(polygon, speeds, tolerance)` takes a simple polygon (either orientation; edge
`i` runs from vertex `i` to the next) and, per edge, the horizontal distance it moves inward per
unit of height. Speed zero is an edge that does not move: its plane is vertical. The result,
`Skeleton`, holds convex `SkeletonCell`s, each in the plane of one edge (counterclockwise seen
from above, vertical for an edge of speed zero), which together cover the polygon once; the
`SkeletonArc`s, each a `Ridge` (level), `Hip` (from a convex vertex) or `Valley` (from a reflex
vertex) between the planes of two edges; the height of the top; `edge_height(edge, point)`, the
height of an edge's plane over a point; and `height_at(point, tolerance)`, the height of the
surface over a point of the polygon.

- The front of moving edges is simulated from event to event (an edge shrinking to no length, a
  reflex vertex reaching another edge); at each event height the front is cleaned up within
  `tolerance.linear` (meeting vertices merge, a vertex on an edge splits its polygon, edges folded
  onto each other end in a ridge), so several events at one height are one step. The work is
  `O(n²)` per event and there are at most `8n + 16` events.
- Adjacent collinear edges of one speed are merged (the second edge gets no cell); with different
  speeds they are refused. The plan areas of the cells must add up to the polygon's area within
  `tolerance.linear` times the length of the cell boundaries, else `geom.skeleton.degenerate`:
  a wrong surface is never returned.

The tests check a hip roof over a rectangle (ridge length and height), vertical gable cells, the
valley of an L shape, a shed roof, unequal speeds, every refusal, and as property tests over
random convex and star-shaped polygons with random speeds (some zero) that every cell corner lies
in its edge's plane, that the cells cover the polygon and that sloped cells agree on the height at
shared corners.

