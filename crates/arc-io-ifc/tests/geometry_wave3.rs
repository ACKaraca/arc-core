//! Geometry wave 3 (CORE-IO-015, #123): B-spline curves and surfaces, advanced B-reps on planes,
//! cylindrical, spherical, toroidal, swept and B-spline surfaces, and sectioned solids, each against
//! hand-computed or independently evaluated values. `ARC_IFC_DUMP=<directory>` writes every fixture
//! for `tools/ifc-oracle/geometry_compare.py`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names,
    clippy::cast_sign_loss,
    reason = "test code: fixtures and assertions"
)]

mod ifcgeo;

use std::f64::consts::{FRAC_PI_2, PI};

use arc_io_ifc::{Shape, reasons};
use ifcgeo::{
    Step, assert_watertight, check, close, dumped, fixture_with, loss_count, meshes, polylines,
    reason_count,
};

/// The point of a B-spline curve at `t` by the Cox-de Boor recurrence on the basis functions, as an
/// independent reference of the evaluation the library does with the de Boor recurrence.
fn basis(knots: &[f64], i: usize, p: usize, t: f64, last: usize) -> f64 {
    if p == 0 {
        let inside = knots[i] <= t && t < knots[i + 1];
        let end = t == knots[i + 1] && i == last;
        return f64::from(u8::from(inside || end && knots[i] < knots[i + 1]));
    }
    let left = if knots[i + p] > knots[i] {
        (t - knots[i]) / (knots[i + p] - knots[i]) * basis(knots, i, p - 1, t, last)
    } else {
        0.0
    };
    let right = if knots[i + p + 1] > knots[i + 1] {
        (knots[i + p + 1] - t) / (knots[i + p + 1] - knots[i + 1])
            * basis(knots, i + 1, p - 1, t, last)
    } else {
        0.0
    };
    left + right
}

fn curve_point(
    degree: usize,
    control: &[[f64; 3]],
    weights: &[f64],
    knots: &[f64],
    t: f64,
) -> [f64; 3] {
    let n = control.len();
    // The last non-empty span closes the domain.
    let last = (degree..n)
        .rev()
        .find(|&i| knots[i] < knots[i + 1])
        .unwrap();
    let (mut sum, mut w) = ([0.0; 3], 0.0);
    for (i, c) in control.iter().enumerate() {
        let b = basis(knots, i, degree, t, last) * weights[i];
        w += b;
        for k in 0..3 {
            sum[k] += b * c[k];
        }
    }
    [sum[0] / w, sum[1] / w, sum[2] / w]
}

fn expand(multiplicities: &[usize], values: &[f64]) -> Vec<f64> {
    multiplicities
        .iter()
        .zip(values)
        .flat_map(|(&m, &v)| std::iter::repeat_n(v, m))
        .collect()
}

