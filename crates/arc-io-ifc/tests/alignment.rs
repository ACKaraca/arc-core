//! Alignment curves and linear placement: positions and tangents against closed forms and
//! independent numerical integration, the layouts against the representation, errors and limits.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::float_cmp,
    reason = "test code: fixtures and assertions"
)]

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use arc_base::{Cancel, Limits, Tolerance};
use arc_io_ifc::alignment::Curve;
use arc_io_ifc::{IfcFile, ReadOptions, read};

fn file(data: &str, length_unit: &str) -> IfcFile {
    let text = format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('a.ifc','',(),(),'','','');\n\
         FILE_SCHEMA(('IFC4X3_ADD2'));\nENDSEC;\nDATA;\n\
         #1=IFCPROJECT('p',$,'P',$,$,$,$,$,#2);\n#2=IFCUNITASSIGNMENT((#3,#4));\n{length_unit}\n\
         #4=IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.);\n{data}\nENDSEC;\nEND-ISO-10303-21;\n"
    );
    read(text.as_bytes(), &mut ReadOptions::default())
        .unwrap()
        .file
}

const METRE: &str = "#3=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);";

/// A 2D segment: placement at (`x`, `y`) with reference direction (`dx`, `dy`), `start`, `len`,
/// and the parent curve record text. Names from `n` upward.
#[allow(
    clippy::too_many_arguments,
    reason = "a fixture builder: placement, trim and parent"
)]
fn segment(n: u64, x: f64, y: f64, dx: f64, dy: f64, start: f64, len: f64, parent: &str) -> String {
    format!(
        "#{a}=IFCCARTESIANPOINT(({x:?},{y:?}));\n#{b}=IFCDIRECTION(({dx:?},{dy:?}));\n\
         #{c}=IFCAXIS2PLACEMENT2D(#{a},#{b});\n{parent_record}\n\
         #{e}=IFCCURVESEGMENT(.CONTINUOUS.,#{c},IFCLENGTHMEASURE({start:?}),IFCLENGTHMEASURE({len:?}),#{p});\n",
        a = n,
        b = n + 1,
        c = n + 2,
        p = n + 3,
        e = n + 4,
        parent_record = parent.replace("{p}", &(n + 3).to_string()),
    )
}

const LINE: &str = "#{p}=IFCLINE(#2000,#2001);";
const BASE: &str = "#2000=IFCCARTESIANPOINT((0.,0.));\n#2002=IFCDIRECTION((1.,0.));\n\
#2001=IFCVECTOR(#2002,1.);\n";

fn near(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b}");
}

fn curve(file: &IfcFile, name: u64) -> Curve {
    file.curve(name, &Limits::DEFAULT, &Cancel::new()).unwrap()
}

#[test]
fn line_and_circle_follow_their_closed_forms() {
    let quarter = FRAC_PI_2 * 100.0;
    let data = format!(
        "{BASE}{}{}#100=IFCCOMPOSITECURVE((#14,#24),.F.);\n#2100=IFCCIRCLE(#2003,100.);\n#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n",
        segment(10, 0.0, 0.0, 1.0, 0.0, 0.0, 100.0, LINE),
        segment(
            20,
            100.0,
            0.0,
            1.0,
            0.0,
            0.0,
            quarter,
            "#{p}=IFCCIRCLE(#2003,100.);"
        ),
    );
    let f = file(&data, METRE);
    let c = curve(&f, 100);
    near(c.length(), 100.0 + quarter, 1e-9, "length");
    let p = c.evaluate(60.0, 1e-6).unwrap();
    near(p.position.x, 60.0, 1e-12, "line x");
    near(p.tangent.x, 1.0, 1e-12, "line tangent");
    let p = c.evaluate(100.0 + quarter / 2.0, 1e-6).unwrap();
    near(p.position.x, 100.0 + 100.0 * FRAC_PI_4.sin(), 1e-9, "arc x");
    near(p.position.y, 100.0 - 100.0 * FRAC_PI_4.cos(), 1e-9, "arc y");
    near(p.tangent.y, FRAC_PI_4.sin(), 1e-9, "arc tangent");
    // The frame: X tangent, Y left, Z up.
    let m = p.transform.matrix();
    near(m[(0, 1)], -FRAC_PI_4.sin(), 1e-9, "left x");
    near(m[(2, 2)], 1.0, 1e-12, "up");
    let end = c.evaluate(c.length() + 5e-7, 1e-6).unwrap();
    near(end.position.x, 200.0, 1e-9, "end x");
    near(end.position.y, 100.0, 1e-9, "end y");
    let error = c.evaluate(c.length() + 1.0, 1e-6).unwrap_err();
    assert_eq!(error.code(), "io.ifc.distance_out_of_range");
    assert_eq!(
        c.evaluate(-1.0, 1e-6).unwrap_err().code(),
        "io.ifc.distance_out_of_range"
    );
    assert_eq!(
        c.evaluate(f64::NAN, 1e-6).unwrap_err().code(),
        "io.ifc.distance_out_of_range"
    );
}

