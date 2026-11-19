//! Geometry evaluation (CORE-IO-015) over hand-written IFC fixtures composed in the tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::float_cmp,
    reason = "test code: fixtures and assertions"
)]

use std::sync::Arc;

use arc_base::math::Point3;
use arc_base::{Cancel, Limits};
use arc_io_ifc::{Geometry, GeometryItem, ReadOptions, Shape, read, reasons};
use arc_mesh::measure::{bounds, component_topology, volume};

const HEADER: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition []'),'2;1');\n\
FILE_NAME('geometry.ifc','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n";

const MILLIMETERS: &str = "#100=IFCUNITASSIGNMENT((#101,#102));\n\
#101=IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.);\n\
#102=IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.);\n";

/// A file of the header, the body and the end marker.
fn file(body: &str) -> String {
    format!("{HEADER}{body}ENDSEC;\nEND-ISO-10303-21;\n")
}

/// The file with a project whose unit assignment is `#100`, so that lengths are millimeters.
fn project(body: &str) -> String {
    file(&format!(
        "#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Demo',$,$,$,$,$,#100);\n{MILLIMETERS}{body}"
    ))
}

fn evaluate(text: &str) -> Geometry {
    evaluate_with(text, &Limits::CONSTRAINED)
}

fn evaluate_with(text: &str, limits: &Limits) -> Geometry {
    let output = read(text.as_bytes(), &mut ReadOptions::with_limits(*limits)).expect("read");
    output
        .file
        .geometry(limits, &Cancel::new())
        .expect("geometry")
}

fn mesh_of(item: &GeometryItem) -> &arc_mesh::TriMesh {
    match item.shape.as_ref() {
        Shape::Mesh(mesh) => mesh,
        Shape::Polylines(_) => panic!("expected a mesh"),
    }
}

/// The volume of an item in world coordinates, with reflections handled.
fn world_volume(item: &GeometryItem) -> f64 {
    let mesh = item
        .world_mesh(&Limits::CONSTRAINED)
        .unwrap()
        .expect("mesh");
    volume(&mesh, &Limits::CONSTRAINED).unwrap()
}

/// The area of the triangles of an item in world coordinates, in square meters.
fn surface_area(item: &GeometryItem) -> f64 {
    let mesh = item
        .world_mesh(&Limits::CONSTRAINED)
        .unwrap()
        .expect("mesh");
    mesh.triangles()
        .iter()
        .map(|t| {
            let p = t.map(|i| mesh.positions()[i as usize]);
            (p[1] - p[0]).cross(&(p[2] - p[0])).norm() / 2.0
        })
        .sum()
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not {expected} within {tolerance}"
    );
}

fn loss_count(geometry: &Geometry, reason: &str, kind: &str) -> i64 {
    geometry
        .report
        .items
        .iter()
        .filter(|item| item.reason == reason && item.record_kind.as_deref() == Some(kind))
        .map(|item| match item.arguments.get("count") {
            Some(arc_io_ifc::loss::Argument::Int(count)) => *count,
            _ => 0,
        })
        .sum()
}

/// A product with a placement and a rectangle extrusion, under a storey and a building.
/// The world box is x 500..1500, y 1000..3000, z 3000..6000 mm.
const PLACED_BOX: &str = "\
#10=IFCLOCALPLACEMENT($,#11);
#11=IFCAXIS2PLACEMENT3D(#12,$,$);
#12=IFCCARTESIANPOINT((0.,0.,3000.));
#20=IFCLOCALPLACEMENT(#10,#21);
#21=IFCAXIS2PLACEMENT3D(#22,#23,#24);
#22=IFCCARTESIANPOINT((1000.,0.,0.));
#23=IFCDIRECTION((0.,0.,1.));
#24=IFCDIRECTION((0.,1.,0.));
#30=IFCLOCALPLACEMENT(#20,#31);
#31=IFCAXIS2PLACEMENT3D(#32,$,$);
#32=IFCCARTESIANPOINT((2000.,0.,0.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,3000.);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,2000.,1000.);
#62=IFCDIRECTION((0.,0.,1.));
#50=IFCBUILDINGELEMENTPROXY('box0000000000000000000A',$,'Box',$,$,#30,#40,$,$);
";

#[test]
fn extruded_rectangle_in_a_placement_chain_is_in_meters_with_outward_winding() {
    let geometry = evaluate(&project(PLACED_BOX));
    assert_eq!(geometry.items.len(), 1);
    assert_eq!(geometry.units.meters_per_length_unit, 0.001);
    assert!(!geometry.units.length_assumed);
    let item = &geometry.items[0];
    assert_eq!(item.product, 50);
    assert_eq!(item.item, 60);
    assert_eq!(item.representation_identifier.as_deref(), Some("Body"));
    assert_eq!(item.representation_type.as_deref(), Some("SweptSolid"));

    let mesh = item
        .world_mesh(&Limits::CONSTRAINED)
        .unwrap()
        .expect("mesh");
    let world = bounds(&mesh);
    assert_close(world.min().x, 0.5, 1e-9);
    assert_close(world.max().x, 1.5, 1e-9);
    assert_close(world.min().y, 1.0, 1e-9);
    assert_close(world.max().y, 3.0, 1e-9);
    assert_close(world.min().z, 3.0, 1e-9);
    assert_close(world.max().z, 6.0, 1e-9);
    assert_close(volume(&mesh, &Limits::CONSTRAINED).unwrap(), 6.0, 1e-9);
    assert!(
        geometry.report.items.is_empty(),
        "{:?}",
        geometry.report.items
    );
}

#[test]
fn circle_profile_volume_is_within_the_chord_tolerance_of_the_cylinder() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCCIRCLEPROFILEDEF(.AREA.,$,$,500.);
#62=IFCDIRECTION((0.,0.,1.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('cyl0000000000000000000A',$,'Cyl',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    let volume_m3 = world_volume(&geometry.items[0]);
    let exact = std::f64::consts::PI * 0.5 * 0.5 * 1.0;
    let chord = arc_base::Tolerance::DEFAULT.tessellation_chord;
    let perimeter = 2.0 * std::f64::consts::PI * 0.5;
    assert!(volume_m3 < exact && exact - volume_m3 <= perimeter * chord);
}

