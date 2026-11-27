//! Geometry wave 2 (CORE-IO-015, #123): general booleans, CSG, openings of any shape, offset curves,
//! sweeps with parameters, tapered revolutions and geometric sets, each against hand-computed
//! volumes and areas. `ARC_IFC_DUMP=<directory>` writes every fixture for
//! `tools/ifc-oracle/geometry_compare.py`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    reason = "test code: fixtures and assertions"
)]

mod ifcgeo;

use std::f64::consts::{PI, TAU};

use arc_io_ifc::reasons;
use ifcgeo::{
    Step, assert_watertight, check, close, dumped, evaluate, evaluate_with, fixture, fixture_with,
    loss_count, measure, meshes, polylines, reason_count, support,
};

/// An extruded box: lower corner `lo`, size `size`, as the item `#name` (profile `#name+1`, placement
/// `#name+2`, point `#name+3`).
fn block(name: u32, lo: [f64; 3], size: [f64; 3]) -> String {
    let (n, p, a, c) = (name, name + 1, name + 2, name + 3);
    format!(
        "#{n}=IFCEXTRUDEDAREASOLID(#{p},#{a},#2,{:?});\n\
#{p}=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,{:?},{:?});\n\
#{a}=IFCAXIS2PLACEMENT3D(#{c},$,$);\n#{c}=IFCCARTESIANPOINT(({:?},{:?},{:?}));\n",
        size[2],
        size[0],
        size[1],
        lo[0] + size[0] / 2.0,
        lo[1] + size[1] / 2.0,
        lo[2]
    )
}

fn boolean(operator: &str, a: &str, b: &str) -> String {
    format!("#10=IFCBOOLEANRESULT(.{operator}.,#20,#30);\n{a}{b}")
}

#[test]
fn boolean_of_two_boxes_has_the_analytic_volumes() {
    // A is [0,2]^3 and B is [1,3]^3: the overlap is a unit cube.
    let a = block(20, [0.0; 3], [2.0; 3]);
    let b = block(30, [1.0; 3], [2.0; 3]);
    let g = check(
        "wave2",
        "bool-difference",
        &boolean("DIFFERENCE", &a, &b),
        7.0,
        0.0,
        1e-9,
    );
    // The difference is the cube with a unit corner removed: the same surface area as the cube.
    close(measure(&g).1, 24.0, 1e-9, "difference area");
    assert_watertight(&meshes(&g, 902)[0], "difference");
    let g = check(
        "wave2",
        "bool-union",
        &boolean("UNION", &a, &b),
        15.0,
        0.0,
        1e-9,
    );
    close(measure(&g).1, 48.0 - 6.0 + 0.0, 1e-9, "union area");
    check(
        "wave2",
        "bool-intersection",
        &boolean("INTERSECTION", &a, &b),
        1.0,
        6.0,
        1e-9,
    );
    assert_eq!(reason_count(&g, reasons::GEOMETRY_APPROXIMATED.code), 0);
}

