//! Regression tests for the review of the preservation unit: references are written through one
//! source map under both policies, unknown references are nulled, duplicate source numbers are
//! ambiguous, the layouts number names the export's layouts dictionary, the owner of an entity is
//! found past application groups, a second import replaces the class, application and dictionary
//! rows, the root merge honours the record budget, names are validated, dictionary owners stay
//! reachable, class counts ignore case, and malformed extended data is omitted.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::{Limits, ObjectId};
use arc_draft_dxf::{HandlePolicy, ImportOptions, export_drawing_with, plan};
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

fn record(kind: &str, groups: Vec<Group>) -> Entity {
    Entity::new(EntityKind::Preserved(PreservedRecord {
        kind: kind.to_owned(),
        groups,
    }))
}

fn object(groups: Vec<Group>) -> Object {
    Object::new(ObjectKind::Preserved(PreservedRecord {
        kind: "ACDBCUSTOM".to_owned(),
        groups,
    }))
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

fn line_on_0x20() -> Entity {
    let mut e = with_handle(on_layer(line([0.0; 3], [10.0, 0.0, 0.0]), "0"), 0x20);
    e.head.owner = None;
    e
}

fn exported_line(d: &Drawing) -> &Entity {
    d.entities
        .iter()
        .find(|e| matches!(e.kind, EntityKind::Line(_)))
        .expect("the line is written")
}

fn handle_of(e: &Entity) -> Option<Handle> {
    e.head.handle
}

fn record_groups(d: &Drawing, kind: &str) -> Vec<Vec<Group>> {
    d.entities
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Preserved(r) if r.kind == kind => Some(r.groups.clone()),
            _ => None,
        })
        .collect()
}

/// The line on handle 0x20 carries a 1005 that names itself, and a preserved entity has a reactor
/// inside an application group that names the line, after an owner outside the group.
fn reference_file() -> Drawing {
    let mut line = line_on_0x20();
    line.xdata = vec![XData {
        app: "APP_A".to_owned(),
        groups: vec![Group::text(1005, "20")],
    }];
    let helix = record(
        "HELIX",
        vec![
            Group::text(5, "30"),
            Group::text(330, "1F"),
            Group::text(100, "AcDbEntity"),
            Group::text(102, "{ACAD_REACTORS"),
            Group::text(330, "20"),
            Group::text(102, "}"),
        ],
    );
    drawing(vec![line, helix], Some(4))
}

#[test]
fn references_follow_the_handles_the_export_writes_under_both_policies() {
    let (env, space) = imported(&reference_file());
    for policy in policies() {
        let result = export(&env, space, &policy);
        let back = written(&result.drawing);
        let line_handle = handle_of(exported_line(&back)).expect("the line has a handle");
        let helix = &record_groups(&back, "HELIX")[0];
        // The reactor inside the application group names the line under its new handle.
        let reactor = helix
            .iter()
            .rfind(|g| g.code == 330)
            .and_then(Group::as_handle);
        assert_eq!(reactor, Some(line_handle), "{policy:?}");
        // The line's own extended data names itself.
        assert_eq!(
            exported_line(&back).xdata[0].groups[0].as_handle(),
            Some(line_handle),
            "{policy:?}"
        );
    }
}

#[test]
fn the_owner_of_an_entity_is_found_past_application_groups() {
    // The reactor inside the application group comes before the owner: the first group 330 is
    // the reactor, and the owner is the next one.
    let helix = record(
        "HELIX",
        vec![
            Group::text(5, "30"),
            Group::text(102, "{ACAD_REACTORS"),
            Group::text(330, "20"),
            Group::text(102, "}"),
            Group::text(330, "1F"),
            Group::text(100, "AcDbEntity"),
        ],
    );
    let mut d = reference_file();
    d.entities = vec![line_on_0x20(), helix];
    let (env, space) = imported(&d);
    for policy in policies() {
        let result = export(&env, space, &policy);
        let back = written(&result.drawing);
        let line_handle = handle_of(exported_line(&back)).expect("the line has a handle");
        let owner = exported_line(&back).head.owner;
        let helix = &record_groups(&back, "HELIX")[0];
        // The owner after the application group is the container, as the line's owner is; the
        // reactor inside the group keeps its reference.
        let outside = helix
            .iter()
            .take_while(|g| g.code != 100)
            .filter(|g| g.code == 330)
            .last()
            .and_then(Group::as_handle);
        assert_eq!(outside, owner, "{policy:?}");
        let inside = helix
            .iter()
            .find(|g| g.code == 330)
            .and_then(Group::as_handle);
        assert_eq!(inside, Some(line_handle), "{policy:?}");
    }
}