#[test]
fn arbitrary_profile_with_a_void_has_the_volume_of_outer_minus_inner() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCARBITRARYPROFILEDEFWITHVOIDS(.AREA.,$,#70,(#80));
#70=IFCPOLYLINE((#71,#72,#73,#74,#71));
#71=IFCCARTESIANPOINT((0.,0.));
#72=IFCCARTESIANPOINT((2000.,0.));
#73=IFCCARTESIANPOINT((2000.,2000.));
#74=IFCCARTESIANPOINT((0.,2000.));
#80=IFCPOLYLINE((#81,#82,#83,#84,#81));
#81=IFCCARTESIANPOINT((500.,500.));
#82=IFCCARTESIANPOINT((1500.,500.));
#83=IFCCARTESIANPOINT((1500.,1500.));
#84=IFCCARTESIANPOINT((500.,1500.));
#62=IFCDIRECTION((0.,0.,1.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('void000000000000000000A',$,'Void',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert_close(world_volume(&geometry.items[0]), 3.0, 1e-9);
}

#[test]
fn triangulated_face_set_tetrahedron_has_its_volume_and_assumed_units() {
    let body = "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#60));
#60=IFCTRIANGULATEDFACESET(#61,$,.T.,((1,3,2),(1,2,4),(1,4,3),(2,3,4)),$);
#61=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.),(0.,0.,1.)));
#50=IFCBUILDINGELEMENTPROXY('tetra000000000000000000A',$,'Tetra',$,$,$,#40,$,$);
";
    let geometry = evaluate(&file(body));
    assert_eq!(geometry.items.len(), 1);
    assert_close(world_volume(&geometry.items[0]), 1.0 / 6.0, 1e-12);
    assert!(geometry.units.length_assumed);
    assert_eq!(geometry.report.items.len(), 1);
    assert_eq!(geometry.report.items[0].reason, reasons::UNIT_ASSUMED.code);
}

/// A closed box of unit edge with a square pocket of side 0.5 m in its top face, half a metre deep.
/// Its volume is 1 - 0.125 = 0.875 m3. Points 1 to 8 are the box, 9 to 12 the pocket's top corners
/// and 13 to 16 its floor corners.
const POCKETED_CUBE: &str = "\
#61=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(1.,1.,0.),(0.,1.,0.),\
(0.,0.,1.),(1.,0.,1.),(1.,1.,1.),(0.,1.,1.),\
(0.25,0.25,1.),(0.75,0.25,1.),(0.75,0.75,1.),(0.25,0.75,1.),\
(0.25,0.25,0.5),(0.75,0.25,0.5),(0.75,0.75,0.5),(0.25,0.75,0.5)));
#1=IFCINDEXEDPOLYGONALFACE((1,4,3,2));
#2=IFCINDEXEDPOLYGONALFACE((1,2,6,5));
#3=IFCINDEXEDPOLYGONALFACE((2,3,7,6));
#4=IFCINDEXEDPOLYGONALFACE((3,4,8,7));
#5=IFCINDEXEDPOLYGONALFACE((4,1,5,8));
#6=IFCINDEXEDPOLYGONALFACEWITHVOIDS((5,6,7,8),((9,12,11,10)));
#7=IFCINDEXEDPOLYGONALFACE((9,10,14,13));
#8=IFCINDEXEDPOLYGONALFACE((10,11,15,14));
#9=IFCINDEXEDPOLYGONALFACE((11,12,16,15));
#10=IFCINDEXEDPOLYGONALFACE((12,9,13,16));
";

#[test]
fn polygonal_face_set_with_a_void_is_a_closed_solid_of_the_expected_volume() {
    let body = format!(
        "{POCKETED_CUBE}#11=IFCINDEXEDPOLYGONALFACE((13,14,15,16));\n\
#62=IFCPOLYGONALFACESET(#61,.T.,(#1,#2,#3,#4,#5,#6,#7,#8,#9,#10,#11),$);\n\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));\n#41=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#62));\n\
#50=IFCBUILDINGELEMENTPROXY('cube0000000000000000000A',$,'Cube',$,$,$,#40,$,$);\n"
    );
    let geometry = evaluate(&file(&body));
    assert_eq!(geometry.items.len(), 1);
    let mesh = geometry.items[0]
        .world_mesh(&Limits::CONSTRAINED)
        .unwrap()
        .unwrap();
    assert_close(volume(&mesh, &Limits::CONSTRAINED).unwrap(), 0.875, 1e-12);
    assert_closed(mesh_of(&geometry.items[0]));
}

