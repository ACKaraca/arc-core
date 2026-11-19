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
