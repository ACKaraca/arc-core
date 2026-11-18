//! Headless layout of every widget of the crate at several UI scales in every theme
//! (CORE-UI-022 and CORE-UI-027 in part).
//!
//! Each widget is drawn into a fixed container rectangle (1400 x 2600 points) through a
//! headless egui context with the AccessKit tree enabled, after `Appearance::apply` for the
//! theme, density and scale under test, and measured on the third pass from the AccessKit
//! nodes' bounds. The test measures exactly this, per widget, theme, density (compact,
//! standard, comfortable) and scale (75, 100, 200 and 300 percent):
//!
//! 1. **Not clipped by the parent.** Every node's bounds lie inside the container rectangle,
//!    and inside the bounds of its parent node when that has bounds (for example the text run
//!    inside a text field). egui's plain containers have no bounds, so the container rectangle
//!    stands in for them.
//! 2. **No overlap among siblings in a row.** No two leaf nodes (nodes without children) of a
//!    widget intersect by more than half a point in both directions. Nodes with identical rectangles are
//!    alternative handles of one region (the view cube's 26 targets) and are not compared with
//!    each other. The viewport canvas is excluded from this check; instead the view cube must
//!    lie inside the canvas.
//! 3. **Row height.** Every button, check box and text field is at least the token row height
//!    (`Metrics::row_height` after density and scale) tall, and the rows of the palette, tree
//!    and diagnostics lists are all exactly the same height.
//!
//! It does not measure text truncation inside a node, pixel-level painting, or popups that are
//! closed (open menus), and it does not compare rendered pixels.

#![cfg(feature = "viewport")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use std::collections::BTreeMap;

use arc_base::error::Value as ArgValue;
use arc_base::id::Identifier;
use arc_base::math::{Point3, Vector3};
use arc_base::{
    Dimension, DisplayUnits, Error, ErrorCode, ObjectId, Quantity, Result as BaseResult, Tolerance,
};
use arc_command::spec::CommandSpec;
use arc_command::{Completion, Permission, Registry};
use arc_doc::diagnostic::{Diagnostic, Origin};
use arc_doc::diagnostics::DiagnosticsSnapshot;
use arc_doc::schema::{FieldDecl, Severity, Value, ValueType};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{PickHit, PickRegion};
use arc_ui::egui::{self, accesskit};
use arc_ui::notify::{Action, Severity as Level};
use arc_ui::view_cube::{ViewCube, ViewCubeInput};
use arc_ui::{
    Appearance, CommandLineView, Density, DiagnosticsPanel, Dock, DockArea, DockLayout,
    GridContext, Intent, LineCommands, LineContext, Localize, Lookup, Menu, Notifications, Palette,
    Progress, PropertyGrid, PropertyModel, PropertyObject, Recent, ThemeId, Toolbar, ToolbarItem,
    TreeCommands, TreeItem, TreeSource, TreeView, UiScale, ViewLink, ViewPicker, ViewportCommands,
    ViewportInput, ViewportWidget,
};

const WIDTH: f32 = 1400.0;
/// Tall enough that no list scrolls: egui emits one row beyond a scroll area's edge.
const HEIGHT: f32 = 2600.0;
const SCALES: [u16; 4] = [75, 100, 200, 300];
const DENSITIES: [Density; 3] = [Density::Compact, Density::Standard, Density::Comfortable];
/// Rectangles closer than this, in points, count as touching, not overlapping or clipped.
const EPS: f64 = 0.5;

/// Answers every identifier with a readable text so the widgets lay out real strings.
struct Ids;
impl Localize for Ids {
    fn message(&self, id: &str, _: &[(&str, ArgValue)]) -> Option<String> {
        Some(id.to_owned())
    }
}

type Draw = Box<dyn FnMut(&mut egui::Ui)>;

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

fn registry() -> Registry {
    let mut r = Registry::new();
    for (id, label) in [
        ("demo.file.save", "demo-save"),
        ("demo.edit.undo", "demo-undo"),
        ("demo.view.fit", "demo-fit"),
    ] {
        r.register(
            CommandSpec::new(id, label, "demo-save", |_, _: NoParams| Ok(()))
                .with_permissions([Permission::Read]),
        )
        .unwrap();
    }
    r
}