/// Every directed edge of the mesh has an opposite edge: the surface is closed.
fn assert_closed(mesh: &arc_mesh::TriMesh) {
    let mut directed = std::collections::BTreeSet::new();
    for t in mesh.triangles() {
        for k in 0..3 {
            directed.insert((t[k], t[(k + 1) % 3]));
        }
    }
    // Weld by position so that faces meeting at a point count as sharing a vertex.
    let key = |i: u32| {
        let p = mesh.positions()[i as usize];
        [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
    };
    let mut welded = std::collections::BTreeSet::new();
    for (a, b) in &directed {
        welded.insert((key(*a), key(*b)));
    }
    for (a, b) in &welded {
        assert!(welded.contains(&(*b, *a)), "open edge");
    }
}

#[test]
fn faceted_brep_cube_is_closed_and_honors_a_reversed_bound() {
    let mut points = String::new();
    let corners = [
        (0.0, 0.0, 0.0),
        (1000.0, 0.0, 0.0),
        (1000.0, 1000.0, 0.0),
        (0.0, 1000.0, 0.0),
        (0.0, 0.0, 1000.0),
        (1000.0, 0.0, 1000.0),
        (1000.0, 1000.0, 1000.0),
        (0.0, 1000.0, 1000.0),
    ];
    for (k, (x, y, z)) in corners.iter().enumerate() {
        points.push_str(&format!(
            "#{}=IFCCARTESIANPOINT(({x:?},{y:?},{z:?}));\n",
            200 + k
        ));
    }
    // Each face is a loop of corner indices (0-based), counterclockwise from outside; the bottom
    // face is given reversed with Orientation .F., which is the same face.
    let faces: [([usize; 4], bool); 6] = [
        ([0, 3, 2, 1], true),
        ([0, 1, 5, 4], true),
        ([1, 2, 6, 5], true),
        ([2, 3, 7, 6], true),
        ([3, 0, 4, 7], true),
        ([4, 5, 6, 7], true),
    ];
    let mut face_names = Vec::new();
    let mut body = points;
    let mut next = 300;
    for (k, (loop_, outward)) in faces.iter().enumerate() {
        let loop_ = if k == 0 {
            loop_.iter().rev().copied().collect::<Vec<_>>()
        } else {
            loop_.to_vec()
        };
        let orientation = if k == 0 {
            ".F."
        } else if *outward {
            ".T."
        } else {
            ".F."
        };
        let refs: Vec<String> = loop_.iter().map(|i| format!("#{}", 200 + i)).collect();
        body.push_str(&format!("#{next}=IFCPOLYLOOP(({}));\n", refs.join(",")));
        body.push_str(&format!(
            "#{}=IFCFACEOUTERBOUND(#{next},{orientation});\n",
            next + 1
        ));
        body.push_str(&format!("#{}=IFCFACE((#{}));\n", next + 2, next + 1));
        face_names.push(format!("#{}", next + 2));
        next += 3;
    }
    body.push_str(&format!(
        "#150=IFCCLOSEDSHELL(({}));\n",
        face_names.join(",")
    ));
    body.push_str(
        "#151=IFCFACETEDBREP(#150);\n#152=IFCSHAPEREPRESENTATION($,'Body','Brep',(#151));\n",
    );
    body.push_str("#153=IFCPRODUCTDEFINITIONSHAPE($,$,(#152));\n");
    body.push_str(
        "#154=IFCBUILDINGELEMENTPROXY('brep0000000000000000000A',$,'Brep',$,$,$,#153,$,$);\n",
    );
    let geometry = evaluate(&project(&body));
    assert_eq!(geometry.items.len(), 1);
    assert_close(world_volume(&geometry.items[0]), 1.0, 1e-12);
    assert_closed(mesh_of(&geometry.items[0]));
    assert!(
        geometry.report.items.is_empty(),
        "{:?}",
        geometry.report.items
    );
}

/// Three products that map one representation map of a 1 m box: one translated, one scaled by two
/// in x through a non-uniform operator, and one mirrored in z through its third axis (the axes are
/// x, y and -z, so the operator is a reflection with a determinant of -1).
const MAPPED: &str = "\
#70=IFCREPRESENTATIONMAP(#71,#72);
#71=IFCAXIS2PLACEMENT3D(#73,$,$);
#73=IFCCARTESIANPOINT((0.,0.,0.));
#72=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#74));
#74=IFCEXTRUDEDAREASOLID(#75,$,#76,1000.);
#75=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#76=IFCDIRECTION((0.,0.,1.));
#80=IFCMAPPEDITEM(#70,#81);
#81=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,#82,$,$);
#82=IFCCARTESIANPOINT((5000.,0.,0.));
#83=IFCMAPPEDITEM(#70,#84);
#84=IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM($,$,#85,2.,$,1.,1.);
#85=IFCCARTESIANPOINT((0.,3000.,0.));
#86=IFCMAPPEDITEM(#70,#87);
#87=IFCCARTESIANTRANSFORMATIONOPERATOR3D(#122,#121,#88,1.,#120);
#88=IFCCARTESIANPOINT((0.,0.,0.));
#120=IFCDIRECTION((0.,0.,-1.));
#121=IFCDIRECTION((0.,1.,0.));
#122=IFCDIRECTION((1.,0.,0.));
#90=IFCPRODUCTDEFINITIONSHAPE($,$,(#91));
#91=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#80));
#92=IFCPRODUCTDEFINITIONSHAPE($,$,(#93));
#93=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#83));
#94=IFCPRODUCTDEFINITIONSHAPE($,$,(#95));
#95=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#86));
#200=IFCBUILDINGELEMENTPROXY('map1000000000000000000A',$,'A',$,$,$,#90,$,$);
#201=IFCBUILDINGELEMENTPROXY('map2000000000000000000A',$,'B',$,$,$,#92,$,$);
#202=IFCBUILDINGELEMENTPROXY('map3000000000000000000A',$,'C',$,$,$,#94,$,$);
";

#[test]
fn mapped_geometry_is_shared_and_each_placement_is_correct() {
    let geometry = evaluate(&project(MAPPED));
    assert_eq!(geometry.items.len(), 3);
    let [a, b, c] = geometry.items.as_slice() else {
        panic!("three items");
    };
    assert!(
        Arc::ptr_eq(&a.shape, &b.shape),
        "the mapped shape is shared"
    );
    assert!(
        Arc::ptr_eq(&a.shape, &c.shape),
        "the mapped shape is shared"
    );

    let box_a = bounds(&a.world_mesh(&Limits::CONSTRAINED).unwrap().unwrap());
    assert_close(box_a.min().x, 4.5, 1e-9);
    assert_close(box_a.max().x, 5.5, 1e-9);
    assert_close(world_volume(a), 1.0, 1e-12);

    assert_close(world_volume(b), 2.0, 1e-12);
    let box_b = bounds(&b.world_mesh(&Limits::CONSTRAINED).unwrap().unwrap());
    assert_close(box_b.min().x, -1.0, 1e-9);
    assert_close(box_b.max().x, 1.0, 1e-9);
    assert_close(box_b.min().y, 2.5, 1e-9);
    assert_close(box_b.max().y, 3.5, 1e-9);

    // The mirror reverses the triangles; world_mesh restores outward winding.
    assert!(c.transform.matrix().fixed_view::<3, 3>(0, 0).determinant() < 0.0);
    assert_close(world_volume(c), 1.0, 1e-12);
    let box_c = bounds(&c.world_mesh(&Limits::CONSTRAINED).unwrap().unwrap());
    assert_close(box_c.min().z, -1.0, 1e-9);
    assert_close(box_c.max().z, 0.0, 1e-9);
    assert_eq!(a.item, 80);
}

#[test]
fn polyline_axis_representation_is_returned_as_meters() {
    let body = "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Axis','Curve3D',(#60));
#60=IFCPOLYLINE((#61,#62,#63));
#61=IFCCARTESIANPOINT((0.,0.,0.));
#62=IFCCARTESIANPOINT((1000.,0.,0.));
#63=IFCCARTESIANPOINT((1000.,2000.,0.));
#50=IFCBUILDINGELEMENTPROXY('axis0000000000000000000A',$,'Axis',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 1);
    let item = &geometry.items[0];
    assert_eq!(item.representation_identifier.as_deref(), Some("Axis"));
    let Shape::Polylines(lines) = item.shape.as_ref() else {
        panic!("polylines");
    };
    assert_eq!(lines.len(), 1);
    let points: &[Point3] = &lines[0];
    assert_eq!(points.len(), 3);
    assert_close(points[2].y, 2.0, 1e-12);
    assert!(item.world_mesh(&Limits::CONSTRAINED).unwrap().is_none());
}

#[test]
fn boolean_result_emits_nothing_and_is_reported_once_per_kind() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#62=IFCDIRECTION((0.,0.,1.));
#65=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#60,#66);
#66=IFCHALFSPACESOLID($,.T.);
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','Clipping',(#65));
#42=IFCPRODUCTDEFINITIONSHAPE($,$,(#43));
#43=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60,#65));
#50=IFCBUILDINGELEMENTPROXY('bool0000000000000000000A',$,'Bool',$,$,$,#40,$,$);
#51=IFCBUILDINGELEMENTPROXY('bool0000000000000000000B',$,'Bool2',$,$,$,#42,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(
        geometry.items.len(),
        1,
        "only the box of the second product"
    );
    assert_eq!(geometry.items[0].product, 51);
    // The boolean is one representation item, evaluated and reported once, whichever products use it.
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_NOT_EVALUATED.code,
            "IFCBOOLEANCLIPPINGRESULT"
        ),
        1
    );
}

