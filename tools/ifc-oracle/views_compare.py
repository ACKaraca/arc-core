#!/usr/bin/env python3
"""Compares the import views of `arc-io-ifc` with IfcOpenShell.

    views_compare.py [--verbose] [--max-diffs N] <files or directories>

Runs `cargo run --release -p arc-io-ifc --example import_views` (set `ARC_IMPORT_VIEWS` to a built
binary to skip cargo) and reads the same files with `ifcopenshell`. For every file and category the
two sides are compared item by item; the output is an agreement table (items compared, agreeing,
differing) and, with `--verbose`, every difference. Exit status 1 when a difference remains that is
not listed in `KNOWN` (documented oracle conventions, see README.md).

Categories: products by class, type objects by class, containment, aggregation, nesting, type
assignment, predefined type, property sets (set names, property names, values), quantity sets,
materials (kind, names, layer thicknesses), classifications, openings, fills, groups,
georeference, units, placements (translation and axes), element connections, space boundaries,
surface styles.
"""

import argparse
import collections
import json
import math
import os
import subprocess
import sys

import ifcopenshell
import ifcopenshell.util.classification as classification
import ifcopenshell.util.element as element_util
import ifcopenshell.util.placement as placement_util
import ifcopenshell.util.unit as unit_util

CATEGORIES = [
    "products", "types", "containment", "aggregation", "nesting", "type_assignment",
    "predefined_type", "psets", "quantities", "materials", "classifications", "openings",
    "fills", "groups", "georeference", "units", "placements", "connections",
    "space_boundaries", "styles", "owner_history", "spatial",
]


def close(a, b, tol=1e-9):
    if isinstance(a, bool) or isinstance(b, bool):
        return a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return math.isclose(a, b, rel_tol=1e-9, abs_tol=tol)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(close(x, y) for x, y in zip(a, b))
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(close(a[k], b[k]) for k in a)
    return a == b


class Table:
    def __init__(self):
        self.rows = collections.defaultdict(lambda: [0, 0])  # compared, agreeing
        self.diffs = collections.defaultdict(list)
        self.conventions = collections.defaultdict(collections.Counter)  # category -> convention -> items

    def check(self, category, key, ours, theirs, convention=None):
        """Counts one item. `convention` names a documented oracle convention (README.md) that
        explains a difference; such an item counts as agreeing and is tallied under that name."""
        row = self.rows[category]
        row[0] += 1
        if close(ours, theirs):
            row[1] += 1
        elif convention:
            row[1] += 1
            self.conventions[category][convention] += 1
        else:
            self.diffs[category].append((key, ours, theirs))


def wrapped(v):
    return getattr(v, "wrappedValue", v)


def oracle_value(v):
    """Normalizes an IfcOpenShell property value to the shape the example prints."""
    if isinstance(v, dict) and "UpperBound" in v:
        def bound(b):
            return wrapped(b) if b is not None else None
        return {"upper": bound(v.get("UpperBoundValue")), "lower": bound(v.get("LowerBoundValue")),
                "set_point": bound(v.get("SetPointValue"))}
    if isinstance(v, (list, tuple)):
        return [oracle_value(x) for x in v]
    return wrapped(v)


def psets(file_element, qtos):
    result = element_util.get_psets(file_element, psets_only=not qtos, qtos_only=qtos)
    return result


