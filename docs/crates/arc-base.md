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
| `Color` | Linear RGBA `f32`; `ColorSource` records where a color comes from (explicit, indexed, named, by layer, by block, material) |
| `Notation` | How a `DisplayFormat` writes a quantity: decimal, architectural feet and inches, degrees-minutes-seconds or surveyor bearing |
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
| CORE-BASE-007 | Unit conversion | Converts between any two units of the same dimension; conversions of a value to a unit and back reproduce the value within four units in the last place (for temperature scales with an offset, of the larger of the value and the offset). | T1 | 0 |
| CORE-BASE-008 | Display formatting | Formats a quantity in display units with configured precision, rounding increment, trailing-zero rule and unit symbol, using the locale's decimal separator. | T1 | 0 |
| CORE-BASE-009 | Architectural length formats | Formats and parses lengths as feet and inches with fractional inches (denominator up to 1/256) and as decimal feet or inches, round-tripping every value at the configured precision. | T2 | 1 |
| CORE-BASE-010 | Angle formats | Formats angles as decimal degrees, degrees-minutes-seconds, radians, gradians and surveyor bearings (quadrant and angle). | T2 | 1 |
| CORE-BASE-011 | Document display units | Holds per-dimension display units as one record that documents store; changing it changes formatting only, never stored values. | T1 | 0 |

### Tolerances, limits and cancellation

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-012 | Tolerance context | Supplies linear, angular and tessellation tolerances with documented defaults (1 µm, 1 nrad, and a 1 mm chord with a 15° turning angle when the caller supplies no tessellation values); functions that compare geometry take a tolerance context and never use literals. | T1 | 0 |
| CORE-BASE-013 | Limits profiles | Defines the `strict`, `default`, `large-models` and `constrained` hostile-input profiles with every limit value named; callers can derive custom profiles that only tighten or explicitly override named limits. | T1 | 0 |
| CORE-BASE-014 | Cancellation tokens | Shares a token between a requester and workers; cancelling is observed by all clones and is irreversible for that token. | T1 | 0 |

### Errors

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-BASE-015 | Error categories and codes | Every error has one of eleven categories and a stable dotted code; a registry test fails if two crates register the same code or a code lacks a message identifier. | T1 | 0 |
| CORE-BASE-016 | Localizable error messages | Errors carry a Fluent message identifier and typed arguments; formatting happens in the caller's locale, and the English catalog contains every registered message. | T1 | 0 |
| CORE-BASE-017 | Error source chains | Errors keep the chain of underlying causes for logs and diagnostics without exposing internal details in user messages. | T1 | 0 |
| CORE-BASE-018 | Machine-readable error form | Serializes an error to JSON with category, code, message identifier, arguments and details for protocols and CLI output. | T1 | 0 |

### Error codes

The codes `arc-base` owns. A code is never reused for a different meaning; messages are in
`locales/en-US/arc-base.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `base.id.invalid` | `invalid_input` | A text is not an identifier in hyphenated or plain hexadecimal form |
| `base.unit.dimension_mismatch` | `invalid_input` | Quantities or units of different dimensions are added, subtracted, compared or converted |
| `base.unit.unknown` | `not_found` | A unit name or symbol is not in the built-in registry |
| `base.display.invalid_increment` | `invalid_input` | A display rounding increment is not a positive finite number |
| `base.display.notation_mismatch` | `invalid_input` | A quantity of another dimension is written as feet and inches (length) or as an angle notation (angle) |
| `base.display.invalid_notation` | `invalid_input` | An inch denominator is not a power of two from 1 to 256, or a seconds precision is above 9 |
| `base.display.value_out_of_range` | `invalid_input` | A value to write is not finite or is too large for the notation |
| `base.display.parse_invalid` | `invalid_input` | Text is not a value in the notation it is parsed as, or is longer than 256 characters |
| `base.cancelled` | `cancelled` | An operation observed a cancelled token |

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

## Notation and color details

### Architectural and angle notations (CORE-BASE-009, CORE-BASE-010)

`DisplayFormat::format_with` and `DisplayFormat::parse_with` take a `Notation`. `DisplayFormat::format`
is the decimal notation and is unchanged.

| Notation | Dimension | Example | Parameter |
|---|---|---|---|
| `Decimal` | any | `1.25 m` | the format's unit, precision, rounding increment and symbol |
| `Architectural { denominator }` | length | `5'-3 1/2"` | a power of two from 1 to 256 |
| `DegreesMinutesSeconds` | angle | `45°30'15"` | the format's precision, the decimals of the seconds (0 to 9) |
| `SurveyorBearing` | angle | `N 45°30'15" E` | the format's precision, as above |