fn list(values: &[f64]) -> String {
    values
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A B-spline curve item #10 with its control points; returns the lines and the item text.
fn bspline_curve(
    degree: usize,
    control: &[[f64; 3]],
    multiplicities: &[usize],
    knots: &[f64],
    weights: Option<&[f64]>,
) -> String {
    let mut s = Step::new();
    let points: Vec<String> = control
        .iter()
        .map(|c| format!("#{}", s.point(c[0], c[1], c[2])))
        .collect();
    let mult: Vec<String> = multiplicities.iter().map(ToString::to_string).collect();
    let (kind, tail) = match weights {
        Some(w) => (
            "IFCRATIONALBSPLINECURVEWITHKNOTS",
            format!(",({})", list(w)),
        ),
        None => ("IFCBSPLINECURVEWITHKNOTS", String::new()),
    };
    format!(
        "#10={kind}({degree},({}),.UNSPECIFIED.,.F.,.F.,({}),({}),.UNSPECIFIED.{tail});\n{}",
        points.join(","),
        mult.join(","),
        list(knots),
        s.lines
    )
}

/// The distance of `p` to the polyline `line`.
fn distance_to(line: &[[f64; 3]], p: [f64; 3]) -> f64 {
    let mut best = f64::MAX;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let d: Vec<f64> = (0..3).map(|k| b[k] - a[k]).collect();
        let len2: f64 = d.iter().map(|x| x * x).sum();
        let t = if len2 > 0.0 {
            ((0..3).map(|k| (p[k] - a[k]) * d[k]).sum::<f64>() / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dist = (0..3)
            .map(|k| (p[k] - a[k] - t * d[k]).powi(2))
            .sum::<f64>()
            .sqrt();
        best = best.min(dist);
    }
    best
}

fn check_curve(
    name: &str,
    degree: usize,
    control: &[[f64; 3]],
    mult: &[usize],
    knots: &[f64],
    weights: Option<&[f64]>,
) {
    let text = fixture_with(
        &bspline_curve(degree, control, mult, knots, weights),
        "#10",
        "Curve3D",
        "",
    );
    let g = dumped("wave3", name, &text);
    let lines = polylines(&g, 902);
    assert_eq!(lines.len(), 1, "{name}");
    let ours: Vec<[f64; 3]> = lines[0].iter().map(|p| [p.x, p.y, p.z]).collect();
    let expanded = expand(mult, knots);
    let ones = vec![1.0; control.len()];
    let w = weights.unwrap_or(&ones);
    let (a, b) = (expanded[degree], expanded[control.len()]);
    let reference: Vec<[f64; 3]> = (0..=20_000)
        .map(|k| {
            curve_point(
                degree,
                control,
                w,
                &expanded,
                a + (b - a) * f64::from(k) / 20_000.0,
            )
        })
        .collect();
    // Every vertex is on the curve and the polyline is within the chord tolerance of the curve.
    for p in &ours {
        assert!(
            distance_to(&reference, *p) < 1e-6,
            "{name}: vertex {p:?} off the curve"
        );
    }
    for q in reference.iter().step_by(37) {
        assert!(
            distance_to(&ours, *q) <= 1e-3 * 1.01,
            "{name}: chord deviation at {q:?}"
        );
    }
    for k in 0..3 {
        close(
            ours[0][k] + 1.0,
            reference[0][k] + 1.0,
            1e-12,
            &format!("{name} start"),
        );
        close(
            ours[ours.len() - 1][k] + 1.0,
            reference[20_000][k] + 1.0,
            1e-12,
            &format!("{name} end"),
        );
    }
}

#[test]
fn b_spline_curves_match_the_cox_de_boor_reference() {
    // A cubic Bezier.
    check_curve(
        "bspline-curve-bezier",
        3,
        &[
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 2.0, 1.0],
            [4.0, 0.0, 0.0],
        ],
        &[4, 4],
        &[0.0, 1.0],
        None,
    );
    // A quadratic with interior knots of multiplicity one and an interior double knot, over [0, 2].
    check_curve(
        "bspline-curve-knots",
        2,
        &[
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [2.0, -1.0, 0.5],
            [3.0, 1.0, 1.0],
            [4.0, 0.0, 0.0],
            [5.0, 2.0, 0.0],
        ],
        &[3, 1, 2, 3],
        &[0.0, 0.5, 1.5, 2.0],
        None,
    );
    // A rational quarter circle: every point is on the unit circle.
    let w = (0.5_f64).sqrt();
    check_curve(
        "bspline-curve-rational-circle",
        2,
        &[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        &[3, 3],
        &[0.0, 1.0],
        Some(&[1.0, w, 1.0]),
    );
    let text = fixture_with(
        &bspline_curve(
            2,
            &[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            &[3, 3],
            &[0.0, 1.0],
            Some(&[1.0, w, 1.0]),
        ),
        "#10",
        "Curve3D",
        "",
    );
    let g = dumped("wave3", "bspline-curve-rational-circle", &text);
    for p in &polylines(&g, 902)[0] {
        assert!(
            (p.coords.norm() - 1.0).abs() < 1e-12,
            "radius {}",
            p.coords.norm()
        );
    }
}

#[test]
fn malformed_b_splines_are_skipped_and_reported() {
    // Too few knots for the degree.
    let lines = bspline_curve(
        3,
        &[[0.0; 3], [1.0, 0.0, 0.0], [2.0, 1.0, 0.0], [3.0, 0.0, 0.0]],
        &[4, 3],
        &[0.0, 1.0],
        None,
    );
    // Not dumped: the knots violate the schema.
    let g = ifcgeo::evaluate(&fixture_with(&lines, "#10", "Curve3D", ""));
    assert!(polylines(&g, 902).is_empty());
    assert_eq!(
        loss_count(
            &g,
            reasons::GEOMETRY_DEGENERATE.code,
            "IFCBSPLINECURVEWITHKNOTS"
        ),
        1
    );
}

/// A bicubic Bezier surface item #10 of the parabolic cylinder z = x^2 over the unit square.
fn parabolic_patch() -> String {
    let mut s = Step::new();
    let z = [0.0, 0.0, 1.0 / 3.0, 1.0];
    let mut rows = Vec::new();
    for i in 0..4 {
        let points: Vec<String> = (0..4)
            .map(|j| {
                format!(
                    "#{}",
                    s.point(f64::from(i) / 3.0, f64::from(j) / 3.0, z[i as usize])
                )
            })
            .collect();
        rows.push(format!("({})", points.join(",")));
    }
    format!(
        "#10=IFCBSPLINESURFACEWITHKNOTS(3,3,({}),.UNSPECIFIED.,.F.,.F.,.F.,(4,4),(4,4),(0.,1.),(0.,1.),.UNSPECIFIED.);\n{}",
        rows.join(","),
        s.lines
    )
}

fn mesh_area(mesh: &arc_mesh::TriMesh) -> f64 {
    mesh.triangles()
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| mesh.positions()[i as usize]);
            (b - a).cross(&(c - a)).norm() / 2.0
        })
        .sum()
}

