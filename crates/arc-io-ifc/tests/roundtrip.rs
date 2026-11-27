//! Round trip of read files: without edits the written file is the source byte for byte; an edit
//! changes only the lines of the edited instances; edits keep the instance names and refuse
//! dangling references; IFC-ZIP archives written by the crate read back; truncated and corrupted
//! files never panic.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

#[path = "support/mod.rs"]
mod support;

use arc_base::Limits;
use arc_io_ifc::{IfcFile, Parameter, ReadOptions, Record, ifc_zip, read};
use proptest::prelude::*;

fn file_of(bytes: &[u8]) -> IfcFile {
    read(bytes, &mut ReadOptions::default()).unwrap().file
}

fn written(file: &IfcFile) -> String {
    String::from_utf8(file.write_step(&Limits::LARGE_MODELS).unwrap()).unwrap()
}

/// The lines of `after` that differ from `before`, as (line number, before, after); both must have
/// the same number of lines.
fn changed_lines<'a>(before: &'a str, after: &'a str) -> Vec<(usize, &'a str, &'a str)> {
    let (old, new): (Vec<_>, Vec<_>) = (before.lines().collect(), after.lines().collect());
    assert_eq!(old.len(), new.len(), "{after}");
    old.into_iter()
        .zip(new)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (a, b))| (i, a, b))
        .collect()
}

/// Files with every feature of the clear text encoding that lies outside the instances.
fn edge_cases() -> Vec<(&'static str, Vec<u8>)> {
    let project = support::project("IFC4");
    let crlf = project.replace('\n', "\r\n");
    let commented = project
        .replace("DATA;\n", "DATA;\n/* exported */\n")
        .replace(
            "#8=IFCWALL('7Lj9hJ7xJ5rB6tPcY4E5iU',$,'Wall A'",
            "#8 = IFCWALL ( '7Lj9hJ7xJ5rB6tPcY4E5iU' , $ ,/* name */ 'Wall A'",
        );
    let mut bom = b"\xEF\xBB\xBF".to_vec();
    bom.extend_from_slice(project.as_bytes());
    let mut trailing = project.as_bytes().to_vec();
    trailing.extend_from_slice(b"\0\0\x1Agarbage\n");
    let sections = project.replace(
        "ENDSEC;\nEND-ISO-10303-21;",
        "ENDSEC;\nANCHOR;\nENDSEC;\nDATA;\n#100=IFCOWNERHISTORY(*,$,.A.,\"3F\",(#1,#2),-1.5E-3);\n\
         #101=(IFCA(1)IFCB('\\X2\\00E7\\X0\\\\S\\a'));\nENDSEC;\nEND-ISO-10303-21;",
    );
    let packed = "ISO-10303-21;HEADER;FILE_DESCRIPTION(('x'),'2;1');FILE_NAME('a','',(''),(''),'','','');\
                  FILE_SCHEMA(('IFC4X3_ADD2'));ENDSEC;DATA;#1=IFCX($);#2=IFCY(#1);ENDSEC;END-ISO-10303-21;";
    vec![
        ("project-ifc2x3", support::project("IFC2X3").into_bytes()),
        ("project-ifc4", project.clone().into_bytes()),
        (
            "project-ifc4x3",
            support::project("IFC4X3_ADD2").into_bytes(),
        ),
        ("crlf", crlf.into_bytes()),
        ("comments", commented.into_bytes()),
        ("byte-order-mark", bom),
        ("trailing", trailing),
        ("sections", sections.into_bytes()),
        ("one-line", packed.as_bytes().to_vec()),
    ]
}

#[test]
fn unedited_files_are_written_byte_for_byte() {
    for (name, bytes) in edge_cases() {
        let file = file_of(&bytes);
        assert_eq!(
            file.write_step(&Limits::CONSTRAINED).unwrap(),
            bytes,
            "{name}"
        );
        // The shared fixture is a minimal layout for the views, not a conformant file.
        support::dump_nonconformant(&format!("roundtrip-{name}.ifc"), &bytes);
    }
}

