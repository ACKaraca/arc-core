# arc-solve

Geometric constraint solving for 2D sketches and 3D rigid-body assemblies, and the unit-aware
expression and parameter engine used across the suite.

## Owns

- Solver entities, constraints, decomposition, the numeric core, degree-of-freedom diagnostics and
  over-constraint reporting.
- Drag solving and branch-stability rules.
- The expression language: grammar, unit literals, functions, parameter scopes and evaluation.
- Quantity parsing and formatting used by numeric input fields.
- The sketch corpus and its runner.

## Stays in products

Sketch objects in documents, the mapping of sketch geometry to solver entities, inference of
constraints while drawing, family and feature parameter tables, and domain functions registered
into the expression engine.

## Key concepts and types

| Concept | Description |
|---|---|
| `System` | Entities, constraints and parameters to solve, with previous solution |
| `Entity` | Point, line, circle, arc, ellipse, spline control polygon (2D); rigid body frame (3D) |
| `Constraint` | Typed relation with identifier, driving or driven flag and value expression |
| `Cluster` | A rigidly solvable subset found by decomposition |
| `SolveResult` | Solved parameters, status per cluster, residuals, iterations |
| `Dof` | Remaining degrees of freedom per entity with free directions |
| `ConflictSet` | Minimal set of redundant or conflicting constraints |
| `Expr` | Parsed expression with source spans |
| `Scope` | Named parameters visible to expressions, with dependency order |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom` |
| Third-party | `faer` |

The expression engine uses `arc-base` only; `arc-geom` and `faer` enter with the constraint solver.

## Test strategy

The sketch corpus with expected solutions and diagnostics; property tests that perturb driving
dimensions and assert branch stability and convergence; Jacobian tests comparing analytic and
finite-difference derivatives for every constraint; fuzzing of the expression parser; dimensional
analysis property tests; benchmarks on large sketches (1,000 and 10,000 constraints).

## Capabilities

### Constraint solving

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-SOLVE-001 | 2D geometric constraints | Supports coincident, point on curve, horizontal, vertical, parallel, perpendicular, tangent with side, concentric, equal, symmetric, midpoint and fixed constraints between 2D entities. | T1 | 2 |
| CORE-SOLVE-002 | Dimensional constraints | Supports distance, horizontal and vertical distance, angle, radius, diameter and arc length constraints whose values are expressions with units. | T1 | 2 |
| CORE-SOLVE-003 | Driven dimensions | Dimensions marked driven report their measured value after each solve and never constrain the system. | T1 | 2 |
| CORE-SOLVE-004 | Graph decomposition | Splits systems into connected components and rigid clusters and solves clusters in dependency order; a change re-solves only affected clusters. | T1 | 2 |
| CORE-SOLVE-005 | Numeric core | Solves non-decomposable clusters with Levenberg–Marquardt on analytic Jacobians and sparse factorization, starting from the previous solution. | T1 | 2 |
| CORE-SOLVE-006 | Branch stability | Rejects converged solutions that reverse an arc direction, loop winding or tangency side the user did not change, and retries from a perturbed start before reporting failure. | T1 | 2 |
| CORE-SOLVE-007 | Degrees-of-freedom report | Reports per entity whether it is fully constrained, under-constrained with its free directions, or over-constrained. | T1 | 2 |
| CORE-SOLVE-008 | Over-constraint diagnosis | Identifies minimal redundant sets and minimal conflicting sets by constraint identifier; a conflicting system leaves the previous solution unchanged. | T1 | 2 |
| CORE-SOLVE-009 | Drag solving | Moves a dragged entity toward the pointer while minimizing change to other entities, at interactive rates for sketches up to 1,000 constraints. | T1 | 2 |
| CORE-SOLVE-010 | Solve budgets | Stops after configured iteration and time budgets and returns `solve.not_converged` with the best residual and the clusters involved. | T1 | 2 |
| CORE-SOLVE-011 | Spline and ellipse constraints | Constrains ellipse axes and spline control points and supports tangency and curvature continuity at spline ends. | T2 | 3 |
| CORE-SOLVE-012 | 3D rigid-body constraints | Solves assemblies of rigid bodies with coincident, concentric, distance, angle, parallel, perpendicular, tangent and fixed mates, reporting remaining degrees of freedom per body. | T1 | 3 |
| CORE-SOLVE-013 | Joint limits | Applies range limits to distances and angles in 3D systems and reports which limit is active. | T2 | 3 |
| CORE-SOLVE-014 | Constraint inference support | Answers whether adding a candidate constraint would over-constrain the current system, so drawing tools can skip conflicting inferences. | T2 | 2 |
| CORE-SOLVE-015 | Sketch corpus | Runs the sketch corpus and fails on any change in solutions beyond tolerance or in diagnostics. | T1 | 2 |

### Expressions and parameters

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-SOLVE-016 | Unit literals | Parses numbers with metric, imperial, feet-inch and angle units, and bare numbers in a context unit supplied by the caller. | T1 | 0 |
| CORE-SOLVE-017 | Dimensional arithmetic | Evaluates arithmetic with dimension checking; dimensionally invalid expressions fail with the offending span. | T1 | 0 |
| CORE-SOLVE-018 | Function library | Provides arithmetic, trigonometric (angle-aware), rounding to increments, minimum, maximum, absolute value, conditionals and string formatting functions. | T1 | 0 |
| CORE-SOLVE-019 | Host functions and tables | Lets hosts register pure functions and lookup tables with typed signatures, callable from expressions. | T2 | 2 |
| CORE-SOLVE-020 | Parameter scopes | Evaluates named parameters in dependency order within a scope; cycles are rejected with the cycle listed; changing one parameter re-evaluates only its dependents. | T1 | 2 |
| CORE-SOLVE-021 | Bounded evaluation | Limits expression depth, list length and evaluation steps; exceeding a limit fails with `solve.expr.limit_exceeded`. | T1 | 0 |
| CORE-SOLVE-022 | Parse and format round trip | Formats quantities in display units such that parsing the output returns the same value at the display precision. | T1 | 0 |
| CORE-SOLVE-023 | Error spans and suggestions | Reports parse and evaluation errors with source spans and, for unknown names, the closest known names. | T2 | 2 |
| CORE-SOLVE-024 | Lists and conditionals | Supports boolean logic, comparisons with tolerance, and bounded list functions (sum, count, map over ranges) in expressions. | T2 | 2 |

### Expression language

The rules of the expression engine as implemented (CORE-SOLVE-016 to CORE-SOLVE-018,
CORE-SOLVE-021, CORE-SOLVE-022). Parameter scopes, lookup tables, typed host signatures, name
suggestions and comparisons with tolerance belong to the later rows and are not implemented.

#### Grammar

```ebnf
expression  = or ;
or          = and , { "||" , and } ;
and         = comparison , { "&&" , comparison } ;
comparison  = sum , [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) , sum ] ;
sum         = product , { ( "+" | "-" ) , product } ;
product     = unary , { ( "*" | "/" ) , unary } ;
unary       = ( "-" | "+" | "!" ) , unary | power ;
power       = primary , [ "^" , unary ] ;
primary     = quantity | text | "true" | "false" | name | call | list
            | "(" , expression , ")" ;
