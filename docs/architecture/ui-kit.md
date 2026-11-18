# UI kit

The widgets (`arc-ui`) and application shell (`arc-app`) shared by every Arc product on the desktop,
in the browser and on tablets and phones, and the interaction standards that make the products
behave alike. All UI is egui on wgpu and winit;
upstream crates are used unmodified, and any fork carries a patch ledger with removal conditions.

## Structure

- `arc-ui` contains widgets and interaction logic that know nothing about any product. Widgets read
  data through traits (a property source, a tree source, a table source) and emit command
  invocations; they never mutate documents directly.
- `arc-app` composes the shell: windows, document tabs, workspace layouts, settings, localization,
  logging, crash reporting, update checks, plug-in management and the assistant panel. A product's
  desktop application is wiring: it registers its commands, panels, tools and extractors with
  `arc-app`.
- All egui use goes through `arc-ui` re-exports, so an egui upgrade is a single change in `arc-core`.

## Workspace and docking

- A **workspace** is a named layout: dock tree of panels (tabs, splits, floating windows), toolbar
  and menu configuration, and the visible tool sets. Products ship default workspaces; users save
  their own. Workspaces are stored per user and can be exported as files.
- Docking is a widget of `arc-ui` over a validated layout of tabs and splits: panels can be tabbed, split, dragged out into native windows
  (multi-viewport) and restored. Window positions are restored per monitor configuration; a layout
  saved on a monitor that is no longer present opens on the primary monitor.
- Document tabs hold one document each; a document may have several viewport panels and sheet
  panels at once.
- Native menu bars use `muda` on macOS and Windows; file dialogs use `rfd` with portal support on
  Linux. Both are driven from command identifiers.

## Widgets

| Widget | Specification |
|---|---|
| Viewport widget | Hosts an `arc-gpu` viewport texture; routes pointer and keyboard input to navigation, picking and the active prompt machine; shows the overlay; supports multiple viewports per document with synchronized or independent cameras |
| Command line | Prompt display, input with completion of commands, options, names and units, history, inline parameter entry; driven by `arc-command` parsing |
| Palette | Fuzzy command search with ranking by relevance and recent use, showing key bindings and disabled reasons |
| Tool option bar | The parameters and keyword options of the active prompt state as editable controls, synchronized with the command line |
| Property grid | Categorized fields of the selection with mixed-value display for multi-selection, unit-aware numeric input, expression entry, enum and reference pickers, per-field override markers and read-only states supplied by the source |
| Tree and outliner | Virtualized hierarchical view with lazy child loading, multi-selection, drag and drop (emits commands), inline rename, filtering and search, and visibility and lock toggles |
| Tables | Virtualized grids for very large data sets (schedules, layer lists, query results): rows are fetched in windows from a table source, so millions of rows scroll at display rate; sorting and filtering run in the source; frozen columns, column resizing and reordering, in-place editing that emits commands, copy to clipboard as tab-separated text |
| Numeric and unit input | Parses expressions with units through `arc-solve`, shows the evaluated value in display units, supports drag-scrub with modifier-controlled step, and rejects dimensionally invalid input with an inline message |
| Node editor | Graph canvas for `arc-nodes` graphs built on `egui-snarl`: typed sockets with color and shape per type, connection validation, groups and frames, subgraph navigation, inline parameter widgets, evaluation state and errors on nodes |
| Notifications | Non-modal messages with severity, optional action (a command) and a history panel; long operations show progress with cancel |
| Dialogs | Only for decisions that need the user's attention; every dialog maps to command parameters so programmatic paths never need it |
| Color, layer, linetype and lineweight pickers | Shared pickers reading document style tables |

## Theming and density

- Themes are **token sets**: colors (surfaces, text, accents, states, viewport backgrounds, selection
  and pre-selection), spacing, corner radii, font sizes and icon sizes, defined in data files.
  Widgets use tokens only, never literal colors.
- Shipped themes: light, dark, and high-contrast light and dark. Every theme passes the contrast
  checks below.
- **Density** settings (compact, standard, comfortable) scale spacing and row heights; **UI scale**
  from 75 % to 300 % applies on top of the operating system scale factor, per monitor.
- Icons are vector (SVG rasterized with `resvg` into an atlas at the current scale), drawn by the
  project, and recolored by tokens.

## Localization with Fluent

- Every user-visible string is a Fluent message identifier. Catalogs live in
  `locales/<language>/<crate or product>.ftl`. English is the source language; Turkish is the first
  complete translation.
- Message identifiers are namespaced by owner (`core-doc-save-failed`, `ui-property-grid-mixed`).
  Errors carry message identifiers and arguments, so L0–L4 never format sentences.
- Numbers, dates, units and lists are formatted through locale-aware functions; plural and gender
  rules come from Fluent; Turkish casing rules (dotted and dotless i) are respected in search,
  sorting and case conversion.
- Right-to-left support is designed in: layouts use logical start and end, text runs are shaped
  bidirectionally (`parley`), and a layout direction setting mirrors panels and toolbars.
- A pseudo-locale (expanded and accented strings) and a pseudo right-to-left locale are built in for
  testing truncation, hard-coded strings and mirroring.
