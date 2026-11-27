//! Geometry wave 1 (CORE-IO-015, #123): parameterized profiles, curves, swept solids, half-space
//! clipping, prism openings and B-reps with voids, each against hand-computed areas and volumes.
//! `ARC_IFC_DUMP=<directory>` writes every fixture for `tools/ifc-oracle/geometry_compare.py`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    reason = "test code: fixtures and assertions"
)]

mod support;

use std::f64::consts::PI;

use arc_base::{Cancel, Limits};
use arc_io_ifc::{Geometry, ReadOptions, Shape, read, reasons};
use arc_mesh::measure::{surface_area, volume};

const HEADER: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition []'),'2;1');\n\
FILE_NAME('wave1.ifc','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n";

/// A project in meters, the common axes (#2 z, #3 x, #4 y, #6 the world placement, #8 the 2D one) and
/// one element (#902) whose body is the item #10 of `lines`.
fn fixture(lines: &str) -> String {
    format!(
        "{HEADER}#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Demo',$,$,$,$,$,#100);\n\
#100=IFCUNITASSIGNMENT((#101,#102));\n\
#101=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n\
#102=IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.);\n\
#2=IFCDIRECTION((0.,0.,1.));\n#3=IFCDIRECTION((1.,0.,0.));\n#4=IFCDIRECTION((0.,1.,0.));\n\
#5=IFCCARTESIANPOINT((0.,0.,0.));\n#6=IFCAXIS2PLACEMENT3D(#5,$,$);\n\
#7=IFCCARTESIANPOINT((0.,0.));\n#8=IFCAXIS2PLACEMENT2D(#7,$);\n\
{lines}\
#900=IFCPRODUCTDEFINITIONSHAPE($,$,(#901));\n\
#901=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#10));\n\
#902=IFCBUILDINGELEMENTPROXY('case0000000000000000001',$,'Case',$,$,$,#900,$,$);\n\
ENDSEC;\nEND-ISO-10303-21;\n"
    )
}

fn evaluate(text: &str) -> Geometry {
    let output = read(text.as_bytes(), &mut ReadOptions::default()).expect("read");
    output
        .file
        .geometry(&Limits::DEFAULT, &Cancel::new())
        .expect("geometry")
}

/// Volume and area of all items of product #902.
fn measure(geometry: &Geometry) -> (f64, f64) {
    measure_of(geometry, 902)
}

/// Volume and area of all items of a product.
fn measure_of(geometry: &Geometry, product: u64) -> (f64, f64) {
    let (mut v, mut a) = (0.0, 0.0);
    for item in geometry.items.iter().filter(|i| i.product == product) {
        let Some(mesh) = item.world_mesh(&Limits::DEFAULT).unwrap() else {
            continue;
        };
        v += volume(&mesh, &Limits::DEFAULT).expect("closed mesh");
        a += surface_area(&mesh, &Limits::DEFAULT).unwrap();
    }
    (v, a)
}

/// The count of the loss items of a reason and record kind.
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

fn close(actual: f64, expected: f64, relative: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= relative * expected.abs().max(1e-12),
        "{what}: {actual} vs {expected}"
    );
}

/// Evaluates `lines` (the item is #10) and compares the volume and area, then dumps the file.
fn check(name: &str, lines: &str, volume: f64, area: f64, relative: f64) -> Geometry {
    let text = fixture(lines);
    support::dump_nonconformant(&format!("wave1-{name}.ifc"), text.as_bytes());
    let geometry = evaluate(&text);
    assert!(!geometry.items.is_empty(), "{name}: nothing evaluated");
    let (v, a) = measure(&geometry);
    close(v, volume, relative, &format!("{name} volume"));
    close(a, area, relative, &format!("{name} area"));
    geometry
}

/// A prism of `depth` over the profile #11.
fn prism(profile: &str, depth: f64) -> String {
    format!("#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,{depth:?});\n#11={profile};\n")
}

/// The area and perimeter of a prism of unit depth give its volume and its area.
fn profile_case(name: &str, profile: &str, area: f64, perimeter: f64, relative: f64) {
    check(
        name,
        &prism(profile, 1.0),
        area,
        2.0 * area + perimeter,
        relative,
    );
}

#[test]
fn rectangle_family() {
    profile_case(
        "rectangle",
        "IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,2.,3.)",
        6.0,
        10.0,
        1e-9,
    );
    // Rounded corners of radius 0.2 (inscribed polygon: a small deficit).
    profile_case(
        "rounded-rectangle",
        "IFCROUNDEDRECTANGLEPROFILEDEF(.AREA.,$,#8,2.,1.,0.2)",
        2.0 - (4.0 - PI) * 0.04,
        6.0 - 1.6 + 2.0 * PI * 0.2,
        3e-3,
    );
    profile_case(
        "rectangle-hollow",
        "IFCRECTANGLEHOLLOWPROFILEDEF(.AREA.,$,#8,2.,1.,0.1,$,$)",
        0.56,
        6.0 + 5.2,
        1e-9,
    );
}

