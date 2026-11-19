//! Incremental extraction (CORE-GPU-003): product extractors from document objects to render
//! items, run off the caller's thread, applied to a [`Scene`] only when newer than what it
//! holds.
//!
//! - An [`ExtractorRegistry`] maps an object type key to an [`Extractor`]: a function from the
//!   snapshot and one object of it to an [`Extraction`], the object's complete list of render
//!   items plus the geometries those items use. An object whose type has no extractor has no
//!   items.
//! - An [`ExtractionDriver`] owns a fixed pool of worker threads. [`ExtractionDriver::submit`]
//!   queues one job per named object against a snapshot; each submission gets a sequence number
//!   from the driver, and the job's **stamp** is `(snapshot revision, sequence)`. A newer
//!   submission for an object replaces its queued job and cancels its running one; a cancelled
//!   job's result is never delivered. A job resolves its object in the snapshot: an object that
//!   is not there (deleted) yields [`Outcome::Removed`]; an extractor error yields
//!   [`Outcome::Failed`] with the extractor's error; a panic yields `gpu.extractor.panicked`.
//! - [`Scene::apply_extractions`] applies results in any order: a result changes the scene only
//!   when its stamp is greater than the stamp last applied for its object, so a late result of
//!   an older revision never replaces a newer one. A removal keeps its stamp, so an older result
//!   cannot bring a deleted object back. A failed result, or one whose items or geometries are
//!   rejected, leaves the object's previous items and stamp in place.
//!
//! The objects whose items a scene receives from extraction should not also be changed with
//! [`Scene::update`] directly: the stamp would no longer describe the items.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arc_base::math::Point3;
use arc_base::{Cancel, Error, ObjectId, Result};
use arc_doc::events::Changes;
use arc_doc::{Object, Snapshot, TypeKey};
use arc_mesh::TriMesh;
use arc_mesh::lod::LodChain;

use crate::error::codes;
use crate::scene::{GeometryId, RenderItem, Scene};
use crate::tile::TiledGeometry;

/// The most worker threads an [`ExtractionDriver`] starts.
pub const MAX_WORKERS: usize = 16;

/// A product extractor: the render items of one object of a snapshot. It may run on any worker
/// thread and concurrently with itself; long extractors should return early once the token is
/// cancelled.
pub type Extractor =
    Arc<dyn Fn(&Snapshot, &Object, &Cancel) -> Result<Extraction> + Send + Sync + 'static>;

/// Extractors by object type key.
#[derive(Clone, Default)]
pub struct ExtractorRegistry {
    extractors: HashMap<TypeKey, Extractor>,
}

impl fmt::Debug for ExtractorRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.extractors.keys()).finish()
    }
}

impl ExtractorRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the extractor of objects of type `key`, replacing and returning a previous one.
    pub fn register(
        &mut self,
        key: TypeKey,
        extractor: impl Fn(&Snapshot, &Object, &Cancel) -> Result<Extraction> + Send + Sync + 'static,
    ) -> Option<Extractor> {
        self.extractors.insert(key, Arc::new(extractor))
    }
}

/// What an extractor returns for one object: its complete list of render items, and the
/// geometries those items use that the scene does not hold yet.
///
/// Geometries added with [`Extraction::add_geometry`] are uploaded when the extraction is
/// applied and belong to the object: they are removed when the object's items are next replaced
/// or removed. Items may also refer to geometries the scene already holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extraction {
    /// The geometries to upload, with the origin their positions are relative to.
    pub geometries: Vec<(TriMesh, Point3)>,
    /// The items of the object, in sub-item order.
    pub items: Vec<RenderItem>,
    /// Groups of `geometries` that are the levels of one geometry (CORE-GPU-019).
    pub lods: Vec<LodGroup>,
}

/// The levels of detail of one geometry in an [`Extraction`]: the geometries from `first` on,
/// one per level of `chain`, level 0 first. Items use the handle of level 0.
#[derive(Debug, Clone, PartialEq)]
pub struct LodGroup {
    /// The index in [`Extraction::geometries`] of level 0.
    pub first: usize,
    /// The chain; the scene keeps it to choose levels.
    pub chain: LodChain,
}