The feet-and-inch and angle notations write their own units, so they ignore the format's unit, its
rounding increment and `show_symbol`; the decimal separator applies to the seconds. Their marks are
`'` and `"` for feet and inches, and `°`, `'` and `"` for angles.

Rules shared by the notations:

- Rounding happens once. Architectural values round to the nearest multiple of `1 / denominator` inch,
  and angles round to the nearest unit of `10^-precision` seconds, with ties away from zero. Carries
  then propagate through integer arithmetic: 11 31/32" written at denominator 32 is `11 31/32"`, and
  11.99" at denominator 32 is `1'-0"`.
- Architectural text writes the fraction in lowest terms. Feet appear only when they are not zero, but
  when they appear the inches are always written (`1'-0"`). Zero is `0"`. A negative value has one
  leading `-`; a value that rounds to zero has no sign.
- Degrees are not padded; minutes and seconds have two digits. Trailing zeros of the seconds are removed
  unless `trailing_zeros` is set, and the decimal point is removed with an empty fraction.
- Surveyor bearings reduce the angle modulo 360 degrees before rounding, reduce the rounded count modulo
  360 degrees again (so a count that rounds to 360 becomes 0), and select the quadrant from that count, so `89.9999999` at precision 0 is `N 90°00'00" E`. The quadrant
  intervals are [0, 90], (90, 180], (180, 270] and (270, 360). The written angle lies in [0, 90].
- Parsing reads the exact value of the text; it does not round. Architectural fractions may have any
  denominator from 1 to 256 and need not be reduced. Feet-and-inch text accepts `5'-3 1/2"`,
  `5' 3 1/2"`, `3 1/2"`, `1/2"` and `5'`. Angle text accepts `45°30'15.5"`, with minutes and seconds
  optional, and `N 45°30'15" E`. Minutes and seconds of 60 or more are rejected. A bearing angle
  outside [0, 90] is rejected. `N 0°00'00" W` reads as 0 degrees.
- `Notation` is a parameter of the two methods, not a field of `DisplayFormat`. The struct keeps its
  layout, so existing literal constructions compile unchanged. Storing a notation in a document needs a
  separate change to the stored display record.

### Indexed palette (CORE-BASE-020)

`INDEXED_PALETTE` holds the 256 entries of the indexed color table as sRGB bytes; `indexed_rgb` reads
an entry. Index 0 is black, which the DXF format uses as its by-block marker; the by-layer marker (256)
is not a palette entry.

`nearest_index` returns the entry nearest to an sRGB color by Euclidean distance in OKLab. Every entry
is a candidate, including index 0, so black maps to 0. When entries are equally near, the lowest index
wins. Consequently an entry maps to itself or to an earlier entry with identical bytes. A format that
cannot write index 0 must handle black before it calls `nearest_index`.

### Color sources (CORE-BASE-021)

`ColorSource` records where a color comes from without resolving it:

| Variant | Meaning | `direct_color` |
|---|---|---|
| `Explicit(Color)` | A color given directly | the color |
| `Indexed(u8)` | An indexed palette entry | the palette color |
| `Named(String)` | A name from a document color table | none |
| `ByLayer` | The color of the object's layer | none |
| `ByBlock` | The color of the inserting block reference | none |
| `Material(ObjectId)` | The color of a material object | none |

Serialized variants use snake_case tags, for example `"by_layer"` and `{"indexed":3}`.

The stored document value in `arc-doc` has only `Indexed` and `Named`, and its encoding is part of the
stored payload. `arc-base` cannot depend on `arc-doc`, so the two types are not converted in this crate.
Adopting `ColorSource` in documents needs a payload version change, with fixtures and an ADR, because
it adds variants to a stored encoding. `arc-draft` resolves layer and block colors through its own
appearance settings; it keeps those settings in place until the document encoding changes.
