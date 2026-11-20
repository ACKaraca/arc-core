# arc-io family

One crate per interchange format family, each exposing a neutral model of its format: `arc-io-dxf`,
`arc-io-dwg`, `arc-io-ifc`, `arc-io-step`, `arc-io-mesh`, `arc-io-usd`, `arc-io-pointcloud`,
`arc-io-geo`, `arc-io-pdf`, `arc-io-bcf`, `arc-io-exchange` (the Arc exchange package used by
connectors), `arc-io-3dm`, `arc-io-fbx`, `arc-io-dgn` and `arc-io-bridge` (licensed bridges). The
architecture, library choices and limits are specified in
[interchange.md](../architecture/interchange.md).

## Owns

- Readers and writers for each format, with limits profiles, cancellation and progress.
- The neutral model of each format, including source identifiers and preserved records.
- Loss-report items for format-level losses (unsupported versions, unknown records, approximations).
- The loss-report schema, the loss report types and their aggregator (`arc-io-common`, module `loss`),
  and the shared bounded readers for ZIP and XML used by the family.
- Fuzz targets, hostile-input tests and corpus runners for each format.
- Shared core mappings: drafting plot streams to PDF and SVG (`arc-io-pdf`).
- The Arc exchange package: container, schemas, validator, neutral model and fixtures
  (`arc-io-exchange`).
- The licensed-bridge host: descriptors, the `bridge.convert` worker job and output reading
  (`arc-io-bridge`).

## Stays in products

All mapping between neutral models (including the exchange package) and product domain models, the
element mapping tables that turn host categories into product object types, the fidelity level per format and
direction, product-specific export options and presets, and product-level loss items (what the
product could not map).

## Key concepts and types

| Concept | Description |
|---|---|
| Neutral model | Per crate: the format's own entities and structure as typed Rust values |
| `SourceId` | The format's identifier of an entity (DXF handle, IFC GlobalId, STEP entity name, USD path) |
| `PreservedRecord` | Format, version, source identifier, kind, data writable by the same crate |
| `ReadOptions`, `WriteOptions` | Limits profile, cancellation, progress, format options |
| `LossItem` | Outcome, severity, source identifier, record kind, reason code, message |
| `LossReport` | Header, summary counts and items, serializable to the published schema |

The loss report types and the aggregator are defined once, in the `loss` module of `arc-io-common`.
The format crates re-export them under their own paths (`arc_io_dxf::loss`, `arc_io_mesh::loss`,
`arc_io_ifc::loss`, `arc_io_pdf::loss`) and keep their reason codes and catalogs there. The aggregator
keeps one item per reason and record kind, with exact counts. Every format crate caps the
distinct items (`MAX_LOSS_ITEMS`, 1,024) plus one overflow item per reason, and folds the rest into
that overflow item. The overflow item keeps the arguments of the first folded occurrence, a record
kind is cut to 256 bytes, and a report that folded any occurrence is marked `truncated`; its counts
stay exact. The limit is the
same for every reader, so a file cannot grow a report without bound.

## Dependencies

| Crate | Arc | Third-party |
|---|---|---|
| `arc-io-dxf` | `arc-base`, `arc-io-common` (loss report types) | none |
| `arc-io-dwg` | `arc-base`, `arc-io-dxf` (neutral drawing model and reading options), `arc-io-common` (loss log) | `acadrust` (MPL-2.0, wrapped behind the byte-level reader; see below) |
| `arc-io-ifc` | `arc-base`, `arc-mesh`, `arc-io-common` (loss report types) | the STEP physical file reader and the typed views are implemented in the crate from ISO 10303-21 and the published IFC schemas, so no third-party parser is used; the ZIP archive reader is `arc-io-common` |
| `arc-io-step` | `arc-base`, `arc-brep`, `arc-command` (worker jobs) | none beyond the kernel tier |
| `arc-io-mesh` | `arc-base`, `arc-mesh`, `arc-io-common` (loss report types) | OBJ with MTL, STL and PLY are implemented in the crate; `gltf`, `threemf`, `image` and `exr` are for the formats that follow |
| `arc-io-usd` | `arc-base`, `arc-geom`, `arc-mesh` | `openusd` |
| `arc-io-pointcloud` | `arc-base`, `arc-geom`, `arc-doc` (asset storage of octrees) | `las`, `laz`, `e57` |
| `arc-io-geo` | `arc-base`, `arc-geom` | `proj`, `geo`, `geozero`, `shapefile`, `flatgeobuf`, `rusqlite`, `quick-xml` |
| `arc-io-pdf` | `arc-base`, `arc-geom`, `arc-draft`, `arc-command` (worker jobs), `arc-io-common` (loss report types), `arc-pdfium` (in `arc-worker` only) | `krilla`, `lopdf` |
| `arc-io-bcf` | `arc-base` | `quick-xml`, `zip`, `image` |
| `arc-io-exchange` | `arc-base`, `arc-geom`, `arc-mesh` | `zip`, `serde_json`, `jsonschema`, `blake3` |
| `arc-io-3dm` | `arc-base`, `arc-geom`, `arc-mesh`, `arc-command` (worker jobs), `arc-opennurbs` (in `arc-worker` only) | none beyond openNURBS |
| `arc-io-fbx` | `arc-base`, `arc-geom`, `arc-mesh` | `fbxcel`, `fbxcel-dom` |
| `arc-io-dgn` | `arc-base`, `arc-geom` | none |
| `arc-io-bridge` | `arc-base`, `arc-command` (worker jobs) | none |
| `arc-io-common` | `arc-base` | `miniz_oxide` (inflate), `quick-xml` (XML parsing), `serde` (loss report types) |

## Test strategy

Per crate: unit tests for every record kind; round-trip tests (read, write, re-read, compare neutral
models); a hostile-input test over malformed files asserting errors and no panics; a fuzz target per
reader; oracle comparison in the corpus job; determinism tests for writers (identical bytes for
identical input); limit tests that construct inputs exceeding each limit. Exchange package: fixtures for every released
schema version, one failing fixture per validation level and error code, a round trip through a
reference connector written in a second language, and a differential test of the Rust validator
against the independent validator. Bridge host: a test adapter that crashes, hangs, exceeds its
memory limit and emits an invalid package, each asserting that the document is untouched.

## Capabilities

### Shared

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-001 | Neutral models | Every reader produces a typed neutral model of its format with source identifiers on every entity and no product types. | T1 | 1 |
| CORE-IO-002 | Limits enforcement | Every reader enforces the `input_bytes`, `string_bytes`, `array_length` and `record_count` limits of the caller's profile and stops with `io.limit_exceeded` naming the limit before allocating beyond it. `nesting_depth` is enforced by every reader except DWG and PDF. `memory_bytes` and `wall_clock_seconds` are consulted by the IFC reader only. | T1 | 1 |
| CORE-IO-003 | Preserved records | Readers emit unmodeled records as preserved records; writers of the same format re-emit them with remapped references. | T1 | 1 |
| CORE-IO-004 | Loss reports | Every import and export returns a loss report conforming to the published schema, also when nothing was lost. | T1 | 1 |
| CORE-IO-005 | Deterministic writers | Writers produce identical bytes for identical neutral models and options. | T1 | 1 |
| CORE-IO-006 | Progress and cancellation | Readers and writers report progress and stop promptly on cancellation, returning `cancelled`. | T1 | 1 |
| CORE-IO-007 | Bounded ZIP and XML readers | Shared readers enforce entry counts, decompressed sizes, ratios, path safety and element depth for every ZIP- and XML-based format. | T1 | 1 |

### DXF and DWG

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-008 | DXF reading | Reads ASCII and binary DXF from R12 to 2018 into sections, tables, blocks, entities, objects and extended data; unknown groups are preserved. | T1 | 1 |
| CORE-IO-009 | DXF writing | Writes DXF versions 2000 to 2018, re-emitting preserved groups; files written re-read into an equal neutral model. | T1 | 1 |
| CORE-IO-010 | DXF encodings | Decodes legacy code-page text and Unicode escapes into UTF-8 and encodes back according to the target version. | T1 | 1 |
| CORE-IO-011 | DWG reading | Reads DWG R13 to R2018 through the byte-level wrapper into the same neutral drawing model as DXF, reporting unsupported objects as preserved or lost. | T1 | 1 |
| CORE-IO-012 | DWG writing | Writes DWG R2004 to R2018 from the neutral drawing model, validated by re-reading and by the out-of-process oracle. | T1 | 2 |
| CORE-IO-013 | DWG isolation | Runs DWG reading and writing in `arc-worker` when the caller requests isolation, with identical results. | T2 | 2 |

### IFC and BCF

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-014 | IFC reading | Reads IFC2x3, IFC4 and IFC4.3 files (STEP physical file and IFC-ZIP) into entity instances with GlobalIds, property sets, quantities, relationships and geometry representations. | T1 | 1 |
| CORE-IO-015 | IFC geometry | Evaluates IFC geometry representations (extrusions, swept solids, boolean results, tessellations, mapped items, B-rep faces) into meshes and curves with per-representation identifiers. | T1 | 1 |
| CORE-IO-016 | IFC writing | Writes IFC4 and IFC4.3 files from the neutral model; output passes schema validation by the out-of-process oracle. | T1 | 2 |
| CORE-IO-017 | IFC streaming | Reads large IFC files in a streaming pass with bounded memory for entity indexing and on-demand geometry evaluation. | T2 | 2 |
| CORE-IO-018 | IDS reading | Reads IDS files into neutral requirement specifications (applicability and requirement facets) for products to check. | T2 | 2 |
| CORE-IO-019 | BCF exchange | Reads and writes BCF 2.1 and 3.0 archives with topics, comments, viewpoints (camera, components, clipping planes) and snapshots. | T1 | 2 |

### STEP

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-020 | STEP product structure | Reads assembly structure, product names, colors, layers and units from STEP through the B-rep tier in `arc-worker`. | T1 | 1 |
| CORE-IO-021 | STEP writing | Writes product structure and shapes to STEP AP214 or AP242 with names and colors. | T1 | 3 |
| CORE-IO-022 | IGES exchange | Reads and writes IGES surfaces and solids through the same isolation path. | T2 | 3 |

### Meshes and USD

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-023 | glTF reading | Reads glTF 2.0 and GLB with scene hierarchy, meshes, PBR materials, textures, cameras, lights and the supported extensions; unsupported extensions are preserved. | T1 | 1 |
| CORE-IO-024 | glTF writing | Writes glTF and GLB with instancing, quantized attributes on request and embedded or external textures; output passes the validator out of process. | T1 | 1 |
| CORE-IO-025 | OBJ, STL and PLY | Reads and writes OBJ with MTL, ASCII and binary STL, and ASCII and binary PLY with vertex attributes. | T1 | 1 |
| CORE-IO-026 | 3MF | Reads and writes 3MF packages with meshes, materials, colors and build items. | T2 | 2 |
| CORE-IO-027 | USD reading | Reads USDA, USDC and USDZ with composition into a neutral stage model with prims, attributes, relationships and metadata. | T1 | 4 |
| CORE-IO-028 | USD writing | Writes USDA, USDC and USDZ stages from the neutral model; output passes the OpenUSD checker out of process. | T1 | 4 |

### Point clouds and geospatial

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-029 | LAS and LAZ | Reads and writes LAS 1.2 to 1.4 and LAZ with point formats, classifications, intensity, color and coordinate reference. | T1 | 3 |
| CORE-IO-030 | E57 | Reads and writes E57 with multiple scans, poses, color and intensity; embedded images are preserved. | T1 | 3 |
| CORE-IO-031 | Point-cloud octree cache | Builds a level-of-detail octree from point clouds of billions of points out of core, stored as document assets for streaming display. | T1 | 3 |
| CORE-IO-032 | Coordinate operations | Transforms coordinates between reference systems using the PROJ database, including vertical datums and grid shifts when grids are available, reporting accuracy. | T1 | 2 |
| CORE-IO-033 | GIS vector data | Reads Shapefile, GeoPackage, FlatGeobuf and GeoJSON features with attributes and reference system; writes GeoPackage and GeoJSON. | T1 | 3 |
| CORE-IO-034 | Elevation and imagery rasters | Reads GeoTIFF elevation grids and imagery with georeferencing for terrain and underlays. | T2 | 3 |
| CORE-IO-035 | LandXML | Reads and writes LandXML 1.2 surfaces, alignments, profiles, cross sections, parcels and pipe networks. | T1 | 3 |
| CORE-IO-036 | CityJSON | Reads and writes CityJSON city objects with semantics and levels of detail. | T3 | 3 |

### PDF and SVG

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-037 | PDF output | Writes plot streams to vector PDF with outline text as text over embedded subset TrueType fonts, layers as optional content groups and multiple pages. | T1 | 1 |
| CORE-IO-038 | SVG output | Writes plot streams to SVG with exact arcs, lineweights, linetypes and text. | T1 | 1 |
| CORE-IO-039 | PDF vector import | Reads vector content of PDF pages (paths, fills, text runs) into neutral 2D geometry with page transforms. | T2 | 2 |
| CORE-IO-040 | PDF raster underlays | Rasterizes PDF pages in `arc-worker` at requested resolution and region for underlay display. | T2 | 2 |

### Exchange package

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-041 | Exchange package reading | Reads an Arc exchange package into a neutral model of manifest, elements, types, parameters, relationships, materials, classifications, views, sheets and geometry definitions, with source identifiers on every record. | T3 | 5 |
| CORE-IO-042 | Exchange package writing | Writes deterministic packages from the neutral model with manifest hashes and counts; output passes all validation levels. | T3 | 5 |
| CORE-IO-043 | Exchange validation | Validates structure, schema, integrity, references and semantics in five levels and reports each failure with a stable `io.exchange.*` code in a loss report. | T3 | 5 |
| CORE-IO-044 | Exchange versioning | Reads every minor version of a supported major version, preserves unknown fields and optional extensions, and refuses unknown major versions and unimplemented required extensions without reading data. | T3 | 5 |
| CORE-IO-045 | Exchange geometry | Reads and writes B-rep bodies with analytic and NURBS geometry, indexed meshes, curves and shared definitions with `f64` placements, checking shell closure, orientation and index ranges. | T3 | 5 |
| CORE-IO-046 | Exchange drafting views | Reads and writes view definitions, sheets with viewports, schedules and 2D drafting geometry with styles and links to source elements. | T3 | 5 |
| CORE-IO-047 | Exchange incremental update | Reads packages that amend a base revision and writes change-only packages carrying the Arc `ObjectId` of each object, so a host update matches existing elements. | T3 | 5 |
| CORE-IO-048 | Producer report | Reads and writes the connector's producer report in the loss-report item shape and merges it into the import loss report. | T3 | 5 |

