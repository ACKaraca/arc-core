//! The pure mapping functions: plans of hand-built drawings, and exports read back.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use arc_base::math::Point2;
use arc_draft::dimension::Kind;
use arc_draft::types::SpaceUnit;
use arc_draft_dxf::plan::{By, Container, ImportOptions, Op, Shape, plan};
use arc_geom::Curve2;
use arc_io_dxf::model::{
    Arc, Dimension, Hatch, HatchBoundary, HatchEdge, HatchLoop, LinetypeElement, LwPolyline,
    LwVertex, PatternLine, Polyline, Spline, Vertex,
};
use arc_io_dxf::{Entity, EntityKind, Handle, Version};
use serde_json::json;
use support::{Env, drawing, line, on_layer};

fn entities(p: &arc_draft_dxf::Plan) -> Vec<(&Container, &arc_draft_dxf::plan::EntitySpec)> {
    p.ops
        .iter()
        .filter_map(|op| match op {
            Op::Entity { container, spec } => Some((container, spec.as_ref())),
            _ => None,
        })
        .collect()
}

fn options() -> ImportOptions {
    ImportOptions {
        target_unit: SpaceUnit::Millimeter,
        ..ImportOptions::default()
    }
}

#[test]
fn the_plan_creates_definitions_before_the_entities_that_use_them() {
    let mut d = drawing(
        vec![on_layer(line([0.0; 3], [1.0, 0.0, 0.0]), "Undefined")],
        Some(4),
    );
    d.blocks
        .push(support::block("B", vec![line([0.0; 3], [2.0, 0.0, 0.0])]));
    let p = plan(&d, &options());
    let order: Vec<&str> = p
        .ops
        .iter()
        .map(|op| match op {
            Op::Linetype(_) => "linetype",
            Op::Layer(_) => "layer",
            Op::TextStyle(_) => "style",
            Op::DimStyle(_) => "dimstyle",
            Op::Block(_) => "block",
            Op::Entity { .. } => "entity",
        })
        .collect();
    let mut sorted = order.clone();
    sorted.sort_by_key(|k| {
        ["linetype", "layer", "style", "dimstyle", "block", "entity"]
            .iter()
            .position(|x| x == k)
    });
    assert_eq!(order, sorted);
    let layers = p.ops.iter().filter(|op| matches!(op, Op::Layer(_))).count();
    assert_eq!(
        layers, 3,
        "0, WALLS and the layer the entity uses without a definition"
    );
    assert!(
        p.report
            .items
            .iter()
            .any(|i| i.reason == "draft.dxf.definition_created")
    );
    // The block's entities come before the entities of the model space.
    let containers: Vec<_> = entities(&p).iter().map(|(c, _)| (*c).clone()).collect();
    assert_eq!(
        containers,
        [Container::Block("B".to_owned()), Container::Space]
    );
}

