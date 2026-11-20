//! The shell commands (CORE-APP-009): file, undo, redo and selection commands that menus, key
//! bindings, the palette and the command line name, registered beside the product's commands.
//!
//! They act on the application, not inside a document transaction, so the windowed shell
//! carries them out itself when one of them reaches it (an intent or a command-line run);
//! invoked through [`arc_command::Invoker`] they change nothing. They are exposed on the UI and
//! command-line surfaces only, so automation never reaches a dialog. `app.file.open` and
//! `app.file.save_as` without a `path` ask the platform for a file dialog.

use arc_base::Result;
use arc_command::{CommandSpec, IdParam, Permission, Registry, Surface, UndoBehavior};
use schemars::JsonSchema;
use serde::Deserialize;

/// The session setting that holds the language tag of the interface (`en`, `tr`): the shell keeps
/// it current in the sessions it runs commands in, and in the session of a new document's starting
/// command, so a command can name what it creates in the language the user reads. Absent in
/// headless runs.
pub const LANGUAGE_SETTING: &str = "app.language";

/// Opens a new, empty document.
pub const FILE_NEW: &str = "app.file.new";
/// Opens a file: `path`, or a file dialog without one.
pub const FILE_OPEN: &str = "app.file.open";
/// Saves the active document in place; a document never saved asks for a file.
pub const FILE_SAVE: &str = "app.file.save";
/// Saves the active document to `path`, or to a file chosen in a dialog.
pub const FILE_SAVE_AS: &str = "app.file.save_as";
/// Closes the active document, asking about unsaved changes.
pub const FILE_CLOSE: &str = "app.file.close";
/// Ends the application, asking about unsaved changes.
pub const APP_QUIT: &str = "app.shell.quit";
/// Undoes the last step of the active document.
pub const EDIT_UNDO: &str = "app.edit.undo";
/// Redoes the last undone step of the active document.
pub const EDIT_REDO: &str = "app.edit.redo";
/// Changes the selection: `objects`, `mode` (`replace`, `add`, `toggle`) and the optional
/// `gesture` of the viewport.
pub const SELECT_SET: &str = "app.select.set";
/// Clears the selection.
pub const SELECT_CLEAR: &str = "app.select.clear";

/// Starts recording the product commands the shell runs as a macro.
pub const MACRO_RECORD_START: &str = "app.macro.record_start";
/// Stops recording and asks for a file to save the macro to.
pub const MACRO_RECORD_STOP: &str = "app.macro.record_stop";

/// No parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

/// Parameters of [`FILE_OPEN`] and [`FILE_SAVE_AS`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathParams {
    /// The file; a dialog asks for it when it is missing.
    #[serde(default)]
    #[schemars(length(max = 4096))]
    pub path: Option<String>,
}

/// How [`SELECT_SET`] combines its objects with the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectMode {
    /// The objects become the selection.
    #[default]
    Replace,
    /// The objects are added.
    Add,
    /// Each object is added when absent and removed when present.
    Toggle,
}

/// Parameters of [`SELECT_SET`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectParams {
    /// The objects.
    #[schemars(length(max = 100000))]
    pub objects: Vec<IdParam>,
    /// The combination.
    #[serde(default)]
    pub mode: SelectMode,
    /// The viewport gesture that produced the selection (`click`, `window`, `crossing`).
    #[serde(default)]
    #[schemars(length(max = 16))]
    pub gesture: Option<String>,
}

/// The label message identifier of a command, with its description's.
struct Names {
    id: &'static str,
    message_id: &'static str,
    description: Description,
}

struct Description {
    message_id: &'static str,
}

