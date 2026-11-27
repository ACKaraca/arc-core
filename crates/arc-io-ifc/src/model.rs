//! The neutral model of an IFC file: the schema, the header records and the instances of the
//! data section, indexed once by instance name and by entity type name.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

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

/// The physical file a model was read from, shared by the clones of the model. Its `Debug` form
/// gives only the length.
#[derive(Clone, PartialEq, Default)]
pub(crate) struct SourceBytes(pub(crate) Arc<[u8]>);

impl std::fmt::Debug for SourceBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SourceBytes({} bytes)", self.0.len())
    }
}

/// Where the read content sits in the source, for writing it back (see [`IfcFile::write_step`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Layout {
    /// The physical file, one copy for the whole model.
    pub(crate) source: SourceBytes,
    /// The header records as read, with their spans.
    pub(crate) header: Vec<(Record, Range<usize>)>,
    /// The span of each instance read, by position in `IfcFile::instances`; `None` for an added
    /// instance.
    pub(crate) spans: Vec<Option<Range<usize>>>,
    /// The spans of the instances read and then removed.
    pub(crate) removed: Vec<Range<usize>>,
    /// The instances read and then replaced or changed.
    pub(crate) modified: BTreeSet<u64>,
    /// The offset of the `ENDSEC` closing the last data section, where added instances go.
    pub(crate) data_end: usize,
    /// The name the next added instance receives: above every name the file ever held.
    pub(crate) next_name: u64,
}

/// A parsed IFC file.
#[derive(Debug, Clone, PartialEq)]
pub struct IfcFile {
    /// The schema named by `FILE_SCHEMA`.
    pub schema: Schema,
    /// The header records, in file order (`FILE_DESCRIPTION`, `FILE_NAME`, `FILE_SCHEMA`, ...).
    /// They may be edited in place; [`IfcFile::write_step`] writes a record that differs from the
    /// one read in the canonical form and keeps the others as read.
    pub header: Vec<Record>,
    pub(crate) instances: Vec<Instance>,
    pub(crate) by_name: BTreeMap<u64, usize>,
    pub(crate) by_type: BTreeMap<String, Vec<usize>>,
    parsed_bytes: u64,
    pub(crate) layout: Layout,
}

impl IfcFile {
    /// Builds the file and its indexes. `instances` must have distinct names; the parser checks
    /// that before it calls this. `layout.spans` is parallel to `instances`.
    pub(crate) fn new(
        schema: Schema,
        instances: Vec<Instance>,
        parsed_bytes: u64,
        layout: Layout,
    ) -> Self {
        let header = layout
            .header
            .iter()
            .map(|(record, _)| record.clone())
            .collect();
        let mut file = Self {
            schema,
            header,
            instances,
            by_name: BTreeMap::new(),
            by_type: BTreeMap::new(),
            parsed_bytes,
            layout,
        };
        file.reindex();
        file.layout.next_name = file
            .by_name
            .last_key_value()
            .map_or(1, |(name, _)| name.saturating_add(1));
        file
    }

    /// Rebuilds the indexes by name and by type from the instance list.
    pub(crate) fn reindex(&mut self) {
        self.by_name.clear();
        self.by_type.clear();
        for (position, instance) in self.instances.iter().enumerate() {
            self.by_name.insert(instance.name, position);
            for record in &instance.records {
                let positions = self.by_type.entry(record.type_name.clone()).or_default();
                if positions.last() != Some(&position) {
                    positions.push(position);
                }
            }
        }
    }

    /// The byte span of an instance in the physical file it was read from, from its `#` to its
    /// `;`. `None` for an instance that was added, replaced or changed since, or is not in the file.
    #[must_use]
    pub fn span(&self, name: u64) -> Option<Range<u64>> {
        if self.layout.modified.contains(&name) {
            return None;
        }
        let position = *self.by_name.get(&name)?;
        let span = self.layout.spans.get(position)?.as_ref()?;
        Some(to_u64(span.start)..to_u64(span.end))
    }

    /// The source text of an instance that is unchanged since it was read (see [`IfcFile::span`]).
    #[must_use]
    pub fn source_text(&self, name: u64) -> Option<&[u8]> {
        let span = self.span(name)?;
        let start = usize::try_from(span.start).ok()?;
        let end = usize::try_from(span.end).ok()?;
        self.layout.source.0.get(start..end)
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

fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
