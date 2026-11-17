# Geometry

The geometry stack shared by all products: the pure-Rust tier (`arc-geom`, `arc-mesh`), the exact
B-rep tier (`arc-brep`) with its isolated kernel binding, the constraint solver and the expression
engine (`arc-solve`).

## Two tiers

| Tier | Crates | Implementation | Linked by |
|---|---|---|---|
| Owned geometry | `arc-geom`, `arc-mesh` | Pure Rust, owned by the project, WebAssembly-capable | Everything that needs geometry |
| Exact solids | `arc-brep` | Arc-owned API and types; first implementation delegates robust operations to the Open CASCADE kernel through `arc-brep-occt` | Only crates and products that need exact solids |

The B-rep tier never leaks kernel types: products see `arc-brep` handles, Arc-owned names and
Arc-owned errors. Operations move from the kernel to native Rust one family at a time when a native
implementation passes the same regression corpus. Products that only need polyhedral solids
(extrusions of profiles, mesh-based massing, review clash detection) use `arc-mesh` and never link
the kernel.

## Tolerance and robustness model

- Model coordinates are `f64`. Lengths are metres and angles radians in BIM, civil, solid and mesh
  documents; drafting spaces declare their unit (see [drafting](drafting-and-views.md)).
- Tolerances are values of `arc_base::Tolerance`, never literals:

| Tolerance | Default | Meaning |
|---|---|---|
| `linear` | 1e-6 m (1 µm) | Two points closer than this are coincident |
| `angular` | 1e-9 rad | Two directions closer than this are parallel |
| `parametric` | Derived per curve from `linear` and the parameterization speed | Parameter equality on a curve or surface |
| `tessellation_chord` | Supplied by the caller (view-dependent or export setting) | Maximum distance between a curve and its polyline |
| `tessellation_angle` | Supplied by the caller | Maximum turning angle per segment |

- When the caller supplies no tessellation values, the `arc-base` defaults apply
  ([CORE-BASE-012](../crates/arc-base.md)).
- A document stores its model tolerance in metadata; operations take a tolerance context so results
  are reproducible.
