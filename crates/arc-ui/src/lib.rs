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
//! - the virtualized table and its clipboard (CORE-UI-013, CORE-UI-014, [`table`]);
//! - the tool option bar of the active prompt (CORE-UI-008, [`option_bar`]);
//! - the dynamic input panel for point input near the pointer (CORE-UI-009, [`dynamic_input`]);
//! - the snapping feedback: marker, name and tracking lines (CORE-UI-030, [`snap`]);
//! - the layout direction of right-to-left languages (CORE-UI-025, [`text::Direction`]);
//! - the style pickers for color, layer, linetype, lineweight and material (CORE-UI-015,
//!   [`style`]);
//! - with the feature `viewport`, the viewport widget, its interaction state machine and the
//!   selection gestures (CORE-UI-005, CORE-UI-029, `gesture` and `viewport`), the product hooks
//!   of the viewport (pointer handler, projection and overlays, [`hooks`]) and the view cube
//!   (`view_cube`).
//!
//! Widgets never run commands; they return an [`Intent`]. No function in this crate panics.

pub mod command;
pub mod command_line;
pub mod diagnostics;
pub mod dock;
pub mod dynamic_input;
pub mod error;
#[cfg(feature = "viewport")]
mod fill;
#[cfg(feature = "viewport")]
pub mod gesture;
#[cfg(feature = "viewport")]
pub mod hooks;
pub mod icons;
pub mod keys;
pub mod menu;
pub mod notify;
pub mod option_bar;
pub mod palette;
pub mod property;
pub mod quantity;
pub mod snap;
pub mod style;
pub mod table;
pub mod table_model;
pub mod text;
pub mod theme;
pub mod tree;
#[cfg(feature = "viewport")]
pub mod view_cube;
#[cfg(feature = "viewport")]
pub mod viewport;
pub mod workspace;

/// The pinned egui, re-exported so products use the same version.
pub use egui;

pub use command::{AllEnabled, CommandState, CommandView, Intent, Lookup, object_list};
pub use command_line::{
    CommandLineView, History, LineAction, LineCommands, LineContext, LineStatus, prompt_text,
};
pub use diagnostics::{DiagnosticsFilter, DiagnosticsPanel, filter_in};
pub use dock::{Axis, Dock, DockArea, DockLayout, Floating, Side, Target, flat_tab};
pub use dynamic_input::{DynamicEvent, DynamicField, DynamicInput, Relative, show_fields};
pub use error::codes;
#[cfg(feature = "viewport")]
pub use gesture::{
    Band, BandKind, Gestures, Reaction, View, ViewEvent, ViewPicker, ViewportCommands,
};
#[cfg(feature = "viewport")]
pub use hooks::{
    OverlayColor, PointerEvent, PointerHandler, PointerKind, ProductOverlay, ViewProjection,
    WorkPlane,
};
pub use icons::{IconAtlas, Icons};
pub use keys::{Binding, ChordResolver, KeyCombo, KeyOutcome, KeySequence, Keymap};
pub use menu::{Entry, Menu, MenuItem, ToolEntry, Toolbar, ToolbarItem};
pub use notify::{Notification, Notifications, Progress, ProgressView, Severity};
pub use option_bar::{OptionBar, OptionControl, ParameterControl, PromptControls};
pub use palette::{MatchKind, Palette, Ranked, Recent, rank, rank_in};
pub use property::{GridContext, PropertyGrid, PropertyModel, PropertyObject, PropertyRow, Shown};
pub use quantity::{QuantityField, QuantityInput};
pub use snap::{MarkerShape, Snap, SnapKind};
pub use style::{Preview, StyleEntry, StyleKind, StylePicker};
pub use table::{
    CellRef, ColumnInfo, Selection, Sort, SortDirection, TableCommands, TableSource, TableView,
};
pub use text::{
    Direction, Fallback, Localize, Message, decimal_separator, fold_case, matches_filter, resolve,
};
pub use theme::{Appearance, Colors, Density, Metrics, Theme, ThemeId, UiScale};
pub use tree::{TreeCommands, TreeItem, TreeSource, TreeView};
#[cfg(feature = "viewport")]
pub use view_cube::{CubeOutput, CubeTarget, ViewCube, ViewCubeInput, ViewIntent, orbit_by_drag};
#[cfg(feature = "viewport")]
pub use viewport::{
    DynamicPanel, ProductInput, ScenePicker, ViewLink, ViewportDisplay, ViewportInput,
    ViewportOutput, ViewportWidget, paint_crosshair,
};
pub use workspace::{NamedToolbar, ToolSet, Workspace, Workspaces};
