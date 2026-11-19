//! Mapping of library entities to the neutral entity kinds of `arc-io-dxf`.
//!
//! Each entity yields an [`Outcome`]: a mapped entity with the names of the data the model does
//! not carry, an unsupported type, or an entity whose numbers the model does not accept. The
//! elevation of a two-dimensional polyline is the z of its origin point and the elevation of a
//! hatch is the z of its elevation point, as the DXF reader stores them; the common properties
//! carry the elevation of the other kinds.

use std::f64::consts::TAU;

use acadrust::entities::{self as dwg, EntityType};
use acadrust::types::{Vector2 as DwgVector2, Vector3 as DwgVector3};
use arc_base::{Limits, Result};
use arc_io_dxf::model::{self as m, Entity, EntityKind, EntityProps, P2};

use crate::convert::{self, RecordBudget, all_finite, array, finite2, finite3, p2, p3, text, xy};

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

/// Records the name of data the model does not carry, once for the entity.
fn note(partial: &mut Vec<&'static str>, name: &'static str) {
    if !partial.contains(&name) {
        partial.push(name);
    }
}

/// Maps one entity of the library. Blocks and end markers are never passed here. The records the
/// entity holds (vertices, attributes) are counted against `records`.
pub(crate) fn map(
    entity: &EntityType,
    paper_space: bool,
    limits: &Limits,
    records: &mut RecordBudget<'_>,
) -> Result<Outcome> {
    let common = entity.common();
    let (mut props, mut partial) = convert::props(common, paper_space, limits)?;
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
        EntityType::Polyline2D(e) => polyline2d(e, limits, records, &mut props, &mut partial)?,
        EntityType::Spline(e) => spline(e, limits, &mut props, &mut partial)?,
        EntityType::Text(e) => text_entity(e, limits, &mut props)?,
        EntityType::MText(e) => mtext(e, limits, &mut props, &mut partial)?,
        EntityType::Insert(e) => insert(e, limits, records, &mut props, &mut partial)?,
        EntityType::Dimension(e) => dimension(e, limits, &mut props, &mut partial)?,
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

/// The flag bits a two-dimensional polyline keeps: closed (1), curve fit (2), spline fit (4) and
/// linetype generation (128). The other bits describe meshes and are not carried by this entity.
const POLYLINE2D_FLAGS: u16 = 1 | 2 | 4 | 128;

fn polyline2d(
    e: &dwg::Polyline2D,
    limits: &Limits,
    records: &mut RecordBudget<'_>,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    array(e.vertices.len(), limits)?;
    records.spend(e.vertices.len())?;
    if !plane(props, e.thickness, 0.0, e.normal)
        || !e.elevation.is_finite()
        || !e.start_width.is_finite()
        || !e.end_width.is_finite()
    {
        return Ok(None);
    }
    if e.flags.bits() & !POLYLINE2D_FLAGS != 0 {
        note(partial, "polyline_flags");
    }
    let flags = i16::try_from(e.flags.bits() & POLYLINE2D_FLAGS).unwrap_or_default();
    let smooth_type = match e.smooth_surface {
        dwg::SmoothSurfaceType::None => None,
        dwg::SmoothSurfaceType::QuadraticBSpline => Some(5),
        dwg::SmoothSurfaceType::CubicBSpline => Some(6),
        dwg::SmoothSurfaceType::Bezier => Some(8),
    };
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
        // A vertex takes the layer and paper-space flag of its polyline.
        vertices.push(Entity {
            head: m::Head::default(),
            props: EntityProps {
                paper_space: props.paper_space,
                layer: props.layer.clone(),
                ..EntityProps::default()
            },
            kind: EntityKind::Vertex(vertex),
            extras: Vec::new(),
            xdata: Vec::new(),
        });
    }
    // The elevation is the z of the origin point, which is where the DXF reader keeps it.
    Ok(Some(EntityKind::Polyline(m::Polyline {
        origin: Some([0.0, 0.0, e.elevation]),
        flags,
        start_width: (e.start_width != 0.0).then_some(e.start_width),
        end_width: (e.end_width != 0.0).then_some(e.end_width),
        mesh_m: None,
        mesh_n: None,
        smooth_m: None,
        smooth_n: None,
        smooth_type,
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
        note(partial, "knot_parameterization");
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
        note(partial, "mtext_columns");
    }
    if e.background_fill_flags != 0 {
        note(partial, "mtext_background");
    }
    if e.is_annotative {
        note(partial, "annotative");
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
        spacing_style: Some(match e.line_spacing_style {
            dwg::LineSpacingStyle::AtLeast => 1,
            dwg::LineSpacingStyle::Exactly => 2,
        }),
        spacing_factor: Some(e.line_spacing_factor),
        text: text(&e.value, limits)?,
    })))
}