fn menu_bar() -> Draw {
    let registry = registry();
    let lookup = Lookup {
        registry: &registry,
        localize: &Ids,
        state: &arc_ui::AllEnabled,
        keymap: None,
    };
    let menu: Menu = serde_json::from_str(
        r#"{"items":[{"kind":"submenu","label":"demo-file","items":[
            {"kind":"command","command":"demo.file.save"}]},
            {"kind":"submenu","label":"demo-edit","items":[
            {"kind":"command","command":"demo.edit.undo"}]}]}"#,
    )
    .unwrap();
    let (entries, _) = lookup.menu(&menu);
    Box::new(move |ui| {
        let _ = arc_ui::menu::show_menu_bar(ui, &entries);
    })
}

fn toolbar() -> Draw {
    let registry = registry();
    let lookup = Lookup {
        registry: &registry,
        localize: &Ids,
        state: &arc_ui::AllEnabled,
        keymap: None,
    };
    let toolbar = Toolbar::new(vec![
        ToolbarItem::Command {
            command: "demo.file.save".into(),
            icon: Some("check".into()),
        },
        ToolbarItem::Separator,
        ToolbarItem::Command {
            command: "demo.edit.undo".into(),
            icon: None,
        },
    ])
    .unwrap();
    let (entries, _) = lookup.toolbar(&toolbar);
    let atlas = std::cell::RefCell::new(None);
    Box::new(move |ui| {
        let mut slot = atlas.borrow_mut();
        let icons = slot.get_or_insert_with(|| {
            arc_ui::IconAtlas::build(arc_ui::icons::STARTER, 16)
                .unwrap()
                .upload(ui.ctx())
        });
        let _ = arc_ui::menu::show_toolbar(ui, &entries, Some(icons));
    })
}

fn palette() -> Draw {
    let registry = registry();
    let lookup = Lookup {
        registry: &registry,
        localize: &Ids,
        state: &arc_ui::AllEnabled,
        keymap: None,
    };
    let views = lookup.all();
    let mut palette = Palette::default();
    let recent = Recent::default();
    Box::new(move |ui| {
        let _ = palette.show(ui, &views, &recent, &Ids);
    })
}

fn notifications() -> Draw {
    let mut notes = Notifications::default();
    for severity in [Level::Info, Level::Warning, Level::Error] {
        notes.post(
            severity,
            "demo-save",
            Vec::new(),
            vec![Action {
                label: "demo-undo".into(),
                intent: Intent::new("demo.edit.undo"),
            }],
            0,
        );
    }
    let colors = ThemeId::Light.theme().colors;
    let progress = Progress::start(0, true);
    Box::new(move |ui| {
        let _ = notes.show(ui, &colors, &Ids);
        let _ = progress.show(ui, 10_000, &Ids);
    })
}

fn command_line() -> Draw {
    let commands = LineCommands {
        undo: "core.edit.undo".into(),
        redo: "core.edit.redo".into(),
        clear_selection: "core.select.clear".into(),
    };
    let line = arc_command::CommandLine::new();
    let mut view = CommandLineView::default();
    Box::new(move |ui| {
        let context = LineContext {
            localize: &Ids,
            has_selection: false,
            commands: &commands,
        };
        let _ = view.show(ui, &line, &Completion::default(), &context);
    })
}

fn fields() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("width", ValueType::Quantity(Dimension::LENGTH)).in_aspect("geometry"),
        FieldDecl::new("visible", ValueType::Bool),
    ]
}

fn property_grid() -> Draw {
    let decls = fields();
    let values = [
        Some(Value::Quantity(Quantity::new(1.0, Dimension::LENGTH))),
        Some(Value::Bool(true)),
    ];
    let objects = [PropertyObject {
        id: ObjectId::from_u128(1),
        fields: &decls,
        values: &values,
        read_only: &[],
        overridden: &[],
    }];
    let model = PropertyModel::build(&objects).unwrap();
    let units = DisplayUnits::new();
    let mut grid = PropertyGrid::default();
    Box::new(move |ui| {
        let label = |name: &str| name.to_owned();
        let context = GridContext {
            localize: &Ids,
            label: &label,
            units: &units,
            decimal_separator: '.',
            edit_command: "core.object.set_field",
        };
        let _ = grid.show(ui, &model, &context);
    })
}