#[test]
fn unknown_references_are_written_as_the_null_handle_and_reported() {
    let d = drawing(
        vec![record(
            "HELIX",
            vec![
                Group::text(5, "30"),
                Group::text(330, "1F"),
                Group::text(100, "AcDbEntity"),
                Group::text(330, "99"),
            ],
        )],
        Some(4),
    );
    let (env, space) = imported(&d);
    for policy in policies() {
        let result = export(&env, space, &policy);
        let helix = &record_groups(&written(&result.drawing.clone()), "HELIX")[0];
        let reactor = helix
            .iter()
            .rfind(|g| g.code == 330)
            .and_then(Group::as_handle);
        assert_eq!(reactor, Some(Handle(0)), "{policy:?}");
        assert_eq!(
            reason_count(&result.report, "draft.dxf.reference_nulled"),
            1
        );
    }
}

#[test]
fn a_zero_handle_gets_a_new_handle_and_is_never_looked_up() {
    let d = drawing(vec![], Some(4));
    let mut d = d;
    d.objects.push(object(vec![
        Group::text(5, "0"),
        Group::text(330, "C"),
        Group::text(100, "AcDbCustom"),
    ]));
    let (env, space) = imported(&d);
    for policy in policies() {
        let back = written(&export(&env, space, &policy).drawing);
        let custom = back
            .objects
            .iter()
            .find_map(|o| match &o.kind {
                ObjectKind::Preserved(r) if r.kind == "ACDBCUSTOM" => r.handle(),
                _ => None,
            })
            .expect("the object is written");
        assert_ne!(custom, Handle(0), "{policy:?}");
    }
}

#[test]
fn a_source_number_held_twice_is_ambiguous_and_its_references_are_nulled() {
    // The first import holds a record numbered 0x30; the second import holds another one with the
    // same number, and a record that names 0x30. The number names two records: it is nulled.
    let (mut env, space) = imported(&drawing(
        vec![record(
            "HELIX",
            vec![
                Group::text(5, "30"),
                Group::text(330, "1F"),
                Group::text(100, "A"),
            ],
        )],
        Some(4),
    ));
    let second = drawing(
        vec![
            record(
                "HELIX",
                vec![
                    Group::text(5, "30"),
                    Group::text(330, "1F"),
                    Group::text(100, "B"),
                ],
            ),
            record(
                "HELIX",
                vec![
                    Group::text(5, "31"),
                    Group::text(330, "1F"),
                    Group::text(100, "C"),
                    Group::text(330, "30"),
                ],
            ),
        ],
        Some(4),
    );
    let path = env.write(FILE, &bytes(&second));
    env.import(&path, space).unwrap();
    for policy in policies() {
        let back = written(&export(&env, space, &policy).drawing);
        let third = record_groups(&back, "HELIX")
            .into_iter()
            .find(|g| {
                g.iter()
                    .any(|x| x.code == 5 && x.as_handle() == Some(Handle(0x31)))
            })
            .expect("the third record is written");
        assert_eq!(
            third
                .iter()
                .rfind(|g| g.code == 330)
                .and_then(Group::as_handle),
            Some(Handle(0)),
            "{policy:?}"
        );
    }
}

#[test]
fn the_layouts_number_names_the_export_layouts_dictionary_and_an_unrelated_dictionary_survives() {
    let mut d = drawing(
        vec![record(
            "HELIX",
            vec![
                Group::text(5, "30"),
                Group::text(330, "C"),
                Group::text(100, "A"),
                Group::text(330, "60"),
            ],
        )],
        Some(4),
    );
    // The file's root names its layouts dictionary 0x60; a second dictionary holds the same number.
    d.objects
        .push(dictionary(0xC, None, &[("ACAD_LAYOUT", 0x60)]));
    d.objects
        .push(dictionary(0x60, Some(0xC), &[("Model", 0x61)]));
    d.objects
        .push(dictionary(0x60, Some(0xC), &[("OTHER", 0x40)]));
    d.objects.push(object(vec![
        Group::text(5, "40"),
        Group::text(330, "C"),
        Group::text(100, "B"),
    ]));
    let (env, space) = imported(&d);
    for policy in policies() {
        let back = written(&export(&env, space, &policy).drawing);
        let root = back
            .objects
            .iter()
            .find(|o| {
                matches!(&o.kind, ObjectKind::Dictionary(_))
                    && o.head.owner.is_none_or(|h| h.0 == 0)
            })
            .expect("the root is written");
        let ObjectKind::Dictionary(rd) = &root.kind else {
            panic!("a dictionary")
        };
        let layouts = rd
            .entries
            .iter()
            .find(|e| e.name == "ACAD_LAYOUT")
            .expect("the export's layouts entry")
            .handle;
        let reactor = record_groups(&back, "HELIX")[0]
            .iter()
            .rfind(|g| g.code == 330)
            .and_then(Group::as_handle);
        assert_eq!(reactor, Some(layouts), "{policy:?}");
        assert!(
            back.objects
                .iter()
                .any(|o| matches!(&o.kind, ObjectKind::Dictionary(d)
                if d.entries.iter().any(|e| e.name == "OTHER"))),
            "the unrelated dictionary with the same number is kept under {policy:?}"
        );
    }
}

