# arc-mesh tarafından sahip olunan hataların iletileri. Tanımlayıcılar
# crates/arc-mesh/src/error.rs içinde kayıtlıdır.

mesh-index-out-of-range = Ağda, var olmayan bir köşeye başvuran bir üçgen var.
mesh-value-not-finite = Ağın bir konum veya öznitelik değeri sonlu bir sayı değil.
mesh-channel-length-mismatch = Ağın bir özniteliği her köşe için bir değer içermiyor.
mesh-channel-invalid = Ağın bir özniteliğinin türü yanlış, adı geçersiz veya değerleri geçersiz ya da ağda çok fazla öznitelik var.
mesh-channel-missing = İşlem, ağda bulunmayan bir öznitelik gerektiriyor.
mesh-group-invalid = Ağın bir yüz grubu boş, üçgenlerinin dışında kalıyor veya başka bir grupla çakışıyor.
mesh-limit-exceeded = Ağdaki köşe veya üçgen sayısı sınırı aşıyor.
mesh-transform-singular = Dönüşümün tersi alınamıyor.
mesh-precision-insufficient = Sonuç yeterli duyarlılıkla hesaplanamıyor.
mesh-merge-incompatible = Ağların öznitelikleri farklı olduğundan birleştirilemez.
mesh-tolerance-invalid = Bir tolerans negatif veya sonlu bir sayı değil.
mesh-crease-invalid = Kıvrım açısı sıfır ile yarım tur arasında olmalıdır.
mesh-quantization-invalid = Nicemleme hata sınırı pozitif bir sayı olmalıdır.
mesh-decode-invalid = Saklanan ağ hasarlı veya çok büyük.
mesh-transform-not-affine = Dönüşüm afin değil: son satırı 0, 0, 0, 1 olmalıdır.
mesh-measure-not-closed = Ağ kapalı ve tutarlı yönlendirilmiş olmadığından hacmi yoktur.
mesh-measure-zero-volume = Ağ hiçbir hacim çevrelemediğinden ağırlık merkezi yoktur.
mesh-topology-non-manifold = Ağda ikiden fazla üçgenin paylaştığı bir kenar veya sıkışmış bir köşe var; cinsi tanımsızdır.
mesh-query-invalid = Işın, nokta veya uzaklık sınırı sonlu bir sayı değil ya da ışının yönü yok.
mesh-query-empty = Sorgulanacak üçgeni olan bir ağ gerekir.
mesh-repair-invalid = Bir onarım seçeneği aralığın dışında.
mesh-simplify-invalid = Hedef üçgen sayısı veya sonlu, negatif olmayan bir hata sınırı verin.
mesh-lod-invalid = Bir ayrıntı düzeyi ayarı aralığın dışında ya da zincirde hiç düzey yok.
