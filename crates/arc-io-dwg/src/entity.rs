//! Mapping of library entities to the neutral entity kinds of `arc-io-dxf`.
//!
//! Each entity yields an [`Outcome`]: a mapped entity with the names of the data the model does
//! not carry, an unsupported type, or an entity whose numbers the model does not accept.

use acadrust::entities::{self as dwg, EntityType};
use arc_base::{Limits, Result};
use arc_io_dxf::model::{self as m, Entity, EntityKind, EntityProps};

use acadrust::types::{Vector2 as DwgVector2, Vector3 as DwgVector3};

use crate::convert::{self, all_finite, finite2, finite3, p2, p3, xy};
use crate::error::limit;

/// The result of mapping one library entity.
pub(crate) enum Outcome {
    /// The entity, and the names of the data that is not carried.
    Mapped {
        entity: Box<Entity>,
        partial: Vec<&'static str>,
    },
    /// The entity type is not mapped.
    Unsupported,
    /// The entity holds a number the model does not accept.
    Invalid,
}

/// The name of an entity type, as recorded in loss items.
pub(crate) fn type_name(entity: &EntityType) -> &'static str {
    match entity {
        EntityType::Point(_) => "POINT",
        EntityType::Line(_) => "LINE",
        EntityType::Circle(_) => "CIRCLE",
        EntityType::Arc(_) => "ARC",
        EntityType::Ellipse(_) => "ELLIPSE",
        EntityType::Polyline(_) => "POLYLINE",
        EntityType::Polyline2D(_) => "POLYLINE2D",
        EntityType::Polyline3D(_) => "POLYLINE3D",
        EntityType::LwPolyline(_) => "LWPOLYLINE",
        EntityType::Text(_) => "TEXT",
        EntityType::MText(_) => "MTEXT",
        EntityType::Spline(_) => "SPLINE",
        EntityType::Helix(_) => "HELIX",
        EntityType::Dimension(_) => "DIMENSION",
        EntityType::Hatch(_) => "HATCH",
        EntityType::Solid(_) => "SOLID",
        EntityType::Face3D(_) => "3DFACE",
        EntityType::Insert(_) => "INSERT",
        EntityType::Block(_) => "BLOCK",
        EntityType::BlockEnd(_) => "ENDBLK",
        EntityType::Ray(_) => "RAY",
        EntityType::XLine(_) => "XLINE",
        EntityType::Viewport(_) => "VIEWPORT",
        EntityType::AttributeDefinition(_) => "ATTDEF",
        EntityType::AttributeEntity(_) => "ATTRIB",
        EntityType::Leader(_) => "LEADER",
        EntityType::MultiLeader(_) => "MULTILEADER",
        EntityType::MLine(_) => "MLINE",
        EntityType::Mesh(_) => "MESH",
        EntityType::RasterImage(_) => "IMAGE",
        EntityType::Solid3D(_) => "3DSOLID",
        EntityType::Region(_) => "REGION",
        EntityType::Body(_) => "BODY",
        EntityType::Surface(_) => "SURFACE",
        EntityType::Table(_) => "ACAD_TABLE",
        EntityType::Tolerance(_) => "TOLERANCE",
        EntityType::PolyfaceMesh(_) => "POLYFACE_MESH",
        EntityType::Wipeout(_) => "WIPEOUT",
        EntityType::Shape(_) => "SHAPE",
        EntityType::Underlay(_) => "UNDERLAY",
        EntityType::Seqend(_) => "SEQEND",
        EntityType::Ole2Frame(_) => "OLE2FRAME",
        EntityType::PolygonMesh(_) => "POLYGON_MESH",
        EntityType::Light(_) => "LIGHT",
        EntityType::SectionSymbol(_) => "SECTION",
        EntityType::ViewBorder(_) => "VIEWBORDER",
        EntityType::Extended(_) => "EXTENDED",
        EntityType::Unknown(_) => "UNKNOWN",
    }
}

fn array(len: usize, limits: &Limits) -> Result<()> {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    if len > limits.array_length {
        return Err(limit("array_length", len));
    }
    Ok(())
}

pub(crate) fn text(value: &str, limits: &Limits) -> Result<String> {
    let len = u64::try_from(value.len()).unwrap_or(u64::MAX);
    if len > limits.string_bytes {
        return Err(limit("string_bytes", len));
    }
    Ok(value.to_owned())
}

