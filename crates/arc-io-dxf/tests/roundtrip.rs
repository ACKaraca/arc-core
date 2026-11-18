//! Property tests: generated drawings with every entity kind, tables, blocks, extended data and
//! preserved records are written for each version, in ASCII and binary, and read back equal;
//! writing is deterministic; every report is schema-valid.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

mod common;

use arc_io_dxf::encoding::CODE_PAGES;
use arc_io_dxf::model::{
    AppId, Arc, AttDef, Attrib, Block, BlockBegin, BlockRecord, Circle, Class, Dictionary,
    DictionaryEntry, DimStyle, Dimension, Ellipse, EntityProps, GroupObject, Hatch, HatchBoundary,
    HatchEdge, HatchLoop, HatchSpline, HatchVertex, Head, HeaderVariable, Image, Insert, Layer,
    Layout, Leader, Line, Linetype, LinetypeElement, LwPolyline, LwVertex, MText, P2, P3,
    PatternLine, Point, Polyline, PreservedSection, PreservedTable, Quad, Ray, SeqEnd, Spline,
    Table, TableEntry, Text, TextStyle, Ucs, VPort, Vertex, View, Viewport, XData, XRecord,
};
use arc_io_dxf::{
    Drawing, Entity, EntityKind, Group, Handle, Object, ObjectKind, PreservedRecord, ReadOptions,
    Value, ValueKind, Version, WriteOptions, read, value_kind, write,
};
use common::assert_report_valid;
use proptest::collection::vec;
use proptest::option::of;
use proptest::prelude::*;

fn num() -> impl Strategy<Value = f64> {
    use proptest::num::f64::{NEGATIVE, NORMAL, POSITIVE, SUBNORMAL, ZERO};
    prop_oneof![
        3 => -1e3..1e3f64,
        1 => POSITIVE | NEGATIVE | NORMAL | SUBNORMAL | ZERO,
        1 => prop_oneof![Just(1e300), Just(-1e300), Just(1e-300), Just(-0.0)],
    ]
}

fn p3() -> impl Strategy<Value = P3> {
    [num(), num(), num()]
}

fn p2() -> impl Strategy<Value = P2> {
    [num(), num()]
}

/// Text from several scripts, with characters the encoder must escape, literal escape
/// sequences and raw-byte characters.
fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-zA-Z0-9 _.-]{0,10}",
        2 => "[ğüşıöçĞÜŞİÖÇαβγΩабвЖąęłŁāēšŠ€½日本😀\\\\\n\r\t\u{0}\u{F781}\u{F7E9} a]{0,10}",
        1 => Just("literal \\U+0041 and \\U+".to_owned()),
    ]
}

fn handle() -> impl Strategy<Value = Handle> {
    (0u64..0x2000).prop_map(Handle)
}

fn i8v() -> impl Strategy<Value = i16> {
    0i16..=255
}

/// A group whose value matches its code.
fn group_of(codes: &'static [i16]) -> impl Strategy<Value = Group> {
    (
        prop::sample::select(codes),
        text(),
        num(),
        any::<i64>(),
        vec(any::<u8>(), 0..40),
    )
        .prop_map(|(code, t, n, i, b)| {
            let value = match value_kind(code).unwrap() {
                ValueKind::Text => Value::Text(t),
                ValueKind::Double => Value::Double(n),
                ValueKind::Int8 => Value::Int(i.rem_euclid(256)),
                ValueKind::Int16 => Value::Int(i64::from(i as i16)),
                ValueKind::Int32 => Value::Int(i64::from(i as i32)),
                ValueKind::Int64 => Value::Int(i),
                ValueKind::Bool => Value::Bool(i % 2 == 0),
                ValueKind::Bytes => Value::Bytes(b),
            };
            Group { code, value }
        })
}

/// Codes no typed record claims, for preserved groups of typed records.
const SAFE: &[i16] = &[460, 465, 470, 475];
/// Codes for opaque data (preserved records, plot settings, extension records, header values).
const ANY: &[i16] = &[
    1, 2, 3, 10, 40, 62, 70, 90, 160, 280, 290, 300, 310, 330, 1000, 1040, 1071,
];
const XDATA: &[i16] = &[1000, 1004, 1005, 1010, 1020, 1030, 1040, 1070, 1071];

fn extras() -> impl Strategy<Value = Vec<Group>> {
    vec(group_of(SAFE), 0..3)
}

