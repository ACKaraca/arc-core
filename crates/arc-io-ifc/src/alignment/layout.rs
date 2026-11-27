//! Curves from the layouts of an `IfcAlignment`: the horizontal, vertical and cant segments with
//! their design parameters, converted to the same segments the geometric representation uses.

use arc_base::Result;

use super::curve::{Cant, CantKind, CantPiece, Curve, Gradient, Plan, Seg};
use super::parent::{Parent, Shape, Spiral};
use super::read::{Reader, invalid, unsupported};
use crate::model::Record;

/// The shape named by a segment type, `None` for a type that has none.
fn transition(name: &str) -> Option<Shape> {
    Some(match name {
        "CLOTHOID" | "LINEARTRANSITION" => Shape::Linear,
        "HELMERTCURVE" => Shape::Helmert,
        "BLOSSCURVE" => Shape::Bloss,
        "COSINECURVE" => Shape::Cosine,
        "SINECURVE" => Shape::Sine,
        "VIENNESEBEND" => Shape::Viennese,
        "CONSTANTCANT" => Shape::Constant,
        _ => return None,
    })
}

/// The cant of the layout at the distance along `x`, as the cross slope of the rails and the
/// centerline raise, for the Viennese bend.
struct CantSeg {
    x0: f64,
    x1: f64,
    shape: Shape,
    left: [f64; 2],
    right: [f64; 2],
}

impl CantSeg {
    fn slope(&self, end: usize) -> f64 {
        self.right[end] - self.left[end]
    }
}

impl<'f> Reader<'f> {
    /// The layouts nested in an alignment, or in its parent for the layouts a child alignment
    /// reuses: `(horizontal, vertical, cant)`.
    fn layouts(&mut self, alignment: u64) -> (Option<u64>, Option<u64>, Option<u64>) {
        let mut found = (None, None, None);
        let mut chain = vec![alignment];
        for parent in self.parents(alignment) {
            chain.push(parent);
        }
        for owner in chain {
            for child in self.related("IFCRELNESTS", owner) {
                let kind = self
                    .file
                    .instance(child)
                    .and_then(|i| i.records.first())
                    .map(|r| r.type_name.as_str());
                let slot = match kind {
                    Some("IFCALIGNMENTHORIZONTAL") => &mut found.0,
                    Some("IFCALIGNMENTVERTICAL") => &mut found.1,
                    Some("IFCALIGNMENTCANT") => &mut found.2,
                    _ => continue,
                };
                if slot.is_none() {
                    *slot = Some(child);
                }
            }
        }
        found
    }

    /// The alignments that aggregate `alignment` as a child.
    fn parents(&mut self, alignment: u64) -> Vec<u64> {
        let mut out = Vec::new();
        for instance in self.file.instances_of("IFCRELAGGREGATES") {
            let Some(record) = instance.records.first() else {
                continue;
            };
            if let (
                Some(crate::model::Parameter::Reference(parent)),
                Some(crate::model::Parameter::List(items)),
            ) = (
                self.file.attribute(record, "RelatingObject"),
                self.file.attribute(record, "RelatedObjects"),
            ) && items
                .iter()
                .any(|i| matches!(i, crate::model::Parameter::Reference(n) if *n == alignment))
                && self
                    .file
                    .instance(*parent)
                    .and_then(|i| i.records.first())
                    .is_some_and(|r| r.type_name == "IFCALIGNMENT")
            {
                out.push(*parent);
            }
        }
        out
    }