#[test]
fn round_family() {
    profile_case(
        "circle",
        "IFCCIRCLEPROFILEDEF(.AREA.,$,#8,0.5)",
        PI * 0.25,
        PI,
        3e-3,
    );
    profile_case(
        "circle-hollow",
        "IFCCIRCLEHOLLOWPROFILEDEF(.AREA.,$,#8,0.5,0.1)",
        PI * (0.25 - 0.16),
        PI * (1.0 + 0.8),
        3e-3,
    );
    // An ellipse perimeter by Ramanujan's approximation.
    let (a, b): (f64, f64) = (1.0, 0.5);
    let perimeter = PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt());
    profile_case(
        "ellipse",
        "IFCELLIPSEPROFILEDEF(.AREA.,$,#8,1.,0.5)",
        PI * a * b,
        perimeter,
        3e-3,
    );
}

/// The area and perimeter of a polygon given by its corners.
fn polygon(corners: &[[f64; 2]]) -> (f64, f64) {
    let (mut area, mut perimeter) = (0.0, 0.0);
    for k in 0..corners.len() {
        let (a, b) = (corners[k], corners[(k + 1) % corners.len()]);
        area += a[0] * b[1] - b[0] * a[1];
        perimeter += (b[0] - a[0]).hypot(b[1] - a[1]);
    }
    (area.abs() / 2.0, perimeter)
}

fn polygon_case(name: &str, profile: &str, corners: &[[f64; 2]]) {
    let (area, perimeter) = polygon(corners);
    profile_case(name, profile, area, perimeter, 1e-9);
}

#[test]
fn section_shapes() {
    // I: width 0.2, depth 0.4, web 0.01, flange 0.02.
    polygon_case(
        "i-shape",
        "IFCISHAPEPROFILEDEF(.AREA.,$,#8,0.2,0.4,0.01,0.02,$,$,$)",
        &[
            [-0.1, -0.2],
            [0.1, -0.2],
            [0.1, -0.18],
            [0.005, -0.18],
            [0.005, 0.18],
            [0.1, 0.18],
            [0.1, 0.2],
            [-0.1, 0.2],
            [-0.1, 0.18],
            [-0.005, 0.18],
            [-0.005, -0.18],
            [-0.1, -0.18],
        ],
    );
    // L: depth 0.1 (y), width 0.15 (x), thickness 0.01.
    polygon_case(
        "l-shape",
        "IFCLSHAPEPROFILEDEF(.AREA.,$,#8,0.1,0.15,0.01,$,$,$)",
        &[
            [-0.075, -0.05],
            [0.075, -0.05],
            [0.075, -0.04],
            [-0.065, -0.04],
            [-0.065, 0.05],
            [-0.075, 0.05],
        ],
    );
    // T: depth 0.2, flange width 0.2, web 0.02, flange 0.03.
    polygon_case(
        "t-shape",
        "IFCTSHAPEPROFILEDEF(.AREA.,$,#8,0.2,0.2,0.02,0.03,$,$,$,$,$)",
        &[
            [-0.01, -0.1],
            [0.01, -0.1],
            [0.01, 0.07],
            [0.1, 0.07],
            [0.1, 0.1],
            [-0.1, 0.1],
            [-0.1, 0.07],
            [-0.01, 0.07],
        ],
    );
    // U: depth 0.2, flange width 0.1, web 0.01, flange 0.015.
    polygon_case(
        "u-shape",
        "IFCUSHAPEPROFILEDEF(.AREA.,$,#8,0.2,0.1,0.01,0.015,$,$,$)",
        &[
            [-0.05, -0.1],
            [0.05, -0.1],
            [0.05, -0.085],
            [-0.04, -0.085],
            [-0.04, 0.085],
            [0.05, 0.085],
            [0.05, 0.1],
            [-0.05, 0.1],
        ],
    );
    // C: depth 0.2, width 0.1, wall 0.01, girth 0.03.
    polygon_case(
        "c-shape",
        "IFCCSHAPEPROFILEDEF(.AREA.,$,#8,0.2,0.1,0.01,0.03,$)",
        &[
            [-0.05, -0.1],
            [0.05, -0.1],
            [0.05, -0.07],
            [0.04, -0.07],
            [0.04, -0.09],
            [-0.04, -0.09],
            [-0.04, 0.09],
            [0.04, 0.09],
            [0.04, 0.07],
            [0.05, 0.07],
            [0.05, 0.1],
            [-0.05, 0.1],
        ],
    );
    // Z: depth 0.2, flange width 0.1, web 0.01, flange 0.015.
    polygon_case(
        "z-shape",
        "IFCZSHAPEPROFILEDEF(.AREA.,$,#8,0.2,0.1,0.01,0.015,$,$)",
        &[
            [-0.005, -0.1],
            [0.1, -0.1],
            [0.1, -0.085],
            [0.005, -0.085],
            [0.005, 0.1],
            [-0.1, 0.1],
            [-0.1, 0.085],
            [-0.005, 0.085],
        ],
    );
    // Asymmetric I: bottom flange 0.2 x 0.02, top flange 0.1 x 0.01, depth 0.4, web 0.01.
    polygon_case(
        "asymmetric-i-shape",
        "IFCASYMMETRICISHAPEPROFILEDEF(.AREA.,$,#8,0.2,0.4,0.01,0.02,$,0.1,0.01,$,$,$,$,$)",
        &[
            [-0.1, -0.2],
            [0.1, -0.2],
            [0.1, -0.18],
            [0.005, -0.18],
            [0.005, 0.19],
            [0.05, 0.19],
            [0.05, 0.2],
            [-0.05, 0.2],
            [-0.05, 0.19],
            [-0.005, 0.19],
            [-0.005, -0.18],
            [-0.1, -0.18],
        ],
    );
    // Trapezium: bottom 2, top 1, height 1, top edge offset 0.5 from the bottom edge's left end.
    polygon_case(
        "trapezium",
        "IFCTRAPEZIUMPROFILEDEF(.AREA.,$,#8,2.,1.,1.,0.5)",
        &[[-1.0, -0.5], [1.0, -0.5], [0.5, 0.5], [-0.5, 0.5]],
    );
}