/// An attribute of an insert: its entity and the names of the data it does not carry. The
/// paper-space flag is the one of the insert that owns the attribute.
fn attribute(
    e: &dwg::AttributeEntity,
    limits: &Limits,
    paper_space: bool,
) -> Result<Option<(Entity, Vec<&'static str>)>> {
    let common = &e.common;
    let (mut props, mut partial) = convert::props(common, paper_space, limits)?;
    let values = [e.height, e.rotation, e.width_factor, e.oblique_angle];
    if !finite3(e.insertion_point)
        || !finite3(e.alignment_point)
        || !all_finite(&values)
        || !props.linetype_scale.is_none_or(f64::is_finite)
        || !plane(&mut props, 0.0, 0.0, e.normal)
    {
        return Ok(None);
    }
    let Ok(flags_bits) = i16::try_from(e.flags.to_bits()) else {
        return Ok(None);
    };
    if e.is_multiline || e.embedded_mtext.is_some() {
        note(&mut partial, "attribute_mtext");
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
    records: &mut RecordBudget<'_>,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    array(e.attributes.len(), limits)?;
    records.spend(e.attributes.len())?;
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
        match attribute(attribute_entity, limits, props.paper_space)? {
            Some((entity, names)) => {
                attribs.push(entity);
                for name in names {
                    note(partial, name);
                }
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
    limits: &Limits,
    props: &mut EntityProps,
    partial: &mut Vec<&'static str>,
) -> Result<Option<EntityKind>> {
    let base = e.base();
    if !plane(props, 0.0, 0.0, base.normal) {
        return Ok(None);
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
            note(partial, "large_radial");
            4
        }
    };
    note(partial, "dimension_detail");
    let points = [
        base.definition_point,
        base.text_middle_point,
        base.insertion_point,
    ];
    if !points.iter().all(|v| finite3(*v))
        || !base.text_rotation.is_finite()
        || !base.actual_measurement.is_finite()
    {
        return Ok(None);
    }
    let block = (!base.block_name.is_empty())
        .then(|| text(&base.block_name, limits))
        .transpose()?;
    let style = (!base.style_name.is_empty())
        .then(|| text(&base.style_name, limits))
        .transpose()?;
    let user_or_text = base.user_text.clone().unwrap_or_else(|| base.text.clone());
    let value = (!user_or_text.is_empty())
        .then(|| text(&user_or_text, limits))
        .transpose()?;
    Ok(Some(EntityKind::Dimension(m::Dimension {
        block,
        style,
        def_point: p3(base.definition_point),
        text_mid: p3(base.text_middle_point),
        insert: Some(p3(base.insertion_point)),
        kind,
        attachment: None,
        spacing_style: None,
        spacing_factor: None,
        measurement: Some(base.actual_measurement),
        text: value,
        text_rotation: (base.text_rotation != 0.0).then_some(base.text_rotation.to_degrees()),
        horizontal: None,
        point13: None,
        point14: None,
        point15: None,
        point16: None,
        angle: None,
        oblique: None,
        leader_length: None,
    })))
}

/// The circular angle, in degrees, of an elliptic hatch edge point given by its ellipse parameter.
///
/// The model keeps the angle the DXF hatch edge stores (groups 50 and 51): the direction from the
/// center, in degrees. The `arc-io-dxf` model documents its angles as degrees (module documentation,
/// "angles are in degrees and parameters in radians, as the format stores them"), and the DXF reader
/// stores the values of the file without conversion. The library stores the
/// ellipse parameter `t` in radians, the point `(a cos t, b sin t)` where `b / a` is the ratio. The
/// direction of that point from the center is `atan2(ratio sin t, cos t)`, and the result stays on
/// the turn of `t`. For a ratio of 1 the two are equal.
fn circular_degrees(ratio: f64, parameter: f64) -> f64 {
    let direction = (ratio * parameter.sin()).atan2(parameter.cos());
    let turns = ((parameter - direction) / TAU).round();
    (direction + turns * TAU).to_degrees()
}

/// The arc of a polyline segment with a bulge, as (center, radius, start angle, end angle) in
/// radians. The bulge is the tangent of a quarter of the included angle, positive for a
/// counterclockwise arc; the start and end angles are those of the two ends on the circle.
/// Absent for a straight segment.
fn bulge_arc((a, b): (P2, P2), bulge: f64) -> Option<(P2, f64, f64, f64)> {
    if bulge == 0.0 || !bulge.is_finite() {
        return None;
    }
    let [ax, ay] = a;
    let [bx, by] = b;
    let (dx, dy) = (bx - ax, by - ay);
    let chord = dx.hypot(dy);
    if chord == 0.0 {
        return None;
    }
    let half = chord / 2.0;
    let (mx, my) = ((ax + bx) / 2.0, (ay + by) / 2.0);
    // The left normal of the chord; the center lies on it, at the signed distance below.
    let (nx, ny) = (-dy / chord, dx / chord);
    let offset = half * (1.0 - bulge * bulge) / (2.0 * bulge);
    let (cx, cy) = (mx + nx * offset, my + ny * offset);
    let radius = half * (1.0 + bulge * bulge) / (2.0 * bulge.abs());
    let start_angle = (ay - cy).atan2(ax - cx);
    let end_angle = (by - cy).atan2(bx - cx);
    Some(([cx, cy], radius, start_angle, end_angle))
}

/// The edge of one segment of a polyline boundary: an arc when the bulge of its start vertex is
/// not zero, a line otherwise.
fn segment_edge(start: DwgVector3, end: DwgVector3) -> m::HatchEdge {
    let bulge = start.z;
    let (a, b) = (xy(start), xy(end));
    match bulge_arc((a, b), bulge) {
        Some((center, radius, start_angle, end_angle)) => m::HatchEdge::Arc {
            center,
            radius,
            start_angle: start_angle.to_degrees(),
            end_angle: end_angle.to_degrees(),
            counterclockwise: bulge > 0.0,
        },
        None => m::HatchEdge::Line { start: a, end: b },
    }
}

/// The segments of a polyline boundary: each vertex starts the segment to the next one, and the
/// last vertex starts the closing segment when the polyline is closed.
fn polyline_segments(vertices: &[DwgVector3], closed: bool) -> Vec<(DwgVector3, DwgVector3)> {
    let mut segments = Vec::with_capacity(vertices.len());
    for (index, start) in vertices.iter().enumerate() {
        let end = match vertices.get(index + 1) {
            Some(end) => *end,
            None if closed => match vertices.first() {
                Some(first) => *first,
                None => break,
            },
            None => break,
        };
        segments.push((*start, end));
    }
    segments
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
                start_angle: circular_degrees(a.minor_axis_ratio, a.start_angle),
                end_angle: circular_degrees(a.minor_axis_ratio, a.end_angle),
                counterclockwise: a.counter_clockwise,
            });
        }
        dwg::hatch::BoundaryEdge::Spline(s) => {
            array(s.control_points.len(), limits)?;
            array(s.fit_points.len(), limits)?;
            array(s.knots.len(), limits)?;
            let tangents_ok = finite2(s.start_tangent) && finite2(s.end_tangent);
            if !all_finite(&s.knots)
                || !tangents_ok
                || !s.control_points.iter().all(|v| finite3(*v))
                || !s.fit_points.iter().all(|v| finite2(*v))
            {
                return Ok(false);
            }
            let tangent = |v: DwgVector2| (v != DwgVector2::default()).then(|| p2(v));
            // A rational control point carries its weight in z.
            let weights = if s.rational {
                s.control_points.iter().map(|v| v.z).collect()
            } else {
                Vec::new()
            };
            edges.push(m::HatchEdge::Spline(m::HatchSpline {
                degree: s.degree,
                rational: s.rational,
                periodic: s.periodic,
                knots: s.knots.clone(),
                control_points: s.control_points.iter().map(|v| xy(*v)).collect(),
                weights,
                fit_points: s.fit_points.iter().map(|v| p2(*v)).collect(),
                start_tangent: tangent(s.start_tangent),
                end_tangent: tangent(s.end_tangent),
            }));
        }
        dwg::hatch::BoundaryEdge::Polyline(poly) => {
            // A polyline inside a path of several edges has no polyline form in that path; its
            // segments are kept as edges, with an arc for each bulged segment, and the change is
            // reported.
            array(poly.vertices.len(), limits)?;
            if !poly.vertices.iter().all(|v| finite3(*v)) {
                return Ok(false);
            }
            note(partial, "hatch_polyline_edges");
            for (start, end) in polyline_segments(&poly.vertices, poly.is_closed) {
                edges.push(segment_edge(start, end));
            }
        }
    }
    Ok(true)
}

