//! The display lists of dimensions, leaders and multi-leaders: their geometry for every
//! rendition, mapped to primitives, with text laid out by [`crate::text_layout`].

use arc_base::math::{Matrix3, Point2, Transform2, Vector3};
use arc_base::{Limits, ObjectId, Result};
use arc_doc::Value;

use crate::annotation::{
    Arrow, Drawn, Part, Rendition, TextPlacement, renditions, representations,
};
use crate::appearance::{Source, Sourced};
use crate::dimension::{DIMENSION, Definition, Kind, Sizes, ToleranceText, geometry, text_runs};
use crate::display::{
    DisplayOptions, Look, Primitive, Reader, Shape, lay_out, read_enum, read_flag, read_number,
    reference_list, text_primitives, text_style_of,
};
use crate::format::{NumberFormat, UnitFormat, format_angle, format_length};
use crate::leader::{Attachment, LEADER, frame_geometry, leader_geometry, multileader_geometry};
use crate::text::{Paragraph, Run, paragraphs_from_plain};
use crate::text_layout::{Columns, HAlign, TextInput, TextLayout, TextStyle, VAlign};
use crate::types::{SpaceUnit, entity};

/// The primitives of the dimension, leader or multi-leader `id` of type `key`.
pub(crate) fn primitives(
    reader: &dyn Reader,
    id: ObjectId,
    key: &str,
    base: &Primitive,
    look: &Look,
    options: &DisplayOptions,
) -> Result<Vec<Primitive>> {
    let annotative = read_flag(reader, id, "annotative")?;
    let scales = representations(reader.field(id, "scales")?.as_ref());
    let mut out = Vec::new();
    match key {
        DIMENSION => dimension(reader, id, base, annotative, &scales, options, &mut out)?,
        LEADER => leader(reader, id, base, annotative, &scales, options, &mut out)?,
        _ => multileader(
            reader, id, base, look, annotative, &scales, options, &mut out,
        )?,
    }
    Ok(out)
}

fn reference(reader: &dyn Reader, id: ObjectId, name: &str) -> Result<Option<ObjectId>> {
    Ok(match reader.field(id, name)? {
        Some(Value::Reference(r)) => Some(r),
        _ => None,
    })
}

fn point(reader: &dyn Reader, id: ObjectId, name: &str) -> Result<Option<Point2>> {
    Ok(match reader.field(id, name)? {
        Some(Value::Point2(p)) => Some(p),
        _ => None,
    })
}

fn number(reader: &dyn Reader, id: Option<ObjectId>, name: &str, default: f64) -> Result<f64> {
    Ok(match id {
        Some(id) => read_number(reader, id, name)?.unwrap_or(default),
        None => default,
    })
}

fn integer(reader: &dyn Reader, id: Option<ObjectId>, name: &str, default: i64) -> Result<i64> {
    Ok(match id {
        Some(id) => match reader.field(id, name)? {
            Some(Value::I64(n)) => n,
            _ => default,
        },
        None => default,
    })
}

fn code(reader: &dyn Reader, id: Option<ObjectId>, name: &str) -> Result<u32> {
    Ok(match id {
        Some(id) => read_enum(reader, id, name)?,
        None => 0,
    })
}

fn boolean(reader: &dyn Reader, id: Option<ObjectId>, name: &str, default: bool) -> Result<bool> {
    Ok(match id {
        Some(id) => match reader.field(id, name)? {
            Some(Value::Bool(b)) => b,
            _ => default,
        },
        None => default,
    })
}

/// Maps drawn geometry to primitives of `scale`.
fn emit(drawn: &Drawn, base: &Primitive, scale: Option<f64>, out: &mut Vec<Primitive>) {
    let template = Primitive {
        linetype: Some(Sourced {
            value: None,
            source: Source::Object,
        }),
        annotation_scale: scale,
        ..base.clone()
    };
    for (part, curve) in &drawn.strokes {
        out.push(Primitive {
            shape: Shape::Stroke {
                curve: curve.clone(),
                drawn: None,
            },
            part: *part as u32,
            ..template.clone()
        });
    }
    for (part, region) in &drawn.fills {
        out.push(Primitive {
            shape: Shape::Fill(region.clone()),
            part: *part as u32,
            ..template.clone()
        });
    }
}

