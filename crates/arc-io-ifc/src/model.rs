//! The neutral model of an IFC file: the schema, the header records and the instances of the
//! data section, indexed once by instance name and by entity type name.

use std::collections::BTreeMap;

/// The schema named by `FILE_SCHEMA`. A version suffix such as `_ADD2` or `_TC1` does not change
/// the schema; any other name is kept as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schema {
    /// IFC2X3.
    Ifc2x3,
    /// IFC4.
    Ifc4,
    /// IFC4X3.
    Ifc4x3,
    /// A schema this reader does not know, with the identifier in upper case.
    Other(String),
}

impl Schema {
    /// The schema of a `FILE_SCHEMA` identifier, compared case-insensitively.
    #[must_use]
    pub fn from_identifier(identifier: &str) -> Self {
        let upper = identifier.to_ascii_uppercase();
        let base = upper.split('_').next().unwrap_or_default();
        match base {
            "IFC2X3" => Self::Ifc2x3,
            "IFC4" => Self::Ifc4,
            "IFC4X3" => Self::Ifc4x3,
            _ => Self::Other(upper),
        }
    }

    /// The identifier written in the header of the schema, for `Other` the stored name.
    #[must_use]
    pub fn identifier(&self) -> &str {
        match self {
            Self::Ifc2x3 => "IFC2X3",
            Self::Ifc4 => "IFC4",
            Self::Ifc4x3 => "IFC4X3",
            Self::Other(name) => name,
        }
    }
}

/// One parameter of a record.
#[derive(Debug, Clone, PartialEq)]
pub enum Parameter {
    /// An integer literal.
    Integer(i64),
    /// A real literal; always finite.
    Real(f64),
    /// A string, decoded from its encoded forms to UTF-8.
    Text(String),
    /// An enumeration such as `.T.` (stored without the dots, as written).
    Enumeration(String),
    /// A binary literal, the hexadecimal digits between the quotes as written.
    Binary(String),
    /// A reference to an instance by its name.
    Reference(u64),
    /// `$`: the attribute has no value.
    Omitted,
    /// `*`: the attribute is derived.
    Derived,
    /// A typed parameter such as `IFCLABEL('x')`; the type name is in upper case.
    Typed {
        /// The type name, in upper case.
        type_name: String,
        /// The wrapped value.
        value: Box<Parameter>,
    },
    /// A parenthesized list.
    List(Vec<Parameter>),
}

/// A typed record: an entity type name and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The entity type name, in upper case.
    pub type_name: String,
    /// The parameters in order.
    pub parameters: Vec<Parameter>,
}

/// An instance of the data section: its name and one record, or several for a complex instance.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// The instance name, the number after `#`.
    pub name: u64,
    /// The records; a simple instance has one, a complex instance one per part.
    pub records: Vec<Record>,
}

/// A parsed IFC file.
#[derive(Debug, Clone, PartialEq)]
pub struct IfcFile {
    /// The schema named by `FILE_SCHEMA`.
    pub schema: Schema,
    /// The header records, in file order (`FILE_DESCRIPTION`, `FILE_NAME`, `FILE_SCHEMA`, ...).
    pub header: Vec<Record>,
    instances: Vec<Instance>,
    by_name: BTreeMap<u64, usize>,
    by_type: BTreeMap<String, Vec<usize>>,
    parsed_bytes: u64,
}

impl IfcFile {
    /// Builds the file and its indexes. `instances` must have distinct names; the parser checks
    /// that before it calls this.
    pub(crate) fn new(
        schema: Schema,
        header: Vec<Record>,
        instances: Vec<Instance>,
        parsed_bytes: u64,
    ) -> Self {
        let mut by_name = BTreeMap::new();
        let mut by_type: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (position, instance) in instances.iter().enumerate() {
            by_name.insert(instance.name, position);
            for record in &instance.records {
                let positions = by_type.entry(record.type_name.clone()).or_default();
                if positions.last() != Some(&position) {
                    positions.push(position);
                }
            }
        }
        Self {
            schema,
            header,
            instances,
            by_name,
            by_type,
            parsed_bytes,
        }
    }

    /// The estimate of the memory the parser allocated for this file, the amount charged against
    /// `memory_bytes` while reading. The typed views share the rest of `memory_bytes`.
    #[must_use]
    pub fn parsed_bytes(&self) -> u64 {
        self.parsed_bytes
    }

    /// The instances of the data section, in file order.
    #[must_use]
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// The instance with the given name, found by binary search.
    #[must_use]
    pub fn instance(&self, name: u64) -> Option<&Instance> {
        self.by_name
            .get(&name)
            .and_then(|&position| self.instances.get(position))
    }

    /// Whether an instance with the given name exists.
    #[must_use]
    pub fn contains(&self, name: u64) -> bool {
        self.by_name.contains_key(&name)
    }

    /// The instances with a record of the given type, in file order, from the index built once
    /// at read time. The name is compared in upper case.
    pub fn instances_of<'a>(&'a self, type_name: &str) -> impl Iterator<Item = &'a Instance> + 'a {
        let positions = self
            .by_type
            .get(&type_name.to_ascii_uppercase())
            .map_or(&[][..], Vec::as_slice);
        positions
            .iter()
            .filter_map(move |&position| self.instances.get(position))
    }
}
