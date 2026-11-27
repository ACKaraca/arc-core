# IFC oracle

Independent checks of the IFC files `arc-io-ifc` writes and reads, using IfcOpenShell as a second
implementation of the schemas. It is an oracle **outside the gate**: it runs by hand and nightly,
nothing in `cargo xtask ci` depends on it, and no Python is part of any shipped binary.

## Requirements

- Python 3.10 or later and `ifcopenshell` 0.9.0 or later (developed with 0.9.0.post1;
  `pip install ifcopenshell`).
- The Rust toolchain of the repository (for `geometry_compare.py` and `run_all.sh`).

## Dump hook

Every `arc-io-ifc` test that writes IFC calls `support::dump(name, bytes)` (in
`crates/arc-io-ifc/tests/support`). With `ARC_IFC_DUMP=<directory>` set, the file is written to
`<directory>/<name>` under a stable name; without it nothing is written. `ARC_IFC_OUT` is still
read as an older name of the same variable.

## Scripts

| Script | What it does | Exit status |
|---|---|---|
| `validate.py <files or directories>` | `ifcopenshell.validate` with the EXPRESS rules on; JSON summary per file (issue count, counts by kind) | 1 on any issue or unreadable file |
| `geometry_compare.py <file>... [--tol-rel] [--tol-abs] [--openings] [--deflection D] [--summary]` | per product, volume, surface area and bounding box of IfcOpenShell's geometry iterator (world coordinates, metres) against `cargo run -p arc-io-ifc --example geometry_stats` (set `ARC_GEOMETRY_STATS` to a built binary to skip `cargo`). Openings are left in on both sides unless `--openings` subtracts them on both; `--deflection` sets IfcOpenShell's linear deflection and the chord tolerance of arc-io-ifc to the same value; the report names the representation item kinds of every mismatching product, and `--summary` aggregates several files by kind | 1 on a difference above the tolerance (default 0.1 % relative, 1e-6 absolute) or a product only one side has |
| `schema_diff.py <tables.json> [schema ...]` | entities, supertypes, abstract flags and explicit attributes of a JSON dump of the `arc-io-ifc` schema tables against IfcOpenShell's schema introspection; skips (exit 0) when the file does not exist | 1 on a difference |
| `conformance_compare.py [--json] [--rules] <files or directories>` | `IfcFile::conformance` (`cargo run -p arc-io-ifc --example conformance`; set `ARC_CONFORMANCE` to a built binary to skip `cargo`) against `ifcopenshell.validate`, per rule class and (instance, attribute): agree, Arc only, oracle only. Without `--rules`, levels 1 and 2 against the validator without EXPRESS rules; with it, also level 3 against the validator with EXPRESS rules: entity and type rules per (instance, `Owner.Label`), global rules per rule name, followed by a table per rule with the checks Arc evaluated and could not evaluate | 1 when a file cannot be checked by either side |
| `pset_diff.py <templates.json> [schema ...]` | names, kinds, data types and quantity kinds of a JSON dump of the `arc-io-ifc` property and quantity set templates against IfcOpenShell's `PsetQto` templates; enumeration items, and data types the published definitions leave open, are listed as source differences without failing; skips (exit 0) when the file does not exist | 1 on a difference |
| `run_all.sh [directory]` | runs the `arc-io-ifc` tests with `ARC_IFC_DUMP`, then `validate.py` and `geometry_compare.py` on every dumped file | 1 on any failure |

`schema_diff.py` expects `{"<SCHEMA>": {"entities": {"<Name>": {"supertype", "abstract",
"attributes": [{"name", "optional"}]}}}}`. With the generated tables present, write it with
`ARC_IFC_SCHEMA_DUMP=<file> cargo test -p arc-io-ifc --test schema_tables`; the dump also holds the
attribute types, the record positions with their derived flags, the inverse attributes, the
enumerations and the selects.

`pset_diff.py` reads the dump `ARC_IFC_PSET_DUMP=<file> cargo test -p arc-io-ifc --test psets`
writes when the templates are generated (`cargo xtask ifc-psets`). The written files themselves are
compared with the templates by `cargo run -p arc-io-ifc --example pset_check <files>`, which prints one
JSON line per file and exits 1 on any finding.

Run from the repository root: `tools/ifc-oracle/run_all.sh`.

## Conformance differences

`conformance_compare.py` compares rule classes; the oracle reports every attribute value error
(kind, enumeration item, select member, referenced entity, aggregate size and uniqueness, width) with
one message, so those Arc rules form one class. Where the two disagree on files with one injected
error, the cause is a documented behavior of the oracle's reader or validator (IfcOpenShell 0.9.0):

