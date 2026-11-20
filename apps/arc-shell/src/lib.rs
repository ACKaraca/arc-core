//! The reference windowed shell (CORE-APP-025): the sample document types of
//! `arc-shell-sample` with a viewport, property grid, outliner, command line, diagnostics,
//! settings and log panels, wired into the windowed shell of `arc-app`. The binary only builds this
//! product and starts the event loop; tests drive the same product headless.

use std::sync::Arc;

use arc_app::product::{Entry, Extractor, FileType, Product, ProductBuilder};
use arc_app::render::SnapshotRenderer;
use arc_app::shell::{
    self, CommandLinePanel, DiagnosticsPanelView, LogPanel, OutlinerPanel, PropertiesPanel,
    SettingsPanel, ShellConfig, ViewportPanel,
};
use arc_base::math::Point3;
use arc_base::{Cancel, Color, Error, Result};
use arc_command::Clock;
use arc_doc::package::LockOwner;
use arc_doc::{Object, Snapshot, Value};
use arc_gpu::scene::{LineItem, LinePath, LineStyle, LineWidth, RenderItem};
use arc_gpu::{Extraction, extract};
use arc_ui::{Axis, Binding, Dock, DockLayout};

/// The panel identifiers, in dock order.
pub const VIEWPORT: &str = "shell.viewport";
/// The property grid.
pub const PROPERTIES: &str = "shell.properties";
/// The outliner.
pub const OUTLINER: &str = "shell.outliner";
/// The command line.
pub const COMMAND_LINE: &str = "shell.command_line";
/// The diagnostics list.
pub const DIAGNOSTICS: &str = "shell.diagnostics";
/// The settings editor.
pub const SETTINGS: &str = "shell.settings";
/// The log viewer.
pub const LOG: &str = "shell.log";

/// The command the property grid's edits name.
pub const EDIT_COMMAND: &str = "shell.object.set_field";

/// The width of drawn points and lines in pixels.
const WIDTH_PX: f32 = 3.0;
/// The color of drawn objects (sRGB).
const INK: [u8; 4] = [40, 40, 40, 255];

fn point_of(object: &Object, at: usize) -> Option<Point3> {
    match object.values.get(at)? {
        Some(Value::Point3(p)) => Some(*p),
        _ => None,
    }
}

/// The offset of `to` from `from` as GPU coordinates.
#[allow(
    clippy::cast_possible_truncation,
    reason = "render data is f32 relative to an origin"
)]
fn offset(from: Point3, to: Point3) -> [f32; 3] {
    let d = to - from;
    [d.x as f32, d.y as f32, d.z as f32]
}

