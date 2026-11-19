# Interchange

The `arc-io-*` crate family reads and writes open and published formats for every product, and the
Arc exchange package that connectors running inside other applications use. Each crate is a
faithful, bounded implementation of its format; the semantics of the suite's
interoperability policy (fidelity levels, preserved data, loss reports, independent validation)
are realized here and in the products' mapping crates.

## The neutral-model rule

- A format crate exposes a **neutral model** of its format: the format's own entities, attributes
  and structure, typed in Rust, with nothing product-specific. `arc-io-ifc` exposes IFC entities;
  `arc-io-dxf` exposes DXF sections, tables, blocks and entities.
- A format crate never depends on a product and never creates document objects of a product type.
  The **product owns the mapping** between the neutral model and its domain model in its
  `<product>-io` crate, including which fidelity level it reaches.
- Shared mappings to core types live with the core type: drafting display lists to PDF in
  `arc-io-pdf`, the Arc Scene to USD in `arc-scene`.
- Every reader accepts a limits profile and a cancellation token and reports progress; every writer
  is deterministic for the same input.
- Readers keep source identifiers on every neutral-model entity so products can fill the document's
  source map, and keep unmodeled records as preserved data in a form the same crate can write back.

## Formats

Status words: **ADOPT** (third-party library used directly), **WRAP** (third-party library used
only behind the crate's own API, replaceable), **BUILD** (Arc-owned implementation).

| Crate | Format | Directions | Library and status | License of library | Isolation | Validation oracle in CI |
|---|---|---|---|---|---|---|
| `arc-io-dxf` | DXF, ASCII and binary, versions R12 to 2018 | Read; write 2000 to 2018 | Arc-owned reader and writer from the published DXF reference, BUILD | Apache-2.0 (Arc) | In-process with limits | An independent open-source DXF reader run out of process; round-trip comparison |
| `arc-io-dwg` | DWG R13 to R2018 | Read (phase 1); write (phase 2) | `acadrust`, WRAP behind a byte-level API (bytes in, neutral DXF-shaped model out) | MPL-2.0, used unmodified | Separate crate boundary; optional execution in `arc-worker` | Round trip through DXF; an open-source DWG reader run out of process in a container, never shipped |
| `arc-io-ifc` | IFC2x3, IFC4, IFC4.3 (STEP physical file and IFC-ZIP); IDS | Read; write IFC4 and IFC4.3; read IDS | STEP physical file reader and typed views BUILD from ISO 10303-21 and the IFC schemas; IDS parser BUILD | Apache-2.0 (Arc) | In-process with limits | IfcOpenShell run out of process: schema validation and geometry comparison |
| `arc-io-step` | STEP AP203, AP214, AP242; IGES 5.3 | Read and write | Through `arc-brep` (kernel tier), WRAP; product structure and metadata model BUILD | LGPL-2.1 with exception (kernel, dynamic) | Import in `arc-worker` | Re-read in a fresh worker with topology and mass-property comparison; an independent open-source STEP reader run out of process |
| `arc-io-mesh` | glTF 2.0 (`.gltf`, `.glb`, core extensions for materials, texture transform, mesh quantization and instancing), OBJ with MTL, STL (ASCII, binary), PLY, 3MF | Read and write | `gltf` ADOPT; `threemf` ADOPT; OBJ, STL and PLY BUILD; textures through `image` and `exr` | MIT or Apache-2.0; BSD-3 (`exr`) | In-process with limits | Khronos glTF validator run out of process; round-trip comparison |
| `arc-io-usd` | OpenUSD (`.usda`, `.usdc`, `.usdz`) | Read and write | `openusd` (pure Rust), ADOPT | MIT | In-process with limits | OpenUSD's checker tool run out of process |
| `arc-io-pointcloud` | LAS 1.2 to 1.4, LAZ, E57 | Read and write | `las`, `laz`, `e57`, ADOPT; level-of-detail octree cache BUILD | MIT, Apache-2.0 | In-process with limits; octree build on the worker pool | Round trip and point statistics compared with an open-source point-cloud tool run out of process |
| `arc-io-geo` | Coordinate reference systems; Shapefile, GeoPackage, FlatGeobuf, GeoJSON; GeoTIFF elevation and imagery; LandXML 1.2; CityJSON | Read (all); write GeoPackage, GeoJSON, LandXML, CityJSON | `proj` ADOPT (PROJ library and database); `geo`, `geozero`, `shapefile`, `flatgeobuf` ADOPT; GeoPackage reader on SQLite, GeoTIFF subset, LandXML and CityJSON BUILD | MIT, Apache-2.0 | In-process with limits | GDAL tools run out of process; LandXML schema validation |
| `arc-io-pdf` | PDF output (vector, PDF/A-2b option), SVG output, PDF input (vector content and raster underlays) | Write PDF and SVG; read PDF | `krilla` ADOPT (output); SVG writer BUILD; `lopdf` ADOPT (vector import); PDFium WRAP (rasterization) | MIT or Apache-2.0; PDFium BSD-3 | Output and vector parsing in-process; PDFium only in `arc-worker` through `arc-pdfium` | PDF structure checker and PDF/A validator run out of process |
| `arc-io-bcf` | BCF 2.1 and 3.0 (`.bcfzip`) | Read and write | BUILD on bounded ZIP and XML readers | Apache-2.0 (Arc) | In-process with limits | Published BCF XML schemas |
| `arc-io-exchange` | Arc exchange package (`.arcx`), see [below](#the-arc-exchange-package) | Read and write | BUILD on bounded ZIP readers; `serde_json`, `jsonschema`, `blake3` ADOPT | Apache-2.0 (Arc); MIT; MIT; CC0-1.0 or Apache-2.0 | In-process with limits | The published JSON Schemas and the independent validator in the connector repository |
| `arc-io-3dm` | Rhino 3DM, versions 5 to 8 | Read | openNURBS behind the FFI crate `arc-opennurbs`, WRAP; a pure-Rust reader replaces it when it passes the corpus | openNURBS license (permissive, BSD-style; needs an allowlist entry before first use) | FFI crate; runs only in `arc-worker` | An independent open-source 3DM reader run out of process; geometry and layer comparison |
| `arc-io-fbx` | FBX 7.x binary | Read | `fbxcel` and `fbxcel-dom`, ADOPT; scene mapping BUILD | MIT or Apache-2.0 | In-process with limits | Scene hierarchy, mesh and animation-curve comparison against an independent open-source reader run out of process |
| `arc-io-dgn` | DGN V7 (ISFF) | Read | Arc-owned reader from the published specification, BUILD | Apache-2.0 (Arc) | In-process with limits | An independent open-source DGN reader run out of process; element-count and extent comparison |
| `arc-io-bridge` | Output of a licensed bridge (exchange package or an open format above) | Run and read | Process host and descriptor handling, BUILD; no third-party code | Apache-2.0 (Arc); the bridge library keeps its own license and is never distributed | Separate process started by `arc-worker` | The bridge output passes the validator of the format it returns |

Fidelity levels (view, data, edit) are stated by each product per format and direction in its
`docs/architecture/interoperability.md`; a format crate's job is to make the data available
without loss.

Data in formats without a published specification is exchanged by connectors through the Arc exchange package, through a licensed bridge, or through the open formats above ([suite decision](https://github.com/ACKaraca/arc-suite/blob/main/docs/adr/0017-exchange-connectors-and-licensed-bridges.md)).
A proprietary format is read natively only when a published specification or a permissively
licensed open reader exists, as for 3DM, FBX and DGN V7. DGN V8 and other formats without either
go through a connector or a bridge.

## Hostile-input limits

Every reader treats its input as hostile. Limits are values of a **limits profile** passed by the
caller; exceeding one stops the import with `io.limit_exceeded` naming the limit, never with a crash
or unbounded allocation.

| Limit | `strict` | `default` | `large-models` | `constrained` |
|---|---|---|---|---|
| Input file size | 256 MiB | 4 GiB | 64 GiB | 512 MiB |
| Total decompressed size (ZIP, zstd, LAZ, deflate streams) | 1 GiB | 16 GiB | 256 GiB | 2 GiB |
| Decompression ratio per stream | 100 : 1 | 200 : 1 | 500 : 1 | 100 : 1 |
| Record or entity count | 1 million | 50 million | 1 billion | 10 million |
| Nesting depth (blocks, XML elements, IFC aggregation, USD composition) | 32 | 64 | 128 | 32 |
| Single string length | 1 MiB | 16 MiB | 64 MiB | 4 MiB |
| Single array length (vertices, points per entity, knots) | 1 million | 50 million | 500 million | 10 million |
| Memory budget for the import | 2 GiB | 16 GiB | Available memory minus 2 GiB | 1 GiB |
| Wall-clock time | 60 s | none | none | 120 s |

The `constrained` profile is the default in browser and tablet or phone builds, where memory is
limited and the operating system may end a process that exceeds it
([platform targets](overview.md#platform-targets)).

Further rules: readers use bounded readers for ZIP, XML and binary streams; recursion is converted
to explicit stacks where depth is input-driven; every count read from a file is checked against
the remaining input size before allocation; references between records are resolved through
bounded indexes, and cycles are detected. Every reader has a fuzz target and a "hostile input does
not panic" test over a corpus of malformed files.

## Preserved foreign data

What a product does not model is kept. Readers emit unmodeled records as **preserved records**:
format, version, source identifier, record kind, and data in a form the same crate's writer accepts
(raw tag groups for DXF, entity instances for IFC, prim specs for USD, extension JSON for glTF).
Products store them in the package (`arc_foreign`) attached to the nearest object they model. On
export to the same format, the writer re-emits preserved records whose owners still exist, with
references remapped; preserved records whose owners were deleted are dropped and listed in the
export loss report.

## Loss reports

Every import and export produces a loss report, also when nothing was lost.

| Field | Content |
|---|---|
| Header | Format, version, direction, file name and hash, application and version, time, limits profile |
| Summary | Counts by outcome: `mapped`, `approximated`, `preserved`, `dropped`, `failed` |
| Items | Outcome, severity, source identifier and record kind, Arc object id (if any), reason code, message identifier and arguments. At most 1,024 distinct items plus one overflow item per reason; further record kinds are folded into that overflow item, and the report is marked `truncated` |

Reports are JSON (schema in `schemas/loss-report.schema.json`) with a rendered human-readable view
in the application and CLI. Reason codes are stable. The CLI writes the report beside the output or
to the path given; `--fail-on-loss` turns `dropped` and `failed` items into a non-zero exit.

## Open readers for proprietary formats

| Format | Basis | Notes |
|---|---|---|
| 3DM | openNURBS, the open-source reference library of the format's owner, behind `arc-opennurbs` | Reads layers, objects with attributes, instance definitions, NURBS curves and surfaces, B-rep, meshes, extrusions, materials and user strings. Extrusions and subdivision objects keep their parametric definition in the neutral model. Runs only in `arc-worker` because it is native code parsing untrusted files. Data written by third-party extensions is preserved as opaque records. |
| FBX | `fbxcel` open reader library | Reads the node hierarchy, meshes with attributes, materials and textures, cameras, lights, skeletons, skins, blend shapes and animation curves with their stacks. Binary FBX 7.x only; ASCII files fail with `io.unsupported_version`. The vendor's SDK is not used. |
| DGN V7 | The published ISFF specification | Reads 2D and 3D elements (lines, shapes, arcs, ellipses, B-spline curves and surfaces, text, cells, complex chains and shapes), levels, color tables and reference attachments. DGN V8 has no published specification and is not read natively. |

Each neutral model keeps source identifiers (3DM object ids, FBX node ids, DGN element offsets and
levels) and preserved records, and produces a loss report like every other reader.

## The Arc exchange package

The **Arc exchange package** is the open container through which a **connector** exchanges a model
with an Arc product. A connector is an add-in that runs inside the user's own licensed copy of
another application, uses only that application's official public API, and reads or writes this
package. Connectors live in a separate repository, are written in whatever language the host
requires and are not Arc desktop applications. The package is not an Arc document: it carries a
snapshot of model data to be mapped, not a store with a journal. The decision and its alternatives
are in [ADR-0011](../adr/0011-exchange-package-container.md); the suite policy is
[ADR-0017](https://github.com/ACKaraca/arc-suite/blob/main/docs/adr/0017-exchange-connectors-and-licensed-bridges.md).

### Container

A package is a ZIP archive with the extension `.arcx`. Text is UTF-8 JSON; tabular data is JSON
Lines (one object per line); geometry is little-endian binary buffers. ZIP and JSON are readable in
every language a host may require, without a native library. No entry is encrypted; entries use
deflate or stored; paths are relative, contain no `..` and no duplicates, and are checked by the
bounded ZIP reader (CORE-IO-007).

```
manifest.json
tables/elements.jsonl        instances and system elements
tables/types.jsonl           types (symbols, families, assemblies)
tables/relationships.jsonl   hosting, containment, connections, joins
tables/materials.jsonl
tables/classifications.jsonl
tables/views.jsonl           view definitions
tables/sheets.jsonl          sheets, viewports and schedules
geometry/index.jsonl         one record per geometry definition
geometry/<xid>.bin           mesh, curve and B-rep buffers
drafting/<view xid>.jsonl    2D drafting geometry of one view
assets/<blake3>              textures, images, embedded files
foreign/<name>               preserved host data, opaque to Arc
report/producer-report.json  what the connector could not read or write
```

### Logical contents

Every record has an **exchange identifier** (`xid`, a string of at most 128 characters unique in
the package) and a `source` object. Records refer to each other only by `xid`.

| Content | Carried data |
|---|---|
| Geometry | Definitions of three kinds, each placed by instances with an `f64` affine transform. **B-rep** bodies: vertices, edges with curve geometry, loops, faces with analytic (plane, cylinder, cone, sphere, torus) or NURBS surfaces, shells, orientation and tolerances. **Meshes**: indexed polygon meshes with optional normals, texture coordinates, creases, per-face material indices and per-vertex attributes. **Curves and points**: lines, arcs, ellipses, NURBS curves, polylines with bulges and point sets. A definition may carry several representations of one body (for example B-rep with a mesh for display); the reader picks the richest it supports. |
| Elements | Name, host category, an optional IFC class, classification references, the type, geometry references with a role (body, footprint, axis, opening profile), material assignments (including compound layers with thickness and function), phase, design-option membership, status flags and the element's parameters. |
| Types and instances | A type holds shared parameters, a family name, a category and optional type geometry; an instance refers to its type and adds instance parameters. Families record whether they are system-defined or loadable and, when the host exposes it, their parameter definitions with formulas. |
| Parameters | Name, group, storage kind (number, integer, text, boolean, reference, enumeration), value, quantity dimension and display unit for numbers, optional formula text, read-only flag and a shared identifier (UUID) when the host has cross-project parameters. Numbers are stored in the SI-coherent unit of their dimension: lengths in meters, angles in radians; the display unit is a hint. Formulas are carried as text in the host's syntax and as an `expr` in the bounded expression language of `arc-solve` when the connector can translate them. |
| Relationships | Typed edges between records (table below), each with optional attributes such as the end of a join or a port flow direction. |
| Materials | Name, class, appearance (base color, metalness, roughness, opacity, emission, textures as assets, aligned with the `arc-scene` material subset), named physical properties with quantities, and surface and cut pattern names. |
| Classification | Classification system name and edition, item code, title and the elements that carry it. |
| Views | Kind (plan, section, elevation, detail, drafting, three-dimensional), name, scale, cut frame or view range, crop region, detail level, discipline and the elements shown. Plans, sections, elevations and details carry **drafting geometry**: lines, polylines with bulges, arcs, circles, ellipses, splines, text with style, dimensions (measured value, defining points, style), hatches (loops with pattern name or solid color), symbol references and raster images, each with a layer or category, color, lineweight in millimeters, linetype and an optional link to its source element. |
| Sheets and schedules | Sheet number, name, size, title-block fields, viewports (view, placement, scale) and schedules as typed grids (columns with quantity dimensions, rows with source elements). |
| Levels, grids, reference planes | Elements of categories `level`, `grid` and `reference_plane` with their geometry; relationships use them as hosts and constraints. |
| Source identifiers | `source` holds the host's name for itself (`host`), the session id of the object, the persistent unique id, the native class and, for data that came from Arc, the Arc `ObjectId` of the object being returned. These fill the document's source map and make round trips update instead of duplicate. |

Relationship kinds:

| Kind | Meaning |
|---|---|
| `hosted_by` | An element placed in or on another (door in wall, fixture on a ceiling) |
| `contained_in` | Spatial containment (element in room, storey or building) |
| `aggregates` | Whole and parts (assembly and members, stair and flights) |
| `joins` | Join between elements, with the end or face and the join kind |
| `connects` | Port-to-port connection of building-services elements, with flow direction |
| `voids` and `fills` | An opening cuts an element; an element fills an opening |
| `member_of` | Membership of a group, system or design option |
| `constrained_to` | Offset or alignment to a level, grid or reference plane |
| `references` | A view, tag or dimension referring to an element |

### Manifest

`manifest.json` holds:

| Field | Content |
|---|---|
| `format` and `schema_version` | The string `arc-exchange` and the content schema version `major.minor` of every table and buffer layout |
| `package_id`, `revision`, `base` | UUID of the package, a counter, and for an incremental update the `package_id` and `revision` it amends |
| `direction` | `host_to_arc` or `arc_to_host` |
| `producer` | Connector name and version, host application name and version, host API version and platform |
| `created` | Time of creation, UTC |
| `project` | Name, host project identifier, default units and, when known, the coordinate reference system and survey point |
| `frame` | Local origin offset in `f64` meters applied to all geometry |
| `tables` | For each file: path, record count and BLAKE3 hash |
| `extensions` | Namespaced optional features, each marked `required` or not |

### Validation

A package is validated in five levels; every failure is a loss-report item with a stable
`io.exchange.*` code, and the shared `validate` subcommand of the product CLIs runs all of them.

| Level | Checks | Example code |
|---|---|---|
| 1 Structure | ZIP safety and limits profile, required entries, manifest present and parseable | `io.exchange.structure` |
| 2 Schema | Every table and buffer header against the JSON Schemas in `schemas/exchange/`; schema version supported | `io.exchange.schema`, `io.exchange.version_unsupported` |
| 3 Integrity | BLAKE3 hash and record count of every file; duplicate `xid` | `io.exchange.hash_mismatch`, `io.exchange.duplicate_xid` |
| 4 References | Every `xid` reference resolves; relationships have no cycles where a tree is required; types exist for all instances | `io.exchange.dangling_reference` |
| 5 Semantics | Finite coordinates, valid index ranges, closed and consistently oriented B-rep shells (edge pairing and Euler characteristic), quantity dimensions consistent with the parameter's unit, unit-length rotations in transforms | `io.exchange.geometry_invalid`, `io.exchange.unit_mismatch` |

Failures at levels 1 to 3 stop the import. Failures at levels 4 and 5 drop the offending record and
report it; the rest of the package is imported. A package written by `arc-io-exchange` passes all
levels; the connector repository carries an independent validator that connectors run in their own
tests.

### Versioning

- The schema version is `major.minor`, independent of crate versions and of the document package
  format. A new minor version only adds optional fields, tables and extensions. A major version may
  change anything.
- A reader accepts every minor version of a major version it supports, keeps fields it does not know
  as preserved data and ignores unknown optional extensions. It refuses an unknown major version, or
  a `required` extension it does not implement, with `io.exchange.version_unsupported` and reads
  nothing.
- A reader supports the previous major version for at least one suite release. Fixtures for every
  released schema version live in `fixtures/exchange/` and are read by the current reader in CI.
- Changing the schema needs an ADR, new fixtures and a matching update of the connector repository.

### Mapping into a product

The format crate exposes the package as a neutral model (tables, geometry definitions, drafting
geometry) and never creates product objects. The product maps it in its `<product>-io` crate:

1. Read and validate with a limits profile. Every `source` becomes an entry of the source map, keyed
   by `host` and the persistent unique id.
2. Resolve each element's category and class to a product object type through a mapping table the
   product owns and users can extend; unmapped elements become generic objects with their
   parameters, or are preserved and reported.
3. Convert geometry: B-rep through `arc-brep`, meshes through `arc-mesh`, curves through `arc-geom`;
   instances become instances of shared definitions rather than copies.
4. Map types, parameters and relationships to the product's types, properties and references;
   parameters with a formula keep it as a property with an `expr` when present.
5. Keep everything unmapped in the package as preserved foreign data (`arc_foreign`) and list it in
   the loss report. The fidelity level reached (view, data or edit) is stated per category by the
   product in its interoperability document.
6. When a package carries `source` ids that exist in the document, the import is an update: changed
   records modify the existing objects through commands in one transaction, deleted records are
   proposed for deletion, new records are created.

On export to a host, the product writes the objects that changed since a base revision with their
`ObjectId` in `source`, so the connector updates the matching host elements and reports what the
host rejected in `producer-report.json`.

## Licensed bridges

A **licensed bridge** lets a product use a conversion library that a vendor or consortium licenses
to the user, without Arc distributing, linking or depending on it. The mechanism lives in
`arc-io-bridge` and `arc-worker` and is specified by
[ADR-0017](https://github.com/ACKaraca/arc-suite/blob/main/docs/adr/0017-exchange-connectors-and-licensed-bridges.md).

- The user installs the library and a small adapter executable that wraps it and speaks the bridge
  contract. Arc ships no adapter for a licensed library; the contract is documented so anyone may
  write one.
- A **bridge descriptor** (TOML, in the user's configuration directory) names the adapter
  executable, the input formats it accepts, the output format it returns and its declared
  resource needs. The user enables a bridge per descriptor in settings; none is enabled by default.
  Arc never searches the system for libraries and never downloads one.
- A conversion is a `bridge.convert` job in `arc-worker`: the worker starts the adapter as a
  separate process with the input path and an empty output directory, no network access where the
  platform can deny it, a memory and wall-clock limit from the limits profile, and the framing of
  the Arc Automation Protocol on stdio for progress and cancellation. It needs the `process`
  permission class.
- The adapter returns an Arc exchange package or an open format from the table above. The output is
  read by the ordinary reader under the same limits profile and produces a loss report whose header
  names the bridge and its version. The input file is kept as the source of preserved records.
- Licensing is the library's own business: Arc does not inspect, check or circumvent license
  mechanisms. A missing, unlicensed or failing bridge fails the job with `bridge.unavailable` or
  `bridge.failed`, and no core workflow depends on it.
- Bridges run on desktop and on Hub workers; they are unavailable in the browser and on tablet and
  phone builds, which submit the job to a worker the user operates.

## What remains excluded

Reverse engineering undocumented formats or binaries, running compiled or protected script files of
other applications, and codecs whose license forbids open distribution are not provided, because
they create legal problems; data in such formats is exchanged through the mechanisms above.

## Corpus repositories

Interchange test files live outside `arc-core`, in one corpus repository per format family
(`arc-corpus-dwg`, `arc-corpus-ifc`, `arc-corpus-step`, and so on), created when the first importer
of the family is implemented.

Admission rules:

- Every file has one row in `LICENSES.tsv`: path, owner, author, license (an SPDX identifier from the
  suite allowlist or a written permission reference), origin (how and with which application it was
  made), description, and SHA-256. `SHA256SUMS` lists every file and is checked in CI.
- No sample files published by software vendors, no client or project work without written
  permission, no personal data, no files whose license forbids redistribution.
- Files are small: at most 20 MiB per file and 2 GiB per repository; larger models are generated by
  scripts kept in the corpus repository.
- Each file states the features it exercises and the expected outcome (round trip exact, round trip
  with listed losses, or rejected with a stated error).
- `arc-core` and products pin a corpus commit; they never track a moving branch. Updating the pin is
  a pull request that shows the new report.
- `cargo xtask corpus` runs every file: import, export to the same format, re-import, compare, and
  compare against the oracle; it writes a report (JSON and Markdown) that CI keeps as an artifact.
  The test suites pass without a corpus checkout; the corpus run is a separate job.
