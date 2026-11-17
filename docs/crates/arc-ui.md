# arc-ui

The widget kit on egui shared by every Arc product on the desktop, in the browser and on tablets and phones: docking workspace, viewport widget,
command line and palette, tool option bar, property grid, tree and outliner, virtualized tables,
numeric and unit input, node editor, notifications, theming and the shared interaction standards.
The architecture is specified in [ui-kit.md](../architecture/ui-kit.md).

## Owns

- Widgets and their data-source traits; widgets emit command invocations only.
- Interaction standards: selection gestures, snapping feedback, Escape and Enter behavior, undo key
  handling.
- Theme tokens, shipped themes, density and UI scale, icon atlas.
- Accessibility integration (names, roles, focus order) and its test helpers.
- Pointer sources (mouse, touch, pen), gesture recognizers, size-class layouts and soft-keyboard
  integration for the browser and tablet and phone targets.
- Re-export of the pinned egui family crates for products.
- UI test helpers on `egui_kittest`.

## Stays in products

Panels with domain content (family editor, alignment editor, timeline), tools and their viewport
interaction, product icons and default workspaces, and product-specific widgets until a second
product needs them.

## Key concepts and types

| Concept | Description |
|---|---|
| `Workspace` | Dock tree, toolbars, menus, tool sets |
| `ViewportWidget` | Hosts a viewport texture and routes input |
| `PropertySource` | Trait providing categorized fields, values (possibly mixed), editors and override markers |
| `TreeSource` | Trait providing lazily loaded children, labels, icons and toggles |
| `TableSource` | Trait providing row counts, row windows, columns, sorting and filtering |
| `QuantityField` | Expression-capable numeric input bound to a dimension |
| `NodeEditor` | Canvas for `arc-nodes` graphs |
| `Theme` | Token set for colors, spacing, radii, fonts, icon sizes |
| `Notification` | Severity, message, optional command, progress |
| `PointerEvent` | Pointer kind (mouse, touch, pen), position, pressure, tilt, contact size and hover state |
| `SizeClass` | Compact, medium or expanded, derived from the window width, selecting a layout |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-solve`, `arc-doc`, `arc-command`, `arc-gpu`; `arc-nodes` (feature `nodes`, node editor, from phase 3) |
| Third-party | `egui`, `egui_dock`, `egui_table`, `egui_ltreeview`, `egui-snarl`, AccessKit (through egui), `resvg`, `parley` |

## Test strategy

`egui_kittest` tests for every widget, driven by simulated mouse, touch and pen streams: AccessKit name and role checks, keyboard traversal, snapshot
images at 100 % and 200 % scale, pseudo-locale and pseudo right-to-left locale; contrast tests for
all themes; virtualization tests with one million rows asserting constant frame cost; property grid
tests for mixed values and invalid input; interaction-standard tests for selection gestures and
Escape and Enter sequences; gesture recognizer tests over recorded event streams; snapshots of
each panel in the compact, medium and expanded size classes.

## Capabilities

### Workspace

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-UI-001 | Docking | Arranges panels as tabs and splits, drags panels into native windows and back, and restores layouts per monitor configuration. | T1 | 0 |
| CORE-UI-002 | Workspaces | Saves, switches, resets, exports and imports named workspaces including dock layout, toolbars and tool sets. | T1 | 0 |
| CORE-UI-003 | Menus and toolbars from data | Builds menus, toolbars and context menus from data that refers to command identifiers, showing bindings and disabled reasons. | T1 | 0 |
| CORE-UI-004 | Key bindings | Binds keys and chords to commands from settings, detects conflicts, and shows bindings everywhere a command appears. | T1 | 0 |

### Command input

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-UI-005 | Viewport widget | Hosts viewport textures, routes navigation, picks and prompt input, and supports several synchronized or independent viewports per document. | T1 | 0 |
| CORE-UI-006 | Command line | Shows prompts and keyword options, accepts commands with inline parameters, completes input and keeps a searchable history. | T1 | 0 |
| CORE-UI-007 | Command palette | Searches enabled commands by localized label, keyword, identifier and alias with fuzzy ranking weighted by recent use. | T1 | 0 |
| CORE-UI-008 | Tool option bar | Shows the active prompt state's parameters and options as controls synchronized with the command line. | T1 | 1 |
| CORE-UI-009 | Dynamic input near the cursor | Shows the current prompt and editable distance and angle fields next to the pointer during point input. | T2 | 1 |

### Data widgets

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-UI-010 | Property grid | Shows categorized fields for single and multiple selection with mixed-value display, override markers and read-only states; edits emit one command per change. | T1 | 0 |
| CORE-UI-011 | Numeric and unit input | Accepts expressions with units, shows evaluated values in display units, supports drag scrub, and rejects invalid input inline without changing the value. | T1 | 0 |
| CORE-UI-012 | Tree and outliner | Displays virtualized hierarchies with lazy children, multi-selection, drag and drop as commands, inline rename, filtering and visibility and lock toggles. | T1 | 0 |
| CORE-UI-013 | Virtualized tables | Scrolls tables of millions of rows at display rate by fetching row windows, with sorting and filtering in the source, frozen columns, resizable and reorderable columns and in-place editing as commands. | T1 | 1 |
| CORE-UI-014 | Table clipboard | Copies selected cells as tab-separated text and pastes tab-separated text into editable cells as one command. | T2 | 1 |
| CORE-UI-015 | Style pickers | Provides color, layer, linetype, lineweight and material pickers reading document style tables. | T1 | 1 |
| CORE-UI-016 | Node editor | Edits `arc-nodes` graphs with typed sockets, link validation, frames, subgraph navigation, inline parameters and node evaluation states. | T2 | 3 |
| CORE-UI-017 | History and comparison views | Shows journal history and state comparisons with filtering and navigation to objects. | T2 | 2 |

### Feedback

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-UI-018 | Notifications | Shows non-modal messages with severity and optional actions, kept in a history panel. | T1 | 0 |
| CORE-UI-019 | Progress and cancel | Shows progress for operations over 300 ms and a cancel control for operations over 2 s. | T1 | 0 |
| CORE-UI-020 | Diagnostics panel | Lists document diagnostics with severity, filtering and navigation to the objects involved. | T1 | 0 |

### Theming, localization and accessibility

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-UI-021 | Theme tokens | Styles every widget from theme tokens; light, dark and two high-contrast themes are shipped and switch without restart. | T1 | 0 |
| CORE-UI-022 | Density and scale | Applies compact, standard and comfortable density and UI scale from 75 % to 300 % per monitor. | T1 | 0 |
| CORE-UI-023 | Vector icons | Renders project-drawn SVG icons into an atlas at the current scale, recolored by theme tokens. | T1 | 0 |
| CORE-UI-024 | Localized strings | Resolves all visible strings through Fluent message identifiers; missing translations fall back to English. | T1 | 0 |
| CORE-UI-025 | Right-to-left layout | Mirrors panel layouts and aligns text by logical start and end when the layout direction is right to left. | T2 | 1 |
| CORE-UI-026 | Keyboard operation | Makes every control reachable and operable by keyboard with visible focus and focus order matching visual order. | T1 | 0 |
| CORE-UI-027 | Accessible names and roles | Exposes a name and role for every interactive widget through AccessKit, including viewport selection and prompt text. | T1 | 0 |
| CORE-UI-028 | Contrast and motion | Meets 4.5:1 text and 3:1 control contrast in every theme and follows the operating system's reduced-motion setting. | T1 | 0 |

### Interaction standards

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-UI-029 | Selection gestures | Implements click, Shift+click, Ctrl+click, window and crossing selection, pre-selection and click cycling identically in every viewport. | T1 | 0 |
| CORE-UI-030 | Snapping feedback | Shows a marker shaped by snap kind with a tooltip, and alignment tracking lines from acquired points. | T1 | 1 |
| CORE-UI-031 | Escape and Enter | Escape steps back through prompt step, command and selection; Enter accepts defaults, completes input or repeats the last command when idle. | T1 | 0 |
| CORE-UI-032 | Undo keys | Undo and redo work from the keyboard at all times, stepping back inside an active command first. | T1 | 0 |

### Touch, pen and small screens

The behavior is specified in [ui-kit.md](../architecture/ui-kit.md#browser-tablet-and-phone-targets).

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-UI-033 | Pointer sources | Delivers mouse, touch and pen events with kind, position, pressure, tilt and contact size, and applies every interaction standard identically to each kind. | T3 | 5 |
| CORE-UI-034 | Touch navigation gestures | Maps tap, long press, one- and two-finger drags, pinch, twist, double tap and two- and three-finger taps to selection, navigation, context menus, undo and redo, each with a button or menu equivalent. | T3 | 5 |
| CORE-UI-035 | Pen input | Treats the pen as the primary pointer while touch navigates, rejects palm contacts, exposes pressure and tilt to tools, maps barrel button and eraser end to configurable commands and shows pre-selection on hover where the hardware reports it. | T3 | 5 |
| CORE-UI-036 | Touch precision | Shows a loupe while a point is placed by touch, offers offset-cursor and nudge input, scales pick and snap radii by pointer kind and keeps interactive targets at 44 logical pixels or more. | T3 | 5 |
| CORE-UI-037 | Size-class layouts | Switches between compact, medium and expanded layouts live as the window changes, storing the dock layout per class and turning panels, menus and dialogs into sheets in the compact class. | T3 | 5 |
| CORE-UI-038 | Safe areas and orientation | Keeps panels and overlays inside safe areas and preserves camera, selection and the active prompt across rotation, resizing and split-screen changes. | T3 | 5 |
| CORE-UI-039 | Soft keyboard and text entry | Raises an on-screen keyboard with accessory rows and a numeric keypad for quantity fields, keeps the focused field visible and preserves input method composition. | T3 | 5 |
| CORE-UI-040 | Hover-free alternatives | Provides a touch equivalent for every hover-only feature, including pre-selection on contact, tooltips on long press and snap names in a status strip. | T3 | 5 |
| CORE-UI-041 | Touch accessibility | Exposes the accessibility tree to the platform screen readers on tablet and phone and through an accessible mirror in the browser, follows system text, contrast and motion settings and offers a single-pointer alternative for every gesture. | T3 | 5 |