    /// The design parameter records of the segments nested in a layout.
    fn design_parameters(&mut self, layout: u64) -> Result<Vec<&'f Record>> {
        let mut out = Vec::new();
        for segment in self.related("IFCRELNESTS", layout) {
            let record = self.record(segment, &["IFCALIGNMENTSEGMENT"])?;
            let parameters = self.reference(record, "DesignParameters")?;
            out.push(self.record(
                parameters,
                &[
                    "IFCALIGNMENTHORIZONTALSEGMENT",
                    "IFCALIGNMENTVERTICALSEGMENT",
                    "IFCALIGNMENTCANTSEGMENT",
                ],
            )?);
        }
        Ok(out)
    }

    fn predefined(&self, record: &'f Record) -> Result<&'f str> {
        match self.opt(record, "PredefinedType") {
            Some(crate::model::Parameter::Enumeration(name)) => Ok(name),
            _ => Err(invalid()),
        }
    }

    /// The curve defined by the layouts of an alignment: plan from the horizontal layout, heights
    /// from the vertical layout, cant from the cant layout.
    ///
    /// # Errors
    ///
    /// `io.ifc.curve_invalid` without a horizontal layout or with a segment that lacks a value;
    /// `io.ifc.curve_unsupported` for a vertical clothoid.
    pub(crate) fn layout_curve(&mut self, alignment: u64) -> Result<Curve> {
        let (horizontal, vertical, cant) = self.layouts(alignment);
        let horizontal = horizontal.ok_or_else(invalid)?;
        let cants = match cant {
            Some(layout) => {
                let record = self.record(layout, &["IFCALIGNMENTCANT"])?;
                let rail_head = self.opt_real(record, "RailHeadDistance")?.unwrap_or(0.0);
                Some((self.cant_segments(layout)?, rail_head))
            }
            None => None,
        };
        let plan = self.horizontal(horizontal, cants.as_ref().map(|c| (&c.0[..], c.1)))?;
        let gradient = match vertical {
            Some(layout) => Some(self.vertical(layout)?),
            None => None,
        };
        let cant = cants.map(|(segs, rail_head)| Cant {
            pieces: segs
                .iter()
                .map(|s| CantPiece {
                    x0: s.x0,
                    x1: s.x1,
                    kind: CantKind::Layout {
                        shape: s.shape,
                        raise: [
                            0.5 * (s.left[0] + s.right[0]),
                            0.5 * (s.left[1] + s.right[1]),
                        ],
                        slope: [s.slope(0), s.slope(1)],
                        rail_head,
                    },
                })
                .collect(),
        });
        Ok(Curve {
            plan,
            gradient,
            cant,
            scale: self.scale,
        })
    }

    fn cant_segments(&mut self, layout: u64) -> Result<Vec<CantSeg>> {
        let mut out = Vec::new();
        for record in self.design_parameters(layout)? {
            let x0 = self.real(record, "StartDistAlong")?;
            let length = self.real(record, "HorizontalLength")?;
            if length <= 0.0 {
                continue;
            }
            let name = self.predefined(record)?;
            let shape = transition(name)
                .ok_or_else(|| unsupported(name))
                .or_else(|e| {
                    if name == "NOTDEFINED" {
                        Ok(Shape::Linear)
                    } else {
                        Err(e)
                    }
                })?;
            let l0 = self.real(record, "StartCantLeft")?;
            let r0 = self.real(record, "StartCantRight")?;
            let l1 = self.opt_real(record, "EndCantLeft")?.unwrap_or(l0);
            let r1 = self.opt_real(record, "EndCantRight")?.unwrap_or(r0);
            out.push(CantSeg {
                x0,
                x1: x0 + length,
                shape,
                left: [l0, l1],
                right: [r0, r1],
            });
        }
        Ok(out)
    }

    /// The plan of a horizontal layout. `cants` gives the cant layout for the Viennese bend.
    fn horizontal(&mut self, layout: u64, cants: Option<(&[CantSeg], f64)>) -> Result<Plan> {
        let mut segs = Vec::new();
        let mut start = 0.0;
        for record in self.design_parameters(layout)? {
            self.cancel.check()?;
            let length = self.real(record, "SegmentLength")?;
            if length < 0.0 {
                return Err(invalid());
            }
            let point = self.coordinates(self.reference(record, "StartPoint")?)?;
            let loc = [
                *point.first().ok_or_else(invalid)?,
                *point.get(1).ok_or_else(invalid)?,
                0.0,
            ];
            let phi = self.real(record, "StartDirection")? * self.angle;
            let name = self.predefined(record)?;
            let (parent, len) = self.horizontal_parent(record, name, length, start, cants)?;
            start += len.abs();
            segs.push(Seg {
                kind: name.to_owned(),
                parent,
                s0: 0.0,
                len,
                loc,
                phi,
                axis: [0.0, 0.0, 1.0],
            });
        }
        if segs.is_empty() {
            return Err(invalid());
        }
        Ok(Plan::new(segs))
    }

    /// The parent curve of a horizontal segment of the given type and its signed length. `start`
    /// is the distance along at the start of the segment, for the cant of a Viennese bend.
    fn horizontal_parent(
        &self,
        record: &'f Record,
        name: &str,
        length: f64,
        start: f64,
        cants: Option<(&[CantSeg], f64)>,
    ) -> Result<(Parent, f64)> {
        let curvature = |attr: &str| -> Result<f64> {
            Ok(self
                .opt_real(record, attr)?
                .filter(|r| *r != 0.0)
                .map_or(0.0, f64::recip))
        };
        let (parent, len) = match name {
            _ if length == 0.0 => (Parent::Line, 0.0),
            "LINE" => (Parent::Line, length),
            "CIRCULARARC" => {
                let k = curvature("StartRadiusOfCurvature")?;
                if k == 0.0 {
                    return Err(invalid());
                }
                (Parent::Circle(k.abs().recip()), k.signum() * length)
            }
            "CUBIC" => {
                let k1 = curvature("StartRadiusOfCurvature")?;
                let k2 = curvature("EndRadiusOfCurvature")?;
                // y = x^3 / (6 R L) from the straight side; the curvature that is not zero
                // gives R.
                let k = if k2 == 0.0 { k1 } else { k2 };
                if length == 0.0 || k == 0.0 {
                    return Err(invalid());
                }
                (
                    Parent::Polynomial {
                        x: vec![0.0, 1.0],
                        y: vec![0.0, 0.0, 0.0, k / (6.0 * length)],
                        by_length: true,
                    },
                    length,
                )
            }
            "VIENNESEBEND" | "CLOTHOID" | "HELMERTCURVE" | "BLOSSCURVE" | "COSINECURVE"
            | "SINECURVE" => {
                let shape = transition(name).ok_or_else(invalid)?;
                let k1 = curvature("StartRadiusOfCurvature")?;
                let k2 = curvature("EndRadiusOfCurvature")?;
                let mut vc = 0.0;
                if shape == Shape::Viennese
                    && length > 0.0
                    && let Some((segs, rail_head)) = cants
                    && rail_head > 0.0
                {
                    let h = self
                        .opt_real(record, "GravityCenterLineHeight")?
                        .unwrap_or(0.0);
                    let psi = |x: f64| -> f64 {
                        let seg = segs.iter().find(|s| x >= s.x0 - 1e-9 && x <= s.x1 + 1e-9);
                        seg.map_or(0.0, |s| {
                            let f = if s.x1 > s.x0 {
                                s.shape.value(((x - s.x0) / (s.x1 - s.x0)).clamp(0.0, 1.0))
                            } else {
                                1.0
                            };
                            let d = s.slope(0) + f * (s.slope(1) - s.slope(0));
                            d / rail_head
                        })
                    };
                    vc = 420.0 * h * (psi(start + length) - psi(start)) / (length * length);
                }
                (
                    Parent::Spiral(Spiral::Blend {
                        k1,
                        dk: k2 - k1,
                        len: if length == 0.0 { 1.0 } else { length },
                        shape,
                        vc,
                    }),
                    length,
                )
            }
            other => return Err(unsupported(other)),
        };
        Ok((parent, len))
    }

    /// The gradient of a vertical layout.
    fn vertical(&mut self, layout: u64) -> Result<Gradient> {
        let mut pieces = Vec::new();
        for record in self.design_parameters(layout)? {
            self.cancel.check()?;
            let length = self.real(record, "HorizontalLength")?;
            if length <= 0.0 {
                continue;
            }
            let x0 = self.real(record, "StartDistAlong")?;
            let z0 = self.real(record, "StartHeight")?;
            let g0 = self.real(record, "StartGradient")?;
            let g1 = self.opt_real(record, "EndGradient")?.unwrap_or(g0);
            let a0 = g0.atan();
            let name = self.predefined(record)?;
            let (parent, len) = match name {
                "CONSTANTGRADIENT" => (Parent::Line, length * g0.hypot(1.0)),
                "PARABOLICARC" => (
                    Parent::Polynomial {
                        x: vec![0.0, 1.0],
                        y: vec![0.0, g0, (g1 - g0) / (2.0 * length)],
                        by_length: false,
                    },
                    length,
                ),
                "CIRCULARARC" => {
                    let a1 = g1.atan();
                    // The horizontal length and the two gradients fix the circle: the
                    // horizontal extent of an arc from heading a0 to a1 is R (sin a1 - sin a0).
                    // The radius attribute is redundant and agrees with them up to rounding.
                    let rise = a1.sin() - a0.sin();
                    if rise.abs() < 1e-12 {
                        return Err(invalid());
                    }
                    let curvature = rise / length;
                    let arc = (a1 - a0).abs() / curvature.abs();
                    (
                        Parent::Circle(curvature.abs().recip()),
                        curvature.signum() * arc,
                    )
                }
                other => return Err(unsupported(other)),
            };
            pieces.push((
                x0,
                x0 + length,
                Seg {
                    kind: name.to_owned(),
                    parent,
                    s0: 0.0,
                    len,
                    loc: [x0, z0, 0.0],
                    phi: a0,
                    axis: [0.0, 0.0, 1.0],
                },
            ));
        }
        if pieces.is_empty() {
            return Err(invalid());
        }
        Ok(Gradient { pieces })
    }
}