/// Lays out `paragraphs` at `height` anchored by `halign` and `valign`.
fn layout(
    reader: &dyn Reader,
    paragraphs: &[Paragraph],
    height: f64,
    halign: HAlign,
    valign: VAlign,
    style: &TextStyle,
    options: &DisplayOptions,
) -> Result<TextLayout> {
    lay_out(
        reader,
        &TextInput {
            paragraphs,
            height,
            width: 0.0,
            halign,
            valign,
            columns: Columns::default(),
            style,
        },
        options,
    )
}

fn width(layout: &TextLayout) -> f64 {
    layout.bounds.map_or(0.0, |(a, b)| b.x - a.x)
}

/// Adds the primitives of a laid-out text placed at `placement` (the layout's origin goes to
/// the center, its baseline turns to the rotation).
fn place_text(
    layout: &TextLayout,
    placement: TextPlacement,
    base: &Primitive,
    scale: Option<f64>,
    options: &DisplayOptions,
    out: &mut Vec<Primitive>,
) -> Result<()> {
    let (sin, cos) = placement.rotation.sin_cos();
    let c = placement.center;
    let map = Transform2::from_matrix_unchecked(Matrix3::new(
        cos, -sin, c.x, sin, cos, c.y, 0.0, 0.0, 1.0,
    ));
    let template = Primitive {
        part: Part::Text as u32,
        annotation_scale: scale,
        ..base.clone()
    };
    out.extend(text_primitives(
        layout,
        &map,
        &template,
        &options.tolerance,
    )?);
    Ok(())
}

/// The values of a dimension style that apply to a kind.
struct DimStyle {
    text_style: TextStyle,
    text_height: f64,
    gap: f64,
    arrow: Arrow,
    arrow_size: f64,
    offset: f64,
    beyond: f64,
    above: bool,
    number: NumberFormat,
    angular_precision: u32,
    measurement_scale: f64,
    overall_scale: f64,
    tolerance: u32,
    upper: f64,
    lower: f64,
}

fn dim_style(reader: &dyn Reader, style: Option<ObjectId>, kind: Kind) -> Result<DimStyle> {
    // The child of the referenced style for the dimension's kind, when there is one.
    let mut s = style;
    if let Some(parent) = style {
        for child in reader.children(parent) {
            if read_enum(reader, child, "kind")? == kind.style_kind() {
                s = Some(child);
                break;
            }
        }
    }
    let separator = match s {
        Some(id) => match reader.field(id, "decimal_separator")? {
            Some(Value::String(t)) => t.chars().next().unwrap_or('.'),
            _ => '.',
        },
        None => '.',
    };
    let precision = |name: &str, default: i64| -> Result<u32> {
        Ok(u32::try_from(integer(reader, s, name, default)?.clamp(0, 8)).unwrap_or(2))
    };
    Ok(DimStyle {
        text_style: text_style_of(
            reader,
            s.map(|id| reference(reader, id, "text_style"))
                .transpose()?
                .flatten(),
        )?,
        text_height: number(reader, s, "text_height", 2.5)?,
        gap: number(reader, s, "text_gap", 0.625)?,
        arrow: Arrow::of(code(reader, s, "arrow")?),
        arrow_size: number(reader, s, "arrow_size", 2.5)?,
        offset: number(reader, s, "extension_offset", 0.625)?,
        beyond: number(reader, s, "extension_beyond", 1.25)?,
        above: code(reader, s, "text_vertical")? == 1,
        number: NumberFormat {
            unit_format: UnitFormat::of(code(reader, s, "unit_format")?),
            precision: precision("precision", 2)?,
            rounding: number(reader, s, "rounding", 0.0)?,
            suppress_leading: boolean(reader, s, "suppress_leading_zeros", false)?,
            suppress_trailing: boolean(reader, s, "suppress_trailing_zeros", false)?,
            separator,
            suffix: boolean(reader, s, "unit_suffix", false)?.then(String::new),
        },
        angular_precision: precision("angular_precision", 0)?,
        measurement_scale: number(reader, s, "measurement_scale", 1.0)?,
        overall_scale: number(reader, s, "overall_scale", 1.0)?,
        tolerance: code(reader, s, "tolerance")?,
        upper: number(reader, s, "tolerance_upper", 0.0)?,
        lower: number(reader, s, "tolerance_lower", 0.0)?,
    })
}

