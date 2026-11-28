use super::*;

#[test]
fn xml_reader_handles_markup_and_refuses_malformed_input() {
    let root = parse_xml(
        "<?xml version=\"1.0\"?>\n<?style x?><!-- c --><a k='v &amp; w'><b/>x &lt;y&gt;&#65;<c>t</c></a>",
    )
    .unwrap();
    assert_eq!(root.name, "a");
    assert_eq!(root.attribute("k"), Some("v & w"));
    assert_eq!(root.children.len(), 2);
    assert_eq!(root.text, "x <y>A");
    assert_eq!(root.text_of("c"), Some("t"));
    for bad in [
        "<a><b></a>",
        "<a>",
        "<a k=v/>",
        "<!DOCTYPE a><a/>",
        "<a/><b/>",
        "<a>&bogus;</a>",
    ] {
        assert!(parse_xml(bad).is_err(), "{bad}");
    }
    let deep = "<a>".repeat(MAX_DEPTH + 2);
    assert!(parse_xml(&deep).is_err());
}

#[test]
fn applicable_classes_split_predefined_types_and_modules() {
    assert_eq!(applicable("IfcWall"), Some(("IfcWall".into(), None)));
    assert_eq!(
        applicable("IfcDoor/GATE"),
        Some(("IfcDoor".into(), Some("GATE".into())))
    );
    assert_eq!(
        applicable("IFCKERNEL/IfcProduct"),
        Some(("IfcProduct".into(), None))
    );
    assert_eq!(applicable(" "), None);
}

/// A definition written for this test with one property of each kind.
const SET: &str = r#"<PropertySetDef templatetype="PSET_TYPEDRIVENOVERRIDE">
  <Name>Pset_TestCommon</Name>
  <ApplicableClasses><ClassName>IfcSlab</ClassName><ClassName>IfcSlab/ROOF</ClassName></ApplicableClasses>
  <PropertyDefs>
    <PropertyDef><Name>A</Name><PropertyType><TypePropertySingleValue><DataType type="IfcLabel"/></TypePropertySingleValue></PropertyType></PropertyDef>
    <PropertyDef><Name>B</Name><PropertyType><TypePropertyEnumeratedValue><EnumList name="PEnum_X"><EnumItem>ONE</EnumItem><EnumItem> TWO </EnumItem></EnumList></TypePropertyEnumeratedValue></PropertyType></PropertyDef>
    <PropertyDef><Name>C</Name><PropertyType><TypePropertyBoundedValue><DataType type="IfcReal"/></TypePropertyBoundedValue></PropertyType></PropertyDef>
    <PropertyDef><Name>D</Name><PropertyType><TypePropertyListValue><ListValue><DataType type="IfcText"/></ListValue></TypePropertyListValue></PropertyType></PropertyDef>
    <PropertyDef><Name>E</Name><PropertyType><TypePropertyTableValue><Expression/><DefiningValue><DataType type="IfcReal"/></DefiningValue><DefinedValue><DataType type=" IfcTimeMeasure"/></DefinedValue></TypePropertyTableValue></PropertyType></PropertyDef>
    <PropertyDef><Name>F</Name><PropertyType><TypePropertyReferenceValue reftype="IfcPerson "/></PropertyType></PropertyDef>
    <PropertyDef><Name>G</Name><PropertyType><TypeSimpleProperty><DataType type="IfcTimeSeries"/></TypeSimpleProperty></PropertyType></PropertyDef>
    <PropertyDef><Name>J</Name><PropertyType><TypePropertyEnumeratedValue><EnumList/><ConstantList><ConstantDef><Name>X</Name></ConstantDef></ConstantList></TypePropertyEnumeratedValue></PropertyType></PropertyDef>
    <PropertyDef><Name>H</Name><PropertyType><TypeComplexProperty name="CP_X"><PropertyDef><Name>I</Name><PropertyType><TypePropertySingleValue><DataType type="IfcInteger"/></TypePropertySingleValue></PropertyType></PropertyDef></TypeComplexProperty></PropertyType></PropertyDef>
  </PropertyDefs>
</PropertySetDef>"#;