const NAMES: [Names; 12] = [
    Names {
        id: FILE_NEW,
        message_id: "app-command-file-new",
        description: Description {
            message_id: "app-command-file-new-description",
        },
    },
    Names {
        id: FILE_OPEN,
        message_id: "app-command-file-open",
        description: Description {
            message_id: "app-command-file-open-description",
        },
    },
    Names {
        id: FILE_SAVE,
        message_id: "app-command-file-save",
        description: Description {
            message_id: "app-command-file-save-description",
        },
    },
    Names {
        id: FILE_SAVE_AS,
        message_id: "app-command-file-save-as",
        description: Description {
            message_id: "app-command-file-save-as-description",
        },
    },
    Names {
        id: FILE_CLOSE,
        message_id: "app-command-file-close",
        description: Description {
            message_id: "app-command-file-close-description",
        },
    },
    Names {
        id: APP_QUIT,
        message_id: "app-command-quit",
        description: Description {
            message_id: "app-command-quit-description",
        },
    },
    Names {
        id: EDIT_UNDO,
        message_id: "app-command-undo",
        description: Description {
            message_id: "app-command-undo-description",
        },
    },
    Names {
        id: EDIT_REDO,
        message_id: "app-command-redo",
        description: Description {
            message_id: "app-command-redo-description",
        },
    },
    Names {
        id: SELECT_SET,
        message_id: "app-command-select",
        description: Description {
            message_id: "app-command-select-description",
        },
    },
    Names {
        id: SELECT_CLEAR,
        message_id: "app-command-select-clear",
        description: Description {
            message_id: "app-command-select-clear-description",
        },
    },
    Names {
        id: MACRO_RECORD_START,
        message_id: "app-command-macro-record-start",
        description: Description {
            message_id: "app-command-macro-record-start-description",
        },
    },
    Names {
        id: MACRO_RECORD_STOP,
        message_id: "app-command-macro-record-stop",
        description: Description {
            message_id: "app-command-macro-record-stop-description",
        },
    },
];

fn spec<P: serde::de::DeserializeOwned + JsonSchema>(id: &'static str) -> CommandSpec {
    let names = NAMES.iter().find(|n| n.id == id);
    let label = names.map_or(id, |n| n.message_id);
    let description = names.map_or(id, |n| n.description.message_id);
    CommandSpec::new(id, label, description, |_, _: P| Ok(()))
        .with_exposure([Surface::Ui, Surface::CommandLine])
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read])
}

/// Registers the shell commands into `registry`.
///
/// # Errors
/// `cmd.duplicate` when one of the identifiers is already registered.
pub fn register(registry: &mut Registry) -> Result<()> {
    let specs = [
        spec::<NoParams>(FILE_NEW),
        spec::<PathParams>(FILE_OPEN).with_permissions([Permission::FileRead]),
        spec::<NoParams>(FILE_SAVE).with_permissions([Permission::FileWrite]),
        spec::<PathParams>(FILE_SAVE_AS).with_permissions([Permission::FileWrite]),
        spec::<NoParams>(FILE_CLOSE),
        spec::<NoParams>(APP_QUIT),
        spec::<NoParams>(EDIT_UNDO)
            .with_undo(UndoBehavior::UndoControl)
            .with_permissions([Permission::Edit]),
        spec::<NoParams>(EDIT_REDO)
            .with_undo(UndoBehavior::UndoControl)
            .with_permissions([Permission::Edit]),
        spec::<SelectParams>(SELECT_SET).with_positional(["objects"]),
        spec::<NoParams>(SELECT_CLEAR),
        spec::<NoParams>(MACRO_RECORD_START),
        spec::<NoParams>(MACRO_RECORD_STOP).with_permissions([Permission::FileWrite]),
    ];
    for spec in specs {
        registry.register(spec)?;
    }
    Ok(())
}

/// True when `command` is one of the shell commands.
#[must_use]
pub fn is_shell_command(command: &str) -> bool {
    NAMES.iter().any(|n| n.id == command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_once_and_only_for_ui_surfaces() {
        let mut registry = Registry::new();
        register(&mut registry).unwrap();
        assert_eq!(register(&mut registry).unwrap_err().code(), "cmd.duplicate");
        for spec in registry.iter() {
            assert!(is_shell_command(spec.id()), "{}", spec.id());
            assert!(!spec.exposure().contains(&Surface::Automation));
        }
        assert_eq!(registry.iter().count(), 12);
        assert!(!is_shell_command("shell.point.create"));
    }
}
