//! The values of properties: the IFC simple types and measures the published property set
//! templates use, checked against the rules of their defined types.

use arc_base::Result;

use super::invalid;
use crate::model::Parameter;

/// The longest `IfcLabel` or `IfcIdentifier`, in characters (`STRING(255)`).
const MAX_LABEL_CHARS: usize = 255;

/// A measure type of a property value, with the rule IFC4 gives its defined type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// `IfcLengthMeasure`, in the length unit.
    Length,
    /// `IfcPositiveLengthMeasure`, in the length unit; greater than zero.
    PositiveLength,
    /// `IfcNonNegativeLengthMeasure`, in the length unit; zero or greater.
    NonNegativeLength,
    /// `IfcAreaMeasure`, in the length unit squared.
    Area,
    /// `IfcVolumeMeasure`, in the length unit cubed.
    Volume,
    /// `IfcPlaneAngleMeasure`, given in radians and written in the plane angle unit of the
    /// project.
    PlaneAngle,
    /// `IfcPositivePlaneAngleMeasure`, like [`Measure::PlaneAngle`]; greater than zero.
    PositivePlaneAngle,
    /// `IfcRatioMeasure`.
    Ratio,
    /// `IfcPositiveRatioMeasure`; greater than zero.
    PositiveRatio,
    /// `IfcNormalisedRatioMeasure`; from zero to one.
    NormalisedRatio,
    /// `IfcCountMeasure`.
    Count,
    /// `IfcThermalTransmittanceMeasure`, in W/(m²·K).
    ThermalTransmittance,
    /// `IfcThermodynamicTemperatureMeasure`, in kelvin.
    ThermodynamicTemperature,
    /// `IfcPowerMeasure`, in watts.
    Power,
    /// `IfcPressureMeasure`, in pascals.
    Pressure,
    /// `IfcVolumetricFlowRateMeasure`, in m³/s.
    VolumetricFlowRate,
    /// `IfcMassMeasure`, in kilograms.
    Mass,
    /// `IfcMassDensityMeasure`, in kg/m³.
    MassDensity,
    /// `IfcAreaDensityMeasure`, in kg/m².
    AreaDensity,
    /// `IfcTimeMeasure`, in seconds.
    Time,
    /// `IfcIlluminanceMeasure`, in lux.
    Illuminance,
    /// `IfcLinearVelocityMeasure`, in m/s.
    LinearVelocity,
    /// `IfcHeatFluxDensityMeasure`, in W/m².
    HeatFluxDensity,
    /// `IfcPlanarForceMeasure`, in N/m².
    PlanarForce,
    /// `IfcMonetaryMeasure`, in the currency of the project.
    Monetary,
}

impl Measure {
    /// The IFC type name, in upper case.
    #[must_use]
    pub fn type_name(self) -> &'static str {
        match self {
            Self::Length => "IFCLENGTHMEASURE",
            Self::PositiveLength => "IFCPOSITIVELENGTHMEASURE",
            Self::NonNegativeLength => "IFCNONNEGATIVELENGTHMEASURE",
            Self::Area => "IFCAREAMEASURE",
            Self::Volume => "IFCVOLUMEMEASURE",
            Self::PlaneAngle => "IFCPLANEANGLEMEASURE",
            Self::PositivePlaneAngle => "IFCPOSITIVEPLANEANGLEMEASURE",
            Self::Ratio => "IFCRATIOMEASURE",
            Self::PositiveRatio => "IFCPOSITIVERATIOMEASURE",
            Self::NormalisedRatio => "IFCNORMALISEDRATIOMEASURE",
            Self::Count => "IFCCOUNTMEASURE",
            Self::ThermalTransmittance => "IFCTHERMALTRANSMITTANCEMEASURE",
            Self::ThermodynamicTemperature => "IFCTHERMODYNAMICTEMPERATUREMEASURE",
            Self::Power => "IFCPOWERMEASURE",
            Self::Pressure => "IFCPRESSUREMEASURE",
            Self::VolumetricFlowRate => "IFCVOLUMETRICFLOWRATEMEASURE",
            Self::Mass => "IFCMASSMEASURE",
            Self::MassDensity => "IFCMASSDENSITYMEASURE",
            Self::AreaDensity => "IFCAREADENSITYMEASURE",
            Self::Time => "IFCTIMEMEASURE",
            Self::Illuminance => "IFCILLUMINANCEMEASURE",
            Self::LinearVelocity => "IFCLINEARVELOCITYMEASURE",
            Self::HeatFluxDensity => "IFCHEATFLUXDENSITYMEASURE",
            Self::PlanarForce => "IFCPLANARFORCEMEASURE",
            Self::Monetary => "IFCMONETARYMEASURE",
        }
    }

    /// Whether `value` is finite and satisfies the rule of the defined type.
    #[must_use]
    pub fn accepts(self, value: f64) -> bool {
        value.is_finite()
            && match self {
                Self::PositiveLength | Self::PositivePlaneAngle | Self::PositiveRatio => {
                    value > 0.0
                }
                Self::NonNegativeLength => value >= 0.0,
                Self::NormalisedRatio => (0.0..=1.0).contains(&value),
                _ => true,
            }
    }

    fn is_angle(self) -> bool {
        matches!(self, Self::PlaneAngle | Self::PositivePlaneAngle)
    }
}

