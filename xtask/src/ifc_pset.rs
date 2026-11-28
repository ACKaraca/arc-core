//! `cargo xtask ifc-psets --psd-dir <dir> [--check]`: generates the property and quantity set
//! templates of `arc-io-ifc` from the property set definitions (PSD XML) published by
//! buildingSMART International.
//!
//! `<dir>` is a checkout of the `Standards-builds` repository of buildingSMART International; the
//! task reads the PSD and QTO folders of IFC2X3 TC1 and IFC4 ADD2 TC1 from it, never downloads, and
//! checks each folder against a pinned BLAKE3 digest of its manifest (the sorted lines
//! `<path>\t<BLAKE3 of the file>`). The output is one file per schema under
//! `crates/arc-io-ifc/src/schema/generated/`, which is not stored in the repository until the
//! license basis of ADR-0018 is recorded; `--check` regenerates in memory and fails when a file
//! differs.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ifc_schema::output_dir;
use crate::workspace::Failure;

/// A folder of published definition files.
pub struct Folder {
    /// The path below the PSD directory.
    pub path: &'static str,
    /// The BLAKE3 digest of the folder's manifest.
    pub blake3: &'static str,
}

/// The published definitions of one schema.
pub struct Source {
    /// The schema name written into the tables.
    pub schema: &'static str,
    /// The static in the generated file; its lower-case form names the file `psets_<table>.rs`.
    pub table: &'static str,
    /// The folders read, recursively.
    pub folders: &'static [Folder],
}

/// IFC2X3 TC1 and IFC4 ADD2 TC1. IFC4X3 ADD2 publishes its property sets only as documentation
/// pages, not as PSD XML.
pub const SOURCES: [Source; 2] = [
    Source {
        schema: "IFC2X3",
        table: "IFC2X3",
        folders: &[Folder {
            path: "IFC/RELEASE/IFC2x3/TC1/HTML/psd",
            blake3: "cc826797d4e35afd4e07117d6941bd04a6bc39b0620ee8ee4cd472c610d3f268",
        }],
    },
    Source {
        schema: "IFC4",
        table: "IFC4",
        folders: &[
            Folder {
                path: "IFC/RELEASE/IFC4/ADD2_TC1/HTML/psd",
                blake3: "48bd17998c7e91314d70b5df959e98439790587ba187642ee84f0d4fcac21c23",
            },
            Folder {
                path: "IFC/RELEASE/IFC4/ADD2_TC1/HTML/qto",
                blake3: "9e3f018e4404287a90055f4fe823ab7ec1b545fe38e8271ebb980d5845419d16",
            },
        ],
    },
];

/// Largest definition file read.
const MAX_FILE_BYTES: u64 = 4 << 20;
/// Largest number of files in one folder.
const MAX_FILES: usize = 4096;
/// Deepest element nesting accepted.
const MAX_DEPTH: usize = 64;

// ---------------------------------------------------------------------------------------------
// A small XML reader for the definition files: elements, attributes and text, with comments,
// processing instructions and the declaration skipped. The files are pinned by digest; the
// reader still bounds depth and refuses what it does not understand.

/// An element with its attributes, child elements and concatenated text.
#[derive(Debug, Default)]
pub struct Node {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<Node>,
    pub text: String,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }
}

