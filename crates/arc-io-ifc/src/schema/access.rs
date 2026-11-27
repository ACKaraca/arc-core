//! Attribute access by name through the tables of the file's schema: record positions over the
//! supertype chain, typed values by the declared type, and the attributes the typed views and the
//! geometry evaluator read.
//!
//! The views and the geometry evaluator name every attribute they read ([`Attr`]). Where the tables
//! of the file's schema declare the record's entity, the position comes from the tables, so an
//! attribute the schema does not declare (such as `Formula` of a quantity in IFC2X3) is absent.
//! Where they do not (a build without the generated tables, a schema other than IFC2X3, IFC4 or
//! IFC4X3, or an entity the schema does not declare) the position the attribute has in all three
//! schemas is used; a test checks every such position against the tables.

use crate::model::{IfcFile, Instance, Parameter, Record};

use super::{AggregateKind, Attribute, Named, SchemaTables, TypeRef};

/// Steps of a chain of defined types followed before a type is given up as malformed.
const MAX_TYPE_DEPTH: usize = 32;

impl SchemaTables {
    /// The record position of the explicit attribute `name` of `entity` over its supertype chain,
    /// ignoring ASCII case, with the attribute. `None` when the entity does not have it.
    #[must_use]
    pub fn attribute_position(
        &self,
        entity: u16,
        name: &str,
    ) -> Option<(usize, &'static Attribute)> {
        // Positions count from the root supertype; the walk goes from the entity up.
        let mut below = 0usize;
        let mut found = None;
        for e in self
            .supertypes(entity)
            .filter_map(|i| self.entities.get(usize::from(i)))
        {
            if found.is_none()
                && let Some((i, a)) = e
                    .attributes
                    .iter()
                    .enumerate()
                    .find(|(_, a)| a.name.eq_ignore_ascii_case(name))
            {
                found = Some((below + e.attributes.len(), i, a));
            }
            below += e.attributes.len();
        }
        let (from_declaring_down, i, attribute) = found?;
        Some((below - from_declaring_down + i, attribute))
    }

    /// The type `ty` with its defined types followed to the underlying type. A chain longer than
    /// the tables can hold is returned where it stops.
    #[must_use]
    pub fn underlying(&self, ty: TypeRef) -> TypeRef {
        let mut current = ty;
        for _ in 0..MAX_TYPE_DEPTH {
            match current {
                TypeRef::Named(Named::Defined(i)) => match self.defined_types.get(usize::from(i)) {
                    Some(d) => current = d.underlying,
                    None => return current,
                },
                _ => return current,
            }
        }
        current
    }

    /// Whether the select `select` lists `member`, directly or through a nested select.
    #[must_use]
    pub fn select_contains(&self, select: u16, member: Named) -> bool {
        self.select_walk(select, &mut |m| m == member)
    }

    /// Whether the select `select` accepts an instance of `entity`: a member is the entity or one
    /// of its supertypes, directly or through a nested select.
    #[must_use]
    pub fn select_accepts_entity(&self, select: u16, entity: u16) -> bool {
        self.select_walk(select, &mut |m| match m {
            Named::Entity(e) => self.is_subtype_of(entity, e),
            _ => false,
        })
    }

    /// Visits the members of a select and of the selects nested in it, each select once, until
    /// `hit` returns `true`.
    fn select_walk(&self, select: u16, hit: &mut dyn FnMut(Named) -> bool) -> bool {
        let mut pending = vec![select];
        let mut seen = Vec::new();
        while let Some(s) = pending.pop() {
            if seen.contains(&s) {
                continue;
            }
            seen.push(s);
            let Some(def) = self.selects.get(usize::from(s)) else {
                continue;
            };
            for &member in def.members {
                if hit(member) {
                    return true;
                }
                if let Named::Select(nested) = member {
                    pending.push(nested);
                }
            }
        }
        false
    }
}

