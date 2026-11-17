# arc-io family

One crate per interchange format family, each exposing a neutral model of its format: `arc-io-dxf`,
`arc-io-dwg`, `arc-io-ifc`, `arc-io-step`, `arc-io-mesh`, `arc-io-usd`, `arc-io-pointcloud`,
`arc-io-geo`, `arc-io-pdf` and `arc-io-bcf`. The architecture, library choices and limits are
specified in [interchange.md](../architecture/interchange.md).

## Owns

- Readers and writers for each format, with limits profiles, cancellation and progress.
- The neutral model of each format, including source identifiers and preserved records.
- Loss-report items for format-level losses (unsupported versions, unknown records, approximations).
- The loss-report schema and the shared bounded readers for ZIP and XML used by the family.
- Fuzz targets, hostile-input tests and corpus runners for each format.
- Shared core mappings: drafting plot streams to PDF and SVG (`arc-io-pdf`).

## Stays in products

All mapping between neutral models and product domain models, the fidelity level per format and
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

## Test strategy

Per crate: unit tests for every record kind; round-trip tests (read, write, re-read, compare neutral
models); a hostile-input test over malformed files asserting errors and no panics; a fuzz target per
reader; oracle comparison in the corpus job; determinism tests for writers (identical bytes for
identical input); limit tests that construct inputs exceeding each limit.

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
