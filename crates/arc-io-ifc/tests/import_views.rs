//! The import views of a model that exercises each of them, in IFC2X3, IFC4 and IFC4X3: units,
//! placements, objects with their types, sets, materials, classifications, styles, openings,
//! aggregation, connections, space boundaries, groups, georeferencing and owner history; and the
//! coverage of the file. The tests need the schema tables of the schema they read; without them
//! the views are empty and a test returns early.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::cognitive_complexity,
    reason = "test code: fixtures and assertions"
)]

#[path = "support/mod.rs"]
mod support;

use arc_base::{Cancel, Limits};
use arc_io_ifc::{
    GeoSource, IfcFile, MaterialAssociation, ObjectKind, ObjectView, Origin, PlacementState,
    PropertyValue, ReadOptions, TypedViews, Value, read,
};

#[derive(Clone, Copy, PartialEq)]
enum Flavor {
    X23,
    X4,
    X43,
}

impl Flavor {
    fn name(self) -> &'static str {
        match self {
            Self::X23 => "IFC2X3",
            Self::X4 => "IFC4",
            Self::X43 => "IFC4X3_ADD2",
        }
    }
}

const ALL: [Flavor; 3] = [Flavor::X23, Flavor::X4, Flavor::X43];

/// A GlobalId of 22 characters made from a number: unique per number.
fn gid(n: u32) -> String {
    const DIGITS: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";
    let mut out = String::from("0000000000000000");
    for k in (0..6).rev() {
        out.push(char::from(DIGITS[((n >> (6 * k)) & 63) as usize]));
    }
    out
}

/// The units, owner history, project, spatial structure and placements.
fn base(f: Flavor) -> String {
    let mut s = String::new();
    s += "#1=IFCPERSON('p1','Doe','Jane',$,$,$,$,$);\n";
    s += "#2=IFCORGANIZATION('o1','Org',$,$,$);\n";
    s += "#3=IFCPERSONANDORGANIZATION(#1,#2,$);\n";
    s += "#4=IFCAPPLICATION(#2,'1.2','App','app');\n";
    s += "#5=IFCOWNERHISTORY(#3,#4,.READWRITE.,.NOCHANGE.,1700000100,#3,#4,1700000000);\n";
    s += "#10=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n";
    s += "#11=IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.);\n";
    s += "#12=IFCSIUNIT(*,.VOLUMEUNIT.,$,.CUBIC_METRE.);\n";
    s += "#13=IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.);\n";
    s += "#14=IFCMEASUREWITHUNIT(IFCPLANEANGLEMEASURE(0.01745329251994),#13);\n";
    s += "#15=IFCDIMENSIONALEXPONENTS(0,0,0,0,0,0,0);\n";
    s += "#16=IFCCONVERSIONBASEDUNIT(#15,.PLANEANGLEUNIT.,'DEGREE',#14);\n";
    s += "#17=IFCSIUNIT(*,.MASSUNIT.,$,.GRAM.);\n";
    s += "#18=IFCDERIVEDUNITELEMENT(#17,1);\n";
    s += "#19=IFCDERIVEDUNITELEMENT(#10,-3);\n";
    s += &match f {
        Flavor::X43 => "#20=IFCDERIVEDUNIT((#18,#19),.MASSDENSITYUNIT.,$,$);\n".to_owned(),
        _ => "#20=IFCDERIVEDUNIT((#18,#19),.MASSDENSITYUNIT.,$);\n".to_owned(),
    };
    s += "#22=IFCUNITASSIGNMENT((#10,#11,#12,#16,#20));\n";
    s += "#23=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-5,#24,$);\n";
    s += "#24=IFCAXIS2PLACEMENT3D(#25,$,$);\n#25=IFCCARTESIANPOINT((0.,0.,0.));\n";
    s += &format!(
        "#30=IFCPROJECT('{}',#5,'Project',$,$,$,$,(#23),#22);\n",
        gid(30)
    );
    s += &format!(
        "#31=IFCSITE('{}',#5,'Site',$,$,#50,$,$,.ELEMENT.,(50,30,0,500000),(-8,-15,-30,0),12.5,$,$);\n",
        gid(31)
    );
    s += &format!(
        "#32=IFCBUILDING('{}',#5,'Building',$,$,#53,$,$,.ELEMENT.,4.,$,$);\n",
        gid(32)
    );
    s += &format!(
        "#33=IFCBUILDINGSTOREY('{}',#5,'Storey',$,$,#56,$,$,.ELEMENT.,3000.);\n",
        gid(33)
    );
    s += &match f {
        Flavor::X23 => format!(
            "#34=IFCSPACE('{}',#5,'Room',$,$,#200,$,$,.ELEMENT.,.INTERNAL.,$);\n",
            gid(34)
        ),
        _ => format!(
            "#34=IFCSPACE('{}',#5,'Room',$,$,#200,$,$,.ELEMENT.,.SPACE.,$);\n",
            gid(34)
        ),
    };
    s += "#35=IFCRELAGGREGATES('";
    s += &gid(35);
    s += "',#5,$,$,#30,(#31));\n";
    s += &format!("#36=IFCRELAGGREGATES('{}',#5,$,$,#31,(#32));\n", gid(36));
    s += &format!("#37=IFCRELAGGREGATES('{}',#5,$,$,#32,(#33));\n", gid(37));
    s += &format!("#38=IFCRELAGGREGATES('{}',#5,$,$,#33,(#34));\n", gid(38));
    // Placements: site, building, storey (turned 90 degrees about z), wall, window by grid.
    s += "#50=IFCLOCALPLACEMENT($,#51);\n#51=IFCAXIS2PLACEMENT3D(#52,$,$);\n#52=IFCCARTESIANPOINT((1000.,2000.,0.));\n";
    s += "#53=IFCLOCALPLACEMENT(#50,#54);\n#54=IFCAXIS2PLACEMENT3D(#55,$,$);\n#55=IFCCARTESIANPOINT((0.,5000.,0.));\n";
    s += "#56=IFCLOCALPLACEMENT(#53,#57);\n#57=IFCAXIS2PLACEMENT3D(#58,#59,#60);\n#58=IFCCARTESIANPOINT((0.,0.,3000.));\n";
    s += "#59=IFCDIRECTION((0.,0.,1.));\n#60=IFCDIRECTION((0.,1.,0.));\n";
    s += "#61=IFCLOCALPLACEMENT(#56,#62);\n#62=IFCAXIS2PLACEMENT3D(#63,$,$);\n#63=IFCCARTESIANPOINT((100.,0.,0.));\n";
    s += "#203=IFCLOCALPLACEMENT(#56,#62);\n#202=IFCLOCALPLACEMENT(#56,#62);\n#200=IFCLOCALPLACEMENT(#56,#62);\n#201=IFCLOCALPLACEMENT(#56,#62);\n";
    s += &match f {
        Flavor::X43 => "#64=IFCGRIDPLACEMENT($,#65,$);\n".to_owned(),
        _ => "#64=IFCGRIDPLACEMENT(#65,$);\n".to_owned(),
    };
    s += "#65=IFCVIRTUALGRIDINTERSECTION((#66,#67),(0.,0.));\n";
    s += "#66=IFCGRIDAXIS('A',#68,.T.);\n#67=IFCGRIDAXIS('1',#69,.T.);\n";
    s += "#68=IFCPOLYLINE((#210,#211));\n#69=IFCPOLYLINE((#210,#212));\n";
    s += "#210=IFCCARTESIANPOINT((0.,0.));\n#211=IFCCARTESIANPOINT((10.,0.));\n#212=IFCCARTESIANPOINT((0.,10.));\n";
    s += "#213=IFCLOCALPLACEMENT(#56,#62);\n";
    s += &format!(
        "#214=IFCGRID('{}',#5,'Grid',$,$,#213,$,(#66),(#67),${});\n",
        gid(214),
        if f == Flavor::X23 { "" } else { ",$" }
    );
    s
}