/// The unit of the container of `id`.
fn unit_of(reader: &dyn Reader, id: ObjectId) -> Result<SpaceUnit> {
    Ok(match reader.owner(id)? {
        Some(owner) => match reader.field(owner, "unit")? {
            Some(Value::Enum(c)) => SpaceUnit::of(c).unwrap_or(SpaceUnit::Unitless),
            _ => SpaceUnit::Unitless,
        },
        None => SpaceUnit::Unitless,
    })
}

/// The runs of the text of a dimension with the measured value `measurement`.
fn dimension_runs(
    style: &DimStyle,
    kind: Kind,
    measurement: f64,
    unit: SpaceUnit,
    template: Option<&str>,
) -> Vec<Run> {
    let mut number = style.number.clone();
    let write = |value: f64, number: &NumberFormat| {
        if kind.is_angle() {
            let angle = NumberFormat {
                precision: style.angular_precision,
                ..number.clone()
            };
            format_angle(value, &angle)
        } else {
            format_length(value, number, unit.factor_to(SpaceUnit::Inch))
        }
    };
    let value = if kind.is_angle() {
        measurement
    } else {
        measurement * style.measurement_scale
    };
    let bare = NumberFormat {
        suffix: None,
        ..number.clone()
    };
    if number.suffix.is_some() {
        number.suffix = unit.unit().map(|u| u.symbol.to_owned());
    }
    // Angular tolerances are in degrees.
    let delta = |d: f64| if kind.is_angle() { d.to_radians() } else { d };
    let tolerance = match style.tolerance {
        1 => ToleranceText::Symmetric(write(delta(style.upper), &bare)),
        2 => ToleranceText::Deviation(
            write(delta(style.upper), &bare),
            write(delta(style.lower), &bare),
        ),
        3 => ToleranceText::Limits(
            write(value + delta(style.upper), &number),
            write(value - delta(style.lower), &number),
        ),
        _ => ToleranceText::None,
    };
    text_runs(&write(value, &number), template, &tolerance)
}

fn dimension(
    reader: &dyn Reader,
    id: ObjectId,
    base: &Primitive,
    annotative: bool,
    scales: &[crate::annotation::Representation],
    options: &DisplayOptions,
    out: &mut Vec<Primitive>,
) -> Result<()> {
    let resolution = reader.resolved(id)?;
    let kind = Kind::of(read_enum(reader, id, "kind")?)
        .ok_or_else(|| arc_base::Error::new(crate::error::codes::DIMENSION_POINTS_INVALID))?;
    let style = dim_style(reader, reference(reader, id, "style")?, kind)?;
    let template = match reader.field(id, "text_override")? {
        Some(Value::String(t)) => Some(t),
        _ => None,
    };
    let runs = dimension_runs(
        &style,
        kind,
        resolution.measurement,
        unit_of(reader, id)?,
        template.as_deref(),
    );
    let paragraphs = [Paragraph {
        runs,
        ..Paragraph::default()
    }];
    let line_position = point(reader, id, "line_position")?.unwrap_or_else(Point2::origin);
    let text_position = point(reader, id, "text_position")?;
    let rotation = read_number(reader, id, "rotation")?.unwrap_or(0.0);
    for r in renditions(annotative, scales, style.overall_scale) {
        let Rendition { scale, factor, .. } = r;
        let sizes = Sizes {
            text_height: style.text_height * factor,
            gap: style.gap * factor,
            arrow: style.arrow,
            arrow_size: style.arrow_size * factor,
            extension_offset: style.offset * factor,
            extension_beyond: style.beyond * factor,
            above: style.above,
        };
        let text = layout(
            reader,
            &paragraphs,
            sizes.text_height,
            HAlign::Center,
            VAlign::Middle,
            &style.text_style,
            options,
        )?;
        let def = Definition {
            kind,
            points: resolution.points.clone(),
            line_position: r.position.unwrap_or(line_position),
            text_position: r.text_position.or(text_position),
            rotation,
        };
        let drawn = geometry(&def, &sizes, width(&text), &options.tolerance)?;
        emit(&drawn, base, scale, out);
        if let Some(placement) = drawn.text {
            place_text(&text, placement, base, scale, options, out)?;
        }
    }
    Ok(())
}