#[test]
fn fillets_round_the_corners_of_a_shape() {
    // An I shape with web fillets of 0.01: each of the four removes (1 - pi/4) r^2 from the
    // sharp corner's material... the fillet adds material: (1 - pi/4) r^2 per corner.
    let sharp = polygon(&[
        [-0.1, -0.2],
        [0.1, -0.2],
        [0.1, -0.18],
        [0.005, -0.18],
        [0.005, 0.18],
        [0.1, 0.18],
        [0.1, 0.2],
        [-0.1, 0.2],
        [-0.1, 0.18],
        [-0.005, 0.18],
        [-0.005, -0.18],
        [-0.1, -0.18],
    ])
    .0;
    let gain = 4.0 * (1.0 - PI / 4.0) * 0.01 * 0.01;
    let text = fixture(&prism(
        "IFCISHAPEPROFILEDEF(.AREA.,$,#8,0.2,0.4,0.01,0.02,0.01,$,$)",
        1.0,
    ));
    support::dump_nonconformant("wave1-i-shape-fillet.ifc", text.as_bytes());
    let (v, _) = measure(&evaluate(&text));
    // The arcs are inscribed polygons of the chord tolerance, which adds slightly more material in a
    // concave corner than the exact fillet.
    close(v, sharp + gain, 5e-3, "filleted area");
}

#[test]
fn composite_derived_mirrored_and_center_line_profiles() {
    // Composite: a 1 x 1 square and a circle of radius 0.25 apart.
    let composite = "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,1.);\n\
#11=IFCCOMPOSITEPROFILEDEF(.AREA.,$,(#12,#13),$);\n\
#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,1.);\n\
#13=IFCCIRCLEPROFILEDEF(.AREA.,$,#14,0.25);\n\
#14=IFCAXIS2PLACEMENT2D(#15,$);\n#15=IFCCARTESIANPOINT((3.,0.));\n";
    check(
        "composite-profile",
        composite,
        1.0 + PI * 0.0625,
        2.0 * (1.0 + PI * 0.0625) + 4.0 + PI * 0.5,
        3e-3,
    );
    // Derived: a 1 x 1 square scaled by 2 along x and 3 along y, moved by (5, 0).
    let derived = "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,1.);\n\
#11=IFCDERIVEDPROFILEDEF(.AREA.,$,#12,#13,$);\n\
#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,1.);\n\
#13=IFCCARTESIANTRANSFORMATIONOPERATOR2DNONUNIFORM($,$,#14,2.,3.);\n\
#14=IFCCARTESIANPOINT((5.,0.));\n";
    check("derived-profile", derived, 6.0, 12.0 + 10.0, 1e-9);
    // Mirrored: an L shape keeps its area.
    let mirrored = "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,1.);\n\
#11=IFCMIRROREDPROFILEDEF(.AREA.,$,#12,$,$);\n\
#12=IFCLSHAPEPROFILEDEF(.AREA.,$,#8,0.1,0.15,0.01,$,$,$);\n";
    let (area, perimeter) = polygon(&[
        [-0.075, -0.05],
        [0.075, -0.05],
        [0.075, -0.04],
        [-0.065, -0.04],
        [-0.065, 0.05],
        [-0.075, 0.05],
    ]);
    check(
        "mirrored-profile",
        mirrored,
        area,
        2.0 * area + perimeter,
        1e-9,
    );
    // Center line: a strip of thickness 0.1 along 1 m.
    let center = "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,1.);\n\
#11=IFCCENTERLINEPROFILEDEF(.AREA.,$,#12,0.1);\n\
#12=IFCPOLYLINE((#7,#13));\n#13=IFCCARTESIANPOINT((1.,0.));\n";
    check("center-line-profile", center, 0.1, 0.2 + 2.2, 1e-9);
}

/// An arbitrary profile over the closed curve #12 of `lines`, extruded 1 m.
fn curve_profile(name: &str, lines: &str, area: f64, perimeter: f64) {
    let text = format!(
        "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,1.);\n#11=IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#12);\n{lines}"
    );
    check(name, &text, area, 2.0 * area + perimeter, 3e-3);
}

