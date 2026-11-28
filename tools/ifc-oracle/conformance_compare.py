#!/usr/bin/env python3
"""Compares the conformance findings of arc-io-ifc with ifcopenshell.validate.

Usage: conformance_compare.py [--json] [--rules] <file-or-directory>...

Runs `cargo run -p arc-io-ifc --example conformance` (set ARC_CONFORMANCE to a built binary to skip
cargo) and ifcopenshell.validate on every file, maps both to rule classes and compares them per
(file, instance, attribute). Without --rules, levels 1 and 2 are compared and the oracle runs
without EXPRESS rules. With --rules, the oracle runs with them and level 3 is compared too: entity
and defined type WHERE rules per (instance, `Owner.Label`), global rules per rule name; a table per
rule follows, with the number of checks Arc evaluated and could not evaluate. An unknown enumeration item or a dangling reference,
which the oracle reads as no value and reports as a missing required value, is counted once as
`read_as_missing_by_oracle` when both sides report it. Prints a confusion table per class: agree, Arc only,
oracle only; with --json, every disagreement. Exits 1 when a file cannot be checked by either side.
"""
import collections
import json
import logging
import os
import re
import subprocess
import sys
from pathlib import Path

import ifcopenshell
import ifcopenshell.validate

# Arc rule -> class compared with the oracle. The oracle reports every attribute value error with one
# message, so the value rules share one class.
VALUE = "attribute_value"
ARC_CLASS = {
    "entity_unknown": "entity_unknown",
    "entity_abstract": "entity_abstract",
    "complex_invalid": "complex_invalid",
    "attribute_count": "attribute_count",
    "value_missing": "value_missing",
    "derived_misplaced": "derived_misplaced",
    "value_type": VALUE,
    "enumeration_item": VALUE,
    "select_member": VALUE,
    "reference_type": VALUE,
    "aggregate_size": VALUE,
    "aggregate_unique": VALUE,
    "value_width": VALUE,
    "reference_dangling": "reference_dangling",
    "inverse_cardinality": "inverse_cardinality",
    "unique_rule": "unique_rule",
    "where_rule": "where_rule",
    "type_rule": "type_rule",
    "global_rule": "global_rule",
}

# Oracle statement types of the EXPRESS rules.
ORACLE_RULE_TYPES = {"entity_rule": "where_rule", "simpletype_rule": "type_rule", "global_rule": "global_rule"}


# Arc rules whose defect the oracle reports as a missing value.
NULLED = ("enumeration_item", "reference_dangling")


def files(args):
    for a in args:
        p = Path(a)
        yield from sorted(p.rglob("*.ifc")) if p.is_dir() else [p]


