//! The platform side of menus and dialogs (CORE-APP-009) behind small traits, so tests use
//! fakes: [`FileDialogs`] (native dialogs through `rfd` in [`NativeDialogs`]), [`NativeMenu`]
//! (a `muda` menu bar on macOS in [`native_menu`], none elsewhere, where the shell draws the
//! egui menu bar), [`carry_out`] for the shell's [`Request`]s, and [`system_font`] for the text
//! font the shell loads from the operating system.

use std::path::{Path, PathBuf};

use arc_base::{Error, Result};
use arc_ui::{Entry, Intent, Localize};

use super::{Request, Shell};
use crate::application::{Application, CloseAnswer, Closed};
use crate::error::codes;
use crate::product::{FileType, PathDialog, PathMode};

/// File and question dialogs.
pub trait FileDialogs {
    /// A file to open among `types`, or `None` when the user cancelled.
    fn pick_open(&mut self, types: &[FileType]) -> Option<PathBuf>;
    /// A file to save to among `types`, or `None` when the user cancelled. Confirming an
    /// overwrite is the dialog's task.
    fn pick_save(&mut self, types: &[FileType], name: Option<&str>) -> Option<PathBuf>;
    /// A file the product's `dialog` asks for, or `None` when the user cancelled. `name` is the
    /// default file name of a save dialog.
    fn pick_path(&mut self, dialog: &PathDialog, name: Option<&str>) -> Option<PathBuf>;
    /// Whether to save, discard or keep the document `title` with unsaved changes.
    fn ask_close(&mut self, title: &str) -> CloseAnswer;
    /// Whether [`Self::ask_close`] can show its question. When it cannot, the shell asks in its
    /// own window instead (see [`Request::AskClose`]). The default is true.
    fn can_ask(&self) -> bool {
        true
    }
}

/// A menu bar outside the window.
pub trait NativeMenu {
    /// Shows `entries`; false when there is no native menu bar, so the shell draws its own.
    fn install(&mut self, entries: &[Entry]) -> bool;
    /// The intents of items activated since the last call.
    fn poll(&mut self) -> Vec<Intent>;
}

/// No native menu bar: the shell draws the egui menu bar.
#[derive(Debug, Default)]
pub struct NoNativeMenu;

impl NativeMenu for NoNativeMenu {
    fn install(&mut self, _: &[Entry]) -> bool {
        false
    }

    fn poll(&mut self) -> Vec<Intent> {
        Vec::new()
    }
}

/// The native menu of the platform: a `muda` menu bar on macOS, none elsewhere (Windows
/// would need an `unsafe` call to attach one, Linux has no common menu bar).
#[must_use]
pub fn native_menu() -> Box<dyn NativeMenu> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MudaMenu::default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoNativeMenu)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{Entry, Intent, NativeMenu};

    /// A `muda` menu bar; item identifiers are command identifiers.
    #[derive(Default)]
    pub(super) struct MudaMenu {
        menu: Option<muda::Menu>,
    }

    fn append(parent: &dyn Fn(&dyn muda::IsMenuItem) -> bool, entries: &[Entry]) -> bool {
        entries.iter().all(|entry| match entry {
            Entry::Command(view) => {
                let item =
                    muda::MenuItem::with_id(view.id.as_str(), &view.label, view.is_enabled(), None);
                parent(&item)
            }
            Entry::Submenu { label, entries } => {
                let sub = muda::Submenu::new(label, true);
                append(&|i| sub.append(i).is_ok(), entries) && parent(&sub)
            }
            Entry::Separator => parent(&muda::PredefinedMenuItem::separator()),
        })
    }

    impl NativeMenu for MudaMenu {
        fn install(&mut self, entries: &[Entry]) -> bool {
            let menu = muda::Menu::new();
            if !append(&|i| menu.append(i).is_ok(), entries) {
                return false;
            }
            menu.init_for_nsapp();
            self.menu = Some(menu);
            true
        }

        fn poll(&mut self) -> Vec<Intent> {
            let mut out = Vec::new();
            while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
                out.push(Intent::new(event.id.0));
            }
            out
        }
    }
}

/// Native file and message dialogs through `rfd` (the desktop portal on Linux).
#[derive(Default)]
pub struct NativeDialogs<'a> {
    /// Texts of the close question: title and description message identifiers are resolved
    /// through this source.
    pub localize: Option<&'a dyn Localize>,
}

impl std::fmt::Debug for NativeDialogs<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeDialogs").finish_non_exhaustive()
    }
}

/// A message identifier this module resolves.
struct Message {
    message_id: &'static str,
}

