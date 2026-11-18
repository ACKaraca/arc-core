# arc-base tarafından sahip olunan hataların iletileri. Tanımlayıcılar
# crates/arc-base/src/error.rs içinde kayıtlıdır.
#
# Sözlük (tüm tr-TR kataloglarında tutarlı kullanılır):
#   document = belge          object = nesne           command = komut
#   property = özellik        field = alan             selection = seçim
#   undo = geri al            redo = yinele            save = kaydet
#   file = dosya              setting = ayar           panel = panel
#   outliner = nesne ağacı    viewport = görünüm alanı diagnostics = tanılama
#   error = hata              warning = uyarı          layout = yerleşim
#   mesh = ağ (örgü)          geometry = geometri      tolerance = tolerans
#   transform = dönüşüm       catalog = ileti kataloğu parameter = parametre
#   unit = birim              expression = ifade       key binding = tuş ataması
#   identifier = tanımlayıcı  derived data = türetilmiş veri
#   Hata kodları, komut tanımlayıcıları ve değişken adları çevrilmez.

base-id-invalid = Metin geçerli bir tanımlayıcı değil.
base-unit-dimension-mismatch = Nicelikler farklı şeyleri ölçüyor ve birleştirilemez.
base-unit-unknown = { $name } birimi bilinmiyor.
base-display-invalid-increment = Yuvarlama adımı pozitif bir sayı olmalıdır.
base-cancelled = İşlem iptal edildi.
