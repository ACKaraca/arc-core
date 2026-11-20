//! The neutral drawing model (CORE-IO-001).
//!
//! The model follows the structure of the format: header variables, classes, symbol tables,
//! blocks, entities and objects. Every record keeps its handle as its source identifier, groups
//! the model does not type as preserved groups ([`Entity::extras`] and its siblings) and
//! extended data per application. Records of unknown types are [`PreservedRecord`]s. Fields that
//! the format makes optional are `Option`s; a field that is not an `Option` is always written.
//!
//! Points are `[x, y, z]` or `[x, y]`; angles are in degrees and parameters in radians, as the
//! format stores them. Values are kept exactly as read; validation of geometry is the product's
//! mapping work.

use arc_base::Result;

use crate::error::{Position, codes};
use crate::fields::{Body, Cursor, Cx, Field, WriteCx, count, fields};
use crate::group::{Group, Handle, Value, Version};

/// A three-dimensional point or vector.
pub type P3 = [f64; 3];
/// A two-dimensional point or vector.
pub type P2 = [f64; 2];

/// A drawing: the whole content of one file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Drawing {
    /// The format version (`$ACADVER`); the writer replaces it by the target version.
    pub version: Version,
    /// The header variables in file order, without `$ACADVER`.
    pub header: Vec<HeaderVariable>,
    /// The class definitions of the CLASSES section.
    pub classes: Vec<Class>,
    /// The symbol tables.
    pub tables: Tables,
    /// The block definitions.
    pub blocks: Vec<Block>,
    /// The entities of the ENTITIES section.
    pub entities: Vec<Entity>,
    /// The objects of the OBJECTS section.
    pub objects: Vec<Object>,
    /// Sections the model does not type, in file order.
    pub preserved_sections: Vec<PreservedSection>,
}

impl Drawing {
    /// The value groups of a header variable, by name including the `$`.
    #[must_use]
    pub fn header_variable(&self, name: &str) -> Option<&[Group]> {
        self.header
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
            .map(|v| v.values.as_slice())
    }

    /// The legacy code page named by `$DWGCODEPAGE`, `ANSI_1252` when absent.
    #[must_use]
    pub fn code_page(&self) -> &str {
        self.header_variable("$DWGCODEPAGE")
            .and_then(|values| values.iter().find_map(Group::as_text))
            .unwrap_or("ANSI_1252")
    }
}

/// A header variable: its name (with `$`) and its value groups, typed by their group codes (a
/// point is three groups 10, 20, 30).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeaderVariable {
    /// The name including the `$`.
    pub name: String,
    /// The values.
    pub values: Vec<Group>,
}

/// The handle, owner and application groups that start a record.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Head {
    /// The handle (group 5, or 105 for dimension styles), the record's source identifier.
    pub handle: Option<Handle>,
    /// The owner handle (the first group 330 before the subclass markers).
    pub owner: Option<Handle>,
    /// Application-defined groups (`102 {NAME` ... `102 }`) before the subclass markers.
    pub app_groups: Vec<Group>,
}

/// Extended data of one registered application.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct XData {
    /// The application name (group 1001).
    pub app: String,
    /// The groups 1000–1071 that follow it.
    pub groups: Vec<Group>,
}

/// A record the model does not type, kept for writing back unchanged (CORE-IO-003).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreservedRecord {
    /// The record type (the value of group 0).
    pub kind: String,
    /// Every group after group 0, in file order.
    pub groups: Vec<Group>,
}

impl PreservedRecord {
    /// The handle (group 5), the record's source identifier.
    #[must_use]
    pub fn handle(&self) -> Option<Handle> {
        self.groups
            .iter()
            .find(|g| g.code == 5)
            .and_then(Group::as_handle)
    }
}

/// A section the model does not type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreservedSection {
    /// The section name (group 2 of the SECTION record).
    pub name: String,
    /// Groups between the name and the first record.
    pub leading: Vec<Group>,
    /// The records of the section.
    pub records: Vec<PreservedRecord>,
}

fields! {
    /// A class definition of the CLASSES section.
    pub struct Class {
        /// The record name used in the file.
        dxf_name: String = 1,
        /// The class name.
        class_name: String = 2,
        /// The application name.
        application: String = 3,
        /// The proxy capability flags.
        proxy_flags: i32 = 90,
        /// The instance count.
        instance_count: Option<i32> = 91,
        /// Whether the class was a proxy.
        was_proxy: i16 = 280,
        /// Whether the class is an entity.
        is_entity: i16 = 281,
        /// Groups the model does not type.
        extras: Vec<Group>,
    }
}

impl Body for Class {}

/// The symbol tables. A table is `None` when the file has no such table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tables {
    /// Viewport configurations (VPORT).
    pub vports: Option<Table<VPort>>,
    /// Linetypes (LTYPE).
    pub linetypes: Option<Table<Linetype>>,
    /// Layers (LAYER).
    pub layers: Option<Table<Layer>>,
    /// Text styles (STYLE).
    pub styles: Option<Table<TextStyle>>,
    /// Named views (VIEW).
    pub views: Option<Table<View>>,
    /// User coordinate systems (UCS).
    pub ucs: Option<Table<Ucs>>,
    /// Registered applications (APPID).
    pub appids: Option<Table<AppId>>,
    /// Dimension styles (DIMSTYLE).
    pub dimstyles: Option<Table<DimStyle>>,
    /// Block records (`BLOCK_RECORD`).
    pub block_records: Option<Table<BlockRecord>>,
    /// Tables the model does not type.
    pub preserved: Vec<PreservedTable>,
}

/// A symbol table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table<T> {
    /// The table record's head.
    pub head: Head,
    /// Groups of the table record the model does not type (the entry count is derived).
    pub extras: Vec<Group>,
    /// Extended data of the table record.
    pub xdata: Vec<XData>,
    /// The entries.
    pub entries: Vec<TableEntry<T>>,
}

/// One entry of a symbol table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableEntry<T> {
    /// The head.
    pub head: Head,
    /// The name (group 2).
    pub name: String,
    /// The standard flags (group 70; the insertion units of a block record).
    pub flags: i16,
    /// The table-specific data.
    pub data: T,
    /// Groups the model does not type.
    pub extras: Vec<Group>,
    /// Extended data.
    pub xdata: Vec<XData>,
}

/// A table the model does not type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreservedTable {
    /// The table name.
    pub name: String,
    /// The groups of the table record after the name.
    pub groups: Vec<Group>,
    /// The entries.
    pub entries: Vec<PreservedRecord>,
}

fields! {
    /// Layer data.
    pub struct Layer {
        /// The color number; negative when the layer is off.
        ["AcDbLayerTableRecord"]
        color: i16 = 62,
        /// The true color.
        true_color: Option<i32> = 420,
        /// The linetype name.
        linetype: String = 6,
        /// Whether the layer plots.
        plot: Option<bool> = 290,
        /// The lineweight.
        lineweight: Option<i16> = 370,
        /// The plot style handle.
        plot_style: Option<Handle> = 390,
        /// The material handle.
        material: Option<Handle> = 347,
    }
}

fields! {
    /// One element of a linetype pattern.
    pub struct LinetypeElement {
        /// The dash length; negative for a gap.
        length: f64 = 49,
        /// The element type flags.
        kind: i16 = 74,
        /// The shape number.
        shape: Option<i16> = 75,
        /// The text style handle of a shape or text element.
        style: Option<Handle> = 340,
        /// The scale.
        scale: Option<f64> = 46,
        /// The rotation.
        rotation: Option<f64> = 50,
        /// The x offset.
        offset_x: Option<f64> = 44,
        /// The y offset.
        offset_y: Option<f64> = 45,
        /// The text.
        text: Option<String> = 9,
    }
}

