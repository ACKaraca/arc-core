//! Resolves the names of a parsed schema and writes the static tables of
//! `crates/arc-io-ifc/src/schema/mod.rs` as Rust source. Every table is sorted by the upper-case
//! name; cross references become indexes. Constructs the tables cannot hold (multiple
//! inheritance, supertype expressions other than one `ONEOF`, extensible or based-on types,
//! re-declared explicit attributes) fail the generation instead of being dropped.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::ast::{
    AggregateKind, AttrName, Bound, EntityDecl, Expr, Schema, Supertype, TypeBody, TypeSpec,
    WhereRule,
};

/// Where a name points.
#[derive(Clone, Copy)]
enum Target {
    Entity(u16),
    Defined(u16),
    Enumeration(u16),
    Select(u16),
    Function,
    Rule,
}

/// A schema with its declarations sorted and indexed.
pub struct Resolved<'a> {
    pub schema: &'a Schema,
    pub entities: Vec<&'a EntityDecl>,
    pub defined: Vec<&'a super::ast::TypeDecl>,
    pub enumerations: Vec<&'a super::ast::TypeDecl>,
    pub selects: Vec<&'a super::ast::TypeDecl>,
    pub functions: Vec<&'a super::ast::Algorithm>,
    pub rules: Vec<&'a super::ast::RuleDecl>,
    names: HashMap<String, Target>,
}

fn upper(name: &str) -> String {
    name.to_ascii_uppercase()
}

fn idx(i: usize, what: &str) -> Result<u16, String> {
    u16::try_from(i).map_err(|_| format!("more than {} {what}", u16::MAX))
}

impl<'a> Resolved<'a> {
    pub fn new(schema: &'a Schema) -> Result<Self, String> {
        let mut entities: Vec<_> = schema.entities.iter().collect();
        entities.sort_by_key(|e| upper(&e.name));
        let mut types: Vec<_> = schema.types.iter().collect();
        types.sort_by_key(|t| upper(&t.name));
        let kind = |t: &&super::ast::TypeDecl| match t.body {
            TypeBody::Defined(_) => 0,
            TypeBody::Enumeration { .. } => 1,
            TypeBody::Select { .. } => 2,
        };
        let pick = |k| {
            types
                .iter()
                .copied()
                .filter(|t| kind(t) == k)
                .collect::<Vec<_>>()
        };
        let (defined, enumerations, selects) = (pick(0), pick(1), pick(2));
        let mut functions: Vec<_> = schema.functions.iter().collect();
        functions.sort_by_key(|f| upper(&f.name));
        let mut rules: Vec<_> = schema.rules.iter().collect();
        rules.sort_by_key(|r| upper(&r.name));

        let mut names = HashMap::new();
        let mut add = |name: &str, target| match names.insert(upper(name), target) {
            None => Ok(()),
            Some(_) => Err(format!("{name} is declared twice")),
        };
        for (i, e) in entities.iter().enumerate() {
            add(&e.name, Target::Entity(idx(i, "entities")?))?;
        }
        for (i, t) in defined.iter().enumerate() {
            add(&t.name, Target::Defined(idx(i, "defined types")?))?;
        }
        for (i, t) in enumerations.iter().enumerate() {
            add(&t.name, Target::Enumeration(idx(i, "enumerations")?))?;
        }
        for (i, t) in selects.iter().enumerate() {
            add(&t.name, Target::Select(idx(i, "selects")?))?;
        }
        for f in &functions {
            add(&f.name, Target::Function)?;
        }
        for r in &rules {
            add(&r.name, Target::Rule)?;
        }
        Ok(Self {
            schema,
            entities,
            defined,
            enumerations,
            selects,
            functions,
            rules,
            names,
        })
    }

    fn entity(&self, name: &str) -> Result<u16, String> {
        match self.names.get(&upper(name)) {
            Some(Target::Entity(i)) => Ok(*i),
            _ => Err(format!("{name} is not an entity of {}", self.schema.name)),
        }
    }