#[test]
fn openings_are_reported_once_per_file_with_a_count() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#62=IFCDIRECTION((0.,0.,1.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('wall0000000000000000000A',$,'Wall',$,$,$,#40,$,$);
#51=IFCRELVOIDSELEMENT('op000000000000000000000A',$,$,$,#50,#52);
#52=IFCBUILDINGELEMENTPROXY('open000000000000000000A',$,'Opening',$,$,$,$,$,$);
#53=IFCRELVOIDSELEMENT('op000000000000000000000B',$,$,$,#50,#52);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 1);
    assert_eq!(
        loss_count(
            &geometry,
            reasons::OPENING_NOT_SUBTRACTED.code,
            "IFCRELVOIDSELEMENT"
        ),
        2
    );
}

#[test]
fn placement_cycle_is_cut_and_reported() {
    let body = "\
#10=IFCLOCALPLACEMENT(#11,#12);
#11=IFCLOCALPLACEMENT(#10,#12);
#12=IFCAXIS2PLACEMENT3D(#13,$,$);
#13=IFCCARTESIANPOINT((1000.,0.,0.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#62=IFCDIRECTION((0.,0.,1.));
#50=IFCBUILDINGELEMENTPROXY('cycle000000000000000000A',$,'Cycle',$,$,#10,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 1);
    assert_eq!(
        loss_count(&geometry, reasons::CYCLE_CUT.code, "IFCLOCALPLACEMENT"),
        1
    );
}

#[test]
fn mapped_representation_that_maps_itself_is_cut_and_reported() {
    let body = "\
#70=IFCREPRESENTATIONMAP(#71,#72);
#71=IFCAXIS2PLACEMENT3D(#73,$,$);
#73=IFCCARTESIANPOINT((0.,0.,0.));
#72=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#80));
#80=IFCMAPPEDITEM(#70,#81);
#81=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,#82,$,$);
#82=IFCCARTESIANPOINT((0.,0.,0.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#80));
#50=IFCBUILDINGELEMENTPROXY('self000000000000000000A',$,'Self',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(&geometry, reasons::CYCLE_CUT.code, "IFCMAPPEDITEM"),
        1
    );
}

#[test]
fn text_number_outside_the_real_range_is_refused_by_the_reader() {
    let body = "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Axis','Curve3D',(#60));
#60=IFCPOLYLINE((#61,#62));
#61=IFCCARTESIANPOINT((0.,0.,0.));
#62=IFCCARTESIANPOINT((1.E400,0.,0.));
#50=IFCBUILDINGELEMENTPROXY('nan000000000000000000000A',$,'Nan',$,$,$,#40,$,$);
";
    // The reader refuses the value before any geometry is evaluated, so no NaN reaches a point.
    let error = read(file(body).as_bytes(), &mut ReadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "io.ifc.value_invalid");
}

#[test]
fn index_out_of_range_skips_the_face_set() {
    let body = "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#60));
#60=IFCTRIANGULATEDFACESET(#61,$,.T.,((1,2,9)),$);
#61=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.)));
#50=IFCBUILDINGELEMENTPROXY('index000000000000000000A',$,'Index',$,$,$,#40,$,$);
";
    let geometry = evaluate(&file(body));
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(
            &geometry,
            reasons::INDEX_INVALID.code,
            "IFCTRIANGULATEDFACESET"
        ),
        1
    );
}

#[test]
fn coordinate_that_is_not_finite_after_conversion_is_skipped() {
    // Kilometers: 1.E306 is finite as a number, but 1.E306 km is not finite in meters.
    let body = "\
#100=IFCUNITASSIGNMENT((#101));
#101=IFCSIUNIT(*,.LENGTHUNIT.,.KILO.,.METRE.);
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Axis','Curve3D',(#60));
#60=IFCPOLYLINE((#61,#62));
#61=IFCCARTESIANPOINT((0.,0.,0.));
#62=IFCCARTESIANPOINT((1.E306,0.,0.));
#50=IFCBUILDINGELEMENTPROXY('nonfinite00000000000000A',$,'Big',$,$,$,#40,$,$);
";
    let text = file(&format!(
        "#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Demo',$,$,$,$,$,#100);\n{body}"
    ));
    let geometry = evaluate(&text);
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_DEGENERATE.code,
            "IFCCARTESIANPOINT"
        ),
        1
    );
}

#[test]
fn huge_circle_with_tiny_tolerance_uses_a_bounded_segment_count() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCCIRCLEPROFILEDEF(.AREA.,$,$,1.E9);
#62=IFCDIRECTION((0.,0.,1.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('circle00000000000000000A',$,'Circle',$,$,$,#40,$,$);
";
    let tolerance = arc_base::Tolerance {
        tessellation_chord: 1e-12,
        ..arc_base::Tolerance::DEFAULT
    };
    let output = read(
        project(body).as_bytes(),
        &mut ReadOptions::with_limits(Limits::CONSTRAINED),
    )
    .unwrap();
    let geometry = output
        .file
        .geometry_with_tolerance(&Limits::CONSTRAINED, &Cancel::new(), &tolerance)
        .unwrap();
    assert_eq!(geometry.items.len(), 1);
    assert_eq!(mesh_of(&geometry.items[0]).vertex_count(), 2 * 512);
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_APPROXIMATED.code,
            "IFCCIRCLEPROFILEDEF"
        ),
        1
    );
}

#[test]
fn declared_size_above_array_length_is_refused_before_reading() {
    let body = "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#60));
#60=IFCTRIANGULATEDFACESET(#61,$,.T.,((1,3,2),(1,2,4)),$);
#61=IFCCARTESIANPOINTLIST3D(((0.,0.,0.),(1.,0.,0.),(0.,1.,0.),(0.,0.,1.)));
#50=IFCBUILDINGELEMENTPROXY('array0000000000000000000A',$,'Array',$,$,$,#40,$,$);
";
    let output = read(file(body).as_bytes(), &mut ReadOptions::default()).unwrap();
    let limits = Limits {
        array_length: 3,
        ..Limits::CONSTRAINED
    };
    let error = output.file.geometry(&limits, &Cancel::new()).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert!(format!("{:?}", error.arguments()).contains("array_length"));
}