/// Sets the thickness, elevation and extrusion of an entity lying in a plane. Returns false when a
/// number is not finite.
fn plane(props: &mut EntityProps, thickness: f64, elevation: f64, normal: DwgVector3) -> bool {
    if !(thickness.is_finite() && elevation.is_finite() && finite3(normal)) {
        return false;
    }
    props.thickness = (thickness != 0.0).then_some(thickness);
    props.elevation = (elevation != 0.0).then_some(elevation);
    props.extrusion = Some(p3(normal));
    true
}

/// Maps one entity of the library. Blocks and end markers are never passed here.
pub(crate) fn map(entity: &EntityType, paper_space: bool, limits: &Limits) -> Result<Outcome> {
    let common = entity.common();
    let (mut props, mut partial) = convert::props(common, paper_space);
    if !props.linetype_scale.is_none_or(f64::is_finite) {
        return Ok(Outcome::Invalid);
    }
    let kind = match entity {
        EntityType::Point(e) => point(e, &mut props),
        EntityType::Line(e) => line(e, &mut props),
        EntityType::Circle(e) => circle(e, &mut props),
        EntityType::Arc(e) => arc(e, &mut props),
        EntityType::Ellipse(e) => ellipse(e, &mut props),
        EntityType::Ray(e) => ray(e.base_point, e.direction).map(EntityKind::Ray),
        EntityType::XLine(e) => ray(e.base_point, e.direction).map(EntityKind::ConstructionLine),
        EntityType::LwPolyline(e) => lwpolyline(e, limits, &mut props)?,
        EntityType::Polyline2D(e) => polyline2d(e, limits, &mut props, &mut partial)?,
        EntityType::Spline(e) => spline(e, limits, &mut props, &mut partial)?,
        EntityType::Text(e) => text_entity(e, limits, &mut props)?,
        EntityType::MText(e) => mtext(e, limits, &mut props, &mut partial)?,
        EntityType::Insert(e) => insert(e, limits, &mut props, &mut partial)?,
        EntityType::Dimension(e) => dimension(e, &mut props, &mut partial),
        EntityType::Hatch(e) => hatch(e, limits, &mut props, &mut partial)?,
        _ => return Ok(Outcome::Unsupported),
    };
    let Some(kind) = kind else {
        return Ok(Outcome::Invalid);
    };
    let entity = Entity {
        head: convert::head(common.handle, common.owner_handle),
        props,
        kind,
        extras: Vec::new(),
        xdata: Vec::new(),
    };
    Ok(Outcome::Mapped {
        entity: Box::new(entity),
        partial,
    })
}

fn point(e: &dwg::Point, props: &mut EntityProps) -> Option<EntityKind> {
    if !plane(props, e.thickness, 0.0, e.normal)
        || !finite3(e.location)
        || !e.x_axis_angle.is_finite()
    {
        return None;
    }
    Some(EntityKind::Point(m::Point {
        location: p3(e.location),
        angle: (e.x_axis_angle != 0.0).then(|| e.x_axis_angle.to_degrees()),
    }))
}

fn line(e: &dwg::Line, props: &mut EntityProps) -> Option<EntityKind> {
    if !plane(props, e.thickness, 0.0, e.normal) || !finite3(e.start) || !finite3(e.end) {
        return None;
    }
    Some(EntityKind::Line(m::Line {
        start: p3(e.start),
        end: p3(e.end),
    }))
}

fn circle(e: &dwg::Circle, props: &mut EntityProps) -> Option<EntityKind> {
    if !plane(props, e.thickness, 0.0, e.normal) || !finite3(e.center) || !e.radius.is_finite() {
        return None;
    }
    Some(EntityKind::Circle(m::Circle {
        center: p3(e.center),
        radius: e.radius,
    }))
}

fn arc(e: &dwg::Arc, props: &mut EntityProps) -> Option<EntityKind> {
    let angles = [e.start_angle, e.end_angle, e.radius];
    if !plane(props, e.thickness, 0.0, e.normal) || !finite3(e.center) || !all_finite(&angles) {
        return None;
    }
    // The library stores angles in radians; the model keeps the degrees of the file format.
    Some(EntityKind::Arc(m::Arc {
        center: p3(e.center),
        radius: e.radius,
        start_angle: e.start_angle.to_degrees(),
        end_angle: e.end_angle.to_degrees(),
    }))
}

fn ellipse(e: &dwg::Ellipse, props: &mut EntityProps) -> Option<EntityKind> {
    let values = [e.minor_axis_ratio, e.start_parameter, e.end_parameter];
    if !plane(props, 0.0, 0.0, e.normal)
        || !finite3(e.center)
        || !finite3(e.major_axis)
        || !all_finite(&values)
    {
        return None;
    }
    Some(EntityKind::Ellipse(m::Ellipse {
        center: p3(e.center),
        major_axis: p3(e.major_axis),
        ratio: e.minor_axis_ratio,
        start_param: e.start_parameter,
        end_param: e.end_parameter,
    }))
}