#[test]
fn instance_spans_point_into_the_source() {
    let text = support::project("IFC4");
    let file = file_of(text.as_bytes());
    let span = file.span(8).unwrap();
    let source = file.source_text(8).unwrap();
    assert!(source.starts_with(b"#8=IFCWALL("));
    assert!(source.ends_with(b";"));
    assert_eq!(
        &text.as_bytes()[usize::try_from(span.start).unwrap()..usize::try_from(span.end).unwrap()],
        source
    );
    assert_eq!(file.span(999), None);
}

#[test]
fn one_changed_attribute_changes_one_line() {
    let text = support::project("IFC4");
    let mut file = file_of(text.as_bytes());
    file.set_attribute(8, 0, 2, Parameter::Text("Wall A\u{e7} 'renamed'".into()))
        .unwrap();
    assert_eq!(file.span(8), None);
    let after = written(&file);
    support::dump_nonconformant("roundtrip-edited.ifc", after.as_bytes());
    assert_eq!(
        changed_lines(&text, &after),
        [(
            14,
            "#8=IFCWALL('7Lj9hJ7xJ5rB6tPcY4E5iU',$,'Wall A','North elevation',$,$,$,$,$);",
            "#8=IFCWALL('7Lj9hJ7xJ5rB6tPcY4E5iU',$,'Wall A\\X2\\00E7\\X0\\ ''renamed''','North elevation',$,$,$,$,$);"
        )]
    );
    let again = file_of(after.as_bytes());
    assert_eq!(again.instance(8), file.instance(8));
}

#[test]
fn an_edit_keeps_the_line_breaks_and_comments_around_it() {
    let (_, bytes) = edge_cases()
        .into_iter()
        .find(|(n, _)| *n == "comments")
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let mut file = file_of(text.as_bytes());
    file.set_attribute(9, 0, 3, Parameter::Text("x".into()))
        .unwrap();
    let after = written(&file);
    assert!(after.contains("/* exported */"));
    assert!(after.contains("/* name */"));
    assert_eq!(changed_lines(&text, &after).len(), 1);

    let crlf = support::project("IFC4").replace('\n', "\r\n");
    let mut file = file_of(crlf.as_bytes());
    file.set_attribute(9, 0, 3, Parameter::Real(1e-5)).unwrap();
    let name = file
        .add(vec![Record {
            type_name: "IFCWALL".into(),
            parameters: vec![Parameter::Reference(9)],
        }])
        .unwrap();
    let after = written(&file);
    assert!(
        after.contains("#9=IFCWALL('8Mk0iK8yK6sC7uQdZ5F6jV',$,'Wall B',1.E-05,$,$,$,$,$);\r\n")
    );
    assert!(after.contains(&format!("\r\n#{name}=IFCWALL(#9);\r\nENDSEC;\r\nEND-ISO")));
    assert!(!after.replace("\r\n", "").contains('\n'));
}

#[test]
fn every_parameter_kind_is_written_canonically() {
    let text = support::project("IFC4");
    let mut file = file_of(text.as_bytes());
    let records = vec![
        Record {
            type_name: "IFCA".into(),
            parameters: vec![
                Parameter::Integer(-3),
                Parameter::Real(0.1),
                Parameter::Real(2.0),
                Parameter::Real(-1.5e300),
                Parameter::Text("a\\b \u{1F600} ğ".into()),
                Parameter::Enumeration("T".into()),
                Parameter::Binary("2FA".into()),
                Parameter::Omitted,
                Parameter::Derived,
                Parameter::Typed {
                    type_name: "IFCLABEL".into(),
                    value: Box::new(Parameter::Text("x".into())),
                },
                Parameter::List(vec![Parameter::Reference(1), Parameter::List(Vec::new())]),
            ],
        },
        Record {
            type_name: "IFCB".into(),
            parameters: Vec::new(),
        },
    ];
    file.replace(30, records.clone()).unwrap();
    let after = written(&file);
    assert!(after.contains(
        "#30=(IFCA(-3,0.1,2.,-1.5E+300,'a\\\\b \\X4\\0001F600\\X0\\ \\X2\\011F\\X0\\',.T.,\"2FA\",$,*,IFCLABEL('x'),(#1,()))IFCB());\n"
    ));
    assert_eq!(changed_lines(&text, &after).len(), 1);
    assert_eq!(
        file_of(after.as_bytes()).instance(30).unwrap().records,
        records
    );
    assert_eq!(file.instances_of("IFCSIUNIT").count(), 0);
    assert_eq!(file.instances_of("IFCB").count(), 1);
}

