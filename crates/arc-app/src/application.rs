//! The application model without a window: document tabs (CORE-APP-003), open and save flows
//! over paths (CORE-APP-004), read-only and lock banners (CORE-APP-005) and the recovery prompt
//! (CORE-APP-006).
//!
//! A file dialog, drag and drop, a command-line argument and a file association all reduce to
//! [`Application::open`] with a path. Decisions the user makes (save, discard or cancel on
//! close; recover, compare or discard at start-up) are answered by the caller; this module never
//! asks. Banners are data: a Fluent message identifier and its arguments.

use std::any::Any;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_base::error::Value;
use arc_base::{DocumentId, Error, IdGenerator, Limits, Result, SessionId};
use arc_command::{Invocation, Invoker, Outcome, Permission, Session};
use arc_doc::package::recovery::{self, BaseFile, Orphan, RecoverOptions, RecoveryJournal};
use arc_doc::package::{Access, Metadata, Mode, Package, ReadOnlyCause};
use arc_doc::{Document, DocumentConfig, LoadOptions};

use crate::codes;
use crate::product::{NativeFormat, Product};
use crate::recent::RecentFiles;
use crate::settings::SettingsStore;

/// A message shown above a document: a Fluent identifier and its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Banner {
    /// The Fluent message identifier.
    pub message_id: &'static str,
    /// The named arguments of the message.
    pub arguments: BTreeMap<&'static str, Value>,
}

impl Banner {
    /// Adds a named argument.
    fn with(mut self, name: &'static str, value: impl Into<Value>) -> Self {
        self.arguments.insert(name, value.into());
        self
    }

    /// The banner of a read-only cause; `None` for [`ReadOnlyCause::Requested`].
    fn of_cause(cause: &ReadOnlyCause) -> Option<Self> {
        let empty = BTreeMap::new;
        let holder = |banner: Self, h: &arc_doc::package::LockHolder| {
            banner
                .with("user", h.user.as_str())
                .with("host", h.host.as_str())
                .with("application", h.application.as_str())
        };
        Some(match cause {
            ReadOnlyCause::Requested => return None,
            ReadOnlyCause::Locked(Some(h)) => holder(
                Self {
                    message_id: "app-banner-locked",
                    arguments: empty(),
                },
                h,
            ),
            ReadOnlyCause::Locked(None) => Self {
                message_id: "app-banner-locked-unknown",
                arguments: empty(),
            },
            ReadOnlyCause::StaleLock(h) => holder(
                Self {
                    message_id: "app-banner-stale-lock",
                    arguments: empty(),
                },
                h,
            ),
            ReadOnlyCause::NewerFormat(v) => Self {
                message_id: "app-banner-newer-format",
                arguments: empty(),
            }
            .with("version", format!("{}.{}", v.major, v.minor).as_str()),
        })
    }
}

impl From<&Error> for Banner {
    /// A banner showing an error: its message identifier and arguments.
    fn from(error: &Error) -> Self {
        Self {
            message_id: error.message_id(),
            arguments: error.arguments().clone(),
        }
    }
}

/// Identifies an open tab for the lifetime of the application; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabId(u64);

/// One open document.
pub struct Tab {
    id: TabId,
    path: Option<PathBuf>,
    package: Option<Package>,
    /// The product's own format the document was opened with, and saves through; `None` for a
    /// package document. A native document has no edit lock and no recovery journal.
    format: Option<Arc<NativeFormat>>,
    document: Document,
    journal: Option<Arc<RecoveryJournal>>,
    /// The revision of the saved state; `None` when the state was never saved (a recovered
    /// document), which is always dirty.
    saved_revision: Option<u64>,
    banners: Vec<Banner>,
    views: Vec<Box<dyn Any + Send>>,
}

impl std::fmt::Debug for Tab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tab")
            .field("id", &self.id)
            .field("path", &self.path)
            .field("saved_revision", &self.saved_revision)
            .field("banners", &self.banners)
            .finish_non_exhaustive()
    }
}

