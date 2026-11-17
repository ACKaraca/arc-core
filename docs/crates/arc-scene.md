# arc-scene

The renderable scene description and the Arc Scene contract used between products and the
RenderBuilder process, render-farm workers and visualization interchange. The architecture is
specified in [rendering.md](../architecture/rendering.md#the-arc-scene-contract-arc-scene).

## Owns

- The scene model: prims, meshes, curves, points, instancers, materials, lights, cameras and render
  settings, with time samples.
- The OpenPBR material parameter set and the supported MaterialX node subset.
- Change batches and sequence numbering for incremental updates.
- The session protocol: control and data channels, messages, frame records and errors.
- Content-addressed scene assets.
- The mapping to and from OpenUSD (feature `usd`, through `arc-io-usd`).
- The contract version and its published JSON Schemas.

## Stays in products

Extraction of a scene from a product's document (which objects become which prims and materials),
product material libraries, render presets, and the renderer itself (RenderBuilder).

## Key concepts and types

| Concept | Description |
|---|---|
| `Scene` | Contract version, units, up axis, time range, root prims, render settings |
| `Prim` | Path, `SceneId`, optional source `ObjectId`, transform samples, visibility, purpose, kind data |
| `MeshPrim` | Asset reference, primvars, subdivision data, face-set material bindings |
| `Instancer` | Prototypes plus per-instance transforms, ids and primvars |
| `Material` | OpenPBR parameters with constant, texture or node-graph inputs |
| `Light` | Kind, physical intensity, color or temperature, shaping, linking sets |
| `Camera` | Projection, lens, sensor, focus, aperture, shutter, clip range, exposure |
| `ChangeBatch` | Sequence number, document revision, added, removed and changed prims |
| `FrameRecord` | Sequence applied, sample count, format, pixels or tiles |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom`, `arc-mesh`, `arc-doc` (asset hashing and identifiers), `arc-command` (protocol framing), `arc-io-usd` (feature `usd`) |
| Third-party | `serde`, `serde_json`, `blake3` |

## Test strategy

Schema conformance tests for every message and scene element; USD round-trip tests (scene to USD to
scene equals the original) and validation of exported stages with the OpenUSD checker out of process;
change-batch replay tests (initial scene plus batches equals the final scene sent whole); session
tests against a test renderer that echoes scenes and produces synthetic frames; fuzzing of message
and record decoding.

## Capabilities

### Scene model

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-SCENE-001 | Prim hierarchy | Represents scenes as hierarchies of prims with paths, stable scene identifiers, optional source object identifiers, transforms, visibility and purpose. | T1 | 4 |
| CORE-SCENE-002 | Meshes and primvars | Represents meshes by asset reference with normals, UV sets and arbitrary primvars by interpolation class, and face-set material bindings. | T1 | 4 |
| CORE-SCENE-003 | Subdivision description | Marks meshes for subdivision with scheme, creases, corners and interpolation rules matching `arc-mesh`. | T2 | 4 |
| CORE-SCENE-004 | Curves and points | Represents curves with widths and bases, and point sets with widths and colors. | T2 | 4 |
| CORE-SCENE-005 | Instancing | Represents instancers with prototypes and per-instance transforms, identifiers and primvars, so repeated objects are sent once. | T1 | 4 |
| CORE-SCENE-006 | Time samples | Stores time-sampled transforms, camera parameters and primvars over a time range with a frame rate. | T2 | 4 |
| CORE-SCENE-007 | Render settings | Describes resolution, pixel aspect, sample budget, outputs and color space. | T1 | 4 |
| CORE-SCENE-008 | Output variables | Describes beauty and auxiliary outputs (depth, normal, albedo, object and material identifiers, light groups) requested from the renderer. | T2 | 4 |

### Materials, lights and cameras

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-SCENE-009 | OpenPBR materials | Represents every material as OpenPBR surface parameters with constant, texture or node-graph inputs. | T1 | 4 |
| CORE-SCENE-010 | MaterialX node subset | Accepts node graphs built from the documented subset of MaterialX standard nodes and rejects others with the node named. | T2 | 4 |
| CORE-SCENE-011 | Textures | References textures as assets with color space, wrap modes, UV set and transform. | T1 | 4 |
| CORE-SCENE-012 | Physical lights | Represents distant, sphere, disk, rectangle, cylinder and dome lights with physical intensity units, color or temperature, cone shaping and IES profiles. | T1 | 4 |
| CORE-SCENE-013 | Light linking | Restricts lights to sets of prims for illumination and shadowing. | T2 | 4 |
| CORE-SCENE-014 | Physical cameras | Represents perspective, orthographic and spherical cameras with focal length, sensor, focus distance, aperture, shutter and exposure. | T1 | 4 |
| CORE-SCENE-015 | Viewport camera conversion | Converts between viewport view frames and scene cameras so a render matches the viewport framing exactly. | T1 | 4 |

### Updates and session

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-SCENE-016 | Change batches | Sends added, removed and changed prims, camera and settings changes as numbered batches tagged with the document revision. | T1 | 4 |
| CORE-SCENE-017 | Asset deduplication | Sends assets by content hash once per session; a batch referencing an already-sent hash does not resend data. | T1 | 4 |
| CORE-SCENE-018 | Session control | Opens, updates and closes scenes and starts, pauses and stops interactive or final renders over the control channel. | T1 | 4 |
| CORE-SCENE-019 | Progressive frames | Receives progressive frames or tiles with sample counts and the applied batch sequence over the data channel. | T1 | 4 |
| CORE-SCENE-020 | Renderer crash handling | Ends a session with an error when the renderer exits unexpectedly, leaving the document unchanged, and supports restarting from the last scene state. | T1 | 4 |
| CORE-SCENE-021 | Farm job description | Serializes a scene with settings and frame range as a self-contained job package for render-farm workers. | T2 | 4 |
| CORE-SCENE-022 | Contract versioning | Negotiates contract versions at session start; minor versions add optional elements that older peers ignore. | T1 | 4 |

### OpenUSD mapping

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-SCENE-023 | Export to USD | Writes scenes as USD stages using standard geometry, lighting, camera and shading schemas with OpenPBR MaterialX surfaces and `arc:` identifier attributes. | T1 | 4 |
| CORE-SCENE-024 | Import from USD | Reads USD stages into scenes and keeps unsupported prims and attributes as preserved data with a loss report. | T2 | 4 |
| CORE-SCENE-025 | Round-trip identity | A scene exported to USD and imported again equals the original, including identifiers and material parameters. | T1 | 4 |