fn xdata() -> impl Strategy<Value = Vec<XData>> {
    vec(
        (text(), vec(group_of(XDATA), 0..4)).prop_map(|(app, groups)| XData { app, groups }),
        0..2,
    )
}

fn head() -> impl Strategy<Value = Head> {
    (of(handle()), any::<bool>(), handle()).prop_map(|(owner, reactors, r)| Head {
        handle: None,
        owner,
        app_groups: if reactors {
            vec![
                Group::text(102, "{ACAD_REACTORS"),
                Group::text(330, r.to_hex()),
                Group::text(102, "}"),
            ]
        } else {
            Vec::new()
        },
    })
}

fn props() -> impl Strategy<Value = EntityProps> {
    (
        (
            any::<bool>(),
            of(text()),
            text(),
            of(text()),
            of(any::<i16>()),
            of(any::<i32>()),
        ),
        (
            of(text()),
            of(any::<i16>()),
            of(any::<i32>()),
            of(num()),
            any::<bool>(),
        ),
        (of(num()), of(num()), of(p3())),
    )
        .prop_map(|(a, b, c)| EntityProps {
            paper_space: a.0,
            layout: a.1,
            layer: a.2,
            linetype: a.3,
            color: a.4,
            true_color: a.5,
            color_name: b.0,
            lineweight: b.1,
            transparency: b.2,
            linetype_scale: b.3,
            invisible: b.4,
            elevation: c.0,
            thickness: c.1,
            extrusion: c.2,
        })
}

fn wrap(kind: impl Strategy<Value = EntityKind>) -> impl Strategy<Value = Entity> {
    (head(), props(), kind, extras(), xdata()).prop_map(|(head, props, kind, extras, xdata)| {
        Entity {
            head,
            props,
            kind,
            extras,
            xdata,
        }
    })
}

fn seqend() -> impl Strategy<Value = Entity> {
    wrap(Just(EntityKind::SeqEnd(SeqEnd::default())))
}

fn attrib() -> impl Strategy<Value = Attrib> {
    (
        (p3(), num(), text(), of(num()), of(num()), of(num())),
        (of(text()), of(any::<i16>()), of(any::<i16>()), of(p3())),
        (text(), any::<i16>(), of(any::<i16>()), of(any::<i16>())),
    )
        .prop_map(|(a, b, c)| Attrib {
            insert: a.0,
            height: a.1,
            value: a.2,
            rotation: a.3,
            width_factor: a.4,
            oblique: a.5,
            style: b.0,
            generation: b.1,
            horizontal: b.2,
            align: b.3,
            tag: c.0,
            flags: c.1,
            field_length: c.2,
            vertical: c.3,
        })
}

fn spline_edge(version: Version) -> impl Strategy<Value = HatchEdge> {
    let fit = version >= Version::R2010;
    (
        any::<i32>(),
        any::<bool>(),
        any::<bool>(),
        vec(num(), 0..5),
        vec((p2(), num()), 0..4),
        vec(p2(), 0..3),
        of(p2()),
        of(p2()),
    )
        .prop_map(
            move |(degree, rational, periodic, knots, control, fits, s, e)| {
                HatchEdge::Spline(HatchSpline {
                    degree,
                    rational,
                    periodic,
                    knots,
                    control_points: control.iter().map(|c| c.0).collect(),
                    weights: if rational {
                        control.iter().map(|c| c.1).collect()
                    } else {
                        Vec::new()
                    },
                    fit_points: if fit { fits } else { Vec::new() },
                    start_tangent: s.filter(|_| fit),
                    end_tangent: e.filter(|_| fit),
                })
            },
        )
}

fn edge(version: Version) -> impl Strategy<Value = HatchEdge> {
    prop_oneof![
        (p2(), p2()).prop_map(|(start, end)| HatchEdge::Line { start, end }),
        (p2(), num(), num(), num(), any::<bool>()).prop_map(|(c, r, s, e, ccw)| HatchEdge::Arc {
            center: c,
            radius: r,
            start_angle: s,
            end_angle: e,
            counterclockwise: ccw,
        }),
        (p2(), p2(), num(), num(), num(), any::<bool>()).prop_map(|(c, m, r, s, e, ccw)| {
            HatchEdge::Ellipse {
                center: c,
                major_axis: m,
                ratio: r,
                start_angle: s,
                end_angle: e,
                counterclockwise: ccw,
            }
        }),
        spline_edge(version),
    ]
}

