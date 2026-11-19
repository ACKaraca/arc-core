//! A product's own file format through the application's open, save and save-as (CORE-APP-003,
//! 004, 005): a fake format reads a marker file into an empty sample document and writes the
//! marker back. The package format stays the default for the package extension.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use arc_app::application::{AppConfig, Application, TabId};
use arc_app::product::{FileType, NativeFormat, Product, ProductBuilder};
use arc_app::recent::RecentFiles;
use arc_base::{Error, IdGenerator, Limits};
use arc_command::{FixedClock, Invocation, Permission, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::package::LockOwner;
use arc_doc::{Document, DocumentConfig, Snapshot};
use serde_json::json;

const MARKER: &str = "fake-native-v1\n";

/// A directory removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("arc-native-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn owner() -> LockOwner {
    LockOwner {
        user: "tester".into(),
        host: "test-host".into(),
        application: "arc-shell".into(),
        time_unix_ms: 0,
    }
}

/// Counts of the reader and writer calls of a test format.
#[derive(Default)]
struct Calls {
    opens: AtomicUsize,
    saves: AtomicUsize,
}

fn failure() -> Error {
    Error::new(arc_app::codes::FILE_UNSUPPORTED_TYPE)
}

/// A format whose reader accepts the marker file and whose writer is `save`.
fn format_with(
    calls: Arc<Calls>,
    save: impl Fn(&Document, &Path) -> arc_base::Result<()> + Send + Sync + 'static,
) -> NativeFormat {
    let types = Arc::new(arc_shell_sample::types().unwrap());
    let reader = Arc::clone(&calls);
    NativeFormat::new(
        move |path: &Path, config: DocumentConfig| {
            reader.opens.fetch_add(1, Ordering::SeqCst);
            let text = std::fs::read_to_string(path).map_err(|_| failure())?;
            if text != MARKER {
                return Err(failure());
            }
            Document::open(
                Snapshot::empty(Arc::clone(&types)),
                Arc::new(Evaluators::new()),
                config,
            )
        },
        move |document: &Document, path: &Path| {
            calls.saves.fetch_add(1, Ordering::SeqCst);
            save(document, path)
        },
    )
}

/// The format of the tests: the writer writes the marker.
fn fake_format(calls: Arc<Calls>) -> NativeFormat {
    format_with(calls, |_, path| {
        std::fs::write(path, MARKER).map_err(|_| failure())
    })
}

/// Declares the file types `(extension, open, save)` and registers `openers`, then the package
/// extension when given.
fn product_with(
    types: &[(&str, bool, bool)],
    openers: Vec<(&str, NativeFormat)>,
    package: Option<&str>,
) -> arc_base::Result<Product> {
    let mut builder = ProductBuilder::new("fake", "0.1.0")
        .with_types(arc_shell_sample::types().unwrap())
        .with_commands(arc_shell_sample::registry().unwrap());
    for (extension, open, save) in types {
        builder.file_type(FileType {
            extension: (*extension).into(),
            message_id: "shell-file-arc".into(),
            open: *open,
            save: *save,
        })?;
    }
    if let Some(extension) = package {
        builder.package_extension(extension);
    }
    for (extension, format) in openers {
        builder.opener(extension, format)?;
    }
    builder.build(Arc::new(FixedClock(0)), owner())
}

/// The product of the tests: `arc` (package) and `sample` (native, with `format`).
fn product(format: NativeFormat) -> Product {
    product_with(
        &[("arc", true, true), ("sample", true, true)],
        vec![("sample", format)],
        Some("arc"),
    )
    .unwrap()
}

fn app(product: Product, data: &Path) -> Application {
    let next = AtomicU64::new(1 << 32);
    Application::new(
        product,
        AppConfig {
            data_dir: data.to_path_buf(),
            limits: Limits::DEFAULT,
            id_generators: Box::new(move || {
                IdGenerator::seeded(next.fetch_add(1, Ordering::Relaxed))
            }),
            recent: RecentFiles::new(20),
        },
    )
}

/// The temporary files the native save leaves behind in `dir`.
fn temporary_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "tmp"))
        .collect()
}

fn edit(app: &Application, tab: TabId) {
    let session = Session::new(Surface::Automation, [Permission::Edit]);
    let point = Invocation::new("shell.point.create", json!({"position": [1.0, 2.0, 0.0]}));
    app.invoke(tab, &session, point).unwrap();
}

