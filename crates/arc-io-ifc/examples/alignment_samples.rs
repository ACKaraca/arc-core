//! Prints samples of the curves and alignments of an IFC file as JSON, for the oracle in
//! `tools/ifc-oracle/alignment_compare.py`:
//! `cargo run -p arc-io-ifc --example alignment_samples <file> [--samples N]`.
//! For every composite, gradient and segmented reference curve and for the layout of every
//! alignment, `N` + 1 distances from 0 to the length (default 200) with the frame at each, in
//! meters, row-major; and every `IfcLinearPlacement` as a world transform. Not part of any shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool of the oracle"
)]

use arc_base::{Cancel, Limits, Tolerance};
use arc_io_ifc::alignment::Curve;
use arc_io_ifc::{ReadOptions, read};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let samples = args
        .iter()
        .position(|a| a == "--samples")
        .and_then(|k| args.get(k + 1))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(200);
    let Some(path) = args
        .iter()
        .find(|a| !a.starts_with("--") && a.parse::<f64>().is_err())
    else {
        eprintln!("usage: alignment_samples <file.ifc> [--samples N]");
        std::process::exit(2);
    };
    if let Err(message) = run(path, samples) {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn json_text(value: Option<&String>) -> String {
    value.map_or_else(|| "null".to_owned(), |v| format!("\"{v}\""))
}

fn sampled(curve: &Curve, samples: usize) -> String {
    let length = curve.length();
    let mut out = Vec::new();
    for i in 0..=samples {
        let d = length * i as f64 / samples.max(1) as f64;
        let Ok(pose) = curve.evaluate(d, 1e-6) else {
            continue;
        };
        let m = pose.transform.matrix();
        let kinds = curve.segment_kinds(d);
        out.push(format!(
            "{{\"d\":{d:?},\"m\":[{}],\"plan\":{},\"gradient\":{},\"cant\":{}}}",
            (0..3)
                .flat_map(|r| (0..4).map(move |c| (r, c)))
                .map(|(r, c)| format!("{:?}", m[(r, c)]))
                .collect::<Vec<_>>()
                .join(","),
            json_text(kinds.plan.as_ref()),
            json_text(kinds.gradient.as_ref()),
            json_text(kinds.cant.as_ref()),
        ));
    }
    format!("[{}]", out.join(","))
}

fn run(path: &str, samples: usize) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let output = read(&bytes, &mut ReadOptions::default()).map_err(|e| e.to_string())?;
    let file = output.file;
    let (limits, cancel) = (Limits::default(), Cancel::new());
    let mut entries = Vec::new();
    for kind in [
        "IFCCOMPOSITECURVE",
        "IFCGRADIENTCURVE",
        "IFCSEGMENTEDREFERENCECURVE",
    ] {
        for instance in file.instances_of(kind) {
            match file.curve(instance.name, &limits, &cancel) {
                Ok(curve) => entries.push(format!(
                    "{{\"kind\":\"curve\",\"item\":{},\"type\":\"{kind}\",\"length\":{:?},\"samples\":{}}}",
                    instance.name,
                    curve.length(),
                    sampled(&curve, samples)
                )),
                Err(e) => entries.push(format!(
                    "{{\"kind\":\"curve\",\"item\":{},\"type\":\"{kind}\",\"error\":\"{}\"}}",
                    instance.name,
                    e.code()
                )),
            }
        }
    }
    let alignments = file
        .alignments(&limits, &cancel)
        .map_err(|e| e.to_string())?;
    for a in &alignments.items {
        if let Some(curve) = &a.layout {
            entries.push(format!(
                "{{\"kind\":\"layout\",\"alignment\":{},\"item\":{},\"length\":{:?},\"samples\":{}}}",
                a.alignment,
                a.representation_item.map_or_else(|| "null".to_owned(), |n| n.to_string()),
                curve.length(),
                sampled(curve, samples)
            ));
        }
    }
    let mut resolver = file
        .linear_resolver(&limits, &cancel, &Tolerance::DEFAULT)
        .map_err(|e| e.to_string())?;
    let names: Vec<u64> = file
        .instances_of("IFCLINEARPLACEMENT")
        .map(|i| i.name)
        .collect();
    for name in names {
        match resolver.placement(name) {
            Ok(t) => entries.push(format!(
                "{{\"kind\":\"placement\",\"item\":{name},\"m\":[{}]}}",
                (0..3)
                    .flat_map(|r| (0..4).map(move |c| (r, c)))
                    .map(|(r, c)| format!("{:?}", t.matrix()[(r, c)]))
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            Err(e) => entries.push(format!(
                "{{\"kind\":\"placement\",\"item\":{name},\"error\":\"{}\"}}",
                e.code()
            )),
        }
    }
    println!("[{}]", entries.join(",\n"));
    Ok(())
}