#[test]
fn b_spline_surface_patches_are_on_their_surface() {
    let text = fixture_with(&parabolic_patch(), "#10", "Surface3D", "");
    let g = dumped("wave3", "bspline-surface-parabolic", &text);
    let meshes = meshes(&g, 902);
    assert_eq!(meshes.len(), 1);
    let mesh = &meshes[0];
    for p in mesh.positions() {
        assert!(
            (p.z - p.x * p.x).abs() < 1e-12,
            "vertex {p:?} off the surface"
        );
        assert!((-1e-12..=1.0 + 1e-12).contains(&p.x) && (-1e-12..=1.0 + 1e-12).contains(&p.y));
    }
    // The area of z = x^2 over the unit square: the integral of sqrt(1 + 4 x^2) from 0 to 1.
    let exact = (5.0_f64.sqrt() + 0.5 * (2.0 + 5.0_f64.sqrt()).ln()) / 2.0;
    close(mesh_area(mesh), exact, 2e-3, "patch area");
    // A rational quarter cylinder of radius 1 and height 2: biquadratic, with the weights of the arc.
    let w = (0.5_f64).sqrt();
    let mut s = Step::new();
    let arc = [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let rows: Vec<String> = arc
        .iter()
        .map(|a| {
            let l = [
                s.point(a[0], a[1], 0.0),
                s.point(a[0], a[1], 1.0),
                s.point(a[0], a[1], 2.0),
            ];
            format!("(#{},#{},#{})", l[0], l[1], l[2])
        })
        .collect();
    let weights = format!(
        "(({},{},{}),({},{},{}),({},{},{}))",
        1.0, 1.0, 1.0, w, w, w, 1.0, 1.0, 1.0
    );
    let lines = format!(
        "#10=IFCRATIONALBSPLINESURFACEWITHKNOTS(2,2,({}),.UNSPECIFIED.,.F.,.F.,.F.,(3,3),(3,3),(0.,1.),(0.,1.),.UNSPECIFIED.,{weights});\n{}",
        rows.join(","),
        s.lines
    );
    let g = dumped(
        "wave3",
        "bspline-surface-rational-cylinder-validator-limit",
        &fixture_with(&lines, "#10", "Surface3D", ""),
    );
    let mesh = &meshes_of(&g)[0];
    for p in mesh.positions() {
        assert!(
            ((p.x * p.x + p.y * p.y).sqrt() - 1.0).abs() < 1e-12,
            "radius at {p:?}"
        );
    }
    close(mesh_area(mesh), PI, 3e-3, "quarter cylinder area");
}

fn meshes_of(g: &arc_io_ifc::Geometry) -> Vec<arc_mesh::TriMesh> {
    meshes(g, 902)
}

// ---- advanced B-reps ----

/// The six planar faces of the box `[lo, hi]` with straight edges; `inward` winds them against the
/// outside (the shell of a void).
fn box_faces(
    s: &mut Step,
    lo: [f64; 3],
    hi: [f64; 3],
    inward: bool,
    plain_first: bool,
) -> Vec<u64> {
    let corner = |s: &mut Step, k: usize| {
        s.vertex(
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        )
    };
    let vertices: Vec<u64> = (0..8).map(|k| corner(s, k)).collect();
    let pos = |k: usize| {
        [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    let mut edge = std::collections::HashMap::new();
    for &(a, b) in &[
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ] {
        let line = s.line(pos(a), pos(b));
        edge.insert((a, b), s.edge(vertices[a], vertices[b], line, true));
    }
    let along = |a: usize, b: usize| -> (u64, bool) {
        edge.get(&(a, b))
            .map_or_else(|| (edge[&(b, a)], false), |&e| (e, true))
    };
    // Outward faces by corner index (counterclockwise from outside) and their plane normals.
    let faces: [([usize; 4], [f64; 3], usize); 6] = [
        ([0, 2, 3, 1], [0.0, 0.0, -1.0], 0),
        ([4, 5, 7, 6], [0.0, 0.0, 1.0], 4),
        ([0, 1, 5, 4], [0.0, -1.0, 0.0], 0),
        ([2, 6, 7, 3], [0.0, 1.0, 0.0], 2),
        ([0, 4, 6, 2], [-1.0, 0.0, 0.0], 0),
        ([1, 3, 7, 5], [1.0, 0.0, 0.0], 1),
    ];
    let mut out = Vec::new();
    for (index, (corners, normal, anchor)) in faces.into_iter().enumerate() {
        if plain_first && index == 0 {
            // A plain face with a polygon loop (the published rule on the voids of an advanced B-rep
            // admits only shells that have one).
            let mut ids: Vec<u64> = corners
                .iter()
                .map(|&k| {
                    let p = pos(k);
                    s.point(p[0], p[1], p[2])
                })
                .collect();
            if inward {
                ids.reverse();
            }
            let list: Vec<String> = ids.iter().map(|i| format!("#{i}")).collect();
            let polygon = s.add(&format!("IFCPOLYLOOP(({}))", list.join(",")));
            let bound = s.add(&format!("IFCFACEOUTERBOUND(#{polygon},.T.)"));
            out.push(s.add(&format!("IFCFACE((#{bound}))")));
            continue;
        }
        let n = if inward {
            [-normal[0], -normal[1], -normal[2]]
        } else {
            normal
        };
        let reference = if n[0].abs() > 0.5 {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let plane = s.plane(pos(anchor), n, reference);
        let mut ring: Vec<(u64, bool)> = (0..4)
            .map(|k| {
                let (e, f) = along(corners[k], corners[(k + 1) % 4]);
                (e, f)
            })
            .collect();
        if inward {
            ring = ring.into_iter().rev().map(|(e, f)| (e, !f)).collect();
        }
        out.push(s.face(&[&ring], plane, true));
    }
    out
}

#[test]
fn advanced_box_and_box_with_a_void_have_their_volumes() {
    let mut s = Step::new();
    let faces = box_faces(&mut s, [0.0; 3], [2.0; 3], false, false);
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    let g = check("wave3", "advanced-box", &text, 8.0, 24.0, 1e-9);
    assert_watertight(&meshes(&g, 902)[0], "advanced box");
    // A void: the outer 2 x 2 x 2 box and an inner unit box, wound outward from the material.
    let mut s = Step::new();
    let outer = box_faces(&mut s, [0.0; 3], [2.0; 3], false, false);
    let inner = box_faces(&mut s, [0.5; 3], [1.5; 3], true, true);
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&outer)));
    let void = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&inner)));
    let lines = format!(
        "#10=IFCADVANCEDBREPWITHVOIDS(#{shell},(#{void}));\n{}",
        s.lines
    );
    let g = check("wave3", "advanced-box-void", &lines, 7.0, 30.0, 1e-9);
    assert_eq!(reason_count(&g, reasons::GEOMETRY_APPROXIMATED.code), 0);
}

