//! The template check of property and quantity sets.
//!
//! Every `IfcPropertySet` and `IfcElementQuantity` whose name starts with `Pset_` or `Qto_` is
//! compared with the template of that name for the file's schema: an unknown template, a set of
//! the other kind, unknown property and quantity names, properties written as another property
//! entity, quantities of another quantity class, values of another type, enumeration values that
//! are not items of the enumeration, and objects the set does not apply to. A check that needs
//! knowledge the build lacks (the schema tables, for subtypes and predefined types) is skipped,
//! never reported as passed or failed.

use std::collections::BTreeMap;

use arc_base::{Cancel, Error, ErrorCode, Limits, Result};

use super::{PropertyKind, PropertyTemplate, PsetTemplates, SetKind, SetTemplate};
use crate::error::limit;
use crate::model::{IfcFile, Parameter, Record};
use crate::schema::SchemaTables;

/// The finding codes of the template check. Messages are in `locales/*/arc-io-ifc.ftl`; every
/// finding carries the arguments `set`, `property`, `expected` and `found`.
pub mod findings {
    use arc_base::{Category, ErrorCode};

    /// A set name starts with `Pset_` or `Qto_` but no template of the schema has that name.
    pub const PSET_TEMPLATE_UNKNOWN: ErrorCode = ErrorCode {
        code: "io.ifc.pset_template_unknown",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-template-unknown",
    };
    /// A property set carries the name of a quantity set template, or the reverse.
    pub const PSET_SET_KIND: ErrorCode = ErrorCode {
        code: "io.ifc.pset_set_kind",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-set-kind",
    };
    /// A property or quantity name is not in the template.
    pub const PSET_PROPERTY_UNKNOWN: ErrorCode = ErrorCode {
        code: "io.ifc.pset_property_unknown",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-property-unknown",
    };
    /// A property is written as another property entity than its template's kind.
    pub const PSET_PROPERTY_KIND: ErrorCode = ErrorCode {
        code: "io.ifc.pset_property_kind",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-property-kind",
    };
    /// A quantity is of another quantity class than its template's.
    pub const PSET_QUANTITY_KIND: ErrorCode = ErrorCode {
        code: "io.ifc.pset_quantity_kind",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-quantity-kind",
    };
    /// A value is not of the template's data type, or not typed at all.
    pub const PSET_VALUE_TYPE: ErrorCode = ErrorCode {
        code: "io.ifc.pset_value_type",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-value-type",
    };
    /// An enumerated value is not an item of the template's enumeration.
    pub const PSET_ENUMERATION_ITEM: ErrorCode = ErrorCode {
        code: "io.ifc.pset_enumeration_item",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-enumeration-item",
    };
    /// The set is assigned to an object whose entity or predefined type the template does not
    /// apply to.
    pub const PSET_NOT_APPLICABLE: ErrorCode = ErrorCode {
        code: "io.ifc.pset_not_applicable",
        category: Category::InvalidInput,
        message_id: "io-ifc-pset-not-applicable",
    };

    /// Every finding code.
    pub const ALL: &[ErrorCode] = &[
        PSET_TEMPLATE_UNKNOWN,
        PSET_SET_KIND,
        PSET_PROPERTY_UNKNOWN,
        PSET_PROPERTY_KIND,
        PSET_QUANTITY_KIND,
        PSET_VALUE_TYPE,
        PSET_ENUMERATION_ITEM,
        PSET_NOT_APPLICABLE,
    ];
}

/// One difference between a set and its template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsetFinding {
    /// The finding code, one of [`findings::ALL`].
    pub code: ErrorCode,
    /// The instance name of the set.
    pub set: u64,
    /// The set name.
    pub set_name: String,
    /// The property or quantity, with the names of enclosing complex properties joined by `.`;
    /// empty for a finding about the set.
    pub property: String,
    /// The instance name of the object, for [`findings::PSET_NOT_APPLICABLE`].
    pub object: Option<u64>,
    /// What the template expects, such as `IfcPositiveLengthMeasure`.
    pub expected: String,
    /// What the file holds.
    pub found: String,
}

impl PsetFinding {
    /// The finding as an error value with the message arguments `set`, `property`, `expected`
    /// and `found`, and the details `set_instance` and `object`.
    #[must_use]
    pub fn error(&self) -> Error {
        let mut error = Error::new(self.code)
            .with_arg("set", self.set_name.clone())
            .with_arg("property", self.property.clone())
            .with_arg("expected", self.expected.clone())
            .with_arg("found", self.found.clone())
            .with_detail("set_instance", clamp(self.set));
        if let Some(object) = self.object {
            error = error.with_detail("object", clamp(object));
        }
        error
    }
}

fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The result of the template check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsetCheck {
    /// The schema of the templates used; `None` when the build or the file's schema has no
    /// templates, in which case nothing was evaluated.
    pub templates: Option<&'static str>,
    /// The number of sets compared with a template.
    pub sets_checked: u64,
    /// The findings in file order of the sets.
    pub findings: Vec<PsetFinding>,
}

/// The deepest nesting of complex properties followed.
const MAX_COMPLEX_DEPTH: u32 = 16;

impl IfcFile {
    /// Compares every property set and element quantity named like a published template
    /// (`Pset_`, `Qto_`) with that template; see [`crate::psets`].
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` when the properties visited exceed `record_count` or complex
    /// properties nest deeper than `nesting_depth`; `base.cancelled` when the token was cancelled.
    pub fn check_property_sets(&self, limits: &Limits, cancel: &Cancel) -> Result<PsetCheck> {
        match PsetTemplates::of(&self.schema) {
            Some(templates) => check(self, templates, limits, cancel),
            None => Ok(PsetCheck {
                templates: None,
                sets_checked: 0,
                findings: Vec::new(),
            }),
        }
    }
}

/// Compares the sets of `file` with `templates`.
pub(super) fn check(
    file: &IfcFile,
    templates: &'static PsetTemplates,
    limits: &Limits,
    cancel: &Cancel,
) -> Result<PsetCheck> {
    let mut checker = Checker {
        file,
        tables: SchemaTables::of(&file.schema),
        limits,
        visited: 0,
        findings: Vec::new(),
    };
    let owners = checker.owners();
    let mut sets_checked = 0;
    for instance in file.instances() {
        cancel.check()?;
        let [record] = instance.records.as_slice() else {
            continue;
        };
        let quantities = match record.type_name.as_str() {
            "IFCPROPERTYSET" => false,
            "IFCELEMENTQUANTITY" => true,
            _ => continue,
        };
        let Some(name) = text(record.parameters.get(2)) else {
            continue;
        };
        if !(name.starts_with("Pset_") || name.starts_with("Qto_")) {
            continue;
        }
        let at = Set {
            instance: instance.name,
            name,
        };
        let Some(template) = templates.get(name) else {
            checker.find(&at, findings::PSET_TEMPLATE_UNKNOWN, "", "", name);
            continue;
        };
        sets_checked += 1;
        let kind = if quantities {
            SetKind::Quantities
        } else {
            SetKind::Properties
        };
        if kind != template.kind {
            let entity = |k| match k {
                SetKind::Properties => "IfcPropertySet",
                SetKind::Quantities => "IfcElementQuantity",
            };
            checker.find(
                &at,
                findings::PSET_SET_KIND,
                "",
                entity(template.kind),
                entity(kind),
            );
            continue;
        }
        let members = record.parameters.get(if quantities { 5 } else { 4 });
        checker.members(&at, template.properties, members, "", 0)?;
        for &object in owners.get(&instance.name).into_iter().flatten() {
            checker.applicability(&at, template, object);
        }
    }
    Ok(PsetCheck {
        templates: Some(templates.schema),
        sets_checked,
        findings: checker.findings,
    })
}

/// The set a finding is about.
struct Set<'a> {
    instance: u64,
    name: &'a str,
}

struct Checker<'a> {
    file: &'a IfcFile,
    tables: Option<&'static SchemaTables>,
    limits: &'a Limits,
    visited: u64,
    findings: Vec<PsetFinding>,
}

fn text(parameter: Option<&Parameter>) -> Option<&str> {
    match parameter {
        Some(Parameter::Text(t)) => Some(t),
        _ => None,
    }
}

fn reference_list(parameter: Option<&Parameter>) -> Vec<u64> {
    match parameter {
        Some(Parameter::Reference(r)) => vec![*r],
        Some(Parameter::List(items)) => items
            .iter()
            .filter_map(|p| match p {
                Parameter::Reference(r) => Some(*r),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The values of a list parameter, or the one value of a single parameter.
fn values(parameter: Option<&Parameter>) -> Vec<&Parameter> {
    match parameter {
        Some(Parameter::List(items)) => items.iter().collect(),
        Some(Parameter::Omitted) | None => Vec::new(),
        Some(other) => vec![other],
    }
}

/// The entity name of an occurrence for a type object name (`IFCWALLTYPE` gives `IFCWALL`).
fn occurrence_of(entity: &str) -> Option<&str> {
    entity
        .strip_suffix("TYPE")
        .or_else(|| entity.strip_suffix("STYLE"))
}

impl<'a> Checker<'a> {
    fn find(
        &mut self,
        set: &Set<'_>,
        code: ErrorCode,
        property: &str,
        expected: &str,
        found: &str,
    ) {
        self.findings.push(PsetFinding {
            code,
            set: set.instance,
            set_name: set.name.to_owned(),
            property: property.to_owned(),
            object: None,
            expected: expected.to_owned(),
            found: found.to_owned(),
        });
    }

    fn record(&self, name: u64) -> Option<&'a Record> {
        let file: &'a IfcFile = self.file;
        match file.instance(name)?.records.as_slice() {
            [record] => Some(record),
            _ => None,
        }
    }

    /// Whether an entity is a type object: by the schema tables, otherwise by its name.
    fn is_type_object(&self, entity: &str) -> bool {
        if let Some(t) = self.tables
            && let (Some(e), Some(root)) = (t.entity_index(entity), t.entity_index("IFCTYPEOBJECT"))
        {
            return t.is_subtype_of(e, root);
        }
        !entity.starts_with("IFCREL") && occurrence_of(entity).is_some()
    }

    /// The objects each set is assigned to: through `IfcRelDefinesByProperties` and through
    /// `HasPropertySets` of type objects.
    fn owners(&self) -> BTreeMap<u64, Vec<u64>> {
        let mut owners: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        for instance in self.file.instances() {
            let [record] = instance.records.as_slice() else {
                continue;
            };
            let p = |i: usize| record.parameters.get(i);
            if record.type_name == "IFCRELDEFINESBYPROPERTIES" {
                let objects = reference_list(p(4));
                for set in reference_list(p(5)) {
                    owners.entry(set).or_default().extend(&objects);
                }
            } else if self.is_type_object(&record.type_name) {
                for set in reference_list(p(5)) {
                    owners.entry(set).or_default().push(instance.name);
                }
            }
        }
        owners
    }

    fn members(
        &mut self,
        set: &Set<'_>,
        templates: &'static [PropertyTemplate],
        members: Option<&Parameter>,
        prefix: &str,
        depth: u32,
    ) -> Result<()> {
        if depth > MAX_COMPLEX_DEPTH.min(self.limits.nesting_depth) {
            return Err(limit("nesting_depth", u64::from(depth)));
        }
        for name in reference_list(members) {
            self.visited += 1;
            if self.visited > self.limits.record_count {
                return Err(limit("record_count", self.visited));
            }
            let Some(record) = self.record(name) else {
                continue;
            };
            let Some(property) = text(record.parameters.first()) else {
                continue;
            };
            let path = if prefix.is_empty() {
                property.to_owned()
            } else {
                format!("{prefix}.{property}")
            };
            let Some(template) = templates.iter().find(|t| t.name == property) else {
                self.find(set, findings::PSET_PROPERTY_UNKNOWN, &path, "", "");
                continue;
            };
            let expected = template.kind.entity();
            if record.type_name != expected {
                let code = if matches!(template.kind, PropertyKind::Quantity(_))
                    && record.type_name.starts_with("IFCQUANTITY")
                {
                    findings::PSET_QUANTITY_KIND
                } else {
                    findings::PSET_PROPERTY_KIND
                };
                self.find(set, code, &path, expected, &record.type_name);
                continue;
            }
            self.value(set, template, record, &path, depth)?;
        }
        Ok(())
    }

    fn value(
        &mut self,
        set: &Set<'_>,
        template: &'static PropertyTemplate,
        record: &Record,
        path: &str,
        depth: u32,
    ) -> Result<()> {
        let p = |i: usize| record.parameters.get(i);
        match &template.kind {
            PropertyKind::Single { data_type } => self.typed(set, path, data_type, values(p(2))),
            PropertyKind::Bounded { data_type } => {
                let mut bounds = values(p(2));
                bounds.extend(values(p(3)));
                bounds.extend(values(p(5)));
                self.typed(set, path, data_type, bounds);
            }
            PropertyKind::List { data_type } => self.typed(set, path, data_type, values(p(2))),
            PropertyKind::Table { defining, defined } => {
                self.typed(set, path, defining, values(p(2)));
                self.typed(set, path, defined, values(p(3)));
            }
            PropertyKind::Enumerated { items, .. } => {
                for value in values(p(2)) {
                    let item = match value {
                        Parameter::Typed { value, .. } => match value.as_ref() {
                            Parameter::Text(t) | Parameter::Enumeration(t) => t.as_str(),
                            _ => "",
                        },
                        _ => "",
                    };
                    if !items.iter().any(|i| i.eq_ignore_ascii_case(item)) {
                        self.find(
                            set,
                            findings::PSET_ENUMERATION_ITEM,
                            path,
                            &items.join(", "),
                            item,
                        );
                    }
                }
            }
            PropertyKind::Reference { entity } => {
                if let Some(Parameter::Reference(target)) = p(3)
                    && let Some(found) = self.record(*target).map(|r| r.type_name.clone())
                    && self.subtype(&found, entity) == Some(false)
                {
                    self.find(set, findings::PSET_VALUE_TYPE, path, entity, &found);
                }
            }
            PropertyKind::Complex { properties, .. } => {
                self.members(set, properties, p(3), path, depth + 1)?;
            }
            PropertyKind::Quantity(_) => {}
        }
        Ok(())
    }

    /// Checks that every value is wrapped in `data_type`; an open data type accepts any value.
    fn typed(&mut self, set: &Set<'_>, path: &str, data_type: &str, values: Vec<&Parameter>) {
        if data_type.is_empty() {
            return;
        }
        for value in values {
            let found = match value {
                Parameter::Typed { type_name, .. } => type_name.as_str(),
                Parameter::Omitted => continue,
                _ => "",
            };
            if !found.eq_ignore_ascii_case(data_type) {
                self.find(set, findings::PSET_VALUE_TYPE, path, data_type, found);
            }
        }
    }

    /// Whether `entity` is `ancestor` or a subtype of it; `None` without schema tables or for a
    /// name they do not hold.
    fn subtype(&self, entity: &str, ancestor: &str) -> Option<bool> {
        let t = self.tables?;
        Some(t.is_subtype_of(t.entity_index(entity)?, t.entity_index(ancestor)?))
    }

    /// The `PredefinedType` of an instance as an enumeration value, by the schema tables.
    fn predefined_type<'r>(&self, record: &'r Record) -> Option<&'r str> {
        let t = self.tables?;
        let entity = t.entity_index(&record.type_name)?;
        let at = t
            .positions(entity)
            .iter()
            .position(|p| p.attribute.name == "PredefinedType")?;
        match record.parameters.get(at) {
            Some(Parameter::Enumeration(v)) => Some(v),
            _ => None,
        }
    }

    /// Reports an object the template does not apply to. The entity of a type object stands for
    /// the entity of its occurrences, on both sides; an entity or predefined type that cannot be
    /// resolved is not judged.
    fn applicability(&mut self, set: &Set<'_>, template: &SetTemplate, object: u64) {
        let Some(record) = self.record(object) else {
            return;
        };
        let entity = record.type_name.as_str();
        let is_type = self.is_type_object(entity);
        let mut judged = false;
        let mut matching = Vec::new();
        for applicable in template.applicable {
            let target = applicable.entity.to_ascii_uppercase();
            let mut pairs = vec![(entity, target.as_str())];
            if is_type && let Some(occurrence) = occurrence_of(entity) {
                pairs.push((occurrence, occurrence_of(&target).unwrap_or(&target)));
            }
            for (own, other) in pairs {
                match self.subtype(own, other) {
                    Some(true) => {
                        judged = true;
                        matching.push(applicable.predefined_type);
                    }
                    Some(false) => judged = true,
                    None => {}
                }
            }
        }
        if !judged {
            return;
        }
        let names = || {
            template
                .applicable
                .iter()
                .map(|a| match a.predefined_type {
                    Some(p) => format!("{}/{p}", a.entity),
                    None => a.entity.to_owned(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let predefined = self.predefined_type(record);
        let fits = matching.iter().any(|restriction| match restriction {
            None => true,
            Some(required) => match predefined {
                Some(value) if value != "USERDEFINED" && value != "NOTDEFINED" => {
                    value.eq_ignore_ascii_case(required)
                }
                // The occurrence defers to its type or names its own kind: not judged.
                _ => true,
            },
        });
        if !fits {
            let found = match predefined {
                Some(p) if !matching.is_empty() => format!("{entity}/{p}"),
                _ => entity.to_owned(),
            };
            let expected = names();
            self.findings.push(PsetFinding {
                code: findings::PSET_NOT_APPLICABLE,
                set: set.instance,
                set_name: set.name.to_owned(),
                property: String::new(),
                object: Some(object),
                expected,
                found,
            });
        }
    }
}