/// The point at length `s` of a curve with heading `theta`, by Simpson's rule on 20000 steps.
fn integrate(theta: impl Fn(f64) -> f64, s: f64) -> (f64, f64) {
    let n = 20_000;
    let h = s / f64::from(n);
    let (mut x, mut y) = (0.0, 0.0);
    for i in 0..=n {
        let w = if i == 0 || i == n {
            1.0
        } else if i % 2 == 1 {
            4.0
        } else {
            2.0
        };
        let t = theta(h * f64::from(i));
        x += w * t.cos();
        y += w * t.sin();
    }
    (x * h / 3.0, y * h / 3.0)
}

#[test]
fn spirals_follow_numerical_integration_of_their_heading() {
    // Each spiral on its own segment, 60 long, from the origin along +x.
    type Heading = Box<dyn Fn(f64) -> f64>;
    let cases: [(&str, Heading); 5] = [
        (
            "IFCCLOTHOID(#2003,150.)",
            Box::new(|s| s * s / (2.0 * 150.0 * 150.0)),
        ),
        (
            // curvature s^2 / A2^3 + 1 / A0
            "IFCSECONDORDERPOLYNOMIALSPIRAL(#2003,80.,$,500.)",
            Box::new(|s| s.powi(3) / (3.0 * 80.0_f64.powi(3)) + s / 500.0),
        ),
        (
            // curvature A3 s^3 / |A3|^5 + s^2 / A2^3 + A1 s / |A1|^3 + 1 / A0
            "IFCTHIRDORDERPOLYNOMIALSPIRAL(#2003,-90.,70.,-120.,400.)",
            Box::new(|s| {
                -s.powi(4) / (4.0 * 90.0_f64.powi(4)) + s.powi(3) / (3.0 * 70.0_f64.powi(3))
                    - s * s / (2.0 * 120.0 * 120.0)
                    + s / 400.0
            }),
        ),
        (
            // CosineTerm A1 = 800, ConstantTerm A0 = -800: kappa = 1/A0 + cos(pi s / L) / A1
            "IFCCOSINESPIRAL(#2003,800.,-800.)",
            Box::new(|s| -s / 800.0 + 60.0 / (PI * 800.0) * (PI * s / 60.0).sin()),
        ),
        (
            // theta = s/A0 + (1/2) sgn(A1) (s/A1)^2 - L / (2 pi A2) (cos(2 pi s / L) - 1)
            "IFCSINESPIRAL(#2003,-3000.,200.,$)",
            Box::new(|s| {
                0.5 * (s / 200.0).powi(2)
                    + 60.0 / (2.0 * PI * 3000.0) * ((2.0 * PI * s / 60.0).cos() - 1.0)
            }),
        ),
    ];
    for (parent, theta) in &cases {
        let data = format!(
            "{BASE}#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n#2004=IFCSEVENTHORDERPOLYNOMIALSPIRAL(#2003,$,$,$,$,$,$,$,1000.);\n{}#100=IFCCOMPOSITECURVE((#14),.F.);\n",
            segment(
                10,
                5.0,
                7.0,
                1.0,
                0.0,
                0.0,
                60.0,
                &format!("#{{p}}={parent};")
            ),
        );
        let f = file(&data, METRE);
        let c = curve(&f, 100);
        for s in [0.0, 12.5, 30.0, 59.0, 60.0] {
            let (x, y) = integrate(theta, s);
            let p = c.evaluate(s, 1e-6).unwrap();
            near(p.position.x, 5.0 + x, 1e-8, parent);
            near(p.position.y, 7.0 + y, 1e-8, parent);
            let h = theta(s);
            near(p.tangent.x, h.cos(), 1e-8, parent);
            near(p.tangent.y, h.sin(), 1e-8, parent);
        }
    }
    // A seventh order spiral with only the constant term is a circle of radius 1000.
    let data = format!(
        "{BASE}#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n{}#100=IFCCOMPOSITECURVE((#14),.F.);\n",
        segment(
            10,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            100.0,
            "#{p}=IFCSEVENTHORDERPOLYNOMIALSPIRAL(#2003,$,$,$,$,$,$,$,1000.);"
        ),
    );
    let c = curve(&file(&data, METRE), 100);
    let p = c.evaluate(100.0, 1e-6).unwrap();
    near(p.position.x, 1000.0 * (0.1_f64).sin(), 1e-9, "septic arc x");
    near(
        p.position.y,
        1000.0 * (1.0 - (0.1_f64).cos()),
        1e-9,
        "septic arc y",
    );
}