### Open readers for proprietary formats

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-049 | 3DM reading | Reads 3DM files through openNURBS in `arc-worker` into layers, objects with attributes, instance definitions, NURBS geometry, B-rep, meshes, materials and user strings, preserving unknown plug-in data. | T3 | 5 |
| CORE-IO-050 | FBX reading | Reads binary FBX 7.x into a neutral scene model with hierarchy, meshes, materials, cameras, lights, skeletons, skins, blend shapes and animation curves; ASCII files fail with `io.unsupported_version`. | T3 | 5 |
| CORE-IO-051 | DGN V7 reading | Reads DGN V7 files from the published specification into levels, 2D and 3D elements, cells, complex chains and shapes, text, color tables and reference attachments. | T3 | 5 |

### Licensed bridges

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-052 | Bridge descriptors | Loads user-installed bridge descriptors that name an adapter executable, accepted input formats and output format; every bridge is disabled until the user enables it, and none is searched for or downloaded. | T3 | 5 |
| CORE-IO-053 | Bridge conversion job | Runs `bridge.convert` in `arc-worker`: starts the adapter as a separate process with an empty output directory, limits and cancellation, then reads the result under the caller's limits profile; failures leave the document unchanged. | T3 | 5 |
| CORE-IO-054 | Bridge provenance | Records the bridge name and version in the loss report header and keeps the input file as the source of preserved records. | T3 | 5 |

### Constrained platforms

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-IO-055 | Constrained limits profile | Provides the `constrained` limits profile for browser and tablet or phone builds, and reports `io.limit_exceeded` with a suggestion to submit the import to a worker. | T3 | 5 |

## DXF as implemented

The mapping of the neutral model to the drafting types, with the import and export commands, is
in [arc-draft-dxf.md](arc-draft-dxf.md).

`arc-io-dxf` is the first part of the family. It depends on `arc-base` only (the table above also
allows `arc-geom`; the neutral model stores coordinates as plain `f64` arrays and needs nothing
from it), reads and writes byte buffers and streams, and builds for `wasm32-unknown-unknown`.

### Reading

- `read(bytes, options)` and `read_stream(reader, options)` accept ASCII and binary files. A
  binary file starts with the binary sentinel; one-byte group codes (R12 binary) and two-byte
  group codes are told apart by the first group. A leading UTF-8 byte-order mark is skipped.
- The version comes from `$ACADVER`: `AC1009` (R12), `AC1012`, `AC1014`, `AC1015`, `AC1018`,
  `AC1021`, `AC1024`, `AC1027` and `AC1032`. A file without the variable is read as R12; any other
  value fails with `io.unsupported_version`.
- Group values are typed by the published group-code ranges (`value_kind`). An ASCII group code
  outside the ranges keeps its value as text; in a binary file it fails with
  `io.dxf.group_code_invalid`. Numbers must be finite. 8-bit integers (codes 280–289) are stored
  as 0 to 255; negative ASCII values are taken as their unsigned byte. Comments (999) are skipped
  and counted.
- Errors carry the one-based line of the group-code line (ASCII) or the byte offset of the group
  (binary) as the detail `line` or `offset`; errors found while typing a record carry the position
  of the record's group 0.
- `ReadOptions` holds the limits profile (`constrained` by default), a `Cancel` token and a
  progress callback. Every limit is checked before the allocation it guards and fails with
  `io.limit_exceeded` whose argument `limit` names the field of `Limits`:

| Limit | Enforced on |
|---|---|
| `input_bytes` | The input length, before any parsing; `read_stream` reads at most one byte more |
| `record_count` | Records (groups 0) over the whole file |
| `string_bytes` | Every text value and binary chunk before decoding, and the joined content of multi-line text |
| `array_length` | Groups per record, and every count group of a hatch before its elements are read |
| `nesting_depth` | Section (1), record (2), table entry or block entity (3), vertex or attribute (one more), plus open `102` and `1002` braces |

  The decompression limits do not apply to DXF. The memory and wall-clock limits are not checked
  by the reader: memory is bounded by the limits above, and the caller ends a read by cancelling.
- Progress reports bytes read on the first record, every 1024 records and at the end; the token is
  checked before every record and a cancelled read returns `base.cancelled`.

### Text encodings

Files before 2007 are decoded with the code page of `$DWGCODEPAGE` (`ANSI_1252` when absent),
found by a first pass over the header before any text is decoded. The single-byte code pages 1250,
1251, 1252, 1253, 1254 and 1257 are implemented. A byte without a character in the page, and every
byte of 0x80 or above under any other page (including the East Asian multi-byte pages), becomes the
raw-byte character U+F700 + byte and is reported (`io.dxf.byte_unmapped` or
`io.dxf.code_page_unsupported`, outcome `approximated`); writing to the same code page restores the
byte. Files of 2007 and later are UTF-8; invalid UTF-8 fails with `io.dxf.text_invalid`. In every
version `\U+XXXX` escapes decode to their character, surrogate pairs combine, and malformed
escapes stay as written. `\M+` escapes are not decoded.

### The neutral model

`Drawing` holds the version, header variables (name and value groups typed by group code), classes,
the symbol tables, blocks, entities, objects and preserved sections. Every record starts with a
`Head`: its handle (the source identifier; group 5, or 105 for dimension styles), owner (the first
330 before the subclass markers) and application groups (`102` braces before the markers).
Entities carry `EntityProps`: paper space, layout name, layer, linetype, color in its three forms
(number, true color, color name), lineweight, transparency, linetype scale, visibility,
elevation, thickness and extrusion. Every record keeps groups the model does not type as `extras`
and extended data as `XData` per application.

| Kind | Typed |
|---|---|
| Tables | VPORT, LTYPE (with pattern elements), LAYER, STYLE, VIEW, UCS, APPID, DIMSTYLE (dimension variables stay groups by code), `BLOCK_RECORD` (with preview bytes) |
| Entities | POINT, LINE, RAY, XLINE, CIRCLE, ARC, ELLIPSE, LWPOLYLINE (bulges, widths, vertex ids), POLYLINE with VERTEX and SEQEND records, SPLINE (knots, weights, control and fit points), TEXT, MTEXT (raw content joined from its chunks), ATTRIB, ATTDEF, INSERT with ATTRIB and SEQEND records, DIMENSION (all kinds as data), LEADER, HATCH (polyline and edge loops with line, arc, ellipse and spline edges, source handles, pattern lines, seed points), SOLID, TRACE, 3DFACE, VIEWPORT, IMAGE, WIPEOUT, BLOCK, ENDBLK |
| Objects | DICTIONARY, XRECORD, LAYOUT (plot settings stay groups), GROUP |
| Preserved | Every other section, table, entity type and object type as `PreservedRecord`s with all groups in order; unknown groups of typed records as `extras`; groups after a `101` embedded-object marker as `extras` |

Subclass markers (100) of typed records are not stored; the writer writes the markers of each
typed kind. Table entry counts and list counts are derived. Preserved groups of a typed record are
written after its typed groups, so their position relative to typed groups is not kept.

### Writing

`write(drawing, options)` writes ASCII (or binary with `binary: true`) DXF of versions 2000 to
2018; earlier targets fail with `io.unsupported_version`. Output is deterministic: sections in the
order header, classes, tables (VPORT, LTYPE, LAYER, STYLE, VIEW, UCS, APPID, DIMSTYLE,
`BLOCK_RECORD`, then preserved tables), blocks, entities, objects, preserved sections; numbers in
the shortest decimal form that reads back to the same `f64`; integers in decimal; lines ending in
CR LF. Handles are kept; a record without a handle, or with the handle of a record written before
it, receives the next free handle above the highest handle of the model (`io.dxf.handle_assigned`),
a renumbered polyline or block reference has the owner of its vertex and attribute records
rewritten, a preserved record's group 5 is rewritten the same way, and `$HANDSEED`, when present,
is set to the next free handle. Before 2007, text is encoded in the drawing's code page and
characters outside it become `\U+XXXX` escapes; line breaks, NUL and a backslash that starts `U+`
are always escaped, so every string reads back unchanged. Multi-line text is split into chunks of
250 characters. Hatch spline fit data is written for 2010 and later and reported as dropped for
earlier targets. The writer does not create structure the model lacks (block records, model and
paper space blocks, the root dictionary); a model without a block record table is reported with
`io.dxf.structure_incomplete`. A value that does not fit its group code fails with
`io.dxf.write_value_invalid`.

### Loss reports

Every successful read and write returns a `LossReport` serializable to
`schemas/loss-report.schema.json`. Items are aggregated per reason and record kind; the argument
`count` gives the number of occurrences and `source_id` the handle of the first. `summary.mapped`
counts typed records. The crate leaves file name, hash, application and time empty for the caller.

| Reason | Outcome |
|---|---|
| `io.dxf.section_preserved` | `preserved` |
| `io.dxf.table_preserved` | `preserved` |
| `io.dxf.record_preserved` | `preserved` |
| `io.dxf.groups_preserved` | `preserved` |
| `io.dxf.comment_dropped` | `dropped` |
| `io.dxf.code_page_unsupported` | `approximated` |
| `io.dxf.byte_unmapped` | `approximated` |
| `io.dxf.fit_data_dropped` | `dropped` |
| `io.dxf.raw_bytes_written` | `approximated` |
| `io.dxf.handle_assigned` | `mapped` |
| `io.dxf.structure_incomplete` | `approximated` |

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `io.limit_exceeded` | `limit_exceeded` | A limit of the profile was exceeded; the argument `limit` names it |
| `io.unsupported_version` | `unsupported` | The file's version is not read, or the target version is not written |
| `io.stream_failed` | `io` | Reading from a stream failed |
| `io.dxf.truncated` | `corrupt_data` | The input ends inside a group, section, table, block or sequence |
| `io.dxf.group_code_invalid` | `corrupt_data` | A group code is not a non-negative integer, or is unknown in a binary file |
| `io.dxf.value_invalid` | `corrupt_data` | A value does not parse as its code's type, is out of range or is not finite |
| `io.dxf.structure_invalid` | `corrupt_data` | Sections, tables, blocks, records or hatch data are not arranged as the format requires |
| `io.dxf.text_invalid` | `corrupt_data` | Text of a 2007 or later file is not valid UTF-8 |
| `io.dxf.write_value_invalid` | `invalid_input` | A model value cannot be written |

The `io.*` codes without a format segment are registered here because this is the first crate of
the family; they move to a shared registry when a second format crate needs them.

## DWG reading as implemented