impl Field for Vec<LinetypeElement> {
    fn take(&mut self, code: i16, g: &Group) -> bool {
        if g.code == code {
            let mut e = LinetypeElement::default();
            e.length.take(code, g);
            self.push(e);
            return true;
        }
        self.last_mut()
            .is_some_and(|e| crate::fields::Fields::take_field(e, g))
    }
    fn put(&self, _code: i16, out: &mut Vec<Group>) {
        for e in self {
            crate::fields::Fields::put_fields(e, out);
        }
    }
}

fields! {
    /// Linetype data.
    pub struct Linetype {
        /// The description.
        ["AcDbLinetypeTableRecord"]
        description: String = 3,
        /// The alignment code (65, `A`).
        alignment: i16 = 72,
        /// The total pattern length.
        pattern_length: f64 = 40,
        /// The pattern elements.
        elements: Vec<LinetypeElement> = 49 (count 73),
    }
}

fields! {
    /// Text style data.
    pub struct TextStyle {
        /// The fixed height; 0 when not fixed.
        ["AcDbTextStyleTableRecord"]
        height: f64 = 40,
        /// The width factor.
        width_factor: f64 = 41,
        /// The oblique angle.
        oblique: f64 = 50,
        /// The text generation flags.
        generation: i16 = 71,
        /// The last height used.
        last_height: f64 = 42,
        /// The primary font file.
        font: String = 3,
        /// The big font file.
        big_font: Option<String> = 4,
    }
}

fields! {
    /// Named view data.
    pub struct View {
        /// The view height.
        ["AcDbViewTableRecord"]
        height: f64 = 40,
        /// The view center.
        center: P2 = 10,
        /// The view width.
        width: f64 = 41,
        /// The view direction from the target.
        direction: P3 = 11,
        /// The target point.
        target: P3 = 12,
        /// The lens length.
        lens: f64 = 42,
        /// The front clipping offset.
        front_clip: f64 = 43,
        /// The back clipping offset.
        back_clip: f64 = 44,
        /// The twist angle.
        twist: f64 = 50,
        /// The view mode.
        mode: i16 = 71,
        /// The render mode.
        render_mode: Option<i16> = 281,
        /// Whether a UCS is associated.
        ucs_associated: Option<i16> = 72,
    }
}

fields! {
    /// Viewport configuration data.
    pub struct VPort {
        /// The lower-left corner.
        ["AcDbViewportTableRecord"]
        lower_left: P2 = 10,
        /// The upper-right corner.
        upper_right: P2 = 11,
        /// The view center.
        center: P2 = 12,
        /// The snap base point.
        snap_base: P2 = 13,
        /// The snap spacing.
        snap_spacing: P2 = 14,
        /// The grid spacing.
        grid_spacing: P2 = 15,
        /// The view direction.
        direction: P3 = 16,
        /// The view target.
        target: P3 = 17,
        /// The view height.
        height: f64 = 40,
        /// The aspect ratio.
        aspect: f64 = 41,
        /// The lens length.
        lens: f64 = 42,
        /// The front clipping offset.
        front_clip: f64 = 43,
        /// The back clipping offset.
        back_clip: f64 = 44,
        /// The snap rotation.
        snap_rotation: f64 = 50,
        /// The twist angle.
        twist: f64 = 51,
        /// The view mode.
        mode: Option<i16> = 71,
        /// The circle zoom percent.
        circle_zoom: Option<i16> = 72,
        /// The fast zoom setting.
        fast_zoom: Option<i16> = 73,
        /// The UCS icon setting.
        ucs_icon: Option<i16> = 74,
        /// The snap setting.
        snap: Option<i16> = 75,
        /// The grid setting.
        grid: Option<i16> = 76,
        /// The snap style.
        snap_style: Option<i16> = 77,
        /// The isometric snap plane.
        snap_isopair: Option<i16> = 78,
    }
}

fields! {
    /// Registered application data (none beyond the name).
    pub struct AppId {
        /// Marker only.
        ["AcDbRegAppTableRecord"]
        unused: () ,
    }
}

fields! {
    /// User coordinate system data.
    pub struct Ucs {
        /// The origin.
        ["AcDbUCSTableRecord"]
        origin: P3 = 10,
        /// The x axis direction.
        x_axis: P3 = 11,
        /// The y axis direction.
        y_axis: P3 = 12,
        /// The orthographic type.
        ortho_type: Option<i16> = 79,
        /// The elevation.
        elevation: Option<f64> = 146,
        /// The base UCS handle.
        base_ucs: Option<Handle> = 346,
    }
}

fields! {
    /// Dimension style data. The dimension variables stay groups in
    /// [`TableEntry::extras`], keyed by their group codes as the format assigns them.
    pub struct DimStyle {
        /// Marker only.
        ["AcDbDimStyleTableRecord"]
        unused: (),
    }
}

fields! {
    /// Block record data.
    pub struct BlockRecord {
        /// The layout handle.
        ["AcDbBlockTableRecord"]
        layout: Option<Handle> = 340,
        /// Whether the block can be exploded.
        explodable: Option<i16> = 280,
        /// Whether the block can be scaled non-uniformly.
        scalable: Option<i16> = 281,
        /// The preview image bytes.
        preview: Vec<u8> = 310,
    }
}

impl Body for Layer {}
impl Body for Linetype {}
impl Body for TextStyle {}
impl Body for View {}
impl Body for VPort {}
impl Body for AppId {}
impl Body for Ucs {}
impl Body for DimStyle {}
impl Body for BlockRecord {}

/// A block definition: its BLOCK record, its entities and its ENDBLK record.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The BLOCK record, of kind [`EntityKind::Block`].
    pub begin: Entity,
    /// The entities of the block.
    pub entities: Vec<Entity>,
    /// The ENDBLK record, of kind [`EntityKind::BlockEnd`].
    pub end: Entity,
}

fields! {
    /// The properties every entity shares.
    pub struct EntityProps {
        /// Whether the entity is in paper space (group 67).
        paper_space: bool = 67,
        /// The layout tab name.
        layout: Option<String> = 410,
        /// The layer name.
        layer: String = 8,
        /// The linetype name; absent means by layer.
        linetype: Option<String> = 6,
        /// The color number (ACI); absent means by layer.
        color: Option<i16> = 62,
        /// The true color as 0x00RRGGBB.
        true_color: Option<i32> = 420,
        /// The color name from a color book.
        color_name: Option<String> = 430,
        /// The lineweight in hundredths of a millimeter, or a negative by-layer or by-block code.
        lineweight: Option<i16> = 370,
        /// The transparency.
        transparency: Option<i32> = 440,
        /// The linetype scale.
        linetype_scale: Option<f64> = 48,
        /// Whether the entity is invisible (group 60).
        invisible: bool = 60,
        /// The elevation.
        elevation: Option<f64> = 38,
        /// The thickness.
        thickness: Option<f64> = 39,
        /// The extrusion direction.
        extrusion: Option<P3> = 210,
    }
}

impl EntityProps {
    /// Writes the properties of the `AcDbEntity` subclass.
    pub(crate) fn put_main(&self, out: &mut Vec<Group>) {
        if self.paper_space {
            self.paper_space.put(67, out);
        }
        self.layout.put(410, out);
        self.layer.put(8, out);
        self.linetype.put(6, out);
        self.color.put(62, out);
        self.true_color.put(420, out);
        self.color_name.put(430, out);
        self.lineweight.put(370, out);
        self.transparency.put(440, out);
        self.linetype_scale.put(48, out);
        if self.invisible {
            self.invisible.put(60, out);
        }
    }

    /// Writes the properties stored with the entity's own subclass.
    pub(crate) fn put_tail(&self, out: &mut Vec<Group>) {
        self.elevation.put(38, out);
        self.thickness.put(39, out);
        self.extrusion.put(210, out);
    }
}

