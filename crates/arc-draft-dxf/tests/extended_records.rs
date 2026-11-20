//! Extended data of mapped objects, the registered applications, the class records and the
//! dictionaries of a file are kept on import, written back on export under both handle policies,
//! and bounded: a hostile file stays bounded and does not fail the export.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::{Limits, ObjectId};
use arc_draft_dxf::{HandlePolicy, export_drawing_with};
use arc_io_dxf::model::{
    Class, Dictionary, DictionaryEntry, Object, ObjectKind, Table, TableEntry, XData,
};
use arc_io_dxf::{Drawing, Entity, EntityKind, Group, Handle, PreservedRecord, ReadOptions, Value};
use support::{Env, bytes, drawing, line, on_layer, with_handle};

const FILE: &str = "p.dxf";

fn policies() -> Vec<HandlePolicy> {
    vec![
        HandlePolicy::Assign,
        HandlePolicy::KeepSources {
            document: FILE.to_owned(),
            floor: 0,
        },
    ]
}

/// A line on WALLS with extended data of two applications, one holding a control string, a
/// handle and a binary chunk.
fn xdata_line() -> Entity {
    let mut e = with_handle(on_layer(line([0.0; 3], [10.0, 0.0, 0.0]), "WALLS"), 0x20);
    e.xdata = vec![
        XData {
            app: "APP_A".to_owned(),
            groups: vec![
                Group::text(1000, "tag"),
                Group::text(1002, "{"),
                Group::int(1070, 7),
                Group::text(1005, "5A"),
                Group::text(1002, "}"),
            ],
        },
        XData {
            app: "APP_B".to_owned(),
            groups: vec![
                Group::double(1040, 2.5),
                Group {
                    code: 1004,
                    value: Value::Bytes(vec![1, 2, 3]),
                },
            ],
        },
    ];
    e
}

fn custom(kind: &str, handle: &str, owner: &str, extra: Vec<Group>) -> PreservedRecord {
    let mut groups = vec![
        Group::text(5, handle),
        Group::text(330, owner),
        Group::text(100, "AcDbCustom"),
        Group::int(90, 7),
    ];
    groups.extend(extra);
    PreservedRecord {
        kind: kind.to_owned(),
        groups,
    }
}

fn helix() -> Entity {
    let record = PreservedRecord {
        kind: "HELIX".to_owned(),
        groups: vec![
            Group::text(5, "30"),
            Group::text(330, "1F"),
            Group::text(100, "AcDbEntity"),
            Group::text(8, "0"),
            Group::text(102, "{ACAD_REACTORS"),
            Group::text(330, "21"),
            Group::text(102, "}"),
            Group::text(100, "AcDbHelix"),
            Group::text(1001, "APP_X"),
            Group::text(1000, "helix"),
        ],
    };
    Entity::new(EntityKind::Preserved(record))
}

fn dictionary(handle: u64, owner: Option<u64>, entries: &[(&str, u64)]) -> Object {
    let d = Dictionary {
        hard_owner: Some(1),
        cloning: Some(1),
        entries: entries
            .iter()
            .map(|(name, h)| DictionaryEntry {
                name: (*name).to_owned(),
                handle: Handle(*h),
                hard_owner: true,
            })
            .collect(),
    };
    let mut o = Object::new(ObjectKind::Dictionary(d));
    o.head.handle = Some(Handle(handle));
    o.head.owner = owner.map(Handle);
    o
}

/// The file of the tests: the line with extended data, an entity and two objects of a custom
/// class, the root dictionary naming one of them, a nested dictionary cycle, the classes and the
/// application table, which lacks APP_B and APP_X.
fn file() -> Drawing {
    let mut d = drawing(vec![xdata_line(), helix()], Some(4));
    d.classes = vec![
        Class {
            dxf_name: "HELIX".to_owned(),
            class_name: "HELIX_CLASS".to_owned(),
            application: "APP_X".to_owned(),
            proxy_flags: 0,
            instance_count: Some(-5),
            was_proxy: 0,
            is_entity: 1,
            extras: Vec::new(),
        },
        Class {
            dxf_name: "ACDBCUSTOM".to_owned(),
            class_name: "CUSTOM_CLASS".to_owned(),
            application: "APP_X".to_owned(),
            proxy_flags: 0,
            instance_count: Some(9),
            was_proxy: 0,
            is_entity: 0,
            extras: Vec::new(),
        },
    ];
    d.tables.appids = Some(Table {
        entries: vec![
            TableEntry {
                name: "ACAD".to_owned(),
                ..TableEntry::default()
            },
            TableEntry {
                name: "APP_A".to_owned(),
                ..TableEntry::default()
            },
        ],
        ..Table::default()
    });
    // Root dictionary: a layouts entry (the export writes its own), a missing entry and the
    // entry of a custom object (0x40).
    d.objects.push(dictionary(
        0xC,
        None,
        &[
            ("ACAD_LAYOUT", 0x60),
            ("ACAD_GROUP", 0x99),
            ("MYAPP_OBJ", 0x40),
        ],
    ));
    d.objects
        .push(dictionary(0x60, Some(0xC), &[("Model", 0x61)]));
    // A cycle of two dictionaries that name each other.
    d.objects
        .push(dictionary(0x50, Some(0xC), &[("BACK", 0x51)]));
    d.objects
        .push(dictionary(0x51, Some(0x50), &[("FWD", 0x50)]));
    // Custom objects: one owned by the root, one owned by a handle the file does not hold.
    d.objects.push(Object::new(ObjectKind::Preserved(custom(
        "ACDBCUSTOM",
        "40",
        "C",
        vec![Group::text(1001, "APP_X"), Group::text(1000, "kept")],
    ))));
    d.objects.push(Object::new(ObjectKind::Preserved(custom(
        "ACDBCUSTOM",
        "41",
        "1F",
        Vec::new(),
    ))));
    d
}