impl Extraction {
    /// Adds a geometry to upload and returns the handle a [`crate::MeshItem`] of this
    /// extraction uses for it. The handle is valid only in this extraction.
    pub fn add_geometry(&mut self, mesh: TriMesh, origin: Point3) -> GeometryId {
        let local = self.geometries.len() as u64;
        self.geometries.push((mesh, origin));
        GeometryId::from_raw(GeometryId::LOCAL | local)
    }

    /// Adds the levels of `chain` as one geometry with levels of detail and returns the handle
    /// of level 0 for a [`crate::MeshItem`] of this extraction (see [`Scene::add_lod_geometry`]).
    pub fn add_lod_geometry(&mut self, chain: &LodChain, origin: Point3) -> GeometryId {
        let first = self.geometries.len();
        for level in chain.levels() {
            self.geometries.push((level.mesh.clone(), origin));
        }
        self.lods.push(LodGroup {
            first,
            chain: chain.clone(),
        });
        GeometryId::from_raw(GeometryId::LOCAL | first as u64)
    }

    /// Adds `mesh` as tiles of at most [`crate::tile::TILE_SIZE`] (see
    /// [`crate::tile::split_mesh`]); the handles are valid only in this extraction.
    ///
    /// # Errors
    ///
    /// As [`crate::tile::split_mesh`].
    pub fn add_tiled_geometry(&mut self, mesh: &TriMesh) -> Result<Vec<TiledGeometry>> {
        let tiles = crate::tile::split_mesh(mesh, crate::tile::TILE_SIZE)?;
        Ok(tiles
            .into_iter()
            .map(|tile| TiledGeometry {
                geometry: self.add_geometry(tile.mesh, tile.origin),
                origin: tile.origin,
            })
            .collect())
    }
}

/// The outcome of extracting one object.
#[derive(Debug)]
pub enum Outcome {
    /// The object's new items.
    Items(Extraction),
    /// The object is not in the snapshot (deleted) or its type has no extractor: it has no
    /// items.
    Removed,
    /// The extractor failed; the object keeps its previous items.
    Failed(Error),
}

/// The result of extracting one object at one stamp.
#[derive(Debug)]
pub struct ExtractionResult {
    /// The object.
    pub object: ObjectId,
    /// The revision of the snapshot the extractor read.
    pub revision: u64,
    /// The submission sequence number, which orders results of one revision.
    pub sequence: u64,
    /// What the extractor produced.
    pub outcome: Outcome,
}

impl ExtractionResult {
    /// `(revision, sequence)`: results with greater stamps are newer.
    #[must_use]
    pub fn stamp(&self) -> (u64, u64) {
        (self.revision, self.sequence)
    }

    /// Whether the outcome is [`Outcome::Failed`].
    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(self.outcome, Outcome::Failed(_))
    }
}

/// What [`Scene::apply_extractions`] did.
#[derive(Debug, Default)]
pub struct ApplyReport {
    /// Results that changed the scene.
    pub applied: usize,
    /// Results that were not newer than the applied stamp of their object and were dropped.
    pub stale: usize,
    /// Failed results and results the scene rejected, with the error; those objects kept their
    /// previous items.
    pub failed: Vec<(ObjectId, Error)>,
}

/// The applied stamp and owned geometries of each extracted object.
#[derive(Debug, Default)]
pub(crate) struct Applied(HashMap<ObjectId, Entry>);

#[derive(Debug)]
struct Entry {
    stamp: (u64, u64),
    geometries: Vec<GeometryId>,
}

impl Scene {
    /// Applies extraction results in the given order. A result whose stamp is not greater than
    /// the applied stamp of its object is dropped; a removal empties the object; new items
    /// replace the object's items after their geometries are uploaded, and the geometries the
    /// object owned before are then removed. A rejected result leaves the object as it was.
    pub fn apply_extractions(&mut self, results: Vec<ExtractionResult>) -> ApplyReport {
        let mut report = ApplyReport::default();
        for result in results {
            let object = result.object;
            match self.apply_one(result) {
                Ok(true) => report.applied += 1,
                Ok(false) => report.stale += 1,
                Err(error) => report.failed.push((object, error)),
            }
        }
        report
    }