call        = name , "(" , [ arguments ] , ")" ;
list        = "[" , [ arguments ] , "]" ;
arguments   = expression , { separator , expression } ;
separator   = "," | ";" ;

quantity    = feet-inches | number , [ unit ] ;
number      = ( digits , [ point , digits ] | point , digits ) , [ exponent ] ;
exponent    = ( "e" | "E" ) , [ "+" | "-" ] , digits ;
point       = "." | decimal-separator ;
unit        = factor , { ( "*" | "/" | "·" ) , factor } ;
factor      = unit-name , [ "^" , [ "-" ] , digits | digits | "²" | "³" ] ;
feet-inches = number , "'" , [ [ "-" ] , inches ] | inches ;
inches      = number , '"' | digits , " " , fraction , '"' | fraction , '"' ;
fraction    = digits , "/" , digits ;

text        = '"' , { character | "\\" , ( '"' | "\\" | "n" | "t" ) } , '"' ;
name        = name-start , { name-start | digit | "²" | "³" | "·" } ;
name-start  = letter | "_" | "°" | "′" | "″" | "µ" ;
```

Letters and digits are ASCII. White space separates tokens and is otherwise ignored, with these
exceptions, which keep units apart from arithmetic:

- A unit follows its number directly or after white space (`2400mm`, `2400 mm`). The factors,
  operators and exponents of one unit are written without white space: `12 kN/m` is a line load,
  `12 kN / m` divides 12 kN by the name `m`. A name that directly follows a number is always a
  unit, so `2 x` fails with `solve.expr.unknown_unit`.
- Unit operators apply left to right: `kN/m/s` is kN per meter per second.
- `'` and `"` are the foot and inch marks only directly after a number; elsewhere `"` starts a
  text. `8' 6"`, `8'6"` and `8'-6"` are 8 feet 6 inches; the joining hyphen has no white space on
  either side, so `8' - 6"` subtracts. A fraction is written without white space and follows a whole
  number of inches after white space: `8' 6 1/2"`, `1/2"`. `1 / 2"` divides.