#[test]
fn boolean_with_coplanar_and_touching_faces() {
    let a = block(20, [0.0; 3], [2.0; 3]);
    // Shares the face x = 2.
    let touching = block(30, [2.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    check(
        "wave2",
        "bool-touching-union",
        &boolean("UNION", &a, &touching),
        16.0,
        40.0,
        1e-9,
    );
    check(
        "wave2",
        "bool-touching-difference",
        &boolean("DIFFERENCE", &a, &touching),
        8.0,
        24.0,
        1e-9,
    );
    // A notch with three coplanar faces.
    let notch = block(30, [1.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
    check(
        "wave2",
        "bool-notch",
        &boolean("DIFFERENCE", &a, &notch),
        4.0,
        16.0,
        1e-9,
    );
    // A slot through the whole height.
    let slot = block(30, [0.5, 0.5, -1.0], [1.0, 1.0, 4.0]);
    check(
        "wave2",
        "bool-slot",
        &boolean("DIFFERENCE", &a, &slot),
        8.0 - 2.0,
        30.0,
        1e-9,
    );
}

#[test]
fn cylinder_minus_a_box_through_it() {
    // A cylinder of radius 1 and height 2 along z, minus the slab |x| <= 0.25 through it.
    let lines = "#10=IFCBOOLEANRESULT(.DIFFERENCE.,#20,#30);\n\
#20=IFCRIGHTCIRCULARCYLINDER(#6,2.,1.);\n\
#30=IFCBLOCK(#31,0.5,4.,4.);\n#31=IFCAXIS2PLACEMENT3D(#32,$,$);\n#32=IFCCARTESIANPOINT((-0.25,-2.,-1.));\n";
    let a = 0.25_f64;
    let slab = 2.0 * (a * (1.0 - a * a).sqrt() + a.asin());
    let expected = (PI - slab) * 2.0;
    let g = check("wave2", "cylinder-minus-box", lines, expected, 0.0, 2e-3);
    for mesh in meshes(&g, 902) {
        assert_watertight(&mesh, "cylinder minus box");
    }
}

#[test]
fn booleans_of_extruded_circles_and_spheres_match_the_oracle_kinds() {
    // The extruded form of the cylinder, which every evaluator reads, minus the same slab.
    let lines = format!(
        "#10=IFCBOOLEANRESULT(.DIFFERENCE.,#20,#30);\n\
#20=IFCEXTRUDEDAREASOLID(#21,#6,#2,2.);\n#21=IFCCIRCLEPROFILEDEF(.AREA.,$,#8,1.);\n{}",
        block(30, [-0.25, -2.0, -1.0], [0.5, 4.0, 4.0])
    );
    let a = 0.25_f64;
    let slab = 2.0 * (a * (1.0 - a * a).sqrt() + a.asin());
    let g = check(
        "wave2",
        "bool-extruded-circle-minus-box",
        &lines,
        (PI - slab) * 2.0,
        0.0,
        3e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "extruded circle minus box");
    let lines = format!(
        "#10=IFCBOOLEANRESULT(.UNION.,#20,#30);\n\
#20=IFCEXTRUDEDAREASOLID(#21,#6,#2,2.);\n#21=IFCCIRCLEPROFILEDEF(.AREA.,$,#8,1.);\n{}",
        block(30, [0.0, -0.5, 1.0], [2.0, 1.0, 2.0])
    );
    // The block overlaps the cylinder in x in [0, 1] (to the circle), z in [1, 2].
    let overlap = {
        let (h, r) = (0.5_f64, 1.0_f64);
        // Area of the disk inside x >= 0 and |y| <= 0.5.
        let quarter = |y: f64| y * (r * r - y * y).sqrt() / 2.0 + r * r * (y / r).asin() / 2.0;
        2.0 * quarter(h)
    };
    check(
        "wave2",
        "bool-extruded-circle-union-box",
        &lines,
        PI * 2.0 + 2.0 * 1.0 * 2.0 - overlap * 1.0,
        0.0,
        3e-3,
    );
    // An octant of a sphere: the block [0,2]^3 intersected with the sphere of radius 1 at the origin.
    let lines = format!(
        "#10=IFCBOOLEANRESULT(.INTERSECTION.,#20,#30);\n{}#30=IFCSPHERE(#6,1.);\n",
        block(20, [0.0; 3], [2.0; 3])
    );
    check(
        "wave2",
        "bool-sphere-octant",
        &lines,
        4.0 / 3.0 * PI / 8.0,
        0.0,
        5e-3,
    );
}

#[test]
fn csg_primitives_have_their_volumes() {
    let primitive = |text: &str, volume: f64, area: f64, name: &str| {
        check("wave2", name, &format!("#10={text};\n"), volume, area, 3e-3);
    };
    primitive(
        "IFCBLOCK(#6,2.,3.,4.)",
        24.0,
        2.0 * (6.0 + 8.0 + 12.0),
        "csg-block",
    );
    primitive(
        "IFCRECTANGULARPYRAMID(#6,2.,3.,6.)",
        12.0,
        6.0 + (36.0_f64 + 2.25).sqrt() * 2.0 + 3.0 * (36.0_f64 + 1.0).sqrt(),
        "csg-pyramid",
    );
    primitive(
        "IFCRIGHTCIRCULARCONE(#6,3.,1.)",
        PI / 3.0 * 3.0,
        PI + PI * 1.0 * (10.0_f64).sqrt(),
        "csg-cone",
    );
    primitive(
        "IFCRIGHTCIRCULARCYLINDER(#6,2.,1.)",
        2.0 * PI,
        2.0 * PI + 4.0 * PI,
        "csg-cylinder",
    );
    // The sphere is inscribed: the volume deficit is below three chord tolerances over the radius.
    let text = fixture("#10=IFCSPHERE(#6,1.);\n");
    support::dump_nonconformant("wave2-csg-sphere.ifc", text.as_bytes());
    let g = evaluate(&text);
    let v = measure(&g).0;
    let exact = 4.0 / 3.0 * PI;
    assert!(
        v < exact && v > exact * (1.0 - 3.0 * 1e-3 * 3.0),
        "sphere volume {v} vs {exact}"
    );
    assert_watertight(&meshes(&g, 902)[0], "sphere");
}

#[test]
fn csg_solid_tree_is_its_root_expression() {
    // A cube with a spherical bite: the block [0,2]^3 minus a sphere of radius 1 at its corner (0,0,0).
    let lines = "#10=IFCCSGSOLID(#11);\n#11=IFCBOOLEANRESULT(.DIFFERENCE.,#12,#13);\n\
#12=IFCBLOCK(#6,2.,2.,2.);\n#13=IFCSPHERE(#6,1.);\n";
    let g = check(
        "wave2",
        "csg-solid-tree",
        lines,
        8.0 - 4.0 / 3.0 * PI / 8.0,
        0.0,
        5e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "csg tree");
    // Nested trees: (block - sphere) union a pyramid on top.
    let lines = "#10=IFCCSGSOLID(#11);\n#11=IFCBOOLEANRESULT(.UNION.,#14,#15);\n\
#14=IFCBOOLEANRESULT(.DIFFERENCE.,#12,#13);\n#12=IFCBLOCK(#6,2.,2.,2.);\n#13=IFCSPHERE(#6,1.);\n\
#15=IFCRECTANGULARPYRAMID(#16,2.,2.,3.);\n#16=IFCAXIS2PLACEMENT3D(#17,$,$);\n#17=IFCCARTESIANPOINT((0.,0.,2.));\n";
    let g = check(
        "wave2",
        "csg-solid-tree-union",
        lines,
        8.0 - 4.0 / 3.0 * PI / 8.0 + 4.0,
        0.0,
        5e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "csg union");
}

/// The wall #902 (a 4 x 0.2 x 3 box) with the opening #903 whose body is `body` (the item #50).
fn wall_with_opening(body: &str) -> String {
    let wall = block(10, [0.0, -0.1, 0.0], [4.0, 0.2, 3.0]);
    let rep = if body.contains("IFCEXTRUDEDAREASOLID") {
        "SweptSolid"
    } else {
        "CSG"
    };
    let extra = format!(
        "#903=IFCOPENINGELEMENT('0OpenAAAAAAAAAAAAAAAA1',$,'Opening',$,$,#104,#910,$,$,.OPENING.);\n\
#910=IFCPRODUCTDEFINITIONSHAPE($,$,(#911));\n#911=IFCSHAPEREPRESENTATION(#103,'Body','{rep}',(#50));\n\
#920=IFCRELVOIDSELEMENT('0VoidAAAAAAAAAAAAAAAA1',$,$,$,#902,#903);\n{body}"
    );
    fixture_with(&wall, "#10", "SweptSolid", &extra)
}

#[test]
fn openings_of_any_body_are_subtracted() {
    // A round hole through the wall: a cylinder along y of radius 0.5.
    let round = "#50=IFCRIGHTCIRCULARCYLINDER(#51,0.6,0.5);\n\
#51=IFCAXIS2PLACEMENT3D(#52,#4,#3);\n#52=IFCCARTESIANPOINT((2.,-0.3,1.5));\n";
    let text = wall_with_opening(round);
    let g = dumped("wave2", "opening-round", &text);
    let hole = PI * 0.25 * 0.2;
    let (v, _) = measure(&g);
    close(v, 4.0 * 0.2 * 3.0 - hole, 3e-3, "round opening volume");
    assert_eq!(
        loss_count(
            &g,
            reasons::OPENING_NOT_SUBTRACTED.code,
            "IFCRELVOIDSELEMENT"
        ),
        0
    );
    assert_watertight(&meshes(&g, 902)[0], "wall with a round hole");
    // The wall is solid when the openings are not subtracted.
    let solid = evaluate_with(&text, false);
    close(measure(&solid).0, 2.4, 1e-9, "solid wall");
    // A sphere centered on the face y = -0.1 whose zone between y = -0.1 and 0.1 lies in the wall.
    let bore = "#50=IFCSPHERE(#51,0.4);\n#51=IFCAXIS2PLACEMENT3D(#52,$,$);\n#52=IFCCARTESIANPOINT((2.,-0.1,1.5));\n";
    let g = dumped("wave2", "opening-sphere", &wall_with_opening(bore));
    let (r, h) = (0.4_f64, 0.2_f64);
    let zone = PI * (r * r * h - h * h * h / 3.0);
    close(measure(&g).0, 2.4 - zone, 5e-3, "spherical bore volume");
}

#[test]
fn skew_extrusion_opening_and_an_opening_in_a_brep_wall_use_the_mesh_boolean() {
    // A round opening extruded obliquely through the wall: not a prism perpendicular to its profile.
    let tilted = "#50=IFCEXTRUDEDAREASOLID(#51,#52,#53,1.4);\n#51=IFCCIRCLEPROFILEDEF(.AREA.,$,#8,0.4);\n\
#52=IFCAXIS2PLACEMENT3D(#54,#4,#3);\n#53=IFCDIRECTION((0.3,0.,1.));\n#54=IFCCARTESIANPOINT((2.,-0.6,1.5));\n";
    let text = wall_with_opening(tilted);
    let g = dumped("wave2", "opening-skew-extrusion", &text);
    let hole = PI * 0.16 * 0.2;
    close(measure(&g).0, 2.4 - hole, 3e-2, "skew opening volume");
    assert_eq!(
        loss_count(
            &g,
            reasons::OPENING_NOT_SUBTRACTED.code,
            "IFCRELVOIDSELEMENT"
        ),
        0
    );
    assert_watertight(&meshes(&g, 902)[0], "wall with a skew hole");
}

#[test]
fn opening_in_a_brep_wall_and_a_profile_opening_are_subtracted() {
    // The wall is a faceted B-rep box; the opening is a block placed through it.
    let mut b = Step::new();
    let c: Vec<u64> = [
        [0.0, -0.1, 0.0],
        [4.0, -0.1, 0.0],
        [4.0, 0.1, 0.0],
        [0.0, 0.1, 0.0],
        [0.0, -0.1, 3.0],
        [4.0, -0.1, 3.0],
        [4.0, 0.1, 3.0],
        [0.0, 0.1, 3.0],
    ]
    .iter()
    .map(|p| b.point(p[0], p[1], p[2]))
    .collect();
    let mut faces = Vec::new();
    for f in [
        [0, 3, 2, 1],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [3, 7, 6, 2],
        [0, 4, 7, 3],
        [1, 2, 6, 5],
    ] {
        let points: Vec<String> = f.iter().map(|&k| format!("#{}", c[k])).collect();
        let l = b.add(&format!("IFCPOLYLOOP(({}))", points.join(",")));
        let o = b.add(&format!("IFCFACEOUTERBOUND(#{l},.T.)"));
        faces.push(format!("#{}", b.add(&format!("IFCFACE((#{o}))"))));
    }
    let shell = b.add(&format!("IFCCLOSEDSHELL(({}))", faces.join(",")));
    let brep = b.add(&format!("IFCFACETEDBREP(#{shell})"));
    let lines = format!("#10=IFCFACETEDBREP(#{shell});\n{}", b.lines);
    let _ = brep;
    let extra = "#903=IFCOPENINGELEMENT('0OpenAAAAAAAAAAAAAAAA1',$,'Opening',$,$,#104,#910,$,$,.OPENING.);\n\
#910=IFCPRODUCTDEFINITIONSHAPE($,$,(#911));\n#911=IFCSHAPEREPRESENTATION(#103,'Body','CSG',(#50));\n\
#920=IFCRELVOIDSELEMENT('0VoidAAAAAAAAAAAAAAAA1',$,$,$,#902,#903);\n\
#50=IFCBLOCK(#51,1.,1.,1.);\n#51=IFCAXIS2PLACEMENT3D(#52,$,$);\n#52=IFCCARTESIANPOINT((1.,-0.5,1.));\n";
    let text = fixture_with(&lines, "#10", "Brep", extra);
    let g = dumped("wave2", "opening-brep-wall", &text);
    close(
        measure(&g).0,
        2.4 - 1.0 * 0.2 * 1.0,
        1e-9,
        "brep wall with a block opening",
    );
    assert_eq!(
        loss_count(
            &g,
            reasons::OPENING_NOT_SUBTRACTED.code,
            "IFCRELVOIDSELEMENT"
        ),
        0
    );
}

#[test]
fn offset_curves_are_parallel_to_their_basis() {
    let line = "#10=IFCOFFSETCURVE2D(#11,1.,.F.);\n#11=IFCPOLYLINE((#12,#13));\n\
#12=IFCCARTESIANPOINT((0.,0.));\n#13=IFCCARTESIANPOINT((4.,0.));\n";
    let g = dumped(
        "wave2",
        "offset-line",
        &fixture_with(line, "#10", "Curve2D", ""),
    );
    let lines = polylines(&g, 902);
    assert_eq!(lines.len(), 1);
    for p in &lines[0] {
        close(p.y, 1.0, 1e-12, "offset to the left");
    }
    close(lines[0][1].x - lines[0][0].x, 4.0, 1e-12, "length");
    // A corner is mitered: the offset of an L is an L at distance 1 from both legs.
    let l = "#10=IFCOFFSETCURVE2D(#11,-1.,.F.);\n#11=IFCPOLYLINE((#12,#13,#14));\n\
#12=IFCCARTESIANPOINT((0.,0.));\n#13=IFCCARTESIANPOINT((4.,0.));\n#14=IFCCARTESIANPOINT((4.,4.));\n";
    let g = dumped(
        "wave2",
        "offset-corner",
        &fixture_with(l, "#10", "Curve2D", ""),
    );
    let pts = &polylines(&g, 902)[0];
    close(pts[0].y, -1.0, 1e-12, "first leg");
    close(pts[1].x, 5.0, 1e-12, "miter x");
    close(pts[1].y, -1.0, 1e-12, "miter y");
    close(pts[2].x, 5.0, 1e-12, "second leg");
    // A circle of radius 2 offset by 1 to the left (inside, as it runs counterclockwise) has radius 1
    // at the edges' midpoints.
    let c = "#10=IFCOFFSETCURVE2D(#11,1.,.F.);\n#11=IFCCIRCLE(#8,2.);\n";
    let g = dumped(
        "wave2",
        "offset-circle",
        &fixture_with(c, "#10", "Curve2D", ""),
    );
    let pts = &polylines(&g, 902)[0];
    for w in pts.windows(2) {
        let mid = (w[0].coords + w[1].coords) / 2.0;
        let r = mid.norm();
        assert!((r - 1.0).abs() < 2e-3 && mid.z == 0.0, "offset radius {r}");
    }
    // The 3D form along RefDirection x tangent: with the reference direction +z the left of +x is +y;
    // with +y it is -z.
    let up = "#10=IFCOFFSETCURVE3D(#11,2.,.F.,#4);\n#11=IFCPOLYLINE((#12,#13));\n\
#12=IFCCARTESIANPOINT((0.,0.,1.));\n#13=IFCCARTESIANPOINT((4.,0.,1.));\n";
    let g = dumped(
        "wave2",
        "offset-3d",
        &fixture_with(up, "#10", "Curve3D", ""),
    );
    let pts = &polylines(&g, 902)[0];
    close(pts[0].z, -1.0, 1e-12, "3D offset along ref x tangent");
    // By distances: lateral 0 at the start to 1 at 4 m, and 0.5 up everywhere.
    let by = "#10=IFCOFFSETCURVEBYDISTANCES(#11,(#14,#15),$);\n#11=IFCPOLYLINE((#12,#13));\n\
#12=IFCCARTESIANPOINT((0.,0.,0.));\n#13=IFCCARTESIANPOINT((4.,0.,0.));\n\
#14=IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(0.),0.,0.5,$,#11);\n#15=IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(4.),1.,0.5,$,#11);\n";
    let text = fixture_with(by, "#10", "Curve3D", "").replace("(('IFC4'))", "(('IFC4X3_ADD2'))");
    let g = dumped("wave2", "offset-by-distances", &text);
    let pts = &polylines(&g, 902)[0];
    close(pts[1].y, 1.0, 1e-12, "lateral at the end");
    close(pts[1].z, 0.5, 1e-12, "vertical");
}

#[test]
fn geometric_sets_are_curves_and_points_not_bodies() {
    let set = "#10=IFCGEOMETRICCURVESET((#11,#14,#15));\n#11=IFCPOLYLINE((#12,#13));\n\
#12=IFCCARTESIANPOINT((0.,0.,0.));\n#13=IFCCARTESIANPOINT((1.,0.,0.));\n\
#14=IFCCARTESIANPOINT((5.,5.,5.));\n#15=IFCCIRCLE(#6,1.);\n";
    let g = dumped(
        "wave2",
        "geometric-curve-set",
        &fixture_with(set, "#10", "GeometricCurveSet", ""),
    );
    assert!(meshes(&g, 902).is_empty());
    let lines = polylines(&g, 902);
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[1],
        vec![arc_base::math::Point3::new(5.0, 5.0, 5.0); 2]
    );
    assert!(lines[2].len() > 8);
    // A point as an item of its own.
    let point = "#10=IFCCARTESIANPOINT((1.,2.,3.));\n";
    let g = dumped(
        "wave2",
        "point-item",
        &fixture_with(point, "#10", "Point", ""),
    );
    assert_eq!(
        polylines(&g, 902),
        vec![vec![arc_base::math::Point3::new(1.0, 2.0, 3.0); 2]]
    );
    // A surface element is reported and the rest is kept.
    let mixed =
        "#10=IFCGEOMETRICSET((#11,#12));\n#11=IFCPLANE(#6);\n#12=IFCCARTESIANPOINT((1.,2.,3.));\n";
    let g = dumped(
        "wave2",
        "geometric-set-surface",
        &fixture_with(mixed, "#10", "GeometricSet", ""),
    );
    assert_eq!(polylines(&g, 902).len(), 1);
    assert_eq!(
        loss_count(&g, reasons::GEOMETRY_NOT_EVALUATED.code, "IFCPLANE"),
        1
    );
}

#[test]
fn sweeps_honor_start_and_end_parameters() {
    // A 0.2 x 0.1 rectangle swept along a polyline of three points; the parameters 0 and 1 keep the
    // first segment (length 3).
    let sweep = |params: &str| {
        format!(
            "#10=IFCSURFACECURVESWEPTAREASOLID(#11,#16,#12,{params},#17);\n#17=IFCPLANE(#6);\n#16=IFCAXIS2PLACEMENT3D(#5,#3,#4);\n\
#11=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,0.2,0.1);\n#12=IFCPOLYLINE((#13,#14,#15));\n\
#13=IFCCARTESIANPOINT((0.,0.,0.));\n#14=IFCCARTESIANPOINT((3.,0.,0.));\n#15=IFCCARTESIANPOINT((3.,2.,0.));\n"
        )
    };
    let first = check(
        "wave2",
        "sweep-params-first",
        &sweep("0.,1."),
        0.02 * 3.0,
        0.0,
        1e-9,
    );
    assert_eq!(
        reason_count(&first, reasons::GEOMETRY_NOT_EVALUATED.code),
        0
    );
    // From 0.5 to 1.5: 1.5 m of the first segment and 1 m of the second, mitered at the corner.
    let (v, _) = measure(&dumped(
        "wave2",
        "sweep-params-mitered",
        &fixture(&sweep("0.5,1.5")),
    ));
    close(v, 0.02 * 2.5, 1e-9, "mitered volume");
    // A swept disk along a circle of radius 2 from 0 to a quarter turn.
    let disk = "#10=IFCSWEPTDISKSOLID(#11,0.1,$,0.,1.5707963267949);\n#11=IFCCIRCLE(#6,2.);\n";
    let g = check(
        "wave2",
        "swept-disk-params",
        disk,
        PI * 0.01 * 2.0 * PI / 2.0,
        0.0,
        2e-2,
    );
    assert_eq!(reason_count(&g, reasons::GEOMETRY_NOT_EVALUATED.code), 0);
}

#[test]
fn tapered_revolution_blends_the_profile_over_the_turn() {
    // The start profile is x in [1, 2], z in [0, 1]; the end profile is x in [1, 3], z in [0, 1].
    let lines = "#10=IFCREVOLVEDAREASOLIDTAPERED(#11,#6,#14,1.5707963267949,#12);\n\
#11=IFCRECTANGLEPROFILEDEF(.AREA.,$,#15,1.,1.);\n#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#16,2.,1.);\n\
#14=IFCAXIS1PLACEMENT(#5,#4);\n#15=IFCAXIS2PLACEMENT2D(#17,$);\n#16=IFCAXIS2PLACEMENT2D(#18,$);\n\
#17=IFCCARTESIANPOINT((1.5,0.5));\n#18=IFCCARTESIANPOINT((2.,0.5));\n";
    // The area at fraction s is 1 + s with its centroid at radius 1.5 + s / 2.
    let expected = PI / 2.0 * (1.5 + 1.0 + 1.0 / 6.0);
    check("wave2", "revolution-tapered", lines, expected, 0.0, 5e-3);
}

#[test]
fn the_openings_flag_leaves_the_wall_solid() {
    let round = "#50=IFCRIGHTCIRCULARCYLINDER(#51,0.6,0.5);\n\
#51=IFCAXIS2PLACEMENT3D(#52,#4,#3);\n#52=IFCCARTESIANPOINT((2.,-0.3,1.5));\n";
    let text = wall_with_opening(round);
    let g = evaluate_with(&text, false);
    assert_eq!(
        loss_count(
            &g,
            reasons::OPENING_NOT_SUBTRACTED.code,
            "IFCRELVOIDSELEMENT"
        ),
        1
    );
    let _ = (TAU, Step::new);
}

#[test]
fn a_face_without_area_is_left_out_of_a_faceted_brep_and_reported() {
    // A unit cube shell plus a face of three collinear points: the shell is evaluated without it.
    let mut b = Step::new();
    let c: Vec<u64> = (0..8)
        .map(|k| {
            b.point(
                f64::from(k & 1),
                f64::from((k >> 1) & 1),
                f64::from((k >> 2) & 1),
            )
        })
        .collect();
    let mut faces = Vec::new();
    for f in [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ] {
        let points: Vec<String> = f.iter().map(|&k| format!("#{}", c[k])).collect();
        let l = b.add(&format!("IFCPOLYLOOP(({}))", points.join(",")));
        let o = b.add(&format!("IFCFACEOUTERBOUND(#{l},.T.)"));
        faces.push(format!("#{}", b.add(&format!("IFCFACE((#{o}))"))));
    }
    let middle = b.point(0.5, 0.0, 0.0);
    let flat = b.add(&format!("IFCPOLYLOOP((#{},#{middle},#{}))", c[0], c[1]));
    let bound = b.add(&format!("IFCFACEOUTERBOUND(#{flat},.T.)"));
    faces.push(format!("#{}", b.add(&format!("IFCFACE((#{bound}))"))));
    let shell = b.add(&format!("IFCCLOSEDSHELL(({}))", faces.join(",")));
    let lines = format!("#10=IFCFACETEDBREP(#{shell});\n{}", b.lines);
    let g = dumped(
        "wave2",
        "brep-face-without-area",
        &fixture_with(&lines, "#10", "Brep", ""),
    );
    close(measure(&g).0, 1.0, 1e-9, "the cube");
    assert_eq!(
        loss_count(&g, reasons::GEOMETRY_DEGENERATE.code, "IFCFACE"),
        1
    );
    assert_eq!(
        loss_count(&g, reasons::GEOMETRY_APPROXIMATED.code, "IFCFACETEDBREP"),
        1
    );
}
