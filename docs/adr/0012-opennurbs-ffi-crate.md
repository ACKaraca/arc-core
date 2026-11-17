# 0012. Rhino 3DM reading through an openNURBS FFI crate in the worker

- Status: Accepted
- Issue: Exchange connectors and licensed bridges (suite decision
  [0017](https://github.com/ACKaraca/arc-suite/blob/main/docs/adr/0017-exchange-connectors-and-licensed-bridges.md))

## Context

A permissively licensed open-source library, openNURBS, reads the 3DM format. No pure-Rust reader
of comparable coverage exists. The suite rules allow `unsafe` only in FFI crates named in an
architecture decision, and require hostile files to be parsed with bounded behavior.

## Decision

- One FFI crate, `arc-opennurbs`, wraps openNURBS behind a narrow safe API that returns the neutral
  model of `arc-io-3dm`. It follows the FFI rules of the overview: `SAFETY:` comments, address
  sanitizer tests, and conversion of every foreign failure into an `arc-base` error.
- 3DM import always runs in `arc-worker`.
- The crate is read-only: no 3DM writing is provided.
- The openNURBS license must be added to the allowlist in the suite dependency policy before the
  crate is first built; until then the crate is documented only.
- A pure-Rust reader that passes the 3DM corpus with the same results replaces the FFI crate.

## Consequences

- One more native dependency to build per platform and to keep out of the browser and mobile
  builds, where 3DM import runs on a server-side worker.
- Parsing faults end the worker, not the application.

## Alternatives considered

- **Pure-Rust reader first.** Takes longer to reach coverage of surface and extrusion types.
- **In-process FFI.** Rejected: untrusted native parsing belongs in the worker.
- **Exchange through a connector only.** Needs the other application installed; the open reader
  serves users without it.

## Reopen when

A pure-Rust reader passes the corpus, or the library's license changes.
