# arc-app tarafından sahip olunan iletiler: hata kodları, belge başlık uyarıları ve yerleşik
# ayarların açıklamaları.
# Tanımlayıcılar crates/arc-app/src/error.rs, application.rs ve settings.rs içinde kayıtlıdır.

app-settings-invalid-key = { $key } ayar anahtarı geçerli değil.
app-settings-duplicate-key = { $key } ayarı zaten kayıtlı.
app-settings-default-mismatch = { $key } ayarının varsayılan değeri türüyle uyuşmuyor.
app-settings-unknown-key = { $key } ayarı yok.
app-settings-type-mismatch = { $key } ayarının değeri türü için geçerli değil.
app-settings-scope-not-allowed = { $key } ayarı { $scope } düzeyinde ayarlanamaz.
app-settings-path-conflict = { $key } ayarı saklanamıyor çünkü dosyada yerini başka bir değer tutuyor.
app-settings-file-read-failed = { $path } ayar dosyası okunamadı.
app-settings-file-write-failed = { $path } ayar dosyası kaydedilemedi.
app-settings-file-malformed = { $path } ayar dosyası hasarlı. Dosyanın adı değiştirildi ve varsayılanlar kullanılıyor.
app-settings-file-too-large = { $path } ayar dosyası çok büyük. Dosyanın adı değiştirildi ve varsayılanlar kullanılıyor.
app-settings-file-set-aside-failed = Hasarlı { $path } ayar dosyasının adı değiştirilemedi.
app-locale-invalid-tag = { $tag } metni bir dil etiketi değil.
app-locale-catalog-invalid = { $tag } için ileti kataloğunda hatalar var.
app-locale-catalog-too-large = { $tag } için ileti kataloğu çok büyük.
app-locale-fallback-missing = Yedek dil { $tag } için katalog eksik.
app-log-invalid-config = { $field } günlük ayarı geçerli değil.
app-log-io = Günlük dosyasına yazılamadı.
app-recent-not-found = { $path } dosyası son kullanılanlar arasında yok.
app-recent-too-many-pinned = Daha fazla dosya sabitlenemez.
app-product-invalid-id = { $id } tanımlayıcısı geçerli değil.
app-product-duplicate = { $id } zaten kayıtlı.
app-product-unknown-type = { $id } çıkarıcısı, kayıtlı olmayan { $type } nesne türünü belirtiyor.
app-file-unsupported-type = { $path } dosyası, bu uygulamanın kullanabileceği türde değil.
app-document-unknown = Belge açık değil.
app-document-read-only = { $path } dosyası salt okunur açık ve kaydedilemez.
app-document-no-path = Belge henüz kaydedilmedi. Kaydedileceği bir dosya seçin.

app-banner-locked = { $user }, { $host } üzerinde bu dosyayı { $application } ile düzenliyor. Dosya salt okunur açıldı.
app-banner-locked-unknown = Başka bir uygulama bu dosyayı düzenliyor. Dosya salt okunur açıldı.
app-banner-stale-lock = Bu dosya, artık çalışmayan { $application } ile { $host } üzerinde { $user } tarafından kilitli bırakılmış. Kilidi devralana kadar salt okunur açık.
app-banner-newer-format = Bu dosya, bu uygulamanın desteklediğinden daha yeni olan { $version } biçiminde kaydedilmiş. Dosya salt okunur açıldı.
app-banner-recovered-copy = Bu değişiklikler yapıldıktan sonra dosya değişti. Değişiklikler, önceki sürüm üzerine yeni bir belge olarak kurtarıldı.

app-setting-ui-language = Kullanıcı arayüzünün dili veya işletim sistemini izlemek için "system".
app-setting-log-level = Günlük dosyalarına yazılan en ayrıntılı düzey.
app-setting-log-max-file-bytes = Bir günlük dosyasının yenisiyle değiştirileceği boyut, bayt cinsinden.
app-setting-log-max-files = Tutulan günlük dosyası sayısı.
app-setting-recent-max-entries = Son kullanılanlar listesinde tutulan sabitlenmemiş dosya sayısı.

app-command-file-new = Yeni
app-command-file-new-description = Yeni, boş bir belge açar.
app-command-file-open = Aç…
app-command-file-open-description = Bir dosyadan belge açar.
app-command-file-save = Kaydet
app-command-file-save-description = Etkin belgeyi dosyasına kaydeder.
app-command-file-save-as = Farklı Kaydet…
app-command-file-save-as-description = Etkin belgeyi başka bir dosyaya kaydeder.
app-command-file-close = Kapat
app-command-file-close-description = Etkin belgeyi kapatır.
app-command-quit = Çık
app-command-quit-description = Kaydedilmemiş değişiklikleri sorduktan sonra uygulamayı sonlandırır.
app-command-undo = Geri Al
app-command-undo-description = Etkin belgenin son değişikliğini geri alır.
app-command-redo = Yinele
app-command-redo-description = Etkin belgede geri alınan son değişikliği yineler.
app-command-select = Seç
app-command-select-description = Seçimi değiştirir.
app-command-select-clear = Seçimi Temizle
app-command-select-clear-description = Seçimi temizler.

app-dialog-unsaved = Bu belgede kaydedilmemiş değişiklikler var. Kapatmadan önce kaydedilsin mi?
app-window-failed = Uygulama penceresi açılamadı.
app-window-surface-failed = Grafik aygıtı uygulama penceresine çizim yapamıyor.

app-menu-file = Dosya
app-menu-edit = Düzenle