#[test]
fn trimmed_and_composite_curves() {
    // A half disc: the circle trimmed by parameters from 0 to pi, closed by a polyline.
    let half = "#12=IFCCOMPOSITECURVE((#13,#14),.F.);\n\
#13=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#15);\n\
#14=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#16);\n\
#15=IFCTRIMMEDCURVE(#17,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(3.14159265358979)),.T.,.PARAMETER.);\n\
#17=IFCCIRCLE(#8,1.);\n#16=IFCPOLYLINE((#18,#19));\n\
#18=IFCCARTESIANPOINT((-1.,0.));\n#19=IFCCARTESIANPOINT((1.,0.));\n";
    curve_profile("trimmed-circle-by-parameter", half, PI / 2.0, PI + 2.0);
    // The same by points, running with the parameter and against it.
    let by_points = |sense: &str| {
        format!(
            "#12=IFCCOMPOSITECURVE((#13,#14),.F.);\n\
#13=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#15);\n\
#14=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#16);\n\
#15=IFCTRIMMEDCURVE(#17,(#19),(#18),{sense},.CARTESIAN.);\n\
#17=IFCCIRCLE(#8,1.);\n#16=IFCPOLYLINE((#18,#19));\n\
#18=IFCCARTESIANPOINT((-1.,0.));\n#19=IFCCARTESIANPOINT((1.,0.));\n"
        )
    };
    curve_profile(
        "trimmed-circle-by-points",
        &by_points(".T."),
        PI / 2.0,
        PI + 2.0,
    );
    // Against the parameter the arc is the lower half.
    curve_profile(
        "trimmed-circle-reversed",
        &by_points(".F."),
        PI / 2.0,
        PI + 2.0,
    );
    // A half ellipse of semi-axes 2 and 1 whose segment is reversed in the composite.
    let ellipse = "#12=IFCCOMPOSITECURVE((#16,#13),.F.);\n\
#13=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.F.,#15);\n\
#16=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#20);\n\
#15=IFCTRIMMEDCURVE(#17,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(3.14159265358979)),.T.,.PARAMETER.);\n\
#17=IFCELLIPSE(#8,2.,1.);\n#20=IFCPOLYLINE((#19,#18));\n\
#18=IFCCARTESIANPOINT((-2.,0.));\n#19=IFCCARTESIANPOINT((2.,0.));\n";
    // Ramanujan: half of the ellipse perimeter plus the 4 m chord.
    let (a, b): (f64, f64) = (2.0, 1.0);
    let full = PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt());
    curve_profile("trimmed-ellipse", ellipse, PI, full / 2.0 + 4.0);
    // A square of lines trimmed by parameters and by a point.
    let square = "#12=IFCCOMPOSITECURVE((#30,#31,#32,#33),.F.);\n\
#30=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#40);\n\
#31=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#41);\n\
#32=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#42);\n\
#33=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#43);\n\
#40=IFCTRIMMEDCURVE(#50,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(2.)),.T.,.PARAMETER.);\n\
#41=IFCTRIMMEDCURVE(#51,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(2.)),.T.,.PARAMETER.);\n\
#42=IFCTRIMMEDCURVE(#52,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(2.)),.T.,.PARAMETER.);\n\
#43=IFCTRIMMEDCURVE(#53,(#60),(#59),.T.,.CARTESIAN.);\n\
#50=IFCLINE(#7,#70);\n#51=IFCLINE(#61,#71);\n#52=IFCLINE(#62,#72);\n#53=IFCLINE(#63,#73);\n\
#60=IFCCARTESIANPOINT((0.,2.));\n#61=IFCCARTESIANPOINT((2.,0.));\n#62=IFCCARTESIANPOINT((2.,2.));\n#63=IFCCARTESIANPOINT((0.,2.));\n\
#59=IFCCARTESIANPOINT((0.,0.));\n\
#70=IFCVECTOR(#3,1.);\n#71=IFCVECTOR(#4,1.);\n#72=IFCVECTOR(#80,1.);\n#73=IFCVECTOR(#81,1.);\n\
#80=IFCDIRECTION((-1.,0.));\n#81=IFCDIRECTION((0.,-1.));\n";
    curve_profile("composite-of-trimmed-lines", square, 4.0, 8.0);
}

#[test]
fn indexed_poly_curve_arcs_and_profile_voids() {
    let arc = "#12=IFCINDEXEDPOLYCURVE(#13,(IFCARCINDEX((1,2,3)),IFCLINEINDEX((3,1))),$);\n\
#13=IFCCARTESIANPOINTLIST2D(((-1.,0.),(0.,1.),(1.,0.)));\n";
    curve_profile("indexed-poly-curve-arc", arc, PI / 2.0, PI + 2.0);
    // A square with a circular hole, the hole given as a circle curve.
    let voids = "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,1.);\n\
#11=IFCARBITRARYPROFILEDEFWITHVOIDS(.AREA.,$,#12,(#13));\n\
#12=IFCPOLYLINE((#20,#21,#22,#23,#20));\n#13=IFCCIRCLE(#8,1.);\n\
#20=IFCCARTESIANPOINT((-2.,-2.));\n#21=IFCCARTESIANPOINT((2.,-2.));\n\
#22=IFCCARTESIANPOINT((2.,2.));\n#23=IFCCARTESIANPOINT((-2.,2.));\n";
    let area = 16.0 - PI;
    check(
        "profile-with-circle-void",
        voids,
        area,
        2.0 * area + 16.0 + 2.0 * PI,
        3e-3,
    );
}

