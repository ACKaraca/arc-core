//! Prints the import views of IFC files as JSON, for the oracle in
//! `tools/ifc-oracle/views_compare.py`: `cargo run -p arc-io-ifc --example import_views <file>...`.
//! Not part of any shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::too_many_lines,
    reason = "a command-line tool of the oracle"
)]

use arc_base::{Cancel, Limits};
use arc_io_ifc::{
    MaterialAssociation, Nominal, ObjectView, PlacementState, PropertyValue, ReadOptions, Value,
    read,
};
use serde_json::{Map, Value as Json, json};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: import_views <file.ifc>...");
        std::process::exit(2);
    }
    let files: Map<String, Json> = paths
        .iter()
        .map(|path| {
            (
                path.clone(),
                run(path).unwrap_or_else(|e| json!({ "error": e })),
            )
        })
        .collect();
    println!("{}", Json::Object(files));
}

fn value(v: &Value) -> Json {
    match v {
        Value::Text(t) => json!(t),
        Value::Real(r) => json!(r),
        Value::Integer(i) => json!(i),
        Value::Enumeration(e) => match e.as_str() {
            "T" => json!(true),
            "F" => json!(false),
            other => json!(other),
        },
    }
}

fn nominal(n: &Nominal) -> Json {
    value(&n.value)
}

fn property(value_of: &PropertyValue) -> Json {
    match value_of {
        PropertyValue::Single { nominal: n, .. } => n.as_ref().map_or(Json::Null, nominal),
        PropertyValue::Enumerated { values, .. } | PropertyValue::List { values, .. } => {
            Json::Array(values.iter().map(nominal).collect())
        }
        PropertyValue::Bounded {
            upper,
            lower,
            set_point,
            ..
        } => json!({
            "upper": upper.as_ref().map(nominal),
            "lower": lower.as_ref().map(nominal),
            "set_point": set_point.as_ref().map(nominal),
        }),
    }
}

fn material(m: &MaterialAssociation) -> Json {
    let name = |m: &arc_io_ifc::MaterialRef| m.name.clone();
    match m {
        MaterialAssociation::Single(m) => json!({"kind": "material", "names": [name(m)]}),
        MaterialAssociation::List(list) => {
            json!({"kind": "list", "names": list.iter().map(name).collect::<Vec<_>>()})
        }
        MaterialAssociation::LayerSet {
            name: set,
            layers,
            usage,
            ..
        } => json!({
            "kind": "layers",
            "set": set,
            "names": layers.iter().map(|l| l.material.as_ref().and_then(name)).collect::<Vec<_>>(),
            "thickness": layers.iter().map(|l| l.thickness).collect::<Vec<_>>(),
            "usage": usage.as_ref().map(|u| json!({
                "direction": u.direction, "sense": u.sense, "offset": u.offset,
            })),
        }),
        MaterialAssociation::ProfileSet {
            name: set,
            profiles,
            ..
        } => json!({
            "kind": "profiles",
            "set": set,
            "names": profiles.iter().map(|p| p.material.as_ref().and_then(name)).collect::<Vec<_>>(),
        }),
        MaterialAssociation::ConstituentSet {
            name: set,
            constituents,
            ..
        } => json!({
            "kind": "constituents",
            "set": set,
            "names": constituents.iter().map(|c| c.material.as_ref().and_then(name)).collect::<Vec<_>>(),
        }),
    }
}

