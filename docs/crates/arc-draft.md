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

Drawing commands and their prompt machines, dynamic block behavior, sheet set management, DWG mapping, DXF mapping beyond the shared one of
[arc-draft-dxf.md](arc-draft-dxf.md) (DraftBuilder), discipline symbols and toolset objects, title
block content, and domain annotation objects (tags bound to BIM elements, station labels).

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
| Third-party | `parley` (outline text, feature `outline-text`), `tiny-skia` (raster output), `lyon` (fill tessellation for raster output) |

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
023. Splines, the last kind of CORE-DRAFT-002, are stored as NURBS curves (see "Splines" below).

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
| `core.draft.spline` | common fields; `geometry`: a NURBS curve in the `arc.geom` encoding, version 2; `fit_points`: a list of points (default empty) |
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

### Splines

A spline is an entity whose `geometry` is an `arc-geom` NURBS curve (CORE-GEOM-008). The field
descriptor names the encoding `arc.geom` at version 2, the version `GeomCodec::encoding` reports
for a NURBS curve ([ADR-0013](../adr/0013-geometry-encoding.md), amendment); the type validator
requires the bytes to decode as a NURBS curve (`draft.geometry.invalid`).

- **Control points.** `core.draft.spline.create` with `control_points` builds the curve of
  `degree` on the clamped uniform knot vector over [0, 1] (`Nurbs2::from_control_points`); the
  weights default to 1. `fit_points` is empty.
- **Fit points.** With `fit_points` the curve is `Nurbs2::interpolate` of the points at the
  degree, under the default tolerance, and `fit_points` keeps the points as given. The fit points
  are data: they are not read back to rebuild the curve, and editing the field alone leaves the
  geometry as it is. `core.draft.spline.set` replaces both fields together.
- **Descriptions.** Exactly one of `control_points` and `fit_points` is given, and `weights` only
  with control points (`draft.field.invalid`); the construction errors of `arc-geom` are passed
  on (degree not below the number of points, coincident consecutive fit points, non-finite
  numbers). Point counts are bounded by the invocation's `array_length`.
- **Display.** A spline contributes one `Stroke` with the exact curve and, for a linetype,
  dashes by arc length (`linetype::dashes`), like every curve entity.

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
| `core.draft.entity.create` | Creates a point (`{"kind": "point", "position": [x, y]}`) or a curve entity, chosen by the curve's kind (a NURBS curve gives a spline with no fit points), in `owner`, optionally on `layer` |
| `core.draft.spline.create` | Creates a spline in `owner` from `degree` (default 3) and either `control_points` with optional `weights`, or `fit_points`; optionally on `layer` |
| `core.draft.spline.set` | Replaces the curve and the fit points of splines from the same description |
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

## Layouts, viewports and display lists as implemented

The second part of the crate implements CORE-DRAFT-004, 019 (the style objects and their update
and deletion rules; text and dimension layout come later), 021, 027, 028 and 029.

### Object types

`register_types` also adds these types, each at schema version 1.0.

