# arc-ui tarafından sahip olunan iletiler: hata kodları ve bileşen metinleri.
# Tanımlayıcılar crates/arc-ui/src/error.rs ve crates/arc-ui/src/text.rs içinde kayıtlıdır.

ui-scale-out-of-range = { $percent } % arayüz ölçeği, % 75 ile % 300 aralığının dışında.
ui-icon-invalid = { $name } simgesi geçerli değil.
ui-icon-too-large = { $name } simgesi çok büyük.
ui-icon-too-many = Çok fazla simge var.
ui-icon-duplicate = { $name } simgesi iki kez tanımlanmış.
ui-menu-invalid = { $value } menü veya araç çubuğu girdisi geçerli değil.
ui-menu-too-deep = Menü çok derin iç içe geçmiş.
ui-menu-too-large = Menü veya araç çubuğunda çok fazla girdi var.
ui-command-unknown = { $command } komutu kullanılamıyor.
ui-keys-invalid = { $keys } tuş ataması geçerli değil.
ui-keys-conflict = { $command } için { $keys } tuşları zaten { $other } komutuna atanmış.
ui-keys-prefix-conflict = { $command } için { $keys } tuşları, { $other } için { $other_keys } tuşlarıyla çakışıyor.
ui-keys-too-many = Çok fazla tuş ataması var.
ui-dock-invalid = Panel yerleşimi geçerli değil.
ui-dock-too-deep = Panel yerleşimi çok derin iç içe geçmiş.
ui-dock-too-large = Panel yerleşiminde çok fazla panel var.
ui-dock-panel-unknown = { $panel } paneli yerleşimde yok.
ui-dock-panel-duplicate = { $panel } paneli yerleşimde zaten var.
ui-workspace-invalid = Çalışma alanı geçerli değil.
ui-workspace-unknown = { $name } adlı bir çalışma alanı yok.
ui-workspace-too-large = Çok fazla çalışma alanı var veya çalışma alanı çok büyük.
ui-workspace-version = Çalışma alanı daha yeni bir sürümle veya başka bir biçimde kaydedilmiş.
ui-quantity-out-of-range = Değer { $min } ile { $max } arasında olmalıdır.
ui-quantity-not-integer = Değer bir tam sayı olmalıdır.
ui-property-too-large = Özellikleri göstermek için çok fazla nesne veya alan seçili.
ui-palette-search = Komut ara
ui-palette-no-results = Eşleşen komut yok
ui-notification-dismiss = Kapat
ui-severity-info = Bilgi
ui-severity-success = Başarılı
ui-severity-warning = Uyarı
ui-severity-error = Hata
ui-progress-cancel = İptal
ui-dock-separator = Panel ayırıcısı
ui-dock-close = Kapat
ui-command-line = Komut
ui-property-mixed = (karışık)
ui-property-overridden = (geçersiz kılındı)
ui-quantity-scrub = Değeri değiştirmek için sürükleyin
ui-tree-filter = Süzgeç
ui-tree-expand = Genişlet
ui-tree-collapse = Daralt
ui-tree-visible = Görünür
ui-tree-locked = Kilitli
ui-diagnostics-filter = Tanılamaları süz
ui-diagnostics-none = Tanılama yok
ui-viewport = { $name } görünüm alanı
ui-viewport-selection = { $count ->
    [0] Nesne seçilmedi
    [1] Bir nesne seçildi
   *[other] { $count } nesne seçildi
}
ui-viewport-status = { $selection }. { $prompt }
ui-view-cube = Görünüm küpü
ui-view-cube-top = Üst
ui-view-cube-bottom = Alt
ui-view-cube-front = Ön
ui-view-cube-back = Arka
ui-view-cube-left = Sol
ui-view-cube-right = Sağ
ui-view-cube-edge = { $first }-{ $second } kenarı
ui-view-cube-corner = { $first }, { $second }, { $third } köşesi
ui-table = Tablo
ui-table-sorted = { $name }, { $direction } sıralı
ui-table-ascending = artan
ui-table-descending = azalan
ui-table-paste-refused = { $count ->
    [one] Bir hücre reddedildi.
   *[other] { $count } hücre reddedildi.
}
ui-table-copy-too-large = Seçim kopyalamak için çok fazla hücre içeriyor.
ui-table-too-large = Metin yapıştırmak için çok büyük.
ui-style-color = Renk
ui-style-layer = Katman
ui-style-linetype = Çizgi türü
ui-style-lineweight = Çizgi kalınlığı
ui-style-material = Malzeme
ui-style-filter = Süzgeç
ui-style-true-color = Gerçek renk { $hex }
ui-style-too-many = Çok fazla stil girdisi var.