fn object(o: &ObjectView) -> Json {
    let psets: Map<String, Json> = o
        .property_sets
        .iter()
        .map(|s| {
            let props: Map<String, Json> = s
                .properties
                .iter()
                .map(|p| {
                    (
                        p.property.name.clone().unwrap_or_default(),
                        property(&p.property.value),
                    )
                })
                .collect();
            (s.name.clone().unwrap_or_default(), Json::Object(props))
        })
        .collect();
    let qtos: Map<String, Json> = o
        .quantity_sets
        .iter()
        .map(|s| {
            let q: Map<String, Json> = s
                .quantities
                .iter()
                .map(|q| {
                    (
                        q.quantity.name.clone().unwrap_or_default(),
                        json!(q.quantity.value.map(|n| match n {
                            arc_io_ifc::Number::Integer(i) => i as f64,
                            arc_io_ifc::Number::Real(r) => r,
                        })),
                    )
                })
                .collect();
            (s.name.clone().unwrap_or_default(), Json::Object(q))
        })
        .collect();
    let (placement, placed) = match &o.placement {
        Some(p) => (
            Some(p.world.to_vec()),
            matches!(p.state, PlacementState::Resolved),
        ),
        None => (None, false),
    };
    json!({
        "class": o.type_name,
        "kind": format!("{:?}", o.kind),
        "global_id": o.global_id,
        "name": o.name,
        "description": o.description,
        "object_type": o.object_type,
        "tag": o.tag,
        "predefined": o.effective_predefined_type,
        "user_defined": o.user_defined_type,
        "type": o.type_object,
        "container": o.container,
        "aggregate_parent": o.aggregate_parent,
        "nest_parent": o.nest_parent,
        "psets": psets,
        "qtos": qtos,
        "classifications": o.classifications.iter().map(|c| json!({
            "identification": c.identification, "name": c.name,
            "system": c.system.as_ref().and_then(|s| s.name.clone()),
        })).collect::<Vec<_>>(),
        "material": o.material.as_ref().map(|(m, _)| material(m)),
        "styles": o.styles.iter().map(|s| json!({"color": s.color, "transparency": s.transparency})).collect::<Vec<_>>(),
        "openings": o.openings,
        "filled_by": o.filled_by,
        "groups": o.groups,
        "placement": placement,
        "placement_resolved": placed,
        "elevation": o.elevation,
        "owner_history": o.owner_history,
    })
}

fn run(path: &str) -> Result<Json, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let file = read(&bytes, &mut ReadOptions::default())
        .map_err(|e| e.to_string())?
        .file;
    let start = std::time::Instant::now();
    let views = file
        .typed_views(&Limits::DEFAULT, &Cancel::new())
        .map_err(|e| e.to_string())?;
    let i = &views.import;
    let objects: Map<String, Json> = i
        .objects
        .iter()
        .map(|o| (o.instance.to_string(), object(o)))
        .collect();
    Ok(json!({
        "seconds": start.elapsed().as_secs_f64(),
        "schema": file.schema.identifier(),
        "length_to_meters": i.units.length_to_meters,
        "units": i.units.units.iter().take(i.units.assigned).map(|u| json!({
            "type": u.unit_type, "name": u.name, "to_si": u.to_si,
        })).collect::<Vec<_>>(),
        "objects": objects,
        "groups": i.groups.iter().map(|g| json!({
            "instance": g.instance, "class": g.type_name, "name": g.name, "members": g.members,
        })).collect::<Vec<_>>(),
        "connections": i.connections.iter().map(|c| json!({
            "kind": c.kind, "relating": c.relating, "related": c.related,
            "relating_type": c.relating_type, "related_type": c.related_type,
        })).collect::<Vec<_>>(),
        "space_boundaries": i.space_boundaries.iter().map(|b| json!({
            "level": b.level, "space": b.space, "element": b.element,
        })).collect::<Vec<_>>(),
        "voids": i.objects.iter().filter_map(|o| o.voids.map(|h| json!([h, o.instance]))).collect::<Vec<_>>(),
        "fills": i.objects.iter().filter_map(|o| o.fills.map(|op| json!([op, o.instance]))).collect::<Vec<_>>(),
        "georeference": i.georeference.as_ref().map(|g| json!({
            "source": format!("{:?}", g.source),
            "eastings": g.eastings, "northings": g.northings, "height": g.orthogonal_height,
            "abscissa": g.x_axis_abscissa, "ordinate": g.x_axis_ordinate, "scale": g.scale,
            "crs": g.crs.as_ref().map(|c| json!({"name": c.name, "projection": c.map_projection, "zone": c.map_zone, "datum": c.geodetic_datum})),
        })),
        "owner_histories": i.owner_histories.len(),
        "coverage": i.coverage.iter().map(|c| json!({
            "class": c.type_name, "total": c.total, "interpreted": c.interpreted,
            "partial": c.partial, "preserved": c.preserved,
        })).collect::<Vec<_>>(),
        "report": { "items": views.report.items.len(), "summary": format!("{:?}", views.report.summary) },
    }))
}