    /// The stamp of the last extraction result applied for `object`, including removals.
    #[must_use]
    pub fn applied_stamp(&self, object: &ObjectId) -> Option<(u64, u64)> {
        self.extracted.0.get(object).map(|e| e.stamp)
    }

    fn apply_one(&mut self, result: ExtractionResult) -> Result<bool> {
        let stamp = result.stamp();
        let object = result.object;
        let extraction = match result.outcome {
            Outcome::Failed(error) => return Err(error),
            _ if self.applied_stamp(&object).is_some_and(|s| s >= stamp) => return Ok(false),
            Outcome::Removed => Extraction::default(),
            Outcome::Items(extraction) => extraction,
        };
        let limit = usize::try_from(self.limits().max_items_per_object).unwrap_or(usize::MAX);
        if extraction.geometries.len() > limit {
            return Err(Error::new(codes::LIMIT_EXCEEDED));
        }
        let mut added = Vec::with_capacity(extraction.geometries.len());
        for (mesh, origin) in &extraction.geometries {
            match self.add_geometry(mesh, origin) {
                Ok(id) => added.push(id),
                Err(error) => {
                    self.release(&added);
                    return Err(error);
                }
            }
        }
        for group in &extraction.lods {
            let end = group.first + group.chain.levels().len();
            let registered = added
                .get(group.first..end)
                .ok_or_else(|| Error::new(codes::GEOMETRY_NOT_FOUND))
                .and_then(|levels| self.register_lod(levels.to_vec(), &group.chain));
            if let Err(error) = registered {
                self.release(&added);
                return Err(error);
            }
        }
        let items = extraction
            .items
            .into_iter()
            .map(|item| local_to_scene(item, &added))
            .collect::<Result<Vec<_>>>()
            .and_then(|items| self.update(vec![(object, items)]));
        if let Err(error) = items {
            self.release(&added);
            return Err(error);
        }
        let previous = self.extracted.0.insert(
            object,
            Entry {
                stamp,
                geometries: added,
            },
        );
        if let Some(previous) = previous {
            self.release(&previous.geometries);
        }
        Ok(true)
    }

    fn release(&mut self, geometries: &[GeometryId]) {
        for id in geometries {
            // Still in use only when a caller refers to it directly; it then stays.
            let _ = self.remove_geometry(*id);
        }
    }
}

fn local_to_scene(item: RenderItem, added: &[GeometryId]) -> Result<RenderItem> {
    match item {
        RenderItem::Mesh(mut mesh) if mesh.geometry.raw() & GeometryId::LOCAL != 0 => {
            let index = usize::try_from(mesh.geometry.raw() & !GeometryId::LOCAL)
                .map_err(|_| Error::new(codes::GEOMETRY_NOT_FOUND))?;
            mesh.geometry = *added
                .get(index)
                .ok_or_else(|| Error::new(codes::GEOMETRY_NOT_FOUND))?;
            Ok(RenderItem::Mesh(mesh))
        }
        other => Ok(other),
    }
}

/// Runs extractors on a fixed pool of worker threads; see the [module documentation](self).
///
/// Dropping the driver cancels queued and running jobs and joins the workers; it waits for
/// running extractors to return.
pub struct ExtractionDriver {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl fmt::Debug for ExtractionDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtractionDriver")
            .field("workers", &self.workers.len())
            .finish_non_exhaustive()
    }
}

struct Shared {
    registry: ExtractorRegistry,
    state: Mutex<State>,
    work: Condvar,
    idle: Condvar,
}

struct Job {
    snapshot: Snapshot,
    sequence: u64,
    cancel: Cancel,
}

impl Job {
    fn stamp(&self) -> (u64, u64) {
        (self.snapshot.revision(), self.sequence)
    }
}

