#!/usr/bin/env python3
"""Validates IFC files with ifcopenshell.validate (EXPRESS rules on).

Usage: validate.py <file-or-directory>...
Prints one JSON summary; exits 1 when any file has an issue or cannot be opened.
"""
import collections
import json
import logging
import re
import sys
from pathlib import Path

import ifcopenshell
import ifcopenshell.validate


def files(args):
    for a in args:
        p = Path(a)
        yield from sorted(p.rglob("*.ifc")) if p.is_dir() else [p]


def validate(path):
    try:
        f = ifcopenshell.open(str(path))
    except Exception as e:  # unreadable file is a finding, not a crash
        return {"opened": False, "error": str(e), "issues": 1, "by_kind": {"open": 1}}
    logger = ifcopenshell.validate.json_logger()
    try:
        ifcopenshell.validate.validate(f, logger, express_rules=True)
    except Exception as e:  # the validator itself fails on some malformed input
        return {"opened": True, "schema": f.schema, "issues": 1,
                "by_kind": {"validator_crash": 1}, "first": [repr(e)]}
    # Kind: the validator's level plus the message with ids and numbers masked.
    kinds = collections.Counter(
        s["level"] + ": " + re.sub(r"#\d+|\d+(\.\d+)?|'[^']*'", "_", str(s["message"]))[:80]
        for s in logger.statements
    )
    return {
        "opened": True,
        "schema": f.schema,
        "issues": len(logger.statements),
        "by_kind": dict(kinds),
        "first": [str(s.get("message")) for s in logger.statements[:5]],
    }


def main(argv):
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    logging.disable(logging.CRITICAL)
    report = {str(p): validate(p) for p in files(argv)}
    total = sum(r["issues"] for r in report.values())
    print(json.dumps({"files": report, "file_count": len(report), "issues": total}, indent=2))
    return 1 if total or not report else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
