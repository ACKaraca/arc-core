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

## Model core as implemented

The first part of the crate implements CORE-DRAFT-001, 002, 005, 009, 016, 017, 018, 022 and
023. Splines (part of CORE-DRAFT-002) wait for the NURBS curves of `arc-geom`.

### Object types

`register_types` adds the types below to a `TypeRegistry`, each at schema version 1.0.

| Type key | Fields |
|---|---|
| `core.draft.space` | `name`; `kind` (`model`, `layout`); `unit` |
| `core.draft.layer` | `name`; `color` (default opaque white); `linetype` (absent: continuous); `lineweight` in millimeters (absent: the default lineweight); `transparency` (0 to 1); `plot_style`; `on`; `frozen`; `locked`; `plottable`; `description` |
| `core.draft.linetype` | `name`; `description`; `elements`: up to 256 records of `kind` (`dash`, `gap`, `dot`, `text`, `shape`), `length`, `content`, `style`, `scale`, `rotation`, `offset` |
| `core.draft.block` | `name`; `base_point`; `unit`; `description`; `attributes`: up to 1024 records of `tag`, `prompt`, `default`, `constant`, `invisible`, `preset`, `multiline`, `position` |
| `core.draft.point` | common fields; `position` |
| `core.draft.line`, `core.draft.ray`, `core.draft.construction_line`, `core.draft.circle`, `core.draft.arc`, `core.draft.ellipse`, `core.draft.elliptical_arc` | common fields; `geometry`: a segment, ray, line, circle, arc, ellipse or elliptical arc in the `arc.geom` encoding, version 1 |
| `core.draft.polyline` | common fields; `geometry`: a polyline with bulges and per-vertex widths; `continuous_pattern` |
| `core.draft.block_ref` | common fields; `block`; `position`; `scale` (per axis); `rotation` in radians; `attributes`: a map from tag to value |

The common fields of every entity, in order: `layer`; `color_by` and `color`; `linetype_by` and
`linetype`; `linetype_scale` (positive, default 1); `lineweight_by` and `lineweight`;
`transparency_by` and `transparency`; `plot_style` (absent: the layer's); `visible`;
`draw_order`; `elevation`; `thickness`; `extrusion` (default `(0, 0, 1)`). A `*_by` field is
`layer` (default), `block` or `object`.

Rules:

- **Units (CORE-DRAFT-001).** A unit is `unitless`, `millimeter`, `centimeter`, `meter`,
  `kilometer`, `inch`, `foot`, `yard`, `mile` or `us_survey_foot`, each the `arc-base` unit of
  that name. A layout must not be `unitless`; a new model space defaults to `unitless` and a new
  layout to `millimeter`. `SpaceUnit::factor_to` is the explicit conversion between two units; it
  is 1 when either is `unitless`. Nothing converts implicitly.
- **Containment.** An entity belongs to a space or block definition through its header owner;
  deleting the container deletes its entities.
- **References.** The `layer` and `linetype` fields of entities and layers, and the `block` field
  of a block reference, are associative references whose deletion rule is `block`: a layer,
  linetype or block definition in use cannot be deleted.
- **Type validators.** The geometry of a curve entity must decode, under the default limits, as
  the curve kind of its type (`draft.geometry.invalid`); the extrusion direction is not zero
  (`draft.extrusion.zero`); no scale factor of a block reference is zero
  (`draft.block.scale_zero`); a linetype pattern has lengths that are finite and not negative
  with a positive sum (`draft.linetype.pattern_empty`).
- **Elevation and extrusion (CORE-DRAFT-009).** `elevation`, `thickness` and `extrusion` are
  stored and returned unchanged; no operation of this part interprets them.

### Document validators

`validate::validators(max_nesting)` returns the two commit validators; `install` adds them to a
document with the default bound of 64. A product adds them to every document it opens.

- **Names.** A layer, linetype or block definition a transaction creates or modifies must not
  have the exact name, byte for byte, of another object of its type (`draft.name.taken`).
- **Nesting (CORE-DRAFT-023).** A block definition contains the block references it owns. The
  depth of a chain of references is the number of references in it. For each block reference a
  transaction creates, modifies or recreates, the validator searches the definitions below its
  target and the references above its container, expanding each definition once with an explicit
  stack. A definition reached from itself fails with `draft.block.cycle`; the detail `cycle` lists
  the definitions of the cycle in order, the first one repeated at the end, as identifiers
  separated by ` -> `. A chain through the reference deeper than the bound fails with
  `draft.block.nesting_too_deep` (argument `limit`). The search stops at the bound, so a cycle
  longer than the bound reports as too deep. A definition that nothing inserts counts from its
  own references.

### Draw order (CORE-DRAFT-005)

Entities of one container draw in ascending order of (`draw_order`, identifier). A move takes the
moved entities in their current relative order and places them at the front, at the back, or
directly above or below a reference entity. They receive keys spread evenly over the open interval
between their new neighbors; beyond an end of the order the interval extends 2^20 per moved
entity. Only the moved entities change, unless the interval has too few free keys or leaves the
`i64` range: then every entity of the container is renumbered 2^20, 2·2^20, … in the new order. A
new entity is placed at the front. The reference must not be among the moved entities
(`draft.order.reference_moved`), and all entities must share one container
(`draft.order.container_mismatch`).

### Appearance resolution (CORE-DRAFT-017)

`appearance::resolve` takes the chain of levels from the entity outward (the entity, the block
reference inserting its definition, the reference inserting that one, and so on), each with its
settings and layer, and the viewport layer overrides. Color, linetype, lineweight and
transparency resolve independently:

1. `object`: the level's own value; an absent value gives the default (opaque white, continuous,
   the default lineweight, transparency 0).
