//! Write, read, compare: the typed views read back what the writer writes for the element kinds of
//! the ArchBuilder families. Each element's class, predefined type and `ObjectType`; its type
//! object with the type's property and quantity sets; its geometry through an `IfcMappedItem`,
//! whose resolved transform places the shared body where the direct body is; and its aggregation
//! through its host.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code: fixtures and assertions"
)]

mod support;

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Cancel, Limits};
use arc_io_ifc::schema::SchemaTables;
use arc_io_ifc::write::{
    Body, Element, ElementKind, ElementType, OutputSchema, Placement, Property, PropertySet,
    PropertyValue, Quantity, QuantitySet, QuantityValue, Spatial, Storey, WriteModel, WriteOptions,
};
use arc_io_ifc::{
    IfcFile, Matrix, ObjectKind, ObjectView, Origin, ReadOptions, ResolvedPropertySet,
    ResolvedQuantitySet, Schema, TypedViews, read, write,
};
use arc_mesh::measure::{bounds, volume};

const STOREY: u128 = 0x100;
const DESK: u128 = 0x1000;
const LAMP: u128 = 0x1001;
const KETTLE: u128 = 0x1002;

fn rectangle(x: f64, y: f64) -> Vec<Point2> {
    vec![
        Point2::new(0.0, 0.0),
        Point2::new(x, 0.0),
        Point2::new(x, y),
        Point2::new(0.0, y),
    ]
}

/// A box of `x` by `y` by `z`, from the origin of its own coordinates.
fn box_body(x: f64, y: f64, z: f64) -> Body {
    Body::Extrusion {
        outer: rectangle(x, y),
        holes: Vec::new(),
        direction: Vector3::z(),
        depth: z,
    }
}

fn base() -> WriteModel {
    let mut model = WriteModel::new(
        Spatial::new(1, "Project"),
        Spatial::new(2, "Site"),
        Spatial::new(3, "Building"),
    );
    model.storeys.push(Storey::new(STOREY, "Ground", 0.0));
    model
}

fn element(id: u128, kind: ElementKind, name: &str, predefined: Option<&str>) -> Element {
    let mut e = Element::new(id, kind, name, STOREY);
    e.predefined_type = predefined.map(str::to_owned);
    e.body = Some(box_body(0.5, 0.5, 0.9));
    e
}

fn written(model: &WriteModel, name: &str) -> (Vec<u8>, IfcFile) {
    // The import views read through the schema tables; IFC4X3's are part of every build, IFC4's
    // only where they were generated (ADR-0018).
    let schema = if SchemaTables::of(&Schema::Ifc4).is_some() {
        OutputSchema::Ifc4
    } else {
        OutputSchema::Ifc4x3
    };
    let options = WriteOptions {
        schema,
        ..WriteOptions::default()
    };
    let output = write(model, &options).unwrap();
    support::dump(name, &output.bytes);
    let file = read(&output.bytes, &mut ReadOptions::default())
        .unwrap()
        .file;
    (output.bytes, file)
}

fn views_of(file: &IfcFile) -> TypedViews {
    file.typed_views(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap()
}

fn instance_of(file: &IfcFile, entity: &str) -> u64 {
    file.instances_of(entity).next().expect(entity).name
}

fn object(views: &TypedViews, instance: u64) -> &ObjectView {
    views
        .import
        .objects
        .iter()
        .find(|o| o.instance == instance)
        .expect("the object is read")
}

fn set<'a>(sets: &'a [ResolvedPropertySet], name: &str) -> &'a ResolvedPropertySet {
    sets.iter()
        .find(|s| s.name.as_deref() == Some(name))
        .expect(name)
}

/// Row-major product of two transforms: `a` applied after `b`.
fn compose(a: &Matrix, b: &Matrix) -> Matrix {
    let mut out = [0.0; 16];
    for row in 0..4 {
        for col in 0..4 {
            out[row * 4 + col] = (0..4).map(|k| a[row * 4 + k] * b[k * 4 + col]).sum();
        }
    }
    out
}

fn assert_same(a: &Matrix, b: &Matrix, what: &str) {
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < 1e-9, "{what}: {a:?} != {b:?}");
    }
}

