//! The schema the writer emits and the translation of its records into it.
//!
//! The writer builds every record in the IFC4 layout (`layout::IFC4_LAYOUT`: attribute names in
//! order). For IFC4 output the values are written as built. For another schema each record is
//! rebuilt by attribute name through the schema tables: the order and the number of attributes come
//! from the tables, an attribute the schema does not declare is dropped and reported, an attribute
//! the schema requires and the model does not have gets a neutral value and is reported, and an
//! enumeration value the schema does not know becomes `NOTDEFINED` (or `USERDEFINED`) and is
//! reported. The mapping differences between the schemas are the tables of this module: entity
//! names, attribute names and enumeration values.

use arc_base::{Error, Result};

use crate::error::{codes, reasons};
use crate::loss::{LossLog, Source};
use crate::model::{Parameter, Schema};
use crate::schema::{Named, SchemaTables, TypeRef};

use super::layout::IFC4_LAYOUT;

/// The schema a file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum OutputSchema {
    /// IFC4 ADD2 TC1 (`FILE_SCHEMA(('IFC4'))`): the default. Needs no schema tables.
    #[default]
    Ifc4,
    /// IFC4X3 ADD2 (`IFC4X3_ADD2`). Its tables are part of every build.
    Ifc4x3,
    /// IFC2X3 TC1 (`IFC2X3`). Needs the generated tables, which a build without them lacks
    /// (`io.ifc.write_schema_unavailable`). Content IFC2X3 cannot carry is reported.
    Ifc2x3,
}

impl OutputSchema {
    /// The identifier written in `FILE_SCHEMA` and as the version of the loss report.
    #[must_use]
    pub const fn identifier(self) -> &'static str {
        match self {
            Self::Ifc4 => "IFC4",
            Self::Ifc4x3 => "IFC4X3_ADD2",
            Self::Ifc2x3 => "IFC2X3",
        }
    }

    /// The reader's schema of the same name.
    #[must_use]
    pub const fn schema(self) -> Schema {
        match self {
            Self::Ifc4 => Schema::Ifc4,
            Self::Ifc4x3 => Schema::Ifc4x3,
            Self::Ifc2x3 => Schema::Ifc2x3,
        }
    }

    /// Whether this build can write the schema: IFC4 always (the writer's own layout), the others
    /// when their generated tables are part of the build.
    #[must_use]
    pub fn available(self) -> bool {
        self == Self::Ifc4 || SchemaTables::of(&self.schema()).is_some()
    }
}

/// The output schema with its tables.
#[derive(Clone, Copy)]
pub(super) struct Target {
    pub(super) schema: OutputSchema,
    tables: Option<&'static SchemaTables>,
}

/// The attributes of an IFC4 layout entry, or `None` for an entity the writer does not build.
fn layout(entity: &str) -> Option<&'static [&'static str]> {
    IFC4_LAYOUT
        .binary_search_by(|(name, _)| name.cmp(&entity))
        .ok()
        .and_then(|i| IFC4_LAYOUT.get(i))
        .map(|(_, attributes)| *attributes)
}

/// The layout the writer builds `entity` in, for the test that compares it with the IFC4 tables.
#[cfg(all(test, arc_ifc_tables_ifc4))]
pub(super) fn ifc4_layout() -> &'static [(&'static str, &'static [&'static str])] {
    IFC4_LAYOUT
}

impl Target {
    /// The target of `schema`; IFC2X3 and IFC4X3 need their tables.
    pub(super) fn new(schema: OutputSchema) -> Result<Self> {
        let tables = SchemaTables::of(&schema.schema());
        if schema != OutputSchema::Ifc4 && tables.is_none() {
            return Err(Error::new(codes::IFC_WRITE_SCHEMA_UNAVAILABLE)
                .with_arg("schema", schema.identifier()));
        }
        Ok(Self { schema, tables })
    }

    /// Whether records are written as built (IFC4).
    pub(super) fn is_native(&self) -> bool {
        self.schema == OutputSchema::Ifc4
    }

