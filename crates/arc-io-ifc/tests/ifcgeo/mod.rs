//! Fixtures and measurements shared by the geometry wave 2 and 3 tests.

#![allow(dead_code, reason = "each test file uses part of the helpers")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test code"
)]

use arc_base::{Cancel, Limits};
use arc_io_ifc::{Geometry, GeometryOptions, ReadOptions, Shape, read};
use arc_mesh::TriMesh;
use arc_mesh::measure::{surface_area, volume};

#[path = "../support/mod.rs"]
pub mod support;

pub const HEADER: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition []'),'2;1');\n\
FILE_NAME('wave.ifc','',(''),(''),'','','');\nFILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n";

/// A project in meters with the common axes (#2 z, #3 x, #4 y, #5 origin, #6 the world placement, #8
/// the 2D one) and one element (#902) whose body representation holds the items `items`
/// (`#10`, by default) of `lines`; `extra` adds further lines after the element.
pub fn fixture_with(lines: &str, items: &str, representation: &str, extra: &str) -> String {
    format!(
        "{HEADER}#1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'Demo',$,$,$,$,(#103),#100);\n\
#103=IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-5,#6,$);\n\
#104=IFCLOCALPLACEMENT($,#6);\n\
#100=IFCUNITASSIGNMENT((#101,#102));\n\
#101=IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.);\n\
#102=IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.);\n\
#2=IFCDIRECTION((0.,0.,1.));\n#3=IFCDIRECTION((1.,0.,0.));\n#4=IFCDIRECTION((0.,1.,0.));\n\
#5=IFCCARTESIANPOINT((0.,0.,0.));\n#6=IFCAXIS2PLACEMENT3D(#5,$,$);\n\
#7=IFCCARTESIANPOINT((0.,0.));\n#8=IFCAXIS2PLACEMENT2D(#7,$);\n\
{lines}\
#900=IFCPRODUCTDEFINITIONSHAPE($,$,(#901));\n\
#901=IFCSHAPEREPRESENTATION(#103,'Body','{representation}',({items}));\n\
#902=IFCBUILDINGELEMENTPROXY('0CaseAAAAAAAAAAAAAAAA1',$,'Case',$,$,#104,#900,$,$);\n\
{extra}ENDSEC;\nEND-ISO-10303-21;\n"
    )
}

/// The representation type the schema allows for the item #10 of `lines`.
pub fn representation_of(lines: &str) -> &'static str {
    let item = lines
        .split("#10=")
        .nth(1)
        .and_then(|rest| rest.split('(').next())
        .unwrap_or("");
    match item {
        "IFCBOOLEANCLIPPINGRESULT" => "Clipping",
        "IFCBOOLEANRESULT"
        | "IFCBLOCK"
        | "IFCRECTANGULARPYRAMID"
        | "IFCRIGHTCIRCULARCONE"
        | "IFCRIGHTCIRCULARCYLINDER"
        | "IFCSPHERE"
        | "IFCCSGSOLID" => "CSG",
        "IFCSWEPTDISKSOLID"
        | "IFCSURFACECURVESWEPTAREASOLID"
        | "IFCFIXEDREFERENCESWEPTAREASOLID"
        | "IFCREVOLVEDAREASOLIDTAPERED"
        | "IFCEXTRUDEDAREASOLIDTAPERED" => "AdvancedSweptSolid",
        "IFCADVANCEDBREP" | "IFCADVANCEDBREPWITHVOIDS" => "AdvancedBrep",
        "IFCFACETEDBREP" => "Brep",
        "IFCGEOMETRICCURVESET" => "GeometricCurveSet",
        "IFCGEOMETRICSET" => "GeometricSet",
        "IFCOFFSETCURVE2D" => "Curve2D",
        "IFCOFFSETCURVE3D"
        | "IFCOFFSETCURVEBYDISTANCES"
        | "IFCBSPLINECURVEWITHKNOTS"
        | "IFCRATIONALBSPLINECURVEWITHKNOTS" => "Curve3D",
        "IFCBSPLINESURFACEWITHKNOTS"
        | "IFCRATIONALBSPLINESURFACEWITHKNOTS"
        | "IFCSURFACEOFLINEAREXTRUSION"
        | "IFCSURFACEOFREVOLUTION" => "Surface3D",
        _ => "SweptSolid",
    }
}

