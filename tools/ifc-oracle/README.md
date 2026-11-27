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
| `geometry_compare.py <file> [--tol-rel] [--tol-abs]` | per product, volume, surface area and bounding box of IfcOpenShell's geometry iterator (openings not subtracted, world coordinates, metres) against `cargo run -p arc-io-ifc --example geometry_stats` (set `ARC_GEOMETRY_STATS` to a built binary to skip `cargo`) | 1 on a difference above the tolerance (default 0.1 % relative, 1e-6 absolute) or a product only one side has |
| `schema_diff.py <tables.json> [schema ...]` | entities, supertypes, abstract flags and explicit attributes of a JSON dump of the `arc-io-ifc` schema tables against IfcOpenShell's schema introspection; skips (exit 0) when the file does not exist | 1 on a difference |
| `conformance_compare.py [--json] <files or directories>` | levels 1 and 2 of `IfcFile::conformance` (`cargo run -p arc-io-ifc --example conformance`; set `ARC_CONFORMANCE` to a built binary to skip `cargo`) against `ifcopenshell.validate` without EXPRESS rules, per rule class and (instance, attribute): agree, Arc only, oracle only | 1 when a file cannot be checked by either side |
| `run_all.sh [directory]` | runs the `arc-io-ifc` tests with `ARC_IFC_DUMP`, then `validate.py` and `geometry_compare.py` on every dumped file | 1 on any failure |

`schema_diff.py` expects `{"<SCHEMA>": {"entities": {"<Name>": {"supertype", "abstract",
"attributes": [{"name", "optional"}]}}}}`. With the generated tables present, write it with
`ARC_IFC_SCHEMA_DUMP=<file> cargo test -p arc-io-ifc --test schema_tables`; the dump also holds the
attribute types, the record positions with their derived flags, the inverse attributes, the
enumerations and the selects.

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