/// Maps a hatch boundary path to the model. A path that holds one polyline keeps the polyline
/// form, with the bulge of each vertex; other paths are lists of edges.
fn boundary(
    path: &dwg::hatch::BoundaryPath,
    limits: &Limits,
    partial: &mut Vec<&'static str>,
) -> Result<Option<m::HatchBoundary>> {
    array(path.edges.len(), limits)?;
    if let [dwg::hatch::BoundaryEdge::Polyline(poly)] = path.edges.as_slice() {
        array(poly.vertices.len(), limits)?;
        if !poly.vertices.iter().all(|v| finite3(*v)) {
            return Ok(None);
        }
        return Ok(Some(m::HatchBoundary::Polyline {
            has_bulge: poly.vertices.iter().any(|v| v.z != 0.0),
            closed: poly.is_closed,
            vertices: poly
                .vertices
                .iter()
                .map(|v| m::HatchVertex {
                    location: xy(*v),
                    bulge: v.z,
                })
                .collect(),
        }));
    }
    let mut edges = Vec::with_capacity(path.edges.len());
    for edge in &path.edges {
        if !hatch_edge(edge, limits, partial, &mut edges)? {
            return Ok(None);
        }
        array(edges.len(), limits)?;
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
    if !plane(props, 0.0, 0.0, e.normal)
        || !e.elevation.is_finite()
        || !e.pattern_angle.is_finite()
        || !e.pattern_scale.is_finite()
        || !e.pixel_size.is_finite()
    {
        return Ok(None);
    }
    if e.is_mpolygon {
        note(partial, "hatch_mpolygon");
    }
    if e.gradient_color.enabled {
        note(partial, "hatch_gradient");
    }
    let mut loops = Vec::with_capacity(e.paths.len());
    for path in &e.paths {
        let Some(boundary) = boundary(path, limits, partial)? else {
            return Ok(None);
        };
        let Ok(flags) = i32::try_from(path.flags.bits()) else {
            return Ok(None);
        };
        array(path.boundary_handles.len(), limits)?;
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
    // The elevation is the z of the elevation point, which is where the DXF reader keeps it.
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

    /// Maps one entity with a fresh record budget, as the mapping of a drawing does for one record.
    fn map_entity(entity: &EntityType, paper_space: bool, limits: &Limits) -> Result<Outcome> {
        let mut used = 0;
        let mut budget = RecordBudget::new(&mut used, limits.record_count);
        map(entity, paper_space, limits, &mut budget)
    }

    fn mapped(entity: &EntityType) -> (Entity, Vec<&'static str>) {
        match map_entity(entity, false, &limits()) {
            Ok(Outcome::Mapped { entity, partial }) => (*entity, partial),
            _ => panic!("the entity is mapped"),
        }
    }

    /// Maps one entity lying on paper space.
    fn mapped_paper(entity: &EntityType) -> (Entity, Vec<&'static str>) {
        match map_entity(entity, true, &limits()) {
            Ok(Outcome::Mapped { entity, partial }) => (*entity, partial),
            _ => panic!("the entity is mapped"),
        }
    }

    fn hatch_with(path: dwg::hatch::BoundaryPath) -> dwg::Hatch {
        let mut hatch = dwg::Hatch::new();
        hatch.add_path(path);
        hatch
    }

    #[test]
    fn finite_line_is_mapped_on_its_layer() {
        let line = dwg::Line::from_points(V3::new(0.0, 1.0, 0.0), V3::new(2.0, 3.0, 0.0));
        let Outcome::Mapped { entity, partial } =
            map_entity(&EntityType::Line(line), false, &limits())
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
            map_entity(&EntityType::Line(line), false, &limits()),
            Ok(Outcome::Invalid)
        ));
        let circle = dwg::Circle::from_center_radius(V3::new(0.0, 0.0, 0.0), f64::INFINITY);
        assert!(matches!(
            map_entity(&EntityType::Circle(circle), false, &limits()),
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
            map_entity(&EntityType::Solid(solid), false, &limits()),
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
        let Ok(Outcome::Mapped { entity, .. }) =
            map_entity(&EntityType::Arc(arc), false, &limits())
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

    #[test]
    fn elliptic_hatch_edges_keep_the_circular_angle_of_the_file() {
        // With ratio 0.5 the parameter pi/4 is the direction atan(0.5), not 45 degrees.
        let mut path = dwg::hatch::BoundaryPath::new();
        path.add_edge(dwg::hatch::BoundaryEdge::EllipticArc(
            dwg::hatch::EllipticArcEdge {
                center: DwgVector2::new(0.0, 0.0),
                major_axis_endpoint: DwgVector2::new(2.0, 0.0),
                minor_axis_ratio: 0.5,
                start_angle: std::f64::consts::FRAC_PI_4,
                end_angle: std::f64::consts::FRAC_PI_2,
                counter_clockwise: true,
            },
        ));
        let (entity, _) = mapped(&EntityType::Hatch(Box::new(hatch_with(path))));
        let EntityKind::Hatch(hatch) = entity.kind else {
            panic!("a hatch is mapped to a hatch");
        };
        let m::HatchBoundary::Edges(edges) = &hatch.loops[0].boundary else {
            panic!("an elliptic edge keeps the edge form");
        };
        let [
            m::HatchEdge::Ellipse {
                ratio,
                start_angle,
                end_angle,
                ..
            },
        ] = edges.as_slice()
        else {
            panic!("one elliptic edge is mapped");
        };
        assert_eq!(*ratio, 0.5);
        assert!(
            (start_angle - 0.5_f64.atan().to_degrees()).abs() < 1e-9,
            "{start_angle}"
        );
        assert!((end_angle - 90.0).abs() < 1e-9, "{end_angle}");
    }

    #[test]
    fn circular_degrees_match_the_angle_at_ratio_one() {
        for t in [0.3_f64, 1.2, -0.7, 4.0] {
            assert!((circular_degrees(1.0, t) - t.to_degrees()).abs() < 1e-9);
        }
    }

    #[test]
    fn bulged_hatch_polyline_keeps_its_bulges() {
        let mut poly = dwg::hatch::PolylineEdge::new(
            vec![DwgVector2::new(0.0, 0.0), DwgVector2::new(2.0, 0.0)],
            true,
        );
        poly.vertices[0].z = 1.0;
        let mut path = dwg::hatch::BoundaryPath::new();
        path.add_edge(dwg::hatch::BoundaryEdge::Polyline(poly));
        let (entity, _) = mapped(&EntityType::Hatch(Box::new(hatch_with(path))));
        let EntityKind::Hatch(hatch) = entity.kind else {
            panic!("a hatch is mapped to a hatch");
        };
        let m::HatchBoundary::Polyline {
            has_bulge,
            closed,
            vertices,
        } = &hatch.loops[0].boundary
        else {
            panic!("a single polyline keeps the polyline form");
        };
        assert!(*has_bulge && *closed);
        assert_eq!(vertices[0].bulge, 1.0);
        assert_eq!(vertices[1].bulge, 0.0);
    }

    #[test]
    fn bulged_segments_in_a_multi_edge_path_become_arcs() {
        // A half circle of radius 1 centered at the origin, from (1, 0) counterclockwise to (-1, 0).
        let poly = dwg::hatch::PolylineEdge {
            vertices: vec![
                DwgVector3::new(1.0, 0.0, 1.0),
                DwgVector3::new(-1.0, 0.0, 0.0),
            ],
            is_closed: false,
        };
        let mut path = dwg::hatch::BoundaryPath::new();
        path.add_edge(dwg::hatch::BoundaryEdge::Polyline(poly));
        path.add_edge(dwg::hatch::BoundaryEdge::Line(dwg::hatch::LineEdge {
            start: DwgVector2::new(-1.0, 0.0),
            end: DwgVector2::new(1.0, 0.0),
        }));
        let (entity, partial) = mapped(&EntityType::Hatch(Box::new(hatch_with(path))));
        assert!(partial.contains(&"hatch_polyline_edges"));
        let EntityKind::Hatch(hatch) = entity.kind else {
            panic!("a hatch is mapped to a hatch");
        };
        let m::HatchBoundary::Edges(edges) = &hatch.loops[0].boundary else {
            panic!("a multi-edge path keeps the edge form");
        };
        let [
            m::HatchEdge::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                counterclockwise,
            },
            ..,
        ] = edges.as_slice()
        else {
            panic!("the bulged segment is an arc");
        };
        assert!(center[0].abs() < 1e-9 && center[1].abs() < 1e-9);
        assert!((radius - 1.0).abs() < 1e-9);
        assert!((start_angle - 0.0).abs() < 1e-9);
        assert!((end_angle - 180.0).abs() < 1e-9);
        assert!(*counterclockwise);
    }

    #[test]
    fn two_dimensional_polyline_flags_and_smoothing_are_kept() {
        let mut poly = dwg::Polyline2D::new();
        poly.flags = dwg::PolylineFlags::from_bits(2 | 4 | 128 | 16);
        poly.smooth_surface = dwg::SmoothSurfaceType::CubicBSpline;
        poly.elevation = 2.5;
        let (entity, partial) = mapped(&EntityType::Polyline2D(poly));
        assert!(!partial.contains(&"smooth_surface"));
        assert_eq!(partial, vec!["polyline_flags"]);
        assert_eq!(
            entity.props.elevation, None,
            "the elevation is in the origin"
        );
        let EntityKind::Polyline(poly) = entity.kind else {
            panic!("a polyline is mapped to a polyline");
        };
        assert_eq!(poly.flags, 2 | 4 | 128);
        assert_eq!(poly.smooth_type, Some(6));
        assert_eq!(poly.origin, Some([0.0, 0.0, 2.5]));
    }

    #[test]
    fn polyline_vertices_take_the_layer_of_their_polyline() {
        let mut poly = dwg::Polyline2D::new();
        poly.common.layer = "WALLS".to_owned();
        poly.vertices
            .push(dwg::Vertex2D::new(DwgVector3::new(0.0, 0.0, 0.0)));
        let (entity, _) = mapped(&EntityType::Polyline2D(poly));
        let EntityKind::Polyline(poly) = entity.kind else {
            panic!("a polyline is mapped to a polyline");
        };
        assert_eq!(poly.vertices[0].props.layer, "WALLS");
    }

    #[test]
    fn hatch_elevation_is_the_elevation_point_only() {
        let mut hatch = dwg::Hatch::new();
        hatch.elevation = 4.0;
        let (entity, _) = mapped(&EntityType::Hatch(Box::new(hatch)));
        assert_eq!(entity.props.elevation, None);
        let EntityKind::Hatch(hatch) = entity.kind else {
            panic!("a hatch is mapped to a hatch");
        };
        assert_eq!(hatch.elevation_point, [0.0, 0.0, 4.0]);
    }

    #[test]
    fn spline_edge_weights_come_from_the_rational_control_points() {
        let edge = dwg::hatch::SplineEdge {
            degree: 1,
            rational: true,
            periodic: false,
            knots: vec![0.0, 0.0, 1.0, 1.0],
            control_points: vec![
                DwgVector3::new(0.0, 0.0, 2.0),
                DwgVector3::new(1.0, 0.0, 0.5),
            ],
            fit_points: Vec::new(),
            start_tangent: DwgVector2::default(),
            end_tangent: DwgVector2::default(),
        };
        let mut path = dwg::hatch::BoundaryPath::new();
        path.add_edge(dwg::hatch::BoundaryEdge::Spline(edge));
        let (entity, _) = mapped(&EntityType::Hatch(Box::new(hatch_with(path))));
        let EntityKind::Hatch(hatch) = entity.kind else {
            panic!("a hatch is mapped to a hatch");
        };
        let m::HatchBoundary::Edges(edges) = &hatch.loops[0].boundary else {
            panic!("a spline keeps the edge form");
        };
        let [m::HatchEdge::Spline(spline)] = edges.as_slice() else {
            panic!("one spline edge is mapped");
        };
        assert_eq!(spline.weights, vec![2.0, 0.5]);
    }

    #[test]
    fn boundary_handles_and_edges_are_counted_against_the_array_limit() {
        let limits = Limits {
            array_length: 2,
            ..Limits::CONSTRAINED
        };
        let mut path = dwg::hatch::BoundaryPath::new();
        for _ in 0..3 {
            path.add_boundary_handle(acadrust::types::Handle::new(0x10));
        }
        let hatch = EntityType::Hatch(Box::new(hatch_with(path)));
        let err = map_entity(&hatch, false, &limits).err();
        assert_eq!(err.as_ref().map(|e| e.code()), Some("io.limit_exceeded"));
        assert_eq!(
            err.and_then(|e| e.arguments().get("limit").cloned()),
            Some(arc_base::error::Value::Text("array_length".to_owned()))
        );
    }

    #[test]
    fn non_finite_elevations_and_normals_are_invalid() {
        let mut poly = dwg::Polyline2D::new();
        poly.elevation = f64::NAN;
        assert!(matches!(
            map_entity(&EntityType::Polyline2D(poly), false, &limits()),
            Ok(Outcome::Invalid)
        ));
        let mut hatch = dwg::Hatch::new();
        hatch.normal = DwgVector3::new(f64::INFINITY, 0.0, 1.0);
        assert!(matches!(
            map_entity(&EntityType::Hatch(Box::new(hatch)), false, &limits()),
            Ok(Outcome::Invalid)
        ));
    }

    #[test]
    fn text_entity_oblique_is_in_degrees() {
        // Text entities keep their own oblique angle in degrees, as the model stores it.
        let mut text = dwg::Text::with_value("A", DwgVector3::new(0.0, 0.0, 0.0));
        text.oblique_angle = std::f64::consts::FRAC_PI_6;
        let (entity, _) = mapped(&EntityType::Text(text));
        let EntityKind::Text(text) = entity.kind else {
            panic!("a text is mapped to a text");
        };
        assert!((text.oblique.unwrap_or(f64::NAN) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn dimension_names_are_checked_against_the_string_limit() {
        let limits = Limits {
            string_bytes: 3,
            ..Limits::CONSTRAINED
        };
        let mut dim = dwg::DimensionLinear::new(
            DwgVector3::new(0.0, 0.0, 0.0),
            DwgVector3::new(1.0, 0.0, 0.0),
        );
        dim.base.style_name = "A_LONG_DIMENSION_STYLE".to_owned();
        let err = map_entity(
            &EntityType::Dimension(Box::new(dwg::Dimension::Linear(dim))),
            false,
            &limits,
        )
        .err();
        assert_eq!(err.map(|e| e.code()), Some("io.limit_exceeded"));
    }

    #[test]
    fn attributes_take_the_paper_space_flag_and_normal_of_their_insert() {
        let mut insert = dwg::Insert::new("DOOR", DwgVector3::new(0.0, 0.0, 0.0));
        let mut attribute = dwg::AttributeEntity::new("NUMBER".to_owned(), "101".to_owned());
        attribute.normal = DwgVector3::new(0.0, 0.0, -1.0);
        insert.attributes.push(attribute);
        let (entity, _) = mapped_paper(&EntityType::Insert(insert));
        let EntityKind::Insert(insert) = entity.kind else {
            panic!("an insert is mapped to an insert");
        };
        let attrib = &insert.attribs[0];
        assert!(attrib.props.paper_space);
        assert_eq!(attrib.props.extrusion, Some([0.0, 0.0, -1.0]));
    }

    #[test]
    fn an_attribute_with_a_normal_that_is_not_finite_makes_the_insert_invalid() {
        let mut insert = dwg::Insert::new("DOOR", DwgVector3::new(0.0, 0.0, 0.0));
        let mut attribute = dwg::AttributeEntity::new("NUMBER".to_owned(), "101".to_owned());
        attribute.normal = DwgVector3::new(f64::NAN, 0.0, 1.0);
        insert.attributes.push(attribute);
        assert!(matches!(
            map_entity(&EntityType::Insert(insert), false, &limits()),
            Ok(Outcome::Invalid)
        ));
    }

    #[test]
    fn mtext_line_spacing_style_is_kept() {
        let mut text = dwg::MText::with_value("NOTE", DwgVector3::new(0.0, 0.0, 0.0));
        text.line_spacing_style = dwg::LineSpacingStyle::Exactly;
        let (entity, partial) = mapped(&EntityType::MText(Box::new(text)));
        assert!(partial.is_empty());
        let EntityKind::MText(text) = entity.kind else {
            panic!("an mtext is mapped to an mtext");
        };
        assert_eq!(text.spacing_style, Some(2));
    }

    #[test]
    fn hatch_gradients_are_partial() {
        let mut hatch = dwg::Hatch::new();
        hatch.gradient_color.enabled = true;
        let (_, partial) = mapped(&EntityType::Hatch(Box::new(hatch)));
        assert_eq!(partial, vec!["hatch_gradient"]);
    }

    #[test]
    fn partial_names_of_a_multi_edge_hatch_are_listed_once() {
        let poly = dwg::hatch::PolylineEdge {
            vertices: vec![
                DwgVector3::new(0.0, 0.0, 0.0),
                DwgVector3::new(1.0, 0.0, 0.0),
                DwgVector3::new(1.0, 1.0, 0.0),
            ],
            is_closed: false,
        };
        let mut path = dwg::hatch::BoundaryPath::new();
        path.add_edge(dwg::hatch::BoundaryEdge::Polyline(poly.clone()));
        path.add_edge(dwg::hatch::BoundaryEdge::Polyline(poly));
        path.add_edge(dwg::hatch::BoundaryEdge::Line(dwg::hatch::LineEdge {
            start: DwgVector2::new(1.0, 1.0),
            end: DwgVector2::new(0.0, 0.0),
        }));
        let (_, partial) = mapped(&EntityType::Hatch(Box::new(hatch_with(path))));
        assert_eq!(partial, vec!["hatch_polyline_edges"]);
    }
}