#[derive(Default)]
struct State {
    /// Objects with a queued job, in submission order; each at most once.
    order: VecDeque<ObjectId>,
    queued: HashMap<ObjectId, Job>,
    /// The stamp and token of the newest running job of each object.
    running: HashMap<ObjectId, ((u64, u64), Cancel)>,
    /// Running jobs, which may include superseded ones of the same object.
    busy: usize,
    /// Undelivered results: per object at most one success and one newer failure.
    results: HashMap<ObjectId, Vec<ExtractionResult>>,
    /// Objects whose last delivered outcome had items, for "everything changed" events.
    known: HashSet<ObjectId>,
    next_sequence: u64,
    shutdown: bool,
}

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ExtractionDriver {
    /// Starts a driver with `workers` threads, clamped to 1..=[`MAX_WORKERS`].
    ///
    /// # Errors
    ///
    /// `gpu.thread.failed` when a worker thread cannot be started.
    pub fn new(registry: ExtractorRegistry, workers: usize) -> Result<Self> {
        let shared = Arc::new(Shared {
            registry,
            state: Mutex::new(State::default()),
            work: Condvar::new(),
            idle: Condvar::new(),
        });
        let mut driver = Self {
            shared,
            workers: Vec::new(),
        };
        for n in 0..workers.clamp(1, MAX_WORKERS) {
            let shared = Arc::clone(&driver.shared);
            let handle = std::thread::Builder::new()
                .name(format!("arc-gpu extract {n}"))
                .spawn(move || work(&shared))
                .map_err(|e| Error::new(codes::THREAD_FAILED).with_source(e))?;
            driver.workers.push(handle);
        }
        Ok(driver)
    }

    /// Queues the extraction of `objects` from `snapshot`. Each object gets the next sequence
    /// number; a queued job of the same object with a smaller stamp is replaced and a running
    /// one is cancelled.
    pub fn submit(&self, snapshot: &Snapshot, objects: impl IntoIterator<Item = ObjectId>) {
        let mut state = lock(&self.shared);
        for object in objects {
            let sequence = state.next_sequence;
            state.next_sequence = sequence.saturating_add(1);
            let job = Job {
                snapshot: snapshot.clone(),
                sequence,
                cancel: Cancel::new(),
            };
            let stamp = job.stamp();
            if let Some((running, cancel)) = state.running.get(&object)
                && *running < stamp
            {
                cancel.cancel();
            }
            match state.queued.get_mut(&object) {
                Some(queued) if queued.stamp() < stamp => *queued = job,
                Some(_) => {}
                None => {
                    state.queued.insert(object, job);
                    state.order.push_back(object);
                }
            }
        }
        drop(state);
        self.shared.work.notify_all();
    }

    /// Queues the extraction of the objects a committed-event's `changes` list, from
    /// `snapshot` (the event's snapshot). For "everything changed" it queues every object of
    /// the snapshot and every object whose last delivered result had items, so deleted objects
    /// are removed.
    pub fn submit_changes(&self, snapshot: &Snapshot, changes: &Changes) {
        match changes {
            Changes::Listed(by_type) => self.submit(
                snapshot,
                by_type
                    .values()
                    .flat_map(|g| {
                        g.created
                            .iter()
                            .chain(&g.modified)
                            .chain(&g.deleted)
                            .chain(&g.recreated)
                    })
                    .copied()
                    .collect::<HashSet<_>>(),
            ),
            Changes::All => {
                let mut objects: HashSet<ObjectId> = lock(&self.shared).known.clone();
                objects.extend(snapshot.iter().map(|(_, object)| object.id));
                self.submit(snapshot, objects);
            }
        }
    }

    /// Takes the results delivered so far. Per object, a newer result replaces older ones,
    /// except that a failure never replaces an older success, which stays deliverable.
    pub fn take_results(&self) -> Vec<ExtractionResult> {
        let mut state = lock(&self.shared);
        std::mem::take(&mut state.results)
            .into_values()
            .flatten()
            .collect()
    }

    /// Waits until no job is queued or running, at most `timeout`; whether that happened.
    #[must_use]
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.shared);
        while !state.queued.is_empty() || state.busy > 0 {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            state = self
                .shared
                .idle
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Drop for ExtractionDriver {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.shared);
            state.shutdown = true;
            state.order.clear();
            state.queued.clear();
            for (_, cancel) in state.running.values() {
                cancel.cancel();
            }
        }
        self.shared.work.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn work(shared: &Shared) {
    loop {
        let (object, job) = {
            let mut state = lock(shared);
            loop {
                if state.shutdown {
                    return;
                }
                if let Some(object) = state.order.pop_front() {
                    if let Some(job) = state.queued.remove(&object) {
                        state
                            .running
                            .insert(object, (job.stamp(), job.cancel.clone()));
                        state.busy += 1;
                        break (object, job);
                    }
                    continue;
                }
                state = shared
                    .work
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let outcome = extract(&shared.registry, &job.snapshot, object, &job.cancel);
        let mut state = lock(shared);
        state.busy -= 1;
        if state
            .running
            .get(&object)
            .is_some_and(|(s, _)| *s == job.stamp())
        {
            state.running.remove(&object);
        }
        if !job.cancel.is_cancelled() {
            match outcome {
                Outcome::Items(_) => state.known.insert(object),
                Outcome::Removed => state.known.remove(&object),
                Outcome::Failed(_) => false,
            };
            deliver(
                state.results.entry(object).or_default(),
                ExtractionResult {
                    object,
                    revision: job.snapshot.revision(),
                    sequence: job.sequence,
                    outcome,
                },
            );
        }
        if state.queued.is_empty() && state.busy == 0 {
            shared.idle.notify_all();
        }
        drop(state);
        // The snapshot may be the last holder of a large document; free it outside the lock.
        drop(job);
    }
}

/// Adds `result` to the undelivered results of its object, dropping superseded ones.
fn deliver(results: &mut Vec<ExtractionResult>, result: ExtractionResult) {
    let (stamp, failed) = (result.stamp(), result.is_failed());
    if results
        .iter()
        .any(|o| o.stamp() > stamp && (failed || !o.is_failed()))
    {
        return;
    }
    results.retain(|o| o.stamp() > stamp || (failed && !o.is_failed()));
    results.push(result);
    results.sort_by_key(ExtractionResult::stamp);
}

fn extract(
    registry: &ExtractorRegistry,
    snapshot: &Snapshot,
    object: ObjectId,
    cancel: &Cancel,
) -> Outcome {
    let found = snapshot.resolve(object).ok().and_then(|handle| {
        let value = snapshot.get(handle).ok()?;
        let key = snapshot.type_key(handle)?;
        Some((value, key))
    });
    let Some((value, key)) = found else {
        return Outcome::Removed;
    };
    let Some(extractor) = registry.extractors.get(key) else {
        return Outcome::Removed;
    };
    match catch_unwind(AssertUnwindSafe(|| extractor(snapshot, value, cancel))) {
        Ok(Ok(extraction)) => Outcome::Items(extraction),
        Ok(Err(error)) => Outcome::Failed(error),
        Err(_) => Outcome::Failed(
            Error::new(codes::EXTRACTOR_PANICKED).with_detail("type", key.as_str().to_owned()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_base::id::Identifier;

    fn result(sequence: u64, failed: bool) -> ExtractionResult {
        ExtractionResult {
            object: ObjectId::from_u128(1),
            revision: 1,
            sequence,
            outcome: if failed {
                Outcome::Failed(Error::new(codes::ITEM_INVALID))
            } else {
                Outcome::Removed
            },
        }
    }

    fn stamps(results: &[ExtractionResult]) -> Vec<(u64, bool)> {
        results
            .iter()
            .map(|r| (r.sequence, r.is_failed()))
            .collect()
    }

    #[test]
    fn delivery_keeps_the_newest_success_and_a_newer_failure() {
        let mut held = Vec::new();
        deliver(&mut held, result(5, false));
        deliver(&mut held, result(4, false));
        deliver(&mut held, result(3, true));
        assert_eq!(stamps(&held), vec![(5, false)]);
        deliver(&mut held, result(6, true));
        deliver(&mut held, result(7, true));
        assert_eq!(stamps(&held), vec![(5, false), (7, true)]);
        deliver(&mut held, result(8, false));
        assert_eq!(stamps(&held), vec![(8, false)]);
    }
}