fn ray(base: DwgVector3, direction: DwgVector3) -> Option<m::Ray> {
    (finite3(base) && finite3(direction)).then(|| m::Ray {
        start: p3(base),
        direction: p3(direction),
    })
}

fn lwpolyline(
    e: &dwg::LwPolyline,
    limits: &Limits,
    props: &mut EntityProps,
) -> Result<Option<EntityKind>> {
    array(e.vertices.len(), limits)?;
    if !plane(props, e.thickness, e.elevation, e.normal) || !e.constant_width.is_finite() {
        return Ok(None);
    }
    let mut vertices = Vec::with_capacity(e.vertices.len());
    for v in &e.vertices {
        if !finite2(v.location)
            || !v.bulge.is_finite()
            || !v.start_width.is_finite()
            || !v.end_width.is_finite()
        {
            return Ok(None);
        }
        vertices.push(m::LwVertex {
            location: p2(v.location),
            id: (v.vertex_id != 0).then_some(v.vertex_id),
            start_width: (v.start_width != 0.0).then_some(v.start_width),
            end_width: (v.end_width != 0.0).then_some(v.end_width),
            bulge: (v.bulge != 0.0).then_some(v.bulge),
        });
    }
    let flags = i16::from(e.is_closed) | if e.plinegen { 128 } else { 0 };
    Ok(Some(EntityKind::LwPolyline(m::LwPolyline {
        flags,
        constant_width: (e.constant_width != 0.0).then_some(e.constant_width),
        vertices,
    })))
}

fn polyline2d(
    e: &dwg::Polyline2D,
    limits: &Limits,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    array(e.vertices.len(), limits)?;
    if !plane(props, e.thickness, e.elevation, e.normal)
        || !e.start_width.is_finite()
        || !e.end_width.is_finite()
    {
        return Ok(None);
    }
    if !matches!(e.smooth_surface, dwg::SmoothSurfaceType::None) {
        partial.push("smooth_surface");
    }
    let mut vertices = Vec::with_capacity(e.vertices.len());
    for v in &e.vertices {
        if !finite3(v.location)
            || !v.start_width.is_finite()
            || !v.end_width.is_finite()
            || !v.bulge.is_finite()
            || !v.curve_tangent.is_finite()
        {
            return Ok(None);
        }
        let vertex = m::Vertex {
            location: p3(v.location),
            start_width: (v.start_width != 0.0).then_some(v.start_width),
            end_width: (v.end_width != 0.0).then_some(v.end_width),
            bulge: (v.bulge != 0.0).then_some(v.bulge),
            flags: i16::from(v.flags.bits()),
            tangent: (v.curve_tangent != 0.0).then_some(v.curve_tangent),
            index1: None,
            index2: None,
            index3: None,
            index4: None,
            id: (v.id != 0).then_some(v.id),
        };
        vertices.push(Entity {
            head: m::Head::default(),
            props: EntityProps {
                layer: "0".to_owned(),
                ..EntityProps::default()
            },
            kind: EntityKind::Vertex(vertex),
            extras: Vec::new(),
            xdata: Vec::new(),
        });
    }
    Ok(Some(EntityKind::Polyline(m::Polyline {
        origin: None,
        flags: i16::from(e.flags.is_closed()),
        start_width: (e.start_width != 0.0).then_some(e.start_width),
        end_width: (e.end_width != 0.0).then_some(e.end_width),
        mesh_m: None,
        mesh_n: None,
        smooth_m: None,
        smooth_n: None,
        smooth_type: None,
        vertices,
        seqend: None,
    })))
}