pub fn fixture(lines: &str) -> String {
    fixture_with(lines, "#10", representation_of(lines), "")
}

pub fn evaluate_with(text: &str, openings: bool) -> Geometry {
    let output = read(text.as_bytes(), &mut ReadOptions::default()).expect("read");
    output
        .file
        .geometry_with_options(
            &Limits::DEFAULT,
            &Cancel::new(),
            &arc_base::Tolerance::DEFAULT,
            &GeometryOptions {
                subtract_openings: openings,
            },
        )
        .expect("geometry")
}

pub fn evaluate(text: &str) -> Geometry {
    evaluate_with(text, true)
}

/// Dumps the file for the oracle and evaluates it.
pub fn dumped(prefix: &str, name: &str, text: &str) -> Geometry {
    support::dump_nonconformant(&format!("{prefix}-{name}.ifc"), text.as_bytes());
    evaluate(text)
}

pub fn meshes(geometry: &Geometry, product: u64) -> Vec<TriMesh> {
    geometry
        .items
        .iter()
        .filter(|i| i.product == product)
        .filter_map(|i| i.world_mesh(&Limits::DEFAULT).unwrap())
        .collect()
}

/// Volume and area of the closed meshes of a product.
pub fn measure_of(geometry: &Geometry, product: u64) -> (f64, f64) {
    let (mut v, mut a) = (0.0, 0.0);
    for mesh in meshes(geometry, product) {
        v += volume(&mesh, &Limits::DEFAULT).expect("closed mesh");
        a += surface_area(&mesh, &Limits::DEFAULT).unwrap();
    }
    (v, a)
}

pub fn measure(geometry: &Geometry) -> (f64, f64) {
    measure_of(geometry, 902)
}

pub fn polylines(geometry: &Geometry, product: u64) -> Vec<Vec<arc_base::math::Point3>> {
    geometry
        .items
        .iter()
        .filter(|i| i.product == product)
        .filter_map(|i| match i.shape.as_ref() {
            Shape::Polylines(lines) => Some(lines.clone()),
            Shape::Mesh(_) => None,
        })
        .flatten()
        .collect()
}

pub fn loss_count(geometry: &Geometry, reason: &str, kind: &str) -> i64 {
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

pub fn reason_count(geometry: &Geometry, reason: &str) -> usize {
    geometry
        .report
        .items
        .iter()
        .filter(|item| item.reason == reason)
        .count()
}

pub fn close(actual: f64, expected: f64, relative: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= relative * expected.abs().max(1e-12),
        "{what}: {actual} vs {expected}"
    );
}

/// Every undirected edge of the mesh is used by exactly two triangles, once in each direction.
pub fn assert_watertight(mesh: &TriMesh, what: &str) {
    let mut edges: std::collections::HashMap<(u32, u32), (u32, u32)> =
        std::collections::HashMap::new();
    for t in mesh.triangles() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let key = if a < b { (a, b) } else { (b, a) };
            let entry = edges.entry(key).or_default();
            if a < b {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
    }
    assert!(
        edges.values().all(|&(f, b)| f == 1 && b == 1),
        "{what}: not watertight"
    );
}

/// Evaluates `lines` (the item is #10) and compares the volume and area, then dumps the file.
pub fn check(
    prefix: &str,
    name: &str,
    lines: &str,
    volume: f64,
    area: f64,
    relative: f64,
) -> Geometry {
    let text = fixture(lines);
    support::dump_nonconformant(&format!("{prefix}-{name}.ifc"), text.as_bytes());
    let geometry = evaluate(&text);
    assert!(!geometry.items.is_empty(), "{name}: nothing evaluated");
    let (v, a) = measure(&geometry);
    close(v, volume, relative, &format!("{name} volume"));
    if area > 0.0 {
        close(a, area, relative, &format!("{name} area"));
    }
    geometry
}

/// A builder of STEP lines with fresh instance names from #1000 on.
pub struct Step {
    pub lines: String,
    next: u64,
}