fn hatch_loop(version: Version) -> impl Strategy<Value = HatchLoop> {
    let boundary = prop_oneof![
        (any::<bool>(), any::<bool>(), vec((p2(), num()), 0..4)).prop_map(
            |(has_bulge, closed, v)| {
                HatchBoundary::Polyline {
                    has_bulge,
                    closed,
                    vertices: v
                        .into_iter()
                        .map(|(location, b)| HatchVertex {
                            location,
                            bulge: if has_bulge { b } else { 0.0 },
                        })
                        .collect(),
                }
            }
        ),
        vec(edge(version), 0..4).prop_map(HatchBoundary::Edges),
    ];
    (any::<i32>(), boundary, vec(handle(), 0..3)).prop_map(|(flags, boundary, sources)| {
        let flags = match boundary {
            HatchBoundary::Polyline { .. } => flags | 2,
            HatchBoundary::Edges(_) => flags & !2,
        };
        HatchLoop {
            flags,
            boundary,
            sources,
        }
    })
}

fn hatch(version: Version) -> impl Strategy<Value = Hatch> {
    (
        (
            p3(),
            text(),
            any::<i16>(),
            any::<i16>(),
            of(any::<i16>()),
            of(any::<i16>()),
        ),
        (of(num()), of(num()), of(any::<i16>()), of(num())),
        vec(hatch_loop(version), 0..3),
        vec(
            (num(), p2(), p2(), vec(num(), 0..3)).prop_map(|(angle, base, offset, dashes)| {
                PatternLine {
                    angle,
                    base,
                    offset,
                    dashes,
                }
            }),
            0..3,
        ),
        vec(p2(), 0..3),
    )
        .prop_map(|(a, b, loops, pattern_lines, seeds)| Hatch {
            elevation_point: a.0,
            pattern_name: a.1,
            solid: a.2,
            associative: a.3,
            style: a.4,
            pattern_type: a.5,
            pattern_angle: b.0,
            pattern_scale: b.1,
            pattern_double: b.2,
            pixel_size: b.3,
            loops,
            pattern_lines,
            seeds,
        })
}

fn quad() -> impl Strategy<Value = Quad> {
    (p3(), p3(), p3(), p3(), of(any::<i16>())).prop_map(|(p1, p2, p3, p4, edge_flags)| Quad {
        p1,
        p2,
        p3,
        p4,
        edge_flags,
    })
}

fn image() -> impl Strategy<Value = Image> {
    (
        (of(any::<i32>()), p3(), p3(), p3(), p2(), of(handle())),
        (of(any::<i16>()), of(i8v()), of(i8v()), of(i8v()), of(i8v())),
        (of(handle()), of(any::<i16>()), vec(p2(), 0..4)),
    )
        .prop_map(|(a, b, c)| Image {
            class_version: a.0,
            insert: a.1,
            u_vector: a.2,
            v_vector: a.3,
            size: a.4,
            definition: a.5,
            display: b.0,
            clipping: b.1,
            brightness: b.2,
            contrast: b.3,
            fade: b.4,
            reactor: c.0,
            clip_type: c.1,
            clip_vertices: c.2,
        })
}

fn opt_p3() -> impl Strategy<Value = Option<P3>> {
    of(p3())
}

