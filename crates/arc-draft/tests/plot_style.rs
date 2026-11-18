//! Plot style tables: the stored form, the validators and the commands, and pen resolution
//! checked against a reference model for both table kinds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test code"
)]

mod support;

use std::collections::BTreeMap;

use arc_base::Color;
use arc_doc::{ColorSource, ColorValue};
use arc_draft::plot_style::{
    Appearance, DEFAULT_LINEWEIGHT_MM, EndStyle, JoinStyle, PlotStyleTable, resolve_pen,
};
use proptest::prelude::*;
use serde_json::{Value as Json, json};
use support::Fixture;

#[derive(Debug, Clone)]
struct Style {
    color: Option<[f32; 3]>,
    lineweight: Option<f64>,
    screening: f64,
    end: u8,
    join: u8,
}

fn end_name(code: u8) -> &'static str {
    ["butt", "square", "round"][usize::from(code)]
}

fn join_name(code: u8) -> &'static str {
    ["miter", "bevel", "round"][usize::from(code)]
}

fn style_json(key: Json, name_field: &str, s: &Style) -> Json {
    let mut v = json!({
        "screening": s.screening,
        "end": end_name(s.end),
        "join": join_name(s.join),
    });
    v[name_field] = key;
    if let Some([r, g, b]) = s.color {
        v["color"] = json!([r, g, b, 1.0]);
    }
    if let Some(w) = s.lineweight {
        v["lineweight"] = json!(w);
    }
    v
}

fn table_of(f: &mut Fixture, named: bool, styles: &[(Json, Style)]) -> PlotStyleTable {
    let records: Vec<Json> = styles
        .iter()
        .map(|(k, s)| style_json(k.clone(), if named { "name" } else { "index" }, s))
        .collect();
    let id = f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "t", "kind": if named { "named" } else { "color_mapped" }, "styles": records}),
    );
    PlotStyleTable::from_values(&f.values(id))
}

#[derive(Debug, Clone)]
struct Object {
    color: [f32; 4],
    index: Option<u8>,
    lineweight: Option<f64>,
    transparency: f64,
    name: Option<&'static str>,
}

const NAMES: [&str; 4] = ["a", "b", "c", "d"];

fn style() -> impl Strategy<Value = Style> {
    (
        proptest::option::of((0_u8..=8, 0_u8..=8, 0_u8..=8)),
        proptest::option::of(0.0..=10.0_f64),
        0.0..=100.0_f64,
        0_u8..3,
        0_u8..3,
    )
        .prop_map(|(c, lineweight, screening, end, join)| Style {
            color: c.map(|(r, g, b)| [f32::from(r) / 8.0, f32::from(g) / 8.0, f32::from(b) / 8.0]),
            lineweight,
            screening,
            end,
            join,
        })
}

fn object() -> impl Strategy<Value = Object> {
    (
        (0_u8..=8, 0_u8..=8, 0_u8..=8, 1_u8..=8),
        proptest::option::of(0_u8..6),
        proptest::option::of(0.0..=10.0_f64),
        0.0..=1.0_f64,
        proptest::option::of(0_usize..4),
    )
        .prop_map(
            |((r, g, b, a), index, lineweight, transparency, name)| Object {
                color: [
                    f32::from(r) / 8.0,
                    f32::from(g) / 8.0,
                    f32::from(b) / 8.0,
                    f32::from(a) / 8.0,
                ],
                index,
                lineweight,
                transparency,
                name: name.map(|i| NAMES[i]),
            },
        )
}

