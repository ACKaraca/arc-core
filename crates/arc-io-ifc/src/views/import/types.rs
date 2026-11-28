//! The neutral model of the import views: objects with their resolved definitions, the units,
//! georeferencing and the coverage of the file. No product mapping lives here; an importer maps
//! these values to its own model.

use std::sync::Arc;

use super::super::{Nominal, Property, PropertyValue, Quantity};

/// What an [`ObjectView`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    /// An `IfcProduct`: an element, a spatial structure element, a port, an annotation.
    Product,
    /// An `IfcTypeObject`.
    Type,
    /// Any other `IfcObjectDefinition` outside groups: the project, a process, a resource, a
    /// control, an actor.
    Other,
}

/// Where a resolved value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Assigned to the occurrence itself.
    Occurrence,
    /// Inherited from the type object, because the occurrence has no value of its own.
    Type,
}

/// A 4x4 transform, row-major, with the translation in meters.
pub type Matrix = [f64; 16];

/// How far a placement could be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementState {
    /// The `IfcLocalPlacement` chain ends at the world coordinate system.
    Resolved,
    /// The chain meets a placement the views do not evaluate (`IfcGridPlacement`,
    /// `IfcLinearPlacement`); the transform is the part up to it, relative to it. The entity name
    /// is given.
    NotResolved(String),
    /// The chain is longer than the nesting limit or repeats a placement; it was cut.
    Cut,
}

/// The placement of an object in world coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementView {
    /// The `IfcObjectPlacement` instance.
    pub instance: u64,
    /// The transform to world coordinates (see [`PlacementState`]), translation in meters.
    pub world: Matrix,
    /// Whether the chain was fully resolved.
    pub state: PlacementState,
}

