//! Edit locks (CORE-DOC-046): opening for editing takes the sidecar lock; a second opener gets a
//! read-only package naming the holder; closing releases the lock; a stale lock of this host is
//! taken over only through `take_over`; a lock of another host never; read-only opens create
//! nothing.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use arc_base::Limits;
use arc_doc::error::codes;
use arc_doc::package::{Access, LockHolder, Metadata, Mode, Package, ReadOnlyCause, lock_path};
use package_support::{TempDir, owner};

fn holder_json(host: &str, pid: u32) -> String {
    format!(
        "{{\"format\":1,\"user\":\"ghost\",\"host\":\"{host}\",\"pid\":{pid},\
         \"application\":\"old app 0\",\"time_unix_ms\":5}}"
    )
}

fn ghost(host: &str, pid: u32) -> LockHolder {
    LockHolder {
        user: "ghost".into(),
        host: host.into(),
        pid,
        application: "old app 0".into(),
        time_unix_ms: 5,
    }
}

fn new_package(dir: &TempDir) -> std::path::PathBuf {
    let path = dir.path("model.arc");
    drop(Package::create(&path, &Metadata::default(), &owner("ann", "here")).unwrap());
    path
}

#[test]
fn the_second_opener_is_read_only_and_sees_the_holder() {
    let dir = TempDir::new("lock-live");
    let path = new_package(&dir);
    let first = Package::open(&path, Mode::Edit(&owner("ann", "here")), &Limits::DEFAULT).unwrap();
    assert!(first.is_editable());
    let lock = lock_path(&path);
    assert_eq!(lock.file_name().unwrap(), "~model.arc.lock");
    let content: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock).unwrap()).unwrap();
    assert_eq!(content["user"], "ann");
    assert_eq!(content["host"], "here");
    assert_eq!(content["pid"], std::process::id());
    assert_eq!(content["format"], 1);

    let second = Package::open(&path, Mode::Edit(&owner("bob", "here")), &Limits::DEFAULT).unwrap();
    let Access::ReadOnly(ReadOnlyCause::Locked(Some(holder))) = second.access() else {
        panic!("{:?}", second.access());
    };
    assert_eq!(holder.user, "ann");
    assert_eq!(holder.pid, std::process::id());
    // A live lock cannot be taken over.
    let mut second = second;
    assert_eq!(
        second.take_over(&owner("bob", "here")).unwrap_err().code(),
        codes::PACKAGE_LOCKED.code
    );

    drop(first);
    assert!(!lock.exists());
    let third = Package::open(&path, Mode::Edit(&owner("bob", "here")), &Limits::DEFAULT).unwrap();
    assert!(third.is_editable());
}

#[test]
fn a_stale_lock_of_this_host_is_taken_over_only_on_request() {
    let dir = TempDir::new("lock-stale");
    let path = new_package(&dir);
    let lock = lock_path(&path);
    std::fs::write(&lock, holder_json("here", 4_000_000)).unwrap();
    let mut package =
        Package::open(&path, Mode::Edit(&owner("ann", "here")), &Limits::DEFAULT).unwrap();
    assert_eq!(
        package.access(),
        &Access::ReadOnly(ReadOnlyCause::StaleLock(ghost("here", 4_000_000)))
    );
    // Nothing was broken by opening.
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        holder_json("here", 4_000_000)
    );
    package.take_over(&owner("ann", "here")).unwrap();
    assert!(package.is_editable());
    let content: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock).unwrap()).unwrap();
    assert_eq!(content["user"], "ann");
    drop(package);
    assert!(!lock.exists());
}

#[test]
fn a_lock_of_another_host_is_never_stale() {
    let dir = TempDir::new("lock-host");
    let path = new_package(&dir);
    let lock = lock_path(&path);
    std::fs::write(&lock, holder_json("elsewhere", 1)).unwrap();
    let mut package =
        Package::open(&path, Mode::Edit(&owner("ann", "here")), &Limits::DEFAULT).unwrap();
    assert_eq!(
        package.access(),
        &Access::ReadOnly(ReadOnlyCause::Locked(Some(ghost("elsewhere", 1))))
    );
    assert!(package.take_over(&owner("ann", "here")).is_err());
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap(),
        holder_json("elsewhere", 1)
    );
    // An unreadable lock counts as live, holder unknown.
    std::fs::write(&lock, b"{garbage").unwrap();
    let package =
        Package::open(&path, Mode::Edit(&owner("ann", "here")), &Limits::DEFAULT).unwrap();
    assert_eq!(
        package.access(),
        &Access::ReadOnly(ReadOnlyCause::Locked(None))
    );
}

#[test]
fn a_take_over_fails_when_the_lock_was_taken_in_between() {
    let dir = TempDir::new("lock-race");
    let path = new_package(&dir);
    let lock = lock_path(&path);
    std::fs::write(&lock, holder_json("here", 4_000_000)).unwrap();
    let mut late =
        Package::open(&path, Mode::Edit(&owner("ann", "here")), &Limits::DEFAULT).unwrap();
    // Someone else confirms first.
    let mut early =
        Package::open(&path, Mode::Edit(&owner("bob", "here")), &Limits::DEFAULT).unwrap();
    early.take_over(&owner("bob", "here")).unwrap();
    assert_eq!(
        late.take_over(&owner("ann", "here")).unwrap_err().code(),
        codes::PACKAGE_LOCKED.code
    );
    assert!(!late.is_editable());
    assert!(early.is_editable());
}

#[test]
fn read_only_opens_create_no_lock() {
    let dir = TempDir::new("lock-ro");
    let path = new_package(&dir);
    let files = dir.files();
    let package = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    assert_eq!(
        package.access(),
        &Access::ReadOnly(ReadOnlyCause::Requested)
    );
    assert_eq!(dir.files(), files);
    drop(package);
    assert_eq!(dir.files(), files);
}