impl Step {
    pub fn new() -> Self {
        Self {
            lines: String::new(),
            next: 1000,
        }
    }

    /// Adds `text` (an entity without its name) and returns its name.
    pub fn add(&mut self, text: &str) -> u64 {
        let n = self.next;
        self.next += 1;
        self.lines.push_str(&format!("#{n}={text};\n"));
        n
    }

    pub fn point(&mut self, x: f64, y: f64, z: f64) -> u64 {
        self.add(&format!("IFCCARTESIANPOINT(({x:?},{y:?},{z:?}))"))
    }

    pub fn direction(&mut self, x: f64, y: f64, z: f64) -> u64 {
        self.add(&format!("IFCDIRECTION(({x:?},{y:?},{z:?}))"))
    }

    /// An `IfcAxis2Placement3D` at `at` with the axis and reference direction.
    pub fn placement(&mut self, at: [f64; 3], axis: [f64; 3], reference: [f64; 3]) -> u64 {
        let p = self.point(at[0], at[1], at[2]);
        let a = self.direction(axis[0], axis[1], axis[2]);
        let r = self.direction(reference[0], reference[1], reference[2]);
        self.add(&format!("IFCAXIS2PLACEMENT3D(#{p},#{a},#{r})"))
    }

    pub fn vertex(&mut self, x: f64, y: f64, z: f64) -> u64 {
        let p = self.point(x, y, z);
        self.add(&format!("IFCVERTEXPOINT(#{p})"))
    }

    pub fn edge(&mut self, a: u64, b: u64, curve: u64, same: bool) -> u64 {
        self.add(&format!(
            "IFCEDGECURVE(#{a},#{b},#{curve},{})",
            if same { ".T." } else { ".F." }
        ))
    }

    pub fn oriented(&mut self, edge: u64, forward: bool) -> u64 {
        self.add(&format!(
            "IFCORIENTEDEDGE(*,*,#{edge},{})",
            if forward { ".T." } else { ".F." }
        ))
    }

    /// A face of `loops` (the first is the outer bound), each a list of (edge, forward) pairs, on
    /// the surface.
    pub fn face(&mut self, loops: &[&[(u64, bool)]], surface: u64, same: bool) -> u64 {
        let mut bounds = Vec::new();
        for (k, l) in loops.iter().enumerate() {
            let oriented: Vec<String> = l
                .iter()
                .map(|&(e, f)| format!("#{}", self.oriented(e, f)))
                .collect();
            let edge_loop = self.add(&format!("IFCEDGELOOP(({}))", oriented.join(",")));
            let kind = if k == 0 {
                "IFCFACEOUTERBOUND"
            } else {
                "IFCFACEBOUND"
            };
            bounds.push(format!(
                "#{}",
                self.add(&format!("{kind}(#{edge_loop},.T.)"))
            ));
        }
        self.add(&format!(
            "IFCADVANCEDFACE(({}),#{surface},{})",
            bounds.join(","),
            if same { ".T." } else { ".F." }
        ))
    }

    /// An `IfcAdvancedBrep` of the faces; returns its name.
    pub fn brep(&mut self, faces: &[u64]) -> u64 {
        let list: Vec<String> = faces.iter().map(|f| format!("#{f}")).collect();
        let shell = self.add(&format!("IFCCLOSEDSHELL(({}))", list.join(",")));
        self.add(&format!("IFCADVANCEDBREP(#{shell})"))
    }

    pub fn circle(&mut self, placement: u64, radius: f64) -> u64 {
        self.add(&format!("IFCCIRCLE(#{placement},{radius:?})"))
    }

    pub fn line(&mut self, from: [f64; 3], to: [f64; 3]) -> u64 {
        let p = self.point(from[0], from[1], from[2]);
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let dir = self.direction(d[0] / length, d[1] / length, d[2] / length);
        let vector = self.add(&format!("IFCVECTOR(#{dir},{length:?})"));
        self.add(&format!("IFCLINE(#{p},#{vector})"))
    }

    pub fn plane(&mut self, at: [f64; 3], axis: [f64; 3], reference: [f64; 3]) -> u64 {
        let placement = self.placement(at, axis, reference);
        self.add(&format!("IFCPLANE(#{placement})"))
    }
}
