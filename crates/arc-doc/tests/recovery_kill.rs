//! Crash recovery (CORE-DOC-041, CORE-DOC-042, roadmap phase 0 exit criterion 2): child
//! processes edit a document under a recovery journal, committing transactions at a steady rate
//! and appending the wall-clock time of each acknowledged commit to a side file; the parent kills
//! each at a random moment and recovers its journal. Every transaction acknowledged at least two
//! seconds before the kill (the one-second rule with a generous margin for slow runners) must be
//! recovered, and the recovered state must be the state after a whole number of transactions,
//! never a partial one: each transaction sets two objects to the same value.
//!
//! The children are this test executable, re-run with `ARC_DOC_RECOVERY_ROOT` set so that the
//! `recovery_child` entry runs the editing session. The children of one round run at the same
//! time. `kill_during_editing_300` is the long variant:
//! `cargo nextest run -p arc-doc --all-features --run-ignored only kill_during_editing_300`.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;
mod recovery_support;

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arc_base::id::Identifier;
use arc_base::{Limits, SessionId};
use arc_doc::graph::Evaluators;
use arc_doc::package::recovery::{self, RecoverOptions};
use package_support::TempDir;
use recovery_support::{State, open, protect, registry, state, write_base};

const ROOT: &str = "ARC_DOC_RECOVERY_ROOT";
const BASE: &str = "ARC_DOC_RECOVERY_BASE";
const ACKS: &str = "ARC_DOC_RECOVERY_ACKS";
/// The pause between two commits of a child.
const PERIOD: Duration = Duration::from_millis(10);
/// How long before the kill a transaction must have been acknowledged to be required.
const MARGIN_MS: u64 = 2_000;

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn initial() -> State {
    (1..=4).map(|i| (i, 0)).collect()
}

/// The state after `k` transactions: transaction `i` sets objects `a` and `b` to `i`.
fn after(k: u64) -> State {
    let mut state = initial();
    for i in 1..=k {
        let (a, b) = pair(i);
        state.insert(a, i64::try_from(i).unwrap());
        state.insert(b, i64::try_from(i).unwrap());
    }
    state
}

fn pair(i: u64) -> (u128, u128) {
    let a = u128::from(i % 4) + 1;
    (a, a % 4 + 1)
}

/// The child entry: edits until it is killed when started by the harness.
#[test]
fn recovery_child() {
    let (Ok(root), Ok(base), Ok(acks)) = (
        std::env::var(ROOT),
        std::env::var(BASE),
        std::env::var(ACKS),
    ) else {
        return;
    };
    let (document, fingerprint) = open(Path::new(&base));
    let _journal = protect(&document, Path::new(&root), 1, Some(fingerprint));
    let mut acks = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(acks)
        .unwrap();
    for i in 1..=30_000_u64 {
        let (a, b) = pair(i);
        let v = i64::try_from(i).unwrap();
        document
            .submit(move |txn| {
                for id in [a, b] {
                    txn.modify(
                        arc_base::ObjectId::from_u128(id),
                        arc_doc::FieldPath::new("v"),
                        Some(arc_doc::Value::I64(v)),
                    )?;
                }
                Ok(())
            })
            .wait()
            .unwrap();
        writeln!(acks, "{i} {}", now_ms()).unwrap();
        std::thread::sleep(PERIOD);
    }
}

struct Run {
    dir: TempDir,
    child: Child,
    delay: Duration,
}

/// A small deterministic generator for the kill delays.
fn next(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

fn spawn(n: u64, seed: &mut u64) -> Run {
    let dir = TempDir::new(&format!("rec-kill-{n}"));
    let base = dir.path("model.arc");
    write_base(&base, &initial(), 1);
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "recovery_child",
            "--exact",
            "--test-threads",
            "1",
            "--quiet",
        ])
        .env(ROOT, dir.path("app"))
        .env(BASE, &base)
        .env(ACKS, dir.path("acks.txt"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Between 0.5 and 4 seconds of editing.
    let delay = Duration::from_millis(500 + next(seed) % 3_500);
    Run { dir, child, delay }
}

/// Checks one killed run; returns the number of required transactions.
fn check(run: &Run, killed_at: u64) -> u64 {
    let acks = std::fs::read_to_string(run.dir.path("acks.txt")).unwrap_or_default();
    let mut acknowledged = 0;
    let mut required = 0;
    for line in acks.lines() {
        // The last line may be cut by the kill.
        let Some((i, time)) = line.split_once(' ') else {
            continue;
        };
        let (Ok(i), Ok(time)) = (i.parse::<u64>(), time.parse::<u64>()) else {
            continue;
        };
        acknowledged = acknowledged.max(i);
        if time + MARGIN_MS <= killed_at {
            required = required.max(i);
        }
    }
    let root = run.dir.path("app");
    let orphans = recovery::scan(&root, &Limits::DEFAULT).unwrap();
    if orphans.is_empty() {
        // Killed before the journal existed.
        assert_eq!(required, 0);
        return 0;
    }
    assert_eq!(orphans.len(), 1);
    let options = RecoverOptions::new(&root, SessionId::from_u128(2));
    let recovered = recovery::recover(
        &orphans[0],
        registry(),
        Arc::new(Evaluators::new()),
        options,
    )
    .unwrap();
    assert!(recovered.stopped.is_none(), "{:?}", recovered.stopped);
    let k = recovered.replayed;
    assert!(
        k >= required && k <= acknowledged + 1,
        "recovered {k}, required {required}, acknowledged {acknowledged}"
    );
    assert_eq!(state(&recovered.document.snapshot()), after(k));
    required
}

fn kill_during_editing(iterations: u64, parallel: u64) {
    let mut seed = 0x0dd_5eed;
    let mut required_total = 0;
    let mut n = 0;
    while n < iterations {
        let round: Vec<Run> = (n..iterations.min(n + parallel))
            .map(|i| spawn(i, &mut seed))
            .collect();
        n += u64::try_from(round.len()).unwrap();
        let started = Instant::now();
        let mut order: Vec<usize> = (0..round.len()).collect();
        order.sort_by_key(|&i| round[i].delay);
        let mut runs: Vec<Option<Run>> = round.into_iter().map(Some).collect();
        let mut killed = vec![0; runs.len()];
        for i in order {
            let run = runs[i].as_mut().unwrap();
            if let Some(wait) = run.delay.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
            killed[i] = now_ms();
            let _ = run.child.kill();
            run.child.wait().unwrap();
        }
        for (i, run) in runs.iter().enumerate() {
            required_total += check(run.as_ref().unwrap(), killed[i]);
        }
    }
    // The harness is not blind: some runs had transactions to require.
    assert!(required_total > 0);
}

#[test]
fn kill_during_editing_30() {
    kill_during_editing(30, 30);
}

#[test]
#[ignore = "300 killed sessions; takes about a minute"]
fn kill_during_editing_300() {
    kill_during_editing(300, 30);
}
