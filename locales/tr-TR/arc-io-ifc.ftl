# arc-io-ifc tarafından sahip olunan hataların ve kayıp raporu nedenlerinin iletileri.
# Tanımlayıcılar crates/arc-io-ifc/src/error.rs içinde kayıtlıdır.

io-limit-exceeded = Dosya içe aktarma sınırını aşıyor: { $limit }.
io-ifc-truncated = IFC verisi dosya tamamlanmadan sona eriyor.
io-ifc-syntax = IFC verisi bu konumda geçerli bir fiziksel dosya sözdizimi değil.
io-ifc-structure-invalid = IFC dosyasının üstbilgi, veri veya şema bölümü eksik ya da sırası hatalı.
io-ifc-text-invalid = IFC verisindeki bir metin, fiziksel dosyanın kodlamasında geçerli değil.
io-ifc-value-invalid = IFC verisindeki bir sayı, ikili veya sıralama değeri hatalı ya da aralık dışında.
io-ifc-instance-duplicate = IFC verisinde aynı adı taşıyan iki örnek var.
io-ifc-archive-entry-missing = IFC arşivinde adı .ifc ile biten bir girdi yok.
io-ifc-archive-entry-ambiguous = IFC arşivinde adı .ifc ile biten birden fazla girdi var.
io-ifc-loss-entity-preserved = { $count } varlık, tipli görünümlerin okumadığı bir türden olduğu için genel örnek olarak korundu.
io-ifc-loss-reference-unresolved = { $count } başvuru dosyada bulunmayan örneklere işaret ediyor; başvurular olduğu gibi korundu.
io-ifc-loss-schema-unrecognized = Dosya, bu okuyucunun tanımadığı bir şema adı veriyor; varlıkları genel örnek olarak korundu.
io-ifc-loss-value-unexpected = { $count } öznitelik değeri, tipli görünümlerin okumadığı türden olduğu için bırakıldı.
io-ifc-loss-cycle-cut = { $count } uzamsal ilişki bir öğeyi kendini içerir hale getiriyor ya da iki kez yerleştiriyor; bu ilişkiler kesildi.
io-ifc-loss-duplicate-placement = Bir öğenin, özelliğin veya niceliğin { $count } yinelenen yerleştirmesi bir kez okundu; yinelemeler bırakıldı.
io-ifc-loss-target-missing = { $count } ilişki, özellik veya nicelik üyesi, bulunmayan ya da görünümlerin kullanamadığı türde bir örneğe işaret ediyor; okunmadılar.
io-ifc-loss-not-mapped = { $count } varlık görünümlerin anladığı ama eşleyemediği türde (başka türde bir özellik veya nicelik, karmaşık bir özellik ya da uzamsal yapının dışındaki bir öğe); genel örnek olarak korundu.
io-ifc-loss-section-skipped = DATA dışındaki { $count } bölüm (ANCHOR, REFERENCE veya SIGNATURE) bütün olarak atlandı.
io-ifc-loss-archive-entry-skipped = { $count } arşiv girdisi kaynak çatalı veya üst veri içeriyordu; model olmadıkları için atlandı.
io-ifc-loss-text-latin1 = UTF-8 olmayan metinlerdeki { $count } bayt ISO 8859-1 olarak okundu.
io-ifc-loss-text-control-kept = Metinlerin içindeki { $count } ham sekme, satır başı veya satır sonu baytı olduğu gibi korundu.
io-ifc-loss-bytes-ignored = Bölümlerin dışındaki { $count } bayt yok sayıldı: bayt sırası işareti veya dosya sonundaki dolgu.
io-ifc-loss-trailing-bytes = Dosya sonundan sonraki, dolgu olmayan { $count } bayt yok sayıldı.