#[test]
fn tapered_extrusion_is_a_frustum() {
    let lines = "#10=IFCEXTRUDEDAREASOLIDTAPERED(#11,#6,#2,3.,#12);\n\
#11=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,2.,2.);\n\
#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,1.);\n";
    // Volume h/3 (A1 + A2 + sqrt(A1 A2)); lateral area of four trapezoids of height sqrt(9 + 0.25).
    let slant = (9.0_f64 + 0.25).sqrt();
    check(
        "tapered-extrusion",
        lines,
        3.0 / 3.0 * (4.0 + 1.0 + 2.0),
        4.0 + 1.0 + 4.0 * 1.5 * slant,
        1e-9,
    );
}

#[test]
fn revolved_solids() {
    let revolve = |angle: &str| {
        format!(
            "#10=IFCREVOLVEDAREASOLID(#11,#6,#12,{angle});\n\
#11=IFCRECTANGLEPROFILEDEF(.AREA.,$,#13,1.,1.);\n\
#12=IFCAXIS1PLACEMENT(#5,#4);\n#13=IFCAXIS2PLACEMENT2D(#14,$);\n#14=IFCCARTESIANPOINT((1.5,0.5));\n"
        )
    };
    // x in [1, 2], y in [0, 1] about the y axis: a washer, volume pi (4 - 1) and area 12 pi.
    check(
        "revolved-full",
        &revolve("6.28318530717959"),
        3.0 * PI,
        12.0 * PI,
        3e-3,
    );
    // Half a turn: half the washer plus the two end faces of 1 m^2.
    check(
        "revolved-half",
        &revolve("3.14159265358979"),
        1.5 * PI,
        6.0 * PI + 2.0,
        3e-3,
    );
}

/// A tube of radius 0.05 m has 16 sides at the 1 mm chord tolerance, whose area is 2.6 % below the
/// circle's.
const TUBE: f64 = 3.5e-2;

#[test]
fn swept_disks() {
    let disk = |directrix: &str, radius: &str| {
        format!(
            "#10=IFCSWEPTDISKSOLID(#11,{radius},$,$,$);\n#11={directrix};\n#12=IFCCARTESIANPOINT((0.,0.,0.));\n#13=IFCCARTESIANPOINT((1.,0.,0.));\n#14=IFCCARTESIANPOINT((1.,1.,0.));\n"
        )
    };
    let r: f64 = 0.1;
    // A straight tube of 2 m: the points are (0,0,0) and (2,0,0).
    let straight = "#10=IFCSWEPTDISKSOLID(#11,0.1,$,$,$);\n#11=IFCPOLYLINE((#12,#13));\n#12=IFCCARTESIANPOINT((0.,0.,0.));\n#13=IFCCARTESIANPOINT((2.,0.,0.));\n";
    check(
        "swept-disk-straight",
        straight,
        PI * r * r * 2.0,
        2.0 * PI * r * 2.0 + 2.0 * PI * r * r,
        TUBE,
    );
    // A tube with a right angle, mitered: the centerline length is the length of the tube.
    check(
        "swept-disk-bend",
        &disk("IFCPOLYLINE((#12,#13,#14))", "0.1"),
        PI * r * r * 2.0,
        2.0 * PI * r * 2.0 + 2.0 * PI * r * r,
        TUBE,
    );
    // A quarter circle of radius 1 as the directrix.
    let arc = "#10=IFCSWEPTDISKSOLID(#11,0.05,$,$,$);\n\
#11=IFCTRIMMEDCURVE(#12,(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE(1.5707963267949)),.T.,.PARAMETER.);\n\
#12=IFCCIRCLE(#6,1.);\n";
    let rr: f64 = 0.05;
    check(
        "swept-disk-arc",
        arc,
        PI * rr * rr * PI / 2.0,
        2.0 * PI * rr * PI / 2.0 + 2.0 * PI * rr * rr,
        TUBE,
    );
    // A hollow tube.
    let hollow = "#10=IFCSWEPTDISKSOLID(#11,0.1,0.05,$,$);\n#11=IFCPOLYLINE((#12,#13));\n#12=IFCCARTESIANPOINT((0.,0.,0.));\n#13=IFCCARTESIANPOINT((2.,0.,0.));\n";
    check(
        "swept-disk-hollow",
        hollow,
        PI * (0.01 - 0.0025) * 2.0,
        2.0 * PI * (0.1 + 0.05) * 2.0 + 2.0 * PI * (0.01 - 0.0025),
        TUBE,
    );
}

/// A box of 2 x 2 x 2 (x and y from -1 to 1, z from 0 to 2) as the item #11.
const BOX: &str =
    "#11=IFCEXTRUDEDAREASOLID(#12,#6,#2,2.);\n#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,2.,2.);\n";