struct Rows;
impl TreeSource for Rows {
    fn child_count(&self, parent: Option<ObjectId>, _: &str) -> usize {
        if parent.is_none() { 4 } else { 0 }
    }
    fn child(&self, parent: Option<ObjectId>, index: usize, _: &str) -> Option<TreeItem> {
        (parent.is_none() && index < 4).then(|| TreeItem {
            id: ObjectId::from_u128(index as u128 + 1),
            label: format!("Wall {}", index + 1),
            has_children: index == 0,
            selected: index == 1,
            visible: Some(true),
            locked: Some(index == 2),
        })
    }
    fn index_of(&self, _: Option<ObjectId>, id: ObjectId, _: &str) -> Option<usize> {
        usize::try_from(id.as_u128()).ok()?.checked_sub(1)
    }
}

fn tree_commands() -> TreeCommands {
    TreeCommands {
        select: "core.select.set".into(),
        rename: "core.object.rename".into(),
        set_visible: "core.object.set_visible".into(),
        set_locked: "core.object.set_locked".into(),
        move_to: "core.object.move_to".into(),
    }
}

fn tree() -> Draw {
    let mut tree = TreeView::default();
    let commands = tree_commands();
    Box::new(move |ui| {
        let _ = tree.show(ui, &Rows, &commands, &Ids);
    })
}

fn snapshot() -> DiagnosticsSnapshot {
    let code = ErrorCode {
        code: "demo.wall.gap",
        category: arc_base::Category::InvalidInput,
        message_id: "demo-wall-has-a-gap",
    };
    DiagnosticsSnapshot {
        version: 1,
        diagnostics: [Severity::Warning, Severity::Error, Severity::Warning]
            .map(|s| {
                Diagnostic::new(
                    s,
                    Error::new(code),
                    vec![ObjectId::from_u128(9)],
                    Origin::Validator,
                )
            })
            .to_vec()
            .into(),
        dropped: 0,
    }
}

fn diagnostics() -> Draw {
    let mut panel = DiagnosticsPanel::default();
    let snapshot = snapshot();
    Box::new(move |ui| {
        let _ = panel.show(ui, &snapshot, &Ids, "core.view.show_objects");
    })
}

fn dock() -> Draw {
    let root: Dock = serde_json::from_value::<DockLayout>(serde_json::json!({
        "root": 6,
        "nodes": [
            {"kind": "tabs", "tabs": ["line"]},
            {"kind": "tabs", "tabs": ["props"]},
            {"kind": "tabs", "tabs": ["tree", "other"]},
            {"kind": "tabs", "tabs": ["diag"]},
            {"kind": "split", "axis": "vertical", "fraction": 0.5, "first": 2, "second": 3},
            {"kind": "split", "axis": "vertical", "fraction": 0.3, "first": 1, "second": 4},
            {"kind": "split", "axis": "vertical", "fraction": 0.15, "first": 0, "second": 5}
        ]
    }))
    .unwrap()
    .root()
    .clone();
    let mut layout = DockLayout::new(root, vec![]).unwrap();
    let mut line = command_line();
    let mut grid = property_grid();
    let mut tree = tree();
    let mut diag = diagnostics();
    Box::new(move |ui| {
        let title = |panel: &str| panel.to_owned();
        let mut content = |ui: &mut egui::Ui, panel: &str| match panel {
            "line" => line(ui),
            "props" => grid(ui),
            "tree" => tree(ui),
            "diag" => diag(ui),
            _ => {
                let _ = ui.button("other");
            }
        };
        DockArea::default().show(ui, &mut layout, &Ids, &title, &mut content);
    })
}

struct NoPicker;
impl ViewPicker for NoPicker {
    fn pick(&mut self, _: &ViewFrame, _: u32, _: u32, _: &PickRegion) -> BaseResult<Vec<PickHit>> {
        Ok(Vec::new())
    }
    fn bounds(&mut self) -> Option<(Point3, f64)> {
        None
    }
}

