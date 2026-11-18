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
| Third-party | `egui`, AccessKit (through egui), `resvg`; planned for later capabilities: `egui_table`, `egui-snarl`, `parley`. Docking and the tree are widgets of this crate. |

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
and not on `winit` or (without the feature `viewport`, see below) `arc-gpu`. Tests drive it through a headless `egui::Context`. Widgets
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
- The enabled state comes from a `CommandState` the application supplies; it answers in one line
  from `CommandSpec::enabled` with a `Probe` of the current snapshot, selection and session.
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
| `ui.dock.invalid` | `invalid_input` | A dock layout has an empty group other than the main root, a dangling, shared or unreachable node, an active index out of range, a malformed panel identifier, a fraction outside 0.05 to 0.95 or a malformed window rectangle |
| `ui.dock.too_deep` | `limit_exceeded` | A dock tree nests splits more than 16 levels |
| `ui.dock.too_large` | `limit_exceeded` | A dock layout has more than 545 nodes, 256 panels or 32 floating trees |
| `ui.dock.panel_unknown` | `not_found` | A dock operation names a panel or anchor the layout does not show |
| `ui.dock.panel_duplicate` | `conflict` | A dock layout would show a panel twice |
| `ui.workspace.invalid` | `invalid_input` | A workspace, toolbar or tool set name is malformed or repeated, a tool set names a malformed command, no built-in workspace is given, or an import text does not parse |
| `ui.workspace.unknown` | `not_found` | No workspace, or for reset no built-in workspace, has the name |
| `ui.workspace.too_large` | `limit_exceeded` | More than 64 workspaces, 32 toolbars, 64 tool sets or 256 tools per set, or an import text over 1 MiB |
| `ui.workspace.version` | `unsupported` | An import text has another format name or a version other than 1 |
| `ui.quantity.out_of_range` | `invalid_input` | An entered value is outside the field's range |
| `ui.quantity.not_integer` | `invalid_input` | An entered value is not a whole number where one is needed |
| `ui.property.too_large` | `limit_exceeded` | The selected objects times their fields exceed 1,000,000 |

## Panels and widgets as implemented

The second part adds the workspace, command input and data widgets. It depends in addition on
`arc-doc` (field declarations, values and diagnostics), `arc-solve` (expressions with units) and
`serde_json` (intent parameters and workspace export); no third-party crate was added. Every
widget has a data model that tests use without drawing. The error codes are in the table above.

### Intents with parameters

An `Intent` carries a command identifier and a JSON object of parameters, which the application
passes to `CommandLine::start` or an invocation; the command prompts for anything missing.
Objects are listed as hyphenated identifier texts.

### Docking (CORE-UI-001)

- No published `egui_dock` release supports egui 0.37 (the newest needs egui 0.36), so the dock is
  this crate's own: a `DockLayout` of a main tree and up to 32 floating trees. A tree is a tab
  group of panel identifiers (1 to 128 bytes of ASCII letters, digits, `.`, `-`, `_`, starting
  with a letter) or a horizontal or vertical split with a fraction in 0.05 to 0.95. Only the main
  root may be an empty group. Splits nest at most 16 levels; a layout holds at most 256 panels,
  each at most once.
- The serialized form is a flat node list with indices and a root index per tree, so decoding
  does not recurse. Every node must be referenced exactly once and reached from a root within
  the depth limit; then the nodes form trees. Floating trees carry a window rectangle of finite
  coordinates up to 10^6 points with positive size.
- Operations: add a panel, move it (into a group at an index, beside a group on a side, beside
  the whole main tree, or into a new floating tree), close, activate, float, unfloat (beside the
  main tree on the right) and set a split fraction (clamped). Each works on a copy that is
  normalized (empty groups removed, a split with one part replaced by that part) and checked, so
  a failed operation leaves the layout unchanged. Floating trees are data only; native windows
  for them come with the application shell.
- `DockArea` draws the main tree: a tab bar per group with a close button per tab, the active
  panel's content, and separators that pointer drag or, when focused, the arrow keys along the
  split (5 % per press) move. A tab dragged onto a group's outer quarter on a side splits the
  group on that side; onto its center it joins the group. Separators have the role splitter.

### Workspaces (CORE-UI-002)

- A `Workspace` has a name (1 to 64 characters, no control characters or surrounding white
  space), a dock layout, up to 32 named toolbars and up to 64 named tool sets of up to 256
  command identifiers; names are unique within a workspace.
- `Workspaces` holds 1 to 64 built-in workspaces, up to 64 saved ones and the active name. Saving
  under a built-in name replaces that built-in until `reset`; switching needs an existing name.
- Export writes `{ "format": "arc-workspace", "version": 1, "workspace": ... }` as JSON text;
  import reads at most 1 MiB, requires that format and version, and validates the workspace and
  its layout. The caller reads and writes files.