/// Every entity kind that may stand in an entity list.
fn kind(version: Version) -> impl Strategy<Value = EntityKind> {
    let simple = prop_oneof![
        (p3(), of(num()))
            .prop_map(|(location, angle)| EntityKind::Point(Point { location, angle })),
        (p3(), p3()).prop_map(|(start, end)| EntityKind::Line(Line { start, end })),
        (p3(), p3()).prop_map(|(start, direction)| EntityKind::Ray(Ray { start, direction })),
        (p3(), p3()).prop_map(|(start, direction)| {
            EntityKind::ConstructionLine(Ray { start, direction })
        }),
        (p3(), num()).prop_map(|(center, radius)| EntityKind::Circle(Circle { center, radius })),
        (p3(), num(), num(), num()).prop_map(|(center, radius, s, e)| EntityKind::Arc(Arc {
            center,
            radius,
            start_angle: s,
            end_angle: e,
        })),
        (p3(), p3(), num(), num(), num()).prop_map(|(center, major_axis, ratio, s, e)| {
            EntityKind::Ellipse(Ellipse {
                center,
                major_axis,
                ratio,
                start_param: s,
                end_param: e,
            })
        }),
        (
            any::<i16>(),
            of(num()),
            vec(
                (p2(), of(any::<i32>()), of(num()), of(num()), of(num())).prop_map(
                    |(location, id, start_width, end_width, bulge)| LwVertex {
                        location,
                        id,
                        start_width,
                        end_width,
                        bulge,
                    }
                ),
                0..5
            ),
        )
            .prop_map(|(flags, constant_width, vertices)| {
                EntityKind::LwPolyline(LwPolyline {
                    flags,
                    constant_width,
                    vertices,
                })
            }),
        quad().prop_map(EntityKind::Solid),
        quad().prop_map(EntityKind::Trace),
        quad().prop_map(EntityKind::Face),
        image().prop_map(EntityKind::Image),
        image().prop_map(EntityKind::Wipeout),
    ];
    let text_kinds = prop_oneof![
        (
            (p3(), num(), text(), of(num()), of(num()), of(num())),
            (
                of(text()),
                of(any::<i16>()),
                of(any::<i16>()),
                opt_p3(),
                of(any::<i16>())
            ),
        )
            .prop_map(|(a, b)| EntityKind::Text(Text {
                insert: a.0,
                height: a.1,
                value: a.2,
                rotation: a.3,
                width_factor: a.4,
                oblique: a.5,
                style: b.0,
                generation: b.1,
                horizontal: b.2,
                align: b.3,
                vertical: b.4,
            })),
        (
            (
                p3(),
                num(),
                of(num()),
                of(any::<i16>()),
                of(any::<i16>()),
                of(text())
            ),
            (opt_p3(), of(num()), of(any::<i16>()), of(num())),
            prop_oneof![text(), "[a-zé日\\\\]{240,600}"],
        )
            .prop_map(|(a, b, text)| EntityKind::MText(MText {
                insert: a.0,
                height: a.1,
                width: a.2,
                attachment: a.3,
                direction: a.4,
                style: a.5,
                x_axis: b.0,
                rotation: b.1,
                spacing_style: b.2,
                spacing_factor: b.3,
                text,
            })),
        (attrib(), text()).prop_map(|(a, prompt)| EntityKind::AttDef(AttDef {
            insert: a.insert,
            height: a.height,
            value: a.value,
            rotation: a.rotation,
            width_factor: a.width_factor,
            oblique: a.oblique,
            style: a.style,
            generation: a.generation,
            horizontal: a.horizontal,
            align: a.align,
            prompt,
            tag: a.tag,
            flags: a.flags,
            field_length: a.field_length,
            vertical: a.vertical,
        })),
        attrib().prop_map(EntityKind::Attrib),
    ];
    let compound = prop_oneof![
        (
            (
                opt_p3(),
                any::<i16>(),
                of(num()),
                of(num()),
                of(any::<i16>()),
                of(any::<i16>())
            ),
            (of(any::<i16>()), of(any::<i16>()), of(any::<i16>())),
            vec(
                wrap(
                    (
                        (
                            p3(),
                            of(num()),
                            of(num()),
                            of(num()),
                            any::<i16>(),
                            of(num())
                        ),
                        (of(any::<i16>()), of(any::<i16>()), of(any::<i16>())),
                        (of(any::<i16>()), of(any::<i32>())),
                    )
                        .prop_map(|(a, b, c)| EntityKind::Vertex(Vertex {
                            location: a.0,
                            start_width: a.1,
                            end_width: a.2,
                            bulge: a.3,
                            flags: a.4,
                            tangent: a.5,
                            index1: b.0,
                            index2: b.1,
                            index3: b.2,
                            index4: c.0,
                            id: c.1,
                        }))
                ),
                0..3
            ),
            seqend(),
        )
            .prop_map(|(a, b, vertices, seqend)| EntityKind::Polyline(Polyline {
                origin: a.0,
                flags: a.1,
                start_width: a.2,
                end_width: a.3,
                mesh_m: a.4,
                mesh_n: a.5,
                smooth_m: b.0,
                smooth_n: b.1,
                smooth_type: b.2,
                vertices,
                seqend: Some(Box::new(seqend)),
            })),
        (
            (text(), p3(), of(num()), of(num()), of(num()), of(num())),
            (of(any::<i16>()), of(any::<i16>()), of(num()), of(num())),
            vec(wrap(attrib().prop_map(EntityKind::Attrib)), 0..3),
            of(seqend()),
        )
            .prop_map(|(a, b, attribs, seqend)| {
                // A sequence end always follows attributes.
                let seqend = if attribs.is_empty() {
                    seqend
                } else {
                    seqend.or_else(|| Some(Entity::new(EntityKind::SeqEnd(SeqEnd::default()))))
                };
                EntityKind::Insert(Insert {
                    block: a.0,
                    insert: a.1,
                    x_scale: a.2,
                    y_scale: a.3,
                    z_scale: a.4,
                    rotation: a.5,
                    columns: b.0,
                    rows: b.1,
                    column_spacing: b.2,
                    row_spacing: b.3,
                    attribs,
                    seqend: seqend.map(Box::new),
                })
            }),
        (
            (
                any::<i16>(),
                any::<i16>(),
                of(num()),
                of(num()),
                of(num()),
                opt_p3()
            ),
            (opt_p3(), vec(num(), 0..6), vec(num(), 0..3)),
            (vec(p3(), 0..4), vec(p3(), 0..3)),
        )
            .prop_map(|(a, b, c)| EntityKind::Spline(Spline {
                flags: a.0,
                degree: a.1,
                knot_tolerance: a.2,
                control_tolerance: a.3,
                fit_tolerance: a.4,
                start_tangent: a.5,
                end_tangent: b.0,
                knots: b.1,
                weights: b.2,
                control_points: c.0,
                fit_points: c.1,
            })),
        (
            (of(text()), of(text()), p3(), p3(), opt_p3(), any::<i16>()),
            (
                of(any::<i16>()),
                of(any::<i16>()),
                of(num()),
                of(num()),
                of(text()),
                of(num())
            ),
            (of(num()), opt_p3(), opt_p3(), opt_p3(), opt_p3(), of(num())),
            (of(num()), of(num())),
        )
            .prop_map(|(a, b, c, d)| EntityKind::Dimension(Dimension {
                block: a.0,
                style: a.1,
                def_point: a.2,
                text_mid: a.3,
                insert: a.4,
                kind: a.5,
                attachment: b.0,
                spacing_style: b.1,
                spacing_factor: b.2,
                measurement: b.3,
                text: b.4,
                text_rotation: b.5,
                horizontal: c.0,
                point13: c.1,
                point14: c.2,
                point15: c.3,
                point16: c.4,
                angle: c.5,
                oblique: d.0,
                leader_length: d.1,
            })),
        (
            (
                text(),
                of(any::<i16>()),
                of(any::<i16>()),
                of(any::<i16>()),
                of(any::<i16>())
            ),
            (
                of(any::<i16>()),
                of(num()),
                of(num()),
                of(any::<i16>()),
                of(handle())
            ),
            vec(p3(), 0..4),
        )
            .prop_map(|(a, b, vertices)| EntityKind::Leader(Leader {
                style: a.0,
                arrowhead: a.1,
                path_type: a.2,
                creation: a.3,
                hookline_direction: a.4,
                hookline: b.0,
                text_height: b.1,
                text_width: b.2,
                color: b.3,
                annotation: b.4,
                vertices,
            })),
        hatch(version).prop_map(EntityKind::Hatch),
        (
            (
                p3(),
                num(),
                num(),
                of(any::<i16>()),
                of(any::<i16>()),
                of(p2())
            ),
            (of(p2()), of(p2()), of(p2()), opt_p3(), opt_p3(), of(num())),
            (
                of(num()),
                of(num()),
                of(num()),
                of(num()),
                of(num()),
                of(any::<i32>())
            ),
            vec(handle(), 0..3),
        )
            .prop_map(|(a, b, c, frozen_layers)| EntityKind::Viewport(Viewport {
                center: a.0,
                width: a.1,
                height: a.2,
                status: a.3,
                id: a.4,
                view_center: a.5,
                snap_base: b.0,
                snap_spacing: b.1,
                grid_spacing: b.2,
                view_direction: b.3,
                view_target: b.4,
                lens: b.5,
                front_clip: c.0,
                back_clip: c.1,
                view_height: c.2,
                snap_angle: c.3,
                twist: c.4,
                flags: c.5,
                frozen_layers,
            })),
    ];
    let preserved = (vec(group_of(ANY), 0..5)).prop_map(|groups| {
        EntityKind::Preserved(PreservedRecord {
            kind: "ARC_TEST_SHAPE".to_owned(),
            groups,
        })
    });
    prop_oneof![4 => simple.boxed(), 2 => text_kinds.boxed(), 3 => compound.boxed(), 1 => preserved.boxed()]
}

