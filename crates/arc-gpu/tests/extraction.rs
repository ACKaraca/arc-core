//! CORE-GPU-003 incremental extraction: results apply only when newer, deletions remove items,
//! failures keep the previous items, and superseded work is cancelled.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "tests fail by panicking; small fingerprints as coordinates; pixel arithmetic in the shared support module"
)]

mod support;

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use arc_base::id::Identifier;
use arc_base::math::Point3;
use arc_base::{Color, Error, ObjectId};
use arc_doc::events::Changes;
use arc_doc::{
    FieldDecl, Header, Object, Schema, SchemaVersion, Snapshot, TypeDecl, TypeKey, TypeRegistry,
    Value, ValueType, WorkingSet,
};
use arc_gpu::{
    Extraction, ExtractionDriver, ExtractionResult, ExtractorRegistry, Gpu, LineItem, LinePath,
    LineStyle, LineWidth, MeshInstance, MeshItem, Outcome, RenderItem, Scene, SceneLimits,
};
use proptest::prelude::*;

const WAIT: Duration = Duration::from_secs(60);

fn shared_gpu() -> &'static Gpu {
    static GPU: OnceLock<Gpu> = OnceLock::new();
    GPU.get_or_init(support::gpu)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n + 1)
}

fn dot(x: f64) -> RenderItem {
    RenderItem::Lines(LineItem {
        origin: Point3::new(x, 0.0, 0.0),
        paths: vec![LinePath {
            points: vec![[0.0; 3]],
            closed: false,
        }],
        style: LineStyle::round(LineWidth::Screen(2.0)),
        color: Color::WHITE,
    })
}

/// The x of the object's only item, which the tests set to an integer fingerprint of the
/// result; rounded, since the bounding sphere of a mesh is centered within rounding error.
fn shown(scene: &Scene, object: ObjectId) -> Option<f64> {
    scene
        .bounding_sphere(Some(&[object]))
        .map(|(c, _)| c.x.round())
}

// --- Model-based test of applying results out of order -------------------------------------

#[derive(Debug, Clone, Copy)]
enum Kind {
    Line,
    Mesh,
    Removed,
    Failed,
    Invalid,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        Just(Kind::Line),
        Just(Kind::Mesh),
        Just(Kind::Removed),
        Just(Kind::Failed),
        Just(Kind::Invalid),
    ]
}

fn make(object: u128, revision: u64, sequence: u64, kind: Kind) -> ExtractionResult {
    let x = (revision * 1000 + sequence) as f64;
    let outcome = match kind {
        Kind::Line => Outcome::Items(Extraction {
            geometries: Vec::new(),
            lods: Vec::new(),
            items: vec![dot(x)],
        }),
        Kind::Mesh => {
            let mut extraction = Extraction::default();
            let geometry = extraction.add_geometry(support::square(), Point3::origin());
            extraction.items.push(RenderItem::Mesh(MeshItem {
                geometry,
                origin: Point3::new(x, 0.0, 0.0),
                instances: vec![MeshInstance::new(Color::WHITE)],
            }));
            Outcome::Items(extraction)
        }
        Kind::Removed => Outcome::Removed,
        Kind::Failed => Outcome::Failed(Error::new(arc_gpu::error::codes::ITEM_INVALID)),
        Kind::Invalid => Outcome::Items(Extraction {
            geometries: Vec::new(),
            lods: Vec::new(),
            items: vec![dot(f64::NAN)],
        }),
    };
    ExtractionResult {
        object: id(object),
        revision,
        sequence,
        outcome,
    }
}

