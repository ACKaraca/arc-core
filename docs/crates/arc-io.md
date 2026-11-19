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
- The loss-report schema and the shared bounded readers for ZIP and XML used by the family.
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

## Dependencies

| Crate | Arc | Third-party |
|---|---|---|
| `arc-io-dxf` | `arc-base`, `arc-geom` | none |
| `arc-io-dwg` | `arc-base`, `arc-geom`, `arc-command` (worker jobs) | `acadrust` |
| `arc-io-ifc` | `arc-base`, `arc-geom`, `arc-mesh` | `ifc-lite`, `quick-xml`, `zip` |
| `arc-io-step` | `arc-base`, `arc-brep`, `arc-command` (worker jobs) | none beyond the kernel tier |
| `arc-io-mesh` | `arc-base`, `arc-mesh` | `serde` (loss report only); OBJ with MTL, STL and PLY are implemented in the crate; `gltf`, `threemf`, `image` and `exr` are for the formats that follow |
| `arc-io-usd` | `arc-base`, `arc-geom`, `arc-mesh` | `openusd` |
| `arc-io-pointcloud` | `arc-base`, `arc-geom`, `arc-doc` (asset storage of octrees) | `las`, `laz`, `e57` |
| `arc-io-geo` | `arc-base`, `arc-geom` | `proj`, `geo`, `geozero`, `shapefile`, `flatgeobuf`, `rusqlite`, `quick-xml` |
| `arc-io-pdf` | `arc-base`, `arc-geom`, `arc-draft`, `arc-command` (worker jobs), `arc-pdfium` (in `arc-worker` only) | `krilla`, `lopdf` |
| `arc-io-bcf` | `arc-base` | `quick-xml`, `zip`, `image` |
| `arc-io-exchange` | `arc-base`, `arc-geom`, `arc-mesh` | `zip`, `serde_json`, `jsonschema`, `blake3` |
| `arc-io-3dm` | `arc-base`, `arc-geom`, `arc-mesh`, `arc-command` (worker jobs), `arc-opennurbs` (in `arc-worker` only) | none beyond openNURBS |
| `arc-io-fbx` | `arc-base`, `arc-geom`, `arc-mesh` | `fbxcel`, `fbxcel-dom` |
| `arc-io-dgn` | `arc-base`, `arc-geom` | none |
| `arc-io-bridge` | `arc-base`, `arc-command` (worker jobs) | none |
| `arc-io-common` | `arc-base` | `miniz_oxide` (inflate), `quick-xml` (XML parsing) |

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
| CORE-IO-002 | Limits enforcement | Every reader enforces the caller's limits profile and stops with `io.limit_exceeded` naming the limit, never allocating beyond it. | T1 | 1 |
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
| CORE-IO-037 | PDF output | Writes plot streams to vector PDF with embedded subset fonts, layers as optional content groups, hyperlinks and multiple pages; PDF/A-2b on request. | T1 | 1 |
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
- **Text.** Stroke text is stroke items and is written like any stroke. Outline glyph runs are not
  written: each is a loss item (`io.pdf.glyphs_dropped`, `dropped`). Embedded subset fonts and
  PDF/A-2b are not provided; PDF/A is not requestable.
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
page tree, information dictionary, optional content groups, then page and content stream per page.

- **Pages.** The media box is the page in points (1 mm = 72 / 25.4 pt) from the lower left
  corner, the origin of the stream.
- **Content.** Content streams are not compressed (no encoder is added; the vector content is
  compact and a deflate stream would add a dependency for no capability). Operators are `m`, `l`,
  `c`, `h`, `re`, `W n`, `S`, `f`, `w`, `J`, `j`, `M`, `RG`, `rg`, `gs`, `q`, `Q`, `BDC`, `EMC`.
- **Arcs.** PDF has no arc operator. An arc or ellipse is cubic Bezier pieces with control points
  from the tangents (factor 4/3 tan(a/4) for a piece of angle a). The piece count is the least
  that keeps the radial error of a circle of radius r below `chord_mm`, using the error
  0.00027 r (a / 90 degrees)^6 of a piece and at most 90 degrees per piece, at most 1024 pieces
  per arc; for an ellipse r is the larger semi-axis, which bounds the error. The count of arcs
  written this way is a loss item (`io.pdf.arcs_approximated`, `approximated`, argument
  `tolerance_mm`).
- **Layers.** Each distinct layer of the items of all pages is an optional content group named
  by the layer's object identifier, listed in the catalog (`OCProperties`, all on), ordered by
  identifier, and used per page through `Properties`. Consecutive items with the same clip and
  layer form one `q ... BDC ... EMC ... Q` run; items without a layer are outside any group.
  The stream carries identifiers, not layer names; a product maps the names (a gap).
- **Information.** `Producer` is `Arc arc-io-pdf`; `Title` is a literal string for printable
  ASCII and UTF-16 hexadecimal otherwise; `CreationDate` is written only when
  `creation_date` is set.
- Not written: hyperlinks, embedded fonts, PDF/A-2b, compression, a file identifier.

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
| `decompressed_bytes` | Each declared size and the sum of declared sizes at open; the output of all reads together |
| `decompression_ratio` | Each declared size against the compressed size times the ratio, at open; the output of an inflate call |

### ZIP archives

- `ZipArchive::open(bytes, options)` validates the archive before any entry is inflated and returns
  the entries in directory order. `read_entry(index)` inflates one entry and returns its bytes after
  its CRC-32 is checked.