#[test]
fn added_instances_get_names_above_every_name_held() {
    let text = support::project("IFC4");
    let mut file = file_of(text.as_bytes());
    let wall = |reference| Record {
        type_name: "IFCWALL".into(),
        parameters: vec![Parameter::Reference(reference), Parameter::Omitted],
    };
    let first = file.add(vec![wall(8)]).unwrap();
    assert_eq!(first, 46);
    file.remove(first).unwrap();
    let second = file.add(vec![wall(9)]).unwrap();
    assert_eq!(second, 47);
    assert_eq!(file.instances_of("IFCWALL").count(), 3);
    let after = written(&file);
    assert!(after.ends_with("#47=IFCWALL(#9,$);\nENDSEC;\nEND-ISO-10303-21;\n"));
    assert!(!after.contains("#46="));
    let again = file_of(after.as_bytes());
    assert_eq!(again.instance(47), file.instance(47));
}

#[test]
fn edits_never_leave_a_dangling_reference() {
    let text = support::project("IFC4");
    let mut file = file_of(text.as_bytes());
    let code = |e: arc_base::Error| e.code().to_owned();
    let dangling = Record {
        type_name: "IFCX".into(),
        parameters: vec![Parameter::List(vec![Parameter::Reference(999)])],
    };
    assert_eq!(
        code(file.add(vec![dangling]).unwrap_err()),
        "io.ifc.edit_reference_dangling"
    );
    assert_eq!(
        code(
            file.set_attribute(8, 0, 4, Parameter::Reference(31))
                .unwrap_err()
        ),
        "io.ifc.edit_reference_dangling"
    );
    // #30 is the unit of the bounded value and of a quantity.
    assert_eq!(
        code(file.remove(30).unwrap_err()),
        "io.ifc.edit_reference_dangling"
    );
    assert_eq!(
        code(file.remove(1000).unwrap_err()),
        "io.ifc.edit_target_missing"
    );
    assert_eq!(
        code(
            file.set_attribute(8, 0, 99, Parameter::Omitted)
                .unwrap_err()
        ),
        "io.ifc.edit_target_missing"
    );
    assert_eq!(
        code(file.set_attribute(8, 1, 0, Parameter::Omitted).unwrap_err()),
        "io.ifc.edit_target_missing"
    );
    for value in [
        Parameter::Real(f64::NAN),
        Parameter::Text("a\0".into()),
        Parameter::Enumeration("A.B".into()),
        Parameter::Binary("4F".into()),
        Parameter::Typed {
            type_name: "ifclabel".into(),
            value: Box::new(Parameter::Omitted),
        },
    ] {
        assert_eq!(
            code(file.set_attribute(8, 0, 4, value).unwrap_err()),
            "io.ifc.edit_value_invalid"
        );
    }
    assert_eq!(
        code(file.add(Vec::new()).unwrap_err()),
        "io.ifc.edit_value_invalid"
    );
    // Nothing was changed by the refused edits.
    assert_eq!(written(&file), text);

    // A removal drops the line; the references to the removed instance go first.
    file.set_attribute(24, 0, 4, Parameter::Omitted).unwrap();
    file.set_attribute(40, 0, 2, Parameter::Omitted).unwrap();
    let unit = file.remove(30).unwrap();
    assert_eq!(unit.records[0].type_name, "IFCSIUNIT");
    let after = written(&file);
    assert_eq!(after.lines().count() + 1, text.lines().count());
    assert!(!after.contains("#30="));
    assert!(!file_of(after.as_bytes()).contains(30));
}