#[test]
fn polyline_widths_bulges_and_vertex_flags_are_kept() {
    let lw = LwPolyline {
        flags: 1,
        constant_width: Some(0.5),
        vertices: vec![
            LwVertex {
                location: [0.0, 0.0],
                bulge: Some(0.5),
                start_width: Some(1.0),
                end_width: Some(2.0),
                ..LwVertex::default()
            },
            LwVertex {
                location: [10.0, 0.0],
                ..LwVertex::default()
            },
            LwVertex {
                location: [10.0, 0.0],
                ..LwVertex::default()
            },
            LwVertex {
                location: [10.0, 10.0],
                ..LwVertex::default()
            },
        ],
    };
    let classic = Polyline {
        flags: 0,
        start_width: Some(0.25),
        vertices: vec![
            Entity::new(EntityKind::Vertex(Vertex {
                location: [0.0, 0.0, 3.0],
                ..Vertex::default()
            })),
            Entity::new(EntityKind::Vertex(Vertex {
                location: [5.0, 0.0, 3.0],
                bulge: Some(1.0),
                ..Vertex::default()
            })),
            Entity::new(EntityKind::Vertex(Vertex {
                location: [6.0, 1.0, 3.0],
                flags: 8,
                ..Vertex::default()
            })),
            Entity::new(EntityKind::Vertex(Vertex {
                location: [9.0, 3.0, 3.0],
                ..Vertex::default()
            })),
        ],
        ..Polyline::default()
    };
    let mesh = Polyline {
        flags: 16,
        ..Polyline::default()
    };
    let d = drawing(
        vec![
            Entity::new(EntityKind::LwPolyline(lw)),
            Entity::new(EntityKind::Polyline(classic)),
            Entity::new(EntityKind::Polyline(mesh)),
        ],
        Some(4),
    );
    let p = plan(&d, &options());
    let e = entities(&p);
    assert_eq!(e.len(), 2, "the mesh is not imported");
    let Shape::Curve(Curve2::Polyline(first)) = &e[0].1.shape else {
        panic!()
    };
    assert!(
        first.is_closed() && first.vertices().len() == 3,
        "the repeated point is removed"
    );
    assert!(
        (first.vertices()[0].start_width - 1.0).abs() < 1e-12
            && (first.vertices()[1].start_width - 0.5).abs() < 1e-12
    );
    let Shape::Curve(Curve2::Polyline(second)) = &e[1].1.shape else {
        panic!()
    };
    assert_eq!(
        second.vertices().len(),
        3,
        "the spline-fit vertex is skipped"
    );
    assert!(
        (second.vertices()[1].bulge - 1.0).abs() < 1e-12
            && (second.vertices()[0].end_width).abs() < 1e-12
    );
    assert!((second.vertices()[0].start_width - 0.25).abs() < 1e-12);
    assert!((e[1].1.appearance.elevation - 3.0).abs() < 1e-12);
    assert!(
        p.report
            .items
            .iter()
            .any(|i| i.reason == "draft.dxf.entity_unmapped"
                && i.record_kind.as_deref() == Some("POLYLINE"))
    );
}

