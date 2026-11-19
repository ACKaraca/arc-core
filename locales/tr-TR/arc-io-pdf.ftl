# arc-io-pdf tarafından sahip olunan hataların ve kayıp nedenlerinin iletileri. Tanımlayıcılar
# crates/arc-io-pdf/src/error.rs içinde kayıtlıdır. Sözlük için arc-base.ftl dosyasına bakın.

io-pdf-option-invalid = { $option } seçeneği aralığın dışında veya hatalı biçimde.
io-pdf-stream-invalid = Çizim akışı yazılamıyor: { $what } aralığın dışında.
io-pdf-loss-items-written = { $count } çizim öğesi değişmeden yazıldı.
io-pdf-loss-glyphs-dropped = { $count } dış çizgili metin dizisi yazılmadı; yalnızca çizgi metni destekleniyor.
io-pdf-loss-nurbs-flattened = { $count } eğri { $tolerance_mm } mm içinde doğru parçalarına dönüştürülerek yazıldı.
io-pdf-loss-arcs-approximated = { $count } yay { $tolerance_mm } mm içinde Bezier eğrileri olarak yazıldı.
io-pdf-loss-polyline-width-approximated = Genişliği değişen { $count } çoklu çizgi tek genişlikle çizildi.
io-pdf-loss-items-failed = { $count } çizim öğesi yazılamadı.
io-pdf-loss-sources-failed = { $count } nesne çizilemediği için çizim akışından zaten çıkarılmıştı.
io-pdf-loss-layers-dropped = SVG katman içermediğinden { $count } katman yazılmadı.
io-pdf-raster-unavailable = PNG çıktısı için raster özelliğiyle derlenmiş bir yapı gerekir.
io-pdf-plot-params-invalid = { $what } çizim parametresi eksik, tutarsız veya aralığın dışında.
io-pdf-loss-items-skipped = { $count } çizim öğesi görüntüye çizilmedi.
io-pdf-command-plot = Çiz
io-pdf-command-plot-description = Yerleşimleri veya bir model alanının penceresini PDF dosyasına, SVG dosyalarına ya da PNG dosyalarına çizer.