#[test]
fn header_records_are_kept_and_editable() {
    let text = support::project("IFC4");
    let mut file = file_of(text.as_bytes());
    assert_eq!(file.header[0].type_name, "FILE_DESCRIPTION");
    file.header[0].parameters[0] = Parameter::List(vec![Parameter::Text(
        "ViewDefinition [ReferenceView_V1.2]".into(),
    )]);
    file.header[2].parameters[0] = Parameter::List(vec![Parameter::Text("IFC4X3_ADD2".into())]);
    let after = written(&file);
    let changed = changed_lines(&text, &after);
    assert_eq!(
        changed.iter().map(|(_, _, line)| *line).collect::<Vec<_>>(),
        [
            "FILE_DESCRIPTION(('ViewDefinition [ReferenceView_V1.2]'),'2;1');",
            "FILE_SCHEMA(('IFC4X3_ADD2'));"
        ]
    );
    let again = file_of(after.as_bytes());
    assert_eq!(again.schema, arc_io_ifc::Schema::Ifc4x3);
    assert_eq!(again.header, file.header);

    file.header.push(Record {
        type_name: "FILE_POPULATION".into(),
        parameters: vec![Parameter::Text("x".into())],
    });
    let after = written(&file);
    assert!(after.contains("FILE_SCHEMA(('IFC4X3_ADD2'));\nFILE_POPULATION('x');\nENDSEC;"));

    file.header.remove(2);
    let error = file.write_step(&Limits::LARGE_MODELS).unwrap_err();
    assert_eq!(error.code(), "io.ifc.edit_value_invalid");
}

#[test]
fn ifc_zip_written_by_the_crate_reads_back() {
    let text = support::project("IFC4");
    let archive = ifc_zip("project.ifc", text.as_bytes()).unwrap();
    assert!(archive.len() < text.len());
    let file = file_of(&archive);
    assert_eq!(file, file_of(text.as_bytes()));
    assert_eq!(
        file.write_step(&Limits::CONSTRAINED).unwrap(),
        text.as_bytes()
    );
    support::dump_nonconformant("roundtrip.ifczip", &archive);
    for bad in ["project.txt", "__MACOSX/project.ifc", "._project.ifc"] {
        assert_eq!(
            ifc_zip(bad, text.as_bytes()).unwrap_err().code(),
            "io.ifc.archive_entry_missing"
        );
    }
    assert_eq!(
        ifc_zip("../project.ifc", b"").unwrap_err().code(),
        "io.zip.path_unsafe"
    );
}

/// A truncated or corrupted file reads to an error or to a file that writes back byte for byte.
fn assert_round_trip_or_error(bytes: &[u8]) {
    match read(bytes, &mut ReadOptions::default()) {
        Ok(output) => assert_eq!(
            output.file.write_step(&Limits::LARGE_MODELS).unwrap(),
            bytes
        ),
        Err(error) => assert!(error.code().starts_with("io."), "{error:?}"),
    }
}

#[test]
fn truncation_at_every_byte_never_panics() {
    for (_, bytes) in edge_cases() {
        for end in 0..bytes.len() {
            assert_round_trip_or_error(&bytes[..end]);
        }
    }
}

proptest! {
    #[test]
    fn flipped_bytes_never_panic(flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..6)) {
        let mut bytes = support::project("IFC4").into_bytes();
        for (at, value) in flips {
            let at = at % bytes.len();
            bytes[at] = value;
        }
        assert_round_trip_or_error(&bytes);
    }
}

/// Reads every `.ifc` file in the directory named by `ARC_IFC_ROUNDTRIP_DIR` (recursively) and
/// checks that it writes back byte for byte; does nothing when the variable is not set. Used on
/// the files of `ARC_IFC_DUMP` and on sample corpora that are not part of the repository.
#[test]
fn files_of_a_directory_round_trip() {
    let Some(dir) = std::env::var_os("ARC_IFC_ROUNDTRIP_DIR") else {
        return;
    };
    let mut pending = vec![std::path::PathBuf::from(dir)];
    let (mut total, mut identical, mut failures) = (0, 0, Vec::new());
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if !matches!(ext.as_deref(), Some("ifc" | "ifczip")) {
            continue;
        }
        total += 1;
        let bytes = std::fs::read(&path).unwrap();
        let options = &mut ReadOptions {
            limits: Limits::LARGE_MODELS,
            ..ReadOptions::default()
        };
        match read(&bytes, options) {
            Ok(output) => {
                let out = output.file.write_step(&Limits::LARGE_MODELS).unwrap();
                if ext.as_deref() == Some("ifczip") || out == bytes {
                    identical += 1;
                } else {
                    failures.push(format!("{}: written bytes differ", path.display()));
                }
            }
            Err(error) => failures.push(format!("{}: {}", path.display(), error.code())),
        }
    }
    assert!(
        failures.is_empty(),
        "{total} files, {identical} byte-identical; failures: {failures:#?}"
    );
}
