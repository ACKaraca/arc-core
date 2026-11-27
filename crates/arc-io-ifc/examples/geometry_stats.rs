//! Prints per-product tessellation statistics of an IFC file as JSON, for the oracle in
//! `tools/ifc-oracle/geometry_compare.py`: `cargo run -p arc-io-ifc --example geometry_stats <file>`.
//! Not part of any shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool of the oracle"
)]

use std::collections::BTreeMap;

use arc_base::{Cancel, Limits};
use arc_io_ifc::{ReadOptions, read};
use arc_mesh::measure::{surface_area, volume};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: geometry_stats <file.ifc>");
        std::process::exit(2);
    };
    if let Err(message) = run(&path) {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run(path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let limits = Limits::DEFAULT;
    let file = read(&bytes, &mut ReadOptions::default())
        .map_err(|e| e.to_string())?
        .file;
    let geometry = file
        .geometry(&limits, &Cancel::new())
        .map_err(|e| e.to_string())?;
    // product step id -> [volume, area, min x y z, max x y z, items]
    let mut products: BTreeMap<u64, [f64; 9]> = BTreeMap::new();
    for item in &geometry.items {
        let Some(mesh) = item.world_mesh(&limits).map_err(|e| e.to_string())? else {
            continue;
        };
        let v = volume(&mesh, &limits).map_err(|e| e.to_string())?;
        let a = surface_area(&mesh, &limits).map_err(|e| e.to_string())?;
        let entry = products.entry(item.product).or_insert([
            0.0,
            0.0,
            f64::MAX,
            f64::MAX,
            f64::MAX,
            f64::MIN,
            f64::MIN,
            f64::MIN,
            0.0,
        ]);
        entry[0] += v;
        entry[1] += a;
        for p in mesh.positions() {
            for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                entry[2 + k] = entry[2 + k].min(c);
                entry[5 + k] = entry[5 + k].max(c);
            }
        }
        entry[8] += 1.0;
    }
    let rows: Vec<String> = products
        .iter()
        .map(|(id, s)| {
            format!(
                "{{\"id\":{id},\"volume\":{},\"area\":{},\"min\":[{},{},{}],\"max\":[{},{},{}],\"items\":{}}}",
                s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7], s[8]
            )
        })
        .collect();
    println!("{{\"unit\":\"metre\",\"products\":[{}]}}", rows.join(","));
    Ok(())
}
