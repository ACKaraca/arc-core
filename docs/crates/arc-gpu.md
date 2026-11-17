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
| `GpuContext` | Device, queue, adapter information, feature set |
| `ViewportScene` | Render items keyed by object identifier with dirty tracking |
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
| Arc | `arc-base`, `arc-geom`, `arc-mesh`, `arc-doc`; `arc-draft` (feature `draft`, display-list extractor, from phase 1); `arc-scene` (feature `scene`, material and camera conversion, from phase 4) |
| Third-party | `wgpu`, `glam`, `lyon`, `swash` (glyph rasterization), `image` (headless output) |

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