def compare(path, ours, table, verbose):
    model = ifcopenshell.open(path)
    objects = ours["objects"]
    scale = unit_util.calculate_unit_scale(model)
    ref = f"{model.schema}:{os.path.basename(path)}"

    def check(cat, key, a, b, convention=None):
        table.check(cat, f"{ref}:{key}", a, b, convention)

    # Units.
    check("units", "length_scale", ours["length_to_meters"], scale)
    project = model.by_type("IfcProject")
    if project and project[0].UnitsInContext:
        theirs = sorted(
            (u.UnitType, u.Name if u.is_a("IfcNamedUnit") and u.Name else getattr(u, "UserDefinedType", None))
            for u in project[0].UnitsInContext.Units if u.is_a("IfcNamedUnit") or u.is_a("IfcDerivedUnit"))
        mine = sorted((u["type"], u["name"] if u["name"] else None) for u in ours["units"]
                      if u["name"] or u["type"])
        # Derived units carry their type in UnitType; the user defined name is not compared.
        theirs_types = sorted(t for t, _ in theirs)
        mine_types = sorted(t for t, _ in mine)
        check("units", "unit_types", mine_types, theirs_types)

    # Products and types by class.
    for name, kind, cat in (("IfcProduct", "Product", "products"), ("IfcTypeObject", "Type", "types")):
        theirs = collections.Counter(e.is_a() for e in model.by_type(name))
        mine = collections.Counter(o["class"].lower() for o in objects.values() if o["kind"] == kind)
        theirs = collections.Counter({k.lower(): v for k, v in theirs.items()})
        for cls in sorted(set(theirs) | set(mine)):
            check(cat, cls, mine.get(cls, 0), theirs.get(cls, 0))

    contained = {}
    for r in model.by_type("IfcRelContainedInSpatialStructure"):
        for x in r.RelatedElements:
            contained.setdefault(x.id(), r.RelatingStructure.id())

    def mine_of(e):
        return objects.get(str(e.id()))

    # Per object.
    for e in model.by_type("IfcObjectDefinition"):
        if e.is_a("IfcGroup"):
            continue
        o = mine_of(e)
        if o is None:
            check("products", f"#{e.id()} missing", None, e.is_a())
            continue
        eid = f"#{e.id()}"
        if e.is_a("IfcProduct"):
            # `util.get_container` reads the inverse `ContainedInStructure`, which only IfcElement
            # has; the relationship is read directly so that spatial zones and parts count too.
            check("containment", eid, o["container"], contained.get(e.id()))
        a = element_util.get_aggregate(e)
        check("aggregation", eid, o["aggregate_parent"], a.id() if a else None)
        n = element_util.get_nest(e)
        check("nesting", eid, o["nest_parent"], n.id() if n else None)
        if e.is_a("IfcObject"):
            t = element_util.get_type(e)
            check("type_assignment", eid, o["type"], t.id() if t else None)
            if hasattr(e, "PredefinedType") or (t is not None and hasattr(t, "PredefinedType")):
                want = element_util.get_predefined_type(e)
                have = o["user_defined"] if o["predefined"] == "USERDEFINED" and o["user_defined"] else o["predefined"]
                fallback = have in (None, "NOTDEFINED") and want is not None and want == o["object_type"]
                check("predefined_type", eid, have, want,
                      "ObjectType returned where no usable PredefinedType exists" if fallback else None)
        # Property sets and quantities.
        theirs = {}
        for set_name, props in psets(e, False).items():
            theirs[set_name] = {k: oracle_value(v) for k, v in props.items() if k != "id"}
        check("psets", f"{eid}:sets", sorted(o["psets"]), sorted(theirs))
        for set_name, props in theirs.items():
            mine_props = o["psets"].get(set_name)
            if mine_props is None:
                continue
            check("psets", f"{eid}:{set_name}:names", sorted(mine_props), sorted(props))
            for k, v in props.items():
                if k in mine_props:
                    check("psets", f"{eid}:{set_name}.{k}", mine_props[k], v)
        theirs_q = {}
        for set_name, props in psets(e, True).items():
            theirs_q[set_name] = {k: wrapped(v) for k, v in props.items() if k != "id"}
        check("quantities", f"{eid}:sets", sorted(o["qtos"]), sorted(theirs_q))
        for set_name, props in theirs_q.items():
            mine_props = o["qtos"].get(set_name)
            if mine_props is None:
                continue
            check("quantities", f"{eid}:{set_name}:names", sorted(mine_props), sorted(props))
            for k, v in props.items():
                if k in mine_props:
                    check("quantities", f"{eid}:{set_name}.{k}", mine_props[k], v)
        # Materials.
        m = element_util.get_material(e, should_skip_usage=False)
        mm = o["material"]
        theirs_m = material_view(m, scale) if m is not None else None
        check("materials", eid, mm_shape(mm), theirs_m)
        # Classifications.
        refs = classification.get_references(e) if e.is_a("IfcObject") or e.is_a("IfcTypeObject") else set()
        theirs_c = sorted((r.Identification if hasattr(r, "Identification") else r.ItemReference, r.Name) for r in refs
                          if r.is_a("IfcClassificationReference"))
        mine_c = sorted((c["identification"], c["name"]) for c in o["classifications"]
                        if c["identification"] is not None or c["name"] is not None)
        check("classifications", eid, mine_c, theirs_c)
        # Openings and fills.
        if e.is_a("IfcElement"):
            check("openings", eid, sorted(o["openings"]), sorted(r.RelatedOpeningElement.id() for r in e.HasOpenings))
            check("fills", eid, o["filled_by"], [r.RelatedBuildingElement.id() for r in getattr(e, "HasFillings", [])])
        # Groups.
        theirs_g = sorted(r.RelatingGroup.id() for r in (getattr(e, "HasAssignments", None) or [])
                          if r.is_a("IfcRelAssignsToGroup"))
        check("groups", eid, sorted(o["groups"]), theirs_g)
        # Styles: the distinct surface styles of the representation items.
        if e.is_a("IfcProduct"):
            theirs_s = raw_styles(e)
            mine_s = sorted(tuple(s["color"]) + (s["transparency"] or 0.0,) for s in o["styles"] if s["color"])
            check("styles", eid, mine_s, theirs_s)
            util_s = len(element_util.get_styles(e)) > 0
            if util_s != bool(theirs_s):
                table.conventions["styles"]["util.get_styles finds no styles where the representation context has no TargetView (IFC2X3)"] += 1
        # Placements.
        if e.is_a("IfcProduct") and e.ObjectPlacement is not None and o["placement"] is not None:
            if o["placement_resolved"]:
                theirs_p = placement_util.get_local_placement(e.ObjectPlacement)
                flat = [float(theirs_p[r][c]) for r in range(4) for c in range(4)]
                flat = [v * scale if i in (3, 7, 11) else v for i, v in enumerate(flat)]
                check("placements", eid, o["placement"], flat)

    # Groups: members.
    for g in model.by_type("IfcGroup"):
        mine_g = next((x for x in ours["groups"] if x["instance"] == g.id()), None)
        check("groups", f"group #{g.id()}", sorted(mine_g["members"]) if mine_g else None,
              sorted(o.id() for r in g.IsGroupedBy for o in r.RelatedObjects))
    # Voids.
    theirs_v = sorted((r.RelatingBuildingElement.id(), r.RelatedOpeningElement.id()) for r in model.by_type("IfcRelVoidsElement"))
    check("openings", "voids", sorted(tuple(x) for x in ours["voids"]), theirs_v)
    theirs_f = sorted((r.RelatingOpeningElement.id(), r.RelatedBuildingElement.id()) for r in model.by_type("IfcRelFillsElement"))
    check("fills", "fills", sorted(tuple(x) for x in ours["fills"]), theirs_f)
    # Connections.
    theirs_k = sorted((r.is_a().upper(), r.RelatingElement.id(), r.RelatedElement.id(),
                       getattr(r, "RelatingConnectionType", None), getattr(r, "RelatedConnectionType", None))
                      for r in model.by_type("IfcRelConnectsElements"))
    mine_k = sorted((c["kind"], c["relating"], c["related"], c["relating_type"], c["related_type"]) for c in ours["connections"])
    check("connections", "all", mine_k, theirs_k)
    # Space boundaries.
    theirs_b = sorted((r.RelatingSpace.id(), r.RelatedBuildingElement.id() if r.RelatedBuildingElement else None)
                      for r in model.by_type("IfcRelSpaceBoundary"))
    mine_b = sorted((b["space"], b["element"]) for b in ours["space_boundaries"])
    check("space_boundaries", "all", mine_b, theirs_b)
    # Georeference: IfcMapConversion, or the ePSet_MapConversion property set in IFC2X3.
    g = ours["georeference"]
    mine_g = None if g is None else {
        "e": g["eastings"], "n": g["northings"], "h": g["height"], "a": g["abscissa"], "o": g["ordinate"],
        "s": g["scale"], "crs": g["crs"] and g["crs"]["name"]}
    theirs_g = None
    if model.schema != "IFC2X3":
        maps = model.by_type("IfcMapConversion")
        if maps:
            mc = maps[0]
            theirs_g = {"e": mc.Eastings, "n": mc.Northings, "h": mc.OrthogonalHeight,
                        "a": mc.XAxisAbscissa if mc.XAxisAbscissa is not None else 1.0,
                        "o": mc.XAxisOrdinate if mc.XAxisOrdinate is not None else 0.0,
                        "s": mc.Scale if mc.Scale is not None else 1.0, "crs": mc.TargetCRS.Name}
    else:
        for root in model.by_type("IfcProject") + model.by_type("IfcSite"):
            sets = element_util.get_psets(root, psets_only=True)
            if "ePSet_MapConversion" in sets:
                m = sets["ePSet_MapConversion"]
                crs = sets.get("ePSet_ProjectedCRS", {})
                theirs_g = {"e": wrapped(m.get("Eastings", 0.0)), "n": wrapped(m.get("Northings", 0.0)),
                            "h": wrapped(m.get("OrthogonalHeight", 0.0)),
                            "a": m.get("XAxisAbscissa", 1.0), "o": m.get("XAxisOrdinate", 0.0),
                            "s": m.get("Scale", 1.0), "crs": crs.get("Name")}
                break
    check("georeference", "georeference", mine_g, theirs_g)
    # Owner histories: per object and as a set.
    for e in model.by_type("IfcObjectDefinition"):
        o = mine_of(e)
        if o is not None:
            check("owner_history", f"#{e.id()}", o["owner_history"], e.OwnerHistory.id() if e.OwnerHistory else None)
    used = sorted({e.OwnerHistory.id() for e in model.by_type("IfcObjectDefinition") if e.OwnerHistory})
    check("owner_history", "histories", ours["owner_histories"], len(used))
    # Spatial attributes: storey elevations in meters.
    for e in model.by_type("IfcBuildingStorey"):
        o = mine_of(e)
        check("spatial", f"#{e.id()}.Elevation", o["elevation"], None if e.Elevation is None else e.Elevation * scale)
    # Unit conversion of every unit in the assignment with a factor: SI units, conversion based and derived.
    for u in (project[0].UnitsInContext.Units if project and project[0].UnitsInContext else []):
        if not (u.is_a("IfcNamedUnit") or u.is_a("IfcDerivedUnit")):
            continue
        mine_u = [x for x in ours["units"] if x["type"] == u.UnitType]
        if u.is_a("IfcDerivedUnit"):
            theirs_f = unit_util.get_derived_unit_scale(u)
        else:
            theirs_f = unit_util.get_named_unit_scale(u)
        if mine_u:
            # IfcOpenShell scales a named unit by its prefix only, so GRAM is 1; arc converts to the
            # SI base unit of the type (kilogram), so GRAM is 0.001.
            if u.is_a("IfcDerivedUnit"):
                grams = sum(e.Exponent for e in u.Elements if e.Unit.is_a("IfcSIUnit") and e.Unit.Name == "GRAM")
            else:
                grams = 1 if u.is_a("IfcSIUnit") and u.Name == "GRAM" else 0
            check("units", f"{u.UnitType}", mine_u[0]["to_si"], theirs_f,
                  "GRAM is scaled to kilogram (arc) or taken as 1 (IfcOpenShell)"
                  if grams and close(mine_u[0]["to_si"], theirs_f * 1e-3 ** grams) else None)