#[test]
fn half_space_clipping() {
    let clip = |flag: &str, point: &str, axis: &str| {
        format!(
            "#10=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#11,#20);\n{BOX}\
#20=IFCHALFSPACESOLID(#21,{flag});\n#21=IFCPLANE(#22);\n\
#22=IFCAXIS2PLACEMENT3D(#23,{axis},$);\n#23=IFCCARTESIANPOINT({point});\n\
#24=IFCDIRECTION((1.,0.,1.));\n#25=IFCDIRECTION((1.,0.,0.));\n"
        )
    };
    // The half space on the side of the normal is removed when the flag is false.
    check(
        "half-space-below",
        &clip(".F.", "(0.,0.,1.)", "$"),
        4.0,
        16.0,
        1e-9,
    );
    check(
        "half-space-above",
        &clip(".T.", "(0.,0.,1.)", "$"),
        4.0,
        16.0,
        1e-9,
    );
    // A slanted plane through (0, 0, 1) with the normal (1, 0, 1): the kept part is x + z >= 1.
    check(
        "half-space-slanted",
        &clip(".T.", "(0.,0.,1.)", "#24"),
        4.0,
        12.0 + 4.0 * 2.0_f64.sqrt(),
        1e-9,
    );
    // A plane that misses the solid leaves it whole, and one that covers it removes it.
    check(
        "half-space-misses",
        &clip(".F.", "(0.,0.,5.)", "$"),
        8.0,
        24.0,
        1e-9,
    );
    let removed = evaluate(&fixture(&clip(".T.", "(0.,0.,5.)", "$")));
    assert!(removed.items.is_empty());
}

#[test]
fn clipping_results_nest_and_polygonal_boundaries_apply() {
    // Two clips: remove x > 0, then remove z > 1.
    let nested = format!(
        "#10=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#30,#31);\n\
#30=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#11,#20);\n{BOX}\
#20=IFCHALFSPACESOLID(#21,.F.);\n#21=IFCPLANE(#22);\n#22=IFCAXIS2PLACEMENT3D(#5,#3,$);\n\
#31=IFCHALFSPACESOLID(#32,.F.);\n#32=IFCPLANE(#33);\n\
#33=IFCAXIS2PLACEMENT3D(#34,$,$);\n#34=IFCCARTESIANPOINT((0.,0.,1.));\n"
    );
    // x in [-1, 0], y in [-1, 1], z in [0, 1].
    check(
        "clipping-nested",
        &nested,
        2.0,
        2.0 * (2.0 + 1.0 + 2.0),
        1e-9,
    );
    // A polygonal bounded half space whose boundary covers the solid is the plain half space.
    let bounded = format!(
        "#10=IFCBOOLEANCLIPPINGRESULT(.DIFFERENCE.,#11,#20);\n{BOX}\
#20=IFCPOLYGONALBOUNDEDHALFSPACE(#21,.F.,#23,#24);\n#21=IFCPLANE(#22);\n\
#22=IFCAXIS2PLACEMENT3D(#34,$,$);\n#34=IFCCARTESIANPOINT((0.,0.,1.));\n\
#23=IFCAXIS2PLACEMENT3D(#5,$,$);\n#24=IFCPOLYLINE((#40,#41,#42,#43,#40));\n\
#40=IFCCARTESIANPOINT((-5.,-5.));\n#41=IFCCARTESIANPOINT((5.,-5.));\n\
#42=IFCCARTESIANPOINT((5.,5.));\n#43=IFCCARTESIANPOINT((-5.,5.));\n"
    );
    check("polygonal-bounded-half-space", &bounded, 4.0, 16.0, 1e-9);
    // A boundary that clips the solid is not evaluated: the first operand is kept and reported.
    let small = bounded
        .replace("(-5.,-5.)", "(-0.5,-5.)")
        .replace("(-5.,5.)", "(-0.5,5.)");
    let geometry = check(
        "polygonal-bounded-half-space-clips",
        &small,
        8.0,
        24.0,
        1e-9,
    );
    assert!(
        geometry
            .report
            .items
            .iter()
            .any(|i| i.reason == reasons::GEOMETRY_APPROXIMATED.code)
    );
}

/// The lines of an axis-aligned cube shell from `lo` to `hi` with step ids from `base`; `inward`
/// winds every face against the outside.
fn cube_shell(base: u32, lo: f64, hi: f64, inward: bool) -> String {
    let c = |x: f64, y: f64, z: f64| format!("({x:?},{y:?},{z:?})");
    let corners = [
        c(lo, lo, lo),
        c(hi, lo, lo),
        c(hi, hi, lo),
        c(lo, hi, lo),
        c(lo, lo, hi),
        c(hi, lo, hi),
        c(hi, hi, hi),
        c(lo, hi, hi),
    ];
    // Outward faces by corner index (counterclockwise from outside).
    let faces: [[usize; 4]; 6] = [
        [0, 3, 2, 1],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [3, 7, 6, 2],
        [0, 4, 7, 3],
        [1, 2, 6, 5],
    ];
    let mut out = String::new();
    for (k, corner) in corners.iter().enumerate() {
        out += &format!("#{}=IFCCARTESIANPOINT({corner});\n", base + 1 + k as u32);
    }
    let mut face_ids = Vec::new();
    for (k, face) in faces.iter().enumerate() {
        let mut ring = face.to_vec();
        if inward {
            ring.reverse();
        }
        let points: Vec<String> = ring
            .iter()
            .map(|&i| format!("#{}", base + 1 + i as u32))
            .collect();
        let at = base + 20 + 3 * k as u32;
        out += &format!("#{at}=IFCPOLYLOOP(({}));\n", points.join(","));
        out += &format!("#{}=IFCFACEOUTERBOUND(#{at},.T.);\n", at + 1);
        out += &format!("#{}=IFCFACE((#{}));\n", at + 2, at + 1);
        face_ids.push(format!("#{}", at + 2));
    }
    out += &format!("#{}=IFCCLOSEDSHELL(({}));\n", base, face_ids.join(","));
    out
}

