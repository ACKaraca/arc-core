//! The model of an IDS file: specifications with their applicability and requirement facets.

use super::regex::Pattern;

/// The cardinality of a specification: how many applicable instances the model needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// At least one instance must be applicable and every applicable instance must satisfy the
    /// requirements (`minOccurs="1"`).
    Required,
    /// Applicable instances need not exist; those that do must satisfy the requirements
    /// (`minOccurs="0"`, `maxOccurs="unbounded"`).
    Optional,
    /// No instance may be applicable (`maxOccurs="0"`).
    Prohibited,
}

/// The cardinality of a requirement facet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetCardinality {
    /// The facet must be satisfied.
    Required,
    /// If the described data exists, it must satisfy the facet.
    Optional,
    /// The facet must not be satisfied.
    Prohibited,
}

/// A restriction of a value: the facets of an `xs:restriction`.
#[derive(Debug, Clone, PartialEq)]
pub struct Restriction {
    /// The local name of the `base` type, such as `string`, `integer` or `double`.
    pub base: String,
    /// `xs:enumeration` values; a value must equal one of them.
    pub enumeration: Vec<String>,
    /// `xs:pattern` values with their compiled form; a string must match one of them.
    pub patterns: Vec<(String, Pattern)>,
    /// `xs:minInclusive`.
    pub min_inclusive: Option<String>,
    /// `xs:minExclusive`.
    pub min_exclusive: Option<String>,
    /// `xs:maxInclusive`.
    pub max_inclusive: Option<String>,
    /// `xs:maxExclusive`.
    pub max_exclusive: Option<String>,
    /// `xs:length`.
    pub length: Option<usize>,
    /// `xs:minLength`.
    pub min_length: Option<usize>,
    /// `xs:maxLength`.
    pub max_length: Option<usize>,
}

/// An `ids:idsValue`: a simple value, or a restriction.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `simpleValue`, the text exactly.
    Simple(String),
    /// `xs:restriction`.
    Restriction(Restriction),
}

/// The entity facet.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityFacet {
    /// The IFC class, upper case.
    pub name: Value,
    /// The predefined type.
    pub predefined_type: Option<Value>,
}

/// The attribute facet.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeFacet {
    /// The attribute name.
    pub name: Value,
    /// The value, when the facet constrains it.
    pub value: Option<Value>,
}

/// The classification facet.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationFacet {
    /// The name of the classification system.
    pub system: Value,
    /// The reference.
    pub value: Option<Value>,
}

/// The property facet.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyFacet {
    /// The property set name.
    pub property_set: Value,
    /// The property name (`baseName`).
    pub base_name: Value,
    /// The data type, an IFC defined type name in upper case.
    pub data_type: Option<String>,
    /// The value.
    pub value: Option<Value>,
}

/// The material facet.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialFacet {
    /// The material name or category.
    pub value: Option<Value>,
}

/// The part-of facet.
#[derive(Debug, Clone, PartialEq)]
pub struct PartOfFacet {
    /// The entity the instance is part of.
    pub entity: EntityFacet,
    /// The relation type, upper case; every relation when absent.
    pub relation: Option<String>,
}

/// A facet.
#[derive(Debug, Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "a document holds a few facets, read many times; boxing would add indirection to every access"
)]
pub enum Facet {
    /// The IFC class and predefined type.
    Entity(EntityFacet),
    /// An attribute.
    Attribute(AttributeFacet),
    /// A classification.
    Classification(ClassificationFacet),
    /// A property.
    Property(PropertyFacet),
    /// A material.
    Material(MaterialFacet),
    /// A part-of relation.
    PartOf(PartOfFacet),
}

impl Facet {
    /// The element name of the facet in an IDS file.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Entity(_) => "entity",
            Self::Attribute(_) => "attribute",
            Self::Classification(_) => "classification",
            Self::Property(_) => "property",
            Self::Material(_) => "material",
            Self::PartOf(_) => "partOf",
        }
    }
}

/// A requirement: a facet with its cardinality.
#[derive(Debug, Clone, PartialEq)]
pub struct Requirement {
    /// The facet.
    pub facet: Facet,
    /// Its cardinality; an entity facet is always required.
    pub cardinality: FacetCardinality,
    /// The `instructions` attribute.
    pub instructions: Option<String>,
}

/// A specification.
#[derive(Debug, Clone, PartialEq)]
pub struct Specification {
    /// The `name` attribute.
    pub name: String,
    /// The `identifier` attribute.
    pub identifier: Option<String>,
    /// The `description` attribute.
    pub description: Option<String>,
    /// The `ifcVersion` list, such as `IFC4`. It is metadata and does not change a result.
    pub ifc_versions: Vec<String>,
    /// Whether instances must exist.
    pub cardinality: Cardinality,
    /// The facets that select the instances.
    pub applicability: Vec<Facet>,
    /// The facets the selected instances must satisfy.
    pub requirements: Vec<Requirement>,
}

/// An IDS document.
#[derive(Debug, Clone, PartialEq)]
pub struct Ids {
    /// The `title` of the `info` element.
    pub title: String,
    /// The specifications, in order.
    pub specifications: Vec<Specification>,
}
