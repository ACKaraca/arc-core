//! IFC4X3 alignments and linear placement (CORE-IO-015, alignment part): the curves of an
//! `IfcAlignment`, evaluated to a position, a tangent and a frame at a distance along.
//!
//! An alignment has two descriptions that this module reads and evaluates separately, so that one
//! can check the other:
//! - the *representation*: an `IfcCompositeCurve` (plan), `IfcGradientCurve` (plan and heights) or
//!   `IfcSegmentedReferenceCurve` (plan, heights and cant) of `IfcCurveSegment`s whose parent
//!   curves are `IfcLine`, `IfcCircle`, `IfcClothoid`, `IfcPolynomialCurve`,
//!   `IfcSecondOrderPolynomialSpiral`, `IfcThirdOrderPolynomialSpiral`,
//!   `IfcSeventhOrderPolynomialSpiral`, `IfcCosineSpiral` and `IfcSineSpiral`;
//! - the *layout*: `IfcAlignmentHorizontal`, `IfcAlignmentVertical` and `IfcAlignmentCant` with
//!   their `IfcAlignmentSegment` design parameters (`LINE`, `CIRCULARARC`, `CLOTHOID`, `CUBIC`,
//!   `HELMERTCURVE`, `BLOSSCURVE`, `COSINECURVE`, `SINECURVE`, `VIENNESEBEND`; vertical
//!   `CONSTANTGRADIENT`, `CIRCULARARC`, `PARABOLICARC`; cant with every transition shape).
//!
//! [`IfcFile::alignments`] returns both curves of every alignment. A [`Curve`] evaluates a
//! distance along it with [`Curve::evaluate`]; the distance runs along the plan (for a gradient
//! curve and a segmented reference curve it is the distance along the base curve, as the
//! `IfcGradientCurve` parameterization defines). [`LinearResolver`] resolves
//! `IfcLinearPlacement`, `IfcAxis2PlacementLinear` and `IfcPointByDistanceExpression` to world
//! transforms.
//!
//! Conventions and limits:
//! - Lengths are converted to meters; the tolerance of an evaluation is the slack, in meters, by
//!   which a distance may lie beyond the ends.
//! - The `Position` of a parent curve is not used: the `Placement` of the curve segment fixes the
//!   start point and the start tangent, which is all a segment needs.
//! - For polynomial curves the parameter `t` is the segment parameter; for the layout `CUBIC`
//!   the segment length is the arc length.
//! - The frame at a point has X along the tangent, Y to the left and Z up, tilted toward the left
//!   by the cant angle, which is `asin(cross slope / rail head distance)`.
//! - A segmented reference curve is evaluated for parent curves `IfcLine`, `IfcPolynomialCurve`
//!   and `IfcCosineSpiral`; the deviation follows the description of the entity, and the tilt of
//!   the `Axis` changes between the start and end axis in proportion to the deviation (or to the
//!   distance when the deviation does not change). Other parents are reported unsupported.
//! - A vertical layout `CLOTHOID` is not evaluated from the layout (its start curvature is not
//!   among the attributes); its representation is.

mod curve;
mod layout;
mod num;
mod parent;
mod placement;
mod read;

pub use curve::{Curve, Pose, SegmentKinds};
pub use placement::LinearResolver;

use arc_base::{Cancel, Limits, Result};

use crate::error::{codes, reasons};
use crate::geometry::Units;
use crate::loss::{Direction, LossHeader, LossLog, LossReport, Source};
use crate::model::{IfcFile, Parameter};
use read::Reader;

/// The curves of one `IfcAlignment`.
#[derive(Debug, Clone, PartialEq)]
pub struct AlignmentCurves {
    /// The instance name of the alignment.
    pub alignment: u64,
    /// The `Name` of the alignment.
    pub name: Option<String>,
    /// The instance name of the curve item of the axis representation, when there is one.
    pub representation_item: Option<u64>,
    /// The curve of the axis representation, when the alignment has one that can be evaluated.
    pub representation: Option<Curve>,
    /// The curve of the layouts, when the alignment has horizontal layout that can be evaluated.
    pub layout: Option<Curve>,
}

