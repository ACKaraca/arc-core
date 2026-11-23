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
  into units of a fixed stride (24, 4, 80 and 128 bytes). Allocation is first-fit from a sorted,
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
color (4 `f32`), then material, flags (bits 0 to 3 paint and depth flags of drawings, bit 4
the closed-solid flag of sections, bits 24 to 27 the category of display overrides), the
object's identifier slot (kept for the picking pass) and a reserved `u32`. A line segment is its start, end, previous and next points (3 `f32` each;
the neighbors equal the ends when absent), the color, the width at the start and the miter limit
(`f32`), the style bits (bit 0 square cap; bits 1–2 join: 0 round, 1 miter, 2 bevel; bit 3 has a
previous segment; bit 4 has a next segment; bit 5 paper width; bit 6 pixel positions, for
overlays; bit 7 the width is a lineweight; bit 8 dashed; bit 9 widths in item units), the
identifier slot, then the width at the end, the pattern positions at the start and end and the
pattern period (`f32`), and four `(start, end)` pattern intervals (8 `f32`, unused ones −1).

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
the image) are culled when `cull_back_faces` is set. Items whose instances all have a positive
determinant are culled by the rasterizer. An item with a mirrored instance (a negative determinant
reverses the winding on the screen) is drawn with a variant that does not cull in the rasterizer
and discards back faces in its fragment stage, so the mirrored instance keeps its own front faces;
the items of one pass are split by this rule, and the variant is used only under culling. Caps
disable culling. The fragment stage calls one shading function whose lit flag and face opacity
the shading mode sets.

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
highlight outline and the overlay; a pass with nothing to draw is skipped. The per-frame
view state of overrides, sections and manipulators is described in
[Overrides, sections, levels of detail, tiles and manipulators](#overrides-sections-levels-of-detail-tiles-and-manipulators).

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
lines. Silhouette edges are described in [Edge display](#edge-display-core-gpu-017). Feature
edges come from `arc_mesh::features::feature_edges`. Edge depth is moved toward the eye by a
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
  when `cull_back_faces` is set, by the same rule as drawing (a mirrored item is culled per
  fragment; caps disable culling).
- A pick counts as a frame for write accounting (it writes translations when the eye moved).
- A pick culls items against the view before it draws (see [Culling](#culling-core-gpu-018)),
  and widens the culling margin of lines by the pick tolerance.
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
the far plane or by section planes.

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
  scene, a requested progressive frame, or full quality returning after navigation. A frame
  takes every change pending when it starts.
  Otherwise the thread waits on a condition variable and submits no GPU work.
  `Viewport::frames` counts frames, `Viewport::dirty` reports pending reasons and each `Frame`
  carries the reasons it was drawn for.
- **Adaptive quality.** While the camera moves, a viewport draws at navigation quality; when the
  camera has been still for the settle time, it draws one frame at full quality with the reason
  `quality` (see [Adaptive quality](#adaptive-quality-core-gpu-021)).
- **Frames.** Each frame renders into its own `Rgba8UnormSrgb` texture usable as a texture
  binding and copy source; the `Frame` holds the texture and a view for a compositor on the same
  device and, when `ViewportConfig::readback` is set, the read-back `Image`. Without readback the
  thread waits for the device to finish each frame, so at most one frame is in flight.
  `latest_frame` returns the newest frame without waiting; `wait_frame` and `sync` (every
  earlier call handled) wait with a caller-given bound and fail with `gpu.timeout`; `caught_up`
  says without waiting whether every call so far is handled (false while a frame is drawn, when
  `dirty` is already empty).
- **Jobs on the scene.** `with_scene(timeout, job)` runs `job` on the render thread with the
  renderer, the scene (after every extraction result queued before the call is applied) and the
  render settings of the frame drawn with it (lowered while the camera moves, see
  [Adaptive quality](#adaptive-quality-core-gpu-021)), between frames, and waits at most `timeout`
  for its result
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
| `gpu.settings.invalid` | `invalid_input` | A settings value is out of range: a sample count other than 1 or 4, a negative or non-finite ambient term or plot scale, an X-ray opacity or tint outside [0, 1], an outline wider than 8 pixels, a curve tolerance outside [0.01, 16] pixels, a negative or non-finite level-of-detail budget, a non-finite color, or a limit of zero; display overrides with an opacity outside [0, 1] or a category above 15; a section with more than six planes, a zero plane normal, an inverted box or a cap hatch with a spacing outside (0, 4096] pixels or a width that is not positive and below the spacing; a manipulator with a zero axis, a size outside (0, 4096] pixels or a width that is not positive; a tile bound that is not positive |
| `gpu.device.request_failed` | `failed_operation` | The adapter refused to create a device with the required features and limits |
| `gpu.device.lost` | `failed_operation` | The device was lost or destroyed; the scene and renderer must be recreated on a new device |
| `gpu.out_of_memory` | `limit_exceeded` | The device ran out of memory while creating a resource or executing work |
| `gpu.validation` | `internal` | The device reported a validation error: a defect in Arc |
| `gpu.timeout` | `failed_operation` | A wait for the device exceeded its bound |
| `gpu.readback.failed` | `failed_operation` | Reading a rendered image back from the device failed |
| `gpu.limit.exceeded` | `limit_exceeded` | A count or size exceeds the scene limits or the device limits |
| `gpu.value.not_finite` | `invalid_input` | A coordinate, origin, transform, color, width or hatch angle is NaN or infinite |
| `gpu.item.invalid` | `invalid_input` | A render item is empty (no instances, no points) or a line width or miter limit is out of range, or a curve item has an unbounded curve, an invalid dash pattern or an empty clip rectangle |
| `gpu.geometry.invalid` | `invalid_input` | A mesh has no triangles |
| `gpu.geometry.not_found` | `not_found` | An item or call refers to a geometry handle the scene does not hold |
| `gpu.geometry.in_use` | `conflict` | A geometry is removed while render items still refer to it |
| `gpu.precision.insufficient` | `failed_operation` | A camera-relative translation or projection cannot be represented in `f32` |
| `gpu.pick.invalid` | `invalid_input` | A pick region is empty or outside the image, or the pick tolerance is negative, not finite or above the line width limit |
| `gpu.navigation.invalid` | `invalid_input` | A navigation parameter is out of range: a non-finite value, a zero or negative zoom factor or image size, a negative radius, or a pan anchor at or behind the eye |
| `gpu.extractor.panicked` | `internal` | A product extractor panicked; the object keeps its previous items |
| `gpu.thread.failed` | `failed_operation` | A worker or render thread could not be started, or the render thread stopped unexpectedly |

## Overrides, sections, levels of detail, tiles and manipulators

The sixth part implements CORE-GPU-005, 016, 017, 019, 024 and 026, and the seventh part
CORE-GPU-018 (CPU path only) and 021. Overrides, sections and manipulators are per-frame inputs
(`FrameExtras::overrides`, `section` and `manipulator`, and `Viewport::set_overrides`,
`set_section` and `set_manipulator`, each a dirty reason of the viewport); they are uploaded in
the view uniform (960 bytes) and per-frame bind groups and write no scene buffer.
`Renderer::pick_with` takes the same extras, so picks follow what is drawn. CORE-GPU-020 is not
part of these steps.

### Tiled large items (CORE-GPU-005)

- `tile::split_mesh(mesh, max_extent)` partitions the triangles by recursive median bisection of
  their centroids along the longest axis of the bounds, until the bounds of a set are at most
  `max_extent` on every axis (`TILE_SIZE`, 1,000 m). Each tile is a mesh with exactly copied
  vertices (normals kept) and an origin at the center of its bounds, so every vertex is within
  500 m of its origin and the `f32` error stays below 0.1 mm, also 10^7 m from the model origin.
  The tiles hold every input triangle exactly once. A triangle is never cut: a triangle larger
  than the bound is a tile of its own and exceeds the bound; this is the one exception.
- `Scene::add_tiled_geometry(mesh)` and `Extraction::add_tiled_geometry(&mesh)` upload the tiles
  and return `TiledGeometry { geometry, origin }`; `TiledGeometry::item(instances)` makes the
  mesh item of a tile. Instance transforms are relative to the tile origin, so the identity
  places the mesh where it was authored. A mesh within the bound is one tile.
- `tile::split_lines(paths, max_extent, max_segments)` splits polylines into runs with their own
  origins. A segment longer than half the bound is divided into equal parts first; the runs of a
  polyline share their end points. The divided segments of all paths of one call are bounded by
  `max_segments` (the caller passes `SceneLimits::max_segments_per_item`); the parts of a segment
  are counted before any point is built, so a path of 2 × 10^12 units at a 1 km bound fails with
  `gpu.limit.exceeded` at once. The run bounds grow by each point, so the split takes time linear in
  the points. A join at a run boundary is two caps, so miter and bevel joins
  are not exact at a boundary vertex. Open item: the dash phase is also lost at a run boundary,
  because each run is a path of its own that starts its pattern at 0, so a dashed split line
  does not keep its pattern across tiles.
- Tiling is explicit: `Scene::add_geometry` and `Scene::update` do not split what they are
  given. A producer calls the functions above for items larger than the bound.

### Display overrides (CORE-GPU-016)

- `DisplayOverrides` holds styles by object and by category, an optional isolation and the
  ghost opacity (0.15 by default). `OverrideStyle` has an optional color, an opacity, and the
  flags `ghost` and `hidden`.
- **Objects** are resolved on the CPU: the draws of hidden objects (and of objects an isolation
  hides) are skipped in every pass, so they are not shaded, tinted, outlined or picked, and
  items are drawn grouped by their object style with one bind group each.
- **Categories** are resolved on the GPU: an instance carries a category 0 to 15 in the bits 24
  to 27 of its flags (`category_flags`); the vertex shader looks the style up in a table of 16
  entries in the view uniform. One draw can therefore mix categories. An object's own style
  replaces the style of its instances' categories, except for hiding: an instance is hidden when
  its category or the object's style hides it.
- **Rules** (by property, layer or type) are evaluated by the caller into object lists or a
  category; the renderer has no access to document properties.
- **Isolation** shows the listed objects; the others are hidden or ghosted (`Others`).
  **Ghosts** are drawn at the ghost opacity and are not picked. An object outside the isolation
  that has its own style keeps that style (color and opacity) and is ghosted as well.
- **Effects.** A color replaces the instance color of faces and of edges drawn in the instance
  color; the alpha is kept. An opacity below 1 blends faces: in the modes with colored opaque
  faces (shaded, shaded with edges, consistent colors) such faces are discarded from the opaque
  pass and drawn again in a blended pass without depth writes, after the opaque faces and in
  item order; the X-ray mode multiplies its own opacity; in the hidden-line mode a translucent
  face does not hide lines. Edges blend by the same opacity. Hiding and isolation apply to line
  items as well; color, opacity and ghosting do not.
- Overlays are not affected by overrides.

### Edge display (CORE-GPU-017)

- **Feature edges** are boundary, sharp (more than `FEATURE_EDGE_ANGLE`) and non-manifold edges
  from `arc_mesh::features::feature_edges`, one pixel wide.
- **Silhouette edges** are enabled by `SceneLimits::silhouette_edges` (derive the data when a
  geometry is added) and `RenderSettings::silhouette_edges` (draw it, in the modes that draw
  edges over faces: hidden line, shaded with edges and X-ray). The data is, for every smooth
  edge between exactly two faces (creases and boundaries are feature edges), two records of an
  end point and the unit normals of both faces, kept in the vertex pool: 72 bytes, three vertex
  units, per edge. It is counted as vertex memory and is off by default.
- An edge is drawn where its two faces face opposite ways as seen from the eye, tested for each
  fragment against the eye direction (the view direction for orthographic views, the direction
  to the fragment for perspective views), so perspective contours are exact. Faces are assumed
  consistently wound; a mesh with flipped faces can miss contours. Silhouette edges use the edge
  color of the mode, the depth allowance of feature edges and the instance transform.
- Edges are one pixel wide; wider edges are a later step.

### Levels of detail (CORE-GPU-019)

- `Scene::add_lod_geometry(&chain, origin)` and `Extraction::add_lod_geometry(&chain, origin)`
  upload every level of an `arc_mesh::lod::LodChain` (each with its feature and silhouette
  data) and return the handle of level 0, which items use. The scene keeps its own copy of the
  chain. Removing the handle removes all levels; a level cannot be removed alone.
- Before each frame and pick the scene calls `LodChain::select` for every item whose geometry
  has levels, with the budget `RenderSettings::lod_budget_px` (1 pixel by default; 0 always
  draws level 0) and the `ViewParams` of the item. The distance is the item's nearest depth in
  front of the near plane (its origin depth minus its radius, at least the near distance), and
  one for orthographic views. The projection scale is the view's pixels per unit at unit
  distance times the largest scale of the item's instance transforms. One level is chosen for
  all instances of an item. `Scene::lod_level(key)` reports the level of the last frame.
- Selection is monotonic: `LodChain::select` never returns a finer level for a smaller
  projected size, and the projected size only decreases with distance.

### Culling (CORE-GPU-018)

Only the CPU path is implemented. Culling on the GPU with hierarchical depth and multi-draw
indirect is not implemented; every frame and pick uses the CPU path below.

- **Bounds.** Each item has a box relative to its origin, computed when the item is added or
  extracted. A mesh item covers the transformed box of each of its instances over every level of
  detail of its geometry, so the bound holds whichever level is drawn. A line or curve item covers
  its path vertices. The boxes are computed from the `f32` values the GPU holds.
- **Test.** Before each frame and each pick, after the curves and levels of detail are refreshed,
  an item is culled when one clip-space half-space of the view is negative at all eight corners of
  its box. The half-spaces are those of the reversed projection times the view rotation, computed
  in `f64` relative to the translation origin. A linear function that is negative at the corners
  of a box is negative on the whole box, so the test never removes an item with a point inside the
  view; an item partly inside is always drawn.
- **Margins.** The side planes are widened by the pixels an item draws past its vertices: 2 pixels
  for a mesh (its feature and silhouette edges are one pixel wide), and for a line item half its
  width (at least one pixel) times the larger of 1 and its miter limit, plus one pixel of coverage. A pick adds
  its tolerance to the margin. The near and far planes are exact. An item with a paper width or an
  item-unit width has a width that depends on the view, so no pixel margin bounds it and it is
  never culled. A non-finite value keeps the item.
- **What is still drawn for every item.** The outline mask of the highlights (CORE-GPU-023) and
  the tint of selected and hovered objects use all items of those objects, because an item outside
  the view can outline a pixel inside it and a tinted item outside the view draws nothing. The
  camera-relative translations of culled items are still written, so a later frame that shows them
  needs no rewrite.
- **Counts.** `Scene::cull_stats` gives the item count and the visible count of the last frame or
  pick, and `Scene::item_visible` the flag of one item. `RenderSettings::cull_items` (on by
  default) switches the culling off; with it off every item is visible. A frame with culling is
  pixel-identical to the same frame without it.

### Adaptive quality (CORE-GPU-021)

- `arc_gpu::quality::AdaptiveQuality` decides the quality of a frame from the camera's motion. Its
  methods take the time `now` as a `Duration` from an origin the caller chooses; the type never
  reads the system clock. `camera_moved(now)` records a camera change, `is_navigating(now)` tells
  whether the camera is moving (its last change is less than the settle time before `now`),
  `settled_at()` gives the time full quality returns, and `apply(settings, now)` gives the
  settings a frame is drawn with.
- **Lowered effects.** While the camera moves, `apply` sets `sample_count` to 1, turns
  `silhouette_edges` off and multiplies `lod_budget_px` by `NAVIGATION_LOD_FACTOR` (4), so coarser
  levels of detail are chosen; a budget of 0 stays 0 and keeps the finest level. Every other
  setting is unchanged, so overrides, sections and highlights look the same.
- **Viewport.** A camera change after a viewport's first frame is navigation. The render thread
  draws each change at navigation quality. Once the camera has been still for the settle time it
  draws one more frame at full quality, with the reason `DirtyReasons::quality`; the thread waits
  for that time with a timeout, not by polling. The clock of the thread is the time since it
  started. The settle time is `ViewportConfig::settle` (default `SETTLE_TIME`, 250 ms), and the
  clock is `ViewportConfig::clock`: `None` reads the time since the thread started, and a clock
  a caller injects is read by the thread instead. The viewport's tests inject a clock they advance
  by hand and wake the thread after each advance, and set the settle time to a value no test
  reaches where no settle frame is wanted; no test depends on the elapsed real time.
- **Rest.** A frame drawn at rest is the same before and after a navigation episode (test in
  `tests/adaptive_quality.rs`).

### Section planes and box (CORE-GPU-024)

- A `Section` has up to six `SectionPlane`s (a point and a normal; the side the normal points to
  is removed), one axis-aligned `SectionBox` (everything outside is removed) and an optional
  cap color. Planes and box are converted in `f64` to offsets from the frame's translation
  origin, so they hold 10^7 m from the model origin.
- Clipping is a `discard` in the fragment shaders of faces, edges, tint, mask and identifier
  passes, never a clip distance. Picking respects the section. Line items are not clipped;
  overlays and manipulators are never clipped.
- **Caps.** An instance with the flag `FLAG_SOLID` is a closed solid. While the section has a cap
  color, the back faces of such an instance that remain visible after clipping are drawn unlit
  in the cap color, which fills the cut. Back-face culling is off while a section has a cap
  color. Caps are drawn in the modes with colored opaque faces. The cap has the depth of the
  back face behind the cut, not of the cutting plane, so objects inside a cut solid show
  through it. A mirrored instance (a negative determinant of its linear part) winds its triangles
  the other way, so the shader takes a face as front when its winding is front for a positive
  determinant and back otherwise; a mirrored solid is capped at its cut and shaded outside. A stencil cap on
  the cutting plane needs a stencil format outside the default features and was not built.
- **Hatch.** A `Section` may carry a `CapHatch` (`hatch`): parallel lines in a second color drawn
  over the caps. The lines are in screen space, so their spacing and width are in pixels and their
  density follows the view. `angle_deg` is the direction of the lines, counterclockwise as seen on
  the screen; `spacing_px` is in (0, 4096] and `width_px` is above 0 and below the spacing. The
  alpha of the hatch color is ignored. The fragment shader of the cap takes the position of the
  fragment in pixels, projects it on the normal of the lines and draws a line where the fractional
  part of that quotient by the spacing is below the width over the spacing; no derivatives are
  used. A hatch without caps has no effect and is still validated. The hatch is part of the view
  uniform (two `vec4` after the section box), so the uniform is 960 bytes.

### Manipulators (CORE-GPU-026)

- A `Manipulator` (move, rotate or scale; origin; three axis directions; size, width and colors
  in pixels; an active axis) is resolved on the CPU in `f64` and drawn through the overlay, so
  it is above the scene and not clipped by sections. The pixel size is converted to model units
  at the depth of the origin (`manipulator::world_per_pixel`), so handles keep their size on the
  screen at any camera distance and in both projections.
- Move and scale draw a line per axis with an arrowhead or a square at the tip; rotate draws a
  48-segment ring per axis in the plane of the other two axes.
- `Manipulator::hit_test(frame, size, pixel, tolerance_px)` returns the nearest axis whose
  projected line or ring is within the tolerance of the pixel (the lower axis on a tie). Parts
  behind the near plane are not drawn and not hit. Plane handles and a uniform scale handle
  are not provided.

## Drafting display as implemented

The fourth part implements CORE-GPU-009, CORE-GPU-010 and CORE-GPU-013 for the drafting
primitives that exist in `arc-draft` (points, curves, polylines with widths, fills, wipeout
masks and block references). The fifth part adds hatches (CORE-GPU-011) and outline text
(CORE-GPU-012), described in [Hatches and fills](#hatches-and-fills-core-gpu-011) and
[Outline text](#outline-text-core-gpu-012).

### Curve items (CORE-GPU-010)

A `RenderItem::Curves` (`CurveItem`) holds exact `arc-geom` curves (segments, circles, arcs,
ellipses, elliptical arcs, polylines; rays and lines are `gpu.item.invalid`) and points relative
to an `f64` origin, in the plane `z = origin.z`, with a line style, a color, an optional
`DashPattern` and an optional clip rectangle. The scene keeps the curves and flattens them into
line segments:

- The chord tolerance is `curve_tolerance_px / (2 · s₀)` item units (`RenderSettings::
  curve_tolerance_px`, 0.5 by default, 0.01 to 16), where `s₀` is the zoom in pixels per item
  unit the item was flattened for; arcs and ellipses also turn at most `CURVE_ANGLE_STEP` (45°)
  per segment.
- Before each frame and pick, every curve item compares its current zoom `s` (orthographic: the
  view's pixels per unit; perspective: at the item's nearest depth in front of the near plane)
  with `s₀`. It is flattened again only when `s / s₀` leaves `[1/2, 2]` or the tolerance setting
  changed, and only its segments are written (counted as line-segment writes). Until then the
  chord error is at most `curve_tolerance_px · s / (2 · s₀) ≤ curve_tolerance_px` pixels.
  Panning an orthographic view never changes `s`, so it never flattens again. Items without
  arcs or ellipses never depend on the zoom.
- A new item is flattened for the zoom of the last orthographic frame, or, before the first
  frame, for its extent over 1000 pixels.
- When the tolerance needs more segments than `max_segments_per_item` or a finer resolution than
  `f64` gives at the curve, the chord is doubled until the flattening succeeds, so at extreme
  zooms the error may exceed the tolerance instead of failing the frame. A flattening that still
  fails keeps the previous segments until the zoom leaves the band again. Validation checks that
  the coarsest flattening fits the segment limit.
- `Scene::flattening(key)` reports the zoom and tolerance an item is flattened for;
  `Scene::path_of(hit)` maps a picked segment to the path of its item.

### Widths and lineweight display

- Polyline widths (in item units, linear along each segment, arcs included) replace the style
  width of the segments that have them: they are converted to pixels through the view, with a
  one-pixel minimum, and free ends are cut flat.
- A style whose `lineweight` flag is set draws one pixel wide while `RenderSettings::lineweights`
  is off. The switch is a per-frame uniform: toggling it writes no scene buffer and needs no
  re-extraction.

### Dash patterns (CORE-GPU-009)

- A `DashPattern` is a period and at most `MAX_DASH_INTERVALS` (4) drawn intervals
  `(start, end)` within it, in item units; an interval of length zero is a dot. Every path has a
  starting phase and may restart the pattern at position 0 at each polyline vertex after the
  first.
- Every flattened vertex carries the exact arc length along its curve (arcs and circles are
  divided into equal parts; ellipses are integrated between vertices). Each segment carries the
  pattern position at its start, reduced to one period in `f64`, and at its end.
- The fragment shader interpolates the position linearly in the image between the segment's
  ends, so dashes continue across vertices and stay fixed in the document while the view pans
  (exact for orthographic views). A fragment is drawn where the position is inside a dash (ends
  cut flat, antialiased over one pixel) or within the half width of a dot. Picking and the
  highlight mask ignore the pattern and cover the whole path.
- Division of labor with `arc_draft::linetype::dashes`: the viewport evaluates patterns with at
  most four drawn elements on the GPU; display lists for the viewport are therefore built without
  dash generation. Longer patterns and plot output use the CPU pieces. Both start the pattern at
  the start of the curve with the same scale, so the rendered dash ends agree with the CPU ends
  to within a pixel.

### The shared extractor (CORE-GPU-013)

With the feature `draft` (off by default, so products that draw no drawings do not depend on
`arc-draft`), the module `arc_gpu::draft` maps `arc-draft` display lists to render items.
`display_options` turns dash generation off (`max_dashes` 0), so strokes are whole or cut only
by clips and wipeouts.

- **Strokes and points** become paths of curve items in the primitive's color, its alpha
  multiplied by one minus the transparency. The width is the lineweight (the default lineweight
  is `DraftOptions::default_lineweight_mm`, 0.25 mm), marked as a lineweight: in a model space
  in screen pixels (`lineweight_px_per_mm`, 96 per inch by default), on a sheet in paper
  millimeters through the plot scale. Caps and joins are round; polyline widths are kept.
  Consecutive primitives of one source object with the same appearance share one item.
  Unbounded curves that no clip bounded are not drawn.
- **Linetypes**: the pattern scale is the entity's linetype scale times the scale of the map
  from the drawing entity to the list, measured as the ratio of the curve's length in the list
  to its length in the entity (exact for similarities). Each drawn piece starts at the pattern
  position of its start on the whole curve (from the start of its polyline segment when the
  pattern restarts at vertices). Patterns with more than four drawn elements are generated on
  the CPU, intersected with the drawn pieces in parameter space, and drawn solid.
- **Fills and wipeout masks** become meshes triangulated from the boundary
  (`arc_geom::triangulate` with the display tolerance). A fill is drawn in its color; a mask is
  painted unlit in the background color in a model space (instance flag `PAINT_BACKGROUND`) and
  in `RenderSettings::paper` on a sheet (`PAINT_PAPER`), so changing either color needs no
  re-extraction. Wipeouts cut the curves beneath them on the CPU in the assembled lists; masks
  and lines are not ordered on the GPU. Masks and the paper are faces, so the `Wireframe` and
  `HiddenLine` modes do not paint them; drawings are drawn in the `Shaded` or
  `ConsistentColors` mode.
- **Source and sub-object.** Items are keyed by the primitive's source object;
  `DraftExtraction::sub_objects` gives the source, nested path and part of every path of every
  item, and `Scene::path_of` the path of a picked segment, so picking and highlights resolve the
  block reference and the member.
- **Entry points.** `extract_entity` extracts one entity by itself (its own list, primitives on
  off or frozen layers left out, nothing for entities of block definitions) and
  `register_extractors` registers it for every drafting entity type in an `ExtractorRegistry`.
  Wipeouts cut other entities only in assembled lists: `space_extractions` extracts a space from
  `space_list`, one extraction per source object. `apply` applies extractions to a scene with
  stamps newer than what it holds.
- **Sheets.** `sheet_extractions` draws a layout space: the paper rectangle from `(0, 0)` to the
  sheet size in the paper color (keyed by the layout settings object), the printable area inside
  the margins as a one-pixel gray outline, every viewport's `viewport_list` as one object keyed
  by the viewport, and the layout's own entities; it returns the sheet size and the plot scale
  (sheet units per paper millimeter) to render it with. Lineweights stay in paper millimeters.
- **Viewport clips.** Curves and points are already cut by the viewport list. When the clip is an
  axis-aligned rectangle, the viewport's curve items carry it, and the GPU limits them to the
  pixels whose centers are inside it by a scissor rectangle when the view maps it to an
  image-aligned rectangle (orthographic views); this also cuts line widths and caps that reach
  past the boundary. Fills and masks that the list marks as needing a clip are intersected with
  the clip boundary on the CPU for any clip shape: both regions are triangulated and every
  pair of triangles is clipped with the Sutherland–Hodgman algorithm against the convex clip
  triangle. For a clip that is not a rectangle, the half width of a line that reaches the
  boundary is not clipped.

### Hatches and fills (CORE-GPU-011)

- **Pattern hatches stay CPU strokes.** `arc-draft` generates the lines and dots of a pattern
  hatch in document coordinates from the hatch's pattern lines, scale, angle and origin, clipped
  to its region, so the lattice is fixed in the document and a pan or zoom never regenerates it.
  The extractor draws them as paths of one curve item: all strokes and dots of one hatch share
  one item (one draw and one range of line segments), whatever their number, so
  `max_items_per_object` does not limit the lines of a hatch. A GPU pattern path was not
  built: it would have to clip patterns to regions with holes in the shader, and the strokes are
  exact. The cost is one path per line.
- **The generation bound.** A hatch takes `DisplayOptions::max_dashes` as the bound of the lines
  and marks it generates and falls back to a solid fill (part 1) beyond it. Extraction builds
  lists with `max_dashes` 0 (dashes are left to the GPU), which would turn every pattern hatch
  into that fill. A list that holds such a fill is therefore built a second time with the
  configured `max_dashes`, and the strokes of each hatch from that list replace its fill. A hatch
  that still exceeds the bound, or a second list that fails, keeps the solid fill. A separate bound for hatches
  in `arc-draft` would remove the second build.
- **Solid fills** (`Shape::Region`, `Shape::Fill`) are triangulated meshes; a region with holes
  is filled by nesting parity, so a ring is filled and its hole is not. Their instances carry the
  flag `DRAFT_FILL`, which moves their depth back by half the allowance of masks: strokes drawn
  on a fill win without relying on equal depths in the two shaders, and masks stay behind fills
  as before.
- **Gradient fills** are drawn as the flat color the display list carries (the first gradient
  color). The gradient kind, angle and second color are not in the display list, so no gradient is
  drawn; this is a gap until `arc-draft` puts them in a primitive.
- **Updates.** An edited hatch is extracted alone and replaces its own items: only its line
  segments (a pattern hatch) or its vertices, indices and mesh instances (a solid hatch) are
  written; the buffers of every other object are untouched.
- **Picking and highlight** resolve to the hatch object: its strokes are path elements of its
  curve item, its fill is a mesh item of the hatch. Picking a pixel between pattern lines
  returns nothing.

### Outline text (CORE-GPU-012)

- **Stroke text** arrives from `arc-draft` as strokes and is drawn as curve items at the layout
  positions with the entity's color and lineweight; it needed no change.
- **Outline text** (`Shape::Glyphs`) is drawn as triangulated glyph meshes, not from an atlas:
  the row's atlas needs cache management and rasterization at every size and zoom, while a mesh
  is crisp at any zoom, is positioned by an exact `f64` transform, and picks and highlights as
  geometry. The cost is the triangle count of the glyphs, which grows with the glyph's detail
  and not with the size on the screen.
- **The glyph cache** (`arc_gpu::glyph::GlyphCache`, held by `DraftOptions::glyphs`; clones
  share it) maps a font (its bytes and face index) and a glyph identifier to a triangle mesh in
  em units. Outlines are read with `skrifa` from the bytes of the font the plot primitive carries
  (its family, bytes and face index), at the font's default location; the provider is not
  consulted, so two files under one family name draw their own glyphs. Quadratic and cubic segments are flattened once, in em units, with the chord
  tolerance `GLYPH_CHORD_EM` (1/2048 em), not by the zoom rule of curve items: one mesh serves
  every size and view, so the flattening error in pixels is the em size in pixels over 2048
  (0.5 px at an em of 1024 px, more beyond). The contours become a region with
  `arc_geom::build_region` (nesting parity) and are triangulated with `arc_geom::triangulate`.
  A glyph with contours that overlap or touch, an unreadable outline or more than 65,536
  flattened points is not drawn and is counted in `DraftExtraction::skipped_glyphs`; a space or a
  glyph without contours is not counted. The cache holds at most 65,536 glyphs and is emptied when full.
- **Items.** A glyph run becomes one mesh item for each distinct glyph of the run, whose
  instances are its occurrences: the transform maps em units to the list through the run's
  placement, the em size and the pen position (relative to the item origin in `f32`, the
  rest in `f64`), the color is the primitive's. Items of one extraction share the geometry of a
  glyph. The instance flag `DRAFT_TEXT` moves the depth of a glyph toward the eye by the edge
  allowance, so text wins over the fills it lies on in every backend. `sub_objects` of such an item has one
  entry per instance, which is the element of a picked hit. Text items keep the source object
  and sub-object key of the primitive, so picking and highlights resolve to the text.
- **Updates.** An edited text replaces its own items: its vertices, indices and instances are
  written and nothing else.
- **Gaps.** Glyphs are not clipped to a viewport boundary (a run is drawn whole); glyphs with
  overlapping contours are skipped; color fonts, hinting and variation coordinates other than
  the default are not used.