    /// The entity of the output schema that stands for the writer's entity (upper case).
    fn entity_name(&self, entity: &str) -> &'static str {
        match (self.schema, entity) {
            (OutputSchema::Ifc2x3, "IFCDOORTYPE") => "IFCDOORSTYLE",
            (OutputSchema::Ifc2x3, "IFCWINDOWTYPE") => "IFCWINDOWSTYLE",
            _ => IFC4_LAYOUT
                .binary_search_by(|(name, _)| name.cmp(&entity))
                .ok()
                .and_then(|i| IFC4_LAYOUT.get(i))
                .map_or("", |(name, _)| name),
        }
    }

    /// Whether the output schema declares the entity (or defined type) the writer calls `name`.
    pub(super) fn supports(&self, name: &str) -> bool {
        let Some(tables) = self.tables else {
            return true;
        };
        let target = if layout(name).is_some() {
            self.entity_name(name)
        } else {
            name
        };
        tables.find(target).is_some()
    }

    /// The measure or simple type of the output schema that carries a value the writer types as
    /// `name`: the type itself, or its nearest wider type when the schema lacks it; `None` when
    /// there is none.
    /// Whether the output schema declares the defined type `name` as an `INTEGER`.
    pub(super) fn is_integer(&self, name: &str) -> bool {
        self.tables
            .and_then(|t| t.defined_type(name))
            .is_some_and(|d| d.underlying == TypeRef::Integer)
    }

    pub(super) fn value_type(&self, name: &str) -> Option<String> {
        let Some(tables) = self.tables else {
            return Some(name.to_owned());
        };
        let wider = match name {
            "IFCNONNEGATIVELENGTHMEASURE" => Some("IFCLENGTHMEASURE"),
            _ => None,
        };
        [Some(name), wider]
            .into_iter()
            .flatten()
            .find(|n| tables.find(n).is_some())
            .map(str::to_owned)
    }

    /// The record `entity` of the writer's IFC4 layout rebuilt in the output schema: the entity
    /// name as written and its parameters in the order of the tables.
    pub(super) fn translate(
        &self,
        entity: &str,
        parameters: &[Parameter],
        owner_history: Option<u64>,
        loss: &mut LossLog,
    ) -> Result<(String, Vec<Parameter>)> {
        let internal = || Error::new(codes::IFC_WRITE_VALUE_INVALID).with_arg("what", "schema map");
        let (Some(tables), Some(source)) = (self.tables, layout(entity)) else {
            return Err(internal());
        };
        if source.len() != parameters.len() {
            return Err(internal());
        }
        let target_name = self.entity_name(entity);
        let index = tables.entity_index(target_name).ok_or_else(|| {
            Error::new(codes::IFC_WRITE_SCHEMA_UNAVAILABLE).with_arg("entity", entity)
        })?;
        let mut used = vec![false; source.len()];
        let mut out = Vec::new();
        for position in tables.positions(index) {
            let attribute = position.attribute;
            if position.derived {
                out.push(Parameter::Derived);
                continue;
            }
            let candidates = aliases(self.schema, target_name, attribute.name);
            let found = std::iter::once(attribute.name)
                .chain(candidates.iter().copied())
                .find_map(|name| {
                    source
                        .iter()
                        .position(|s| s.eq_ignore_ascii_case(name))
                        .filter(|&i| !used.get(i).copied().unwrap_or(true))
                });
            let value = found.and_then(|i| {
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
                parameters.get(i)
            });
            let kind = format!("{entity}.{}", attribute.name);
            let value = match value {
                Some(Parameter::Omitted | Parameter::Derived) | None => None,
                Some(v) => Some(v),
            };
            out.push(match value {
                Some(Parameter::Enumeration(item)) => match enumeration_of(tables, attribute.ty) {
                    Some(e) if !e.contains(item) => {
                        let silent = mapped_item(self.schema, e.name, item);
                        let fallback = silent.or_else(|| {
                            ["NOTDEFINED", "USERDEFINED"]
                                .into_iter()
                                .find(|f| e.contains(f))
                        });
                        match fallback {
                            Some(f) => {
                                if silent.is_none() {
                                    loss.add(reasons::VALUE_NOT_REPRESENTABLE, &kind, None, 1);
                                }
                                Parameter::Enumeration(f.to_owned())
                            }
                            None if attribute.optional => {
                                loss.add(reasons::VALUE_NOT_REPRESENTABLE, &kind, None, 1);
                                Parameter::Omitted
                            }
                            None => return Err(internal()),
                        }
                    }
                    _ => Parameter::Enumeration(item.clone()),
                },
                Some(Parameter::Real(v)) if tables.underlying(attribute.ty) == TypeRef::Integer => {
                    // IFC4X3 declares counts as integers where IFC4 has numbers.
                    loss.add(
                        reasons::VALUE_NOT_REPRESENTABLE,
                        &kind,
                        None,
                        u64::from(v.fract() != 0.0),
                    );
                    Parameter::Integer(rounded(*v))
                }
                Some(v) => v.clone(),
                None if attribute.optional => Parameter::Omitted,
                None => required_default(tables, attribute, owner_history, &kind, loss)
                    .ok_or_else(internal)?,
            });
        }
        // A value of the model with no attribute to go to.
        for (i, name) in source.iter().enumerate() {
            let set = match parameters.get(i) {
                Some(Parameter::Omitted | Parameter::Derived) | None => false,
                Some(Parameter::Enumeration(e)) => e != "NOTDEFINED",
                Some(_) => true,
            };
            if set && !used.get(i).copied().unwrap_or(true) {
                loss.add(
                    reasons::ATTRIBUTE_NOT_REPRESENTABLE,
                    &format!("{entity}.{name}"),
                    None,
                    1,
                );
            }
        }
        Ok((target_name.to_ascii_uppercase(), out))
    }

    /// Reports an object the output schema cannot carry.
    pub(super) fn unrepresentable(loss: &mut LossLog, entity: &str, id: Option<&str>) {
        loss.add(
            reasons::ENTITY_NOT_REPRESENTABLE,
            entity,
            id.map(Source::Text),
            1,
        );
    }
}

