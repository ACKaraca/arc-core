#!/usr/bin/env python3
"""Compares per-product volume, surface area and bounding box of IfcOpenShell's geometry
iterator with arc-io-ifc's evaluation (example `geometry_stats`).

Usage: geometry_compare.py <file.ifc> [--tol-rel 1e-3] [--tol-abs 1e-6]
Prints one JSON report; exits 1 on differences above the tolerance or products one side lacks.
Openings are not subtracted by arc-io-ifc, so IfcOpenShell runs with disable-opening-subtractions.
Quantities are in metres, metres squared and cubed (IfcOpenShell's default unit handling).
"""
import argparse
import json
import os
import subprocess
import sys

import ifcopenshell
import ifcopenshell.geom
import ifcopenshell.util.shape as shape

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def arc_stats(path):
    exe = os.environ.get("ARC_GEOMETRY_STATS")
    cmd = [exe, path] if exe else [
        "cargo", "run", "-q", "-p", "arc-io-ifc", "--example", "geometry_stats", "--", path]
    out = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, check=True).stdout
    return {p["id"]: p for p in json.loads(out)["products"]}


def oracle_stats(path):
    f = ifcopenshell.open(path)
    settings = ifcopenshell.geom.settings()
    settings.set("disable-opening-subtractions", True)
    settings.set("use-world-coords", True)
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


def close(a, b, rel, absolute):
    return abs(a - b) <= max(absolute, rel * max(abs(a), abs(b)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--tol-rel", type=float, default=1e-3)
    ap.add_argument("--tol-abs", type=float, default=1e-6)
    a = ap.parse_args()
    arc, ref = arc_stats(a.file), oracle_stats(a.file)
    differences = []
    for pid in sorted(arc.keys() | ref.keys()):
        if pid not in ref or pid not in arc:
            differences.append({"id": pid, "problem": "only in " + ("arc-io-ifc" if pid in arc else "oracle")})
            continue
        x, y = arc[pid], ref[pid]
        bad = {}
        for k in ("volume", "area"):
            if not close(x[k], y[k], a.tol_rel, a.tol_abs):
                bad[k] = [x[k], y[k]]
        for k in ("min", "max"):
            if not all(close(p, q, a.tol_rel, a.tol_abs) for p, q in zip(x[k], y[k])):
                bad[k] = [x[k], y[k]]
        if bad:
            differences.append({"id": pid, "differences": bad})
    print(json.dumps({"file": a.file, "products_arc": len(arc), "products_oracle": len(ref),
                      "differences": differences}, indent=2))
    return 1 if differences else 0


if __name__ == "__main__":
    sys.exit(main())
