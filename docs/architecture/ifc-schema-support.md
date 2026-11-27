# IFC schema support

The design for complete support of the IFC schemas in `arc-io-ifc`: what the crate covers, the size
of the schemas, the schema-aware layer and conformance checker, round trips of foreign content,
geometry coverage, verification and the order of implementation. The decision for generated schema
tables is [ADR-0018](../adr/0018-ifc-schema-tables-from-express.md) (proposed). What is implemented
is specified in [arc-io.md](../crates/arc-io.md#ifc-reading-as-implemented); this document describes
the target and does not claim that any of it exists.

## Starting point

| Topic | Implemented |
|---|---|
| Schemas read | IFC2X3, IFC4 and IFC4X3, recognized from `FILE_SCHEMA`; no schema knowledge: every entity name is accepted and no attribute count, type, enumeration value, derived position, inverse attribute or rule is checked |
| Schemas written | IFC4 |
| Encodings | STEP physical file (clear text) read and written; IFC-ZIP read |
| Model | Generic instances, records and parameters, indexed by name and type; complex instances kept as several records; no byte spans, no serializer from a read file |
| Typed views | Spatial structure, type assignment, property sets (four property kinds) and quantity sets (six kinds): 26 IFC4 entity types, with attribute positions written by hand |
| Geometry evaluated | 9 representation item kinds (extrusion, triangulated and polygonal face sets, faceted B-rep, face and shell based surface models, polyline, indexed poly curve with line segments, mapped item), 4 profile kinds; openings not subtracted |
| Writer | 16 element kinds, 9 element type kinds, grids, material layer sets and usages, property and quantity sets with a few value kinds; 74 IFC4 entity types and 7 of the 42 relationship types |

Unknown content stays in the generic model and is counted in the loss report, but nothing read can be
written back, so foreign content does not survive a round trip.

## Schema size

Counted from the schemas of the published releases (IFC4 is ADD2 TC1). WHERE rules are counted per
entity or type that declares them.

| Quantity | IFC2X3 TC1 | IFC4 ADD2 TC1 | IFC4X3 ADD2 |
|---|---|---|---|
| Entities (abstract) | 653 (97) | 776 (123) | 876 (133) |
| Defined types | 117 | 130 | 132 |
| Select types | 46 | 60 | 61 |
| Enumeration types (items) | 164 (1310) | 207 (1632) | 243 (2117) |
| Explicit attributes | 1320 | 1491 | 1644 |
| Inverse attributes | 115 | 153 | 165 |
| Derived attributes | 55 | 62 | 60 |
| Entity WHERE rules | 339 | 652 | 752 |
| Type WHERE rules | 24 | 25 | 25 |
| Global rules | 2 | 2 | 2 |
| Functions | 38 | 47 | 48 |
| Concrete representation items | 106 | 111 | 128 |
| Concrete relationships | 45 | 42 | 45 |

IFC4X3 ADD2 adds 116 entities to IFC4 (alignment and linear placement, spirals, facilities and their
parts, earthworks, geotechnics, infrastructure elements, `IfcBuiltElement`, `IfcQuantityNumber`) and
removes 16 (`IfcBuildingElement`, `IfcBuildingElementType`, the `StandardCase` and `ElementedCase`
entities, `IfcDoorStyle`, `IfcWindowStyle`, `IfcProxy`, `IfcPresentationStyleAssignment`).

Coverage at the start, in IFC4: 13.5 % of the entities are named anywhere in the crate, 3.4 % are
decoded by the typed views, 9.5 % are written, 8 % of the representation item kinds are evaluated,
and no attribute, inverse attribute or rule is checked.

## Schema tables

`cargo xtask ifc-schema --schema-dir <dir> [--check]` parses the EXPRESS subset the IFC schemas use
(schema, defined, enumeration and select types including extensible selects, entities with
`ABSTRACT SUPERTYPE OF (ONEOF ...)`, `SUBTYPE OF`, explicit, `DERIVE`, `INVERSE`, `UNIQUE` and
`WHERE` clauses, functions and rules). The source files are read from the given directory and
checked against pinned BLAKE3 digests; the task never downloads and the files are not stored in the
repository (ADR-0018). Output, one file per schema under `crates/arc-io-ifc/src/schema/`:

- entities in name order: name, supertype index, abstract flag, the supertype expression for complex
  instances, the explicit attributes in order (name, type reference, optional flag, aggregate kind,
  bounds and uniqueness), the explicit attributes the entity re-declares as derived, the inverse
  attributes (name, source entity, source attribute, aggregate kind and bounds);
- defined types with their underlying type and WHERE rules, enumerations with their items in order,
  selects with their members;
- in a later step, the WHERE rules, global rules and functions as expression trees.

Lookups are binary searches on upper-case names, so a name from a STEP file is found as written.
`--check` regenerates and fails on any difference; `cargo xtask ci` runs it when the source directory
is configured. A schema revision is a new digest and a reviewed table diff.

## Schema-aware access

`arc_io_ifc::schema` gives the tables of a file's `Schema`, the position of an attribute by name over
the supertype chain, supertype tests and an inverse index built once on demand from the references
of the file. The typed views and the geometry evaluator use resolved positions instead of the
positions written by hand, so the differences between IFC2X3, IFC4 and IFC4X3 come from the tables.

## Conformance checker

`IfcFile::conformance(limits, cancel)` returns findings with stable `io.ifc.conformance_*` codes and,
per rule, a count of passed, failed and not evaluated instances. A rule that is not evaluated is
never counted as passed.

1. **Structure.** The entity exists and is not abstract; the parameter count equals the explicit
   attributes over the supertype chain; `$` appears only for optional attributes and `*` exactly where
   a subtype re-declares the attribute as derived; each value matches its type: the underlying kind of
   a defined type, an item of the enumeration, a member of the select (with the typed parameter that
   names a defined type inside a select), a reference to an instance of the attribute's entity or a
   subtype, an aggregate of the right kind within its bounds and, where required, without repeats; the
   records of a complex instance form a valid combination of the supertype expressions.
2. **Inverse attributes and uniqueness.** The number of instances referring to each instance through
   the source attribute lies within the inverse bounds; `UNIQUE` clauses hold over the file.
3. **Rules.** WHERE rules and global rules evaluated by an interpreter of the generated expression
   trees, with the EXPRESS functions interpreted; recursion and step counts are bounded by the
   caller's limits, and exceeding them makes the rule not evaluated.

The writer builds every record through the tables and is tested to produce files with no finding at
any level. Products use the checker for export self-validation.

## Property and quantity set templates

A second generator reads the published property and quantity set definitions of IFC4 ADD2 TC1 and
IFC4X3 ADD2 into tables: set name, applicable entities and predefined types, property name, property
kind (single, enumerated, bounded, list, table, reference), value type and enumeration items. The
checker reports sets that use a template name with properties of another kind, type or enumeration
item. The writer gains the property and quantity kinds the templates need (integers, reals, texts,
identifiers, enumerations, counts, weights, times and the measure types). The license condition of
ADR-0018 applies to these sources too.

## Round trip of foreign content

- Each instance keeps the byte range of its source. `IfcFile::write_to` writes the header records
  and every unmodified instance byte-identical to the source, and changed or new instances in the
  canonical form of the writer.
- An edit API adds, replaces and removes instances. Existing instance names are kept, new names are
  allocated above the largest one, and a reference to a missing instance is refused.
- Products keep content they do not convert as document foreign data
  ([document-package.md](document-package.md#preserved-foreign-data)): format `ifc`, the schema as version, the `GlobalId` or the
  instance name as source identifier, the entity as record kind and the source bytes of the instance
  and of what it alone references as data, with the original file stored as an asset. An export to the
  same schema writes that data back unchanged.
- `GlobalId` values on export are derived from object identifiers; on import they are recorded with
  the object. Matching on re-import follows the product's identity rules and never uses geometry.

## Writer

The writer gains a schema option, IFC4, IFC4X3 ADD2 or IFC2X3, with the entity and attribute
differences taken from the tables; content a schema cannot carry is reported. Further additions:
IFC-ZIP output (stored entries with the existing CRC-32); element types for stairs, stair flights,
ramps, ramp flights and railings; axis, footprint and bounding box representations; a tessellated
body option for reference exchanges; surface styles and colors; groups and zones; classification
references; space boundaries; covering relationships; map conversion and projected coordinate
reference system. The model view named in `FILE_DESCRIPTION` is written only when the file passes the
rule set of that view.

## Geometry coverage

Every representation item kind is either evaluated or reported as not evaluated and kept. Waves give
the order; `*` marks IFC4X3 entities.

| Group | Entities | Method | Wave |
|---|---|---|---|
| Profiles | All parameterized profiles (I, asymmetric I, L, C, U, T, Z, rectangle hollow, rounded rectangle, circle hollow, ellipse, trapezium), arbitrary open, center line, derived, mirrored, composite, open cross* | Exact outlines, fillets flattened within the tolerance | 1 |
| Curves | `IfcLine`, `IfcCircle`, `IfcEllipse`, `IfcTrimmedCurve`, `IfcCompositeCurve` and its segments, B-spline curves with knots (rational), `IfcOffsetCurve2D`, `IfcOffsetCurve3D`, arcs of `IfcIndexedPolyCurve` | Evaluated with `arc-geom` curves, flattened for meshes | 1 |
| Clipping | `IfcBooleanClippingResult` with `IfcHalfSpaceSolid`, `IfcPolygonalBoundedHalfSpace`, `IfcBoxedHalfSpace` | Planar clipping of polyhedral operands in `arc-geom` | 1 |
| Openings | `IfcRelVoidsElement` | Prism subtraction in `arc-geom` when the opening passes through its host; otherwise wave 2 | 1 |
| Solids and sets | `IfcFacetedBrepWithVoids`, `IfcTriangulatedIrregularNetwork`*, `IfcGeometricSet`, `IfcGeometricCurveSet`, 2D transformation operators | Inner shells, flags, members | 1 |
| Placement | `IfcGridPlacement`, `IfcAxis1Placement`, context world coordinate systems and true north | Composition | 1 |
| Swept solids | Tapered extrusion, revolved area solid (tapered), swept disk solid (polygonal), surface curve, fixed reference and directrix derived* swept area solids | Sweeps with frames along the directrix | 2 |
| CSG | `IfcBlock`, `IfcSphere`, `IfcRightCircularCone`, `IfcRightCircularCylinder`, `IfcRectangularPyramid`, `IfcCsgSolid`, general `IfcBooleanResult` | Primitives tessellated; general booleans by the route chosen under open decisions | 2 |
| Curved B-rep | `IfcAdvancedBrep` (with voids), `IfcAdvancedFace`, `IfcFaceSurface`, `IfcEdgeCurve`, `IfcOrientedEdge`, `IfcEdgeLoop`, `IfcVertexPoint` | Trimmed-surface tessellation in parameter space, planar faces first | 3 |
| Surfaces | `IfcPlane`, cylindrical, spherical and toroidal surfaces, surfaces of linear extrusion and revolution, B-spline surfaces (rational), rectangular trimmed and curve bounded surfaces, `IfcSectionedSurface`* | Evaluated in parameter space and tessellated within the tolerance | 3 |
| Curves on surfaces | `IfcPcurve`, `IfcSurfaceCurve`, `IfcIntersectionCurve`, `IfcSeamCurve`, boundary curves | Through surface evaluation | 3 |
| Alignment | Alignment entities*, `IfcCurveSegment`*, clothoid, sine, cosine and polynomial spirals*, `IfcGradientCurve`*, `IfcSegmentedReferenceCurve`*, `IfcOffsetCurveByDistances`*, `IfcLinearPlacement`*, `IfcAxis2PlacementLinear`*, `IfcPointByDistanceExpression`*, `IfcSectionedSolidHorizontal`* | Numeric evaluation with a stated error; preserved until then | 4 |
| Not meshed | Bounding box (optional box mesh), annotation fill areas, text literals, hatching and tile styles, light sources, styled items | Exposed through typed views of annotation and styles | 1–3 |

Every result states the chordal error it was tessellated with; tolerances come from
`arc_base::Tolerance`. Geometry evaluation of `arc-io-ifc` depends on `arc-geom` (layer 1).

## Other encodings

ifcXML and ifcJSON are not planned: the first has few producers and the second is not a ratified
standard. Both are reconsidered when a consumer requires them. IFC-ZIP writing is part of the writer
work.

## Model views and IDS

Reference View and Design Transfer View are expressed as rule sets over the tables (allowed entities,
representation types and relationships) and checked before a view is named in the header. An IDS 1.0
reader and checker (entity, attribute, classification, property, material and part-of facets with
their restrictions) reads XML through `arc-io-common`; the IDS test files are written by the project.

## Verification

- **Gate.** Table counts equal the [schema size](#schema-size) table; one fixture per conformance
  finding; every file the writer tests produce has no finding; read then write is byte-identical for
  every fixture; property tests and fuzz targets for the parser, the checker, the rule interpreter and
  the geometry evaluator.
- **Oracle, outside the gate.** Scripts under `tools/ifc-oracle/` run IfcOpenShell out of process,
  by hand and nightly, over the files the tests write when `ARC_IFC_DUMP=1` is set and over the
  corpus: schema and rule validation must report no issue; the generated tables must equal the
  oracle's attribute names, order, optionality, bounds, inverse attributes and enumeration items;
  conformance findings are compared rule by rule; per product, volume, surface area and bounding box
  from the oracle's geometry are compared with `IfcFile::geometry` within the linear tolerance plus the
  stated chordal error.
- **Corpus.** Files live in `arc-corpus-ifc` under the suite's
  [corpus admission rules](https://github.com/ACKaraca/arc-suite/blob/main/docs/engineering/clean-room-policy.md#corpus-admission):
  fixtures generated by the project, one per representation item kind, profile, relationship and
  schema, and published test files whose license is on the allowlist, each with its ledger row.
- **Round trips.** Foreign file, read, written without edits: byte-identical. Foreign file imported
  and exported by a product: unmodified instances byte-identical, converted objects equal by
  `GlobalId`, values and geometry within tolerance. Repeated exports of one document: identical bytes.

## Implementation order

Each step is one issue with its tests. Steps 1 to 3 do not depend on each other.

| Step | Content | Depends on |
|---|---|---|
| 1 | EXPRESS parser and schema table generator, count tests | License basis recorded |
| 2 | Oracle scripts and the output dump of the writer tests | — |
| 3 | Product export of the writer kinds that exist (stairs, ramps, railings, curtain walls, grids), in the product repository | — |
| 4 | Instance byte spans, serializer and edit API | — |
| 5 | Schema-aware access; views and geometry on resolved positions | 1 |
| 6 | Conformance level 1 | 1, 5 |
| 7 | Conformance level 2 | 6 |
| 8 | Rules and functions: generation and interpreter | 1, 7 |
| 9 | Property and quantity set templates, checker, writer value kinds | 1 |
| 10 | Typed views for import: materials, openings and fills, element attributes, aggregation, types with their sets, classification, groups, styles, georeferencing | 5 |
| 11 | Geometry wave 1 | 2, 5 |
| 12 | Writer on the tables, schema option, IFC-ZIP output, view rule sets | 6 |
| 13 | Writer model additions | 9, 12 |
| 14 | Geometry wave 2 | 11 and the boolean decision |
| 15 | Geometry wave 3 | 11 |
| 16 | Alignment and linear placement | 11 |
| 17 | IDS reader and checker | 5, 10 |
| 18 | Streaming read (CORE-IO-017) | 5 |

Product steps (import to native elements, export of property and quantity sets for every kind,
self-validation on the conformance checker, round trip with foreign data) follow steps 4, 6, 9, 10,
11 and 13 in the product repositories.

## Open decisions

- The license basis for tables generated from the published schemas and property set definitions
  (ADR-0018).
- The `arc-corpus-ifc` repository and the admission of published test files.
- General solid booleans for import: a mesh boolean in `arc-mesh`, or the B-rep kernel in
  `arc-worker` so that hostile input stays out of process.
- The location of the oracle scripts outside the gate.