/// The values of a leader style.
struct LeaderStyle {
    text_style: TextStyle,
    text_height: f64,
    arrow: Arrow,
    arrow_size: f64,
    landing: f64,
    gap: f64,
}

/// The style values of the leader or multi-leader `id`; a leader's own `arrow` wins.
fn leader_style(reader: &dyn Reader, id: ObjectId, own_arrow: bool) -> Result<LeaderStyle> {
    let s = reference(reader, id, "style")?;
    let own = if own_arrow {
        reader.field(id, "arrow")?
    } else {
        None
    };
    let arrow = match own {
        Some(Value::Enum(c)) => Arrow::of(c),
        _ => Arrow::of(code(reader, s, "arrow")?),
    };
    let landing = if boolean(reader, s, "landing", true)? {
        number(reader, s, "landing_length", 2.5)?
    } else {
        0.0
    };
    Ok(LeaderStyle {
        text_style: text_style_of(
            reader,
            s.map(|st| reference(reader, st, "text_style"))
                .transpose()?
                .flatten(),
        )?,
        text_height: number(reader, s, "text_height", 2.5)?,
        arrow,
        arrow_size: number(reader, s, "arrow_size", 2.5)?,
        landing,
        gap: number(reader, s, "text_gap", 0.625)?,
    })
}

