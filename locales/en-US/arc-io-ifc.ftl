# Messages of the errors and loss reasons owned by arc-io-ifc. Identifiers are registered in
# crates/arc-io-ifc/src/error.rs.

io-limit-exceeded = The file exceeds the import limit { $limit }.
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