#[test]
fn a_native_file_opens_edits_saves_and_saves_as_through_its_product_functions() {
    let dir = TempDir::new("cycle");
    let calls = Arc::new(Calls::default());
    let mut app = app(product(fake_format(Arc::clone(&calls))), &dir.join("data"));
    let file = dir.join("drawing.sample");
    std::fs::write(&file, MARKER).unwrap();

    let tab = app.open(&file).unwrap();
    assert_eq!(calls.opens.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.tab(tab).unwrap().title().as_deref(),
        Some("drawing.sample")
    );
    assert!(
        !app.tab(tab).unwrap().is_dirty(),
        "a native document opens clean"
    );
    edit(&app, tab);
    assert!(app.tab(tab).unwrap().is_dirty());
    app.save(tab).unwrap();
    assert_eq!(calls.saves.load(Ordering::SeqCst), 1);
    assert!(!app.tab(tab).unwrap().is_dirty());
    assert_eq!(
        app.recent().iter().next().map(|e| e.path.clone()),
        Some(file.clone())
    );

    let copy = dir.join("copy.sample");
    app.save_as(tab, &copy).unwrap();
    assert_eq!(app.tab(tab).unwrap().path(), Some(copy.as_path()));
    assert_eq!(std::fs::read_to_string(&copy).unwrap(), MARKER);
    let package = dir.join("copy.arc");
    assert_eq!(
        app.save_as(tab, &package).unwrap_err().code(),
        "app.file.unsupported_type"
    );
    assert_eq!(app.tab(tab).unwrap().path(), Some(copy.as_path()));
    assert!(!package.exists());
    assert_eq!(
        calls.opens.load(Ordering::SeqCst),
        1,
        "the package extension never reaches the native reader"
    );
}

#[test]
fn an_extension_without_an_opener_takes_the_package_path() {
    let dir = TempDir::new("default");
    let calls = Arc::new(Calls::default());
    let product = product_with(
        &[("arc", true, true), ("sample", true, true)],
        vec![],
        Some("arc"),
    )
    .unwrap();
    let mut app = app(product, &dir.join("data"));
    let file = dir.join("drawing.sample");
    std::fs::write(&file, MARKER).unwrap();
    // The package reader refuses a file that is not a package; the native reader never runs.
    assert_eq!(
        app.open(&file).unwrap_err().code(),
        "doc.package.not_a_package"
    );
    assert_eq!(calls.opens.load(Ordering::SeqCst), 0);
}

#[test]
fn a_native_reader_error_opens_nothing() {
    let dir = TempDir::new("reader");
    let calls = Arc::new(Calls::default());
    let mut app = app(product(fake_format(Arc::clone(&calls))), &dir.join("data"));
    let file = dir.join("broken.sample");
    std::fs::write(&file, "not the marker").unwrap();
    assert!(app.open(&file).is_err());
    assert!(app.tabs().is_empty());
    assert!(
        app.recent().iter().next().is_none(),
        "a failed open is not recent"
    );
}

#[test]
fn an_opener_extension_must_be_well_formed_and_unique() {
    let calls = Arc::new(Calls::default());
    let mut builder = ProductBuilder::new("fake", "0.1.0");
    assert_eq!(
        builder
            .opener("Sample", fake_format(Arc::clone(&calls)))
            .unwrap_err()
            .code(),
        "app.product.invalid_id"
    );
    builder
        .opener("sample", fake_format(Arc::clone(&calls)))
        .unwrap();
    assert_eq!(
        builder
            .opener("sample", fake_format(calls))
            .unwrap_err()
            .code(),
        "app.product.duplicate"
    );
}

#[test]
fn an_opener_without_a_declared_file_type_fails_the_build() {
    let calls = Arc::new(Calls::default());
    let result = product_with(
        &[("arc", true, true)],
        vec![("sample", fake_format(calls))],
        Some("arc"),
    );
    assert_eq!(result.unwrap_err().code(), "app.product.invalid_id");
}

#[test]
fn an_opener_for_the_package_extension_is_refused() {
    let calls = Arc::new(Calls::default());
    let result = product_with(
        &[("arc", true, true)],
        vec![("arc", fake_format(calls))],
        Some("arc"),
    );
    assert_eq!(result.unwrap_err().code(), "app.product.invalid_id");
}

#[test]
fn a_native_document_opens_with_the_unprotected_banner() {
    let dir = TempDir::new("banner");
    let calls = Arc::new(Calls::default());
    let mut app = app(product(fake_format(calls)), &dir.join("data"));
    let file = dir.join("drawing.sample");
    std::fs::write(&file, MARKER).unwrap();
    let tab = app.open(&file).unwrap();
    assert!(
        app.tab(tab)
            .unwrap()
            .banners()
            .iter()
            .any(|b| b.message_id == "app-document-unprotected"),
        "a native document has no recovery journal and says so"
    );
}

#[test]
fn a_save_on_a_native_tab_checks_the_file_type_allows_saving() {
    let dir = TempDir::new("type");
    let calls = Arc::new(Calls::default());
    let product = product_with(
        &[("arc", true, true), ("sample", true, false)],
        vec![("sample", fake_format(Arc::clone(&calls)))],
        Some("arc"),
    )
    .unwrap();
    let mut app = app(product, &dir.join("data"));
    let file = dir.join("drawing.sample");
    std::fs::write(&file, MARKER).unwrap();
    let tab = app.open(&file).unwrap();
    edit(&app, tab);
    assert_eq!(
        app.save(tab).unwrap_err().code(),
        "app.file.unsupported_type"
    );
    assert_eq!(calls.saves.load(Ordering::SeqCst), 0);
    assert!(app.tab(tab).unwrap().is_dirty());
}