/// The viewport with a texture id that no renderer backs, and the view cube over its corner.
fn viewport(scale: UiScale) -> Draw {
    let frame = ViewFrame::new(
        Point3::new(-10.0, -10.0, 10.0),
        Point3::origin(),
        Vector3::z(),
        Projection::Orthographic { height: 20.0 },
        0.1,
        100.0,
    )
    .unwrap();
    let commands = ViewportCommands::default();
    let tolerance = Tolerance::DEFAULT;
    let colors = ThemeId::Light.theme().colors;
    let mut widget = ViewportWidget::new();
    let mut cube = ViewCube;
    let _ = ViewLink::new();
    Box::new(move |ui| {
        let mut picker = NoPicker;
        let out = widget.show(
            ui,
            ViewportInput {
                texture: Some(egui::TextureId::Managed(7)),
                frame: &frame,
                prompt: None,
                selected: 0,
                picker: &mut picker,
                commands: &commands,
                localize: &Ids,
                colors: &colors,
                scale,
                tolerance: &tolerance,
                name: "Top",
                time: 0.0,
            },
        );
        let size = ui.spacing().interact_size.y * 4.0;
        let _ = cube.show_over(
            ui,
            out.response.rect,
            &ViewCubeInput {
                frame: &frame,
                localize: &Ids,
                colors: &colors,
                size,
            },
        );
    })
}

struct Node {
    role: accesskit::Role,
    label: String,
    rect: egui::Rect,
    has_children: bool,
    parent: Option<usize>,
}

fn measure(appearance: Appearance, mut draw: Draw) -> Vec<Node> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    appearance.apply(&ctx);
    let container = egui::Rect::from_min_size(egui::pos2(40.0, 40.0), egui::vec2(WIDTH, HEIGHT));
    let mut nodes = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(WIDTH + 80.0, HEIGHT + 80.0),
            )),
            ..Default::default()
        };
        let mut full = ctx.run_ui(input, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(container), |ui| draw(ui));
        });
        full.textures_delta.clear();
        let list = full
            .platform_output
            .accesskit_update
            .map(|u| u.nodes)
            .unwrap_or_default();
        let index: BTreeMap<_, _> = list
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (*id, i))
            .collect();
        let mut parents = vec![None; list.len()];
        for (i, (_, node)) in list.iter().enumerate() {
            for child in node.children() {
                if let Some(c) = index.get(child) {
                    parents[*c] = Some(i);
                }
            }
        }
        nodes = list
            .iter()
            .enumerate()
            .map(|(i, (_, n))| Node {
                role: n.role(),
                label: n.label().or(n.value()).unwrap_or_default().to_owned(),
                rect: n.bounds().map_or(egui::Rect::NOTHING, |b| {
                    egui::Rect::from_min_max(
                        egui::pos2(b.x0 as f32, b.y0 as f32),
                        egui::pos2(b.x1 as f32, b.y1 as f32),
                    )
                }),
                has_children: !n.children().is_empty(),
                parent: parents[i],
            })
            .collect();
    }
    nodes
}

fn inside(inner: egui::Rect, outer: egui::Rect) -> bool {
    let e = EPS as f32;
    inner.min.x >= outer.min.x - e
        && inner.min.y >= outer.min.y - e
        && inner.max.x <= outer.max.x + e
        && inner.max.y <= outer.max.y + e
}

fn overlap(a: egui::Rect, b: egui::Rect) -> bool {
    let w = a.max.x.min(b.max.x) - a.min.x.max(b.min.x);
    let h = a.max.y.min(b.max.y) - a.min.y.max(b.min.y);
    f64::from(w) > EPS && f64::from(h) > EPS
}