impl Tab {
    /// The tab's identifier.
    #[must_use]
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The file the document belongs to; `None` for a document never saved.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The title: the file name, or `None` for a document never saved, which the shell names
    /// itself.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        Some(
            self.path
                .as_ref()?
                .file_name()?
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// The document, with its own undo history.
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// True when the document's revision differs from the saved one.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.saved_revision != Some(self.document.snapshot().revision())
    }

    /// True when the package is open read-only: it cannot be saved in place and commands of the
    /// `edit` class are refused.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.package.as_ref().is_some_and(|p| !p.is_editable())
    }

    /// The package's access, when the document has a package.
    #[must_use]
    pub fn access(&self) -> Option<&Access> {
        self.package.as_ref().map(Package::access)
    }

    /// True while a recovery journal records the document's changes.
    #[must_use]
    pub fn is_protected(&self) -> bool {
        self.journal.as_ref().is_some_and(|j| j.is_protected())
    }

    /// The banners to show, in order.
    #[must_use]
    pub fn banners(&self) -> &[Banner] {
        &self.banners
    }

    /// The view states the shell keeps for this document; opaque to this crate.
    pub fn views_mut(&mut self) -> &mut Vec<Box<dyn Any + Send>> {
        &mut self.views
    }

    /// The view states.
    #[must_use]
    pub fn views(&self) -> &[Box<dyn Any + Send>] {
        &self.views
    }
}

/// The caller's answer to closing a document with unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAnswer {
    /// Save, then close; a failed save keeps the document open.
    Save,
    /// Close without saving and delete the recovery journal.
    Discard,
    /// Keep the document open.
    Cancel,
}

/// The result of [`Application::close`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closed {
    /// The tab is closed.
    Closed,
    /// The caller answered [`CloseAnswer::Cancel`]; the tab is open.
    Cancelled,
}

/// What [`Application::compare`] reports about a recoverable session without replaying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// The base file the changes apply to; `None` for a document never saved.
    pub base: Option<PathBuf>,
    /// True when the base file still has the recorded fingerprint, so recovery replays onto it;
    /// false when it changed or is gone, so recovery uses the copy and yields a new document.
    pub base_unchanged: bool,
    /// The number of objects in the base file as it is now; `None` when there is no base or it
    /// cannot be inspected.
    pub base_objects: Option<u64>,
    /// The number of recorded transactions recovery would replay.
    pub records: u64,
}

/// What the application is given at start-up besides the product.
pub struct AppConfig {
    /// The per-user application data directory: recovery databases live under
    /// `<data_dir>/recovery`.
    pub data_dir: PathBuf,
    /// The limits of opening, loading and recovery.
    pub limits: Limits,
    /// Makes an identifier generator on the application's clock and entropy: one for the
    /// application's session and document identifiers and one per document's transactions.
    pub id_generators: Box<dyn FnMut() -> IdGenerator + Send>,
    /// The recent-files list, as loaded by the caller.
    pub recent: RecentFiles,
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfig")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

/// The application: a product, its settings and the open documents.
pub struct Application {
    product: Product,
    settings: SettingsStore,
    data_dir: PathBuf,
    limits: Limits,
    id_generators: Box<dyn FnMut() -> IdGenerator + Send>,
    ids: IdGenerator,
    recent: RecentFiles,
    tabs: Vec<Tab>,
    active: Option<TabId>,
    next_tab: u64,
}

impl std::fmt::Debug for Application {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Application")
            .field("product", &self.product)
            .field("tabs", &self.tabs)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl Application {
    /// An application of `product` with no open document; settings hold only defaults until
    /// the caller sets their layers.
    #[must_use]
    pub fn new(product: Product, mut config: AppConfig) -> Self {
        let ids = (config.id_generators)();
        Self {
            settings: SettingsStore::new(product.settings().clone()),
            product,
            data_dir: config.data_dir,
            limits: config.limits,
            id_generators: config.id_generators,
            ids,
            recent: config.recent,
            tabs: Vec::new(),
            active: None,
            next_tab: 0,
        }
    }

    /// The product.
    #[must_use]
    pub fn product(&self) -> &Product {
        &self.product
    }

    /// The settings.
    #[must_use]
    pub fn settings(&self) -> &SettingsStore {
        &self.settings
    }

    /// The settings, for loading layers and for the settings editor.
    pub fn settings_mut(&mut self) -> &mut SettingsStore {
        &mut self.settings
    }

    /// The recent files; open and save move a path to the front.
    #[must_use]
    pub fn recent(&self) -> &RecentFiles {
        &self.recent
    }

    /// The recent files, for pinning and saving.
    pub fn recent_mut(&mut self) -> &mut RecentFiles {
        &mut self.recent
    }

    /// The open tabs, in display order.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The active tab.
    #[must_use]
    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    fn index(&self, id: TabId) -> Result<usize> {
        self.tabs
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| Error::new(codes::DOCUMENT_UNKNOWN).with_arg("tab", tab_arg(id)))
    }