/// Every kind, its element and type entities, the value the writer names and the one each case
/// writes.
type Case = (ElementKind, &'static str, &'static str, &'static str);

const CASES: [Case; 8] = [
    (
        ElementKind::Furniture,
        "IFCFURNITURE",
        "IFCFURNITURETYPE",
        "FILECABINET",
    ),
    (
        ElementKind::SanitaryTerminal,
        "IFCSANITARYTERMINAL",
        "IFCSANITARYTERMINALTYPE",
        "WCSEAT",
    ),
    (
        ElementKind::LightFixture,
        "IFCLIGHTFIXTURE",
        "IFCLIGHTFIXTURETYPE",
        "DIRECTIONSOURCE",
    ),
    (
        ElementKind::ElectricAppliance,
        "IFCELECTRICAPPLIANCE",
        "IFCELECTRICAPPLIANCETYPE",
        "FRIDGE_FREEZER",
    ),
    (
        ElementKind::UnitaryEquipment,
        "IFCUNITARYEQUIPMENT",
        "IFCUNITARYEQUIPMENTTYPE",
        "SPLITSYSTEM",
    ),
    (
        ElementKind::Column,
        "IFCCOLUMN",
        "IFCCOLUMNTYPE",
        "PILASTER",
    ),
    (ElementKind::Beam, "IFCBEAM", "IFCBEAMTYPE", "T_BEAM"),
    (
        ElementKind::BuildingElementProxy,
        "IFCBUILDINGELEMENTPROXY",
        "IFCBUILDINGELEMENTPROXYTYPE",
        "PARTIAL",
    ),
];

#[test]
fn every_new_kind_reads_back_its_class_predefined_and_user_defined_type() {
    let mut model = base();
    for (index, (kind, _, _, chosen)) in CASES.into_iter().enumerate() {
        let index = u128::try_from(index).unwrap();
        // An occurrence of a type with the predefined value.
        let mut typed = element(0x1000 + index, kind, "Typed", Some(chosen));
        typed.element_type = Some(0x2000 + index);
        model.elements.push(typed);
        let mut element_type = ElementType::new(0x2000 + index, kind, "Type");
        element_type.predefined_type = Some(chosen.to_owned());
        model.types.push(element_type);
        // An occurrence with a value of its own: written as USERDEFINED with its ObjectType.
        model
            .elements
            .push(element(0x3000 + index, kind, "Bespoke", Some("SITEBUILT")));
    }
    let (_, file) = written(&model, "view_kinds_predefined.ifc");
    let views = views_of(&file);

    for (_, entity, type_entity, chosen) in CASES {
        let typed_instance = file
            .instances_of(entity)
            .map(|i| i.name)
            .find(|name| object(&views, *name).name.as_deref() == Some("Typed"))
            .expect("the typed occurrence");
        let typed = object(&views, typed_instance);
        assert_eq!(typed.type_name, entity);
        assert_eq!(typed.kind, ObjectKind::Product);
        assert_eq!(typed.predefined_type.as_deref(), Some(chosen));
        assert_eq!(typed.effective_predefined_type.as_deref(), Some(chosen));
        assert_eq!(typed.user_defined_type, None);
        let type_instance = file.instances_of(type_entity).next().unwrap().name;
        assert_eq!(typed.type_object, Some(type_instance), "{entity}");

        let type_view = object(&views, type_instance);
        assert_eq!(type_view.kind, ObjectKind::Type);
        assert_eq!(type_view.type_name, type_entity);
        assert_eq!(type_view.predefined_type.as_deref(), Some(chosen));

        let bespoke_instance = file
            .instances_of(entity)
            .map(|i| i.name)
            .find(|name| object(&views, *name).name.as_deref() == Some("Bespoke"))
            .expect("the bespoke occurrence");
        let bespoke = object(&views, bespoke_instance);
        assert_eq!(bespoke.predefined_type.as_deref(), Some("USERDEFINED"));
        assert_eq!(bespoke.object_type.as_deref(), Some("SITEBUILT"));
        assert_eq!(bespoke.user_defined_type.as_deref(), Some("SITEBUILT"));
        assert_eq!(bespoke.type_object, None, "{entity}");
    }
}

/// A column type with a property set and a quantity set, an empty set the writer reports, and two
/// occurrences: one with no sets of its own, one overriding a property of the type's set.
fn column_model() -> WriteModel {
    let mut model = base();
    let mut column_type = ElementType::new(0x2100, ElementKind::Column, "Concrete 300");
    column_type.predefined_type = Some("COLUMN".into());
    column_type.property_sets = vec![
        PropertySet::new(
            "Pset_ColumnCommon",
            vec![
                Property::new("Reference", PropertyValue::identifier("C-300").unwrap()),
                Property::new(
                    "Width",
                    PropertyValue::measure(arc_io_ifc::write::Measure::PositiveLength, 0.3)
                        .unwrap(),
                ),
            ],
        ),
        PropertySet::new("Pset_Empty", Vec::new()),
    ];
    column_type.quantity_sets = vec![QuantitySet::new(
        "Qto_ColumnBaseQuantities",
        vec![Quantity::new("Length", QuantityValue::Length(3.0))],
    )];
    model.types.push(column_type);

    let mut inherits = element(0x1100, ElementKind::Column, "Inherits", Some("COLUMN"));
    inherits.element_type = Some(0x2100);
    model.elements.push(inherits);

    let mut overrides = element(0x1101, ElementKind::Column, "Overrides", Some("COLUMN"));
    overrides.element_type = Some(0x2100);
    overrides.property_sets = vec![PropertySet::new(
        "Pset_ColumnCommon",
        vec![Property::new(
            "Width",
            PropertyValue::measure(arc_io_ifc::write::Measure::PositiveLength, 0.4).unwrap(),
        )],
    )];
    model.elements.push(overrides);
    model
}

#[test]
fn type_property_and_quantity_sets_read_back_on_the_type_and_its_occurrences() {
    let (_, file) = written(&column_model(), "view_kinds_type_sets.ifc");
    let views = views_of(&file);
    let type_instance = instance_of(&file, "IFCCOLUMNTYPE");
    let column_type = object(&views, type_instance);

    // The type lists the sets it owns; the empty one is not written, so it is not read.
    let names: Vec<_> = column_type
        .property_sets
        .iter()
        .map(|s| s.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["Pset_ColumnCommon"]);
    assert_eq!(column_type.quantity_sets.len(), 1);
    assert_eq!(
        column_type.quantity_sets[0].name.as_deref(),
        Some("Qto_ColumnBaseQuantities")
    );

    let by_name = |name: &str| {
        let instance = file
            .instances_of("IFCCOLUMN")
            .map(|i| i.name)
            .find(|n| object(&views, *n).name.as_deref() == Some(name))
            .expect(name);
        object(&views, instance)
    };

    // An occurrence without sets of its own inherits all of the type's.
    let inherits = by_name("Inherits");
    let common: &ResolvedPropertySet = set(&inherits.property_sets, "Pset_ColumnCommon");
    assert_eq!(common.properties.len(), 2);
    assert!(common.properties.iter().all(|p| p.origin == Origin::Type));
    let width = common
        .properties
        .iter()
        .find(|p| p.property.name.as_deref() == Some("Width"))
        .unwrap();
    assert_eq!(width.si_numbers(), vec![0.3]);
    let reference = common
        .properties
        .iter()
        .find(|p| p.property.name.as_deref() == Some("Reference"))
        .unwrap();
    assert_eq!(reference.origin, Origin::Type);
    let quantities: &ResolvedQuantitySet =
        set_quantities(&inherits.quantity_sets, "Qto_ColumnBaseQuantities");
    assert_eq!(quantities.quantities.len(), 1);
    assert_eq!(quantities.quantities[0].origin, Origin::Type);
    assert_eq!(quantities.quantities[0].si_value, Some(3.0));

    // An occurrence's own value replaces the type's of the same name; the others are inherited.
    let overrides = by_name("Overrides");
    let common = set(&overrides.property_sets, "Pset_ColumnCommon");
    let width = common
        .properties
        .iter()
        .find(|p| p.property.name.as_deref() == Some("Width"))
        .unwrap();
    assert_eq!(width.origin, Origin::Occurrence);
    assert_eq!(width.si_numbers(), vec![0.4]);
    let reference = common
        .properties
        .iter()
        .find(|p| p.property.name.as_deref() == Some("Reference"))
        .unwrap();
    assert_eq!(reference.origin, Origin::Type);
    assert_eq!(common.properties.len(), 2);
}

fn set_quantities<'a>(sets: &'a [ResolvedQuantitySet], name: &str) -> &'a ResolvedQuantitySet {
    sets.iter()
        .find(|s| s.name.as_deref() == Some(name))
        .expect(name)
}