#[test]
fn negative_length_reverses_the_sense_and_clockwise_arcs_turn_right() {
    // A circle segment of negative length on a circle runs clockwise: heading +x, turning right.
    let data = format!(
        "{BASE}#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n{}#100=IFCCOMPOSITECURVE((#14),.F.);\n",
        segment(
            10,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            -PI * 50.0,
            "#{p}=IFCCIRCLE(#2003,100.);"
        ),
    );
    let c = curve(&file(&data, METRE), 100);
    assert_eq!(c.length(), PI * 50.0);
    let p = c.evaluate(PI * 50.0, 1e-6).unwrap();
    near(p.position.x, 100.0, 1e-9, "x");
    near(p.position.y, -100.0, 1e-9, "y");
    near(p.tangent.y, -1.0, 1e-9, "tangent");
}

/// Arc length of the parabola y = 0.02 x - 0.0001 x^2 for x in [0, 100].
fn parabola_length() -> f64 {
    let n = 20_000;
    let h = 100.0 / f64::from(n);
    (0..n)
        .map(|i| {
            let x = h * (f64::from(i) + 0.5);
            (1.0 + (0.02 - 0.0002 * x).powi(2)).sqrt() * h
        })
        .sum()
}

fn gradient_fixture(length_unit: &str) -> IfcFile {
    let slope = 0.02_f64;
    let line_len = 100.0 * (1.0 + slope * slope).sqrt();
    let data = format!(
        "{BASE}#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n{}#100=IFCCOMPOSITECURVE((#14),.F.);\n\
         {}{}#300=IFCGRADIENTCURVE((#34,#44),.F.,#100,$);\n",
        segment(10, 0.0, 0.0, 1.0, 0.0, 0.0, 200.0, LINE),
        segment(30, 0.0, 10.0, 1.0, slope, 0.0, line_len, LINE),
        segment(
            40,
            100.0,
            12.0,
            1.0,
            slope,
            0.0,
            parabola_length(),
            "#{p}=IFCPOLYNOMIALCURVE(#2003,(0.,1.),(0.,0.02,-0.0001),$);"
        ),
    );
    file(&data, length_unit)
}

#[test]
fn a_gradient_curve_gives_the_height_at_the_distance_along_the_plan() {
    let c = curve(&gradient_fixture(METRE), 300);
    assert!(c.has_gradient());
    for (u, z, m) in [
        (0.0, 10.0, 0.02),
        (50.0, 11.0, 0.02),
        (100.0, 12.0, 0.02),
        (
            150.0,
            12.0 + 0.02 * 50.0 - 0.0001 * 2500.0,
            0.02 - 0.0002 * 50.0,
        ),
        (
            199.0,
            12.0 + 0.02 * 99.0 - 0.0001 * 99.0 * 99.0,
            0.02 - 0.0002 * 99.0,
        ),
    ] {
        let p = c.evaluate(u, 1e-6).unwrap();
        near(p.position.x, u, 1e-9, "x");
        near(p.position.z, z, 1e-7, "z");
        near(p.tangent.z / p.tangent.x, m, 1e-7, "slope");
    }
    // Up is perpendicular to the sloping tangent and has a positive z.
    let p = c.evaluate(50.0, 1e-6).unwrap();
    let m = p.transform.matrix();
    near(
        m[(0, 2)] * m[(0, 0)] + m[(1, 2)] * m[(1, 0)] + m[(2, 2)] * m[(2, 0)],
        0.0,
        1e-12,
        "orthogonal",
    );
    assert!(m[(2, 2)] > 0.99);
}