fn entity(version: Version) -> impl Strategy<Value = Entity> {
    wrap(kind(version)).prop_map(|mut e| {
        if matches!(e.kind, EntityKind::Preserved(_)) {
            e = Entity {
                head: Head::default(),
                props: EntityProps::default(),
                kind: e.kind,
                extras: Vec::new(),
                xdata: Vec::new(),
            };
        }
        e
    })
}

fn entry<T: std::fmt::Debug + Clone>(
    data: impl Strategy<Value = T>,
) -> impl Strategy<Value = TableEntry<T>> {
    (head(), text(), any::<i16>(), data, extras(), xdata()).prop_map(
        |(head, name, flags, data, extras, xdata)| TableEntry {
            head,
            name,
            flags,
            data,
            extras,
            xdata,
        },
    )
}

fn table<T: std::fmt::Debug + Clone>(
    data: impl Strategy<Value = T>,
) -> impl Strategy<Value = Option<Table<T>>> {
    of(
        (head(), extras(), xdata(), vec(entry(data), 0..3)).prop_map(
            |(head, extras, xdata, entries)| Table {
                head,
                extras,
                xdata,
                entries,
            },
        ),
    )
}

fn tables() -> impl Strategy<Value = arc_io_dxf::model::Tables> {
    let layer = (
        any::<i16>(),
        of(any::<i32>()),
        text(),
        of(any::<bool>()),
        of(any::<i16>()),
        of(handle()),
    )
        .prop_map(
            |(color, true_color, linetype, plot, lineweight, plot_style)| Layer {
                color,
                true_color,
                linetype,
                plot,
                lineweight,
                plot_style,
                material: None,
            },
        );
    let linetype = (
        text(),
        any::<i16>(),
        num(),
        vec(
            (
                num(),
                any::<i16>(),
                of(any::<i16>()),
                of(handle()),
                of(num()),
                of(text()),
            )
                .prop_map(|(length, kind, shape, style, scale, text)| {
                    LinetypeElement {
                        length,
                        kind,
                        shape,
                        style,
                        scale,
                        rotation: None,
                        offset_x: Some(0.5),
                        offset_y: None,
                        text,
                    }
                }),
            0..3,
        ),
    )
        .prop_map(
            |(description, alignment, pattern_length, elements)| Linetype {
                description,
                alignment,
                pattern_length,
                elements,
            },
        );
    let style =
        (num(), num(), text(), of(text())).prop_map(|(height, w, font, big_font)| TextStyle {
            height,
            width_factor: w,
            font,
            big_font,
            ..TextStyle::default()
        });
    let view =
        (num(), p2(), p3(), of(i8v())).prop_map(|(height, center, direction, render_mode)| View {
            height,
            center,
            direction,
            render_mode,
            ..View::default()
        });
    let vport = (p2(), p2(), p3(), num(), of(any::<i16>())).prop_map(
        |(lower_left, upper_right, target, aspect, grid)| VPort {
            lower_left,
            upper_right,
            target,
            aspect,
            grid,
            ..VPort::default()
        },
    );
    let ucs =
        (p3(), p3(), of(num()), of(handle())).prop_map(|(origin, x_axis, elevation, base_ucs)| {
            Ucs {
                origin,
                x_axis,
                elevation,
                base_ucs,
                ..Ucs::default()
            }
        });
    let block_record = (of(handle()), of(i8v()), of(i8v()), vec(any::<u8>(), 0..300)).prop_map(
        |(layout, explodable, scalable, preview)| BlockRecord {
            layout,
            explodable,
            scalable,
            preview,
        },
    );
    let preserved = vec(
        (
            vec(group_of(ANY), 0..3),
            vec(vec(group_of(ANY), 0..3), 0..3),
        )
            .prop_map(|(groups, entries)| PreservedTable {
                name: "ARC_TEST_TABLE".to_owned(),
                groups,
                entries: entries
                    .into_iter()
                    .map(|groups| PreservedRecord {
                        kind: "ARC_TEST_TABLE".to_owned(),
                        groups,
                    })
                    .collect(),
            }),
        0..2,
    );
    (
        (
            table(vport),
            table(linetype),
            table(layer),
            table(style),
            table(view),
        ),
        (
            table(ucs),
            table(Just(AppId::default())),
            table(Just(DimStyle::default())),
            table(block_record),
            preserved,
        ),
    )
        .prop_map(|(a, b)| arc_io_dxf::model::Tables {
            vports: a.0,
            linetypes: a.1,
            layers: a.2,
            styles: a.3,
            views: a.4,
            ucs: b.0,
            appids: b.1,
            dimstyles: b.2,
            block_records: b.3,
            preserved: b.4,
        })
}

