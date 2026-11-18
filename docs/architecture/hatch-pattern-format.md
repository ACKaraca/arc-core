# Hatch-pattern format

The file format of Arc hatch patterns (CORE-DRAFT-020): `arc-hatch-patterns`, version 1. The
decision for an Arc-owned, versioned text format is recorded in
[ADR-0016](../adr/0016-hatch-pattern-format.md). The reader and writer are
`arc_draft::hatch_pattern` ([arc-draft](../crates/arc-draft.md#hatches-as-implemented)).

A pattern is a list of line families. Each family is a set of parallel lines with an optional dash
list; the patterns of a file are independent of each other.

## File

A pattern file is UTF-8 text made of lines separated by line feed (U+000A); a carriage return
before a line feed is ignored. Tokens on a line are separated by spaces or tabs. Leading and
trailing spaces and tabs are ignored. After the first line, an empty line or a line whose first
character is `#` is a comment.

The first line is the magic and the version:

```
arc-hatch-patterns 1
```

An optional `license` line follows, then pattern blocks. A `license` line after the first pattern
is an error, as is a second one.

## Numbers

A **number** is an optional `-`, 1 to 6 decimal digits, and optionally a `.` followed by 1 to 6
decimal digits: `12`, `-0.75`, `5.656854`. Exponents, a leading `+`, a leading or trailing point,
`inf` and `nan` are not numbers. A number is read as the nearest binary64 value; `-0` reads as 0.

## Pattern blocks

```
pattern <name>
description <text>
line <angle> <x> <y> <shift> <spacing> [<dash> ...]
end
```

- `pattern` starts a block. The **name** is lowercase ASCII letters, digits and hyphens, starts
  with a letter and has at most 64 bytes. A name occurs once in a file.
- `description` is optional, occurs at most once in a block, and is the rest of the line: 1 to 256
  bytes without control characters.
- `line` defines one family; a block holds 1 to 64 of them, in drawing order.
- `end` closes the block. A block that is still open at the end of the file is an error.

`license` takes the same kind of text as `description`.

## Line families

Units are pattern units; the hatch's scale turns them into drawing units. Let `u` be the unit
vector at `angle` degrees counterclockwise from the positive x axis and `n` be `u` turned a quarter
turn counterclockwise. Line `k` of the family, for every integer `k`, passes through the point

```
origin + k · (shift · u + spacing · n)
```

where `origin` is `(x, y)`. The dash list starts at that point on every line and runs along `u`;
the list repeats in both directions. Consecutive lines are therefore `|spacing|` apart and are
displaced by `shift` along their direction relative to each other, which staggers the dashes.

| Value | Rule |
|---|---|
| `angle` | At most 360 in magnitude |
| `x`, `y`, `shift` | At most 1,000,000 in magnitude |
| `spacing` | Not zero; at most 1,000,000 in magnitude |
| dash | A positive number is a drawn dash of that length, a negative number is a gap of that length, zero is a dot at that position; at most 32 elements, each at most 1,000,000 in magnitude |

No dash list is a continuous line. A dash list needs a total length (the sum of the magnitudes)
greater than zero.

## Limits

| Bound | Value |
|---|---|
| File | 4 MiB, or `input_bytes` of the limits profile if smaller |
| Line | 1024 bytes |
| Patterns per file | 4096, or `array_length` if smaller |
| Families per pattern | 64 |
| Dash elements per family | 32 |

## Canonical form

The writer emits the magic, the license if any, then the patterns in name order. Each block has
its description if any, its families in order and `end`. Numbers have at most six fraction digits
and no trailing zeros; a zero is written `0`. The reader reads canonical text back to an equal
file. Patterns are held in name order, so a file that lists them in another order reads equal to
its canonical form.

## Errors

| Code | When |
|---|---|
| `draft.pattern.syntax` | A line, keyword, value or block is malformed; the bytes are not UTF-8 (line 0) |
| `draft.pattern.version_unsupported` | The version is not 1 |
| `draft.pattern.duplicate` | A pattern name occurs twice |
| `draft.pattern.line_invalid` | A family breaks a rule of the table above |
| `draft.pattern.empty` | A block has no `line` |
| `draft.pattern.limit_exceeded` | A bound above is exceeded |

The golden vectors of every code are in the tests of `arc_draft::hatch_pattern`.

## Versioning

A change to the grammar or the meaning of a value is a new version. A reader rejects versions and
keywords it does not implement.

## The shipped library

`crates/arc-draft/patterns/arc-patterns.ahp` is the library Arc ships: brick-bond, concrete-mix,
cross-lines, diagonal-lines, earth-fill, gravel-scatter, insulation-zigzag, square-grid,
steel-double and wood-grain. It is an original work of the project, designed and drawn for Arc
from its own geometry, and is distributed under the license of the repository. The concrete,
gravel, earth and wood-grain patterns approximate their materials with short dashes, dots and
differently angled families; none reproduces the pattern set of another product.
