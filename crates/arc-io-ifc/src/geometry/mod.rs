//! Geometry evaluation (CORE-IO-015): the representations of the products of a file, evaluated
//! into triangle meshes and polylines in meters.
//!
//! [`IfcFile::geometry`] walks every product (an instance whose `Representation` refers to an
//! `IfcProductDefinitionShape`), evaluates the representation items of its representations and
//! returns one [`GeometryItem`] per evaluated piece. Each item holds its product and representation
//! item names, the representation identifier and type, and a shape with a transform to world
//! coordinates. The shape of a representation item is computed once and shared: a mapped item
//! refers to the shape of its representation map through an `Arc`, with its own transform, so the
//! geometry of a type used by many products is not copied. The module documentation of each part
//! lists the entities evaluated and the reasons of the loss items for the rest.
//!
//! Every allocation is charged to one [`Budget`] before it is made. Recursion (placement chains,
//! mapped items) shares one depth counter capped at [`MAX_NESTING`]. Cancellation is checked per
//! product and every 1024 steps of a long loop.

mod attr;
mod budget;
mod curves;
mod faces;
mod mapped;
mod place;
mod polygon;
mod profile;
mod solid;
mod units;

/// The most points of one loop or polyline that are evaluated; the triangulation is quadratic in
/// the loop length. A longer loop is not evaluated.
pub(crate) const MAX_LOOP_VERTICES: usize = 4096;

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::math::{Point3, Transform3};
use arc_base::{Cancel, Error, Limits, Result, Tolerance};
use arc_mesh::TriMesh;

use crate::error::reasons;
use crate::loss::{LossHeader, LossLog, LossReport, Reason};
use crate::model::{IfcFile, Parameter, Record};
use crate::step::MAX_NESTING;

use budget::Budget;

pub use units::Units;

/// The geometry of one shape: a triangle mesh, or polylines of 3D points, in the coordinates of
/// the representation item (in meters, before the item's transform).
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A triangle mesh with counterclockwise triangles seen from outside a closed solid.
    Mesh(TriMesh),
    /// Polylines of at least two points each.
    Polylines(Vec<Vec<Point3>>),
}

/// One evaluated piece of a representation item of a product.
#[derive(Debug, Clone, PartialEq)]
pub struct GeometryItem {
    /// The product's instance name.
    pub product: u64,
    /// The representation item's instance name. For an item inside a mapped representation, the
    /// name of the mapped item of the product.
    pub item: u64,
    /// The `RepresentationIdentifier` of the representation the item belongs to.
    pub representation_identifier: Option<String>,
    /// The `RepresentationType` of the representation the item belongs to.
    pub representation_type: Option<String>,
    /// The shape, shared with every other item that uses the same representation item.
    pub shape: Arc<Shape>,
    /// The transform from the shape's coordinates to world coordinates, in meters. It may be a
    /// reflection (a negative determinant), in which case the triangles are wound from inside.
    pub transform: Transform3,
}

impl GeometryItem {
    /// The shape in world coordinates as a new mesh, with the winding of reflected triangles
    /// reversed so that the triangles face outward. `None` for polylines.
    ///
    /// # Errors
    ///
    /// The limits of [`arc_mesh::TriMeshBuilder::build`] apply to the new mesh.
    pub fn world_mesh(&self, limits: &Limits) -> Result<Option<TriMesh>> {
        let Shape::Mesh(mesh) = self.shape.as_ref() else {
            return Ok(None);
        };
        let reflected = self
            .transform
            .matrix()
            .fixed_view::<3, 3>(0, 0)
            .determinant()
            < 0.0;
        let positions = mesh
            .positions()
            .iter()
            .map(|p| self.transform.transform_point(p))
            .collect();
        let triangles = mesh
            .triangles()
            .iter()
            .map(|t| if reflected { [t[0], t[2], t[1]] } else { *t })
            .collect();
        TriMesh::new(positions, triangles, limits).map(Some)
    }
}

/// The result of a geometry evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    /// The evaluated items, products in file order.
    pub items: Vec<GeometryItem>,
    /// The units the coordinates were converted with.
    pub units: Units,
    /// The loss report of the evaluation.
    pub report: LossReport,
}

