//! Fixtures composed in the tests: physical files as string literals, and ZIP archives written
//! byte by byte.

#![allow(dead_code, reason = "each test file uses part of the fixtures")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "test data"
)]

use arc_io_common::crc32::checksum;

/// The minimal project in the given schema identifier: a project, a site, a building, two
/// storeys, a space, two walls with a type, a property set with one property of each kind, and an
/// element quantity set with one quantity of each kind. The body uses attribute positions that
/// IFC2X3, IFC4 and IFC4X3 share for every attribute the typed views read.
pub fn project(schema: &str) -> String {
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition []'),'2;1');\n\
FILE_NAME('project.ifc','2026-01-01T00:00:00',(''),(''),'','','');\n\
FILE_SCHEMA(('{schema}'));\nENDSEC;\nDATA;\n{BODY}ENDSEC;\nEND-ISO-10303-21;\n"
    )
}

const BODY: &str = "\
#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Demo Project','Project description',$,$,$,$,$);
#2=IFCSITE('1Xk3kqQ3T1A8E0J5sG9Uy0',$,'Site A',$,$,$,$,$,$,$,$,$,$,$);
#3=IFCBUILDING('2Hb4dC2sD0mW1nKxT9z0dP',$,'Building A',$,$,$,$,$,$,$,$,$);
#4=IFCBUILDINGSTOREY('3Gf5dE3tE1nX2pLyU0A1eQ',$,'Ground',$,$,$,$,$,$,0.);
#5=IFCBUILDINGSTOREY('4Hg6eF4uF2oY3qMzV1B2fR',$,'First',$,$,$,$,$,$,3000.);
#6=IFCSPACE('5Jh7fG5vG3pZ4rNaW2C3gS',$,'Room 1',$,$,$,$,$,$,$,$,$,$,$);
#7=IFCWALLTYPE('6Ki8gH6wH4qA5sObX3D4hT',$,'Brick wall',$,$,$,$,$,$,.NOTDEFINED.);
#8=IFCWALL('7Lj9hJ7xJ5rB6tPcY4E5iU',$,'Wall A','North elevation',$,$,$,$,$);
#9=IFCWALL('8Mk0iK8yK6sC7uQdZ5F6jV',$,'Wall B',$,$,$,$,$,$);
#10=IFCRELAGGREGATES('9Nl1jL9zL7tD8vReA6G7kW',$,$,$,#1,(#2));
#11=IFCRELAGGREGATES('AOm2kM0AM8uE9wSfB7H8lX',$,$,$,#2,(#3));
#12=IFCRELAGGREGATES('BPn3lN1BN9vF0xTgC8I9mY',$,$,$,#3,(#4,#5));
#13=IFCRELAGGREGATES('CQo4mO2CO0wG1yUhD9J0nZ',$,$,$,#4,(#6));
#14=IFCRELCONTAINEDINSPATIALSTRUCTURE('DRp5nP3DP1xH2zViE0K1oA',$,$,$,(#8,#9),#5);
#15=IFCRELDEFINESBYTYPE('ESq6oQ4EQ2yI3AWjF1L2pB',$,$,$,(#8,#9),#7);
#16=IFCPROPERTYSET('FTr7pR5FR3zJ4BXkG2M3qC',$,'Pset_WallCommon',$,(#20,#21,#22,#23,#24));
#17=IFCRELDEFINESBYPROPERTIES('GUs8qS6GS4AK5CYlH3N4rD',$,$,$,(#8,#9),#16);
#18=IFCELEMENTQUANTITY('HVt9rT7HT5BL6DZmI4O5sE',$,'Qto_WallBaseQuantities',$,$,(#40,#41,#42,#43,#44,#45));
#19=IFCRELDEFINESBYPROPERTIES('IWu0sU8IU6CM7EAnJ5P6tF',$,$,$,(#8,#9),#18);
#20=IFCPROPERTYSINGLEVALUE('IsExternal',$,IFCBOOLEAN(.T.),$);
#21=IFCPROPERTYSINGLEVALUE('FireRating',$,IFCLABEL('2HR'),$);
#22=IFCPROPERTYENUMERATEDVALUE('Finish',$,(IFCLABEL('Paint'),IFCLABEL('Tile')),$);
#23=IFCPROPERTYLISTVALUE('Layers',$,(IFCREAL(1.5),IFCREAL(2.5)),$);
#24=IFCPROPERTYBOUNDEDVALUE('Thickness',$,IFCREAL(300.),IFCREAL(100.),#30,IFCREAL(200.));
#30=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);
#40=IFCQUANTITYLENGTH('Length',$,#30,IFCLENGTHMEASURE(2500.),$);
#41=IFCQUANTITYAREA('GrossArea',$,$,IFCAREAMEASURE(12.5),$);
#42=IFCQUANTITYVOLUME('Volume',$,$,IFCVOLUMEMEASURE(1.25),$);
#43=IFCQUANTITYCOUNT('Count',$,$,IFCCOUNTMEASURE(4),$);
#44=IFCQUANTITYWEIGHT('Weight',$,$,IFCMASSMEASURE(250.),$);
#45=IFCQUANTITYTIME('Time',$,$,IFCTIMEMEASURE(3600.),$);
";

/// The CRC-32 and size fields of an archive entry.
fn le16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn le32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// A ZIP archive of stored entries: `(name, data)`. A name that ends in `/` with empty data is a
/// directory entry. The archive is valid: CRCs and sizes are correct.
pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = checksum(data);
        let size = data.len() as u32;
        le32(&mut out, 0x0403_4B50);
        le16(&mut out, 20);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le16(&mut out, 0);
        le32(&mut out, crc);
        le32(&mut out, size);
        le32(&mut out, size);
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        le32(&mut central, 0x0201_4B50);
        le16(&mut central, 20);
        le16(&mut central, 20);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le32(&mut central, crc);
        le32(&mut central, size);
        le32(&mut central, size);
        le16(&mut central, name.len() as u16);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le16(&mut central, 0);
        le32(&mut central, 0);
        le32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    le32(&mut out, 0x0605_4B50);
    le16(&mut out, 0);
    le16(&mut out, 0);
    le16(&mut out, entries.len() as u16);
    le16(&mut out, entries.len() as u16);
    le32(&mut out, central_size);
    le32(&mut out, central_offset);
    le16(&mut out, 0);
    out
}
