//! Product registration (CORE-APP-001): a product describes itself as data and callbacks, and
//! `arc-app` holds no product code.
//!
//! Object types, evaluators and commands are the registries of `arc-doc` and `arc-command`,
//! collected into the [`arc_command::headless::Product`] that the headless runner takes, so one
//! registration serves the CLI and the application. [`ProductBuilder`] adds what only the
//! application uses: file types, settings, and panels, tools and extractors as opaque entries
//! whose [`Factory`] the windowed shell calls and downcasts to its own types.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::{Error, Result};
use arc_command::Registry;
use arc_command::headless;
use arc_command::selection::LastHook;
use arc_command::{Clock, Session, ViewRenderer};
use arc_doc::graph::Evaluators;
use arc_doc::package::LockOwner;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};

use crate::codes;
use crate::settings::{Setting, SettingsRegistry};

/// The longest file extension, in bytes.
pub const MAX_EXTENSION_BYTES: usize = 16;

/// Creates the run-time object of a panel, tool or extractor. The windowed shell knows the
/// concrete type it registered and downcasts the box; this crate never looks inside.
pub trait Factory: Send + Sync {
    /// A new instance.
    fn create(&self) -> Box<dyn Any + Send>;
}

impl<F, T> Factory for F
where
    F: Fn() -> T + Send + Sync,
    T: Any + Send,
{
    fn create(&self) -> Box<dyn Any + Send> {
        Box::new(self())
    }
}

/// A registered panel or tool: an identifier, the Fluent identifier of its label and its
/// factory.
#[derive(Clone)]
pub struct Entry {
    /// A dotted key (`<owner>.<name>`, lowercase), unique among the product's panels or tools.
    pub id: String,
    /// The Fluent identifier of the label.
    pub message_id: String,
    /// Creates the panel or tool.
    pub factory: Arc<dyn Factory>,
}

/// Names the objects whose render data depends on objects a commit changed, so they are extracted
/// again: given the committed snapshot and its changes, the extra objects to extract (a wall's
/// solid depends on the elevation of its storey's level, for example). See
/// [`ProductBuilder::extraction_dependents`].
pub type Dependents =
    Arc<dyn Fn(&Snapshot, &arc_doc::events::Changes) -> Vec<ObjectId> + Send + Sync>;

/// A registered extractor: it derives render data from objects of the listed types.
#[derive(Clone)]
pub struct Extractor {
    /// A dotted key, unique among the product's extractors.
    pub id: String,
    /// The object type keys it handles; each must be a registered type.
    pub types: Vec<String>,
    /// Creates the extractor.
    pub factory: Arc<dyn Factory>,
}

macro_rules! debug_without_factory {
    ($ty:ty, $($field:ident),+) => {
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty))
                    $(.field(stringify!($field), &self.$field))+
                    .finish_non_exhaustive()
            }
        }
    };
}
debug_without_factory!(Entry, id, message_id);
debug_without_factory!(Extractor, id, types);

/// A file type the product opens or saves. Every file type is a document package; the
/// extension only routes a path to the product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileType {
    /// The extension without the dot: 1 to [`MAX_EXTENSION_BYTES`] lowercase ASCII letters and
    /// digits. Paths match it case-insensitively.
    pub extension: String,
    /// The Fluent identifier of the description shown in dialogs.
    pub message_id: String,
    /// Whether paths of this type can be opened.
    pub open: bool,
    /// Whether documents can be saved to paths of this type.
    pub save: bool,
}

/// The reader of a product's own format: the document of a file, made with the configuration the
/// application gives a document, and the session to run product commands with.
type NativeOpen = dyn Fn(&Path, DocumentConfig, &Session) -> Result<Document> + Send + Sync;

/// The writer of a product's own format: a document to a file, with the session to run product
/// commands with. The session's write roots contain the file's directory.
type NativeSave = dyn Fn(&Document, &Path, &Session) -> Result<()> + Send + Sync;

/// A product's own file format, opened and saved by the product's functions instead of the
/// package format. Registered with [`ProductBuilder::opener`] for one extension.
pub struct NativeFormat {
    open: Box<NativeOpen>,
    save: Arc<NativeSave>,
}