- Orientation, incidence and in-circle decisions use robust adaptive-precision predicates
  (`robust`) on exactly prescaled coordinates; inputs whose binary exponents span more than the
  exact computation can hold report `geom.precision.insufficient` (see
  [arc-geom](../crates/arc-geom.md#robust-predicates)). Constructions (intersection points) use
  `f64` with explicit error bounds; a construction whose error bound exceeds `linear` reports
  `geom.precision.insufficient` instead of returning a wrong topology.
- Large coordinates (georeferenced projects) are handled by keeping a document local origin and
  working in local coordinates; algorithms never subtract large absolute coordinates directly.
- Every algorithm states its degenerate-input behavior: zero-length segments, coincident points,
  collinear and overlapping curves, tangencies, self-intersections. Degenerate inputs either have a
  defined result or a specific error; they never produce a silently wrong result.

## Curves, surfaces and regions

`arc-geom` owns:

- **2D and 3D curves:** line segments, rays and lines, circular arcs and circles (exact, never
  approximated by splines), ellipses and elliptical arcs, polylines with bulges, NURBS curves
  (evaluation, knot operations and fitting through `curvo`), clothoid spirals and other transition
  curves used by alignments, offsets of each, and composite curves.
- **Surfaces:** planes, cylinders, cones, spheres, tori, NURBS surfaces, extrusion and revolution
  surfaces, with evaluation, normals, closest point and isoparametric curves.
- **Queries:** closest point, curve–curve and curve–surface intersection with tangency
  classification, curve splitting and joining, length, area, centroid and bounding boxes.
- **Regions:** planar regions bounded by loops of exact curves, with booleans (union, difference,
  intersection, exclusive or), offsets (round, miter, square joins), point containment and
  hatching support. Booleans flatten arcs only at a known tolerance for the `i_overlay` core and
  then restore exact arcs on the result where the output edge lies on an input arc within
  tolerance.
- **Triangulation:** constrained Delaunay triangulation and **TIN surfaces** (`spade`) with
  breaklines, boundaries and holes; elevation queries, contours, slope and volume between surfaces.
- **Spatial indexes:** R*-trees (`rstar`) for 2D and 3D bounding volumes and k-d trees for point
  sets, updatable incrementally per object.
- **Transforms and views:** rigid and affine transforms, coordinate frames, and the view frame
  (eye, target, up, perspective or orthographic projection, clip planes) shared by `arc-view`,
  `arc-scene` and `arc-gpu`.

## Meshes and subdivision

`arc-mesh` owns:

- **Indexed triangle meshes** for rendering and interchange, and a **half-edge polygon mesh** for
  editing (n-gons, boundaries, non-manifold detection), with conversion between them.
- **Attributes** on vertices, face corners, faces and edges: normals, tangents, UV sets, colors,
  creases, weights and user attributes, with typed storage and interpolation rules.
- **Subdivision surfaces:** Arc-owned Catmull–Clark with sharp and semi-sharp creases, corner
  vertices and boundary interpolation rules that follow the published OpenSubdiv specification, so
  limit surfaces match other tools on interchange ([ADR-0008](../adr/0008-native-subdivision-surfaces.md)).
- **Mesh booleans** through Manifold, wrapped behind an `arc-mesh` API with Arc-owned error
  reporting. Inputs that are not closed manifolds are rejected with the offending edges listed.
- **Simplification, vertex-cache optimization and meshlet building** through `meshopt`.
- **Queries** through `parry`: ray casts, closest points, intersection tests, distance and contact,
  used for picking, snapping and clash detection.
- **Repair:** weld, remove degenerate faces, orient consistently, fill small holes, each reporting
  what it changed.

## The arc-brep API

### Data model

| Type | Meaning |
|---|---|
| `Shape` | An opaque, immutable, reference-counted handle to a solid, a shell, a face set, a wire or a compound |
| `Topo` element | Vertex, edge, loop, face, shell or solid within a shape, addressed by a `TopoName` |
| `TopoName` | The persistent name of a topological element (below) |
| `Geometry` accessors | Exact `arc-geom` curves and surfaces of edges and faces, converted from kernel geometry |
| `ShapeData` | The serialized form stored as a document asset: kernel-neutral header, the kernel's exact representation, and the naming table |

### Operation families

| Family | Operations |
|---|---|
| Primitives | Box, cylinder, cone, sphere, torus, wedge, prism from profile |
| Sweeps | Extrude (with draft and to-face limits), revolve, sweep along path (with orientation modes), loft through sections, helical sweep |
| Booleans | Union, subtract, intersect, split by tool, imprint; with an optional fuzzy tolerance |
| Local operations | Fillet (constant and variable radius), chamfer (distance and angle), shell (hollow) with face removal, offset faces, draft faces, thicken surfaces |
| Direct editing | Move, rotate, replace or delete faces with healing of neighbors |
| Surfaces | Fill boundary, extend, trim, sew faces into shells |
| Queries | Mass properties, closest points, distance, containment, interference, curvature |
| Sectioning | Exact plane and surface sections producing `arc-geom` curves and regions |
| Tessellation | Triangle meshes with chord and angle tolerance, per-face groups and edge polylines carrying names |
| Exchange | STEP (AP203, AP214, AP242) and IGES read and write, including assembly structure, colors and names, through the kernel tier |
| Healing | Fix small edges, gaps, orientation; report changes |

### Result and error model

Every operation returns either a result or an error; neither panics nor returns an invalid shape.

- A **result** contains the new `Shape`, the naming table for its elements, a **history** (for each
  output element: generated from, modified from, or new; for each input element: kept, modified or
  deleted) and warnings (for example "fillet radius reduced at edge E").
- An **error** carries a stable code (`brep.fillet.failed`, `brep.boolean.non_manifold_result`,
  `brep.sweep.self_intersection`), the input elements involved as `TopoName`s where known, and a
  message identifier. Products show which feature and which edge failed.
- Every result passes a validity check (closed shells, consistent orientation, tolerances within
  bounds) before it is returned; a kernel result that fails the check becomes an error.
- Operations are cancellable at kernel progress points and run on worker threads. The kernel binding
  is thread-safe per shape; shapes are immutable.

### Persistent topological naming

A feature that references "this edge" must still find it after an upstream edit. `TopoName` is
built in three layers:

1. **Authored identifiers.** Inputs authored by users or scripts (sketch entities, profile loops,
   reference planes, explicit tags placed on faces and edges by commands) carry stable identifiers.
   Elements produced directly from them inherit names such as `(feature F, role lateral, source
   sketch-edge S)`.
2. **History-derived names.** Every other element is named from the operation history: the feature
   identifier, the operation role (`start_cap`, `end_cap`, `lateral`, `fillet_face`, `split_part`,
   `intersection_edge`) and the names of the source elements it was generated or modified from.
   Names are canonical (sorted, deduplicated) so the same history always yields the same name.
3. **Geometric fallback.** When history leaves several candidates (a split produced two faces from
   one), a geometric signature (surface type, normal or axis, centroid, area or length, adjacency
   to named neighbors) disambiguates. A reference resolved by the fallback is marked
   `resolved_by_geometry` and is **surfaced to the user**: the feature shows a warning and the
   user confirms or re-picks. A reference that cannot be resolved is `broken`, keeps its last name
   and signature, and the feature reports it; nothing is guessed silently.

References stored in documents hold the `TopoName` and the geometric signature. Products should
prefer semantic references (a BIM opening hosted by "the exterior face role of wall W") over B-rep
names wherever the domain allows. Property tests mutate upstream parameters across the regression
corpus and assert that references survive or are reported.

## Kernel isolation

| Aspect | Rule |
|---|---|
| Crate | `arc-brep-occt`, the only crate that contains the kernel binding; private to `arc-brep` |
| Binding | An Arc-owned C++ shim exposing a narrow, handle-based C++ API, bound to Rust with `cxx`; only operations `arc-brep` uses are bound |
| Errors | The shim catches every C++ exception at the boundary and returns error codes; no exception crosses into Rust |
| Linking | Dynamic: the kernel's shared libraries ship beside the product binary, satisfying LGPL-2.1 replaceability |
| Version | One kernel release pinned in `arc-core`; upgrades are single pull requests that must pass the golden-model corpus |
| Build | `cargo xtask occt` builds the pinned kernel from source with a fixed configuration and produces a cached prebuilt artifact per platform; CI and developers download the artifact by checksum instead of building |
| Packaging | Products include the kernel libraries and their license text in installers; `arc-worker` uses the same libraries |
| Hostile input | STEP and IGES import run inside `arc-worker`; the result returns as `ShapeData` bytes |
| Fault containment | Modeling operations run in-process; batch and Hub jobs may request isolated mode, which runs them in `arc-worker` |

The **golden-model regression corpus** is a repository of STEP files and parametric feature
scripts, each with expected results: volume, surface area, bounding box, topology counts (solids,
shells, faces, edges, vertices), validity, and naming survival after a scripted parameter edit.
Tolerances on numeric expectations are stated per model. Every kernel upgrade, every native
reimplementation and every nightly build runs it; a regression blocks the change. Details are in
[ADR-0006](../adr/0006-kernel-binding-and-isolation.md).

## Constraint solver

`arc-solve` solves geometric constraint systems for 2D sketches and 3D assemblies of rigid bodies
([ADR-0007](../adr/0007-arc-owned-constraint-solver.md)).

- **Entities:** points, lines, circles, arcs, ellipses, splines (control points), and in 3D rigid
  bodies with frames. Parameters are `f64` in model units.
- **Constraints:** coincident, point on curve, horizontal, vertical, parallel, perpendicular,
  tangent (with side), concentric, equal length or radius, symmetric, midpoint, fixed, distance,
  angle, radius, diameter, arc length, and in 3D mates (coincident, concentric, distance, angle,
  parallel, perpendicular, fixed). Dimensions may be **driving** (constrain) or **driven**
  (measure).
- **Decomposition:** the system is a bipartite graph of entities and constraints. It is split into
  connected components, then into rigid clusters solvable in sequence (well-constrained subsystems
  placed by construction, then merged). Only clusters affected by a change are re-solved.
- **Numeric core:** each cluster that cannot be placed constructively is solved by
  Levenberg–Marquardt on residuals with analytic Jacobians and sparse factorization (`faer`).
  The previous solution is the starting point; **branch stability** is enforced by orientation
  hints (arc direction, loop winding, side of tangency) and by rejecting a converged solution that
  flips an orientation the user did not change.
- **Degrees-of-freedom diagnostics:** rank-revealing factorization of the Jacobian gives the
  remaining degrees of freedom per entity. Entities are reported fully constrained, under-constrained
  (with the free directions) or over-constrained.
- **Over-constraint reporting:** redundant constraints (consistent but unnecessary) and conflicting
  constraints (inconsistent) are identified as minimal sets and reported by constraint identifier;
  the solver never silently drops a constraint.
- **Dragging:** interactive drags solve for the minimum change from the previous solution with the
  dragged entity as a soft target.
- **Limits:** iteration and time budgets per solve; exceeding them returns
  `solve.not_converged` with the best residual and the clusters involved.

## Expression engine with units

`arc-solve` also provides the expression and parameter engine used by sketches, families, feature
parameters, property formulas, node graphs and numeric input fields.

- **Values** are numbers with physical dimensions (`arc-base` quantities), booleans, strings, and
  lists. Arithmetic checks dimensions: adding a length to an area is an error, multiplying lengths
  gives an area.
- **Literals** accept units (`2400 mm`, `2.4 m`, `8' 6"`, `45 deg`, `0.5 rad`, `12 kN/m`); a bare
  number takes the unit of the context (the field's display unit).
- **Functions:** arithmetic, trigonometric (angle-aware), rounding (to increments with units),
  minimum and maximum, conditionals, lookups in tables provided by the host, and string formatting.
  The function set is closed; hosts can register additional pure functions.
- **Parameters** are named expressions owned by a document object. Parameter references form a
  dependency graph inside the owning scope; cycles are rejected with the cycle listed.
- **Evaluation** is deterministic, bounded (expression depth, list length, evaluation steps) and
  side-effect free. There is no I/O, no loops other than bounded list functions, and no access to
  anything the host did not pass in.
- **Display** formats quantities in the document's display units with configured precision and
  rounding; parsing and formatting round-trip.