/// The value of a property. A single value is written as `IfcPropertySingleValue`, an
/// enumerated value as `IfcPropertyEnumeratedValue` with its `IfcPropertyEnumeration`. The
/// variants can be built directly; the constructors refuse what the writer would refuse.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// `IfcLabel`, at most 255 characters.
    Label(String),
    /// `IfcLengthMeasure`, in the length unit.
    Length(f64),
    /// `IfcAreaMeasure`, in the length unit squared.
    Area(f64),
    /// `IfcVolumeMeasure`, in the length unit cubed.
    Volume(f64),
    /// `IfcBoolean`.
    Boolean(bool),
    /// `IfcIdentifier`, at most 255 characters.
    Identifier(String),
    /// `IfcText`.
    Text(String),
    /// `IfcInteger`.
    Integer(i64),
    /// `IfcReal`, finite.
    Real(f64),
    /// `IfcLogical`: `Some(true)`, `Some(false)` or unknown (`None`).
    Logical(Option<bool>),
    /// A measure with its type; see [`Measure`].
    Measure(Measure, f64),
    /// An enumerated value: the chosen items of an enumeration, written as `IfcLabel` values with
    /// a reference to the `IfcPropertyEnumeration` named `enumeration`, which the file holds
    /// once per name.
    Enumerated {
        /// The enumeration name, such as `PEnum_ElementStatus`.
        enumeration: String,
        /// The items of the enumeration, without repeats.
        items: Vec<String>,
        /// The chosen items; at least one, each one of `items`.
        values: Vec<String>,
    },
}

impl PropertyValue {
    /// A measure value.
    ///
    /// # Errors
    ///
    /// `io.ifc.write_value_invalid` for a value that is not finite or breaks the rule of the
    /// measure (a positive measure that is zero or negative, a normalized ratio outside 0 to 1).
    pub fn measure(measure: Measure, value: f64) -> Result<Self> {
        let value = Self::Measure(measure, value);
        value.check()?;
        Ok(value)
    }

    /// An `IfcReal`.
    ///
    /// # Errors
    ///
    /// `io.ifc.write_value_invalid` for a value that is not finite.
    pub fn real(value: f64) -> Result<Self> {
        let value = Self::Real(value);
        value.check()?;
        Ok(value)
    }

    /// An `IfcLabel`.
    ///
    /// # Errors
    ///
    /// `io.ifc.write_value_invalid` for more than 255 characters or a NUL.
    pub fn label(text: &str) -> Result<Self> {
        let value = Self::Label(text.to_owned());
        value.check()?;
        Ok(value)
    }

    /// An `IfcIdentifier`.
    ///
    /// # Errors
    ///
    /// `io.ifc.write_value_invalid` for more than 255 characters or a NUL.
    pub fn identifier(text: &str) -> Result<Self> {
        let value = Self::Identifier(text.to_owned());
        value.check()?;
        Ok(value)
    }

    /// An enumerated value of `enumeration` with its `items`, choosing `values`.
    ///
    /// # Errors
    ///
    /// `io.ifc.write_value_invalid` for an empty or repeated item list, no chosen value, a value
    /// that is not an item, or a text the label rules refuse.
    pub fn enumerated(enumeration: &str, items: &[&str], values: &[&str]) -> Result<Self> {
        let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
        let value = Self::Enumerated {
            enumeration: enumeration.to_owned(),
            items: owned(items),
            values: owned(values),
        };
        value.check()?;
        Ok(value)
    }

