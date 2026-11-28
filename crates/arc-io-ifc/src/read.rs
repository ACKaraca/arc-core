//! The entry point of the reader: a physical file, or an IFC-ZIP archive holding one.

use std::sync::Arc;

use arc_base::{Cancel, Error, Limits, Result};
use arc_io_common::ReadOptions as ArchiveOptions;
use arc_io_common::zip::ZipArchive;

use crate::error::{codes, reasons};
use crate::loss::{Direction, LossHeader, LossLog, LossReport, Source};
use crate::model::{IfcFile, Layout, Parameter, Record, Schema, SourceBytes};
use crate::options::{ReadOptions, ReadOutput};
use crate::step::{self, Notes};
use crate::views::{UNDERSTOOD, UNDERSTOOD_IMPORT};

/// Reads an IFC file. The bytes are either the STEP physical file itself or a ZIP archive whose
/// single `.ifc` entry is the physical file; the ZIP signature selects which. Resource-fork and
/// metadata entries (`__MACOSX/` and names that start with `._`) never count as the model.
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
        // The archive inflates no entry beyond the input bound, so an entry cannot be larger than
        // the input the profile allows.
        let limits = Limits {
            decompressed_bytes: options
                .limits
                .decompressed_bytes
                .min(options.limits.input_bytes),
            ..options.limits
        };
        let archive_options = ArchiveOptions {
            limits,
            cancel: options.cancel.clone(),
        };
        let mut archive = ZipArchive::open(bytes, &archive_options)?;
        let mut found = None;
        let mut skipped = 0u64;
        for (index, entry) in archive.entries().iter().enumerate() {
            if entry.is_directory() {
                continue;
            }
            if is_metadata(&entry.name) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            if entry.name.to_ascii_lowercase().ends_with(".ifc") {
                if found.is_some() {
                    return Err(Error::new(codes::IFC_ARCHIVE_ENTRY_AMBIGUOUS));
                }
                found = Some(index);
            }
        }
        let index = found.ok_or_else(|| Error::new(codes::IFC_ARCHIVE_ENTRY_MISSING))?;
        let data = archive.read_entry(index)?;
        let notes = Notes {
            skipped_entries: skipped,
            ..Notes::default()
        };
        read_physical(&data, options, notes)
    } else {
        read_physical(bytes, options, Notes::default())
    }
}

/// Whether an archive entry is a resource fork or other metadata rather than model data.
pub(crate) fn is_metadata(name: &str) -> bool {
    name.split('/').any(|part| part == "__MACOSX")
        || name
            .rsplit('/')
            .next()
            .is_some_and(|base| base.starts_with("._"))
}

fn read_physical(data: &[u8], options: &mut ReadOptions<'_>, skipped: Notes) -> Result<ReadOutput> {
    let limits: Limits = options.limits;
    let total = u64::try_from(data.len()).unwrap_or(u64::MAX);
    let parsed = {
        let mut control = options.control();
        let parsed = step::parse(data, &limits, &mut control)?;
        control.finish(total);
        parsed
    };
    options.cancel.check()?;
    let schema = schema_of(&parsed.header)?;
    let mut notes = parsed.notes;
    notes.skipped_entries = skipped.skipped_entries;
    let layout = Layout {
        source: SourceBytes(Arc::from(data)),
        header: parsed.header.into_iter().zip(parsed.header_spans).collect(),
        spans: parsed.spans.into_iter().map(Some).collect(),
        data_end: parsed.data_end,
        ..Layout::default()
    };
    let file = IfcFile::new(schema, parsed.instances, parsed.estimated_bytes, layout);
    let report = report_of(&file, limits, &notes, &options.cancel)?;
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

/// The loss report of a read: what the reader read leniently or skipped, entity types the typed
/// views do not read, references to instances that are not in the file, and a schema this reader
/// does not know.
fn report_of(file: &IfcFile, limits: Limits, notes: &Notes, cancel: &Cancel) -> Result<LossReport> {
    let mut log = LossLog::default();
    log.add(
        reasons::ARCHIVE_ENTRY_SKIPPED,
        "ZIP_ENTRY",
        None,
        notes.skipped_entries,
    );
    log.add(reasons::TEXT_LATIN1, "STRING", None, notes.text_latin1);
    log.add(
        reasons::TEXT_CONTROL_KEPT,
        "STRING",
        None,
        notes.text_controls,
    );
    log.add(reasons::BYTES_IGNORED, "FILE", None, notes.ignored_bytes);
    log.add(reasons::TRAILING_BYTES, "FILE", None, notes.trailing_bytes);
    for (section, count) in &notes.sections {
        log.add(reasons::SECTION_SKIPPED, section, None, *count);
    }
    if matches!(file.schema, Schema::Other(_)) {
        log.add(reasons::SCHEMA_UNRECOGNIZED, "FILE_SCHEMA", None, 1);
    }
    for instance in file.instances() {
        cancel.check()?;
        for record in &instance.records {
            if !UNDERSTOOD.contains(&record.type_name.as_str())
                && !UNDERSTOOD_IMPORT.contains(&record.type_name.as_str())
            {
                log.add(
                    reasons::ENTITY_PRESERVED,
                    &record.type_name,
                    Some(Source::Decimal(instance.name)),
                    1,
                );
            }
            let mut unresolved = 0u64;
            let mut stack: Vec<&Parameter> = record.parameters.iter().collect();
            while let Some(parameter) = stack.pop() {
                match parameter {
                    Parameter::Reference(target) if !file.contains(*target) => {
                        unresolved = unresolved.saturating_add(1);
                    }
                    Parameter::Typed { value, .. } => stack.push(value),
                    Parameter::List(items) => stack.extend(items.iter()),
                    _ => {}
                }
            }
            log.add(
                reasons::REFERENCE_UNRESOLVED,
                &record.type_name,
                Some(Source::Decimal(instance.name)),
                unresolved,
            );
        }
    }
    Ok(log.finish(LossHeader::new(
        "ifc",
        file.schema.identifier(),
        Direction::Import,
        Some(limits),
    )))
}