- Validation order: the input size; the end record, found by a backward search over at most 65,558
  offsets for a signature whose comment length ends at the end of the buffer; the ZIP64 locator and
  end record when a field holds its marker; the disk fields (all zero, otherwise `io.zip.multi_disk`);
  every central record, with the count checked against `record_count` and against the directory size
  before the directory is allocated; for each entry, the name, the encryption flags, the method, the
  declared sizes and the ratio, and the uniqueness of the name; for each entry, its local header
  (signature, name, method, flags, and CRC and sizes unless a descriptor follows the data) and its
  data descriptor when the central record sets bit 3; finally the layout, in which no two of the
  entry ranges, the directory range and the trailing end records share a byte.
- Supported methods are stored (0) and deflate (8). Encrypted entries (flags bit 0, bit 6) are refused
  with `io.zip.encrypted`; other methods with `io.zip.method_unsupported`.
- ZIP64 is read where a 32-bit field holds `0xFFFFFFFF` or an entry count holds `0xFFFF`: the sizes
  and the offset come from the ZIP64 extension, and the values are then checked like any others.
- A data descriptor is accepted when its CRC and sizes equal the central record's (the signature is
  optional, the sizes are 8 bytes for a ZIP64 record); otherwise `io.zip.data_descriptor_unverified`.
- Offsets are absolute. Data before the first entry (for example a self-extracting stub) makes the
  local headers disagree with the directory and is refused as `io.zip.header_mismatch`.
- Names: the bytes are UTF-8 when flag bit 11 is set. Otherwise each byte maps to the code point of
  the same value (ISO 8859-1), which names the bytes without claiming a code page. A name is refused
  (`io.zip.name_invalid`) when it is empty, contains a control character or an empty component, or
  is not valid UTF-8. It is refused (`io.zip.path_unsafe`) when it starts with `/`, contains a
  backslash or a colon, or has a `.` or `..` component. A trailing `/` marks a directory. Two names
  are duplicates (`io.zip.name_duplicate`) when their paths are equal after lowercasing every
  character by its simple Unicode mapping; canonical equivalence of Unicode forms is not considered.
- While inflating, the output is capped by the declared size, by the ratio and by the remaining
  budget, and the stream is stopped one byte past the cap. A stream that produces a different
  number of bytes from its declared size, or leaves compressed bytes unread, is `io.zip.size_mismatch`.
- The budget is the archive's: every read charges its output against `decompressed_bytes`, so
  reading an entry again uses budget again.

### XML documents

- `XmlReader::new(bytes, options)` returns a pull reader; `next_event()` returns `Event::Start`
  (name and attributes), `Event::End` (name) or `Event::Text`, and `None` at the end. An empty
  element yields a start and an end. Adjacent character data, CDATA sections and references form one
  text node. Comments and the XML declaration are consumed without an event.
- Attribute values are normalized as XML requires (white space becomes a space; character references
  are kept). Text line ends become line feeds. Names and values are UTF-8.
- Refused: a document type declaration (`io.xml.doctype_refused`), a processing instruction
  (`io.xml.processing_instruction_refused`), any entity reference other than the five predefined
  entities and character references (`io.xml.entity_refused`), a character reference to a value that
  XML does not allow (`io.xml.reference_invalid`), and a declared encoding other than UTF-8 or US-ASCII,
  or a UTF-16 byte-order mark (`io.xml.encoding_unsupported`). Entity expansion and external
  entities therefore cannot occur.
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
| `io.zip.structure_invalid` | `corrupt_data` | A signature, count, offset or size of the archive's structures is inconsistent |
| `io.zip.multi_disk` | `unsupported` | The archive spans several disks |
| `io.zip.encrypted` | `unsupported` | An entry is encrypted |
| `io.zip.method_unsupported` | `unsupported` | An entry uses a method other than stored or deflate |
| `io.zip.data_descriptor_unverified` | `corrupt_data` | A data descriptor does not agree with its central record |
| `io.zip.header_mismatch` | `corrupt_data` | A local header does not agree with its central record |
| `io.zip.overlap` | `corrupt_data` | Two parts of the archive share a byte |
| `io.zip.name_invalid` | `corrupt_data` | An entry name is empty, malformed or not valid UTF-8 |
| `io.zip.path_unsafe` | `corrupt_data` | An entry name is absolute, contains a backslash or a colon, or has a `.` or `..` component |
| `io.zip.name_duplicate` | `corrupt_data` | Two entry names are equal after normalization |
| `io.zip.size_mismatch` | `corrupt_data` | Compressed data does not produce its declared size, or leaves bytes unread |
| `io.zip.inflate_failed` | `corrupt_data` | A deflate stream is malformed |
| `io.zip.crc_mismatch` | `corrupt_data` | The data of an entry does not match its CRC-32 |
| `io.zip.entry_missing` | `not_found` | The index names no entry |
| `io.xml.truncated` | `corrupt_data` | The document ends inside a tag, a text node or an open element, or has no root |
| `io.xml.syntax` | `corrupt_data` | The markup is not well formed |
| `io.xml.structure_invalid` | `corrupt_data` | Elements do not nest, a second root starts, text appears outside the root, or an attribute repeats |
| `io.xml.text_invalid` | `corrupt_data` | A name, value or text is not UTF-8 or contains a character XML does not allow |
| `io.xml.doctype_refused` | `unsupported` | The document has a document type declaration |
| `io.xml.processing_instruction_refused` | `unsupported` | The document has a processing instruction |
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
