//! Enables the generated IFC schema tables (`cfg(arc_ifc_schema_tables)`) when
//! `src/schema/generated/` holds the files of all three schemas. They are produced by
//! `cargo xtask ifc-schema` and not stored in the repository until the license basis of ADR-0018
//! is recorded; without them the crate builds and `SchemaTables::of` returns `None`.

// Build scripts talk to cargo on standard output.
#![allow(clippy::print_stdout)]

use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(arc_ifc_schema_tables)");
    println!("cargo::rerun-if-changed=src/schema");
    let dir = Path::new("src/schema/generated");
    if ["ifc2x3.rs", "ifc4.rs", "ifc4x3.rs"]
        .iter()
        .all(|f| dir.join(f).is_file())
    {
        println!("cargo::rustc-cfg=arc_ifc_schema_tables");
    }
}
