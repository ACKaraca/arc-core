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