fn join(ids: &[u64]) -> String {
    ids.iter()
        .map(|i| format!("#{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The cylinder of radius 1 and height 2 along z: a bottom plane, a top plane and the side with a seam.
fn cylinder_faces(s: &mut Step) -> Vec<u64> {
    let (bottom, top) = (s.vertex(1.0, 0.0, 0.0), s.vertex(1.0, 0.0, 2.0));
    let p0 = s.placement([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let p2 = s.placement([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let (c0, c2) = (s.circle(p0, 1.0), s.circle(p2, 1.0));
    let (eb, et) = (s.edge(bottom, bottom, c0, true), s.edge(top, top, c2, true));
    let seam_line = s.line([1.0, 0.0, 0.0], [1.0, 0.0, 2.0]);
    let seam = s.edge(bottom, top, seam_line, true);
    let side = s.add(&format!("IFCCYLINDRICALSURFACE(#{p0},1.)"));
    let floor = s.plane([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let roof = s.plane([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    vec![
        s.face(
            &[&[(eb, true), (seam, true), (et, false), (seam, false)]],
            side,
            true,
        ),
        s.face(&[&[(eb, false)]], floor, true),
        s.face(&[&[(et, true)]], roof, true),
    ]
}

#[test]
fn advanced_cylinder_with_a_seam_is_a_closed_solid() {
    let mut s = Step::new();
    let faces = cylinder_faces(&mut s);
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    // The inscribed polygon prism: volume and area within the chord tolerance of the cylinder.
    let g = check(
        "wave3",
        "advanced-cylinder",
        &text,
        2.0 * PI,
        6.0 * PI,
        4e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "advanced cylinder");
}

#[test]
fn advanced_cylinder_without_a_seam_uses_two_winding_loops() {
    // The side face with two bounds, each a full circle, and no seam edge.
    let mut s = Step::new();
    let (bottom, top) = (s.vertex(1.0, 0.0, 0.0), s.vertex(1.0, 0.0, 2.0));
    let p0 = s.placement([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let p2 = s.placement([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let (c0, c2) = (s.circle(p0, 1.0), s.circle(p2, 1.0));
    let (eb, et) = (s.edge(bottom, bottom, c0, true), s.edge(top, top, c2, true));
    let side = s.add(&format!("IFCCYLINDRICALSURFACE(#{p0},1.)"));
    let floor = s.plane([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let roof = s.plane([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let faces = [
        s.face(&[&[(eb, true)], &[(et, false)]], side, true),
        s.face(&[&[(eb, false)]], floor, true),
        s.face(&[&[(et, true)]], roof, true),
    ];
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    let g = check(
        "wave3",
        "advanced-cylinder-annular",
        &text,
        2.0 * PI,
        6.0 * PI,
        4e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "annular cylinder");
}

#[test]
fn advanced_hemisphere_closes_through_the_pole() {
    let mut s = Step::new();
    let equator = s.vertex(1.0, 0.0, 0.0);
    let p0 = s.placement([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let c = s.circle(p0, 1.0);
    let e = s.edge(equator, equator, c, true);
    let sphere = s.add(&format!("IFCSPHERICALSURFACE(#{p0},1.)"));
    let floor = s.plane([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let faces = [
        s.face(&[&[(e, true)]], sphere, true),
        s.face(&[&[(e, false)]], floor, true),
    ];
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    let g = check(
        "wave3",
        "advanced-hemisphere",
        &text,
        2.0 * PI / 3.0,
        3.0 * PI,
        6e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "hemisphere");
}

#[test]
fn advanced_sphere_octant_has_a_pole_vertex() {
    // The octant of the unit ball: a spherical triangle with the pole (0, 0, 1) as a vertex and three
    // planar quarter discs.
    let mut s = Step::new();
    let (a, b, n, o) = (
        s.vertex(1.0, 0.0, 0.0),
        s.vertex(0.0, 1.0, 0.0),
        s.vertex(0.0, 0.0, 1.0),
        s.vertex(0.0, 0.0, 0.0),
    );
    let equator = s.placement([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let meridian_b = s.placement([0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let meridian_a = s.placement([0.0; 3], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    let (c_ab, c_bn, c_an) = (
        s.circle(equator, 1.0),
        s.circle(meridian_b, 1.0),
        s.circle(meridian_a, 1.0),
    );
    let (ab, bn, an) = (
        s.edge(a, b, c_ab, true),
        s.edge(b, n, c_bn, true),
        s.edge(a, n, c_an, true),
    );
    let (la, lb, ln) = (
        s.line([0.0; 3], [1.0, 0.0, 0.0]),
        s.line([0.0; 3], [0.0, 1.0, 0.0]),
        s.line([0.0; 3], [0.0, 0.0, 1.0]),
    );
    let (oa, ob, on) = (
        s.edge(o, a, la, true),
        s.edge(o, b, lb, true),
        s.edge(o, n, ln, true),
    );
    let sphere = s.add(&format!("IFCSPHERICALSURFACE(#{equator},1.)"));
    let floor = s.plane([0.0; 3], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let west = s.plane([0.0; 3], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let south = s.plane([0.0; 3], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    let faces = [
        s.face(&[&[(ab, true), (bn, true), (an, false)]], sphere, true),
        s.face(&[&[(ob, true), (ab, false), (oa, false)]], floor, true),
        s.face(&[&[(on, true), (bn, false), (ob, false)]], west, true),
        s.face(&[&[(oa, true), (an, true), (on, false)]], south, true),
    ];
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    let g = check(
        "wave3",
        "advanced-sphere-octant",
        &text,
        PI / 6.0,
        4.0 * PI / 8.0 + 3.0 * PI / 4.0,
        6e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "sphere octant");
}

#[test]
fn advanced_torus_bend_is_a_pipe_elbow() {
    // A quarter of a torus of major radius 3 and minor radius 1: the tube between u = 0 and u = pi / 2.
    let mut s = Step::new();
    let (v0, v1) = (s.vertex(4.0, 0.0, 0.0), s.vertex(0.0, 4.0, 0.0));
    let q0 = s.placement([3.0, 0.0, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    let q1 = s.placement([0.0, 3.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let (c0, c1) = (s.circle(q0, 1.0), s.circle(q1, 1.0));
    let (e0, e1) = (s.edge(v0, v0, c0, true), s.edge(v1, v1, c1, true));
    let origin = s.placement([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let torus = s.add(&format!("IFCTOROIDALSURFACE(#{origin},3.,1.)"));
    let cap0 = s.plane([3.0, 0.0, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    let cap1 = s.plane([0.0, 3.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let faces = [
        s.face(&[&[(e0, true)], &[(e1, false)]], torus, true),
        s.face(&[&[(e0, true)]], cap0, true),
        s.face(&[&[(e1, false)]], cap1, true),
    ];
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    let g = check(
        "wave3",
        "advanced-torus-bend",
        &text,
        PI * 3.0 * FRAC_PI_2,
        0.0,
        6e-3,
    );
    assert_watertight(&meshes(&g, 902)[0], "torus bend");
}

#[test]
fn advanced_extrusion_and_b_spline_faces() {
    // A half cylinder: the side is a surface of linear extrusion of a half circle.
    let mut s = Step::new();
    let (a, b) = (s.vertex(1.0, 0.0, 0.0), s.vertex(-1.0, 0.0, 0.0));
    let (a2, b2) = (s.vertex(1.0, 0.0, 2.0), s.vertex(-1.0, 0.0, 2.0));
    let p0 = s.placement([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let p2 = s.placement([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let (c0, c2) = (s.circle(p0, 1.0), s.circle(p2, 1.0));
    let arc0 = s.edge(a, b, c0, true);
    let arc2 = s.edge(a2, b2, c2, true);
    let base = s.line([-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
    let top = s.line([-1.0, 0.0, 2.0], [1.0, 0.0, 2.0]);
    let diameter0 = s.edge(b, a, base, true);
    let diameter2 = s.edge(b2, a2, top, true);
    let (la, lb) = (
        s.line([1.0, 0.0, 0.0], [1.0, 0.0, 2.0]),
        s.line([-1.0, 0.0, 0.0], [-1.0, 0.0, 2.0]),
    );
    let (right, left) = (s.edge(a, a2, la, true), s.edge(b, b2, lb, true));
    // The swept curve: the half circle as a trimmed curve in the profile plane.
    let origin2 = s.add("IFCCARTESIANPOINT((0.,0.))");
    let placement2 = s.add(&format!("IFCAXIS2PLACEMENT2D(#{origin2},$)"));
    let flat = s.circle(placement2, 1.0);
    let trimmed = s.add(&format!(
        "IFCTRIMMEDCURVE(#{flat},(IFCPARAMETERVALUE(0.)),(IFCPARAMETERVALUE({PI:?})),.T.,.PARAMETER.)"
    ));
    let profile = s.add(&format!("IFCARBITRARYOPENPROFILEDEF(.CURVE.,$,#{trimmed})"));
    let identity = s.placement([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let up = s.direction(0.0, 0.0, 1.0);
    let swept = s.add(&format!(
        "IFCSURFACEOFLINEAREXTRUSION(#{profile},#{identity},#{up},2.)"
    ));
    let floor = s.plane([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let roof = s.plane([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let wall = s.plane([0.0, 0.0, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    let faces = [
        s.face(
            &[&[(arc0, true), (left, true), (arc2, false), (right, false)]],
            swept,
            true,
        ),
        s.face(&[&[(arc0, false), (diameter0, false)]], floor, true),
        s.face(&[&[(arc2, true), (diameter2, true)]], roof, true),
        s.face(
            &[&[
                (diameter0, true),
                (right, true),
                (diameter2, false),
                (left, false),
            ]],
            wall,
            true,
        ),
    ];
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    let g = check("wave3", "advanced-extrusion-surface", &text, PI, 0.0, 6e-3);
    assert_watertight(&meshes(&g, 902)[0], "half cylinder");
}

#[test]
fn advanced_b_spline_top_face() {
    // The solid under z = x^2 over the unit square: a B-spline top, four planar sides (the side at
    // x = 0 has no height), and the bottom.
    let mut s = Step::new();
    let z = [0.0, 0.0, 1.0 / 3.0, 1.0];
    let mut rows = Vec::new();
    for i in 0..4 {
        let points: Vec<String> = (0..4)
            .map(|j| {
                format!(
                    "#{}",
                    s.point(f64::from(i) / 3.0, f64::from(j) / 3.0, z[i as usize])
                )
            })
            .collect();
        rows.push(format!("({})", points.join(",")));
    }
    let surface = s.add(&format!(
        "IFCBSPLINESURFACEWITHKNOTS(3,3,({}),.UNSPECIFIED.,.F.,.F.,.F.,(4,4),(4,4),(0.,1.),(0.,1.),.UNSPECIFIED.)",
        rows.join(",")
    ));
    let v = [
        s.vertex(0.0, 0.0, 0.0),
        s.vertex(1.0, 0.0, 0.0),
        s.vertex(1.0, 1.0, 0.0),
        s.vertex(0.0, 1.0, 0.0),
        s.vertex(1.0, 0.0, 1.0),
        s.vertex(1.0, 1.0, 1.0),
    ];
    // The parabola above y = 0 and y = 1 as a cubic B-spline curve in the plane y.
    let parabola = |s: &mut Step, y: f64| {
        let pts: Vec<String> = [
            [0.0, 0.0],
            [1.0 / 3.0, 0.0],
            [2.0 / 3.0, 1.0 / 3.0],
            [1.0, 1.0],
        ]
        .iter()
        .map(|c| format!("#{}", s.point(c[0], y, c[1])))
        .collect();
        s.add(&format!(
            "IFCBSPLINECURVEWITHKNOTS(3,({}),.UNSPECIFIED.,.F.,.F.,(4,4),(0.,1.),.UNSPECIFIED.)",
            pts.join(",")
        ))
    };
    let (pa, pb) = (parabola(&mut s, 0.0), parabola(&mut s, 1.0));
    let front_curve = s.edge(v[0], v[4], pa, true);
    let back_curve = s.edge(v[3], v[5], pb, true);
    let line = |s: &mut Step, a: usize, b: usize, p: [[f64; 3]; 2]| {
        let l = s.line(p[0], p[1]);
        s.edge(v[a], v[b], l, true)
    };
    let e01 = line(&mut s, 0, 1, [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]);
    let e12 = line(&mut s, 1, 2, [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]);
    let e23 = line(&mut s, 2, 3, [[1.0, 1.0, 0.0], [0.0, 1.0, 0.0]]);
    let e30 = line(&mut s, 3, 0, [[0.0, 1.0, 0.0], [0.0, 0.0, 0.0]]);
    let e14 = line(&mut s, 1, 4, [[1.0, 0.0, 0.0], [1.0, 0.0, 1.0]]);
    let e25 = line(&mut s, 2, 5, [[1.0, 1.0, 0.0], [1.0, 1.0, 1.0]]);
    let e45 = line(&mut s, 4, 5, [[1.0, 0.0, 1.0], [1.0, 1.0, 1.0]]);
    let floor = s.plane([0.0; 3], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let front = s.plane([0.0; 3], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    let back = s.plane([0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
    let end = s.plane([1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let faces = [
        s.face(
            &[&[(e01, false), (e30, false), (e23, false), (e12, false)]],
            floor,
            true,
        ),
        s.face(
            &[&[(e01, true), (e14, true), (front_curve, false)]],
            front,
            true,
        ),
        s.face(
            &[&[(e23, true), (back_curve, true), (e25, false)]],
            back,
            true,
        ),
        s.face(
            &[&[(e12, true), (e25, true), (e45, false), (e14, false)]],
            end,
            true,
        ),
        s.face(
            &[&[
                (front_curve, true),
                (e45, true),
                (back_curve, false),
                (e30, true),
            ]],
            surface,
            true,
        ),
    ];
    let shell = s.add(&format!("IFCCLOSEDSHELL(({}))", join(&faces)));
    let text = format!("#10=IFCADVANCEDBREP(#{shell});\n{}", s.lines);
    let g = check("wave3", "advanced-bspline-top", &text, 1.0 / 3.0, 0.0, 3e-3);
    assert_watertight(&meshes(&g, 902)[0], "b-spline top");
    assert!(matches!(g.items[0].shape.as_ref(), Shape::Mesh(_)));
}

#[test]
fn sectioned_solids_loft_their_cross_sections() {
    // A frustum: a 1 x 1 square at z = 0 and a 2 x 2 square at z = 3, as an IfcSectionedSpine.
    let lines = "#10=IFCSECTIONEDSPINE(#11,(#12,#13),(#14,#15));\n\
#11=IFCCOMPOSITECURVE((#16),.F.);\n#16=IFCCOMPOSITECURVESEGMENT(.CONTINUOUS.,.T.,#17);\n\
#17=IFCPOLYLINE((#18,#19));\n#18=IFCCARTESIANPOINT((0.,0.,0.));\n#19=IFCCARTESIANPOINT((0.,0.,3.));\n\
#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,1.);\n#13=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,2.,2.);\n\
#14=IFCAXIS2PLACEMENT3D(#18,$,$);\n#15=IFCAXIS2PLACEMENT3D(#19,$,$);\n";
    let g = check("wave3", "sectioned-spine", lines, 7.0, 0.0, 1e-9);
    assert_watertight(&meshes(&g, 902)[0], "frustum");
    // Sections with different point counts are not evaluated and reported.
    let bad = lines.replace(
        "#13=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,2.,2.);",
        "#13=IFCCIRCLEPROFILEDEF(.AREA.,$,#8,1.);",
    );
    let g = ifcgeo::evaluate(&fixture_with(&bad, "#10", "SectionedSpine", ""));
    assert!(meshes(&g, 902).is_empty());
    assert_eq!(
        loss_count(
            &g,
            reasons::GEOMETRY_NOT_EVALUATED.code,
            "IFCSECTIONEDSPINE"
        ),
        1
    );
    // The horizontal form: a 1 x 1 section along a straight directrix of 10 m, raised by 1 m and moved
    // 2 m to the left at the far end: an oblique prism of the same volume.
    let horizontal = "#10=IFCSECTIONEDSOLIDHORIZONTAL(#11,(#12,#13),(#14,#15));\n\
#11=IFCPOLYLINE((#16,#17));\n#16=IFCCARTESIANPOINT((0.,0.,0.));\n#17=IFCCARTESIANPOINT((10.,0.,0.));\n\
#12=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,1.);\n#13=IFCRECTANGLEPROFILEDEF(.AREA.,$,#8,1.,1.);\n\
#14=IFCAXIS2PLACEMENTLINEAR(#18,$,$);\n#15=IFCAXIS2PLACEMENTLINEAR(#19,$,$);\n\
#18=IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(0.),0.,0.,$,#11);\n\
#19=IFCPOINTBYDISTANCEEXPRESSION(IFCLENGTHMEASURE(10.),2.,1.,$,#11);\n";
    let text = fixture_with(horizontal, "#10", "AdvancedSweptSolid", "")
        .replace("(('IFC4'))", "(('IFC4X3_ADD2'))");
    let g = dumped("wave3", "sectioned-solid-horizontal", &text);
    close(ifcgeo::measure(&g).0, 10.0, 1e-9, "horizontal volume");
    assert_eq!(
        loss_count(
            &g,
            reasons::GEOMETRY_APPROXIMATED.code,
            "IFCSECTIONEDSOLIDHORIZONTAL"
        ),
        1
    );
    let mesh = &meshes(&g, 902)[0];
    let ys: Vec<f64> = mesh.positions().iter().map(|p| p.y).collect();
    close(
        ys.iter().cloned().fold(f64::MIN, f64::max),
        2.5,
        1e-12,
        "far end to the left",
    );
    let zs: Vec<f64> = mesh.positions().iter().map(|p| p.z).collect();
    close(
        zs.iter().cloned().fold(f64::MIN, f64::max),
        1.5,
        1e-12,
        "far end raised",
    );
}