#[test]
fn memory_limit_below_the_output_is_an_error_naming_the_limit() {
    let output = read(
        project(PLACED_BOX).as_bytes(),
        &mut ReadOptions::with_limits(Limits::CONSTRAINED),
    )
    .unwrap();
    let limits = Limits {
        memory_bytes: Some(output.file.parsed_bytes() + 64),
        ..Limits::CONSTRAINED
    };
    let error = output.file.geometry(&limits, &Cancel::new()).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert!(format!("{:?}", error.arguments()).contains("memory_bytes"));
}

#[test]
fn cancelled_token_stops_the_evaluation() {
    let output = read(
        project(PLACED_BOX).as_bytes(),
        &mut ReadOptions::with_limits(Limits::CONSTRAINED),
    )
    .unwrap();
    let cancel = Cancel::new();
    cancel.cancel();
    let error = output
        .file
        .geometry(&Limits::CONSTRAINED, &cancel)
        .unwrap_err();
    assert_eq!(error.code(), "base.cancelled");
}

#[test]
fn skew_extrusion_has_the_volume_of_its_oblique_prism() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1414.2135623730951);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,#63,1000.,1000.);
#62=IFCDIRECTION((0.7071067811865476,0.,0.7071067811865476));
#63=IFCAXIS2PLACEMENT2D(#64,$);
#64=IFCCARTESIANPOINT((500.,500.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('skew0000000000000000000A',$,'Skew',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    // The base is 1 m by 1 m, centered at (0.5, 0.5); the displacement is (1, 0, 1) m: the height
    // perpendicular to the base is 1 m.
    assert_close(world_volume(&geometry.items[0]), 1.0, 1e-9);
    let mesh = geometry.items[0]
        .world_mesh(&Limits::CONSTRAINED)
        .unwrap()
        .unwrap();
    let world = bounds(&mesh);
    // The base spans x 0..1 m; the top is displaced by (1, 0, 1) m.
    assert_close(world.max().x, 2.0, 1e-9);
    assert_close(world.max().z, 1.0, 1e-9);
}

#[test]
fn polyline_keeps_a_repeated_point_in_its_order() {
    let body = "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Axis','Curve3D',(#60));
#60=IFCPOLYLINE((#61,#62,#61,#63));
#61=IFCCARTESIANPOINT((0.,0.,0.));
#62=IFCCARTESIANPOINT((1000.,0.,0.));
#63=IFCCARTESIANPOINT((1000.,2000.,0.));
#50=IFCBUILDINGELEMENTPROXY('repeat00000000000000000A',$,'Repeat',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    let Shape::Polylines(lines) = geometry.items[0].shape.as_ref() else {
        panic!("polylines");
    };
    // The point that comes back is a second place of the curve, not a duplicate to drop.
    assert_eq!(lines[0].len(), 4);
    assert_close(lines[0][2].x, lines[0][0].x, 0.0);
    assert_close(lines[0][3].y, 2.0, 1e-12);
}

#[test]
fn keyhole_face_loop_keeps_its_reused_points_and_has_the_area_of_outer_minus_hole() {
    // The outer square is 4 m and the hole 2 m, so the face has 16 - 4 = 12 square meters. The loop
    // walks the outer square, the bridge to the hole and the hole, and back over the bridge.
    let body = "\
#300=IFCCARTESIANPOINT((0.,0.,0.));
#301=IFCCARTESIANPOINT((4000.,0.,0.));
#302=IFCCARTESIANPOINT((4000.,4000.,0.));
#303=IFCCARTESIANPOINT((0.,4000.,0.));
#304=IFCCARTESIANPOINT((1000.,1000.,0.));
#305=IFCCARTESIANPOINT((1000.,3000.,0.));
#306=IFCCARTESIANPOINT((3000.,3000.,0.));
#307=IFCCARTESIANPOINT((3000.,1000.,0.));
#310=IFCPOLYLOOP((#300,#301,#302,#303,#300,#304,#305,#306,#307,#304));
#311=IFCFACEOUTERBOUND(#310,.T.);
#312=IFCFACE((#311));
#313=IFCOPENSHELL((#312));
#314=IFCSHELLBASEDSURFACEMODEL((#313));
#315=IFCSHAPEREPRESENTATION($,'Body','Surface',(#314));
#316=IFCPRODUCTDEFINITIONSHAPE($,$,(#315));
#317=IFCBUILDINGELEMENTPROXY('keyhole000000000000000A',$,'Key',$,$,$,#316,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 1);
    assert_close(surface_area(&geometry.items[0]), 12.0, 1e-9);
}

#[test]
fn loop_longer_than_the_vertex_cap_is_refused_before_it_is_reduced() {
    // 4097 references to one point: repeats are kept, so the loop is over the cap of 4096 points and
    // is not evaluated. Dropping the repeats first would leave a degenerate loop instead.
    let refs = vec!["#300"; 4097].join(",");
    let body = format!(
        "\
#300=IFCCARTESIANPOINT((0.,0.,0.));
#310=IFCPOLYLOOP(({refs}));
#311=IFCFACEOUTERBOUND(#310,.T.);
#312=IFCFACE((#311));
#313=IFCOPENSHELL((#312));
#314=IFCSHELLBASEDSURFACEMODEL((#313));
#315=IFCSHAPEREPRESENTATION($,'Body','Surface',(#314));
#316=IFCPRODUCTDEFINITIONSHAPE($,$,(#315));
#317=IFCBUILDINGELEMENTPROXY('longloop00000000000000A',$,'Long',$,$,$,#316,$,$);
"
    );
    let geometry = evaluate(&project(&body));
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_NOT_EVALUATED.code,
            "IFCPOLYLOOP"
        ),
        1
    );
}

#[test]
fn long_representation_identifier_is_charged_against_memory() {
    let identifier = "A".repeat(3000);
    let body = format!(
        "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'{identifier}','SweptSolid',(#60));
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#62=IFCDIRECTION((0.,0.,1.));
#50=IFCBUILDINGELEMENTPROXY('ident0000000000000000000A',$,'Ident',$,$,$,#40,$,$);
"
    );
    let output = read(
        file(&body).as_bytes(),
        &mut ReadOptions::with_limits(Limits::CONSTRAINED),
    )
    .unwrap();
    // Room for the parser and a little more: the 3000 bytes of the identifier do not fit.
    let limits = Limits {
        memory_bytes: Some(output.file.parsed_bytes() + 2000),
        ..Limits::CONSTRAINED
    };
    let error = output.file.geometry(&limits, &Cancel::new()).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
    assert!(format!("{:?}", error.arguments()).contains("memory_bytes"));
}

