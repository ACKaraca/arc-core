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
| Third-party | `rayon` |

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