impl NativeFormat {
    /// A format whose files `open` reads into a document and whose documents `save` writes to a
    /// path. The application never saves a native document to a package.
    ///
    /// What the callbacks receive:
    ///
    /// - `open` gets the path of the file, the configuration the application makes for a document,
    ///   and a session. The session is the application's session (see
    ///   [`Application::set_session`](crate::application::Application::set_session)) with its read
    ///   roots replaced by the directory subtree that contains the file. Nothing else in it is
    ///   changed.
    /// - `save` gets the document, the path of an existing, empty temporary file beside the target,
    ///   and a session with the same construction: its write roots are the directory subtree
    ///   containing the target, and its read roots too. The application created the temporary file
    ///   exclusively before the call. After `save` returns `Ok`, the application renames the file
    ///   over the target with the roots' replace operation; after an error it removes the file and
    ///   the target stays as it was. `save` writes the temporary path itself, by its own means.
    ///
    /// A command run with `arc_command::Invoker` and the session can access files only inside those
    /// subtrees, through the same checks as any other command.
    pub fn new(
        open: impl Fn(&Path, DocumentConfig, &Session) -> Result<Document> + Send + Sync + 'static,
        save: impl Fn(&Document, &Path, &Session) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            open: Box::new(open),
            save: Arc::new(save),
        }
    }

    /// Reads the document at `path`.
    pub(crate) fn read(
        &self,
        path: &Path,
        config: DocumentConfig,
        session: &Session,
    ) -> Result<Document> {
        (self.open)(path, config, session)
    }

    /// The save function, shared with the tabs that opened this format.
    pub(crate) fn writer(&self) -> Arc<NativeSave> {
        Arc::clone(&self.save)
    }
}

impl std::fmt::Debug for NativeFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeFormat").finish_non_exhaustive()
    }
}

/// A product as the application runs it: the headless product and the application-only
/// registrations.
pub struct Product {
    core: headless::Product,
    settings: SettingsRegistry,
    file_types: Vec<FileType>,
    panels: Vec<Entry>,
    tools: Vec<Entry>,
    extractors: Vec<Extractor>,
    dependents: Option<Dependents>,
    view_renderer: Option<Arc<dyn ViewRenderer>>,
    openers: BTreeMap<String, Arc<NativeFormat>>,
    new_document: Option<String>,
    #[cfg(feature = "window")]
    interaction: Option<crate::shell::interaction::Factory>,
}

impl std::fmt::Debug for Product {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Product")
            .field("core", &self.core)
            .field("settings", &self.settings)
            .field("view_renderer", &self.view_renderer.is_some())
            .finish_non_exhaustive()
    }
}

impl Product {
    /// The command that fills a new, empty document, when the product registered one (see
    /// [`ProductBuilder::new_document_command`]).
    #[must_use]
    pub fn new_document_command(&self) -> Option<&str> {
        self.new_document.as_deref()
    }

    /// The renderer of `view.render` and of the automation socket, when the product has one.
    #[must_use]
    pub fn view_renderer(&self) -> Option<Arc<dyn ViewRenderer>> {
        self.view_renderer.clone()
    }

    /// The factory of the product's interaction controller, when the product registered one
    /// (see [`ProductBuilder::interaction`]).
    #[cfg(feature = "window")]
    #[must_use]
    pub fn interaction(&self) -> Option<&crate::shell::interaction::Factory> {
        self.interaction.as_ref()
    }

    /// The part the headless runner takes: name, version, types, evaluators, commands, clock
    /// and edit-lock owner.
    #[must_use]
    pub fn headless(&self) -> &headless::Product {
        &self.core
    }

    /// The built-in settings and the product's.
    #[must_use]
    pub fn settings(&self) -> &SettingsRegistry {
        &self.settings
    }

    /// The file types, in registration order.
    #[must_use]
    pub fn file_types(&self) -> &[FileType] {
        &self.file_types
    }

    /// The panels, in registration order.
    #[must_use]
    pub fn panels(&self) -> &[Entry] {
        &self.panels
    }

    /// The tools, in registration order.
    #[must_use]
    pub fn tools(&self) -> &[Entry] {
        &self.tools
    }

    /// The extractors, in registration order.
    #[must_use]
    pub fn extractors(&self) -> &[Extractor] {
        &self.extractors
    }

    /// The product's function naming the objects to extract again after a commit, when it
    /// registered one (see [`ProductBuilder::extraction_dependents`]).
    #[must_use]
    pub fn extraction_dependents(&self) -> Option<&Dependents> {
        self.dependents.as_ref()
    }

    /// The file type of `path` by its extension, when one is registered.
    #[must_use]
    pub fn file_type(&self, path: &Path) -> Option<&FileType> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        self.file_types.iter().find(|t| t.extension == extension)
    }

    /// The product's own format for `path`, by extension (case-insensitive); `None` means the
    /// package format.
    #[must_use]
    pub fn opener(&self, path: &Path) -> Option<&Arc<NativeFormat>> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        self.openers.get(&extension)
    }
}

/// Collects a product's registrations.
pub struct ProductBuilder {
    name: String,
    version: String,
    types: TypeRegistry,
    evaluators: Evaluators,
    commands: Registry,
    settings: SettingsRegistry,
    file_types: Vec<FileType>,
    panels: Vec<Entry>,
    tools: Vec<Entry>,
    extractors: Vec<Extractor>,
    dependents: Option<Dependents>,
    view_renderer: Option<Arc<dyn ViewRenderer>>,
    #[cfg(feature = "window")]
    interaction: Option<crate::shell::interaction::Factory>,
    last: Option<LastHook>,
    openers: BTreeMap<String, Arc<NativeFormat>>,
    package_extension: Option<String>,
    new_document: Option<String>,
}