#[test]
fn a_second_import_replaces_the_classes_and_the_dictionaries_of_the_space() {
    let mut first = drawing(vec![], Some(4));
    first.classes = vec![Class {
        dxf_name: "HELIX".to_owned(),
        application: "APP_X".to_owned(),
        ..Class::default()
    }];
    first
        .objects
        .push(dictionary(0xC, None, &[("MYAPP_OBJ", 0x40)]));
    first.objects.push(object(vec![
        Group::text(5, "40"),
        Group::text(330, "C"),
        Group::text(100, "A"),
    ]));
    let (mut env, space) = imported(&first);
    let mut second = drawing(vec![], Some(4));
    second.classes = vec![Class {
        dxf_name: "OTHER".to_owned(),
        application: "APP_Y".to_owned(),
        ..Class::default()
    }];
    let path = env.write(FILE, &bytes(&second));
    env.import(&path, space).unwrap();
    let back = written(&export(&env, space, &HandlePolicy::Assign).drawing);
    let names: Vec<&str> = back.classes.iter().map(|c| c.dxf_name.as_str()).collect();
    assert_eq!(names, ["OTHER"]);
    let root = back
        .objects
        .iter()
        .find(|o| {
            matches!(o.kind, ObjectKind::Dictionary(_)) && o.head.owner.is_none_or(|h| h.0 == 0)
        })
        .expect("the root is written");
    let ObjectKind::Dictionary(d) = &root.kind else {
        panic!("a dictionary")
    };
    assert!(d.entries.iter().all(|e| e.name != "MYAPP_OBJ"));
}

#[test]
fn an_adopted_layer_takes_the_extended_data_of_the_last_import() {
    let layer_with = |value: &str| {
        let mut d = drawing(vec![], Some(4));
        let layers = d.tables.layers.as_mut().unwrap();
        layers.entries[1].xdata = vec![XData {
            app: "APP_A".to_owned(),
            groups: vec![Group::text(1000, value)],
        }];
        d
    };
    let (mut env, space) = imported(&layer_with("one"));
    let path = env.write(FILE, &bytes(&layer_with("two")));
    env.import(&path, space).unwrap();
    let back = written(&export(&env, space, &HandlePolicy::Assign).drawing);
    let walls = back
        .tables
        .layers
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .find(|e| e.name == "WALLS")
        .expect("the layer is written");
    assert_eq!(walls.xdata.len(), 1);
    assert_eq!(walls.xdata[0].groups, vec![Group::text(1000, "two")]);
}

#[test]
fn the_root_merge_honours_the_record_budget() {
    let mut d = drawing(vec![], Some(4));
    let mut entries = Vec::new();
    for i in 0..10_u64 {
        let handle = 0x40 + i;
        d.objects.push(object(vec![
            Group::text(5, format!("{handle:X}")),
            Group::text(330, "C"),
            Group::text(100, "A"),
        ]));
        entries.push((format!("E{i}"), handle));
    }
    let named: Vec<(&str, u64)> = entries.iter().map(|(n, h)| (n.as_str(), *h)).collect();
    d.objects.push(dictionary(0xC, None, &named));
    let (env, space) = imported(&d);
    let limits = Limits {
        record_count: 3,
        ..Limits::DEFAULT
    };
    let result = arc_draft_dxf::export_drawing_limited(
        &env.snapshot(),
        space,
        &HandlePolicy::Assign,
        &limits,
    )
    .unwrap();
    let ObjectKind::Dictionary(root) = &result.drawing.objects[0].kind else {
        panic!("a dictionary")
    };
    assert!(root.entries.len() <= 1 + 3, "{}", root.entries.len());
    assert!(reason_count(&result.report, "draft.dxf.export_unmapped") > 0);
}