/// A value read by the type its attribute declares.
#[derive(Debug, Clone, PartialEq)]
pub enum Typed<'a> {
    /// `INTEGER`.
    Integer(i64),
    /// `REAL` or `NUMBER`.
    Real(f64),
    /// `BOOLEAN`.
    Boolean(bool),
    /// `LOGICAL`; `None` is unknown.
    Logical(Option<bool>),
    /// `STRING`.
    String(&'a str),
    /// `BINARY`, the hexadecimal digits as written.
    Binary(&'a str),
    /// A reference to an instance of an entity.
    Entity(u64),
    /// An item of an enumeration, as written.
    Enumeration(&'a str),
    /// A member of a select written as a typed parameter (`IFCLABEL('x')`): the member type and the
    /// value read by it.
    Selected {
        /// The member type.
        member: Named,
        /// The value.
        value: Box<Typed<'a>>,
    },
    /// An aggregate, element by element.
    Aggregate(Vec<Typed<'a>>),
    /// `$`.
    Omitted,
    /// `*`.
    Derived,
}

impl SchemaTables {
    /// `parameter` read as a value of type `ty`, or `None` when it is not a value of that type.
    /// References are not followed: the referenced instance is not checked here.
    #[must_use]
    pub fn decode<'a>(&self, ty: TypeRef, parameter: &'a Parameter) -> Option<Typed<'a>> {
        self.decode_at(ty, parameter, 0)
    }

    fn decode_at<'a>(
        &self,
        ty: TypeRef,
        parameter: &'a Parameter,
        depth: usize,
    ) -> Option<Typed<'a>> {
        if depth > crate::step::MAX_NESTING as usize {
            return None;
        }
        match parameter {
            Parameter::Omitted => return Some(Typed::Omitted),
            Parameter::Derived => return Some(Typed::Derived),
            _ => {}
        }
        Some(match (self.underlying(ty), parameter) {
            (TypeRef::Integer, Parameter::Integer(v)) => Typed::Integer(*v),
            #[allow(clippy::cast_precision_loss, reason = "an integer written for a real")]
            (TypeRef::Real | TypeRef::Number, Parameter::Integer(v)) => Typed::Real(*v as f64),
            (TypeRef::Real | TypeRef::Number, Parameter::Real(v)) => Typed::Real(*v),
            (TypeRef::Boolean, Parameter::Enumeration(e)) => Typed::Boolean(logical(e)??),
            (TypeRef::Logical, Parameter::Enumeration(e)) => Typed::Logical(logical(e)?),
            (TypeRef::String { .. }, Parameter::Text(t)) => Typed::String(t),
            (TypeRef::Binary { .. }, Parameter::Binary(b)) => Typed::Binary(b),
            (TypeRef::Named(Named::Entity(_)), Parameter::Reference(r)) => Typed::Entity(*r),
            (TypeRef::Named(Named::Enumeration(i)), Parameter::Enumeration(e)) => {
                self.enumerations
                    .get(usize::from(i))?
                    .contains(e)
                    .then_some(())?;
                Typed::Enumeration(e)
            }
            (TypeRef::Named(Named::Select(_)), Parameter::Reference(r)) => Typed::Entity(*r),
            (TypeRef::Named(Named::Select(s)), Parameter::Typed { type_name, value }) => {
                let member = self.find(type_name)?;
                if matches!(member, Named::Entity(_)) || !self.select_contains(s, member) {
                    return None;
                }
                Typed::Selected {
                    member,
                    value: Box::new(self.decode_at(TypeRef::Named(member), value, depth + 1)?),
                }
            }
            (TypeRef::Aggregate(a), Parameter::List(items)) => Typed::Aggregate(
                items
                    .iter()
                    .map(|item| match item {
                        Parameter::Omitted if a.optional && a.kind == AggregateKind::Array => {
                            Some(Typed::Omitted)
                        }
                        Parameter::Omitted | Parameter::Derived => None,
                        _ => self.decode_at(a.element, item, depth + 1),
                    })
                    .collect::<Option<_>>()?,
            ),
            _ => return None,
        })
    }
}

/// `T`, `F` and `U` as a logical value, ignoring ASCII case; `None` for another item.
fn logical(item: &str) -> Option<Option<bool>> {
    match item.to_ascii_uppercase().as_str() {
        "T" => Some(Some(true)),
        "F" => Some(Some(false)),
        "U" => Some(None),
        _ => None,
    }
}