fn decode(raw: &str) -> Result<String, String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let end = rest[at..]
            .find(';')
            .ok_or_else(|| "unterminated reference".to_owned())?;
        let entity = &rest[at + 1..at + end];
        let ch = match entity {
            "lt" => '<',
            "gt" => '>',
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    entity.strip_prefix('#').and_then(|d| d.parse().ok())
                };
                code.and_then(char::from_u32)
                    .ok_or_else(|| format!("unknown reference &{entity};"))?
            }
        };
        out.push(ch);
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Parses a document and returns its root element.
pub fn parse_xml(text: &str) -> Result<Node, String> {
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut rest = text.strip_prefix('\u{feff}').unwrap_or(text);
    while !rest.is_empty() {
        let Some(at) = rest.find('<') else {
            let top = stack.last_mut().ok_or("unbalanced")?;
            top.text.push_str(&decode(rest)?);
            break;
        };
        if at > 0 {
            let top = stack.last_mut().ok_or("unbalanced")?;
            top.text.push_str(&decode(&rest[..at])?);
        }
        rest = &rest[at..];
        let skip = |rest: &str, end: &str| {
            rest.find(end)
                .map(|i| i + end.len())
                .ok_or_else(|| format!("unterminated {end}"))
        };
        if rest.starts_with("<?") {
            rest = &rest[skip(rest, "?>")?..];
        } else if rest.starts_with("<!--") {
            rest = &rest[skip(rest, "-->")?..];
        } else if rest.starts_with("<!") {
            return Err("document type declarations and CDATA are not read".to_owned());
        } else if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>').ok_or("unterminated end tag")?;
            let name = after[..end].trim();
            let node = stack.pop().ok_or("unbalanced")?;
            if node.name != name {
                return Err(format!("</{name}> closes <{}>", node.name));
            }
            stack.last_mut().ok_or("unbalanced")?.children.push(node);
            rest = &after[end + 1..];
        } else {
            let end = rest.find('>').ok_or("unterminated start tag")?;
            let tag = &rest[1..end];
            let (tag, empty) = match tag.strip_suffix('/') {
                Some(t) => (t, true),
                None => (tag, false),
            };
            let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
            let mut node = Node {
                name: tag[..name_end].to_owned(),
                ..Node::default()
            };
            let mut attrs = tag[name_end..].trim();
            while !attrs.is_empty() {
                let eq = attrs.find('=').ok_or("attribute without value")?;
                let name = attrs[..eq].trim().to_owned();
                let value = attrs[eq + 1..].trim_start();
                let quote = value.chars().next().ok_or("attribute without value")?;
                if quote != '"' && quote != '\'' {
                    return Err("unquoted attribute".to_owned());
                }
                let close = value[1..].find(quote).ok_or("unterminated attribute")?;
                node.attributes.push((name, decode(&value[1..=close])?));
                attrs = value[close + 2..].trim_start();
            }
            if empty {
                stack.last_mut().ok_or("unbalanced")?.children.push(node);
            } else {
                if stack.len() > MAX_DEPTH {
                    return Err("elements nested too deeply".to_owned());
                }
                stack.push(node);
            }
            rest = &rest[end + 1..];
        }
    }
    let mut document = stack.pop().ok_or("unbalanced")?;
    if !stack.is_empty() {
        return Err("unclosed element".to_owned());
    }
    match document.children.len() {
        1 => document.children.pop().ok_or_else(|| "no root".to_owned()),
        n => Err(format!("{n} root elements")),
    }
}

// ---------------------------------------------------------------------------------------------
// The definitions.