    /// The tab `id`.
    ///
    /// # Errors
    /// `app.document.unknown`.
    pub fn tab(&self, id: TabId) -> Result<&Tab> {
        let at = self.index(id)?;
        self.tabs
            .get(at)
            .ok_or_else(|| Error::new(codes::DOCUMENT_UNKNOWN))
    }

    /// The tab `id`, for its view states.
    ///
    /// # Errors
    /// `app.document.unknown`.
    pub fn tab_mut(&mut self, id: TabId) -> Result<&mut Tab> {
        let at = self.index(id)?;
        self.tabs
            .get_mut(at)
            .ok_or_else(|| Error::new(codes::DOCUMENT_UNKNOWN))
    }

    /// Makes `id` the active tab.
    ///
    /// # Errors
    /// `app.document.unknown`.
    pub fn activate(&mut self, id: TabId) -> Result<()> {
        self.index(id)?;
        self.active = Some(id);
        Ok(())
    }

    /// Moves tab `id` to position `to`, clamped to the last position.
    ///
    /// # Errors
    /// `app.document.unknown`.
    pub fn move_tab(&mut self, id: TabId, to: usize) -> Result<()> {
        let at = self.index(id)?;
        let tab = self.tabs.remove(at);
        let to = to.min(self.tabs.len());
        self.tabs.insert(to, tab);
        Ok(())
    }

    /// Runs a command on tab `id`'s document through the product's commands. On a read-only
    /// tab the `edit` class is removed from the session first, so editing commands are refused.
    ///
    /// # Errors
    /// `app.document.unknown` and the errors of [`Invoker::invoke`].
    pub fn invoke(&self, id: TabId, session: &Session, invocation: Invocation) -> Result<Outcome> {
        let tab = self.tab(id)?;
        let mut session = session.clone();
        if tab.is_read_only() {
            session.permissions.remove(&Permission::Edit);
        }
        Invoker::new(&self.product.headless().registry).invoke(&tab.document, &session, invocation)
    }

    /// Undoes the top undo step of tab `id`'s document and waits for it; false when there was
    /// nothing to undo. A read-only tab has no edits, so nothing to undo.
    ///
    /// # Errors
    /// `app.document.unknown` and the errors of [`Document::undo`].
    pub fn undo(&self, id: TabId) -> Result<bool> {
        Ok(self.tab(id)?.document.undo().wait()?.value)
    }

    /// Redoes the top redo step of tab `id`'s document, like [`Application::undo`].
    ///
    /// # Errors
    /// `app.document.unknown` and the errors of [`Document::redo`].
    pub fn redo(&self, id: TabId) -> Result<bool> {
        Ok(self.tab(id)?.document.redo().wait()?.value)
    }

    fn document_config(&mut self, session: SessionId) -> DocumentConfig {
        DocumentConfig {
            ids: (self.id_generators)(),
            session,
            limits: self.limits,
            ..DocumentConfig::default()
        }
    }

