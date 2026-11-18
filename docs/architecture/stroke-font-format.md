# Stroke-font format

The file format of Arc single-stroke fonts (CORE-DRAFT-035): `arc-stroke-font`, version 1. The
decision for an Arc-owned, versioned text format is recorded in
[ADR-0015](../adr/0015-stroke-font-format.md). The reader and writer are
`arc_draft::stroke_font` ([arc-draft](../crates/arc-draft.md#text-and-fonts-as-implemented)).

A stroke font draws each character with a pen: a glyph is a list of pen-down paths, straight
polylines and circular arcs, with an advance width. There are no filled outlines.

## File

A font file is UTF-8 text made of lines separated by line feed (U+000A); a carriage return
before a line feed is ignored. Tokens on a line are separated by spaces or tabs. Leading and
trailing spaces and tabs are ignored. After the first line, an empty line or a line whose first
character is `#` is a comment.

The first line is the magic and the version:

```
arc-stroke-font 1
```

Then come header lines, then glyph blocks and kerning lines in any order. A header line after
the first glyph or kerning line is an error.

## Numbers and characters

- A **number** is an optional `-`, 1 to 6 decimal digits, and optionally a `.` followed by 1 to 6
  decimal digits: `12`, `-0.75`, `13.1716`. Exponents, a leading `+`, a leading or trailing point,
  `inf` and `nan` are not numbers. A number is read as the nearest binary64 value.
- A **scalar** is a Unicode scalar value in hexadecimal without prefix, 1 to 6 digits, either
  case: `41` is `A`, `fffd` is the replacement character. Surrogates and values above `10ffff` are
  invalid.

## Coordinates

Coordinates are font units with y up. The origin of a glyph is on the baseline at the left end
of its advance. `units` is the size of the em square and the scale reference of every number:
every coordinate, metric, advance, radius and kerning value lies within 4 × `units` of zero.

## Header

| Key | Value | Required | Rule |
|---|---|---|---|
| `name` | The rest of the line | Yes | No control characters |
| `license` | The rest of the line | No | No control characters |
| `units` | Number | Yes | Greater than 0, at most 16384 |
| `cap-height` | Number | Yes | Greater than 0; text height maps to it |
| `x-height` | Number | Yes | Greater than 0 |
| `ascender` | Number | Yes | At least `cap-height`: the top of every glyph box |
| `descender` | Number | Yes | 0 or less: the bottom of every glyph box |
| `line-spacing` | Number | Yes | Greater than 0: the default distance between baselines |
| `missing` | Scalar | No | A glyph the font defines, drawn for characters it lacks |

Each key appears at most once.

## Glyphs

A glyph block starts with `glyph`, the scalar and the advance width (a number, 0 or more), holds
element lines, and ends with a line `end`:

```
glyph 41 20
l 2 0 10 20 18 0
l 5 7 15 7
end
```

| Element | Tokens | Meaning |
|---|---|---|
| Polyline | `l x1 y1 x2 y2 …` | A pen-down path through 2 to 256 points |
| Arc | `a cx cy r start sweep` | A circular arc: center, radius (greater than 0), start angle and signed sweep in degrees, counterclockwise positive; `start` is at most 360 in magnitude, `sweep` is not 0 and at most 360 in magnitude |

A glyph holds at most 256 elements and may hold none (a space). A scalar has at most one glyph.

**Glyph box.** Every point of every element lies inside the glyph's box: x from 0 to the
advance, y from `descender` to `ascender`. For an arc the reader checks its two ends and every
point where it reaches a horizontal or vertical extreme within its sweep; the check allows 10⁻⁹
of `units` beyond the box for the rounding of those points.

## Kerning

```
kern 41 56 -2
```

A kerning line names two scalars, both with glyphs, and a number added to the advance of the
first when the second follows it in the same run. A pair appears at most once.

## Bounds

The reader rejects a file larger than 16 MiB or than the `input_bytes` of its limits, a line longer
than 4096 bytes, more than 65,536 glyphs or kerning pairs, more than 256 elements in a glyph and
more than 256 points in a polyline. The `array_length` of its limits lowers the four counts.

## Errors

| Code | Arguments | When |
|---|---|---|
| `draft.font.syntax` | `line` (1-based; 0 when the file is not UTF-8) | A line or value does not follow the grammar, a glyph block is not closed, a kerning pair repeats |
| `draft.font.version_unsupported` | `version` | The version on the first line is not 1 |
| `draft.font.header_invalid` | `key` | A header value is missing, repeated, out of range or after the glyphs |
| `draft.font.glyph_duplicate` | `glyph` | A scalar has two glyph blocks |
| `draft.font.glyph_unknown` | `glyph` | `missing` or a kerning pair names a scalar without a glyph |
| `draft.font.glyph_out_of_bounds` | `glyph` | A glyph leaves its box or the coordinate range |
| `draft.font.limit_exceeded` | `limit` (`file_bytes`, `line_bytes`, `glyphs`, `elements`, `points`, `kerning`) | A bound above is exceeded |

## Canonical form

The writer emits the magic line, then `name`, `license` (when present), `units`, `cap-height`,
`x-height`, `ascender`, `descender`, `line-spacing`, `missing` (when present), then the glyphs in
scalar order and the kerning pairs in pair order, each number in its shortest decimal form, one
space between tokens, a line feed after every line and no comments. The reader reads the
canonical form of a font it read back to an equal font.

## Versioning

Any change to the grammar or to the meaning of a line is a new version; a reader rejects versions
it does not implement. A reader of version 1 rejects keywords it does not know.

## Shipped font

*Arc Stroke* (`crates/arc-draft/fonts/arc-stroke.asf`) is an original work of the Arc project,
drawn for this format from straight strokes and circular arcs on a grid of 32 units with a cap
height of 20, and distributed under the license of the `arc-core` repository. It aims at
legible, consistent drafting text, not typographic refinement. Its coverage:

| Block | Coverage |
|---|---|
| Basic Latin (U+0020–U+007E) | Complete |
| Latin-1 Supplement letters (U+00C0–U+00FF, with × and ÷) | Complete |
| Latin-1 Supplement symbols | U+00A0–U+00A3, U+00A5, U+00A6, U+00A8, U+00A9, U+00AB, U+00AC, U+00AF–U+00B5, U+00B7–U+00B9, U+00BB, U+00BF; not U+00A4, U+00A7, U+00AA, U+00AD, U+00AE, U+00B6, U+00BA, U+00BC–U+00BE |
| Turkish letters | Complete: Ç ç Ğ ğ I ı İ i Ö ö Ş ş Ü ü (U+011E, U+011F, U+0130, U+0131, U+015E, U+015F with the Latin-1 letters) |
| Greek capitals and lowercase (U+0391–U+03A9, U+03B1–U+03C9) | Complete, including final sigma; no accented Greek letters |
| Cyrillic (U+0410–U+044F, U+0401, U+0451) | The basic alphabet and Ё ё; no other letters of the block |
| General punctuation and symbols | U+2013, U+2014, U+2018, U+2019, U+201C, U+201D, U+2022, U+2026, U+2032, U+2033 |
| Drafting symbols | Degree U+00B0, plus-minus U+00B1, diameter U+2300, centerline U+2104, angle U+2220, infinity U+221E, almost equal U+2248, less or equal U+2264, greater or equal U+2265 |
| Missing glyph | U+FFFD, a crossed box |

A test of `arc-draft` asserts this list and that every glyph stays inside its box.
