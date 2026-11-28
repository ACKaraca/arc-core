#!/usr/bin/env python3
"""Compares per-product volume, surface area and bounding box of IfcOpenShell's geometry
iterator with arc-io-ifc's evaluation (example `geometry_stats`).

Usage: geometry_compare.py <file.ifc>... [--tol-rel 1e-3] [--tol-abs 1e-6] [--openings]
                           [--summary] [--deflection D]
Prints one JSON report (one file) or a JSON summary over all files with `--summary`; exits 1 on
differences above the tolerance or on products one side lacks.

By default openings are not subtracted on either side (IfcOpenShell runs with
disable-opening-subtractions). `--openings` enables the subtraction on both sides, to compare
the prism openings of arc-io-ifc with IfcOpenShell's. Quantities are in metres, metres squared and
cubed (IfcOpenShell's default unit handling). `--deflection D` sets IfcOpenShell's
mesher-linear-deflection and arc-io-ifc's chord tolerance to D metres, so both discretize curves
alike (without it IfcOpenShell uses its default and arc-io-ifc its 1 mm chord).

Each mismatch lists the representation item kinds of the product (leaf kinds of its body items,
with the profile kind of swept solids and the operands of boolean results), so the report groups
mismatches by item kind. Products only one side evaluates are reported as `only in ...` with the
same kinds.
"""
import argparse
import collections
import json
import os
import subprocess
import sys

import ifcopenshell
import ifcopenshell.geom
import ifcopenshell.util.shape as shape

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
# The chord tolerance of arc-io-ifc (Tolerance::DEFAULT.tessellation_chord) in metres.
CHORD = 1e-3


def arc_stats(path, openings, chord=None):
    exe = os.environ.get("ARC_GEOMETRY_STATS")
    cmd = [exe] if exe else [
        "cargo", "run", "-q", "-p", "arc-io-ifc", "--example", "geometry_stats", "--"]
    cmd.append(path)
    if openings:
        cmd.append("--openings")
    if chord is not None:
        cmd += ["--chord", repr(chord)]
    run = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if run.returncode != 0:
        # A file arc-io-ifc cannot read counts as no products; the error is reported.
        return {}, run.stderr.strip()
    return {p["id"]: p for p in json.loads(run.stdout)["products"]}, None


def oracle_stats(path, openings, deflection):
    # IfcOpenShell's C++ side prints diagnostics to the process's stdout; keep stdout for the JSON.
    sys.stdout.flush()
    saved = os.dup(1)
    os.dup2(2, 1)
    try:
        return _oracle_stats(path, openings, deflection)
    finally:
        sys.stdout.flush()
        os.dup2(saved, 1)
        os.close(saved)


def _oracle_stats(path, openings, deflection):
    f = ifcopenshell.open(path)
    settings = ifcopenshell.geom.settings()
    settings.set("disable-opening-subtractions", not openings)
    settings.set("use-world-coords", True)
    if deflection is not None:
        settings.set("mesher-linear-deflection", deflection)
    result = {}
    it = ifcopenshell.geom.iterator(settings, f, 1)
    if it.initialize():
        while True:
            el = it.get()
            g = el.geometry
            if len(g.verts):
                w = shape.get_vertices(g).reshape(-1, 3)
                result[el.id] = {
                    "volume": shape.get_volume(g),
                    "area": shape.get_area(g),
                    "min": w.min(axis=0).tolist(),
                    "max": w.max(axis=0).tolist(),
                }
            if not it.next():
                break
    return result


def leaf_kinds(item, out, depth=0):
    """The kinds that decide how an item is evaluated: the item, the profile of a swept solid,
    the operands of a boolean result, the items of a mapped representation."""
    if depth > 8:
        return
    kind = item.is_a()
    if item.is_a("IfcMappedItem"):
        out.add(kind)
        for sub in item.MappingSource.MappedRepresentation.Items:
            leaf_kinds(sub, out, depth + 1)
    elif item.is_a("IfcBooleanResult"):
        out.add(kind)
        for operand in (item.FirstOperand, item.SecondOperand):
            leaf_kinds(operand, out, depth + 1)
    elif item.is_a("IfcSweptAreaSolid"):
        out.add(kind)
        out.add(kind + "/" + item.SweptArea.is_a())
    elif item.is_a("IfcCsgSolid"):
        out.add(kind)
    else:
        out.add(kind)


