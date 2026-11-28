#!/usr/bin/env python3
"""Compares a JSON dump of arc-io-ifc's property and quantity set templates with ifcopenshell's
PsetQto templates.

Usage: pset_diff.py <templates.json> [schema ...]

The JSON is written by `ARC_IFC_PSET_DUMP=<file> cargo test -p arc-io-ifc --test psets`:
  {"IFC4": {"Pset_WallCommon": {"properties": {"Reference": {"kind": "P_SINGLEVALUE",
            "types": ["IfcIdentifier"], "extra": null}, ...}}}}
For every template the property names, the kinds, the data types (primary type; for tables also
the defined type) and the quantity kinds must agree. The data type of a
time series reference (its secondary type) is not part of the published definitions and is not
compared. Enumeration items are compared trimmed and in upper case. Item differences, and data types the
published definition leaves open (`<DataType />`) where the oracle names one, are listed
separately as source differences and do not change the exit status: the oracle's template data
was revised after the published definitions. Prints a JSON report and exits 1 on differences; skips (exit 0) when the file does not
exist.
"""
import json
import sys
from pathlib import Path

import ifcopenshell.util.pset as pset


def oracle_property(t):
    if t.is_a("IfcComplexPropertyTemplate"):
        return {
            "kind": "P_COMPLEX",
            "types": [t.UsageName],
            "children": {name(c): oracle_property(c) for c in t.HasPropertyTemplates or []},
        }
    kind, primary = t.TemplateType, t.PrimaryMeasureType
    if kind and kind.startswith("Ifc"):
        # A few templates of the oracle's data shift the attributes by one (the measure type in
        # TemplateType); read them as the single values they are.
        kind, primary = "P_SINGLEVALUE", t.TemplateType
    items = []
    if t.Enumerators:
        items = [str(v.wrappedValue) for v in t.Enumerators.EnumerationValues or []]
    types = []
    if kind in ("P_SINGLEVALUE", "P_BOUNDEDVALUE", "P_LISTVALUE", "P_REFERENCEVALUE"):
        types = [primary or ""]
    elif kind == "P_TABLEVALUE":
        types = [primary or "", t.SecondaryMeasureType or ""]
    return {"kind": kind, "types": types, "items": items}


def name(t):
    # The shifted templates hold the name in the OwnerHistory position.
    if t.is_a("IfcSimplePropertyTemplate") and (t.TemplateType or "").startswith("Ifc"):
        return t[1]
    return t.Name


def oracle(schema):
    out = {}
    for f in pset.PsetQto(schema).templates:
        for s in f.by_type("IfcPropertySetTemplate"):
            out[s.Name] = {name(p): oracle_property(p) for p in s.HasPropertyTemplates or []}
    return out


def compare(where, mine, ref, diffs, items=None):
    for name in sorted(ref.keys() - mine.keys()):
        diffs.append(f"{where}.{name}: only in oracle")
    for name in sorted(mine.keys() - ref.keys()):
        diffs.append(f"{where}.{name}: only in arc-io-ifc")
    for name in sorted(mine.keys() & ref.keys()):
        m, r = mine[name], ref[name]
        at = f"{where}.{name}"
        if m["kind"] != r["kind"]:
            diffs.append(f"{at}: kind {m['kind']} != {r['kind']}")
            continue
        norm = lambda ts: [x.strip().lower() for x in ts]
        if m["kind"] == "P_COMPLEX":
            compare(at, m["extra"], r["children"], diffs, items)
        elif norm(m["types"]) != norm(r["types"]):
            open_types = [a == "" and b != "" for a, b in zip(norm(m["types"]), norm(r["types"]))]
            if items is not None and len(m["types"]) == len(r["types"]) and all(
                o or a == b for o, a, b in zip(open_types, norm(m["types"]), norm(r["types"]))
            ):
                items.append(f"{at}: data type open in the published definition, oracle {r['types']}")
            else:
                diffs.append(f"{at}: types {m['types']} != {r['types']}")
        if m["kind"] == "P_ENUMERATEDVALUE" and items is not None:
            ours = [i.strip().upper() for i in m["extra"]["items"]]
            theirs = [i.strip().upper() for i in r["items"]]
            if ours != theirs:
                items.append(f"{at}: items {ours} != {theirs}")


def main(argv):
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    path = Path(argv[0])
    if not path.is_file():
        print(json.dumps({"skipped": f"{path} not found (templates not generated)"}))
        return 0
    ours = json.loads(path.read_text())
    report = {}
    status = 0
    for schema in argv[1:] or list(ours):
        ref = oracle(schema)
        mine = {k: v["properties"] for k, v in ours[schema].items()}
        diffs = [f"{n}: only in oracle" for n in sorted(ref.keys() - mine.keys())]
        diffs += [f"{n}: only in arc-io-ifc" for n in sorted(mine.keys() - ref.keys())]
        properties = 0
        items = []
        for n in sorted(mine.keys() & ref.keys()):
            properties += len(mine[n])
            compare(n, mine[n], ref[n], diffs, items)
        report[schema] = {
            "templates": len(mine),
            "oracle_templates": len(ref),
            "properties": properties,
            "differences": len(diffs),
            "details": diffs[:200],
            "source_differences": len(items),
            "source_details": items,
        }
        status |= bool(diffs)
    print(json.dumps(report, indent=2))
    return 1 if status else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