/// A material with its name and category.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialRef {
    /// The `IfcMaterial` instance.
    pub instance: u64,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Description`; absent in IFC2X3.
    pub description: Option<String>,
    /// The `Category`; absent in IFC2X3.
    pub category: Option<String>,
    /// The surface styles of the material (`IfcMaterialDefinitionRepresentation`).
    pub styles: Vec<StyleView>,
}

/// A layer of a material layer set.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerView {
    /// The `IfcMaterialLayer` instance.
    pub instance: u64,
    /// The material; a layer without one is an air gap.
    pub material: Option<MaterialRef>,
    /// The `LayerThickness`, in meters.
    pub thickness: Option<f64>,
    /// The `IsVentilated` flag; `None` when unknown.
    pub ventilated: Option<bool>,
    /// The `Name`; absent in IFC2X3.
    pub name: Option<String>,
    /// The `Category`; absent in IFC2X3.
    pub category: Option<String>,
    /// The `Priority`; absent in IFC2X3.
    pub priority: Option<i64>,
}

/// How a layer set is placed relative to the reference line of the element.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerUsage {
    /// The `IfcMaterialLayerSetUsage` instance.
    pub instance: u64,
    /// `LayerSetDirection`: `AXIS1`, `AXIS2` or `AXIS3`.
    pub direction: Option<String>,
    /// `DirectionSense`: `POSITIVE` or `NEGATIVE`.
    pub sense: Option<String>,
    /// `OffsetFromReferenceLine`, in meters.
    pub offset: Option<f64>,
    /// `ReferenceExtent`, in meters; absent in IFC2X3.
    pub reference_extent: Option<f64>,
}

/// A profile of a material profile set.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileView {
    /// The `IfcMaterialProfile` instance.
    pub instance: u64,
    /// The `Name`.
    pub name: Option<String>,
    /// The material.
    pub material: Option<MaterialRef>,
    /// The `IfcProfileDef` instance.
    pub profile: Option<u64>,
    /// The `Priority`.
    pub priority: Option<i64>,
    /// The `Category`.
    pub category: Option<String>,
}

/// A constituent of a material constituent set.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstituentView {
    /// The `IfcMaterialConstituent` instance.
    pub instance: u64,
    /// The `Name`.
    pub name: Option<String>,
    /// The material.
    pub material: Option<MaterialRef>,
    /// The `Fraction`.
    pub fraction: Option<f64>,
    /// The `Category`.
    pub category: Option<String>,
}

/// The material an object is associated with (`IfcRelAssociatesMaterial`).
#[derive(Debug, Clone, PartialEq)]
pub enum MaterialAssociation {
    /// An `IfcMaterial`.
    Single(MaterialRef),
    /// An `IfcMaterialList`.
    List(Vec<MaterialRef>),
    /// An `IfcMaterialLayerSet`, or the one an `IfcMaterialLayerSetUsage` refers to.
    LayerSet {
        /// The `IfcMaterialLayerSet` instance.
        instance: u64,
        /// The `LayerSetName`.
        name: Option<String>,
        /// The layers, in order.
        layers: Vec<LayerView>,
        /// The usage, when the object refers to the set through one.
        usage: Option<LayerUsage>,
    },
    /// An `IfcMaterialProfileSet`, or the one an `IfcMaterialProfileSetUsage` refers to.
    ProfileSet {
        /// The `IfcMaterialProfileSet` instance.
        instance: u64,
        /// The `Name`.
        name: Option<String>,
        /// The profiles, in order.
        profiles: Vec<ProfileView>,
        /// The `CardinalPoint` of the usage.
        cardinal_point: Option<i64>,
        /// The `ReferenceExtent` of the usage, in meters.
        reference_extent: Option<f64>,
    },
    /// An `IfcMaterialConstituentSet`.
    ConstituentSet {
        /// The `IfcMaterialConstituentSet` instance.
        instance: u64,
        /// The `Name`.
        name: Option<String>,
        /// The constituents, in order.
        constituents: Vec<ConstituentView>,
    },
}

/// A surface style of an item or a material.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleView {
    /// The `IfcStyledItem` instance, or the `IfcSurfaceStyle` of a material.
    pub instance: u64,
    /// The `IfcSurfaceStyle` instance.
    pub style: u64,
    /// The `Name` of the style.
    pub name: Option<String>,
    /// The shading or rendering color, red, green and blue in 0 to 1.
    pub color: Option<[f64; 3]>,
    /// The transparency, 0 opaque to 1 transparent; absent in IFC2X3 shading styles.
    pub transparency: Option<f64>,
}

/// A classification system.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationSystem {
    /// The `IfcClassification` instance.
    pub instance: u64,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Source`.
    pub source: Option<String>,
    /// The `Edition`.
    pub edition: Option<String>,
}