/// The render items of a sample object: a dot for a point, a segment for a line; layers have
/// none. A non-finite offset is refused by the scene when the result is applied.
///
/// # Errors
/// None; objects without their point values have no items.
pub fn extract_sample(snapshot: &Snapshot, object: &Object, _: &Cancel) -> Result<Extraction> {
    let key = snapshot
        .resolve(object.id)
        .ok()
        .and_then(|h| snapshot.type_key(h).map(|k| k.as_str().to_owned()))
        .unwrap_or_default();
    let item = |origin: Point3, points: Vec<[f32; 3]>| {
        RenderItem::Lines(LineItem {
            origin,
            paths: vec![LinePath {
                points,
                closed: false,
            }],
            style: LineStyle::round(LineWidth::Screen(WIDTH_PX)),
            color: Color::from_srgb8(INK),
        })
    };
    let items = match key.as_str() {
        arc_shell_sample::POINT => point_of(object, 0)
            .map(|p| item(p, vec![[0.0, 0.0, 0.0]]))
            .into_iter()
            .collect(),
        arc_shell_sample::LINE => match (point_of(object, 0), point_of(object, 1)) {
            (Some(a), Some(b)) => vec![item(a, vec![[0.0, 0.0, 0.0], offset(a, b)])],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    Ok(Extraction {
        geometries: Vec::new(),
        lods: Vec::new(),
        items,
    })
}

/// The render items of a sample object for the view renderer: those of [`extract_sample`], which
/// nothing cancels here. An extraction error is returned to the client, not drawn as nothing.
fn draw_sample(snapshot: &Snapshot, object: &Object) -> Result<Vec<RenderItem>> {
    extract_sample(snapshot, object, &Cancel::new()).map(|extraction| extraction.items)
}

fn entry(id: &str, label: Message, make: fn() -> Box<dyn shell::Panel>) -> Entry {
    Entry {
        id: id.into(),
        message_id: label.message_id.into(),
        factory: Arc::new(make),
    }
}

/// The product of the reference shell: the sample types and commands, the shell commands, the
/// appearance settings, the `.arc` file type, the seven panels and the extractor of points and
/// lines.
///
/// # Errors
/// The registries' errors; none occur for the sample set.
pub fn product(clock: Arc<dyn Clock>, owner: LockOwner) -> Result<Product> {
    let mut builder = ProductBuilder::new("arc-shell", env!("CARGO_PKG_VERSION"))
        .with_types(arc_shell_sample::types()?)
        .with_commands(arc_shell_sample::registry()?);
    arc_app::commands::register(builder.commands_mut())?;
    shell::prefs::register_settings(&mut builder)?;
    arc_app::automation::register_settings(&mut builder)?;
    builder.file_type(FileType {
        extension: "arc".into(),
        message_id: "shell-file-arc".into(),
        open: true,
        save: true,
    })?;
    let panels = [
        entry(
            VIEWPORT,
            Message {
                message_id: "shell-panel-viewport",
            },
            || shell::panel(ViewportPanel::new()),
        ),
        entry(
            PROPERTIES,
            Message {
                message_id: "shell-panel-properties",
            },
            || shell::panel(PropertiesPanel::new(EDIT_COMMAND)),
        ),
        entry(
            OUTLINER,
            Message {
                message_id: "shell-panel-outliner",
            },
            || shell::panel(OutlinerPanel::default()),
        ),
        entry(
            COMMAND_LINE,
            Message {
                message_id: "shell-panel-command-line",
            },
            || shell::panel(CommandLinePanel),
        ),
        entry(
            DIAGNOSTICS,
            Message {
                message_id: "shell-panel-diagnostics",
            },
            || shell::panel(DiagnosticsPanelView),
        ),
        entry(
            SETTINGS,
            Message {
                message_id: "shell-panel-settings",
            },
            || shell::panel(SettingsPanel::default()),
        ),
        entry(
            LOG,
            Message {
                message_id: "shell-panel-log",
            },
            || shell::panel(LogPanel),
        ),
    ];
    for p in panels {
        builder.panel(p)?;
    }
    builder.extractor(Extractor {
        id: "shell.render".into(),
        types: vec![
            arc_shell_sample::POINT.into(),
            arc_shell_sample::LINE.into(),
        ],
        factory: Arc::new(|| -> extract::Extractor { shell::extractor(extract_sample) }),
    })?;
    // The automation socket serves `view.render` with the same drawing as the window.
    builder.view_renderer(Arc::new(SnapshotRenderer::new(draw_sample)));
    builder.build(clock, owner)
}

fn tabs(panels: &[&str]) -> Dock {
    Dock::Tabs {
        tabs: panels.iter().map(|p| (*p).to_owned()).collect(),
        active: 0,
    }
}

/// The shell configuration: the viewport in the middle, the outliner and the property grid on
/// the right, the command line with diagnostics, settings and the log below; the default menu bar; and
/// key bindings for the file commands. `seed` seeds the invocations (see
/// [`ShellConfig::seed`]).
///
/// # Errors
/// None for this layout and menu; their validation errors otherwise.
pub fn config(seed: u64) -> Result<ShellConfig> {
    let right = Dock::Split {
        axis: Axis::Vertical,
        fraction: 0.4,
        first: Box::new(tabs(&[OUTLINER])),
        second: Box::new(tabs(&[PROPERTIES])),
    };
    let top = Dock::Split {
        axis: Axis::Horizontal,
        fraction: 0.7,
        first: Box::new(tabs(&[VIEWPORT])),
        second: Box::new(right),
    };
    let root = Dock::Split {
        axis: Axis::Vertical,
        fraction: 0.75,
        first: Box::new(top),
        second: Box::new(tabs(&[COMMAND_LINE, DIAGNOSTICS, SETTINGS, LOG])),
    };
    let binding = |keys: &str, command: &str| Binding {
        keys: keys.into(),
        command: command.into(),
    };
    Ok(ShellConfig {
        layout: DockLayout::new(root, Vec::new())?,
        menu: shell::default_menu()?,
        bindings: vec![
            binding("Ctrl+N", arc_app::commands::FILE_NEW),
            binding("Ctrl+O", arc_app::commands::FILE_OPEN),
            binding("Ctrl+S", arc_app::commands::FILE_SAVE),
            binding("Ctrl+Shift+S", arc_app::commands::FILE_SAVE_AS),
            binding("Ctrl+W", arc_app::commands::FILE_CLOSE),
            binding("Ctrl+Q", arc_app::commands::APP_QUIT),
        ],
        appearance: appearance(),
        toolbar: Vec::new(),
        status: Vec::new(),
        display: None,
        overlays: None,
        initial_view: None,
        seed,
    })
}

/// The reference shell's appearance: dark, as drafting and modeling applications use.
fn appearance() -> arc_ui::Appearance {
    arc_ui::Appearance {
        theme: arc_ui::ThemeId::Dark,
        ..arc_ui::Appearance::default()
    }
}

/// The message catalogs the shell formats with, for the requested languages.
///
/// # Errors
/// `app.locale.catalog_invalid` when a catalog does not parse.
pub fn localizer(requested: &[String]) -> Result<arc_app::locale::Localizer> {
    arc_app::locale::Localizer::new(catalogs()?, requested)
}

/// The localizer for the requested languages, and one line per shipped language whose catalog did
/// not parse: that language is left out, so its messages fall back to English, and the caller
/// reports the lines.
///
/// # Errors
/// `app.locale.catalog_invalid` when the English catalog does not parse.
pub fn localizer_with_report(
    requested: &[String],
) -> Result<(arc_app::locale::Localizer, Vec<String>)> {
    let (catalogs, skipped) = arc_app::locale::shell_catalogs_with_fallback()?;
    let report = skipped
        .iter()
        .map(|(tag, error)| format!("{tag}: {}", error.code()))
        .collect();
    Ok((
        arc_app::locale::Localizer::new(catalogs, requested)?,
        report,
    ))
}

/// The shell's message catalogs for every shipped locale, from
/// [`arc_app::locale::shell_catalogs`]; callers may append their own catalogs.
///
/// # Errors
/// `app.locale.catalog_invalid` when a catalog does not parse.
pub fn catalogs() -> Result<arc_app::locale::Catalogs> {
    arc_app::locale::shell_catalogs()
}

/// A message of the shell.
struct Message {
    message_id: &'static str,
}

const TITLE: Message = Message {
    message_id: "shell-title",
};

/// The window title.
#[must_use]
pub fn title(localizer: &arc_app::locale::Localizer) -> String {
    localizer.format(TITLE.message_id, &[]).text
}

/// The error of a start-up step, for the report on standard error.
#[must_use]
pub fn describe(error: &Error, localizer: Option<&arc_app::locale::Localizer>) -> String {
    match localizer {
        Some(l) => format!("{}: {}", error.code(), l.format_error(error).text),
        None => error.code().to_owned(),
    }
}
