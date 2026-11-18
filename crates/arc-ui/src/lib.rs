//! The widget kit of the Arc suite on egui.
//!
//! The parts that exist run without a window, so tests drive them through a headless
//! [`egui::Context`]:
//!
//! - theme tokens, the four shipped themes, contrast checks and reduced motion (CORE-UI-021,
//!   CORE-UI-028, [`theme`]);
//! - density and UI scale as token transforms (CORE-UI-022, [`theme`]);
//! - project-drawn vector icons rasterized into an atlas and recolored by tokens (CORE-UI-023,
//!   [`icons`]);
//! - localized strings through Fluent message identifiers with English fallback (CORE-UI-024,
//!   [`text`]);
//! - menus, toolbars and context menus from data resolved against the command registry
//!   (CORE-UI-003, [`command`], [`menu`]);
//! - key bindings, chords and conflict detection (CORE-UI-004, [`keys`]);
//! - the command palette and its ranking (CORE-UI-007, [`palette`]);
//! - notifications and progress with cancel (CORE-UI-018, CORE-UI-019, [`notify`]);
//! - docking layouts as validated data with a widget (CORE-UI-001, [`dock`]) and named
//!   workspaces (CORE-UI-002, [`workspace`]);
//! - the command line with the Escape, Enter and undo key standards (CORE-UI-006, CORE-UI-031,
//!   CORE-UI-032, [`command_line`]);
//! - the property grid and numeric and unit input (CORE-UI-010, CORE-UI-011, [`property`],
//!   [`quantity`]);
//! - the virtualized tree and outliner (CORE-UI-012, [`tree`]);
//! - the diagnostics panel (CORE-UI-020, [`diagnostics`]);
//! - with the feature `viewport`, the viewport widget, its interaction state machine and the
//!   selection gestures (CORE-UI-005, CORE-UI-029, `gesture` and `viewport`).
//!
//! Widgets never run commands; they return an [`Intent`]. No function in this crate panics.

pub mod command;
pub mod command_line;
pub mod diagnostics;
pub mod dock;
pub mod error;
#[cfg(feature = "viewport")]
pub mod gesture;
pub mod icons;
pub mod keys;
pub mod menu;
pub mod notify;
pub mod palette;
pub mod property;
pub mod quantity;
pub mod text;
pub mod theme;
pub mod tree;
#[cfg(feature = "viewport")]
pub mod viewport;
pub mod workspace;

/// The pinned egui, re-exported so products use the same version.
pub use egui;

pub use command::{AllEnabled, CommandState, CommandView, Intent, Lookup, object_list};
pub use command_line::{
    CommandLineView, History, LineAction, LineCommands, LineContext, LineStatus,
};
pub use diagnostics::{DiagnosticsFilter, DiagnosticsPanel};
pub use dock::{Axis, Dock, DockArea, DockLayout, Floating, Side, Target};
pub use error::codes;
#[cfg(feature = "viewport")]
pub use gesture::{
    Band, BandKind, Gestures, Reaction, View, ViewEvent, ViewPicker, ViewportCommands,
};
pub use icons::{IconAtlas, Icons};
pub use keys::{Binding, ChordResolver, KeyCombo, KeyOutcome, KeySequence, Keymap};
pub use menu::{Entry, Menu, MenuItem, ToolEntry, Toolbar, ToolbarItem};
pub use notify::{Notification, Notifications, Progress, ProgressView, Severity};
pub use palette::{MatchKind, Palette, Ranked, Recent, rank};
pub use property::{GridContext, PropertyGrid, PropertyModel, PropertyObject, PropertyRow, Shown};
pub use quantity::{QuantityField, QuantityInput};
pub use text::{Fallback, Localize, Message, resolve};
pub use theme::{Appearance, Colors, Density, Metrics, Theme, ThemeId, UiScale};
pub use tree::{TreeCommands, TreeItem, TreeSource, TreeView};
#[cfg(feature = "viewport")]
pub use viewport::{ScenePicker, ViewLink, ViewportInput, ViewportOutput, ViewportWidget};
pub use workspace::{NamedToolbar, ToolSet, Workspace, Workspaces};