#[test]
fn hole_outside_the_outer_boundary_is_refused_and_reported() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCARBITRARYPROFILEDEFWITHVOIDS(.AREA.,$,#70,(#80));
#70=IFCPOLYLINE((#71,#72,#73,#74,#71));
#71=IFCCARTESIANPOINT((0.,0.));
#72=IFCCARTESIANPOINT((2000.,0.));
#73=IFCCARTESIANPOINT((2000.,2000.));
#74=IFCCARTESIANPOINT((0.,2000.));
#80=IFCPOLYLINE((#81,#82,#83,#81));
#81=IFCCARTESIANPOINT((5000.,5000.));
#82=IFCCARTESIANPOINT((6000.,5000.));
#83=IFCCARTESIANPOINT((6000.,6000.));
#62=IFCDIRECTION((0.,0.,1.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('outside0000000000000000A',$,'Outside',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_DEGENERATE.code,
            "IFCEXTRUDEDAREASOLID"
        ),
        1
    );
}

#[test]
fn profile_whose_loops_are_each_under_the_cap_but_not_in_total_is_not_evaluated() {
    // Two holes of 2100 references each: each is under the 4096-point cap, but the outer loop and
    // both holes, bridged twice, are 4209 points.
    let refs = vec!["#75"; 2100].join(",");
    let body = format!(
        "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCARBITRARYPROFILEDEFWITHVOIDS(.AREA.,$,#70,(#80,#81));
#70=IFCPOLYLINE((#71,#72,#73,#74,#71));
#71=IFCCARTESIANPOINT((0.,0.));
#72=IFCCARTESIANPOINT((2000.,0.));
#73=IFCCARTESIANPOINT((2000.,2000.));
#74=IFCCARTESIANPOINT((0.,2000.));
#75=IFCCARTESIANPOINT((500.,500.));
#80=IFCPOLYLINE(({refs}));
#81=IFCPOLYLINE(({refs}));
#62=IFCDIRECTION((0.,0.,1.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('total0000000000000000000A',$,'Total',$,$,$,#40,$,$);
"
    );
    let geometry = evaluate(&project(&body));
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_NOT_EVALUATED.code,
            "IFCARBITRARYPROFILEDEFWITHVOIDS"
        ),
        1
    );
}

#[test]
fn repeated_inner_curve_is_read_once_as_one_hole() {
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCARBITRARYPROFILEDEFWITHVOIDS(.AREA.,$,#70,(#80,#80));
#70=IFCPOLYLINE((#71,#72,#73,#74,#71));
#71=IFCCARTESIANPOINT((0.,0.));
#72=IFCCARTESIANPOINT((2000.,0.));
#73=IFCCARTESIANPOINT((2000.,2000.));
#74=IFCCARTESIANPOINT((0.,2000.));
#80=IFCPOLYLINE((#81,#82,#83,#84,#81));
#81=IFCCARTESIANPOINT((500.,500.));
#82=IFCCARTESIANPOINT((1500.,500.));
#83=IFCCARTESIANPOINT((1500.,1500.));
#84=IFCCARTESIANPOINT((500.,1500.));
#62=IFCDIRECTION((0.,0.,1.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('dupvoid00000000000000A',$,'DupVoid',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 1);
    assert_close(world_volume(&geometry.items[0]), 3.0, 1e-9);
}

#[test]
fn representation_items_that_are_not_shape_representations_are_skipped_and_reported() {
    let body = "\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCTOPOLOGYREPRESENTATION($,'Body','Vertex',(#60));
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#62=IFCDIRECTION((0.,0.,1.));
#50=IFCBUILDINGELEMENTPROXY('topology00000000000000A',$,'Topology',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_NOT_EVALUATED.code,
            "IFCTOPOLOGYREPRESENTATION"
        ),
        1
    );
}

#[test]
fn local_placement_accepts_a_two_dimensional_relative_placement() {
    let body = "\
#10=IFCLOCALPLACEMENT($,#11);
#11=IFCAXIS2PLACEMENT2D(#12,$);
#12=IFCCARTESIANPOINT((1000.,0.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#60=IFCEXTRUDEDAREASOLID(#61,$,#62,1000.);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#62=IFCDIRECTION((0.,0.,1.));
#50=IFCBUILDINGELEMENTPROXY('plane00000000000000000A',$,'Plane',$,$,#10,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 1);
    let world = bounds(
        &geometry.items[0]
            .world_mesh(&Limits::CONSTRAINED)
            .unwrap()
            .unwrap(),
    );
    // The rectangle is centered on its position: x from 0.5 to 1.5 m.
    assert_close(world.min().x, 0.5, 1e-9);
    assert_close(world.max().x, 1.5, 1e-9);
}

#[test]
fn zero_conversion_factor_makes_the_unit_unusable_and_meters_are_assumed() {
    let text = file(
        "#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Demo',$,$,$,$,$,#100);\n\
#100=IFCUNITASSIGNMENT((#101));\n\
#101=IFCCONVERSIONBASEDUNIT($,.LENGTHUNIT.,'zero',#103);\n\
#103=IFCMEASUREWITHUNIT(IFCLENGTHMEASURE(0.),#104);\n\
#104=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n",
    );
    let geometry = evaluate(&text);
    assert!(geometry.units.length_assumed);
    assert_eq!(geometry.units.meters_per_length_unit, 1.0);
    assert_eq!(geometry.report.items[0].reason, reasons::UNIT_ASSUMED.code);
}

/// A map of a 1 m box at its origin, mapped with the operator of the body given by `operator`.
fn mapped_box(operator: &str) -> String {
    format!(
        "\
#70=IFCREPRESENTATIONMAP($,#72);
#72=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#74));
#74=IFCEXTRUDEDAREASOLID(#75,$,#76,1000.);
#75=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#76=IFCDIRECTION((0.,0.,1.));
#80=IFCMAPPEDITEM(#70,#81);
{operator}
#90=IFCPRODUCTDEFINITIONSHAPE($,$,(#91));
#91=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#80));
#50=IFCBUILDINGELEMENTPROXY('mapped00000000000000000A',$,'Mapped',$,$,$,#90,$,$);
"
    )
}

