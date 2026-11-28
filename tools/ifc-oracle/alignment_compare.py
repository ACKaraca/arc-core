#!/usr/bin/env python3
"""Compares the alignment curves of arc-io-ifc with IfcOpenShell.

For every composite, gradient and segmented reference curve of a file, and for the layout of every
alignment, the frame at sampled distances (`cargo run -p arc-io-ifc --example alignment_samples`;
set ARC_ALIGNMENT_SAMPLES to a built binary to skip cargo) against IfcOpenShell's function item
evaluator on the same curve (for a layout, on the curve item of the axis representation). Prints the
largest deviation per segment type and exits 1 when a position deviates by more than --tol.

Usage: alignment_compare.py <file.ifc>... [--samples N] [--tol METERS] [--json]
"""
import argparse
import json
import math
import os
import subprocess
import sys

import ifcopenshell
import ifcopenshell.ifcopenshell_wrapper as w
import ifcopenshell.geom


def arc_samples(path, samples):
    binary = os.environ.get("ARC_ALIGNMENT_SAMPLES")
    cmd = [binary] if binary else ["cargo", "run", "-q", "-p", "arc-io-ifc", "--example", "alignment_samples", "--"]
    out = subprocess.run(cmd + [path, "--samples", str(samples)], check=True, capture_output=True, text=True)
    return json.loads(out.stdout)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="+")
    ap.add_argument("--samples", type=int, default=200)
    ap.add_argument("--tol", type=float, default=1e-3)
    ap.add_argument("--json", metavar="FILE", help="write the per segment type table as JSON to FILE (the oracle prints to standard output itself)")
    args = ap.parse_args()
    placements = {}
    worst = {}  # (plan, gradient, cant) -> [max position, max tangent angle, max frame, count]
    failed = False
    for path in args.files:
        model = ifcopenshell.open(path)
        settings = ifcopenshell.geom.settings()
        label = os.path.basename(path)
        for entry in arc_samples(path, args.samples):
            if "error" in entry:
                print(f"{label} #{entry['item']}: arc error {entry['error']}")
                continue
            item = entry.get("item")
            if item is None:
                continue
            if entry["kind"] == "placement":
                import ifcopenshell.util.placement as placement

                o = placement.get_local_placement(model.by_id(item))
                a = entry["m"]
                dp = math.dist([a[3], a[7], a[11]], [o[0][3], o[1][3], o[2][3]])
                df = max(abs(a[4 * r + c] - o[r][c]) for r in range(3) for c in range(3))
                rec = placements.setdefault(label, [0.0, 0.0, 0])
                rec[0], rec[1], rec[2] = max(rec[0], dp), max(rec[1], df), rec[2] + 1
                if dp > args.tol or df > 1e-6:
                    failed = True
                continue
            try:
                shape = w.map_shape(settings, model.by_id(item))
                evaluator = w.function_item_evaluator(settings, shape)
            except Exception as exc:  # noqa: BLE001
                print(f"{label} #{item}: oracle cannot map ({exc})")
                continue
            # The oracle works in the file's length unit converted to meters by its settings.
            for s in entry["samples"]:
                try:
                    m = evaluator.evaluate(s["d"])
                except Exception as exc:  # noqa: BLE001
                    print(f"{label} #{item} d={s['d']:.3f}: oracle error {exc}")
                    continue
                # The evaluator returns a column-major 4x4: axes in the first three columns,
                # translation in the last.
                ot = [m[0][3], m[1][3], m[2][3]]
                axes = [[m[0][i], m[1][i], m[2][i]] for i in range(3)]
                a = s["m"]
                at = [a[3], a[7], a[11]]
                aaxes = [[a[0], a[4], a[8]], [a[1], a[5], a[9]], [a[2], a[6], a[10]]]
                dp = math.dist(at, ot)
                dot = max(-1.0, min(1.0, sum(x * y for x, y in zip(aaxes[0], axes[0]))))
                dt = math.acos(dot)
                df = max(abs(aaxes[i][j] - axes[i][j]) for i in range(3) for j in range(3))
                key = (entry["kind"], s["plan"], s["gradient"], s["cant"])
                rec = worst.setdefault(key, [0.0, 0.0, 0.0, 0, label, 0.0])
                if dp > rec[0]:
                    rec[0], rec[5] = dp, s["d"]
                rec[1] = max(rec[1], dt)
                rec[2] = max(rec[2], df)
                rec[3] += 1
                if dp > args.tol:
                    failed = True
    rows = sorted(worst.items(), key=lambda kv: -kv[1][0])
    if args.json:
        open(args.json, "w").write(json.dumps([{"kind": k[0], "plan": k[1], "gradient": k[2], "cant": k[3], "max_position_m": v[0], "max_tangent_rad": v[1], "max_frame": v[2], "samples": v[3], "file": v[4], "at": v[5]} for k, v in rows], indent=1))
        print(f"wrote {args.json}")
    else:
        print(f"{'source':9} {'plan':32} {'gradient':20} {'cant':20} {'pos m':>10} {'tan rad':>10} {'frame':>10} {'n':>5}")
        for k, v in rows:
            print(f"{k[0]:9} {str(k[1]):32} {str(k[2]):20} {str(k[3]):20} {v[0]:10.2e} {v[1]:10.2e} {v[2]:10.2e} {v[3]:5d}  {v[4]}")
    for label, (dp, df, n) in placements.items():
        print(f"placements {label}: {n} linear placements, max position {dp:.2e} m, max frame {df:.2e}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
