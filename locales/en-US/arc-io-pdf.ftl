# Messages of the errors and loss reasons owned by arc-io-pdf. Identifiers are registered in
# crates/arc-io-pdf/src/error.rs.

io-pdf-limit-exceeded = The output exceeds the limit { $limit }.
io-pdf-option-invalid = The option { $option } is out of range or malformed.
io-pdf-stream-invalid = The plot stream cannot be written: { $what } is out of range.
io-pdf-loss-items-written = { $count } plot items were written without change.
io-pdf-loss-glyphs-dropped = { $count } outline text runs were not written; only stroke text is supported.
io-pdf-loss-nurbs-flattened = { $count } spline curves were written as line segments within { $tolerance_mm } mm.
io-pdf-loss-arcs-approximated = { $count } arcs were written as Bezier curves within { $tolerance_mm } mm.
io-pdf-loss-polyline-width-approximated = { $count } polylines with varying width were drawn with one width.
io-pdf-loss-items-failed = { $count } plot items could not be written.
io-pdf-loss-sources-failed = { $count } entities were already left out of the plot because they could not be plotted.
io-pdf-loss-layers-dropped = { $count } layers were not written because SVG has no layers.