/// An entity: its head, shared properties, kind, preserved groups and extended data. For
/// [`EntityKind::Preserved`] every group is in the preserved record and the other fields are
/// empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    /// The head.
    pub head: Head,
    /// The shared properties.
    pub props: EntityProps,
    /// The kind with its own data.
    pub kind: EntityKind,
    /// Groups the model does not type, written after the typed groups.
    pub extras: Vec<Group>,
    /// Extended data.
    pub xdata: Vec<XData>,
}

impl Entity {
    /// An entity of the given kind with default head and properties on layer `0`.
    #[must_use]
    pub fn new(kind: EntityKind) -> Self {
        Self {
            head: Head::default(),
            props: EntityProps {
                layer: "0".to_owned(),
                ..EntityProps::default()
            },
            kind,
            extras: Vec::new(),
            xdata: Vec::new(),
        }
    }

    /// The source identifier: the handle of the entity.
    #[must_use]
    pub fn source_id(&self) -> Option<Handle> {
        match &self.kind {
            EntityKind::Preserved(record) => record.handle(),
            _ => self.head.handle,
        }
    }
}

macro_rules! kinds {
    (
        $(#[$em:meta])*
        pub enum $enum:ident {
            $( $(#[$vm:meta])* $v:ident($t:ty) = $name:literal ),* $(,)?
        }
    ) => {
        $(#[$em])*
        #[derive(Debug, Clone, PartialEq)]
        pub enum $enum {
            $( $(#[$vm])* $v($t), )*
            /// A type the model does not type, kept as a preserved record.
            Preserved(PreservedRecord),
        }

        impl $enum {
            pub(crate) fn from_type(name: &str) -> Option<Self> {
                match name {
                    $( $name => Some(Self::$v(<$t>::default())), )*
                    _ => None,
                }
            }

            /// The record type written as group 0.
            #[must_use]
            pub fn type_name(&self) -> &str {
                match self {
                    $( Self::$v(_) => $name, )*
                    Self::Preserved(record) => &record.kind,
                }
            }

            pub(crate) fn body(&mut self) -> Option<&mut dyn Body> {
                match self {
                    $( Self::$v(b) => Some(b), )*
                    Self::Preserved(_) => None,
                }
            }

            pub(crate) fn body_ref(&self) -> Option<&dyn Body> {
                match self {
                    $( Self::$v(b) => Some(b), )*
                    Self::Preserved(_) => None,
                }
            }
        }
    };
}

kinds! {
    /// The entity kinds the model types.
    pub enum EntityKind {
        /// A point.
        Point(Point) = "POINT",
        /// A line segment.
        Line(Line) = "LINE",
        /// A ray.
        Ray(Ray) = "RAY",
        /// A construction line, infinite in both directions.
        ConstructionLine(Ray) = "XLINE",
        /// A circle.
        Circle(Circle) = "CIRCLE",
        /// A circular arc.
        Arc(Arc) = "ARC",
        /// An ellipse or elliptical arc.
        Ellipse(Ellipse) = "ELLIPSE",
        /// A lightweight polyline.
        LwPolyline(LwPolyline) = "LWPOLYLINE",
        /// A classic polyline with vertex records.
        Polyline(Polyline) = "POLYLINE",
        /// A vertex of a classic polyline.
        Vertex(Vertex) = "VERTEX",
        /// The end of a vertex or attribute sequence.
        SeqEnd(SeqEnd) = "SEQEND",
        /// A spline.
        Spline(Spline) = "SPLINE",
        /// Single-line text.
        Text(Text) = "TEXT",
        /// Multi-line text.
        MText(MText) = "MTEXT",
        /// An attribute of a block reference.
        Attrib(Attrib) = "ATTRIB",
        /// An attribute definition of a block.
        AttDef(AttDef) = "ATTDEF",
        /// A block reference.
        Insert(Insert) = "INSERT",
        /// A dimension.
        Dimension(Dimension) = "DIMENSION",
        /// A leader.
        Leader(Leader) = "LEADER",
        /// A hatch.
        Hatch(Hatch) = "HATCH",
        /// A filled quadrilateral.
        Solid(Quad) = "SOLID",
        /// A trace.
        Trace(Quad) = "TRACE",
        /// A three-dimensional face.
        Face(Quad) = "3DFACE",
        /// A paper-space viewport.
        Viewport(Viewport) = "VIEWPORT",
        /// A raster image reference.
        Image(Image) = "IMAGE",
        /// A wipeout.
        Wipeout(Image) = "WIPEOUT",
        /// The start of a block definition.
        Block(BlockBegin) = "BLOCK",
        /// The end of a block definition.
        BlockEnd(SeqEnd) = "ENDBLK",
    }
}

impl EntityKind {
    /// The subclass markers written before the kind's fields, beyond what the fields carry.
    pub(crate) fn lead_markers(&self) -> &'static [&'static str] {
        match self {
            Self::BlockEnd(_) => &["AcDbBlockEnd"],
            Self::Solid(_) | Self::Trace(_) => &["AcDbTrace"],
            Self::Face(_) => &["AcDbFace"],
            Self::Wipeout(_) => &["AcDbRasterImage", "AcDbWipeout"],
            Self::Image(_) => &["AcDbRasterImage"],
            Self::Ray(_) => &["AcDbRay"],
            Self::ConstructionLine(_) => &["AcDbXline"],
            _ => &[],
        }
    }
}

fields! {
    /// A point.
    pub struct Point {
        /// The location.
        ["AcDbPoint"]
        location: P3 = 10,
        /// The x-axis angle of the display.
        angle: Option<f64> = 50,
    }
}

fields! {
    /// A line segment.
    pub struct Line {
        /// The start point.
        ["AcDbLine"]
        start: P3 = 10,
        /// The end point.
        end: P3 = 11,
    }
}

fields! {
    /// A ray or construction line.
    pub struct Ray {
        /// The start point.
        start: P3 = 10,
        /// The unit direction.
        direction: P3 = 11,
    }
}

fields! {
    /// A circle.
    pub struct Circle {
        /// The center.
        ["AcDbCircle"]
        center: P3 = 10,
        /// The radius.
        radius: f64 = 40,
    }
}

fields! {
    /// A circular arc, counterclockwise from start to end angle.
    pub struct Arc {
        /// The center.
        ["AcDbCircle"]
        center: P3 = 10,
        /// The radius.
        radius: f64 = 40,
        /// The start angle.
        ["AcDbArc"]
        start_angle: f64 = 50,
        /// The end angle.
        end_angle: f64 = 51,
    }
}

fields! {
    /// An ellipse or elliptical arc.
    pub struct Ellipse {
        /// The center.
        ["AcDbEllipse"]
        center: P3 = 10,
        /// The major axis endpoint relative to the center.
        major_axis: P3 = 11,
        /// The minor-to-major axis ratio.
        ratio: f64 = 40,
        /// The start parameter.
        start_param: f64 = 41,
        /// The end parameter.
        end_param: f64 = 42,
    }
}

fields! {
    /// A vertex of a lightweight polyline.
    pub struct LwVertex {
        /// The location.
        location: P2 = 10,
        /// The vertex identifier.
        id: Option<i32> = 91,
        /// The start width.
        start_width: Option<f64> = 40,
        /// The end width.
        end_width: Option<f64> = 41,
        /// The bulge of the segment that starts here.
        bulge: Option<f64> = 42,
    }
}

impl Field for Vec<LwVertex> {
    fn take(&mut self, code: i16, g: &Group) -> bool {
        if g.code == code {
            let mut v = LwVertex::default();
            v.location.take(code, g);
            self.push(v);
            return true;
        }
        self.last_mut()
            .is_some_and(|v| crate::fields::Fields::take_field(v, g))
    }
    fn put(&self, _code: i16, out: &mut Vec<Group>) {
        for v in self {
            crate::fields::Fields::put_fields(v, out);
        }
    }
}

fields! {
    /// A lightweight polyline.
    pub struct LwPolyline {
        /// The flags (1 closed, 128 plinegen).
        ["AcDbPolyline"]
        flags: i16 = 70,
        /// The constant width.
        constant_width: Option<f64> = 43,
        /// The vertices.
        vertices: Vec<LwVertex> = 10 (count 90),
    }
}

fields! {
    /// A classic polyline; its vertices and end marker are separate records.
    pub struct Polyline {
        /// The dummy point that carries the elevation.
        origin: Option<P3> = 10,
        /// The flags.
        flags: i16 = 70,
        /// The default start width.
        start_width: Option<f64> = 40,
        /// The default end width.
        end_width: Option<f64> = 41,
        /// The mesh vertex count in M, or the polyface vertex count.
        mesh_m: Option<i16> = 71,
        /// The mesh vertex count in N, or the polyface face count.
        mesh_n: Option<i16> = 72,
        /// The smooth surface density in M.
        smooth_m: Option<i16> = 73,
        /// The smooth surface density in N.
        smooth_n: Option<i16> = 74,
        /// The curve or surface type.
        smooth_type: Option<i16> = 75,
        /// The VERTEX records.
        vertices: Vec<Entity>,
        /// The SEQEND record; the writer writes a default one when absent.
        seqend: Option<Box<Entity>>,
    }
}

impl Polyline {
    fn marker(&self) -> &'static str {
        match self.flags {
            f if f & 64 != 0 => "AcDbPolyFaceMesh",
            f if f & 16 != 0 => "AcDbPolygonMesh",
            f if f & 8 != 0 => "AcDb3dPolyline",
            _ => "AcDb2dPolyline",
        }
    }
}

fields! {
    /// A vertex of a classic polyline.
    pub struct Vertex {
        /// The location.
        location: P3 = 10,
        /// The start width.
        start_width: Option<f64> = 40,
        /// The end width.
        end_width: Option<f64> = 41,
        /// The bulge.
        bulge: Option<f64> = 42,
        /// The vertex flags.
        flags: i16 = 70,
        /// The curve-fit tangent direction.
        tangent: Option<f64> = 50,
        /// Polyface vertex index 1.
        index1: Option<i16> = 71,
        /// Polyface vertex index 2.
        index2: Option<i16> = 72,
        /// Polyface vertex index 3.
        index3: Option<i16> = 73,
        /// Polyface vertex index 4.
        index4: Option<i16> = 74,
        /// The vertex identifier.
        id: Option<i32> = 91,
    }
}

impl Vertex {
    fn markers(&self) -> &'static [&'static str] {
        match self.flags {
            f if f & 128 != 0 && f & 64 == 0 => &["AcDbFaceRecord"],
            f if f & 64 != 0 => &["AcDbVertex", "AcDbPolyFaceMeshVertex"],
            f if f & 32 != 0 => &["AcDbVertex", "AcDb3dPolylineVertex"],
            _ => &["AcDbVertex", "AcDb2dVertex"],
        }
    }
}

fields! {
    /// The end of a sequence or block; it has no data of its own.
    pub struct SeqEnd {
        /// Nothing.
        unused: (),
    }
}

fields! {
    /// A spline: degree, knots, weights, control points and fit points as stored.
    pub struct Spline {
        /// The flags (1 closed, 2 periodic, 4 rational, 8 planar, 16 linear).
        ["AcDbSpline"]
        flags: i16 = 70,
        /// The degree.
        degree: i16 = 71,
        /// The knot tolerance.
        knot_tolerance: Option<f64> = 42,
        /// The control-point tolerance.
        control_tolerance: Option<f64> = 43,
        /// The fit tolerance.
        fit_tolerance: Option<f64> = 44,
        /// The start tangent.
        start_tangent: Option<P3> = 12,
        /// The end tangent.
        end_tangent: Option<P3> = 13,
        /// The knot vector.
        knots: Vec<f64> = 40 (count 72),
        /// The weights of a rational spline.
        weights: Vec<f64> = 41,
        /// The control points.
        control_points: Vec<P3> = 10 (count 73),
        /// The fit points.
        fit_points: Vec<P3> = 11 (count 74),
    }
}

fields! {
    /// Single-line text.
    pub struct Text {
        /// The first alignment point.
        ["AcDbText"]
        insert: P3 = 10,
        /// The height.
        height: f64 = 40,
        /// The text as stored, including format codes.
        value: String = 1,
        /// The rotation.
        rotation: Option<f64> = 50,
        /// The width factor.
        width_factor: Option<f64> = 41,
        /// The oblique angle.
        oblique: Option<f64> = 51,
        /// The text style name.
        style: Option<String> = 7,
        /// The generation flags.
        generation: Option<i16> = 71,
        /// The horizontal justification.
        horizontal: Option<i16> = 72,
        /// The second alignment point.
        align: Option<P3> = 11,
        /// The vertical justification.
        ["AcDbText"]
        vertical: Option<i16> = 73,
    }
}

fields! {
    /// Text attribute fields shared by attributes and attribute definitions.
    pub struct Attrib {
        /// The first alignment point.
        ["AcDbText"]
        insert: P3 = 10,
        /// The height.
        height: f64 = 40,
        /// The value.
        value: String = 1,
        /// The rotation.
        rotation: Option<f64> = 50,
        /// The width factor.
        width_factor: Option<f64> = 41,
        /// The oblique angle.
        oblique: Option<f64> = 51,
        /// The text style name.
        style: Option<String> = 7,
        /// The generation flags.
        generation: Option<i16> = 71,
        /// The horizontal justification.
        horizontal: Option<i16> = 72,
        /// The second alignment point.
        align: Option<P3> = 11,
        /// The tag.
        ["AcDbAttribute"]
        tag: String = 2,
        /// The attribute flags.
        flags: i16 = 70,
        /// The field length.
        field_length: Option<i16> = 73,
        /// The vertical justification.
        vertical: Option<i16> = 74,
    }
}

fields! {
    /// An attribute definition.
    pub struct AttDef {
        /// The first alignment point.
        ["AcDbText"]
        insert: P3 = 10,
        /// The height.
        height: f64 = 40,
        /// The default value.
        value: String = 1,
        /// The rotation.
        rotation: Option<f64> = 50,
        /// The width factor.
        width_factor: Option<f64> = 41,
        /// The oblique angle.
        oblique: Option<f64> = 51,
        /// The text style name.
        style: Option<String> = 7,
        /// The generation flags.
        generation: Option<i16> = 71,
        /// The horizontal justification.
        horizontal: Option<i16> = 72,
        /// The second alignment point.
        align: Option<P3> = 11,
        /// The prompt.
        ["AcDbAttributeDefinition"]
        prompt: String = 3,
        /// The tag.
        tag: String = 2,
        /// The attribute flags.
        flags: i16 = 70,
        /// The field length.
        field_length: Option<i16> = 73,
        /// The vertical justification.
        vertical: Option<i16> = 74,
    }
}

fields! {
    /// Multi-line text; the content is kept raw, with its format codes.
    pub struct MText {
        /// The insertion point.
        ["AcDbMText"]
        insert: P3 = 10,
        /// The nominal height.
        height: f64 = 40,
        /// The reference rectangle width.
        width: Option<f64> = 41,
        /// The attachment point.
        attachment: Option<i16> = 71,
        /// The drawing direction.
        direction: Option<i16> = 72,
        /// The text style name.
        style: Option<String> = 7,
        /// The x-axis direction.
        x_axis: Option<P3> = 11,
        /// The rotation.
        rotation: Option<f64> = 50,
        /// The line spacing style.
        spacing_style: Option<i16> = 73,
        /// The line spacing factor.
        spacing_factor: Option<f64> = 44,
        /// The content, joined from its chunks (groups 3 and 1).
        text: String,
    }
}

fields! {
    /// A block reference.
    pub struct Insert {
        /// The block name.
        ["AcDbBlockReference"]
        block: String = 2,
        /// The insertion point.
        insert: P3 = 10,
        /// The x scale.
        x_scale: Option<f64> = 41,
        /// The y scale.
        y_scale: Option<f64> = 42,
        /// The z scale.
        z_scale: Option<f64> = 43,
        /// The rotation.
        rotation: Option<f64> = 50,
        /// The column count.
        columns: Option<i16> = 70,
        /// The row count.
        rows: Option<i16> = 71,
        /// The column spacing.
        column_spacing: Option<f64> = 44,
        /// The row spacing.
        row_spacing: Option<f64> = 45,
        /// The ATTRIB records.
        attribs: Vec<Entity>,
        /// The SEQEND record after the attributes.
        seqend: Option<Box<Entity>>,
    }
}

fields! {
    /// A dimension, kept as the data the format stores; the kind is the low bits of `kind`.
    pub struct Dimension {
        /// The block holding the dimension's picture.
        block: Option<String> = 2,
        /// The dimension style name.
        style: Option<String> = 3,
        /// The definition point.
        def_point: P3 = 10,
        /// The text midpoint.
        text_mid: P3 = 11,
        /// The insertion point of clones.
        insert: Option<P3> = 12,
        /// The kind (0 rotated, 1 aligned, 2 angular, 3 diameter, 4 radius, 5 three-point
        /// angular, 6 ordinate) plus flags.
        kind: i16 = 70,
        /// The text attachment point.
        attachment: Option<i16> = 71,
        /// The line spacing style.
        spacing_style: Option<i16> = 72,
        /// The line spacing factor.
        spacing_factor: Option<f64> = 41,
        /// The measurement.
        measurement: Option<f64> = 42,
        /// The text override.
        text: Option<String> = 1,
        /// The text rotation.
        text_rotation: Option<f64> = 53,
        /// The horizontal direction.
        horizontal: Option<f64> = 51,
        /// Definition point 13.
        point13: Option<P3> = 13,
        /// Definition point 14.
        point14: Option<P3> = 14,
        /// Definition point 15.
        point15: Option<P3> = 15,
        /// Definition point 16.
        point16: Option<P3> = 16,
        /// The rotation angle of a rotated dimension.
        angle: Option<f64> = 50,
        /// The oblique angle of extension lines.
        oblique: Option<f64> = 52,
        /// The leader length.
        leader_length: Option<f64> = 40,
    }
}

impl Dimension {
    fn markers(&self) -> &'static [&'static str] {
        match self.kind & 7 {
            0 => &["AcDbAlignedDimension", "AcDbRotatedDimension"],
            1 => &["AcDbAlignedDimension"],
            2 => &["AcDb2LineAngularDimension"],
            3 => &["AcDbDiametricDimension"],
            4 => &["AcDbRadialDimension"],
            5 => &["AcDb3PointAngularDimension"],
            _ => &["AcDbOrdinateDimension"],
        }
    }
}

fields! {
    /// A leader.
    pub struct Leader {
        /// The dimension style name.
        ["AcDbLeader"]
        style: String = 3,
        /// The arrowhead flag.
        arrowhead: Option<i16> = 71,
        /// The path type.
        path_type: Option<i16> = 72,
        /// The creation flag.
        creation: Option<i16> = 73,
        /// The hookline direction flag.
        hookline_direction: Option<i16> = 74,
        /// The hookline flag.
        hookline: Option<i16> = 75,
        /// The text annotation height.
        text_height: Option<f64> = 40,
        /// The text annotation width.
        text_width: Option<f64> = 41,
        /// The color of a by-block leader.
        color: Option<i16> = 77,
        /// The annotation handle.
        annotation: Option<Handle> = 340,
        /// The vertices.
        vertices: Vec<P3> = 10 (count 76),
    }
}

/// One edge of a hatch boundary loop.
#[derive(Debug, Clone, PartialEq)]
pub enum HatchEdge {
    /// A line segment.
    Line {
        /// The start point.
        start: P2,
        /// The end point.
        end: P2,
    },
    /// A circular arc.
    Arc {
        /// The center.
        center: P2,
        /// The radius.
        radius: f64,
        /// The start angle.
        start_angle: f64,
        /// The end angle.
        end_angle: f64,
        /// Whether the arc runs counterclockwise.
        counterclockwise: bool,
    },
    /// An elliptical arc.
    Ellipse {
        /// The center.
        center: P2,
        /// The major axis endpoint relative to the center.
        major_axis: P2,
        /// The minor-to-major ratio.
        ratio: f64,
        /// The start angle.
        start_angle: f64,
        /// The end angle.
        end_angle: f64,
        /// Whether the arc runs counterclockwise.
        counterclockwise: bool,
    },
    /// A spline.
    Spline(HatchSpline),
}

/// A spline edge of a hatch boundary.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HatchSpline {
    /// The degree.
    pub degree: i32,
    /// Whether the spline is rational.
    pub rational: bool,
    /// Whether the spline is periodic.
    pub periodic: bool,
    /// The knots.
    pub knots: Vec<f64>,
    /// The control points.
    pub control_points: Vec<P2>,
    /// The weights, one per control point of a rational spline.
    pub weights: Vec<f64>,
    /// The fit points (written for version 2010 and later).
    pub fit_points: Vec<P2>,
    /// The start tangent (written for version 2010 and later).
    pub start_tangent: Option<P2>,
    /// The end tangent (written for version 2010 and later).
    pub end_tangent: Option<P2>,
}

/// A vertex of a polyline hatch boundary.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HatchVertex {
    /// The location.
    pub location: P2,
    /// The bulge; zero when the boundary has no bulges.
    pub bulge: f64,
}