    pub fn supertype(&self, e: &EntityDecl) -> Result<Option<u16>, String> {
        match e.subtype_of.as_slice() {
            [] => Ok(None),
            [one] => self.entity(one).map(Some),
            _ => Err(format!("{}: multiple inheritance is not supported", e.name)),
        }
    }

    /// The entity of the supertype chain of `start` that declares the explicit attribute `name`,
    /// with its position.
    fn declaring(&self, start: u16, name: &str) -> Result<(u16, u16), String> {
        let mut current = Some(start);
        for _ in 0..self.entities.len() {
            let Some(i) = current else { break };
            let e = self.entities[usize::from(i)];
            if let Some(p) = e
                .explicit
                .iter()
                .position(|a| matches!(&a.name, AttrName::Plain(n) if n.eq_ignore_ascii_case(name)))
            {
                return Ok((i, idx(p, "attributes")?));
            }
            current = self.supertype(e)?;
        }
        Err(format!("no supertype declares the attribute {name}"))
    }

    /// The Rust expression of a `TypeRef`.
    fn type_ref(&self, spec: &TypeSpec) -> Result<String, String> {
        Ok(match spec {
            TypeSpec::Integer => "TypeRef::Integer".into(),
            TypeSpec::Real(None) => "TypeRef::Real".into(),
            TypeSpec::Number => "TypeRef::Number".into(),
            TypeSpec::Boolean => "TypeRef::Boolean".into(),
            TypeSpec::Logical => "TypeRef::Logical".into(),
            TypeSpec::String { width, fixed } | TypeSpec::Binary { width, fixed } => {
                let width = match width {
                    None => "None".to_owned(),
                    Some(Expr::Integer(w)) => format!("Some({})", u32_of(*w)?),
                    Some(_) => return Err("a string or binary width is not a literal".into()),
                };
                let kind = if matches!(spec, TypeSpec::String { .. }) {
                    "String"
                } else {
                    "Binary"
                };
                format!("TypeRef::{kind} {{ width: {width}, fixed: {fixed} }}")
            }
            TypeSpec::Named(name) => format!("TypeRef::Named({})", self.named(name)?),
            TypeSpec::Aggregate(a) => {
                let kind = aggregate_kind(a.kind)?;
                let (lower, upper) = match &a.bounds {
                    Some((l, u)) => (bound(l)?, bound(u)?),
                    None => ("Bound::Value(0)".to_owned(), "Bound::Unbounded".to_owned()),
                };
                format!(
                    "TypeRef::Aggregate(&Aggregate {{ kind: {kind}, lower: {lower}, upper: {upper}, \
                     optional: {}, unique: {}, element: {} }})",
                    a.optional,
                    a.unique,
                    self.type_ref(&a.element)?
                )
            }
            TypeSpec::Real(Some(_)) | TypeSpec::Generic(_) | TypeSpec::GenericEntity(_) => {
                return Err(format!("{spec:?} cannot be the type of an attribute"));
            }
        })
    }

    fn named(&self, name: &str) -> Result<String, String> {
        Ok(match self.names.get(&upper(name)) {
            Some(Target::Entity(i)) => format!("Named::Entity({i})"),
            Some(Target::Defined(i)) => format!("Named::Defined({i})"),
            Some(Target::Enumeration(i)) => format!("Named::Enumeration({i})"),
            Some(Target::Select(i)) => format!("Named::Select({i})"),
            Some(Target::Function | Target::Rule) | None => {
                return Err(format!("{name} is not a type of {}", self.schema.name));
            }
        })
    }
}

fn u32_of(v: u64) -> Result<u32, String> {
    u32::try_from(v).map_err(|_| format!("bound {v} is too large"))
}

fn aggregate_kind(kind: AggregateKind) -> Result<&'static str, String> {
    Ok(match kind {
        AggregateKind::Array => "AggregateKind::Array",
        AggregateKind::List => "AggregateKind::List",
        AggregateKind::Set => "AggregateKind::Set",
        AggregateKind::Bag => "AggregateKind::Bag",
        AggregateKind::General => return Err("AGGREGATE cannot be the type of an attribute".into()),
    })
}