/// A real as the nearest integer.
pub(super) fn rounded(v: f64) -> i64 {
    format!("{v:.0}").parse().unwrap_or(0)
}

/// The enumeration an attribute is typed with, through defined types.
fn enumeration_of(
    tables: &'static SchemaTables,
    ty: TypeRef,
) -> Option<&'static crate::schema::Enumeration> {
    match tables.underlying(ty) {
        TypeRef::Named(Named::Enumeration(i)) => tables.enumerations.get(usize::from(i)),
        _ => None,
    }
}

/// The value of a required attribute the model does not have, or `None` when no neutral value
/// exists.
fn required_default(
    tables: &'static SchemaTables,
    attribute: &'static crate::schema::Attribute,
    owner_history: Option<u64>,
    kind: &str,
    loss: &mut LossLog,
) -> Option<Parameter> {
    if attribute.name == "OwnerHistory" {
        return owner_history.map(Parameter::Reference);
    }
    let value = match tables.underlying(attribute.ty) {
        TypeRef::Boolean => Parameter::Enumeration("F".to_owned()),
        TypeRef::Logical => Parameter::Enumeration("U".to_owned()),
        TypeRef::Named(Named::Enumeration(i)) => {
            let e = tables.enumerations.get(usize::from(i))?;
            let item = ["NOTDEFINED", "USERDEFINED"]
                .into_iter()
                .find(|f| e.contains(f))?;
            Parameter::Enumeration(item.to_owned())
        }
        _ => return None,
    };
    loss.add(reasons::DEFAULT_SUPPLIED, kind, None, 1);
    Some(value)
}

/// The names of the writer's IFC4 attributes that stand for an attribute of another name in the
/// output schema, for the written entity `entity` (upper case).
fn aliases(schema: OutputSchema, entity: &str, attribute: &str) -> &'static [&'static str] {
    match (schema, entity, attribute) {
        // IfcProperty.Description is Specification in IFC4X3.
        (
            OutputSchema::Ifc4x3,
            "IFCPROPERTYSINGLEVALUE" | "IFCPROPERTYENUMERATEDVALUE",
            "Specification",
        ) => &["Description"],
        // IFC2X3 names.
        (OutputSchema::Ifc2x3, "IFCPERSON" | "IFCORGANIZATION", "Id") => &["Identification"],
        (OutputSchema::Ifc2x3, "IFCSTAIRFLIGHT", "NumberOfRiser") => &["NumberOfRisers"],
        (OutputSchema::Ifc2x3, "IFCROOF" | "IFCSTAIR" | "IFCRAMP", "ShapeType") => {
            &["PredefinedType"]
        }
        (OutputSchema::Ifc2x3, "IFCTRANSPORTELEMENT", "OperationType") => &["PredefinedType"],
        (OutputSchema::Ifc2x3, "IFCSPACE", "InteriorOrExteriorSpace") => &["PredefinedType"],
        (OutputSchema::Ifc2x3, "IFCWINDOWSTYLE", "OperationType") => &["PartitioningType"],
        _ => &[],
    }
}

/// An enumeration value of the writer's layout that the output schema spells differently, where
/// the mapping is exact in meaning and needs no report.
fn mapped_item(schema: OutputSchema, enumeration: &str, item: &str) -> Option<&'static str> {
    match (schema, enumeration, item) {
        // An IFC2X3 space is internal, external or neither; `SPACE` is the plain space.
        (OutputSchema::Ifc2x3, "IfcInternalOrExternalEnum", "SPACE") => Some("NOTDEFINED"),
        _ => None,
    }
}

#[cfg(all(test, arc_ifc_tables_ifc4))]
mod tests {
    use super::*;

    /// The writer's IFC4 layout is the IFC4 tables' attribute order for every entity it names.
    #[test]
    fn the_layout_equals_the_ifc4_tables() {
        let tables = SchemaTables::of(&Schema::Ifc4).expect("tables");
        for (name, attributes) in ifc4_layout() {
            let index = tables.entity_index(name).expect(name);
            let expected: Vec<&str> = tables
                .positions(index)
                .iter()
                .map(|p| p.attribute.name)
                .collect();
            assert_eq!(&expected, attributes, "{name}");
        }
    }
}
