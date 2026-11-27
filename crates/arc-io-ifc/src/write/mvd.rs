//! Model view rule sets: the exchange requirements of the Reference View and the Design Transfer
//! View that the writer can check on its output, as data: the representation item kinds allowed in
//! a shape representation and the products allowed.
//!
//! The Reference View exchanges tessellated geometry; the Design Transfer View also exchanges
//! swept solids, B-reps and clipping results. Both are IFC4 views; IFC2X3 is exchanged through
//! other views and IFC4X3 uses the same rule sets under the same labels. A file is named in
//! `FILE_DESCRIPTION` only when it passes the rule set (see [`ViewCheck`]).

use crate::model::{IfcFile, Parameter};

use super::OutputSchema;

/// A model view with a rule set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelView {
    /// Reference View: tessellated geometry, bounding boxes and annotation curves only.
    ReferenceView,
    /// Design Transfer View: the Reference View's items plus extrusions, B-reps and clipping.
    DesignTransferView,
}

/// What the writer does with a model view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewCheck {
    /// No check: `WriteOptions::view_definition` is written as given.
    #[default]
    Off,
    /// Check the file against the view; the findings are returned, and the header names the view
    /// only when there are none (otherwise `ViewDefinition []`).
    Report(ModelView),
    /// Check the file; a finding fails the write with `io.ifc.write_view_violated`.
    Enforce(ModelView),
}

/// A finding of a model view check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFinding {
    /// The view.
    pub view: ModelView,
    /// The stable code: `io.ifc.view_representation_not_allowed`, `io.ifc.view_entity_not_allowed`
    /// or `io.ifc.view_schema_unsupported`.
    pub code: &'static str,
    /// The instance name; 0 for the schema.
    pub instance: u64,
    /// The entity of the instance, upper case; empty for the schema.
    pub entity: String,
}

/// A finding code with its Fluent message.
#[derive(Debug, Clone, Copy)]
pub struct ViewRule {
    /// The stable code.
    pub code: &'static str,
    /// The Fluent message.
    pub message_id: &'static str,
}

/// The finding code and Fluent message of an item kind that the view does not allow in a shape
/// representation.
pub const REPRESENTATION_NOT_ALLOWED: ViewRule = ViewRule {
    code: "io.ifc.view_representation_not_allowed",
    message_id: "io-ifc-view-representation-not-allowed",
};
/// The finding code and Fluent message of a product the view does not exchange.
pub const ENTITY_NOT_ALLOWED: ViewRule = ViewRule {
    code: "io.ifc.view_entity_not_allowed",
    message_id: "io-ifc-view-entity-not-allowed",
};
/// The finding code and Fluent message of a schema the view is not defined for.
pub const SCHEMA_UNSUPPORTED: ViewRule = ViewRule {
    code: "io.ifc.view_schema_unsupported",
    message_id: "io-ifc-view-schema-unsupported",
};

/// The finding codes of a view check with their Fluent messages.
pub const VIEW_RULES: &[ViewRule] = &[
    REPRESENTATION_NOT_ALLOWED,
    ENTITY_NOT_ALLOWED,
    SCHEMA_UNSUPPORTED,
];

/// Items of a shape representation both views exchange.
const COMMON_ITEMS: &[&str] = &[
    "IFCTRIANGULATEDFACESET",
    "IFCPOLYGONALFACESET",
    "IFCMAPPEDITEM",
    "IFCBOUNDINGBOX",
    "IFCPOLYLINE",
    "IFCGEOMETRICCURVESET",
    "IFCCOMPOSITECURVE",
    "IFCTRIMMEDCURVE",
];

/// Items only the Design Transfer View exchanges.
const DESIGN_ITEMS: &[&str] = &[
    "IFCEXTRUDEDAREASOLID",
    "IFCREVOLVEDAREASOLID",
    "IFCFACETEDBREP",
    "IFCFACETEDBREPWITHVOIDS",
    "IFCADVANCEDBREP",
    "IFCBOOLEANCLIPPINGRESULT",
    "IFCBOOLEANRESULT",
    "IFCSWEPTDISKSOLID",
];

/// The building products both views exchange.
const PRODUCTS: &[&str] = &[
    "IFCSITE",
    "IFCBUILDING",
    "IFCBUILDINGSTOREY",
    "IFCSPACE",
    "IFCWALL",
    "IFCWALLSTANDARDCASE",
    "IFCSLAB",
    "IFCDOOR",
    "IFCWINDOW",
    "IFCOPENINGELEMENT",
    "IFCROOF",
    "IFCCOVERING",
    "IFCSTAIR",
    "IFCSTAIRFLIGHT",
    "IFCRAMP",
    "IFCRAMPFLIGHT",
    "IFCRAILING",
    "IFCCURTAINWALL",
    "IFCPLATE",
    "IFCMEMBER",
    "IFCBEAM",
    "IFCCOLUMN",
    "IFCFOOTING",
    "IFCFURNISHINGELEMENT",
    "IFCBUILDINGELEMENTPROXY",
    "IFCTRANSPORTELEMENT",
    "IFCPIPESEGMENT",
    "IFCGRID",
];

impl ModelView {
    /// The label written in `FILE_DESCRIPTION` for `schema`; `None` where the view is not defined.
    #[must_use]
    pub fn label(self, schema: OutputSchema) -> Option<&'static str> {
        match (schema, self) {
            (OutputSchema::Ifc2x3, _) => None,
            (_, Self::ReferenceView) => Some("ReferenceView_V1.2"),
            (_, Self::DesignTransferView) => Some("DesignTransferView_V1.0"),
        }
    }

    fn allows_item(self, entity: &str) -> bool {
        COMMON_ITEMS.contains(&entity)
            || (self == Self::DesignTransferView && DESIGN_ITEMS.contains(&entity))
    }
}

/// Checks a written file against the rule set of `view`.
pub(super) fn check(file: &IfcFile, view: ModelView, schema: OutputSchema) -> Vec<ViewFinding> {
    if view.label(schema).is_none() {
        return vec![ViewFinding {
            view,
            code: SCHEMA_UNSUPPORTED.code,
            instance: 0,
            entity: String::new(),
        }];
    }
    let mut findings = Vec::new();
    let mut found = |code, instance, entity: &str| {
        findings.push(ViewFinding {
            view,
            code,
            instance,
            entity: entity.to_owned(),
        });
    };
    for instance in file.instances() {
        let Some(record) = instance.records.first() else {
            continue;
        };
        let entity = record.type_name.to_ascii_uppercase();
        if entity == "IFCSHAPEREPRESENTATION" {
            // Items is the fourth attribute.
            if let Some(Parameter::List(items)) = record.parameters.get(3) {
                for item in items {
                    let Parameter::Reference(name) = item else {
                        continue;
                    };
                    let kind = file
                        .instance(*name)
                        .and_then(|i| i.records.first())
                        .map(|r| r.type_name.to_ascii_uppercase());
                    if let Some(kind) = kind.filter(|k| !view.allows_item(k)) {
                        found(REPRESENTATION_NOT_ALLOWED.code, instance.name, &kind);
                    }
                }
            }
        }
        // A product has a local placement at position 5 (after the rooted and object attributes).
        let placed = matches!(record.parameters.get(5), Some(Parameter::Reference(p))
            if file.instance(*p).and_then(|i| i.records.first())
                .is_some_and(|r| r.type_name.eq_ignore_ascii_case("IFCLOCALPLACEMENT")));
        if placed && !PRODUCTS.contains(&entity.as_str()) {
            found(ENTITY_NOT_ALLOWED.code, instance.name, &entity);
        }
    }
    findings
}