fn written(d: &Drawing) -> Drawing {
    arc_io_dxf::read(
        &bytes(d),
        &mut ReadOptions {
            limits: Limits::DEFAULT,
            ..ReadOptions::default()
        },
    )
    .unwrap()
    .drawing
}

/// Imports `d` into a new model space and returns the environment, the space and the file bytes
/// that the exports read.
fn imported(d: &Drawing) -> (Env, ObjectId) {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(FILE, &bytes(d));
    env.import(&path, space).unwrap();
    (env, space)
}

fn export(env: &Env, space: ObjectId, policy: &HandlePolicy) -> arc_draft_dxf::export::Exported {
    export_drawing_with(&env.snapshot(), space, policy).unwrap()
}

fn find_line(d: &Drawing) -> &Entity {
    d.entities
        .iter()
        .find(|e| matches!(e.kind, EntityKind::Line(_)))
        .expect("the line is written")
}

fn root_of(d: &Drawing) -> &Object {
    d.objects
        .iter()
        .find(|o| {
            matches!(o.kind, ObjectKind::Dictionary(_)) && o.head.owner.is_none_or(|h| h.0 == 0)
        })
        .expect("the root dictionary is written")
}

/// The handle of the custom object the tests mark as kept (its group 1000 is `kept`).
fn kept_object(d: &Drawing) -> Handle {
    preserved_objects(d, "ACDBCUSTOM")
        .into_iter()
        .find(|o| match &o.kind {
            ObjectKind::Preserved(r) => r.groups.iter().any(|g| g.as_text() == Some("kept")),
            _ => false,
        })
        .and_then(|o| {
            o.head.handle.or_else(|| match &o.kind {
                ObjectKind::Preserved(r) => r.handle(),
                _ => None,
            })
        })
        .expect("the kept object is written")
}

fn preserved_objects<'d>(d: &'d Drawing, kind: &str) -> Vec<&'d Object> {
    d.objects
        .iter()
        .filter(|o| matches!(&o.kind, ObjectKind::Preserved(r) if r.kind == kind))
        .collect()
}

fn reason_count(report: &arc_io_dxf::LossReport, reason: &str) -> u64 {
    report
        .items
        .iter()
        .filter(|i| i.reason == reason)
        .map(|i| match i.arguments.get("count") {
            Some(arc_io_dxf::loss::Argument::Int(n)) => u64::try_from(*n).unwrap_or(0),
            _ => 1,
        })
        .sum()
}

#[test]
fn extended_data_of_a_mapped_entity_is_written_back_exactly() {
    let (env, space) = imported(&file());
    for policy in policies() {
        let exported = export(&env, space, &policy).drawing;
        let back = written(&exported);
        // The handle 5A names no object of the output: it is written as the null handle.
        let mut expected = xdata_line().xdata;
        expected[0].groups[3] = Group::text(1005, "0");
        assert_eq!(find_line(&back).xdata, expected, "{policy:?}");
    }
}