fn object() -> impl Strategy<Value = Object> {
    let opaque = |codes: &'static [i16]| vec(group_of(codes), 0..4);
    let kind = prop_oneof![
        (
            of(i8v()),
            of(i8v()),
            vec((text(), handle(), any::<bool>()), 0..3)
        )
            .prop_map(|(h, c, e)| {
                ObjectKind::Dictionary(Dictionary {
                    hard_owner: h,
                    cloning: c,
                    entries: e
                        .into_iter()
                        .map(|(name, handle, hard_owner)| DictionaryEntry {
                            name,
                            handle,
                            hard_owner,
                        })
                        .collect(),
                })
            }),
        (i8v(), opaque(ANY)).prop_map(|(c, data)| ObjectKind::XRecord(XRecord {
            cloning: Some(c),
            data,
        })),
        (
            opaque(ANY),
            text(),
            any::<i16>(),
            any::<i16>(),
            of(handle()),
            of(handle())
        )
            .prop_map(
                |(plot_settings, name, flags, tab_order, block_record, viewport)| {
                    ObjectKind::Layout(Layout {
                        plot_settings,
                        name,
                        flags,
                        tab_order,
                        block_record,
                        viewport,
                    })
                }
            ),
        (text(), any::<i16>(), any::<i16>(), vec(handle(), 0..3)).prop_map(|(d, u, s, e)| {
            ObjectKind::Group(GroupObject {
                description: d,
                unnamed: u,
                selectable: s,
                entities: e,
            })
        }),
    ];
    let typed = (head(), kind, extras(), xdata()).prop_map(|(head, kind, extras, xdata)| Object {
        // Every group after the marker of an extension record is its data.
        extras: if matches!(kind, ObjectKind::XRecord(_)) {
            Vec::new()
        } else {
            extras
        },
        head,
        kind,
        xdata,
    });
    let preserved = vec(group_of(ANY), 0..4).prop_map(|groups| {
        Object::new(ObjectKind::Preserved(PreservedRecord {
            kind: "ARC_TEST_OBJECT".to_owned(),
            groups,
        }))
    });
    prop_oneof![4 => typed, 1 => preserved]
}