/// The pen the rules give, written independently of the implementation: the style found by key,
/// ink as `1 − s·(1 − c)`.
fn reference(named: bool, styles: &BTreeMap<String, Style>, o: &Object) -> ([f32; 4], f64, u8, u8) {
    let key = if named {
        o.name.map(str::to_owned)
    } else {
        o.index.map(|i| i.to_string())
    };
    let found = key.and_then(|k| styles.get(&k));
    let s = found.map_or(1.0, |s| s.screening / 100.0);
    let base = found
        .and_then(|st| st.color)
        .map_or([o.color[0], o.color[1], o.color[2]], |c| c);
    let ink = |c: f32| 1.0 - s as f32 * (1.0 - c);
    // The alpha is that of the color the pen takes: the style's (opaque here) or the object's.
    let own = if found.is_some_and(|st| st.color.is_some()) {
        1.0
    } else {
        o.color[3]
    };
    let alpha = own * (1.0 - o.transparency as f32);
    (
        [ink(base[0]), ink(base[1]), ink(base[2]), alpha],
        found
            .and_then(|st| st.lineweight)
            .or(o.lineweight)
            .unwrap_or(0.25),
        found.map_or(0, |st| st.end),
        found.map_or(0, |st| st.join),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn pens_match_the_reference_model(
        named in any::<bool>(),
        entries in proptest::collection::btree_map(0_usize..8, style(), 0..6),
        objects in proptest::collection::vec(object(), 1..8),
    ) {
        // Keys: indices 0..8 for a color-mapped table, the four names for a named one.
        let styles: BTreeMap<String, Style> = entries
            .into_iter()
            .filter_map(|(k, s)| {
                let key = if named { NAMES.get(k).map(|n| (*n).to_owned()) } else { Some(k.to_string()) };
                key.map(|key| (key, s))
            })
            .collect();
        let listed: Vec<(Json, Style)> = styles
            .iter()
            .map(|(k, s)| (if named { json!(k) } else { json!(k.parse::<u8>().unwrap()) }, s.clone()))
            .collect();
        let mut f = Fixture::new();
        let table = table_of(&mut f, named, &listed);
        prop_assert_eq!(table.entries.len(), listed.len());
        for o in &objects {
            let value = ColorValue {
                rgba: Color::new(o.color[0], o.color[1], o.color[2], o.color[3]),
                source: o.index.map(ColorSource::Indexed),
            };
            let appearance = Appearance {
                color: &value,
                lineweight: o.lineweight,
                linetype: None,
                transparency: o.transparency,
                style_name: o.name,
            };
            let pen = resolve_pen(Some(&table), &appearance);
            let (rgba, width, end, join) = reference(named, &styles, o);
            let got = [pen.color.r, pen.color.g, pen.color.b, pen.color.a];
            for (g, w) in got.iter().zip(rgba) {
                prop_assert!((g - w).abs() < 1e-5, "{:?} vs {:?}", got, rgba);
            }
            prop_assert_eq!(pen.width, width);
            prop_assert_eq!(pen.cap as u8, end);
            prop_assert_eq!(pen.join as u8, join);
            prop_assert!(!pen.linetype_overridden);
            // Plot styles off: the object's pen.
            let plain = resolve_pen(None, &appearance);
            prop_assert_eq!(plain.width, o.lineweight.unwrap_or(DEFAULT_LINEWEIGHT_MM));
            prop_assert_eq!(plain.color.r, o.color[0]);
            prop_assert_eq!(plain.cap, EndStyle::Butt);
            prop_assert_eq!(plain.join, JoinStyle::Miter);
        }
    }
}

#[test]
fn screening_mixes_linear_color_with_white_and_use_object_color_falls_back() {
    let mut f = Fixture::new();
    let style = Style {
        color: None,
        lineweight: None,
        screening: 25.0,
        end: 2,
        join: 1,
    };
    let table = table_of(&mut f, false, &[(json!(3), style)]);
    let object = ColorValue {
        rgba: Color::new(0.0, 0.4, 1.0, 1.0),
        source: Some(ColorSource::Indexed(3)),
    };
    let pen = resolve_pen(
        Some(&table),
        &Appearance {
            color: &object,
            lineweight: Some(0.7),
            linetype: None,
            transparency: 0.5,
            style_name: None,
        },
    );
    assert!((pen.color.r - 0.75).abs() < 1e-6);
    assert!((pen.color.g - 0.85).abs() < 1e-6);
    assert!((pen.color.b - 1.0).abs() < 1e-6);
    assert!((pen.color.a - 0.5).abs() < 1e-6);
    assert_eq!(pen.width, 0.7);
    assert_eq!((pen.cap, pen.join), (EndStyle::Round, JoinStyle::Bevel));
    // Another index, or no index, finds no style in a color-mapped table.
    for source in [
        Some(ColorSource::Indexed(4)),
        None,
        Some(ColorSource::Named("x".into())),
    ] {
        let other = ColorValue {
            source,
            ..object.clone()
        };
        let pen = resolve_pen(
            Some(&table),
            &Appearance {
                color: &other,
                lineweight: None,
                linetype: None,
                transparency: 0.0,
                style_name: Some("3"),
            },
        );
        assert_eq!(pen.color.g, 0.4);
        assert_eq!(pen.width, DEFAULT_LINEWEIGHT_MM);
    }
}

#[test]
fn a_linetype_override_names_the_linetype_and_is_flagged() {
    let mut f = Fixture::new();
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [{"kind": "dash", "length": 1.0}, {"kind": "gap", "length": 1.0}]}),
    );
    let table = f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "t", "kind": "named", "styles": [{"name": "hidden", "linetype": dashed.to_string()}]}),
    );
    let table = PlotStyleTable::from_values(&f.values(table));
    let color = ColorValue {
        rgba: Color::BLACK,
        source: None,
    };
    let appearance = |name| Appearance {
        color: &color,
        lineweight: None,
        linetype: None,
        transparency: 0.0,
        style_name: name,
    };
    let pen = resolve_pen(Some(&table), &appearance(Some("hidden")));
    assert_eq!(pen.linetype, Some(dashed));
    assert!(pen.linetype_overridden);
    let pen = resolve_pen(Some(&table), &appearance(Some("other")));
    assert_eq!(pen.linetype, None);
    // The linetype is referenced: purging it is refused while the table uses it.
    assert!(
        f.run("core.draft.purge", json!({"objects": [dashed.to_string()]}))
            .is_err()
    );
}

