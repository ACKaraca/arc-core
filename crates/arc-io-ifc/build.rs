//! Enables the generated IFC schema tables (`cfg(arc_ifc_schema_tables)`) when
//! `src/schema/generated/` holds the files of all three schemas, and the generated property and
//! quantity set templates (`cfg(arc_ifc_pset_tables)`) when it holds those of IFC2X3 and IFC4.
//! They are produced by `cargo xtask ifc-schema` and `cargo xtask ifc-psets` and not stored in the
//! repository until the license basis of ADR-0018 is recorded; without them the crate builds and
//! `SchemaTables::of` and `PsetTemplates::of` return `None`.

// Build scripts talk to cargo on standard output.
#![allow(clippy::print_stdout)]

use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(arc_ifc_schema_tables)");
    println!("cargo::rustc-check-cfg=cfg(arc_ifc_pset_tables)");
    println!("cargo::rerun-if-changed=src/schema");
    let dir = Path::new("src/schema/generated");
    let all = |files: &[&str]| files.iter().all(|f| dir.join(f).is_file());
    if all(&["ifc2x3.rs", "ifc4.rs", "ifc4x3.rs"]) {
        println!("cargo::rustc-cfg=arc_ifc_schema_tables");
    }
    if all(&["psets_ifc2x3.rs", "psets_ifc4.rs"]) {
        println!("cargo::rustc-cfg=arc_ifc_pset_tables");
    }
}
