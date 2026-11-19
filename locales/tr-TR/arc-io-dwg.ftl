# arc-io-dwg tarafından sahip olunan hataların ve kayıp nedenlerinin iletileri. Tanımlayıcılar
# crates/arc-io-dwg/src/error.rs içinde kayıtlıdır. Sözlük için arc-base.ftl dosyasına bakın.

io-dwg-signature-invalid = Veri bir DWG sürüm imzasıyla başlamıyor.
io-dwg-version-unsupported = { $version } DWG sürümü desteklenmiyor.
io-dwg-truncated = DWG verisi yapıları tamamlanmadan sona eriyor.
io-dwg-structure-invalid = DWG verisi bozuk veya var olmayan kayıtlara başvuruyor.
io-dwg-checksum-mismatch = DWG verisindeki bir sağlama değeri içeriğiyle eşleşmiyor.
io-dwg-library-failed = DWG okuyucusu beklenmedik biçimde durdu; dosya okunamadı.
io-dwg-loss-header-dropped = Çizimin tuttuğu dışındaki başlık değişkenleri aktarılmadı.
io-dwg-loss-table-dropped = Çizimin tutmadığı bir tablonun { $count } girdisi aktarılmadı.
io-dwg-loss-table-entry-partial = { $count } tablo girdisi, çizimin taşımadığı bazı veriler içeriyordu.
io-dwg-loss-object-dropped = { $count } grafik olmayan nesne aktarılmadı.
io-dwg-loss-entity-dropped = Çizimin tutmadığı türde veya bir blok dışındaki { $count } öğe aktarılmadı.
io-dwg-loss-entity-partial = { $count } öğe, çizimin taşımadığı bazı veriler içeriyordu.
io-dwg-loss-value-skipped = Bir değer sonlu bir sayı olmadığı için { $count } kayıt atlandı.
io-dwg-loss-handle-unresolved = Bir blok tanımının dosyada bulunmayan öğelere verdiği { $count } başvuru aktarılmadı.
