# Technology baseline

The third-party choices of the suite, maintained here. Versions are pinned once in the `arc-core`
workspace. A dependency not listed in this document fails `cargo xtask layers`; adding one is a pull
request to this file that states the same columns. Licenses must be on the suite allowlist enforced
by `cargo deny` ([dependency policy](https://github.com/ACKaraca/arc-suite/blob/main/docs/engineering/dependency-policy.md)).

Status words: **ADOPT** (used directly), **WRAP** (used only behind an Arc-owned API, replaceable),
**BUILD** (Arc-owned implementation).

## Baseline

| Concern | Choice | Status | Reason | License | Isolation | Replaced when |
|---|---|---|---|---|---|---|
| Toolchain | Stable Rust, edition 2024, pinned in `rust-toolchain.toml`; minimum supported version is current stable minus two | Policy | Reproducible builds; recent language features without nightly | MIT or Apache-2.0 | n/a | Never replaced; the pin moves monthly |
| Small math | `glam` (f32 GPU data), `nalgebra` (f64 model math), `faer` (dense and sparse solves) | ADOPT | Mature, fast, widely used; `faer` has the fastest sparse factorizations in Rust | MIT or Apache-2.0; Apache-2.0; MIT | Used in L0 aliases and internal code; public APIs use `arc-base` aliases | A crate stops being maintained for a year or blocks a required numeric feature |
| 2D geometry | Arc-owned curves and regions; `i_overlay` (polygon booleans), `kurbo`, `robust` predicates, `spade` (Delaunay, TIN), `rstar` | BUILD + ADOPT | Exact arcs must stay exact entities, which general polygon libraries cannot provide; proven libraries handle the robust cores | Apache-2.0 / MIT or Apache-2.0 | Behind `arc-geom` APIs | A library fails robustness cases in the geometry corpus that cannot be fixed upstream |
| NURBS | `curvo` | ADOPT behind `arc-geom` | Complete NURBS evaluation, knot operations and fitting in pure Rust | MIT | Used only inside `arc-geom` | Upstream inactive for a year; then vendored into `arc-geom` |
| Meshes | Arc-owned half-edge mesh; Manifold (mesh booleans), `parry` (queries), `meshopt` | BUILD + WRAP | Editing meshes are core to several products; Manifold gives guaranteed-manifold robust booleans; `parry` covers queries and clash tests | Apache-2.0; Apache-2.0; MIT | Manifold and `meshopt` behind `arc-mesh` APIs | A pure-Rust boolean passes the mesh corpus with equal robustness |
| Subdivision surfaces | Arc-owned Catmull–Clark with creases, following the published OpenSubdiv rules | BUILD | Interchange fidelity without a C++ dependency; rules are public and compact | Apache-2.0 (Arc) | `arc-mesh` | Never by a library; GPU evaluation may be added |
| B-rep kernel | Open CASCADE through Arc-owned bindings in one FFI crate, dynamically linked, behind `arc-brep` | WRAP | Robust blends, booleans and STEP exchange represent decades of work; no pure-Rust kernel reaches production quality | LGPL-2.1 with exception | `arc-brep-occt`; dynamic linking; STEP import in `arc-worker` | A pure-Rust kernel passes the golden-model corpus for an operation family; that family moves to native code |
| Constraint solver | Arc-owned: graph decomposition with a Levenberg–Marquardt numeric core on `faer` | BUILD | Every product uses it; existing open solvers are under licenses incompatible with in-process use; diagnostics quality is a product feature | Apache-2.0 (Arc) | `arc-solve` | Not planned |
| Document store | Arc-owned typed stores with UUIDv7 persistent identifiers and a transaction journal | BUILD | Element-level operations, references and change sets are the suite's central data structure | Apache-2.0 (Arc) | `arc-doc` | Not planned |
| Storage | SQLite through `rusqlite` (bundled); `postcard` payloads; `zstd`; content-defined chunking (`fastcdc`) for large assets | ADOPT | Atomic commits, partial loading, inspectable with standard tools; compact deterministic encoding | Public domain (SQLite), MIT; MIT or Apache-2.0; MIT or Apache-2.0; MIT | Behind `arc-doc` package APIs | SQLite cannot meet a documented package requirement |
| GPU | `wgpu` behind `arc-gpu`; WGSL shaders | ADOPT | Vulkan, Metal, DirectX 12 and WebGPU from one API; active upstream | MIT or Apache-2.0 | Only `arc-gpu` and `arc-ui` see `wgpu` types | A required GPU capability is unavailable for more than a year |
| Text | `parley` and `swash`; Arc-owned single-stroke drafting fonts | ADOPT + BUILD | Rich text layout, shaping and bidirectional text in pure Rust; drafting fonts must be owned for licensing and plot fidelity | MIT or Apache-2.0 | `arc-draft` text layout and `arc-ui` | Text quality requirements cannot be met upstream |
| UI | `egui` with `egui_dock`, AccessKit, `muda` (native menus), `rfd` (native dialogs); Arc widgets in `arc-ui` | ADOPT + BUILD | Embeds custom GPU viewports directly, one code path on three platforms, accessibility through AccessKit | MIT or Apache-2.0; MIT; MIT or Apache-2.0; MIT or Apache-2.0; MIT | All use through `arc-ui` re-exports | A required accessibility or text capability cannot be met without a permanent fork |
| DXF | Arc-owned reader and writer from the published DXF reference | BUILD | Native data path for drafting; full control of hostile-input limits and preservation | Apache-2.0 (Arc) | `arc-io-dxf` | Not planned |
| DWG | `acadrust` (pure Rust) behind `arc-io-dwg`'s byte-level API | WRAP | The only permissively usable DWG library; byte-level API keeps it replaceable | MPL-2.0, unmodified | `arc-io-dwg`; optional `arc-worker` execution | Coverage gaps that block DraftBuilder workflows and cannot be closed upstream |
| IFC | `ifc-lite` behind `arc-io-ifc`; IfcOpenShell only as an out-of-process test oracle | WRAP | Fast pure-Rust IFC with current schemas; the oracle validates our output independently | MPL-2.0, unmodified; LGPL-3.0 (oracle, not shipped) | `arc-io-ifc` | API churn or correctness gaps that block ArchBuilder workflows |
| STEP and IGES | Through the B-rep tier | WRAP | Exchange fidelity comes with the kernel | LGPL-2.1 with exception | `arc-io-step` over `arc-brep`; import in `arc-worker` | Follows the B-rep kernel |
| USD | `openusd` (pure-Rust reader and writer); no C++ OpenUSD, no Hydra delegate | ADOPT | Reads and writes all USD encodings with composition, without a large C++ build | MIT | `arc-io-usd` | Conformance gaps against the OpenUSD checker that block exchange |
| PDF | `krilla` for output; `lopdf` plus PDFium for import and underlays | ADOPT + WRAP | High-quality vector output with fonts and PDF/A; PDFium gives the best raster fidelity for underlays | MIT or Apache-2.0; MIT; BSD-3-Clause | PDFium only in `arc-worker` via `arc-pdfium` | Output defects that cannot be fixed upstream |
| Exchange package | Arc-owned container and schemas (see [interchange](interchange.md#the-arc-exchange-package)) on `zip`, `serde_json`, `jsonschema` and `blake3`; connectors outside this repository write and read it | ADOPT + BUILD | Hosts that run connectors can read ZIP and JSON without a native library; the schemas are published and validated independently | Apache-2.0 (Arc); MIT; MIT; CC0-1.0 or Apache-2.0 | `arc-io-exchange` | Measured open time or random-access needs exceed the browser budget ([ADR-0011](../adr/0011-exchange-package-container.md)) |
| 3DM | openNURBS behind the FFI crate `arc-opennurbs` | WRAP | The open reference library of the format; no pure-Rust reader has comparable coverage | openNURBS license (permissive, BSD-style; allowlist entry required before first build) | `arc-opennurbs`; import only in `arc-worker` | A pure-Rust reader passes the 3DM corpus ([ADR-0012](../adr/0012-opennurbs-ffi-crate.md)) |
| FBX | `fbxcel` and `fbxcel-dom` (binary FBX 7.x reading) | ADOPT | An open reader exists; the vendor's SDK is not used | MIT or Apache-2.0 | `arc-io-fbx` | Corpus failures that cannot be fixed upstream |
| DGN V7 | Arc-owned reader from the published specification | BUILD | The specification is public and the format is small | Apache-2.0 (Arc) | `arc-io-dgn` | Not planned |
| Licensed bridges | Arc-owned process host and descriptors; the conversion library is user-installed, user-licensed and never distributed | BUILD | Lets a product use a library under a vendor or consortium license without depending on it | Apache-2.0 (Arc); the library keeps its own license | Separate process started by `arc-worker` | Not planned |
| Browser and mobile targets | `wasm-bindgen`, `web-sys`, `js-sys`; `winit` web, iOS and Android backends; `wgpu` on WebGPU, Metal and Vulkan | ADOPT | The same Rust crates and UI kit reach the browser and mobile; generated glue holds no application logic ([ADR-0018](https://github.com/ACKaraca/arc-suite/blob/main/docs/adr/0018-web-and-mobile-targets.md)) | MIT or Apache-2.0 | `arc-app` and `arc-gpu` target modules | A required platform capability is unavailable through these crates for more than a year |
| Point clouds and GIS | `las`, `laz`, `e57`; `proj`, `geo`; FlatGeobuf, GeoPackage and Shapefile readers | ADOPT | Mature format libraries; PROJ is the reference for coordinate operations | MIT, Apache-2.0 | `arc-io-pointcloud`, `arc-io-geo` | Library inactive and failing corpus cases |
| Render building blocks | Embree (ray intersection, FFI crate), Open Image Denoise, `exr`, OpenColorIO | WRAP | Used by RenderBuilder; listed so the baseline covers the suite. `exr` is also used by `arc-io-mesh` | Apache-2.0; Apache-2.0; BSD-3-Clause; BSD-3-Clause | FFI crates live in RenderBuilder | Per RenderBuilder decisions |
| Simulation | `rapier` (rigid bodies); CalculiX, Gmsh, EPA SWMM and EPANET as separate solver processes behind adapters | ADOPT + WRAP | Used by products; process boundaries keep copyleft solvers separate | Apache-2.0; solvers under their own licenses, never linked | Separate processes | Per product decisions |
| Plug-ins | `wasmtime` component model | ADOPT | Sandboxed, language-neutral plug-ins with versioned interfaces | Apache-2.0 with LLVM exception | `arc-command` feature `wasm-plugins` | A sandboxing requirement cannot be met |
| Server | `axum`, `sqlx` with PostgreSQL, `object_store`, `openidconnect`, WebSocket | ADOPT | Used by Arc Hub; `arc-hub-api` shares request and response types | MIT, MIT or Apache-2.0 | Server code lives in Arc Hub | Per Arc Hub decisions |
| Quality | `cargo-nextest`, `insta`, `egui_kittest`, `cargo-fuzz`, `criterion`, `cargo-deny`, `cargo-audit` | ADOPT | Standard Rust quality tooling | MIT or Apache-2.0 | Development dependencies and tools only | Tool unmaintained |
| Shipping | Velopack (desktop updates), `cargo-dist` (CLI and server), Flatpak, MSI, DMG; opt-in crash reports in a Sentry-compatible format; Fluent localization | ADOPT | Proven installers and updaters; no Arc-owned infrastructure | MIT; MIT or Apache-2.0; MIT or Apache-2.0 (`sentry`, `fluent-bundle`) | `arc-app` and release tooling | Tool unmaintained |

## Supporting crates

Small, widely used crates the baseline relies on. The same columns apply; all are ADOPT.

| Crate | Use | License |
|---|---|---|
| `serde`, `serde_json` | Serialization of protocol messages, descriptors, settings | MIT or Apache-2.0 |
| `uuid` | UUIDv7 identifiers | MIT or Apache-2.0 |
| `blake3` | Content hashes for assets, payloads and scene data | CC0-1.0 or Apache-2.0 |
| `thiserror` | Error enums | MIT or Apache-2.0 |
| `tracing`, `tracing-subscriber` | Structured logging | MIT |
| `rayon` | Work-stealing worker pool | MIT or Apache-2.0 |
| `crossbeam-channel` | Bounded channels between threads | MIT or Apache-2.0 |
| `arc-swap` | Lock-free publication of snapshots | MIT or Apache-2.0 |
| `jsonschema` | Validation of command parameters against schemas | MIT |
| `schemars` | Schema generation from parameter types | MIT |
| `quick-xml` | Bounded XML reading and writing (IFC-ZIP manifests, BCF, LandXML, IDS) | MIT |
| `zip` | Bounded ZIP reading and writing | MIT |
| `lyon` | Fill tessellation | MIT or Apache-2.0 |
| `resvg` | Icon and SVG rasterization | Apache-2.0 or MIT |
| `image` | Image decoding and encoding | MIT or Apache-2.0 |
| `tiny-skia` | CPU rasterization for plot raster output and reference images | BSD-3-Clause |
| `cxx` | C++ binding in `arc-brep-occt` | MIT or Apache-2.0 |
| `egui_table`, `egui_ltreeview`, `egui-snarl` | Bases for virtualized tables, trees and the node editor in `arc-ui` | MIT or Apache-2.0 |
| `minidumper`, `crash-handler`, `sentry` | Out-of-process crash capture and opt-in reporting | MIT or Apache-2.0 |
| `tokio` | Asynchronous runtime for networking in `arc-app` and `arc-hub-api` only | MIT |
| `reqwest` | HTTP client in `arc-hub-api` and update checks | MIT or Apache-2.0 |
| `wasm-bindgen`, `web-sys`, `js-sys` | Generated browser glue in `arc-app` and `arc-gpu` (browser target only) | MIT or Apache-2.0 |
| `proptest` | Property tests | MIT or Apache-2.0 |
| `robust` | Adaptive exact orientation and in-circle predicates in `arc-geom` | MIT or Apache-2.0 |
| `rstar` | R*-tree behind the spatial index of `arc-geom`; no `rstar` type appears in a public signature | MIT or Apache-2.0 |
| `spade` | Constrained Delaunay triangulation behind the polygon triangulation of `arc-geom`; no `spade` type appears in a public signature | MIT or Apache-2.0 |
| `toml` | Reading the layer table and manifests in `xtask` | MIT or Apache-2.0 |

## Not built in the first two years

The open readers and the exchange package listed above are built; the following are not.

| Not built | Reason |
|---|---|
| A B-rep kernel from scratch | Robust blends and booleans take many years; the owned API keeps the native path open one operation family at a time |
| A GUI toolkit | egui meets the requirements with Arc widgets on top; toolkit work would compete with product work |
| A GPU abstraction | `wgpu` covers every target backend |
| A CRDT engine | Document invariants (valid joins, solvable constraints, unique numbering) cannot be enforced by automatic merges; change sets with conflict records are used instead |
| A native finite-element solver or mesher | Open solvers run as separate processes behind adapters; a native kernel replaces the wrapped engine once it passes the same validation suites |
| A native slicer | Additive preparation uses a wrapped engine; a native kernel replaces it once it passes the same validation suites |
| Native multi-axis toolpath kernels | 2.5D and 3-axis machining come first; wrapped engines serve multi-axis until a native kernel passes the same validation suites |
| A DWG or IFC parser from scratch | Wrapped libraries exist under usable licenses; effort goes into mapping and validation |
| A native reader for formats without a published specification (such as native BIM project files) | Their data is exchanged through connectors and the Arc exchange package, licensed bridges and open formats; formats with a published specification or an open reader are read natively |
| An installer or updater | Velopack, `cargo-dist` and platform packaging cover distribution |
| A scripting language | Python runs out of process through the automation protocol; the expression engine is deliberately not a general language |