def arc(paths):
    binary = os.environ.get("ARC_CONFORMANCE")
    cmd = [binary] if binary else ["cargo", "run", "-q", "-p", "arc-io-ifc", "--example", "conformance", "--"]
    out = subprocess.run(cmd + [str(p) for p in paths], capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def last(name):
    return (name or "").split(".")[-1].lower()


def rule_key(cls, name):
    """The compared name of a level 3 rule: `owner.label`, the owner alone for a global rule."""
    name = (name or "").lower()
    return name.split(".")[0] if cls == "global_rule" else name


def oracle_class(statement):
    message = str(statement.get("message", ""))
    if statement.get("type") in ORACLE_RULE_TYPES:
        cls = ORACLE_RULE_TYPES[statement["type"]]
        return cls, rule_key(cls, str(statement.get("attribute", "")))
    attribute = str(statement.get("attribute", ""))
    if message.startswith("Entity ") and message.endswith(" is abstract"):
        return "entity_abstract", ""
    if message == "Attribute not optional":
        return "value_missing", last(attribute)
    if message.startswith("With attribute:"):
        if re.search(r"\nValue:\n\s*\*\n", message):
            return "derived_misplaced", last(attribute)
        return VALUE, last(attribute)
    if message.startswith("With inverse:"):
        return "inverse_cardinality", last(attribute)
    m = re.search(r"\nRule \w+\.(UR\d+):\n\s*The attribute (.*) should be unique", message)
    if m:
        return "unique_rule", ",".join(last(a) for a in m.group(2).split(", "))
    if "IfcGloballyUniqueId base64 validation" in message:
        if "length should be" in message:
            return VALUE, "globalid"
        return "globalid_encoding", "globalid"
    return "other", message.splitlines()[0][:60] if message else ""


def instance_id(statement):
    m = re.match(r"#(\d+)=", str(statement.get("instance", "")))
    return int(m.group(1)) if m else (0 if statement.get("type") == "global_rule" else None)


def oracle(path, rules):
    try:
        f = ifcopenshell.open(str(path))
    except Exception as e:  # unreadable file
        return None, str(e)
    logger = ifcopenshell.validate.json_logger()
    try:
        ifcopenshell.validate.validate(f, logger, express_rules=rules)
    except Exception as e:  # the validator fails on some malformed input
        return None, repr(e)
    found = set()
    for s in logger.statements:
        cls = oracle_class(s)
        found.add((cls, instance_id(s)))
        if cls[0] == "unique_rule":
            # The rule fails for every instance of the group; the oracle names the others here.
            for other in re.findall(r"Violated by:\n\s*#(\d+)=", str(s.get("message", ""))):
                found.add((cls, int(other)))
    return found, None


def main(argv):
    as_json = "--json" in argv
    rules = "--rules" in argv
    paths = list(files(a for a in argv if a not in ("--json", "--rules")))
    if not paths:
        print(__doc__, file=sys.stderr)
        return 2
    logging.disable(logging.CRITICAL)
    ours = arc(paths)
    table = collections.defaultdict(lambda: collections.Counter())
    disagreements = []
    per_rule = collections.defaultdict(lambda: collections.Counter())
    status = 0
    for path in paths:
        mine = ours.get(str(path), {})
        theirs, error = oracle(path, rules)
        if "error" in mine or error is not None:
            disagreements.append({"file": str(path), "arc_error": mine.get("error"), "oracle_error": error})
            status = 1
            continue
        a = set()
        nulled = set()
        for f in mine["findings"]:
            cls = ARC_CLASS[f["rule"].removeprefix("io.ifc.conformance_")]
            attr = "" if cls in ("entity_abstract",) else last(f["attribute"])
            if cls == "unique_rule":
                attr = ",".join(last(x) for x in (f["attribute"] or "").split(","))
            if cls in ORACLE_RULE_TYPES.values():
                if not rules:
                    continue
                attr = rule_key(cls, f["attribute"])
            a.add(((cls, attr), f["instance"]))
            rule = f["rule"].removeprefix("io.ifc.conformance_")
            if rule in NULLED:
                nulled.add((attr, f["instance"]))
        b = {((cls, attr), i) for (cls, attr), i in theirs}
        # The oracle reads an unknown enumeration item or a dangling reference as no value and
        # reports a missing value: the same defect in another class.
        for attr, inst in nulled:
            pair = ((VALUE if ((VALUE, attr), inst) in a else "reference_dangling", attr), inst)
            missing = (("value_missing", attr), inst)
            if pair in a and missing in b:
                a.discard(pair)
                b.discard(missing)
                table["read_as_missing_by_oracle"]["agree"] += 1
        for c in mine.get("where_rules", []) if rules else []:
            cls = {"Entity": "where_rule", "Type": "type_rule", "Global": "global_rule"}[c["scope"]]
            counter = per_rule[(cls, rule_key(cls, c["rule"]))]
            counter["evaluated"] += c["passed"] + c["unknown"] + c["failed"]
            counter["not_evaluated"] += c["not_evaluated"]
        for key in a | b:
            (cls, attr), inst = key
            kind = "agree" if key in a and key in b else ("arc_only" if key in a else "oracle_only")
            table[cls][kind] += 1
            if cls in ORACLE_RULE_TYPES.values():
                per_rule[(cls, attr)][kind] += 1
            if kind != "agree":
                disagreements.append({"file": str(path), "class": cls, "instance": inst,
                                      "attribute": attr, "side": kind})
    summary = {cls: dict(c) for cls, c in sorted(table.items())}
    # Rules with a finding on either side or a check that was not evaluated.
    rule_table = {f"{cls}:{name}": dict(c) for (cls, name), c in sorted(per_rule.items())
                  if c["agree"] or c["arc_only"] or c["oracle_only"] or c["not_evaluated"]}
    if as_json:
        print(json.dumps({"files": len(paths), "classes": summary, "rules": rule_table,
                          "disagreements": disagreements}, indent=2))
    else:
        print(f"{'class':24} {'agree':>7} {'arc_only':>9} {'oracle_only':>12}")
        for cls, c in summary.items():
            print(f"{cls:24} {c.get('agree', 0):7} {c.get('arc_only', 0):9} {c.get('oracle_only', 0):12}")
        if rules:
            evaluated = sum(c["evaluated"] for c in per_rule.values())
            skipped = sum(c["not_evaluated"] for c in per_rule.values())
            print(f"\nrule checks evaluated by Arc: {evaluated}, not evaluated: {skipped}")
            print(f"{'rule':60} {'agree':>6} {'arc_only':>9} {'oracle_only':>12} {'not_eval':>9}")
            for name, c in rule_table.items():
                print(f"{name:60} {c.get('agree', 0):6} {c.get('arc_only', 0):9} "
                      f"{c.get('oracle_only', 0):12} {c.get('not_evaluated', 0):9}")
    return status


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