    /// Checks the value against the rules of its type.
    ///
    /// # Errors
    ///
    /// `io.ifc.write_value_invalid` as the constructors describe; a length, area, volume or real
    /// that is not finite.
    pub fn check(&self) -> Result<()> {
        let ok = match self {
            Self::Label(t) | Self::Identifier(t) => label_ok(t),
            Self::Text(t) => !t.contains('\0'),
            Self::Length(v) | Self::Area(v) | Self::Volume(v) | Self::Real(v) => v.is_finite(),
            Self::Boolean(_) | Self::Integer(_) | Self::Logical(_) => true,
            Self::Measure(measure, v) => measure.accepts(*v),
            Self::Enumerated {
                enumeration,
                items,
                values,
            } => {
                let mut seen = std::collections::BTreeSet::new();
                label_ok(enumeration)
                    && !items.is_empty()
                    && items.iter().all(|i| label_ok(i) && seen.insert(i.as_str()))
                    && !values.is_empty()
                    && values.iter().all(|v| items.contains(v))
            }
        };
        if ok {
            Ok(())
        } else {
            Err(invalid("property value"))
        }
    }

    /// The nominal value of a single value; `None` for an enumerated value. Plane angles are
    /// scaled from radians by `angle_scale`.
    pub(super) fn nominal(&self, angle_scale: f64) -> Option<Parameter> {
        let typed = |name: &str, value| super::typed(name, value);
        Some(match self {
            Self::Label(t) => typed("IFCLABEL", Parameter::Text(t.clone())),
            Self::Identifier(t) => typed("IFCIDENTIFIER", Parameter::Text(t.clone())),
            Self::Text(t) => typed("IFCTEXT", Parameter::Text(t.clone())),
            Self::Length(v) => typed("IFCLENGTHMEASURE", Parameter::Real(*v)),
            Self::Area(v) => typed("IFCAREAMEASURE", Parameter::Real(*v)),
            Self::Volume(v) => typed("IFCVOLUMEMEASURE", Parameter::Real(*v)),
            Self::Real(v) => typed("IFCREAL", Parameter::Real(*v)),
            Self::Integer(v) => typed("IFCINTEGER", Parameter::Integer(*v)),
            Self::Boolean(v) => typed("IFCBOOLEAN", super::boolean(*v)),
            Self::Logical(v) => typed(
                "IFCLOGICAL",
                super::enumeration(match v {
                    Some(true) => "T",
                    Some(false) => "F",
                    None => "U",
                }),
            ),
            Self::Measure(measure, v) => {
                let v = if measure.is_angle() {
                    v * angle_scale
                } else {
                    *v
                };
                typed(measure.type_name(), Parameter::Real(v))
            }
            Self::Enumerated { .. } => return None,
        })
    }
}

fn label_ok(text: &str) -> bool {
    !text.contains('\0') && text.chars().count() <= MAX_LABEL_CHARS
}

/// The value of a quantity; it must be finite and not negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QuantityValue {
    /// `IfcQuantityLength`, in the length unit.
    Length(f64),
    /// `IfcQuantityArea`, in the length unit squared.
    Area(f64),
    /// `IfcQuantityVolume`, in the length unit cubed.
    Volume(f64),
    /// `IfcQuantityCount` (`IfcCountMeasure`).
    Count(f64),
    /// `IfcQuantityWeight` (`IfcMassMeasure`), in kilograms.
    Weight(f64),
    /// `IfcQuantityTime` (`IfcTimeMeasure`), in seconds.
    Time(f64),
}

impl QuantityValue {
    /// The entity and the value.
    pub(super) fn parts(self) -> (&'static str, f64) {
        match self {
            Self::Length(v) => ("IFCQUANTITYLENGTH", v),
            Self::Area(v) => ("IFCQUANTITYAREA", v),
            Self::Volume(v) => ("IFCQUANTITYVOLUME", v),
            Self::Count(v) => ("IFCQUANTITYCOUNT", v),
            Self::Weight(v) => ("IFCQUANTITYWEIGHT", v),
            Self::Time(v) => ("IFCQUANTITYTIME", v),
        }
    }

    /// Checks that the value is finite and not negative.
    ///
    /// # Errors
    ///
    /// `io.ifc.write_value_invalid` otherwise.
    pub fn check(self) -> Result<()> {
        let (_, v) = self.parts();
        if v.is_finite() && v >= 0.0 {
            Ok(())
        } else {
            Err(invalid("quantity"))
        }
    }
}