/// Returns the problems found for one widget under one appearance.
fn check(
    name: &str,
    appearance: Appearance,
    draw: Draw,
    uniform: Option<accesskit::Role>,
) -> Vec<String> {
    let row = appearance.tokens().metrics.row_height;
    let tag = format!(
        "{name} {:?} {:?} {} %",
        appearance.theme,
        appearance.density,
        appearance.scale.percent()
    );
    let nodes = measure(appearance, draw);
    let container = egui::Rect::from_min_size(egui::pos2(40.0, 40.0), egui::vec2(WIDTH, HEIGHT));
    let mut problems = Vec::new();
    let boxed: Vec<usize> = (0..nodes.len())
        .filter(|i| nodes[*i].rect.is_finite() && nodes[*i].rect.width() > 0.0)
        .collect();
    assert!(!boxed.is_empty(), "{tag}: nothing was drawn");
    for &i in &boxed {
        let n = &nodes[i];
        // 1. Clipping by the container and by a parent that has bounds.
        if !inside(n.rect, container) {
            problems.push(format!(
                "{tag}: {:?} {:?} {:?} leaves the container",
                n.role, n.label, n.rect
            ));
        }
        if let Some(p) = n
            .parent
            .map(|p| &nodes[p])
            .filter(|p| p.rect.is_finite() && p.rect.width() > 0.0)
            && !inside(n.rect, p.rect)
        {
            problems.push(format!(
                "{tag}: {:?} {:?} is clipped by its parent {:?}",
                n.role, n.label, p.role
            ));
        }
    }
    // 2. Overlap among leaves; the viewport canvas is checked against the cube instead.
    let leaves: Vec<usize> = boxed
        .iter()
        .copied()
        .filter(|i| !nodes[*i].has_children && nodes[*i].role != accesskit::Role::Canvas)
        .collect();
    for (k, &a) in leaves.iter().enumerate() {
        for &b in &leaves[k + 1..] {
            let (ra, rb) = (nodes[a].rect, nodes[b].rect);
            if ra != rb && overlap(ra, rb) {
                problems.push(format!(
                    "{tag}: {:?} {:?} {ra:?} overlaps {:?} {:?} {rb:?}",
                    nodes[a].role, nodes[a].label, nodes[b].role, nodes[b].label
                ));
            }
        }
    }
    if let Some(canvas) = boxed
        .iter()
        .find(|i| nodes[**i].role == accesskit::Role::Canvas)
    {
        for &i in &boxed {
            if nodes[i].label.contains("ui-view-cube")
                && !inside(nodes[i].rect, nodes[*canvas].rect)
            {
                problems.push(format!("{tag}: the view cube leaves the viewport"));
            }
        }
    }
    // 3. Row height.
    let mut uniform_heights = Vec::new();
    for &i in &boxed {
        let n = &nodes[i];
        let interactive = matches!(
            n.role,
            accesskit::Role::Button | accesskit::Role::CheckBox | accesskit::Role::TextInput
        ) && !n.label.contains("ui-view-cube");
        if interactive && f64::from(n.rect.height()) < f64::from(row) - EPS {
            problems.push(format!(
                "{tag}: {:?} {:?} is {} tall, below the row height {row}",
                n.role,
                n.label,
                n.rect.height()
            ));
        }
        if Some(n.role) == uniform {
            uniform_heights.push(n.rect.height());
        }
    }
    if let (Some(lo), Some(hi)) = (
        uniform_heights.iter().copied().reduce(f32::min),
        uniform_heights.iter().copied().reduce(f32::max),
    ) && f64::from(hi - lo) > EPS
    {
        problems.push(format!("{tag}: list rows differ in height ({lo} to {hi})"));
    }
    problems
}

#[test]
fn every_widget_lays_out_without_clipping_overlap_or_short_rows() {
    type Make = fn(UiScale) -> Draw;
    let widgets: [(&str, Make, Option<accesskit::Role>); 10] = [
        ("menu bar", |_| menu_bar(), None),
        ("toolbar", |_| toolbar(), None),
        ("palette", |_| palette(), Some(accesskit::Role::Button)),
        ("notifications", |_| notifications(), None),
        ("command line", |_| command_line(), None),
        ("property grid", |_| property_grid(), None),
        ("tree", |_| tree(), None),
        (
            "diagnostics",
            |_| diagnostics(),
            Some(accesskit::Role::Button),
        ),
        ("dock", |_| dock(), None),
        ("viewport", viewport, None),
    ];
    let mut problems = Vec::new();
    for (name, make, uniform) in widgets {
        for theme in ThemeId::ALL {
            for density in DENSITIES {
                for percent in SCALES {
                    let appearance = Appearance {
                        theme,
                        density,
                        scale: UiScale::new(percent).unwrap(),
                        reduced_motion: true,
                    };
                    problems.extend(check(name, appearance, make(appearance.scale), uniform));
                }
            }
        }
    }
    problems.sort();
    problems.dedup();
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
