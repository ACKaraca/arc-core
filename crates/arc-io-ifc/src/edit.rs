//! Editing a read file and writing it back.
//!
//! Instance names are kept: an edit never renumbers, and an added instance receives a name above
//! every name the file ever held, so a removed name is not given out again. No edit leaves a
//! reference to an instance that is not in the file. [`IfcFile::write_step`] copies every part of
//! the source that no edit touched byte for byte, comments and white space included, and writes
//! the edited records in the canonical form of `write/step.rs`.

use arc_base::{Error, Limits, Result};

use crate::error::codes;
use crate::model::{IfcFile, Instance, Parameter, Record};
use crate::step::MAX_NESTING;
use crate::write::step;

impl IfcFile {
    /// Adds an instance with the given records and returns its name, one above the highest name
    /// the file has held.
    ///
    /// # Errors
    ///
    /// `io.ifc.edit_value_invalid` for a value that cannot be written or when no name is left;
    /// `io.ifc.edit_reference_dangling` when a value refers to an instance that is not in the file.
    pub fn add(&mut self, records: Vec<Record>) -> Result<u64> {
        let name = self.layout.next_name;
        if name == u64::MAX {
            return Err(Error::new(codes::IFC_EDIT_VALUE_INVALID).with_arg("value", "name"));
        }
        self.check_records(&records, None)?;
        self.layout.next_name = name + 1;
        let position = self.instances.len();
        self.instances.push(Instance { name, records });
        self.layout.spans.push(None);
        self.by_name.insert(name, position);
        self.index_types(position);
        Ok(name)
    }

    /// Replaces the records of instance `name`; the name and the place in the file stay.
    ///
    /// # Errors
    ///
    /// `io.ifc.edit_target_missing` when the instance is not in the file; otherwise as
    /// [`IfcFile::add`].
    pub fn replace(&mut self, name: u64, records: Vec<Record>) -> Result<()> {
        let position = self.position(name)?;
        self.check_records(&records, Some(name))?;
        for positions in self.by_type.values_mut() {
            positions.retain(|&p| p != position);
        }
        self.by_type.retain(|_, positions| !positions.is_empty());
        if let Some(instance) = self.instances.get_mut(position) {
            instance.records = records;
        }
        self.index_types(position);
        self.layout.modified.insert(name);
        Ok(())
    }

    /// Sets parameter `index` of record `record` (0 for a simple instance) of instance `name`.
    ///
    /// # Errors
    ///
    /// `io.ifc.edit_target_missing` when the instance, the record or the parameter does not
    /// exist; otherwise as [`IfcFile::add`].
    pub fn set_attribute(
        &mut self,
        name: u64,
        record: usize,
        index: usize,
        value: Parameter,
    ) -> Result<()> {
        let position = self.position(name)?;
        self.check_parameter(&value, Some(name), 1)?;
        let slot = self
            .instances
            .get_mut(position)
            .and_then(|instance| instance.records.get_mut(record))
            .and_then(|record| record.parameters.get_mut(index))
            .ok_or_else(|| missing(name))?;
        *slot = value;
        self.layout.modified.insert(name);
        Ok(())
    }

    /// Removes instance `name` and returns it. Its line leaves the written file.
    ///
    /// The other instances are searched for references to it, and the indexes are rebuilt, so a
    /// removal costs time in proportion to the file.
    ///
    /// # Errors
    ///
    /// `io.ifc.edit_target_missing` when the instance is not in the file;
    /// `io.ifc.edit_reference_dangling` when another instance still refers to it.
    pub fn remove(&mut self, name: u64) -> Result<Instance> {
        let position = self.position(name)?;
        for instance in &self.instances {
            if instance.name != name && refers_to(&instance.records, name) {
                return Err(Error::new(codes::IFC_EDIT_REFERENCE_DANGLING)
                    .with_detail("name", clamp(name))
                    .with_detail("referrer", clamp(instance.name)));
            }
        }
        let instance = self.instances.remove(position);
        if position < self.layout.spans.len()
            && let Some(span) = self.layout.spans.remove(position)
        {
            self.layout.removed.push(span);
        }
        self.layout.modified.remove(&name);
        self.reindex();
        Ok(instance)
    }

