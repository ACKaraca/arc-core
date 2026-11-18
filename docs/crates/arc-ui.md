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

## Foundations as implemented

The first part of `arc-ui` runs without a window: it depends on `arc-base`, `arc-command`,
`egui` (without default features, so no bundled fonts) and `resvg` (without default features),
and not on `winit` or `arc-gpu`. Tests drive it through a headless `egui::Context`. Widgets
return an `Intent` naming a command and never run one. Fallible functions return
`arc_base::Error`.

### Theme tokens and contrast (CORE-UI-021, CORE-UI-028)

- A theme is `Colors` (surfaces, field, control and hover fills, selection, viewport, text,
  muted and disabled text, border, accent and text on accent, focus, the four severities,
  viewport selection and pre-selection) and `Metrics` (spacing, padding, row height, indent,
  margin, corner radii, stroke and focus widths, four type sizes, icon size). The four shipped
  themes are light, dark, high-contrast light and high-contrast dark; they are Rust constants.
- `Appearance` (theme, density, scale, reduced motion) maps the tokens onto an `egui::Style`.
  `apply` installs that style in the light and the dark slot of a context, so the platform's
  light or dark preference does not replace the chosen theme; it takes effect on the next pass.
- `Colors::pairs` lists every foreground and background token pair the style mapping and these
  widgets draw together. Text pairs need 4.5 : 1: text on every surface, field, control, hover
  and selection fill; muted text on surfaces and fields; the severities and the accent on
  surfaces; text on accent. Control pairs need 3 : 1: border and accent on surfaces and fields,
  focus on surfaces, fields and controls, viewport selection and pre-selection on the viewport.
  Disabled text is not checked, as inactive components are exempt. Ratios use the WCAG 2.x
  relative luminance of sRGB colors.
- Reduced motion is an input from the caller (the operating-system setting). It sets the
  animation time to zero, turns off scroll animation and cursor blinking, and stops the
  indeterminate progress animation.

### Density and scale (CORE-UI-022)

- Density multiplies spacing, padding, indent, margins and row height: compact 0.8, standard
  1.0, comfortable 1.25. Type sizes, radii, strokes and icons do not change with density.
- `UiScale` is an integer percent from 75 to 300 (`ui.scale.out_of_range` otherwise, also when
  deserialized). It multiplies every metric after density and applies on top of the platform's
  pixels-per-point factor.

### Vector icons (CORE-UI-023)

- The starter set in `crates/arc-ui/icons/` (check, chevron-right, close, error, info, plus,
  search, warning) is original work drawn for this repository and licensed under the
  repository license. Icons are single-color on a 16 × 16 view box.
- `IconAtlas::build` rasterizes up to 256 icons, each source at most 16 KiB, at an edge of 1 to
  128 pixels into square cells with one pixel of padding, at most 16 per row. Names are 1 to 64
  lowercase letters, digits and `-`. Only coverage is kept: every texel is white with the icon's
  alpha. Drawing multiplies by a color token, so one atlas serves every theme. `edge_for` turns
  the icon token size and the pixels-per-point factor into the pixel edge.
- References to files and embedded images in SVG sources are never resolved.

### Localized strings (CORE-UI-024)

- Visible strings are Fluent message identifiers resolved through the `Localize` trait, which
  returns `None` for an identifier the source does not define. `arc-app` is in the same layer and
  may depend on `arc-ui`, not the reverse, so the trait lives here and the application's
  localizer implements it.
- `Fallback` puts an English source behind the active one per message; `resolve` shows the
  identifier itself when no source defines it. The messages of this crate are in
  `locales/en-US/arc-ui.ftl`.

### Menus, toolbars and context menus (CORE-UI-003)

- A `Menu` is a list of items (`command` with an identifier, `submenu` with a label message
  identifier and items, `separator`); a `Toolbar` is a list of `command` items with an optional
  icon name and `separator`s. Both deserialize with unknown fields rejected and are validated:
  command identifiers as in `arc-command`, label identifiers of at most 128 bytes, at most 8
  levels and 4,096 items over all levels.
- Resolution against the registry gives the localized label, the display text of the first key
  binding and the localized disabled reason. A command that is not registered or not exposed on
  the UI surface is left out and reported with `ui.command.unknown`; the rest still resolves.
- The enabled state comes from a `CommandState` the application supplies, because the command
  specification's enabled predicate is not public outside `arc-command`.
- Disabled items are drawn disabled with their reason as the hover text. Icon toolbar buttons
  take the command's label as their accessible name and tooltip.

### Key bindings (CORE-UI-004)

