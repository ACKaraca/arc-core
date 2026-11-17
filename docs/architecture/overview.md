# Architecture overview

How `arc-core` is divided into crates, which rules hold between them, and the cross-cutting models
(threading, errors, features, versioning) every crate follows. Area documents refine each part.

## Layers

| Layer | Crates | Purpose | May depend on |
|---|---|---|---|
| L0 | `arc-base` | Vocabulary shared by everything: identifiers, quantities, tolerances, errors, color | Standard library and allowlisted leaf crates |
| L1 | `arc-geom`, `arc-mesh` | Pure-Rust geometry owned by the project | L0 |
| L2 | `arc-brep`, `arc-solve` | Exact solid modeling and constraint solving | L0–L1 |
| L3 | `arc-doc`, `arc-command` | The document and the only way to change it | L0–L2 |
| L4 | `arc-draft`, `arc-view`, `arc-scene`, `arc-nodes`, `arc-io-*`, `arc-hub-api` | Shared domain-neutral models and run-time contracts | L0–L3 |
| L5 | `arc-gpu` | Real-time rendering | L0–L4 |
| L6 | `arc-ui`, `arc-app` | Widgets and the desktop shell | L0–L5 |

Crates in the same layer may depend on each other only where the layer table lists the edge
explicitly. The allowed same-layer edges are:

| From | To | Reason |
|---|---|---|
| `arc-mesh` | `arc-geom` | Triangulation, bounding volumes and spatial indexes |
| `arc-solve` | `arc-brep` is forbidden | Sketch solving works on `arc-geom` entities; solids reach the solver only as projected references supplied by the caller |
| `arc-command` | `arc-doc` | Commands run transactions |
| `arc-view` | `arc-draft` | Derived views produce drafting geometry |
| `arc-io-pdf` | `arc-draft` | Plot output is a drafting display list |
| `arc-scene` | `arc-io-usd` (feature `usd`) | The Arc Scene contract defines its own OpenUSD mapping |
| `arc-app` | `arc-ui` | The shell composes widgets; `arc-ui` never depends on `arc-app` |

All other `arc-io-*` crates have no same-layer edges. The DXF and DWG crates produce their own
neutral drawing model; mapping it to `arc-draft` or to a product model is the product's work.

Only `arc-brep` links the B-rep kernel. A crate that needs exact solids depends on `arc-brep`
explicitly; `arc-geom`, `arc-mesh`, `arc-doc`, `arc-draft` and `arc-gpu` never do. `arc-view`
accepts geometry through its own view-source trait with mesh and curve inputs; exact solid input is
an adapter behind its optional `brep` feature, so derived views of meshes and polyhedral data work
without the kernel.

## Dependency rules and their enforcement

`cargo xtask layers` reads `cargo metadata` and a layer table kept in `xtask/layers.toml`. Each
workspace crate has one class: `layer(n)`, `ffi(n)` (an FFI crate at layer n), `app`, `testkit` or
`tool`. The task fails when any of the following holds:

1. A crate depends on a workspace crate of a higher layer, or on a same-layer crate without a
   listed edge.
2. A crate of layer L0–L4 depends, directly or transitively through normal (non-dev) dependencies,
   on a crate in the UI deny list: `winit`, `wgpu`, `egui`, `eframe`, `egui_dock`, `accesskit`,
   `muda`, `rfd`, `raw-window-handle`.
3. A crate declares `unsafe` code (detected from the workspace lint table and source attributes)
   without the `ffi` class.
4. A crate below L6 depends on `arc-app` or `arc-ui`, or any crate depends on an `app` crate.
5. A crate of the WebAssembly set (below) gains a dependency that is not marked wasm-compatible in
   the table.
6. A third-party dependency appears that is not listed in the technology baseline or its
   supporting-crates table.

The rule engine is a pure function from (crate classes, edges) to violations and is unit-tested
without `cargo metadata`. The CI gate runs it on every pull request; a violation names the crate,
the edge and the rule. Changing the table needs an architecture decision record
([ADR-0001](../adr/0001-enforced-layering.md)).

## Headless and WebAssembly constraints

- L0–L4 compile and pass their tests on a machine with no display server and no GPU adapter.
- L0–L4 never open a window, a dialog or a notification. Functions that need a user decision
  return a value that describes the decision; the caller (UI or automation client) asks.
- L0–L4 perform no network access, except `arc-hub-api`, which only does so when its client is
  constructed with an endpoint.