`arc-io-dwg` is the first DWG part (CORE-IO-011). It reads DWG files of versions R13 to R2018 from
bytes and returns the neutral drawing model of `arc-io-dxf` with a loss report. The file is read by
the library `acadrust` 0.6.8 (MPL-2.0, used unmodified), which is wrapped so that no type of the
library appears in a signature, a result or an error. The layer amendment and the reasons for the
same-layer edge to `arc-io-dxf` are in [ADR-0001](../adr/0001-enforced-layering.md#amendment-dwg-reader-over-a-wrapped-library).
The crate is in the WebAssembly set; its build for `wasm32-unknown-unknown` succeeds with the
library's default features off. Reading DWG files that are not trusted is experimental until
parsing runs in a separate worker process (CORE-IO-013).

### Reading

- `read(bytes, options)` takes the options of the DXF reader (`ReadOptions`: limits profile,
  cancellation token and progress callback) and returns `ReadOutput`. The DWG file is never read by
  path.
- Before the library is called, two checks run in this order. The input size is compared with
  `input_bytes`. The first six bytes must be `AC` followed by four digits, and the version they name
  must be one of the signatures below.

| Signature | Release |
|---|---|
| `AC1012` | R13 |
| `AC1014` | R14 |
| `AC1015` | R2000 |
| `AC1018` | R2004 |
| `AC1021` | R2007 |
| `AC1024` | R2010 |
| `AC1027` | R2013 |
| `AC1032` | R2018 |

  Four-digit signatures outside the table, such as R12 (`AC1009`), are refused with
  `io.dwg.version_unsupported`. Input that does not begin with `AC` and four digits, including the
  `AD` prefix, is refused with `io.dwg.signature_invalid`. The release names follow the version
  families the library documents.
- The library reads the bytes through a cursor. Its decompression budget is the profile's
  `decompressed_bytes`. The failsafe mode of the library is off: a damaged file is an error, not a
  repaired drawing. A budget refusal of the library is reported as `io.limit_exceeded` with the
  argument `limit` set to `decompressed_bytes`; the refusal is recognized by the library's message
  text, and a test of this crate fails if that text changes.
- The read, the whole mapping and the drop of the library's document run inside one
  `catch_unwind` (the isolation boundary of `library.rs`), in native builds only. A panic anywhere
  in that span, including in a library accessor, a destructor or the progress callback, becomes
  `io.dwg.library_failed`. Only the neutral drawing and report leave the boundary. Cancellation and
  limit errors are not panics and keep their own codes. The WebAssembly build has no such boundary:
  there a panic aborts the module, so the library's panics are not contained. The boundary also does
  not protect against aborts, stack overflows or unbounded memory use inside the library; running the
  read in `arc-worker` is the intended hardening, and it is required before untrusted files are read
  (CORE-IO-013).
- The library bounds some per-record allocations only by the record length times the size of the
  element, so a small multiline, table or mesh record can make the library reserve memory far larger
  than the record. This crate cannot prevent that; untrusted files are read only in the worker
  process (CORE-IO-013).
- The library's mesh budget is set to zero faces and zero face indices (`with_mesh_limits(0, 0)`):
  a MESH record is read without its faces and reported as not carried. The library still probes host
  memory for its default budget, which reserves nothing.
- The library's decompression budget is the profile's `decompressed_bytes`, as above.
- After the library returns, the model is checked against the profile. `record_count` counts every
  entity, block entity, table entry, polyline vertex and insert attribute, as the DXF reader counts
  its records; vertices and attributes count when they are mapped. `array_length` bounds the
  vertices, control and fit points, knots, boundary paths, the edges of a boundary path, boundary
  handles, pattern lines and dashes, linetype elements and preview bytes of each record.
  `string_bytes` bounds every text value and name, including layer, linetype and color names of
  entities, and the names of dimension blocks, styles and text. A failure is `io.limit_exceeded`
  naming the field. These limits bound the mapped model; the library has already allocated the file
  when they apply.
- Cancellation is checked before the library call, after it, and before every record of the mapping.
  The library call cannot be cancelled once it has started. Progress is reported while the drawing is
  mapped, every 1024 records and at the end; the total is the number of entities.
- A number that is not finite is not passed through. The record or field that holds it is skipped and
  counted in `io.dwg.value_skipped`: a header variable with a number that is not finite (the report names it, for example `$LTSCALE`), a linetype with a length that is
  not finite, a text style with a number that is not finite, and the base point of a block (which
  becomes the origin). An entity with such a number is invalid and is skipped, and an insert with an
  attribute whose number is not finite is skipped as a whole.

### Test fixtures

The `testing` feature (off by default, not part of the WebAssembly build) exposes
`arc_io_dwg::testing::sample_dwg_bytes`, which builds a small R2018 drawing in memory, writes it with
the library's writer and returns the DWG bytes. The drawing has a layer, a line, a block with a circle
whose record units are inches, an insert of the block and a fixed fingerprint and version identifier.
Products enable the feature in their dev-dependencies to read the fixture in their own tests. The
library's writer is the only writer used; no file from another product is involved.

### Mapping

| Library data | Neutral model |
|---|---|
| Header variables | Header variables under their DXF names and group codes, the same names the DXF reader keeps. Mapped: the identity values `$FINGERPRINTGUID` and `$VERSIONGUID` (group 2), `$HANDSEED` (group 5, hexadecimal text of the library's handle seed), `$DWGCODEPAGE`, `$MENU`, `$PROJECTNAME`, `$LASTSAVEDBY` and `$HYPERLINKBASE`; units and measurement (`$INSUNITS`, `$MEASUREMENT`, `$LUNITS`, `$LUPREC`, `$AUNITS`, `$AUPREC`, `$ANGBASE`, `$ANGDIR`); model space limits, extents and insertion base (`$LIMMIN`, `$LIMMAX`, `$LIMCHECK`, `$EXTMIN`, `$EXTMAX`, `$INSBASE`); current layer, text style, linetype, dimension style and multiline style by name; and the drafting settings `$LTSCALE`, `$CELTSCALE`, `$TEXTSIZE`, `$TRACEWID`, `$ELEVATION`, `$THICKNESS`, `$PDSIZE`, `$PDMODE`, `$ORTHOMODE`, `$FILLMODE`, `$PSLTSCALE`, `$PLINEGEN`, `$LWDISPLAY`, `$CELWEIGHT`, `$USRTIMER` and the creation and editing times `$TDCREATE`, `$TDUPDATE` and `$TDINDWG`. A real value that is not finite is skipped and reported. Every other header variable, including the dimension system variables (`DIM*`) and the view, camera, render, UCS and spline settings, is counted as one item of `io.dwg.header_dropped`. The R12 view, snap and grid state (`legacy`) is not mapped |
| Layers | LAYER entries: color number (negative when the layer is off), true color, linetype name, plot flag, lineweight; flags frozen (1), frozen in new viewports (2) and locked (4). A layer that is off with the by-block color cannot carry the off state in a color number of 0, so the color 7 is used, negated, and the layer is partial (`color`) |
| Linetypes | LTYPE entries: description, alignment, pattern length and element lengths; complex elements are partial. A linetype with a length that is not finite is skipped |
| Text styles | STYLE entries: height, width factor, oblique angle in degrees (the library stores radians), last height, font names, generation flags. A style with a number that is not finite is skipped |
| Application names, dimension style names | APPID and DIMSTYLE entries by name; dimension variables are partial |
| Block records | BLOCK_RECORD entries: insertion units in the flags (group 70, as the DXF reader keeps them), layout handle, explodable and scalable flags, preview bytes; the xref load state is partial (`xref_load_state`) |
| Block definitions | One block for each block record that is not a layout. A layout is a record with a layout handle, or a record named `*Model_Space` or `*Paper_Space`, whatever its handle. The definition holds the block flags (anonymous 1, has attributes 2, xref 4, xref overlay 8, external 16), the base point, and its entities. Block members named by a handle that names no entity of the file are counted in `io.dwg.handle_unresolved` |
| Model space and paper space | Top-level entities; paper space entities carry the paper-space flag |
| LINE, RAY, XLINE, CIRCLE, ARC, ELLIPSE, POINT | The same kinds; angles of arcs and points are converted from radians to degrees; ellipse parameters stay in radians, as the DXF model stores them |
| Lineweight of an entity | The lineweight is absent when the entity takes the layer's or the default lineweight, as an entity without the lineweight group has none in the DXF reader; by-block is the code -2 and a value is kept |
| LWPOLYLINE | Vertices with bulges, widths and ids; closed and plinegen flags; the elevation of the entity is the common elevation |
| POLYLINE (two-dimensional) | Polyline with its vertices, which take the layer and paper-space flag of the polyline. Flags closed (1), curve fit (2), spline fit (4) and linetype generation (128) are kept, and the other flags are partial (`polyline_flags`). The smoothing type is kept as the smooth type. The elevation is the z of the origin point, as the DXF reader keeps it, and not a common property |
| SPLINE | Knots, weights, control points, fit points, tangents, degree and flags |
| TEXT, MTEXT | Text with height, rotation, width factor, oblique angle, style, alignments and attachment; the line spacing style and factor are kept; MTEXT columns and background are partial |
| INSERT | Block reference with scales, rotation, array counts and spacings, and its ATTRIB entities. Each attribute takes the paper-space flag of its insert and its own normal, which must be finite |
| HATCH | Pattern name, solid and associative flags, pattern angle, scale and type, boundary paths, pattern lines and seed points. The elevation is the z of the elevation point, not a common property. A path made of one polyline keeps the polyline form, with the bulge of each vertex and `has_bulge` set when a bulge is not zero. Other paths keep line, arc, ellipse and spline edges. Elliptic edges store the circular angle of each end, in degrees, as the file does: the library's ellipse parameter `t` converts to `atan2(ratio sin t, cos t)`. Rational spline edges take their weights from the z of their control points. A polyline inside a multi-edge path becomes line edges, and an arc edge for each bulged segment, and is partial (`hatch_polyline_edges`). Gradient fills are partial (`hatch_gradient`) |
| DIMENSION | Definition, text and insertion points, type, text, style and block names; the geometry beyond these is partial |

The library exposes about 300 header variables as typed fields, each documented with its DXF name.
The mapping above reads the identity, units, limits and current-name fields and the drafting
settings listed in the header row. Not exposed by the library: the raw fixed-layout header of
R12 files (crate-private in the library), and any DXF header variable that has no typed field. The
variable list was taken from the library's field documentation, not from a DXF specification, so a
variable missing from that list may also be absent from the library.

Entity types the model does not carry, and entities outside any block or layout, are counted in
`io.dwg.entity_dropped` by type name. Objects, classes and notifications are counted in
`io.dwg.object_dropped`. The view port, view, user coordinate system and VX tables are counted in
`io.dwg.table_dropped`. Block members named by a handle that names no entity of the file are counted
in `io.dwg.handle_unresolved`.

Data of a mapped record that the model does not carry is named in the argument `properties` of the
partial item for the record kind: `color`, `transparency`, `graphic_data`, `extended_data`,
`references`, `knot_parameterization`, `dimension_detail`, `mtext_columns`,
`mtext_background`, `annotative`, `attribute_mtext`, `hatch_mpolygon`, `hatch_polyline_edges`,
`hatch_gradient`, `polyline_flags`, `plot_style`, `xref`, `xref_load_state`, `true_type_font`,
`shape_file`, `vertical`, `complex_elements`, `xref_path`, `description`, `insert_handles`,
`dimension_variables`, `large_radial` and `alignment`. The names of all records of a group are
merged, in alphabetical order; at most 32 names are listed and any others are summarized as
`and more`. The count covers the whole group.

### Loss reports

Every successful read returns a `LossReport` in the form of `schemas/loss-report.schema.json`. The
header has format `dwg`, the release name as version and the direction `import`. `summary.mapped`
counts the records the model carries, as the DXF reader counts its kept records: the entities (the
top-level and block members), the BLOCK and ENDBLK record of each block definition, the six symbol
tables and every table entry. Objects are not carried, so they are not counted.

| Reason | Outcome |
|---|---|
| `io.dwg.header_dropped` | `dropped` |
| `io.dwg.table_dropped` | `dropped` |
| `io.dwg.table_entry_partial` | `approximated` |
| `io.dwg.object_dropped` | `dropped` |
| `io.dwg.entity_dropped` | `dropped` |
| `io.dwg.entity_partial` | `approximated` |
| `io.dwg.value_skipped` | `dropped` |
| `io.dwg.handle_unresolved` | `dropped` |

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `io.dwg.signature_invalid` | `corrupt_data` | The input does not start with `AC` and four digits |
| `io.dwg.version_unsupported` | `unsupported` | The signature names a version outside R13 to R2018; the argument `version` is the signature |
| `io.dwg.truncated` | `corrupt_data` | The library reached the end of the input inside a structure it needs |
| `io.dwg.structure_invalid` | `corrupt_data` | The library reports malformed structures, unknown handles or unsupported content |
| `io.dwg.checksum_mismatch` | `corrupt_data` | A checksum stored in the file does not match its data |
| `io.dwg.library_failed` | `internal` | The read or the mapping of the wrapped library panicked (native builds only; a WebAssembly panic aborts instead); the file was not read |

The shared codes `io.limit_exceeded` (`limit_exceeded`) and `base.cancelled` (`cancelled`) apply as
in the DXF reader. The limit code names the field of the profile in the argument `limit`.

### Tests

The tests are in `crates/arc-io-dwg/tests` and in the modules of the crate. A sample drawing is built
in the test with the library's own writer, for each version from R13 to R2018, with layers, lines,
an arc, text, a polyline, an mtext, a hatch, an unmapped solid and a block with an insert and an
attribute. Its mapped values, the dropped and partial items, the limit errors, the signature and
version checks, the truncation and the cancellation are asserted. Further tests build one feature
with the library's writer (an off layer, a text style with an oblique angle, a two-dimensional
polyline, a bulged hatch polyline, an elliptic hatch edge with ratio 0.5, a mesh, a model space in
the R13 version) and assert the mapped value. A writer refusal fails the test. Inputs the writer
cannot produce, such as a block member that names no entity or a linetype with a number that is not
finite, are tested on the mapping functions with constructed library values. The isolation boundary
is tested with a closure that panics and one that panics in a destructor (native builds). Property tests read
arbitrary bytes, bytes that start with a supported signature, and up to 128 mutations of a valid
file at any position, and assert that no read unwinds or reports a library panic. No sample file is
part of the repository.

## SVG and PDF output as implemented

`arc-io-pdf` writes plot streams (`arc_draft::plot::PlotStream`, see [arc-draft.md](arc-draft.md),
"Plotting as implemented") to SVG (CORE-IO-038) and PDF (CORE-IO-037). It depends on `arc-base`,
`arc-geom`, `arc-draft` and `arc-command` and on no third-party crate beyond the ones those use
(`serde`, `serde_json`, `schemars`): both writers are Arc-owned, written from the published SVG 1.1
and PDF 1.7 file structures, so `krilla` and `lopdf` of the dependency table are not used by this
part. The writers read and write byte buffers, never paths; the only code that touches files is the
command `core.draft.plot` (see "The plot command"), which goes through the command context. The
crate builds for `wasm32-unknown-unknown`, where the context denies every file access. The
stream's units are millimeters from the lower left corner of the page.

### Entry points and options

- `write_svg(stream, options)` writes one page as one SVG document. A caller with several pages
  calls it once per page; one document with nested page groups was rejected because viewers show
  only one page of it.
- `write_pdf(streams, options)` writes one page per stream, in the order given; no stream is
  `io.pdf.option_invalid` (argument `option` is `streams`).
- Both return `Output { bytes, report }`. `WriteOptions` holds the limits profile (`constrained`
  by default), a `Cancel` token, a progress callback, `chord_mm`, `title` and `creation_date`.
  `chord_mm` is the chord tolerance in millimeters (0.000001 to 100, default 0.005) and `Tolerance`
  of `arc-base` is not used, because its chord is in meters of model space and a plot needs
  page-space values.
- Limits, as for an export: `input_bytes` bounds the file size (the largest file a reader with the
  same profile accepts), `record_count` the plot items plus pages (and, for SVG, viewport clips), `array_length` the vertices of
  one polyline, flattened curve or list of pieces, `string_bytes` the title. Counts are checked
  before any work, the file size before every item and at the end, and each failure is
  `io.limit_exceeded` whose argument `limit` names the field. `nesting_depth` and the decompression
  limits do not apply. The caller ends a write by cancelling; the token is checked before every
  item and a cancelled write returns `base.cancelled`. Progress reports items done and in total
  (plot items plus one per page, and per clip in SVG) on the first item, every 256 items and at the end.
- Hostile values: a `chord_mm` outside its range, a title with a control character, a creation
  date that is not a valid `YYYY-MM-DDTHH:MM:SSZ`, are `io.pdf.option_invalid` (argument
  `option`). A stream with a page edge that is not finite and positive, larger than 1,000,000
  millimeters (SVG) or 14400 points (PDF), a plotted area that is not finite and ascending, an
  item whose clip index is out of range, or any written coordinate that is not finite or beyond
  1,000,000,000 millimeters is `io.pdf.stream_invalid` (argument `what`).

### What both writers draw

- **Order and clips.** Items are written in stream order. Everything is clipped to the plotted
  area; an item with a clip index is also clipped to that viewport region (a closed polyline,
  exact arcs kept). Consecutive items with the same clip share one clip group.
- **Paper.** The page is first filled with the paper color, opaque, as the raster output does, so
  masks (wipeouts) written as fills in the paper color hide what is beneath them.
- **Strokes.** A stroke without pieces draws the whole curve; with pieces it draws exactly those
  pieces, each a separate subpath, so dashes are the stream's and no dash pattern is written.
  A piece that is a point is a dot, a disc of the pen width. A point item is a dot. Pen width 0
  draws a line of width 0 in PDF (the thinnest line) and of 0.05 mm (`HAIRLINE_MM`) in SVG, and
  dots use the same minimum. End and join styles come from the pen; the miter limit is 4, as in
  the raster output.
- **Fills and masks.** A region is a closed polyline with exact arcs; it is filled with the
  nonzero rule (the stream carries one boundary and no rule; the raster output also fills nonzero).
  A region fill (parts with holes) is one path of all its loops, each one closed sub-path of its
  exact curves, filled with the even-odd rule (`fill-rule="evenodd"` in SVG, `f*` in PDF).
- **Polyline widths.** A polyline with vertex widths is stroked with the larger of the pen width
  and the mean vertex width, as the raster output does; the taper is not drawn
  (`io.pdf.polyline_width_approximated`).
- **Color.** The pen's linear color is converted to 8-bit sRGB; alpha is written as stroke and
  fill opacity (SVG) or an extended graphics state (PDF). Masks and the paper are opaque.
- **Text.** Stroke text is stroke items and is written like any stroke. Outline glyph runs are
  written as text over embedded subset fonts (see Fonts below) and count as mapped items. A run
  whose font forbids embedding, or whose outlines are not TrueType glyph data, is not written and
  is a loss item (`io.pdf.font_restricted` or `io.pdf.font_unsupported`, `dropped`); a run whose
  font bytes cannot be read is `io.pdf.font_invalid` (`failed`). PDF/A-2b is not provided; PDF/A is
  not requestable. The SVG writer writes no outline glyph runs: each is a loss item
  (`io.pdf.glyphs_dropped`, `dropped`).
- **Curves.** Segments are lines, polylines are lines and arcs, circles and ellipses are arcs
  (a full turn is split in half arcs), NURBS curves are flattened with `chord_mm` and a turning
  angle of 15 degrees (`io.pdf.nurbs_flattened`, argument `tolerance_mm`). A ray or line without
  pieces, and a curve that cannot be flattened for a reason other than the vertex limit, is not
  written and is a loss item (`io.pdf.items_failed`, `failed`).
- **Numbers.** Every number is written with at most four decimals (0.1 micrometer in SVG
  millimeters, 0.0001 point in PDF), trailing zeros removed, no exponent and no negative zero.
  Output depends on the stream, the options and the platform's trigonometric functions only:
  no timestamp, identifier or random value is written.

### SVG

The document is sized in millimeters (`width="210mm"`) with a view box in paper millimeters
(`viewBox="0 0 210 297"`). The y axis is flipped in the coordinates (y is the page height minus
the stream's y), so no transform is written and a counterclockwise arc has sweep flag 0.

- Strokes are `path` elements with `fill="none"`, stroke color, width, cap, join and miter limit;
  lines are `L` commands; circular and elliptical arcs are `A` commands with exact radii and the
  rotation of the major axis (0 for a circle), split into pieces of at most a half turn. Fills
  are `path` elements with `fill-rule="nonzero"` (`evenodd` for region fills); dots are `circle` elements.
- The plotted area and each viewport clip are `clipPath` elements (`a`, `c0`, `c1`, ...) applied
  by `g` elements.
- A title is the `title` element, XML-escaped. Layers have no SVG form: the writer reports their
  number (`io.pdf.layers_dropped`, `dropped`, information).

### PDF

The file is PDF 1.7: a header, indirect objects, a cross-reference table with 20-byte entries,
a trailer without an identifier, and the start of the table. Object order is fixed: catalog,
page tree, information dictionary, optional content groups, then page and content stream per page,
then the five objects of each embedded font in the order of its family name and face index.

- **Pages.** The media box is the page in points (1 mm = 72 / 25.4 pt) from the lower left
  corner, the origin of the stream.
- **Content.** Content streams are not compressed (no encoder is added; the vector content is
  compact and a deflate stream would add a dependency for no capability). Operators are `m`, `l`,
  `c`, `h`, `re`, `W n`, `S`, `f`, `w`, `J`, `j`, `M`, `RG`, `rg`, `gs`, `q`, `Q`, `BDC`, `EMC`,
  and for outline text `BT`, `ET`, `Tf`, `Tm`, `Tj`.
- **Arcs.** PDF has no arc operator. An arc or ellipse is cubic Bezier pieces with control points
  from the tangents (factor 4/3 tan(a/4) for a piece of angle a). The piece count is the least
  that keeps the radial error of a circle of radius r below `chord_mm`, using the error
  0.00027 r (a / 90 degrees)^6 of a piece and at most 90 degrees per piece, at most 1024 pieces
  per arc; for an ellipse r is the larger semi-axis, which bounds the error. The count of arcs
  written this way is a loss item (`io.pdf.arcs_approximated`, `approximated`, argument
  `tolerance_mm`).
- **Fonts.** Each font that an outline glyph run draws with is embedded once for the document, in
  the order of its family name, face index, byte length and digest, as five objects: a composite
  font (`Type0`, `Identity-H`), its `CIDFontType2` descendant with `CIDToGIDMap /Identity` and the
  `W` array of the drawn glyphs' advances, the font descriptor, the `FontFile2` stream, and a
  `ToUnicode` CMap that maps each drawn glyph to the lowest Unicode scalar value the font's character
  map gives it; code points that are not scalar values (surrogates) are skipped, and a glyph with no
  such value has no entry. Two files under one family name are two embedded fonts. The font file is
  a TrueType subset with the font's glyph identifiers: drawn glyphs and the components of composite
  glyphs keep their outlines, every other glyph is empty. The copied outline bytes never exceed the
  length of the font's glyph data, so overlapping glyph ranges cannot grow the file. Its tables are
  `head`, `hhea`, `maxp`, `hmtx`, `loca`, `glyf`, and `cvt `, `fpgm` and `prep` when present. The base
  font name is a six-letter tag, a digest of the subset bytes, `+`, and the family name in ASCII
  letters and digits, cut so that the name stays within 127 bytes (the family part at 120
  characters). The descriptor takes its bounding box, ascent, descent and italic angle from `head`,
  `hhea` and `post`, and its cap height from `OS/2` (`sCapHeight`), in 1/1000 em. The `W` entries and
  the descriptor values are rounded half away from zero, not truncated. Each glyph is one text object
  (`BT`, `Tf`, `Tm`, `Tj`, `ET`) whose text matrix carries the placement, the size and the pen
  position. The embedding permission is the `OS/2` `fsType` field: a font with the restricted-license
  bit (`0x0002`), the no-subsetting bit (`0x0100`) or the bitmap-only bit (`0x0200`) is not embedded
  (`io.pdf.font_restricted`); an `OS/2` table too short to hold `fsType` is malformed
  (`io.pdf.font_invalid`). A font with CFF outlines is `io.pdf.font_unsupported`. Every other problem
  is `io.pdf.font_invalid`: bytes that do not parse, a size above `input_bytes` or a glyph count above
  `array_length` of the limits profile, a glyph identifier the font lacks, a missing glyph table, more
  than 1,000,000 component references followed for one subset (`COMPONENT_BUDGET`), or more than
  2,000,000 character map entries read for one ToUnicode map (`MAPPING_BUDGET`). A malformed font
  is a loss item for its runs, not an error of the document. A run whose glyphs are not drawn is
  neither written nor lost.
- **Layers.** Each distinct layer of the items of all pages is an optional content group named
  by the layer's object identifier, listed in the catalog (`OCProperties`, all on), ordered by
  identifier, and used per page through `Properties`. Consecutive items with the same clip and
  layer form one `q ... BDC ... EMC ... Q` run; items without a layer are outside any group.
  The stream carries identifiers, not layer names; a product maps the names (a gap).
- **Information.** `Producer` is `Arc arc-io-pdf`; `Title` is a literal string for printable
  ASCII and UTF-16 hexadecimal otherwise; `CreationDate` is written only when
  `creation_date` is set.
- Not written: hyperlinks, PDF/A-2b, compression, a file identifier.

### The plot command

`register_commands` adds `core.draft.plot` (CORE-DRAFT-033). Permission classes `read` and
`file_write`, not undoable, on every surface; it needs no display and runs headless. Files are
written only through `Context::create_write`, so the session's write roots decide where it may
write, and every file is encoded before the first one is created: a failing page leaves no file.

| Parameter | Meaning |
|---|---|
| `layouts` | Layout space identifiers, in page order, at most 256 |
| `model` | Instead of `layouts`: `space`, `window` (`[min x, min y, max x, max y]`), `scale` (paper length per model length) and optional `paper` (`[width, height]` in millimeters, default 210 by 297); the window is centered on the paper without margins |
| `format` | `pdf`, `svg` or `png` |
| `path` | The output file, inside the write roots |
| `dpi` | Pixels per inch of PNG output, 1 to 10000, default 300 |
| `options` | Overrides of the plot settings: `area` (`layout`, `extents`, `window`), `window`, `scale`, `fit`, `center`, `origin`, `rotation`, `lineweights`, `styles`, `style_table`, `paper_color` (linear RGBA), `chord_mm`, `title` |

Exactly one of `layouts` and `model` is given. Each layout is plotted with `PlotOptions::from_layout`
(its sheet, window, scale, origin and plot style table) and the overrides applied on top; a
`window` alone implies `area` `window`; `fit` excludes `scale` and `center` excludes `origin`.

- **PDF.** All pages go into one file at `path`, in the order of `layouts`.
- **SVG and PNG.** One file per page. With one page the file is `path`; with several, `-N` (N from
  1, no padding) is inserted before the extension of the file name: `sheet.svg` gives
  `sheet-1.svg`, `sheet-2.svg`; a name without an extension gets the suffix at its end. The
  extension of `path` is not changed or checked.
- **PNG** needs the cargo feature `raster` of `arc-io-pdf`, which enables the feature of
  `arc-draft`. Without it the command fails with `io.pdf.raster_unavailable` before any work.
- **Result.** `paths` (the files written, as given or numbered), `pages`, and `reports`, one loss
  report per file in the order of `paths`, with `header.file_name` set. The PNG report has the
  format `png`, version `1.2`; it counts the plot items drawn (`io.pdf.items_written`), those the
  raster output skipped (`io.pdf.items_skipped`: outline glyph runs and curves that cannot be
  flattened) and the entities the stream left out (`io.pdf.sources_failed`).
- **Errors.** An inconsistent or out of range parameter is `io.pdf.plot_params_invalid` (argument
  `what` names it: `layouts`, `model`, `model.window`, `model.scale`, `model.paper`, `dpi`,
  `options.window`, `options.scale`, `options.origin`); a path outside the roots, a missing
  permission class and an input/output failure are the codes of `arc-command`; the plot and
  writer errors are passed through.
- **Paper rule.** Pens equal to the paper color plot in the contrasting extreme (see the plot
  style rules in [arc-draft.md](arc-draft.md)), so the default white entities show on white paper
  in every format.

### Loss reports

Every write returns a `LossReport` (header format `svg` or `pdf`, version `1.1` or `1.7`,
direction `export`) valid against `schemas/loss-report.schema.json`, also when nothing was lost.
Items are aggregated per reason and plot item kind (`point`, `stroke`, `fill`, `mask`, `glyphs`);
`source_id` is the source object of the first occurrence. `summary.mapped` counts items written
without any approximation. The type is the same shape as that of `arc-io-dxf`, defined again
because the layer table does not allow one format crate to depend on another.

| Reason | Outcome |
|---|---|
| `io.pdf.items_written` | `mapped` |
| `io.pdf.glyphs_dropped` | `dropped` |
| `io.pdf.font_restricted` | `dropped` |
| `io.pdf.font_unsupported` | `dropped` |
| `io.pdf.font_invalid` | `failed` |
| `io.pdf.items_skipped` | `dropped` |
| `io.pdf.layers_dropped` | `dropped` |
| `io.pdf.nurbs_flattened` | `approximated` |
| `io.pdf.arcs_approximated` | `approximated` |
| `io.pdf.polyline_width_approximated` | `approximated` |
| `io.pdf.items_failed` | `failed` |
| `io.pdf.sources_failed` | `failed` |

`io.pdf.sources_failed` counts the entities the stream itself lists in `failed`.

### Error codes of the output

| Code | Category | Meaning |
|---|---|---|
| `io.limit_exceeded` | `limit_exceeded` | A limit of the profile was exceeded; the argument `limit` names it (the same code as in the DXF table; it moves to a shared registry) |
| `io.pdf.option_invalid` | `invalid_input` | An option is out of range or malformed; the argument `option` names it |
| `io.pdf.stream_invalid` | `invalid_input` | A page size, area, clip index or coordinate of the stream cannot be written; the argument `what` names it |

| `io.pdf.raster_unavailable` | `unsupported` | The format is PNG and the crate has no `raster` feature enabled |
| `io.pdf.plot_params_invalid` | `invalid_input` | A parameter of `core.draft.plot` is missing, inconsistent or out of range; the argument `what` names it |

`base.cancelled` is returned when the token is cancelled.

## Shared bounded readers as implemented

`arc-io-common` holds the bounded ZIP and XML readers that CORE-IO-007 names, so that every
ZIP- and XML-based format reads them the same way. It depends on `arc-base`, on `miniz_oxide` for
inflation and on `quick-xml` for XML parsing; it reads byte buffers only, has no product type and
builds for `wasm32-unknown-unknown`. Format crates depend on it through same-layer edges that are
added when each one starts using it (ADR-0001, amendment "Shared bounded ZIP and XML readers").
The limits are `arc_base::Limits`, used directly: the crate defines no limits type of its own, so
the field names and profile values are those of [interchange.md](../architecture/interchange.md#hostile-input-limits).

- `ReadOptions` holds the limits profile (`constrained` by default) and a `Cancel` token. The token
  is checked every 1024 XML events, before every 32 KiB of inflate output and every 64 KiB of CRC
  computation, and a cancelled read returns `base.cancelled`.
- Every limit fails with `io.limit_exceeded` whose argument `limit` names the field of `Limits`:

| Limit | Enforced on |
|---|---|
| `input_bytes` | The buffer length, before any parsing |
| `record_count` | ZIP entries (before the directory is allocated); XML parser events |
| `string_bytes` | A ZIP entry name; an XML name, the bytes of one element's attributes, one text node |
| `array_length` | XML attributes of one element; namespace declarations in scope |
| `nesting_depth` | XML open elements |
| `decompressed_bytes` | Each declared size and the sum of declared sizes at open; the output of all reads together, failed reads included |
| `decompression_ratio` | Each declared size against the compressed size times the ratio, at open; the output of an inflate call |

### ZIP archives

- `ZipArchive::open(bytes, options)` validates the archive before any entry is inflated and returns
  the entries in directory order. `read_entry(index)` inflates one entry and returns its bytes after
  its CRC-32 is checked.
- Validation order: the input size; the end record, found by a backward search over at most 65,558
  offsets for a signature whose comment length ends at the end of the buffer; the ZIP64 locator and
  end record when the locator sits immediately before the end record; the disk fields (all zero,
  otherwise `io.zip.multi_disk`); every central record, with the count checked against
  `record_count` and against the directory size before the directory is allocated; for each entry,
  the name, the encryption flags, the method, the declared sizes and the ratio, and the uniqueness
  of the name; the names of all entries together, for file and folder conflicts; then, in the order
  of their local headers, each entry's local header (signature, name, method, the agreed flags, and
  CRC and sizes unless a descriptor follows the data) and its data descriptor when the central record
  sets bit 3. The directory and the end records must not share a byte with an entry, and no two
  entries may either. A local header that starts inside the directory, the end records or an
  earlier entry, or whose data reaches one of those ranges, is `io.zip.overlap`; the check happens
  before any later header is parsed.
- Supported methods are stored (0) and deflate (8). Encrypted entries (flags bit 0, bit 6) are refused
  with `io.zip.encrypted`; other methods with `io.zip.method_unsupported`.
- ZIP64 is read when the ZIP64 locator sits immediately before the end record. A field of the end
  record that holds its marker (`0xFFFF` for the count, `0xFFFFFFFF` for the size and the offset) takes
  the ZIP64 value; a field that does not hold a marker must equal the ZIP64 value, or the archive is
  refused with `io.zip.structure_invalid`. A marker without the locator is `io.zip.structure_invalid`;
  without the locator, an entry count of 65,535 is an ordinary count. In a central record, a 32-bit
  field that holds `0xFFFFFFFF` takes its value from the ZIP64 extension, in the order size, compressed
  size, offset. The values are then checked like any others.
- The local header and its central record must agree on the encryption (bits 0 and 6), UTF-8 (bit 11)
  and data descriptor (bit 3) flags; otherwise `io.zip.header_mismatch`.
- A data descriptor is accepted when its CRC and sizes equal the central record's (the signature is
  optional). Its sizes are 8 bytes wide when the local header carries a ZIP64 extra field and 4 bytes
  otherwise; a mismatch is `io.zip.data_descriptor_unverified`.
- Offsets are absolute. Data before the first entry (for example a self-extracting stub) makes the
  local headers disagree with the directory and is refused as `io.zip.header_mismatch`.
- Names: the bytes are UTF-8 when flag bit 11 is set. Otherwise each byte maps to the code point of
  the same value (ISO 8859-1), which names the bytes without claiming a code page. A name is refused
  (`io.zip.name_invalid`) when it is empty, contains a control character or an empty component, or
  is not valid UTF-8. It is refused (`io.zip.path_unsafe`) when it starts with `/`, contains a
  backslash, a colon or one of the characters `< > " | ? *`, or has a `.` or `..` component. It is
  refused with the same code when a component ends with a dot or a space, or when the stem of a
  component (the part before its first dot, without trailing spaces) is a Windows device name (`CON`,
  `PRN`, `AUX`, `NUL`, `COM1` to `COM9`, `LPT1` to `LPT9`), compared without regard to case. The
  errors for these Windows rules name the reason in their details. A trailing `/` marks a directory. Two names are duplicates
  (`io.zip.name_duplicate`) when their paths are equal after lowercasing every character by its simple
  Unicode mapping; canonical equivalence of Unicode forms is not considered. A file whose path is also
  a folder that another entry needs, such as `a` with `a/b`, is `io.zip.name_conflict`, under the same
  comparison.
- While inflating, the output is capped by the declared size, by the ratio and by the remaining
  budget, and the stream is stopped one byte past the cap. The output vector grows toward the declared
  size in bounded steps and never holds more than the declared size plus one byte of capacity. A stream
  that produces a different number of bytes from its declared size, or leaves compressed bytes unread
  (for example trailing bytes inside the entry's compressed range), is `io.zip.size_mismatch`.
- The budget is the archive's: every call to `read_entry` charges the bytes it produced against
  `decompressed_bytes`, whether the entry then passes its CRC-32 check or not. Reading an entry again
  uses budget again, and an entry that fails its checks cannot be inflated without limit.

### XML documents

- `XmlReader::new(bytes, options)` returns a pull reader; `next_event()` returns `Event::Start`
  (name and attributes), `Event::End` (name) or `Event::Text`, and `None` at the end. An empty
  element yields a start and an end. Adjacent character data, CDATA sections and references form one
  text node. A comment ends a text node: text on either side of a comment is delivered as two
  `Event::Text` events. Comments are consumed without an event.
- The XML declaration is consumed without an event when it is the first event of the document. Any
  other declaration is a processing instruction and is refused.
- Attribute values are normalized as XML requires (white space becomes a space; character references
  are kept). Text line ends become line feeds. Names and values are UTF-8.
- Element and attribute names follow the `Name` production of XML 1.0 (fifth edition), checked by
  character ranges; a name that breaks it is `io.xml.syntax`. A qualified name has at most one colon,
  which is neither its first nor its last character; otherwise it is `io.xml.structure_invalid`. An
  empty namespace prefix (`xmlns:`) is therefore refused.
- Markup characters are refused where XML forbids them: a raw `<` in an attribute value and the
  sequence `]]>` in character data are `io.xml.syntax`. Write `&lt;` and `]]&gt;` instead.
- Outside the root element only white space is allowed, and it is not delivered. Character data,
  CDATA sections and references outside the root are `io.xml.structure_invalid`.
- Refused: a document type declaration (`io.xml.doctype_refused`), a processing instruction, including
  an XML declaration that is not the first event (`io.xml.processing_instruction_refused`), any entity
  reference other than the five predefined entities and character references (`io.xml.entity_refused`),
  a character reference to a value that XML does not allow (`io.xml.reference_invalid`), and a declared
  encoding other than UTF-8 or US-ASCII, or a UTF-16 byte-order mark (`io.xml.encoding_unsupported`).
  Entity expansion and external entities therefore cannot occur.
- Namespace declarations stay as attributes named `xmlns` or `xmlns:prefix`. `resolve_prefix(prefix)`
  returns the URI in scope (the innermost declaration wins; `xml` is bound to the XML namespace) and
  `split_qualified_name(name)` splits a qualified name. The declarations in scope are bounded by
  `array_length`.

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `io.limit_exceeded` | `limit_exceeded` | A limit of the profile was exceeded; the argument `limit` names it |
| `io.zip.truncated` | `corrupt_data` | The archive ends inside a record, a header, a name or an entry's data |
| `io.zip.end_missing` | `corrupt_data` | No end record matches the end of the buffer |
| `io.zip.structure_invalid` | `corrupt_data` | A signature, count, offset or size of the archive's structures is inconsistent, including a ZIP64 record that disagrees with the end record |
| `io.zip.multi_disk` | `unsupported` | The archive spans several disks |
| `io.zip.encrypted` | `unsupported` | An entry is encrypted |
| `io.zip.method_unsupported` | `unsupported` | An entry uses a method other than stored or deflate |
| `io.zip.data_descriptor_unverified` | `corrupt_data` | A data descriptor does not agree with its central record |
| `io.zip.header_mismatch` | `corrupt_data` | A local header does not agree with its central record |
| `io.zip.overlap` | `corrupt_data` | Two parts of the archive share a byte |
| `io.zip.name_invalid` | `corrupt_data` | An entry name is empty, malformed or not valid UTF-8 |
| `io.zip.path_unsafe` | `corrupt_data` | An entry name is absolute, contains a backslash, a colon, a character Windows does not allow or a `.` or `..` component, or names a Windows device or ends a component with a dot or a space |
| `io.zip.name_duplicate` | `corrupt_data` | Two entry names are equal after normalization |
| `io.zip.name_conflict` | `corrupt_data` | A file has the name of a folder that another entry needs |
| `io.zip.size_mismatch` | `corrupt_data` | Compressed data does not produce its declared size, or leaves bytes unread |
| `io.zip.inflate_failed` | `corrupt_data` | A deflate stream is malformed |
| `io.zip.crc_mismatch` | `corrupt_data` | The data of an entry does not match its CRC-32 |
| `io.zip.entry_missing` | `not_found` | The index names no entry |
| `io.xml.truncated` | `corrupt_data` | The document ends inside a tag, a text node or an open element, or has no root |
| `io.xml.syntax` | `corrupt_data` | The markup is not well formed, including a name outside the XML grammar, a raw `<` in an attribute value and `]]>` in character data |
| `io.xml.structure_invalid` | `corrupt_data` | Elements do not nest, a second root starts, content other than white space appears outside the root, a qualified name has a misplaced colon, or an attribute repeats |
| `io.xml.text_invalid` | `corrupt_data` | A name, value or text is not UTF-8, or a value or text contains a character XML does not allow |
| `io.xml.doctype_refused` | `unsupported` | The document has a document type declaration |
| `io.xml.processing_instruction_refused` | `unsupported` | The document has a processing instruction, or an XML declaration that is not its first event |
| `io.xml.entity_refused` | `unsupported` | An entity reference is not one of the five predefined entities |
| `io.xml.reference_invalid` | `corrupt_data` | A character reference names no valid character or is not terminated |
| `io.xml.encoding_unsupported` | `unsupported` | The document is not encoded in UTF-8 |

## Mesh formats as implemented

`arc-io-mesh` is the OBJ, STL, PLY and glTF part of the family. It depends on `arc-base` and `arc-mesh`,
and on `serde` and `serde_json` for the loss report and the glTF JSON. The four formats are implemented in the
crate from their published descriptions; none needs a third-party crate. Its readers take byte buffers and never paths, and it builds for
`wasm32-unknown-unknown`. The neutral model is `MeshScene`: named `arc_mesh::TriMesh` parts, each
with an optional material name, and the materials of an MTL library. Per-vertex normals, texture
coordinates and colors are the attribute channels `Normal`, `Uv(0)` and `Color` of the mesh.

### Reading

- `obj::read(obj, mtl, options)` takes the OBJ bytes and, as an optional second buffer, the MTL
  bytes. The `mtllib` statement is ignored. Vertices (`v`), texture coordinates (`vt`) and normals
  (`vn`) are referred to by one-based indices, or by negative indices counted back from the end of
  the pool read so far. A part holds one vertex per distinct corner (position, texture coordinate
  and normal together), so attributes stay per vertex. Polygons are fan-triangulated. Each `o` or
  `g` statement starts a part, and a `usemtl` statement after faces starts a part too, so every
  part has one material. A `usemtl` stays in effect across following `o` and `g` statements, so a
  new object or group starts with the material in effect. Smoothing groups (`s`)
  and `mtllib` are not kept. A backslash at a line end joins the next line, and `#` starts a
  comment. Other statements (`l`, `p`, curves and surfaces) are skipped and reported. Every statement
  that is not blank counts against `record_count`, and every part that holds faces counts against
  `array_length`; a part without faces creates no mesh.
- MTL: `newmtl` starts a material. `Ka`, `Kd` and `Ks` take three colors; other forms (spectral or
  CIE curves) are skipped and reported. `d` (after an optional `-halo`) and `Tr` give the opacity.
  The statements `map_*`, `bump`, `disp`, `decal`, `refl` and `norm` keep the text after their
  name as written. Other statements are skipped and reported.
- `stl::read(bytes, options)` takes a file as binary when its length is 84 bytes plus 50 bytes for
  each facet, the count being the little-endian `u32` at bytes 80 to 84. Any other file is read as
  ASCII, one part for each `solid`. Corners with equal coordinates share a vertex. Facet normals
  and attribute bytes are not kept; a non-zero one is reported.
- `ply::read(bytes, options)` reads ASCII, binary little-endian and binary big-endian files. The
  first `vertex` and first `face` elements are read. Positions come from `x y z`, normals from
  `nx ny nz`, texture coordinates from `s t`, `u v`, `texture_u texture_v` or `texture_s
  texture_t`, and colors from `red green blue` with an optional `alpha`; integer colors are divided
  by 255. Faces come from the list `vertex_indices` (or `vertex_index`), are fan-triangulated, and
  are fan-triangulated; their triangles count against `array_length` as each face is read, and
  indices are checked against the vertex count after every element is read. Integer values are
  checked against the range of their declared type. A binary element without properties takes no
  bytes: it counts once against `record_count` and is skipped. Vertices keep their order. Other
  elements and properties are skipped and reported. The result is one part with an empty name.
- Every count, line and total size is checked before it is used. The limits of the profile are
  enforced as follows:

| Limit | Enforced on |
|---|---|
| `input_bytes` | Each input buffer, before any parsing |
| `string_bytes` | Each line, and each joined OBJ statement |
| `record_count` | OBJ statements that are not blank, MTL statements, STL facets and PLY records over all elements |
| `array_length` | Corners of a polygon, vertices and triangles of a part, parts of an OBJ file, triangles of a PLY file, PLY list lengths and header elements |

  A count that the remaining bytes cannot hold fails with `io.mesh.truncated` before any record is
  read, so a declared count does not drive an allocation.
- A loss report keeps at most 1024 distinct items. Further record kinds join one item per reason
  with the record kind `(overflow)`, and the counts stay exact.

### Writing

- `obj::write(scene, options)` writes ASCII. Each part writes `o`, then `usemtl` when it has a
  material, then its positions, normals and texture coordinates when present, then faces with
  one-based indices. Parts without triangles are skipped and reported. With `mtl_name` set and
  materials present, the file begins with `mtllib`. The result carries the MTL bytes when the scene
  has materials; the caller stores them under the name it chose. Names may not contain `#`, a
  backslash or a line break. Texture statements of the MTL file are written only for the slots the
  reader keeps (`map_*`, `bump`, `disp`, `decal`, `refl`, `norm`); other slots are skipped and
  reported as dropped texture references.
- `stl::write(scene, options)` writes ASCII, one solid for each part, or little-endian binary with
  one solid for all parts; binary drops the part names and the division into parts, and both are
  reported. Positions are rounded to single precision, and each rounded position is reported. Facet
  normals are the unit normals of the facets, or zero where they are not representable. Big-endian
  binary fails with `io.mesh.encoding_unsupported`.
- `ply::write(scene, options)` writes ASCII, binary little-endian or binary big-endian with one
  `vertex` element for all parts and one `face` element. Positions are `double`; normals are `float`
  `nx ny nz`; texture coordinates are `float` `s t`; colors are `uchar` `red green blue alpha`,
  rounded to 8 bits, and each value that changes is reported. An attribute that not every written
  part has is dropped. Part names and materials have no place in the format and are reported.

Numbers are written in the shortest form that reads back to the same value: plain decimal for
magnitudes from 0.0001 up to, not including, 10^15, and exponent form otherwise. Output depends
only on the scene and the options; a writer never writes a timestamp or an identifier.

### glTF reading

- `gltf::read(bytes, resolver, options)` takes the bytes of a glTF JSON file or of a GLB container.
  The `resolver` maps a URI, as the file writes it (percent-encoding is not decoded), to the bytes
  of the resource, or to `None`; the crate never opens a file. The GLB magic selects the container:
  the header version must be 2, the first chunk must be JSON, and the first binary chunk is the
  buffer that has no URI. Unknown and repeated chunks are skipped and reported. The nesting depth
  of the JSON is checked before it is parsed.
- Buffers come from the binary chunk, from `data:` URIs in base64 (decoded by a bounded decoder in
  the crate), or from the resolver. Decoded and resolved bytes count against
  `Limits::decompressed_bytes`. The decoder accepts padding only at the end of the data, and only
  as many `=` as the length of the data requires.
- Accessors read every component type (5120 to 5126) and element type (`SCALAR` to `MAT4`).
  `byteStride` may not be shorter than an element. `normalized` applies to integer components.
  Sparse accessors replace the listed elements; `min` and `max` are not read. Matrix layouts pad
  each column to four bytes. Matrix accessors are not read into geometry, and the mesh attributes
  must have the element types named below.
- Primitives in modes triangles, triangle strip and triangle fan become triangles. A strip's odd
  triangles swap their first two corners to keep one orientation; a fan keeps its first corner.
  Points, lines and line strips are skipped and reported. Attributes read: `POSITION` (required,
  `VEC3`), `NORMAL` (`VEC3`), `TEXCOORD_0` (`VEC2`) and `COLOR_0` (`VEC3`, with alpha 1, or
  `VEC4`), kept as the channels `Normal`, `Uv(0)` and `Color`. Other attributes are skipped and
  reported. Each primitive is a `MeshPart`; each glTF mesh is a `MeshGroup` that names its parts.
- Nodes keep their file order and their transforms. A node gives a matrix, or translation, rotation
  and scale, where a missing component takes its glTF default; giving both forms is an error. A node
  names a mesh group, a camera and a light, and lists its children. The roots are the nodes of the
  default scene (the scene named by `scene`, or the first one). A node reached twice, a cycle, or a
  node deeper than `Limits::nesting_depth` is an error. A node outside the default scene that no
  node names as a child is validated as a root of its own, but it is not a root of the scene; a
  cycle that nothing outside it reaches is an error. Nodes that name one mesh group share its
  parts; the parts are stored once. Every number stored as `f32`, such as a transform, camera,
  light or material value, must be finite after conversion to `f32`; `1e39` is refused.
- Materials keep the `pbrMetallicRoughness` factors, the base color texture as
  `ImageSource::Uri` or `ImageSource::BufferView` (images are not decoded), `alphaMode`,
  `alphaCutoff` and `doubleSided`. A material keeps its name when the name is present and unused;
  otherwise it is named `material-<index>`, with `#<n>` added when that name is taken. A value the
  file does not give stays absent. Other material textures and factors are skipped and reported.
- Cameras (perspective and orthographic) and `KHR_lights_punctual` lights (directional, point and
  spot) are plain data; a light with no color or intensity takes the extension's defaults.
- Extensions: `KHR_lights_punctual` is interpreted. Every other extension object of an element, and
  each `extras` value, is kept on that element as canonical JSON text (`Preserved`) and reported as
  preserved. An entry of `extensionsRequired` other than `KHR_lights_punctual` is an error.
- Limits: `record_count` counts every node, mesh, primitive, accessor, buffer view, material,
  camera and light; `array_length` bounds accessor counts and vertex and index counts;
  `input_bytes` bounds the file; `string_bytes` bounds each name and each URI given to the resolver
  (a `data:` URI is bounded by `decompressed_bytes` instead); `nesting_depth` bounds the JSON and
  the node hierarchy; `decompressed_bytes` bounds the decoded and resolved buffers and the primitive
  data. Each primitive is charged before any of its data is decoded: 24 bytes per vertex for the
  positions, 12 for normals, 8 for texture coordinates and 16 for colors (kept as `Vec4`), 4 bytes
  per index, 12 per triangle, and the lengths of the mesh group name and the material name. The
  charges of all primitives are added, so an accessor shared by many primitives counts for each of
  them. The limit that refuses is `decompressed_bytes`. A view or accessor is checked against its
  buffer before any element is read, so a declared count cannot drive an allocation. An accessor
  without a view is zero-filled up to `array_length`, and its zeros are charged the same way.

### glTF writing

- `gltf::write(scene, options)` writes three layouts. `Encoding::Ascii` writes JSON with the buffer
  embedded as a base64 data URI. `Encoding::Ascii` with `WriteOptions::buffer_name` writes JSON that
  refers to the buffer by that URI and returns the bytes in `WriteOutput::buffer`.
  `Encoding::BinaryLittleEndian` writes one GLB with the buffer in its binary chunk. Big-endian
  binary, and a buffer name with binary encoding, fail with `io.mesh.encoding_unsupported`. The
  output is deterministic: the asset is `{"version": "2.0"}`, and the writer writes no generator
  name and no time.
- A mesh group becomes a glTF mesh with one primitive for each part, and a group shared by several
  nodes is written once. A part that no group names is written as a mesh under a root node of its
  own, and a group that no node names gets a root node. Parts without triangles are skipped and
  reported.
- A primitive writes `POSITION` as `f32` `VEC3` with its minimum and maximum; `NORMAL`, `TEXCOORD_0`
  and `COLOR_0` as `f32` when the mesh has them; and `indices` as `u16` when the mesh has at most
  65 535 vertices, so that no index is 65 535, and as `u32` otherwise. Each buffer view starts on a
  four-byte boundary and is padded with zeros. A GLB pads its JSON chunk with spaces.
- Positions are rounded to single precision, and each value that changes is reported as
  approximated. Tangents, texture coordinate sets after the first, and user channels are dropped
  and reported.
- Materials write the PBR fields that are present, and the names. A base color texture with a URI
  is written as an image and a texture. A buffer view reference cannot be written, because the
  image bytes are not kept; it is dropped and reported. The OBJ and MTL fields (`Ka`, `Kd`, `Ks`,
  `d` and the texture maps) are dropped and reported.
- Cameras and lights are written from their fields. Preserved extension objects and extras are
  written back. `extensionsUsed` lists every extension the output contains.
- A reference to a missing mesh group, material, camera, light or child, a node reached twice or on
  a cycle, and a value that is not finite fail with `io.mesh.write_value_invalid`.
- Not written: quantized attributes, image bytes (images are referenced only), sparse accessors,
  morph targets, skins and animations (the reader skips them and reports them), and the run of an
  external glTF validator.

### Loss reports

| Reason | Outcome |
|---|---|
| `io.mesh.element_skipped` | `dropped` |
| `io.mesh.property_skipped` | `dropped` |
| `io.mesh.polygon_triangulated` | `approximated` |
| `io.mesh.attribute_dropped` | `dropped` |
| `io.mesh.attribute_incomplete` | `dropped` |
| `io.mesh.precision_reduced` | `approximated` |
| `io.mesh.material_inherited` | `approximated` |
| `io.mesh.extension_preserved` | `preserved` |
| `io.mesh.texture_dropped` | `dropped` |

The record kind of an item names the element, property, statement or attribute it concerns. The
argument `attribute` of the attribute items names the attribute in the words of the report.

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `io.mesh.truncated` | `corrupt_data` | The input ends inside a line, record, list or declared element |
| `io.mesh.value_invalid` | `corrupt_data` | A value does not parse as its type, or is not finite |
| `io.mesh.index_out_of_range` | `corrupt_data` | A face refers to a vertex that does not exist |
| `io.mesh.face_invalid` | `corrupt_data` | A face has fewer than three corners |
| `io.mesh.structure_invalid` | `corrupt_data` | Statements, records or blocks are not arranged as the format requires |
| `io.mesh.header_invalid` | `corrupt_data` | The PLY header or the GLB header is malformed, or names an unknown format or version |
| `io.mesh.reference_invalid` | `corrupt_data` | A glTF index names no element, or a node is reached twice |
| `io.mesh.extension_unsupported` | `unsupported` | A glTF file requires an extension this reader does not support; the argument `extension` names it |
| `io.mesh.resource_unavailable` | `not_found` | A buffer stored outside the file was not supplied by the resolver; the argument `uri` names it |
| `io.mesh.text_invalid` | `corrupt_data` | Text of the file is not valid UTF-8 |
| `io.mesh.encoding_unsupported` | `unsupported` | The writer was asked for an encoding the format does not have |
| `io.mesh.write_value_invalid` | `invalid_input` | A value or name cannot be written in the format |

`io.limit_exceeded` is the limit code of the family defined in the DXF section above; its argument
`limit` names the field of the profile.

## IFC reading as implemented

`arc-io-ifc` is the first part of CORE-IO-014. It reads IFC2X3, IFC4 and IFC4X3 files, given as the
STEP physical file of ISO 10303-21 (clear text encoding) or as an IFC-ZIP archive holding one, into a
neutral entity model and typed views of the spatial structure, the element types, the property sets
and the quantity sets. It depends on `arc-base`, on `arc-io-common` for the ZIP archive reader (the
same-layer edge of the ADR-0001 amendment "Shared bounded ZIP and XML readers"), on `arc-mesh` for the
triangle meshes of the geometry evaluation (a lower-layer edge) and on `serde` for the loss report. Its
readers take byte buffers and never paths, and the crate builds for `wasm32-unknown-unknown`. The
geometry evaluation (CORE-IO-015) is described in [Geometry evaluation as implemented](#geometry-evaluation-as-implemented).
Writing (CORE-IO-016), streaming (CORE-IO-017) and IDS (CORE-IO-018) are not implemented.

### Reading

- `read(bytes, options)` takes the bytes of a physical file or of an archive. Bytes that begin with the
  ZIP local header signature are an archive. It is opened with `arc-io-common` under the same limits,
  except that `decompressed_bytes` is lowered to the smaller of `decompressed_bytes` and `input_bytes`,
  so no entry inflates beyond the input bound. Directory entries are ignored. Entries that hold
  resource-fork or metadata data (a path with a `__MACOSX` component, or a base name that starts with
  `._`) are skipped and counted as `io.ifc.archive_entry_skipped`. The one remaining entry whose name
  ends in `.ifc` (compared in lower case) is read. An archive with no such entry fails with
  `io.ifc.archive_entry_missing`, and one with several fails with `io.ifc.archive_entry_ambiguous`.
  Any other bytes are read as a physical file.
- The physical file is `ISO-10303-21;`, the `HEADER;` section, one or more `DATA;` sections, and
  `END-ISO-10303-21;`. Between the header and the end, `ANCHOR`, `REFERENCE` and `SIGNATURE` sections
  may appear in any order; each is parsed, dropped as a whole and counted as `io.ifc.section_skipped`.
  A data section may carry a parenthesized parameter list (`DATA('x');`), which is parsed and not
  used. The instances of all data sections form one list with one name space, so a name repeated in
  two sections is refused. Section keywords are case-sensitive.
- A UTF-8 byte order mark before `ISO-10303-21;` is skipped and counted as `io.ifc.bytes_ignored`.
  After `END-ISO-10303-21;`, white space is ignored without a report, NUL and 0x1A bytes are ignored
  and counted as `io.ifc.bytes_ignored`, and any other byte (a comment included) is ignored and counted
  as `io.ifc.trailing_bytes`.
- White space (space, tab, carriage return, line feed) and comments `/* ... */` are accepted between
  any two tokens and not inside a string. An unterminated comment is truncation.
- Words (entity type names, identifiers and parameter type names) are a letter, then letters, digits or
  `_`. A word is at most 255 bytes; a longer one is `io.ifc.syntax`, and a word longer than
  `string_bytes` is a `string_bytes` limit error. `-` is not part of a word, except in the section names
  `ISO-10303-21` and `END-ISO-10303-21`. A user-defined word starts with `!`; the `!` is kept in the
  type name. Entity type names are stored in upper case.
- Header records are kept in file order (`FILE_DESCRIPTION`, `FILE_NAME`, `FILE_SCHEMA` and others);
  only the first identifier of `FILE_SCHEMA` is read, as the schema. An identifier with a suffix such as
  `_ADD2` or `_TC1` names the schema without it. Any other identifier is `Schema::Other`, with its
  identifier in upper case, and is read without a schema-specific reading.
- Instances are `#n=TYPE(parameters);` for a simple instance and `#n=(TYPE(parameters)TYPE(parameters));`
  for a complex instance, which keeps one record per part. A complex instance with no record, `#n=();`,
  is a syntax error. The instance name is a decimal number up to `u64::MAX`. Instance names are unique;
  a second instance with the same name is refused. Instances may refer to instances declared later;
  references are resolved after the data section is read.
- Parameters: an integer (a sign and digits; the value must fit `i64`); a real (a point is required, the
  sign and the digits before the point are optional, and an exponent `E` with a sign is allowed; the
  value must be finite; an exponent without a point is refused; a lowercase `e` is a syntax error); a
  string (below); an enumeration such as `.T.` (kept as written, without the dots); a binary literal
  (`"`, a digit from 0 to 3, hexadecimal digits, `"`; kept as written); a reference `#n`; `$` (omitted);
  `*` (derived); a typed parameter `TYPE(value)`, whose type name is stored in upper case; and a list
  `(...)`, which may be empty.
- Strings: `''` stands for one apostrophe; `\\` for one backslash; `\S\c` for the character `c` with its
  upper half set, in ISO 8859-1, and only while page `A` is selected; `\X\hh` for the character U+00hh,
  whatever page is selected; `\X2\...\X0\` holds UTF-16 code units in four hexadecimal digits each, and a
  surrogate pair is combined; `\X4\...\X0\` holds code points in eight hexadecimal digits each. Both
  `\X2\` and `\X4\` denote Unicode characters whatever page is selected. `\P?\` selects the ISO 8859
  page whose letter is given (`A` to `I`); pages other than `A` have no table, so `\S\` after one of them
  is refused. A decoded U+0000, from any form, is refused as a raw NUL is. Raw bytes are UTF-8, except
  that a byte that starts no well-formed UTF-8 sequence is read as ISO 8859-1 and counted
  (`io.ifc.text_latin1`). A raw tab, carriage return or line feed is kept and counted
  (`io.ifc.text_control_kept`). Any other raw control byte below 0x20 is refused.
- What is not read: the content of the header beyond `FILE_SCHEMA` is not interpreted; the parameter
  count and the parameter types of an entity are not checked against the schema, and no WHERE rule is
  evaluated. An entity keeps the values its file gives, whatever the schema says of them.
- Memory estimate: the parser charges an estimate of the memory it allocates against `memory_bytes`:
  48 bytes per parameter, 64 bytes per record plus its type name, 96 bytes per instance, and the raw
  length of each string, binary literal, enumeration and type name. The constants are in
  `crates/arc-io-ifc/src/step.rs`. The estimate is kept on the file as `IfcFile::parsed_bytes`.

### The neutral model

`IfcFile` holds the schema, the header records and the instances in file order. `instance(name)` finds
an instance by name with a binary search over an index built once when the file is read, and
`instances_of(type)` iterates over the instances of a type through a second index, without a scan of
all instances per query. `Instance` holds its name and its records; `Record` holds its type name and its
parameters; `Parameter` is the enumeration of the parameter kinds above. `parsed_bytes()` is the memory
estimate of the parser.

### Typed views

`IfcFile::typed_views(limits, cancel)` returns `TypedViews`: the spatial structure, the type
assignments, the property sets, the quantity sets and a loss report of the views.

- Sharing: each distinct instance is materialized once per `TypedViews`. Entities, properties and
  quantities are held in `Arc` values, and the spatial nodes, assignments and sets refer to them. A
  name listed many times costs one value and a loss count for the repeats.
- Lists: every list of related instance names is deduplicated while it is read; a repeated name in a
  list, or a repeated pair in a relationship, is dropped and counted once per relationship and reason as
  `io.ifc.duplicate_placement`.
- Budget: the views charge the bytes they allocate (copied strings, values, the fixed size of each
  struct and each shared reference) against `memory_bytes` minus `IfcFile::parsed_bytes()`, before the
  allocation. They count the entries they emit against `record_count`. A failure is `io.limit_exceeded`
  naming `memory_bytes` or `record_count`.
- Rooted attributes are read from the positions `GlobalId` (0), `Name` (2) and `Description` (3) of any
  entity that has them. A wrongly typed value is read as absent and counted as dropped.
- Spatial structure: `IfcProject`, `IfcSite`, `IfcBuilding`, `IfcBuildingStorey`, `IfcSpace`,
  `IfcSpatialZone`, `IfcFacility`, `IfcFacilityPart`, `IfcRoad`, `IfcBridge`, `IfcRailway` and
  `IfcMarineFacility`. The aggregation relationships `IfcRelAggregates` (`RelatingObject` 4,
  `RelatedObjects` 5) link spatial elements. A child whose parent or whose own type is not spatial is
  kept as a generic instance and counted as `io.ifc.not_mapped`; a name that is not in the file is
  counted as `io.ifc.target_missing`. The roots are the spatial elements that no spatial element
  aggregates, in file order, and the children are in the order of the relationships. The elements
  contained in a spatial structure come from `IfcRelContainedInSpatialStructure` (`RelatedElements` 4,
  `RelatingStructure` 5), in file order; a structure that is not spatial is `io.ifc.not_mapped`.
- A child placed a second time, under a second parent or through a repeated pair, is not placed again
  and is counted as `io.ifc.duplicate_placement`. A child on the path from its own root is a cycle, cut
  and counted as `io.ifc.cycle_cut`; a spatial element that no root reaches is cut the same way. The
  depth of the tree is bounded by `nesting_depth` of the profile and by 64 whatever the profile says,
  and a deeper tree fails with `io.limit_exceeded` naming `nesting_depth`.
- Element types: `IfcRelDefinesByType` (`RelatedObjects` 4, `RelatingType` 5) gives a type assignment for
  each element.
- Property sets: `IfcRelDefinesByProperties` (`RelatedObjects` 4, `RelatingPropertyDefinition` 5) assigns
  an `IfcPropertySet` (`HasProperties` 4) to objects. The definition may be one reference or the IFC4 form
  `IFCPROPERTYSETDEFINITIONSET((#a,#b))`, whose members are read as the definitions. A definition that is
  missing, or that is neither a property set nor an element quantity, is `io.ifc.target_missing`. The
  properties read are `IfcPropertySingleValue` (`NominalValue` 2, `Unit` 3), `IfcPropertyEnumeratedValue`
  (`EnumerationValues` 2, `EnumerationReference` 3), `IfcPropertyListValue` (`ListValues` 2, `Unit` 3) and
  `IfcPropertyBoundedValue` (`UpperBoundValue` 2, `LowerBoundValue` 3, `Unit` 4, `SetPointValue` 5), with
  `Name` 0 and `Description` 1 on each. `SetPointValue` exists in IFC4 and IFC4X3 only, so IFC2X3 gives no
  set point. A property of another type, and a complex property, are `io.ifc.not_mapped`; another kind
  of member is `io.ifc.target_missing`. A nominal value is kept with its wrapping type when the file wraps
  it (`IFCLABEL('x')`) and is a text, a real, an integer or an enumeration; `IFCBOOLEAN(.T.)` is the
  enumeration `T`. A unit is kept as the instance name of its unit. Any other value kind is dropped and
  counted.
- Quantity sets: `IfcElementQuantity` (`Quantities` 5) with `IfcQuantityLength`, `IfcQuantityArea`,
  `IfcQuantityVolume`, `IfcQuantityCount`, `IfcQuantityWeight` and `IfcQuantityTime`. Each quantity reads
  `Name` 0, `Description` 1, `Unit` 2, its value at 3 and `Formula` 4. `Formula` exists in IFC4 and
  IFC4X3 only, so IFC2X3 gives no formula. A count is an integer, the others are reals; a measure wrapper
  such as `IFCAREAMEASURE(12.5)` is read through. Another quantity type is `io.ifc.not_mapped`.
- Schemas: the reader reads attributes by position and does not consult the schema. The positions of the
  rooted attributes, the relationships, the property values and the quantity values agree in IFC2X3, IFC4
  and IFC4X3. The two exceptions are the layouts named above (`Formula` and `SetPointValue`), which are
  absent in IFC2X3. The minimal project is tested in each schema identifier, with an IFC2X3 layout of its
  quantities and bounded value; the schema of a file is available as `IfcFile::schema`.
- A relationship member of the wrong kind drops that member only; the other members of the list are kept.
- Entity types the views do not read stay generic instances and are not part of the views.

### Loss reports

`read` returns a loss report of the read; `typed_views` returns a loss report of the views. Both are
`LossReport` values in the form of `schemas/loss-report.schema.json`. Items are aggregated per reason and
record kind, and the number of distinct items is capped (the overflow is one item per reason, with exact
counts). The read reports every record of an entity type that the views do not read
(`io.ifc.entity_preserved`), every reference to an instance that is not in the file
(`io.ifc.reference_unresolved`), a schema the reader does not know (`io.ifc.schema_unrecognized`), and the
leniency and skipping rules of the reading section. The views report values of the wrong kind
(`io.ifc.value_unexpected`), placements and members they do not use, and cut relationships. This unit
reports no mapped records, so the `mapped` count is zero.

| Reason | Outcome |
|---|---|
| `io.ifc.entity_preserved` | `preserved` |
| `io.ifc.reference_unresolved` | `preserved` |
| `io.ifc.schema_unrecognized` | `preserved` |
| `io.ifc.value_unexpected` | `dropped` |
| `io.ifc.cycle_cut` | `dropped` |
| `io.ifc.duplicate_placement` | `dropped` |
| `io.ifc.target_missing` | `dropped` |
| `io.ifc.not_mapped` | `preserved` |
| `io.ifc.section_skipped` | `dropped` |
| `io.ifc.archive_entry_skipped` | `dropped` |
| `io.ifc.text_latin1` | `approximated` |
| `io.ifc.text_control_kept` | `preserved` |
| `io.ifc.bytes_ignored` | `dropped` |
| `io.ifc.trailing_bytes` | `dropped` |
| `io.ifc.geometry_not_evaluated` | `dropped` |
| `io.ifc.geometry_degenerate` | `dropped` |
| `io.ifc.index_invalid` | `dropped` |
| `io.ifc.opening_not_subtracted` | `approximated` |
| `io.ifc.unit_assumed` | `approximated` |
| `io.ifc.geometry_approximated` | `approximated` |

### Limits

Every limit of the profile is checked before the element it bounds is stored, and a failure names the
limit in the argument `limit` of `io.limit_exceeded`.

| Limit | Enforced on |
|---|---|
| `input_bytes` | The input, before any parsing; each buffer of an archive entry is the same input, and no archive entry inflates beyond it |
| `decompressed_bytes`, `decompression_ratio` | The archive entry, through `arc-io-common` |
| `record_count` | Instances of the data section; the entries the views emit |
| `array_length` | Parameters of a record, elements of a list, records of a complex instance, header records |
| `nesting_depth` | Open parentheses (lists and typed parameters), and the depth of the spatial tree; at most 64 whatever the profile |
| `string_bytes` | The raw content of a string, a binary literal, an enumeration and a word |
| `memory_bytes` | The parser's estimate of its allocations, then the bytes the views copy, from the share the parser left; `None` leaves the budget to the caller and nothing is checked |
| `wall_clock_seconds` | The time between the start of the read and a check every 1024 parser steps, through the injected clock; `None` is unlimited |

### Cancellation and time

Cancellation is checked before each instance and each list element of the parser, after parsing and
before the file is built, before each instance of the loss report, before each instance and each node of
the views; a cancelled read returns `base.cancelled`.

`ReadOptions::clock` supplies the monotonic time for the wall-clock limit (`Clock`, milliseconds from an
origin of the source's choosing). The default is `SystemClock`, whose origin is its creation. On
`wasm32-unknown-unknown` there is no default clock, because the system clock is not available there, so
the wall-clock limit is not enforced unless the caller supplies a clock. The views do not check time.

### Geometry evaluation as implemented

`IfcFile::geometry(limits, cancel)` evaluates the geometry of the products of the file;
`IfcFile::geometry_with_tolerance` takes the `arc_base::Tolerance` to use, and `geometry` uses
`Tolerance::DEFAULT`. The output is a `Geometry`: the items, the units and a loss report of the
evaluation. The representation items that are not listed below are not evaluated and are reported.

- Products: an instance whose sixth attribute `Representation` (position 6) refers to an
  `IfcProductDefinitionShape`. Its `ObjectPlacement` (position 5) is the placement; an omitted
  placement is the identity. The representations are the `Representations` (position 2) of the
  shape, each read once, and the items of a representation are its `Items` (position 3), each
  evaluated once. The representation identifier and type (positions 1 and 2) are kept as text: the
  items of one representation share them (`Arc<str>`), and each is charged once per representation.
  A representation whose type is not `IfcShapeRepresentation` (a topology representation, for
  example) is not evaluated: it is reported as `io.ifc.geometry_not_evaluated` with its type as the
  record kind, and nothing is emitted for it. The representation context and its world coordinate
  system are not read: the coordinates of a representation are world coordinates.
- Output: a `GeometryItem` per evaluated piece of a representation item, with `product` and `item`
  (the instance names of the product and of the representation item, or of the product's mapped item
  for an item of a mapped representation), `representation_identifier`, `representation_type`, `shape`
  and `transform`. The `shape` is `Shape::Mesh` (a `TriMesh` whose counterclockwise triangles face
  outward for a closed solid) or `Shape::Polylines` (polylines of at least two points). The
  `transform` maps shape coordinates to world coordinates. The shape and the transform are in meters;
  a point in world coordinates is `transform * point`. A transform may be a reflection, which
  `GeometryItem::world_mesh(limits)` applies while reversing the winding of the triangles; that method
  returns `None` for polylines. Items are in product order, then in the order of the representations
  and of their items.
- Units: the unit assignment of the project (`IfcProject` attribute `UnitsInContext`, position 8), or
  else the first `IfcUnitAssignment` of the file. Length units are `IfcSIUnit` with `LENGTHUNIT`,
  `METRE` and an optional prefix, and `IfcConversionBasedUnit` whose `ConversionFactor` is an
  `IfcMeasureWithUnit` referring to an `IfcSIUnit`; its value times the factor of that unit is the
  length unit. A factor that is zero, negative or not finite makes the unit unusable and it is ignored.
  Plane angle units (`RADIAN`, or a conversion-based unit such as a degree) give
  `Units::radians_per_angle_unit`; no evaluated item reads an angle. Each length read (point
  coordinates, placement locations, profile dimensions, radii, depths) is multiplied by the length unit
  in meters, and a product that is not finite after the multiplication skips its item. With no usable
  length unit, meters and radians are assumed and reported once as `io.ifc.unit_assumed`;
  `Units::length_assumed` is then `true`.
- Placement: `IfcAxis2Placement3D` (`Location` 0, `Axis` 1, `RefDirection` 2) and `IfcAxis2Placement2D`
  (`Location` 0, `RefDirection` 1, placed in the plane `z = 0`). An absent axis is (0, 0, 1). An absent
  reference direction is the first of (1, 0, 0) and (0, 1, 0) that is not parallel to the axis. The
  reference direction is orthogonalized against the axis, and the second axis is the cross product of
  the axis and the reference direction. A zero or parallel direction skips the item. `IfcLocalPlacement`
  (`PlacementRelTo` 0, `RelativePlacement` 1, an `IfcAxis2Placement3D` or an `IfcAxis2Placement2D`) is
  composed from the top of the chain down. The chain is walked iteratively with a visited set and the
  depth bound; a repeated placement is cut and reported as `io.ifc.cycle_cut` with the record kind
  `IFCLOCALPLACEMENT`. Another placement kind, such as `IfcGridPlacement`, is not evaluated and skips
  the product's items.
- Evaluated representation items, with the attribute positions of IFC2X3, IFC4 and IFC4X3:

| Item | Output | Notes |
|---|---|---|
| `IfcExtrudedAreaSolid` (`SweptArea` 0, `Position` 1, `ExtrudedDirection` 2, `Depth` 3) | mesh | Caps and sides. The direction is normalized and may be skew to the profile plane. A depth that is not positive, and a direction parallel to the profile plane, skip the item. |
| `IfcRectangleProfileDef` (`Position` 2, `XDim` 3, `YDim` 4) | loop | Centered on its position; both dimensions positive. |
| `IfcCircleProfileDef` (`Position` 2, `Radius` 3) | loop | A regular polygon; see Bounds for the segment count. |
| `IfcArbitraryClosedProfileDef` (`OuterCurve` 2) | loop | |
| `IfcArbitraryProfileDefWithVoids` (`OuterCurve` 2, `InnerCurves` 3) | loops | Holes are bridged to the outer loop. |
| `IfcPolyline` (`Points` 0) and `IfcIndexedPolyCurve` (`Points` 0, `Segments` 1) inside a profile | loop | Line segments only; the points must lie in the plane. |
| `IfcTriangulatedFaceSet` (`Coordinates` 0, `CoordIndex` 3, `PnIndex` 4) | mesh | `Coordinates` is an `IfcCartesianPointList3D`. |
| `IfcPolygonalFaceSet` (`Coordinates` 0, `Faces` 2, `PnIndex` 3) with `IfcIndexedPolygonalFace` (`CoordIndex` 0) and `IfcIndexedPolygonalFaceWithVoids` (`InnerCoordIndices` 1) | mesh | Each face is triangulated in its plane. |
| `IfcFacetedBrep` (`Outer` 0) | mesh | The outer `IfcClosedShell` (`CfsFaces` 0). |
| `IfcFaceBasedSurfaceModel` (`FbsmFaces` 0) and `IfcShellBasedSurfaceModel` (`SbsmBoundary` 0) | mesh | The shells and connected face sets are `IfcClosedShell`, `IfcOpenShell` or `IfcConnectedFaceSet`. |
| `IfcFace` (`Bounds` 0), `IfcFaceOuterBound` and `IfcFaceBound` (`Bound` 0, `Orientation` 1), `IfcPolyLoop` (`Polygon` 0) | part of a mesh | The outer bound is the first `IfcFaceOuterBound`, or else the first bound. `Orientation` `.F.` reverses the loop. Any other loop type is not evaluated. |
| `IfcMappedItem` (`MappingSource` 0, `MappingTarget` 1) | the pieces of the mapped representation | `IfcRepresentationMap` (`MappingOrigin` 0, `MappedRepresentation` 1). `MappingTarget` is an `IfcCartesianTransformationOperator3D` or its `...3DnonUniform` form (`Scale` 3, `Scale2` 5, `Scale3` 6, which default to `Scale`). The map's items are composed with `MappingTarget * MappingOrigin`. |
| `IfcPolyline` (`Points` 0) and `IfcIndexedPolyCurve` (`Points` 0, `Segments` 1) as representation items | polylines | Two or three coordinates per point; a two-dimensional point is at `z = 0`. Line segments only. |

- Cartesian transformation operators (`MappingTarget` of an `IfcMappedItem`): the third axis is `Axis3`
  (default (0, 0, 1)), normalized. The first axis is `Axis1` (default (1, 0, 0)) made orthogonal to the
  third and normalized. The second is `Axis2` (default: the third crossed with the first) made
  orthogonal to the third and to the first and normalized, so the basis has no shear. A given axis
  that is parallel to an axis it must be orthogonal to is replaced by a perpendicular direction (the
  projection of (1, 0, 0), or else of (0, 1, 0)), and the mapped item is reported as
  `io.ifc.geometry_approximated` with the record kind `IFCMAPPEDITEM`. A `Scale`, `Scale2` or `Scale3`
  that is not positive skips the item as `io.ifc.geometry_degenerate`; a mirror is expressed through
  the axes, not through a negative scale.
- Not evaluated: every other representation item, profile, curve, loop, shell or placement kind. Each
  is reported as `io.ifc.geometry_not_evaluated` with the record kind of the entity, one item per kind
  with its count, and nothing is emitted for it. This covers `IfcBooleanResult`,
  `IfcBooleanClippingResult` (no operand is emitted in its place), `IfcSweptDiskSolid`,
  `IfcRevolvedAreaSolid`, `IfcAdvancedBrep`, `IfcExtrudedAreaSolidTapered`, the CSG primitives, the other
  profile types, `IfcArcIndex` (the indexed poly-curve holding it is skipped), the other curve types and
  `IfcEdgeLoop`. An item is emitted as a whole or not at all: a shell with any face that cannot be
  evaluated is not emitted.
- Openings: `IfcRelVoidsElement` relationships are not subtracted. The report holds one
  `io.ifc.opening_not_subtracted` item per file whose count is the number of such relationships, and
  the elements are emitted solid.
- Skips: an attribute of the wrong type skips its item and is reported as `io.ifc.value_unexpected`. An
  index outside its list, or a value that is not an index, is `io.ifc.index_invalid`. A degenerate item
  (zero or negative depth, a loop with zero area, a zero direction, a scale that is not positive, a
  region whose holes are not valid, or a result that is not finite) is `io.ifc.geometry_degenerate`. A
  mapped item whose representation reaches the same mapped item again is cut and reported as
  `io.ifc.cycle_cut` with the record kind `IFCMAPPEDITEM`. Each reason is reported once per record kind
  with its count.
- Repeats: a list where order and multiplicity are the geometry keeps its repeats: the points of an
  `IfcPolyline` and of an `IfcPolyLoop`, the point indices of a face and the bounds of an `IfcFace`. A
  polyline keeps a point that comes back, so `(p1, p2, p1, p3)` has four points. A closing repeat of a
  loop (its last point the same instance as its first) is dropped. The segments of an
  `IfcIndexedPolyCurve` drop only a repeated join point. A face loop may reuse a point instance, as a
  face with a keyhole does. The other lists are read once, in the order of first occurrence: the
  representation items of a representation (a representation item listed twice is evaluated once and
  its repeat is counted as `io.ifc.duplicate_placement`), the representations of a product, the faces of
  a face set or shell, the shells of a model and the inner curves of a profile. Those lists drop their
  repeats without a report.
- Shared geometry: the pieces of a representation item are computed once per file and cached by the
  item's instance name. A mapped item does not copy the pieces of its representation: each piece keeps
  its `Arc<Shape>` and receives the transform composed from the mapping. Products that map one
  representation hold the same `Arc`, and the vertices are stored once. The alternative, a copy of the
  mesh with the transform applied for each use, multiplies the stored geometry by the number of uses.
  An item whose evaluation cut a mapping cycle is not cached: its pieces depend on the items being
  evaluated around it, so a later reference evaluates it again.
- Decisions: planar polygons (profiles and faces) are triangulated by an ear-clipping triangulation with
  hole bridging, implemented in the crate. The flattening triangulation of `arc-geom` was not used,
  because it drops every segment no longer than the chord tolerance; two faces that share such an edge
  could then keep different vertices and the shell would open. The triangulation keeps the input
  vertices, and every loop point is a corner of some triangle. Before bridging, a region is refused
  unless every hole vertex is strictly inside the outer loop and in no other hole, and no edge of one
  loop touches an edge of another loop; the edge test is quadratic in the number of points. The holes
  are bridged to the outer loop in order of decreasing maximum x, each from its rightmost vertex. The
  output is checked: a ring of `n` points gives `n - 2` triangles, each counterclockwise. A loop that
  crosses itself is not detected. Collinear points are kept in faces, so that a neighbouring face that
  keeps the same point meets it; the profiles of extrusions drop them, and their caps and sides use the
  same prepared ring. Vertices of face sets are welded by their coordinates (the bit pattern, with the
  sign of zero normalized). A face that is not planar within the linear tolerance is triangulated in the
  projection onto the mean plane of its outer loop, with the original vertices, and reported as
  `io.ifc.geometry_approximated`.

#### Bounds

Every allocation is charged to one budget of the call before it is made, against `memory_bytes` minus
`IfcFile::parsed_bytes()`: 24 bytes per vertex, 12 per triangle, 48 per entry of the vertex index, 96 per
shape, 160 per mapped piece, 128 per placement, 32 per reference of a list, the size of one emitted
`GeometryItem` per item, the length of each representation text once per representation, and 48 per point
copy and 48 per triangle of a triangulation, charged before the triangulation runs. Each evaluated
representation item, each mapped piece and each emitted item counts one record against `record_count`. A
mesh, a polyline or a point list longer than `array_length` is refused, and a face set is refused by its
declared list length before its points are read. Exceeding a limit returns `io.limit_exceeded` with the
argument `limit` naming `memory_bytes`, `record_count` or `array_length`.

- A circle has `n = ceil(pi / acos(1 - c / r))` segments for the chord tolerance `c` (`tessellation_chord`)
  and radius `r`, at least 3 and at most 512 (`MAX_CIRCLE_SEGMENTS`). A circle that needs more is
  tessellated with 512 segments and reported as `io.ifc.geometry_approximated`.
- A loop of more than 4096 points (`MAX_LOOP_VERTICES`) is not evaluated and is reported as
  `io.ifc.geometry_not_evaluated`, with the record kind of the curve, loop or solid that holds it; the
  list is refused before its points are built. The loops of one face or profile are capped in total as
  well: the outer loop and every hole, each hole's two bridge points counted, together at most 4096
  points, or the face or profile is not evaluated and is reported the same way. Polylines are not
  triangulated and have no point limit beyond `array_length`.
- Placement chains and mapped nesting share one depth counter, capped at the `nesting_depth` of the
  profile and at 64. A deeper nesting returns `io.limit_exceeded` with `limit` set to `nesting_depth`.
- Cancellation is checked before each product and every 1024 steps. A step is taken per face, row, point,
  bound, representation item, mapped item and piece of a loop over a list, and per candidate of the ear
  search; a cancelled evaluation returns `base.cancelled`. `wall_clock_seconds` is checked by the parser
  only.
- The evaluation adds no error codes. Its loss reasons are listed in the table of the loss reports.

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `io.limit_exceeded` | `limit_exceeded` | A limit of the profile was exceeded; the argument `limit` names it |
| `io.ifc.truncated` | `corrupt_data` | The input ends inside a comment, a string, a list, a record or a section |
| `io.ifc.syntax` | `corrupt_data` | A token does not fit the grammar at its position; a word longer than 255 bytes; an empty complex instance; a lowercase exponent |
| `io.ifc.structure_invalid` | `corrupt_data` | A section is missing or out of order, no data section is present, or `FILE_SCHEMA` is missing or malformed |
| `io.ifc.text_invalid` | `corrupt_data` | A string holds a control character other than tab, carriage return or line feed, a NUL (raw or decoded), a malformed encoded form, or an unsupported page for `\S\` |
| `io.ifc.value_invalid` | `corrupt_data` | A number is out of range or not finite, an exponent has no point, or a binary or enumeration literal is malformed |
| `io.ifc.instance_duplicate` | `corrupt_data` | Two instances have the same name, in one or several data sections |
| `io.ifc.archive_entry_missing` | `not_found` | The archive has no entry whose name ends in `.ifc` after the metadata entries are skipped |
| `io.ifc.archive_entry_ambiguous` | `unsupported` | The archive has more than one entry whose name ends in `.ifc` after the metadata entries are skipped |

The `io.zip.*` codes of the archive reader are returned as the `arc-io-common` section defines them.
