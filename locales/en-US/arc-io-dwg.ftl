# Messages of the errors and loss reasons owned by arc-io-dwg. Identifiers are registered in
# crates/arc-io-dwg/src/error.rs.

io-dwg-signature-invalid = The data does not start with a DWG version signature.
io-dwg-version-unsupported = The DWG version { $version } is not supported.
io-dwg-truncated = The DWG data ends before its structures are complete.
io-dwg-structure-invalid = The DWG data is malformed or refers to records that do not exist.
io-dwg-checksum-mismatch = A checksum in the DWG data does not match its contents.
io-dwg-library-failed = The DWG reader stopped unexpectedly; the file was not read.
io-dwg-loss-header-dropped = Header variables other than the ones the drawing keeps were not carried.
io-dwg-loss-table-dropped = { $count } entries of a table the drawing does not keep were not carried.
io-dwg-loss-table-entry-partial = { $count } table entries kept some data the drawing does not carry.
io-dwg-loss-object-dropped = { $count } non-graphical objects were not carried.
io-dwg-loss-entity-dropped = { $count } entities of a type the drawing does not keep, or outside any block, were not carried.
io-dwg-loss-entity-partial = { $count } entities kept some data the drawing does not carry.
io-dwg-loss-value-skipped = { $count } records were skipped because a value is not a finite number.
