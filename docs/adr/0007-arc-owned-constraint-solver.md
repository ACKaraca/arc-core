# 0007. Arc-owned hybrid constraint solver

- Status: Accepted

## Context

Sketches, families, assemblies, alignments and rigs all rely on geometric constraint solving. Users
judge a solver by stability (no flipping when a dimension changes), speed on large sketches, and
the quality of its diagnostics when a sketch is over- or under-constrained. The proven open solvers
are under licenses that cannot be linked into an Apache-2.0 suite.

## Decision

- `arc-solve` implements a hybrid solver: decomposition of the constraint graph into connected
  components and rigid clusters placed in sequence, with a Levenberg–Marquardt numeric core using
  analytic Jacobians and sparse factorization from `faer` for clusters that cannot be placed
  constructively.
- The previous solution seeds every solve; orientation hints and rejection of unrequested
  orientation flips keep solutions on the user's branch.
- Rank-revealing factorization provides degrees of freedom per entity and identifies minimal
  redundant and conflicting constraint sets, reported by constraint identifier.
- Drags minimize the change from the previous solution with the dragged entity as a soft target.
- The solver sits behind a trait so that a process-isolated alternative backend could be added
  without changing callers.
- A sketch corpus (generated and contributed sketches with expected outcomes) and property tests
  that perturb dimensions and assert branch stability gate every change.

## Consequences

- One solver with one diagnostic vocabulary serves every product.
- Two code paths (constructive and numeric) must both be tested; the corpus covers both.
- The project owns convergence quality; this is a long-term commitment.

## Alternatives considered

- **Purely numeric solving of the whole system.** Simpler, but slower on large sketches, prone to
  branch flips and poor at localizing conflicts.
- **Purely constructive solving.** Fast and stable for decomposable systems, but cannot handle cyclic
  or non-decomposable systems without a numeric fallback anyway.
- **Linking an existing open solver.** License incompatible with in-process use.
- **An out-of-process existing solver.** Too much latency for interactive dragging.

## Reopen when

The corpus shows a class of common sketches that the hybrid solver cannot solve within the time
budget, or a permissively licensed solver with better diagnostics becomes available.