impl IfcFile {
    /// The tables of the file's schema, or `None` for another schema or a build without them.
    #[must_use]
    pub fn tables(&self) -> Option<&'static SchemaTables> {
        SchemaTables::of(&self.schema)
    }

    /// The value of the attribute `name` (ignoring ASCII case) in a record of a simple instance,
    /// at the position the tables of the file's schema give it. `None` without tables, for an
    /// entity or attribute the schema does not declare, or when the record is too short.
    #[must_use]
    pub fn attribute<'a>(&self, record: &'a Record, name: &str) -> Option<&'a Parameter> {
        let tables = self.tables()?;
        let entity = tables.entity_index(&record.type_name)?;
        let (position, _) = tables.attribute_position(entity, name)?;
        record.parameters.get(position)
    }

    /// The value of the attribute `name` of an instance. For a complex instance the attribute is
    /// read from the part whose entity declares it, where the part holds only its own attributes.
    #[must_use]
    pub fn instance_attribute<'a>(
        &self,
        instance: &'a Instance,
        name: &str,
    ) -> Option<&'a Parameter> {
        match instance.records.as_slice() {
            [record] => self.attribute(record, name),
            parts => {
                let tables = self.tables()?;
                parts.iter().find_map(|part| {
                    let entity = tables.entity(&part.type_name)?;
                    let i = entity
                        .attributes
                        .iter()
                        .position(|a| a.name.eq_ignore_ascii_case(name))?;
                    part.parameters.get(i)
                })
            }
        }
    }

    /// The attribute `name` of a record of a simple instance, read by its declared type. `None`
    /// as for [`IfcFile::attribute`], or when the value is not of the declared type.
    #[must_use]
    pub fn typed_attribute<'a>(&self, record: &'a Record, name: &str) -> Option<Typed<'a>> {
        let tables = self.tables()?;
        let entity = tables.entity_index(&record.type_name)?;
        let (position, attribute) = tables.attribute_position(entity, name)?;
        tables.decode(attribute.ty, record.parameters.get(position)?)
    }

    /// The value of an attribute the views or the geometry evaluator read: by name where the
    /// tables declare the record's entity, at the attribute's fixed position otherwise.
    pub(crate) fn attr<'a>(&self, record: &'a Record, attr: Attr) -> Option<&'a Parameter> {
        match self
            .tables()
            .and_then(|t| t.entity_index(&record.type_name).map(|e| (t, e)))
        {
            Some((tables, entity)) => tables
                .attribute_position(entity, attr.name)
                .and_then(|(position, _)| record.parameters.get(position)),
            None => record.parameters.get(attr.position),
        }
    }
}

/// An attribute the crate reads: the entity that declares it (upper case), its name, and its
/// position in a record of that entity in IFC2X3, IFC4 and IFC4X3 where the schema declares it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Attr {
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "read by the check against the tables")
    )]
    pub(crate) entity: &'static str,
    pub(crate) name: &'static str,
    pub(crate) position: usize,
}

macro_rules! attrs {
    ($($id:ident = $entity:literal . $name:literal @ $position:literal;)*) => {
        $(pub(crate) const $id: Attr = Attr { entity: $entity, name: $name, position: $position };)*
        /// Every attribute the crate reads, for the check against the tables.
        #[cfg_attr(not(test), allow(dead_code, reason = "read by the tests"))]
        pub(crate) const ALL: &[Attr] = &[$($id),*];
    };
}

/// The attributes the typed views and the geometry evaluator read.
pub(crate) mod known {
    use super::Attr;

