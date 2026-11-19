//! The entry point of the reader: a physical file, or an IFC-ZIP archive holding one.

use arc_base::{Error, Limits, Result};
use arc_io_common::ReadOptions as ArchiveOptions;
use arc_io_common::zip::ZipArchive;

use crate::error::{codes, reasons};
use crate::loss::{LossHeader, LossLog, LossReport};
use crate::model::{IfcFile, Parameter, Record, Schema};
use crate::options::{ReadOptions, ReadOutput};
use crate::step;
use crate::views::UNDERSTOOD;

/// Reads an IFC file. The bytes are either the STEP physical file itself or a ZIP archive whose
/// single `.ifc` entry is the physical file; the ZIP signature selects which.
///
/// # Errors
///
/// `io.limit_exceeded` when a limit of the profile is exceeded; `io.ifc.*` for malformed input;
/// `io.ifc.archive_entry_missing` or `io.ifc.archive_entry_ambiguous` for an archive with no or
/// several `.ifc` entries; `io.zip.*` for a malformed archive; `base.cancelled` when the token was
/// cancelled.
pub fn read(bytes: &[u8], options: &mut ReadOptions<'_>) -> Result<ReadOutput> {
    // A local header starts a non-empty archive; an empty archive starts with its end record.
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        let archive_options = ArchiveOptions {
            limits: options.limits,
            cancel: options.cancel.clone(),
        };
        let mut archive = ZipArchive::open(bytes, &archive_options)?;
        let mut found = None;
        for (index, entry) in archive.entries().iter().enumerate() {
            if !entry.is_directory() && entry.name.to_ascii_lowercase().ends_with(".ifc") {
                if found.is_some() {
                    return Err(Error::new(codes::IFC_ARCHIVE_ENTRY_AMBIGUOUS));
                }
                found = Some(index);
            }
        }
        let index = found.ok_or_else(|| Error::new(codes::IFC_ARCHIVE_ENTRY_MISSING))?;
        let data = archive.read_entry(index)?;
        read_physical(&data, options)
    } else {
        read_physical(bytes, options)
    }
}

fn read_physical(data: &[u8], options: &mut ReadOptions<'_>) -> Result<ReadOutput> {
    let limits: Limits = options.limits;
    let total = u64::try_from(data.len()).unwrap_or(u64::MAX);
    let parsed = {
        let mut control = options.control();
        let parsed = step::parse(data, &limits, &mut control)?;
        control.finish(total);
        parsed
    };
    let schema = schema_of(&parsed.header)?;
    let file = IfcFile::new(schema, parsed.header, parsed.instances);
    let report = report_of(&file, limits);
    Ok(ReadOutput { file, report })
}

/// The schema named by the first identifier of `FILE_SCHEMA`.
fn schema_of(header: &[Record]) -> Result<Schema> {
    let missing = || Error::new(codes::IFC_STRUCTURE_INVALID);
    let record = header
        .iter()
        .find(|record| record.type_name == "FILE_SCHEMA")
        .ok_or_else(missing)?;
    let Some(Parameter::List(names)) = record.parameters.first() else {
        return Err(missing());
    };
    match names.first() {
        Some(Parameter::Text(identifier)) => Ok(Schema::from_identifier(identifier)),
        _ => Err(missing()),
    }
}

/// The loss report of a read: entity types the typed views do not read, references to instances
/// that are not in the file, and a schema this reader does not know.
fn report_of(file: &IfcFile, limits: Limits) -> LossReport {
    let mut log = LossLog::default();
    if matches!(file.schema, Schema::Other(_)) {
        log.add(reasons::SCHEMA_UNRECOGNIZED, "FILE_SCHEMA", None, 1);
    }
    for instance in file.instances() {
        let source = instance.name.to_string();
        for record in &instance.records {
            if !UNDERSTOOD.contains(&record.type_name.as_str()) {
                log.add(
                    reasons::ENTITY_PRESERVED,
                    &record.type_name,
                    Some(&source),
                    1,
                );
            }
            let mut stack: Vec<&Parameter> = record.parameters.iter().collect();
            while let Some(parameter) = stack.pop() {
                match parameter {
                    Parameter::Reference(target) if !file.contains(*target) => {
                        log.add(
                            reasons::REFERENCE_UNRESOLVED,
                            &record.type_name,
                            Some(&source),
                            1,
                        );
                    }
                    Parameter::Typed { value, .. } => stack.push(value),
                    Parameter::List(items) => stack.extend(items.iter()),
                    _ => {}
                }
            }
        }
    }
    log.finish(LossHeader::import(file.schema.identifier(), limits))
}