#[test]
fn lengths_are_converted_to_meters() {
    let mm = "#3=IFCCONVERSIONBASEDUNIT($,.LENGTHUNIT.,'FOOT',#5);\n\
              #5=IFCMEASUREWITHUNIT(IFCLENGTHMEASURE(0.3048),#6);\n#6=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);";
    let c = curve(&gradient_fixture(mm), 300);
    near(c.length(), 200.0 * 0.3048, 1e-9, "length");
    let p = c.evaluate(100.0 * 0.3048, 1e-6).unwrap();
    near(p.position.x, 30.48, 1e-9, "x");
    near(p.position.z, 12.0 * 0.3048, 1e-9, "z");
}

fn layout_fixture() -> String {
    String::from(
        "#10=IFCALIGNMENT('a',$,$,$,$,$,$,$);\n\
         #11=IFCALIGNMENTHORIZONTAL('h',$,$,$,$,$,$);\n#12=IFCALIGNMENTVERTICAL('v',$,$,$,$,$,$);\n\
         #13=IFCALIGNMENTCANT('c',$,$,$,$,$,$,1.5);\n\
         #14=IFCRELNESTS('r1',$,$,$,#10,(#11,#12,#13));\n\
         #20=IFCCARTESIANPOINT((0.,0.));\n\
         #21=IFCALIGNMENTHORIZONTALSEGMENT($,$,#20,0.,0.,0.,100.,$,.LINE.);\n\
         #22=IFCALIGNMENTSEGMENT('s1',$,$,$,$,$,$,#21);\n\
         #23=IFCCARTESIANPOINT((100.,0.));\n\
         #24=IFCALIGNMENTHORIZONTALSEGMENT($,$,#23,0.,300.,300.,150.,$,.CIRCULARARC.);\n\
         #25=IFCALIGNMENTSEGMENT('s2',$,$,$,$,$,$,#24);\n\
         #26=IFCRELNESTS('r2',$,$,$,#11,(#22,#25));\n\
         #30=IFCALIGNMENTVERTICALSEGMENT($,$,0.,250.,10.,0.01,0.01,$,.CONSTANTGRADIENT.);\n\
         #31=IFCALIGNMENTSEGMENT('s3',$,$,$,$,$,$,#30);\n#32=IFCRELNESTS('r3',$,$,$,#12,(#31));\n\
         #40=IFCALIGNMENTCANTSEGMENT($,$,100.,150.,0.,0.,0.12,0.12,.CONSTANTCANT.);\n\
         #41=IFCALIGNMENTSEGMENT('s4',$,$,$,$,$,$,#40);\n#42=IFCRELNESTS('r4',$,$,$,#13,(#41));\n",
    )
}

#[test]
fn an_alignment_layout_gives_plan_height_and_cant() {
    let f = file(&layout_fixture(), METRE);
    let all = f.alignments(&Limits::DEFAULT, &Cancel::new()).unwrap();
    assert_eq!(all.items.len(), 1);
    assert!(all.items[0].representation.is_none());
    let c = all.items[0].layout.as_ref().unwrap();
    near(c.length(), 250.0, 1e-9, "length");
    assert!(c.has_gradient() && c.has_cant());
    let p = c.evaluate(50.0, 1e-6).unwrap();
    near(p.position.z, 10.5, 1e-9, "height before the cant");
    // On the arc the cant raises the centerline by half the right rail and tilts left by
    // asin(0.12 / 1.5).
    let p = c.evaluate(150.0, 1e-6).unwrap();
    let angle: f64 = 50.0 / 300.0;
    near(p.position.x, 100.0 + 300.0 * angle.sin(), 1e-9, "x");
    near(p.position.y, 300.0 * (1.0 - angle.cos()), 1e-9, "y");
    near(
        p.position.z,
        10.0 + 150.0 * 0.01 + 0.06,
        1e-9,
        "raised height",
    );
    let tilt = (0.12_f64 / 1.5).asin();
    let m = p.transform.matrix();
    // Z leans toward the horizontal left of the tangent.
    let h = p.tangent.x.hypot(p.tangent.y);
    let left = (-p.tangent.y / h, p.tangent.x / h);
    let lean = (m[(0, 2)] * left.0 + m[(1, 2)] * left.1).asin();
    near(lean, tilt, 1e-3, "tilt");
    // The segment kinds are reported.
    let kinds = c.segment_kinds(150.0);
    assert_eq!(kinds.plan.as_deref(), Some("CIRCULARARC"));
    assert_eq!(kinds.gradient.as_deref(), Some("CONSTANTGRADIENT"));
    assert_eq!(kinds.cant.as_deref(), Some("Constant"));
}