/// The geometry of a hatch boundary loop.
#[derive(Debug, Clone, PartialEq)]
pub enum HatchBoundary {
    /// A polyline loop.
    Polyline {
        /// Whether bulges are stored.
        has_bulge: bool,
        /// Whether the polyline is closed.
        closed: bool,
        /// The vertices.
        vertices: Vec<HatchVertex>,
    },
    /// A loop of edges.
    Edges(Vec<HatchEdge>),
}

/// A hatch boundary loop.
#[derive(Debug, Clone, PartialEq)]
pub struct HatchLoop {
    /// The boundary path flags (2 marks a polyline loop and is kept consistent by the writer).
    pub flags: i32,
    /// The geometry.
    pub boundary: HatchBoundary,
    /// Handles of the source boundary objects.
    pub sources: Vec<Handle>,
}

/// One line of a hatch pattern.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PatternLine {
    /// The line angle.
    pub angle: f64,
    /// The base point.
    pub base: P2,
    /// The offset between repeated lines.
    pub offset: P2,
    /// The dash lengths.
    pub dashes: Vec<f64>,
}

fields! {
    /// A hatch: boundary loops and pattern data.
    pub struct Hatch {
        /// The elevation point (x and y are zero).
        elevation_point: P3 = 10,
        /// The pattern name.
        pattern_name: String = 2,
        /// Whether the fill is solid.
        solid: i16 = 70,
        /// Whether the hatch is associative.
        associative: i16 = 71,
        /// The hatch style.
        style: Option<i16> = 75,
        /// The pattern type.
        pattern_type: Option<i16> = 76,
        /// The pattern angle.
        pattern_angle: Option<f64> = 52,
        /// The pattern scale or spacing.
        pattern_scale: Option<f64> = 41,
        /// Whether the pattern is doubled.
        pattern_double: Option<i16> = 77,
        /// The pixel size.
        pixel_size: Option<f64> = 47,
        /// The boundary loops.
        loops: Vec<HatchLoop>,
        /// The pattern lines.
        pattern_lines: Vec<PatternLine>,
        /// The seed points.
        seeds: Vec<P2>,
    }
}