/// The wall type, walls, opening, window and their relationships.
fn wall_type(f: Flavor) -> &'static str {
    if f == Flavor::X23 {
        "STANDARD"
    } else {
        "SOLIDWALL"
    }
}

fn elements(f: Flavor) -> String {
    let tail = |value: &str| {
        if f == Flavor::X23 {
            String::new()
        } else {
            format!(",{value}")
        }
    };
    let mut s = String::new();
    // Type with a property set and a quantity set.
    s += &format!(
        "#40=IFCWALLTYPE('{}',#5,'Brick wall',$,$,(#41,#47),$,'TT','Brick',.{}.);\n",
        gid(40),
        wall_type(f)
    );
    s += &format!(
        "#41=IFCPROPERTYSET('{}',#5,'Pset_WallCommon',$,(#42,#43,#44));\n",
        gid(41)
    );
    s += "#42=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('2HR'),$);\n";
    s += "#43=IFCPROPERTYSINGLEVALUE('IsExternal',$,IFCBOOLEAN(.T.),$);\n";
    s += "#44=IFCPROPERTYSINGLEVALUE('Thickness',$,IFCLENGTHMEASURE(200.),$);\n";
    s += &format!(
        "#47=IFCELEMENTQUANTITY('{}',#5,'Qto_WallBaseQuantities',$,$,(#48));\n",
        gid(47)
    );
    s += &match f {
        Flavor::X23 => "#48=IFCQUANTITYLENGTH('Width',$,$,200.);\n".to_owned(),
        _ => "#48=IFCQUANTITYLENGTH('Width',$,$,200.,$);\n".to_owned(),
    };
    // Wall A: placed, styled, with an occurrence pset that overrides the type's.
    s += &format!(
        "#70=IFCWALL('{}',#5,'Wall A','First wall','Custom',#61,#80,'T1'{});\n",
        gid(70),
        tail(".NOTDEFINED.")
    );
    s += &format!(
        "#71=IFCWALL('{}',#5,'Wall B',$,'UD',#202,$,'T2'{});\n",
        gid(71),
        tail(".USERDEFINED.")
    );
    s += &format!(
        "#45=IFCWALLTYPE('{}',#5,'Generic',$,$,$,$,$,$,.NOTDEFINED.);\n",
        gid(45)
    );
    s += &format!(
        "#96=IFCWALL('{}',#5,'Wall C',$,'UD',#203,$,'T3'{});\n",
        gid(96),
        tail(".USERDEFINED.")
    );
    s += &format!("#97=IFCRELDEFINESBYTYPE('{}',#5,$,$,(#96),#45);\n", gid(97));
    s += "#72=IFCRELDEFINESBYTYPE('";
    s += &gid(72);
    s += "',#5,$,$,(#70,#71),#40);\n";
    s += &format!(
        "#73=IFCPROPERTYSET('{}',#5,'Pset_WallCommon',$,(#74,#75));\n",
        gid(73)
    );
    s += "#74=IFCPROPERTYSINGLEVALUE('IsExternal',$,IFCBOOLEAN(.F.),$);\n";
    s += "#75=IFCPROPERTYSINGLEVALUE('LoadBearing',$,IFCBOOLEAN(.T.),$);\n";
    s += &format!(
        "#76=IFCRELDEFINESBYPROPERTIES('{}',#5,$,$,(#70),#73);\n",
        gid(76)
    );
    s += &format!(
        "#77=IFCELEMENTQUANTITY('{}',#5,'Qto_WallBaseQuantities',$,$,(#78,#79));\n",
        gid(77)
    );
    s += &match f {
        Flavor::X23 => "#78=IFCQUANTITYLENGTH('Length',$,$,5000.);\n#79=IFCQUANTITYAREA('NetSideArea',$,$,12.5);\n".to_owned(),
        _ => "#78=IFCQUANTITYLENGTH('Length',$,$,5000.,$);\n#79=IFCQUANTITYAREA('NetSideArea',$,$,12.5,$);\n".to_owned(),
    };
    s += &format!(
        "#7A=IFCRELDEFINESBYPROPERTIES('{}',#5,$,$,(#70),#77);\n",
        gid(7)
    );
    // Styled representation item of wall A.
    s += "#80=IFCPRODUCTDEFINITIONSHAPE($,$,(#81));\n#81=IFCSHAPEREPRESENTATION(#23,'Body','Point',(#82));\n#82=IFCCARTESIANPOINT((0.,0.,0.));\n";
    s += "#83=IFCCOLOURRGB($,0.8,0.2,0.1);\n";
    s += &match f {
        Flavor::X23 => {
            "#84=IFCSURFACESTYLERENDERING(#83,0.25,$,$,$,$,$,$,.NOTDEFINED.);\n".to_owned()
        }
        _ => "#84=IFCSURFACESTYLERENDERING(#83,0.25,$,$,$,$,$,$,.NOTDEFINED.);\n".to_owned(),
    };
    s += "#85=IFCSURFACESTYLE('Red',.BOTH.,(#84));\n";
    s += &match f {
        Flavor::X43 => "#86=IFCSTYLEDITEM(#82,(#85),$);\n".to_owned(),
        _ => "#87=IFCPRESENTATIONSTYLEASSIGNMENT((#85));\n#86=IFCSTYLEDITEM(#82,(#87),$);\n"
            .to_owned(),
    };
    // Containment, opening and window.
    s += &format!(
        "#90=IFCRELCONTAINEDINSPATIALSTRUCTURE('{}',#5,$,$,(#70,#71,#92,#214,#96),#33);\n",
        gid(90)
    );
    s += &format!(
        "#91=IFCOPENINGELEMENT('{}',#5,'Opening',$,$,#201,$,${});\n",
        gid(91),
        tail(".OPENING.")
    );
    s += &match f {
        Flavor::X23 => format!(
            "#92=IFCWINDOW('{}',#5,'Window',$,$,#64,$,$,1200.,900.);\n",
            gid(92)
        ),
        _ => format!(
            "#92=IFCWINDOW('{}',#5,'Window',$,$,#64,$,$,1200.,900.,.WINDOW.,.SINGLE_PANEL.,$);\n",
            gid(92)
        ),
    };
    s += &format!("#93=IFCRELVOIDSELEMENT('{}',#5,$,$,#70,#91);\n", gid(93));
    s += &format!("#94=IFCRELFILLSELEMENT('{}',#5,$,$,#91,#92);\n", gid(94));
    s
}