/// A classification association (`IfcRelAssociatesClassification`).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationRef {
    /// The `IfcClassificationReference` or `IfcClassification` instance.
    pub instance: u64,
    /// The `Identification` (`ItemReference` in IFC2X3).
    pub identification: Option<String>,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Location`.
    pub location: Option<String>,
    /// The system the reference belongs to.
    pub system: Option<ClassificationSystem>,
    /// Where the association is assigned.
    pub origin: Origin,
}

/// A property of a resolved property set.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProperty {
    /// The property.
    pub property: Arc<Property>,
    /// Where the value comes from; an occurrence value replaces the type's of the same name.
    pub origin: Origin,
    /// The unit that applies: the property's own, or the project's for the measure type; an index
    /// into [`UnitSystem::units`].
    pub unit: Option<usize>,
    /// The factor to multiply a numeric value by to get SI base units.
    pub scale_to_si: Option<f64>,
}

impl ResolvedProperty {
    /// The numeric values of the property in SI base units (nominal, list items, bounds in that
    /// order); empty when the property holds none or no unit factor is known.
    #[must_use]
    pub fn si_numbers(&self) -> Vec<f64> {
        let Some(scale) = self.scale_to_si else {
            return Vec::new();
        };
        let number = |n: &Nominal| match n.value {
            super::super::Value::Real(r) => Some(r * scale),
            #[allow(clippy::cast_precision_loss, reason = "a property value, not an id")]
            super::super::Value::Integer(i) => Some(i as f64 * scale),
            _ => None,
        };
        match &self.property.value {
            PropertyValue::Single { nominal, .. } => nominal.iter().filter_map(number).collect(),
            PropertyValue::Enumerated { values, .. } | PropertyValue::List { values, .. } => {
                values.iter().filter_map(number).collect()
            }
            PropertyValue::Bounded {
                upper,
                lower,
                set_point,
                ..
            } => [lower, upper, set_point]
                .into_iter()
                .flatten()
                .filter_map(number)
                .collect(),
        }
    }
}

/// A property set of an object with the type's merged in: a set of the same name is merged
/// property by property, the occurrence's winning.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPropertySet {
    /// The set instance (the occurrence's when both have one).
    pub instance: u64,
    /// The `Name`.
    pub name: Option<String>,
    /// The properties, the occurrence's first.
    pub properties: Vec<ResolvedProperty>,
}

/// A quantity of a resolved quantity set.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedQuantity {
    /// The quantity.
    pub quantity: Arc<Quantity>,
    /// Where the value comes from.
    pub origin: Origin,
    /// The unit that applies: the quantity's own, or the project's for its kind; an index into
    /// [`UnitSystem::units`].
    pub unit: Option<usize>,
    /// The value in SI base units, when a unit factor is known (counts have the value itself).
    pub si_value: Option<f64>,
}

/// A quantity set of an object, merged like [`ResolvedPropertySet`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedQuantitySet {
    /// The set instance.
    pub instance: u64,
    /// The `Name`.
    pub name: Option<String>,
    /// The quantities, the occurrence's first.
    pub quantities: Vec<ResolvedQuantity>,
}

/// An `IfcMappedItem` of an object's representation: the shared representation of a type, placed
/// by the mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct MappedView {
    /// The `IfcMappedItem` instance.
    pub instance: u64,
    /// The `IfcRepresentationMap` instance (`MappingSource`).
    pub map: Option<u64>,
    /// The type object whose `RepresentationMaps` lists the map.
    pub type_object: Option<u64>,
    /// The `MappedRepresentation` of the map; its items are in the map's coordinates.
    pub representation: Option<u64>,
    /// The transform from the map's coordinates to the coordinates of the object's placement: the
    /// `MappingTarget` composed with the `MappingOrigin`, translation in meters. `None` when it
    /// cannot be evaluated; the loss report names the reason.
    pub transform: Option<Matrix>,
}

/// An object of the file: a product, a type object or another object definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectView {
    /// The instance name.
    pub instance: u64,
    /// The entity type, in upper case.
    pub type_name: String,
    /// What the object is.
    pub kind: ObjectKind,
    /// The `GlobalId`.
    pub global_id: Option<String>,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Description`.
    pub description: Option<String>,
    /// The `ObjectType` of an occurrence, the `ElementType` of a type object.
    pub object_type: Option<String>,
    /// The `Tag`.
    pub tag: Option<String>,
    /// The `PredefinedType` as the object states it.
    pub predefined_type: Option<String>,
    /// The predefined type that applies: the type object's, unless it says `NOTDEFINED` (or
    /// `USERDEFINED` without a name), in which case the occurrence's own.
    pub effective_predefined_type: Option<String>,
    /// The user defined type name that goes with an effective `USERDEFINED`: the type object's
    /// `ElementType`, or the occurrence's `ObjectType`.
    pub user_defined_type: Option<String>,
    /// The type object (`IfcRelDefinesByType`).
    pub type_object: Option<u64>,
    /// The spatial structure element that contains the object.
    pub container: Option<u64>,
    /// The `IfcRelAggregates` parent.
    pub aggregate_parent: Option<u64>,
    /// The `IfcRelAggregates` parts, in file order.
    pub parts: Vec<u64>,
    /// The `IfcRelNests` parent.
    pub nest_parent: Option<u64>,
    /// The `IfcRelNests` children, in file order.
    pub nested: Vec<u64>,
    /// The placement in world coordinates; `None` without an `ObjectPlacement`.
    pub placement: Option<PlacementView>,
    /// The elevation of a storey, a building's reference height or a site's reference elevation,
    /// in meters.
    pub elevation: Option<f64>,
    /// The reference latitude and longitude of a site in decimal degrees.
    pub latitude_longitude: Option<(f64, f64)>,
    /// The property sets, merged with the type's.
    pub property_sets: Vec<ResolvedPropertySet>,
    /// The quantity sets, merged with the type's.
    pub quantity_sets: Vec<ResolvedQuantitySet>,
    /// The classification associations, the occurrence's and the type's.
    pub classifications: Vec<ClassificationRef>,
    /// The material association; the occurrence's, or the type's when it has none.
    pub material: Option<(MaterialAssociation, Origin)>,
    /// The surface styles of the object's representation items.
    pub styles: Vec<StyleView>,
    /// The representation items of the object's own representations that are not mapped items
    /// (bodies, curves and so on), in file order. Their geometry is evaluated by
    /// `IfcFile::geometry`, which the views do not repeat.
    pub bodies: Vec<u64>,
    /// The `IfcMappedItem`s of the object's own representations, in file order.
    pub mapped: Vec<MappedView>,
    /// For a type object, the `IfcRepresentationMap`s in its `RepresentationMaps`.
    pub representation_maps: Vec<u64>,
    /// For a host element, the opening elements that void it.
    pub openings: Vec<u64>,
    /// For an opening element, the element it voids.
    pub voids: Option<u64>,
    /// For an opening element, the elements that fill it.
    pub filled_by: Vec<u64>,
    /// For a filling element, the opening element it fills.
    pub fills: Option<u64>,
    /// Indices into [`ImportViews::connections`] of the connections the object takes part in.
    pub connections: Vec<usize>,
    /// Indices into [`ImportViews::space_boundaries`] of the boundaries the object takes part in
    /// (as the space or as the bounding element).
    pub space_boundaries: Vec<usize>,
    /// The groups, systems and zones the object is assigned to.
    pub groups: Vec<u64>,
    /// The `OwnerHistory` instance.
    pub owner_history: Option<u64>,
}