/// The question asked before closing a document with unsaved changes.
const UNSAVED: Message = Message {
    message_id: "app-dialog-unsaved",
};

fn filtered(
    mut dialog: rfd::FileDialog,
    types: &[FileType],
    localize: Option<&dyn Localize>,
) -> rfd::FileDialog {
    for t in types {
        let name = localize.map_or_else(
            || t.extension.clone(),
            |l| arc_ui::resolve(l, &t.message_id, &[]),
        );
        dialog = dialog.add_filter(name, &[t.extension.as_str()]);
    }
    dialog
}

impl FileDialogs for NativeDialogs<'_> {
    fn pick_open(&mut self, types: &[FileType]) -> Option<PathBuf> {
        let open: Vec<FileType> = types.iter().filter(|t| t.open).cloned().collect();
        filtered(rfd::FileDialog::new(), &open, self.localize).pick_file()
    }

    fn pick_save(&mut self, types: &[FileType], name: Option<&str>) -> Option<PathBuf> {
        let save: Vec<FileType> = types.iter().filter(|t| t.save).cloned().collect();
        let mut dialog = filtered(rfd::FileDialog::new(), &save, self.localize);
        if let Some(name) = name {
            dialog = dialog.set_file_name(name);
        }
        dialog.save_file()
    }

    fn pick_path(&mut self, dialog: &PathDialog, name: Option<&str>) -> Option<PathBuf> {
        let mut native = rfd::FileDialog::new();
        if let Some(l) = self.localize {
            native = native.set_title(arc_ui::resolve(l, &dialog.title, &[]));
        }
        for filter in &dialog.filters {
            let label = self.localize.map_or_else(
                || filter.message_id.clone(),
                |l| arc_ui::resolve(l, &filter.message_id, &[]),
            );
            native = native.add_filter(label, &filter.extensions);
        }
        match dialog.mode {
            PathMode::Open => native.pick_file(),
            PathMode::Save => match name {
                Some(name) => native.set_file_name(name).save_file(),
                None => native.save_file(),
            },
        }
    }

    /// The question is a `zenity` dialog on Linux: without that program on the search path the
    /// library answers Cancel without showing anything. Windows and macOS have the question
    /// in the system library.
    fn can_ask(&self) -> bool {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::env::var_os("PATH").is_some_and(|path| {
                std::env::split_paths(&path).any(|dir| dir.join("zenity").is_file())
            })
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        {
            true
        }
    }

    fn ask_close(&mut self, title: &str) -> CloseAnswer {
        let id = UNSAVED.message_id;
        let text = self
            .localize
            .map_or_else(|| id.to_owned(), |l| arc_ui::resolve(l, id, &[]));
        let result = rfd::MessageDialog::new()
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::YesNoCancel)
            .show();
        match result {
            rfd::MessageDialogResult::Yes => CloseAnswer::Save,
            rfd::MessageDialogResult::No => CloseAnswer::Discard,
            _ => CloseAnswer::Cancel,
        }
    }
}

/// What carrying out a [`Request`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    /// The user cancelled a dialog; nothing changed.
    Cancelled,
    /// A file was opened in this tab.
    Opened(crate::application::TabId),
    /// The tab was saved.
    Saved,
    /// The tab was closed, or its close was cancelled.
    Closed(Closed),
    /// Every tab closed: the application may end.
    Quit,
    /// A product command ran with the file a dialog returned.
    Ran,
    /// The close question is shown in the window; its answer arrives with a later frame.
    Asking,
}

fn untitled(app: &Application, tab: crate::application::TabId) -> String {
    app.tab(tab)
        .ok()
        .and_then(crate::application::Tab::title)
        .unwrap_or_default()
}

