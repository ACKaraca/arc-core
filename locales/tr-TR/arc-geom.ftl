# arc-geom tarafından sahip olunan hataların iletileri. Tanımlayıcılar
# crates/arc-geom/src/error.rs içinde kayıtlıdır.

geom-input-not-finite = Bir koordinat, uzunluk, açı, çarpan veya dönüşüm girdisi sonlu bir sayı değil.
geom-segment-zero-length = Doğru parçası aynı noktada başlıyor ve bitiyor.
geom-direction-zero = Yönün uzunluğu yok.
geom-radius-not-positive = Yarıçap pozitif bir sayı olmalıdır.
geom-sweep-invalid = Yay, sıfırdan büyük ve en fazla bir tam turluk açıyı taramalıdır.
geom-ellipse-invalid-axis = Elips, uzunluğu pozitif bir ana eksen ile sıfırdan büyük ve en fazla bir olan bir küçük/ana eksen oranı gerektirir.
geom-polyline-too-few-vertices = Bir çoklu çizgi en az iki köşe gerektirir.
geom-polyline-zero-length-segment = Çoklu çizginin ardışık iki köşesi aynı noktada.
geom-polyline-negative-width = Çoklu çizgi genişliği negatif olamaz.
geom-bbox-invalid = Sınır kutusunun en küçük değeri en büyük değerinin üstünde.
geom-transform-singular = Dönüşümün tersi alınamıyor.
geom-transform-not-similarity = Yaylı veya genişlikli bir çoklu çizgi yalnızca taşınabilir, döndürülebilir, yansıtılabilir veya düzgün ölçeklenebilir.
geom-precision-insufficient = Sonuç yeterli duyarlılıkla hesaplanamıyor.
geom-decode-invalid = Saklanan geometri hasarlı veya çok büyük.
geom-tolerance-invalid = Bir tolerans negatif, sonlu bir sayı değil veya pozitif bir değer gerekirken sıfır.
geom-view-eye-at-target = Görünümün bakış noktası ile hedefi aynı noktada.
geom-view-up-invalid = Yukarı yönü sıfır olmamalı ve bakış yönüne paralel olmamalıdır.
geom-view-fov-invalid = Görüş açısı sıfırdan büyük ve yarım turdan küçük olmalıdır.
geom-view-scale-not-positive = Görünüm yüksekliği pozitif bir sayı olmalıdır.
geom-view-clip-invalid = Yakın kesme uzaklığı uzak olandan küçük olmalı; perspektif görünümde ayrıca pozitif olmalıdır.
geom-view-aspect-invalid = En-boy oranı pozitif bir sayı olmalıdır.
geom-length-out-of-range = Uzunluk eğrinin dışında kalıyor.
geom-curve-unbounded = İşlem iki ucu olan bir eğri gerektirir; ışınların ve doğruların ucu yoktur.
geom-flatten-too-many-vertices = Eğriyi düzleştirmek için sınırın izin verdiğinden fazla köşe gerekiyor.
geom-region-loop-empty = Bir çevrim en az bir eğri içermelidir.
geom-region-loop-open = Çevrimin eğrileri uç uca birleşmiyor.
geom-region-loop-zero-area = Çevrim hiçbir alan çevrelemiyor.
geom-region-empty = Bir bölge en az bir dış çevrim gerektirir.
geom-region-hole-outside = Bir delik, dış çevriminin dışında kalıyor.
geom-triangulate-edges-cross = Bölgenin düzleştirilmiş sınırı kendini kesiyor veya kendine değiyor.
geom-triangulate-loop-collapsed = Bölgenin bir çevrimi bu toleransta düzleştirilince çöküyor.
geom-triangulate-nesting-invalid = Bir delik dış çevriminin dışında kalıyor veya bir parça başka bir parçayla çakışıyor.
geom-mesh-invalid = Üçgen ağında geçersiz bir köşe, köşelerinin dışında bir üçgen indisi veya saat yönünün tersine olmayan bir üçgen var.
geom-intersect-too-many-results = Eğrilerin kesişim sayısı sınırın izin verdiğinden fazla.