#[test]
fn dimensions_map_to_their_kinds_with_free_definition_points() {
    let mk = |kind: i16, f: &dyn Fn(&mut Dimension)| {
        let mut d = Dimension {
            kind,
            def_point: [10.0, 5.0, 0.0],
            text_mid: [5.0, 5.0, 0.0],
            text: Some("<>mm".to_owned()),
            ..Dimension::default()
        };
        f(&mut d);
        Entity::new(EntityKind::Dimension(d))
    };
    let d = drawing(
        vec![
            mk(0, &|d| {
                d.point13 = Some([0.0, 0.0, 0.0]);
                d.point14 = Some([10.0, 0.0, 0.0]);
                d.angle = Some(90.0);
            }),
            mk(1, &|d| {
                d.point13 = Some([0.0, 0.0, 0.0]);
                d.point14 = Some([10.0, 0.0, 0.0]);
            }),
            mk(4, &|d| {
                d.def_point = [0.0, 0.0, 0.0];
                d.point15 = Some([5.0, 0.0, 0.0]);
            }),
            mk(3, &|d| {
                d.def_point = [-5.0, 0.0, 0.0];
                d.point15 = Some([5.0, 0.0, 0.0]);
            }),
            mk(5, &|d| {
                d.point13 = Some([1.0, 0.0, 0.0]);
                d.point14 = Some([0.0, 1.0, 0.0]);
                d.point15 = Some([0.0, 0.0, 0.0]);
            }),
            mk(6 | 64 | 128, &|d| {
                d.point13 = Some([3.0, 4.0, 0.0]);
                d.point14 = Some([6.0, 4.0, 0.0]);
            }),
            mk(7, &|_| {}),
            mk(0, &|_| {}),
        ],
        Some(4),
    );
    let p = plan(&d, &options());
    let e = entities(&p);
    assert_eq!(e.len(), 6);
    let kinds: Vec<Kind> = e
        .iter()
        .map(|(_, s)| match &s.shape {
            Shape::Dimension(d) => d.kind,
            _ => panic!(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            Kind::Linear,
            Kind::Aligned,
            Kind::Radial,
            Kind::Diameter,
            Kind::AngularPoints,
            Kind::OrdinateX
        ]
    );
    let Shape::Dimension(linear) = &e[0].1.shape else {
        panic!()
    };
    assert!((linear.rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    assert_eq!(linear.text_override.as_deref(), Some("{}mm"));
    assert!(linear.text_position.is_none());
    let Shape::Dimension(diameter) = &e[3].1.shape else {
        panic!()
    };
    assert_eq!(
        diameter.points[0],
        Point2::new(0.0, 0.0),
        "the center is midway between the two points"
    );
    let Shape::Dimension(ordinate) = &e[5].1.shape else {
        panic!()
    };
    assert!(ordinate.text_position.is_some());
    assert!(
        p.report
            .items
            .iter()
            .any(|i| i.reason == "draft.dxf.entity_unmapped"
                && i.record_kind.as_deref() == Some("DIMENSION"))
    );
    assert!(
        p.report
            .items
            .iter()
            .any(|i| i.reason == "draft.dxf.geometry_invalid"
                && i.record_kind.as_deref() == Some("DIMENSION"))
    );
}

fn hatch(boundary: HatchBoundary, solid: i16, lines: Vec<PatternLine>) -> Entity {
    Entity::new(EntityKind::Hatch(Hatch {
        pattern_name: "X".to_owned(),
        solid,
        loops: vec![HatchLoop {
            flags: 1,
            boundary,
            sources: vec![],
        }],
        pattern_lines: lines,
        ..Hatch::default()
    }))
}

#[test]
fn hatch_boundaries_chain_lines_and_arcs_and_patterns_keep_their_lines() {
    let line_edge = |a: [f64; 2], b: [f64; 2]| HatchEdge::Line { start: a, end: b };
    // Two lines and a half circle, the second line given backwards.
    let chain = HatchBoundary::Edges(vec![
        line_edge([0.0, 0.0], [10.0, 0.0]),
        line_edge([10.0, 10.0], [10.0, 0.0]),
        HatchEdge::Arc {
            center: [5.0, 10.0],
            radius: 5.0,
            start_angle: 0.0,
            end_angle: 180.0,
            counterclockwise: true,
        },
        line_edge([0.0, 10.0], [0.0, 0.0]),
    ]);
    let open = HatchBoundary::Edges(vec![
        line_edge([0.0, 0.0], [10.0, 0.0]),
        line_edge([10.0, 0.0], [10.0, 5.0]),
    ]);
    let ellipse_arc = HatchBoundary::Edges(vec![HatchEdge::Ellipse {
        center: [0.0; 2],
        major_axis: [2.0, 0.0],
        ratio: 0.5,
        start_angle: 0.0,
        end_angle: 90.0,
        counterclockwise: true,
    }]);
    let circle = HatchBoundary::Edges(vec![HatchEdge::Arc {
        center: [0.0; 2],
        radius: 3.0,
        start_angle: 0.0,
        end_angle: 360.0,
        counterclockwise: true,
    }]);
    let lines = vec![
        PatternLine {
            angle: 45.0,
            base: [0.0, 0.0],
            offset: [-0.5, 0.5],
            dashes: vec![1.0, -1.0],
        },
        PatternLine {
            angle: 0.0,
            base: [0.0, 0.0],
            offset: [0.0, 0.0],
            dashes: vec![],
        },
    ];
    let d = drawing(
        vec![
            hatch(chain, 1, vec![]),
            hatch(open, 1, vec![]),
            hatch(ellipse_arc, 1, vec![]),
            hatch(circle, 0, lines),
            hatch(
                HatchBoundary::Polyline {
                    has_bulge: false,
                    closed: true,
                    vertices: vec![],
                },
                1,
                vec![],
            ),
        ],
        Some(4),
    );
    let p = plan(&d, &options());
    let e = entities(&p);
    assert_eq!(
        e.len(),
        2,
        "the open chain, the elliptical arc and the empty loop are dropped"
    );
    let Shape::Hatch(first) = &e[0].1.shape else {
        panic!()
    };
    let Curve2::Polyline(loop_) = &first.loops[0] else {
        panic!()
    };
    assert_eq!(loop_.vertices().len(), 4);
    assert!(
        loop_
            .vertices()
            .iter()
            .any(|v| (v.bulge.abs() - 1.0).abs() < 1e-9),
        "the half circle is one segment with bulge 1"
    );
    let Shape::Hatch(second) = &e[1].1.shape else {
        panic!()
    };
    assert!(matches!(second.loops[0], Curve2::Circle(_)));
    assert!(
        !second.solid && second.families.len() == 1,
        "the line with no spacing is dropped"
    );
    let family = &second.families[0];
    assert!(
        (family.spacing - 0.5 * 2.0_f64.sqrt()).abs() < 1e-9 && family.shift.abs() < 1e-9,
        "the offset is split into shift and spacing"
    );
    assert_eq!(
        p.report.summary.failed,
        2 + 1,
        "two hatches and the empty loop's geometry"
    );
}

#[test]
fn splines_fall_back_to_their_fit_points_and_arcs_keep_their_direction() {
    let good = Spline {
        degree: 3,
        knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        control_points: vec![
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 2.0, 0.0],
            [4.0, 0.0, 0.0],
        ],
        ..Spline::default()
    };
    let broken_knots = Spline {
        degree: 3,
        knots: vec![0.0, 1.0],
        control_points: good.control_points.clone(),
        fit_points: vec![
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [2.0, 0.0, 0.0],
            [3.0, 1.0, 0.0],
        ],
        ..Spline::default()
    };
    let nothing = Spline {
        degree: 3,
        ..Spline::default()
    };
    let arc = Arc {
        center: [0.0; 3],
        radius: 5.0,
        start_angle: 350.0,
        end_angle: 10.0,
    };
    let d = drawing(
        vec![
            Entity::new(EntityKind::Spline(good)),
            Entity::new(EntityKind::Spline(broken_knots)),
            Entity::new(EntityKind::Spline(nothing)),
            Entity::new(EntityKind::Arc(arc)),
        ],
        Some(4),
    );
    let p = plan(&d, &options());
    let e = entities(&p);
    assert_eq!(e.len(), 3);
    let Shape::Spline { fit, .. } = &e[1].1.shape else {
        panic!()
    };
    assert_eq!(fit.len(), 4);
    let Shape::Curve(Curve2::Arc(a)) = &e[2].1.shape else {
        panic!()
    };
    assert!(
        (a.sweep() - 20.0_f64.to_radians()).abs() < 1e-12,
        "an arc across 0 degrees sweeps 20 degrees"
    );
}

#[test]
fn appearance_and_table_values_map_to_the_drafting_fields() {
    let mut e = line([0.0; 3], [1.0, 0.0, 0.0]);
    e.head.handle = Some(Handle(0x10));
    e.props.color = Some(3);
    e.props.lineweight = Some(35);
    e.props.linetype = Some("ByBlock".to_owned());
    e.props.transparency = Some(0x0200_0000 | 51);
    e.props.linetype_scale = Some(2.0);
    e.props.invisible = true;
    e.props.thickness = Some(2.0);
    e.props.extrusion = Some([0.0, 0.0, -1.0]);
    let mut f = line([0.0; 3], [1.0, 0.0, 0.0]);
    f.props.color = Some(0);
    f.props.lineweight = Some(-2);
    f.props.linetype = Some("Dashed".to_owned());
    let mut g = line([0.0; 3], [1.0, 0.0, 0.0]);
    g.props.color = Some(256);
    let mut h = line([0.0; 3], [1.0, 0.0, 0.0]);
    h.props.true_color = Some(0x0012_3456);
    h.props.color = Some(5);
    g.props.lineweight = Some(-3);
    let mut d = drawing(vec![e, f, g, h], Some(4));
    d.tables.linetypes = Some(arc_io_dxf::model::Table {
        entries: vec![arc_io_dxf::model::TableEntry {
            name: "Dashed".to_owned(),
            data: arc_io_dxf::model::Linetype {
                description: "dashed".to_owned(),
                elements: vec![
                    LinetypeElement {
                        length: 6.0,
                        ..LinetypeElement::default()
                    },
                    LinetypeElement {
                        length: -3.0,
                        ..LinetypeElement::default()
                    },
                    LinetypeElement {
                        length: 0.0,
                        ..LinetypeElement::default()
                    },
                ],
                ..arc_io_dxf::model::Linetype::default()
            },
            ..arc_io_dxf::model::TableEntry::default()
        }],
        ..arc_io_dxf::model::Table::default()
    });
    d.tables.layers.as_mut().unwrap().entries[1].flags = 1 | 4;
    d.tables.layers.as_mut().unwrap().entries[1].data.color = -1;
    let p = plan(&d, &options());
    let a = &entities(&p)[0].1.appearance;
    assert!(
        matches!(&a.color, By::Object(c) if c.source == Some(arc_doc::ColorSource::Indexed(3)))
    );
    assert_eq!(a.lineweight, By::Object(Some(0.35)));
    assert_eq!(a.linetype, By::Block);
    assert!(matches!(a.transparency, By::Object(t) if (t - 0.8).abs() < 1e-9));
    assert!(
        (a.linetype_scale - 2.0).abs() < 1e-12 && !a.visible && (a.thickness - 2.0).abs() < 1e-12
    );
    assert_eq!(a.extrusion, [0.0, 0.0, -1.0]);
    let b = &entities(&p)[1].1.appearance;
    assert!(matches!(&b.color, By::Block));
    assert_eq!(b.lineweight, By::Block);
    assert_eq!(b.linetype, By::Object(Some("Dashed".to_owned())));
    let c = &entities(&p)[2].1.appearance;
    assert_eq!(
        (c.color.clone(), c.lineweight.clone()),
        (By::Layer, By::Object(None))
    );
    let d = &entities(&p)[3].1.appearance;
    assert!(
        matches!(&d.color, By::Object(c) if c.source.is_none() && c.rgba.to_srgb8() == [0x12, 0x34, 0x56, 255]),
        "a true color wins over the index"
    );
    let Some(Op::Linetype(lt)) = p.ops.iter().find(|op| matches!(op, Op::Linetype(_))) else {
        panic!()
    };
    let kinds: Vec<_> = lt.elements.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            arc_draft::types::ElementKind::Dash,
            arc_draft::types::ElementKind::Gap,
            arc_draft::types::ElementKind::Dot
        ]
    );
    let layer = p
        .ops
        .iter()
        .find_map(|op| match op {
            Op::Layer(l) if l.name == "WALLS" => Some(l),
            _ => None,
        })
        .unwrap();
    assert!(!layer.on && layer.frozen && layer.locked);
    let colored = p
        .ops
        .iter()
        .find_map(|op| match op {
            Op::Layer(l) if l.name == "0" => Some(l),
            _ => None,
        })
        .unwrap();
    assert_eq!(colored.color.source, Some(arc_doc::ColorSource::Indexed(7)));
}

