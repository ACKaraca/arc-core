# arc-draft-dxf

The shared mapping between the neutral drawing model of `arc-io-dxf` and the drafting model of
`arc-draft`, with the commands that import a DXF file into a model space and export a model space
to a DXF file. Layer 4. The architecture of the neutral-model rule is in
[interchange.md](../architecture/interchange.md); the format crate is specified in
[arc-io.md](arc-io.md) (section "DXF as implemented") and the drafting types in
[arc-draft.md](arc-draft.md).

## Owns

- The import plan: a pure function from a `Drawing` to the creation operations of drafting objects
  and a loss report.
- The export: a pure function from a snapshot (one model space, the tables and the blocks of the
  document) to a `Drawing` and a loss report.
- The commands `core.draft.import.dxf` and `core.draft.export.dxf`.
- The 256-entry indexed color table and the insertion-unit table of the format.

## Stays in products

Everything that depends on a product object model: DWG mapping, dynamic block behavior, paper
space and layouts, associative dimensions and hatches, product-specific options and presets, and
the fidelity level a product promises. A product that needs a different mapping depends on
`arc-io-dxf` and writes its own; it can still reuse the commands of this crate.

## Where the mapping lives

The architecture documents give the mapping of a format's neutral model to the product that owns
the domain model, and the invariants forbid it in `arc-io-dxf` and a format dependency in
`arc-draft`. The drafting types are shared by every product (they are the "shared mappings live
with the core type" case of the interchange architecture), so the part of the mapping that targets
the drafting types lives in this crate. It is a layer 4 crate with the same-layer edges to
`arc-draft` and `arc-io-dxf` recorded in the layer table, in the overview and in
[ADR-0001](../adr/0001-enforced-layering.md) (amendment "Mapping crate between a format crate and
the drafting model"). The rejected alternative, a module of `arc-io-dxf` that depends on
`arc-draft`, breaks the invariant that format crates carry no product mapping.

## Dependencies

| Crate | Arc | Third-party |
|---|---|---|
| `arc-draft-dxf` | `arc-base`, `arc-geom`, `arc-doc`, `arc-command`, `arc-draft`, `arc-io-dxf` | `serde`, `serde_json`, `schemars` |

The crate is in the WebAssembly set: its file access goes only through `arc_command::Context`.

## Public API

| Item | Meaning |
|---|---|
| `plan(&Drawing, &ImportOptions) -> Plan` | The creation operations (`Plan::ops`, in creation order) and the loss report of the mapping |
| `plan_with_report(&Drawing, &ImportOptions, &LossReport) -> Plan` | The same, with the report of reading the drawing merged into the plan's report |
| `apply(&mut Context, space, &Plan) -> Applied` | Creates the plan's objects in the transaction of the running command, binds each object that has a source handle to it in the source map, lists every created object in creation order (`Applied::created`, a `Created` with the identifier, the type key and the source handle), and counts the existing objects bound (`adopted`) and the handles moved from earlier imports (`rebound`) |
| `ImportOptions::source_document`, `Plan::source_document` | The label of the source document the bindings refer to; empty by default |
| `export_drawing(&Snapshot, space) -> Exported` | The model space with the document's tables and blocks as a `Drawing`, every handle assigned by the export (`HandlePolicy::Assign`), with a report and the handle written for each exported object (`Exported::handles`) |
| `export_drawing_with(&Snapshot, space, &HandlePolicy) -> Exported` | The same, with the handles chosen by the policy |
| `HandlePolicy` | `Assign` (the default) or `KeepSources { document, floor }`; see [Source identity](#source-identity) |
| `register_commands(&mut Registry)` | Registers the two commands |
| `Builder`, `Reader` (module `fields`) | Set and read the values of drafting objects by field name |
| `color::aci_rgb`, `color::from_index`, `color::from_true`, `color::to_true` | The indexed color table and color conversions |
| `units::factor`, `units::unit_of_code`, `units::code_of_unit` | Insertion units |

## Commands

| Command | Permission classes | Undo | Parameters |
|---|---|---|---|
| `core.draft.import.dxf` | `edit`, `file_read` | undoable | `path`, `space` (a model space), `list_created` (default `false`). The result holds the counts and the loss report; with `list_created` it also holds `created` (each object with `object`, `kind` as its type key and `source` as the handle in hexadecimal or null) |
| `core.draft.export.dxf` | `read`, `file_write` | not undoable | `path`, `space`, `version` (`2000`, `2004`, `2007`, `2010`, `2013`, `2018`; default `2018`), `binary` (default false), `keep_sources` (optional: `document`, the label of the import's source document, and `floor`, default 0). The result holds the counts, `handles` (each exported object with the handle written) and the loss report |

Both are exposed on every surface the command specification allows (user interface, command
line, automation, MCP and plug-ins). Neither opens a dialog: a shell that lets a person pick a
file passes the chosen path as the parameter. Files are opened with `Context::open_read` and
`Context::create_write` only, so the session's roots and permission classes decide what a command
may touch; the reference command-line shell does not register these commands and needs not.

The import reads the file with the session's limits (`io.limit_exceeded`, argument `limit`) and
checks the cancellation token of the invocation at every 1024 records of the file and every 256
operations of the plan (`cmd.cancelled`; the format crate's own token reports `base.cancelled`).
It creates every object in the one transaction of the command, so one undo removes the whole
import and a failure or cancellation changes nothing. The result holds the counts of the created
objects and the loss report in the form of `schemas/loss-report.schema.json`.

The export builds the drawing from the snapshot, writes it with the format crate, and only then
creates the file, so a failed export leaves no file.

## Source identity

An import binds the objects it creates to the handles they have in the file, and an export with
`keep_sources` writes those handles back. The bindings are the source map of `arc-doc` (CORE-DOC-052):
one entry per object, format `dxf`, document the source document label, identifier the handle in
hexadecimal. A binding is a foreign data row created in the transaction of the import, so one undo
removes the objects and their bindings.

### Import

- Bound: each layer, linetype, text style, dimension style, block definition and entity the import
  creates from a file object. A block definition is bound to its block record (the `BLOCK_RECORD`
  entry that owns the entities of the block).
- Existing definitions: a file layer, linetype, text style or dimension style whose name matches an
  object of the document without regard to case is not created. The existing object is bound to the
  file's handle when it has no binding under the label yet (adopted, counted in `Applied::adopted`,
  not listed as created). The standard linetypes `ByBlock`, `ByLayer` and `Continuous` bind the same
  way to an existing linetype of that name; the import creates none of them.
- Not bound:
  - an object without a handle in the file (absent or zero);
  - an object whose handle a preserved record of the file also holds: the record keeps the handle and
    the object is created unbound (`draft.dxf.handle_duplicate`);
  - a repeated handle after the first occurrence (`draft.dxf.handle_duplicate`);
  - an entity on paper space, which is not imported;
  - the `BLOCK` and `ENDBLK` records of a block; only the block record is bound;
  - the attribute definitions (`ATTDEF`) of a block, and the attribute values (`ATTRIB`) of a block
    reference, whose values the reference holds;
  - the records kept as foreign data rows, which keep their handle in their own row;
  - the standard table entries the export writes by itself, when the document has no linetype of
    that name.
- Source document label: the command uses the final component of the path (the file name, without
  its directories). When the path has no final component, such as a root or a path ending in `..`,
  the path as given is the label. The label is part of the key `(dxf, label, handle)`. Two files
  with one final component imported into one document share their keys. Products that need another
  label call `plan` with `ImportOptions::source_document`.
- Rebinding: when a key `(dxf, label, handle)` is held by a live object of an earlier import, the
  key moves to the object of this import. Each move is counted and reported as
  `draft.dxf.binding_moved` (the count is the number of moves). The earlier object loses its binding,
  and an export writes its handle as a new one. A second import of the same file therefore moves the
  bindings of the objects it creates, and reports them; existing objects bound by the first import keep
  their bindings.
- Repeated handles: of several objects of one file with the same handle, the first in plan order is
  bound. Plan order is the definitions (layers, linetypes, text and dimension styles, block records),
  then the entities of the blocks, then those of the model space, each in file order. The others are
  created without a source and reported with `draft.dxf.handle_duplicate`.
- Created identifiers: `Applied::created` lists every object the import created, in creation order,
  including the unbound ones. The command returns this list in its `created` result only when the
  parameter `list_created` is `true` (default `false`); the counts and the loss report are always
  returned.
- Limits: each binding is one more foreign data row and one more operation of the import's
  transaction. Both count against the document limits (`record_count` and `operations`), so a large
  file can exceed them; the import then fails in its transaction (`doc.txn.limit_exceeded`) and
  changes nothing.

### Export with `keep_sources`

- `HandlePolicy::Assign` is the default. Every record gets a new handle above the handles of the
  preserved records, and the bindings are ignored, so the output is the same as before the policy
  existed.
- `HandlePolicy::KeepSources { document, floor }` writes each object bound to `document` with its
  bound handle. Every other record gets a new handle above the largest handle in use (bound handles of
  the document and handles of the preserved records), above `floor`, and not below 0x20, the lowest
  handle an export assigns.
- Handle space: the export never wraps a handle. It is refused with `draft.dxf.handle_space_exhausted`
  when the handle space above its start has fewer than 2^32 values, which covers a floor or a bound or
  preserved handle near the top of the 64-bit range.
- Collisions: the export never writes one handle twice. A record written first keeps a handle, in the
  order the export writes the tables, the blocks and the model space. A bound object whose handle
  another record already holds gets a new handle, reported with `draft.dxf.handle_reassigned`.
  Preserved records follow the same rule: the first written keeps its handle, and the other is
  renumbered (its handle group is replaced) and reported. A bound object is written only when it is
  written, so an entity that the export skips does not hold its bound handle.
- Preserved records renumbered by the export: pointers to them inside other preserved records (for
  example owner or soft-pointer groups) are not rewritten. The renumbered record keeps its groups
  otherwise.
- Root dictionary: the format does not fix the handle of the root dictionary. The export keeps its
  usual handle 0xC unless a bound or preserved record has that handle; then the bound object keeps it,
  the root dictionary takes a new handle, and the report names the replaced handle (`DICTIONARY`).
- `$HANDSEED`: the writer computes the seed as one above the largest handle it writes, so the seed is
  above every written handle. A drawing whose largest handle leaves fewer than 2^32 values above it is
  refused by the writer (`io.dxf.write_value_invalid`), so the seed never wraps. The export refuses
  such a space before it builds the drawing, as above.
- Result: `Exported::handles` (and the command's `handles`) names the handle written for every
  exported object (entities, layers, linetypes, styles and block records), so a product records the
  handles the export assigned. Structure records (the root, the layouts and the standard entries) are
  not listed.
- Name lookups: an existing table object is matched to a table entry by name without regard to case,
  as the table merge does, so a style named `STANDARD` keeps its bound handle under the name `Standard`.
- A product that writes a file back keeps its objects' handles by importing with the document label
  it uses, editing in the document, and exporting with the same label and its own `floor` (its handle
  high-water mark). Objects it adds take handles above the floor and above every bound handle.

## DXF mapping as implemented

Names in a DXF file do not distinguish case; layers, linetypes, styles and blocks are matched
without regard to case, and an object used but not defined in the file is created with default
values (`draft.dxf.definition_created`). Names longer than 255 bytes are cut.

### Units

The `$INSUNITS` code of the file selects the unit of its coordinates (codes 1 to 21 are known:
the ten units of the drafting model and microinch, mil, angstrom, nanometer, micron, decimeter,
decameter, hectometer, gigameter, astronomical unit, light year and parsec). Lengths of the model
space content (coordinates, radii, widths, heights, elevations, thicknesses, dimension points,
pattern lines, linetype element lengths and offsets, text style heights and dimension style
lengths) are multiplied by the factor from the file's unit to the unit of the target space. Scales,
rotations and angles are not converted.

Block content is not converted on import. Each block keeps its content in its own unit: the
`BLOCK_RECORD` insertion units (group 70, kept as the table entry's flags), and the file's unit when
that record has none, because a unitless block takes the units of the drawing that holds it. The
drafting model converts a reference to the block by the factor from the block's unit to the unit of
the space that holds the reference, and places the reference position with the file factor. So a
block inserted from a file in inches shows its content at 25.4 times its raw size in a millimeter
space, and a block whose record names millimeters shows it at its raw size. `tests/block_units.rs`
checks the displayed radius and position for a record with a unit, a unitless record and a record in
another unit. When the file has no unit, an unknown code, or the target space is
`unitless`, the factor is 1 and the import reports `draft.dxf.units_unitless`. The export writes
the unit of the space as `$INSUNITS` and the unit of each block in its block record.

### Tables and blocks

| DXF | Drafting object | Notes |
|---|---|---|
| LAYER | `core.draft.layer` | Color (index with the index kept as source, or true color), linetype, lineweight (hundredths of a millimeter, `-3` default, above 10 mm clamped), on (negative color is off), frozen (flag 1), locked (flag 4), plot flag, transparency (group 440 of the entry). A layer of the document with the same name is reused |
| LTYPE | `core.draft.linetype` | Dash, gap, dot (length 0), text and shape elements with scale, rotation, offset, text and style name; `Continuous`, `ByLayer` and `ByBlock` entries are built in and not created; a pattern without positive total length is reported (`draft.dxf.linetype_invalid`); more than 256 elements are cut |
| STYLE | `core.draft.text_style` | Font file name without directory and extension, height, width factor, oblique angle (limited to 85 degrees), backwards and upside-down flags, vertical flag; shape files are skipped |
| DIMSTYLE | `core.draft.dim_style` | Name and the variables 40, 41, 42, 44, 140, 144, 147 (to `overall_scale`, `arrow_size`, `extension_offset`, `extension_beyond`, `text_height`, `measurement_scale`, `text_gap`) and 271 (`precision`); other variables stay in the neutral model |
| BLOCK with its entities | `core.draft.block` and the entities it owns | Base point (x, y), unit, `ATTDEF` as attribute definitions (tag, prompt, default, constant, invisible, preset, position); the `*Model_Space` block contributes its entities to the model space, `*Paper_Space` blocks, external references and the anonymous blocks of dimensions that nothing inserts are not imported |
| INSERT | `core.draft.block_ref` | Position, per-axis scale (a zero or non-finite scale becomes 1), rotation, attribute values for tags the block defines; an array (columns and rows) is expanded into single references; a reference to a missing block, or one that would make a block contain itself, is not imported; a reference whose level exceeds the drafting model's nesting limit is not imported (see [Nesting limit](#nesting-limit)) |
| VPORT, VIEW, UCS, APPID, classes, other tables and sections | none | Listed in the report as skipped tables; the OBJECTS section (dictionaries, layouts, groups) is counted in one item |

### Entities

| DXF | Drafting object | Notes |
|---|---|---|
| POINT | `core.draft.point` | |
| LINE, RAY, XLINE | `core.draft.line`, `ray`, `construction_line` | |
| CIRCLE | `core.draft.circle` | |
| ARC | `core.draft.arc`; a full turn becomes a circle | Angles in degrees, counterclockwise |
| ELLIPSE | `core.draft.ellipse` or `elliptical_arc` | A parameter range of a full turn is an ellipse |
| LWPOLYLINE, POLYLINE | `core.draft.polyline` | Bulges, per-vertex and constant widths, closed flag; repeated points are removed; classic polylines of 2D vertices only; vertices created by curve or spline fitting are skipped and a smoothed polyline is reported as approximated; 3D polylines and meshes are not imported |
| SPLINE | `core.draft.spline` | A NURBS curve of the control points, knots and weights; when those do not form a valid curve the fit points are interpolated; fit points are kept as data |
| TEXT | `core.draft.text` | Percent codes `%%d`, `%%p`, `%%c` and `%%nnn` are replaced; underline and overline switches are dropped; the second alignment point is the position for non-default alignment; aligned and fit alignment are left-aligned and reported; width factor and oblique angle of the text are not kept |
| MTEXT | `core.draft.rich_text` (plain paragraphs) | Formatting codes are removed (`\P` and `\N` start a paragraph, `\~` is a space, a stacked fraction keeps its slash); the removal is reported (`draft.dxf.text_formatting`) |
| DIMENSION | `core.draft.dimension` | Rotated, aligned, two-line angular, three-point angular, radial, diameter and ordinate dimensions with their definition points, as non-associative dimensions (`draft.dxf.dimension_nonassociative`); the text `<>` stands for the measurement; the anonymous block of the file is not used |
| HATCH | `core.draft.hatch` | Boundary loops of polylines with bulges, chains of line and arc edges (closed within a relative tolerance of 1e-6, reversed edges are turned) and single full circle or ellipse edges; solid fill or pattern lines stored inline in the final frame of the hatch (offset converted to shift and spacing); island style normal, outer and ignore; a loop with ellipse or spline edges drops the hatch (`draft.dxf.hatch_dropped`); gradient fills and pattern names without lines become solid fills (`draft.dxf.hatch_approximated`) |
| WIPEOUT | `core.draft.wipeout` | The clip boundary (rectangle or polygon) in world coordinates; the frame flag is not kept |
| LEADER, SOLID, TRACE, 3DFACE, VIEWPORT, IMAGE, ATTRIB and ATTDEF outside their block | none | Reported with their kind and handle (`draft.dxf.entity_unmapped`) |
| Preserved records (types the neutral model does not type) | foreign data rows | Kept and reported (`draft.dxf.record_preserved`), see below |
| Entities of paper space | none | Reported (`draft.dxf.paper_space_skipped`) |

Records the neutral model does not type (`PreservedRecord`: entities of a type unknown to the
format crate, in the model space or in a block, and the like records of the OBJECTS section) are
kept in the document as foreign data rows ([arc-doc](arc-doc.md#journal-foreign-data-and-diagnostics-in-the-package-core-doc-040)):
one row per record, `source_format` `dxf`, `source_version` the format version of the file,
`source_id` the handle in hexadecimal, `record_kind` `entity` or `object`, `data` the record
(its type and groups, encoded by `plan::encode_record`) and `object` the space or block that holds
the entity, or the target space for a record of the OBJECTS section. The rows are added by the
import's transaction, so one undo removes them with the rest of the import. At most 10,000 rows
are kept per import (and no more than `record_count` of the limits, nor rows larger than
`input_bytes`); further records are counted under `draft.dxf.foreign_overflow`. Kept records are
reported under `draft.dxf.record_preserved`. Entities of a type the neutral model types but the
drafting model has no object for (leaders, solids, images and the like) and the typed objects of
the OBJECTS section are listed in the loss report only.

### Appearance and geometry fields

| DXF | Drafting field |
|---|---|
| Color 256 or absent | `color_by` layer |
| Color 0 | `color_by` block |
| Color 1 to 255 | `color_by` object, the table entry with the index kept as source |
| True color (420) | `color_by` object, exact; wins over the index |
| Linetype `BYLAYER`, `BYBLOCK`, `CONTINUOUS`, other name | `linetype_by` layer, block, object without a linetype, object with the linetype (an unknown name is continuous and reported) |
| Linetype scale (48) | `linetype_scale` (positive values only) |
| Lineweight -1, -2, -3, hundredths of a millimeter | `lineweight_by` layer, block, object with the default lineweight, object in millimeters |
| Transparency (440) | `transparency_by` layer (0), block (`0x01000000`), object (1 minus alpha over 255) |
| Invisible (60) | `visible` |
| Elevation (38), or the z of the defining point | `elevation`; coordinates of the same entity at another z are not kept (`draft.dxf.z_dropped`) |
| Thickness (39), extrusion (210) | `thickness`, `extrusion`, stored unchanged |

Entities are created in file order, each on top of the previous one in the draw order; an import
into a space that has entities goes on top of them.

### Export

The export writes the entities of the model space in draw order and, per block of the document,
its entities, attribute definitions and base point, with the standard structure around them: the
tables VPORT, LTYPE (with `ByBlock`, `ByLayer` and `Continuous`), LAYER (with layer `0`), STYLE,
APPID, DIMSTYLE (each with its standard entry) and `BLOCK_RECORD`, the blocks `*Model_Space` and
`*Paper_Space`, and a root dictionary with the layout dictionary and the layouts `Model` and
`Layout1`. Every record has a handle and an owner assigned by the export.

| Drafting object | DXF | Notes |
|---|---|---|
| point, line, ray, construction line, circle | POINT, LINE, RAY, XLINE, CIRCLE | z is the elevation |
| arc | ARC | A clockwise arc is written counterclockwise from its end to its start; a full turn is read back as a circle |
| ellipse, elliptical arc | ELLIPSE | Parameters in radians, counterclockwise |
| polyline | LWPOLYLINE | Bulges, widths, closed flag, elevation |
| spline | SPLINE | Planar, rational when the weights differ; knots, control points, fit points |
| text | TEXT | Alignment code and second alignment point when not left and baseline; a height of 0 is written as the default height 2.5 |
| rich text | MTEXT | Paragraphs joined with `\P`, `\`, `{` and `}` escaped, stacked fractions as `\S`; run and paragraph formatting is not written (`draft.dxf.export_approximated`, `text_formatting`) |
| dimension | DIMENSION | Linear, aligned, radial, diameter, angular (points and lines) and ordinate; stored definition points are written (an associative dimension is written with its last stored positions and reported); angular-arc and arc-length dimensions are not written; the file has no anonymous dimension block, so a reader draws the dimension from its definition |
| hatch | HATCH | Loops (polyline, circle and ellipse edges), solid or the lines of the inline families or of the referenced pattern in the final frame (scale and angle applied); a gradient is written solid (reported) |
| wipeout | WIPEOUT | The boundary as the clip polygon of a unit image; the frame is not written (reported) |
| block reference | INSERT with ATTRIB | Attribute values for the tags of the definition, at the definition's attribute positions transformed by the reference; the attribute text height is 2.5 times the y scale |
| leader, multi-leader, other types | none | Reported (`draft.dxf.export_unmapped`) |
| foreign data rows of format `dxf` | the preserved records | Rows of kind `entity` attached to the exported space or a block are written after its drawn entities, rows of kind `object` attached to the exported space after the standard objects, each with its groups unchanged except the owner group of an entity, which names the container it is written to. The handles the export assigns start above the handles of the rows. A row that does not decode is reported (`draft.dxf.export_unmapped`) |
| layer, linetype, text style, dimension style, block | LAYER, LTYPE, STYLE, DIMSTYLE, BLOCK_RECORD and BLOCK | Same fields as the import; linetype text style names are written as handles of the style entries |

### Loss reasons

Items of the mapping are listed one per occurrence, up to 4096 items. Further occurrences are
aggregated per reason and record kind, up to 4096 such items; beyond that, kinds share one overflow
item per reason. The overflow item keeps the arguments of the first occurrence folded into it, and
a record kind named by the file is cut to 256 bytes before it is used. Items of the reader and
writer of `arc-io-dxf` are merged into the same report. A report that folded occurrences is marked
`truncated`; its counts stay exact. The bound on the mapping report is therefore 4096 items, 4096
aggregated items and one overflow item per reason.

### Nesting limit

The drafting model rejects a transaction in which a chain of block references is deeper than
`arc_draft::validate::DEFAULT_MAX_NESTING` (64). The level of a reference is the number of
references on the longest chain ending at it, the reference included, where a chain may start at
any root: the model space, or a block that nothing references. The depths are taken over the kept
references only. Before any
object is applied, the mapping computes the level of every block reference over the reference graph
(Kahn's order, no recursion; a reference on a cycle is refused earlier, see the INSERT row). Each
reference whose level exceeds the limit is dropped and reported once (`draft.dxf.reference_depth_exceeded`,
argument `limit`); its block definition is still created. Every kept reference has a level no
greater than the limit, so the import completes. Arrays are dropped cell by cell, and the count of
the item counts the references that were not created. The DWG reader returns the same neutral
drawing, so a DWG file mapped through this plan follows the same rule; no command of this crate
reads DWG files.

| Reason | Outcome |
|---|---|
| `draft.dxf.entity_unmapped` | `dropped` |
| `draft.dxf.record_preserved` | `preserved` |
| `draft.dxf.foreign_overflow` | `dropped` |
| `draft.dxf.paper_space_skipped` | `dropped` |
| `draft.dxf.objects_skipped` | `dropped` |
| `draft.dxf.table_skipped` | `dropped` |
| `draft.dxf.geometry_invalid` | `failed` |
| `draft.dxf.text_formatting` | `approximated` |
| `draft.dxf.text_adjusted` | `approximated` |
| `draft.dxf.value_adjusted` | `approximated` |
| `draft.dxf.z_dropped` | `approximated` |
| `draft.dxf.units_converted` | `mapped` |
| `draft.dxf.units_unitless` | `approximated` |
| `draft.dxf.dimension_nonassociative` | `approximated` |
| `draft.dxf.hatch_dropped` | `failed` |
| `draft.dxf.hatch_approximated` | `approximated` |
| `draft.dxf.array_expanded` | `approximated` |
| `draft.dxf.attribute_dropped` | `dropped` |
| `draft.dxf.reference_unresolved` | `failed` |
| `draft.dxf.reference_depth_exceeded` | `dropped` |
| `draft.dxf.definition_created` | `mapped` |
| `draft.dxf.linetype_invalid` | `failed` |
| `draft.dxf.name_adjusted` | `approximated` |
| `draft.dxf.export_unmapped` | `dropped` |
| `draft.dxf.export_approximated` | `approximated` |
| `draft.dxf.handle_duplicate` | `approximated` |
| `draft.dxf.handle_reassigned` | `approximated` |
| `draft.dxf.binding_moved` | `approximated` |

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `draft.dxf.space_invalid` | `invalid_input` | The target of an import or export is not a model space |
| `draft.dxf.document_invalid` | `corrupt_data` | A stored drafting object does not have the shape its type declares |
| `draft.dxf.handle_space_exhausted` | `invalid_input` | The export needs a handle above the highest one in use, and fewer than 2^32 handles remain above the start of the assignment |

The import and export also fail with the codes of `arc-io-dxf` (`io.limit_exceeded`,
`io.unsupported_version`, `io.dxf.*`), of `arc-command` (`cmd.path.outside_roots`,
`cmd.permission_denied`, `cmd.cancelled`, `cmd.params.invalid`) and of the transaction.

## Test strategy

Unit tests for the color table, units and message registry; integration tests through the command
registry and invoker (one transaction and undo, unit conversion, loss reports, limits,
cancellation, roots and permissions, every surface, name reuse, block cycles and arrays); a
property test that exports random documents of the mapped kinds, imports the result into an empty
space and compares the content, and its reverse for hand-built neutral drawings. The unit rule of block
definitions is tested on the displayed geometry (`tests/block_units.rs`), and the linetype element
lengths and offsets are tested in `tests/mapping.rs`. The nesting limit
is tested on chains of 64 and 65 references (the deepest one dropped, exactly one item), on a wide
graph with shared sub-blocks, and through the import command, which must complete.

The source identity tests (`tests/source_identity.rs`) import a drawing with bound layers, linetypes,
styles, block records, block entities and model-space entities, export it with `keep_sources`, and
read the written file back: every bound handle is written unchanged, the seed is above the written
handles, and an object added afterwards takes a handle above the floor. They also cover a repeated
handle, a preserved record that shares a bound handle, a bound handle on the root dictionary, and one
undo removing the bindings.
