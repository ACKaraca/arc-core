#!/usr/bin/env python3
"""Compares a JSON dump of arc-io-ifc's schema tables with ifcopenshell's schema introspection.

Usage: schema_diff.py <tables.json> [schema ...]

The JSON is written by `ARC_IFC_SCHEMA_DUMP=<file> cargo test -p arc-io-ifc --test schema_tables`:
  {"IFC4": {"entities": {"IfcWall": {"supertype": "IfcBuiltElement", "abstract": false,
            "attributes": [{"name": "Tag", "optional": true}, ...]}}}, ...}
Prints a JSON report and exits 1 on differences. When the file does not exist (U1 not merged
yet) it reports "skipped" and exits 0.
"""
import json
import sys
from pathlib import Path

import ifcopenshell.ifcopenshell_wrapper as w

NAMES = {"IFC2X3": "IFC2X3", "IFC4": "IFC4", "IFC4X3": "IFC4X3_ADD2"}


def oracle(schema):
    s = w.schema_by_name(NAMES.get(schema, schema))
    out = {}
    for d in s.declarations():
        if not isinstance(d, w.entity):
            continue
        sup = d.supertype()
        out[d.name()] = {
            "supertype": sup.name() if sup else None,
            "abstract": d.is_abstract(),
            "attributes": [(a.name(), a.optional()) for a in d.attributes()],
        }
    return out


def main(argv):
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    path = Path(argv[0])
    if not path.is_file():
        print(json.dumps({"skipped": f"{path} not found (schema tables not generated yet)"}))
        return 0
    ours = json.loads(path.read_text())
    report = {}
    for schema in argv[1:] or list(ours):
        mine = {k.lower(): v for k, v in ours[schema]["entities"].items()}
        ref = {k.lower(): (k, v) for k, v in oracle(schema).items()}
        diffs = [f"{n}: only in oracle" for n in ref.keys() - mine.keys()]
        diffs += [f"{n}: only in arc-io-ifc" for n in mine.keys() - ref.keys()]
        for n in mine.keys() & ref.keys():
            name, r = ref[n]
            m = mine[n]
            sup = (m.get("supertype") or "").lower() or None
            if sup != (r["supertype"].lower() if r["supertype"] else None):
                diffs.append(f"{name}: supertype {m.get('supertype')} != {r['supertype']}")
            if bool(m.get("abstract")) != r["abstract"]:
                diffs.append(f"{name}: abstract flag differs")
            attrs = [(a["name"].lower(), bool(a.get("optional"))) for a in m.get("attributes", [])]
            if attrs != [(a.lower(), o) for a, o in r["attributes"]]:
                diffs.append(f"{name}: explicit attributes differ")
        report[schema] = {"entities_arc": len(mine), "entities_oracle": len(ref), "differences": sorted(diffs)}
    print(json.dumps(report, indent=2))
    return 1 if any(r["differences"] for r in report.values()) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
