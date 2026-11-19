# Rendering

Two rendering paths serve the suite: the real-time viewport in every product (`arc-gpu`) and
photoreal rendering in a separate RenderBuilder process, reached through the Arc Scene contract
(`arc-scene`). Products never link the path tracer.

## The viewport renderer (arc-gpu)

### Scene extraction from documents

- The viewport draws a **viewport scene**: a retained set of render items held by `arc-gpu`. A
  render item is one of mesh instance, line batch, fill batch, glyph run, point set, image, or
  overlay item, and carries the `ObjectId` and sub-object range it came from, a world transform
  with an `f64` origin, a viewport material, and display flags.
- Products supply **extractors**: functions from a snapshot and a set of changed object ids to
  render items. Extractors run on the worker pool after each commit or regeneration, read only
  derived data (tessellations, display lists), and return items keyed by object id. `arc-gpu`
  replaces the items of changed objects and leaves the rest.
- `arc-draft` display lists map to line, fill and glyph batches by a shared extractor in `arc-gpu`,
  so all products draw drafting content identically.
- Extraction is tagged with the revision it read; the render thread drops an extraction result
  older than one it has already applied for the same object.

### Camera-relative precision

- Documents store `f64` coordinates; GPUs work in `f32`. Each render item has an `f64` origin. Each
  frame, the CPU computes `origin − camera position` in `f64` and uploads it as an `f32`
  translation, so precision is highest near the camera regardless of absolute coordinates.
- Vertex positions are `f32` relative to the item origin. Items whose extent exceeds 1 km (terrain,
  long alignments, large point clouds) are split into tiles with their own origins, which keeps
  vertex error below 0.1 mm.
- Depth uses reversed-Z with a floating-point depth buffer; orthographic views use a depth range
  fitted to the visible bounds.

### Batching and GPU-driven culling

- Geometry lives in large pooled vertex and index buffers with sub-allocation; instances,
  transforms and material indices live in storage buffers. Items with the same pipeline state are
  drawn together with instancing.
- Culling runs on the GPU: a compute pass tests instance bounds against the frustum and against a
  hierarchical depth pyramid from the previous frame, then writes indirect draw arguments consumed
  by multi-draw indirect. Adapters without multi-draw indirect (WebGPU, the OpenGL fallback) use a
  CPU culling path that produces the same visible set.
- Level of detail: meshes supply simplified levels (built with `meshopt`) selected by projected
  size; meshlets with mesh shaders are a later optimization behind the same API.

### Drafting-quality lines and hatches

- Lines are drawn as instanced segment quads with signed-distance caps and joins, so widths are
  exact at any zoom. Widths are either **screen widths** (pixels, for model display) or **paper
  widths** (millimetres converted through the plot scale and display DPI, for sheet display), with a
  one-pixel minimum.
- **Linetype patterns** are evaluated in the fragment shader from the accumulated arc length along
  each polyline, so dashes are continuous across vertices and stable while panning. Pattern scale,
  phase and the rule for centering patterns on segments follow the linetype definition.
- Exact curves from display lists are flattened on the CPU at a tolerance derived from the current
  zoom and re-flattened when zoom changes by more than a factor of two.
- **Hatches** draw pattern line families in world space, clipped to the hatch region by a stencil
  pass, with the pattern origin fixed in the document so patterns do not swim. Solid fills are
  tessellated with `lyon`.
- Lineweight display, line smoothing and the background color of model and sheet spaces are
  viewport settings.

### Shading modes

| Mode | Description |
|---|---|
| Wireframe | Edges only, no hidden-line removal |
| Hidden line | Edges with hidden edges removed or drawn dashed |
| Shaded | Flat or smooth lighting with object or material colors |
| Shaded with edges | Shaded plus feature and silhouette edges |
| Realistic | The viewport material model (an OpenPBR subset: base color, metalness, roughness, specular, transmission approximation, emission, normal maps) with image-based lighting, shadows and ambient occlusion |
| Consistent colors | Unlit material colors, for diagrams |
| X-ray | Semi-transparent surfaces with visible edges |

Per-view overrides apply on top of the mode: color by object, category, property or rule;
transparency; ghosting of non-selected objects; isolation and hiding. Overrides are data evaluated
on the GPU through per-instance flags, not by re-extraction.

### Section planes

Up to six arbitrary clip planes and one section box per view. Clipped solids are capped: fragments
on the cut side are discarded and the back faces left visible through the cut are drawn in the
section color, with an optional hatch of screen-space lines. Section
state is part of the view and saved with it.

### Selection and identifier picking

- A pick pass renders object identifiers (`R32Uint`) and sub-object indices into an ID target,
  restricted by scissor to the area under the cursor (9 × 9 pixels for clicks, the rectangle for
  window and crossing selection). The result maps back to `(ObjectId, sub-object key)` through a
  table maintained with the render items.
- Pre-selection highlight uses the same pass on pointer movement, throttled to the display refresh.
- Exact snapping and measurement never use the GPU: they query `arc-geom` and `arc-mesh` spatial
  indexes on the CPU so results are exact in model coordinates.
- Selection highlight is drawn as an outline and tint pass that works in every shading mode.

### Overlays and manipulators

An overlay pass draws transient content above the scene: grips, snap markers and tracking lines,
rubber bands, prompt-machine previews, measurement graphics, the view cube, axis triad, grid and
manipulators (move, rotate, scale handles with constant screen size). Overlays ignore section
planes, can opt out of depth testing, and have their own hit testing so manipulators can be
dragged without a document pick.

