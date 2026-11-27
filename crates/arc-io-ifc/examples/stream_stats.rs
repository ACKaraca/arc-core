//! Streaming read against the in-memory reader on real files:
//! `cargo run --release -p arc-io-ifc --example stream_stats -- <mode> <file>...` with the mode
//! `stream` (index the file from a `Read`, then parse every instance on demand), `memory` (the
//! in-memory reader) or `compare` (both, instance by instance, spans and type lists; exit status 1
//! on any difference). Prints per file the instance count, the time and, on Linux, the peak
//! resident set of the process (`VmHWM`), so run one file per process for memory figures. Not
//! part of any shipped binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line tool of the oracle"
)]

use std::fs::File;
use std::time::Instant;

use arc_base::{Cancel, Limits};
use arc_io_ifc::stream::{StreamFile, StreamLimits};
use arc_io_ifc::{ReadOptions, read};

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn limits() -> Limits {
    // The profile for trusted local files: large enough for the biggest sample.
    Limits {
        input_bytes: 1 << 33,
        record_count: 1 << 32,
        memory_bytes: None,
        ..Limits::DEFAULT
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((mode, files)) = args.split_first() else {
        eprintln!("usage: stream_stats <stream|memory|compare> <file>...");
        std::process::exit(2);
    };
    let mut failed = false;
    for path in files {
        let start = Instant::now();
        let result = match mode.as_str() {
            "stream" => stream(path),
            "memory" => memory(path),
            _ => compare(path),
        };
        match result {
            Ok(text) => println!(
                "{mode} {path}: {text}, {:.2} s, peak {} MiB",
                start.elapsed().as_secs_f64(),
                peak_rss_kib().unwrap_or(0) / 1024
            ),
            Err(message) => {
                failed = true;
                println!("{mode} {path}: DIFFERENT {message}");
            }
        }
    }
    std::process::exit(i32::from(failed));
}

fn stream(path: &str) -> Result<String, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut stream = StreamFile::open(
        std::io::BufReader::new(file),
        &limits(),
        &StreamLimits::default(),
        &Cancel::new(),
    )
    .map_err(|e| e.code().to_owned())?;
    let names: Vec<u64> = stream.index().names().collect();
    let mut records = 0usize;
    for name in &names {
        let instance = stream.instance(*name).map_err(|e| e.code().to_owned())?;
        records += instance.map_or(0, |i| i.records.len());
    }
    let summary = stream.index().summary();
    Ok(format!(
        "{} instances, {records} records, window {} KiB, index {} MiB",
        names.len(),
        summary.peak_window_bytes / 1024,
        stream.index().estimated_bytes() / (1024 * 1024)
    ))
}

fn memory(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut options = ReadOptions {
        limits: limits(),
        ..ReadOptions::default()
    };
    let output = read(&bytes, &mut options).map_err(|e| e.code().to_owned())?;
    Ok(format!("{} instances", output.file.instances().len()))
}

fn compare(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut options = ReadOptions {
        limits: limits(),
        ..ReadOptions::default()
    };
    let loaded = read(&bytes, &mut options);
    let streamed = StreamFile::open(
        std::io::Cursor::new(&bytes),
        &limits(),
        &StreamLimits::default(),
        &Cancel::new(),
    );
    match (loaded, streamed) {
        (Ok(loaded), Ok(mut streamed)) => {
            let file = loaded.file;
            if streamed.index().len() != file.instances().len() {
                return Err("instance count".to_owned());
            }
            if streamed.index().schema() != &file.schema {
                return Err("schema".to_owned());
            }
            let header: Vec<_> = streamed
                .index()
                .summary()
                .header
                .iter()
                .map(|(r, _)| r.clone())
                .collect();
            if header != file.header {
                return Err("header".to_owned());
            }
            for instance in file.instances() {
                let got = streamed
                    .instance(instance.name)
                    .map_err(|e| e.code().to_owned())?;
                if got.as_ref() != Some(instance) {
                    return Err(format!("instance #{}", instance.name));
                }
                if streamed.index().span(instance.name) != file.span(instance.name) {
                    return Err(format!("span of #{}", instance.name));
                }
            }
            Ok(format!("{} instances equal", file.instances().len()))
        }
        (Err(a), Err(b)) => Ok(format!("both fail ({} / {})", a.code(), b.code())),
        (Ok(_), Err(b)) => Err(format!("only the stream fails: {}", b.code())),
        (Err(a), Ok(_)) => Err(format!("only the reader fails: {}", a.code())),
    }
}