#[test]
fn registered_applications_and_classes_are_written_with_recomputed_counts() {
    let (env, space) = imported(&file());
    for policy in policies() {
        let result = export(&env, space, &policy);
        let back = written(&result.drawing);
        let names: Vec<&str> = back
            .tables
            .appids
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            names
                .iter()
                .filter(|n| n.eq_ignore_ascii_case("acad"))
                .count(),
            1
        );
        for app in ["APP_A", "APP_B", "APP_X"] {
            assert!(names.contains(&app), "{app} in {names:?} under {policy:?}");
        }
        assert_eq!(reason_count(&result.report, "draft.dxf.appid_added"), 2);
        let count = |name: &str| {
            back.classes
                .iter()
                .find(|c| c.dxf_name == name)
                .and_then(|c| c.instance_count)
        };
        // Negative and stale counts are replaced by the records the file holds.
        assert_eq!(count("HELIX"), Some(1));
        assert_eq!(count("ACDBCUSTOM"), Some(2));
    }
}

#[test]
fn owners_and_reactors_name_the_written_handles() {
    let (env, space) = imported(&file());
    let keep = HandlePolicy::KeepSources {
        document: FILE.to_owned(),
        floor: 0,
    };
    for policy in policies() {
        let result = export(&env, space, &policy);
        let back = written(&result.drawing);
        let root = root_of(&back).head.handle.unwrap();
        let objects = preserved_objects(&back, "ACDBCUSTOM");
        assert_eq!(objects.len(), 2);
        let owner_of = |o: &Object| match &o.kind {
            ObjectKind::Preserved(r) => r
                .groups
                .iter()
                .find(|g| g.code == 330)
                .and_then(Group::as_handle),
            _ => None,
        };
        let owners: Vec<Option<Handle>> = objects.iter().map(|o| owner_of(o)).collect();
        // Object 0x40 is owned by the root, which the export writes under its own handle.
        assert!(owners.contains(&Some(root)), "{policy:?}: {owners:?}");
        // The owner 0x1F is not written: the root is named, and the substitution is reported.
        assert_eq!(owners.iter().filter(|o| **o == Some(root)).count(), 2);
        assert_eq!(
            reason_count(&result.report, "draft.dxf.owner_substituted"),
            1
        );
        // The reactor 0x21 names no object of the output: it is written as the null handle.
        let helix = back
            .entities
            .iter()
            .find_map(|e| match &e.kind {
                EntityKind::Preserved(r) if r.kind == "HELIX" => Some(r),
                _ => None,
            })
            .expect("the entity is written");
        assert!(
            helix
                .groups
                .iter()
                .any(|g| g.code == 330 && g.as_handle() == Some(Handle(0)))
        );
    }
    // Under KeepSources the custom object keeps its handle.
    let back = written(&export(&env, space, &keep).drawing);
    assert_eq!(
        preserved_objects(&back, "ACDBCUSTOM")
            .iter()
            .filter_map(|o| o.head.handle.or_else(|| match &o.kind {
                ObjectKind::Preserved(r) => r.handle(),
                _ => None,
            }))
            .collect::<Vec<_>>(),
        vec![Handle(0x40), Handle(0x41)]
    );
}

#[test]
fn dictionary_entries_name_the_written_objects_and_cycles_are_kept() {
    let (env, space) = imported(&file());
    for policy in policies() {
        let result = export(&env, space, &policy);
        let back = written(&result.drawing);
        let kept = kept_object(&back);
        let root = root_of(&back);
        let ObjectKind::Dictionary(d) = &root.kind else {
            panic!("a dictionary")
        };
        let entry = |name: &str| d.entries.iter().find(|e| e.name == name).map(|e| e.handle);
        assert_eq!(entry("MYAPP_OBJ"), Some(kept), "{policy:?}");
        assert_eq!(entry("ACAD_GROUP"), None, "a missing target is dropped");
        assert!(
            entry("ACAD_LAYOUT").is_some(),
            "the structural layouts entry stays"
        );
        // The cycle: the two dictionaries name each other through the written handles.
        let dicts: Vec<&Object> = back
            .objects
            .iter()
            .filter(|o| matches!(&o.kind, ObjectKind::Dictionary(d) if d.entries.iter().any(|e| e.name == "BACK" || e.name == "FWD")))
            .collect();
        assert_eq!(dicts.len(), 2);
        let (a, b) = (dicts[0], dicts[1]);
        let ObjectKind::Dictionary(da) = &a.kind else {
            panic!("a dictionary")
        };
        let ObjectKind::Dictionary(db) = &b.kind else {
            panic!("a dictionary")
        };
        assert_eq!(da.entries[0].handle, b.head.handle.unwrap(), "{policy:?}");
        assert_eq!(db.entries[0].handle, a.head.handle.unwrap(), "{policy:?}");
        assert_eq!(a.head.owner, Some(root.head.handle.unwrap()));
    }
}

