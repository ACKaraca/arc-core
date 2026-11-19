# arc-io-mesh tarafından sahip olunan hataların ve kayıp raporu nedenlerinin iletileri.
# Tanımlayıcılar crates/arc-io-mesh/src/error.rs içinde kayıtlıdır.

io-mesh-truncated = Ağ verisi beklenmedik biçimde sona eriyor.
io-mesh-value-invalid = Ağ verisi, beklenen türde sonlu bir sayı olmayan bir değer içeriyor.
io-mesh-index-out-of-range = Ağ verisi, var olmayan bir köşeye başvuruyor.
io-mesh-face-invalid = Ağ verisi üçten az köşesi olan bir yüzey içeriyor.
io-mesh-structure-invalid = Ağ verisi biçimin gerektirdiği gibi düzenlenmemiş.
io-mesh-header-invalid = PLY üst bilgisi bozuk ya da bilinmeyen bir biçim veya sürüm adı içeriyor.
io-mesh-text-invalid = Ağ verisi geçerli bir UTF-8 metni değil.
io-mesh-encoding-unsupported = Bu biçim istenen kodlama ile yazılamaz.
io-mesh-write-value-invalid = Ağ, bu biçimin yazamayacağı bir değer ya da ad içeriyor.
io-mesh-loss-element-skipped = Bu biçimin tutmadığı türde { $count } öğe atlandı.
io-mesh-loss-property-skipped = Bu biçimin tutmadığı özelliğe ait { $count } değer atlandı.
io-mesh-loss-polygon-triangulated = Üçten fazla köşesi olan { $count } çokgen üçgenlere bölündü.
io-mesh-loss-attribute-dropped = Bu biçim { $count } öğenin { $attribute } bilgisini tutamaz; bilgi atıldı.
io-mesh-loss-attribute-incomplete = { $attribute } bilgisi { $count } köşede eksik; bu yüzden tutulmadı.
io-mesh-loss-precision-reduced = { $count } değerin { $attribute } bu biçimin duyarlığına yuvarlandı.
io-mesh-loss-material-inherited = Malzemesi olmayan { $count } parça, malzemeli bir parçanın ardından geliyor ve o malzemeyle okunur; bu biçim malzemenin kaldırılmasını desteklemez.
io-mesh-reference-invalid = Dosya, var olmayan bir öğeye başvuruyor ya da bir düğüme iki kez ulaşıyor.
io-mesh-extension-unsupported = Dosya, bu okuyucunun desteklemediği bir uzantı gerektiriyor.
io-mesh-resource-unavailable = Dosya dışında saklanan bir arabellek okuyucuya verilmedi.
io-mesh-loss-extension-preserved = Bu okuyucunun yorumlamadığı { $count } uzantı veya ekstra değer JSON metni olarak saklandı.
io-mesh-loss-texture-dropped = Bu biçimin tutamadığı { $count } doku başvurusu bırakıldı.
