# 0009. Text stack and Arc-owned single-stroke drafting fonts

- Status: Accepted

## Context

Drafting text must look the same on screen, on plots and in exported PDF and SVG, on every
platform. Drawings need single-stroke fonts (fast to plot, legible at small sizes, engraving and
cutting friendly) and outline fonts with full shaping and bidirectional support. Fonts commonly
used in drafting are proprietary and cannot be redistributed.

## Decision

- Text layout and shaping use `parley`; glyph outlines and rasterization use `swash`. The same
  layout result feeds the viewport, the plot pipeline and the UI.
- Arc defines its own single-stroke font format: a documented file with per-glyph stroke paths
  (lines and arcs in font units), advance widths, kerning pairs and metadata. Arc ships original
  single-stroke fonts drawn by the project, covering Latin (including Turkish), Greek, Cyrillic and
  drafting symbols.
- Outline fonts are system or user-supplied OpenType and TrueType fonts; Arc bundles an open-licensed
  outline family for consistent defaults.
- A font provider interface in `arc-draft` lets products register additional font sources. Font
  names in imported drawings that are not available resolve through a substitution table
  (per document and per user), and every substitution is listed in the loss report.
- Plot output embeds subsetted outline fonts in PDF, and draws single-stroke text as vector strokes.

## Consequences

- Consistent text everywhere with no proprietary font dependency.
- The project designs and maintains its own stroke fonts and their glyph coverage.
- Drawings that rely on fonts Arc cannot load display with substitutes until the user supplies the
  fonts; positions are preserved.

## Alternatives considered

- **Platform text APIs.** Inconsistent across platforms and unusable headless.
- **Rendering only outline fonts.** Slower plotting and poorer legibility for drafting.
- **Bundling fonts from other drafting tools.** Not redistributable.

## Reopen when

Users cannot achieve required drawing fidelity through substitution and the provider interface, or
upstream text crates cannot meet shaping requirements for a supported script.