    fn load_options(&self) -> LoadOptions {
        LoadOptions {
            limits: self.limits,
            ..LoadOptions::default()
        }
    }

    /// Starts a recovery journal for `document`; a failure becomes a banner and the document
    /// is not protected.
    fn protect(
        &self,
        document: &Document,
        id: DocumentId,
        session: SessionId,
        base: Option<BaseFile>,
        banners: &mut Vec<Banner>,
    ) -> Option<Arc<RecoveryJournal>> {
        let mut config = recovery::Session::new(&self.data_dir, id, session, base);
        config.limits = self.limits;
        match RecoveryJournal::start(config) {
            Ok(journal) => {
                document.set_journal(Arc::clone(&journal) as _);
                Some(journal)
            }
            Err(error) => {
                banners.push(Banner::from(&error));
                None
            }
        }
    }

    fn push(&mut self, mut tab: Tab) -> TabId {
        let id = TabId(self.next_tab);
        self.next_tab += 1;
        tab.id = id;
        self.tabs.push(tab);
        self.active = Some(id);
        id
    }

    /// Opens a new, empty document that was never saved; it is clean until edited.
    ///
    /// # Errors
    /// `doc.executor.stopped` when the document cannot start.
    pub fn new_document(&mut self) -> Result<TabId> {
        let session = self.ids.generate();
        let config = self.document_config(session);
        let core = self.product.headless();
        let document = Document::open(
            arc_doc::Snapshot::empty(Arc::clone(&core.types)),
            Arc::clone(&core.evaluators),
            config,
        )?;
        let mut banners = Vec::new();
        let doc_id = self.ids.generate();
        let journal = self.protect(&document, doc_id, session, None, &mut banners);
        let revision = document.snapshot().revision();
        Ok(self.push(Tab {
            id: TabId(0),
            path: None,
            package: None,
            format: None,
            document,
            journal,
            saved_revision: Some(revision),
            banners,
            views: Vec::new(),
        }))
    }

    fn require_type(&self, path: &Path, save: bool) -> Result<()> {
        match self.product.file_type(path) {
            Some(t) if (save && t.save) || (!save && t.open) => Ok(()),
            _ => Err(Error::new(codes::FILE_UNSUPPORTED_TYPE).with_arg("path", path_arg(path))),
        }
    }

    /// Opens the package at `path` as a new tab and activates it; a path that is already open
    /// activates its tab. The package opens for editing under the product's edit lock, or
    /// read-only with a banner naming the lock holder or the newer format version. A document
    /// open for editing gets a recovery journal. The path moves to the front of the recent
    /// files.
    ///
    /// # Errors
    /// `app.file.unsupported_type` when no file type with the open capability matches the
    /// extension; the errors of [`Package::open`] (among them `doc.package.newer_format`, with
    /// arguments `version` and `current`, for a newer format major version, which is refused)
    /// and of [`Package::open_document`]. Nothing is opened on an error.
    pub fn open(&mut self, path: &Path) -> Result<TabId> {
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|t| t.path.as_deref().is_some_and(|p| same_file(p, path)))
        {
            let id = tab.id;
            self.active = Some(id);
            return Ok(id);
        }
        self.require_type(path, false)?;
        if let Some(format) = self.product.opener(path).cloned() {
            return self.open_native(path, format);
        }
        let core = self.product.headless();
        let package = Package::open(path, Mode::Edit(&core.owner), &self.limits)?;
        let session = self.ids.generate();
        let config = self.document_config(session);
        let core = self.product.headless();
        let document = package.open_document(
            Arc::clone(&core.types),
            Arc::clone(&core.evaluators),
            &self.load_options(),
            config,
        )?;
        let mut banners: Vec<Banner> = match package.access() {
            Access::ReadOnly(cause) => Banner::of_cause(cause).into_iter().collect(),
            Access::Edit => Vec::new(),
        };
        let journal = if package.is_editable() {
            self.protect_package(&document, &package, session, &mut banners)
        } else {
            None
        };
        let revision = document.snapshot().revision();
        self.recent.touch(path);
        Ok(self.push(Tab {
            id: TabId(0),
            path: Some(path.to_path_buf()),
            package: Some(package),
            format: None,
            document,
            journal,
            saved_revision: Some(revision),
            banners,
            views: Vec::new(),
        }))
    }

