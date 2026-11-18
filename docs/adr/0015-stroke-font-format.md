# 0015. Versioned text format for Arc stroke fonts

- Status: Accepted
- Issue: [arc-core#70](https://github.com/ACKaraca/arc-core/issues/70)

## Context

[ADR-0009](0009-text-stack-and-drafting-fonts.md) decided that Arc defines its own single-stroke
font format and ships original stroke fonts. The format is a long-lived contract: products ship
fonts in it, users add their own, and drawings refer to fonts by name. It must be readable and
writable from its documentation alone, small enough to edit by hand, exact (the same file gives
the same glyphs on every platform) and safe against hostile files.

## Decision

- `arc-draft` reads and writes `arc-stroke-font` version 1, a line-oriented UTF-8 text format
  specified in [stroke-font-format.md](../architecture/stroke-font-format.md): a magic line with
  the version, header lines (name, license, units per em, cap height, x-height, ascender,
  descender, line spacing, missing glyph), glyph blocks keyed by Unicode scalar with an advance
  width and pen-down polylines and circular arcs, and kerning pairs.
- Numbers are short decimals (at most 6 digits before and after the point) read as the nearest
  binary64 value; there are no exponents, infinities or NaNs. Every value lies within four em of
  zero, and every glyph inside its box (x from 0 to its advance, y from the descender to the
  ascender), which the reader checks.
- The reader works on a byte slice under an `arc_base::Limits` profile and fixed bounds on file
  size, line length, glyph, element, point and kerning counts; every failure is one of seven
  `draft.font.*` codes, and no input panics.
- The writer emits one canonical form, which the reader reads back to an equal font. Golden
  vectors in the test suite fix the canonical text and the error of each rule.
- Any change to the grammar or meaning is a new version; a reader rejects versions and keywords
  it does not implement.
- The shipped font *Arc Stroke* is an original work of the project in this format, under the
  repository's license.

## Consequences

- Fonts can be written and reviewed with a text editor and diffed line by line.
- A text file is larger and slower to read than a binary one; the shipped font has about 1,500
  lines and is parsed once per process.
- Glyph shapes are limited to straight strokes and circular arcs. Elliptical strokes come only
  from scaling a whole glyph (width factor) or slanting it.
- Extending the format (composite glyphs, curves other than arcs) needs a new version, a reader
  for it and new golden vectors.

## Alternatives considered

- **A binary format.** Smaller and faster to read, but needs tooling to inspect and edit, and a
  documented byte layout gains nothing for files of this size.
- **Reading the stroke-font formats of other drafting tools.** Their fonts cannot be
  redistributed, and an Arc format must not depend on another product's format. Import of such
  files, if ever needed, belongs in a product mapper.
- **Outline fonts with a stroke rendering.** Outline glyphs have two edges per stroke; drawing
  them as single strokes is not possible from their data.

## Reopen when

A third-party tool cannot write fonts from the documentation, or font files become large enough
that reading them is a measured cost.