/// Each case creates a scene on the device, so the default run uses fewer cases than the usual
/// 256; `PROPTEST_CASES` overrides it.
fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    #[test]
    fn results_apply_only_when_newer_in_any_completion_order(
        results in prop::collection::vec((0u128..4, 0u64..6, kind()), 1..24),
        order in prop::collection::vec(any::<prop::sample::Index>(), 24),
        batch in 1usize..6,
    ) {
        let mut scene = Scene::new(shared_gpu(), SceneLimits::DEFAULT).unwrap();
        // Results are produced in submission order (the sequence) and complete in any order.
        let mut pending: Vec<_> = results
            .iter()
            .enumerate()
            .map(|(seq, &(o, r, k))| (o, r, seq as u64, k))
            .collect();
        let mut completion = Vec::new();
        for pick in &order {
            if pending.is_empty() {
                break;
            }
            completion.push(pending.remove(pick.index(pending.len())));
        }
        completion.append(&mut pending);
        // The model: per object, the greatest stamp among results the scene accepts.
        let mut model: BTreeMap<u128, ((u64, u64), Kind)> = BTreeMap::new();
        for chunk in completion.chunks(batch) {
            let report = scene.apply_extractions(
                chunk.iter().map(|&(o, r, s, k)| make(o, r, s, k)).collect(),
            );
            prop_assert_eq!(report.applied + report.stale + report.failed.len(), chunk.len());
            for &(o, r, s, k) in chunk {
                if matches!(k, Kind::Line | Kind::Mesh | Kind::Removed)
                    && model.get(&o).is_none_or(|(best, _)| *best < (r, s))
                {
                    model.insert(o, ((r, s), k));
                }
            }
            for o in 0..4 {
                let expected = match model.get(&o) {
                    Some(((r, s), Kind::Line | Kind::Mesh)) => Some((r * 1000 + s) as f64),
                    _ => None,
                };
                prop_assert_eq!(shown(&scene, id(o)), expected);
                prop_assert_eq!(scene.applied_stamp(&id(o)), model.get(&o).map(|m| m.0));
            }
        }
        // Removing everything frees every geometry the extractions uploaded.
        scene.apply_extractions((0..4).map(|o| make(o, 100, 0, Kind::Removed)).collect());
        prop_assert!(scene.keys().is_empty());
        prop_assert_eq!(scene.memory().vertex.used_bytes, 0);
    }
}