    /// Opens a file of the product's own format as a new tab: the product's reader makes the
    /// document, which is clean. The path moves to the front of the recent files.
    fn open_native(&mut self, path: &Path, format: Arc<NativeFormat>) -> Result<TabId> {
        let session = self.ids.generate();
        let config = self.document_config(session);
        let document = format.read(path, config)?;
        let revision = document.snapshot().revision();
        self.recent.touch(path);
        Ok(self.push(Tab {
            id: TabId(0),
            path: Some(path.to_path_buf()),
            package: None,
            format: Some(format),
            document,
            journal: None,
            saved_revision: Some(revision),
            banners: vec![unprotected_banner()],
            views: Vec::new(),
        }))
    }

    fn protect_package(
        &mut self,
        document: &Document,
        package: &Package,
        session: SessionId,
        banners: &mut Vec<Banner>,
    ) -> Option<Arc<RecoveryJournal>> {
        let doc_id = package
            .metadata()
            .document_id
            .unwrap_or_else(|| self.ids.generate());
        match BaseFile::of(package.path(), package.metadata().save_counter) {
            Ok(base) => self.protect(document, doc_id, session, Some(base), banners),
            Err(error) => {
                banners.push(Banner::from(&error));
                None
            }
        }
    }

    /// Takes over a stale edit lock of tab `id` after the user confirmed it; the tab becomes
    /// editable, its banners are cleared and a recovery journal starts.
    ///
    /// # Errors
    /// `app.document.unknown`, `app.document.no_path` for a tab without a package, and
    /// `doc.package.locked` when the lock is not stale (the tab stays read-only).
    pub fn take_over(&mut self, id: TabId) -> Result<()> {
        let at = self.index(id)?;
        let owner = self.product.headless().owner.clone();
        let tab = self.tabs.get_mut(at).ok_or_else(no_path)?;
        tab.package
            .as_mut()
            .ok_or_else(no_path)?
            .take_over(&owner)?;
        tab.banners.clear();
        let mut tab = self.tabs.remove(at);
        let mut banners = Vec::new();
        if let Some(package) = &tab.package {
            let session = self.ids.generate();
            tab.journal = self.protect_package(&tab.document, package, session, &mut banners);
        }
        tab.banners = banners;
        self.tabs.insert(at, tab);
        Ok(())
    }

    /// Saves tab `id` in place. On success the tab is clean, the recovery journal holds nothing
    /// to replay and the path moves to the front of the recent files. On failure the document
    /// stays open and dirty and the error is returned.
    ///
    /// A failure to tell the recovery journal after the file was written
    /// (`doc.recovery.unavailable`) counts as saved; it becomes a banner.
    ///
    /// # Errors
    /// `app.document.unknown`; `app.document.no_path` for a document never saved (use
    /// [`Application::save_as`]); `app.document.read_only` for a read-only package; the errors
    /// of [`Package::save_document`].
    pub fn save(&mut self, id: TabId) -> Result<()> {
        let at = self.index(id)?;
        if let Some(format) = self.tabs.get(at).and_then(|t| t.format.clone()) {
            let path = self
                .tabs
                .get(at)
                .and_then(|t| t.path.clone())
                .ok_or_else(no_path)?;
            self.require_type(&path, true)?;
            let tab = self.tabs.get_mut(at).ok_or_else(no_path)?;
            let revision = tab.document.snapshot().revision();
            write_native(&format, &tab.document, &path)?;
            tab.saved_revision = Some(revision);
            self.recent.touch(&path);
            return Ok(());
        }
        let tab = self.tabs.get_mut(at).ok_or_else(no_path)?;
        let Some(package) = tab.package.as_mut() else {
            return Err(no_path());
        };
        if !package.is_editable() {
            return Err(
                Error::new(codes::DOCUMENT_READ_ONLY).with_arg("path", path_arg(package.path()))
            );
        }
        // Read before the save: a commit racing the save then leaves the tab dirty, never
        // falsely clean.
        let revision = tab.document.snapshot().revision();
        let result =
            package.save_document(&tab.document, &[], tab.journal.as_deref(), &mut |_| Ok(()));
        match result {
            Ok(_) => {}
            Err(error) if error.code() == "doc.recovery.unavailable" => {
                tab.banners.push(Banner::from(&error));
            }
            Err(error) => return Err(error),
        }
        tab.saved_revision = Some(revision);
        let path = package.path().to_path_buf();
        self.recent.touch(&path);
        Ok(())
    }