- A CI check fails on missing identifiers in the English catalog and on unused identifiers; missing
  translations fall back to English and are reported.

## Accessibility

Requirements:

- Full keyboard operation: every command and every control is reachable and operable without a
  pointer; focus order follows visual order; focus is always visible.
- Every control has an accessible name and role, exposed through AccessKit to the platform screen
  reader; viewports expose the current selection and prompt as accessible text.
- UI scale up to 300 %, high-contrast themes, minimum contrast ratios of 4.5:1 for text and 3:1 for
  controls and focus indicators, no information conveyed by color alone, reduced motion following
  the operating system setting.

How they are tested:

- UI tests with `egui_kittest` query the AccessKit tree: every interactive node must have a name
  and role; a traversal test tabs through each panel and asserts every control receives focus.
- Contrast tests compute ratios for all token pairs used together in each theme.
- Snapshot tests run each panel at 100 % and 200 % scale, in the pseudo-locale and the pseudo
  right-to-left locale.
- Keyboard-only scenario tests drive the reference shell through open, edit, save and undo without
  pointer events.

## Settings

- Settings are typed and registered with a key, value type, default, scope and description message.
- Scopes resolve in order: built-in default → system (administrator) → user → workspace → document
  (only for settings declared document-scoped). The effective value and its source are visible in
  the settings editor.
- User settings are TOML files in the operating system configuration directory; unknown keys are
  preserved; a malformed file is set aside with a notification and defaults apply.
- Changing a setting from automation is a command (`core.settings.set`) that requires the
  `settings` permission class.

## Crash reporting and update checks

- Crash handling is out of process: a small monitor captures a minidump and the log tail when the
  application crashes and stores them locally. On the next start the user sees the report content
  and may send it to the configured endpoint (Sentry-compatible format). Sending is off by default;
  no endpoint receives anything without consent; reports never include document content.
- Update checks are opt-in. When enabled, the application fetches a signed release feed, verifies
  the signature, and offers the update; installation uses Velopack. No other network request is
  made at start-up.
- Both settings are visible on first run and in settings, and both can be locked off by system
  scope for managed installations.

## Interaction standards

All products follow these rules; product documents only add to them.

**Selection**

| Input | Effect |
|---|---|
| Click on an object | Replace the selection with it |
| Shift + click | Add to the selection |
| Ctrl + click (Cmd on macOS) | Toggle in the selection |
| Drag left to right | Window selection: objects fully inside |
| Drag right to left | Crossing selection: objects inside or touching |
| Escape with no active command | Clear the selection |
| Hover | Pre-selection highlight of the object that a click would select |

Repeated clicks at the same spot cycle through overlapping objects. Selection is session state;
commands that change it record the resulting selection so undo restores it.

**Snapping feedback.** The active snap shows a marker whose shape identifies the snap kind (end,
middle, center, intersection, perpendicular, tangent, nearest, grid, and product-defined kinds)
with a tooltip naming it; alignment tracking draws dotted guide lines from acquired points. Snap
kinds are toggled by commands and remembered per user.

**Escape and Enter.** Escape cancels the current prompt step, then the command, then clears the
selection. Enter accepts the prompt's default or completes a multi-step input; with no active
command, Enter repeats the last command. Both behave identically in the viewport, the command line
and the tool option bar.

**Undo granularity.** One completed command is one undo step; a drag is one step; a script, macro or
batch run is one step unless it manages groups itself. Undo and redo are always available from the
keyboard, also during a prompt (where they step back inside the command first).

**Feedback for long work.** Operations longer than 300 ms show progress; longer than 2 s offer
cancel; the UI never freezes on document work.

## Browser, tablet and phone targets