/// Materials, classifications, aggregation, connections, boundaries, groups.
fn relations(f: Flavor) -> String {
    let mut s = String::new();
    // Material layer set usage on wall A, material on the type, list on wall B.
    s += &match f {
        Flavor::X23 => "#100=IFCMATERIAL('Brick');\n#101=IFCMATERIAL('Plaster');\n".to_owned(),
        _ => {
            "#100=IFCMATERIAL('Brick',$,'masonry');\n#101=IFCMATERIAL('Plaster',$,$);\n".to_owned()
        }
    };
    s += &match f {
        Flavor::X23 => "#102=IFCMATERIALLAYER(#100,200.,.F.);\n#103=IFCMATERIALLAYER(#101,100.,$);\n#104=IFCMATERIALLAYERSET((#102,#103),'Wall build-up');\n#105=IFCMATERIALLAYERSETUSAGE(#104,.AXIS2.,.POSITIVE.,-150.);\n".to_owned(),
        _ => "#102=IFCMATERIALLAYER(#100,200.,.F.,'Core',$,'core',1);\n#103=IFCMATERIALLAYER(#101,100.,$,'Finish',$,$,$);\n#104=IFCMATERIALLAYERSET((#102,#103),'Wall build-up',$);\n#105=IFCMATERIALLAYERSETUSAGE(#104,.AXIS2.,.POSITIVE.,-150.,$);\n".to_owned(),
    };
    s += &format!(
        "#106=IFCRELASSOCIATESMATERIAL('{}',#5,$,$,(#70),#105);\n",
        gid(106)
    );
    s += &format!(
        "#107=IFCRELASSOCIATESMATERIAL('{}',#5,$,$,(#40),#100);\n",
        gid(107)
    );
    s += "#108=IFCMATERIALLIST((#100,#101));\n";
    s += &format!(
        "#109=IFCRELASSOCIATESMATERIAL('{}',#5,$,$,(#71),#108);\n",
        gid(109)
    );
    // Material style.
    let style = if f == Flavor::X43 { "#85" } else { "#87" };
    s += &format!(
        "#110=IFCSTYLEDREPRESENTATION(#23,'Style','Material',(#111));\n#111=IFCSTYLEDITEM($,({style}),$);\n#112=IFCMATERIALDEFINITIONREPRESENTATION($,$,(#110),#100);\n"
    );
    // Classification: the wall has its own reference, the type another one in the same system.
    s += &match f {
        Flavor::X23 => "#120=IFCCLASSIFICATION('Uniclass','2015',$,'Uniclass 2015');\n#121=IFCCLASSIFICATIONREFERENCE($,'Pr_20','Walls',#120);\n#122=IFCCLASSIFICATIONREFERENCE($,'Pr_20_10','Masonry walls',#120);\n".to_owned(),
        Flavor::X4 => "#120=IFCCLASSIFICATION('Uniclass','2015',$,'Uniclass 2015',$,$,$);\n#121=IFCCLASSIFICATIONREFERENCE($,'Pr_20','Walls',#120,$,$);\n#122=IFCCLASSIFICATIONREFERENCE($,'Pr_20_10','Masonry walls',#120,$,$);\n".to_owned(),
        Flavor::X43 => "#120=IFCCLASSIFICATION('Uniclass','2015',$,'Uniclass 2015',$,$,$);\n#121=IFCCLASSIFICATIONREFERENCE($,'Pr_20','Walls',#120,$,$);\n#122=IFCCLASSIFICATIONREFERENCE($,'Pr_20_10','Masonry walls',#120,$,$);\n".to_owned(),
    };
    s += &format!(
        "#123=IFCRELASSOCIATESCLASSIFICATION('{}',#5,$,$,(#70),#121);\n",
        gid(123)
    );
    s += &format!(
        "#124=IFCRELASSOCIATESCLASSIFICATION('{}',#5,$,$,(#40),#122);\n",
        gid(124)
    );
    // Aggregation and nesting of building element parts.
    let part = |id: u32| match f {
        Flavor::X23 => format!(
            "#{id}=IFCBUILDINGELEMENTPART('{}',#5,'Part',$,$,$,$,$);\n",
            gid(id)
        ),
        _ => format!(
            "#{id}=IFCBUILDINGELEMENTPART('{}',#5,'Part',$,$,$,$,$,$);\n",
            gid(id)
        ),
    };
    s += &part(130);
    s += &part(131);
    s += &format!("#132=IFCRELAGGREGATES('{}',#5,$,$,#70,(#130));\n", gid(132));
    s += &format!("#133=IFCRELNESTS('{}',#5,$,$,#130,(#131));\n", gid(133));
    // Connection, boundaries and groups.
    s += &format!(
        "#140=IFCRELCONNECTSPATHELEMENTS('{}',#5,'Corner',$,$,#70,#71,(0),(0),.ATSTART.,.ATEND.);\n",
        gid(140)
    );
    s += &match f {
        Flavor::X23 => format!(
            "#141=IFCRELSPACEBOUNDARY('{}',#5,$,$,#34,#70,$,.PHYSICAL.,.INTERNAL.);\n",
            gid(141)
        ),
        _ => format!(
            "#141=IFCRELSPACEBOUNDARY1STLEVEL('{}',#5,$,$,#34,#70,$,.PHYSICAL.,.INTERNAL.,$);\n#142=IFCRELSPACEBOUNDARY2NDLEVEL('{}',#5,$,$,#34,#71,$,.PHYSICAL.,.EXTERNAL.,#141,$);\n#143=IFCRELSPACEBOUNDARY2NDLEVEL('{}',#5,$,$,#34,#71,$,.PHYSICAL.,.EXTERNAL.,#141,#142);\n",
            gid(141),
            gid(142),
            gid(143)
        ),
    };
    s += &format!("#150=IFCGROUP('{}',#5,'Group',$,$);\n", gid(150));
    s += &format!(
        "#151=IFCZONE('{}',#5,'Zone',$,${});\n",
        gid(151),
        if f == Flavor::X23 { "" } else { ",$" }
    );
    s += &format!(
        "#152=IFCRELASSIGNSTOGROUP('{}',#5,$,$,(#70,#71),$,#150);\n",
        gid(152)
    );
    s += &format!(
        "#153=IFCRELASSIGNSTOGROUP('{}',#5,$,$,(#34),$,#151);\n",
        gid(153)
    );
    s
}