fn spline(
    e: &dwg::Spline,
    limits: &Limits,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    array(e.knots.len(), limits)?;
    array(e.control_points.len(), limits)?;
    array(e.fit_points.len(), limits)?;
    array(e.weights.len(), limits)?;
    if !plane(props, 0.0, 0.0, e.normal) {
        return Ok(None);
    }
    let scalars = [e.knot_tolerance, e.control_tolerance, e.fit_tolerance];
    let degree = i16::try_from(e.degree).ok();
    let vectors_ok = e.control_points.iter().all(|v| finite3(*v))
        && e.fit_points.iter().all(|v| finite3(*v))
        && finite3(e.begin_tangent)
        && finite3(e.end_tangent);
    if !all_finite(&scalars) || !all_finite(&e.knots) || !all_finite(&e.weights) || !vectors_ok {
        return Ok(None);
    }
    let Some(degree) = degree else {
        return Ok(None);
    };
    if e.knot_parameterization != 0 {
        partial.push("knot_parameterization");
    }
    let f = &e.flags;
    let flags = i16::from(f.closed)
        | (i16::from(f.periodic) << 1)
        | (i16::from(f.rational) << 2)
        | (i16::from(f.planar) << 3)
        | (i16::from(f.linear) << 4);
    let tangent = |v: DwgVector3| (v != DwgVector3::default()).then(|| p3(v));
    Ok(Some(EntityKind::Spline(m::Spline {
        flags,
        degree,
        knot_tolerance: (e.knot_tolerance != 0.0).then_some(e.knot_tolerance),
        control_tolerance: (e.control_tolerance != 0.0).then_some(e.control_tolerance),
        fit_tolerance: (e.fit_tolerance != 0.0).then_some(e.fit_tolerance),
        start_tangent: tangent(e.begin_tangent),
        end_tangent: tangent(e.end_tangent),
        knots: e.knots.clone(),
        weights: e.weights.clone(),
        control_points: e.control_points.iter().map(|v| p3(*v)).collect(),
        fit_points: e.fit_points.iter().map(|v| p3(*v)).collect(),
    })))
}

fn h_code(value: dwg::TextHorizontalAlignment) -> i16 {
    match value {
        dwg::TextHorizontalAlignment::Left => 0,
        dwg::TextHorizontalAlignment::Center => 1,
        dwg::TextHorizontalAlignment::Right => 2,
        dwg::TextHorizontalAlignment::Aligned => 3,
        dwg::TextHorizontalAlignment::Middle => 4,
        dwg::TextHorizontalAlignment::Fit => 5,
    }
}

fn v_code(value: dwg::TextVerticalAlignment) -> i16 {
    match value {
        dwg::TextVerticalAlignment::Baseline => 0,
        dwg::TextVerticalAlignment::Bottom => 1,
        dwg::TextVerticalAlignment::Middle => 2,
        dwg::TextVerticalAlignment::Top => 3,
    }
}

fn text_entity(
    e: &dwg::Text,
    limits: &Limits,
    props: &mut EntityProps,
) -> Result<Option<EntityKind>> {
    let values = [e.height, e.rotation, e.width_factor, e.oblique_angle];
    if !plane(props, e.thickness, 0.0, e.normal)
        || !finite3(e.insertion_point)
        || !all_finite(&values)
    {
        return Ok(None);
    }
    let align = match e.alignment_point {
        Some(v) if !finite3(v) => return Ok(None),
        other => other.map(p3),
    };
    Ok(Some(EntityKind::Text(m::Text {
        insert: p3(e.insertion_point),
        height: e.height,
        value: text(&e.value, limits)?,
        rotation: (e.rotation != 0.0).then_some(e.rotation.to_degrees()),
        width_factor: (e.width_factor != 1.0).then_some(e.width_factor),
        oblique: (e.oblique_angle != 0.0).then_some(e.oblique_angle.to_degrees()),
        style: (!e.style.is_empty())
            .then(|| text(&e.style, limits))
            .transpose()?,
        generation: (e.generation_flags != 0).then_some(e.generation_flags),
        horizontal: Some(h_code(e.horizontal_alignment)),
        align,
        vertical: Some(v_code(e.vertical_alignment)),
    })))
}

fn attachment_code(value: dwg::AttachmentPoint) -> i16 {
    match value {
        dwg::AttachmentPoint::TopLeft => 1,
        dwg::AttachmentPoint::TopCenter => 2,
        dwg::AttachmentPoint::TopRight => 3,
        dwg::AttachmentPoint::MiddleLeft => 4,
        dwg::AttachmentPoint::MiddleCenter => 5,
        dwg::AttachmentPoint::MiddleRight => 6,
        dwg::AttachmentPoint::BottomLeft => 7,
        dwg::AttachmentPoint::BottomCenter => 8,
        dwg::AttachmentPoint::BottomRight => 9,
    }
}

