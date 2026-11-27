//! Reading an IDS document: a bounded XML tree from the shared reader of `arc-io-common`, then the
//! specifications of the IDS 1.0 schema. Anything outside the schema is `io.ids.invalid`.

use arc_base::{Cancel, Error, Limits, Result};
use arc_io_common::ReadOptions as XmlOptions;
use arc_io_common::xml::{Event, XmlReader, split_qualified_name};

use super::model::{
    AttributeFacet, Cardinality, ClassificationFacet, EntityFacet, Facet, FacetCardinality, Ids,
    MaterialFacet, PartOfFacet, PropertyFacet, Requirement, Restriction, Specification, Value,
};
use super::regex::Pattern;
use crate::error::{codes, limit};

/// The namespace of IDS 1.0.
const IDS_NAMESPACE: &str = "http://standards.buildingsmart.org/IDS";
/// The namespace of XML Schema, which the restrictions use.
const XS_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema";

fn invalid(what: &str) -> Error {
    Error::new(codes::IDS_INVALID).with_arg("reason", what.to_owned())
}

/// An element of the document, names without prefix.
#[derive(Debug)]
struct Node {
    name: String,
    namespace: Option<String>,
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Refuses a child or an attribute that the schema does not list.
    fn allow(&self, children: &[&str], attributes: &[&str]) -> Result<()> {
        if let Some(c) = self
            .children
            .iter()
            .find(|c| !children.contains(&c.name.as_str()))
        {
            return Err(invalid(&format!("element {} inside {}", c.name, self.name)));
        }
        for (name, _) in &self.attributes {
            let (prefix, local) = split_qualified_name(name);
            let namespace_declaration = prefix == "xmlns" || name == "xmlns" || prefix == "xsi";
            if !attributes.contains(&local) && !namespace_declaration {
                return Err(invalid(&format!("attribute {name} on {}", self.name)));
            }
        }
        Ok(())
    }
}

/// Reads the document into a tree.
fn tree(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Node> {
    let options = XmlOptions {
        limits: *limits,
        cancel: cancel.clone(),
    };
    let mut reader = XmlReader::new(bytes, &options)?;
    let mut stack: Vec<Node> = Vec::new();
    let mut root = None;
    let mut count = 0u64;
    while let Some(event) = reader.next_event()? {
        match event {
            Event::Start(element) => {
                count += 1;
                if count > limits.record_count {
                    return Err(limit("record_count", count));
                }
                let (prefix, local) = split_qualified_name(&element.name);
                let namespace = reader.resolve_prefix(prefix).map(str::to_owned);
                stack.push(Node {
                    name: local.to_owned(),
                    namespace,
                    attributes: element
                        .attributes
                        .into_iter()
                        .map(|a| (a.name, a.value))
                        .collect(),
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            Event::Text(text) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&text);
                }
            }
            Event::End(_) => {
                let node = stack.pop().ok_or_else(|| invalid("unbalanced element"))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = Some(node),
                }
            }
        }
    }
    root.ok_or_else(|| invalid("no root element"))
}

/// Reads an IDS document.
///
/// # Errors
///
/// `io.ids.invalid` for a document that is not valid IDS 1.0 (the argument `reason` names the
/// first problem); `io.xml.*` for malformed XML; `io.limit_exceeded` for a limit of the profile;
/// `base.cancelled`.
pub fn read(bytes: &[u8], limits: &Limits, cancel: &Cancel) -> Result<Ids> {
    cancel.check()?;
    let root = tree(bytes, limits, cancel)?;
    if root.name != "ids" || root.namespace.as_deref() != Some(IDS_NAMESPACE) {
        return Err(invalid("the root element is not ids in the IDS namespace"));
    }
    root.allow(&["info", "specifications"], &[])?;
    let info = root
        .child("info")
        .ok_or_else(|| invalid("info is missing"))?;
    info.allow(
        &[
            "title",
            "copyright",
            "version",
            "description",
            "author",
            "date",
            "purpose",
            "milestone",
        ],
        &[],
    )?;
    let title = info
        .child("title")
        .ok_or_else(|| invalid("title is missing"))?
        .text
        .clone();
    let list = root
        .child("specifications")
        .ok_or_else(|| invalid("specifications is missing"))?;
    list.allow(&["specification"], &[])?;
    if list.children.is_empty() {
        return Err(invalid("there is no specification"));
    }
    let specifications = list
        .children
        .iter()
        .map(|s| specification(s, limits))
        .collect::<Result<Vec<_>>>()?;
    Ok(Ids {
        title,
        specifications,
    })
}