fields! {
    /// A solid, trace or three-dimensional face: four corners.
    pub struct Quad {
        /// The first corner.
        p1: P3 = 10,
        /// The second corner.
        p2: P3 = 11,
        /// The third corner.
        p3: P3 = 12,
        /// The fourth corner.
        p4: P3 = 13,
        /// The invisible-edge flags of a face.
        edge_flags: Option<i16> = 70,
    }
}

fields! {
    /// A paper-space viewport.
    pub struct Viewport {
        /// The center in paper space.
        ["AcDbViewport"]
        center: P3 = 10,
        /// The width in paper space.
        width: f64 = 40,
        /// The height in paper space.
        height: f64 = 41,
        /// The status.
        status: Option<i16> = 68,
        /// The identifier.
        id: Option<i16> = 69,
        /// The view center.
        view_center: Option<P2> = 12,
        /// The snap base.
        snap_base: Option<P2> = 13,
        /// The snap spacing.
        snap_spacing: Option<P2> = 14,
        /// The grid spacing.
        grid_spacing: Option<P2> = 15,
        /// The view direction.
        view_direction: Option<P3> = 16,
        /// The view target.
        view_target: Option<P3> = 17,
        /// The lens length.
        lens: Option<f64> = 42,
        /// The front clipping offset.
        front_clip: Option<f64> = 43,
        /// The back clipping offset.
        back_clip: Option<f64> = 44,
        /// The view height in model space.
        view_height: Option<f64> = 45,
        /// The snap angle.
        snap_angle: Option<f64> = 50,
        /// The twist angle.
        twist: Option<f64> = 51,
        /// The status flags.
        flags: Option<i32> = 90,
        /// Handles of layers frozen in this viewport.
        frozen_layers: Vec<Handle> = 331,
    }
}

