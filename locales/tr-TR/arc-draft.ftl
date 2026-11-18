# arc-draft tarafından sahip olunan hata iletileri ile komutlarının adları ve açıklamaları.
# Hata tanımlayıcıları crates/arc-draft/src/error.rs, komut tanımlayıcıları
# crates/arc-draft/src/commands.rs içinde kayıtlıdır.

draft-geometry-invalid = Saklanan geometri, bu nesnenin tuttuğu türde geçerli bir eğri değil.
draft-extrusion-zero = Ekstrüzyon yönünün uzunluğu yok.
draft-space-layout-unitless = Bir yerleşim bir uzunluk birimi belirtmelidir.
draft-linetype-pattern-empty = Bir çizgi tipi deseni, negatif olmayan ve toplamı sıfırdan büyük uzunluklar gerektirir.
draft-linetype-scale-invalid = Çizgi tipi ölçeği pozitif bir sayı olmalıdır.
draft-block-scale-zero = Bir blok başvurusu sıfırla ölçeklenemez.
draft-block-cycle = Blok tanımları birbirini içerir hâle gelirdi.
draft-block-nesting-too-deep = Blok başvuruları { $limit } düzeyden daha derin iç içe geçmiş.
draft-block-attribute-unknown = Blok tanımında { $tag } özniteliği yok.
draft-name-taken = { $name } adı zaten kullanılıyor.
draft-object-wrong-type = Nesne bu işlemin kabul ettiği türden değil.
draft-field-invalid = Değer { $field } alanına uymuyor.
draft-order-reference-moved = Bir nesne kendisinin üstüne veya altına yerleştirilemez.
draft-order-container-mismatch = Nesneler aynı alanda veya blok tanımında değil.

draft-command-space-create = Yeni çizim alanı
draft-command-space-create-description = Belirtilen birimle bir model alanı veya yerleşim oluşturur.
draft-command-space-modify = Çizim alanını değiştir
draft-command-space-modify-description = Model alanlarının veya yerleşimlerin bir alanını ayarlar.
draft-command-entity-create = Yeni çizim nesnesi
draft-command-entity-create-description = Bir çizim alanında veya blok tanımında nokta ya da eğri oluşturur.
draft-command-entity-modify = Çizim nesnelerini değiştir
draft-command-entity-modify-description = Çizim nesnelerinin bir alanını ayarlar.
draft-command-entity-delete = Çizim nesnelerini sil
draft-command-entity-delete-description = Çizim nesnelerini siler.
draft-command-layer-create = Yeni katman
draft-command-layer-create-description = Bir katman oluşturur.
draft-command-layer-modify = Katmanları değiştir
draft-command-layer-modify-description = Katmanların bir alanını ayarlar.
draft-command-layer-delete = Katmanları sil
draft-command-layer-delete-description = Hiçbir nesnenin kullanmadığı katmanları siler.
draft-command-linetype-load = Çizgi tipi yükle
draft-command-linetype-load-description = Deseninden bir çizgi tipi tanımı ekler.
draft-command-linetype-modify = Çizgi tiplerini değiştir
draft-command-linetype-modify-description = Çizgi tipi tanımlarının bir alanını ayarlar.
draft-command-block-define = Blok tanımla
draft-command-block-define-description = Bir blok tanımı oluşturur ve verilen nesneleri içine taşır.
draft-command-block-modify = Blok tanımlarını değiştir
draft-command-block-modify-description = Blok tanımlarının bir alanını ayarlar.
draft-command-block-insert = Blok ekle
draft-command-block-insert-description = Bir blok tanımına başvuru yerleştirir.
draft-command-order-front = En öne getir
draft-command-order-front-description = Nesneleri çizim alanlarındaki diğer tüm nesnelerden sonra çizer.
draft-command-order-back = En arkaya gönder
draft-command-order-back-description = Nesneleri çizim alanlarındaki diğer tüm nesnelerden önce çizer.
draft-command-order-above = Nesnenin üstüne getir
draft-command-order-above-description = Nesneleri başka bir nesneden hemen sonra çizer.
draft-command-order-below = Nesnenin altına gönder
draft-command-order-below-description = Nesneleri başka bir nesneden hemen önce çizer.
