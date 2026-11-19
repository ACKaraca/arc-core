//! The commands `core.draft.import.dxf` and `core.draft.export.dxf`, registered with
//! [`register_commands`].
//!
//! Files are reached only through [`Context::open_read`] and [`Context::create_write`], so the
//! session's roots and permission classes decide what a command may touch. The import runs in
//! the transaction of the command: one undo removes everything it created.

use std::io::Write;
use std::path::Path;

use arc_base::{Error, Result};
use arc_command::{CommandSpec, Context, IdParam, Permission, Registry, UndoBehavior};
use arc_io_dxf::{Progress, ReadOptions, Version, WriteOptions};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::apply::{Applied, apply, model_space_unit};
use crate::export::export_drawing;
use crate::plan::{ImportOptions, plan_with_report};

/// A message identifier of this crate that is not an error code or a loss reason.
pub struct Message {
    /// The identifier in `locales/en-US/arc-draft-dxf.ftl`.
    pub message_id: &'static str,
}

/// The progress message of an import.
pub const IMPORT_PROGRESS: Message = Message {
    message_id: "draft-dxf-progress-import",
};
/// The progress message of an export.
pub const EXPORT_PROGRESS: Message = Message {
    message_id: "draft-dxf-progress-export",
};

struct Names {
    id: &'static str,
    label: Message,
    description: Message,
}

const IMPORT: Names = Names {
    id: "core.draft.import.dxf",
    label: Message {
        message_id: "draft-dxf-command-import",
    },
    description: Message {
        message_id: "draft-dxf-command-import-description",
    },
};

const EXPORT: Names = Names {
    id: "core.draft.export.dxf",
    label: Message {
        message_id: "draft-dxf-command-export",
    },
    description: Message {
        message_id: "draft-dxf-command-export-description",
    },
};

/// Parameters of `core.draft.import.dxf`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportParams {
    /// The file to read, inside the session's read roots.
    #[schemars(length(min = 1, max = 4095))]
    pub path: String,
    /// The model space that receives the content.
    pub space: IdParam,
}

/// The DXF version an export writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub enum DxfVersion {
    /// Release 2000.
    #[serde(rename = "2000")]
    V2000,
    /// Release 2004.
    #[serde(rename = "2004")]
    V2004,
    /// Release 2007.
    #[serde(rename = "2007")]
    V2007,
    /// Release 2010.
    #[serde(rename = "2010")]
    V2010,
    /// Release 2013.
    #[serde(rename = "2013")]
    V2013,
    /// Release 2018, the default.
    #[default]
    #[serde(rename = "2018")]
    V2018,
}

impl DxfVersion {
    /// The format crate's version.
    #[must_use]
    pub fn version(self) -> Version {
        match self {
            Self::V2000 => Version::R2000,
            Self::V2004 => Version::R2004,
            Self::V2007 => Version::R2007,
            Self::V2010 => Version::R2010,
            Self::V2013 => Version::R2013,
            Self::V2018 => Version::R2018,
        }
    }
}

/// Parameters of `core.draft.export.dxf`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportParams {
    /// The file to write, inside the session's write roots.
    #[schemars(length(min = 1, max = 4095))]
    pub path: String,
    /// The model space to write.
    pub space: IdParam,
    /// The version to write; the default is the newest.
    #[serde(default)]
    pub version: DxfVersion,
    /// Whether to write the binary form instead of ASCII.
    #[serde(default)]
    pub binary: bool,
}

/// What `core.draft.import.dxf` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportResult {
    /// Linetypes created.
    pub linetypes: u64,
    /// Layers created.
    pub layers: u64,
    /// Text styles created.
    pub text_styles: u64,
    /// Dimension styles created.
    pub dim_styles: u64,
    /// Block definitions created.
    pub blocks: u64,
    /// Entities created.
    pub entities: u64,
    /// The loss report, in the form of `schemas/loss-report.schema.json`.
    pub report: serde_json::Value,
}

/// What `core.draft.export.dxf` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportResult {
    /// The bytes written.
    pub bytes: u64,
    /// The entities written, in the space and in blocks.
    pub entities: u64,
    /// The loss report, in the form of `schemas/loss-report.schema.json`.
    pub report: serde_json::Value,
}

