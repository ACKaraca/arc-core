//! Product registration (CORE-APP-001): a product describes itself as data and callbacks, and
//! `arc-app` holds no product code.
//!
//! Object types, evaluators and commands are the registries of `arc-doc` and `arc-command`,
//! collected into the [`arc_command::headless::Product`] that the headless runner takes, so one
//! registration serves the CLI and the application. [`ProductBuilder`] adds what only the
//! application uses: file types, settings, and panels, tools and extractors as opaque entries
//! whose [`Factory`] the windowed shell calls and downcasts to its own types.

use std::any::Any;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use arc_base::{Error, Result};
use arc_command::Registry;
use arc_command::headless;
use arc_command::{Clock, ViewRenderer};
use arc_doc::TypeRegistry;
use arc_doc::graph::Evaluators;
use arc_doc::package::LockOwner;

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

/// A product as the application runs it: the headless product and the application-only
/// registrations.
pub struct Product {
    core: headless::Product,
    settings: SettingsRegistry,
    file_types: Vec<FileType>,
    panels: Vec<Entry>,
    tools: Vec<Entry>,
    extractors: Vec<Extractor>,
    view_renderer: Option<Arc<dyn ViewRenderer>>,
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
    /// The renderer of `view.render` and of the automation socket, when the product has one.
    #[must_use]
    pub fn view_renderer(&self) -> Option<Arc<dyn ViewRenderer>> {
        self.view_renderer.clone()
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

    /// The file type of `path` by its extension, when one is registered.
    #[must_use]
    pub fn file_type(&self, path: &Path) -> Option<&FileType> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        self.file_types.iter().find(|t| t.extension == extension)
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
    view_renderer: Option<Arc<dyn ViewRenderer>>,
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
            view_renderer: None,
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
            },
            settings: self.settings,
            file_types: self.file_types,
            panels: self.panels,
            tools: self.tools,
            extractors: self.extractors,
            view_renderer: self.view_renderer,
        })
    }
}