/// An index of a mesh as a `u32`; a larger one exceeds the array limit of any mesh.
pub(crate) fn index32(index: usize) -> Step<u32> {
    u32::try_from(index).map_err(|_| {
        Flow::Hard(crate::error::limit(
            "array_length",
            u64::try_from(index).unwrap_or(u64::MAX),
        ))
    })
}

/// Bytes charged for one shape or item, besides its vertices and triangles.
const SHAPE_BYTES: u64 = 96;
/// Bytes charged for one vertex of a mesh or one point of a polyline.
pub(crate) const VERTEX_BYTES: u64 = 24;
/// Bytes charged for one triangle of a mesh.
pub(crate) const TRIANGLE_BYTES: u64 = 12;
/// Bytes charged for one piece of a mapped item, a transform and a shared reference.
const PIECE_BYTES: u64 = 160;
/// Bytes charged for one placement.
const PLACEMENT_BYTES: u64 = 128;

/// A piece of a representation item: a shape and its transform to the coordinates of the item's
/// representation.
#[derive(Debug, Clone)]
pub(crate) struct Piece {
    pub(crate) shape: Arc<Shape>,
    pub(crate) transform: Transform3,
}

/// The pieces of an evaluated item, shared between every use of the item.
pub(crate) type Pieces = Arc<Vec<Piece>>;

/// A reason that skips an item: its loss reason and the record kind it is reported under.
#[derive(Debug)]
pub(crate) struct Skip {
    pub(crate) reason: Reason,
    pub(crate) kind: String,
}

/// Why a step of the evaluation stops: a skip of the current item, or an error of the whole call.
#[derive(Debug)]
pub(crate) enum Flow {
    Skip(Skip),
    Hard(Error),
}

impl From<Error> for Flow {
    fn from(error: Error) -> Self {
        Self::Hard(error)
    }
}

/// The result of one step: its value, or the reason it stops.
pub(crate) type Step<T> = std::result::Result<T, Flow>;

/// The state of one evaluation: the file, the tolerance, the budget, the loss log and the caches
/// of placements and evaluated items.
pub(crate) struct Evaluator<'f> {
    pub(crate) file: &'f IfcFile,
    pub(crate) limits: Limits,
    pub(crate) tol: Tolerance,
    pub(crate) budget: Budget,
    pub(crate) log: LossLog,
    /// Meters per length unit of the file, applied to every length read.
    pub(crate) scale: f64,
    /// The deepest recursion allowed: the nesting limit of the profile, at most [`MAX_NESTING`].
    pub(crate) depth_limit: u32,
    placements: BTreeMap<u64, Transform3>,
    items: BTreeMap<u64, Option<Pieces>>,
    stack: Vec<u64>,
}

impl<'f> Evaluator<'f> {
    fn new(file: &'f IfcFile, limits: &Limits, tol: &Tolerance, cancel: &Cancel) -> Self {
        Self {
            file,
            limits: *limits,
            tol: *tol,
            budget: Budget::new(limits, file.parsed_bytes(), cancel),
            log: LossLog::default(),
            scale: 1.0,
            depth_limit: limits.nesting_depth.min(MAX_NESTING),
            placements: BTreeMap::new(),
            items: BTreeMap::new(),
            stack: Vec::new(),
        }
    }

    /// Logs a skip of the item `source`, or passes on an error of the whole call.
    pub(crate) fn settle<T>(&mut self, step: Step<T>, source: u64) -> Result<Option<T>> {
        match step {
            Ok(value) => Ok(Some(value)),
            Err(Flow::Skip(skip)) => {
                self.log.add(skip.reason, &skip.kind, Some(source), 1);
                Ok(None)
            }
            Err(Flow::Hard(error)) => Err(error),
        }
    }

    /// The pieces of the representation item `name`, evaluated once and cached. `None` when the
    /// item is skipped (logged once) or is already being evaluated (a cycle, cut and logged).
    pub(crate) fn item(&mut self, name: u64, depth: u32) -> Result<Option<Pieces>> {
        if let Some(cached) = self.items.get(&name) {
            return Ok(cached.clone());
        }
        if self.stack.contains(&name) {
            self.log
                .add(reasons::CYCLE_CUT, "IFCMAPPEDITEM", Some(name), 1);
            return Ok(None);
        }
        if depth > self.depth_limit {
            return Err(crate::error::limit("nesting_depth", u64::from(depth)));
        }
        self.budget.record()?;
        self.stack.push(name);
        let step = self.evaluate(name, depth);
        self.stack.pop();
        let pieces = self.settle(step, name)?.map(Arc::new);
        self.items.insert(name, pieces.clone());
        Ok(pieces)
    }