/// Georeferencing: `IfcMapConversion` in IFC4 and IFC4X3, the property set convention in IFC2X3.
fn georef(f: Flavor) -> String {
    match f {
        Flavor::X23 => {
            let mut s = format!("#160=IFCPROPERTYSET('{}',#5,'ePSet_MapConversion',$,(#161,#162,#163,#164,#165,#166));\n", gid(160));
            s += "#161=IFCPROPERTYSINGLEVALUE('Eastings',$,IFCLENGTHMEASURE(1000.),$);\n#162=IFCPROPERTYSINGLEVALUE('Northings',$,IFCLENGTHMEASURE(2000.),$);\n#163=IFCPROPERTYSINGLEVALUE('OrthogonalHeight',$,IFCLENGTHMEASURE(3.),$);\n#164=IFCPROPERTYSINGLEVALUE('XAxisAbscissa',$,IFCREAL(0.8),$);\n#165=IFCPROPERTYSINGLEVALUE('XAxisOrdinate',$,IFCREAL(0.6),$);\n#166=IFCPROPERTYSINGLEVALUE('Scale',$,IFCREAL(1.),$);\n";
            s += &format!("#167=IFCRELDEFINESBYPROPERTIES('{}',#5,$,$,(#30),#160);\n", gid(167));
            s += &format!("#168=IFCPROPERTYSET('{}',#5,'ePSet_ProjectedCRS',$,(#169));\n#169=IFCPROPERTYSINGLEVALUE('Name',$,IFCLABEL('EPSG:25832'),$);\n", gid(168));
            s += &format!("#170=IFCRELDEFINESBYPROPERTIES('{}',#5,$,$,(#30),#168);\n", gid(170));
            s
        }
        _ => "#160=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n#161=IFCPROJECTEDCRS('EPSG:25832','desc','ETRS89','DHHN2016','UTM','32N',#160);\n#162=IFCMAPCONVERSION(#23,#161,1000.,2000.,3.,0.8,0.6,1.);\n".to_owned(),
    }
}