#[test]
fn application_names_must_be_valid_table_names() {
    let mut d = drawing(vec![], Some(4));
    d.tables.appids = Some(Table {
        entries: vec![
            TableEntry {
                name: "APP_OK".to_owned(),
                ..TableEntry::default()
            },
            TableEntry {
                name: "A".repeat(300),
                ..TableEntry::default()
            },
        ],
        ..Table::default()
    });
    let (env, space) = imported(&d);
    let result = export(&env, space, &HandlePolicy::Assign);
    let names: Vec<String> = result
        .drawing
        .tables
        .appids
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(names.contains(&"APP_OK".to_owned()));
    assert!(names.iter().all(|n| n.len() <= 255));
    assert!(reason_count(&result.report, "draft.dxf.export_unmapped") > 0);
}

#[test]
fn dictionary_owners_stay_reachable_or_are_dropped() {
    let mut d = drawing(vec![], Some(4));
    d.objects.push(dictionary(0xC, None, &[]));
    // An ownerless dictionary that is not the root: written under the root, listed in it.
    d.objects.push(dictionary(0x70, None, &[("INNER", 0x40)]));
    // A dictionary that owns itself: the root owns it, and lists it.
    d.objects.push(dictionary(0x72, Some(0x72), &[]));
    // An extension dictionary of an entity the file does not write: dropped.
    d.objects
        .push(dictionary(0x71, Some(0x1F), &[("EXT", 0x40)]));
    // A custom object whose owner is missing: written under the root, listed in it.
    d.objects.push(object(vec![
        Group::text(5, "41"),
        Group::text(330, "1F"),
        Group::text(100, "A"),
    ]));
    d.objects.push(object(vec![
        Group::text(5, "40"),
        Group::text(330, "C"),
        Group::text(100, "A"),
    ]));
    let (env, space) = imported(&d);
    for policy in policies() {
        let back = written(&export(&env, space, &policy).drawing);
        let root = back
            .objects
            .iter()
            .find(|o| {
                matches!(&o.kind, ObjectKind::Dictionary(_))
                    && o.head.owner.is_none_or(|h| h.0 == 0)
            })
            .expect("the root is written");
        let ObjectKind::Dictionary(rd) = &root.kind else {
            panic!("a dictionary")
        };
        let listed: Vec<&str> = rd.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            listed.iter().filter(|n| n.starts_with("ARC_")).count(),
            3,
            "{policy:?}: {listed:?}"
        );
        assert!(
            back.objects
                .iter()
                .all(|o| !matches!(&o.kind, ObjectKind::Dictionary(d)
                if d.entries.iter().any(|e| e.name == "EXT"))),
            "the extension dictionary is dropped under {policy:?}"
        );
    }
}

#[test]
fn class_counts_ignore_case() {
    let mut d = reference_file();
    d.classes = vec![Class {
        dxf_name: "helix".to_owned(),
        instance_count: Some(-5),
        ..Class::default()
    }];
    let (env, space) = imported(&d);
    let back = written(&export(&env, space, &HandlePolicy::Assign).drawing);
    assert_eq!(back.classes[0].instance_count, Some(1));
}

fn xdata_line(groups: Vec<Group>) -> Drawing {
    let mut e = line_on_0x20();
    e.xdata = vec![XData {
        app: "APP_A".to_owned(),
        groups,
    }];
    drawing(vec![e], Some(4))
}

#[test]
fn unbalanced_control_braces_omit_the_extended_data() {
    let d = xdata_line(vec![Group::text(1002, "{"), Group::text(1000, "x")]);
    let p = plan(&d, &ImportOptions::default());
    assert_eq!(reason_count(&p.report, "draft.dxf.xdata_omitted"), 1);
    let kept: usize = p
        .ops
        .iter()
        .map(|op| match op {
            arc_draft_dxf::plan::Op::Entity { spec, .. } => spec.xdata.len(),
            _ => 0,
        })
        .sum();
    assert_eq!(kept, 0);
}

#[test]
fn binary_chunks_over_the_text_bound_omit_the_extended_data() {
    let d = xdata_line(vec![Group {
        code: 1004,
        value: Value::Bytes(vec![7; 200]),
    }]);
    let p = plan(&d, &ImportOptions::default());
    assert_eq!(reason_count(&p.report, "draft.dxf.xdata_omitted"), 1);
}
