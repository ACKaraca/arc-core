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
| `export_drawing_limited(&Snapshot, space, &HandlePolicy, &Limits) -> Exported` | The same, with the block references of the arrays bounded by the limits (see [Arrays](#arrays)); the command passes the session's limits |
| `export_drawing_for_mode(&Snapshot, space, &HandlePolicy, &Limits, binary: bool) -> Exported` | The same, for the output form the writer is given: a stored value the writer refuses in that form is omitted and reported, so the write does not fail. `export_drawing_limited` is the ASCII form; the export command passes its `binary` parameter |
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
  Preserved records, dictionaries and registered applications follow the same rule under both policies:
  the first written keeps its handle, and the other is renumbered (its handle group is replaced) and
  reported with `draft.dxf.handle_reassigned`. The default policy reports this too; earlier versions
  renumbered such a record silently in the writer. A record whose handle is zero, or missing, gets a new
  handle. A bound object is written only when it is written, so an entity that the export skips does not
  hold its bound handle.
- References: every reference of a kept record is written through one map from the source numbers of the
  file to the handles the export writes (see [References of kept records](#references-of-kept-records)).
  The map is the same under both policies.
- Root dictionary: the format does not fix the handle of the root dictionary. The export keeps its
  usual handle 0xC unless a stored record or a registered application has that number; then the root
  takes a new handle and the report names the replaced handle (`DICTIONARY`). The named-object root of
  the file is merged into it, see [Extended data, registered applications, classes and
  dictionaries](#extended-data-registered-applications-classes-and-dictionaries).
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
the OBJECTS section, other than dictionaries, are listed in the loss report only. Dictionaries are kept
(see below), and so are the extended data, the class records and the registered applications.

Header variables the import does not consume are kept the same way, unless they are not storable.
A variable is storable when its name is a dollar sign followed by letters, digits or underscores
(compared without regard to case, and kept as the file has it); it has at least one value; every
value has the kind its group code gives (a real is finite, a code the crate does not know takes
text); and every point is complete (an X group, codes 10 to 18, is followed by its Y group, codes
20 to 28, which is followed by a Z group, codes 30 to 38, when there is one). A variable that is not
storable is not kept and is reported under `draft.dxf.header_value_omitted`.

Before a kept variable is stored, the import converts it to the target's units. The lengths and the
points of the variables listed in the table below are multiplied by the unit factor of the file
(positive `$PDSIZE` only, as a negative size is a percentage of the view). A variable of a name with
no known unit class is kept unscaled and reported once under `draft.dxf.header_unscaled` with all
such names. The variables that are ratios, angles, directions, times or counts are not changed.
The table of unit classes is the crate's list: lengths (group 40) are `$TEXTSIZE`, `$ELEVATION`,
`$PELEVATION`, `$THICKNESS`, `$TRACEWID`, `$VIEWSIZE`, `$FILLETRAD`, `$CHAMFERA`, `$CHAMFERB`,
`$CHAMFERC`, `$PLINEWID`, `$DIMASZ`, `$DIMTXT`, `$DIMEXO`, `$DIMEXE`, `$DIMGAP`, `$DIMDLI`,
`$DIMDLE`, `$DIMTSZ`, `$DIMCEN`, `$DIMRND`, `$DIMTP`, `$DIMTM` and `$DIMFXL`; points are `$LIMMIN`,
`$LIMMAX`, `$PLIMMIN`, `$PLIMMAX`, `$INSBASE`, `$PINSBASE`, `$VIEWCTR`, `$SNAPBASE`, `$SNAPUNIT`,
`$GRIDUNIT`, `$UCSORG` and `$PUCSORG`.

The measurement system follows the target. When the file states one (its `$MEASUREMENT`, or else
the system of its unit) and it is not the target's, `$MEASUREMENT` is written as the target's, and
the file's `$LUNITS` (a format of its own system) is dropped and reported under
`draft.dxf.header_derived_omitted`. A file that states neither keeps its variables as they are.

The import replaces the header rows of its space: the rows of format `dxf` and record kind `header`
that an earlier import wrote into the same space are removed in the same transaction, found by
format, record kind and object, never by order. A second import into a space therefore leaves one
set of header rows, not two.

One foreign data row per
variable, `source_format` `dxf`, `record_kind` `header`, `source_id` empty, `object` the target space,
and `data` the variable as `plan::encode_record` encodes a record whose kind is the variable's name
(`$DIMTXT`, for example) and whose groups are its value groups, unchanged. `$INSUNITS` is the only
variable the import consumes, so it is not kept. Rows are kept in file order, at most
`plan::MAX_HEADER_VARIABLES` (1,024) per import and within the string bound of the limits in encoded
bytes; a variable beyond either bound is counted under `draft.dxf.foreign_overflow` with the kind
`HEADER`. A kept variable needs no loss item. The rows are not counted against the 10,000 rows of
the records above, and one undo removes them with the rest of the import.

Header angle values are kept as the file writes them. This crate does not convert them: the neutral
model holds header angles in degrees, as the DXF text does, and the list of angle variables is in
[arc-io](arc-io.md#dwg-header-variables).

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
APPID (the stored entries, with `ACAD` unless the file has it, and the applications that written
extended data names), DIMSTYLE (each with its standard entry) and `BLOCK_RECORD`, the blocks
`*Model_Space` and `*Paper_Space`, and a root dictionary with the layout dictionary and the layouts
`Model` and `Layout1`, to which the entries of the file's root dictionaries are merged. Every drafted
record has a handle and an owner assigned by the export; a stored record keeps its own owner rules
(see References below).

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
| array | INSERT, one per item | The block reference of each item at the placement the display list draws, with the array's scale and layer; the definition is not written and is reported (`draft.dxf.array_approximated`). See [Arrays](#arrays) |
| leader, multi-leader, other types | none | Reported (`draft.dxf.export_unmapped`) |
| foreign data rows of format `dxf`, kind `header` | the header variables | See [Header variables of the export](#header-variables-of-the-export) |
| foreign data rows of format `dxf` | the preserved records | Rows of kind `entity` attached to the exported space or a block are written after its drawn entities, rows of kind `object` attached to the exported space after the standard objects, each with its groups as stored except the owner group of an entity, which names the container it is written to, and the handle references rewritten as [References](#references-of-kept-records) describes. The handles the export assigns start above the handles of the rows. A row that does not decode is reported (`draft.dxf.export_unmapped`) |
| foreign data rows of kind `xdata` | the extended data of the object the row is attached to | Written with the object, see [Extended data](#extended-data-registered-applications-classes-and-dictionaries) |
| layer, linetype, text style, dimension style, block | LAYER, LTYPE, STYLE, DIMSTYLE, BLOCK_RECORD and BLOCK | Same fields as the import; linetype text style names are written as handles of the style entries |

### Header variables of the export

The export writes the header rows of the exported space (kind `header`, described under Entities above)
in their stored order, then the variables the export owns. A variable the export owns takes the
place of a stored variable of the same name, and otherwise follows the stored ones in the export's
order. A name is written once; of several rows with one name, the last row's groups are written at
the first row's place. A row that does not decode is skipped.

Before a stored variable is written, the export checks its class. A variable that fails its check is
not written and is reported, one loss item per class, with the names of the variables (at most 16 are
listed; the count is complete):

| Class | Variables | Written when | Reason |
|---|---|---|---|
| Derived | `$EXTMIN`, `$EXTMAX`, `$PEXTMIN`, `$PEXTMAX`, `$TDUPDATE`, `$TDUUPDATE`, `$TDINDWG`, `$TDUSRTIMER` | Never. The export computes none of them (no drawing-level bounds exist, and the times and the user timer are the editing history), and a stored value would be stale. A reader recomputes them | `draft.dxf.header_derived_omitted` |
| Handle | any variable with a group code 5, 105, 320 to 369, 390 to 399, 480 or 481 | Every such handle names an object the export writes under that handle: the object is bound to it under `KeepSources` and was written with it. Under the default policy no object keeps a file handle, so these variables are omitted | `draft.dxf.header_handle_omitted` |
| Name | `$CLAYER`, `$CELTYPE`, `$TEXTSTYLE`, `$DIMSTYLE`, `$DIMBLK`, `$DIMBLK1`, `$DIMBLK2`, `$DIMLDRBLK`, `$DIMTXSTY`, `$DIMLTYPE`, `$DIMLTEX1`, `$DIMLTEX2`, `$UCSNAME`, `$UCSBASE`, `$PUCSNAME`, `$PUCSBASE`, `$CMLSTYLE` | The name is an entry of the written table, compared without regard to case (block names: block records; text styles and linetypes: their tables). An empty block name is written, and an empty UCS name is the world coordinate system. The export writes no UCS table, so a named UCS is omitted. `$CMLSTYLE` is always omitted, because the export writes no multiline style table. An empty linetype or text style name is omitted | `draft.dxf.header_name_omitted` |
| Value | `$DWGCODEPAGE`, a name that is not a variable name, and a value that is not storable or that the writer refuses in the output form | Never, see below. The writer refuses in the output form: an integer beyond its group code's range, a real that is not finite, a text in a group code the crate does not know (binary output only), and a binary chunk over 255 bytes (binary output only). The export is given the output form (`export_drawing_for_mode`), so a stored value that the writer would refuse does not fail the write | `draft.dxf.header_value_omitted` |

A stored record (an object or entity record of the file) that holds a value the writer refuses in the
output form is not written either, and is reported under `draft.dxf.export_unmapped`, as a record that
does not decode is.

The code page is not written: the export writes version R2018, whose text is UTF-8, and the writer
uses `Drawing::code_page()` only for versions before 2007. A stored page would name an encoding that
the output does not use, so it is omitted and reported. A reader of the file takes its default page
(`ANSI_1252`), which the output does not depend on.

| Variable | Value | Notes |
|---|---|---|
| `$INSUNITS` | the unit code of the space | Owned: the import does not keep it, so it follows the stored variables |
| `$HANDSEED` | the writer's seed | Owned: the export gives the value `1` and the writer replaces it by one above the largest handle it writes |

Every other stored variable keeps its stored values, including the limits (`$LIMMIN`, `$LIMMAX`),
the creation time (`$TDCREATE`) and the user variables, in their order. A drawing that was not
imported has no header rows, and its export writes only the owned variables.

### Extended data, registered applications, classes and dictionaries

The import keeps four kinds of content that the drafting model has no object for. Each is stored as a
foreign data row of format `dxf` in the import's transaction, so one undo removes it with the rest of
the import, and each is written back by the export.

| Content | Row kind | Object of the row | Stored data |
|---|---|---|---|
| Extended data of a mapped object | `xdata` | the object the import created (or the existing object it adopted, when that object holds no extended data yet) | a record of kind `XDATA` whose groups are the extended data as in the file: each group 1001 with the groups after it, in order |
| CLASS records of the CLASSES section | `class` | the target space | a record of kind `CLASS` with the groups of the class |
| Registered applications (APPID entries) | `appid` | the target space | a record of kind `APPID`: handle, name, flags, other groups, extended data |
| DICTIONARY objects of the OBJECTS section | `dictionary` | the target space | a record of kind `DICTIONARY`: handle, owner, flags, the entries (name, handle, hard or soft owner), other groups, extended data |

Import rules:

- Extended data is kept for every object the import maps: layers, linetypes, text styles, dimension
  styles, block records, and the entities that are created from an entity of the model space or of a
  block. An INSERT's extended data goes with its first reference when the insert is expanded into
  several. The extended data of a preserved record is part of the record and is kept with it.
- Bounds: the encoded data of one object is at most `string_bytes` of the limits, and the encoded data
  of all objects of one import at most `input_bytes`. The rows count against the same row bound as the
  preserved records (`MAX_FOREIGN_RECORDS` and `record_count`). Extended data beyond a bound, or beyond
  the row bound, is not kept and is reported as `draft.dxf.xdata_omitted`; the import does not fail.
- Classes are kept once per class name (the first one in the file wins). Registered applications are kept
  once per name, compared without regard to case. Both are reported with their count under
  `draft.dxf.record_preserved`.
- Replacement: an import replaces the class, registered application and dictionary rows of its space, as
  it replaces the header rows (found by format, record kind and object, in the same transaction). The
  last import's records win for the space: a first file's dictionaries do not reach a second file's export.
  The preserved entities and objects of an earlier import are not replaced; they keep their rows.
- An existing layer, linetype, text style, dimension style or block record that the import adopts takes
  the new file's extended data: its earlier row is removed, and an empty set leaves it without a row.
- Every DICTIONARY object is kept, its entries with their names and handles. The named-object root is
  the one with no owner and handle 0xC, or else the first ownerless dictionary in file order. Its
  entries are merged into the root the export writes; it is not written as a dictionary of its own.
- Not kept: the BLOCK and ENDBLK records of a block, polyline vertices and sequence ends, attribute
  records of an insert, and the extended data of an entity the import does not map (the entity is reported
  with its own item). The first two and the attribute records are reported as `draft.dxf.xdata_omitted`.

Export rules:

- The extended data of each written object is written in its stored order, with the groups as stored
  except the handles of group 1005, which are rewritten (see References). A row that does not decode,
  or holds a group the output form refuses, is skipped and reported as `draft.dxf.export_unmapped`.
- Well formed: the control braces (group 1002) of an object's extended data balance and never close
  before they open; a binary chunk (group 1004) holds at most 127 bytes in text output and 255 bytes in
  binary output; every application name is a valid table name (see Registered applications). An object
  that is not well formed, or whose data is over the bounds of the import, is not written and is
  reported as `draft.dxf.xdata_omitted`. The bounds are `string_bytes` per object and `input_bytes` in all.
- A row of an object that is no longer live is stale. It is not read and counts against no bound.
- The extended data of a drafting array goes with its first block reference.
- Registered applications: the stored entries are written, the first one per name, with their handles
  as for other records. When the file has no `ACAD` entry, the export writes the standard one. Every
  application that written extended data or a written preserved record names and that the table lacks is
  added with flags 0 and a new handle (`draft.dxf.appid_added`, which lists up to 16 names). The format
  requires an application named by extended data to have an APPID entry.
- A table name is valid when it is not empty, is at most `NAME_BYTES` (255) bytes long, and holds no
  control characters, as the import's names are cleaned. A stored entry with an invalid name is skipped
  and reported (`draft.dxf.export_unmapped`), and so is an extended data object or preserved record that
  names an application with an invalid name.
- Class records: every stored class is written, the first one per class name. The instance count
  (group 91), when the class has one, is recomputed as the number of written records whose type is the
  class name, compared without regard to case, so a negative or stale count is replaced. The rule relied
  on is that a record of a type the format does not define is identified by its class record (its
  application and flags), and that a class without instances is valid. So every stored class is written,
  and the export adds none.
- Dictionaries: the named-object root's entries are merged into the root the export writes, first one per
  name without regard to case; `ACAD_LAYOUT` is the export's own; an entry whose target is not written,
  or is the root, is dropped. The layouts dictionary the root names is not written from its row, and its
  number names the export's own layouts dictionary (a second dictionary with that number is written).
  Any other stored dictionary is written with its entries whose targets are written. A dictionary that
  owns itself is owned by the root and listed in it. A dictionary with no owner (other than the named
  root) is owned by the root and listed in it. A dictionary whose owner is not written is dropped
  (`draft.dxf.export_unmapped`): it has no meaning without its owner. Listed objects are named
  `ARC_<hex handle>`, with a numeric suffix when that name is taken, and the root holds one entry per
  listed object.
- Dictionary entries, merged or stored, are bounded by the record limit of the export (`record_count`,
  shared by all of them); entries beyond it are dropped and counted under `draft.dxf.export_unmapped`.

Copies and deletions:

- A copy of a drafting object made by a command does not receive the extended data of its source: no
  command of this crate copies drafting objects, and the rows are attached to the object they were
  written for. A product that copies objects must add the rows of the copy itself.
- Deleting an object does not remove its extended data row. The document keeps a row whose object is
  gone until a transaction removes it (the rule of `arc-doc`, CORE-DOC-040). The export writes the rows of
  the objects it writes only, so the row is not written, and an undo of the delete finds the row again.
  This crate has no command that deletes objects, so the row remains in the saved document until a
  product removes it.

### References of kept records

The export maps each source number of the file to the handle it writes for the record that held the
number, and writes every reference of a kept record through that map. The map is built from everything
the export writes:

- each preserved record, dictionary and registered application entry, with the handle it is written
  with;
- each drafted object that has a source binding in the document (a layer, linetype, text style,
  dimension style, block record, entity, or the entity of an array), with the handle the export writes
  for it: its bound handle under `keep_sources`, its new handle under `Assign`. Under `keep_sources` only
  the bindings of the policy's document count; under `Assign` the bindings of every document count, as
  the policy names none;
- the named-object root, with the root the export writes;
- the layouts dictionary of the file (the target of its root's `ACAD_LAYOUT` entry), with the export's own
  layouts dictionary.

A source number that two records hold is ambiguous. It is removed from the map, so a reference to it is
treated as unknown. The number zero names nothing. A stored record whose handle is zero gets a new
handle, and zero is never looked up.

The rules, applied once to each record after the map is complete:

- A handle group of a preserved record (codes 320 to 369, 390 to 399, 480 and 481: reactors, soft and
  hard pointers and owners, plot style and visual style handles) and a group 1005 in its extended data
  are rewritten through the map. The owner of an entity is found by the same owner position as an
  object's (the first group 330 outside application groups, before the first subclass marker), so a
  reactor inside an application group is an ordinary reference. The owner of an entity is its container
  and is not rewritten through the map.
- A reference that names a written record is written under that record's handle. A reference that names
  nothing the output holds (a number no record holds, a number two records hold, or a malformed value)
  is written as the null handle `0`, and the number of such references is reported in one item,
  `draft.dxf.reference_nulled`. The old number is never written, because it may name another object of
  the output. A zero reference stays zero.
- The owner of a preserved object is written through the map. When that owner is not written, or is the
  object itself, the owner is the root, the object is listed in the root, and the replacement is reported
  as `draft.dxf.owner_substituted`.
- Dictionary owners and entries follow the dictionary rules under Export rules above.
- The extended data of drafted objects and of table entries is rewritten by the same rule (group 1005).

The rules apply the same way under `Assign` and `keep_sources`; the two differ in the handles the drafted
objects are written with, and in which bindings the map holds.

Not preserved by this crate, after this unit:

- Typed objects of the OBJECTS section other than dictionaries: XRECORD, LAYOUT and GROUP are skipped
  (`draft.dxf.objects_skipped`). A preserved record or dictionary that refers to one of them keeps its
  number under the handle rule, and a dictionary entry that names one is dropped.
- The VPORT, VIEW and UCS tables, tables and sections the model does not type, and the paper space.
  They are reported (`draft.dxf.table_skipped`) and not written.
- Extended data listed under "Not kept" above and the extended data of entities that are not mapped.
- References to objects the import dropped (a typed object of the OBJECTS section, a table, a block
  record that is not bound) are written as the null handle, as every reference to nothing the output
  holds is. A product that needs a reference to keep naming an object keeps the object bound.
- Proxy entities and proxy objects are kept as preserved records with all their groups, including
  their proxy graphics (group 310 chunks). The groups are not interpreted and no drafting object is made.
- Header variables are unchanged by this unit (see the header section).

### Arrays

A reference array (`core.draft.array`) is written as block references, since the entities this crate
writes have no associative array:

- One INSERT per item, in item order, at the item's insertion point and rotation, with the array's
  per-axis scale (a mirrored array writes a negative scale), on the array's layer and with its color,
  linetype, lineweight, transparency and visibility. The placements are those of the display list: the
  export calls `arc_draft::array::expansion`, which places the items with the same function as the
  display list. Its tolerance is that of `DisplayOptions::default()`, which the plot path also uses, so
  the placements agree with the plotted drawing.
- The source block is written as a block definition like every other block of the document, also when
  only arrays refer to it.
- Counts, spacings, angles, polar centers, total angles and paths are not written. Each array is
  reported once with `draft.dxf.array_approximated` (argument `count`, the number of references
  written), and counts once as mapped.
- An array whose block is missing, or whose placements the display cannot compute (for example a path
  array whose path entity was deleted), is reported as unmapped (`draft.dxf.export_unmapped`) and
  writes no references; the rest of the drawing is written.
- A rectangular grid is not written as one MINSERT with column and row counts. A MINSERT spaces its
  items along the reference's own rotated axes, and the reader computes the positions with its own
  arithmetic, so the displayed placements would not be reproduced exactly. The rejected alternative,
  a MINSERT when the grid axes coincide with the item rotation, saves records only in that case and is
  not exact for every grid.
- Handles under `keep_sources`: the first reference of an array takes the array's bound handle, when no
  other record of the file holds it (otherwise the handle is reassigned and reported); every other
  reference takes a new handle. `Exported::handles` lists the first reference's handle for the array.
- Budget: the references of all arrays of one export are bounded by the smaller of `record_count` of the
  limits and `ARRAY_REFERENCE_BUDGET` (1,000,000). An export that would exceed it fails with
  `io.limit_exceeded` (argument `limit` = `record_count`) before any reference is built, and no file is
  written. An array holds at most 10,000 items, which `core.draft.array.create` enforces with
  `draft.array.too_many_items`.
- An array has no attribute values, so its references carry no ATTRIB records.
- Arrays owned by a layout are not written: the export writes one model space, as it does for every
  paper-space entity.

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
| `draft.dxf.header_derived_omitted` | `dropped` |
| `draft.dxf.header_handle_omitted` | `dropped` |
| `draft.dxf.header_name_omitted` | `dropped` |
| `draft.dxf.header_unscaled` | `approximated` |
| `draft.dxf.header_value_omitted` | `dropped` |
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
| `draft.dxf.array_approximated` | `approximated` |
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
| `draft.dxf.xdata_omitted` | `dropped` |
| `draft.dxf.owner_substituted` | `approximated` |
| `draft.dxf.appid_added` | `approximated` |
| `draft.dxf.reference_nulled` | `approximated` |

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

The array export tests (`tests/arrays.rs`) write each array kind (rectangular, polar with mirrored items,
path along an arc and along a segment with spacing), read the file back with the DXF reader and compare every
reference with the expansion of the array (positions exact, rotation within 1e-12 radian) and with the display
segments of the drawing. They also cover a source block referenced only by arrays, an array inside a block
definition with a layer, the handle rule under `keep_sources`, the budget and the item bound.

The regression tests of the review (`tests/review_fixes.rs`) cover each rule above that changed
behavior: references under both policies (including a reactor inside an application group and group
1005), references nulled, a zero handle, a source number held twice, the layouts number and an unrelated
dictionary with the same number, the owner position, a second import replacing the class, application and
dictionary rows, the adoption of a layer's extended data, the record budget of the root merge, application
names, dictionary owners (self-owned, ownerless, owner not written, listed objects), class counts without
regard to case, and extended data that is not well formed or holds a chunk over the text bound. Against the
previous unit's code, all of them fail.

The extended data tests (`tests/extended_records.rs`) import a file with a line carrying extended data of
two applications (a control string, a handle, a real and a binary chunk), a preserved entity with a reactor,
a custom object owned by the root, a custom object whose owner is missing, a root dictionary with entries
that name one object and a missing one, a dictionary cycle, classes with a negative count and an application
table without two of the names used. They export under both policies and read the file back: the extended
data is exact, the applications are present once, the counts are recomputed, owners and references follow
the map, the dictionary entries name the written objects, and the cycle is kept. They also cover the bound
of one object (through the planner), 3,000 classes, a second import into the same space, a record without a
handle, one undo, and a document without imports. The foreign row test (`tests/foreign_roundtrip.rs`)
expects the root as the owner of a preserved object whose owner is not written.

The source identity tests (`tests/source_identity.rs`) import a drawing with bound layers, linetypes,
styles, block records, block entities and model-space entities, export it with `keep_sources`, and
read the written file back: every bound handle is written unchanged, the seed is above the written
handles, and an object added afterwards takes a handle above the floor. They also cover a repeated
handle, a preserved record that shares a bound handle, a bound handle on the root dictionary, and one
undo removing the bindings.

The header tests (`tests/header_roundtrip.rs`) import a file with twelve assorted header variables
(strings, integers, reals, points with several group codes, an unknown name and an angle), export it
and compare every kept variable and its position in the written file; they also cover the owned
variables taking the place of stored ones, the count and string bounds, the angle in degrees on this
path, a drawing that was not imported, and one undo removing the rows.

The class tests (`tests/header_classes.rs`) cover each check of the export: derived variables omitted
and listed in one item, handles written only for objects the file keeps under those handles, names
matched against the written tables, values that do not fit their group codes omitted by the import,
and the code page omitted.

The review tests (`tests/header_review.rs`) cover the storable check and the point rules, the case
insensitive names, the unit conversion and its unscaled item, the derived, handle and name classes,
the replacement of header rows by a second import, and the writer's refusals in ASCII and binary
output, including a stored record the writer refuses.
