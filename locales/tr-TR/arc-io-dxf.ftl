# arc-io-dxf tarafından sahip olunan hataların ve kayıp nedenlerinin iletileri. Tanımlayıcılar
# crates/arc-io-dxf/src/error.rs içinde kayıtlıdır. Sözlük için arc-base.ftl dosyasına bakın.

io-unsupported-version = { $version } biçim sürümü desteklenmiyor.
io-stream-failed = Veri kaynağından okunamadı.
io-dxf-truncated = DXF verisi beklenmedik biçimde sona eriyor.
io-dxf-group-code-invalid = DXF verisi geçersiz bir grup kodu içeriyor.
io-dxf-value-invalid = DXF verisi grup koduna uymayan bir değer içeriyor.
io-dxf-structure-invalid = DXF verisinin bölümleri, tabloları veya kayıtları doğru düzende değil.
io-dxf-text-invalid = DXF verisi geçerli UTF-8 olmayan metin içeriyor.
io-dxf-write-value-invalid = Çizim, DXF olarak yazılamayan bir değer içeriyor.
io-dxf-loss-section-preserved = Tanınmayan { $count } bölüm değiştirilmeden korundu.
io-dxf-loss-table-preserved = Tanınmayan { $count } tablo değiştirilmeden korundu.
io-dxf-loss-record-preserved = Tanınmayan türde { $count } kayıt değiştirilmeden korundu.
io-dxf-loss-groups-preserved = Kayıtların tanınmayan { $count } değeri değiştirilmeden korundu.
io-dxf-loss-comment-dropped = { $count } yorum atlandı.
io-dxf-loss-code-page-unsupported = { $code_page } kod sayfası desteklenmiyor; { $count } karakter ham bayt olarak korundu.
io-dxf-loss-byte-unmapped = { $count } baytın { $code_page } kod sayfasında karşılığı yok ve ham bayt olarak korundu.
io-dxf-loss-fit-data-dropped = Hedef sürüm saklayamadığı için { $count } tarama eğrisi uydurma noktası veya teğeti yazılmadı.
io-dxf-loss-raw-bytes-written = { $count } ham bayt karakteri Unicode karakter olarak yazıldı.
io-dxf-loss-handle-assigned = { $count } kayda yeni bir tutamaç atandı.
io-dxf-loss-structure-incomplete = Çizimde bu sürümün dosyalarının gerektirdiği blok kaydı tablosu yok.