    /// Evaluates one representation item by its type.
    fn evaluate(&mut self, name: u64, depth: u32) -> Step<Vec<Piece>> {
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            return Err(attr::skip(reasons::TARGET_MISSING, "IFCREPRESENTATIONITEM"));
        };
        let [record] = instance.records.as_slice() else {
            let kind = instance
                .records
                .first()
                .map_or("IFCREPRESENTATIONITEM", |r| r.type_name.as_str());
            return Err(attr::skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        };
        self.dispatch(name, record, depth)
    }

    fn dispatch(&mut self, name: u64, record: &Record, depth: u32) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        match kind {
            "IFCMAPPEDITEM" => self.mapped(record, depth),
            "IFCEXTRUDEDAREASOLID" => self.extrusion(record),
            "IFCTRIANGULATEDFACESET"
            | "IFCPOLYGONALFACESET"
            | "IFCFACETEDBREP"
            | "IFCFACEBASEDSURFACEMODEL"
            | "IFCSHELLBASEDSURFACEMODEL" => self.faces(name, record),
            "IFCPOLYLINE" | "IFCINDEXEDPOLYCURVE" => self.curve(record),
            _ => Err(attr::skip(reasons::GEOMETRY_NOT_EVALUATED, kind)),
        }
    }

    /// One piece with the identity transform, around a shape the caller has charged.
    pub(crate) fn own(shape: Shape) -> Vec<Piece> {
        vec![Piece {
            shape: Arc::new(shape),
            transform: Transform3::identity(),
        }]
    }

    /// Charges the fixed cost of one shape.
    pub(crate) fn charge_shape(&mut self) -> Step<()> {
        self.budget.charge(SHAPE_BYTES)?;
        Ok(())
    }

    /// Charges a placement before it is stored.
    pub(crate) fn charge_placement(&mut self) -> Step<()> {
        self.budget.charge(PLACEMENT_BYTES)?;
        Ok(())
    }

    /// Charges one piece of a mapped item and counts it as a record.
    pub(crate) fn charge_piece(&mut self) -> Result<()> {
        self.budget.charge(PIECE_BYTES)?;
        self.budget.record()
    }

    /// The placement of a product or of a local placement, memoized by name.
    pub(crate) fn placement_memo(&self, name: u64) -> Option<Transform3> {
        self.placements.get(&name).cloned()
    }

    /// Stores a placement computed for `name`.
    pub(crate) fn store_placement(&mut self, name: u64, transform: Transform3) {
        self.placements.insert(name, transform);
    }

    /// The world transform of a product's placement, or the identity for a product without one.
    fn product_transform(
        &mut self,
        product: u64,
        placement: Option<u64>,
    ) -> Result<Option<Transform3>> {
        let Some(placement) = placement else {
            return Ok(Some(Transform3::identity()));
        };
        let step = self.local_placement(placement);
        self.settle(step, product)
    }

    /// The representations of a product and their evaluated items, appended to `out`.
    fn product_items(
        &mut self,
        product: u64,
        shape: u64,
        world: Transform3,
        out: &mut Vec<GeometryItem>,
    ) -> Result<()> {
        let file = self.file;
        let Some(record) = attr::typed(file, shape, "IFCPRODUCTDEFINITIONSHAPE") else {
            return Ok(());
        };
        let step = attr::references(record.parameters.get(2), "IFCPRODUCTDEFINITIONSHAPE");
        let Some((representations, _)) = self.settle(step, shape)? else {
            return Ok(());
        };
        for representation in representations {
            self.budget.check()?;
            let Some(rep) = file
                .instance(representation)
                .and_then(|i| i.records.first())
            else {
                self.log.add(
                    reasons::TARGET_MISSING,
                    "IFCSHAPEREPRESENTATION",
                    Some(shape),
                    1,
                );
                continue;
            };
            let identifier = attr::text(rep.parameters.get(1), "IFCSHAPEREPRESENTATION");
            let identifier = self.settle(identifier, representation)?.flatten();
            let rtype = attr::text(rep.parameters.get(2), "IFCSHAPEREPRESENTATION");
            let rtype = self.settle(rtype, representation)?.flatten();
            let step = attr::references(rep.parameters.get(3), "IFCSHAPEREPRESENTATION");
            let Some((items, repeats)) = self.settle(step, representation)? else {
                continue;
            };
            self.log.add(
                reasons::DUPLICATE_PLACEMENT,
                &rep.type_name,
                Some(representation),
                repeats,
            );
            for item in items {
                let Some(pieces) = self.item(item, 0)? else {
                    continue;
                };
                for piece in pieces.iter() {
                    self.budget.record()?;
                    out.push(GeometryItem {
                        product,
                        item,
                        representation_identifier: identifier.clone(),
                        representation_type: rtype.clone(),
                        shape: Arc::clone(&piece.shape),
                        transform: world * piece.transform,
                    });
                }
            }
        }
        Ok(())
    }
}