#[test]
fn extended_data_over_the_bound_is_omitted_and_reported() {
    // The per-object bound is the string bound of the limits; the import reports and goes on.
    let mut big = xdata_line();
    big.xdata = vec![XData {
        app: "APP_A".to_owned(),
        groups: (0..40)
            .map(|_| Group::text(1000, "x".repeat(100)))
            .collect(),
    }];
    let d = drawing(vec![big, helix()], Some(4));
    let options = arc_draft_dxf::ImportOptions {
        limits: Limits {
            string_bytes: 1024,
            ..Limits::DEFAULT
        },
        ..arc_draft_dxf::ImportOptions::default()
    };
    let plan = arc_draft_dxf::plan(&d, &options);
    let omitted = reason_count(&plan.report, "draft.dxf.xdata_omitted");
    assert_eq!(omitted, 1);
    let line_xdata: Vec<usize> = plan
        .ops
        .iter()
        .filter_map(|op| match op {
            arc_draft_dxf::plan::Op::Entity { spec, .. } => Some(spec.xdata.len()),
            _ => None,
        })
        .collect();
    assert!(line_xdata.iter().all(|n| *n == 0), "the line keeps no data");
    // A small object within the bound is kept in full.
    let mut small = xdata_line();
    small.xdata = xdata_line().xdata;
    let plan = arc_draft_dxf::plan(&drawing(vec![small], Some(4)), &options);
    assert_eq!(reason_count(&plan.report, "draft.dxf.xdata_omitted"), 0);
    let kept: usize = plan
        .ops
        .iter()
        .filter_map(|op| match op {
            arc_draft_dxf::plan::Op::Entity { spec, .. } => Some(spec.xdata.len()),
            _ => None,
        })
        .sum();
    // Two group 1001 markers and the seven groups of the two applications.
    assert_eq!(kept, 9);
}

#[test]
fn thousands_of_classes_stay_bounded_and_are_written() {
    let mut d = drawing(vec![xdata_line()], Some(4));
    d.classes = (0..3000)
        .map(|i| Class {
            dxf_name: format!("CLASS_{i}"),
            class_name: format!("C{i}"),
            application: "APP_A".to_owned(),
            instance_count: Some(-1),
            ..Class::default()
        })
        .collect();
    let (env, space) = imported(&d);
    let exported = export(&env, space, &HandlePolicy::Assign).drawing;
    let back = written(&exported);
    assert_eq!(back.classes.len(), 3000);
    assert!(back.classes.iter().all(|c| c.instance_count == Some(0)));
}

#[test]
fn undo_removes_the_import_with_its_extended_data_and_rows() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let before = env.snapshot().foreign_len();
    let path = env.write(FILE, &bytes(&file()));
    env.import(&path, space).unwrap();
    assert!(env.snapshot().foreign_len() > before);
    assert!(env.document.undo().wait().unwrap().value);
    assert_eq!(env.snapshot().foreign_len(), before);
}

#[test]
fn a_document_without_imports_exports_only_the_structure() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let exported = export(&env, space, &HandlePolicy::Assign).drawing;
    assert!(exported.classes.is_empty());
    let names: Vec<&str> = exported
        .tables
        .appids
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(names, ["ACAD"]);
    assert_eq!(exported.objects.len(), 4);
}

#[test]
fn a_second_import_into_the_space_does_not_duplicate_the_rows() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write(FILE, &bytes(&file()));
    env.import(&path, space).unwrap();
    env.import(&path, space).unwrap();
    let exported = export(&env, space, &HandlePolicy::Assign).drawing;
    let back = written(&exported);
    assert_eq!(back.classes.len(), 2);
    let roots = back
        .objects
        .iter()
        .filter(|o| matches!(o.kind, ObjectKind::Dictionary(_)) && records_root(o))
        .count();
    assert_eq!(roots, 1, "one root dictionary");
    let acad = back
        .tables
        .appids
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .filter(|e| e.name == "ACAD")
        .count();
    assert_eq!(acad, 1);
}

fn records_root(o: &Object) -> bool {
    o.head.owner.is_none_or(|h| h.0 == 0)
}

#[test]
fn records_without_a_handle_get_a_new_one() {
    let mut d = drawing(vec![xdata_line()], Some(4));
    d.tables.appids = Some(Table {
        entries: vec![TableEntry {
            name: "APP_Q".to_owned(),
            ..TableEntry::default()
        }],
        ..Table::default()
    });
    let (env, space) = imported(&d);
    let exported = export(
        &env,
        space,
        &HandlePolicy::KeepSources {
            document: FILE.to_owned(),
            floor: 0,
        },
    )
    .drawing;
    let entry = exported
        .tables
        .appids
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .find(|e| e.name == "APP_Q")
        .expect("the application is written");
    let handle = entry.head.handle.expect("a handle is assigned");
    assert_ne!(handle, Handle(0));
}
