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
//! - notifications and progress with cancel (CORE-UI-018, CORE-UI-019, [`notify`]).
//!
//! Widgets never run commands; they return an [`Intent`]. No function in this crate panics.

pub mod command;
pub mod error;
pub mod icons;
pub mod keys;
pub mod menu;
pub mod notify;
pub mod palette;
pub mod text;
pub mod theme;

/// The pinned egui, re-exported so products use the same version.
pub use egui;

pub use command::{AllEnabled, CommandState, CommandView, Intent, Lookup};
pub use error::codes;
pub use icons::{IconAtlas, Icons};
pub use keys::{Binding, ChordResolver, KeyCombo, KeyOutcome, KeySequence, Keymap};
pub use menu::{Entry, Menu, MenuItem, ToolEntry, Toolbar, ToolbarItem};
pub use notify::{Notification, Notifications, Progress, ProgressView, Severity};
pub use palette::{MatchKind, Palette, Ranked, Recent, rank};
pub use text::{Fallback, Localize, Message, resolve};
pub use theme::{Appearance, Colors, Density, Metrics, Theme, ThemeId, UiScale};