    /// Saves tab `id` to `target` by a full rewrite and moves the document there: the edit lock
    /// moves from the old package to the new one, read-only banners are cleared and the target
    /// moves to the front of the recent files. An existing target is overwritten when it opens
    /// for editing (confirming the overwrite is the dialog's task). On failure the old package,
    /// its lock and the dirty state stay as they were, and a target this call created is
    /// removed.
    ///
    /// # Errors
    /// `app.document.unknown`; `app.file.unsupported_type` when no file type with the save
    /// capability matches; `app.document.read_only` when an existing target opens read-only;
    /// the errors of [`Package::create`], [`Package::open`] and [`Package::save_document`].
    pub fn save_as(&mut self, id: TabId, target: &Path) -> Result<()> {
        let at = self.index(id)?;
        if self
            .tabs
            .get(at)
            .and_then(|t| t.path.as_deref())
            .is_some_and(|p| same_file(p, target))
        {
            return self.save(id);
        }
        if self
            .tabs
            .iter()
            .any(|t| t.id != id && t.path.as_deref().is_some_and(|p| same_file(p, target)))
        {
            return Err(Error::new(codes::DOCUMENT_HELD).with_arg("path", path_arg(target)));
        }
        self.require_type(target, true)?;
        if let Some(format) = self.product.opener(target).cloned() {
            return self.save_as_native(at, target, &format);
        }
        if self.tabs.get(at).is_some_and(|t| t.format.is_some()) {
            return Err(unsupported_path(target));
        }
        let owner = self.product.headless().owner.clone();
        let tab = self.tabs.get(at).ok_or_else(no_path)?;
        let metadata = match &tab.package {
            Some(p) => p.metadata().clone(),
            None => Metadata {
                document_id: Some(self.ids.generate()),
                ..Metadata::default()
            },
        };
        let created = !target.exists();
        let mut package = if created {
            Package::create(target, &metadata, &owner)?
        } else {
            let package = Package::open(target, Mode::Edit(&owner), &self.limits)?;
            if !package.is_editable() {
                return Err(
                    Error::new(codes::DOCUMENT_READ_ONLY).with_arg("path", path_arg(target))
                );
            }
            package
        };
        let tab = self.tabs.get(at).ok_or_else(no_path)?;
        let revision = tab.document.snapshot().revision();
        let result =
            package.save_document(&tab.document, &[], tab.journal.as_deref(), &mut |_| Ok(()));
        let mut banners = Vec::new();
        match result {
            Ok(_) => {}
            Err(error) if error.code() == "doc.recovery.unavailable" => {
                banners.push(Banner::from(&error));
            }
            Err(error) => {
                drop(package);
                if created {
                    let _ = std::fs::remove_file(target);
                }
                return Err(error);
            }
        }
        let mut tab = self.tabs.remove(at);
        if tab.journal.is_none() {
            let session = self.ids.generate();
            tab.journal = self.protect_package(&tab.document, &package, session, &mut banners);
        }
        // Replacing the package drops the old one, which releases its edit lock.
        tab.package = Some(package);
        tab.path = Some(target.to_path_buf());
        tab.saved_revision = Some(revision);
        tab.banners = banners;
        self.tabs.insert(at, tab);
        self.recent.touch(target);
        Ok(())
    }