/// A group, system or zone (`IfcRelAssignsToGroup`).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupView {
    /// The instance name.
    pub instance: u64,
    /// The entity type, in upper case.
    pub type_name: String,
    /// The `GlobalId`.
    pub global_id: Option<String>,
    /// The `Name`.
    pub name: Option<String>,
    /// The `Description`.
    pub description: Option<String>,
    /// The `ObjectType`.
    pub object_type: Option<String>,
    /// The `PredefinedType`.
    pub predefined_type: Option<String>,
    /// The `LongName` of a zone or system where the schema declares one.
    pub long_name: Option<String>,
    /// The members, in file order; a member may be a group.
    pub members: Vec<u64>,
}

/// A connection between two elements (`IfcRelConnectsElements` and subtypes).
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    /// The relationship instance.
    pub instance: u64,
    /// The relationship entity: `IFCRELCONNECTSELEMENTS`, `IFCRELCONNECTSPATHELEMENTS` or
    /// `IFCRELCONNECTSWITHREALIZINGELEMENTS`.
    pub kind: String,
    /// The `RelatingElement`.
    pub relating: u64,
    /// The `RelatedElement`.
    pub related: u64,
    /// `RelatingConnectionType` (`ATSTART`, `ATEND`, `ATPATH`, `NOTDEFINED`) of a path connection.
    pub relating_type: Option<String>,
    /// `RelatedConnectionType` of a path connection.
    pub related_type: Option<String>,
    /// The realizing elements of `IfcRelConnectsWithRealizingElements`.
    pub realizing: Vec<u64>,
    /// The `Description`.
    pub description: Option<String>,
}