fn bound(b: &Bound) -> Result<String, String> {
    Ok(match b.expr {
        Expr::Integer(v) => format!("Bound::Value({})", u32_of(v)?),
        Expr::Indeterminate => "Bound::Unbounded".to_owned(),
        _ => format!("Bound::Expression({:?})", b.source),
    })
}

fn label(label: Option<&String>) -> String {
    label.map_or("None".to_owned(), |l| format!("Some({l:?})"))
}

fn where_rules(rules: &[WhereRule]) -> String {
    let mut out = String::from("&[");
    for r in rules {
        let _ = write!(
            out,
            "WhereRule {{ label: {}, source: {:?}, tree: None }}, ",
            label(r.label.as_ref()),
            r.source
        );
    }
    out.push(']');
    out
}

/// Writes the tables of `schema` as the static `static_name`.
pub fn emit(
    schema: &Schema,
    static_name: &str,
    file_name: &str,
    blake3: &str,
) -> Result<String, String> {
    let r = Resolved::new(schema)?;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// @generated by `cargo xtask ifc-schema` from {file_name} (BLAKE3 {blake3}).\n\
         // Do not edit. Translated from the EXPRESS schema {name} published by buildingSMART\n\
         // International Ltd. (https://www.buildingsmart.org), with attribution as its notice\n\
         // requires.\n\
         pub(super) static {static_name}: SchemaTables = SchemaTables {{\n    \
         name: {name:?},\n    source_blake3: {blake3:?},\n    entities: &[",
        name = schema.name
    );
    for e in &r.entities {
        emit_entity(&r, e, &mut out).map_err(|m| format!("{}: {m}", e.name))?;
    }
    out.push_str("    ],\n    defined_types: &[\n");
    for t in &r.defined {
        if let TypeBody::Defined(spec) = &t.body {
            let ty = r.type_ref(spec).map_err(|m| format!("{}: {m}", t.name))?;
            let _ = writeln!(
                out,
                "        DefinedType {{ name: {:?}, underlying: {ty}, where_rules: {} }},",
                t.name,
                where_rules(&t.where_rules)
            );
        }
    }
    out.push_str("    ],\n    enumerations: &[\n");
    for t in &r.enumerations {
        if let TypeBody::Enumeration {
            extensible,
            based_on,
            items,
        } = &t.body
        {
            no_extension(&t.name, *extensible, based_on.as_ref(), &t.where_rules)?;
            let _ = writeln!(
                out,
                "        Enumeration {{ name: {:?}, items: &{items:?} }},",
                t.name
            );
        }
    }
    out.push_str("    ],\n    selects: &[\n");
    for t in &r.selects {
        if let TypeBody::Select {
            extensible,
            based_on,
            members,
        } = &t.body
        {
            no_extension(&t.name, *extensible, based_on.as_ref(), &t.where_rules)?;
            let members = members
                .iter()
                .map(|m| r.named(m))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|m| format!("{}: {m}", t.name))?;
            let _ = writeln!(
                out,
                "        Select {{ name: {:?}, members: &[{}] }},",
                t.name,
                members.join(", ")
            );
        }
    }
    out.push_str("    ],\n    functions: &[\n");
    for f in &r.functions {
        let _ = writeln!(
            out,
            "        Function {{ name: {:?}, source: {:?}, tree: None }},",
            f.name, f.source
        );
    }
    out.push_str("    ],\n    rules: &[\n");
    for rule in &r.rules {
        let entities = rule
            .entities
            .iter()
            .map(|e| r.entity(e).map(|i| i.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let _ = writeln!(
            out,
            "        GlobalRule {{ name: {:?}, entities: &[{}], where_rules: {}, source: {:?}, tree: None }},",
            rule.name,
            entities.join(", "),
            where_rules(&rule.where_rules),
            rule.source
        );
    }
    out.push_str("    ],\n};\n");
    Ok(out)
}

fn no_extension(
    name: &str,
    extensible: bool,
    based_on: Option<&String>,
    rules: &[WhereRule],
) -> Result<(), String> {
    if extensible || based_on.is_some() {
        Err(format!(
            "{name}: extensible and based-on types are not supported"
        ))
    } else if !rules.is_empty() {
        Err(format!(
            "{name}: WHERE rules on enumeration and select types are not supported"
        ))
    } else {
        Ok(())
    }
}

fn emit_entity(r: &Resolved<'_>, e: &EntityDecl, out: &mut String) -> Result<(), String> {
    let supertype = r.supertype(e)?;
    let one_of = match &e.supertype_of {
        None => Vec::new(),
        Some(Supertype::OneOf(items)) => items
            .iter()
            .map(|item| match item {
                Supertype::Entity(name) => r.entity(name).map(|i| i.to_string()),
                _ => Err("nested supertype expressions are not supported".to_owned()),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("supertype expressions other than ONEOF are not supported".into()),
    };
    let _ = writeln!(
        out,
        "        Entity {{\n            name: {:?},\n            supertype: {supertype:?},\n            \
         is_abstract: {},\n            one_of: &[{}],\n            attributes: &[",
        e.name,
        e.is_abstract,
        one_of.join(", ")
    );
    for a in &e.explicit {
        let AttrName::Plain(name) = &a.name else {
            return Err("re-declared explicit attributes are not supported".into());
        };
        let _ = writeln!(
            out,
            "                Attribute {{ name: {name:?}, ty: {}, optional: {} }},",
            r.type_ref(&a.ty)?,
            a.optional
        );
    }
    out.push_str("            ],\n            derived: &[\n");
    for d in &e.derived {
        let (name, redeclares) = match &d.name {
            AttrName::Plain(name) => (name.clone(), "None".to_owned()),
            AttrName::Redeclared {
                entity,
                attribute,
                renamed: None,
            } => {
                let (owner, index) = r.declaring(r.entity(entity)?, attribute)?;
                (
                    attribute.clone(),
                    format!("Some(AttributeRef {{ entity: {owner}, index: {index} }})"),
                )
            }
            AttrName::Redeclared { .. } => return Err("RENAMED is not supported".into()),
        };
        let _ = writeln!(
            out,
            "                Derived {{ name: {name:?}, redeclares: {redeclares}, ty: {}, source: {:?} }},",
            r.type_ref(&d.ty)?,
            d.source
        );
    }
    out.push_str("            ],\n            inverses: &[\n");
    for i in &e.inverse {
        let AttrName::Plain(name) = &i.name else {
            return Err("re-declared inverse attributes are not supported".into());
        };
        let aggregate = match &i.aggregate {
            None => "None".to_owned(),
            Some((kind, bounds)) => {
                let (lower, upper) = match bounds {
                    Some((l, u)) => (bound(l)?, bound(u)?),
                    None => ("Bound::Value(0)".to_owned(), "Bound::Unbounded".to_owned()),
                };
                format!(
                    "Some(InverseAggregate {{ kind: {}, lower: {lower}, upper: {upper} }})",
                    aggregate_kind(*kind)?
                )
            }
        };
        let _ = writeln!(
            out,
            "                Inverse {{ name: {name:?}, entity: {}, attribute: {:?}, aggregate: {aggregate} }},",
            r.entity(&i.entity)?,
            i.for_attribute
        );
    }
    out.push_str("            ],\n            unique: &[\n");
    for u in &e.unique {
        let _ = writeln!(
            out,
            "                UniqueRule {{ label: {}, attributes: &{:?} }},",
            label(u.label.as_ref()),
            u.attributes
        );
    }
    let _ = writeln!(
        out,
        "            ],\n            where_rules: {},\n        }},",
        where_rules(&e.where_rules)
    );
    Ok(())
}