def raw_styles(e):
    """The colors of the surface styles of the representation items of a product, followed through
    mapped items (three levels), read from the entities directly."""
    colors = []
    rep = getattr(e, "Representation", None)
    if rep is None:
        return colors
    queue = [(i, 0) for r in rep.Representations for i in (r.Items or [])]
    seen = set()
    while queue:
        item, depth = queue.pop()
        if item.id() in seen:
            continue
        seen.add(item.id())
        for styled in item.StyledByItem or []:
            for style in styled.Styles:
                for surface in (style.Styles if style.is_a("IfcPresentationStyleAssignment") else [style]):
                    if not surface.is_a("IfcSurfaceStyle"):
                        continue
                    for element in surface.Styles:
                        if element.is_a("IfcSurfaceStyleShading") and element.SurfaceColour is not None:
                            c = element.SurfaceColour
                            colors.append((c.Red, c.Green, c.Blue, getattr(element, "Transparency", None) or 0.0))
                            break
        if item.is_a("IfcMappedItem") and depth < 3:
            queue.extend((i, depth + 1) for i in item.MappingSource.MappedRepresentation.Items)
    return sorted(colors)


def mm_shape(mm):
    if mm is None:
        return None
    m = mm if isinstance(mm, dict) else mm
    out = {"kind": m["kind"], "names": m["names"]}
    if m["kind"] == "layers":
        out["thickness"] = m["thickness"]
    return out