fields! {
    /// A raster image reference or wipeout.
    pub struct Image {
        /// The class version.
        class_version: Option<i32> = 90,
        /// The insertion point.
        insert: P3 = 10,
        /// The pixel u vector.
        u_vector: P3 = 11,
        /// The pixel v vector.
        v_vector: P3 = 12,
        /// The image size in pixels.
        size: P2 = 13,
        /// The image definition handle.
        definition: Option<Handle> = 340,
        /// The display flags.
        display: Option<i16> = 70,
        /// Whether clipping is on.
        clipping: Option<i16> = 280,
        /// The brightness.
        brightness: Option<i16> = 281,
        /// The contrast.
        contrast: Option<i16> = 282,
        /// The fade.
        fade: Option<i16> = 283,
        /// The definition reactor handle.
        reactor: Option<Handle> = 360,
        /// The clip boundary type.
        clip_type: Option<i16> = 71,
        /// The clip boundary vertices.
        clip_vertices: Vec<P2> = 14 (count 91),
    }
}

fields! {
    /// The data of a BLOCK record.
    pub struct BlockBegin {
        /// The block name.
        ["AcDbBlockBegin"]
        name: String = 2,
        /// The block type flags.
        flags: i16 = 70,
        /// The base point.
        base: P3 = 10,
        /// The block name repeated (group 3).
        alt_name: Option<String> = 3,
        /// The external reference path.
        xref_path: Option<String> = 1,
        /// The description.
        description: Option<String> = 4,
    }
}

macro_rules! plain_bodies {
    ($($t:ty),*) => { $( impl Body for $t {} )* };
}
plain_bodies!(
    Point, Line, Ray, Circle, Arc, Ellipse, LwPolyline, Spline, Text, Attrib, AttDef, Leader, Quad,
    Viewport, Image, BlockBegin, SeqEnd
);

fn take_66(g: &Group) -> bool {
    g.code == 66
}

impl Body for Polyline {
    fn take_more(&mut self, g: &Group, _: &mut Cursor<'_>, _: &Cx<'_>, _: &str) -> Result<bool> {
        Ok(take_66(g))
    }
    fn put(&self, out: &mut Vec<Group>, _: &mut WriteCx<'_>) -> Result<()> {
        out.push(Group::marker(self.marker()));
        out.push(Group::int(66, 1));
        crate::fields::Fields::put_fields(self, out);
        Ok(())
    }
}

impl Body for Insert {
    fn take_more(&mut self, g: &Group, _: &mut Cursor<'_>, _: &Cx<'_>, _: &str) -> Result<bool> {
        Ok(take_66(g))
    }
    fn put(&self, out: &mut Vec<Group>, _: &mut WriteCx<'_>) -> Result<()> {
        if !self.attribs.is_empty() || self.seqend.is_some() {
            out.push(Group::int(66, 1));
        }
        crate::fields::Fields::put_fields(self, out);
        Ok(())
    }
}

impl Body for Vertex {
    fn put(&self, out: &mut Vec<Group>, _: &mut WriteCx<'_>) -> Result<()> {
        out.extend(self.markers().iter().map(|m| Group::marker(m)));
        crate::fields::Fields::put_fields(self, out);
        Ok(())
    }
}

impl Body for Dimension {
    fn put(&self, out: &mut Vec<Group>, _: &mut WriteCx<'_>) -> Result<()> {
        out.push(Group::marker("AcDbDimension"));
        self.block.put(2, out);
        self.def_point.put(10, out);
        self.text_mid.put(11, out);
        self.insert.put(12, out);
        self.kind.put(70, out);
        self.attachment.put(71, out);
        self.spacing_style.put(72, out);
        self.spacing_factor.put(41, out);
        self.measurement.put(42, out);
        self.text.put(1, out);
        self.text_rotation.put(53, out);
        self.horizontal.put(51, out);
        self.style.put(3, out);
        out.extend(self.markers().iter().map(|m| Group::marker(m)));
        self.point13.put(13, out);
        self.point14.put(14, out);
        self.point15.put(15, out);
        self.point16.put(16, out);
        self.angle.put(50, out);
        self.oblique.put(52, out);
        self.leader_length.put(40, out);
        Ok(())
    }
}

impl Body for MText {
    fn take_more(&mut self, g: &Group, _: &mut Cursor<'_>, cx: &Cx<'_>, _: &str) -> Result<bool> {
        if g.code != 1 && g.code != 3 {
            return Ok(false);
        }
        let Value::Text(chunk) = &g.value else {
            return Ok(false);
        };
        let total = self.text.len().saturating_add(chunk.len());
        if u64::try_from(total).unwrap_or(u64::MAX) > cx.limits.string_bytes {
            return Err(crate::error::limit(
                "string_bytes",
                u64::try_from(total).unwrap_or(u64::MAX),
            ));
        }
        self.text.push_str(chunk);
        Ok(true)
    }
    fn put(&self, out: &mut Vec<Group>, _: &mut WriteCx<'_>) -> Result<()> {
        crate::fields::Fields::put_fields(self, out);
        let chars: Vec<char> = self.text.chars().collect();
        let mut chunks = chars.chunks(250).peekable();
        if chunks.peek().is_none() {
            out.push(Group::text(1, ""));
        }
        while let Some(chunk) = chunks.next() {
            let code = if chunks.peek().is_some() { 3 } else { 1 };
            out.push(Group::text(code, chunk.iter().collect::<String>()));
        }
        Ok(())
    }
}

/// Reads the next group of a hatch and requires its code.
fn expect<'a>(cur: &mut Cursor<'a>, code: i16, at: Position) -> Result<&'a Group> {
    match cur.next() {
        Some(g) if g.code == code => Ok(g),
        _ => Err(at.error(codes::DXF_STRUCTURE_INVALID)),
    }
}

fn number(cur: &mut Cursor<'_>, code: i16, at: Position) -> Result<f64> {
    match expect(cur, code, at)?.value {
        Value::Double(v) => Ok(v),
        _ => Err(at.error(codes::DXF_STRUCTURE_INVALID)),
    }
}

