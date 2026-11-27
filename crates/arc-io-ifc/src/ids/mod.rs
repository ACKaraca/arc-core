//! Information Delivery Specification 1.0 (CORE-IO-014, IDS part): a reader for IDS files and a
//! checker of an [`IfcFile`] against them.
//!
//! [`read`] reads an IDS document with the bounded XML reader of `arc-io-common`. It accepts
//! exactly the documents of the IDS 1.0 schema: the elements and attributes of the schema, the six
//! facets (entity, attribute, classification, property, material, partOf), the restrictions
//! `xs:enumeration`, `xs:pattern` (the pattern language of XML Schema, see [`regex`]), the bounds
//! `minInclusive`, `minExclusive`, `maxInclusive` and `maxExclusive`, and the lengths `length`,
//! `minLength` and `maxLength`. `totalDigits` and `fractionDigits` are `io.ids.unsupported`, as
//! the IDS documentation excludes them.
//!
//! [`Ids::check`] evaluates every specification against a file: the applicability facets select
//! instances, the requirement facets are evaluated on each, and a specification passes according
//! to its cardinality. The result lists, per specification, the applicable instances and the
//! failing ones with the requirement and the reason.

pub mod regex;

mod check;
mod model;
mod read;
mod value;

pub use check::{Failure, SpecificationResult, Status};
pub use model::{
    AttributeFacet, Cardinality, ClassificationFacet, EntityFacet, Facet, FacetCardinality, Ids,
    MaterialFacet, PartOfFacet, PropertyFacet, Requirement, Restriction, Specification, Value,
};
pub use read::read;

/// The result of checking a file against an IDS document.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// One result per specification, in order.
    pub specifications: Vec<SpecificationResult>,
}

impl Report {
    /// Whether every specification passed.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.specifications.iter().all(|s| s.status == Status::Pass)
    }
}