fn report_json(report: &arc_io_dxf::LossReport) -> Result<serde_json::Value> {
    serde_json::to_value(report).map_err(|_| Error::new(arc_command::error::codes::RESULT_INVALID))
}

/// Reads the file, plans the import and creates the objects.
fn import(ctx: &mut Context<'_>, p: ImportParams) -> Result<ImportResult> {
    let space = p.space.0;
    let target_unit = model_space_unit(ctx.snapshot(), space)?;
    let file = ctx.open_read(Path::new(&p.path))?;
    let mut options = ReadOptions::with_limits(*ctx.limits());
    let cancel = options.cancel.clone();
    let output = {
        let probe: &Context<'_> = ctx;
        let mut tick = |_: Progress| {
            if probe.checkpoint().is_err() {
                cancel.cancel();
            }
        };
        options.progress = Some(&mut tick);
        arc_io_dxf::read_stream(file, &mut options)?
    };
    ctx.checkpoint()?;
    let plan = plan_with_report(
        &output.drawing,
        &ImportOptions {
            target_unit,
            limits: *ctx.limits(),
            ..ImportOptions::default()
        },
        &output.report,
    );
    let Applied {
        linetypes,
        layers,
        text_styles,
        dim_styles,
        blocks,
        entities,
    } = apply(ctx, space, &plan)?;
    Ok(ImportResult {
        linetypes,
        layers,
        text_styles,
        dim_styles,
        blocks,
        entities,
        report: report_json(&plan.report)?,
    })
}

/// Builds the drawing of the space, writes it and stores the bytes in the file.
fn export(ctx: &mut Context<'_>, p: ExportParams) -> Result<ExportResult> {
    let exported = export_drawing(ctx.snapshot(), p.space.0)?;
    ctx.progress(0.5, EXPORT_PROGRESS.message_id);
    let mut options = WriteOptions::for_version(p.version.version());
    options.binary = p.binary;
    let cancel = options.cancel.clone();
    let output = {
        let probe: &Context<'_> = ctx;
        let mut tick = |_: Progress| {
            if probe.checkpoint().is_err() {
                cancel.cancel();
            }
        };
        options.progress = Some(&mut tick);
        arc_io_dxf::write(&exported.drawing, &mut options)?
    };
    ctx.checkpoint()?;
    let mut file = ctx.create_write(Path::new(&p.path))?;
    file.write_all(&output.bytes)
        .and_then(|()| file.flush())
        .map_err(|e| Error::new(arc_command::error::codes::PATH_IO).with_source(e))?;
    let mut report = exported.report;
    report.items.extend(output.report.items.iter().cloned());
    report.truncated |= output.report.truncated;
    report.summary.mapped += output.report.summary.mapped;
    report.summary.approximated += output.report.summary.approximated;
    report.summary.preserved += output.report.summary.preserved;
    report.summary.dropped += output.report.summary.dropped;
    report.summary.failed += output.report.summary.failed;
    Ok(ExportResult {
        bytes: u64::try_from(output.bytes.len()).unwrap_or(u64::MAX),
        entities: exported.entities,
        report: report_json(&report)?,
    })
}

/// Registers `core.draft.import.dxf` (permission classes `edit` and `file_read`, undoable) and
/// `core.draft.export.dxf` (`read` and `file_write`, not undoable) into `registry`.
///
/// # Errors
///
/// `cmd.duplicate` when one of the identifiers is already registered.
pub fn register_commands(registry: &mut Registry) -> Result<()> {
    registry.register(
        CommandSpec::new(
            IMPORT.id,
            IMPORT.label.message_id,
            IMPORT.description.message_id,
            import,
        )
        .with_undo(UndoBehavior::Undoable)
        .with_permissions([Permission::Edit, Permission::FileRead]),
    )?;
    registry.register(
        CommandSpec::new(
            EXPORT.id,
            EXPORT.label.message_id,
            EXPORT.description.message_id,
            export,
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read, Permission::FileWrite]),
    )
}