#[test]
fn faceted_brep_with_voids() {
    let lines = format!(
        "#10=IFCFACETEDBREPWITHVOIDS(#300,(#400));\n{}{}",
        cube_shell(300, 0.0, 2.0, false),
        cube_shell(400, 0.5, 1.5, true)
    );
    check(
        "faceted-brep-with-voids",
        &lines,
        8.0 - 1.0,
        24.0 + 6.0,
        1e-9,
    );
}

#[test]
fn mapped_item_with_a_non_uniform_operator() {
    let lines = "#10=IFCMAPPEDITEM(#11,#12);\n\
#11=IFCREPRESENTATIONMAP(#6,#13);\n\
#13=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#14));\n\
#14=IFCEXTRUDEDAREASOLID(#15,#6,#2,1.);\n\
#15=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,1.);\n\
#12=IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM($,$,#5,2.,$,3.,4.);\n";
    check(
        "mapped-non-uniform",
        lines,
        2.0 * 3.0 * 4.0,
        2.0 * (6.0 + 8.0 + 12.0),
        1e-9,
    );
}

/// A wall of 4 x 0.2 x 3 (x from -2 to 2, y from -0.1 to 0.1) and an opening in it, each its own
/// element, joined by `IfcRelVoidsElement`. `opening` holds the lines of the opening's body item #50
/// (placed in the wall's coordinates).
fn wall_with_opening(opening: &str) -> String {
    format!(
        "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,3.);\n#11=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,4.,0.2);\n\
{opening}\
#60=IFCPRODUCTDEFINITIONSHAPE($,$,(#61));\n#61=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#50));\n\
#62=IFCOPENINGELEMENT('open000000000000000000A',$,'Opening',$,$,$,#60,$,.OPENING.);\n\
#63=IFCRELVOIDSELEMENT('rel0000000000000000000A',$,$,$,#902,#62);\n"
    )
}

/// An opening body: `profile` extruded `depth` along y from the plane y = -0.5, with the profile's x
/// along x and its y along -z, the origin at height `z`.
fn opening_body(profile: &str, z: f64, depth: f64) -> String {
    format!(
        "#50=IFCEXTRUDEDAREASOLID(#51,#52,#53,{depth:?});\n#51={profile};\n\
#52=IFCAXIS2PLACEMENT3D(#54,#4,#3);\n#53=IFCDIRECTION((0.,0.,1.));\n#54=IFCCARTESIANPOINT((0.,-0.5,{z:?}));\n"
    )
}

#[test]
fn prism_openings_are_subtracted() {
    // A window: 1 m wide and 2 m high from z = 0.5 to 2.5... the profile is 1 x 2 centered on the
    // origin, so with the origin at z = 1.5 it spans z from 0.5 to 2.5.
    let window = wall_with_opening(&opening_body(
        "IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,2.)",
        1.5,
        1.0,
    ));
    let text = fixture(&window);
    support::dump_nonconformant("wave1-opening-window.ifc", text.as_bytes());
    let geometry = evaluate(&text);
    let (v, a) = measure_of(&geometry, 902);
    close(v, 2.4 - 0.2 * 2.0, 1e-9, "window volume");
    // Front and back lose 2 m^2 each; the reveal adds 6 m of perimeter at 0.2 m.
    close(a, 26.8 - 4.0 + 1.2, 1e-9, "window area");
    assert!(
        !geometry
            .report
            .items
            .iter()
            .any(|i| i.reason == reasons::OPENING_NOT_SUBTRACTED.code)
    );
    // A door that reaches the floor: z from 0 to 2.1 (profile 1 x 2.1 with the origin at 1.05).
    let door = wall_with_opening(&opening_body(
        "IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,2.1)",
        1.05,
        1.0,
    ));
    let text = fixture(&door);
    support::dump_nonconformant("wave1-opening-door.ifc", text.as_bytes());
    let (v, _) = measure_of(&evaluate(&text), 902);
    close(v, 2.4 - 0.2 * 2.1, 1e-9, "door volume");
    // A round opening of radius 0.4 (inscribed polygon: slightly smaller than the circle).
    let round = wall_with_opening(&opening_body(
        "IFCCIRCLEPROFILEDEF(.AREA.,$,#8,0.4)",
        1.5,
        1.0,
    ));
    let text = fixture(&round);
    support::dump_nonconformant("wave1-opening-round.ifc", text.as_bytes());
    let (v, _) = measure_of(&evaluate(&text), 902);
    close(v, 2.4 - 0.2 * PI * 0.16, 3e-3, "round opening volume");
}

