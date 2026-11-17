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