def product_kinds(path):
    f = ifcopenshell.open(path)
    kinds = {}
    for product in f.by_type("IfcProduct"):
        if not getattr(product, "Representation", None):
            continue
        found = set()
        for rep in product.Representation.Representations:
            for item in rep.Items:
                leaf_kinds(item, found)
        kinds[product.id()] = sorted(found)
    return kinds


def close(a, b, rel, absolute):
    return abs(a - b) <= max(absolute, rel * max(abs(a), abs(b)))


def compare(path, args):
    try:
        ref = oracle_stats(path, args.openings, args.deflection)
    except Exception as error:  # a file the oracle cannot open (a deliberately broken one)
        return {"file": path, "products_arc": 0, "products_oracle": 0, "matched": 0,
                "arc_error": None, "oracle_error": str(error), "differences": []}
    arc, arc_error = arc_stats(path, args.openings, args.deflection)
    kinds = product_kinds(path)
    differences = []
    matched = 0
    for pid in sorted(arc.keys() | ref.keys()):
        k = kinds.get(pid, [])
        if pid not in ref or pid not in arc:
            differences.append({"id": pid, "kinds": k, "problem": "only in " + (
                "arc-io-ifc" if pid in arc else "oracle")})
            continue
        x, y = arc[pid], ref[pid]
        bad = {}
        for key in ("volume", "area"):
            if not close(x[key], y[key], args.tol_rel, args.tol_abs):
                bad[key] = [x[key], y[key]]
        for key in ("min", "max"):
            if not all(close(p, q, args.tol_rel, args.tol_abs) for p, q in zip(x[key], y[key])):
                bad[key] = [x[key], y[key]]
        if bad:
            differences.append({"id": pid, "kinds": k, "differences": bad})
        else:
            matched += 1
    return {"file": path, "products_arc": len(arc), "products_oracle": len(ref),
            "matched": matched, "arc_error": arc_error, "differences": differences}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--tol-rel", type=float, default=1e-3)
    ap.add_argument("--tol-abs", type=float, default=1e-6)
    ap.add_argument("--openings", action="store_true")
    ap.add_argument("--summary", action="store_true")
    ap.add_argument("--deflection", type=float, default=None,
                    help="IfcOpenShell mesher-linear-deflection in metres; arc-io-ifc runs with the same "
                         "chord tolerance")
    a = ap.parse_args()
    reports = [compare(p, a) for p in a.files]
    failed = any(r["differences"] for r in reports)
    if not a.summary and len(reports) == 1:
        print(json.dumps(reports[0], indent=2))
        return 1 if failed else 0
    by_kind = collections.Counter()
    only = collections.Counter()
    for r in reports:
        for d in r["differences"]:
            key = ",".join(d["kinds"])
            (only if "problem" in d else by_kind)[key] += 1
    summary = {
        "files": len(reports),
        "products_arc": sum(r["products_arc"] for r in reports),
        "products_oracle": sum(r["products_oracle"] for r in reports),
        "matched": sum(r["matched"] for r in reports),
        "mismatched": sum(1 for r in reports for d in r["differences"] if "differences" in d),
        "only_one_side": sum(1 for r in reports for d in r["differences"] if "problem" in d),
        "mismatches_by_kinds": dict(by_kind),
        "one_side_by_kinds": dict(only),
        "per_file": [{k: r[k] for k in ("file", "products_arc", "products_oracle", "matched", "arc_error")}
                     | {"differences": len(r["differences"])} for r in reports],
    }
    if not a.summary:
        summary["reports"] = reports
    print(json.dumps(summary, indent=2))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
