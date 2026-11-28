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

mod advanced;
mod attr;
mod boolean;
mod bspline;
mod budget;
mod csg;
mod curves;
mod faces;
mod mapped;
mod offset;
mod opening;
mod place;
mod polygon;
mod profile;
mod sectioned;
mod solid;
mod surface;
mod swept;
mod units;

/// The most points of one loop or polyline that are evaluated; the triangulation is quadratic in
/// the loop length. A longer loop is not evaluated.
pub(crate) const MAX_LOOP_VERTICES: usize = 4096;

use crate::schema::access::known;
use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::math::{Point3, Transform3};
use arc_base::{Cancel, Error, Limits, Result, Tolerance};
use arc_mesh::TriMesh;

use crate::error::reasons;
use crate::loss::{Direction, LossHeader, LossLog, LossReport, Reason, Source};
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
    /// The `RepresentationIdentifier` of the representation the item belongs to. Shared by every
    /// item of the representation.
    pub representation_identifier: Option<Arc<str>>,
    /// The `RepresentationType` of the representation the item belongs to. Shared by every item of
    /// the representation.
    pub representation_type: Option<Arc<str>>,
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
        shape_mesh(&self.shape, &self.transform, limits)
    }
}

/// The mesh of `shape` moved by `transform`, with the winding of a reflection reversed so that the
/// triangles face outward; `None` for polylines.
pub(crate) fn shape_mesh(
    shape: &Shape,
    transform: &Transform3,
    limits: &Limits,
) -> Result<Option<TriMesh>> {
    let Shape::Mesh(mesh) = shape else {
        return Ok(None);
    };
    let reflected = transform.matrix().fixed_view::<3, 3>(0, 0).determinant() < 0.0;
    let positions = mesh
        .positions()
        .iter()
        .map(|p| transform.transform_point(p))
        .collect();
    let triangles = mesh
        .triangles()
        .iter()
        .map(|t| if reflected { [t[0], t[2], t[1]] } else { *t })
        .collect();
    TriMesh::new(positions, triangles, limits).map(Some)
}

/// What a geometry evaluation does besides evaluating the representation items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryOptions {
    /// Subtract the bodies of the openings of an element (`IfcRelVoidsElement`) from the element's
    /// extruded body, as the module `opening` describes. When false, or for an opening that cannot be
    /// subtracted, the opening is reported as `io.ifc.opening_not_subtracted` and the element is
    /// emitted solid.
    pub subtract_openings: bool,
}