const IFC_VERSIONS: [&str; 3] = ["IFC2X3", "IFC4", "IFC4X3_ADD2"];

fn specification(node: &Node, limits: &Limits) -> Result<Specification> {
    node.allow(
        &["applicability", "requirements"],
        &[
            "name",
            "ifcVersion",
            "identifier",
            "description",
            "instructions",
        ],
    )?;
    let name = node
        .attribute("name")
        .ok_or_else(|| invalid("specification without a name"))?
        .to_owned();
    let versions: Vec<String> = node
        .attribute("ifcVersion")
        .ok_or_else(|| invalid("specification without ifcVersion"))?
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect();
    if versions.is_empty() || versions.iter().any(|v| !IFC_VERSIONS.contains(&v.as_str())) {
        return Err(invalid(
            "ifcVersion is not a list of IFC2X3, IFC4 and IFC4X3_ADD2",
        ));
    }
    let applicability = node
        .child("applicability")
        .ok_or_else(|| invalid("specification without applicability"))?;
    let cardinality = occurs(applicability)?;
    let applicability_facets = applicability_facets(applicability, limits)?;
    let mut requirements = Vec::new();
    if let Some(requirements_node) = node.child("requirements") {
        requirements_node.allow(
            &[
                "entity",
                "partOf",
                "classification",
                "attribute",
                "property",
                "material",
            ],
            &["description"],
        )?;
        for child in &requirements_node.children {
            requirements.push(requirement(child, limits)?);
        }
    }
    if cardinality == Cardinality::Prohibited && !requirements.is_empty() {
        return Err(invalid("a prohibited specification has requirements"));
    }
    Ok(Specification {
        name,
        identifier: node.attribute("identifier").map(str::to_owned),
        description: node.attribute("description").map(str::to_owned),
        ifc_versions: versions,
        cardinality,
        applicability: applicability_facets,
        requirements,
    })
}

/// The cardinality of a specification from `minOccurs` and `maxOccurs`.
fn occurs(node: &Node) -> Result<Cardinality> {
    let min = node.attribute("minOccurs").unwrap_or("1");
    let max = node.attribute("maxOccurs").unwrap_or("1");
    match (min, max) {
        ("0", "0") => Ok(Cardinality::Prohibited),
        ("0", "unbounded" | "1") => Ok(Cardinality::Optional),
        ("1", "unbounded" | "1") => Ok(Cardinality::Required),
        _ => Err(invalid(
            "minOccurs and maxOccurs are not required, optional or prohibited",
        )),
    }
}

fn applicability_facets(node: &Node, limits: &Limits) -> Result<Vec<Facet>> {
    node.allow(
        &[
            "entity",
            "partOf",
            "classification",
            "attribute",
            "property",
            "material",
        ],
        &["minOccurs", "maxOccurs"],
    )?;
    if node.children.is_empty() {
        return Err(invalid("applicability without a facet"));
    }
    // The schema lists the facets in a fixed order: entity first, then the others by kind; the
    // order does not change the meaning, so it is not enforced.
    node.children.iter().map(|c| facet(c, limits)).collect()
}

