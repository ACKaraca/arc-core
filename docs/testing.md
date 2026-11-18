# Testing

The kinds of tests `arc-core` uses, what each protects, what CI runs, and what a pull request must
show. Products follow the same conventions for their own crates.

## Test kinds

| Kind | Tooling | Where | Protects |
|---|---|---|---|
| Unit | `cargo nextest` | In-crate `#[cfg(test)]` modules | Functions and invariants |
| Integration | `cargo nextest` | `crates/<crate>/tests/`, one file per feature or defect | Public API behavior |
| Property | `proptest` | Beside unit tests; regression seeds committed | Algebraic invariants, round trips, model-based sequences |
| Fuzz | `cargo-fuzz` with `arbitrary` | `crates/<crate>/fuzz/` | Every decoder and reader against hostile input |
| Snapshot | `insta` | Integration tests | Canonical JSON dumps, loss reports, comparisons, catalogs, error codes |
| UI snapshot | `egui_kittest` | `arc-ui`, `arc-app` tests | Widget layout, AccessKit tree, keyboard traversal, scaling and locales |
| Image comparison | `arc-gpu` headless harness | `arc-gpu`, `arc-draft`, `arc-view` tests | Rendering and plot output |
| Round-trip corpus | `cargo xtask corpus` | Corpus repositories | Interchange fidelity against oracles |
| Golden models | `cargo xtask golden` | Golden-model repository | B-rep kernel results and naming survival |
| Package compatibility | Integration tests | `fixtures/packages/` | The compatibility promise |
| Benchmarks | `criterion` | `crates/<crate>/benches/` | Performance budgets |
| No-panic and panic hunt | Integration tests | `arc-command`, every `arc-io-*` | The never-crash rule |

### Unit and integration tests

Every public function with logic has tests. Integration tests are named after the behavior they
check (`undo_restores_selection.rs`, `dxf_bulge_polyline_roundtrip.rs`). A defect fix adds a test
that fails without the fix, named after the defect.

### Property tests

Required for: geometry operations (intersection symmetry, boolean identities, offset distance),
quantity arithmetic, encodings (decode of encode is identity), journal inversion, comparison
(comparison of a state with itself is empty), the solver (branch stability under perturbation), and
model-based document tests that apply random command sequences, undo and redo against a simple
reference model. Failing seeds are committed as regression cases.

### Fuzz tests

Every reader of untrusted input has a fuzz target: package decoding, journal records, change sets,
geometry and mesh decoders, `ShapeData`, protocol messages, expression parser, every `arc-io-*`
reader, plug-in manifests. Fuzz corpora seed from the test fixtures. The nightly job runs each target
for a fixed time budget; a crash produces a minimized input that becomes a regression test.

### Snapshot tests

Snapshots store canonical, deterministic text: JSON dumps of documents, loss reports, comparison
output, the command catalog and the error-code registry. Changing a snapshot requires reviewing the
diff in the pull request; snapshots are never updated automatically in CI.

### UI snapshot tests

Each widget and panel has `egui_kittest` tests that assert accessible names and roles on every
interactive node, traverse focus with the keyboard, and compare rendered images at 100 % and 200 %
scale in the default locale, the pseudo-locale and the pseudo right-to-left locale. Images render on
the pinned software adapter so results are identical across machines.

### Image comparison

Rendering tests draw reference scenes headless and compare against stored reference images with a
perceptual difference metric and a per-test threshold; differences above threshold fail with a
difference image attached as a CI artifact. Reference images change only in pull requests that
explain the change.

### Round-trip corpus and golden models

The corpus job imports, exports, re-imports and compares every corpus file and runs the format's
independent oracle; the golden-model job runs the B-rep corpus. Both pin repository commits and
produce JSON and Markdown reports kept as artifacts. A regression against the pinned report fails
the job. See [interchange](architecture/interchange.md#corpus-repositories) and
[geometry](architecture/geometry.md#kernel-isolation).

### Performance budgets and benchmarks

Budgets are targets with a stated reason and are checked by benchmarks on reference fixtures
(generated models of stated size kept with the benchmarks):

| Budget | Target | Reason |
|---|---|---|
| Commit of a single-object edit | 2 ms at 1 million objects | Interactive editing must feel immediate in large models |
| Snapshot | Constant time, under 50 µs | Readers take snapshots on every frame |
| Undo of one step | Under 50 ms excluding regeneration | Undo is used constantly |
| Load of a 1 GB package to first viewport frame | 3 s on the reference machine | Opening large projects must not stall work |
| Incremental save of 1,000 changed objects in a 1 GB package | 1 s | Frequent saves must be cheap |
| Viewport orbit | 60 frames per second at 10 million triangles and 2 million segments | Navigation fluency on large building and plant models |
| Pick | 16 ms | Pre-selection follows the pointer |
| Sketch solve | 10 ms for 1,000 constraints after a single dimension change | Dragging must be interactive |

Benchmarks run nightly; a regression of more than 5 % against the rolling baseline opens an issue
automatically, and a regression beyond a budget blocks release.

### No-panic and panic-hunt tests

- Workspace lints deny `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!` and `unreachable!`
  outside tests; the gate fails on any violation.
- **Guard test:** a test command that deliberately unwinds is run through the executor; the test
  asserts the document equals the last committed snapshot and the error is `internal` with a
  diagnostic.
- **Panic hunt:** for every command in the catalog, the test generates adversarial parameter sets
  from its schema (boundary numbers, NaN and infinity where numbers are allowed, empty and huge
  strings, unknown and deleted identifiers, wrong types through the protocol path) and runs them
  against seeded documents. It asserts that every invocation returns a result or an error, no fault
  escapes, and the document passes validation afterward.
- **Hostile input:** every reader runs a malformed-file corpus and asserts errors without panics or
  unbounded allocation.

## What CI runs

The single gate command is **`cargo xtask ci`**. It runs, in order:

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
3. `cargo xtask layers`
4. `cargo xtask wasm`
5. `cargo nextest run --workspace --all-features` (unit, integration, property, snapshot, UI snapshot and image
   tests on the software adapter, package compatibility, guard and panic-hunt tests), then
   `cargo test --workspace --doc` for the documentation tests
6. Feature matrix builds: for each crate that declares features, `cargo check` without default
   features, with default features and with all features except `unstable` and `unstable-*`;
   crates without features are skipped
7. `cargo deny check` (licenses, bans, advisories, sources)
8. `cargo audit` (advisories against the locked dependency versions)
9. Localization catalog check: every message identifier of `locales/en-US/<crate>.ftl` is
   referenced by a `message_id` in that crate's sources and the reverse; duplicates, catalogs
   without a crate and crates with identifiers but no catalog fail
10. Schema check: every `schemas/*.json` parses and has `$schema` and `$id`; without a `schemas/`
    directory the step reports that there is nothing to check. Whether a schema matches the
    implementation is checked by the tests of the crate that owns it.

The same command runs locally and in CI on Windows, macOS and Linux. Nightly jobs add: fuzzing,
the corpus and golden-model runs, benchmarks, address-sanitizer runs of the FFI crates, image tests
on real GPUs, and a release dry run.

## What a pull request must show

- A link to the issue and the capability identifiers it implements or changes.
- The test that fails without the change, for features and fixes alike.
- A green `cargo xtask ci`, or an explanation of which part could not run and why.
- Updated documentation in the same pull request when behavior described in `docs/` changes.
- An ADR when the change touches the layer table, an FFI crate, the package format, a protocol, the
  Arc Scene contract or the plug-in interface.
- For changes to snapshots or reference images, the reviewed diff and the reason.
- For performance-sensitive changes, benchmark results before and after.