fn model(f: Flavor) -> String {
    let body = format!("{}{}{}{}", base(f), elements(f), relations(f), georef(f));
    // The instance named `#7A` above is a hexadecimal-looking name; STEP names are decimal.
    let body = body.replace("#7A=", "#7001=");
    let document = if f == Flavor::X23 {
        "#900=IFCDOCUMENTREFERENCE('loc','id','doc');\n"
    } else {
        "#900=IFCDOCUMENTREFERENCE('loc','id','doc',$,$);\n"
    };
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition []'),'2;1');\n\
FILE_NAME('views.ifc','2026-01-01T00:00:00',(''),(''),'','','');\nFILE_SCHEMA(('{}'));\nENDSEC;\nDATA;\n{body}\
{document}ENDSEC;\nEND-ISO-10303-21;\n",
        f.name()
    )
}

fn load(f: Flavor) -> Option<(IfcFile, TypedViews)> {
    let text = model(f);
    support::dump_nonconformant(&format!("import_views_{}.ifc", f.name()), text.as_bytes());
    let file = read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file;
    file.tables()?;
    let views = file.typed_views(&Limits::DEFAULT, &Cancel::new()).unwrap();
    Some((file, views))
}

fn object(views: &TypedViews, instance: u64) -> &ObjectView {
    views
        .import
        .object(instance)
        .unwrap_or_else(|| panic!("object #{instance}"))
}

fn text_of(v: &Option<arc_io_ifc::Nominal>) -> Option<String> {
    match v.as_ref().map(|n| &n.value) {
        Some(Value::Text(t)) => Some(t.clone()),
        Some(Value::Enumeration(e)) => Some(e.clone()),
        _ => None,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn the_fixture_is_conformant() {
    for f in ALL {
        support::assert_conformant(
            &format!("import_views_{}.ifc", f.name()),
            model(f).as_bytes(),
        );
    }
}

#[test]
fn units_are_read_and_applied() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let units = &v.import.units;
        assert!(near(units.length_to_meters, 0.001), "{}", f.name());
        assert!(!units.length_assumed);
        assert!(near(units.angle_to_radians, 0.017_453_292_519_94));
        assert_eq!(units.assigned, 5);
        // Gram per cubic millimeter is 1e6 kilograms per cubic meter.
        let density = units
            .units
            .iter()
            .find(|u| u.unit_type == "MASSDENSITYUNIT")
            .unwrap();
        assert!(near(density.to_si.unwrap() / 1e6, 1.0), "{}", f.name());
        let wall = object(&v, 70);
        let quantities = &wall.quantity_sets[0].quantities;
        let length = quantities
            .iter()
            .find(|q| q.quantity.name.as_deref() == Some("Length"))
            .unwrap();
        assert!(near(length.si_value.unwrap(), 5.0), "{}", f.name());
        let area = quantities
            .iter()
            .find(|q| q.quantity.name.as_deref() == Some("NetSideArea"))
            .unwrap();
        assert!(near(area.si_value.unwrap(), 12.5));
    }
}

#[test]
fn placements_resolve_to_world_transforms() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let storey = object(&v, 33).placement.clone().unwrap();
        assert_eq!(storey.state, PlacementState::Resolved);
        // Site (1, 2, 0) + building (0, 5, 0) + storey (0, 0, 3) in meters.
        assert!(
            near(storey.world[3], 1.0) && near(storey.world[7], 7.0) && near(storey.world[11], 3.0)
        );
        // The storey is turned: its x axis is the world y axis.
        assert!(near(storey.world[1], -1.0) && near(storey.world[4], 1.0));
        let wall = object(&v, 70).placement.clone().unwrap();
        assert!(near(wall.world[3], 1.0) && near(wall.world[7], 7.1) && near(wall.world[11], 3.0));
        let window = object(&v, 92).placement.clone().unwrap();
        assert_eq!(
            window.state,
            PlacementState::NotResolved("IFCGRIDPLACEMENT".to_owned())
        );
        assert!(object(&v, 40).placement.is_none());
        assert!(near(object(&v, 33).elevation.unwrap(), 3.0));
        let (lat, lon) = object(&v, 31).latitude_longitude.unwrap();
        assert!((lat - 50.500_138_888_9).abs() < 1e-9);
        assert!((lon + 8.258_333_333_3).abs() < 1e-9);
        assert!(near(object(&v, 31).elevation.unwrap(), 0.0125));
    }
}