#[test]
fn a_pathless_document_saved_to_a_native_extension_becomes_a_native_tab() {
    let dir = TempDir::new("pathless");
    let calls = Arc::new(Calls::default());
    let mut app = app(product(fake_format(Arc::clone(&calls))), &dir.join("data"));
    let tab = app.new_document().unwrap();
    edit(&app, tab);
    let target = dir.join("new.sample");
    app.save_as(tab, &target).unwrap();
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        MARKER,
        "the native writer wrote it"
    );
    assert_eq!(calls.saves.load(Ordering::SeqCst), 1);
    app.save(tab).unwrap();
    assert_eq!(
        calls.saves.load(Ordering::SeqCst),
        2,
        "the tab saves through the native writer now"
    );
}

#[test]
fn a_package_document_is_refused_as_a_native_target_and_nothing_is_written() {
    let dir = TempDir::new("package-to-native");
    let calls = Arc::new(Calls::default());
    let mut app = app(product(fake_format(Arc::clone(&calls))), &dir.join("data"));
    let tab = app.new_document().unwrap();
    let package = dir.join("doc.arc");
    app.save_as(tab, &package).unwrap();
    let target = dir.join("doc.sample");
    assert_eq!(
        app.save_as(tab, &target).unwrap_err().code(),
        "app.file.unsupported_type"
    );
    assert!(!target.exists());
    assert_eq!(calls.saves.load(Ordering::SeqCst), 0);
}

#[test]
fn a_second_open_of_the_same_file_under_another_spelling_is_the_same_tab() {
    let dir = TempDir::new("spelling");
    let calls = Arc::new(Calls::default());
    let mut app = app(product(fake_format(calls)), &dir.join("data"));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let file = dir.join("drawing.sample");
    std::fs::write(&file, MARKER).unwrap();
    let first = app.open(&file).unwrap();
    let other = dir.join("sub").join("..").join("drawing.sample");
    assert_eq!(app.open(&other).unwrap(), first);
    assert_eq!(app.tabs().len(), 1);
}

#[test]
fn save_as_refuses_a_target_held_by_another_tab_without_writing() {
    let dir = TempDir::new("held");
    let calls = Arc::new(Calls::default());
    let mut app = app(product(fake_format(Arc::clone(&calls))), &dir.join("data"));
    let file = dir.join("a.sample");
    std::fs::write(&file, MARKER).unwrap();
    app.open(&file).unwrap();
    let other = app.new_document().unwrap();
    assert_eq!(
        app.save_as(other, &file).unwrap_err().code(),
        "app.document.held"
    );
    assert_eq!(calls.saves.load(Ordering::SeqCst), 0);
    assert!(app.tab(other).unwrap().path().is_none());
}

#[test]
fn the_writer_receives_a_temporary_path_beside_the_target_and_none_is_left_behind() {
    let dir = TempDir::new("temp-ok");
    let seen = Arc::new(std::sync::Mutex::new(Vec::<PathBuf>::new()));
    let log = Arc::clone(&seen);
    let calls = Arc::new(Calls::default());
    let format = format_with(Arc::clone(&calls), move |_, path| {
        log.lock().unwrap().push(path.to_path_buf());
        std::fs::write(path, MARKER).map_err(|_| failure())
    });
    let mut app = app(product(format), &dir.join("data"));
    let file = dir.join("drawing.sample");
    std::fs::write(&file, MARKER).unwrap();
    let tab = app.open(&file).unwrap();
    edit(&app, tab);
    app.save(tab).unwrap();
    let paths = seen.lock().unwrap().clone();
    assert_eq!(paths.len(), 1);
    assert_ne!(paths[0], file, "the writer does not get the target itself");
    assert_eq!(
        paths[0].parent(),
        file.parent(),
        "the temporary path is beside the target"
    );
    assert!(
        !paths[0].exists(),
        "the temporary file was renamed over the target"
    );
    assert!(
        temporary_files(&dir.0).is_empty(),
        "no temporary file is left"
    );
}

#[test]
fn a_failed_native_save_leaves_the_file_intact_and_the_tab_dirty() {
    let dir = TempDir::new("failed-save");
    let calls = Arc::new(Calls::default());
    // The writer damages whatever path it gets, then fails.
    let format = format_with(Arc::clone(&calls), |_, path| {
        std::fs::write(path, "damaged").map_err(|_| failure())?;
        Err(failure())
    });
    let mut app = app(product(format), &dir.join("data"));
    let file = dir.join("drawing.sample");
    std::fs::write(&file, MARKER).unwrap();
    let tab = app.open(&file).unwrap();
    edit(&app, tab);
    assert!(app.save(tab).is_err());
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        MARKER,
        "the original is untouched"
    );
    assert!(app.tab(tab).unwrap().is_dirty());
    assert!(
        temporary_files(&dir.0).is_empty(),
        "no temporary file is left"
    );
}