impl std::fmt::Debug for ProductBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProductBuilder")
            .field("name", &self.name)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

fn invalid(id: &str) -> Error {
    Error::new(codes::PRODUCT_INVALID_ID).with_arg("id", id)
}

fn duplicate(id: &str) -> Error {
    Error::new(codes::PRODUCT_DUPLICATE).with_arg("id", id)
}

/// Adds `entry` to `list` after checking its identifier.
fn add_entry(list: &mut Vec<Entry>, entry: Entry) -> Result<()> {
    if !arc_doc::schema::is_key(&entry.id) {
        return Err(invalid(&entry.id));
    }
    if list.iter().any(|e| e.id == entry.id) {
        return Err(duplicate(&entry.id));
    }
    list.push(entry);
    Ok(())
}

impl ProductBuilder {
    /// A product without registrations except the built-in settings.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            types: TypeRegistry::new(),
            evaluators: Evaluators::new(),
            commands: Registry::new(),
            settings: SettingsRegistry::builtin(),
            file_types: Vec::new(),
            panels: Vec::new(),
            tools: Vec::new(),
            extractors: Vec::new(),
            dependents: None,
            view_renderer: None,
            #[cfg(feature = "window")]
            interaction: None,
            last: None,
            openers: BTreeMap::new(),
            package_extension: None,
            new_document: None,
        }
    }

    /// The object types, to register into with [`TypeRegistry::register`].
    pub fn types_mut(&mut self) -> &mut TypeRegistry {
        &mut self.types
    }

    /// Replaces the object types with a registry built elsewhere.
    #[must_use]
    pub fn with_types(mut self, types: TypeRegistry) -> Self {
        self.types = types;
        self
    }

    /// The node evaluators.
    pub fn evaluators_mut(&mut self) -> &mut Evaluators {
        &mut self.evaluators
    }

    /// The commands, to register into with [`Registry::register`].
    pub fn commands_mut(&mut self) -> &mut Registry {
        &mut self.commands
    }

    /// Replaces the commands with a registry built elsewhere.
    #[must_use]
    pub fn with_commands(mut self, commands: Registry) -> Self {
        self.commands = commands;
        self
    }

    /// Registers a product setting beside the built-in ones.
    ///
    /// # Errors
    /// The errors of [`SettingsRegistry::register`].
    pub fn setting(&mut self, setting: Setting) -> Result<()> {
        self.settings.register(setting)
    }

    /// Registers a file type.
    ///
    /// # Errors
    /// `app.product.invalid_id` for a malformed extension or one that can neither be opened
    /// nor saved; `app.product.duplicate` for an extension registered twice.
    pub fn file_type(&mut self, file_type: FileType) -> Result<()> {
        let ext = &file_type.extension;
        let well_formed = (1..=MAX_EXTENSION_BYTES).contains(&ext.len())
            && ext
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if !well_formed || !(file_type.open || file_type.save) {
            return Err(invalid(ext));
        }
        if self.file_types.iter().any(|t| t.extension == *ext) {
            return Err(duplicate(ext));
        }
        self.file_types.push(file_type);
        Ok(())
    }

    /// Declares the extension of the product's document package format. Its file type is a
    /// package type, so no opener may be registered for it. Without this call the application
    /// has no package extension to protect.
    pub fn package_extension(&mut self, extension: &str) {
        self.package_extension = Some(extension.to_owned());
    }

    /// Registers the product's own format for `extension`: a path with this extension is opened
    /// and saved by `format` instead of the package format. The extension must also be declared
    /// with [`ProductBuilder::file_type`]; [`ProductBuilder::build`] checks that.
    ///
    /// # Errors
    /// `app.product.invalid_id` for a malformed extension or the package extension;
    /// `app.product.duplicate` for an extension with an opener already.
    pub fn opener(&mut self, extension: &str, format: NativeFormat) -> Result<()> {
        let well_formed = (1..=MAX_EXTENSION_BYTES).contains(&extension.len())
            && extension
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        if !well_formed || self.package_extension.as_deref() == Some(extension) {
            return Err(invalid(extension));
        }
        if self.openers.contains_key(extension) {
            return Err(duplicate(extension));
        }
        self.openers.insert(extension.to_owned(), Arc::new(format));
        Ok(())
    }

    /// Registers a panel.
    ///
    /// # Errors
    /// `app.product.invalid_id` or `app.product.duplicate`.
    pub fn panel(&mut self, panel: Entry) -> Result<()> {
        add_entry(&mut self.panels, panel)
    }

    /// Registers a tool.
    ///
    /// # Errors
    /// `app.product.invalid_id` or `app.product.duplicate`.
    pub fn tool(&mut self, tool: Entry) -> Result<()> {
        add_entry(&mut self.tools, tool)
    }

    /// Attaches the renderer of the product's views: `view.render` on the protocol and the
    /// automation socket. A product without one answers `cmd.render.unavailable`.
    pub fn view_renderer(&mut self, renderer: Arc<dyn ViewRenderer>) {
        self.view_renderer = Some(renderer);
    }

    /// Registers the function that names the objects to extract again after a commit, besides the
    /// changed ones: an object whose render data derives from another object (a wall from the level
    /// of its storey) is listed when that object changes. It is called on the shell's frame with
    /// the committed snapshot; a commit whose changes are "everything" extracts everything and
    /// does not call it.
    pub fn extraction_dependents(
        &mut self,
        f: impl Fn(&Snapshot, &arc_doc::events::Changes) -> Vec<ObjectId> + Send + Sync + 'static,
    ) {
        self.dependents = Some(Arc::new(f));
    }

    /// Registers the product's interaction controller: `factory` creates one per tab, and the
    /// windowed shell routes the command panel, option bar, viewport pointer events and dynamic
    /// input of the tab to it. Without a registration the shell keeps its own command line.
    #[cfg(feature = "window")]
    pub fn interaction(
        &mut self,
        factory: impl Fn() -> Box<dyn crate::shell::interaction::Interaction> + Send + Sync + 'static,
    ) {
        self.interaction = Some(Arc::new(factory));
    }

    /// Names the command that fills a new document before the user sees it (a drawing needs its
    /// first space, for instance). The application runs it, without parameters and with the
    /// permissions `read`, `edit` and `file_read`, on the new document as its starting content
    /// (`arc_command::Invocation::initial`): the transaction is recorded, so the document saves and
    /// recovers with it, but it is no undo step, and the document is clean until the user edits
    /// it. The command reads the interface language from the session setting
    /// [`crate::commands::LANGUAGE_SETTING`] to name what it creates.
    pub fn new_document_command(&mut self, command: impl Into<String>) {
        self.new_document = Some(command.into());
    }

    /// Gives the keyword `last` its meaning in every session the product serves: `hook` returns
    /// the objects the product names as the last one, live or not. Without it, `last` fails with
    /// `cmd.selection.unavailable`.
    pub fn last_objects(
        &mut self,
        hook: impl Fn(&Snapshot) -> Vec<ObjectId> + Send + Sync + 'static,
    ) {
        self.last = Some(Arc::new(hook));
    }

    /// Registers an extractor. Its types are checked by [`ProductBuilder::build`], so types
    /// may be registered later.
    ///
    /// # Errors
    /// `app.product.invalid_id` or `app.product.duplicate`.
    pub fn extractor(&mut self, extractor: Extractor) -> Result<()> {
        if !arc_doc::schema::is_key(&extractor.id) {
            return Err(invalid(&extractor.id));
        }
        if self.extractors.iter().any(|e| e.id == extractor.id) {
            return Err(duplicate(&extractor.id));
        }
        self.extractors.push(extractor);
        Ok(())
    }

    /// Builds the product with the process clock and edit-lock owner.
    ///
    /// # Errors
    /// `app.product.unknown_type` when an extractor names a type that is not registered.
    pub fn build(self, clock: Arc<dyn Clock>, owner: LockOwner) -> Result<Product> {
        for extension in self.openers.keys() {
            let declared = self.file_types.iter().any(|t| t.extension == *extension);
            if !declared || self.package_extension.as_deref() == Some(extension.as_str()) {
                return Err(invalid(extension));
            }
        }
        let known: BTreeSet<&str> = self.types.iter().map(|d| d.key.as_str()).collect();
        for extractor in &self.extractors {
            if let Some(missing) = extractor.types.iter().find(|t| !known.contains(t.as_str())) {
                return Err(Error::new(codes::PRODUCT_UNKNOWN_TYPE)
                    .with_arg("id", extractor.id.as_str())
                    .with_arg("type", missing.as_str()));
            }
        }
        Ok(Product {
            core: headless::Product {
                name: self.name,
                version: self.version,
                types: Arc::new(self.types),
                evaluators: Arc::new(self.evaluators),
                registry: self.commands,
                clock,
                owner,
                last: self.last,
            },
            settings: self.settings,
            file_types: self.file_types,
            panels: self.panels,
            tools: self.tools,
            extractors: self.extractors,
            dependents: self.dependents,
            view_renderer: self.view_renderer,
            #[cfg(feature = "window")]
            interaction: self.interaction,
            openers: self.openers,
            new_document: self.new_document,
        })
    }
}