// --- The driver on document snapshots ------------------------------------------------------

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("x", ValueType::I64)],
    );
    registry
        .register(TypeDecl::new(TypeKey::new("test.dot").unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn object(n: u128, x: i64) -> Object {
    Object::new(
        id(n),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![Some(Value::I64(x))],
    )
}

fn value(object: &Object) -> i64 {
    match object.values.first() {
        Some(Some(Value::I64(x))) => *x,
        _ => panic!("an integer"),
    }
}

/// A dot at the object's value; negative values fail, 13 panics.
fn dots() -> ExtractorRegistry {
    let mut extractors = ExtractorRegistry::new();
    extractors.register(TypeKey::new("test.dot").unwrap(), |_, object, _| {
        let x = value(object);
        if x == 13 {
            panic!("unlucky");
        }
        if x < 0 {
            return Err(Error::new(arc_gpu::error::codes::ITEM_INVALID));
        }
        Ok(Extraction {
            geometries: Vec::new(),
            lods: Vec::new(),
            items: vec![dot(x as f64)],
        })
    });
    extractors
}

/// Sets object `n` to `x` (creating it) or deletes it (`None`) on top of `base`.
fn edit(base: &Snapshot, changes: &[(u128, Option<i64>)]) -> Snapshot {
    let mut set = WorkingSet::new(base);
    for &(n, x) in changes {
        let handle = set.current().resolve(id(n)).ok();
        match (handle, x) {
            (Some(h), Some(x)) => drop(set.replace_values(h, vec![Some(Value::I64(x))]).unwrap()),
            (Some(h), None) => drop(set.delete(h).unwrap()),
            (None, Some(x)) => drop(set.create("test.dot", object(n, x)).unwrap()),
            (None, None) => {}
        }
    }
    set.current().clone()
}

fn run(driver: &ExtractionDriver, scene: &mut Scene, snapshot: &Snapshot, objects: &[u128]) {
    driver.submit(snapshot, objects.iter().map(|&n| id(n)));
    assert!(driver.wait_idle(WAIT));
    scene.apply_extractions(driver.take_results());
}

#[test]
fn the_driver_extracts_changes_removes_deletions_and_keeps_items_on_failure() {
    let driver = ExtractionDriver::new(dots(), 2).unwrap();
    let mut scene = Scene::new(shared_gpu(), SceneLimits::DEFAULT).unwrap();
    let r1 = edit(
        &Snapshot::empty(registry()),
        &[(0, Some(1)), (1, Some(2)), (2, Some(3))],
    );
    run(&driver, &mut scene, &r1, &[0, 1, 2]);
    assert_eq!(shown(&scene, id(0)), Some(1.0));
    assert_eq!(
        scene.applied_stamp(&id(2)).map(|s| s.0),
        Some(r1.revision())
    );

    // An erroring and a panicking extractor keep the previous items; a deletion removes them.
    let r2 = edit(&r1, &[(0, Some(-1)), (1, Some(13)), (2, None)]);
    driver.submit(&r2, [id(0), id(1), id(2)]);
    assert!(driver.wait_idle(WAIT));
    let report = scene.apply_extractions(driver.take_results());
    let mut codes: Vec<_> = report.failed.iter().map(|(_, e)| e.code()).collect();
    codes.sort_unstable();
    assert_eq!(codes, ["gpu.extractor.panicked", "gpu.item.invalid"]);
    assert_eq!(report.applied, 1);
    assert_eq!(shown(&scene, id(0)), Some(1.0));
    assert_eq!(shown(&scene, id(1)), Some(2.0));
    assert_eq!(shown(&scene, id(2)), None);

    // "Everything changed" re-extracts the snapshot and removes objects no longer in it.
    let r3 = edit(&r2, &[(0, Some(7)), (1, None), (3, Some(9))]);
    driver.submit_changes(&r3, &Changes::All);
    assert!(driver.wait_idle(WAIT));
    scene.apply_extractions(driver.take_results());
    assert_eq!(shown(&scene, id(0)), Some(7.0));
    assert_eq!(shown(&scene, id(1)), None);
    assert_eq!(shown(&scene, id(3)), Some(9.0));

    // A late result of an older revision is dropped.
    driver.submit(&r1, [id(0)]);
    assert!(driver.wait_idle(WAIT));
    let report = scene.apply_extractions(driver.take_results());
    assert_eq!(report.stale, 1);
    assert_eq!(shown(&scene, id(0)), Some(7.0));
}

#[test]
fn superseded_running_work_is_cancelled_and_never_delivered() {
    // The extractor of value 1 waits until released; it reports whether it saw cancellation.
    let (started, on_start) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    let (saw_cancel, on_cancel) = mpsc::channel::<bool>();
    let gate = Mutex::new((started, released, saw_cancel));
    let mut extractors = ExtractorRegistry::new();
    extractors.register(
        TypeKey::new("test.dot").unwrap(),
        move |_, object, cancel| {
            let x = value(object);
            if x == 1 {
                let gate = gate.lock().unwrap();
                gate.0.send(()).unwrap();
                gate.1.recv_timeout(WAIT).unwrap();
                gate.2.send(cancel.is_cancelled()).unwrap();
            }
            Ok(Extraction {
                geometries: Vec::new(),
                lods: Vec::new(),
                items: vec![dot(x as f64)],
            })
        },
    );
    let driver = ExtractionDriver::new(extractors, 2).unwrap();
    let r1 = edit(&Snapshot::empty(registry()), &[(0, Some(1))]);
    let r2 = edit(&r1, &[(0, Some(2))]);
    driver.submit(&r1, [id(0)]);
    on_start.recv_timeout(WAIT).unwrap();
    driver.submit(&r2, [id(0)]);
    release.send(()).unwrap();
    assert!(on_cancel.recv_timeout(WAIT).unwrap());
    assert!(driver.wait_idle(WAIT));
    let results = driver.take_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].revision, r2.revision());
    // Queued jobs of one object coalesce into the newest.
    let r3 = edit(&r2, &[(0, Some(3))]);
    let r4 = edit(&r3, &[(0, Some(4))]);
    driver.submit(&r4, [id(0)]);
    driver.submit(&r3, [id(0)]);
    assert!(driver.wait_idle(WAIT));
    let mut scene = Scene::new(shared_gpu(), SceneLimits::DEFAULT).unwrap();
    scene.apply_extractions(driver.take_results());
    assert_eq!(shown(&scene, id(0)), Some(4.0));
}