#[test]
fn objects_carry_attributes_types_and_predefined_types() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let a = object(&v, 70);
        assert_eq!(a.kind, ObjectKind::Product);
        assert_eq!(a.name.as_deref(), Some("Wall A"));
        assert_eq!(a.description.as_deref(), Some("First wall"));
        assert_eq!(a.object_type.as_deref(), Some("Custom"));
        assert_eq!(a.tag.as_deref(), Some("T1"));
        assert_eq!(a.type_object, Some(40));
        assert_eq!(a.container, Some(33));
        assert_eq!(a.owner_history, Some(5));
        // NOTDEFINED (or absent in IFC2X3) on the occurrence: the type's value applies.
        assert_eq!(a.effective_predefined_type.as_deref(), Some(wall_type(f)));
        let t = object(&v, 40);
        assert_eq!(t.kind, ObjectKind::Type);
        assert_eq!(t.object_type.as_deref(), Some("Brick"));
        assert_eq!(t.predefined_type.as_deref(), Some(wall_type(f)));
        let b = object(&v, 71);
        if f == Flavor::X23 {
            assert_eq!(b.effective_predefined_type.as_deref(), Some(wall_type(f)));
            assert_eq!(
                object(&v, 96).effective_predefined_type.as_deref(),
                Some("NOTDEFINED")
            );
        } else {
            // The type says SOLIDWALL, which wins over the occurrence's USERDEFINED.
            assert_eq!(b.effective_predefined_type.as_deref(), Some("SOLIDWALL"));
            assert_eq!(b.user_defined_type, None);
            // The type says NOTDEFINED: the occurrence's USERDEFINED and its `ObjectType` stand.
            let c = object(&v, 96);
            assert_eq!(c.effective_predefined_type.as_deref(), Some("USERDEFINED"));
            assert_eq!(c.user_defined_type.as_deref(), Some("UD"));
        }
        let o = &v.import.owner_histories[0];
        assert_eq!(o.state.as_deref(), Some("READWRITE"));
        assert_eq!(o.user.as_deref(), Some("Jane Doe"));
        assert_eq!(o.organization.as_deref(), Some("Org"));
        assert_eq!(o.application.as_deref(), Some("App"));
        assert_eq!(o.created, Some(1_700_000_000));
    }
}

#[test]
fn property_and_quantity_sets_merge_with_the_type() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let a = object(&v, 70);
        let set = a
            .property_sets
            .iter()
            .find(|s| s.name.as_deref() == Some("Pset_WallCommon"))
            .unwrap();
        let get = |name: &str| {
            set.properties
                .iter()
                .find(|p| p.property.name.as_deref() == Some(name))
                .unwrap()
        };
        // The occurrence's IsExternal wins; FireRating and Thickness come from the type.
        let external = get("IsExternal");
        assert_eq!(external.origin, Origin::Occurrence);
        assert!(
            matches!(&external.property.value, PropertyValue::Single { nominal: Some(n), .. } if n.value == Value::Enumeration("F".into()))
        );
        let fire = get("FireRating");
        assert_eq!(fire.origin, Origin::Type);
        assert!(
            matches!(&fire.property.value, PropertyValue::Single { nominal, .. } if text_of(nominal).as_deref() == Some("2HR"))
        );
        assert_eq!(get("LoadBearing").origin, Origin::Occurrence);
        // A length without its own unit is in the project's: 200 millimeters.
        assert!(near(get("Thickness").si_numbers()[0], 0.2));
        let q = &a.quantity_sets[0];
        assert_eq!(q.quantities.len(), 3);
        let width = q
            .quantities
            .iter()
            .find(|x| x.quantity.name.as_deref() == Some("Width"))
            .unwrap();
        assert_eq!(width.origin, Origin::Type);
        assert!(near(width.si_value.unwrap(), 0.2));
        // Wall B has only the type's sets.
        let b = object(&v, 71);
        assert_eq!(b.property_sets[0].properties.len(), 3);
        assert!(
            b.property_sets[0]
                .properties
                .iter()
                .all(|p| p.origin == Origin::Type)
        );
        // The type object carries its own sets.
        assert_eq!(object(&v, 40).property_sets.len(), 1);
    }
}

#[test]
fn materials_layers_lists_and_inheritance() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let (material, origin) = object(&v, 70).material.clone().unwrap();
        assert_eq!(origin, Origin::Occurrence);
        let MaterialAssociation::LayerSet {
            name,
            layers,
            usage,
            ..
        } = material
        else {
            panic!("layer set");
        };
        assert_eq!(name.as_deref(), Some("Wall build-up"));
        assert_eq!(layers.len(), 2);
        assert!(near(layers[0].thickness.unwrap(), 0.2) && near(layers[1].thickness.unwrap(), 0.1));
        assert_eq!(
            layers[0].material.as_ref().unwrap().name.as_deref(),
            Some("Brick")
        );
        assert_eq!(layers[0].ventilated, Some(false));
        assert_eq!(layers[1].ventilated, None);
        let usage = usage.unwrap();
        assert_eq!(usage.direction.as_deref(), Some("AXIS2"));
        assert_eq!(usage.sense.as_deref(), Some("POSITIVE"));
        assert!(near(usage.offset.unwrap(), -0.15));
        if f != Flavor::X23 {
            assert_eq!(layers[0].name.as_deref(), Some("Core"));
            assert_eq!(layers[0].priority, Some(1));
            assert_eq!(
                layers[0].material.as_ref().unwrap().category.as_deref(),
                Some("masonry")
            );
        }
        // Wall B: a list. The type's single material is inherited by nothing else.
        let (list, _) = object(&v, 71).material.clone().unwrap();
        assert!(matches!(list, MaterialAssociation::List(m) if m.len() == 2));
        let (single, origin) = object(&v, 40).material.clone().unwrap();
        assert_eq!(origin, Origin::Occurrence);
        let MaterialAssociation::Single(brick) = single else {
            panic!("single")
        };
        // The material's style comes through its definition representation.
        assert_eq!(brick.styles.len(), 1);
        assert_eq!(brick.styles[0].color, Some([0.8, 0.2, 0.1]));
    }
}