fn points(value: Option<&Value>) -> Vec<Point2> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|v| match v {
                Value::Point2(p) => Some(*p),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn leader(
    reader: &dyn Reader,
    id: ObjectId,
    base: &Primitive,
    annotative: bool,
    scales: &[crate::annotation::Representation],
    _options: &DisplayOptions,
    out: &mut Vec<Primitive>,
) -> Result<()> {
    let style = leader_style(reader, id, true)?;
    let points = points(reader.field(id, "points")?.as_ref());
    for r in renditions(annotative, scales, 1.0) {
        let drawn = leader_geometry(
            &points,
            style.arrow,
            style.arrow_size * r.factor,
            style.landing * r.factor,
        )?;
        emit(&drawn, base, r.scale, out);
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one entity list with three content kinds"
)]
fn multileader(
    reader: &dyn Reader,
    id: ObjectId,
    base: &Primitive,
    look: &Look,
    annotative: bool,
    scales: &[crate::annotation::Representation],
    options: &DisplayOptions,
    out: &mut Vec<Primitive>,
) -> Result<()> {
    let style = leader_style(reader, id, false)?;
    let leaders: Vec<Vec<Point2>> = match reader.field(id, "leaders")? {
        Some(Value::List(items)) => items.iter().map(|i| points(Some(i))).collect(),
        _ => Vec::new(),
    };
    let position = point(reader, id, "position")?.unwrap_or_else(Point2::origin);
    let direction = if read_enum(reader, id, "direction")? == 1 {
        -1.0
    } else {
        1.0
    };
    let attachment = Attachment::of(read_enum(reader, id, "attachment")?);
    let valign = match attachment {
        Attachment::Top => VAlign::Top,
        Attachment::Middle => VAlign::Middle,
        Attachment::Bottom => VAlign::Bottom,
    };
    let content = read_enum(reader, id, "content")?;
    for r in renditions(annotative, scales, 1.0) {
        let f = r.factor;
        let height = style.text_height * f;
        let geo = multileader_geometry(
            &leaders,
            r.position.unwrap_or(position),
            direction,
            style.landing * f,
            style.gap * f,
            style.arrow,
            style.arrow_size * f,
        )?;
        emit(&geo.drawn, base, r.scale, out);
        let at = |center: Point2| TextPlacement {
            center,
            rotation: 0.0,
        };
        match content {
            0 => {
                let text = match reader.field(id, "text")? {
                    Some(Value::String(t)) => t,
                    _ => String::new(),
                };
                let paragraphs = paragraphs_from_plain(&text, &Limits::DEFAULT)?;
                let halign = if direction < 0.0 {
                    HAlign::Right
                } else {
                    HAlign::Left
                };
                let laid = layout(
                    reader,
                    &paragraphs,
                    height,
                    halign,
                    valign,
                    &style.text_style,
                    options,
                )?;
                place_text(&laid, at(geo.anchor), base, r.scale, options, out)?;
            }
            1 => {
                let Some(block) = reference(reader, id, "block")? else {
                    continue;
                };
                let s = number(reader, Some(id), "block_scale", 1.0)? * f;
                // The fields of a block reference at the content anchor.
                let mut values = vec![None; entity::KIND];
                values.extend([
                    Some(Value::Reference(block)),
                    Some(Value::Point2(geo.anchor)),
                    Some(Value::Vector3(Vector3::new(s, s, 1.0))),
                    Some(Value::F64(0.0)),
                ]);
                let template = Primitive {
                    part: Part::Content as u32,
                    annotation_scale: r.scale,
                    ..base.clone()
                };
                let list = reference_list(reader, &values, template, look, false, options)?;
                out.extend(list.primitives.into_iter().map(|mut p| {
                    p.annotation_scale = r.scale;
                    p
                }));
            }
            2 => {
                let rows: Vec<Vec<String>> = match reader.field(id, "tolerance")? {
                    Some(Value::List(items)) => items
                        .iter()
                        .map(|row| match row {
                            Value::List(cells) => cells
                                .iter()
                                .map(|c| match c {
                                    Value::String(t) => t.clone(),
                                    _ => String::new(),
                                })
                                .collect(),
                            _ => Vec::new(),
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                let mut laid = Vec::new();
                for row in &rows {
                    let mut cells = Vec::new();
                    for cell in row {
                        let paragraphs = [Paragraph {
                            runs: vec![Run {
                                text: cell.clone(),
                                ..Run::default()
                            }],
                            ..Paragraph::default()
                        }];
                        cells.push(layout(
                            reader,
                            &paragraphs,
                            height,
                            HAlign::Center,
                            VAlign::Middle,
                            &style.text_style,
                            options,
                        )?);
                    }
                    laid.push(cells);
                }
                let widths: Vec<Vec<f64>> = laid
                    .iter()
                    .map(|row| row.iter().map(width).collect())
                    .collect();
                let (boxes, centers) = frame_geometry(
                    &widths,
                    height,
                    style.gap * f,
                    geo.anchor,
                    direction,
                    attachment,
                )?;
                let frame = Drawn {
                    strokes: boxes
                        .into_iter()
                        .map(|b| (Part::Frame, arc_geom::Curve2::Polyline(b)))
                        .collect(),
                    ..Drawn::default()
                };
                emit(&frame, base, r.scale, out);
                for (text, center) in laid.iter().flatten().zip(centers) {
                    place_text(text, at(center), base, r.scale, options, out)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