The same widgets and shell run in the browser (WebAssembly with WebGPU) and on tablets and phones
(iOS and Android); what each layer guarantees there is in the
[architecture overview](overview.md#platform-targets). This section specifies what changes for the
user: input, layout, text entry, storage and accessibility. Nothing here changes the desktop
behavior above.

### Pointer sources

Mouse, touch and pen are equal pointer sources. Widgets receive pointer events with a kind
(mouse, touch, pen), position, pressure and tilt where the hardware reports them, and contact
size. Interaction standards (selection, snapping, Escape and Enter, undo) hold for every kind.

| Gesture | Effect |
|---|---|
| Tap | Select the object under the contact; with an active prompt, supply the point or pick |
| Tap on the selection | Cycle through overlapping objects |
| Long press | Open the context menu; press then drag draws a window or crossing selection by the same direction rule as the mouse |
| One-finger drag on empty space | Pan in two-dimensional views, orbit in three-dimensional views; with a drawing tool active it moves the pending point instead |
| Two-finger drag, pinch, twist | Pan, zoom about the contact center, rotate the view |
| Double tap | Zoom to the point or to the selection |
| Two-finger tap, three-finger tap | Undo, redo (configurable) |
| Pen contact | Selects, picks and draws like the primary mouse button |
| Pen barrel button, eraser end | Configurable command, by default pan and delete-under-pen |
| Pen hover | Where the hardware reports hover: pre-selection highlight and snap markers as with a mouse |

Rules:

- **Pen and touch together.** While a pen is in range, touch navigates and the pen draws; touch
  contacts larger than the palm threshold, or made while the pen touches, are ignored. A setting
  makes touch draw as well.
- **Pick radius.** Hit-test and snap radii scale with the pointer kind: 4, 6 and 12 logical pixels
  for mouse, pen and touch as defaults. Grips and manipulators draw at 12 px and hit-test at 44 px.
- **Target size.** Interactive controls are at least 44 logical pixels (about 9 mm) in each
  dimension with at least 8 px between them; touch density is `comfortable` by default.
- **Precision.** While a finger places a point, a loupe shows the area under the contact above it,
  the snap marker is acquired continuously, and the point is committed on release; an offset-cursor
  mode moves the pending point by relative drag, and nudge buttons move it one snap step.
- **No hover dependence.** Every hover-only feature has a touch equivalent: pre-selection while the
  contact is down and before it lifts, tooltips on long press, and the status strip for snap names.
- **Single-pointer alternatives.** Every multi-finger gesture has a button or menu command, and no
  gesture requires a path or a timing the user cannot adjust.

### Size classes and layouts

The layout follows the window's size class, not the device type, and changes live when the window
is resized, rotated or split.

| Class | Width in logical pixels | Layout |
|---|---|---|
| Compact | Below 600 | The viewport fills the window. Panels open one at a time as bottom sheets or full pages; the toolbar sits at the bottom; the command line is a button that opens the palette and a soft input row; dialogs are sheets |
| Medium | 600 to 1,024 | The viewport plus one docked panel group that the user switches between tabs; floating palettes and the tool option bar in a bottom strip |
| Expanded | Above 1,024 | The full docking workspace as on the desktop |

- A workspace stores its dock layout separately for each class; a new class starts from the
  product's default for that class.
- Orientation and size changes keep the camera, selection, active command and prompt.
- Safe areas (notches, system bars, the home indicator) are respected by every panel and by the
  viewport overlay.
- UI scale composes with the platform's text scale setting; layouts are tested at 200 % combined.
- Long menus become scrollable sheets; context menus open beside the contact and never under the
  finger.

### Text entry

- An on-screen keyboard appears for text and quantity fields and shifts the layout so the focused
  field and the active prompt stay visible. Quantity fields request a numeric keypad with an
  accessory row for units and operators; the command line shows an accessory row with Enter,
  Escape, Tab, and the separators used in coordinate input.
- Input method composition is preserved in every text field. Hardware keyboards work in full,
  including every key binding.
- Escape and Enter are always available as buttons in the tool option bar, so touch-only users can
  cancel and accept.

### Storage and file access

The shell reaches storage only through a file-access layer with one implementation per platform.
Documents are packages (or the product's native file) read and written as whole files through it.

| Platform | User files | Private storage |
|---|---|---|
| Desktop | Native dialogs and paths | Configuration and cache directories |
| Browser | A file picker with a retained file handle where the browser supports it, otherwise upload and download; drag and drop | The origin-private file system for the recovery journal, settings, thumbnails and caches |
| Tablet and phone | The application sandbox and the system document picker, with access retained across launches through persisted permissions (security-scoped bookmarks or storage access framework grants); the share sheet for export and open-in | The application sandbox |

- **Saving.** Platforms without in-place file access use the full-rewrite save strategy of the
  [document package](document-package.md): a complete package is written to a temporary location
  and replaces the target through the provider; in the browser the package is an in-memory image
  persisted by this layer. A location behind a provider that cannot guarantee atomic replacement is
  flagged unreliable and warns on save.
- **Recovery.** The recovery journal lives in private storage, is flushed on every suspend or page
  hide notification, and is offered at the next start like on the desktop.
- **Locks.** Where a lock file cannot be created beside the document, the lock is advisory inside
  the application (the Web Locks API between browser tabs); a Hub lock governs a checked-out
  document.
- **Quota and eviction.** The shell reads the storage quota, shows usage, asks for persistent
  storage through an explicit user action, evicts caches first and never evicts documents or
  recovery data without a prompt.
- **Memory pressure.** On a memory warning the shell releases caches, lowers levels of detail, and
  then asks the user to save and close secondary views; it never ends a document silently.
- **Offline.** The browser build is installable and runs offline from cached assets; Hub
  synchronization resumes when a connection returns. No request is made without a user-enabled
  feature (CORE-APP-019).

### Accessibility and tests

- Screen readers use AccessKit platform adapters on mobile; the browser exposes the same tree
  through a generated accessible mirror. Names, roles and focus order are the ones of the desktop
  tests.
- System settings for text size, bold text, contrast and reduced motion are followed; switch and
  keyboard navigation reach every control.
- `egui_kittest` tests drive simulated touch and pen event streams; gesture recognizers have unit
  tests over recorded streams; panel snapshots are taken in each size class at 100 % and 200 %
  scale. The nightly job builds the browser target and runs it in Chromium, Firefox and WebKit, and
  runs the mobile targets in the iOS simulator and the Android emulator.