#[test]
fn classification_of_the_type_is_overridden_per_system() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let a = &object(&v, 70).classifications;
        assert_eq!(a.len(), 1, "{}", f.name());
        assert_eq!(a[0].identification.as_deref(), Some("Pr_20"));
        assert_eq!(
            a[0].system.as_ref().unwrap().name.as_deref(),
            Some("Uniclass 2015")
        );
        assert_eq!(
            a[0].system.as_ref().unwrap().edition.as_deref(),
            Some("2015")
        );
        let b = &object(&v, 71).classifications;
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].identification.as_deref(), Some("Pr_20_10"));
        assert_eq!(b[0].origin, Origin::Type);
    }
}

#[test]
fn styles_of_representation_items() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let styles = &object(&v, 70).styles;
        assert_eq!(styles.len(), 1, "{}", f.name());
        assert_eq!(styles[0].color, Some([0.8, 0.2, 0.1]));
        assert_eq!(styles[0].transparency, Some(0.25));
        assert_eq!(styles[0].name.as_deref(), Some("Red"));
        assert!(object(&v, 71).styles.is_empty());
    }
}

#[test]
fn openings_fills_aggregation_and_nesting() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        assert_eq!(object(&v, 70).openings, vec![91]);
        assert_eq!(object(&v, 91).voids, Some(70));
        assert_eq!(object(&v, 91).filled_by, vec![92]);
        assert_eq!(object(&v, 92).fills, Some(91));
        assert_eq!(object(&v, 70).parts, vec![130]);
        assert_eq!(object(&v, 130).aggregate_parent, Some(70));
        assert_eq!(object(&v, 130).nested, vec![131]);
        assert_eq!(object(&v, 131).nest_parent, Some(130));
        assert_eq!(object(&v, 34).aggregate_parent, Some(33));
    }
}

#[test]
fn connections_boundaries_and_groups() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let c = &v.import.connections;
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].relating, c[0].related), (70, 71));
        assert_eq!(c[0].relating_type.as_deref(), Some("ATEND"));
        assert_eq!(c[0].related_type.as_deref(), Some("ATSTART"));
        assert_eq!(object(&v, 70).connections, vec![0]);
        let b = &v.import.space_boundaries;
        if f == Flavor::X23 {
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].level, 0);
        } else {
            assert_eq!(b.iter().map(|x| x.level).collect::<Vec<_>>(), vec![1, 2, 2]);
            assert_eq!(b[1].parent, Some(141));
            assert_eq!(b[2].corresponding, Some(142));
            assert_eq!(b[1].internal.as_deref(), Some("EXTERNAL"));
        }
        assert_eq!((b[0].space, b[0].element), (34, Some(70)));
        assert_eq!(b[0].physical.as_deref(), Some("PHYSICAL"));
        assert!(!object(&v, 34).space_boundaries.is_empty());
        let group = v.import.group(150).unwrap();
        assert_eq!(group.members, vec![70, 71]);
        assert_eq!(group.name.as_deref(), Some("Group"));
        assert_eq!(object(&v, 70).groups, vec![150]);
        assert_eq!(v.import.group(151).unwrap().members, vec![34]);
    }
}

#[test]
fn georeference_by_map_conversion_or_property_set() {
    for f in ALL {
        let Some((_, v)) = load(f) else { continue };
        let g = v.import.georeference.as_ref().unwrap();
        assert_eq!(
            g.source,
            if f == Flavor::X23 {
                GeoSource::PropertySet
            } else {
                GeoSource::MapConversion
            }
        );
        assert_eq!(
            (g.eastings, g.northings, g.orthogonal_height),
            (1000.0, 2000.0, 3.0)
        );
        assert_eq!(
            (g.x_axis_abscissa, g.x_axis_ordinate, g.scale),
            (0.8, 0.6, 1.0)
        );
        assert_eq!(g.crs.as_ref().unwrap().name.as_deref(), Some("EPSG:25832"));
        if f != Flavor::X23 {
            assert_eq!(
                g.crs.as_ref().unwrap().map_projection.as_deref(),
                Some("UTM")
            );
            assert!(near(g.crs.as_ref().unwrap().map_unit_to_meters, 1.0));
        }
    }
}