#[test]
fn operator_axes_are_made_orthonormal_so_there_is_no_shear() {
    // Axis2 is (1, 1, 0): its component along Axis1 is removed, leaving the y direction.
    let body = mapped_box(
        "#81=IFCCARTESIANTRANSFORMATIONOPERATOR3D(#82,#83,$,$,$);\n#82=IFCDIRECTION((1.,0.,0.));\n#83=IFCDIRECTION((1.,1.,0.));",
    );
    let geometry = evaluate(&project(&body));
    assert_eq!(geometry.items.len(), 1);
    let m = geometry.items[0]
        .transform
        .matrix()
        .fixed_view::<3, 3>(0, 0);
    for i in 0..3 {
        for j in 0..3 {
            let expected = if i == j { 1.0 } else { 0.0 };
            assert_close(m.column(i).dot(&m.column(j)), expected, 1e-12);
        }
    }
    assert!(
        geometry.report.items.is_empty(),
        "{:?}",
        geometry.report.items
    );
}

#[test]
fn operator_axis_parallel_to_the_third_is_replaced_and_reported() {
    // Axis1 (0, 0, 2) is parallel to the default third axis: a perpendicular direction is used.
    let body = mapped_box(
        "#81=IFCCARTESIANTRANSFORMATIONOPERATOR3D(#82,$,$,$,$);\n#82=IFCDIRECTION((0.,0.,2.));",
    );
    let geometry = evaluate(&project(&body));
    assert_eq!(geometry.items.len(), 1);
    assert_close(world_volume(&geometry.items[0]), 1.0, 1e-12);
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_APPROXIMATED.code,
            "IFCMAPPEDITEM"
        ),
        1
    );
}

#[test]
fn non_positive_operator_scale_is_refused_and_reported() {
    let body = mapped_box("#81=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,$,-1.,$);");
    let geometry = evaluate(&project(&body));
    assert!(geometry.items.is_empty());
    assert_eq!(
        loss_count(
            &geometry,
            reasons::GEOMETRY_DEGENERATE.code,
            "IFCCARTESIANTRANSFORMATIONOPERATOR3D"
        ),
        1
    );
}

#[test]
fn mapped_composition_is_mapping_origin_then_target() {
    // The mapping origin rotates by 90 degrees about z and moves by (1, 0, 0) m: a point (1, 0, 0)
    // goes to R(1, 0, 0) + (1, 0, 0) = (0, 1, 0) + (1, 0, 0) = (1, 1, 0). The target translates by
    // (0, 5, 0): (1, 1, 0) + (0, 5, 0) = (1, 6, 0). The origin is applied first, so the world point
    // of the polyline's first point (1, 0, 0) is target(origin(p)) = (1, 6, 0).
    let body = "\
#70=IFCREPRESENTATIONMAP(#71,#72);
#71=IFCAXIS2PLACEMENT3D(#73,#77,#78);
#73=IFCCARTESIANPOINT((1000.,0.,0.));
#77=IFCDIRECTION((0.,0.,1.));
#78=IFCDIRECTION((0.,1.,0.));
#72=IFCSHAPEREPRESENTATION($,'Axis','Curve3D',(#74));
#74=IFCPOLYLINE((#75,#76));
#75=IFCCARTESIANPOINT((1000.,0.,0.));
#76=IFCCARTESIANPOINT((2000.,0.,0.));
#80=IFCMAPPEDITEM(#70,#81);
#81=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,#82,$,$);
#82=IFCCARTESIANPOINT((0.,5000.,0.));
#90=IFCPRODUCTDEFINITIONSHAPE($,$,(#91));
#91=IFCSHAPEREPRESENTATION($,'Axis','MappedRepresentation',(#80));
#50=IFCBUILDINGELEMENTPROXY('compose0000000000000000A',$,'Compose',$,$,$,#90,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 1);
    let item = &geometry.items[0];
    let Shape::Polylines(lines) = item.shape.as_ref() else {
        panic!("polylines");
    };
    let world = item.transform.transform_point(&lines[0][0]);
    assert_close(world.x, 1.0, 1e-12);
    assert_close(world.y, 6.0, 1e-12);
    assert_close(world.z, 0.0, 1e-12);
}

#[test]
fn mapping_cut_result_is_not_cached_for_a_later_reference() {
    // Y maps a map holding Z and a box; Z maps a map holding Y. Reached inside Y, Z is cut at Y and
    // holds nothing, which is correct only inside that evaluation. The second product references Z
    // directly, and Z must give the box of Y's map, not the empty result of the cut.
    let body = "\
#70=IFCREPRESENTATIONMAP($,#71);
#71=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#110,#90));
#80=IFCMAPPEDITEM(#70,#81);
#81=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,$,$,$);
#90=IFCEXTRUDEDAREASOLID(#91,$,#92,1000.);
#91=IFCRECTANGLEPROFILEDEF(.AREA.,$,$,1000.,1000.);
#92=IFCDIRECTION((0.,0.,1.));
#400=IFCREPRESENTATIONMAP($,#401);
#401=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#80));
#110=IFCMAPPEDITEM(#400,#411);
#411=IFCCARTESIANTRANSFORMATIONOPERATOR3D($,$,$,$,$);
#120=IFCPRODUCTDEFINITIONSHAPE($,$,(#121));
#121=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#80));
#122=IFCPRODUCTDEFINITIONSHAPE($,$,(#123));
#123=IFCSHAPEREPRESENTATION($,'Body','MappedRepresentation',(#110));
#50=IFCBUILDINGELEMENTPROXY('cutcache000000000000000A',$,'First',$,$,$,#120,$,$);
#51=IFCBUILDINGELEMENTPROXY('cutcache000000000000000B',$,'Second',$,$,$,#122,$,$);
";
    let geometry = evaluate(&project(body));
    assert_eq!(geometry.items.len(), 2);
    assert_eq!(geometry.items[0].product, 50);
    assert_eq!(geometry.items[1].product, 51);
}

