# arc-view

Derived drawings of 3D models: plans, sections, elevations, details, axonometric and perspective
drawings produced as `arc-draft` geometry that links back to its source objects. The architecture
is specified in [drafting-and-views.md](../architecture/drafting-and-views.md#derived-views-arc-view).

## Owns

- View definitions: frame, cut plane, ranges, crop, scope, detail level.
- The view source interface through which products supply geometry, categories and styles.
- Edge extraction, polyhedral hidden-line removal and the use of exact hidden-line removal from
  `arc-brep`.
- Section cuts, region merging and fill-pattern placement.
- Visibility classification and the mapping to line styles.
- Back-links from derived primitives to source objects and sub-object keys, and annotation
  re-attachment.
- Incremental view regeneration through the dependency graph.

## Stays in products

Which objects appear in which view, the styles per category and visibility class, view templates,
view ranges as a product concept (for example storey-relative ranges), and product annotation
objects.

## Key concepts and types

| Concept | Description |
|---|---|
| `ViewDef` | View frame, optional cut plane, depth ranges, overhead range, crop, scope filter, detail level |
| `ViewSource` | Product trait: objects in scope, their geometry kind, category, styles and joinability |
| `EdgeSet` | Feature, boundary, silhouette and cut edges with provenance |
| `VisibilityClass` | Visible, hidden, cut, beyond, overhead, silhouette |
| `CutRegion` | Closed region from a section with source object, material key and fill style |
| `ViewResult` | `arc-draft` display content plus a back-link table |
| `AnchorRef` | (view, source id, sub-object key, curve parameter) used by annotations |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom`, `arc-mesh`, `arc-doc`, `arc-draft`; `arc-brep` through an optional feature for exact sections and hidden lines |
| Third-party | `rayon` (planned for parallel hidden-line removal; not used yet) |

## Test strategy

Reference-drawing tests (SVG output compared structurally and by image) for standard test models;
property tests that rotate views and assert classification consistency; incremental regeneration
tests that change one object and compare with a full rebuild; annotation re-attachment tests across
edits; benchmarks for hidden-line removal on large models.

## Capabilities

### View definition

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-VIEW-001 | Orthographic views | Produces drawings for any orthographic view frame, including plan, elevation and axonometric directions. | T1 | 2 |
| CORE-VIEW-002 | Perspective drawings | Produces line drawings for perspective view frames with the same classification as orthographic views. | T2 | 3 |
| CORE-VIEW-003 | Cut planes and ranges | Applies a cut plane, near and far ranges and an overhead range; objects are classified as cut, beyond, overhead or omitted accordingly. | T1 | 2 |
| CORE-VIEW-004 | Crop boundaries | Clips view results to a rectangular or arbitrary closed crop boundary in the view plane. | T1 | 2 |
| CORE-VIEW-005 | Scope filters | Includes only objects selected by the product's scope filter, re-evaluated when objects change. | T1 | 2 |
| CORE-VIEW-006 | Detail levels | Passes coarse, medium or fine detail level to the view source so it supplies matching geometry. | T1 | 2 |
| CORE-VIEW-007 | Section boxes for views | Limits a 3D view's drawing to a box with cut faces on the box sides. | T2 | 3 |

### Projection and hidden lines

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-VIEW-008 | Edge extraction | Extracts feature edges above a crease angle, boundary edges and silhouette edges for the view direction from meshes and solids. | T1 | 2 |
| CORE-VIEW-009 | Polyhedral hidden-line removal | Classifies every edge segment as visible or hidden with on-sheet error below 0.05 mm at the view scale, in parallel by screen tiles. | T1 | 2 |
| CORE-VIEW-010 | Exact hidden-line removal | Uses the B-rep tier for exact visible and hidden curves when the view requests exact output. | T2 | 3 |
| CORE-VIEW-011 | Segment merging | Merges collinear and co-circular segments of the same class and source into single primitives. | T1 | 2 |
| CORE-VIEW-012 | Visibility classes | Labels each primitive visible, hidden, cut, beyond, overhead or silhouette and applies the source's line style for that class. | T1 | 2 |
| CORE-VIEW-013 | Incremental regeneration | Recomputes only changed objects' edges and the occlusion within their screen footprint; results equal a full rebuild. | T1 | 2 |

### Sections and fills

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-VIEW-014 | Section cut regions | Produces closed cut regions per object from solids (exact) and closed meshes (planar sections). | T1 | 2 |
| CORE-VIEW-015 | Region joining | Merges cut regions of objects the source marks as joinable so shared boundaries disappear. | T1 | 2 |
| CORE-VIEW-016 | Cut fill patterns | Fills cut regions with the source's pattern, scale, angle and boundary lineweight, with pattern origins stable across regenerations. | T1 | 2 |
| CORE-VIEW-017 | Invalid section reporting | Reports objects whose section is not closed as diagnostics and draws their cut edges without fill. | T1 | 2 |
| CORE-VIEW-018 | Graphic overrides | Applies per-view overrides by object, category or rule (color, line style, pattern, visibility, halftone) when building the view. | T1 | 2 |

### Links to source objects

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-VIEW-019 | Back-links | Tags every primitive with source object, sub-object key and visibility class. | T1 | 2 |
| CORE-VIEW-020 | Selection through views | Resolves a picked primitive to its source object and returns all primitives of that object for highlighting. | T1 | 2 |
| CORE-VIEW-021 | Annotation anchoring | Re-attaches annotations anchored to derived geometry by source and sub-object key after regeneration; missing anchors mark annotations orphaned with a diagnostic. | T1 | 2 |
| CORE-VIEW-022 | Snapping on derived geometry | Exposes derived primitives to snapping with the same snap kinds as drafting geometry. | T1 | 2 |

## Plan views as implemented

Implemented: CORE-VIEW-001 for orthographic frames (plan, elevation, section), CORE-VIEW-003,
CORE-VIEW-004, CORE-VIEW-005, CORE-VIEW-006, CORE-VIEW-008 without silhouette edges, CORE-VIEW-009
single-threaded, CORE-VIEW-011 for straight segments, CORE-VIEW-012, CORE-VIEW-014 for closed planar
solids and triangle meshes, CORE-VIEW-015 to CORE-VIEW-017, CORE-VIEW-018 by object and by category
(rule-based overrides are not implemented), CORE-VIEW-019 and CORE-VIEW-020. Not implemented:
perspective drawings (CORE-VIEW-002), section boxes (CORE-VIEW-007), exact hidden-line removal
through `arc-brep` (CORE-VIEW-010), incremental regeneration (CORE-VIEW-013: the view is rebuilt
whole), annotation anchoring (CORE-VIEW-021) and snapping on derived geometry (CORE-VIEW-022). The
crate has no third-party dependency yet.

### Definition and source

The structs and enums a product builds or matches are `#[non_exhaustive]`; they are built with
constructors (`PlanRange::new`, `ViewDef::plan`, `SourceObject::new` and `single`,
`Part::new`, `Symbol::new`, `CutFill::solid` and `pattern`, `LineStyle::solid`,
`ClassStyles::uniform`), `with_*` methods, or `Default` followed by setting public fields.

- **`ViewDef`.** A `ViewDirection` (`Plan`, looking down the world z axis: world x and y
  are the sheet's x and y; or `Frame`, any orthographic frame made orthonormal by
  `ViewDirection::frame` from an origin, a right axis and an up axis, view z toward the viewer:
  a section has its cut plane through the origin, `cut` 0 and `bottom` minus the far clip
  distance, and an elevation is a section whose plane lies in front of the model), a `PlanRange` (`cut`, `bottom` below it, an optional `top` above
  it), an optional crop `Region` in view coordinates (`crop_rectangle`, `crop_polygon`), a
  `DetailLevel`, a crease angle in [0, π] (default 30 degrees), a scale (model meters per sheet
  meter), `Overrides` and `ViewLimits`. `ViewDef::chord` is `SHEET_ERROR` (0.05 mm) times the
  scale: the chord a source uses for curved geometry so the drawing stays within 0.05 mm on the
  sheet. Everything after the source works in view space (x right, y up, z toward the viewer),
  so another direction only adds its mapping. The tolerance must have a finite positive linear
  value, chord and turning angle and a finite angular value that is not negative.
- **`ViewSource`.** `objects` returns the objects in scope (the product's filter, in drawing
  order; repeats are used once); `object` returns a `SourceObject` at the view's detail level:
  a category, `ClassStyles` (an optional `LineStyle` per visibility class: color, linetype,
  lineweight, layer; a class without a style is not drawn), a list of `Part`s and a list of
  `Symbol`s. An error of `object` becomes a diagnostic and leaves the object out.
- **Parts.** A part is one layer of a compound wall, a slab, or a whole simple object: a
  `Geometry` (a closed `Polyhedron` or a `TriMesh`), the product's stable sub-object key, an
  optional `CutFill` (pattern families, scale, angle, color, layer; no families is a solid
  fill) and an optional join key. Parts are classified, sectioned and occluded one by one;
  each layer keeps its own fill.
- **Symbols.** A symbol is a bounded 2D curve in view coordinates that the product draws for an
  object (a door swing, a stair arrow, the frame of a tag), with a visibility class, a stable
  sub-object key and an optional own style (otherwise the object's style of the class). It goes
  through overrides, crop, draw order and back-links like derived lines, but not through
  hidden-line removal.
- **Checks.** Before use, a non-finite color becomes opaque black, a lineweight that is not
  finite and not negative the default, a fill with a scale that is not finite and positive, a
  non-finite angle or an invalid pattern family a solid fill, and a symbol with an unbounded
  curve or a coordinate beyond `max_coordinate` is left out; each is a `view.style.invalid` or
  `view.geometry.out_of_range` diagnostic of the object. A part whose triangles are all no
  higher than the linear tolerance is left out with `view.geometry.degenerate`.
- **Overrides.** `GraphicOverride` sets visibility, color, linetype, lineweight, cut fill and
  halftone (colors halfway toward white) for every line class, symbol and part of an object;
  each field of an object's override wins over the same field of its category's override.

### Classification

Each part's geometry is meshed, its exactly equal positions are welded, its triangles no higher
than the linear tolerance (zero area or slivers) are dropped, and its height span in view space
is compared with the ranges. A face lying exactly in the cut plane belongs to the part above
it, in classification as in sections: **cut** when the span starts below `cut` and reaches it;
**beyond** when it ends below `cut` and reaches `bottom`; **overhead** when it starts at or above
`cut` below `top`; otherwise omitted. Two solids meeting at the cut height therefore give one
cut part (the lower) and one overhead part.

### Edges, sections and hidden lines

- **Edges** are the boundary, sharp (dihedral angle above the crease angle) and non-manifold
  edges of `arc_mesh::features::feature_edges`. Edges of cut and beyond parts are clipped to
  heights from `bottom` to `cut` (plus the linear tolerance), those of overhead parts from
  `cut` to `top`; edges whose projection is not longer than the linear tolerance are dropped.
- **Sections.** A vertex at exactly the cut height counts as above. Each triangle with vertices
  on both sides gives one segment from its edge running from above to below to its edge
  running from below to above, so an outward mesh gives counterclockwise outer loops and
  clockwise holes. A point on a mesh edge is computed from its lower vertex, so the segments of
  both triangles of an edge meet exactly and chain by mesh topology. Closed chains become a
  region (`arc_geom::build_region`); a chain that does not close gives the diagnostic
  `view.section.open`, and a region that cannot be built gives its geometry error; either way
  the section's edges are drawn as cut lines without fill.
- **Joining.** Regions of parts with the same join key, of any objects, are merged. The regions
  of a key are split into components whose boxes overlap or touch (found with an R*-tree), so
  regions far apart are never united; each component is united with `arc_geom::boolean` as a
  balanced tree of pairwise unions. The curves entering every union count against
  `ViewLimits::max_join_curves`; a union that fails leaves its operands unjoined with the error
  as a diagnostic. A joined region takes the fill of its first part in scope order. A boundary
  piece of a region is drawn as a cut line of the part whose section it lies on (within twice
  the linear tolerance); pieces on no section, such as the crop edge, are not drawn, so shared
  boundaries of joined parts disappear.
- **Hidden lines.** The occluders are the triangles of every cut and beyond part, clipped to the
  cut height (fan triangles no wider than the tolerance, such as those of a clip through a
  vertex in the cut plane, are dropped), and the triangles of every cut region
  (`arc_geom::triangulate`), which lie above everything below the cut. An edge point is hidden
  by a triangle when its projection is inside the triangle's and the triangle is nearer the
  viewer by more than the linear tolerance; both conditions are linear along the edge, so each
  occluder hides one exact parameter interval and the result is exact for polyhedral input up
  to rounding. A condition that is not finite hides nothing. A triangle edge on the outline of
  its surface in projection must be crossed by more than the tolerance; an edge shared with a
  triangle that continues the surface on its other side is widened by the tolerance, as is every
  edge of a cut region, so nothing shows through a face diagonal and edges on a cut boundary lie
  under the cut line. Overhead edges are not occluded. Visible pieces are `Visible` for cut parts
  and `Beyond` for beyond parts; hidden pieces are `Hidden`, drawn only when the source styles
  that class. Silhouette edges are not produced.
- **Cost.** Candidates come from an R*-tree of occluder boxes (`arc_geom::SpatialIndex2`), so
  no grid depends on coordinate magnitude; each candidate and each crop curve test counts
  against `ViewLimits::max_occlusion_tests` (default 50 million, a few seconds). Objects,
  triangles, edges, join curves and primitives are counted against their limits before the work
  that depends on them. The worst case of the occlusion step is the product of edges and
  occluders, cut off by the bound. The `Cancel` token passed to `build_view` is checked between
  objects, parts, edge sets and unions.

### Output

- **Merging.** Segments of one part and class are grouped by direction, then by line offset,
  and overlapping or touching pieces merge into one segment between the extreme endpoints.
  Co-circular merging is not implemented.
- **Crop.** Lines and symbols are cut to the crop region (pieces inside or on it are kept, with
  `arc_draft::clip::cut` against every loop); cut regions are intersected with it before
  filling. Occlusion ignores the crop. A crop that fails on one region leaves that region out
  with a diagnostic.
- **Fills.** A solid fill is one `Shape::Region`; a pattern is generated with
  `arc_draft::hatch::generate` at the image of the world origin, so pattern lines do not move
  when the crop or other objects change. A pattern beyond `ViewLimits::max_pattern_marks` or
  the remaining primitive budget, or one that fails (a diagnostic), is drawn as a solid fill.
- **Draw order.** Fills, then patterns, then the lines of each class in the order beyond,
  hidden, visible and silhouette, cut, overhead. Within a class, objects follow scope order,
  and an object's symbols of that class follow its derived lines. Each primitive's `draw_order`
  is the rank of its kind (0 to 6 in that order).
- **`ViewResult`.** An `arc_draft::display::DisplayList` of strokes, points and regions in view
  coordinates (the style's linetype is carried and dashes are not generated), a `BackLink` per
  primitive and the diagnostics in scope order. A back-link holds the source `ObjectId`, the
  product's stable `sub_key` (the key of the part or symbol), the geometric `face` (the face
  index of a solid or the face-group key of a mesh, of an edge's lowest triangle or of the
  triangle a section segment crosses; `None` for fills and symbols), the `VisibilityClass`, the
  `PrimitiveKind` and, for a joined fill, the other (object, part key) pairs. Face indices
  change when geometry is regenerated and are not stable keys; `sub_key` is. `select` resolves a
  picked primitive to its source and all of that source's primitives; `primitives_of` lists
  them.

Reference-drawing tests check a room of four joined walls, a wall with a door opening, a slab
beyond the cut under the walls, a stepped solid and an overhead beam by counts per class and
exact coordinates within the tolerance. Property tests check that a rotation about the vertical
axis rotates the drawing with the same classes and fill area, that a randomly oriented box
crossing the cut gives one closed region with the area of its exact section, and that no
visible point of a box lies under a higher box. Further tests cover empty input, faces and
vertices in the cut plane (including a far edge that must stay visible), solids meeting at the
cut height, open, soup, sliver and tiny meshes, sloped solids, concave crops, coordinates near
1e7 m, every limit and cancellation, invalid definitions, fills and lineweights, overrides,
layered parts joined by material, symbols, pattern stability under a changed crop, picking, a
grid of 900 columns and 800 joined walls.

## Error codes

The codes `arc-view` owns. A code is never reused for a different meaning; messages are in
`locales/en-US/arc-view.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `view.definition.invalid` | `invalid_input` | A tolerance value, range, crease angle, scale, coordinate bound or crop corner of a view definition is out of range or not finite (argument `parameter`) |
| `view.limit.exceeded` | `limit_exceeded` | The objects, triangles, edges, occlusion tests, join curves or primitives of a view exceed its limits (argument `limit`: `objects`, `triangles`, `edges`, `occlusion_tests`, `join`, `primitives`) |
| `view.geometry.out_of_range` | `invalid_input` | An object has a view-space coordinate beyond `ViewLimits::max_coordinate`; a diagnostic |
| `view.section.open` | `failed_operation` | The section of an object at the cut plane does not close; a diagnostic |
| `view.geometry.degenerate` | `invalid_input` | No triangle of an object part is higher than the linear tolerance (the part is too small to draw); a diagnostic |
| `view.style.invalid` | `invalid_input` | A color, lineweight, cut fill or symbol of an object is invalid and was replaced by a default or left out (detail `what`); a diagnostic |