/// Carries out `request` with `dialogs`: open and save dialogs drive [`Application::open`] and
/// [`Application::save_as`], closing asks for dirty tabs, and quitting closes every tab in
/// order and stops at the first one the user keeps. Closes go through `shell`, so each closing
/// tab's controller is told first (see [`Shell::close_tab`]).
///
/// # Errors
/// The errors of the application's open, save and close flows.
pub fn carry_out(
    shell: &mut Shell,
    app: &mut Application,
    dialogs: &mut dyn FileDialogs,
    request: &Request,
) -> Result<Done> {
    let types = app.product().file_types().to_vec();
    match request {
        Request::OpenDialog => match dialogs.pick_open(&types) {
            Some(path) => app.open(&path).map(Done::Opened),
            None => Ok(Done::Cancelled),
        },
        Request::SaveDialog(tab) => {
            let name = untitled(app, *tab);
            let name = (!name.is_empty()).then_some(name.as_str());
            match dialogs.pick_save(&types, name) {
                Some(path) => app.save_as(*tab, &path).map(|()| Done::Saved),
                None => Ok(Done::Cancelled),
            }
        }
        Request::SaveMacro(text) => match dialogs.pick_save(&[], Some("macro.jsonl")) {
            Some(path) => std::fs::write(path, text)
                .map(|()| Done::Saved)
                .map_err(|e| Error::new(codes::MACRO_WRITE_FAILED).with_source(e)),
            None => Ok(Done::Cancelled),
        },
        Request::PathDialog { tab, dialog, then } => {
            let name = dialog.default_name.as_deref().map(|n| {
                let title = untitled(app, *tab);
                let stem = Path::new(&title).file_stem().and_then(|s| s.to_str());
                n.replace(
                    "{document}",
                    stem.filter(|s| !s.is_empty()).unwrap_or("untitled"),
                )
            });
            match dialogs.pick_path(dialog, name.as_deref()) {
                Some(path) => shell
                    .run_with_path(app, *tab, then.clone(), &path)
                    .map(|()| Done::Ran),
                None => Ok(Done::Cancelled),
            }
        }
        Request::AskClose(tab) if !dialogs.can_ask() && app.tab(*tab)?.is_dirty() => {
            shell.ask_in_window(app, *tab, false);
            Ok(Done::Asking)
        }
        Request::AskClose(tab) => close(shell, app, dialogs, *tab).map(Done::Closed),
        Request::Quit => {
            while let Some(tab) = app.tabs().first().map(crate::application::Tab::id) {
                if !dialogs.can_ask() && app.tab(tab)?.is_dirty() {
                    shell.ask_in_window(app, tab, true);
                    return Ok(Done::Asking);
                }
                if close(shell, app, dialogs, tab)? == Closed::Cancelled {
                    return Ok(Done::Closed(Closed::Cancelled));
                }
            }
            Ok(Done::Quit)
        }
    }
}

fn close(
    shell: &mut Shell,
    app: &mut Application,
    dialogs: &mut dyn FileDialogs,
    tab: crate::application::TabId,
) -> Result<Closed> {
    let title = untitled(app, tab);
    let needs_path = app.tab(tab)?.path().is_none();
    let mut answer = None;
    let closed = shell.close_tab(app, tab, |_| {
        let a = dialogs.ask_close(&title);
        answer = Some(a);
        // A document never saved cannot be saved in place: ask for a file below instead.
        if a == CloseAnswer::Save && needs_path {
            CloseAnswer::Cancel
        } else {
            a
        }
    })?;
    if answer == Some(CloseAnswer::Save) && needs_path {
        let types = app.product().file_types().to_vec();
        return match dialogs.pick_save(&types, None) {
            Some(path) => {
                app.save_as(tab, &path)?;
                shell.close_tab(app, tab, |_| CloseAnswer::Cancel)
            }
            None => Ok(Closed::Cancelled),
        };
    }
    Ok(closed)
}

/// The largest font file read.
pub const MAX_FONT_BYTES: u64 = 32 << 20;

/// Font files the shell tries, in order, per platform. None of them ships with this
/// repository; a system without any of them shows text as boxes.
const FONTS: &[&str] = &[
    #[cfg(target_os = "windows")]
    "C:\\Windows\\Fonts\\segoeui.ttf",
    #[cfg(target_os = "windows")]
    "C:\\Windows\\Fonts\\arial.ttf",
    #[cfg(target_os = "macos")]
    "/System/Library/Fonts/Helvetica.ttc",
    #[cfg(target_os = "macos")]
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// The first readable font file of the platform list, at most [`MAX_FONT_BYTES`].
#[must_use]
pub fn system_font() -> Option<(PathBuf, Vec<u8>)> {
    FONTS.iter().map(Path::new).find_map(read_font)
}

fn read_font(path: &Path) -> Option<(PathBuf, Vec<u8>)> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_FONT_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (u64::try_from(bytes.len()).ok()? <= MAX_FONT_BYTES).then(|| (path.to_path_buf(), bytes))
}

/// Installs `font` as the first font of every family of `ctx`.
pub fn install_font(ctx: &arc_ui::egui::Context, font: Vec<u8>) {
    use arc_ui::egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        "system".into(),
        std::sync::Arc::new(FontData::from_owned(font)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.insert(family, vec!["system".into()]);
    }
    ctx.set_fonts(fonts);
}