| Type key | Fields |
|---|---|
| `core.draft.wipeout` | common fields; `geometry`: a closed polyline; `frame` |
| `core.draft.layout` | `paper_name`; `paper_width`, `paper_height` (positive, in the layout's unit, the paper as named); `orientation` (`portrait`, `landscape`); `margin_left`, `margin_bottom`, `margin_right`, `margin_top` (not negative); `origin`; `plot_scale` (positive, default 1); `plot_style_table`; `plot_area` (`layout`, `extents`, `window`); `window_min`, `window_max` |
| `core.draft.viewport` | `model` (the space shown); `center`; `width`, `height` (positive); `view_center`; `scale` (positive, default 1); `rotation` in radians; `clip` (optional closed polyline); `locked`; `annotation_scale` (positive, default 1; see annotation scaling below); `layer_overrides`: up to 4096 records of `layer`, `frozen`, `color`, `linetype`, `lineweight`, `transparency` |
| `core.draft.text_style` | `name`; `font` (by name); `height` (0: set per text); `width_factor` (positive); `oblique` (at most 85 degrees either way); `backwards`; `upside_down`; `vertical`; `description` |
| `core.draft.dim_style` | `name`; `parent`; `kind` (`all`, `linear`, `angular`, `radial`, `diameter`, `ordinate`, `arc_length`); `text_style`; `text_height`; `text_gap`; `arrow` (`closed_filled`, `closed`, `open`, `dot`, `tick`, `none`); `arrow_size`; `extension_offset`; `extension_beyond`; `unit_format` (`decimal`, `engineering`, `architectural`, `fractional`, `scientific`); `precision` (0 to 8); `measurement_scale`; `overall_scale`; `description` |
| `core.draft.leader_style` | `name`; `text_style`; `text_height`; `arrow`; `arrow_size`; `landing`; `landing_length`; `content` (`text`, `block`, `tolerance`, `none`); `description` |
| `core.draft.table_style` | `name`; `cell_margin`; `title`, `header`, `data`: optional records of `text_style`, `text_height`, `alignment` (`left`, `center`, `right`), `borders`, `format`; `description` |

Rules:

- **Layouts (CORE-DRAFT-027).** A layout's settings are a `core.draft.layout` object owned by a
  space of kind `layout`, at most one per space (`draft.layout.duplicate`); the space created by
  `core.draft.space.create` has none until a command adds them. The sheet is the paper as named
  for `portrait` and turned a quarter for `landscape`. The left and right margins add up to less
  than the sheet's width and the bottom and top margins to less than its height
  (`draft.layout.margins_invalid`). A `plot_area` of `window` needs `window_min` below and left
  of `window_max` (`draft.layout.window_invalid`). The other settings are stored for the plot
  pipeline.
- **Viewports (CORE-DRAFT-028).** A viewport is owned by a layout space and deleted with it. The
  owner of layout settings and viewports must be a space of kind `layout`, and a space of kind
  `model` owns neither (`draft.layout.owner_invalid`). A model point `p` appears on the sheet at
  `center + R(rotation) · k · (p − view_center)` with `k = scale · factor`, where `factor` is
  `SpaceUnit::factor_to` from the unit of the shown space to the unit of the layout: `scale` is
  paper length per model length in one unit, so 1:100 is 0.01 whatever the two units. The clip
  boundary is `clip`, a closed polyline in sheet coordinates (`draft.viewport.clip_invalid`), or
  the rectangle of `width` and `height` about `center`. While `locked` is set,
  `core.draft.viewport.modify` refuses `model`, `view_center`, `scale` and `rotation`
  (`draft.viewport.locked`). In `layer_overrides` the last record of a layer wins; `frozen`
  hides the layer in the viewport and the other values are the overrides of
  `appearance::resolve`. The `layer`, `linetype` and `model` references refuse deletion of their
  targets.
- **Wipeouts (CORE-DRAFT-004).** A wipeout is an entity whose closed polyline boundary
  (`draft.wipeout.boundary_open`) masks what is beneath it in draw order; `frame` also draws the
  boundary.
- **Styles (CORE-DRAFT-019).** `style::style_reference` is the reference type of every field that
  uses a style: associative, with the deletion rule `block`, so deleting a style in use fails
  with `doc.reference.delete_blocked`. The dimension, leader and table styles use text styles
  through it. A dimension style with a `parent` has a kind other than `all`, and its parent has
  the kind `all` and no parent (`draft.style.parent_invalid`). Style names are unique per type
  (`draft.name.taken`). The derived node `core.draft.style.effective` of a style holds its field
  values and the effective data of every style it references; an evaluator of a style user reads
  it, so changing a style marks the derived data of its users stale. No style exists until a
  command creates one; `core.draft.style.defaults` creates a style named `default` of each kind
  that has none.

### Purge (CORE-DRAFT-021)

`purge::unused` lists the layers, linetypes, block definitions and styles that no live object
refers to through a field, read from the reverse index; owning a block's entities is not a use.
`core.draft.purge` deletes the given objects of these types in one undoable transaction. It fails
with `draft.purge.in_use`, deleting nothing, when one of them is in use in the transaction's
state. Purging a block definition deletes its entities. An object used only by an unused object
is listed after that one is purged.

### Display lists (CORE-DRAFT-029)

`register_evaluators` registers the node `core.draft.display` for every entity type and the
style node above. `DisplayOptions` gives the tolerance, the nesting bound (default 64, the
validator's), the most primitives of one list (default the `array_length` of the default limits)
and the dash bound.

- **Primitives.** A `Primitive` is a point, a stroke (a line, ray, construction line, circle, arc,
  ellipse, elliptical arc or polyline with widths, kept exact, with its drawn pieces when they are
  not the whole curve), a filled region or a wipeout mask (closed polyline). It carries its
  resolved color, linetype, lineweight and transparency with their sources, the drawing
  entity's linetype scale and pattern continuity, the layers of its chain, the draw-order key of
  the outermost entity, the source `ObjectId` (the entity in the list's container) and the
  sub-object key: the path of nested entities below the source, outermost first, and the part (0
  the shape, 1 the frame of a wipeout). Dashes are generated: the drawn pieces are exact
  sub-curves (a dot is a point), so consumers draw them without a pattern.
- **Entity lists.** The list of an entity reads the entity, its layer and its linetype. An
  invisible entity has an empty list. Appearance resolves as in `appearance::resolve` with one
  level; a value set to `block` stays open in a block definition and becomes the default in a
  space. Dashes are generated in the entity's container with the entity's `linetype_scale` and a
  global scale of 1.
- **Block references.** The list of a block reference reads the reference, its layer and
  linetype, the definition's `base_point` and `unit`, the unit of its container, the set of the
  definition's entities and their lists. It maps them with translation to the position, rotation,
  the per-axis scale times the unit factor of the definition to the container, and translation by
  the negated base point, in the definition's draw order. An open value takes the reference's own
  resolution; a value of the reference itself gets the source `Block { level }`, the level being
  the length of the path. A stroke whose linetype opens to the reference gets its dashes then, in
  the definition's coordinates. A nesting deeper than the bound fails with
  `draft.block.nesting_too_deep`, more primitives than the bound with `draft.display.too_large`,
  and a polyline with arcs or widths under a non-similar map with `geom.transform.not_similarity`.
- **Regeneration.** Editing an entity regenerates its list and, through the nodes that read it,
  the lists of the references inserting its definition. Editing a layer or linetype regenerates
  the lists that read it; a list whose result does not change leaves its readers clean. Editing a
  block definition's fields or its set of entities regenerates the lists of its references.
  `display::entity_list` evaluates the same rules from a snapshot without the graph.
- **Space lists.** `space_list` takes the lists of the space's entities in draw order, from the
  graph or by direct evaluation (`Lists`), and leaves out every primitive with an off or frozen
  layer in its chain. Then, in draw order, each wipeout mask cuts away what the curves and points
  before it have inside its boundary; fills and masks before it stay and are painted over. An
  entity whose list fails, or whose primitives cannot be cut, is left out and listed in
  `failed`.
- **Viewport lists.** `viewport_list` does the same for the shown space, also leaving out the
  layers frozen in the viewport. Before the masks it applies the viewport's overrides: a value
  whose source is a layer with an override takes the override with the source
  `ViewportOverride`, which is what `appearance::resolve` gives with the overrides; a linetype
  override generates the dashes again in model coordinates. It maps the primitives to the sheet
  and cuts curves and points to the clip boundary; fills and masks that reach the boundary are
  kept whole and the list's `clip` tells the consumer to clip them. Lineweights stay in
  millimeters; polyline widths scale with the map.
- **Cutting.** A curve is split where it meets a boundary (`arc-geom` intersections) and each
  stretch is classified by its middle point; stretches on the boundary are kept, consecutive kept
  stretches join, and an unbounded stretch is outside. Pieces too small to construct are dropped.
  A cut polyline keeps its widths; the width beyond the cut is the consumer's to clip.

### Commands

| Command | Effect |
|---|---|
| `core.draft.layout.create` | Creates a layout space (`name`, optional `unit`, default `millimeter`) and its settings (`paper_width`, `paper_height`, optional `paper_name` and `orientation`); returns `space` and `layout` |
| `core.draft.layout.modify` | Sets a field of layout settings |
| `core.draft.viewport.create` | Creates a viewport in `layout` showing `model` with `center`, `width`, `height` and optional `view_center`, `scale`, `rotation` |
| `core.draft.viewport.modify` | Sets a field of viewports, `layer_overrides` as a list of records; refuses the view fields of a locked viewport |
| `core.draft.viewport.delete` | Deletes viewports |
| `core.draft.wipeout.create` | Creates a wipeout in `owner` from a closed polyline `boundary`, with optional `frame` and `layer` |
| `core.draft.style.create` | Creates a style of `kind` (`text`, `dimension`, `leader`, `table`) named `name` |
| `core.draft.style.modify` | Sets a field of styles |
| `core.draft.style.delete` | Deletes styles; refused while an object uses one |
| `core.draft.style.defaults` | Creates a style named `default` of each kind that has none; returns `created` |
| `core.draft.purge` | Deletes unused layers, linetypes, block definitions and styles in one transaction |

## Text and fonts as implemented

The third part of the crate implements CORE-DRAFT-006, 034, 035 and 036. The stroke-font format
is specified in [stroke-font-format.md](../architecture/stroke-font-format.md)
([ADR-0015](../adr/0015-stroke-font-format.md)).

### Object types

`register_types` also adds these types, each at schema version 1.0.

| Type key | Fields |
|---|---|
| `core.draft.text` | common fields; `content` (one line, at most 4096 bytes); `position`; `rotation` in radians; `style` (a text style); `height` (0: the style's); `halign` (`left`, `center`, `right`); `valign` (`baseline`, `bottom`, `middle`, `top`) |
| `core.draft.rich_text` | common fields; `paragraphs`; `position`; `rotation`; `style`; `height`; `width` (0: no wrapping); `attachment` (`top_left` … `bottom_right`, nine points); `columns` (1 to 64); `column_gutter`; `column_height` (0: one column) |
| `core.draft.font_table` | `entries`: up to 4096 records of `requested`, `substitute` |

A paragraph is a record of `alignment` (`left`, `center`, `right`, `justify`), `indent_first`,
`indent`, `space_before`, `space_after` (not negative), `line_spacing` (a positive factor),
`tabs` (positions from the left edge) and `runs`. A run is a record of `text`, `font`, `height`,
`color`, `bold`, `italic`, `underline`, `overline`, `strike`, `stack` (`none`, `horizontal`,
`diagonal`) and `denominator`. The stored structure is the only form of rich text; there is no
inline formatting-code language.

Rules (`text::validate`, applied by the type validators under the default limits and by the
commands under the invocation's limits):

- The paragraphs of a text, the runs of a paragraph and the tabs of a paragraph number at most
  `array_length`, and the bytes of all run texts and denominators at most `string_bytes`
  (`draft.text.too_large`, argument `limit`).
- Text holds no control character other than tab (`draft.text.content_invalid`); sizes are finite
  and not negative, line spacing and run heights positive.
- A run with a `stack` other than `none` has a `denominator`, and only such a run has one
  (`draft.text.stack_invalid`).
- Tab stops are finite, positive and strictly ascending (`draft.text.tabs_invalid`).
- `text::paragraphs_from_plain` makes one paragraph per line of plain text (`\n`, `\r\n` and `\r`
  end a line), with one run, or none for an empty line.

The text height is the text's `height` when positive, else its style's when positive, else 2.5.
The `style` field is a style reference (`style::style_reference`), so a text style in use cannot
be deleted. A text style's `vertical` flag is stored and not applied.

### Fonts and substitution (CORE-DRAFT-036)

- A `FontProvider` gives stroke fonts by name and outline font bytes (with a face index) by
  family name. `FontRegistry` is the provider of this crate; `FontRegistry::with_builtin` holds
  the shipped font *Arc Stroke*. Products register further fonts in it or implement the trait.
  Layout reads fonts only from the provider, never from the system.
- `Fonts` bundles the provider, the per-user substitution table (data the product supplies) and
  the name of the default stroke font (*Arc Stroke*). `DisplayOptions::fonts` carries it.
- The per-document table is the `core.draft.font_table` object with the fixed identifier
  `fonts::font_table_id()` (`00000000-0000-7000-8000-000000000f01`), created and changed by
  `core.draft.font.substitute`. A fixed identifier lets every text's display list depend on the
  table, and on its creation, without a reference from each text. Objects of the type with
  another identifier are ignored.
- `fonts::resolve` maps a requested name: absent or blank gives the default stroke font with no
  substitution; a name the provider has is used as is (a stroke font first, then outline bytes
  when the feature `outline-text` is enabled; without it outline bytes count as unavailable);
  otherwise the document table's substitute, if the provider has it; otherwise the user table's,
  if the provider has it; otherwise the default stroke font. Each step after the first is a
  `Substitution` (requested, used, and `Document`, `User` or `Default`). Names compare after
  trimming and Unicode lowercasing. A provider without the default stroke font fails with
  `draft.font.default_missing` when it is needed.
- `fonts::substitutions` lists the substitutions a document needs: one per distinct font name of
  its text styles and rich text runs. `fonts::validator(fonts)` is a document validator that
  reports each as a warning `draft.font.substituted` (arguments `requested`, `used`) on every
  commit, so the document's diagnostics list holds those of the current revision; it never
  rejects a commit.

### Layout (CORE-DRAFT-034)

`text_layout::layout` takes the paragraphs, the text height, the wrapping width, the anchor, the
columns, the resolved style (base font, width factor, slant) and the fonts, and returns a
`TextLayout` in text space (x along the baseline, y up, drawing units, origin at the anchor): glyph
runs, line boxes, the bounds (the union of the line boxes) and every substitution made, in order
of first use. The display lists, plot and export use this one function; `display::text_layout`
evaluates it for an entity from a snapshot.

- **Engine.** The base font decides the engine: a stroke font uses the stroke engine below; an
  outline font (feature `outline-text`) uses `parley`. A run naming a font of the other kind uses
  the base font, listed with `SubstitutedBy::Base`.
- **Scale.** A stroke run of height h draws font units at h / `cap-height` vertically and that
  times the width factor horizontally. The slant is the tangent of the style's oblique angle
  plus 0.2126 for italic runs; x grows by the slant times the height above the baseline. Bold
  stroke runs draw every glyph a second time 0.05 h to the right.
- **Advances.** A character advances by its glyph's advance plus the kerning with the next
  character of the same run. A character the font lacks draws the font's missing glyph (counted
  in `missing`) or, without one, advances half an em.
- **Tabs.** A tab moves to the first tab stop more than the linear tolerance beyond the current
  position, past the stops to the next multiple of four text heights.
- **Line breaking.** Lines break only at spaces. Words are placed greedily: a word that would end
  beyond the width (plus the linear tolerance) starts a new line, unless it is the first word of
  its line; so no line exceeds the width unless a single word does. Spaces at a break are
  dropped. The first line of a paragraph starts at `indent_first`, the others at `indent`. A
  width of 0 does not wrap.
- **Alignment.** The alignment region is the width, or without one the widest line of the text.
  `center` and `right` shift a line by half or all of the region's free space. `justify` spreads
  the free space evenly over the gaps between words after the line's last tab, on every line
  but the last of the paragraph, so those lines end at the right margin; a line without such a
  gap stays left-aligned.
- **Lines.** A line's height is the largest run height on it (an empty line: the first run's or
  the text's). The first baseline is one line height below the top; each line advances by its
  height times `line-spacing` / `cap-height` of the base font times the paragraph's
  `line_spacing`. `space_before` and `space_after` add to the advance. A line box reaches the
  line height above its baseline and the height times −`descender` / `cap-height` below.
- **Columns.** With more than one column, a positive `column_height` and a width, a line whose
  box would reach below `column_height` moves to the top of the next column, unless it is the
  first line of its column or the last column is reached; column k starts at k (width + gutter).
- **Stacked fractions.** A stacked run is one unbreakable box whose parts are drawn at 0.6 of
  the run height. `horizontal`: as wide as the wider part plus 0.2 h, a bar at 0.5 h, the
  numerator centered above it with its baseline at 0.65 h, the denominator centered below with
  its baseline at −0.25 h. `diagonal`: the numerator with its baseline at 0.4 h, a slash 0.4 h
  wide from −0.1 h to 1.1 h, then the denominator on the baseline.
- **Decorations.** Underline, overline and strike-through are segments over the run's extent on
  the line at −0.15 h, 1.2 h and 0.5 h above the baseline.
- **Anchor.** The block spans from x = 0 to the alignment region (or the columns used) and from
  y = 0 to the lowest line box. `left`, `center`, `right` and `top`, `middle`, `bottom` take its
  edges or middle; `baseline` takes the first baseline.
- **Output.** A stroke run gives exact curves: polylines, circular arcs, and elliptical arcs
  under a width factor; slanted arcs are mapped by the `arc-geom` transform. An outline run gives
  the provider family name, face index, size and glyph identifiers with pen positions.
- **Determinism.** Fonts come only from the provider and the stroke engine uses no hash order or
  system state. Upright text with a width factor of 1 uses only the four arithmetic operations,
  so its output is identical on every platform; slanted arcs depend on the platform's
  trigonometric functions through `arc-geom`.
- **Bounds.** A height, width, width factor, slant or column value that is not finite or out of
  range fails with `draft.text.content_invalid` (detail `field`); more curves or glyphs than the
  bound with `draft.display.too_large`.

The outline engine (feature `outline-text`, off by default) builds for each layout a `parley`
font collection holding only the provider's fonts the text names, registered under their family
names, and lays out each paragraph with `parley`: shaping, bidirectional reordering, line
breaking at the width and the paragraph alignment. A run's height is its em size. Bold runs ask
for weight 700 and italic runs for the italic style of the family. Decorations are stroke
segments. Stacked fractions are shaped as `numerator/denominator`, and columns and the
paragraph's tab stops are not applied. Bytes the collection cannot read fail with
`draft.font.outline_invalid`.

Stroke fonts have no right-to-left glyphs; the stroke engine lays out in logical order from left
to right and does not reorder bidirectional text.

### Display lists

Text entities contribute to their `core.draft.display` node. A stroke run gives one `Stroke`
primitive per curve, mapped from text space by translation to `position`, rotation and the mirror
flags of the style (`backwards` mirrors x, `upside_down` mirrors y); an outline run gives one
`Glyphs` primitive with the family, face index, size, glyph positions and that map as
`placement`. Text strokes are continuous whatever the linetype (linetype source `Object`, value
none). A run with its own color takes it with the source `Object`; the other values resolve as
for every entity. Glyph primitives are not cut by clips or wipeouts; the consumer clips them like
fills. The node reads the entity, its text style's fields and the font table, so a change of the
text, its style or the document's substitution table regenerates exactly the lists of the texts
concerned. A change of the provider or the user table takes new `DisplayOptions` and evaluators.

### Commands

| Command | Effect |
|---|---|
| `core.draft.text.create` | Creates a single-line text in `owner` with `content`, `position` and optional `rotation`, `style`, `height`, `halign`, `valign`, `layer` |
| `core.draft.rich_text.create` | Creates a rich text from `paragraphs` or plain `text` (not both) at `position`, with optional `rotation`, `style`, `height`, `width`, `attachment`, `columns`, `column_gutter`, `column_height`, `layer` |
| `core.draft.rich_text.set` | Replaces the paragraphs of rich texts with `paragraphs` or plain `text` |
| `core.draft.font.substitute` | Sets the document's substitute for `requested` to `substitute`, or removes the entry without `substitute`; creates the table on first use |

Text fields also change through `core.draft.entity.modify`.

## Dimensions, leaders and annotation scaling as implemented

The fourth part of the crate implements CORE-DRAFT-010, 011, 012, 013 and 014.

### Object types

`register_types` also adds these types, each at schema version 1.0. Dimensions, leaders and
multi-leaders are entities.

| Type key | Fields |
|---|---|
| `core.draft.dimension` | common fields; `kind` (`linear`, `aligned`, `angular_lines`, `angular_points`, `angular_arc`, `radial`, `diameter`, `arc_length`, `ordinate_x`, `ordinate_y`); `points`: definition point records; `line_position`; `rotation` in radians; `text_override`; `text_position`; `style` (a dimension style); `annotative`; `scales` |
| `core.draft.dim_chain` | `kind` (`baseline`, `continued`); `members`: up to 1024 dimensions; `spacing` (not negative, default 10) |
| `core.draft.leader` | common fields; `points` (up to 4096, the arrow tip first); `spline` (stored; the leader is drawn through its points); `arrow` (absent: the style's); `style` (a leader style); `annotative`; `scales` |
| `core.draft.multileader` | common fields; `leaders`: lists of points, each from its arrow tip; `position`; `direction` (`right`, `left`); `content` (`text`, `block`, `tolerance`, `none`); `text`; `block`; `block_scale` (positive); `tolerance`: up to 64 rows of cell texts; `attachment` (`top`, `middle`, `bottom`); `style`; `annotative`; `scales` |

A definition point record holds `position`, `object` (any entity), `path` (records of one
`member` each, at most 64), `key` (`none`, `start`, `end`, `mid`, `center`, `quadrant`,
`parameter`), `quadrant` (0 to 3) and `parameter`. A representation record holds `scale`
(positive), `position` and `text_position` (both optional). The dimension style gains
`rounding` (an increment, 0 for none), `suppress_leading_zeros`, `suppress_trailing_zeros`,
`decimal_separator` (its first character; default `.`), `unit_suffix`, `angular_precision` (0 to
8), `text_vertical` (`centered`, `above`), `tolerance` (`none`, `symmetric`, `deviation`,
`limits`), `tolerance_upper` and `tolerance_lower`; the leader style gains `text_gap`. Alternate
units are not provided.

Type validators: a dimension has exactly the number of definition points its kind needs
(`draft.dimension.points_invalid`): two for linear, aligned, radial, diameter and ordinate
dimensions, three for `angular_points` (vertex, first, second), `angular_arc` and `arc_length`
(center, start, end), four for `angular_lines` (two points on each line). A text override holds
no control character (`draft.text.content_invalid`). A leader has at least two points and every
leader line of a multi-leader at least one (`draft.leader.points_invalid`). No annotation scale is
listed twice (`draft.annotation.scale_invalid`).

### Measurement and formatting (CORE-DRAFT-010)

`dimension::measure` gives the measured value in drawing units, or radians for an angle, from the
resolved points:

- `linear`: the distance of the two points along the direction `rotation` (0 horizontal, π/2
  vertical, any other value rotated); `aligned`: their distance.
- `angular_points`: of the two sectors between the rays from the vertex, the one that contains
  `line_position` (so a reflex angle is measured from the other side); `angular_lines`: of the four
  sectors of the two lines about their intersection, the one that contains `line_position`;
  `angular_arc`: the counterclockwise angle from start to end about the center (a full turn when
  they coincide).
- `radial` and `diameter`: once and twice the distance from the center to the point on the curve.
- `arc_length`: the radius (center to start) times the `angular_arc` angle.
- `ordinate_x` and `ordinate_y`: the signed x or y distance of the feature from the datum.

Coincident points that leave a direction undefined, lines within the angular tolerance of
parallel, a zero radius and a dimension arc through the vertex fail with
`draft.dimension.degenerate`.

The written value is the measured value times the style's `measurement_scale` (lengths only),
formatted by `format::format_length` or `format::format_angle`:

- The value is rounded to the nearest multiple of `rounding` when it is positive, then written
  with `precision` decimals. `suppress_trailing_zeros` drops zeros at the end of the fraction and
  a separator left alone; `suppress_leading_zeros` drops the zero before the separator of a value
  below 1 (`.5`, `-.5`). A value that rounds to zero has no sign. The separator is
  `decimal_separator`. With `unit_suffix`, the symbol of the container's unit follows after a
  space.
- `fractional` writes a whole number and a reduced fraction with the denominator 2^precision
  (`2 3/8`); `scientific` writes a mantissa with `precision` decimals and a signed two-digit
  exponent (`1.25E+02`); `engineering` and `architectural` convert the value to inches (a
  unitless container counts as inches) and write feet and inches, the inches as decimals or a
  fraction (`5'-2.38"`, `5'-2 3/8"`), without a unit suffix.
- Angles are written in decimal degrees with `angular_precision` decimals, followed by `°`.
- The text is `text_override`, or `{}` without one. Its first `{}` stands for the value and its
  tolerance; other text is written as is, and a template without `{}` has no value. A `symmetric`
  tolerance writes ` ±upper` after the value; `deviation` writes the value and a stacked fraction
  `+upper` over `-lower`; `limits` writes value + upper over value − lower as a stacked fraction
  in place of the value. Tolerances of angles are in degrees.

### Dimension geometry

`dimension::geometry` is a pure function of a definition, the sizes (text height, gap, arrow,
arrow size, extension offset and overshoot, text placement) and the text width. It returns
strokes and fills, each with its `annotation::Part`, and the center and rotation of the text
block. Text rotations are reduced to (−π/2, π/2] so text reads from left to right or from
bottom to top.

- **Linear and aligned.** The dimension line runs along the measured direction through
  `line_position`; its ends are the feet of the two points on it. Each extension line runs from
  `extension_offset` past its point to `extension_beyond` past its end of the dimension line.
- **Fit.** The text and arrows fit when the dimension line is at least the text width plus two
  gaps plus two arrow sizes long (no arrow sizes for `none`). Then the arrows point outward to
  the tips from inside and the text is centered on the line, which is broken around it (text
  width plus a gap on each side), or lies above it by the gap plus half the text height with
  `text_vertical` `above`. Otherwise the arrows point inward from outside, the line extends two
  arrow sizes past each tip, and the text lies beyond the second tip.
- **Moved text.** With `text_position` (or a representation's), the text is centered there and
  a line runs from the middle of the dimension line toward it, ending half the text width plus
  the gap before it.
- **Angular and arc length.** The dimension arc is centered on the vertex through
  `line_position`. Extension lines run radially from the definition points; for two lines, from
  the nearer end of a line only when the arc lies beyond that line's extent. The fit rule uses
  the arc's length; the arrows are tangent at its ends; the text is centered at its middle, the
  arc broken around it.
- **Radial.** The line runs from the center to the point on the curve with the arrow at that
  point. The text lies on that ray at the projection of `line_position`: inside the curve the
  line is broken around it; outside, the arrow points in and a line runs from the curve toward
  the text.
- **Diameter.** The line runs between the point on the curve and its reflection through the
  center, with arrows at both, as a linear dimension line with the text at the center.
- **Ordinate.** A leader runs from `extension_offset` past the feature perpendicular to the
  measured axis to the height (or width) of `line_position`, then to `line_position`; the text
  lies beyond its end. Ordinate dimensions have no arrows and ignore `rotation`.
- **Arrowheads** (`annotation::arrowhead`, size s along the pointing direction): `closed_filled`
  a filled triangle from the tip to two corners s back and s/6 aside; `closed` its outline;
  `open` its two sides; `dot` a filled disc of radius s/4 centered on the tip; `tick` a stroke of
  length s through the tip at 45° counterclockwise from the direction; `none` nothing.

### Associativity (CORE-DRAFT-011)

- A definition point with the key `none` is free: its `position`. Otherwise it references
  `object`, an entity in the dimension's container, and through `path` the nested entities below
  it when it is a block reference: each entry is a member of the definition the one before
  inserts, and the point is mapped by the maps of the references on the way.
- Keys: `start` and `end` of a curve (`start` is also a point entity's position and a block
  reference's insertion point; a point entity also gives its position for `end`, `mid` and
  `center`); `mid` the point at the middle of a bounded parameter range; `center` of a circle,
  arc, ellipse or elliptical arc; `quadrant` the point `quadrant` quarter turns from the positive
  x axis of a circle or arc, or at that eccentric angle of an ellipse; `parameter` the curve point
  at `parameter` within the parameter range. Anything else fails with
  `draft.dimension.reference_invalid`, as does a path whose entries are not nested that way.
- The derived node `core.draft.dimension.resolved` of a dimension reads the referenced geometry
  and gives the points and the measured value (`dimension::Resolution`); `dimension::resolve`
  evaluates it from a snapshot. The display list reads the node, so editing referenced geometry
  regenerates exactly the dimensions that reference it. The measured value is never stored.
- `core.draft.dimension.create` stores the resolved point as the `position` of an associated
  point and refuses an object outside the dimension's container.
- The `object` and `member` references are cleared when their target is deleted (on-delete
  `clear`). `core.draft.entity.delete` first stores the resolved points of every dimension that
  refers to a deleted entity, so the dimension keeps its last geometry. A point with a key and a
  cleared reference is disassociated and stays at its stored position; the document validator
  reports a warning `draft.dimension.disassociated` (detail `id`) for each such dimension on every
  commit. A deletion through another command keeps the positions the dimension last stored.

### Chains (CORE-DRAFT-012)

A chain is owned by the container of its first member and holds linear or aligned dimensions,
each once (`draft.chain.member_invalid`). Spacing follows the first member: with its first point
p, its line position l and the unit normal n of its dimension line (turned a quarter
counterclockwise from its direction), member i of a baseline chain gets the line position
l + i · spacing · n · sign((l − p) · n) (sign 1 for 0), so each member's dimension line lies one
spacing farther from the points than the one before; every member of a continued chain gets l,
so the dimension lines are collinear. The chain commands write the members and apply the rule.
A member deleted elsewhere leaves the chain (on-delete `clear`) without a new spacing until the
next chain command.

### Leaders and multi-leaders (CORE-DRAFT-013)

- A leader draws a polyline through its points (repeated points dropped), its arrow at the first
  point pointing from the next distinct point, and, when the style's `landing` is set, a
  horizontal landing of `landing_length` from the last point in the horizontal direction of the
  last segment (right for a vertical one).
- A multi-leader's landing runs `landing_length` (0 without `landing`) to `position` along
  `direction`; every leader line runs through its points to the start of the landing with an
  arrow at its first point. The content's near side lies `text_gap` beyond `position`.
- Text content is the `text`, one paragraph per line, laid out at the style's text height and
  anchored at its left (`right`) or right (`left`) edge and at its top, middle or bottom by
  `attachment`. Block content inserts `block` at that point with `block_scale`, unrotated, like a
  block reference. A tolerance frame draws each cell as a box of its text width and the text
  height plus `text_gap` on every side, the cells of a row side by side and the rows downward,
  with the cell text centered in its box; the frame's top, middle or bottom lies at the
  landing's height by `attachment`.
- A style's `content` is stored; the multi-leader's own `content` decides.

### Annotation scaling (CORE-DRAFT-014)

- An annotative dimension, leader or multi-leader stores one representation per supported scale
  (paper length per model length). Its style sizes (text height, gaps, arrow size, extension
  offset and overshoot, landing length, and the multi-leader's block scale) are paper sizes: the
  representation of scale s divides them by s, so they measure their paper size in a viewport of
  annotation scale s. A representation's `position` replaces the dimension's `line_position` or
  the multi-leader's `position`, and its `text_position` the dimension's text position; a leader
  uses neither. Adding a scale never changes existing representations.
- A non-annotative object multiplies dimension style sizes by the style's `overall_scale` (leader
  sizes are used as they are) and is shown at every scale.
- The display list of an annotative object holds the primitives of every representation, each
  with its scale in `Primitive::annotation_scale`. `viewport_list` keeps those whose scale equals
  the viewport's `annotation_scale` exactly, and `space_list` those equal to
  `DisplayOptions::annotation_scale` (default 1). An object without a representation for the
  scale is not shown there; the nearest-scale display the architecture allows by setting is not
  provided.

### Display lists

Dimensions, leaders and multi-leaders contribute to their `core.draft.display` node. Their
strokes and fills carry the entity's appearance with a continuous linetype (source `Object`)
and `part` set to the `annotation::Part` code: 0 dimension line, arc or leader line, 1 extension
line, 2 arrowhead, 3 text, 4 tolerance frame box, 5 landing, 6 block content. Text is laid out
by `text_layout::layout` with the text style the dimension or leader style names, centered (for
dimensions) or anchored as above, and drawn like text entities. A dimension style with a child
of the dimension's kind (`linear` for linear and aligned, `angular`, `radial`, `diameter`,
`ordinate`, `arc_length`) uses the child's values. A list reads the object, its style, the
child styles, the text style, the substitution table and, for a dimension, its resolution node,
so a change of any of these regenerates it.

### Commands

| Command | Effect |
|---|---|
| `core.draft.dimension.create` | Creates a dimension in `owner` of `kind` from `points` (each a free `position`, or `object`, optional `path`, `key`, `quadrant`, `parameter`) with `line_position` and optional `rotation`, `text_override`, `text_position`, `style`, `layer`, `annotative`, `scales` |
| `core.draft.chain.create` | Creates a `baseline` or `continued` chain of `members` with optional `spacing` (default 10) and spaces it |
| `core.draft.chain.add`, `core.draft.chain.remove` | Adds `dimension` at the end of `chain`, or removes it, and spaces the chain again |
| `core.draft.chain.respace` | Sets an optional new `spacing` and spaces the chain again |
| `core.draft.leader.create` | Creates a leader in `owner` through `points` with optional `spline`, `arrow`, `style`, `layer`, `annotative`, `scales` |
| `core.draft.multileader.create` | Creates a multi-leader in `owner` with `leaders`, `position` and optional `direction`, `content`, `text`, `block`, `block_scale`, `tolerance`, `attachment`, `style`, `layer`, `annotative`, `scales` |
| `core.draft.annotation.scales` | Sets `annotative` of dimensions, leaders and multi-leaders, adds a representation for each scale of `add` they lack and removes those of `remove` |

Other fields change through `core.draft.entity.modify`.

## Plotting as implemented

The fifth part of the crate implements CORE-DRAFT-030, 031 and 032: plot style tables
(`plot_style`), plot streams (`plot`) and raster output (feature `raster`, `raster`). The plot pipeline of the architecture is in
[drafting-and-views.md](../architecture/drafting-and-views.md). PDF and SVG writers and TIFF output
are not part of it.

### Plot style tables (CORE-DRAFT-030)

`register_types` adds `core.draft.plot_style_table`, version 1.0:

| Field | Meaning |
|---|---|
| `name` | Unique among plot style tables (`draft.name.taken`) |
| `kind` | `color_mapped` or `named` |
| `description` | Optional text |
| `styles` | Up to 4096 records of the fields below |

A style record holds `name` (the key in a named table), `index` (the key in a color-mapped table,
an integer from 0 to 255), `color` (absent: use the object's color), `lineweight` (millimeters,
0 to 10; absent: use the object's lineweight), `screening` (percent of ink, 0 to 100, default 100),
`linetype` (a reference that blocks deletion of the linetype; absent: use the object's), `end`
(`butt`, `square`, `round`; default `butt`) and `join` (`miter`, `bevel`, `round`; default
`miter`). A style of a color-mapped table needs an `index`, a style of a named table a non-empty
`name` (`draft.plot_style.entry_invalid`), and no key occurs twice
(`draft.plot_style.duplicate`).

A layout selects a table by name through its `plot_style_table` field; the entity field
`plot_style` (absent: the layer's) selects a style of a named table. Commands:

| Command | Effect |
|---|---|
| `core.draft.plot_style_table.create` | Creates a table from `name`, `kind` and optional `styles` (records with the field names above; `color` is linear RGBA, `linetype` an identifier) |
| `core.draft.plot_style_table.modify` | Sets a field of tables |
| `core.draft.plot_style_table.delete` | Deletes tables |

`plot_style::resolve_pen` maps a table (or none) and an appearance (resolved color with its
source, lineweight, linetype, transparency, plot style name) to a pen:

- **Style lookup.** In a color-mapped table the style is the one whose `index` equals the index of
  the color's source `Indexed(i)`; a color without an indexed source has no style. In a named
  table it is the one whose `name` equals the plot style name byte for byte. No table, or no
  matching style, gives the object's values with 100 percent ink, end `butt` and join `miter`.
- **Values.** Color: the style's, else the object's. Width: the style's lineweight, else the
  object's, else 0.25 millimeter. Linetype: the style's, else the object's; the pen records
  whether the style replaced it.
- **Not provided.** Grayscale and monochrome color modes and fill styles.
- **Screening.** With ink `s` = percent / 100, each linear red, green and blue component `c`
  becomes `c·s + (1 − s)`: the screen mixes the color with white in linear light. The alpha of
  the chosen color is kept and multiplied by `1 − transparency`.

### Plot streams (CORE-DRAFT-031)

`plot::layout_stream` plots a layout space and `plot::model_stream` a window of a model space on a
`Paper`. The result, a `PlotStream`, is in millimeters from the lower left corner of the page;
`area` is the part of the page that may be drawn on. Each `Item` has its source entity, part and
layer, an optional index into `clips` (a closed polyline on the page) and one of: a point, a
stroke (the exact curve and, when it is not drawn whole, its pieces), a fill, a wipeout mask, or a
glyph run (family, face, size, glyph positions and the map to the page), with a `Pen` (color, width
in millimeters, end style, join style, linetype) for everything but masks.

- **Content and order.** A layout stream holds, in this order, the display list of each viewport
  in ascending identifier order (`display::viewport_list`; every item has the viewport's clip) and
  then the layout's own entities (`display::space_list`), each list in draw order. A model stream
  holds the space's list. Text is the stroke or glyph primitives of the shared text layout. A
  wipeout is a mask item after what it hides; the stream paints nothing itself, a consumer fills
  masks in the paper color. Entities that cannot be listed, mapped or cut are left out and listed
  in `failed`.
- **Options.** `PlotOptions` has the plot area (`Layout`, `Extents` or a `Window` in source
  coordinates), the scale (`Fit` or `Ratio`), the offset (`Origin` in millimeters from the lower
  left of the printable area, or `Center`), rotation in quarter turns counterclockwise (reduced
  modulo 4), lineweights on or off, plot styles on or off, the plot style table name, the paper
  color and the most items (`draft.plot.too_large`). `PlotOptions::from_layout` reads the layout's
  `plot_area` with its window, `plot_scale` as a ratio, `origin` as the offset and
  `plot_style_table`.
- **Page.** For a layout the page is the sheet in millimeters (the layout's unit converted);
  `Layout` area is the printable area, the sheet less the margins. A model stream's paper gives the
  page and margins in millimeters; there `Layout` means `Extents`. `Extents` is the bounding box of
  what is drawn (masks and unbounded curves do not count); an area without extent fails with
  `draft.plot.option_invalid` (argument `option`), as does a non-positive or non-finite scale,
  a window whose corners are not ascending, an offset that is not finite and margins that leave no
  printable area.
- **Placement.** A `Ratio` is paper length per source length in one unit (1:100 is 0.01 whatever
  the units; an unitless space counts in millimeters), so source coordinates map to millimeters by
  that ratio times the unit's factor to millimeters. `Fit` is the largest scale at which the area,
  turned by `rotation`, fits in the printable area. The area is turned about its lower left
  corner, then placed with its lower left corner at the offset (or centered). Everything is cut
  to `area`, the plotted area cut to the printable area; a consumer clips to it.
- **Pens.** Each primitive's resolved color, lineweight, linetype and transparency go through
  `resolve_pen` with the plot style table named by the options when styles are on, and the style
  name of the drawing entity, else of its layer. With lineweights off every pen is 0.25
  millimeter wide.
- **Dashes.** Dashes are those of the display list, mapped to the page. A style that replaces the
  linetype generates them again on the mapped curve with the entity's scale times the scale of
  the viewport map (1 outside viewports), so a pattern keeps its length in the space it is drawn
  in: model units inside a viewport, layout units on the sheet.
- **Unbounded curves.** Rays and construction lines outside viewports are cut to the plot area
  and carry the result as pieces.
- **Determinism.** The stream is built from sorted identifiers and ordered vectors; `encode`
  writes a header, every number as its bit pattern and every curve in the `arc.geom` encoding,
  so identical input gives identical bytes. The result depends on the platform's trigonometric
  functions only where the display lists do.

### Raster output (CORE-DRAFT-032)

With the feature `raster` (off by default; it adds `tiny-skia` and the `png` feature of `image`),
`raster::render` draws a stream at `RasterOptions::dpi` (1 to 10000, `draft.raster.invalid`
otherwise; at most `max_pixels` pixels, `draft.raster.too_large`) and `Raster::png` encodes it.

- **Image.** One pixel is 25.4 / dpi millimeters; the image is the page rounded up to whole
  pixels, opaque, filled with the paper color, row 0 at the top of the page.
- **Strokes.** Curves are flattened with a chord tolerance of a tenth of a pixel and a turning
  angle of 15 degrees, and drawn with anti-aliasing. A pen is `width · dpi / 25.4` pixels wide,
  at least one, with its end and join styles (miter limit 4). Each drawn piece is a subpath, so
  dashes have the lengths of the stream; a dot piece is a disc of the pen's width.
- **Polyline widths.** A polyline with widths is stroked with the larger of the pen width and
  the mean vertex width; its taper is not drawn.
- **Fills and masks.** Fills use the pen color; masks the paper color, opaque.
- **Clips.** Everything is clipped to `area`, and viewport items also to their clip region, as
  anti-aliased masks.
- **Not drawn.** Outline glyph runs, and curves that cannot be flattened within one million
  vertices, are skipped and counted in `Raster::skipped`. TIFF is not written.

## Hatches as implemented

The sixth part of the crate implements CORE-DRAFT-003 and CORE-DRAFT-020: hatches (`hatch`), hatch
patterns and their file format (`hatch_pattern`). The file format is specified in
[hatch-pattern-format.md](../architecture/hatch-pattern-format.md) and decided in
[ADR-0016](../adr/0016-hatch-pattern-format.md).

### Object types

`register_types` also adds these types, each at schema version 1.0. A hatch is an entity.

| Type key | Fields |
|---|---|
| `core.draft.hatch` | common fields; `kind` (`pattern`, `solid`, `gradient`); `loops`: up to 4096 geometry values; `island` (`alternating`, `outermost`, `ignore`); `boundary_objects`: up to 4096 records of one optional reference each; `pick`; `pattern` (a hatch pattern); `inline`: up to 64 line families; `scale` (positive, default 1); `angle` in radians; `origin`; `gradient_kind` (`linear`, `radial`); `gradient_angle`; `gradient_color1`; `gradient_color2` |
| `core.draft.hatch_pattern` | `name` (unique, `draft.name.taken`); `description`; `lines`: up to 64 line families |

A line family record holds `angle` (degrees), `origin`, `shift`, `spacing` and `dashes` (up to 32
numbers), with the meaning of the [file format](../architecture/hatch-pattern-format.md#line-families).
Type validators: every stored loop is a circle, an ellipse or a closed polyline and there is at least
one (`draft.hatch.boundary_invalid`); every family follows the rules of the format
(`draft.pattern.line_invalid`); a pattern hatch has a `pattern` or at least one inline family, and a
pattern object has at least one family (`draft.pattern.empty`). A pattern in use by a hatch cannot be
deleted. Unused patterns are purged by `core.draft.purge`.

### Boundary and islands

- **Stored loops.** `loops` is the last boundary: closed circles, ellipses and closed bulge
  polylines (lines and arcs) in the `arc.geom` encoding. A boundary of other curves is stored as the
  closed polyline of its lines and arcs; a boundary with elliptical arc pieces is refused with
  `draft.hatch.boundary_invalid`.
- **Island mode.** The region is built from the loops by `arc_geom::build_region`. `alternating`
  fills by nesting parity (a loop at even depth is an outer loop, at odd depth a hole);
  `outermost` keeps the loops of depth 0 with their direct holes and drops what is nested deeper;
  `ignore` keeps only the loops of depth 0, filled whole. Loops that cross or touch fail with the
  errors of `build_region`.
- **Create from curves.** `core.draft.hatch.create` with `boundary` joins the curves into closed
  chains (gaps up to the linear tolerance) and stores all loops of the arrangement; the island mode
  applies when the hatch resolves, so changing it later needs no new boundary. An open chain fails
  with `draft.hatch.boundary_invalid`.
- **Create from a point.** `core.draft.hatch.pick` traces the region around `pick` among the curve
  entities of the owner (or the given `objects`) with `arc_geom::trace_region` and the island mode;
  it stores the loops of the traced region. The errors of tracing are passed on
  (`geom.trace.not_enclosed`, `geom.trace.on_boundary`). The stored loops hold the result of the
  mode, so they resolve to the same region.

### Associativity

- A hatch with `boundary_objects` follows those objects: the derived node
  `core.draft.hatch.resolved` reads their geometry and gives the region
  (`hatch::Resolution`), by joining their curves into closed chains, or by tracing around `pick`
  when the hatch was created from a point. The display list reads the node, so editing a boundary
  object regenerates exactly the hatches that reference it. `hatch::resolve` evaluates the node
  from a snapshot.
- Boundary objects are entities of the hatch's container; a line, arc, circle, ellipse, elliptical
  arc or polyline. The `object` references are cleared when their target is deleted.
  `core.draft.entity.delete` first stores the current region of every associated hatch that refers
  to a deleted entity in `loops`, so the hatch keeps its last boundary. A hatch with a cleared
  record is disassociated: it resolves from its stored loops, and the document validator reports
  a warning `draft.hatch.disassociated` (detail `id`) on every commit. Setting
  `boundary_objects` to an empty list through `core.draft.hatch.modify` ends the warning. A deletion
  through another command keeps the loops the hatch last stored.
- A stale boundary that no longer bounds a region makes the node fail with the error of the
  construction; the hatch is then listed in `failed` by the space list.

### Pattern generation

`hatch::generate` is a pure function from a `Region`, line families, a `Placement` (scale, angle,
origin) and a bound to `Generated { marks, fallback }`; a mark is a segment or a dot.

- **Frame.** The hatch origin is a point of the document: moving the boundary does not move the
  pattern. A family's lines are laid out as in the file format after scaling by `scale` and rotating
  by `angle` about the hatch `origin`; the family origin is relative to the hatch origin.
- **Line range.** For each part of the region the indices of the lines that can reach it come from
  the part's bounding box projected on the family's normal, so the work depends on the size of the
  region, not on its distance from the origin.
- **Clipping.** Each line is intersected with every loop of the part (loops whose box is farther
  than the linear tolerance from the line are skipped); each stretch between consecutive
  intersections is kept when its middle point is strictly inside the part (outer loop, outside all
  holes); adjacent kept stretches join.
- **Dashes.** A dash list starts at the point of the line the family defines (so the phase depends
  only on the placement), repeats, and is cut to each kept stretch. A dash shorter than the linear
  tolerance after cutting is dropped. A dot is a mark when its position lies on a kept stretch.
- **Bound and fallback.** The number of lines visited plus the number of marks must not exceed the
  bound; the display list uses the smaller of `DisplayOptions::max_dashes` and `max_primitives`.
  When the bound is exceeded, or positions in a dash list cannot be told apart in `f64`, the result
  is `fallback`: no marks, and the hatch is drawn as a solid fill (a region primitive with part 1).
  No error is raised; a solid fill is the defined result of a pattern too dense to draw.
- **Errors.** A scale that is not finite and positive, or a non-finite angle or origin:
  `draft.hatch.parameter_invalid`; an invalid family: `draft.pattern.line_invalid`.

### Display lists

A solid hatch contributes one `Shape::Region` primitive; a pattern hatch contributes one stroke
per segment and one point per dot, continuous, with the entity's appearance; a gradient hatch
contributes one `Shape::Region` in `gradient_color1`. The gradient kind, angle and second color are
stored but no consumer draws them yet: a gradient is shown as a flat fill in its first color. Part
is 0, or 1 for a pattern that fell back to a solid fill.

`Shape::Region` and `plot::ItemKind::RegionFill` carry an exact `arc_geom::Region`: every loop of
every part, which consumers fill with the even-odd rule, so holes stay empty. Plot streams encode it
as item tag 5 (the stream version stays 1; tags 0 to 4 are unchanged). Raster output fills it with the
even-odd rule; `arc-gpu` triangulates the region with its holes. A viewport list keeps a region that
reaches the clip boundary whole, like other fills.

### Pattern library and files

`hatch_pattern::PatternFile::parse` reads a pattern file under an `arc_base::Limits` profile and
`to_text` writes its canonical form. `hatch_pattern::library` is the shipped library
`patterns/arc-patterns.ahp`: parallel lines at 45 degrees (`diagonal-lines`), `cross-lines`,
`brick-bond`, `concrete-mix`, `insulation-zigzag`, `gravel-scatter`, `earth-fill`, `steel-double`,
`wood-grain` and `square-grid`. The library is an original work of the project, distributed under
the license of this repository; its patterns are designed from the geometry of the materials and
copy no other product's patterns or names. A fuzz target for the reader is in `fuzz/`.

### Commands

| Command | Effect |
|---|---|
| `core.draft.hatch.create` | Creates a hatch in `owner` from `boundary` (curves) or `objects` (associative), with `kind`, `island`, `pattern` or `inline` families, `scale`, `angle`, `origin`, `gradient` and `layer` |
| `core.draft.hatch.pick` | Creates a hatch around `pick` among the curve entities of `owner` (or `objects`); `associative` (default true) keeps the objects as boundary objects |
| `core.draft.hatch.modify` | Sets a field of hatches |
| `core.draft.hatch_pattern.load` | Creates pattern objects from the `text` of a pattern file, or from the shipped library, all or the listed `names`; returns `ids` |

## Error codes

The codes of every part of the crate:

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
| `draft.layout.margins_invalid` | `invalid_input` | The margins of a layout leave no printable area on its sheet |
| `draft.layout.window_invalid` | `invalid_input` | A layout plots a window without both corners, or with corners not in ascending order |
| `draft.layout.owner_invalid` | `invalid_input` | Layout settings or a viewport are not owned by a layout space |
| `draft.layout.duplicate` | `conflict` | A layout space would own two layout settings objects |
| `draft.viewport.clip_invalid` | `invalid_input` | The clip boundary of a viewport is not a closed polyline |
| `draft.viewport.locked` | `conflict` | A command changes the view of a locked viewport |
| `draft.wipeout.boundary_open` | `invalid_input` | The boundary of a wipeout is not a closed polyline |
| `draft.style.parent_invalid` | `invalid_input` | A dimension style's parent rule is broken |
| `draft.purge.in_use` | `conflict` | An object selected for purging is in use |
| `draft.display.too_large` | `limit_exceeded` | A display list would hold more primitives than the bound |
| `draft.font.syntax` | `invalid_input` | A line of a stroke-font file is malformed |
| `draft.font.version_unsupported` | `unsupported` | A stroke-font file has a version this reader does not read |
| `draft.font.header_invalid` | `invalid_input` | A stroke-font header value is missing, repeated or out of range |
| `draft.font.glyph_duplicate` | `invalid_input` | A stroke font defines a glyph twice |
| `draft.font.glyph_unknown` | `invalid_input` | A stroke font refers to a glyph it does not define |
| `draft.font.glyph_out_of_bounds` | `invalid_input` | A stroke-font glyph leaves its box or the coordinate range |
| `draft.font.limit_exceeded` | `limit_exceeded` | A stroke-font file exceeds a bound of the reader |
| `draft.font.default_missing` | `not_found` | The font provider lacks the default stroke font |
| `draft.font.substituted` | `not_found` | A requested font is not available and another is used (a warning) |
| `draft.font.outline_invalid` | `corrupt_data` | Outline font bytes cannot be read or laid out |
| `draft.text.content_invalid` | `invalid_input` | Text holds a control character other than tab, or a size or factor out of range |
| `draft.text.too_large` | `limit_exceeded` | A text has more paragraphs, runs, tab stops or bytes than the limits allow |
| `draft.text.stack_invalid` | `invalid_input` | A stacked run lacks a denominator, or an ordinary run has one |
| `draft.text.tabs_invalid` | `invalid_input` | Tab stops are not finite, positive and ascending |
| `draft.dimension.points_invalid` | `invalid_input` | A dimension does not have the number of definition points its kind needs |
| `draft.dimension.degenerate` | `invalid_input` | The definition points of a dimension define no measurement |
| `draft.dimension.reference_invalid` | `invalid_input` | A referenced object and sub-object key give no point |
| `draft.dimension.disassociated` | `not_found` | A dimension lost referenced geometry and keeps its last points (a warning) |
| `draft.chain.member_invalid` | `invalid_input` | A chain member is not a linear or aligned dimension, or is listed twice |
| `draft.annotation.scale_invalid` | `invalid_input` | An annotation scale is not a positive number or is listed twice |
| `draft.leader.points_invalid` | `invalid_input` | A leader or a leader line has too few points |
| `draft.plot_style.entry_invalid` | `invalid_input` | A plot style lacks the key its table kind needs: an index or a name |
| `draft.plot_style.duplicate` | `conflict` | Two styles of a plot style table have the same key |
| `draft.plot.option_invalid` | `invalid_input` | A plot option is out of range, or the plot area has no extent |
| `draft.plot.source_invalid` | `invalid_input` | The object to plot is not a layout space with settings, or not a space |
| `draft.plot.too_large` | `limit_exceeded` | A plot stream would hold more items than the bound |
| `draft.raster.invalid` | `invalid_input` | A raster resolution or size is out of range |
| `draft.raster.too_large` | `limit_exceeded` | A raster image would have more pixels than the bound |
| `draft.raster.encode_failed` | `internal` | The PNG encoder failed |
| `draft.hatch.boundary_invalid` | `invalid_input` | A stored hatch loop is not a closed circle, ellipse or polyline, a hatch has no loop, or a boundary object cannot bound a region |
| `draft.hatch.parameter_invalid` | `invalid_input` | A hatch scale is not a finite positive number, or its angle or origin is not finite |
| `draft.hatch.disassociated` | `not_found` | A hatch lost a boundary object and keeps its last boundary (a warning) |
| `draft.pattern.syntax` | `invalid_input` | A line of a hatch-pattern file is malformed |
| `draft.pattern.version_unsupported` | `unsupported` | A hatch-pattern file has a version this reader does not read |
| `draft.pattern.duplicate` | `invalid_input` | A hatch-pattern file defines a name twice |
| `draft.pattern.line_invalid` | `invalid_input` | A line family breaks a rule of the format |
| `draft.pattern.empty` | `invalid_input` | A pattern, or a pattern hatch, has no line family |
| `draft.pattern.limit_exceeded` | `limit_exceeded` | A hatch-pattern file exceeds a bound |
| `draft.pattern.unknown` | `not_found` | A pattern name is not in the pattern file |