fn requirement(node: &Node, limits: &Limits) -> Result<Requirement> {
    let cardinality = match node.attribute("cardinality") {
        None | Some("required") => FacetCardinality::Required,
        Some("optional") => FacetCardinality::Optional,
        Some("prohibited") => FacetCardinality::Prohibited,
        Some(_) => {
            return Err(invalid(
                "cardinality is not required, optional or prohibited",
            ));
        }
    };
    if node.name == "entity" && node.attribute("cardinality").is_some() {
        return Err(invalid("an entity requirement has no cardinality"));
    }
    if node.name == "partOf" && cardinality == FacetCardinality::Optional {
        return Err(invalid("a partOf requirement cannot be optional"));
    }
    Ok(Requirement {
        facet: facet(node, limits)?,
        cardinality,
        instructions: node.attribute("instructions").map(str::to_owned),
    })
}

fn facet(node: &Node, limits: &Limits) -> Result<Facet> {
    let common = ["cardinality", "instructions", "uri"];
    match node.name.as_str() {
        "entity" => {
            node.allow(&["name", "predefinedType"], &["instructions"])?;
            Ok(Facet::Entity(entity(node, limits)?))
        }
        "attribute" => {
            node.allow(&["name", "value"], &common)?;
            Ok(Facet::Attribute(AttributeFacet {
                name: required_value(node, "name", limits)?,
                value: optional_value(node, "value", limits)?,
            }))
        }
        "classification" => {
            node.allow(&["value", "system"], &common)?;
            Ok(Facet::Classification(ClassificationFacet {
                system: required_value(node, "system", limits)?,
                value: optional_value(node, "value", limits)?,
            }))
        }
        "property" => {
            node.allow(
                &["propertySet", "baseName", "value"],
                &[&common[..], &["dataType"]].concat(),
            )?;
            let data_type = node.attribute("dataType").map(str::to_owned);
            if let Some(t) = &data_type
                && (t.is_empty() || !t.bytes().all(|b| b.is_ascii_uppercase()))
            {
                return Err(invalid("dataType is not an upper case name"));
            }
            Ok(Facet::Property(PropertyFacet {
                property_set: required_value(node, "propertySet", limits)?,
                base_name: required_value(node, "baseName", limits)?,
                data_type,
                value: optional_value(node, "value", limits)?,
            }))
        }
        "material" => {
            node.allow(&["value"], &common)?;
            Ok(Facet::Material(MaterialFacet {
                value: optional_value(node, "value", limits)?,
            }))
        }
        "partOf" => {
            node.allow(&["entity"], &[&common[..], &["relation"]].concat())?;
            let relation = node.attribute("relation").map(str::to_owned);
            if let Some(r) = &relation
                && ![
                    "IFCRELAGGREGATES",
                    "IFCRELASSIGNSTOGROUP",
                    "IFCRELCONTAINEDINSPATIALSTRUCTURE",
                    "IFCRELNESTS",
                    "IFCRELVOIDSELEMENT IFCRELFILLSELEMENT",
                ]
                .contains(&r.as_str())
            {
                return Err(invalid(
                    "relation is not one of the relations of the schema",
                ));
            }
            let entity_node = node
                .child("entity")
                .ok_or_else(|| invalid("partOf without entity"))?;
            entity_node.allow(&["name", "predefinedType"], &[])?;
            Ok(Facet::PartOf(PartOfFacet {
                entity: entity(entity_node, limits)?,
                relation,
            }))
        }
        other => Err(invalid(&format!("unknown facet {other}"))),
    }
}

fn entity(node: &Node, limits: &Limits) -> Result<EntityFacet> {
    Ok(EntityFacet {
        name: required_value(node, "name", limits)?,
        predefined_type: optional_value(node, "predefinedType", limits)?,
    })
}

fn required_value(node: &Node, name: &str, limits: &Limits) -> Result<Value> {
    optional_value(node, name, limits)?
        .ok_or_else(|| invalid(&format!("{} without {name}", node.name)))
}