/// A space boundary (`IfcRelSpaceBoundary` and subtypes).
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceBoundary {
    /// The relationship instance.
    pub instance: u64,
    /// 1 for a first level boundary, 2 for a second level one, 0 when the schema has no levels
    /// (IFC2X3) or the file uses the base entity.
    pub level: u8,
    /// The `RelatingSpace`.
    pub space: u64,
    /// The `RelatedBuildingElement`; absent for a virtual boundary.
    pub element: Option<u64>,
    /// `PhysicalOrVirtualBoundary`: `PHYSICAL`, `VIRTUAL` or `NOTDEFINED`.
    pub physical: Option<String>,
    /// `InternalOrExternalBoundary`.
    pub internal: Option<String>,
    /// The `ConnectionGeometry` instance.
    pub geometry: Option<u64>,
    /// The `ParentBoundary` (first and second level boundaries).
    pub parent: Option<u64>,
    /// The `CorrespondingBoundary` of a second level boundary.
    pub corresponding: Option<u64>,
}

/// The kind of an entry of [`UnitSystem::units`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    /// `IfcSIUnit`.
    Si,
    /// `IfcConversionBasedUnit`, with or without offset.
    ConversionBased,
    /// `IfcDerivedUnit`.
    Derived,
    /// `IfcMonetaryUnit`.
    Monetary,
    /// `IfcContextDependentUnit`.
    ContextDependent,
}

/// A unit of the project's unit assignment.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitView {
    /// The instance name.
    pub instance: u64,
    /// What kind of unit it is.
    pub kind: UnitKind,
    /// The `UnitType` (`LENGTHUNIT`, `AREAUNIT`, ...); the `UserDefinedType` of a derived unit
    /// that has `USERDEFINED`.
    pub unit_type: String,
    /// The `Name`: `METRE`, `FOOT`, or the currency.
    pub name: String,
    /// The factor to SI base units of the unit type (meter, kilogram, second, radian, ...) for a
    /// quantity of one unit; `None` when the unit has no known factor.
    pub to_si: Option<f64>,
    /// The `ConversionOffset` of a conversion based unit with offset, in the base unit.
    pub offset: f64,
}

/// The unit assignment of the project.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitSystem {
    /// The units: the first [`UnitSystem::assigned`] are those of the project's unit assignment,
    /// in file order; the rest are units that properties and quantities name themselves.
    pub units: Vec<UnitView>,
    /// How many of [`UnitSystem::units`] belong to the project's unit assignment.
    pub assigned: usize,
    /// Meters per length unit.
    pub length_to_meters: f64,
    /// Whether no length unit was found and meters were assumed.
    pub length_assumed: bool,
    /// Radians per plane angle unit.
    pub angle_to_radians: f64,
}

impl Default for UnitSystem {
    fn default() -> Self {
        Self {
            units: Vec::new(),
            assigned: 0,
            length_to_meters: 1.0,
            length_assumed: true,
            angle_to_radians: 1.0,
        }
    }
}

/// A coordinate reference system (`IfcProjectedCRS` or the `ePSet_ProjectedCRS` convention).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CrsView {
    /// The `Name`, usually an EPSG code.
    pub name: Option<String>,
    /// The `Description`.
    pub description: Option<String>,
    /// The `GeodeticDatum`.
    pub geodetic_datum: Option<String>,
    /// The `VerticalDatum`.
    pub vertical_datum: Option<String>,
    /// The `MapProjection`.
    pub map_projection: Option<String>,
    /// The `MapZone`.
    pub map_zone: Option<String>,
    /// The `MapUnit` instance, when the file gives one.
    pub map_unit: Option<u64>,
    /// Meters per map unit; meters when the file gives none.
    pub map_unit_to_meters: f64,
}

/// Where the georeference was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoSource {
    /// An `IfcMapConversion` (IFC4 and IFC4X3).
    MapConversion,
    /// The `ePSet_MapConversion` property set, the IFC2X3 convention.
    PropertySet,
}