- L5 (`arc-gpu`) needs a GPU adapter but not a window: it renders to offscreen targets and accepts
  a software adapter, so headless thumbnails and image tests run in CI.

The WebAssembly set builds for `wasm32-unknown-unknown` in CI (`cargo xtask wasm`): `arc-base`,
`arc-geom`, `arc-mesh`, `arc-solve`, `arc-doc` with the `package` feature disabled, `arc-command`
without the `server` and `wasm-plugins` features, `arc-draft`, `arc-view`, `arc-scene`,
`arc-nodes`, `arc-io-mesh`, `arc-io-ifc`, `arc-io-bcf`, `arc-io-exchange`, `arc-io-dgn`,
`arc-io-fbx` and `arc-gpu` with the `webgpu` backend. Browser builds of the products consume this
set ([platform targets](#platform-targets)). `arc-brep` (C++ kernel), file-backed SQLite packages,
PDFium, PROJ, openNURBS and process isolation are outside it.

## Unsafe code and FFI crates

`unsafe` is forbidden in every workspace crate through `[workspace.lints]` except these FFI crates:

| Crate | Wraps | Linking | Notes |
|---|---|---|---|
| `arc-brep-occt` | Open CASCADE through an Arc-owned C++ shim | Dynamic | Private to `arc-brep`; see [geometry](geometry.md#kernel-isolation) |
| `arc-pdfium` | PDFium for PDF import and underlay rasterization | Dynamic, loaded at run time | Used only inside `arc-worker` |
| `arc-opennurbs` | openNURBS for reading Rhino 3DM files | Static | Used only inside `arc-worker`; see [ADR-0012](../adr/0012-opennurbs-ffi-crate.md) |

Adding an FFI crate requires an architecture decision record. Every FFI crate exposes a safe API,
documents the invariant behind each `unsafe` block in a `SAFETY:` comment, runs its tests under
the address sanitizer in the nightly job, and converts every foreign failure (C++ exception,
error code, null result) into an `arc-base` error.

## Process isolation

`arc-core` ships one helper executable, `arc-worker`, that products install beside their binaries.
It hosts work that is either hostile-input parsing in native code or a crash risk the document must
survive:

| Job | Why isolated |
|---|---|
| STEP and IGES import through the kernel | Native parser on untrusted files |
| PDF import and underlay rasterization through PDFium | Native parser on untrusted files |
| 3DM import through openNURBS | Native parser on untrusted files |
| Licensed bridge conversions (`bridge.convert`) | A user-licensed third-party library runs in its own process under resource limits; see [licensed bridges](interchange.md#licensed-bridges) |
| Kernel operations in batch and Hub jobs when isolation is requested | A kernel fault ends the worker, not the job runner |
| Out-of-process test oracles in CI | License and dependency separation |

The worker speaks the Arc Automation Protocol framing on stdio, runs with the confined file access
rules of the caller, and is restarted on failure. A job that crashes the worker fails with a
`worker.crashed` error and leaves the calling document unchanged.

## Platform targets

Products ship for Windows, macOS and Linux, and may also ship a **browser build** (the same crates
compiled to WebAssembly, rendering through WebGPU) and **tablet and phone builds** for iOS and
Android. These are targets of the same code with the same UI kit, not separate applications; the
desktop applications still contain no web view and no application logic in JavaScript or TypeScript.
The glue that a platform requires (a generated loader page for the browser; an activity or
application delegate stub on mobile) is generated from templates in `arc-core` and contains no
application logic. The decision is
[ADR-0018](https://github.com/ACKaraca/arc-suite/blob/main/docs/adr/0018-web-and-mobile-targets.md).

### What each layer must guarantee

| Layer | Guarantee on browser and tablet or phone |
|---|---|
| L0 | No direct operating-system calls. Time, randomness and paths come from traits that the shell implements per platform, so identifiers, quantities and tolerances behave identically everywhere. |
| L1–L2 | `arc-geom` and `arc-solve` build for every target. `arc-mesh` builds everywhere; its Manifold-backed booleans, like `arc-brep`, are native code and follow the table below. |
| L3 | `arc-doc` runs with one executor thread and a worker pool of any size down to one: results are identical, only latency differs. Commits flush the recovery journal on a suspend notification within the time the platform grants. `arc-command` builds without its `server` and `wasm-plugins` features in the browser. |
| L4 | Every `arc-io-*` crate reads and writes byte buffers and streams, never paths. Readers default to the `constrained` limits profile. Format crates start no process; bridge and worker jobs are started by `arc-worker`, which these targets do not have. |
| L5 | `arc-gpu` runs on WebGPU in the browser, Metal on iOS and Vulkan on Android, with the CPU culling path when multi-draw indirect is missing, quality and memory budgets adapted to the device, and recovery from device loss and surface changes without losing the scene. |
| L6 | `arc-ui` and `arc-app` accept touch, pen and mouse as equal pointer sources, adapt layouts to the window size class, and reach storage and files only through the shell's file-access layer ([UI kit](ui-kit.md#browser-tablet-and-phone-targets)). |

### What is unavailable and what replaces it

| Capability | Desktop | Browser | Tablet and phone |
|---|---|---|---|
| B-rep kernel (STEP and IGES import, booleans, blends) | In process or `arc-worker` | Server-side worker | Server-side worker; compiled into the application only where the license and the platform allow |
| Mesh booleans (Manifold) | In process | Server-side worker | Compiled into the application |
| PDF rasterization (PDFium), 3DM import, PROJ coordinate operations | `arc-worker` or in process | Server-side worker | Server-side worker; PROJ compiled in where it builds |
| Licensed bridges | Supported | Not available; run on a worker the user operates | Not available; run on a worker the user operates |
| `arc-worker` and process isolation | Supported | Not available | Not available; isolation relies on the platform sandbox |
| Automation socket, CLI, Python, MCP endpoint | Supported | Not available | Not available; headless jobs run on Hub workers |
| WebAssembly plug-ins | Supported | Not available | Interpreter backend of the engine, where provided |
| Threads | All cores | Web Workers when the page is cross-origin isolated, otherwise one thread | All cores within the memory budget |
| Document package | File on disk with edit lock | In-memory image persisted by the shell | File in the application sandbox |
| Native menus and dialogs, crash monitor, update check | `muda`, `rfd`, monitor process, Velopack | In-application menus, browser file pickers, page reload | In-application menus, system document picker, store-managed updates |

A **server-side worker** is a headless Arc runner reached through Hub jobs
([`arc-hub-api`](../crates/arc-hub-api.md)); it receives input only after an explicit user action,
and the result returns as a document change or an asset. Compiling a native library for a platform
that forbids dynamic loading needs static linking; for LGPL libraries that is enabled only after an
architecture decision records how the license terms are met.

Commands that depend on an unavailable capability are listed as disabled with a reason that names
the alternative (CORE-APP-033), so no command fails after the user starts it.

## Feature-flag policy

- Features are additive. Enabling a feature never changes the meaning of an existing type or
  function; it only adds items or backends.
- Default features give the most common desktop configuration of the crate. Heavy or native
  optional dependencies are features: `package` (SQLite, `arc-doc`), `server` (protocol endpoints,
  `arc-command`), `wasm-plugins` (`arc-command`), `occt` (`arc-brep`), `webgpu` (`arc-gpu`), `package-memory`
  (`arc-doc`, SQLite operating on an in-memory image that the shell persists, for browser builds).
- No feature is named after a product. Products select capabilities, not profiles.
- `unstable-*` features expose APIs that may change in a patch release; a product release must
  not enable them. The release check rejects a product lock file that resolves an `unstable-*`
  feature.
- CI builds each crate with default features, with `--no-default-features`, and with all
  non-unstable features.
- Run-time behavior switches are settings, not Cargo features.

## Threading model

| Thread | Owner | Work | Must never |
|---|---|---|---|
| UI thread | `arc-app` | egui frame, input routing, prompt machines, previews, composing viewport textures | Block on I/O, regeneration or a command `run` |
| Document executor | `arc-doc` | One per open document: runs command `run` functions and commits transactions in order | Touch UI or GPU state |
| Worker pool | `arc-doc` scheduler | Regeneration, imports, exports, scene extraction, tessellation | Mutate a document |
| Storage thread | `arc-doc` package | Recovery journal writes, saves, asset reads | Hold a snapshot longer than a save |
| Render thread | `arc-gpu` | GPU uploads, culling and drawing of viewports into offscreen textures | Read mutable document state |
| Protocol threads | `arc-command` server | Reading and writing automation sessions | Execute commands directly (they enqueue on the executor) |
| Network runtime | `arc-app` | One asynchronous runtime for Hub, update checks and crash upload | Run inside L0–L3 |

Readers never lock the document. Every commit publishes an immutable snapshot (structural sharing,
see [document model](document-model.md#structural-sharing-and-snapshots)); the UI, the render
extractor, the automation query handlers and the save thread each hold the snapshot they started
with. The worker pool is a work-stealing pool sized to the logical core count minus one, shared by
all documents of a process, with the priority classes defined by the
[regeneration scheduler](document-model.md#regeneration-scheduler). L0–L3 crates contain no
asynchronous runtime; blocking I/O runs on the storage or worker threads.

## Error-handling model

- Every crate defines its own error enum. All crate errors convert into `arc_base::Error`, which
  carries a **category**, a **stable code**, a **message identifier** and structured **details**.
- Categories: `invalid_input`, `not_found`, `conflict`, `unsupported`, `limit_exceeded`,
  `permission_denied`, `io`, `corrupt_data`, `cancelled`, `failed_operation`, `internal`.
- The stable code is a dotted lowercase string owned by the crate (`doc.reference.dangling`,
  `brep.boolean.failed`). Codes are part of the public API: they are never reused for a different
  meaning, and they appear unchanged in automation protocol errors and CLI JSON output.
- The message identifier resolves through the Fluent catalogs, so the same error reads correctly
  in every locale. L0–L4 never format user-facing sentences themselves.
- Production code does not panic: `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` and
  `unreachable!` are denied by workspace lints outside tests.
- Command execution is guarded. The executor runs each command inside an unwind boundary; if a
  defect escapes as a panic despite the lints, the transaction is rolled back, the document returns
  to the last committed snapshot, and the failure is reported as `internal` with a diagnostic
  record. A dedicated test ([testing](../testing.md#no-panic-and-panic-hunt-tests)) verifies this.
- Every crash fix ships with a regression test that fails without the fix.

## Versioning and the suite baseline

- `arc-core` has one version for the whole workspace, `MAJOR.MINOR.PATCH`, released as the git tag
  `vMAJOR.MINOR.PATCH`. All crates carry that version, whether or not they changed.
- Before 1.0.0, a minor increment may break APIs and a patch increment may not. From 1.0.0,
  semantic versioning applies to the stable crate set declared in the release notes.
- Products depend on `arc-core` crates by git URL and tag. A product pins exactly one tag. A suite
  release names the single tag all products ship on; the suite release check fails if two products
  resolve different `arc-core` revisions.
- Four version numbers are independent of the crate version and change only by decision record:
  the document package format (`document-package.md`), the Arc Automation Protocol
  (`commands-and-automation.md`), the plug-in interface (`extensibility.md`) and the Arc exchange
  package schema ([interchange](interchange.md#versioning)). The Arc Scene contract and the Hub API
  carry their own versions as well.
- Third-party versions are pinned once, in the `arc-core` workspace, and re-exported where
  products need the same types (`wgpu`, `egui`). Products do not add their own versions of a
  re-exported crate.
- A deprecated public item survives at least one minor release with a deprecation note naming its
  successor.

## What belongs in arc-core

Code moves into `arc-core` when one of the following is true:

1. **A second concrete user exists.** Two products (or a product and a Hub worker) need the same
   capability with the same semantics. Until then, the code lives in the product that needs it.
2. **It is a cross-product contract.** The document package, the Arc Scene contract, the Hub API
   and the automation protocol are defined here even before a second user exists.
3. **It is a foundation every product must share for consistency:** the command model, the
   interaction standards of the UI kit, units and tolerances.

Domain object types (walls, alignments, features, rigs) never live in `arc-core`. A product
proposes a promotion through an issue that names both users and the API surface; the move happens
in one `arc-core` pull request plus one pull request per product, and the promoted code keeps its
tests and error handling. The reverse also applies: a crate or module with a single remaining user
moves back to that product.

## Repository layout (target)

```
arc-core/
  crates/          one directory per crate in the layer table, plus the FFI crates
  apps/
    arc-shell/     reference desktop shell (L6 wiring only)
    arc-shell-cli/ reference headless CLI following the shared CLI conventions
    arc-worker/    out-of-process job host shipped with every product
  clients/python/  official Python client of the automation protocol
  schemas/         JSON Schemas of the automation protocol, Arc Scene and package descriptors
  wit/             WebAssembly plug-in interface definitions
  locales/         Fluent catalogs for strings owned by arc-core
  fixtures/        small generated test inputs and the package compatibility collection
  xtask/           layers, wasm, ci, corpus, bench and release tasks
  docs/
```