    /// Saves tab `at` to `target`, which has the product's own format: a native tab, or a new
    /// document that was never saved, is written by the product's writer and becomes a native tab
    /// (its recovery journal is discarded, since the file now holds the changes). A package
    /// document is refused with `app.file.unsupported_type`.
    fn save_as_native(
        &mut self,
        at: usize,
        target: &Path,
        format: &Arc<NativeFormat>,
    ) -> Result<()> {
        let tab = self.tabs.get(at).ok_or_else(no_path)?;
        if tab.format.is_none() && tab.package.is_some() {
            return Err(unsupported_path(target));
        }
        let revision = tab.document.snapshot().revision();
        write_native(format, &tab.document, target)?;
        let tab = self.tabs.get_mut(at).ok_or_else(no_path)?;
        if let Some(journal) = tab.journal.take() {
            journal.discard();
        }
        tab.path = Some(target.to_path_buf());
        tab.format = Some(Arc::clone(format));
        tab.saved_revision = Some(revision);
        tab.banners = vec![unprotected_banner()];
        self.recent.touch(target);
        Ok(())
    }

    /// Closes tab `id`. A clean tab closes at once; for a dirty one `ask` is called and its
    /// answer decides: save (a failed save keeps the tab open and returns the error), discard
    /// (the recovery journal is deleted) or cancel. Closing releases the edit lock; a recovery
    /// journal without records is deleted. The next tab, else the previous, becomes active.
    ///
    /// # Errors
    /// `app.document.unknown` and, for [`CloseAnswer::Save`], the errors of
    /// [`Application::save`].
    pub fn close(&mut self, id: TabId, ask: impl FnOnce(&Tab) -> CloseAnswer) -> Result<Closed> {
        let at = self.index(id)?;
        let tab = self.tabs.get(at).ok_or_else(no_path)?;
        let mut discard = false;
        if tab.is_dirty() {
            match ask(tab) {
                CloseAnswer::Cancel => return Ok(Closed::Cancelled),
                CloseAnswer::Save => self.save(id)?,
                CloseAnswer::Discard => discard = true,
            }
        }
        let tab = self.tabs.remove(at);
        tab.document.close();
        if let Some(journal) = &tab.journal {
            if discard {
                journal.discard();
            } else {
                journal.close();
            }
        }
        drop(tab);
        if self.active == Some(id) {
            let next = self.tabs.get(at).or_else(|| self.tabs.last());
            self.active = next.map(|t| t.id);
        }
        Ok(Closed::Closed)
    }

    /// The recovery databases no running session holds, for the start-up prompt: each names its
    /// document, base file, record count and times.
    ///
    /// # Errors
    /// `doc.recovery.unavailable` when the recovery directory cannot be listed.
    pub fn recoverable(&self) -> Result<Vec<Orphan>> {
        recovery::scan(&self.data_dir, &self.limits)
    }

    /// Compares a recoverable session with its base without replaying or changing anything.
    #[must_use]
    pub fn compare(&self, orphan: &Orphan) -> Comparison {
        let base = orphan.base.as_ref();
        let base_objects = base.and_then(|b| {
            let report = arc_doc::package::inspect(Path::new(&b.path), &self.limits).ok()?;
            report["statistics"]["objects"].as_u64()
        });
        Comparison {
            base: base.map(|b| PathBuf::from(&b.path)),
            base_unchanged: base.is_some_and(|b| b.unchanged(&self.limits)),
            base_objects,
            records: orphan.records,
        }
    }