- `^` binds tighter than the prefix operators and associates to the right: `-2^2` is −4 and
  `2^3^2` is 2^9. Comparisons do not chain.

`.` is always a decimal separator and `,` separates arguments. `Expr::parse_localized` and
`parse_quantity` accept a second decimal separator supplied by the caller: `,`, or a character
outside ASCII that is not a letter, digit, white space or unit character (otherwise
`solve.expr.separator_invalid`). A separator counts as decimal only between or before digits. When
it is `,`, the comma no longer separates arguments and `;` does: `max(1,5; 2)`. No digit grouping
is accepted.

Numbers are decimal and are converted with correct rounding. A literal that is not finite in SI
units fails with `solve.expr.domain`.

#### Units

A unit name is the symbol or the name of a unit of the `arc-base` registry (`mm`, `m²`, `kg/m³`,
`°C`, `millimeter`), or one of the ASCII aliases `deg`, `arcmin`, `arcsec`, `degC` and `degF`. The
engine defines no units of its own. A unit text that is not one registry unit is resolved factor
by factor: scales multiply, dimensions combine, and a trailing integer is an exponent (`m2`,
`mm^-2`, `kg*m/s^2`). An exponent is an integer from −128 to 127; a dimension exponent outside that
range fails with `solve.expr.limit_exceeded` and a product whose scale leaves the range of `f64`
with `solve.expr.domain`.

Units with an offset (`°C`, `°F`) stand alone and denote absolute temperatures, as in `arc-base`:
`20 °C` is 293.15 K, and `20 °C + 5 °C` adds two absolute temperatures. Inside a product or power
they fail with `solve.expr.offset_unit`; temperature differences are written in `K`. A minus sign
before a number with an offset unit belongs to the reading: `-40 °C` is 233.15 K.

#### Values and bare numbers

A value is a number with a dimension (stored in SI base units), a truth value, a text or a list.
A number written without a unit is a *bare number*:

- Arithmetic among bare numbers stays bare.
- Where a bare number meets a dimensioned number in an addition, subtraction, comparison, `min`,
  `max`, `sum`, `atan2` or a rounding increment, it is read in the context unit if that unit has the
  other operand's dimension; otherwise the operation fails with `solve.expr.dimension_mismatch`.
  Meeting a dimensionless number it is dimensionless.
