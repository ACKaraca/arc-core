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
| `arc-io-mesh` | `arc-base`, `arc-geom`, `arc-mesh` | `gltf`, `tobj`, `stl_io`, `threemf`, `image`, `exr` |
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
`arc-geom` and `arc-draft` and on no third-party crate: both writers are Arc-owned, written from
the published SVG 1.1 and PDF 1.7 file structures, so `krilla` and `lopdf` of the dependency table
are not used by this part. It reads and writes byte buffers, never paths, and builds for
`wasm32-unknown-unknown`. The stream's units are millimeters from the lower left corner of the
page.

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

`base.cancelled` is returned when the token is cancelled.