    /// Recovers `orphan` into a new, dirty tab and activates it. When the base file is
    /// unchanged the tab belongs to it and takes its edit lock (or opens read-only with a lock
    /// banner); when it changed, or for a document never saved, the tab has no path and is
    /// saved with [`Application::save_as`], with the banner `app-banner-recovered-copy` when
    /// the base changed. A replay that stopped early adds a banner of its
    /// `doc.recovery.truncated` error. The orphan is deleted by the recovery.
    ///
    /// # Errors
    /// The errors of [`recovery::recover`]; the orphan is kept on an error.
    pub fn recover(&mut self, orphan: &Orphan) -> Result<TabId> {
        let session = self.ids.generate();
        let mut options = RecoverOptions::new(&self.data_dir, session);
        options.config = self.document_config(session);
        options.load = self.load_options();
        let core = self.product.headless();
        let recovered = recovery::recover(
            orphan,
            Arc::clone(&core.types),
            Arc::clone(&core.evaluators),
            options,
        )?;
        let mut banners = Vec::new();
        if let Some(stopped) = &recovered.stopped {
            banners.push(Banner::from(stopped));
        }
        let mut package = None;
        if recovered.base_changed {
            banners.push(Banner {
                message_id: "app-banner-recovered-copy",
                arguments: BTreeMap::new(),
            });
        } else if let Some(base) = &orphan.base {
            let path = Path::new(&base.path);
            match Package::open(path, Mode::Edit(&core.owner), &self.limits) {
                Ok(p) => {
                    if let Access::ReadOnly(cause) = p.access() {
                        banners.extend(Banner::of_cause(cause));
                    }
                    package = Some(p);
                }
                Err(error) => banners.push(Banner::from(&error)),
            }
        }
        Ok(self.push(Tab {
            id: TabId(0),
            path: package.as_ref().map(|p| p.path().to_path_buf()),
            package,
            format: None,
            document: recovered.document,
            journal: Some(recovered.journal),
            saved_revision: None,
            banners,
            views: Vec::new(),
        }))
    }

    /// Deletes a recoverable session after the user chose to discard it.
    ///
    /// # Errors
    /// `doc.recovery.locked` when a running session holds it.
    pub fn discard_recovery(&self, orphan: &Orphan) -> Result<()> {
        recovery::discard(orphan)
    }
}

/// The banner of a native document: it has no recovery journal, so its unsaved changes are lost
/// if the application stops.
fn unprotected_banner() -> Banner {
    Banner {
        message_id: "app-document-unprotected",
        arguments: BTreeMap::new(),
    }
}

/// The identity of `path` for comparisons: its canonical form. A file that does not exist yet
/// takes the canonical form of its directory; a path that cannot be canonicalized is kept as given.
/// Two paths name the same file when their identities are equal.
fn identity(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if let (Some(name), Some(parent)) = (path.file_name(), path.parent()) {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(directory) = std::fs::canonicalize(parent) {
            return directory.join(name);
        }
    }
    path.to_path_buf()
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b || identity(a) == identity(b)
}

/// Writes a native document to `target` through the writer of `format`. The writer gets a
/// temporary path beside the target; on success the file is renamed over the target, on failure
/// the temporary file is removed and the target is left as it was. The rename is a plain rename on
/// every platform: the package writer's retry on Windows is not shared with this crate.
fn write_native(format: &NativeFormat, document: &Document, target: &Path) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = target.file_name().map_or_else(
        || "document".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let temp = target.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (format.writer())(document, &temp).and_then(|()| {
        std::fs::rename(&temp, target).map_err(|_| {
            Error::new(arc_command::error::codes::PATH_IO).with_detail("reason", "rename")
        })
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn unsupported_path(path: &Path) -> Error {
    Error::new(codes::FILE_UNSUPPORTED_TYPE).with_arg("path", path_arg(path))
}

fn no_path() -> Error {
    Error::new(codes::DOCUMENT_NO_PATH)
}

fn path_arg(path: &Path) -> Value {
    path.display().to_string().as_str().into()
}

fn tab_arg(id: TabId) -> Value {
    i64::try_from(id.0).unwrap_or(i64::MAX).into()
}