2. `layer`: the viewport override of the level's layer when one is given for the property,
   otherwise the layer's value; a level without a layer gives the default.
3. `block`: the same property of the next level outward; past the outermost level, the default.

Every resolved value carries its source: the entity, the block reference at a given level, a
layer, a viewport override of a layer, or the default. `resolve_in` reads the chain from a
snapshot.

### Linetype generation (CORE-DRAFT-018)

`linetype::dashes` maps a curve, a pattern and options (global scale, object scale, continuity,
output bound) to drawn pieces, each an arc-length interval with the curve parameters of its ends.

- The pattern starts at the start of the curve with its first element and repeats; the element
  at the end of the curve is cut. Element lengths are multiplied by the product of both scales,
  which must be a finite positive number (`draft.linetype.scale_invalid`).
- A dash draws its length. A gap, dot, text or shape element advances by its length without
  drawing; a dot also draws a piece of length zero. Text and shape elements are stored as data
  and do not draw in this part.
- A polyline restarts the pattern at every vertex unless `continuous` is set (the entity field
  `continuous_pattern`); then one pattern runs over the whole polyline and pieces may span
  vertices.
- The walk visits at most `(⌈run length / scaled period⌉ + 1) · element count` elements per run
  (the whole curve, or one polyline segment). When the sum over the runs exceeds the bound
  (default 100,000), or positions stop advancing in `f64`, the result is `Continuous`: the curve
  is drawn solid. Rays and construction lines are always `Continuous`.

### Commands

Every command is undoable with the permission class `edit`. Modify commands take `objects`,
`field` and `value`: the field's value in the canonical JSON form of `arc-doc`, where an enum also
accepts its variant name, a color also accepts `[r, g, b, a]`, a geometry is an `arc-geom` curve
in its JSON form and `null` makes an optional field absent; each accepts only objects of its
family.

| Command | Effect |
|---|---|
| `core.draft.space.create` | Creates a space with `name`, `kind` and optional `unit` |
| `core.draft.space.modify` | Sets a field of spaces |
| `core.draft.entity.create` | Creates a point (`{"kind": "point", "position": [x, y]}`) or a curve entity, chosen by the curve's kind, in `owner`, optionally on `layer` |
| `core.draft.entity.modify` | Sets a field of entities, block references included |
| `core.draft.entity.delete` | Deletes entities |
| `core.draft.layer.create` | Creates a layer with `name` and optional `color`, `linetype` and `lineweight` |
| `core.draft.layer.modify` | Sets a field of layers |
| `core.draft.layer.delete` | Deletes layers; refused while an entity uses one |
| `core.draft.linetype.load` | Creates a linetype from `name`, optional `description` and `elements` |
| `core.draft.linetype.modify` | Sets a field of linetypes |
| `core.draft.block.define` | Creates a block definition and moves the listed entities into it |
| `core.draft.block.modify` | Sets a field of block definitions |
| `core.draft.block.insert` | Creates a block reference; attributes not given take their definition's default, and an unknown tag fails with `draft.block.attribute_unknown` |
| `core.draft.order.front`, `core.draft.order.back` | Moves entities to the front or back |
| `core.draft.order.above`, `core.draft.order.below` | Moves entities directly above or below `reference` |

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `draft.geometry.invalid` | `invalid_input` | A geometry field does not hold the curve kind of its type |
| `draft.extrusion.zero` | `invalid_input` | The extrusion direction is the zero vector |
| `draft.space.layout_unitless` | `invalid_input` | A layout declares the unit `unitless` |
| `draft.linetype.pattern_empty` | `invalid_input` | A pattern length is negative or not finite, or the lengths do not add up to a positive number |
| `draft.linetype.scale_invalid` | `invalid_input` | A linetype scale or the product of the scales is not a finite positive number |
| `draft.block.scale_zero` | `invalid_input` | A block reference has a zero scale factor |
| `draft.block.cycle` | `invalid_input` | Block definitions contain each other; the detail `cycle` lists them |
| `draft.block.nesting_too_deep` | `limit_exceeded` | A chain of nested block references is deeper than the bound |
| `draft.block.attribute_unknown` | `invalid_input` | A block reference names an attribute its definition does not define |
| `draft.name.taken` | `conflict` | Another layer, linetype or block definition has the name |
| `draft.object.wrong_type` | `invalid_input` | An object is not of a type the command accepts |
| `draft.field.invalid` | `invalid_input` | A JSON value does not fit the field, or the field does not exist |
| `draft.order.reference_moved` | `invalid_input` | The reference of a draw-order move is one of the moved entities |
| `draft.order.container_mismatch` | `invalid_input` | The entities of a draw-order move are not in one container |