/// The conversion from the local engineering coordinates to a projected coordinate system.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoReference {
    /// Where it was read from.
    pub source: GeoSource,
    /// The `IfcMapConversion` instance, or the property set.
    pub instance: Option<u64>,
    /// `Eastings`, in map units.
    pub eastings: f64,
    /// `Northings`, in map units.
    pub northings: f64,
    /// `OrthogonalHeight`, in map units.
    pub orthogonal_height: f64,
    /// `XAxisAbscissa`, 1 when absent.
    pub x_axis_abscissa: f64,
    /// `XAxisOrdinate`, 0 when absent.
    pub x_axis_ordinate: f64,
    /// `Scale`, 1 when absent.
    pub scale: f64,
    /// The target coordinate reference system.
    pub crs: Option<CrsView>,
}

/// An `IfcOwnerHistory`.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnerHistory {
    /// The instance name.
    pub instance: u64,
    /// `State`: `READONLY`, `LOCKED`, `READWRITE`, ...
    pub state: Option<String>,
    /// `ChangeAction`.
    pub change_action: Option<String>,
    /// `LastModifiedDate`, seconds since 1970-01-01.
    pub last_modified: Option<i64>,
    /// `CreationDate`, seconds since 1970-01-01.
    pub created: Option<i64>,
    /// The person (`FamilyName`, `GivenNames`, `Identification`) of `OwningUser`.
    pub user: Option<String>,
    /// The organization of `OwningUser`.
    pub organization: Option<String>,
    /// The `ApplicationFullName` of `OwningApplication`.
    pub application: Option<String>,
    /// The `Version` of `OwningApplication`.
    pub application_version: Option<String>,
}

/// How the views treated the instances of one entity class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassCoverage {
    /// The entity type, in upper case.
    pub type_name: String,
    /// The instances of the class in the file.
    pub total: u64,
    /// Instances whose content the views read completely.
    pub interpreted: u64,
    /// Instances the views read in part: something in them was not usable and is in the loss
    /// report.
    pub partial: u64,
    /// Instances the views did not read. They stay in the file model and an edit writes them back
    /// unchanged (round trip).
    pub preserved: u64,
}

/// The import views of a file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportViews {
    /// The units of the project.
    pub units: UnitSystem,
    /// The objects in ascending instance name; look one up with [`ImportViews::object`].
    pub objects: Vec<ObjectView>,
    /// The groups, systems and zones, in file order.
    pub groups: Vec<GroupView>,
    /// The element connections, in file order.
    pub connections: Vec<Connection>,
    /// The space boundaries, in file order.
    pub space_boundaries: Vec<SpaceBoundary>,
    /// The owner histories the objects refer to, in file order.
    pub owner_histories: Vec<OwnerHistory>,
    /// The georeference of the project, if the file has one.
    pub georeference: Option<GeoReference>,
    /// The coverage of every entity class of the file, by class name.
    pub coverage: Vec<ClassCoverage>,
}

impl ImportViews {
    /// The object with the given instance name.
    #[must_use]
    pub fn object(&self, instance: u64) -> Option<&ObjectView> {
        self.objects
            .binary_search_by_key(&instance, |o| o.instance)
            .ok()
            .and_then(|i| self.objects.get(i))
    }

    /// The group with the given instance name.
    #[must_use]
    pub fn group(&self, instance: u64) -> Option<&GroupView> {
        self.groups.iter().find(|g| g.instance == instance)
    }
}

impl MaterialAssociation {
    /// A rough estimate of the bytes a copy of the association takes.
    pub(crate) fn estimated_bytes(&self) -> u64 {
        const PART: u64 = 160;
        let parts = match self {
            Self::Single(_) => 1,
            Self::List(list) => list.len(),
            Self::LayerSet { layers, .. } => layers.len(),
            Self::ProfileSet { profiles, .. } => profiles.len(),
            Self::ConstituentSet { constituents, .. } => constituents.len(),
        };
        PART.saturating_mul(u64::try_from(parts).unwrap_or(u64::MAX).saturating_add(1))
    }
}