/// The kind of a property or quantity template.
#[derive(Debug, PartialEq, Eq)]
pub enum Kind {
    Single(String),
    Enumerated { name: String, items: Vec<String> },
    Bounded(String),
    List(String),
    Table { defining: String, defined: String },
    Reference(String),
    Complex { name: String, properties: Vec<Prop> },
    Quantity(&'static str),
}

/// A property or quantity template.
#[derive(Debug, PartialEq, Eq)]
pub struct Prop {
    pub name: String,
    pub kind: Kind,
}

/// A property or quantity set template.
#[derive(Debug, PartialEq, Eq)]
pub struct SetDef {
    pub name: String,
    pub quantities: bool,
    pub template_type: String,
    /// Entities with an optional predefined type, as `(entity, predefined)`.
    pub applicable: Vec<(String, Option<String>)>,
    pub properties: Vec<Prop>,
}

/// The data type of a value definition; empty when the published definition leaves it open
/// (`<DataType />`).
fn data_type(node: &Node) -> Result<String, String> {
    node.child("DataType")
        .map(|d| d.attribute("type").unwrap_or_default().trim().to_owned())
        .ok_or_else(|| format!("{} without a data type", node.name))
}

fn property(node: &Node) -> Result<Prop, String> {
    let name = node
        .text_of("Name")
        .filter(|n| !n.is_empty())
        .ok_or("property without a name")?
        .to_owned();
    let kind = property_kind(node).map_err(|e| format!("{name}: {e}"))?;
    Ok(Prop { name, kind })
}

fn property_kind(node: &Node) -> Result<Kind, String> {
    let ty = node
        .child("PropertyType")
        .and_then(|t| t.children.first())
        .ok_or("no property type")?;
    Ok(match ty.name.as_str() {
        "TypePropertySingleValue" => Kind::Single(data_type(ty)?),
        // IFC2X3 writes the time series of performance histories this way; the data type is
        // the type of the series values, which the IFC4 definitions do not state either.
        "TypeSimpleProperty" => Kind::Reference("IfcTimeSeries".to_owned()),
        "TypePropertyEnumeratedValue" => {
            let list = ty.child("EnumList").ok_or("no enumeration")?;
            let mut items: Vec<String> = list
                .children("EnumItem")
                .map(|i| i.text.trim().to_owned())
                .collect();
            // Some IFC4 definitions leave the list empty and name the items only as constants.
            if items.is_empty() {
                items = ty
                    .child("ConstantList")
                    .into_iter()
                    .flat_map(|c| c.children("ConstantDef"))
                    .filter_map(|c| c.text_of("Name"))
                    .map(str::to_owned)
                    .collect();
            }
            Kind::Enumerated {
                name: list.attribute("name").unwrap_or_default().trim().to_owned(),
                items,
            }
        }
        "TypePropertyBoundedValue" => Kind::Bounded(data_type(ty)?),
        "TypePropertyListValue" => Kind::List(data_type(ty.child("ListValue").ok_or("no list")?)?),
        "TypePropertyTableValue" => Kind::Table {
            defining: data_type(ty.child("DefiningValue").ok_or("no defining value")?)?,
            defined: data_type(ty.child("DefinedValue").ok_or("no defined value")?)?,
        },
        "TypePropertyReferenceValue" => Kind::Reference(
            ty.attribute("reftype")
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or("reference without a type")?
                .to_owned(),
        ),
        "TypeComplexProperty" => {
            // IFC4 nests the definitions directly; IFC2X3 too.
            let properties = ty
                .children("PropertyDef")
                .map(property)
                .collect::<Result<_, _>>()?;
            Kind::Complex {
                name: ty.attribute("name").unwrap_or_default().trim().to_owned(),
                properties,
            }
        }
        other => return Err(format!("unknown property type {other}")),
    })
}

fn quantity(node: &Node) -> Result<Prop, String> {
    let name = node
        .text_of("Name")
        .filter(|n| !n.is_empty())
        .ok_or("quantity without a name")?
        .to_owned();
    let kind = match node.text_of("QtoType").unwrap_or_default() {
        "Q_LENGTH" => "Length",
        "Q_AREA" => "Area",
        "Q_VOLUME" => "Volume",
        "Q_COUNT" => "Count",
        "Q_WEIGHT" => "Weight",
        "Q_TIME" => "Time",
        other => return Err(format!("{name}: unknown quantity type {other}")),
    };
    Ok(Prop {
        name,
        kind: Kind::Quantity(kind),
    })
}

/// One applicable class: IFC4 writes `IfcActuator/ELECTRICACTUATOR` for a predefined type,
/// IFC2X3 sometimes prefixes the schema module in upper case (`IFCKERNEL/IfcProduct`).
fn applicable(class: &str) -> Option<(String, Option<String>)> {
    let class = class.trim();
    if class.is_empty() {
        return None;
    }
    Some(match class.split_once('/') {
        Some((module, entity)) if module.chars().all(|c| !c.is_ascii_lowercase()) => {
            (entity.trim().to_owned(), None)
        }
        Some((entity, predefined)) => {
            (entity.trim().to_owned(), Some(predefined.trim().to_owned()))
        }
        None => (class.to_owned(), None),
    })
}

/// Reads one definition file.
pub fn parse_definition(text: &str) -> Result<SetDef, String> {
    let root = parse_xml(text)?;
    let quantities = match root.name.as_str() {
        "PropertySetDef" => false,
        "QtoSetDef" => true,
        other => return Err(format!("root element {other}")),
    };
    let name = root
        .text_of("Name")
        .filter(|n| !n.is_empty())
        .ok_or("set without a name")?
        .to_owned();
    let mut seen = BTreeSet::new();
    let mut applicable_classes = Vec::new();
    for class in root
        .child("ApplicableClasses")
        .into_iter()
        .flat_map(|c| c.children("ClassName"))
    {
        if let Some(entry) = applicable(&class.text)
            && seen.insert(entry.clone())
        {
            applicable_classes.push(entry);
        }
    }
    let properties = if quantities {
        root.child("QtoDefs")
            .into_iter()
            .flat_map(|d| d.children("QtoDef"))
            .map(quantity)
            .collect::<Result<Vec<_>, _>>()?
    } else {
        root.child("PropertyDefs")
            .into_iter()
            .flat_map(|d| d.children("PropertyDef"))
            .map(property)
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut names = BTreeSet::new();
    if let Some(p) = properties.iter().find(|p| !names.insert(p.name.as_str())) {
        return Err(format!("{name}: property {} twice", p.name));
    }
    Ok(SetDef {
        template_type: root
            .attribute("templatetype")
            .unwrap_or_default()
            .trim()
            .to_owned(),
        name,
        quantities,
        applicable: applicable_classes,
        properties,
    })
}

// ---------------------------------------------------------------------------------------------
// Reading the folders and emitting the tables.

fn xml_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Failure> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.is_dir() {
            xml_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "xml") {
            out.push(path);
        }
        if out.len() > MAX_FILES {
            return Err(format!("{}: more than {MAX_FILES} files", dir.display()));
        }
    }
    Ok(())
}