    attrs! {
        GLOBAL_ID = "IFCROOT"."GlobalId" @ 0;
        NAME = "IFCROOT"."Name" @ 2;
        DESCRIPTION = "IFCROOT"."Description" @ 3;
        AGGREGATES_RELATING = "IFCRELAGGREGATES"."RelatingObject" @ 4;
        AGGREGATES_RELATED = "IFCRELAGGREGATES"."RelatedObjects" @ 5;
        CONTAINED_RELATED = "IFCRELCONTAINEDINSPATIALSTRUCTURE"."RelatedElements" @ 4;
        CONTAINED_RELATING = "IFCRELCONTAINEDINSPATIALSTRUCTURE"."RelatingStructure" @ 5;
        TYPE_RELATED = "IFCRELDEFINESBYTYPE"."RelatedObjects" @ 4;
        TYPE_RELATING = "IFCRELDEFINESBYTYPE"."RelatingType" @ 5;
        PROPERTIES_RELATED = "IFCRELDEFINESBYPROPERTIES"."RelatedObjects" @ 4;
        PROPERTIES_RELATING = "IFCRELDEFINESBYPROPERTIES"."RelatingPropertyDefinition" @ 5;
        PSET_PROPERTIES = "IFCPROPERTYSET"."HasProperties" @ 4;
        PROPERTY_NAME = "IFCPROPERTY"."Name" @ 0;
        PROPERTY_DESCRIPTION = "IFCPROPERTY"."Description" @ 1;
        // IFC4X3 renames the description of a property.
        PROPERTY_SPECIFICATION = "IFCPROPERTY"."Specification" @ 1;
        SINGLE_NOMINAL = "IFCPROPERTYSINGLEVALUE"."NominalValue" @ 2;
        SINGLE_UNIT = "IFCPROPERTYSINGLEVALUE"."Unit" @ 3;
        ENUMERATED_VALUES = "IFCPROPERTYENUMERATEDVALUE"."EnumerationValues" @ 2;
        ENUMERATED_REFERENCE = "IFCPROPERTYENUMERATEDVALUE"."EnumerationReference" @ 3;
        LIST_VALUES = "IFCPROPERTYLISTVALUE"."ListValues" @ 2;
        LIST_UNIT = "IFCPROPERTYLISTVALUE"."Unit" @ 3;
        BOUNDED_UPPER = "IFCPROPERTYBOUNDEDVALUE"."UpperBoundValue" @ 2;
        BOUNDED_LOWER = "IFCPROPERTYBOUNDEDVALUE"."LowerBoundValue" @ 3;
        BOUNDED_UNIT = "IFCPROPERTYBOUNDEDVALUE"."Unit" @ 4;
        BOUNDED_SET_POINT = "IFCPROPERTYBOUNDEDVALUE"."SetPointValue" @ 5;
        QUANTITIES = "IFCELEMENTQUANTITY"."Quantities" @ 5;
        QUANTITY_NAME = "IFCPHYSICALQUANTITY"."Name" @ 0;
        QUANTITY_DESCRIPTION = "IFCPHYSICALQUANTITY"."Description" @ 1;
        QUANTITY_UNIT = "IFCPHYSICALSIMPLEQUANTITY"."Unit" @ 2;
        LENGTH_VALUE = "IFCQUANTITYLENGTH"."LengthValue" @ 3;
        AREA_VALUE = "IFCQUANTITYAREA"."AreaValue" @ 3;
        VOLUME_VALUE = "IFCQUANTITYVOLUME"."VolumeValue" @ 3;
        COUNT_VALUE = "IFCQUANTITYCOUNT"."CountValue" @ 3;
        WEIGHT_VALUE = "IFCQUANTITYWEIGHT"."WeightValue" @ 3;
        TIME_VALUE = "IFCQUANTITYTIME"."TimeValue" @ 3;
        LENGTH_FORMULA = "IFCQUANTITYLENGTH"."Formula" @ 4;
        AREA_FORMULA = "IFCQUANTITYAREA"."Formula" @ 4;
        VOLUME_FORMULA = "IFCQUANTITYVOLUME"."Formula" @ 4;
        COUNT_FORMULA = "IFCQUANTITYCOUNT"."Formula" @ 4;
        WEIGHT_FORMULA = "IFCQUANTITYWEIGHT"."Formula" @ 4;
        TIME_FORMULA = "IFCQUANTITYTIME"."Formula" @ 4;
        POLYLINE_POINTS = "IFCPOLYLINE"."Points" @ 0;
        POLYCURVE_POINTS = "IFCINDEXEDPOLYCURVE"."Points" @ 0;
        POLYCURVE_SEGMENTS = "IFCINDEXEDPOLYCURVE"."Segments" @ 1;
        POINT_LIST_COORDINATES = "IFCCARTESIANPOINTLIST3D"."CoordList" @ 0;
        BREP_OUTER = "IFCMANIFOLDSOLIDBREP"."Outer" @ 0;
        FACE_MODEL_FACES = "IFCFACEBASEDSURFACEMODEL"."FbsmFaces" @ 0;
        SHELL_MODEL_BOUNDARY = "IFCSHELLBASEDSURFACEMODEL"."SbsmBoundary" @ 0;
        FACE_SET_COORDINATES = "IFCTESSELLATEDFACESET"."Coordinates" @ 0;
        TRIANGULATED_COORD_INDEX = "IFCTRIANGULATEDFACESET"."CoordIndex" @ 3;
        TRIANGULATED_PN_INDEX = "IFCTRIANGULATEDFACESET"."PnIndex" @ 4;
        POLYGONAL_FACES = "IFCPOLYGONALFACESET"."Faces" @ 2;
        POLYGONAL_PN_INDEX = "IFCPOLYGONALFACESET"."PnIndex" @ 3;
        INDEXED_FACE_COORD_INDEX = "IFCINDEXEDPOLYGONALFACE"."CoordIndex" @ 0;
        INDEXED_FACE_INNER = "IFCINDEXEDPOLYGONALFACEWITHVOIDS"."InnerCoordIndices" @ 1;
        SHELL_FACES = "IFCCONNECTEDFACESET"."CfsFaces" @ 0;
        FACE_BOUNDS = "IFCFACE"."Bounds" @ 0;
        FACE_BOUND = "IFCFACEBOUND"."Bound" @ 0;
        FACE_BOUND_ORIENTATION = "IFCFACEBOUND"."Orientation" @ 1;
        POLY_LOOP_POLYGON = "IFCPOLYLOOP"."Polygon" @ 0;
        MAPPING_SOURCE = "IFCMAPPEDITEM"."MappingSource" @ 0;
        MAPPING_TARGET = "IFCMAPPEDITEM"."MappingTarget" @ 1;
        MAPPING_ORIGIN = "IFCREPRESENTATIONMAP"."MappingOrigin" @ 0;
        MAPPED_REPRESENTATION = "IFCREPRESENTATIONMAP"."MappedRepresentation" @ 1;
        REPRESENTATION_IDENTIFIER = "IFCREPRESENTATION"."RepresentationIdentifier" @ 1;
        REPRESENTATION_TYPE = "IFCREPRESENTATION"."RepresentationType" @ 2;
        REPRESENTATION_ITEMS = "IFCREPRESENTATION"."Items" @ 3;
        PRODUCT_REPRESENTATIONS = "IFCPRODUCTREPRESENTATION"."Representations" @ 2;
        PRODUCT_PLACEMENT = "IFCPRODUCT"."ObjectPlacement" @ 5;
        PRODUCT_REPRESENTATION = "IFCPRODUCT"."Representation" @ 6;
        POINT_COORDINATES = "IFCCARTESIANPOINT"."Coordinates" @ 0;
        DIRECTION_RATIOS = "IFCDIRECTION"."DirectionRatios" @ 0;
        PLACEMENT_LOCATION = "IFCPLACEMENT"."Location" @ 0;
        AXIS3_AXIS = "IFCAXIS2PLACEMENT3D"."Axis" @ 1;
        AXIS3_REF_DIRECTION = "IFCAXIS2PLACEMENT3D"."RefDirection" @ 2;
        AXIS2_REF_DIRECTION = "IFCAXIS2PLACEMENT2D"."RefDirection" @ 1;
        LOCAL_REL_TO = "IFCLOCALPLACEMENT"."PlacementRelTo" @ 0;
        LOCAL_RELATIVE = "IFCLOCALPLACEMENT"."RelativePlacement" @ 1;
        OPERATOR_AXIS1 = "IFCCARTESIANTRANSFORMATIONOPERATOR"."Axis1" @ 0;
        OPERATOR_AXIS2 = "IFCCARTESIANTRANSFORMATIONOPERATOR"."Axis2" @ 1;
        OPERATOR_ORIGIN = "IFCCARTESIANTRANSFORMATIONOPERATOR"."LocalOrigin" @ 2;
        OPERATOR_SCALE = "IFCCARTESIANTRANSFORMATIONOPERATOR"."Scale" @ 3;
        OPERATOR_AXIS3 = "IFCCARTESIANTRANSFORMATIONOPERATOR3D"."Axis3" @ 4;
        OPERATOR_SCALE2 = "IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM"."Scale2" @ 5;
        OPERATOR_SCALE3 = "IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM"."Scale3" @ 6;
        PROFILE_POSITION = "IFCPARAMETERIZEDPROFILEDEF"."Position" @ 2;
        RECTANGLE_X = "IFCRECTANGLEPROFILEDEF"."XDim" @ 3;
        RECTANGLE_Y = "IFCRECTANGLEPROFILEDEF"."YDim" @ 4;
        CIRCLE_RADIUS = "IFCCIRCLEPROFILEDEF"."Radius" @ 3;
        PROFILE_OUTER_CURVE = "IFCARBITRARYCLOSEDPROFILEDEF"."OuterCurve" @ 2;
        PROFILE_INNER_CURVES = "IFCARBITRARYPROFILEDEFWITHVOIDS"."InnerCurves" @ 3;
        EXTRUSION_AREA = "IFCSWEPTAREASOLID"."SweptArea" @ 0;
        EXTRUSION_POSITION = "IFCSWEPTAREASOLID"."Position" @ 1;
        EXTRUSION_DIRECTION = "IFCEXTRUDEDAREASOLID"."ExtrudedDirection" @ 2;
        EXTRUSION_DEPTH = "IFCEXTRUDEDAREASOLID"."Depth" @ 3;
        UNIT_TYPE = "IFCNAMEDUNIT"."UnitType" @ 1;
        SI_PREFIX = "IFCSIUNIT"."Prefix" @ 2;
        SI_NAME = "IFCSIUNIT"."Name" @ 3;
        CONVERSION_FACTOR = "IFCCONVERSIONBASEDUNIT"."ConversionFactor" @ 3;
        MEASURE_VALUE = "IFCMEASUREWITHUNIT"."ValueComponent" @ 0;
        MEASURE_UNIT = "IFCMEASUREWITHUNIT"."UnitComponent" @ 1;
        PROJECT_UNITS = "IFCPROJECT"."UnitsInContext" @ 8;
        UNIT_ASSIGNMENT_UNITS = "IFCUNITASSIGNMENT"."Units" @ 0;
    }
}