- In a product, quotient or power a bare number is a dimensionless factor.
- A result that is bare is read in the context unit. `evaluate_as(dimension)` reads it in the
  context unit when the unit has that dimension, as dimensionless when `dimension` is none, and
  fails with `solve.expr.dimension_mismatch` otherwise.

The caller sets the context unit (`Context::bare_unit`), for a numeric field the field's display
unit. Without one, bare numbers are dimensionless everywhere, which is how a caller forbids a bare
number where a dimensioned value is expected. In a millimeter field `1200 + 3 in` is 1.2762 m and
`2 * 600 mm` is 1.2 m.

#### Operators

| Operator | Operands | Result |
|---|---|---|
| `+` `-` | Numbers of equal dimension | That dimension |
| `*` `/` | Numbers | Product or quotient of the dimensions |
| `^` | Number, dimensionless exponent | See below |
| `==` `!=` | Numbers of equal dimension, two truth values or two texts | Truth value |
| `<` `<=` `>` `>=` | Numbers of equal dimension | Truth value |
| `&&` `\|\|` `!` | Truth values; the right operand is evaluated only when needed | Truth value |
| prefix `-` `+` | Number | Same dimension |

Comparisons are exact. A dimensionless base takes any exponent. A dimensioned base takes an
exponent equal to n/d with d from 1 to 4 for which every resulting dimension exponent is an
integer: `(4 m²)^0.5` is 2 m, `(2 m)^0.5` fails with `solve.expr.dimension_mismatch`. Every
arithmetic result must be finite: division by zero, the root of a negative number and overflow fail
with `solve.expr.domain`. Underflow rounds to zero as IEEE 754 specifies.

Errors carry the byte span of the offending sub-expression (parentheses included) in the details
`start` and `end`: for an operator the whole operation, for a function the whole call, for
`evaluate_as` the whole expression.

#### Functions

The built-in set is closed. "Same" means arguments of one dimension under the bare-number rule.

| Function | Arguments | Result |
|---|---|---|
| `abs(x)` | Number | Dimension of `x` |
| `min(a, b, ...)`, `max(a, b, ...)`, `min(list)`, `max(list)` | One or more numbers, same | That dimension; an empty list fails with `solve.expr.domain` |
| `sqrt(x)` | Number whose dimension exponents are even | Halved dimension |
| `pow(x, e)` | As `x ^ e` | As `x ^ e` |
| `sin(a)`, `cos(a)`, `tan(a)` | Angle, or a dimensionless number read as radians | Dimensionless |
| `asin(x)`, `acos(x)`, `atan(x)` | Dimensionless | Angle |
| `atan2(y, x)` | Two numbers, same | Angle |
| `round(x)`, `floor(x)`, `ceil(x)` | Dimensionless | Dimensionless; `round` rounds halves away from zero |
| `round(x, i)`, `floor(x, i)`, `ceil(x, i)` | Two numbers, same; `i` positive | The multiple of `i` nearest to, below or above `x` |
| `if(c, a, b)` | Truth value and two expressions | The value of `a` or `b`; only that one is evaluated |
| `len(v)` | List or text | Dimensionless count of elements or characters |
| `sum(list)` | List of numbers, same | That dimension; the empty sum is a bare 0 |
| `first(list)`, `last(list)` | Non-empty list | The element |
| `format(x, unit)`, `format(x, unit, decimals)` | Number, text naming one registry unit of its dimension, whole number from 0 to 17 (default 3) | Text: `arc-base` display formatting in that unit with `.` as separator and trailing zeros removed |
| `concat(a, b, ...)` | Texts | Text |

A dimensioned value has no rounding without an increment, because the result would depend on a
unit: `round(1234 mm, 50 mm)` is 1.25 m, `round(1234 mm)` fails with
`solve.expr.dimension_mismatch`.

