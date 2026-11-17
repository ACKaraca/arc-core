# arc-base

The L0 vocabulary every other crate and product uses: identifiers, units and quantities,
tolerances, the error model, color and math aliases.

## Owns

- `ObjectId`, `TransactionId`, `DocumentId`, `SessionId` and the identifier generator (random and
  seeded).
- Physical dimensions, quantities in SI base units, unit definitions, unit conversion and display
  unit settings.
- `Tolerance` values and the tolerance context.
- `arc_base::Error`: categories, stable codes, message identifiers and details; conversion traits.
- Color: linear RGBA, sRGB conversion, indexed palette support, color sources.
- Math aliases over `nalgebra` (`f64` points, vectors, transforms) and `glam` (`f32` GPU data), so
  public APIs never expose a third-party math crate directly.
- Coordinate reference records (local origin, survey point, geodetic reference, CRS identifier) as
  plain data.
- Limits profiles (hostile-input limit values) and cancellation tokens.

## Stays in products

Domain units and formats (for example station notation in civil, feet-inch fractions styles beyond
the generic formatter), product-specific error codes (products define their own codes in their own
namespace), and any default display-unit choice for a product's documents.

## Key concepts and types

| Concept | Description |
|---|---|
| `ObjectId` | 128-bit UUIDv7; ordering, parsing and formatting in canonical hyphenated form |
| `Dimension` | Exponents over base dimensions (length, mass, time, current, temperature, amount, luminous intensity, angle, solid angle) |
| `Quantity` | `f64` value in SI base units plus a `Dimension`; arithmetic checks dimensions |
| `Unit` | Name, symbol, dimension, scale and offset to SI; a registry of built-in units |
| `DisplayUnits` | Per-dimension display unit, precision, rounding increment and format style |
| `Tolerance` | Linear, angular, parametric and tessellation tolerances with defaults |
| `Error` | Category, stable code, message identifier, arguments, source chain |
| `Color` | Linear RGBA `f32`; source kind (explicit, indexed, from layer, from block, from material) |
| `CrsRecord` | Local origin, survey point, rotation to grid north, CRS identifier string, vertical datum |
| `Limits` | A named profile of hostile-input limits |
| `Cancel` | Shared cancellation token with a checked `is_cancelled` |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | None |
| Third-party | `uuid`, `nalgebra`, `glam`, `thiserror`, `serde` |

## Test strategy

Unit tests for every conversion and formatting rule; property tests for quantity arithmetic
(dimension algebra, round trips of parse and format, conversion inverses); seeded-generator
determinism tests; a test that every error code is unique and documented. The crate is in the
WebAssembly set.

## Capabilities

### Identifiers

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-001 | UUIDv7 object identifiers | Generates version 7 UUIDs with a millisecond timestamp and random bits; two identifiers generated in the same process are always distinct and compare in generation order when generated in different milliseconds. | T1 | 0 |
| CORE-BASE-002 | Seeded identifier generation | A generator created with a seed produces the same identifier sequence on every run and platform; the timestamp part comes from a logical clock starting at a fixed value. | T1 | 0 |
| CORE-BASE-003 | Identifier text form | Formats identifiers as lowercase hyphenated strings and parses upper- or lowercase, hyphenated or not; any other input is rejected with `base.id.invalid`. | T1 | 0 |
| CORE-BASE-004 | Typed identifier wrappers | Provides distinct types for object, transaction, document and session identifiers that cannot be mixed without an explicit conversion. | T1 | 0 |

### Units and quantities

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-005 | Dimensioned quantities | Stores values in SI base units with a dimension; addition and comparison of different dimensions fail with `base.unit.dimension_mismatch`, multiplication and division combine dimensions. | T1 | 0 |
| CORE-BASE-006 | Built-in unit registry | Provides metric and imperial units for length, area, volume, angle, mass, time, temperature (with offset), force, pressure, velocity, density, energy, power, luminous and illuminance quantities, each with symbol and conversion to SI. | T1 | 0 |
| CORE-BASE-007 | Unit conversion | Converts between any two units of the same dimension; conversions of a value to a unit and back reproduce the value within four units in the last place. | T1 | 0 |
| CORE-BASE-008 | Display formatting | Formats a quantity in display units with configured precision, rounding increment, trailing-zero rule and unit symbol, using the locale's decimal separator. | T1 | 0 |
| CORE-BASE-009 | Architectural length formats | Formats and parses lengths as feet and inches with fractional inches (denominator up to 1/256) and as decimal feet or inches, round-tripping every value at the configured precision. | T2 | 1 |
| CORE-BASE-010 | Angle formats | Formats angles as decimal degrees, degrees-minutes-seconds, radians, gradians and surveyor bearings (quadrant and angle). | T2 | 1 |
| CORE-BASE-011 | Document display units | Holds per-dimension display units as one record that documents store; changing it changes formatting only, never stored values. | T1 | 0 |

### Tolerances, limits and cancellation

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-012 | Tolerance context | Supplies linear, angular and tessellation tolerances with documented defaults; functions that compare geometry take a tolerance context and never use literals. | T1 | 0 |
| CORE-BASE-013 | Limits profiles | Defines the `strict`, `default` and `large-models` hostile-input profiles with every limit value named; callers can derive custom profiles that only tighten or explicitly override named limits. | T1 | 0 |
| CORE-BASE-014 | Cancellation tokens | Shares a token between a requester and workers; cancelling is observed by all clones and is irreversible for that token. | T1 | 0 |

### Errors

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-015 | Error categories and codes | Every error has one of eleven categories and a stable dotted code; a registry test fails if two crates register the same code or a code lacks a message identifier. | T1 | 0 |
| CORE-BASE-016 | Localizable error messages | Errors carry a Fluent message identifier and typed arguments; formatting happens in the caller's locale, and the English catalog contains every registered message. | T1 | 0 |
| CORE-BASE-017 | Error source chains | Errors keep the chain of underlying causes for logs and diagnostics without exposing internal details in user messages. | T1 | 0 |
| CORE-BASE-018 | Machine-readable error form | Serializes an error to JSON with category, code, message identifier, arguments and details for protocols and CLI output. | T1 | 0 |

### Color and math

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-019 | Linear color model | Stores colors as linear RGBA and converts to and from 8-bit sRGB exactly for all 256 levels per channel after rounding. | T1 | 0 |
| CORE-BASE-020 | Indexed color palette | Provides a 256-entry indexed palette and maps any RGB color to the nearest index by perceptual distance for formats that need indices. | T2 | 1 |
| CORE-BASE-021 | Color sources | Represents color by source (explicit, indexed, from layer, from block, from material) so resolution can happen in the consuming model. | T1 | 1 |
| CORE-BASE-022 | Math aliases | Exposes `f64` point, vector, matrix and transform aliases for model math and `f32` aliases for GPU data, with conversions that are explicit at call sites. | T1 | 0 |

### Coordinate reference

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-023 | Coordinate reference record | Stores a local origin, survey point, rotation to grid north, CRS identifier and vertical datum as data every document carries; transformations between local and project coordinates are exact affine operations. | T1 | 0 |
| CORE-BASE-024 | Shared geodetic reference | Two documents with the same CRS record and survey point place each other's objects at identical project coordinates when linked. | T2 | 2 |
