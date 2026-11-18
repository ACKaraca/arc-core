# arc-solve tarafından sahip olunan hataların iletileri. Tanımlayıcılar
# crates/arc-solve/src/error.rs içinde kayıtlıdır.

solve-expr-syntax = İfade doğru yazılmamış.
solve-expr-unknown-unit = { $name } birimi bilinmiyor.
solve-expr-offset-unit = °C veya °F gibi ötelemeli bir ölçekteki sıcaklık bu şekilde kullanılamaz: başka birimlerle birleştirilemez, üssü alınamaz, işareti değiştirilemez, yuvarlanamaz ve bu türden başka bir sıcaklıkla toplanamaz veya çarpılamaz. K cinsinden bir fark kullanın.
solve-expr-unknown-function = { $name } işlevi bilinmiyor.
solve-expr-unknown-name = { $name } adı bilinmiyor.
solve-expr-dimension-mismatch = Değerler farklı şeyleri ölçüyor ve burada birleştirilemez.
solve-expr-type-mismatch = Burada başka türde bir değer gerekir.
solve-expr-argument-count = İşleve yanlış sayıda bağımsız değişken verildi.
solve-expr-domain = Değer hesaplanamıyor: bir bağımsız değişken aralığın dışında veya sonuç sonlu bir sayı değil.
solve-expr-limit-exceeded = İfade çok uzun, çok derin iç içe geçmiş, çok fazla adım gerektiriyor veya çok büyük bir değer oluşturuyor.
solve-expr-separator-invalid = Ondalık ayırıcı ifadelerde kullanılamaz.
solve-function-invalid = { $name } işlevi kaydedilemez.
