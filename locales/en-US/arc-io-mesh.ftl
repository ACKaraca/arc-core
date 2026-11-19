# Messages of the errors and loss reasons owned by arc-io-mesh. Identifiers are registered in
# crates/arc-io-mesh/src/error.rs.

io-limit-exceeded = The file exceeds the import limit { $limit }.
io-mesh-truncated = The mesh data ends unexpectedly.
io-mesh-value-invalid = The mesh data contains a value that is not a finite number of the expected kind.
io-mesh-index-out-of-range = The mesh data refers to a vertex that does not exist.
io-mesh-face-invalid = The mesh data contains a face with fewer than three corners.
io-mesh-structure-invalid = The mesh data is not arranged as the format requires.
io-mesh-header-invalid = The PLY header is malformed or names an unknown format or version.
io-mesh-text-invalid = The mesh data contains text that is not valid UTF-8.
io-mesh-encoding-unsupported = The format cannot be written with the requested encoding.
io-mesh-write-value-invalid = The mesh contains a value or a name that this format cannot write.
io-mesh-loss-element-skipped = { $count } elements of a kind this format does not hold were skipped.
io-mesh-loss-property-skipped = { $count } values of a property this format does not hold were skipped.
io-mesh-loss-polygon-triangulated = { $count } polygons with more than three corners were split into triangles.
io-mesh-loss-attribute-dropped = This format cannot hold the { $attribute } of { $count } items, so it was dropped.
io-mesh-loss-attribute-incomplete = The { $attribute } is missing on { $count } vertices, so it was not kept.
io-mesh-loss-precision-reduced = The { $attribute } of { $count } values was rounded to the precision of this format.