#[test]
fn tables_validate_keys_and_names() {
    let mut f = Fixture::new();
    let styled = |k: Json| json!({"index": k});
    // A color-mapped style needs an index; indices are unique and at most 255.
    let missing = f.run(
        "core.draft.plot_style_table.create",
        json!({"name": "a", "kind": "color_mapped", "styles": [{"name": "x"}]}),
    );
    assert_eq!(
        missing.unwrap_err().code(),
        "draft.plot_style.entry_invalid"
    );
    let blank = f.run(
        "core.draft.plot_style_table.create",
        json!({"name": "b", "kind": "named", "styles": [{"name": ""}]}),
    );
    assert_eq!(blank.unwrap_err().code(), "draft.plot_style.entry_invalid");
    let twice = f.run(
        "core.draft.plot_style_table.create",
        json!({"name": "c", "kind": "color_mapped", "styles": [styled(json!(7)), styled(json!(7))]}),
    );
    assert_eq!(twice.unwrap_err().code(), "draft.plot_style.duplicate");
    let big = f.run(
        "core.draft.plot_style_table.create",
        json!({"name": "d", "kind": "color_mapped", "styles": [styled(json!(7))]}),
    );
    assert!(big.is_ok());
    let range = f.run(
        "core.draft.plot_style_table.create",
        json!({"name": "e", "kind": "color_mapped", "styles": [{"index": 3, "screening": 120.0}]}),
    );
    assert!(range.is_err());
    // Table names are unique.
    let taken = f.run(
        "core.draft.plot_style_table.create",
        json!({"name": "d", "kind": "named"}),
    );
    assert_eq!(taken.unwrap_err().code(), "draft.name.taken");
    // Modify and delete.
    let id = f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "f", "kind": "named", "styles": [{"name": "x"}]}),
    );
    f.set(
        "core.draft.plot_style_table.modify",
        &[id],
        "description",
        json!("office"),
    );
    f.run(
        "core.draft.plot_style_table.delete",
        json!({"objects": [id.to_string()]}),
    )
    .unwrap();
    assert!(f.document.snapshot().resolve(id).is_err());
}

#[test]
fn tables_survive_save_and_reopen() {
    let mut f = Fixture::new();
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [{"kind": "dash", "length": 1.0}]}),
    );
    f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "mono", "kind": "color_mapped", "styles": [
            {"index": 1, "color": [0.0, 0.0, 0.0, 1.0], "lineweight": 0.35, "screening": 60.0, "end": "round", "join": "round"},
            {"index": 2, "linetype": dashed.to_string()}]}),
    );
    f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "by name", "kind": "named", "styles": [{"name": "thin", "lineweight": 0.1}]}),
    );
    support::round_trip(&f);
}
