# 0016. Versioned text format for Arc hatch patterns

- Status: Accepted
- Issue: [arc-core#77](https://github.com/ACKaraca/arc-core/issues/77)

## Context

Hatch patterns are data that products ship, users add and drawings refer to by name. The format is a
long-lived contract in the same way as the stroke-font format
([ADR-0015](0015-stroke-font-format.md)): it must be readable and writable from its
documentation alone, small enough to edit by hand, exact on every platform and safe against hostile
files. The specification names an Arc hatch-pattern library and a documented file format as one
capability (CORE-DRAFT-020). The pattern library of another product is not available to Arc and is
not a source for it.

## Decision

- `arc-draft` reads and writes `arc-hatch-patterns` version 1, a line-oriented UTF-8 text format
  specified in [hatch-pattern-format.md](../architecture/hatch-pattern-format.md): a magic line with
  the version, an optional license line and blocks of named patterns. A pattern is a list of line
  families; each family gives an angle, an origin, the shift from one line to the next as a pair
  (along the line, across it) and an optional dash list with drawn dashes, gaps and dots.
- Numbers are short decimals (at most 6 digits before and after the point); there are no
  exponents, infinities or NaNs. Every value lies within one million of zero.
- The reader works on a byte slice under an `arc_base::Limits` profile and fixed bounds on file
  size, line length and the counts of patterns, families and dashes; every failure is one of six
  `draft.pattern.*` codes, and no input panics.
- The writer emits one canonical form, which the reader reads back to an equal file. Golden vectors
  in the test suite fix the canonical text and the error of each rule.
- Any change to the grammar or meaning is a new version; a reader rejects versions and keywords it
  does not implement.
- The shipped library is an original work of the project in this format, under the repository's
  license.

## Consequences

- Patterns can be written and reviewed with a text editor and diffed line by line.
- A pattern is lines and dashes only; it has no filled shapes, curves or embedded symbols. Material
  patterns are approximations built from dashes and dots. Extending the format needs a new version.
- Line generation is a function of the pattern, a scale, an angle and an origin; the same file gives
  the same lines on every platform up to the platform's trigonometric functions.

## Alternatives considered

- **Reading the pattern formats of other drafting tools.** Their libraries cannot be redistributed,
  and an Arc format must not depend on another product's format. Import, if it is ever needed,
  belongs in a product mapper.
- **A binary format.** Smaller, but a pattern file is a few kilobytes and a text file can be
  inspected and edited without tools.
- **Storing patterns as JSON.** The document store already holds patterns as objects; a hand-edited
  library file gains little from nesting and loses the one-family-per-line form.

## Reopen when

Users need pattern elements other than lines, dots and dashes, or a third-party tool cannot write
patterns from the documentation.