#[test]
fn coverage_lists_every_class() {
    for f in ALL {
        let Some((file, v)) = load(f) else { continue };
        let total: u64 = v.import.coverage.iter().map(|c| c.total).sum();
        assert_eq!(total, file.instances().len() as u64);
        for c in &v.import.coverage {
            assert_eq!(
                c.total,
                c.interpreted + c.partial + c.preserved,
                "{}",
                c.type_name
            );
        }
        let class = |name: &str| {
            v.import
                .coverage
                .iter()
                .find(|c| c.type_name == name)
                .unwrap()
        };
        assert_eq!(class("IFCWALL").interpreted, 3);
        assert_eq!(class("IFCSIUNIT").preserved, 0);
        // Nothing reads a document reference: it is kept as it is.
        assert_eq!(class("IFCDOCUMENTREFERENCE").preserved, 1);
        assert_eq!(class("IFCDOCUMENTREFERENCE").interpreted, 0);
    }
}

#[test]
fn a_file_without_tables_has_only_coverage() {
    let text = model(Flavor::X4)
        .replace("'IFC4'", "'IFC9'")
        .replace("FILE_SCHEMA(('IFC4'))", "FILE_SCHEMA(('IFC9'))");
    let file = read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file;
    let v = file.typed_views(&Limits::DEFAULT, &Cancel::new()).unwrap();
    assert!(v.import.objects.is_empty());
    let units = v
        .import
        .coverage
        .iter()
        .find(|c| c.type_name == "IFCSIUNIT")
        .unwrap();
    assert_eq!(units.preserved, units.total);
}

#[test]
fn views_are_bounded_and_cancellable() {
    let Some((file, _)) = load(Flavor::X4) else {
        return;
    };
    let cancel = Cancel::new();
    cancel.cancel();
    assert!(file.typed_views(&Limits::DEFAULT, &cancel).is_err());
    let mut limits = Limits::DEFAULT;
    limits.record_count = 50;
    assert!(file.typed_views(&limits, &Cancel::new()).is_err());
    let mut limits = Limits::DEFAULT;
    limits.memory_bytes = Some(file.parsed_bytes() + 2_000);
    assert!(file.typed_views(&limits, &Cancel::new()).is_err());
}

/// A file of the given data lines in IFC4, and its views.
fn hostile(data: &str, limits: &Limits) -> Option<TypedViews> {
    let text = format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('h.ifc','',(''),(''),'','','');\n\
FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n{data}\nENDSEC;\nEND-ISO-10303-21;\n"
    );
    let file = read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file;
    file.tables()?;
    Some(file.typed_views(limits, &Cancel::new()).unwrap())
}

#[test]
fn hostile_placements_units_and_references_are_reported_not_followed() {
    let mut data = String::new();
    // Two placements that refer to each other, and a chain longer than the nesting limit.
    data += "#1=IFCLOCALPLACEMENT(#2,#9);\n#2=IFCLOCALPLACEMENT(#1,#9);\n#9=IFCAXIS2PLACEMENT3D(#8,$,$);\n#8=IFCCARTESIANPOINT((1.,0.,0.));\n";
    for i in 0..200_u32 {
        let prev = if i == 0 {
            "$".to_owned()
        } else {
            format!("#{}", 100 + i - 1)
        };
        data += &format!("#{}=IFCLOCALPLACEMENT({prev},#9);\n", 100 + i);
    }
    data += "#400=IFCWALL('w1',$,'A',$,$,#1,$,$,$);\n#401=IFCWALL('w2',$,'B',$,$,#299,$,$,$);\n";
    // A conversion based unit that is its own unit component, and a project that uses it.
    data += "#500=IFCMEASUREWITHUNIT(IFCLENGTHMEASURE(2.),#501);\n#501=IFCCONVERSIONBASEDUNIT(#502,.LENGTHUNIT.,'LOOP',#500);\n#502=IFCDIMENSIONALEXPONENTS(1,0,0,0,0,0,0);\n";
    data += "#503=IFCUNITASSIGNMENT((#501));\n#504=IFCPROJECT('p',$,'P',$,$,$,$,$,#503);\n";
    // Relationships to instances that do not exist, and a material that is not one.
    data += "#600=IFCRELVOIDSELEMENT('v',$,$,$,#400,#9999);\n#601=IFCRELASSOCIATESMATERIAL('m',$,$,$,(#400,#9998),#25);\n#25=IFCCARTESIANPOINT((0.,0.,0.));\n";
    data += "#602=IFCRELASSIGNSTOGROUP('g',$,$,$,(#400),$,#9997);\n#603=IFCRELNESTS('n',$,$,$,#400,(#400));\n";
    let mut limits = Limits::CONSTRAINED;
    limits.nesting_depth = 64;
    let Some(v) = hostile(&data, &limits) else {
        return;
    };
    let a = object(&v, 400).placement.clone().unwrap();
    assert_eq!(a.state, PlacementState::Cut);
    let b = object(&v, 401).placement.clone().unwrap();
    assert_eq!(b.state, PlacementState::Cut);
    // The unit conversion has no factor, so meters are assumed and said so.
    assert!(v.import.units.length_assumed);
    assert!(v.import.units.units[0].to_si.is_none());
    assert!(
        v.report
            .items
            .iter()
            .any(|i| i.reason == "io.ifc.unit_assumed")
    );
    assert!(
        v.report
            .items
            .iter()
            .any(|i| i.reason == "io.ifc.cycle_cut")
    );
    assert!(
        v.report
            .items
            .iter()
            .any(|i| i.reason == "io.ifc.target_missing")
    );
    assert!(
        v.report
            .items
            .iter()
            .any(|i| i.reason == "io.ifc.not_mapped")
    );
    assert!(object(&v, 400).openings.is_empty() && object(&v, 400).groups.is_empty());
}