/// A chair type with a shared representation and three occurrences placed by a mapping: at the
/// origin, translated, and translated and turned a quarter turn.
const PLACEMENTS: [([f64; 3], [f64; 3]); 3] = [
    ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
    ([2.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
    ([4.0, 1.0, 0.0], [0.0, 1.0, 0.0]),
];

fn placement(index: usize) -> Placement {
    let (origin, reference) = PLACEMENTS[index];
    Placement::new(
        Point3::new(origin[0], origin[1], origin[2]),
        Vector3::z(),
        Vector3::new(reference[0], reference[1], reference[2]),
    )
}

fn chair_type() -> ElementType {
    let mut chair = ElementType::new(0x2200, ElementKind::Furniture, "Chair");
    chair.predefined_type = Some("CHAIR".into());
    chair.representation = Some(box_body(0.5, 0.5, 0.9));
    chair
}

/// The shared chair: three occurrences that map the type's representation.
fn mapped_model() -> WriteModel {
    let mut model = base();
    model.types.push(chair_type());
    for index in 0..PLACEMENTS.len() {
        let mut item = Element::new(
            0x2300 + u128::try_from(index).unwrap(),
            ElementKind::Furniture,
            "Chair",
            STOREY,
        );
        item.predefined_type = Some("CHAIR".into());
        item.element_type = Some(0x2200);
        item.mapping = Some(placement(index));
        model.elements.push(item);
    }
    model
}

/// The same three chairs with their bodies written directly, at the same placements.
fn direct_model() -> WriteModel {
    let mut model = base();
    for index in 0..PLACEMENTS.len() {
        let mut item = Element::new(
            0x2300 + u128::try_from(index).unwrap(),
            ElementKind::Furniture,
            "Chair",
            STOREY,
        );
        item.predefined_type = Some("CHAIR".into());
        item.placement = placement(index);
        item.body = Some(box_body(0.5, 0.5, 0.9));
        model.elements.push(item);
    }
    model
}

#[test]
fn a_mapped_item_shared_by_occurrences_resolves_to_the_direct_body() {
    let (_, mapped_file) = written(&mapped_model(), "view_kinds_mapped_roundtrip.ifc");
    let (_, direct_file) = written(&direct_model(), "view_kinds_direct_roundtrip.ifc");
    let mapped_views = views_of(&mapped_file);
    let direct_views = views_of(&direct_file);

    let map = instance_of(&mapped_file, "IFCREPRESENTATIONMAP");
    let chair_type = instance_of(&mapped_file, "IFCFURNITURETYPE");
    let type_view = object(&mapped_views, chair_type);
    assert_eq!(type_view.representation_maps, [map]);
    assert!(type_view.bodies.is_empty());

    let mapped_chairs: Vec<u64> = mapped_file
        .instances_of("IFCFURNITURE")
        .map(|i| i.name)
        .collect();
    let direct_chairs: Vec<u64> = direct_file
        .instances_of("IFCFURNITURE")
        .map(|i| i.name)
        .collect();
    assert_eq!(mapped_chairs.len(), PLACEMENTS.len());
    assert_eq!(direct_chairs.len(), PLACEMENTS.len());

    let mapped_geometry = mapped_file
        .geometry(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();
    let direct_geometry = direct_file
        .geometry(&Limits::CONSTRAINED, &Cancel::new())
        .unwrap();

    for (index, (mapped_chair, direct_chair)) in
        mapped_chairs.iter().zip(&direct_chairs).enumerate()
    {
        // The occurrence names the shared map through its own mapped item, and the type.
        let occurrence = object(&mapped_views, *mapped_chair);
        assert!(occurrence.bodies.is_empty(), "no direct body");
        assert_eq!(occurrence.mapped.len(), 1);
        let mapped = &occurrence.mapped[0];
        assert_eq!(mapped.map, Some(map));
        assert_eq!(mapped.type_object, Some(chair_type));
        let transform = mapped.transform.expect("the mapping is resolved");

        // The mapped placement composed with the mapping is the direct occurrence's placement.
        let mapped_world = object(&mapped_views, *mapped_chair)
            .placement
            .as_ref()
            .expect("placed")
            .world;
        let direct_world = object(&direct_views, *direct_chair)
            .placement
            .as_ref()
            .expect("placed")
            .world;
        assert_same(
            &compose(&mapped_world, &transform),
            &direct_world,
            "placement",
        );

        // Both resolved bodies have the same volume and bounds as the direct body.
        let mesh_of = |geometry: &arc_io_ifc::Geometry, product: u64| {
            let item = geometry
                .items
                .iter()
                .find(|item| item.product == product)
                .expect("each occurrence has geometry");
            item.world_mesh(&Limits::CONSTRAINED).unwrap().unwrap()
        };
        let mapped_mesh = mesh_of(&mapped_geometry, *mapped_chair);
        let direct_mesh = mesh_of(&direct_geometry, *direct_chair);
        let measured = volume(&mapped_mesh, &Limits::CONSTRAINED).unwrap();
        let expected = volume(&direct_mesh, &Limits::CONSTRAINED).unwrap();
        assert!((measured - expected).abs() < 1e-9, "{index}: {measured}");
        assert!((measured - 0.5 * 0.5 * 0.9).abs() < 1e-9);
        let (a, b) = (bounds(&mapped_mesh), bounds(&direct_mesh));
        for (p, q) in [(a.min(), b.min()), (a.max(), b.max())] {
            assert!(
                (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9 && (p.z - q.z).abs() < 1e-9,
                "{index}: {p:?} != {q:?}"
            );
        }
    }
}

/// A desk with a lamp and a kettle on it, both aggregated by the desk.
fn desk_model() -> WriteModel {
    let mut model = base();
    model
        .elements
        .push(element(DESK, ElementKind::Furniture, "Desk", Some("DESK")));
    let mut lamp = element(LAMP, ElementKind::LightFixture, "Lamp", Some("POINTSOURCE"));
    lamp.host = Some(DESK);
    model.elements.push(lamp);
    let mut kettle = element(
        KETTLE,
        ElementKind::ElectricAppliance,
        "Kettle",
        Some("KITCHENMACHINE"),
    );
    kettle.host = Some(DESK);
    model.elements.push(kettle);
    model
}

#[test]
fn aggregation_reads_back_as_parts_and_parent_and_not_as_containment() {
    let (_, file) = written(&desk_model(), "view_kinds_aggregation_roundtrip.ifc");
    let views = views_of(&file);
    let storey = instance_of(&file, "IFCBUILDINGSTOREY");
    let desk = file
        .instances_of("IFCFURNITURE")
        .map(|i| i.name)
        .next()
        .unwrap();
    let lamp = file.instances_of("IFCLIGHTFIXTURE").next().unwrap().name;
    let kettle = file
        .instances_of("IFCELECTRICAPPLIANCE")
        .next()
        .unwrap()
        .name;

    let desk_view = object(&views, desk);
    let mut parts = desk_view.parts.clone();
    parts.sort_unstable();
    let mut expected = [lamp, kettle];
    expected.sort_unstable();
    assert_eq!(parts, expected);
    assert_eq!(desk_view.aggregate_parent, None);
    assert_eq!(desk_view.container, Some(storey));

    for part in [lamp, kettle] {
        let view = object(&views, part);
        assert_eq!(view.aggregate_parent, Some(desk), "{part}");
        assert!(view.parts.is_empty());
        // A part is aggregated by its host and is not contained in the storey itself.
        assert_eq!(view.container, None, "{part}");
    }
}
