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
  merged. The closest point is the nearest foot point or end; candidates within the linear
  tolerance tie and the smallest parameter wins.
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
or above a quarter turn, fails with `geom.tolerance.invalid`. Pairs of pieces are found with a
spatial index when the curves have many pieces. More than `limits.array_length` results fail
with `geom.intersect.too_many_results`; the work before that check is bounded by four found
points or overlaps per allowed result.

### Regions and point containment

`Loop2` is a closed chain of bounded curves (segments, arcs, elliptical arcs, polylines, or a
single full circle, full ellipse or closed polyline); `Region` is one or more `RegionPart`s, each
an outer loop with holes (CORE-GEOM-021).

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
`ViewFrame`, `TriangleMesh2`) in the byte form `GeomEncoding::CURRENT` (`arc.geom`, version 1)
specified in [geometry-encoding.md](../architecture/geometry-encoding.md) (CORE-GEOM-032).

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
