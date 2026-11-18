# arc-gpu

The real-time viewport renderer on `wgpu`: retained viewport scenes extracted from documents,
camera-relative precision, GPU-driven culling, drafting-quality lines and hatches, shading modes,
section planes, picking, overlays, point clouds, off-thread frame scheduling and headless rendering.
The architecture is specified in [rendering.md](../architecture/rendering.md#the-viewport-renderer-arc-gpu).

## Owns

- Device and adapter selection, including software adapters for headless use.
- The viewport scene: render items, pooled GPU buffers, material table and identifier table.
- The extractor interface and the shared extractor for `arc-draft` display lists.
- Render pipelines and WGSL shaders for meshes, lines, fills, text, points, overlays, picking,
  section caps and post effects.
- The render thread, viewport targets and frame scheduling.
- Headless rendering to images and the image-comparison harness used by tests.
- Re-export of the pinned `wgpu` version for products.
- Backend selection and budgets for the browser (WebGPU) and tablet and phone (Metal, Vulkan)
  targets: device limits, memory budgets, lifecycle recovery and device-loss handling.

## Stays in products

Extractors for product objects (what a wall or a corridor looks like), product display styles and
override rules, product-specific overlays and manipulators built on the overlay API, and render
modes beyond the documented set (for example analysis color maps a product defines through the
override API).

## Key concepts and types

| Concept | Description |
|---|---|
| `Gpu` | Device, queue and the `AdapterReport` (adapter, backend, software flag, reduced features) |
| `Scene` | Render items keyed by object identifier and sub-item index on pooled buffers, with write accounting and a memory report |
| `RenderItem` | Mesh instance, line batch, fill batch, glyph run, point set, image or overlay item, with `f64` origin, material and flags |
| `Extractor` | Product function from snapshot and changed ids to render items |
| `ViewportMaterial` | Documented OpenPBR subset used for viewport shading |
| `Viewport` | Target size, view frame, shading mode, overrides, section state, lineweight display |
| `PickResult` | Object identifier, sub-object key, depth, position |
| `Overlay` | Transient items with their own hit testing |
| `FrameStats` | Timings, visible counts, memory use |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom`, `arc-mesh`, `arc-doc` (snapshots and change events for extraction) (in use); `arc-draft` (feature `draft`, display-list extractor, from phase 1); `arc-scene` (feature `scene`, material and camera conversion, from phase 4) |
| Third-party | `wgpu` (in use: Vulkan, Metal, DirectX 12 and OpenGL backends, WGSL), `glam`, `lyon`, `swash` (glyph rasterization); `image` (PNG reference images, tests only: headless rendering returns raw RGBA8 buffers) |

## Test strategy

Image-comparison tests rendered headless on a pinned software adapter for every pipeline, shading
mode and line feature, with a perceptual difference threshold; pick tests that render known scenes
and assert identifiers at pixel positions; precision tests at georeferenced coordinates
(10^7 m from origin) asserting sub-millimetre vertex placement; culling equivalence tests (GPU path
and CPU path produce the same visible set); benchmarks for frame time on the reference large models
in the nightly job on real GPUs. Browser and mobile: the image-comparison suite runs on WebGPU in
Chromium, Firefox and WebKit and on the iOS simulator and Android emulator; device-loss tests destroy
the device mid-frame and assert that the scene is restored without re-extraction; budget tests lower
the memory limit and assert eviction order and that no allocation exceeds the limit.

## Capabilities

### Device and scene

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GPU-001 | Adapter selection | Selects Vulkan, Metal or DirectX 12 adapters by preference with a setting to force an adapter or backend, and falls back to the OpenGL backend or a software adapter with reduced features reported. | T1 | 0 |
| CORE-GPU-002 | Retained viewport scene | Holds render items keyed by object identifier; updating items of changed objects leaves all others untouched on the GPU. | T1 | 0 |
| CORE-GPU-003 | Incremental extraction | Runs product extractors for changed objects on the worker pool and applies results only if newer than the applied revision for each object. | T1 | 0 |
| CORE-GPU-004 | Camera-relative precision | Uploads per-item translations relative to the camera computed in `f64`; geometry 10,000 km from the origin renders without visible jitter or vertex error above 0.1 mm. | T1 | 0 |
| CORE-GPU-005 | Tiled large items | Splits items larger than 1 km into tiles with their own origins automatically. | T1 | 1 |
| CORE-GPU-006 | Pooled buffers | Sub-allocates vertex, index and instance data from pooled buffers with defragmentation, reporting GPU memory use per category. | T1 | 0 |

### Drawing

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GPU-007 | Mesh rendering | Draws instanced meshes with per-instance transforms, colors, materials and flags, with reversed-Z depth. | T1 | 0 |
| CORE-GPU-008 | Exact-width lines | Draws lines as instanced segments with exact screen or paper widths, round or square caps and joins, at any zoom level. | T1 | 0 |
| CORE-GPU-009 | Linetype patterns | Evaluates dash patterns by accumulated arc length along polylines in the shader so dashes stay continuous across vertices and stable while panning. | T1 | 1 |
| CORE-GPU-010 | Curve flattening on zoom | Re-flattens exact curves when zoom changes by more than a factor of two so arcs always appear smooth. | T1 | 1 |
| CORE-GPU-011 | Hatches and fills | Draws pattern hatches in world space clipped to regions, solid and gradient fills, with pattern origins fixed in the document. | T1 | 1 |
| CORE-GPU-012 | Text rendering | Draws outline glyphs from an atlas and single-stroke text as lines, from `arc-draft` layout results. | T1 | 1 |
| CORE-GPU-013 | Drafting display lists | Draws `arc-draft` display lists with lineweight display, plot-style preview and sheet backgrounds through the shared extractor. | T1 | 1 |
| CORE-GPU-014 | Shading modes | Provides wireframe, hidden line, shaded, shaded with edges, consistent colors and X-ray modes; switching mode requires no re-extraction. | T1 | 0 |
| CORE-GPU-015 | Realistic mode | Shades the viewport material subset with image-based lighting, shadows and ambient occlusion. | T2 | 4 |
| CORE-GPU-016 | Display overrides | Applies per-view color, transparency, ghosting, isolation and hiding by object, category or rule through per-instance flags. | T1 | 1 |
| CORE-GPU-017 | Edge display | Draws feature and silhouette edges from derived edge data over shaded geometry. | T1 | 1 |

### Performance

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GPU-018 | GPU-driven culling | Culls instances by frustum and hierarchical depth on the GPU and draws with multi-draw indirect where available; the CPU fallback produces the same visible set. | T1 | 1 |
| CORE-GPU-019 | Level of detail | Selects mesh levels of detail by projected screen error. | T1 | 1 |
| CORE-GPU-020 | Performance targets | Sustains 60 frames per second orbiting 10 million triangles and 2 million line segments on the reference mid-range GPU, verified by nightly benchmarks. | T1 | 1 |
| CORE-GPU-021 | Adaptive quality | Lowers expensive effects during navigation to keep the frame target and restores full quality when navigation stops. | T2 | 1 |

### Interaction

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GPU-022 | Identifier picking | Resolves the object and sub-object under a pixel or within a rectangle from an identifier pass restricted to the region of interest, within 16 ms. | T1 | 0 |
| CORE-GPU-023 | Pre-selection and selection highlight | Highlights the object under the pointer and selected objects with outline and tint in every shading mode. | T1 | 0 |
| CORE-GPU-024 | Section planes and box | Clips by up to six planes and one box per view, capping cut solids with section color or hatch. | T1 | 1 |
| CORE-GPU-025 | Overlays | Draws grips, snap markers, tracking lines, rubber bands, previews, measurement graphics, grid, axis triad and view cube above the scene, unaffected by sections. | T1 | 0 |
| CORE-GPU-026 | Manipulators | Draws move, rotate and scale handles at constant screen size with their own hit testing. | T1 | 1 |
| CORE-GPU-027 | Navigation support | Provides orbit, pan, zoom to point, zoom extents, zoom to selection and named view transitions with smooth interpolation, for orthographic and perspective cameras. | T1 | 0 |

### Point clouds, scheduling and headless

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GPU-028 | Point-cloud display | Streams octree nodes within a per-frame point budget, draws adaptive splats with eye-dome lighting, and colors by RGB, intensity, classification, elevation or single color. | T1 | 3 |
| CORE-GPU-029 | Off-thread rendering | Renders viewports on the render thread into offscreen targets that the UI composites; a slow frame never blocks UI input. | T1 | 0 |
| CORE-GPU-030 | On-demand redraw | Draws only when camera, scene, overrides or progressive effects change; an idle viewport consumes no GPU time. | T1 | 0 |
| CORE-GPU-031 | Headless rendering | Renders views to images without a window on any adapter including software adapters, for thumbnails, automation and tests. | T1 | 0 |
| CORE-GPU-032 | Multiview cameras | Accepts several view frames per scene render for stereo and immersive review without duplicating scene data. | T3 | 5 |
| CORE-GPU-033 | WebGPU backend | Builds and renders in browsers through the WebGPU backend with the CPU culling path. | T2 | 2 |

### Browser, tablet and phone

Rendering on these targets follows the same scene, items and pipelines; what differs is the device
contract. The platform guarantees are in the
[architecture overview](../architecture/overview.md#platform-targets).

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-GPU-034 | Mobile backends | Renders through Metal on iOS and Vulkan on Android with the same pipelines, reporting reduced features when an OpenGL ES fallback is used. | T3 | 5 |
| CORE-GPU-035 | Browser device limits | Reads the adapter's WebGPU limits and splits buffers, textures and tiles to fit; WebGPU is required and no WebGL fallback is provided in the first release. | T3 | 5 |
| CORE-GPU-036 | Surface and device lifecycle | Recreates surfaces on suspend, resume, resize and scale change and restores the scene after device loss from retained CPU data without re-extraction. | T3 | 5 |
| CORE-GPU-037 | Memory budget | Sets GPU memory budgets from device limits and platform warnings, evicting texture levels, tiles and levels of detail in a documented order and reporting the budget in frame statistics. | T3 | 5 |
| CORE-GPU-038 | Mobile performance targets | Sustains 30 frames per second orbiting 2 million triangles and 500,000 line segments on the reference tablet, with adaptive quality always enabled during navigation. | T3 | 5 |
| CORE-GPU-039 | Pointer-sized picking | Sizes the identifier-pass region from the pointer kind's pick radius so touch and pen picks resolve the intended object within 16 ms. | T3 | 5 |
| CORE-GPU-040 | Thermal and power response | Lowers frame rate cap and expensive effects when the platform reports thermal pressure or low-power mode and restores them afterward. | T3 | 5 |

## Phase 0 rules

The first part implements CORE-GPU-001, 002, 004, 006, 007, 008 and 031; the second part adds
CORE-GPU-014, 022, 023, 025 and 027 as passes and per-frame inputs that do not change the scene;
the third part adds CORE-GPU-003, 029 and 030: extraction from document snapshots and viewports
rendered on their own thread, on demand. Window surfaces and compositing are the UI kit's.

### Adapter selection

`Gpu::new(&GpuSettings)` enumerates adapters on the Vulkan, Metal, DirectX 12 and OpenGL backends,
or only on the backend `GpuSettings::backend` forces; `adapter_name` keeps the adapters whose name
contains it, ignoring case. An adapter is suitable when it supports the downlevel default limits
and drawing with a base vertex. Suitable adapters are ranked in tiers: hardware adapters on
Vulkan, Metal or DirectX 12; software adapters on those backends; adapters on the OpenGL backend,
hardware before software. Within a tier, discrete adapters come before integrated, virtual and
other ones, then Vulkan before Metal before DirectX 12, then the adapter name. Every tier after
the first is a fallback, which `forbid_fallback` excludes. A forced backend or name never falls
back to a different adapter.

The `AdapterReport` names the adapter, driver, backend and device kind, says whether the adapter
is a software rasterizer and whether it is a fallback, gives the usable multisample count (4 or
1) and lists the reduced features: `SoftwareRasterizer`, `GpuCulling` (no indirect multi-draw
with count, so culling will use the CPU path), `Multisampling` (no four-sample color and depth
targets) and `DownlevelLimits` (only the downlevel default limits). Devices are created with the
downlevel default limits and no optional features. `Gpu::instance` and `Gpu::adapter` expose the
`wgpu` instance and adapter for creating a window surface; the instance has no display handle,
so a surface presents on Vulkan, Metal and DirectX 12 and not on the OpenGL backend.

Device errors never panic. Resource creation, updates and frames run inside out-of-memory and
validation error scopes; errors outside a scope are captured by the device's error handler and
returned by the next operation. Device loss is `gpu.device.lost`. Adapter and device requests and
error-scope results are awaited for at most 30 seconds, image readback for at most 60 seconds.

### Scene and pooled buffers

- A `Scene` holds render items keyed by `ItemKey { object, sub }`. `update` takes, per object, the
  complete new list of its items (the index in the list is `sub`); an empty list removes the
  object. All items are validated before anything changes. Only the instance ranges of the named
  objects are written; every other item's bytes stay untouched on the GPU. A device error during
  the update leaves earlier objects updated and the failing object without items.
- A **mesh item** references a geometry uploaded once with `add_geometry(mesh, origin)` (vertex
  positions relative to `origin`, see `TriMesh::gpu_positions`) and has an `f64` origin and one or
  more instances, each with a 3×4 `f32` transform from geometry coordinates to offsets from the
  item origin, a linear color, a material slot and flags. A geometry is removed only when no item
  uses it.
- A **line item** has an `f64` origin, polylines of `f32` points relative to it (open or closed;
  consecutive equal points merge; a single point is a dot), a style and a color.
- `add_geometry` also extracts the geometry's feature edges once (see
  [Shading modes and edges](#shading-modes-and-edges)) and stores them as index pairs in the
  index pool, counted as index writes.
- Vertex, index, mesh-instance and line-segment data live in four pools: one buffer each, divided
  into units of a fixed stride (24, 4, 80 and 80 bytes). Allocation is first-fit from a sorted,
  coalesced free list. When no free block fits, the pool compacts if its total free space
  suffices, and otherwise grows to at least twice its capacity, up to `max_pool_bytes` and the
  device's buffer size limit. After each update a pool compacts when at least 25 % of it is free
  and its largest free block is less than half of its free space; `Scene::compact` compacts on
  demand. Compaction and growth copy live ranges on the GPU, keep their order and are not counted
  as written bytes.
- Each instance pool has a companion buffer of camera-relative translations (12 bytes per
  instance). A frame writes all translations when the translation origin moved or a pool was
  relocated, and otherwise only those of items changed since the last frame.
- `pending_writes` counts the bytes written per category (vertex, index, mesh instance, line
  segment, translation) since the last frame; `last_frame_writes` holds what the last frame wrote,
  including the updates before it. `memory` reports capacity and used bytes per category.
- `SceneLimits` bounds objects, items per object, instances and segments per item, geometry
  vertices and triangles, bytes per pool, the screen line width and the image side.

Instance layouts are little-endian: a mesh instance is the three transform rows (12 `f32`), the
color (4 `f32`), then material, flags, the object's identifier slot (kept for the picking pass)
and a reserved `u32`. A line segment is its start, end, previous and next points (3 `f32` each;
the neighbors equal the ends when absent), the color, the width and the miter limit (`f32`), the
style bits (bit 0 square cap; bits 1–2 join: 0 round, 1 miter, 2 bevel; bit 3 has a previous
segment; bit 4 has a next segment; bit 5 paper width) and the identifier slot.

### Camera-relative precision and depth

- Each frame computes every item's translation `origin − translation origin` in `f64` and
  narrows it to `f32`; the view matrix on the GPU is the rotation of the view frame only. For a
  perspective view the translation origin is the eye.
- Depth is reversed: 1 on the near plane, 0 on the far plane, the opposite of the view-frame
  convention. The reversed projection is built directly in `f64` from the frame's finite near and
  far distances. The depth target is `Depth32Float`, cleared to 0 and tested with *greater*
  (meshes) or *greater or equal* (lines).
- Orthographic views use the frame's depth range intersected with the depth range of the scene's
  bounding spheres plus a 1 % margin, and take translations relative to the eye moved forward to
  the near end of that range, so depth is computed from small numbers.
- Bound: for an item whose vertices are within 500 m of its origin on each axis (items larger
  than 1 km are tiled by CORE-GPU-005), with the eye inside the item's extent, the camera-relative
  position of every vertex is within 0.1 mm of its exact value at any distance from the model
  origin up to 10,000 km.

### Meshes

Meshes are drawn as indexed, instanced triangles with a base vertex. Normals transform by the
cofactor matrix of each instance's linear part; a mesh without normals is shaded with its face
normal. The phase-0 lighting model is a headlight: the color is the base color times
`ambient + (1 − ambient) · |n · l|`, where `l` points toward the eye along the view direction and
`ambient` is a render setting (0.25 by default). Lighting is two-sided; back faces (clockwise in
the image) are culled when `cull_back_faces` is set. The fragment stage calls one shading function
whose lit flag and face opacity the shading mode sets.

### Lines

Each segment is an instanced quad expanded in the vertex shader in screen space; the fragment
shader computes the signed distance to the segment's region and its coverage.

- **Widths** are full widths. A screen width is in pixels and does not change with zoom. A paper
  width is in paper millimetres: it is multiplied by the plot scale (model units per paper
  millimetre, a render setting) and converted to pixels through the view, evaluated at the
  segment's start for perspective views. Widths below one pixel draw one pixel wide; screen widths
  above `max_line_width_px` are rejected.
- **Caps** apply to ends that join nothing: round adds a half disk, square extends the segment by
  half the width.
- **Joins** apply where two segments of a polyline meet: round adds a disk; miter splits the two
  segments at the bisector of the corner, which extends their outer edges until they meet; when
  the miter would reach farther than the miter limit (a multiple of half the width, at least 1,
  default 4) from the vertex, or for bevel joins, the corner is cut by the line through the two
  outer corners. A path that reverses direction joins round.
- **Zero length.** A segment whose ends coincide in the image draws a disk with round caps and an
  image-aligned square with square caps.
- **Near plane.** A segment crossing the near plane is clipped in clip space before expansion and
  its cut end has no cap; a segment entirely behind it draws nothing.
- **Smoothing.** With `line_smoothing` (the default) coverage falls off over one pixel at the
  edge and blends with the color's alpha; without it a pixel is covered when its center is inside.
- Lines are depth-tested against meshes and do not write depth.

### Headless rendering

`Renderer::render_to_image(scene, frame, width, height, settings)` renders the passes of the
settings' shading mode into an offscreen `Rgba8UnormSrgb` target and returns an `Image` of rows top
to bottom, four bytes per pixel, sRGB-encoded with straight alpha. With `sample_count` 4 the
target is multisampled and resolved; adapters without four-sample support render one sample.
Readback strips the row padding of the copy. Sides are limited by `max_image_side` and the
device's texture limit. Results are deterministic for a given adapter.

### Shading modes and edges

The shading mode is a render setting (`RenderSettings::mode`, default `Shaded`). Modes, highlights
and overlays are uploaded per frame in a uniform buffer and buffers created for the frame, never in
the scene's pools: switching mode writes no scene buffer and needs no re-extraction. A frame draws
faces and edges as the mode says, then lines (the same in every mode), the highlight tint, the
highlight outline and the overlay; a pass with nothing to draw is skipped.

| Mode | Faces | Edges |
|---|---|---|
| `Wireframe` | none | instance color, no hidden-line removal |
| `HiddenLine` | depth only (the background shows) | instance color, hidden behind faces |
| `Shaded` | headlight-lit, depth-written | none |
| `ShadedWithEdges` | headlight-lit, depth-written | `edge_color` (black by default) |
| `ConsistentColors` | unlit base color, depth-written | none |
| `XRay` | headlight-lit at `xray_opacity` (0.3 by default), blended in item order without depth test or writes | instance color |

Edges are the feature edges of each geometry, extracted once when it is added: boundary edges
(one face), non-manifold edges (more than two faces) and edges whose two faces meet at more than
`FEATURE_EDGE_ANGLE` (30 degrees). Vertices at exactly equal positions are welded first, so meshes
with split normals give the same edges; faces whose normal is zero or not finite are skipped.
Edges are drawn for every instance from the mesh's own vertex and instance data, as one-pixel
lines (wider and silhouette edges are CORE-GPU-017). Edge depth is moved toward the eye by a
relative 2⁻¹² of its reversed-Z value, never past the near plane, so edges are not hidden by the
faces they bound.

### Identifier picking

`Renderer::pick(scene, frame, width, height, settings, region)` renders only the region of
interest of a `width × height` image: a clip-space remapping (`camera::region_window`) shows the
region's pixels in an `R32Uint` target of the region's size at the same pixel scale, with its own
depth target.

- Mesh faces are drawn with depth writes and write `instance index + 1`; line segments are drawn
  after them, depth-tested without depth writes, widened by `tolerance_px` on each side, covering
  a pixel when its center is inside, and write `segment index + 1` with bit 31 set. Index values
  are positions in the instance pools; a pool too large for 31 bits fails with
  `gpu.limit.exceeded`.
- Values map back through the items' instance ranges to a `PickHit`: the `ItemKey` and the
  element, which is the instance index within a mesh item or the segment index (in path order)
  within a line item. The result is the sorted, distinct hits visible in the region; a pixel pick
  is a one-pixel region and returns at most one hit. Window and crossing selection rules are the
  caller's.
- A pixel shows the nearest face or, where no face is in front, the last drawn line, so a line
  wins over the face it lies on. Faces are picked in every shading mode; back faces are skipped
  when `cull_back_faces` is set.
- A pick counts as a frame for write accounting (it writes translations when the eye moved).
- On the pinned software adapter a pixel pick in a small scene takes about 1.5 ms and a 96 × 96
  rectangle about 2.5 ms; the tests bound only gross regressions.

### Highlights

`Renderer::render_with(scene, frame, width, height, settings, extras)` takes `FrameExtras`: the
hovered object, the selected objects and an overlay. Unknown objects are ignored.

- **Tint.** The items of selected objects, then of the hovered object, are drawn again in the
  selection or hover color at alpha `tint` (0.35 by default): faces depth-tested *greater or
  equal* against the frame's depth with an invariant position, so only visible faces are tinted (in
  `Wireframe` the edges instead), and lines with their coverage.
- **Outline.** Before the frame, the items of selected objects (mask value 1) and then of the
  hovered object (value 2) are drawn into an `R8Uint` mask of the image size without depth
  testing. A full-image pass then gives every pixel outside the mask that has a masked pixel within
  `outline_px` whole pixels (Euclidean; 0–8, 2 by default, 0 for none) the hover color when a
  hovered pixel is within reach and the selection color otherwise. The outline therefore follows
  whole objects, including occluded parts, in every mode.

### Overlays

An `Overlay` is a list of primitives passed in `FrameExtras`; it is not part of the scene. Every
primitive is resolved on the CPU in `f64` to pixel positions (x right, y down) and drawn after the
outline without depth testing, so it is above the scene regardless of depth and is not clipped by
the far plane or (later) section planes.

- Points are `Anchor::World` (projected through the view; perspective segments are clipped at the
  near plane, and a marker or fill with a point behind it is skipped) or `Anchor::Screen` pixels.
  Segments are clipped to a guard band one image side wide around the image.
- Primitives: `Polyline` (constant pixel width, drawn as separate segments with round ends),
  `Fill` (a simple polygon triangulated by ear clipping in pixels; a self-intersecting one draws the
  ears found), `Marker` (square, circle, triangle or cross of constant pixel size, filled or
  outlined), `Grid` (two families of lines in a plane with optional major lines) and `AxisTriad`
  (the world axes from a pixel position, farthest first). These cover grips, snap markers, tracking
  lines, rubber bands, previews and the grid. A view cube needs text labels and is left to the UI
  kit.
- Fills are drawn before lines; within each kind, primitives draw in list order.
- Segments and fill vertices together are limited by `max_segments_per_item`; widths by
  `max_line_width_px`.

### Navigation

`arc_gpu::navigation` provides pure `f64` functions from a `ViewFrame` to a new one, for
orthographic and perspective frames: `orbit` about a pivot (yaw about the up hint, pitch about the
right direction, the view direction kept at least `tolerance.angular` from the up hint), `pan` (an
anchor point follows the pointer by a pixel offset), `zoom_at` (the point under the pointer keeps
its image position), `zoom_to_sphere` (zoom extents and zoom to selection with
`Scene::bounding_sphere`) and `interpolate` (eased transitions between named views with quaternion
slerp that never pass through a degenerate frame). The module documentation states each rule;
property tests check the invariants with coordinates up to 10⁷ from the origin.

### Incremental extraction

`arc_gpu::extract` turns document objects into render items using `arc-doc` snapshots, revisions
and committed-event changes.

- An `ExtractorRegistry` maps an object type key to an extractor, a function
  `(snapshot, object, cancel token) → Extraction`. An `Extraction` is the object's complete list
  of items plus the geometries they use: `Extraction::add_geometry` returns a handle valid only
  in that extraction. An object whose type has no extractor has no items.
- An `ExtractionDriver` owns 1 to 16 worker threads (std threads, no external pool).
  `submit(snapshot, objects)` gives each object the next sequence number; the job's **stamp** is
  `(snapshot revision, sequence)`. A newer submission for an object replaces its queued job and
  cancels its running one; a cancelled job's result is dropped, so the queue holds at most one
  job per object. `submit_changes(snapshot, changes)` takes the changes of a committed event;
  for "everything changed" it queues every object of the snapshot and every object whose last
  result had items, so deleted objects are removed.
- A job resolves its object in the snapshot. An object that is not there yields `Removed`; an
  extractor error yields `Failed` with that error; a panic is caught and yields
  `gpu.extractor.panicked`. `take_results` returns the results delivered so far; per object a
  newer result replaces older ones, except that a failure never replaces an older success.
  Dropping the driver cancels its jobs and joins the workers after their current extractor.
- `Scene::apply_extractions` applies results in any order. A result changes the scene only when
  its stamp is greater than the stamp last applied for its object (`Scene::applied_stamp`), so
  a late result of an older revision never replaces a newer one, and a removal keeps its stamp
  so an older result cannot bring a deleted object back. New items replace the object's items
  after their geometries are uploaded; the geometries the object owned before are then removed.
  A failed result, or one the scene rejects (invalid items, an unknown local geometry handle,
  more geometries than `max_items_per_object`, a device error while uploading), leaves the
  object's items and stamp as they were. The `ApplyReport` counts applied and stale results and
  lists the failures. Objects that receive items through extraction are not changed with
  `Scene::update` directly.

### Viewports and redraw

A `Viewport` owns a render thread, a scene and a renderer; `Viewport::new(gpu, config)` starts
the thread, which draws the first frame.

- **Inputs.** `set_camera`, `resize`, `set_settings`, `set_highlight` (hovered object and the
  selection, compared as a set), `set_overlay`, `update_scene` (extraction results) and
  `request_progressive_frame` record the input in a mailbox under a short lock and wake the
  thread; they never wait for a frame. The mailbox holds the latest value of each input and, per
  object, the pending result with the greatest stamp (failed results are skipped), so it stays
  bounded and a camera sent while a frame renders replaces the pending one. Sizes, settings,
  selections and pending objects are checked against the limits at the call.
- **On-demand redraw.** The thread draws only for a `DirtyReasons` flag: camera, size,
  settings, highlight or overlay set to a different value, extraction results that changed the
  scene, or a requested progressive frame. A frame takes every change pending when it starts.
  Otherwise the thread waits on a condition variable and submits no GPU work.
  `Viewport::frames` counts frames, `Viewport::dirty` reports pending reasons and each `Frame`
  carries the reasons it was drawn for.
- **Frames.** Each frame renders into its own `Rgba8UnormSrgb` texture usable as a texture
  binding and copy source; the `Frame` holds the texture and a view for a compositor on the same
  device and, when `ViewportConfig::readback` is set, the read-back `Image`. Without readback the
  thread waits for the device to finish each frame, so at most one frame is in flight.
  `latest_frame` returns the newest frame without waiting; `wait_frame` and `sync` (every
  earlier call handled) wait with a caller-given bound and fail with `gpu.timeout`.
- **Jobs on the scene.** `with_scene(timeout, job)` runs `job` on the render thread with the
  renderer, the scene (after every extraction result queued before the call is applied) and the
  current render settings, between frames, and waits at most `timeout` for its result
  (`gpu.timeout` otherwise; the job still runs later and its result is dropped). Picking a
  viewport's scene and reading its bounding sphere go through it. A job draws nothing by
  itself.
- **Errors.** An error on the thread (a rejected extraction result, a device error) is returned
  by the next call, which still records its input. After `gpu.device.lost` the thread stops and
  every call returns that code; a thread that ends otherwise makes calls return
  `gpu.thread.failed`. Dropping the viewport asks the thread to stop and waits for it at most 10
  seconds (`JOIN_TIMEOUT`); a thread still finishing a frame after that is detached and exits
  after the frame.

### Reference images

Reference images live in `crates/arc-gpu/tests/reference/` and are compared only on the pinned
software adapter, Mesa lavapipe on Vulkan, which the Linux CI job installs. The metric blurs both
images with a 3×3 box filter and takes the mean and the maximum absolute channel difference
(0–255); each test states both thresholds. On other adapters each test's analytic checks (colors
of flat regions, depth order, measured widths, coverage masks, areas) are the assertions. A failed
comparison writes the rendered and difference images to `target/tmp/arc-gpu-image-diffs/`.
`ARC_UPDATE_REFERENCES=1 cargo nextest run -p arc-gpu` rewrites the references on the pinned
adapter; changed references are reviewed in the commit that changes them. The rendering tests fail
when no adapter is available.

### Error codes

The codes `arc-gpu` owns. A code is never reused for a different meaning; messages are in
`locales/en-US/arc-gpu.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `gpu.adapter.not_found` | `not_found` | No adapter matches the settings, or none is available |
| `gpu.settings.invalid` | `invalid_input` | A settings value is out of range: a sample count other than 1 or 4, a negative or non-finite ambient term or plot scale, an X-ray opacity or tint outside [0, 1], an outline wider than 8 pixels, a non-finite color, or a limit of zero |
| `gpu.device.request_failed` | `failed_operation` | The adapter refused to create a device with the required features and limits |
| `gpu.device.lost` | `failed_operation` | The device was lost or destroyed; the scene and renderer must be recreated on a new device |
| `gpu.out_of_memory` | `limit_exceeded` | The device ran out of memory while creating a resource or executing work |
| `gpu.validation` | `internal` | The device reported a validation error: a defect in Arc |
| `gpu.timeout` | `failed_operation` | A wait for the device exceeded its bound |
| `gpu.readback.failed` | `failed_operation` | Reading a rendered image back from the device failed |
| `gpu.limit.exceeded` | `limit_exceeded` | A count or size exceeds the scene limits or the device limits |
| `gpu.value.not_finite` | `invalid_input` | A coordinate, origin, transform, color or width is NaN or infinite |
| `gpu.item.invalid` | `invalid_input` | A render item is empty (no instances, no points) or a line width or miter limit is out of range |
| `gpu.geometry.invalid` | `invalid_input` | A mesh has no triangles |
| `gpu.geometry.not_found` | `not_found` | An item or call refers to a geometry handle the scene does not hold |
| `gpu.geometry.in_use` | `conflict` | A geometry is removed while render items still refer to it |
| `gpu.precision.insufficient` | `failed_operation` | A camera-relative translation or projection cannot be represented in `f32` |
| `gpu.pick.invalid` | `invalid_input` | A pick region is empty or outside the image, or the pick tolerance is negative, not finite or above the line width limit |
| `gpu.navigation.invalid` | `invalid_input` | A navigation parameter is out of range: a non-finite value, a zero or negative zoom factor or image size, a negative radius, or a pan anchor at or behind the eye |
| `gpu.extractor.panicked` | `internal` | A product extractor panicked; the object keeps its previous items |
| `gpu.thread.failed` | `failed_operation` | A worker or render thread could not be started, or the render thread stopped unexpectedly |