### Command line (CORE-UI-006, CORE-UI-031, CORE-UI-032)

- `CommandLineView` draws the state of `arc_command::CommandLine` without calling it: the active
  prompt's localized message (or the command-line label when idle), its keyword options as
  buttons (a click submits the option), the input field labelled by the prompt, and up to 10
  completion candidates the caller computed with `CommandLine::complete`. It returns a
  `LineAction`: submit a text, repeat, back, cancel, invoke an intent, or cancel and then invoke.
- Escape clears typed text, else steps back one answer, else cancels a command at its first
  prompt, else (idle) emits the clear-selection intent when the selection is not empty. Enter
  applies the highlighted completion; otherwise it submits the text (an empty text accepts the
  prompt's default), and on an idle empty line it repeats the last command.
- Undo (Ctrl+Z, Cmd+Z on macOS) steps back one answer inside the active command, cancels it at its
  first prompt, and otherwise emits the document undo intent. Redo (Ctrl+Y or Ctrl+Shift+Z)
  emits the redo intent; with an active command it cancels the command first, because its
  answers may refer to state that redo replaces. These keys, Escape and Enter are taken while
  the field or nothing has focus, before the field's own text undo; the field keeps focus on
  Escape and the arrow keys.
- The history keeps 200 submitted lines, most recent first, without repeats, each cut to the
  driver's line limit. Up and Down highlight completion candidates while there are any for typed
  text; otherwise they walk the history entries that contain the text typed before the walk
  (ignoring case), and Down past the newest restores that text.

### Property grid (CORE-UI-010)

- A `PropertyObject` gives an object's field declarations, values and the names of its read-only
  and overridden fields. The model has the fields every selected object has with the same type,
  in the first object's order, grouped by aspect in order of first appearance. A row shows the
  common value (compared bit for bit), the mixed state, or nothing when no object has a value; it
  is read-only when any object marks it so or its type has no editor, and shows an override marker
  when any object marks it overridden. Objects times fields are limited to 1,000,000.
- Editors: a check box (indeterminate when mixed) for truth values, a choice for enums, a text
  field for text, numeric fields for integers, reals and quantities, and three stacked length
  fields (x, y, z, in the display unit of lengths) for a point in space; committing one axis
  commits the point with that axis replaced, and a mixed or absent point is read-only. Other
  types show their canonical JSON text, cut to 64 characters, read-only.
- A check box or choice commits at once; text and numbers commit on Enter or when the field loses
  focus with a changed text, and the committing Enter is consumed so it does not also reach the
  command line; Escape restores the shown value. Each commit returns exactly one
  intent for the grid's edit command with `objects`, `field` (the field path text) and `value`
  (the value's canonical JSON form, quantities in SI base units).

### Numeric and unit input (CORE-UI-011)

- `QuantityField` reads text with `arc_solve::parse_quantity` in the field's dimension, with the
  display unit as the unit of bare numbers and the user's decimal separator, then checks the
  field's range and, for integer fields, that the value is whole. Values show through the
  display format; without one, as the SI number.
- Invalid text stays in the field with the error's localized message below it; nothing is
  returned for it and the value is unchanged.
- A handle (role spin button) beside the field scrubs the value by the step (one display unit by
  default) per logical point of drag, a tenth with Shift and ten times with Ctrl, clamped to the
  range, and returns the value once when the drag ends; focused, Up and Down step it.

### Tree and outliner (CORE-UI-012)

- A `TreeSource` answers child counts, single children by index and the index of a child, each
  for a parent and a filter text (at most 256 characters) the source applies. The view asks only
  for the children of expanded nodes and only for the rows in the scrolled window, at most 256
  rows per frame, using the remembered (parent, index) of each expanded node (at most 4,096,
  shown at most 64 levels deep) to skip collapsed blocks by count. Each frame the view checks the
  expanded nodes against the source, following a node to its new index or forgetting it.
- Intents: select (`objects`, `mode` `replace` or `toggle`; Shift selects the range from the
  anchor row, at most 10,000 rows; Ctrl or Cmd toggles), rename (`object`, `name`; F2 or a double
  click starts it, Enter commits, Escape cancels), visibility and lock toggles (`objects` and the
  new state) and moving a dragged row under the drop row (`objects`, `parent`). Right and Left
  expand and collapse a focused row; Space selects it.

### Diagnostics panel (CORE-UI-020)

- The panel lists a `DiagnosticsSnapshot` virtualized, each row with its severity name and
  localized message. Check boxes keep errors and warnings; the text filter (at most 256
  characters, ignoring case) matches the code or the localized message. Activating a row with
  objects returns the navigation intent with `objects`.

### Keyboard operation (CORE-UI-026, in part)

- Every control of these widgets is focusable and named: tab buttons and close buttons
  (`Close <panel>`), separators, the command field (labelled by the prompt), property editors
  (labelled by the field), scrub handles, tree toggles (`Visible <name>`, `Locked <name>`) and
  rows, and the filter fields. Focus follows drawing order, which is visual order. A headless
  test tabs through a dock holding the command line, a property grid, a tree and the diagnostics
  panel and checks every focused node's name and role and that each comes after the previous one
  down the page or to its right in the same row.

## Viewport and selection gestures as implemented

The third part adds the viewport widget and the selection gestures behind the feature `viewport`,
which native products enable. It depends in addition on `arc-gpu` (navigation, picking types,
overlays) and `arc-geom` (view frames), both optional. The feature is off by default, so the
WebAssembly build of this crate (`cargo xtask wasm`) does not reach `arc-gpu`; the layer check
does not follow optional dependencies that the default features leave off. No error code was
added: picking and navigation errors keep their `arc-gpu` and `arc-geom` codes.

### Interaction state machine (CORE-UI-029)

- `Gestures` turns pointer and keyboard events in viewport pixels into camera changes, the
  pre-selected object, the rubber band, selection intents and prompt answers. It draws nothing
  and holds no selection; the caller supplies the camera, image size, active prompt, selection
  count, tolerance and a `ViewPicker` (the identifier pass of `arc-gpu` over the shown scene and
  the scene's bounding sphere). `ScenePicker` implements it with an `arc_gpu::Renderer` and
  `Scene`; tests use a fake.
- Thresholds are logical points multiplied by the pixels-per-point factor and the UI scale:
  drag distance 4, cycle radius 3, pick square half side 4 (also the line pick tolerance).
  Double-click time is 0.4 s on the caller's clock; a clock running backwards counts as no time
  passed. Non-finite positions and amounts are ignored.
- A primary press and release within the drag distance is a click. A click takes the stack
  under the pointer: the object visible at the pointer pixel, then the other objects visible in
  the pick square (at most 64). A further click within the cycle radius of the previous one,
  with no camera change between them, takes the next object of the same stack, wrapping
  around. A click selects its object (replace; Shift adds; Ctrl, Cmd on macOS, toggles); a click
  on nothing emits nothing. Double clicks are not a selection gesture.
- A primary drag beyond the drag distance is a rubber band. Left to right it is a window: the
  objects with a visible pixel inside the rectangle and none in the rest of the image. Right to
  left it is a crossing: the objects with a visible pixel inside. The press modifiers choose the
  mode; an empty band replaces with nothing and emits nothing for add or toggle. Escape cancels
  a press or band; otherwise, with no prompt and a non-empty selection, it emits the
  clear-selection intent.
- Every click and band emits at most one intent for `ViewportCommands::select` (default
  `core.select.set`) with `objects`, `mode` (`replace`, `add`, `toggle`) and `gesture`
  (`click`, `window`, `crossing`); `clear_selection` defaults to `core.select.clear`. Hover,
  navigation and prompt answers emit no selection intent, so the selection changes only
  through commands.
- Pre-selection, while no button is down, is the object a click would select: the next object
  of the stack near the last click, else the object at the pointer pixel. While a prompt that
  accepts no object is active nothing is pre-selected.
- While a command prompts, the primary button answers it: a click on an object answers a pick
  prompt with `PromptInput::Pick` and a selection prompt with a one-object `Selection`;
  otherwise a point prompt gets the point where the pick ray meets the view plane (through the
  camera target, facing the eye), without z for a 2D-only prompt. A band answers a selection
  prompt and is not started for other prompts; prompts that accept none of these ignore the
  button, and Escape stays with the command line.
- Navigation uses `arc_gpu::navigation`: middle drag pans so the target follows the pointer;
  Shift with middle drag orbits about the target by 0.01 rad per point (`yaw = −dx·k`,
  `pitch = −dy·k`); the wheel zooms by `exp(0.002 · points)` at the view-plane point under the
  pointer (a line is 40 points, a page the viewport height), and a pinch by its factor; a
  second middle press within the double-click time and drag distance zooms to the scene's
  extents. With focus, arrows orbit by 15°, Shift with arrows pans by a tenth of the image
  height, `+` and `-` zoom by 1.25 at the target and Home zooms to the extents. A camera change
  forgets the click stack.
- Picks see only visible pixels: an object hidden behind others in the whole pick square is not
  in the stack, and hidden parts or parts outside the image do not count against a window.

### Viewport widget (CORE-UI-005)

- `ViewportWidget::show` fills the available space, paints the viewport background token and
  the texture the caller registered for the current frame (an `egui::TextureId`; registering
  `arc_gpu::Frame` textures with the egui renderer belongs to the application's integration,
  so this crate does not depend on `egui-wgpu`), and feeds its events to the state machine:
  presses inside the area, moves inside it or during a press, releases, the wheel and pinch
  while the pointer is over it, and keys while it has focus. A press gives it focus.
- It returns the pixel size it wants (the area times the pixels-per-point factor, 1 to 65,535
  per side), the camera for `arc_gpu::Viewport::set_camera`, the pre-selected object for
  `set_highlight`, the rubber-band overlay for `set_overlay`, the selection intent, the prompt
  answer for `CommandLine::answer` and the errors of the frame.
- Rubber bands are `arc-gpu` overlay primitives in screen pixels: a window is a closed solid
  outline in the viewport selection token, a crossing a dashed outline (6 points on, 4 off, at
  most 256 dashes per side) in the pre-selection token, each over a fill of its token at alpha
  40, so the kinds differ by more than color.
- Several viewports of one document each have a widget with its own gesture state. Cameras are
  independent, or shared through a `ViewLink`: a widget whose camera changed publishes it, and
  every other widget on the link returns the published camera on its next frame (in the same
  frame when drawn later).

### Accessibility of the viewport (CORE-UI-027, in part)

- The viewport area has the role canvas, the name `ui-viewport` with the caller's view name and
  the description `ui-viewport-selection` with the selection count, or `ui-viewport-status`
  with that text and the localized prompt message while a command prompts. A headless test
  inspects these in the AccessKit tree.

## Localization, scale and view cube as implemented

**Case folding.** `text::fold_case(text, language)` folds case for comparison with the rule of
`language`, a BCP 47 tag. For Turkish (`tr`) and Azerbaijani (`az`) `I` folds to `ı` and `İ` to
`i`, so a query typed with either letter matches the Turkish word it stands for; for every other
language, including the empty tag, it is Unicode lowercasing, so ASCII behavior is unchanged.
Folding is idempotent and has no global state. `palette::rank_in` and
`diagnostics::filter_in` take the language (`rank` and `filter` fold with the empty tag);
`Palette::language` and `DiagnosticsPanel::language` hold the active tag for the widgets.
Diagnostic codes are matched with plain lowercasing in every language, since they are ASCII
identifiers. The tree filter is applied by the `TreeSource`, which is the caller's; sources call
`text::matches_filter(name, filter, language)` to apply the same rule.

**Turkish catalogs.** `locales/tr-TR/` holds a translation of every English catalog, with the
same identifiers, attributes and variables per message, and a glossary in `arc-base.ftl`.
`cargo xtask catalogs` checks every locale directory besides `en-US` against it: the same files,
the same message identifiers and attributes, the same set of variables in each message and valid
Fluent syntax; a missing or extra message fails. Plural messages use the exact number `[0]`,
`[1]` and the `other` category, since Turkish selects `other` for every count.

**Scale.** `tests/layout.rs` draws the menu bar, toolbar, palette, notifications with progress,
command line, property grid, tree, diagnostics, a dock holding the last four, and the viewport
with a texture and the view cube, in all four themes, three densities and at 75, 100, 200 and
300 percent. It measures, from the AccessKit bounds after three passes: that no node leaves its
container or its parent's bounds, that no two leaf nodes overlap by more than half a point in
both directions (nodes with identical rectangles, the view cube's alternative handles, are one
region), that the cube lies inside the viewport, that buttons, check boxes and text fields are
at least the token row height tall (`Metrics::row_height` after density and scale) and that list
rows are equally tall. It does not measure text truncation inside a node, painted pixels or
closed popups. The test found and the kit fixed three defects: the progress row pushed its
cancel button out of the row, the diagnostics header row did not wrap, and the dock close
button and the tree expand button were shorter than a row.

**View cube** (feature `viewport`, `view_cube`). `ViewCube::show_over` draws an orthographic
cube in the top right corner of a viewport rectangle, oriented like the camera, with the
localized name of each face that faces the eye. The cube has 26 targets, `CubeTarget`, each a
direction from the view target toward the eye with components in -1, 0, 1: six faces, twelve
edges and eight corners. The world is Z-up; faces are Top (+Z), Bottom (-Z), Front (-Y), Back
(+Y), Right (+X) and Left (-X); the up hint is +Y when looking along Z and +Z otherwise. The pure
rule `view_cube::hit(frame, q)` maps a view-plane position to the part under it: the point is
solved onto each face that faces the eye, and within `EDGE_BAND` (0.3 of the half width) of one
or two face edges it selects the edge or corner, otherwise the face. A click or Enter or Space on
a focused target returns a `ViewIntent` (target, direction, up hint); `ViewIntent::frame_from`
turns it into a `ViewFrame` that keeps the target, distance, projection and clip planes, and the
caller animates with `arc_gpu::navigation::interpolate`. The 26 targets are focusable buttons
named by `ui-view-cube-*` messages, hidden ones included. Only the pointer-hovered region is
highlighted; a focused target is shown by a focus indicator around the whole cube.

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