#[cfg(test)]
mod tests {
    use super::known;
    use crate::model::Schema;
    use crate::schema::SchemaTables;

    /// Every attribute the views and the geometry read has, in every schema that declares its
    /// entity, the fixed position used without tables, or it is absent from that schema.
    #[test]
    fn fixed_positions_agree_with_the_tables() {
        let mut checked = 0;
        for schema in [Schema::Ifc2x3, Schema::Ifc4, Schema::Ifc4x3] {
            let Some(tables) = SchemaTables::of(&schema) else {
                return;
            };
            for attr in known::ALL {
                let Some(entity) = tables.entity_index(attr.entity) else {
                    continue;
                };
                if let Some((position, _)) = tables.attribute_position(entity, attr.name) {
                    assert_eq!(
                        position, attr.position,
                        "{schema:?} {}.{}",
                        attr.entity, attr.name
                    );
                    checked += 1;
                } else {
                    // The attributes IFC2X3 lacks, and the description of a property, which
                    // IFC4X3 renames.
                    let expected = match attr.name {
                        "Formula" | "SetPointValue" => schema == Schema::Ifc2x3,
                        "Description" => schema == Schema::Ifc4x3,
                        "Specification" => schema != Schema::Ifc4x3,
                        _ => false,
                    };
                    assert!(
                        expected,
                        "{schema:?} {}.{} is not declared",
                        attr.entity, attr.name
                    );
                }
            }
        }
        assert!(checked > 2 * known::ALL.len(), "{checked}");
    }
}
