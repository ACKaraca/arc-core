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
| `run_all.sh [directory]` | runs the `arc-io-ifc` tests with `ARC_IFC_DUMP`, then `validate.py` and `geometry_compare.py` on every dumped file | 1 on any failure |

`schema_diff.py` expects `{"<SCHEMA>": {"entities": {"<Name>": {"supertype", "abstract",
"attributes": [{"name", "optional"}]}}}}`.

Run from the repository root: `tools/ifc-oracle/run_all.sh`.