    /// Writes the file as an ISO 10303-21 physical file.
    ///
    /// Without edits the output is the source byte for byte: the header, comments, white space,
    /// skipped sections and the bytes after `END-ISO-10303-21;` included (for an IFC-ZIP read, the
    /// source is the physical file inside the archive). An edited instance is written in the
    /// canonical form in place of its source text, so comments inside it are dropped; a removed
    /// instance drops its text and the line break after it; added instances go, one per line, before
    /// the `ENDSEC` of the last data section, in the order they were added. A header record that
    /// differs from the one read is written in the canonical form in its place; records added to the
    /// header follow the last header record.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` when an edited record exceeds `string_bytes`, `array_length` or
    /// `nesting_depth`; `io.ifc.write_value_invalid` for a value that cannot be written;
    /// `io.ifc.edit_value_invalid` for an edited header without `FILE_DESCRIPTION`, `FILE_NAME`
    /// and `FILE_SCHEMA` first.
    pub fn write_step(&self, limits: &Limits) -> Result<Vec<u8>> {
        let layout = &self.layout;
        let source: &[u8] = &layout.source.0;
        let unchanged_header = self.header.len() == layout.header.len()
            && self
                .header
                .iter()
                .zip(&layout.header)
                .all(|(now, (read, _))| now == read);
        if !unchanged_header {
            check_header(&self.header)?;
            for (index, record) in self.header.iter().enumerate() {
                if layout
                    .header
                    .get(index)
                    .is_none_or(|(read, _)| read != record)
                {
                    self.check_records(std::slice::from_ref(record), None)?;
                }
            }
        }
        let newline: &[u8] = if source
            .get(..layout.data_end)
            .is_some_and(|s| s.ends_with(b"\r\n"))
        {
            b"\r\n"
        } else {
            b"\n"
        };
        let mut out = Vec::with_capacity(source.len());
        let mut text = String::new();
        let mut cursor = 0;
        let read = layout.header.len();
        for (index, (_, span)) in layout.header.iter().enumerate() {
            copy(&mut out, source, cursor, span.start);
            cursor = span.end;
            match self.header.get(index) {
                Some(now) if layout.header.get(index).is_some_and(|(r, _)| r == now) => {
                    copy(&mut out, source, span.start, span.end);
                }
                Some(now) => {
                    text.clear();
                    step::body(&mut text, &now.type_name, &now.parameters, limits)?;
                    out.extend_from_slice(text.as_bytes());
                    out.push(b';');
                }
                None => cursor = line_end(source, span.end),
            }
            if index + 1 == read {
                for added in self.header.iter().skip(read) {
                    text.clear();
                    step::body(&mut text, &added.type_name, &added.parameters, limits)?;
                    out.extend_from_slice(newline);
                    out.extend_from_slice(text.as_bytes());
                    out.push(b';');
                }
            }
        }
        // The read instances that are left and the removed ones, in source order.
        let mut placed: Vec<(usize, usize, Option<&Instance>)> = self
            .instances
            .iter()
            .zip(&layout.spans)
            .filter_map(|(instance, span)| span.as_ref().map(|s| (s.start, s.end, Some(instance))))
            .chain(layout.removed.iter().map(|s| (s.start, s.end, None)))
            .collect();
        placed.sort_unstable_by_key(|&(start, ..)| start);
        for (start, end, instance) in placed {
            copy(&mut out, source, cursor, start);
            cursor = end;
            match instance {
                Some(instance) if layout.modified.contains(&instance.name) => {
                    text.clear();
                    step::instance(&mut text, instance.name, &instance.records, limits)?;
                    out.extend_from_slice(text.as_bytes());
                }
                Some(_) => copy(&mut out, source, start, end),
                None => cursor = line_end(source, end),
            }
        }
        copy(&mut out, source, cursor, layout.data_end);
        cursor = cursor.max(layout.data_end);
        let added = self
            .instances
            .iter()
            .zip(&layout.spans)
            .filter(|(_, span)| span.is_none());
        for (instance, _) in added {
            if !(out.ends_with(b"\n") || out.is_empty()) {
                out.extend_from_slice(newline);
            }
            text.clear();
            step::instance(&mut text, instance.name, &instance.records, limits)?;
            out.extend_from_slice(text.as_bytes());
            out.extend_from_slice(newline);
        }
        copy(&mut out, source, cursor, source.len());
        Ok(out)
    }

    fn position(&self, name: u64) -> Result<usize> {
        self.by_name
            .get(&name)
            .copied()
            .ok_or_else(|| missing(name))
    }

