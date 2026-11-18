# Messages of the errors and loss reasons owned by arc-io-dxf. Identifiers are registered in
# crates/arc-io-dxf/src/error.rs.

io-limit-exceeded = The file exceeds the import limit { $limit }.
io-unsupported-version = The format version { $version } is not supported.
io-stream-failed = The data could not be read from its source.
io-dxf-truncated = The DXF data ends unexpectedly.
io-dxf-group-code-invalid = The DXF data contains an invalid group code.
io-dxf-value-invalid = The DXF data contains a value that does not match its group code.
io-dxf-structure-invalid = The sections, tables or records of the DXF data are not arranged correctly.
io-dxf-text-invalid = The DXF data contains text that is not valid UTF-8.
io-dxf-write-value-invalid = The drawing contains a value that cannot be written to DXF.
io-dxf-loss-section-preserved = { $count } unrecognized sections were kept unchanged.
io-dxf-loss-table-preserved = { $count } unrecognized tables were kept unchanged.
io-dxf-loss-record-preserved = { $count } records of an unrecognized type were kept unchanged.
io-dxf-loss-groups-preserved = { $count } unrecognized values of records were kept unchanged.
io-dxf-loss-comment-dropped = { $count } comments were skipped.
io-dxf-loss-code-page-unsupported = The code page { $code_page } is not supported; { $count } characters were kept as raw bytes.
io-dxf-loss-byte-unmapped = { $count } bytes have no character in the code page { $code_page } and were kept as raw bytes.
io-dxf-loss-fit-data-dropped = { $count } hatch spline fit points or tangents were not written because the target version cannot store them.
io-dxf-loss-raw-bytes-written = { $count } raw-byte characters were written as Unicode characters.
io-dxf-loss-handle-assigned = { $count } records received a new handle.
io-dxf-loss-structure-incomplete = The drawing has no block record table, which files of this version require.