#[test]
fn box_whose_side_faces_share_a_collinear_point_on_an_edge_stays_closed() {
    // Unit cube whose front and bottom faces both have the midpoint 8 of their shared edge
    // (0,0,0)-(1,0,0). Both faces keep it, so the shell has no cracks and no boundary.
    let corners = [
        (0.0, 0.0, 0.0),
        (1000.0, 0.0, 0.0),
        (1000.0, 1000.0, 0.0),
        (0.0, 1000.0, 0.0),
        (0.0, 0.0, 1000.0),
        (1000.0, 0.0, 1000.0),
        (1000.0, 1000.0, 1000.0),
        (0.0, 1000.0, 1000.0),
        (500.0, 0.0, 0.0),
    ];
    let mut body = String::new();
    for (k, (x, y, z)) in corners.iter().enumerate() {
        body.push_str(&format!(
            "#{}=IFCCARTESIANPOINT(({x:?},{y:?},{z:?}));\n",
            200 + k
        ));
    }
    // Each face is a loop of corner indices (0-based), counterclockwise seen from outside, and the
    // flag gives the bound's orientation. The bottom is written reversed with `.F.`.
    let faces: [(&[usize], &str); 6] = [
        (&[8, 1, 2, 3, 0], ".F."),
        (&[0, 8, 1, 5, 4], ".T."),
        (&[1, 2, 6, 5], ".T."),
        (&[2, 3, 7, 6], ".T."),
        (&[3, 0, 4, 7], ".T."),
        (&[4, 5, 6, 7], ".T."),
    ];
    let mut names = Vec::new();
    let mut next = 300;
    for (loop_, orientation) in faces {
        let refs: Vec<String> = loop_.iter().map(|i| format!("#{}", 200 + i)).collect();
        body.push_str(&format!("#{next}=IFCPOLYLOOP(({}));\n", refs.join(",")));
        body.push_str(&format!(
            "#{}=IFCFACEOUTERBOUND(#{next},{orientation});\n",
            next + 1
        ));
        body.push_str(&format!("#{}=IFCFACE((#{}));\n", next + 2, next + 1));
        names.push(format!("#{}", next + 2));
        next += 3;
    }
    body.push_str(&format!("#150=IFCCLOSEDSHELL(({}));\n", names.join(",")));
    body.push_str(
        "#151=IFCFACETEDBREP(#150);\n#152=IFCSHAPEREPRESENTATION($,'Body','Brep',(#151));\n",
    );
    body.push_str("#153=IFCPRODUCTDEFINITIONSHAPE($,$,(#152));\n");
    body.push_str(
        "#154=IFCBUILDINGELEMENTPROXY('midbox00000000000000000A',$,'Mid',$,$,$,#153,$,$);\n",
    );
    let geometry = evaluate(&project(&body));
    assert_eq!(geometry.items.len(), 1);
    let mesh = mesh_of(&geometry.items[0]);
    let topology = component_topology(mesh, &Limits::CONSTRAINED).unwrap();
    assert_eq!(topology.len(), 1);
    assert_eq!(topology[0].boundary_loops, 0);
    assert_close(volume(mesh, &Limits::CONSTRAINED).unwrap(), 1.0, 1e-12);
    // The midpoint is a vertex of the mesh, shared by the two faces that meet at that edge.
    assert!(
        mesh.positions()
            .iter()
            .any(|p| (p.x - 0.5).abs() < 1e-12 && p.y.abs() < 1e-12 && p.z.abs() < 1e-12)
    );
}

#[test]
fn skew_extrusion_direction_is_given_in_position_coordinates() {
    // The position turns the x axis of the profile to world y and its y axis to world -x. The
    // direction (1, 0, 1)/sqrt 2 in position coordinates is (0, 1, 1)/sqrt 2 in world coordinates,
    // so the top is the base moved by (0, 1, 1) m: the height along the normal is 1 m.
    let body = "\
#60=IFCEXTRUDEDAREASOLID(#61,#65,#62,1414.2135623730951);
#61=IFCRECTANGLEPROFILEDEF(.AREA.,$,#63,1000.,1000.);
#62=IFCDIRECTION((0.7071067811865476,0.,0.7071067811865476));
#63=IFCAXIS2PLACEMENT2D(#64,$);
#64=IFCCARTESIANPOINT((500.,500.));
#65=IFCAXIS2PLACEMENT3D(#66,#67,#68);
#66=IFCCARTESIANPOINT((0.,0.,0.));
#67=IFCDIRECTION((0.,0.,1.));
#68=IFCDIRECTION((0.,1.,0.));
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));
#41=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#60));
#50=IFCBUILDINGELEMENTPROXY('skewpos00000000000000A',$,'SkewPos',$,$,$,#40,$,$);
";
    let geometry = evaluate(&project(body));
    assert_close(world_volume(&geometry.items[0]), 1.0, 1e-9);
    let world = bounds(
        &geometry.items[0]
            .world_mesh(&Limits::CONSTRAINED)
            .unwrap()
            .unwrap(),
    );
    assert_close(world.min().x, -1.0, 1e-9);
    assert_close(world.max().x, 0.0, 1e-9);
    assert_close(world.max().y, 2.0, 1e-9);
    assert_close(world.max().z, 1.0, 1e-9);
}

mod mutations {
    use super::*;
    use proptest::prelude::*;

    /// The 2 x 2 x 2 m cube with a pocket, as a polygonal face set.
    fn face_set() -> String {
        file(&format!(
            "{POCKETED_CUBE}#11=IFCINDEXEDPOLYGONALFACE((13,14,15,16));\n\
#62=IFCPOLYGONALFACESET(#61,.T.,(#1,#2,#3,#4,#5,#6,#7,#8,#9,#10,#11),$);\n\
#40=IFCPRODUCTDEFINITIONSHAPE($,$,(#41));\n#41=IFCSHAPEREPRESENTATION($,'Body','Tessellation',(#62));\n\
#50=IFCBUILDINGELEMENTPROXY('cube0000000000000000000A',$,'Cube',$,$,$,#40,$,$);\n"
        ))
    }

    /// One of three fixtures: an extrusion in a placement chain, a face set and a mapped item.
    fn base(kind: usize) -> String {
        match kind % 3 {
            0 => project(PLACED_BOX),
            1 => face_set(),
            _ => project(MAPPED),
        }
    }

    proptest! {
        #[test]
        fn mutated_fixtures_never_panic(
            kind in 0_usize..3,
            position in 0_usize..4096,
            replacement in proptest::char::range(' ', '~'),
            delete in 0_usize..3,
        ) {
            let text = base(kind);
            let mut chars: Vec<char> = text.chars().collect();
            let at = position % chars.len().max(1);
            match delete {
                0 => { chars[at] = replacement; }
                1 => { chars.remove(at); }
                _ => { chars.insert(at, replacement); }
            }
            let mutated: String = chars.into_iter().collect();
            if let Ok(output) = read(mutated.as_bytes(), &mut ReadOptions::with_limits(Limits::CONSTRAINED)) {
                let _ = output.file.geometry(&Limits::CONSTRAINED, &Cancel::new());
            }
        }
    }
}