fn mtext(
    e: &dwg::MText,
    limits: &Limits,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    let values = [
        e.height,
        e.rectangle_width,
        e.rotation,
        e.line_spacing_factor,
    ];
    if !plane(props, 0.0, 0.0, e.normal) || !finite3(e.insertion_point) || !all_finite(&values) {
        return Ok(None);
    }
    if e.column_data.column_count != 0 || e.column_data.column_type != 0 {
        partial.push("mtext_columns");
    }
    if e.background_fill_flags != 0 {
        partial.push("mtext_background");
    }
    if e.is_annotative {
        partial.push("annotative");
    }
    let x_axis = match e.dwg_x_direction {
        Some(v) if !finite3(v) => return Ok(None),
        other => other.map(p3),
    };
    Ok(Some(EntityKind::MText(m::MText {
        insert: p3(e.insertion_point),
        height: e.height,
        width: (e.rectangle_width != 0.0).then_some(e.rectangle_width),
        attachment: Some(attachment_code(e.attachment_point)),
        direction: Some(match e.drawing_direction {
            dwg::DrawingDirection::LeftToRight => 1,
            dwg::DrawingDirection::TopToBottom => 3,
            dwg::DrawingDirection::ByStyle => 5,
        }),
        style: (!e.style.is_empty())
            .then(|| text(&e.style, limits))
            .transpose()?,
        x_axis,
        rotation: (e.rotation != 0.0).then_some(e.rotation.to_degrees()),
        spacing_style: None,
        spacing_factor: Some(e.line_spacing_factor),
        text: text(&e.value, limits)?,
    })))
}

/// An attribute of an insert: its entity and the names of the data it does not carry.
fn attribute(
    e: &dwg::AttributeEntity,
    limits: &Limits,
) -> Result<Option<(Entity, Vec<&'static str>)>> {
    let common = &e.common;
    let (props, mut partial) = convert::props(common, false);
    let values = [e.height, e.rotation, e.width_factor, e.oblique_angle];
    if !finite3(e.insertion_point)
        || !finite3(e.alignment_point)
        || !all_finite(&values)
        || !props.linetype_scale.is_none_or(f64::is_finite)
    {
        return Ok(None);
    }
    let Ok(flags_bits) = i16::try_from(e.flags.to_bits()) else {
        return Ok(None);
    };
    if e.is_multiline || e.embedded_mtext.is_some() {
        partial.push("attribute_mtext");
    }
    Ok(Some((
        Entity {
            head: convert::head(common.handle, common.owner_handle),
            props,
            kind: EntityKind::Attrib(m::Attrib {
                insert: p3(e.insertion_point),
                height: e.height,
                value: text(&e.value, limits)?,
                rotation: (e.rotation != 0.0).then_some(e.rotation.to_degrees()),
                width_factor: (e.width_factor != 1.0).then_some(e.width_factor),
                oblique: (e.oblique_angle != 0.0).then_some(e.oblique_angle.to_degrees()),
                style: (!e.text_style.is_empty())
                    .then(|| text(&e.text_style, limits))
                    .transpose()?,
                generation: (e.text_generation_flags != 0).then_some(e.text_generation_flags),
                horizontal: Some(match e.horizontal_alignment {
                    dwg::HorizontalAlignment::Left => 0,
                    dwg::HorizontalAlignment::Center => 1,
                    dwg::HorizontalAlignment::Right => 2,
                    dwg::HorizontalAlignment::Aligned => 3,
                    dwg::HorizontalAlignment::Middle => 4,
                    dwg::HorizontalAlignment::Fit => 5,
                }),
                align: Some(p3(e.alignment_point)),
                tag: text(&e.tag, limits)?,
                flags: flags_bits,
                field_length: Some(e.field_length),
                vertical: Some(match e.vertical_alignment {
                    dwg::VerticalAlignment::Baseline => 0,
                    dwg::VerticalAlignment::Bottom => 1,
                    dwg::VerticalAlignment::Middle => 2,
                    dwg::VerticalAlignment::Top => 3,
                }),
            }),
            extras: Vec::new(),
            xdata: Vec::new(),
        },
        partial,
    )))
}

fn insert(
    e: &dwg::Insert,
    limits: &Limits,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    array(e.attributes.len(), limits)?;
    let (Ok(columns), Ok(rows)) = (i16::try_from(e.column_count), i16::try_from(e.row_count))
    else {
        return Ok(None);
    };
    let scales = [
        e.x_scale(),
        e.y_scale(),
        e.z_scale(),
        e.rotation,
        e.column_spacing,
        e.row_spacing,
    ];
    if !plane(props, 0.0, 0.0, e.normal) || !finite3(e.insert_point) || !all_finite(&scales) {
        return Ok(None);
    }
    let mut attribs = Vec::with_capacity(e.attributes.len());
    for attribute_entity in &e.attributes {
        match attribute(attribute_entity, limits)? {
            Some((entity, names)) => {
                attribs.push(entity);
                partial.extend(names);
            }
            None => return Ok(None),
        }
    }
    let (x, y, z) = (e.x_scale(), e.y_scale(), e.z_scale());
    Ok(Some(EntityKind::Insert(m::Insert {
        block: text(&e.block_name, limits)?,
        insert: p3(e.insert_point),
        x_scale: (x != 1.0).then_some(x),
        y_scale: (y != 1.0).then_some(y),
        z_scale: (z != 1.0).then_some(z),
        rotation: (e.rotation != 0.0).then_some(e.rotation.to_degrees()),
        columns: (columns != 1).then_some(columns),
        rows: (rows != 1).then_some(rows),
        column_spacing: (e.column_spacing != 0.0).then_some(e.column_spacing),
        row_spacing: (e.row_spacing != 0.0).then_some(e.row_spacing),
        attribs,
        seqend: None,
    })))
}