#[test]
fn linear_placement_resolves_offsets_axes_and_the_parent_placement() {
    let data = format!(
        "{BASE}#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n{}#100=IFCCOMPOSITECURVE((#14),.F.);\n\
         #50=IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(50.),2.,1.,0.5,#100);\n\
         #51=IFCAXIS2PLACEMENTLINEAR(#50,$,$);\n#52=IFCLINEARPLACEMENT($,#51,$);\n\
         #53=IFCDIRECTION((0.,1.,0.));\n#54=IFCAXIS2PLACEMENTLINEAR(#50,$,#53);\n\
         #55=IFCCARTESIANPOINT((1000.,0.,0.));\n#56=IFCAXIS2PLACEMENT3D(#55,$,$);\n\
         #57=IFCLOCALPLACEMENT($,#56);\n#58=IFCLINEARPLACEMENT(#57,#54,$);\n",
        segment(10, 0.0, 0.0, 1.0, 0.0, 0.0, 100.0, LINE),
    );
    let f = file(&data, METRE);
    let mut resolver = f
        .linear_resolver(&Limits::DEFAULT, &Cancel::new(), &Tolerance::DEFAULT)
        .unwrap();
    let t = resolver.placement(52).unwrap();
    let m = t.matrix();
    near(m[(0, 3)], 50.5, 1e-12, "x");
    near(m[(1, 3)], 2.0, 1e-12, "lateral is left");
    near(m[(2, 3)], 1.0, 1e-12, "vertical");
    near(m[(0, 0)], 1.0, 1e-12, "x axis along the tangent");
    // RefDirection (0, 1, 0) in the curve frame points the local X to the left of the curve.
    let t = resolver.placement(58).unwrap();
    let m = t.matrix();
    near(m[(0, 3)], 1050.5, 1e-12, "x with parent");
    near(m[(1, 0)], 1.0, 1e-12, "x axis to the left");
    near(m[(0, 1)], -1.0, 1e-12, "y axis");
    near(m[(2, 2)], 1.0, 1e-12, "z axis");
    // The curve is read once.
    assert!(std::sync::Arc::ptr_eq(
        &resolver.curve(100).unwrap(),
        &resolver.curve(100).unwrap()
    ));
}

#[test]
fn a_distance_beyond_the_curve_falls_back_to_the_cartesian_position() {
    let data = format!(
        "{BASE}#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n{}#100=IFCCOMPOSITECURVE((#14),.F.);\n\
         #50=IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(500.),$,$,$,#100);\n\
         #51=IFCAXIS2PLACEMENTLINEAR(#50,$,$);\n\
         #55=IFCCARTESIANPOINT((7.,8.,9.));\n#56=IFCAXIS2PLACEMENT3D(#55,$,$);\n\
         #52=IFCLINEARPLACEMENT($,#51,#56);\n",
        segment(10, 0.0, 0.0, 1.0, 0.0, 0.0, 100.0, LINE),
    );
    let f = file(&data, METRE);
    let mut resolver = f
        .linear_resolver(&Limits::DEFAULT, &Cancel::new(), &Tolerance::DEFAULT)
        .unwrap();
    assert_eq!(
        resolver.placement(52).unwrap_err().code(),
        "io.ifc.distance_out_of_range"
    );
    let t = resolver.placement_with_fallback(52).unwrap();
    near(t.matrix()[(0, 3)], 7.0, 1e-12, "fallback x");
}