fn integer(cur: &mut Cursor<'_>, code: i16, at: Position) -> Result<i64> {
    match expect(cur, code, at)?.value {
        Value::Int(v) => Ok(v),
        Value::Bool(b) => Ok(i64::from(b)),
        _ => Err(at.error(codes::DXF_STRUCTURE_INVALID)),
    }
}

fn point2(cur: &mut Cursor<'_>, code: i16, at: Position) -> Result<P2> {
    Ok([number(cur, code, at)?, number(cur, code + 10, at)?])
}

/// Reads a list length and checks it against the array limit before any element is read.
fn length(cur: &mut Cursor<'_>, code: i16, cx: &Cx<'_>) -> Result<usize> {
    let n = integer(cur, code, cx.at)?;
    let n = u64::try_from(n).map_err(|_| cx.at.error(codes::DXF_VALUE_INVALID))?;
    if n > cx.limits.array_length {
        return Err(crate::error::limit("array_length", n));
    }
    usize::try_from(n).map_err(|_| crate::error::limit("array_length", n))
}

fn read_edge(cur: &mut Cursor<'_>, cx: &Cx<'_>) -> Result<HatchEdge> {
    let at = cx.at;
    Ok(match integer(cur, 72, at)? {
        1 => HatchEdge::Line {
            start: point2(cur, 10, at)?,
            end: point2(cur, 11, at)?,
        },
        2 => HatchEdge::Arc {
            center: point2(cur, 10, at)?,
            radius: number(cur, 40, at)?,
            start_angle: number(cur, 50, at)?,
            end_angle: number(cur, 51, at)?,
            counterclockwise: integer(cur, 73, at)? != 0,
        },
        3 => HatchEdge::Ellipse {
            center: point2(cur, 10, at)?,
            major_axis: point2(cur, 11, at)?,
            ratio: number(cur, 40, at)?,
            start_angle: number(cur, 50, at)?,
            end_angle: number(cur, 51, at)?,
            counterclockwise: integer(cur, 73, at)? != 0,
        },
        4 => {
            let degree = i32::try_from(integer(cur, 94, at)?)
                .map_err(|_| at.error(codes::DXF_VALUE_INVALID))?;
            let rational = integer(cur, 73, at)? != 0;
            let periodic = integer(cur, 74, at)? != 0;
            let knot_count = length(cur, 95, cx)?;
            let control_count = length(cur, 96, cx)?;
            let mut s = HatchSpline {
                degree,
                rational,
                periodic,
                ..HatchSpline::default()
            };
            for _ in 0..knot_count {
                s.knots.push(number(cur, 40, at)?);
            }
            for _ in 0..control_count {
                s.control_points.push(point2(cur, 10, at)?);
                if rational {
                    s.weights.push(number(cur, 42, at)?);
                }
            }
            if cx.version >= Version::R2010 {
                let fit_count = length(cur, 97, cx)?;
                for _ in 0..fit_count {
                    s.fit_points.push(point2(cur, 11, at)?);
                }
                if cur.peek().is_some_and(|g| g.code == 12) {
                    s.start_tangent = Some(point2(cur, 12, at)?);
                }
                if cur.peek().is_some_and(|g| g.code == 13) {
                    s.end_tangent = Some(point2(cur, 13, at)?);
                }
            }
            HatchEdge::Spline(s)
        }
        _ => return Err(at.error(codes::DXF_STRUCTURE_INVALID)),
    })
}

fn read_loop(cur: &mut Cursor<'_>, cx: &Cx<'_>) -> Result<HatchLoop> {
    let at = cx.at;
    let flags =
        i32::try_from(integer(cur, 92, at)?).map_err(|_| at.error(codes::DXF_VALUE_INVALID))?;
    let boundary = if flags & 2 != 0 {
        let has_bulge = integer(cur, 72, at)? != 0;
        let closed = integer(cur, 73, at)? != 0;
        let n = length(cur, 93, cx)?;
        let mut vertices = Vec::new();
        for _ in 0..n {
            let location = point2(cur, 10, at)?;
            let bulge = if has_bulge { number(cur, 42, at)? } else { 0.0 };
            vertices.push(HatchVertex { location, bulge });
        }
        HatchBoundary::Polyline {
            has_bulge,
            closed,
            vertices,
        }
    } else {
        let n = length(cur, 93, cx)?;
        let mut edges = Vec::new();
        for _ in 0..n {
            edges.push(read_edge(cur, cx)?);
        }
        HatchBoundary::Edges(edges)
    };
    let n = length(cur, 97, cx)?;
    let mut sources = Vec::new();
    for _ in 0..n {
        let h = expect(cur, 330, at)?
            .as_handle()
            .ok_or_else(|| at.error(codes::DXF_VALUE_INVALID))?;
        sources.push(h);
    }
    Ok(HatchLoop {
        flags,
        boundary,
        sources,
    })
}

impl Body for Hatch {
    fn take_more(&mut self, g: &Group, cur: &mut Cursor<'_>, cx: &Cx<'_>, _: &str) -> Result<bool> {
        let n = match (g.code, &g.value) {
            (91 | 78 | 98, Value::Int(n)) => {
                let n = u64::try_from(*n).map_err(|_| cx.at.error(codes::DXF_VALUE_INVALID))?;
                if n > cx.limits.array_length {
                    return Err(crate::error::limit("array_length", n));
                }
                n
            }
            _ => return Ok(false),
        };
        for _ in 0..n {
            match g.code {
                91 => {
                    let l = read_loop(cur, cx)?;
                    self.loops.push(l);
                }
                78 => {
                    let mut line = PatternLine {
                        angle: number(cur, 53, cx.at)?,
                        base: [number(cur, 43, cx.at)?, number(cur, 44, cx.at)?],
                        offset: [number(cur, 45, cx.at)?, number(cur, 46, cx.at)?],
                        dashes: Vec::new(),
                    };
                    for _ in 0..length(cur, 79, cx)? {
                        line.dashes.push(number(cur, 49, cx.at)?);
                    }
                    self.pattern_lines.push(line);
                }
                _ => self.seeds.push(point2(cur, 10, cx.at)?),
            }
        }
        Ok(true)
    }

    fn put(&self, out: &mut Vec<Group>, w: &mut WriteCx<'_>) -> Result<()> {
        out.push(Group::marker("AcDbHatch"));
        self.elevation_point.put(10, out);
        self.pattern_name.put(2, out);
        self.solid.put(70, out);
        self.associative.put(71, out);
        out.push(count(91, self.loops.len()));
        for l in &self.loops {
            put_loop(l, out, w);
        }
        self.style.put(75, out);
        self.pattern_type.put(76, out);
        self.pattern_angle.put(52, out);
        self.pattern_scale.put(41, out);
        self.pattern_double.put(77, out);
        if !self.pattern_lines.is_empty() {
            out.push(count(78, self.pattern_lines.len()));
        }
        for line in &self.pattern_lines {
            line.angle.put(53, out);
            line.base[0].put(43, out);
            line.base[1].put(44, out);
            line.offset[0].put(45, out);
            line.offset[1].put(46, out);
            out.push(count(79, line.dashes.len()));
            line.dashes.put(49, out);
        }
        self.pixel_size.put(47, out);
        if !self.seeds.is_empty() {
            out.push(count(98, self.seeds.len()));
            self.seeds.put(10, out);
        }
        Ok(())
    }
}

fn put_loop(l: &HatchLoop, out: &mut Vec<Group>, w: &mut WriteCx<'_>) {
    let polyline = matches!(l.boundary, HatchBoundary::Polyline { .. });
    let flags = if polyline { l.flags | 2 } else { l.flags & !2 };
    out.push(Group::int(92, i64::from(flags)));
    match &l.boundary {
        HatchBoundary::Polyline {
            has_bulge,
            closed,
            vertices,
        } => {
            out.push(Group::int(72, i64::from(*has_bulge)));
            out.push(Group::int(73, i64::from(*closed)));
            out.push(count(93, vertices.len()));
            for v in vertices {
                v.location.put(10, out);
                if *has_bulge {
                    v.bulge.put(42, out);
                }
            }
        }
        HatchBoundary::Edges(edges) => {
            out.push(count(93, edges.len()));
            for e in edges {
                put_edge(e, out, w);
            }
        }
    }
    out.push(count(97, l.sources.len()));
    l.sources.put(330, out);
}

