# Messages of the errors and loss reasons owned by arc-io-ifc. Identifiers are registered in
# crates/arc-io-ifc/src/error.rs.

io-ifc-truncated = The IFC data ends before the file is complete.
io-ifc-syntax = The IFC data is not valid physical file syntax at this position.
io-ifc-structure-invalid = The header, data or schema section of the IFC file is missing or out of order.
io-ifc-text-invalid = A string in the IFC data is not valid text in the physical file encoding.
io-ifc-value-invalid = A number, binary or enumeration value in the IFC data is malformed or out of range.
io-ifc-instance-duplicate = Two instances of the IFC data have the same name.
io-ifc-archive-entry-missing = The IFC archive holds no entry whose name ends in .ifc.
io-ifc-archive-entry-ambiguous = The IFC archive holds more than one entry whose name ends in .ifc.
io-ifc-loss-entity-preserved = { $count } entities of a type that the typed views do not read were kept as generic instances.
io-ifc-loss-reference-unresolved = { $count } references name instances that are not in the file; they were kept as references.
io-ifc-loss-schema-unrecognized = The file names a schema this reader does not know; its entities were kept as generic instances.
io-ifc-loss-value-unexpected = { $count } attribute values of a kind the typed views do not read were dropped.
io-ifc-loss-cycle-cut = { $count } spatial relationships would make an element contain itself or be placed twice; they were cut.
io-ifc-loss-duplicate-placement = { $count } repeated placements of an element, a property or a quantity were read once; the repeats were dropped.
io-ifc-loss-target-missing = { $count } relationship, property or quantity members name an instance that is missing or of a kind the views cannot use; they were not read.
io-ifc-loss-not-mapped = { $count } entities the views understand but do not map (a property or quantity of another kind, a complex property, or an element outside the spatial structure) were kept as generic instances.
io-ifc-loss-section-skipped = { $count } sections other than DATA (ANCHOR, REFERENCE or SIGNATURE) were skipped as a whole.
io-ifc-loss-archive-entry-skipped = { $count } archive entries of resource-fork or metadata data were skipped; they are not the model.
io-ifc-loss-text-latin1 = { $count } bytes of strings that are not UTF-8 were read as ISO 8859-1.
io-ifc-loss-text-control-kept = { $count } raw tab, carriage return or line feed bytes inside strings were kept as they are.
io-ifc-loss-bytes-ignored = { $count } bytes outside the sections were ignored: a byte order mark, or padding after the end of the file.
io-ifc-loss-trailing-bytes = { $count } bytes after the end of the file that are not padding were ignored.
io-ifc-loss-geometry-not-evaluated = { $count } representation items, placements, mappings or loops of a kind the geometry evaluation does not implement, or longer than its point limit, were not evaluated; nothing is emitted for them.
io-ifc-loss-geometry-degenerate = { $count } geometry items were degenerate (zero depth, zero area, zero direction, a hole outside its outer loop or touching another loop, or a coordinate that is not finite) and were skipped.
io-ifc-loss-index-invalid = { $count } geometry items refer to an index outside their point list, or to a value that is not an index, and were skipped.
io-ifc-loss-opening-not-subtracted = { $count } openings were not subtracted from their elements, which are emitted solid.
io-ifc-loss-unit-assumed = The unit assignment holds no usable length unit; meters and radians were assumed.
io-ifc-loss-geometry-approximated = { $count } geometry items were approximated: a circle has fewer segments than its chord tolerance asks for, a face is not planar and was projected onto its mean plane, or an operator axis was replaced by a perpendicular direction.
io-ifc-write-value-invalid = A value of the model cannot be written to IFC: a number that is not finite, a negative quantity, a size that is not positive, a degenerate placement or true north, a NUL character, or a repeated name in a set or among the sets of an element.
io-ifc-write-reference-invalid = An element names a storey or host that is not in the model or of a kind the relationship does not allow, an opening has no host, or a chain of hosts returns to its start.
io-ifc-write-id-duplicate = Two objects of the IFC file would have the same GlobalId.
io-ifc-loss-not-written = { $count } degenerate bodies or empty property or quantity sets were not written.
