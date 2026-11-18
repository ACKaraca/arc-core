//! Hostile packages: random byte changes and truncations of a valid package file are opened,
//! loaded and their previews read under the strict limits. Every outcome is a value or an error
//! with a stable code of this crate or `arc-base`; nothing panics.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::sync::{Arc, OnceLock};

use arc_base::Limits;
use arc_doc::package::{Contents, Metadata, Mode, PREVIEW, Package, THUMBNAIL, write};
use arc_doc::{LoadOptions, StoredObject};
use package_support::{TempDir, cases, preview, registry, stored, types};
use proptest::prelude::*;

/// A valid package with objects of every row case and previews.
fn valid() -> &'static Vec<u8> {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let dir = TempDir::new("hostile-source");
        let path = dir.path("valid.arc");
        let rows: Vec<StoredObject> = (0..package_support::KINDS)
            .map(|kind| stored(kind, u128::from(kind) + 1, 1, Some(1), kind))
            .collect();
        let meta = Metadata {
            partitions: vec!["a".into()],
            ..Metadata::default()
        };
        let declared = types(true);
        let previews = [preview(THUMBNAIL, 256, 256), preview(PREVIEW, 1024, 4)];
        let contents = Contents {
            metadata: &meta,
            types: &declared,
            objects: &rows,
            previews: &previews,
            snapshot: None,
        };
        write(&path, &contents, &mut |_| Ok(())).unwrap();
        std::fs::read(&path).unwrap()
    })
}

/// Opens, loads and reads the previews of `bytes`; the error codes must be registered.
fn exercise(bytes: &[u8]) {
    let dir = TempDir::new("hostile");
    let path = dir.path("mutated.arc");
    std::fs::write(&path, bytes).unwrap();
    let known = |code: &str| {
        arc_doc::error::codes::ALL
            .iter()
            .chain(arc_base::error::codes::ALL)
            .any(|entry| entry.code == code)
    };
    let options = LoadOptions {
        limits: Limits::STRICT,
        ..LoadOptions::default()
    };
    match Package::open(&path, Mode::ReadOnly, &Limits::STRICT) {
        Ok(package) => {
            if let Err(error) = package.load(Arc::clone(&registry(true)), &options) {
                assert!(known(error.code()), "{error:?}");
            }
            if let Err(error) = package.previews() {
                assert!(known(error.code()), "{error:?}");
            }
        }
        Err(error) => assert!(known(error.code()), "{error:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(64), ..ProptestConfig::default() })]

    #[test]
    fn mutated_packages_never_panic(
        changes in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..16),
    ) {
        let mut bytes = valid().clone();
        for (at, value) in changes {
            let at = at.index(bytes.len());
            bytes[at] = value;
        }
        exercise(&bytes);
    }

    #[test]
    fn truncated_packages_never_panic(keep in any::<prop::sample::Index>()) {
        let bytes = valid();
        exercise(&bytes[..keep.index(bytes.len())]);
    }
}

#[test]
fn the_unmutated_package_loads() {
    exercise(valid());
}