impl Default for GeometryOptions {
    fn default() -> Self {
        Self {
            subtract_openings: true,
        }
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

/// The representation identifier and type of a representation, shared by its items.
type TextPair = (Option<Arc<str>>, Option<Arc<str>>);

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
    /// Radians per plane angle unit of the file, applied to every angle read.
    pub(crate) angle: f64,
    /// The deepest recursion allowed: the nesting limit of the profile, at most [`MAX_NESTING`].
    pub(crate) depth_limit: u32,
    placements: BTreeMap<u64, Transform3>,
    items: BTreeMap<u64, Option<Pieces>>,
    stack: Vec<u64>,
    /// The shared `RepresentationIdentifier` and `RepresentationType` of each representation read.
    texts: BTreeMap<u64, TextPair>,
    /// The number of mapping cycles cut so far. An item whose evaluation saw a cut depends on the
    /// items being evaluated around it, so its result is not cached.
    cuts: u64,
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
            angle: 1.0,
            depth_limit: limits.nesting_depth.min(MAX_NESTING),
            placements: BTreeMap::new(),
            items: BTreeMap::new(),
            stack: Vec::new(),
            texts: BTreeMap::new(),
            cuts: 0,
        }
    }

    /// Logs a skip of the item `source`, or passes on an error of the whole call.
    pub(crate) fn settle<T>(&mut self, step: Step<T>, source: u64) -> Result<Option<T>> {
        match step {
            Ok(value) => Ok(Some(value)),
            Err(Flow::Skip(skip)) => {
                self.log
                    .add(skip.reason, &skip.kind, Some(Source::Decimal(source)), 1);
                Ok(None)
            }
            Err(Flow::Hard(error)) => Err(error),
        }
    }

    /// The pieces of the representation item `name`, evaluated and cached. `None` when the item is
    /// skipped (logged once) or is already being evaluated (a cycle, cut and logged). A result that
    /// saw a cycle cut is returned but not cached, so a later reference to the item is evaluated
    /// again from its own place in the file.
    pub(crate) fn item(&mut self, name: u64, depth: u32) -> Result<Option<Pieces>> {
        if let Some(cached) = self.items.get(&name) {
            return Ok(cached.clone());
        }
        if self.stack.contains(&name) {
            self.log.add(
                reasons::CYCLE_CUT,
                "IFCMAPPEDITEM",
                Some(Source::Decimal(name)),
                1,
            );
            self.cuts = self.cuts.saturating_add(1);
            return Ok(None);
        }
        if depth > self.depth_limit {
            return Err(crate::error::limit("nesting_depth", u64::from(depth)));
        }
        self.budget.record()?;
        self.stack.push(name);
        let cuts_before = self.cuts;
        let step = self.evaluate(name, depth);
        self.stack.pop();
        let cut = self.cuts != cuts_before;
        let pieces = self.settle(step, name)?.map(Arc::new);
        if !cut {
            self.items.insert(name, pieces.clone());
        }
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
            "IFCMAPPEDITEM" => self.mapped(name, record, depth),
            "IFCEXTRUDEDAREASOLID" | "IFCEXTRUDEDAREASOLIDTAPERED" => self.extrusion(record),
            "IFCBOOLEANRESULT" | "IFCBOOLEANCLIPPINGRESULT" => self.boolean(name, record, depth),
            "IFCREVOLVEDAREASOLID" | "IFCREVOLVEDAREASOLIDTAPERED" => self.revolution(name, record),
            "IFCBLOCK"
            | "IFCRECTANGULARPYRAMID"
            | "IFCRIGHTCIRCULARCONE"
            | "IFCRIGHTCIRCULARCYLINDER"
            | "IFCSPHERE" => self.csg_primitive(name, record),
            "IFCCSGSOLID" => self.csg_solid(name, record, depth),
            "IFCSWEPTDISKSOLID" | "IFCSWEPTDISKSOLIDPOLYGONAL" => self.swept_disk(name, record),
            "IFCSURFACECURVESWEPTAREASOLID" | "IFCFIXEDREFERENCESWEPTAREASOLID" => {
                self.directrix_sweep(record)
            }
            "IFCSECTIONEDSPINE" | "IFCSECTIONEDSOLIDHORIZONTAL" => self.sectioned(name, record),
            "IFCADVANCEDBREP" | "IFCADVANCEDBREPWITHVOIDS" => self.advanced_brep(name, record),
            "IFCTRIANGULATEDFACESET"
            | "IFCPOLYGONALFACESET"
            | "IFCFACETEDBREP"
            | "IFCFACETEDBREPWITHVOIDS"
            | "IFCFACEBASEDSURFACEMODEL"
            | "IFCSHELLBASEDSURFACEMODEL" => self.faces(name, record),
            "IFCGEOMETRICSET" | "IFCGEOMETRICCURVESET" => self.geometric_set(name, record),
            "IFCCARTESIANPOINT" => {
                let p = self.point(name)?;
                self.charge_shape()?;
                Ok(Self::own(Shape::Polylines(vec![vec![p, p]])))
            }
            "IFCBSPLINESURFACEWITHKNOTS" | "IFCRATIONALBSPLINESURFACEWITHKNOTS" => {
                self.bspline_surface_item(name, record)
            }
            "IFCPOLYLINE"
            | "IFCBSPLINECURVEWITHKNOTS"
            | "IFCRATIONALBSPLINECURVEWITHKNOTS"
            | "IFCOFFSETCURVE2D"
            | "IFCOFFSETCURVE3D"
            | "IFCOFFSETCURVEBYDISTANCES"
            | "IFCINDEXEDPOLYCURVE"
            | "IFCCIRCLE"
            | "IFCELLIPSE"
            | "IFCTRIMMEDCURVE"
            | "IFCCOMPOSITECURVE" => self.curve(name, record),
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

    /// The identifier and type of a representation, shared by all of its items. Each is read and
    /// charged once per representation, however many products use it.
    fn representation_texts(&mut self, name: u64, rep: &Record) -> Result<TextPair> {
        if let Some(pair) = self.texts.get(&name) {
            return Ok(pair.clone());
        }
        let identifier = attr::text(
            self.file.attr(rep, known::REPRESENTATION_IDENTIFIER),
            "IFCSHAPEREPRESENTATION",
        );
        let identifier = self.settle(identifier, name)?.flatten();
        let identifier = shared_text(&mut self.budget, identifier)?;
        let rtype = attr::text(
            self.file.attr(rep, known::REPRESENTATION_TYPE),
            "IFCSHAPEREPRESENTATION",
        );
        let rtype = self.settle(rtype, name)?.flatten();
        let rtype = shared_text(&mut self.budget, rtype)?;
        let pair = (identifier, rtype);
        self.texts.insert(name, pair.clone());
        Ok(pair)
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
        let step = attr::distinct_references(
            &mut self.budget,
            file.attr(record, known::PRODUCT_REPRESENTATIONS),
            "IFCPRODUCTDEFINITIONSHAPE",
            usize::MAX,
        );
        let Some((representations, _)) = self.settle(step, shape)? else {
            return Ok(());
        };
        for representation in representations {
            self.budget.step()?;
            let Some(rep) = file
                .instance(representation)
                .and_then(|i| i.records.first())
            else {
                self.log.add(
                    reasons::TARGET_MISSING,
                    "IFCSHAPEREPRESENTATION",
                    Some(Source::Decimal(shape)),
                    1,
                );
                continue;
            };
            if rep.type_name != "IFCSHAPEREPRESENTATION" {
                self.log.add(
                    reasons::GEOMETRY_NOT_EVALUATED,
                    &rep.type_name,
                    Some(Source::Decimal(representation)),
                    1,
                );
                continue;
            }
            let (identifier, rtype) = self.representation_texts(representation, rep)?;
            let step = attr::distinct_references(
                &mut self.budget,
                file.attr(rep, known::REPRESENTATION_ITEMS),
                "IFCSHAPEREPRESENTATION",
                usize::MAX,
            );
            let Some((items, repeats)) = self.settle(step, representation)? else {
                continue;
            };
            self.log.add(
                reasons::DUPLICATE_PLACEMENT,
                &rep.type_name,
                Some(Source::Decimal(representation)),
                repeats,
            );
            for item in items {
                self.budget.step()?;
                let Some(pieces) = self.item(item, 0)? else {
                    continue;
                };
                for piece in pieces.iter() {
                    self.budget.charge(item_bytes())?;
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

/// Bytes charged for one emitted item.
fn item_bytes() -> u64 {
    u64::try_from(std::mem::size_of::<GeometryItem>()).unwrap_or(u64::MAX)
}

/// A text of a representation, shared by its items: its bytes are charged once, before the copy.
fn shared_text(budget: &mut Budget, text: Option<String>) -> Result<Option<Arc<str>>> {
    let Some(text) = text else {
        return Ok(None);
    };
    budget.charge(u64::try_from(text.len()).unwrap_or(u64::MAX))?;
    Ok(Some(Arc::from(text)))
}

/// The products of the file: instance name, placement reference and product definition shape.
fn products(file: &IfcFile) -> Vec<(u64, Option<u64>, u64)> {
    let mut out = Vec::new();
    for instance in file.instances() {
        for record in &instance.records {
            let Some(Parameter::Reference(shape)) =
                file.attr(record, known::PRODUCT_REPRESENTATION)
            else {
                continue;
            };
            if attr::typed(file, *shape, "IFCPRODUCTDEFINITIONSHAPE").is_none() {
                continue;
            }
            let placement = match file.attr(record, known::PRODUCT_PLACEMENT) {
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
fn run(
    file: &IfcFile,
    limits: &Limits,
    tol: &Tolerance,
    cancel: &Cancel,
    options: &GeometryOptions,
) -> Result<Geometry> {
    cancel.check()?;
    let mut evaluator = Evaluator::new(file, limits, tol, cancel);
    let units = evaluator.read_units()?;
    if !options.subtract_openings {
        count_openings(&mut evaluator);
    }
    let all = products(file);
    let voids = if options.subtract_openings {
        opening::voids(file)
    } else {
        BTreeMap::new()
    };
    let index: BTreeMap<u64, (Option<u64>, u64)> =
        all.iter().map(|&(n, p, s)| (n, (p, s))).collect();
    let mut items = Vec::new();
    for (product, placement, shape) in all {
        evaluator.budget.check()?;
        let Some(world) = evaluator.product_transform(product, placement)? else {
            continue;
        };
        let start = items.len();
        evaluator.product_items(product, shape, world, &mut items)?;
        if let Some(openings) = voids.get(&product) {
            let mut found: Vec<(u64, Option<u64>, u64)> = Vec::with_capacity(openings.len());
            for opening in openings {
                match index.get(opening) {
                    Some(&(p, s)) => found.push((*opening, p, s)),
                    // An opening without a body cannot be subtracted.
                    None => evaluator.log.add(
                        reasons::OPENING_NOT_SUBTRACTED,
                        "IFCRELVOIDSELEMENT",
                        Some(Source::Decimal(*opening)),
                        1,
                    ),
                }
            }
            let openings = found;
            if let Some(slice) = items.get_mut(start..) {
                evaluator.subtract_openings(product, world, &openings, slice)?;
            }
        }
    }
    let report = evaluator.log.finish(LossHeader::new(
        "ifc",
        file.schema.identifier(),
        Direction::Import,
        Some(*limits),
    ));
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
        run(self, limits, tolerance, cancel, &GeometryOptions::default())
    }

    /// Evaluates the geometry of the products of the file as
    /// [`Self::geometry_with_tolerance`] does, with the given options.
    ///
    /// # Errors
    ///
    /// As [`Self::geometry_with_tolerance`].
    pub fn geometry_with_options(
        &self,
        limits: &Limits,
        cancel: &Cancel,
        tolerance: &Tolerance,
        options: &GeometryOptions,
    ) -> Result<Geometry> {
        run(self, limits, tolerance, cancel, options)
    }
}
