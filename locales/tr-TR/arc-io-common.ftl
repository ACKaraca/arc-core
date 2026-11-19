# arc-io-common tarafından sahip olunan hataların iletileri. Tanımlayıcılar
# crates/arc-io-common/src/error.rs içinde kayıtlıdır.

io-limit-exceeded = Dosya içe aktarma sınırını aşıyor: { $limit }.
io-zip-truncated = Arşiv beklenmedik biçimde sona eriyor.
io-zip-end-missing = Arşivin geçerli bir sonlandırma kaydı yok.
io-zip-structure-invalid = Arşivin yapısı tutarsız.
io-zip-multi-disk = Arşiv birden çok diske yayılmış; bu desteklenmiyor.
io-zip-encrypted = Arşiv şifreli bir öğe içeriyor; bu desteklenmiyor.
io-zip-method-unsupported = Arşiv desteklenmeyen bir sıkıştırma yöntemi kullanıyor.
io-zip-data-descriptor-unverified = Bir öğenin veri tanımlayıcısı dizin kaydıyla eşleşmiyor.
io-zip-header-mismatch = Arşivin bir yerel üst bilgisi dizin kaydıyla eşleşmiyor.
io-zip-overlap = Arşivin iki parçası aynı baytları kaplıyor.
io-zip-name-invalid = Arşiv boş ya da bozuk bir öğe adı içeriyor.
io-zip-path-unsafe = Arşiv, çıkarma klasörünün dışına çıkacak bir öğe adı içeriyor.
io-zip-name-duplicate = Arşiv aynı adlı iki öğe içeriyor.
io-zip-size-mismatch = Sıkıştırılmış veri arşivde bildirilen boyutla eşleşmiyor.
io-zip-inflate-failed = Bir öğenin sıkıştırılmış verisi bozuk.
io-zip-crc-mismatch = Bir öğenin verisi sağlama değeriyle eşleşmiyor.
io-zip-entry-missing = Arşivde bu dizinde bir öğe yok.
io-xml-truncated = XML belgesi beklenmedik biçimde sona eriyor.
io-xml-syntax = XML belgesi biçimce doğru değil.
io-xml-structure-invalid = XML öğeleri belgenin gerektirdiği biçimde düzenlenmemiş.
io-xml-text-invalid = XML belgesi geçerli bir UTF-8 metni olmayan ya da XML'in izin vermediği bir karakter içeren metin içeriyor.
io-xml-doctype-refused = XML belgesi bir belge türü bildirimi içeriyor; bu desteklenmiyor.
io-xml-processing-instruction-refused = XML belgesi bir işleme yönergesi içeriyor; bu desteklenmiyor.
io-xml-entity-refused = XML belgesi önceden tanımlı olmayan bir varlığa başvuruyor.
io-xml-reference-invalid = XML belgesi geçerli bir karakter belirtmeyen bir karakter başvurusu içeriyor.
io-xml-encoding-unsupported = XML belgesi UTF-8 ile kodlanmamış.