/// The alignments of a file and what was not evaluated.
#[derive(Debug, Clone, PartialEq)]
pub struct Alignments {
    /// One entry per `IfcAlignment`, in file order.
    pub items: Vec<AlignmentCurves>,
    /// The loss report: curves that were skipped.
    pub report: LossReport,
}

impl IfcFile {
    /// Builds the curve of an `IfcCompositeCurve`, `IfcGradientCurve` or
    /// `IfcSegmentedReferenceCurve`.
    ///
    /// # Errors
    ///
    /// `io.ifc.curve_invalid` or `io.ifc.curve_unsupported`; `io.limit_exceeded` for a segment
    /// list longer than `array_length`; `base.cancelled`.
    pub fn curve(&self, name: u64, limits: &Limits, cancel: &Cancel) -> Result<Curve> {
        let units = Units::of(self, limits, cancel)?;
        Reader::new(
            self,
            limits,
            cancel,
            units.meters_per_length_unit,
            units.radians_per_angle_unit,
        )
        .curve(name)
    }

    /// Builds the curve of the layouts of an `IfcAlignment` (horizontal, vertical, cant).
    ///
    /// # Errors
    ///
    /// As [`Self::curve`].
    pub fn alignment_layout_curve(
        &self,
        alignment: u64,
        limits: &Limits,
        cancel: &Cancel,
    ) -> Result<Curve> {
        let units = Units::of(self, limits, cancel)?;
        Reader::new(
            self,
            limits,
            cancel,
            units.meters_per_length_unit,
            units.radians_per_angle_unit,
        )
        .layout_curve(alignment)
    }

    /// Builds the curves of every `IfcAlignment` of the file. A curve that cannot be built is left
    /// out and reported as `io.ifc.geometry_not_evaluated` (unsupported) or
    /// `io.ifc.geometry_degenerate` (malformed).
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` and `base.cancelled`; malformed curves are not errors.
    pub fn alignments(&self, limits: &Limits, cancel: &Cancel) -> Result<Alignments> {
        let units = Units::of(self, limits, cancel)?;
        let mut reader = Reader::new(
            self,
            limits,
            cancel,
            units.meters_per_length_unit,
            units.radians_per_angle_unit,
        );
        let mut log = LossLog::default();
        let mut items = Vec::new();
        let names: Vec<u64> = self.instances_of("IFCALIGNMENT").map(|i| i.name).collect();
        for alignment in names {
            cancel.check()?;
            let name = self
                .instance(alignment)
                .and_then(|i| i.records.first())
                .and_then(|r| match self.attribute(r, "Name") {
                    Some(Parameter::Text(t)) => Some(t.clone()),
                    _ => None,
                });
            let mut settle = |result: Result<Curve>, kind: &str| -> Result<Option<Curve>> {
                match result {
                    Ok(curve) => Ok(Some(curve)),
                    Err(e) if e.code() == codes::IFC_CURVE_UNSUPPORTED.code => {
                        log.add(
                            reasons::GEOMETRY_NOT_EVALUATED,
                            kind,
                            Some(Source::Decimal(alignment)),
                            1,
                        );
                        Ok(None)
                    }
                    Err(e) if e.code() == codes::IFC_CURVE_INVALID.code => {
                        log.add(
                            reasons::GEOMETRY_DEGENERATE,
                            kind,
                            Some(Source::Decimal(alignment)),
                            1,
                        );
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            };
            let mut representation_item = None;
            let representation = match reader.axis_item(alignment) {
                Ok(Some(item)) => {
                    representation_item = Some(item);
                    settle(reader.curve(item), "IFCALIGNMENT")?
                }
                Ok(None) => None,
                Err(e) => settle(Err(e), "IFCALIGNMENT")?,
            };
            let layout = match reader.layout_curve(alignment) {
                // An alignment without layouts is not a loss.
                Err(e) if e.code() == codes::IFC_CURVE_INVALID.code && representation.is_some() => {
                    None
                }
                other => settle(other, "IFCALIGNMENTHORIZONTAL")?,
            };
            items.push(AlignmentCurves {
                alignment,
                name,
                representation_item,
                representation,
                layout,
            });
        }
        let report = log.finish(LossHeader::new(
            "ifc",
            self.schema.identifier(),
            Direction::Import,
            Some(*limits),
        ));
        Ok(Alignments { items, report })
    }
}
