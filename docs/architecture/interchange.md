# Interchange

The `arc-io-*` crate family reads and writes open and published formats for every product. Each
crate is a faithful, bounded implementation of its format; the semantics of the suite's
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
| `arc-io-ifc` | IFC2x3, IFC4, IFC4.3 (STEP physical file and IFC-ZIP); IDS | Read; write IFC4 and IFC4.3; read IDS | `ifc-lite`, WRAP; IDS parser BUILD | MPL-2.0, used unmodified | In-process with limits | IfcOpenShell run out of process: schema validation and geometry comparison |
| `arc-io-step` | STEP AP203, AP214, AP242; IGES 5.3 | Read and write | Through `arc-brep` (kernel tier), WRAP; product structure and metadata model BUILD | LGPL-2.1 with exception (kernel, dynamic) | Import in `arc-worker` | Re-read in a fresh worker with topology and mass-property comparison; an independent open-source STEP reader run out of process |
| `arc-io-mesh` | glTF 2.0 (`.gltf`, `.glb`, core extensions for materials, texture transform, mesh quantization and instancing), OBJ with MTL, STL (ASCII, binary), PLY, 3MF | Read and write | `gltf` ADOPT; `tobj` ADOPT (read); `stl_io` ADOPT; `threemf` ADOPT; PLY and OBJ writer BUILD; textures through `image` and `exr` | MIT or Apache-2.0; BSD-3 (`exr`) | In-process with limits | Khronos glTF validator run out of process; round-trip comparison |
| `arc-io-usd` | OpenUSD (`.usda`, `.usdc`, `.usdz`) | Read and write | `openusd` (pure Rust), ADOPT | MIT | In-process with limits | OpenUSD's checker tool run out of process |
| `arc-io-pointcloud` | LAS 1.2 to 1.4, LAZ, E57 | Read and write | `las`, `laz`, `e57`, ADOPT; level-of-detail octree cache BUILD | MIT, Apache-2.0 | In-process with limits; octree build on the worker pool | Round trip and point statistics compared with an open-source point-cloud tool run out of process |
| `arc-io-geo` | Coordinate reference systems; Shapefile, GeoPackage, FlatGeobuf, GeoJSON; GeoTIFF elevation and imagery; LandXML 1.2; CityJSON | Read (all); write GeoPackage, GeoJSON, LandXML, CityJSON | `proj` ADOPT (PROJ library and database); `geo`, `geozero`, `shapefile`, `flatgeobuf` ADOPT; GeoPackage reader on SQLite, GeoTIFF subset, LandXML and CityJSON BUILD | MIT, Apache-2.0 | In-process with limits | GDAL tools run out of process; LandXML schema validation |
| `arc-io-pdf` | PDF output (vector, PDF/A-2b option), SVG output, PDF input (vector content and raster underlays) | Write PDF and SVG; read PDF | `krilla` ADOPT (output); SVG writer BUILD; `lopdf` ADOPT (vector import); PDFium WRAP (rasterization) | MIT or Apache-2.0; PDFium BSD-3 | Output and vector parsing in-process; PDFium only in `arc-worker` through `arc-pdfium` | PDF structure checker and PDF/A validator run out of process |
| `arc-io-bcf` | BCF 2.1 and 3.0 (`.bcfzip`) | Read and write | BUILD on bounded ZIP and XML readers | Apache-2.0 (Arc) | In-process with limits | Published BCF XML schemas |

Fidelity levels (view, data, edit) are stated by each product per format and direction in its
`docs/architecture/interoperability.md`; a format crate's job is to make the data available
without loss.

Formats without a published specification are not read or written; they are exchanged through the
open formats above.

## Hostile-input limits

Every reader treats its input as hostile. Limits are values of a **limits profile** passed by the
caller; exceeding one stops the import with `io.limit_exceeded` naming the limit, never with a crash
or unbounded allocation.

| Limit | `strict` | `default` | `large-models` |
|---|---|---|---|
| Input file size | 256 MiB | 4 GiB | 64 GiB |
| Total decompressed size (ZIP, zstd, LAZ, deflate streams) | 1 GiB | 16 GiB | 256 GiB |
| Decompression ratio per stream | 100 : 1 | 200 : 1 | 500 : 1 |
| Record or entity count | 1 million | 50 million | 1 billion |
| Nesting depth (blocks, XML elements, IFC aggregation, USD composition) | 32 | 64 | 128 |
| Single string length | 1 MiB | 16 MiB | 64 MiB |
| Single array length (vertices, points per entity, knots) | 1 million | 50 million | 500 million |
| Memory budget for the import | 2 GiB | 16 GiB | Available memory minus 2 GiB |
| Wall-clock time | 60 s | none | none |

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
| Items | Outcome, severity, source identifier and record kind, Arc object id (if any), reason code, message identifier and arguments |

Reports are JSON (schema in `schemas/loss-report.schema.json`) with a rendered human-readable view
in the application and CLI. Reason codes are stable. The CLI writes the report beside the output or
to the path given; `--fail-on-loss` turns `dropped` and `failed` items into a non-zero exit.

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
