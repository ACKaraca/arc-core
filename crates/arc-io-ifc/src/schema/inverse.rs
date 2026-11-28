//! The inverse index of a file: for every instance, the instances whose records refer to it, with
//! the record and the position of the reference. It is built on demand, once per file, and charged
//! against the caller's limits; named inverse attributes are resolved through the schema tables.

use std::collections::BTreeMap;

use arc_base::{Cancel, Limits, Result};

use crate::error::limit;
use crate::model::{IfcFile, Instance, Parameter};

use super::{AggregateKind, Inverse, SchemaTables};

/// Bytes charged for one entry of the index.
const ENTRY_BYTES: u64 = 32;

/// A reference to an instance: the referring instance, the record of it that holds the reference
/// and the position of the parameter (the reference may sit inside a list at that position).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Referrer {
    /// The referring instance.
    pub instance: u64,
    /// The index of the record in the referring instance.
    pub record: u32,
    /// The parameter position in that record.
    pub position: u32,
}

/// The instances referring to each instance, in file order; a referrer that refers twice through
/// one parameter is listed twice.
#[derive(Debug, Clone, Default)]
pub struct InverseIndex {
    referrers: BTreeMap<u64, Vec<Referrer>>,
}

impl IfcFile {
    /// Builds the inverse index of the file. Every entry counts against `record_count` and is
    /// charged against the share of `memory_bytes` left after the parser; cancellation is checked
    /// per instance.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` when a limit is exceeded, `base.cancelled` when cancelled.
    pub fn inverse_index(&self, limits: &Limits, cancel: &Cancel) -> Result<InverseIndex> {
        let memory = limits
            .memory_bytes
            .map(|max| max.saturating_sub(self.parsed_bytes()));
        let mut entries = 0u64;
        let mut index = InverseIndex::default();
        for instance in self.instances() {
            cancel.check()?;
            for (r, record) in instance.records.iter().enumerate() {
                for (p, parameter) in record.parameters.iter().enumerate() {
                    let mut targets = Vec::new();
                    references(parameter, &mut targets);
                    for target in targets {
                        entries = entries.saturating_add(1);
                        if entries > limits.record_count {
                            return Err(limit("record_count", entries));
                        }
                        let used = entries.saturating_mul(ENTRY_BYTES);
                        if memory.is_some_and(|max| used > max) {
                            return Err(limit("memory_bytes", used));
                        }
                        index.referrers.entry(target).or_default().push(Referrer {
                            instance: instance.name,
                            record: u32::try_from(r).unwrap_or(u32::MAX),
                            position: u32::try_from(p).unwrap_or(u32::MAX),
                        });
                    }
                }
            }
        }
        Ok(index)
    }
}

/// Collects the references of a parameter, through lists and typed parameters. The parser bounds
/// the nesting, so the recursion is bounded.
pub(crate) fn references(parameter: &Parameter, out: &mut Vec<u64>) {
    match parameter {
        Parameter::Reference(name) => out.push(*name),
        Parameter::List(items) => items.iter().for_each(|item| references(item, out)),
        Parameter::Typed { value, .. } => references(value, out),
        _ => {}
    }
}

impl InverseIndex {
    /// Every reference to `target`, in file order.
    #[must_use]
    pub fn referrers(&self, target: u64) -> &[Referrer] {
        self.referrers.get(&target).map_or(&[], Vec::as_slice)
    }

    /// The instances that make up the inverse attribute `name` (ignoring ASCII case) of the
    /// instance `target`: those of the inverse's entity, or a subtype, that refer to `target`
    /// through the inverse's attribute. A `SET` lists each instance once; a `BAG` and a single
    /// inverse list each reference. `None` without tables, for an unknown instance or entity, or
    /// when the entity has no such inverse attribute.
    #[must_use]
    pub fn inverse(&self, file: &IfcFile, target: u64, name: &str) -> Option<Vec<u64>> {
        let tables = file.tables()?;
        let instance = file.instance(target)?;
        let inverse = find_inverse(tables, instance, name)?;
        Some(self.members(file, tables, target, inverse))
    }

    /// The members of `inverse` for the instance `target`.
    pub(crate) fn members(
        &self,
        file: &IfcFile,
        tables: &SchemaTables,
        target: u64,
        inverse: &Inverse,
    ) -> Vec<u64> {
        let mut out: Vec<u64> = Vec::new();
        for referrer in self.referrers(target) {
            let Some(instance) = file.instance(referrer.instance) else {
                continue;
            };
            if refers_through(tables, instance, referrer, inverse) {
                let set = inverse
                    .aggregate
                    .is_some_and(|a| a.kind == AggregateKind::Set);
                if !(set && out.contains(&referrer.instance)) {
                    out.push(referrer.instance);
                }
            }
        }
        out
    }
}

/// The inverse attribute `name` of the instance's entity, the nearest declaration first; for a
/// complex instance, of any of its parts.
pub(crate) fn find_inverse(
    tables: &SchemaTables,
    instance: &Instance,
    name: &str,
) -> Option<&'static Inverse> {
    instance.records.iter().find_map(|record| {
        let entity = tables.entity_index(&record.type_name)?;
        tables
            .supertypes(entity)
            .filter_map(|e| tables.entities.get(usize::from(e)))
            .find_map(|e| {
                e.inverses
                    .iter()
                    .find(|i| i.name.eq_ignore_ascii_case(name))
            })
    })
}

/// Whether the reference `referrer` is made by an instance of the inverse's entity (or a subtype)
/// through the inverse's attribute.
fn refers_through(
    tables: &SchemaTables,
    instance: &Instance,
    referrer: &Referrer,
    inverse: &Inverse,
) -> bool {
    let of_entity = instance.records.iter().any(|r| {
        tables
            .entity_index(&r.type_name)
            .is_some_and(|e| tables.is_subtype_of(e, inverse.entity))
    });
    if !of_entity {
        return false;
    }
    let Some(record) = instance.records.get(referrer.record as usize) else {
        return false;
    };
    let Some(entity) = tables.entity_index(&record.type_name) else {
        return false;
    };
    let position = if instance.records.len() == 1 {
        tables
            .attribute_position(entity, inverse.attribute)
            .map(|(p, _)| p)
    } else {
        // A part of a complex instance holds only the attributes its entity declares.
        tables.entities.get(usize::from(entity)).and_then(|e| {
            e.attributes
                .iter()
                .position(|a| a.name.eq_ignore_ascii_case(inverse.attribute))
        })
    };
    position == Some(referrer.position as usize)
}