#[test]
fn a_box_opening_extruded_along_the_height_cuts_through_the_thickness() {
    // The shape of a common export: closed polyline profiles (the first point repeated), the opening
    // as wide as the wall is thick, extruded upward instead of through the wall.
    let lines = "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,2.);\n\
#11=IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#12);\n\
#12=IFCPOLYLINE((#7,#13,#14,#15,#7));\n\
#13=IFCCARTESIANPOINT((0.,0.3));\n#14=IFCCARTESIANPOINT((3.,0.3));\n#15=IFCCARTESIANPOINT((3.,0.));\n\
#50=IFCEXTRUDEDAREASOLID(#51,#52,#2,1.);\n\
#51=IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#53);\n\
#53=IFCPOLYLINE((#7,#13,#56,#57,#7));\n\
#56=IFCCARTESIANPOINT((1.,0.3));\n#57=IFCCARTESIANPOINT((1.,0.));\n\
#52=IFCAXIS2PLACEMENT3D(#54,$,$);\n#54=IFCCARTESIANPOINT((1.,0.,0.5));\n\
#60=IFCPRODUCTDEFINITIONSHAPE($,$,(#61));\n#61=IFCSHAPEREPRESENTATION($,'Body','SweptSolid',(#50));\n\
#62=IFCOPENINGELEMENT('open000000000000000000A',$,'Opening',$,$,$,#60,$,.OPENING.);\n\
#63=IFCRELVOIDSELEMENT('rel0000000000000000000A',$,$,$,#902,#62);\n";
    let text = fixture(lines);
    support::dump_nonconformant("wave1-opening-box-along-height.ifc", text.as_bytes());
    let geometry = evaluate(&text);
    close(measure_of(&geometry, 902).0, 1.8 - 0.3, 1e-9, "volume");
    assert_eq!(
        loss_count(
            &geometry,
            reasons::OPENING_NOT_SUBTRACTED.code,
            "IFCRELVOIDSELEMENT"
        ),
        0
    );
}

#[test]
fn openings_that_cannot_be_subtracted_are_reported() {
    // An opening whose body is a faceted brep is subtracted by the mesh boolean: the cube
    // [0,1]^3 overlaps the wall (y in [-0.1, 0.1]) in a 1 x 0.1 x 1 box.
    let opening = |body: &str, kind: &str| {
        format!(
            "#10=IFCEXTRUDEDAREASOLID(#11,#6,#2,3.);\n#11=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,4.,0.2);\n\
{body}\
#60=IFCPRODUCTDEFINITIONSHAPE($,$,(#61));\n#61=IFCSHAPEREPRESENTATION($,'Body','{kind}',(#50));\n\
#62=IFCOPENINGELEMENT('open000000000000000000A',$,'Opening',$,$,$,#60,$,.OPENING.);\n\
#63=IFCRELVOIDSELEMENT('rel0000000000000000000A',$,$,$,#902,#62);\n"
        )
    };
    let brep = opening(
        &format!("#50=IFCFACETEDBREP(#300);\n{}", cube_shell(300, 0.0, 1.0, false)),
        "Brep",
    );
    let geometry = evaluate(&fixture(&brep));
    close(measure_of(&geometry, 902).0, 2.4 - 0.1, 1e-9, "the brep opening is cut");
    assert_eq!(
        loss_count(&geometry, reasons::OPENING_NOT_SUBTRACTED.code, "IFCRELVOIDSELEMENT"),
        0
    );
    // An opening whose body is a polyline is not a solid: the wall stays solid and it is reported.
    let line = opening(
        "#50=IFCPOLYLINE((#51,#52));\n#51=IFCCARTESIANPOINT((0.,0.,0.));\n#52=IFCCARTESIANPOINT((1.,0.,0.));\n",
        "Curve3D",
    );
    let geometry = evaluate(&fixture(&line));
    close(measure_of(&geometry, 902).0, 2.4, 1e-9, "the wall stays solid");
    assert_eq!(
        loss_count(&geometry, reasons::OPENING_NOT_SUBTRACTED.code, "IFCRELVOIDSELEMENT"),
        1
    );
    // Without the subtraction every opening is reported.
    let output = read(fixture(&brep).as_bytes(), &mut ReadOptions::default()).unwrap();
    let options = arc_io_ifc::GeometryOptions {
        subtract_openings: false,
    };
    let plain = output
        .file
        .geometry_with_options(
            &Limits::DEFAULT,
            &Cancel::new(),
            &arc_base::Tolerance::DEFAULT,
            &options,
        )
        .unwrap();
    close(measure_of(&plain, 902).0, 2.4, 1e-9, "solid wall");
}

#[test]
fn shapes_stay_meshes() {
    let geometry = evaluate(&fixture(&prism(
        "IFCCIRCLEPROFILEDEF(.AREA.,$,#8,0.5)",
        1.0,
    )));
    assert!(matches!(geometry.items[0].shape.as_ref(), Shape::Mesh(_)));
}