    /// Adds the instance at `position` to the index by type, keeping each list in file order.
    fn index_types(&mut self, position: usize) {
        let Some(instance) = self.instances.get(position) else {
            return;
        };
        for record in &instance.records {
            let positions = self.by_type.entry(record.type_name.clone()).or_default();
            if let Err(at) = positions.binary_search(&position) {
                positions.insert(at, position);
            }
        }
    }

    /// Checks that records can be written and refer only to instances of the file or to `own`.
    fn check_records(&self, records: &[Record], own: Option<u64>) -> Result<()> {
        if records.is_empty() {
            return Err(invalid("records"));
        }
        for record in records {
            if !is_type_name(&record.type_name) {
                return Err(invalid("type_name"));
            }
            for parameter in &record.parameters {
                self.check_parameter(parameter, own, 1)?;
            }
        }
        Ok(())
    }

    fn check_parameter(&self, value: &Parameter, own: Option<u64>, depth: u32) -> Result<()> {
        if depth > MAX_NESTING {
            return Err(invalid("nesting"));
        }
        match value {
            Parameter::Real(number) if !number.is_finite() => Err(invalid("real")),
            Parameter::Text(text) if text.contains('\0') => Err(invalid("text")),
            Parameter::Enumeration(word)
                if word.is_empty()
                    || !word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                Err(invalid("enumeration"))
            }
            Parameter::Binary(digits)
                if !(digits
                    .as_bytes()
                    .first()
                    .is_some_and(|b| (b'0'..=b'3').contains(b))
                    && digits
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))) =>
            {
                Err(invalid("binary"))
            }
            Parameter::Reference(target) if Some(*target) != own && !self.contains(*target) => {
                Err(Error::new(codes::IFC_EDIT_REFERENCE_DANGLING)
                    .with_detail("name", clamp(*target)))
            }
            Parameter::Typed { type_name, value } => {
                if !is_type_name(type_name) {
                    return Err(invalid("type_name"));
                }
                self.check_parameter(value, own, depth + 1)
            }
            Parameter::List(items) => items
                .iter()
                .try_for_each(|item| self.check_parameter(item, own, depth + 1)),
            _ => Ok(()),
        }
    }
}

fn missing(name: u64) -> Error {
    Error::new(codes::IFC_EDIT_TARGET_MISSING).with_detail("name", clamp(name))
}

fn invalid(value: &'static str) -> Error {
    Error::new(codes::IFC_EDIT_VALUE_INVALID).with_arg("value", value)
}

fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// An upper-case entity or type name as the reader stores it: a letter, then letters, digits and
/// `_`, optionally after `!`, at most 255 bytes.
fn is_type_name(name: &str) -> bool {
    let word = name.strip_prefix('!').unwrap_or(name);
    name.len() <= crate::step::MAX_WORD_BYTES
        && word.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && word
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// Whether a record refers to instance `name`, searched without recursion.
fn refers_to(records: &[Record], name: u64) -> bool {
    let mut stack: Vec<&Parameter> = records.iter().flat_map(|r| &r.parameters).collect();
    while let Some(parameter) = stack.pop() {
        match parameter {
            Parameter::Reference(target) if *target == name => return true,
            Parameter::Typed { value, .. } => stack.push(value),
            Parameter::List(items) => stack.extend(items),
            _ => {}
        }
    }
    false
}

/// Refuses an edited header that does not start with the three records ISO 10303-21 requires.
fn check_header(header: &[Record]) -> Result<()> {
    let first: Vec<&str> = header
        .iter()
        .take(3)
        .map(|r| r.type_name.as_str())
        .collect();
    if first == ["FILE_DESCRIPTION", "FILE_NAME", "FILE_SCHEMA"] {
        Ok(())
    } else {
        Err(invalid("header"))
    }
}

fn copy(out: &mut Vec<u8>, source: &[u8], start: usize, end: usize) {
    if let Some(bytes) = source.get(start..end) {
        out.extend_from_slice(bytes);
    }
}

/// The offset after the spaces, tabs and one line break that follow `at`, the end of a removed
/// statement.
fn line_end(source: &[u8], at: usize) -> usize {
    let rest = source.get(at..).unwrap_or_default();
    let blank = rest
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count();
    let after = rest.get(blank..).unwrap_or_default();
    let line = if after.starts_with(b"\r\n") {
        2
    } else {
        usize::from(after.starts_with(b"\n"))
    };
    at + blank + line
}