fn class() -> impl Strategy<Value = Class> {
    (
        text(),
        text(),
        text(),
        any::<i32>(),
        of(any::<i32>()),
        i8v(),
        i8v(),
        extras(),
    )
        .prop_map(
            |(
                dxf_name,
                class_name,
                application,
                proxy_flags,
                instance_count,
                was_proxy,
                is_entity,
                extras,
            )| {
                Class {
                    dxf_name,
                    class_name,
                    application,
                    proxy_flags,
                    instance_count,
                    was_proxy,
                    is_entity,
                    extras,
                }
            },
        )
}

fn block(version: Version) -> impl Strategy<Value = Block> {
    (
        (
            text(),
            any::<i16>(),
            p3(),
            of(text()),
            of(text()),
            of(text()),
        ),
        wrap(Just(EntityKind::SeqEnd(SeqEnd::default()))),
        vec(entity(version), 0..3),
        wrap(Just(EntityKind::BlockEnd(SeqEnd::default()))),
    )
        .prop_map(|(b, begin, entities, end)| Block {
            begin: Entity {
                kind: EntityKind::Block(BlockBegin {
                    name: b.0,
                    flags: b.1,
                    base: b.2,
                    alt_name: b.3,
                    xref_path: b.4,
                    description: b.5,
                }),
                ..begin
            },
            entities,
            end,
        })
}

