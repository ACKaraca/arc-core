//! `cargo xtask ifc-schema --schema-dir <dir> [--check]`: generates the IFC schema tables of
//! `arc-io-ifc` (ADR-0018) from the published EXPRESS schemas.
//!
//! The schema files are read from `<dir>`, never downloaded, and each must match its pinned
//! BLAKE3 digest. The output is one file per schema under
//! `crates/arc-io-ifc/src/schema/generated/`; `--check` regenerates in memory and fails when a
//! file differs.

pub mod ast;
pub mod emit;
pub mod lexer;
pub mod parser;
pub mod trees;

use std::fs;
use std::path::{Path, PathBuf};

use crate::workspace::{self, Failure};

/// A published schema the generator accepts.
pub struct Source {
    /// The file name in the schema directory.
    pub file: &'static str,
    /// The BLAKE3 digest of the published file.
    pub blake3: &'static str,
    /// The static in the generated file and the stem of its name.
    pub table: &'static str,
}

/// IFC2X3 TC1, IFC4 ADD2 TC1 and IFC4X3 ADD2 as published by buildingSMART International.
pub const SOURCES: [Source; 3] = [
    Source {
        file: "IFC2X3_TC1.exp",
        blake3: "459180dfdf194d1ebad81ae641d4472e8bbf65aac503d4d45369f08ca18859f1",
        table: "IFC2X3",
    },
    Source {
        file: "IFC4.exp",
        blake3: "cd04dc1bf2db3f88947a7be1b9cfdea14229be69cb15cdcc743d4b266c29789e",
        table: "IFC4",
    },
    Source {
        file: "IFC4X3_ADD2.exp",
        blake3: "57a975a98ce9402c06d3702de93d32175a79de7bb923a7e8488286bce93fa6b0",
        table: "IFC4X3",
    },
];

/// Largest schema file read.
const MAX_FILE_BYTES: u64 = parser::MAX_SOURCE_BYTES as u64;

pub fn output_dir() -> PathBuf {
    workspace::root().join("crates/arc-io-ifc/src/schema/generated")
}

/// Reads one schema file and checks its digest.
pub fn read_source(dir: &Path, source: &Source) -> Result<String, Failure> {
    let path = dir.join(source.file);
    let size = fs::metadata(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(format!(
            "{}: larger than {MAX_FILE_BYTES} bytes",
            path.display()
        ));
    }
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let digest = blake3::hash(&bytes).to_hex();
    if digest.as_str() != source.blake3 {
        return Err(format!(
            "{}: BLAKE3 {digest} differs from the pinned {} (not the published file)",
            path.display(),
            source.blake3
        ));
    }
    String::from_utf8(bytes).map_err(|e| format!("{}: not UTF-8: {e}", path.display()))
}

/// Parses and emits one schema.
pub fn generate(dir: &Path, source: &Source) -> Result<String, Failure> {
    let text = read_source(dir, source)?;
    let schema = parser::parse(&text).map_err(|e| format!("{}: {e}", source.file))?;
    emit::emit(&schema, source.table, source.file, source.blake3)
        .map_err(|e| format!("{}: {e}", source.file))
}

/// Runs the task with the arguments after `ifc-schema`.
pub fn run(args: &[String]) -> Result<(), Failure> {
    let mut dir = None;
    let mut check = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--schema-dir" => dir = iter.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let Some(dir) = dir else {
        return Err("usage: cargo xtask ifc-schema --schema-dir <dir> [--check]".to_owned());
    };
    let out = output_dir();
    let mut stale = Vec::new();
    for source in &SOURCES {
        let code = generate(&dir, source)?;
        let path = out.join(format!("{}.rs", source.table.to_ascii_lowercase()));
        if check {
            if fs::read_to_string(&path).ok().as_deref() != Some(code.as_str()) {
                stale.push(path.display().to_string());
            }
        } else {
            fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            fs::write(&path, code).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("ifc-schema: wrote {}", path.display());
        }
    }
    if stale.is_empty() {
        if check {
            println!("ifc-schema: generated tables are up to date");
        }
        Ok(())
    } else {
        Err(format!("out of date: {}", stale.join(", ")))
    }
}

#[cfg(test)]
mod tests;
