//! Enables the generated IFC schema tables of each schema (`cfg(arc_ifc_tables_ifc2x3)`,
//! `cfg(arc_ifc_tables_ifc4)`, `cfg(arc_ifc_tables_ifc4x3)`) when `src/schema/generated/` holds its
//! file, and the generated property and quantity set templates (`cfg(arc_ifc_pset_tables)`) when it
//! holds those of IFC2X3 and IFC4. They are produced by `cargo xtask ifc-schema` and
//! `cargo xtask ifc-psets`. Only the IFC4X3 schema tables are stored in the repository (ADR-0018);
//! the others are generated locally. Without a file the crate builds and `SchemaTables::of` and
//! `PsetTemplates::of` return `None` for that schema.

// Build scripts talk to cargo on standard output.
#![allow(clippy::print_stdout)]

use std::path::Path;

fn main() {
    println!("cargo::rerun-if-changed=src/schema");
    let dir = Path::new("src/schema/generated");
    for schema in ["ifc2x3", "ifc4", "ifc4x3"] {
        println!("cargo::rustc-check-cfg=cfg(arc_ifc_tables_{schema})");
        if dir.join(format!("{schema}.rs")).is_file() {
            println!("cargo::rustc-cfg=arc_ifc_tables_{schema}");
        }
    }
    println!("cargo::rustc-check-cfg=cfg(arc_ifc_pset_tables)");
    if ["psets_ifc2x3.rs", "psets_ifc4.rs"]
        .iter()
        .all(|f| dir.join(f).is_file())
    {
        println!("cargo::rustc-cfg=arc_ifc_pset_tables");
    }
}