/// Reads a folder: the files with their text, checked against the pinned manifest digest.
pub fn read_folder(root: &Path, folder: &Folder) -> Result<Vec<(String, String)>, Failure> {
    let dir = root.join(folder.path);
    let mut paths = Vec::new();
    xml_files(&dir, &mut paths)?;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let size = fs::metadata(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        if size > MAX_FILE_BYTES {
            return Err(format!(
                "{}: larger than {MAX_FILE_BYTES} bytes",
                path.display()
            ));
        }
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let relative = path
            .strip_prefix(&dir)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let digest = blake3::hash(&bytes).to_hex().to_string();
        let text =
            String::from_utf8(bytes).map_err(|e| format!("{}: not UTF-8: {e}", path.display()))?;
        files.push((relative, digest, text));
    }
    files.sort();
    let manifest: String = files
        .iter()
        .map(|(path, digest, _)| format!("{path}\t{digest}\n"))
        .collect();
    let digest = blake3::hash(manifest.as_bytes()).to_hex();
    if digest.as_str() != folder.blake3 {
        return Err(format!(
            "{}: manifest BLAKE3 {digest} differs from the pinned {} (not the published files)",
            dir.display(),
            folder.blake3
        ));
    }
    Ok(files.into_iter().map(|(p, _, t)| (p, t)).collect())
}