def material_view(m, scale):
    if m.is_a("IfcMaterialLayerSetUsage"):
        m = m.ForLayerSet
    elif m.is_a("IfcMaterialProfileSetUsage"):
        m = m.ForProfileSet
    if m.is_a("IfcMaterial"):
        return {"kind": "material", "names": [m.Name]}
    if m.is_a("IfcMaterialList"):
        return {"kind": "list", "names": [x.Name for x in m.Materials]}
    if m.is_a("IfcMaterialLayerSet"):
        return {"kind": "layers", "names": [l.Material.Name if l.Material else None for l in m.MaterialLayers],
                "thickness": [l.LayerThickness * scale for l in m.MaterialLayers]}
    if m.is_a("IfcMaterialProfileSet"):
        return {"kind": "profiles", "names": [p.Material.Name if p.Material else None for p in m.MaterialProfiles]}
    if m.is_a("IfcMaterialConstituentSet"):
        return {"kind": "constituents", "names": [c.Material.Name if c.Material else None for c in m.MaterialConstituents]}
    return {"kind": m.is_a(), "names": []}


def files_of(paths):
    for p in paths:
        if os.path.isdir(p):
            for root, _, names in sorted(os.walk(p)):
                for n in sorted(names):
                    if n.endswith(".ifc"):
                        yield os.path.join(root, n)
        else:
            yield p


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("paths", nargs="+")
    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--max-diffs", type=int, default=20)
    args = parser.parse_args()
    files = list(files_of(args.paths))
    binary = os.environ.get("ARC_IMPORT_VIEWS")
    cmd = [binary] if binary else ["cargo", "run", "--release", "-q", "-p", "arc-io-ifc", "--example", "import_views", "--"]
    out = subprocess.run(cmd + files, capture_output=True, text=True, check=True).stdout
    ours = json.loads(out)
    table = Table()
    failed = False
    for path in files:
        data = ours.get(path, {})
        if "error" in data:
            print(f"{path}: arc error {data['error']}", file=sys.stderr)
            failed = True
            continue
        try:
            compare(path, data, table, args.verbose)
        except Exception as e:  # noqa: BLE001 - report and continue with the next file
            print(f"{path}: oracle error {e!r}", file=sys.stderr)
            failed = True
    print(f"{'category':18} {'compared':>9} {'agree':>9} {'differ':>7}")
    for cat in CATEGORIES:
        c, a = table.rows.get(cat, [0, 0])
        print(f"{cat:18} {c:9} {a:9} {c - a:7}")
        for key, mine, theirs in table.diffs.get(cat, [])[: (None if args.verbose else args.max_diffs)]:
            print(f"    {key}\n        arc:    {mine!r}\n        oracle: {theirs!r}")
        for name, n in table.conventions.get(cat, {}).items():
            print(f"    convention ({n} items): {name}")
        if table.diffs.get(cat):
            failed = True
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
