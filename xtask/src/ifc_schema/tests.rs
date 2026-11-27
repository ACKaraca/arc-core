//! Tests on the published schemas. They need the files: set `ARC_IFC_SCHEMA_DIR` to the directory
//! holding them; without it they skip with a message.

use std::path::PathBuf;

use super::ast::{AttrName, Schema, TypeBody};
use super::{SOURCES, Source, emit, output_dir, parser, read_source};

fn schema_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("ARC_IFC_SCHEMA_DIR").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: set ARC_IFC_SCHEMA_DIR to the directory of the published IFC schemas");
    }
    dir
}

fn parsed(dir: &std::path::Path, source: &Source) -> Schema {
    parser::parse(&read_source(dir, source).unwrap()).unwrap()
}

/// Declaration counts in the order of [`EXPECTED`].
fn counts(s: &Schema) -> [usize; 9] {
    let enums = s
        .types
        .iter()
        .filter(|t| matches!(t.body, TypeBody::Enumeration { .. }))
        .count();
    let selects = s
        .types
        .iter()
        .filter(|t| matches!(t.body, TypeBody::Select { .. }))
        .count();
    let entity_where: usize = s.entities.iter().map(|e| e.where_rules.len()).sum();
    let type_where: usize = s.types.iter().map(|t| t.where_rules.len()).sum();
    let redeclared = s
        .entities
        .iter()
        .flat_map(|e| &e.derived)
        .filter(|d| matches!(d.name, AttrName::Redeclared { .. }))
        .count();
    [
        s.entities.len(),
        s.entities.iter().map(|e| e.explicit.len()).sum(),
        s.entities.iter().map(|e| e.inverse.len()).sum(),
        entity_where + type_where + s.rules.len(),
        s.functions.len(),
        enums,
        selects,
        s.entities.iter().filter(|e| e.is_abstract).count(),
        redeclared,
    ]
}

/// Entities, explicit attributes, inverse attributes, WHERE and global rules, functions,
/// enumerations, selects, abstract entities, explicit attributes re-declared as derived; for
/// IFC2X3 TC1, IFC4 ADD2 TC1 and IFC4X3 ADD2 (docs/architecture/ifc-schema-support.md).
const EXPECTED: [[usize; 9]; 3] = [
    [653, 1320, 115, 365, 38, 164, 46, 97, 7],
    [776, 1491, 153, 679, 47, 207, 60, 123, 8],
    [876, 1644, 165, 779, 48, 243, 61, 133, 8],
];

#[test]
fn published_schemas_have_the_expected_counts() {
    let Some(dir) = schema_dir() else { return };
    for (source, expected) in SOURCES.iter().zip(EXPECTED) {
        assert_eq!(counts(&parsed(&dir, source)), expected, "{}", source.file);
    }
}

#[test]
fn published_schemas_emit_and_match_the_generated_files() {
    let Some(dir) = schema_dir() else { return };
    for source in &SOURCES {
        let code = super::generate(&dir, source).unwrap();
        let path = output_dir().join(format!("{}.rs", source.table.to_ascii_lowercase()));
        if let Ok(on_disk) = std::fs::read_to_string(&path) {
            assert!(on_disk == code, "{} is out of date", path.display());
        }
    }
}

#[test]
fn a_changed_schema_file_is_refused() {
    let dir = std::env::temp_dir().join(format!("arc-ifc-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(SOURCES[1].file), "SCHEMA IFC4; END_SCHEMA;").unwrap();
    let error = read_source(&dir, &SOURCES[1]).unwrap_err();
    assert!(error.contains("BLAKE3"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn emission_resolves_names_and_redeclarations() {
    let schema = parser::parse(
        "SCHEMA T;\n\
         TYPE L = STRING(255); END_TYPE;\n\
         TYPE S = SELECT (B, L); END_TYPE;\n\
         TYPE K = ENUMERATION OF (A, B2); END_TYPE;\n\
         ENTITY B ABSTRACT SUPERTYPE OF (ONEOF (C)); Dim : INTEGER; Pts : LIST [1:?] OF ARRAY [1:3] OF L; END_ENTITY;\n\
         ENTITY C SUBTYPE OF (B); DERIVE SELF\\B.Dim : INTEGER := 3; INVERSE Us : SET [0:2] OF B FOR Dim; WHERE W1 : Dim > 0; END_ENTITY;\n\
         END_SCHEMA;",
    )
    .unwrap();
    let code = emit::emit(&schema, "T", "t.exp", "0").unwrap();
    assert!(code.contains("supertype: Some(0)"), "{code}");
    assert!(code.contains("one_of: &[1]"));
    assert!(code.contains("redeclares: Some(AttributeRef { entity: 0, index: 0 })"));
    assert!(code.contains("TypeRef::String { width: Some(255), fixed: false }"));
    assert!(code.contains("members: &[Named::Entity(0), Named::Defined(0)]"));
    assert!(code.contains("lower: Bound::Value(0), upper: Bound::Value(2)"));
    assert!(code.contains("label: Some(\"W1\"), source: \"Dim > 0\""));

    let unknown =
        parser::parse("SCHEMA T; ENTITY A; X : Missing; END_ENTITY; END_SCHEMA;").unwrap();
    assert!(
        emit::emit(&unknown, "T", "t", "0")
            .unwrap_err()
            .contains("Missing")
    );
    let twice =
        parser::parse("SCHEMA T; TYPE A = INTEGER; END_TYPE; ENTITY a; END_ENTITY; END_SCHEMA;")
            .unwrap();
    assert!(
        emit::emit(&twice, "T", "t", "0")
            .unwrap_err()
            .contains("twice")
    );
}