| Arc rule | Oracle | Cause |
|---|---|---|
| `io.ifc.conformance_entity_unknown`, `io.ifc.conformance_complex_invalid` | nothing for the instance | The reader leaves out instances of unknown entities and complex instances; references to them then appear elsewhere as missing values |
| `io.ifc.conformance_attribute_count` | nothing, or value errors on shifted attributes | The reader reads the parameters as given, ignoring extra ones and treating missing ones as unset |
| `io.ifc.conformance_derived_misplaced` for `$` at a derived position | nothing | Derived positions are checked only for `*` at a position that is not derived |
| `io.ifc.conformance_aggregate_unique` | nothing | Repeated elements of a `SET` are kept and not reported |
| `io.ifc.conformance_enumeration_item`, `io.ifc.conformance_reference_dangling` | a missing value where the attribute is required, nothing where it is optional | The reader reads an unknown item or a reference to a missing instance as no value; counted as `read_as_missing_by_oracle` when both report it |
| none | `IfcGloballyUniqueId base64 validation` | The oracle checks the encoding of `GlobalId` beyond its `STRING(22) FIXED` type; Arc checks the width only |

For a `UNIQUE` rule the oracle names one instance of a repeated group and lists the others under
"Violated by"; Arc reports every instance of the group, and the script compares against all of them.

## Rule differences

With `--rules`, the two sides agree on every finding of the published sample files, of the files the
tests write and of samples with one injected rule violation, except where the oracle's rule runner
behaves as follows (IfcOpenShell 0.9.0):

| Arc | Oracle | Cause |
|---|---|---|
| `IfcGeometricRepresentationContext.North2D` holds on a subcontext | fails with "Attribute not set" | The runner reads the derived `TrueNorth` of a subcontext from its own computation, which derives a direction from the parent context's world coordinate system instead of evaluating `ParentContext.TrueNorth`; when that placement is degenerate (a two-dimensional axis, or axes in one direction) the read fails, and a failure to evaluate is reported as a violation |
| a rule is `UNKNOWN` (passes) | fails with a Python error such as `unhashable type: 'indeterminate_type'` | Any error raised while the runner evaluates a rule is reported as a violation of the rule. For example `IfcUniquePropertyName` adds a property without a name (`?`) to a set; Arc gives `?` for the union and `UNKNOWN` for the rule, as ISO 10303-11 does for an operator with an indeterminate operand. The instance always has a level 1 finding for the missing value too |

A rule Arc cannot evaluate is reported as not evaluated in the table per rule; on these files there
is none except for instances whose structure level 1 already rejects.

## Geometry differences (waves 2 and 3)

`geometry_compare.py` on the wave 2 and 3 fixtures (`ARC_IFC_DUMP` with `geometry_wave2` and `geometry_wave3`)
agrees with `arc-io-ifc` at a chord tolerance of 0.1 mm (`--deflection 0.0001`) except where IfcOpenShell
0.9.0.post1 behaves as follows:

| Item | Oracle | Cause |
|---|---|---|
| `IfcRightCircularCone`, `IfcRightCircularCylinder` | no product; a boolean or an opening with one as an operand keeps the other operand | the engine does not tessellate these CSG primitives |
| `IfcRevolvedAreaSolidTapered` | the volume of the untapered `IfcRevolvedAreaSolid` | the engine ignores `EndSweptArea` |
| sweeps with `StartParam` or `EndParam` | no product | the engine does not apply the parameters |
| `IfcOffsetCurveByDistances`, B-spline surface as an item | no product | not evaluated by the engine |
| `IfcAdvancedBrepWithVoids` of the fixture | no product | not evaluated by the engine |
| spherical cap with a pole vertex or an equator bound | open shell (volume 0) or the other side of the equator | the engine builds the face from the wire without the sense of its bound |
| `IfcSurfaceWeightsPositive` of a rational B-spline surface | validation error "unsupported operand type(s) for -: 'list' and 'int'" | the rule runner fails on the derived weights array; the fixture `wave3-bspline-surface-rational-cylinder-validator-limit.ifc` is otherwise valid |
| `VoidsHaveAdvancedFaces` | violated by every B-rep with voids whose void shells have only advanced faces | the published predicate is inverted; the fixture gives its void shell a plain face |
| `IfcSphere`, curved bodies at the default chord (1 mm) | 0.2 to 0.4 % more volume | the discretizations differ; they agree at 0.1 mm |