### Point clouds

Point clouds are drawn from the level-of-detail octree built by `arc-io-pointcloud` and cached as
document assets. A per-frame point budget (default 10 million) selects nodes by projected size and
distance; points render as screen-space splats with adaptive size and eye-dome lighting. Color modes:
stored RGB, intensity, classification, elevation ramp, and single color. Clip planes and section
boxes apply to points.

### Frame scheduling off the UI thread

- The render thread owns all GPU submissions for viewports. Each viewport renders into its own
  offscreen color target; the UI thread composites the most recent completed target into the
  egui frame, so a slow frame never blocks input handling.
- Rendering is on demand: a viewport redraws when its camera moves, its scene or overrides change,
  or a progressive effect (ambient occlusion accumulation, point-cloud refinement) is pending.
  An idle application draws nothing.
- During navigation, the renderer may lower expensive effects to keep the target frame time and
  restores full quality when navigation stops.
- **Performance targets** on a mid-range discrete GPU (stated as targets, verified by benchmarks):
  60 frames per second while orbiting a model of 10 million triangles and 2 million line segments;
  first frame of a 1 GB package within 3 seconds after load; pick response under 16 ms. These
  numbers match the scale of large building and plant models the suite targets.

### Headless rendering

`arc-gpu` can create a device without a surface, on any adapter including software adapters, and
render a view to an image. This serves thumbnails at save, `view.render` in the automation
protocol, `render` in CLIs, and image-comparison tests in CI on a pinned software adapter.

## The Arc Scene contract (arc-scene)

The Arc Scene is the renderable description a product exports for photoreal rendering, render-farm
jobs and visualization interchange. It is versioned independently (`major.minor`) and its JSON
Schema is published in `schemas/scene/`.

### Scene description model

| Element | Content |
|---|---|
| Scene | Identifier, contract version, units (metres), up axis (+Z), time range and frame rate, root prims, render settings |
| Prim | Path (`/World/Building/Level 02/Wall 17`), stable `SceneId`, source `ObjectId` (optional), transform (time-sampled), visibility, purpose (render, proxy, guide) |
| Mesh | Asset reference to vertex and index data, normals, UV sets, primvars, subdivision flag and crease data, material bindings per face set |
| Curves and points | Curve widths and bases; point positions, widths, colors |
| Instancer | Prototype prims and per-instance transforms, identifiers and primvars |
| Material | OpenPBR surface parameters and an optional MaterialX node graph for textures and procedurals |
| Light | Distant, sphere, disk, rectangle, cylinder, dome (environment); intensity in physical units, color or temperature, shaping (cone, IES profile asset), light linking sets |
| Camera | Projection (perspective, orthographic, spherical), focal length, sensor size, focus distance, aperture, shutter interval, clip range, exposure |
| Render settings | Resolution, pixel aspect, sample budget, outputs (beauty and AOVs), color space |

Large data (meshes, textures, IES files) travels as content-addressed assets with BLAKE3 hashes,
so unchanged data is never sent twice.

### Materials

The surface model is OpenPBR: every Arc Scene material is a set of OpenPBR surface parameters
(base, specular, metalness, transmission, subsurface, coat, fuzz, thin-film, emission, geometry
opacity and normal) whose inputs may be constants, textures or MaterialX node graphs. The contract
lists the MaterialX standard nodes it supports. The viewport renders a documented subset; the
production renderer renders all of it. Product material libraries map to this model; there is no
second material model.

### Incremental updates

After the initial scene, a product sends **change batches**: added prims, removed prim paths,
changed attributes per prim, camera changes and render-setting changes, each batch with a sequence
number and the document revision it reflects. The renderer applies batches in order and reports the
sequence it has applied in every frame it returns, so the host knows which revision a frame shows.

### Session and frame-streaming protocol

- The host starts the renderer process (or connects to a running one) and opens two local
  connections on the transport of the automation protocol: a **control channel** (JSON-RPC,
  newline-delimited) and a **data channel** (length-prefixed binary records: a JSON header and a
  payload).
- Control methods: `scene.open`, `scene.update` (change batch), `scene.close`, `render.start`
  (interactive or final, region, outputs), `render.pause`, `render.stop`, `render.status`, and
  notifications `frame.ready` and `render.finished`.
- Assets and frames travel on the data channel. Interactive frames are sent progressively (tiles or
  whole images, RGBA half-float or 8-bit) with the sample count and applied sequence number. A
  shared-memory frame path may be added later behind the same messages when measurements require
  it.
- A renderer crash ends the session with an error; the host's document is unaffected and the session
  can restart from the last scene state.
- The same contract drives render-farm workers: a job carries a serialized scene and settings and
  returns outputs as files.

### Mapping to OpenUSD

`arc-scene` defines a lossless mapping for the elements above to OpenUSD, implemented with
`arc-io-usd`: prims to `UsdGeomXform`, `UsdGeomMesh`, `UsdGeomBasisCurves`, `UsdGeomPoints` and
`UsdGeomPointInstancer`; lights to `UsdLux`; cameras to `UsdGeomCamera`; materials to `UsdShade`
networks using the OpenPBR surface MaterialX node; `SceneId` and source `ObjectId` as custom
attributes in the `arc:` namespace. Exported stages open in any USD reader; importing a USD stage
produces an Arc Scene plus preserved data for what the contract does not model.