/// An `ids:idsValue` child: `simpleValue` or `xs:restriction`.
fn optional_value(node: &Node, name: &str, limits: &Limits) -> Result<Option<Value>> {
    let Some(holder) = node.child(name) else {
        return Ok(None);
    };
    if node.children_named(name).count() > 1 {
        return Err(invalid(&format!("{name} is repeated")));
    }
    holder.allow(&["simpleValue", "restriction"], &[])?;
    match holder.children.as_slice() {
        [simple] if simple.name == "simpleValue" => {
            simple.allow(&[], &[])?;
            Ok(Some(Value::Simple(simple.text.clone())))
        }
        [restriction] if restriction.name == "restriction" => {
            if restriction.namespace.as_deref() != Some(XS_NAMESPACE) {
                return Err(invalid("restriction is not in the XML Schema namespace"));
            }
            Ok(Some(Value::Restriction(restriction_of(
                restriction,
                limits,
            )?)))
        }
        _ => Err(invalid(&format!(
            "{name} holds one simpleValue or one restriction"
        ))),
    }
}

fn restriction_of(node: &Node, _limits: &Limits) -> Result<Restriction> {
    node.allow(
        &[
            "enumeration",
            "pattern",
            "minInclusive",
            "minExclusive",
            "maxInclusive",
            "maxExclusive",
            "length",
            "minLength",
            "maxLength",
            "totalDigits",
            "fractionDigits",
            "annotation",
        ],
        &["base"],
    )?;
    let base = node
        .attribute("base")
        .ok_or_else(|| invalid("restriction without base"))?;
    let base = split_qualified_name(base).1.to_owned();
    let single = |name: &str| -> Result<Option<String>> {
        let mut found = node.children_named(name);
        let first = found.next();
        if found.next().is_some() {
            return Err(invalid(&format!("{name} is repeated")));
        }
        first
            .map(|n| {
                n.allow(&["annotation"], &["value", "fixed"])?;
                n.attribute("value")
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(&format!("{name} without value")))
            })
            .transpose()
    };
    let count = |name: &str| -> Result<Option<usize>> {
        single(name)?
            .map(|v| {
                v.trim()
                    .parse::<usize>()
                    .map_err(|_| invalid(&format!("{name} is not a count")))
            })
            .transpose()
    };
    if node.child("totalDigits").is_some() || node.child("fractionDigits").is_some() {
        return Err(Error::new(codes::IDS_UNSUPPORTED)
            .with_arg("kind", "totalDigits and fractionDigits".to_owned()));
    }
    let mut enumeration = Vec::new();
    for e in node.children_named("enumeration") {
        e.allow(&["annotation"], &["value", "fixed"])?;
        enumeration.push(
            e.attribute("value")
                .ok_or_else(|| invalid("enumeration without value"))?
                .to_owned(),
        );
    }
    let mut patterns = Vec::new();
    for p in node.children_named("pattern") {
        p.allow(&["annotation"], &["value", "fixed"])?;
        let source = p
            .attribute("value")
            .ok_or_else(|| invalid("pattern without value"))?;
        let compiled = Pattern::new(source).map_err(|e| invalid(&format!("pattern: {e}")))?;
        patterns.push((source.to_owned(), compiled));
    }
    let restriction = Restriction {
        base,
        enumeration,
        patterns,
        min_inclusive: single("minInclusive")?,
        min_exclusive: single("minExclusive")?,
        max_inclusive: single("maxInclusive")?,
        max_exclusive: single("maxExclusive")?,
        length: count("length")?,
        min_length: count("minLength")?,
        max_length: count("maxLength")?,
    };
    if restriction.enumeration.is_empty()
        && restriction.patterns.is_empty()
        && restriction.min_inclusive.is_none()
        && restriction.min_exclusive.is_none()
        && restriction.max_inclusive.is_none()
        && restriction.max_exclusive.is_none()
        && restriction.length.is_none()
        && restriction.min_length.is_none()
        && restriction.max_length.is_none()
    {
        return Err(invalid("restriction without a facet"));
    }
    Ok(restriction)
}