#[test]
fn every_property_kind_is_read() {
    let set = parse_definition(SET).unwrap();
    assert_eq!(set.name, "Pset_TestCommon");
    assert!(!set.quantities);
    assert_eq!(set.template_type, "PSET_TYPEDRIVENOVERRIDE");
    assert_eq!(
        set.applicable,
        [
            ("IfcSlab".to_owned(), None),
            ("IfcSlab".to_owned(), Some("ROOF".to_owned()))
        ]
    );
    let kinds: Vec<&Kind> = set.properties.iter().map(|p| &p.kind).collect();
    assert_eq!(kinds[0], &Kind::Single("IfcLabel".into()));
    assert_eq!(
        kinds[1],
        &Kind::Enumerated {
            name: "PEnum_X".into(),
            items: vec!["ONE".into(), "TWO".into()]
        }
    );
    assert_eq!(kinds[2], &Kind::Bounded("IfcReal".into()));
    assert_eq!(kinds[3], &Kind::List("IfcText".into()));
    assert_eq!(
        kinds[4],
        &Kind::Table {
            defining: "IfcReal".into(),
            defined: "IfcTimeMeasure".into()
        }
    );
    assert_eq!(kinds[5], &Kind::Reference("IfcPerson".into()));
    assert_eq!(kinds[6], &Kind::Reference("IfcTimeSeries".into()));
    assert_eq!(
        kinds[7],
        &Kind::Enumerated {
            name: String::new(),
            items: vec!["X".into()]
        }
    );
    let Kind::Complex { name, properties } = kinds[8] else {
        panic!("complex expected");
    };
    assert_eq!(name, "CP_X");
    assert_eq!(properties[0].kind, Kind::Single("IfcInteger".into()));
    let mut code = String::new();
    emit_props(&set.properties, 0, &mut code);
    assert!(code.contains("PropertyKind::Complex { name: \"CP_X\", properties: &["));
}

#[test]
fn quantities_and_refusals() {
    let set = parse_definition(
        "<QtoSetDef><Name>Qto_T</Name><QtoDefs><QtoDef><Name>L</Name><QtoType>Q_LENGTH</QtoType></QtoDef>\
         <QtoDef><Name>N</Name><QtoType>Q_COUNT</QtoType></QtoDef></QtoDefs></QtoSetDef>",
    )
    .unwrap();
    assert!(set.quantities);
    assert_eq!(set.properties[1].kind, Kind::Quantity("Count"));
    for bad in [
        "<QtoSetDef><Name>Q</Name><QtoDefs><QtoDef><Name>L</Name><QtoType>Q_X</QtoType></QtoDef></QtoDefs></QtoSetDef>",
        "<Other/>",
        "<PropertySetDef><Name>P</Name><PropertyDefs><PropertyDef><Name>A</Name><PropertyType><TypeX/></PropertyType></PropertyDef></PropertyDefs></PropertySetDef>",
        "<PropertySetDef><Name>P</Name><PropertyDefs><PropertyDef><Name>A</Name><PropertyType><TypePropertySingleValue/></PropertyType></PropertyDef></PropertyDefs></PropertySetDef>",
        "<QtoSetDef><Name>Q</Name><QtoDefs><QtoDef><Name>L</Name><QtoType>Q_AREA</QtoType></QtoDef><QtoDef><Name>L</Name><QtoType>Q_AREA</QtoType></QtoDef></QtoDefs></QtoSetDef>",
    ] {
        assert!(parse_definition(bad).is_err(), "{bad}");
    }
}

#[test]
fn folders_are_checked_against_their_digest() {
    let dir = std::env::temp_dir().join(format!("arc-ifc-psets-{}", std::process::id()));
    let sub = dir.join("f/sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("Pset_A.xml"), "<a/>").unwrap();
    let wrong = Folder {
        path: "f",
        blake3: "0",
    };
    let error = read_folder(&dir, &wrong).unwrap_err();
    let manifest = format!("sub/Pset_A.xml\t{}\n", blake3::hash(b"<a/>").to_hex());
    let digest = blake3::hash(manifest.as_bytes()).to_hex().to_string();
    assert!(error.contains(&digest), "{error}");
    let right = Folder {
        path: "f",
        blake3: Box::leak(digest.into_boxed_str()),
    };
    assert_eq!(read_folder(&dir, &right).unwrap().len(), 1);
    fs::remove_dir_all(&dir).unwrap();
}

/// With `ARC_IFC_PSD_DIR` set to a `Standards-builds` checkout: the published folders parse, give
/// the published counts, and the generated files are current.
#[test]
fn published_definitions_generate_the_checked_tables() {
    let Some(dir) = std::env::var_os("ARC_IFC_PSD_DIR").map(PathBuf::from) else {
        eprintln!("skipped: set ARC_IFC_PSD_DIR to a checkout of the published definitions");
        return;
    };
    for (source, count) in SOURCES.iter().zip([317, 513]) {
        let (code, sets) = generate(&dir, source).unwrap();
        assert_eq!(sets, count, "{}", source.schema);
        let path = output_dir().join(format!("psets_{}.rs", source.table.to_ascii_lowercase()));
        assert_eq!(
            fs::read_to_string(&path).ok().as_deref(),
            Some(code.as_str()),
            "{} is out of date: run cargo xtask ifc-psets",
            path.display()
        );
    }
}
