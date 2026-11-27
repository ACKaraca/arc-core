//! Prints per-product tessellation statistics of an IFC file as JSON, for the oracle in
//! `tools/ifc-oracle/geometry_compare.py`: `cargo run -p arc-io-ifc --example geometry_stats <file>`.
//! `--chord <m>` sets the chord tolerance in meters (default 1 mm). `--openings` subtracts the openings of the elements (the library's default; without the flag the
//! openings are left in, as the oracle's default setting does). Volume and area are plain sums over
//! the triangles, so a mesh that is not closed (a surface model) still has numbers. Not part of any
//! shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool of the oracle"
)]

use std::collections::BTreeMap;

use arc_base::{Cancel, Limits, Tolerance};
use arc_io_ifc::{GeometryOptions, ReadOptions, read};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let openings = args.iter().any(|a| a == "--openings");
    let chord = args
        .iter()
        .position(|a| a == "--chord")
        .and_then(|k| args.get(k + 1))
        .and_then(|v| v.parse::<f64>().ok());
    let Some(path) = args
        .iter()
        .find(|a| !a.starts_with("--") && a.parse::<f64>().is_err())
    else {
        eprintln!("usage: geometry_stats <file.ifc> [--openings]");
        std::process::exit(2);
    };
    if let Err(message) = run(path, openings, chord) {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run(path: &str, openings: bool, chord: Option<f64>) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let limits = Limits::DEFAULT;
    let file = read(&bytes, &mut ReadOptions::default())
        .map_err(|e| e.to_string())?
        .file;
    let mut tolerance = Tolerance::DEFAULT;
    if let Some(chord) = chord {
        tolerance.tessellation_chord = chord;
    }
    let options = GeometryOptions {
        subtract_openings: openings,
    };
    let geometry = file
        .geometry_with_options(&limits, &Cancel::new(), &tolerance, &options)
        .map_err(|e| e.to_string())?;
    if std::env::args().any(|a| a == "--report") {
        for item in &geometry.report.items {
            eprintln!(
                "{} {:?} {:?} {:?}",
                item.reason, item.record_kind, item.source_id, item.arguments
            );
        }
    }
    // product step id -> [volume, area, min x y z, max x y z, items]
    let mut products: BTreeMap<u64, [f64; 9]> = BTreeMap::new();
    for item in &geometry.items {
        let Some(mesh) = item.world_mesh(&limits).map_err(|e| e.to_string())? else {
            continue;
        };
        let (mut v, mut a) = (0.0, 0.0);
        for t in mesh.triangles() {
            let p = |k: u32| {
                mesh.positions()
                    .get(k as usize)
                    .map_or([0.0; 3], |p| [p.x, p.y, p.z])
            };
            let (x, y, z) = (p(t[0]), p(t[1]), p(t[2]));
            let (u, w) = (sub(y, x), sub(z, x));
            let n = cross(u, w);
            a += 0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let c = cross(y, z);
            v += (x[0] * c[0] + x[1] * c[1] + x[2] * c[2]) / 6.0;
        }
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

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
