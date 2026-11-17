# arc-draft

The 2D drafting model shared by all products: spaces and units, entities, styles, annotation
scaling, blocks, external references, layouts and sheets, text layout and the plot pipeline. The
architecture is specified in [drafting-and-views.md](../architecture/drafting-and-views.md).

## Owns

- Drafting object types (`core.draft.*`) and their validators, regeneration of display lists, and
  migrations.
- Appearance resolution (object, block, layer, viewport override) and plot style resolution.
- Style tables: layers, linetypes, text, dimension, leader and table styles, hatch patterns.
- Annotation scales and scale representations.
- Block definitions, references, attributes and arrays; external drawing reference placement.
- Layouts, sheet viewports and their clipping and overrides.
- Text layout with `parley` and `swash`, the Arc single-stroke font format, bundled fonts, the font
  provider interface and substitution.
- Display lists, their conversion to plot streams, and raster output (PDF and SVG writers live in
  `arc-io-pdf`).
- Arc's linetype and hatch-pattern libraries.

## Stays in products

Drawing commands and their prompt machines, dynamic block behavior, sheet set management, DWG and
DXF mapping (DraftBuilder), discipline symbols and toolset objects, title block content, and domain
annotation objects (tags bound to BIM elements, station labels).

## Key concepts and types

| Concept | Description |
|---|---|
| `Space` | Model space or layout; unit; owns entities in draw order |
| `Entity` | Common header (layer, color, linetype, linetype scale, lineweight, transparency, plot style, visibility) plus kind data |
| `Layer` | Style table entry with per-viewport overrides |
| `Linetype`, `HatchPattern` | Pattern definitions in pattern units |
| `TextStyle`, `DimStyle`, `LeaderStyle`, `TableStyle` | Style table entries; dimension styles with per-kind children |
| `AnnotationScale` | Named ratio of sheet to model units |
| `BlockDef`, `BlockRef` | Definition with base point, unit and attribute definitions; reference with transform and attribute values |
| `Layout`, `SheetViewport` | Paper, plot settings, viewports with scale, clip and overrides |
| `DisplayList` | Exact primitives tagged with `ObjectId` and sub-object key |
| `PlotStyleTable` | Color-mapped or named mapping to pen output |
| `StrokeFont` | Arc single-stroke font data |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom`, `arc-doc`, `arc-command`, `arc-solve` (field expressions) |
| Third-party | `parley`, `swash`, `tiny-skia` (raster output), `lyon` (fill tessellation for raster output) |

## Test strategy

Reference-image tests of display lists rasterized on the CPU for every entity kind and style
feature; SVG and PDF byte-determinism tests; appearance-resolution table tests; annotation-scale
tests across viewports; dimension associativity tests that move geometry and assert updated
measurements; font round-trip and glyph-coverage tests; property tests for block nesting limits and
cycle rejection.

## Capabilities

### Spaces and entities

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DRAFT-001 | Spaces with declared units | Every model space and layout declares its unit; conversion between spaces happens only through viewport scales, block units and explicit commands. | T1 | 1 |
| CORE-DRAFT-002 | Curve entities | Provides points, lines, rays, construction lines, circles, arcs, ellipses, polylines with bulges and widths, and splines as document objects with exact geometry. | T1 | 1 |
| CORE-DRAFT-003 | Hatches and fills | Provides pattern, solid and gradient hatches with boundary loops, islands detection mode, pattern scale, angle and origin; associative hatches update when boundary objects change. | T1 | 1 |
| CORE-DRAFT-004 | Wipeouts | Provides masking regions drawn in the background color that hide entities beneath them in draw order. | T2 | 1 |
| CORE-DRAFT-005 | Draw order | Maintains draw order per space; commands can bring entities to front, send to back, or place above and below others. | T1 | 1 |
| CORE-DRAFT-006 | Single-line and rich text | Provides single-line text and multi-line rich text with paragraphs, runs, fonts, heights, colors, stacked fractions, tabs, columns and bidirectional text. | T1 | 1 |
| CORE-DRAFT-007 | Fields | Evaluates text fields from document data, object properties and expressions, updating when their sources change. | T2 | 1 |
| CORE-DRAFT-008 | Tables | Provides tables with merged cells, typed cell content including formulas, row-type styles and breaking across columns. | T2 | 1 |
| CORE-DRAFT-009 | Elevation and extrusion data | Preserves elevation, thickness and extrusion direction on 2D entities for interchange fidelity. | T2 | 1 |

### Annotation

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DRAFT-010 | Dimensions | Provides linear, aligned, rotated, angular, radial, diameter, arc length and ordinate dimensions with text override, measurement formatting and styles. | T1 | 1 |
| CORE-DRAFT-011 | Dimension associativity | Dimensions referencing geometry through sub-object keys update their measurement and position when the geometry changes, and become non-associative with a diagnostic when it is deleted. | T1 | 1 |
| CORE-DRAFT-012 | Dimension chains | Provides baseline and continued dimension chains that keep spacing when members are added or removed. | T2 | 1 |
| CORE-DRAFT-013 | Leaders | Provides leaders and multi-leaders with text, block or tolerance content, multiple arrows and alignment. | T1 | 1 |
| CORE-DRAFT-014 | Annotation scaling | Annotative objects keep one representation per supported scale; each viewport shows the representation of its annotation scale. | T1 | 1 |
| CORE-DRAFT-015 | Geometric tolerance frames | Provides feature control frames with symbols, values, modifiers and datum references. | T2 | 3 |

### Styles

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DRAFT-016 | Layers | Provides layers with color, linetype, lineweight, transparency, plot style, on, frozen, locked and plottable states; a frozen layer's entities are excluded from display and selection. | T1 | 1 |
| CORE-DRAFT-017 | Appearance resolution | Resolves appearance from object, block and layer values and viewport overrides in the documented order; the resolved source of each value is queryable. | T1 | 1 |
| CORE-DRAFT-018 | Linetypes | Provides linetypes with dashes, gaps, dots, embedded text and shapes; global and per-object scale; patterns continuous across polyline vertices. | T1 | 1 |
| CORE-DRAFT-019 | Text, dimension, leader and table styles | Provides named styles whose changes update every object that uses them; deleting a used style is blocked. | T1 | 1 |
| CORE-DRAFT-020 | Hatch pattern library | Ships an Arc hatch-pattern library and reads and writes the documented pattern file format. | T1 | 1 |
| CORE-DRAFT-021 | Purge of unused styles | Lists styles, layers, blocks and patterns without references and removes selected ones. | T2 | 1 |

### Blocks and references

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DRAFT-022 | Block definitions and references | Provides blocks with base point, unit and attribute definitions, and references with position, per-axis scale, rotation and attribute values; editing a definition updates all references. | T1 | 1 |
| CORE-DRAFT-023 | Nesting limits | Bounds nesting depth and rejects cyclic definitions at commit with the cycle listed. | T1 | 1 |
| CORE-DRAFT-024 | Reference arrays | Provides rectangular, polar and path arrays of block references with editable parameters. | T2 | 1 |
| CORE-DRAFT-025 | External drawing references | Places linked drawings with transform and clip boundary, displays and snaps to their content read-only, and binds them into blocks on command. | T1 | 1 |
| CORE-DRAFT-026 | Underlays and images | Places raster images and PDF underlays with clip, transparency and fade, referencing content through assets or links. | T2 | 2 |

### Layouts and output

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DRAFT-027 | Layouts | Provides layouts with paper size, orientation, printable area, origin and plot settings. | T1 | 1 |
| CORE-DRAFT-028 | Sheet viewports | Provides viewports with scale, center, rotation, rectangular or arbitrary clip, lock, annotation scale and per-viewport layer visibility and overrides. | T1 | 1 |
| CORE-DRAFT-029 | Display lists | Builds display lists of exact primitives tagged with source identifiers per changed object through the dependency graph. | T1 | 1 |
| CORE-DRAFT-030 | Plot style tables | Applies color-mapped and named plot style tables to produce pen color, lineweight, screening, end and join styles. | T1 | 1 |
| CORE-DRAFT-031 | Plot streams | Produces a page-ordered stream of pen-resolved primitives (exact curves, fills, glyph runs, images, clips, optional-content groups by layer) that the PDF and SVG writers in `arc-io-pdf` consume, identical for identical input. | T1 | 1 |
| CORE-DRAFT-032 | Raster output | Writes PNG and TIFF at a requested DPI with anti-aliasing, matching the viewport's linetype and hatch rendering. | T1 | 1 |
| CORE-DRAFT-033 | Batch plotting | Plots selected layouts of one or more documents headless to PDF, SVG or raster through a command usable from the CLI. | T2 | 1 |

### Text and fonts

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DRAFT-034 | Text layout | Lays out text with shaping, bidirectional runs, line breaking and justification identically for viewport, plot and export. | T1 | 1 |
| CORE-DRAFT-035 | Single-stroke fonts | Reads the documented Arc stroke-font format and ships original stroke fonts covering Latin including Turkish, Greek, Cyrillic and drafting symbols. | T1 | 1 |
| CORE-DRAFT-036 | Font providers and substitution | Lets products register font sources and resolves unavailable font names through per-document and per-user substitution tables, listing substitutions. | T1 | 1 |