/// A drawing for `version`, with every handle unique.
fn drawing(version: Version) -> impl Strategy<Value = Drawing> {
    let header = (
        prop::sample::select(CODE_PAGES),
        vec((text(), vec(group_of(ANY), 0..3)), 0..3),
    )
        .prop_map(|(page, vars)| {
            let mut header = vec![HeaderVariable {
                name: "$DWGCODEPAGE".to_owned(),
                values: vec![Group::text(3, page)],
            }];
            header.extend(vars.into_iter().map(|(name, values)| HeaderVariable {
                name: format!("$ARC{name}"),
                values,
            }));
            header
        });
    let sections = vec(
        (
            vec(group_of(ANY), 0..3),
            vec(vec(group_of(ANY), 0..3), 0..3),
        )
            .prop_map(|(leading, records)| PreservedSection {
                name: "ARC_TEST_SECTION".to_owned(),
                leading,
                records: records
                    .into_iter()
                    .map(|groups| PreservedRecord {
                        kind: "ITEM".to_owned(),
                        groups,
                    })
                    .collect(),
            }),
        0..2,
    );
    (
        header,
        vec(class(), 0..2),
        tables(),
        vec(block(version), 0..2),
        vec(entity(version), 0..8),
        vec(object(), 0..4),
        sections,
    )
        .prop_map(
            move |(header, classes, tables, blocks, entities, objects, preserved_sections)| {
                let mut d = Drawing {
                    version,
                    header,
                    classes,
                    tables,
                    blocks,
                    entities,
                    objects,
                    preserved_sections,
                };
                number_handles(&mut d);
                d
            },
        )
}

/// Gives every record a unique handle, as files written by the crate have.
fn number_handles(d: &mut Drawing) {
    let mut next = 0x4000u64;
    let mut take = || {
        next += 1;
        Some(Handle(next))
    };
    fn entity(e: &mut Entity, take: &mut dyn FnMut() -> Option<Handle>) {
        match &mut e.kind {
            EntityKind::Preserved(r) => {
                let h = take().unwrap();
                r.groups.retain(|g| g.code != 5);
                r.groups.insert(0, Group::text(5, h.to_hex()));
                return;
            }
            EntityKind::Polyline(p) => {
                p.vertices.iter_mut().for_each(|v| entity(v, take));
                p.seqend.iter_mut().for_each(|s| entity(s, take));
            }
            EntityKind::Insert(i) => {
                i.attribs.iter_mut().for_each(|v| entity(v, take));
                i.seqend.iter_mut().for_each(|s| entity(s, take));
            }
            _ => {}
        }
        e.head.handle = take();
    }
    macro_rules! table {
        ($slot:expr) => {
            if let Some(t) = &mut $slot {
                t.head.handle = take();
                for e in &mut t.entries {
                    e.head.handle = take();
                }
            }
        };
    }
    table!(d.tables.vports);
    table!(d.tables.linetypes);
    table!(d.tables.layers);
    table!(d.tables.styles);
    table!(d.tables.views);
    table!(d.tables.ucs);
    table!(d.tables.appids);
    table!(d.tables.dimstyles);
    table!(d.tables.block_records);
    for t in &mut d.tables.preserved {
        for e in &mut t.entries {
            e.groups.retain(|g| g.code != 5);
        }
    }
    for b in &mut d.blocks {
        entity(&mut b.begin, &mut take);
        b.entities.iter_mut().for_each(|e| entity(e, &mut take));
        entity(&mut b.end, &mut take);
    }
    d.entities.iter_mut().for_each(|e| entity(e, &mut take));
    for o in &mut d.objects {
        match &mut o.kind {
            ObjectKind::Preserved(r) => r.groups.retain(|g| g.code != 5),
            _ => o.head.handle = take(),
        }
    }
}

fn any_drawing() -> impl Strategy<Value = (Version, Drawing)> {
    prop::sample::select(Version::WRITABLE.to_vec())
        .prop_flat_map(|version| drawing(version).prop_map(move |d| (version, d)))
}

/// Runs on a thread with a large stack: the generated strategy types are deep in debug builds.
#[test]
fn written_drawings_read_back_equal() {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| {
            let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
                failure_persistence: None,
                ..ProptestConfig::default()
            });
            runner
                .run(&(any_drawing(), any::<bool>()), |((version, d), binary)| {
                    let options = || WriteOptions {
                        version,
                        binary,
                        ..WriteOptions::default()
                    };
                    let out = write(&d, &mut options()).unwrap();
                    assert_report_valid(&out.report);
                    prop_assert!(
                        out.report
                            .items
                            .iter()
                            .all(|i| i.reason != "io.dxf.handle_assigned")
                    );
                    let back = read(
                        &out.bytes,
                        &mut ReadOptions::with_limits(arc_base::Limits::DEFAULT),
                    )
                    .unwrap();
                    assert_report_valid(&back.report);
                    prop_assert_eq!(&back.drawing, &d);
                    // Determinism: the same drawing and options give the same bytes.
                    prop_assert_eq!(write(&d, &mut options()).unwrap().bytes, out.bytes);
                    Ok(())
                })
                .unwrap();
        })
        .unwrap()
        .join()
        .unwrap();
}