fn emit_props(props: &[Prop], indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    for p in props {
        let kind = match &p.kind {
            Kind::Single(t) => format!("PropertyKind::Single {{ data_type: {t:?} }}"),
            Kind::Enumerated { name, items } => {
                format!("PropertyKind::Enumerated {{ enumeration: {name:?}, items: &{items:?} }}")
            }
            Kind::Bounded(t) => format!("PropertyKind::Bounded {{ data_type: {t:?} }}"),
            Kind::List(t) => format!("PropertyKind::List {{ data_type: {t:?} }}"),
            Kind::Table { defining, defined } => {
                format!("PropertyKind::Table {{ defining: {defining:?}, defined: {defined:?} }}")
            }
            Kind::Reference(t) => format!("PropertyKind::Reference {{ entity: {t:?} }}"),
            Kind::Quantity(q) => format!("PropertyKind::Quantity(QuantityKind::{q})"),
            Kind::Complex { name, properties } => {
                let mut inner = String::new();
                emit_props(properties, indent + 4, &mut inner);
                format!("PropertyKind::Complex {{ name: {name:?}, properties: &[\n{inner}{pad}] }}")
            }
        };
        let _ = writeln!(
            out,
            "{pad}PropertyTemplate {{ name: {:?}, kind: {kind} }},",
            p.name
        );
    }
}

/// Parses the definitions of one schema and emits its tables.
pub fn generate(root: &Path, source: &Source) -> Result<(String, usize), Failure> {
    let mut sets = Vec::new();
    let mut digests = Vec::new();
    for folder in source.folders {
        for (path, text) in read_folder(root, folder)? {
            sets.push(parse_definition(&text).map_err(|e| format!("{path}: {e}"))?);
        }
        digests.push(format!("{} (BLAKE3 {})", folder.path, folder.blake3));
    }
    sets.sort_by_key(|s| s.name.to_ascii_uppercase());
    if let Some(w) = sets
        .windows(2)
        .find(|w| w[0].name.eq_ignore_ascii_case(&w[1].name))
    {
        return Err(format!("{}: defined twice", w[0].name));
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// @generated by `cargo xtask ifc-psets`. Do not edit. Translated from the property and\n\
         // quantity set definitions of {schema} published by buildingSMART International Ltd.\n\
         // (https://www.buildingsmart.org), with attribution as its notice requires. Sources:\n\
         // {sources}.\n\
         pub(super) static {table}: PsetTemplates = PsetTemplates {{\n    \
         schema: {schema:?},\n    sets: &[",
        schema = source.schema,
        table = source.table,
        sources = digests.join(", ")
    );
    for set in &sets {
        let applicable: Vec<String> = set
            .applicable
            .iter()
            .map(|(entity, predefined)| {
                format!("Applicable {{ entity: {entity:?}, predefined_type: {predefined:?} }}")
            })
            .collect();
        let _ = writeln!(
            out,
            "        SetTemplate {{\n            name: {:?},\n            kind: SetKind::{},\n            \
             template_type: {:?},\n            applicable: &[{}],\n            properties: &[",
            set.name,
            if set.quantities {
                "Quantities"
            } else {
                "Properties"
            },
            set.template_type,
            applicable.join(", ")
        );
        emit_props(&set.properties, 16, &mut out);
        out.push_str("            ],\n        },\n");
    }
    out.push_str("    ],\n};\n");
    Ok((out, sets.len()))
}

/// Runs the task with the arguments after `ifc-psets`.
pub fn run(args: &[String]) -> Result<(), Failure> {
    let mut dir = None;
    let mut check = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--psd-dir" => dir = iter.next().map(PathBuf::from),
            "--check" => check = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let Some(dir) = dir else {
        return Err("usage: cargo xtask ifc-psets --psd-dir <dir> [--check]".to_owned());
    };
    let out = output_dir();
    let mut stale = Vec::new();
    for source in &SOURCES {
        let (code, count) = generate(&dir, source)?;
        let path = out.join(format!("psets_{}.rs", source.table.to_ascii_lowercase()));
        if check {
            if fs::read_to_string(&path).ok().as_deref() != Some(code.as_str()) {
                stale.push(path.display().to_string());
            }
        } else {
            fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            fs::write(&path, code).map_err(|e| format!("{}: {e}", path.display()))?;
            println!("ifc-psets: wrote {} ({count} templates)", path.display());
        }
    }
    if stale.is_empty() {
        if check {
            println!("ifc-psets: generated templates are up to date");
        }
        Ok(())
    } else {
        Err(format!("out of date: {}", stale.join(", ")))
    }
}

#[cfg(test)]
mod tests;
