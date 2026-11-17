# AGENTS.md

Instructions for automated coding agents working in `arc-core`.

## What this repository is

`arc-core` is the shared Rust foundation of the Arc suite: layered crates for geometry, the document
store and package, commands and automation, drafting, rendering, the UI kit and interchange. Every
Arc product depends on one tagged release of it; it contains no product functionality.

## Status and where to start reading

Foundation phase: the workspace, `xtask`, `arc-base` and the first parts of `arc-geom`, `arc-mesh`, `arc-solve` and `arc-doc`
exist; every other crate is still a specification. Read, in order: [README.md](README.md),
[docs/architecture/overview.md](docs/architecture/overview.md), the architecture document for your
area, and the crate specification you are changing under [docs/crates](docs/crates/README.md).
Capability identifiers (`CORE-<AREA>-NNN`) are defined in the crate specifications.

## Repository layout (target)

```
crates/          one directory per crate (arc-base ... arc-app) plus FFI crates
apps/            arc-shell, arc-shell-cli, arc-worker
clients/python/  official automation client
schemas/         JSON Schemas (protocol, scene, package descriptors, loss reports)
wit/             plug-in interface
locales/         Fluent catalogs
fixtures/        generated test inputs and package compatibility fixtures
xtask/           layers, wasm, ci, corpus, golden, bench tasks
docs/            architecture, crates, testing, roadmap, adr, glossary
```

## Commands

The toolchain is pinned in `rust-toolchain.toml`; the gate also needs `cargo-nextest`,
`cargo-deny` and `cargo-audit`.

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo nextest run --workspace` (`cargo test --workspace --doc` for documentation tests)
- `cargo xtask layers`
- `cargo xtask wasm`
- `cargo xtask ci` runs all gates, including the above, `cargo deny check` and `cargo audit`. CI runs it on
  Linux, Windows and macOS on every push to `main`.

## Suite rules

These rules apply in every Arc repository. The canonical text is
[`docs/engineering/agent-rules.md`](https://github.com/ACKaraca/arc-suite/blob/main/docs/engineering/agent-rules.md)
in `arc-suite`; change it there first.

1. **A person is accountable.** Whoever runs an agent answers for what it submits.
2. **Start from an issue.** Search issues and pull requests, check that nobody else is assigned,
   assign yourself and set the status to *In Progress* before changing files. When you finish,
   comment with what changed, the commit or pull request link and open items, then update the status.
   *Done* requires met and verified acceptance criteria.
3. **Verify, do not guess.** Open a definition before using it. Confirm a cause in the code.
4. **The document is the authority and everything is a command.** No change reaches a document
   except through a validated command inside a transaction. Programmatic paths never open dialogs.
5. **Respect the layers.** A crate depends only on lower layers. Nothing below the UI layer depends
   on windowing, GPU or UI crates. Products never depend on other products.
6. **Never crash, never lose work.** No `unwrap`, `expect`, `panic!`, `todo!` or `unreachable!` in
   production code. No `unsafe` outside the FFI crates named in an ADR. Treat every file and message
   as hostile: bound recursion, allocation and counts. A crash fix ships with a regression test.
7. **Clean room.** Implement from published specifications, observed behavior and original
   reasoning. Never copy or consult another product's code, decompiled output, file-format reverse
   engineering, documentation text, icons, fonts, templates or sample files. Do not name our
   features after another company's features.
8. **Licenses.** Add a dependency only if its license is on the allowlist in
   `docs/engineering/dependency-policy.md` in `arc-suite`. Record every asset's origin and license.
9. **Rust only in desktop applications.** No web view, no JavaScript or TypeScript.
10. **Small, tested changes.** One concern per pull request, with the test that fails without it.
    Update the documents that describe the behavior you change in the same pull request.
11. **Report only what ran.** Say what you tested and what you could not run.
12. **Ask a human before** adding a license outside the allowlist, publishing or releasing,
    changing a file format or public protocol, adding a new repository or remote, spending money,
    handling secrets, or any destructive git operation. Otherwise keep working.
13. **Keep secrets and personal data out** of repositories, logs and requests.
14. **Commit directly to `main` for now.** Until the project has a second maintainer and this
    repository has reached its MVP release (for `arc-suite` and `arc-core`: the first product MVP),
    work on `main`: no feature branches and no pull requests. Wherever these rules or the documents
    say "pull request", read "commit". Run the checks before every push and name the issue in the
    commit message. This rule is temporary and is deleted from a repository when that point is
    reached.

## Repository invariants

- The layer table in `xtask/layers.toml` is the authority; L0–L4 never depend on `winit`, `wgpu`,
  `egui` or other UI crates. Changing the table needs an ADR.
- `unsafe` exists only in `arc-brep-occt`, `arc-pdfium` and `arc-opennurbs`. STEP, IGES, PDFium and
  3DM work runs in `arc-worker`; licensed bridges run only as separate processes the user enabled.
- Objects are addressed by `ObjectId` (UUIDv7) everywhere outside one snapshot; handles are never
  persisted or sent.
- Every document change is a transaction from a command; journal records are field-level and
  self-inverting; derived data is never journaled.
- A regeneration result is applied only if none of its inputs changed after the revision it read.
- The package format, the exchange package schema, the automation protocol, the Arc Scene contract
  and the plug-in interface are versioned independently of crates; changing any of them needs an ADR and fixtures.
- Format crates expose neutral models; product mapping never enters `arc-io-*`. Format crates read byte
  buffers and streams, not paths, and the WebAssembly set builds for the browser.
- Tolerances come from `arc-base`, never from literals. User-visible strings are Fluent message
  identifiers.
- Error codes are stable and never reused.

## Documentation rules

- No mention of planning material, internal research or other companies' products except as file
  formats, standards, libraries or factual interoperability statements. No phrasing that derives
  or compares Arc features from or to another product; never name a feature after another
  product's feature.
- American English, declarative, no marketing language, no claims that something is implemented
  while the repository is in the design phase.
- Use the suite's own vocabulary; suite-wide rules live in `arc-suite` and are linked, not copied.
- Each fact is stated once; link to it elsewhere.
- Write behavior descriptions in original words; do not reproduce other products' documentation.

## Issue workflow

Work starts from an issue labeled with type, phase and `area: <crate>`. Statuses: Backlog, Ready,
In Progress, In Review, Blocked, Done. Search for duplicates, assign yourself, set *In Progress*;
on finishing, comment with changes, link and open items, update the status, and open follow-up
issues for discovered work. Commits use Conventional Commits with a DCO sign-off.

## Verification

Run the narrowest relevant check first (the crate's tests, then `cargo xtask layers` if
dependencies changed), broaden only when needed, and report exactly what ran and what could not.

## Ask a human about

Layer table or FFI crate changes, package format or protocol changes, new third-party
dependencies, kernel version upgrades, changes to the compatibility promise, and anything in the
suite rules' ask-first list.