/// The products of the file: instance name, placement reference and product definition shape.
fn products(file: &IfcFile) -> Vec<(u64, Option<u64>, u64)> {
    let mut out = Vec::new();
    for instance in file.instances() {
        for record in &instance.records {
            let Some(Parameter::Reference(shape)) = record.parameters.get(6) else {
                continue;
            };
            if attr::typed(file, *shape, "IFCPRODUCTDEFINITIONSHAPE").is_none() {
                continue;
            }
            let placement = match record.parameters.get(5) {
                Some(Parameter::Reference(name)) => Some(*name),
                _ => None,
            };
            out.push((instance.name, placement, *shape));
            break;
        }
    }
    out
}

/// Counts the openings of the file, reported once.
fn count_openings(evaluator: &mut Evaluator<'_>) {
    let count = evaluator.file.instances_of("IFCRELVOIDSELEMENT").count();
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    if count > 0 {
        evaluator.log.add(
            reasons::OPENING_NOT_SUBTRACTED,
            "IFCRELVOIDSELEMENT",
            None,
            count,
        );
    }
}

/// The whole evaluation.
fn run(file: &IfcFile, limits: &Limits, tol: &Tolerance, cancel: &Cancel) -> Result<Geometry> {
    cancel.check()?;
    let mut evaluator = Evaluator::new(file, limits, tol, cancel);
    let units = evaluator.read_units()?;
    count_openings(&mut evaluator);
    let mut items = Vec::new();
    for (product, placement, shape) in products(file) {
        evaluator.budget.check()?;
        let Some(world) = evaluator.product_transform(product, placement)? else {
            continue;
        };
        evaluator.product_items(product, shape, world, &mut items)?;
    }
    let report = evaluator
        .log
        .finish(LossHeader::import(file.schema.identifier(), *limits));
    Ok(Geometry {
        items,
        units,
        report,
    })
}

impl IfcFile {
    /// Evaluates the geometry of the products of the file with the default tolerance
    /// ([`Tolerance::DEFAULT`]); see [`Self::geometry_with_tolerance`].
    ///
    /// # Errors
    ///
    /// As [`Self::geometry_with_tolerance`].
    pub fn geometry(&self, limits: &Limits, cancel: &Cancel) -> Result<Geometry> {
        self.geometry_with_tolerance(limits, cancel, &Tolerance::DEFAULT)
    }

    /// Evaluates the geometry of the products of the file. The chord tolerance of the tolerance
    /// sets the segment count of circles; the linear tolerance sets the planarity check of faces.
    /// Items that are not evaluated are reported in [`Geometry::report`].
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` for `memory_bytes`, `record_count`, `array_length` (a mesh or polyline
    /// longer than the limit, checked before it is allocated) and `nesting_depth` (a placement
    /// chain or mapped nesting deeper than the limit or 64); `base.cancelled` when the token was
    /// cancelled. Malformed items are not errors: they are skipped and reported.
    pub fn geometry_with_tolerance(
        &self,
        limits: &Limits,
        cancel: &Cancel,
        tolerance: &Tolerance,
    ) -> Result<Geometry> {
        run(self, limits, tolerance, cancel)
    }
}