#[test]
fn an_export_reads_back_in_every_version_without_structure_warnings() {
    let mut env = Env::new();
    let space = env.space("model", "meter");
    let owner = space.to_string();
    env.create("core.draft.layer.create", json!({"name": "0"}));
    env.create(
        "core.draft.entity.create",
        json!({"owner": owner, "geometry": {"kind": "point", "position": [1.0, 2.0]}}),
    );
    let rich = env.create(
        "core.draft.rich_text.create",
        json!({"owner": owner, "text": "a\nb", "position": [0.0, 0.0]}),
    );
    env.run(
        "core.draft.rich_text.set",
        json!({"objects": [rich.to_string()], "paragraphs": [{"runs": [{"text": "bold", "bold": true}]}]}),
    );
    env.create("core.draft.multileader.create", json!({"owner": owner, "leaders": [[[0.0, 0.0], [1.0, 1.0]]], "position": [2.0, 2.0], "text": "x"}));
    for version in ["2000", "2004", "2007", "2010", "2013", "2018"] {
        let out = env.path(&format!("v{version}.dxf"));
        let result = env.run(
            "core.draft.export.dxf",
            json!({"path": out.to_str().unwrap(), "space": owner, "version": version}),
        );
        let read = arc_io_dxf::read(
            &std::fs::read(&out).unwrap(),
            &mut arc_io_dxf::ReadOptions::default(),
        )
        .unwrap();
        assert_eq!(read.drawing.version.name(), version);
        assert_eq!(read.drawing.entities.len(), 2);
        let reasons: Vec<&str> = result["report"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["reason"].as_str().unwrap())
            .collect();
        assert!(
            !reasons.contains(&"io.dxf.handle_assigned")
                && !reasons.contains(&"io.dxf.structure_incomplete"),
            "{reasons:?}"
        );
        assert!(
            reasons.contains(&"draft.dxf.export_unmapped"),
            "the multi-leader has no DXF counterpart"
        );
        assert!(
            reasons.contains(&"draft.dxf.export_approximated"),
            "run formatting is not written"
        );
        assert_eq!(
            read.drawing.header_variable("$INSUNITS").unwrap()[0].value,
            arc_io_dxf::Value::Int(6)
        );
    }
    let binary = env.path("b.dxf");
    env.run(
        "core.draft.export.dxf",
        json!({"path": binary.to_str().unwrap(), "space": owner, "binary": true}),
    );
    let read = arc_io_dxf::read(
        &std::fs::read(&binary).unwrap(),
        &mut arc_io_dxf::ReadOptions::default(),
    )
    .unwrap();
    assert_eq!(read.drawing.version, Version::R2018);
    let error = env
        .invoke(
            "core.draft.export.dxf",
            json!({"path": binary.to_str().unwrap(), "space": owner, "version": "2.0"}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.params.invalid");
}

#[test]
fn extreme_magnitudes_are_imported_or_reported_never_a_panic() {
    let big = 1e307;
    let tiny = 1e-300;
    let text = arc_io_dxf::model::Text {
        insert: [big, 0.0, 0.0],
        height: big,
        value: "x".to_owned(),
        ..arc_io_dxf::model::Text::default()
    };
    let d = drawing(
        vec![
            line([0.0; 3], [big, big, 0.0]),
            line([0.0; 3], [tiny, tiny, 0.0]),
            support::circle([big, 0.0, 0.0], big),
            Entity::new(EntityKind::Point(arc_io_dxf::model::Point {
                location: [big, 0.0, 0.0],
                angle: None,
            })),
            Entity::new(EntityKind::Text(text)),
            Entity::new(EntityKind::Point(arc_io_dxf::model::Point {
                location: [tiny, 1.0, 2.0],
                angle: None,
            })),
        ],
        Some(1),
    );
    let p = plan(&d, &options());
    for (_, spec) in entities(&p) {
        assert!(
            arc_draft_dxf::plan::Appearance::clone(&spec.appearance)
                .elevation
                .is_finite()
        );
    }
    let failed = p
        .report
        .items
        .iter()
        .filter(|i| i.reason == "draft.dxf.geometry_invalid")
        .count();
    assert!(failed >= 3, "{failed}");
    // What was imported is accepted by a document.
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("big.dxf", &support::bytes(&d));
    assert!(env.import(&path, space).is_ok());
}

#[test]
fn linetype_lengths_and_offsets_convert_with_the_file_unit() {
    // A file in inches with a dashed linetype: the dash, gap and offset are drawing lengths and
    // convert to millimeters like the geometry does. The scale is a ratio and keeps its value.
    let mut d = drawing(vec![line([0.0; 3], [1.0, 0.0, 0.0])], Some(1));
    d.tables.linetypes = Some(arc_io_dxf::model::Table {
        entries: vec![arc_io_dxf::model::TableEntry {
            name: "Dashed".to_owned(),
            data: arc_io_dxf::model::Linetype {
                elements: vec![
                    LinetypeElement {
                        length: 0.5,
                        scale: Some(2.0),
                        offset_x: Some(0.1),
                        offset_y: Some(-0.2),
                        ..LinetypeElement::default()
                    },
                    LinetypeElement {
                        length: -0.25,
                        ..LinetypeElement::default()
                    },
                ],
                ..arc_io_dxf::model::Linetype::default()
            },
            ..arc_io_dxf::model::TableEntry::default()
        }],
        ..arc_io_dxf::model::Table::default()
    });
    let p = plan(&d, &options());
    let Some(Op::Linetype(lt)) = p.ops.iter().find(|op| matches!(op, Op::Linetype(_))) else {
        panic!("the linetype is planned")
    };
    let lengths: Vec<f64> = lt.elements.iter().map(|e| e.length).collect();
    assert_eq!(lengths.len(), 2);
    assert!((lengths[0] - 12.7).abs() < 1e-9, "{lengths:?}");
    assert!((lengths[1] - 6.35).abs() < 1e-9, "{lengths:?}");
    let first = &lt.elements[0];
    assert!((first.scale - 2.0).abs() < 1e-12);
    let offset = first.offset.unwrap_or([f64::NAN; 2]);
    assert!(
        (offset[0] - 2.54).abs() < 1e-9 && (offset[1] + 5.08).abs() < 1e-9,
        "{offset:?}"
    );
}