#[test]
fn hostile_input_returns_errors() {
    let limits = Limits::DEFAULT;
    let cancel = Cancel::new();
    // A segment without a parent curve.
    let f = file(
        "#10=IFCCOMPOSITECURVE((#11),.F.);\n#11=IFCCURVESEGMENT(.CONTINUOUS.,#12,IFCLENGTHMEASURE(0.),IFCLENGTHMEASURE(1.),$);\n\
         #12=IFCAXIS2PLACEMENT2D(#13,$);\n#13=IFCCARTESIANPOINT((0.,0.));\n",
        METRE,
    );
    assert_eq!(
        f.curve(10, &limits, &cancel).unwrap_err().code(),
        "io.ifc.curve_invalid"
    );
    // A parent curve the evaluator does not cover.
    let f = file(
        "#10=IFCCOMPOSITECURVE((#11),.F.);\n#11=IFCCURVESEGMENT(.CONTINUOUS.,#12,IFCLENGTHMEASURE(0.),IFCLENGTHMEASURE(1.),#14);\n\
         #12=IFCAXIS2PLACEMENT2D(#13,$);\n#13=IFCCARTESIANPOINT((0.,0.));\n#14=IFCTRIMMEDCURVE($,$,$,$,.T.,.CARTESIAN.);\n",
        METRE,
    );
    assert_eq!(
        f.curve(10, &limits, &cancel).unwrap_err().code(),
        "io.ifc.curve_unsupported"
    );
    // The alignment keeps going and reports it.
    let f = file(
        "#20=IFCALIGNMENT('a',$,$,$,$,$,#21,$);\n#21=IFCPRODUCTDEFINITIONSHAPE($,$,(#22));\n\
         #22=IFCSHAPEREPRESENTATION($,'Axis','Curve2D',(#10));\n\
         #10=IFCCOMPOSITECURVE((#11),.F.);\n#11=IFCCURVESEGMENT(.CONTINUOUS.,#12,IFCLENGTHMEASURE(0.),IFCLENGTHMEASURE(1.),#14);\n\
         #12=IFCAXIS2PLACEMENT2D(#13,$);\n#13=IFCCARTESIANPOINT((0.,0.));\n#14=IFCTRIMMEDCURVE($,$,$,$,.T.,.CARTESIAN.);\n",
        METRE,
    );
    let all = f.alignments(&limits, &cancel).unwrap();
    assert!(all.items[0].representation.is_none());
    assert_eq!(all.report.items.len(), 2);
    // A zero clothoid constant, a zero radius, a zero direction.
    for parent in ["IFCCLOTHOID(#2003,0.)", "IFCCIRCLE(#2003,0.)"] {
        let f = file(
            &format!(
                "{BASE}#2003=IFCAXIS2PLACEMENT2D(#2000,$);\n{}#100=IFCCOMPOSITECURVE((#14),.F.);\n",
                segment(
                    10,
                    0.0,
                    0.0,
                    1.0,
                    0.0,
                    0.0,
                    10.0,
                    &format!("#{{p}}={parent};")
                )
            ),
            METRE,
        );
        assert_eq!(
            f.curve(100, &limits, &cancel).unwrap_err().code(),
            "io.ifc.curve_invalid"
        );
    }
    let f = file(
        &format!(
            "{BASE}{}#100=IFCCOMPOSITECURVE((#14),.F.);\n",
            segment(10, 0.0, 0.0, 0.0, 0.0, 0.0, 10.0, LINE)
        ),
        METRE,
    );
    assert_eq!(
        f.curve(100, &limits, &cancel).unwrap_err().code(),
        "io.ifc.curve_invalid"
    );
    // A cycle of placements and a cancelled token.
    let f = file(
        "#50=IFCLINEARPLACEMENT(#51,#53,$);\n#51=IFCLINEARPLACEMENT(#50,#53,$);\n\
         #53=IFCAXIS2PLACEMENTLINEAR(#54,$,$);\n#54=IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(0.),$,$,$,#99);\n",
        METRE,
    );
    let mut resolver = f
        .linear_resolver(&limits, &cancel, &Tolerance::DEFAULT)
        .unwrap();
    assert!(resolver.placement(50).is_err());
    let stop = Cancel::new();
    stop.cancel();
    assert_eq!(
        f.curve(10, &limits, &stop).unwrap_err().code(),
        "base.cancelled"
    );
    // A segment list longer than array_length.
    let tiny = Limits {
        array_length: 2,
        ..Limits::DEFAULT
    };
    let f = file("#10=IFCCOMPOSITECURVE((#11,#11,#11),.F.);\n", METRE);
    assert_eq!(
        f.curve(10, &tiny, &cancel).unwrap_err().code(),
        "io.limit_exceeded"
    );
}