fn dimension(
    e: &dwg::Dimension,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Option<EntityKind> {
    let base = e.base();
    if !plane(props, 0.0, 0.0, base.normal) {
        return None;
    }
    let kind = match e {
        dwg::Dimension::Linear(_) => 0,
        dwg::Dimension::Aligned(_) => 1,
        dwg::Dimension::Angular2Ln(_) => 2,
        dwg::Dimension::Diameter(_) => 3,
        dwg::Dimension::Radius(_) => 4,
        dwg::Dimension::Angular3Pt(_) => 5,
        dwg::Dimension::Ordinate(_) => 6,
        dwg::Dimension::Arc(_) => 8,
        dwg::Dimension::LargeRadial(_) => {
            partial.push("large_radial");
            4
        }
    };
    partial.push("dimension_detail");
    let points = [
        base.definition_point,
        base.text_middle_point,
        base.insertion_point,
    ];
    if !points.iter().all(|v| finite3(*v))
        || !base.text_rotation.is_finite()
        || !base.actual_measurement.is_finite()
    {
        return None;
    }
    let text_value = base.user_text.clone().unwrap_or_else(|| base.text.clone());
    Some(EntityKind::Dimension(m::Dimension {
        block: (!base.block_name.is_empty()).then(|| base.block_name.clone()),
        style: (!base.style_name.is_empty()).then(|| base.style_name.clone()),
        def_point: p3(base.definition_point),
        text_mid: p3(base.text_middle_point),
        insert: Some(p3(base.insertion_point)),
        kind,
        attachment: None,
        spacing_style: None,
        spacing_factor: None,
        measurement: Some(base.actual_measurement),
        text: (!text_value.is_empty()).then_some(text_value),
        text_rotation: (base.text_rotation != 0.0).then_some(base.text_rotation.to_degrees()),
        horizontal: None,
        point13: None,
        point14: None,
        point15: None,
        point16: None,
        angle: None,
        oblique: None,
        leader_length: None,
    }))
}

/// Appends the model form of one boundary edge; false when a number is not finite.
fn hatch_edge(
    edge: &dwg::hatch::BoundaryEdge,
    limits: &Limits,
    partial: &mut Vec<&'static str>,
    edges: &mut Vec<m::HatchEdge>,
) -> Result<bool> {
    match edge {
        dwg::hatch::BoundaryEdge::Line(l) => {
            if !finite2(l.start) || !finite2(l.end) {
                return Ok(false);
            }
            edges.push(m::HatchEdge::Line {
                start: p2(l.start),
                end: p2(l.end),
            });
        }
        dwg::hatch::BoundaryEdge::CircularArc(a) => {
            let values = [a.radius, a.start_angle, a.end_angle];
            if !finite2(a.center) || !all_finite(&values) {
                return Ok(false);
            }
            edges.push(m::HatchEdge::Arc {
                center: p2(a.center),
                radius: a.radius,
                start_angle: a.start_angle.to_degrees(),
                end_angle: a.end_angle.to_degrees(),
                counterclockwise: a.counter_clockwise,
            });
        }
        dwg::hatch::BoundaryEdge::EllipticArc(a) => {
            let values = [a.minor_axis_ratio, a.start_angle, a.end_angle];
            if !finite2(a.center) || !finite2(a.major_axis_endpoint) || !all_finite(&values) {
                return Ok(false);
            }
            edges.push(m::HatchEdge::Ellipse {
                center: p2(a.center),
                major_axis: p2(a.major_axis_endpoint),
                ratio: a.minor_axis_ratio,
                start_angle: a.start_angle.to_degrees(),
                end_angle: a.end_angle.to_degrees(),
                counterclockwise: a.counter_clockwise,
            });
        }
        dwg::hatch::BoundaryEdge::Spline(s) => {
            array(s.control_points.len(), limits)?;
            array(s.fit_points.len(), limits)?;
            let tangents_ok = finite2(s.start_tangent) && finite2(s.end_tangent);
            if !all_finite(&s.knots)
                || !tangents_ok
                || !s.control_points.iter().all(|v| finite3(*v))
                || !s.fit_points.iter().all(|v| finite2(*v))
            {
                return Ok(false);
            }
            let tangent = |v: DwgVector2| (v != DwgVector2::default()).then(|| p2(v));
            edges.push(m::HatchEdge::Spline(m::HatchSpline {
                degree: s.degree,
                rational: s.rational,
                periodic: s.periodic,
                knots: s.knots.clone(),
                control_points: s.control_points.iter().map(|v| xy(*v)).collect(),
                weights: Vec::new(),
                fit_points: s.fit_points.iter().map(|v| p2(*v)).collect(),
                start_tangent: tangent(s.start_tangent),
                end_tangent: tangent(s.end_tangent),
            }));
        }
        dwg::hatch::BoundaryEdge::Polyline(poly) => {
            // A polyline inside a path of several edges has no edge form in the model; its
            // segments are kept as line edges and the change is reported.
            array(poly.vertices.len(), limits)?;
            if !poly.vertices.iter().all(|v| finite3(*v)) {
                return Ok(false);
            }
            partial.push("hatch_polyline_edges");
            for pair in poly.vertices.windows(2) {
                edges.push(m::HatchEdge::Line {
                    start: xy(pair[0]),
                    end: xy(pair[1]),
                });
            }
            if let (true, Some(first), Some(last)) =
                (poly.is_closed, poly.vertices.first(), poly.vertices.last())
            {
                edges.push(m::HatchEdge::Line {
                    start: xy(*last),
                    end: xy(*first),
                });
            }
        }
    }
    Ok(true)
}

/// Maps a hatch boundary path that holds one polyline edge to the polyline form of the model.
fn boundary(
    path: &dwg::hatch::BoundaryPath,
    limits: &Limits,
    partial: &mut Vec<&'static str>,
) -> Result<Option<m::HatchBoundary>> {
    if let [dwg::hatch::BoundaryEdge::Polyline(poly)] = path.edges.as_slice() {
        array(poly.vertices.len(), limits)?;
        if !poly.vertices.iter().all(|v| finite3(*v)) {
            return Ok(None);
        }
        return Ok(Some(m::HatchBoundary::Polyline {
            has_bulge: false,
            closed: poly.is_closed,
            vertices: poly
                .vertices
                .iter()
                .map(|v| m::HatchVertex {
                    location: xy(*v),
                    bulge: 0.0,
                })
                .collect(),
        }));
    }
    let mut edges = Vec::with_capacity(path.edges.len());
    for edge in &path.edges {
        if !hatch_edge(edge, limits, partial, &mut edges)? {
            return Ok(None);
        }
    }
    Ok(Some(m::HatchBoundary::Edges(edges)))
}

fn hatch(
    e: &dwg::Hatch,
    limits: &Limits,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    array(e.paths.len(), limits)?;
    array(e.pattern.lines.len(), limits)?;
    array(e.seed_points.len(), limits)?;
    if !plane(props, 0.0, e.elevation, e.normal)
        || !e.pattern_angle.is_finite()
        || !e.pattern_scale.is_finite()
        || !e.pixel_size.is_finite()
    {
        return Ok(None);
    }
    if e.is_mpolygon {
        partial.push("hatch_mpolygon");
    }
    let mut loops = Vec::with_capacity(e.paths.len());
    for path in &e.paths {
        let Some(boundary) = boundary(path, limits, partial)? else {
            return Ok(None);
        };
        let Ok(flags) = i32::try_from(path.flags.bits()) else {
            return Ok(None);
        };
        loops.push(m::HatchLoop {
            flags,
            boundary,
            sources: path
                .boundary_handles
                .iter()
                .filter_map(|h| convert::handle(*h))
                .collect(),
        });
    }
    let mut pattern_lines = Vec::with_capacity(e.pattern.lines.len());
    for line in &e.pattern.lines {
        let values = [
            line.angle,
            line.base_point.x,
            line.base_point.y,
            line.offset.x,
            line.offset.y,
        ];
        if !all_finite(&values) || !all_finite(&line.dash_lengths) {
            return Ok(None);
        }
        array(line.dash_lengths.len(), limits)?;
        pattern_lines.push(m::PatternLine {
            angle: line.angle.to_degrees(),
            base: p2(line.base_point),
            offset: p2(line.offset),
            dashes: line.dash_lengths.clone(),
        });
    }
    if e.seed_points.iter().any(|v| !finite2(*v)) {
        return Ok(None);
    }
    let style = match e.style {
        dwg::hatch::HatchStyleType::Normal => 0,
        dwg::hatch::HatchStyleType::Outer => 1,
        dwg::hatch::HatchStyleType::Ignore => 2,
    };
    let pattern_type = match e.pattern_type {
        dwg::hatch::HatchPatternType::UserDefined => 0,
        dwg::hatch::HatchPatternType::Predefined => 1,
        dwg::hatch::HatchPatternType::Custom => 2,
    };
    Ok(Some(EntityKind::Hatch(m::Hatch {
        elevation_point: [0.0, 0.0, e.elevation],
        pattern_name: text(&e.pattern.name, limits)?,
        solid: i16::from(e.is_solid),
        associative: i16::from(e.is_associative),
        style: Some(style),
        pattern_type: Some(pattern_type),
        pattern_angle: Some(e.pattern_angle.to_degrees()),
        pattern_scale: Some(e.pattern_scale),
        pattern_double: Some(i16::from(e.is_double)),
        pixel_size: (e.pixel_size != 0.0).then_some(e.pixel_size),
        loops,
        pattern_lines,
        seeds: e.seed_points.iter().map(|v| p2(*v)).collect(),
    })))
}

#[cfg(test)]
mod tests {
    use acadrust::types::Vector3 as V3;

    use super::*;

    fn limits() -> Limits {
        Limits::CONSTRAINED
    }

    #[test]
    fn finite_line_is_mapped_on_its_layer() {
        let line = dwg::Line::from_points(V3::new(0.0, 1.0, 0.0), V3::new(2.0, 3.0, 0.0));
        let Outcome::Mapped { entity, partial } = map(&EntityType::Line(line), false, &limits())
            .ok()
            .unwrap_or(Outcome::Invalid)
        else {
            panic!("a finite line is mapped");
        };
        assert!(partial.is_empty());
        assert_eq!(entity.props.layer, "0");
        assert!(matches!(
            entity.kind,
            EntityKind::Line(m::Line {
                end: [2.0, 3.0, 0.0],
                ..
            })
        ));
    }

    #[test]
    fn non_finite_coordinates_are_invalid_not_passed_through() {
        let line = dwg::Line::from_points(V3::new(f64::NAN, 0.0, 0.0), V3::new(1.0, 0.0, 0.0));
        assert!(matches!(
            map(&EntityType::Line(line), false, &limits()),
            Ok(Outcome::Invalid)
        ));
        let circle = dwg::Circle::from_center_radius(V3::new(0.0, 0.0, 0.0), f64::INFINITY);
        assert!(matches!(
            map(&EntityType::Circle(circle), false, &limits()),
            Ok(Outcome::Invalid)
        ));
    }

    #[test]
    fn types_outside_the_mapping_are_unsupported() {
        let solid = dwg::Solid::new(
            V3::new(0.0, 0.0, 0.0),
            V3::new(1.0, 0.0, 0.0),
            V3::new(1.0, 1.0, 0.0),
            V3::new(0.0, 1.0, 0.0),
        );
        assert!(matches!(
            map(&EntityType::Solid(solid), false, &limits()),
            Ok(Outcome::Unsupported)
        ));
        assert_eq!(
            type_name(&EntityType::Solid(dwg::Solid::new(
                V3::new(0.0, 0.0, 0.0),
                V3::new(1.0, 0.0, 0.0),
                V3::new(1.0, 1.0, 0.0),
                V3::new(0.0, 1.0, 0.0)
            ))),
            "SOLID"
        );
    }

    #[test]
    fn arc_angles_are_converted_from_radians_to_degrees() {
        let arc = dwg::Arc::from_center_radius_angles(
            V3::new(0.0, 0.0, 0.0),
            1.0,
            std::f64::consts::PI,
            std::f64::consts::FRAC_PI_2,
        );
        let Ok(Outcome::Mapped { entity, .. }) = map(&EntityType::Arc(arc), false, &limits())
        else {
            panic!("a finite arc is mapped");
        };
        let EntityKind::Arc(arc) = entity.kind else {
            panic!("an arc is mapped to an arc");
        };
        assert!((arc.start_angle - 180.0).abs() < 1e-9);
        assert!((arc.end_angle - 90.0).abs() < 1e-9);
    }

    #[test]
    fn long_text_is_refused_by_the_string_limit() {
        let limits = Limits {
            string_bytes: 3,
            ..Limits::CONSTRAINED
        };
        assert!(text("four", &limits).is_err());
        assert_eq!(text("abc", &limits).ok().as_deref(), Some("abc"));
    }
}