fn put_edge(e: &HatchEdge, out: &mut Vec<Group>, w: &mut WriteCx<'_>) {
    match e {
        HatchEdge::Line { start, end } => {
            out.push(Group::int(72, 1));
            start.put(10, out);
            end.put(11, out);
        }
        HatchEdge::Arc {
            center,
            radius,
            start_angle,
            end_angle,
            counterclockwise,
        } => {
            out.push(Group::int(72, 2));
            center.put(10, out);
            radius.put(40, out);
            start_angle.put(50, out);
            end_angle.put(51, out);
            out.push(Group::int(73, i64::from(*counterclockwise)));
        }
        HatchEdge::Ellipse {
            center,
            major_axis,
            ratio,
            start_angle,
            end_angle,
            counterclockwise,
        } => {
            out.push(Group::int(72, 3));
            center.put(10, out);
            major_axis.put(11, out);
            ratio.put(40, out);
            start_angle.put(50, out);
            end_angle.put(51, out);
            out.push(Group::int(73, i64::from(*counterclockwise)));
        }
        HatchEdge::Spline(s) => {
            out.push(Group::int(72, 4));
            out.push(Group::int(94, i64::from(s.degree)));
            out.push(Group::int(73, i64::from(s.rational)));
            out.push(Group::int(74, i64::from(s.periodic)));
            out.push(count(95, s.knots.len()));
            out.push(count(96, s.control_points.len()));
            s.knots.put(40, out);
            for (i, p) in s.control_points.iter().enumerate() {
                p.put(10, out);
                if s.rational {
                    s.weights.get(i).copied().unwrap_or(1.0).put(42, out);
                }
            }
            if w.version >= Version::R2010 {
                out.push(count(97, s.fit_points.len()));
                s.fit_points.put(11, out);
                s.start_tangent.put(12, out);
                s.end_tangent.put(13, out);
            } else if !s.fit_points.is_empty()
                || s.start_tangent.is_some()
                || s.end_tangent.is_some()
            {
                w.loss
                    .add(crate::error::reasons::FIT_DATA_DROPPED, "HATCH", None, 1);
            }
        }
    }
}

/// An object of the OBJECTS section. For [`ObjectKind::Preserved`] every group is in the
/// preserved record and the other fields are empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    /// The head.
    pub head: Head,
    /// The kind with its own data.
    pub kind: ObjectKind,
    /// Groups the model does not type.
    pub extras: Vec<Group>,
    /// Extended data.
    pub xdata: Vec<XData>,
}

impl Object {
    /// An object of the given kind with an empty head.
    #[must_use]
    pub fn new(kind: ObjectKind) -> Self {
        Self {
            head: Head::default(),
            kind,
            extras: Vec::new(),
            xdata: Vec::new(),
        }
    }

    /// The source identifier: the handle of the object.
    #[must_use]
    pub fn source_id(&self) -> Option<Handle> {
        match &self.kind {
            ObjectKind::Preserved(record) => record.handle(),
            _ => self.head.handle,
        }
    }
}

kinds! {
    /// The object kinds the model types.
    pub enum ObjectKind {
        /// A dictionary.
        Dictionary(Dictionary) = "DICTIONARY",
        /// An extension record of arbitrary groups.
        XRecord(XRecord) = "XRECORD",
        /// A layout with its plot settings.
        Layout(Layout) = "LAYOUT",
        /// A named group of entities.
        Group(GroupObject) = "GROUP",
    }
}

/// One entry of a dictionary.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DictionaryEntry {
    /// The entry name (group 3).
    pub name: String,
    /// The handle of the entry's object.
    pub handle: Handle,
    /// Whether the reference owns the object (group 360 rather than 350).
    pub hard_owner: bool,
}

impl Field for Vec<DictionaryEntry> {
    fn take(&mut self, code: i16, g: &Group) -> bool {
        match (g.code, &g.value) {
            (c, Value::Text(name)) if c == code => {
                self.push(DictionaryEntry {
                    name: name.clone(),
                    ..DictionaryEntry::default()
                });
                true
            }
            (350 | 360, _) => match (self.last_mut(), g.as_handle()) {
                (Some(entry), Some(h)) => {
                    entry.handle = h;
                    entry.hard_owner = g.code == 360;
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
    fn put(&self, code: i16, out: &mut Vec<Group>) {
        for e in self {
            out.push(Group::text(code, e.name.clone()));
            out.push(Group::text(
                if e.hard_owner { 360 } else { 350 },
                e.handle.to_hex(),
            ));
        }
    }
}

fields! {
    /// A dictionary.
    pub struct Dictionary {
        /// The hard-owner flag.
        ["AcDbDictionary"]
        hard_owner: Option<i16> = 280,
        /// The duplicate-record cloning flag.
        cloning: Option<i16> = 281,
        /// The entries.
        entries: Vec<DictionaryEntry> = 3,
    }
}

fields! {
    /// An extension record.
    pub struct XRecord {
        /// The duplicate-record cloning flag.
        ["AcDbXrecord"]
        cloning: Option<i16> = 280,
        /// The data groups, in order.
        data: Vec<Group>,
    }
}

fields! {
    /// A layout. Its plot settings stay groups; the layout's own data is typed.
    pub struct Layout {
        /// The groups of the plot settings subclass.
        plot_settings: Vec<Group>,
        /// The layout name.
        name: String = 1,
        /// The layout flags.
        flags: i16 = 70,
        /// The tab order.
        tab_order: i16 = 71,
        /// The block record of the layout's space.
        block_record: Option<Handle> = 330,
        /// The last active viewport.
        viewport: Option<Handle> = 331,
    }
}

fields! {
    /// A named group of entities.
    pub struct GroupObject {
        /// The description.
        ["AcDbGroup"]
        description: String = 300,
        /// Whether the group is unnamed.
        unnamed: i16 = 70,
        /// Whether the group is selectable.
        selectable: i16 = 71,
        /// The member entity handles.
        entities: Vec<Handle> = 340,
    }
}

impl Body for Dictionary {}
impl Body for GroupObject {}

impl Body for XRecord {
    fn take_more(&mut self, g: &Group, _: &mut Cursor<'_>, _: &Cx<'_>, sub: &str) -> Result<bool> {
        if sub != "AcDbXrecord" {
            return Ok(false);
        }
        if g.code == 280 && self.cloning.is_none() && self.data.is_empty() {
            return Ok(crate::fields::Fields::take_field(self, g));
        }
        self.data.push(g.clone());
        Ok(true)
    }
    fn put(&self, out: &mut Vec<Group>, _: &mut WriteCx<'_>) -> Result<()> {
        crate::fields::Fields::put_fields(self, out);
        out.extend(self.data.iter().cloned());
        Ok(())
    }
}

impl Body for Layout {
    fn take_more(&mut self, g: &Group, _: &mut Cursor<'_>, _: &Cx<'_>, sub: &str) -> Result<bool> {
        if sub == "AcDbPlotSettings" {
            self.plot_settings.push(g.clone());
            return Ok(true);
        }
        Ok(false)
    }
    fn put(&self, out: &mut Vec<Group>, _: &mut WriteCx<'_>) -> Result<()> {
        out.push(Group::marker("AcDbPlotSettings"));
        out.extend(self.plot_settings.iter().cloned());
        out.push(Group::marker("AcDbLayout"));
        crate::fields::Fields::put_fields(self, out);
        Ok(())
    }
}