Hosts add pure functions with `Functions::register(name, min_args, max_args, function)`. A name
must be an ASCII identifier that is not a built-in function, `true`, `false` or already registered
(`solve.function.invalid`). A host function receives evaluated values, its argument count is
checked, its result is checked against the limits, and an error it returns is passed on with the
span of the call. Names that are not literals are resolved by the caller's `Names` lookup, a trait
that closures from name to value implement; its results are checked against the limits too. This
lookup is the hook for parameter scopes (CORE-SOLVE-020).

#### Limits

`ExprLimits` bounds every parse and evaluation. Exceeding a limit fails with
`solve.expr.limit_exceeded`, whose detail `limit` names it.

| Limit | Default | Bounds |
|---|---|---|
| `text_bytes` | 65,536 | Length of the input text and of every text value |
| `depth` | 64 | Nesting of parentheses, lists, calls and operators (the depth of the expression tree) and of list values; never above 128 |
| `list_length` | 10,000 | Elements of a list, arguments of a call |
| `steps` | 1,000,000 | Evaluated nodes, list elements visited by a function, and values admitted from names and host functions |
| `dimension_exponent` | −128 to 127 | Every exponent of a dimension |

`ExprLimits::tightened_by` lowers text length, depth and list length to the string length, nesting
depth and array length of an `arc-base` limits profile. A chain of infix operators is as deep as it
is long. An expression is checked against the depth limit of the context again when it is
evaluated. The cancellation token of the context is checked on the first step and every 1,024
steps after it.

#### Determinism

Evaluation has no state, no I/O and no access to anything outside the context. Number parsing,
`+`, `-`, `*`, `/`, integer powers (repeated squaring), the exponent 0.5 and `sqrt`, rounding,
comparisons and formatting give bit-identical results on every platform. `sin`, `cos`, `tan`,
`asin`, `acos`, `atan`, `atan2` and powers with other fractional exponents call the `f64`
functions of the standard library, whose last bit may differ between platforms.

#### Field input round trip (CORE-SOLVE-022)

`arc_base::DisplayFormat::format` writes a quantity; `arc-base` implements the decimal style only,
and this crate adds no other. `parse_quantity(text, context, decimal_separator, dimension)` reads
it back when the context unit is the format's unit and the separator is the one used for
formatting, with or without the unit symbol and for every unit of the registry. The value read
differs from the displayed decimal number only by the rounding of the unit conversion (at most a
few units in the last place), so formatting it again gives the same text whenever the displayed
number has at most 14 digits and a rounding increment, if set, is a multiple of the last displayed
decimal place.

### Error codes

The codes `arc-solve` owns. A code is never reused for a different meaning; messages are in
`locales/en-US/arc-solve.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `solve.expr.syntax` | `invalid_input` | The text is not a well-formed expression |
| `solve.expr.unknown_unit` | `not_found` | A unit suffix or the unit argument of `format` names no registry unit or product of registry units |
| `solve.expr.offset_unit` | `invalid_input` | A unit with an offset is raised to a power or combined with other units |
| `solve.expr.unknown_function` | `not_found` | A call names neither a built-in nor a registered function |
| `solve.expr.unknown_name` | `not_found` | A name is not known to the caller's lookup |
| `solve.expr.dimension_mismatch` | `invalid_input` | Operands or arguments have dimensions the operation cannot combine, or the result does not have the expected dimension |
| `solve.expr.type_mismatch` | `invalid_input` | An operand or argument is a value of the wrong kind |
| `solve.expr.argument_count` | `invalid_input` | A function is called with too few or too many arguments |
| `solve.expr.domain` | `invalid_input` | An argument is outside the domain of the operation or a result is not a finite number |
| `solve.expr.limit_exceeded` | `limit_exceeded` | A limit of the table above is exceeded |
| `solve.expr.separator_invalid` | `invalid_input` | The decimal separator supplied by the caller is not usable |
| `solve.function.invalid` | `invalid_input` | A host function name is not an identifier or is taken, or its argument range is empty |