- A binding is `{ keys, command }` as read from settings. Keys are one to four strokes separated
  by spaces; a stroke is modifiers (`Ctrl`, `Alt`, `Shift`, `Meta`, case-insensitive, each at
  most once) and an egui key name joined by `+`, at most 128 bytes in all. Display text is the
  canonical form (`Ctrl+Shift+S`, `Ctrl+K Ctrl+C`); key names are not localized.
- `Keymap::build` keeps bindings in order. A later binding is dropped and reported when its
  sequence is already bound to another command (`ui.keys.conflict`; a repeat for the same
  command is dropped silently), when it is a prefix of a kept sequence or a kept sequence is a
  prefix of it (`ui.keys.prefix_conflict`), when its text is malformed (`ui.keys.invalid`) or
  its command is unknown (`ui.command.unknown`). At most 1,024 bindings are kept; the rest are
  reported once with `ui.keys.too_many`.
- `ChordResolver` takes strokes with a time from the caller's clock. A completed sequence yields
  its intent; a proper prefix keeps the chord pending; anything else drops the chord and reports
  the stroke unbound. A pending chord expires when the next stroke comes more than 1,500 ms
  after the previous one; a clock running backwards counts as no time passed. Repeats and
  releases are ignored.

### Command palette (CORE-UI-007)

- Only enabled commands are ranked. A command matches through its localized label, identifier,
  user aliases and localized keywords (the keyword message split at commas and white space).
  The match kind is the best over these fields: exact, then prefix of the field or of a word in
  it (words split at white space, `.`, `-` and `_`), then subsequence.
- Order: match kind, then recent use (most recent first), then the span of a subsequence match
  (shorter first), then label and identifier. An empty query lists every enabled command in
  this order.
- Text is compared lowercased. The query is trimmed and cut to 64 characters, each field to 256
  characters, and at most 16 aliases and 16 keywords count, so work per command is bounded.
  Locale-specific case folding (Turkish dotted and dotless i) is not applied yet.
- The recent list holds 50 commands; the application records an invoked command. The widget
  shows at most 50 results; arrow keys move the highlight and Enter returns the highlighted
  command.

### Notifications and progress (CORE-UI-018, CORE-UI-019)

- A notification has a severity (information, success, warning, error), a message identifier
  with arguments, up to four actions (a label identifier and a command intent) and the post time
  from the caller's clock. The history keeps 200 notifications; dismissing keeps a notification
  in the history. The severity name is shown with its color, so color is not the only signal;
  clicking an action returns its intent and dismisses the notification.
- `Progress` shows progress once an operation has run 300 ms and a cancel control once a
  cancellable operation has run 2 s, measured on the caller's clock. A fraction outside
  `[0, 1]` is clamped; a non-finite fraction makes the progress indeterminate.

### Accessibility (CORE-UI-026, CORE-UI-027, in part)

- Menu items, toolbar buttons, palette results and the palette's search field expose a role and
  a name through egui's AccessKit tree; the search field is labelled by a visible label, and
  disabled items are marked disabled. A headless test inspects the tree for a menu, a toolbar
  and the palette. Keyboard traversal tests and image snapshots come with `egui_kittest`.

### Error codes

The codes `arc-ui` owns; messages are in `locales/en-US/arc-ui.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `ui.scale.out_of_range` | `invalid_input` | A UI scale is outside 75 % to 300 % |
| `ui.icon.invalid` | `invalid_input` | An icon name or pixel edge is out of range, or its SVG source does not parse or has no size |
| `ui.icon.too_large` | `limit_exceeded` | An SVG source exceeds 16 KiB |
| `ui.icon.too_many` | `limit_exceeded` | An atlas would hold more than 256 icons |
| `ui.icon.duplicate` | `conflict` | An icon name occurs twice in one set |
| `ui.menu.invalid` | `invalid_input` | A menu or toolbar has a malformed command identifier, label identifier or icon name |
| `ui.menu.too_deep` | `limit_exceeded` | A menu nests more than 8 levels |
| `ui.menu.too_large` | `limit_exceeded` | A menu or toolbar has more than 4,096 items |
| `ui.command.unknown` | `not_found` | A menu item or key binding names a command not registered for the UI surface |
| `ui.keys.invalid` | `invalid_input` | A key binding text is not a key sequence |
| `ui.keys.conflict` | `conflict` | A key sequence is already bound to another command |
| `ui.keys.prefix_conflict` | `conflict` | A key sequence is a prefix of a bound one, or a bound one is a prefix of it |
| `ui.keys.too_many` | `limit_exceeded` | A key map has more than 1,024 bindings |

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
