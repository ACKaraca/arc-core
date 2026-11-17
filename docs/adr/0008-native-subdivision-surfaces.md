# 0008. Native Catmull–Clark subdivision following published rules

- Status: Accepted

## Context

MeshBuilder, SolidBuilder (freeform modeling), RenderBuilder (displacement and subdivision at render
time) and the viewport all need subdivision surfaces, and interchange through USD and glTF requires
limit surfaces that match other tools. The common C++ implementation adds a large native build and
a binding to maintain.

## Decision

- `arc-mesh` implements Catmull–Clark subdivision in Rust on the half-edge mesh: uniform refinement,
  limit-position and limit-normal evaluation, sharp and semi-sharp creases with fractional
  sharpness, corner vertices, boundary interpolation modes (none, edges only, edges and corners) and
  face-varying interpolation of UVs, all following the rules published in the OpenSubdiv
  specification.
- Adaptive refinement around extraordinary vertices and GPU evaluation tables are later additions
  behind the same API.
- Conformance is tested against reference outputs produced by the OpenSubdiv reference tools run out
  of process in CI on a set of test meshes; results must agree within 1e-6 relative to the mesh
  bounding box.

## Consequences

- No C++ build dependency for subdivision; works in the WebAssembly set.
- The project maintains conformance with the published rules; the test set must grow with each
  supported rule.
- Performance of adaptive GPU evaluation arrives later than with an existing library.

## Alternatives considered

- **Binding the C++ library.** Mature and fast, but adds a native build, an FFI crate and a license
  with modifications to review, for logic that is compact to implement.
- **Loop subdivision for triangles only.** Insufficient for quad-dominant modeling and interchange.

## Reopen when

Performance targets for animated subdivision in MeshBuilder cannot be met with the native
implementation after GPU evaluation is added.
