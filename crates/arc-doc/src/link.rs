//! External references and links (CORE-DOC-011), and unresolved and outdated links (CORE-DOC-012).
//!
//! A document links another one through a `core.doc.link` object ([`LINK_TYPE`]). The object
//! names the linked document's identifier, a path relative to the referencing document, the load
//! mode and what was last loaded from the path: the BLAKE3 hash of the linked file's content
//! ([`Opened::content`]) and its revision. An [`ExternalRef`] value `(link, target)` names an
//! object of the linked document through its link.
//!
//! The crate reads no paths. Opening a path is the caller's job: a [`LinkResolver`] turns the
//! path, seen from the document that holds it, into an [`Opened`] document: the loaded snapshot
//! and the content hash the caller computed from the file's bytes. [`load`] walks the links of a
//! document through the resolver. Each document of a load is opened once and walked once, however
//! many links name it, so cycles of links (A links B, B links A) load and terminate. Chains of
//! documents nested deeper than [`Limits::nesting_depth`] are refused.
//!
//! Linked documents are read-only: a [`Links`] holds immutable snapshots and offers no write
//! path, so edits never reach a linked document. A link whose document changed is `outdated`:
//! its objects stay readable from the file as it is now, and [`reload`] records the new content
//! in a transaction of kind [`TxnKind::LinkReload`]. Its field changes are ordinary operations,
//! so undo restores the recorded content and the link is outdated again.
//!
//! An unresolved link keeps its link object and every external reference in the document; they
//! are reported by [`Links::problems`] and are not readable. Nothing is deleted or rewritten.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::error::ErrorCode;
use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};

use crate::error::codes;
use crate::registry::TypeRegistry;
use crate::schema::{
    ExternalRef, FieldDecl, FieldPath, Schema, SchemaVersion, Severity, TypeDecl, TypeKey, Value,
    ValueType,
};
use crate::snapshot::{Header, Object, Snapshot};
use crate::txn::{Txn, TxnKind};

/// The type key of link objects.
pub const LINK_TYPE: &str = "core.doc.link";

const DOCUMENT: &str = "document";
const PATH: &str = "path";
const HUB_RESOURCE: &str = "hub_resource";
const MODE: &str = "load_mode";
const REVISION: &str = "revision";
const CONTENT: &str = "content";

/// Registers the `core.doc.link` type in `registry`: fails like [`TypeRegistry::register`].
pub fn register(registry: &mut TypeRegistry) -> Result<()> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new(DOCUMENT, ValueType::Opaque),
            FieldDecl::new(PATH, ValueType::String),
            FieldDecl::new(HUB_RESOURCE, ValueType::String).optional(),
            FieldDecl::new(MODE, ValueType::String),
            FieldDecl::new(REVISION, ValueType::I64).optional(),
            FieldDecl::new(CONTENT, ValueType::Opaque).optional(),
        ],
    );
    registry.register(TypeDecl::new(TypeKey::new(LINK_TYPE)?, schema))
}

/// What a link loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadMode {
    /// The whole linked document.
    Full,
    /// Only the partitions the product lists.
    PartitionList,
    /// Nothing: the references through the link are not readable.
    NotLoaded,
}

impl LoadMode {
    fn text(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::PartitionList => "partition_list",
            Self::NotLoaded => "not_loaded",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "full" => Some(Self::Full),
            "partition_list" => Some(Self::PartitionList),
            "not_loaded" => Some(Self::NotLoaded),
            _ => None,
        }
    }
}

/// The fields of a new link object.
#[derive(Debug, Clone, Copy)]
pub struct LinkSpec<'a> {
    /// The identifier of the linked document.
    pub document: ObjectId,
    /// The path to the linked document, relative to the referencing document.
    pub path: &'a str,
    /// The identifier of the linked resource in a hub, if any.
    pub hub_resource: Option<&'a str>,
    /// What the link loads.
    pub mode: LoadMode,
}

/// Creates the link object `id` with `spec`; nothing has been loaded through it yet, so it is
/// outdated until [`reload`] records its content. Fails with the codes of [`Txn::create`].
pub fn create(txn: &mut Txn, id: ObjectId, spec: &LinkSpec<'_>) -> Result<()> {
    let values = vec![
        Some(Value::Opaque(id_bytes(spec.document))),
        Some(Value::String(spec.path.to_owned())),
        spec.hub_resource.map(|hub| Value::String(hub.to_owned())),
        Some(Value::String(spec.mode.text().to_owned())),
        None,
        None,
    ];
    txn.create(LINK_TYPE, id, Header::default(), values)
        .map(drop)
}

fn id_bytes(id: ObjectId) -> Vec<u8> {
    id.as_u128().to_be_bytes().to_vec()
}

/// A problem with one link: its stable code and a short reason, both as the error reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Problem {
    /// The code of the problem.
    pub code: ErrorCode,
    /// The reason: the code of the failure that caused it, or a fixed token.
    pub reason: &'static str,
}

impl Problem {
    fn new(code: ErrorCode, reason: &'static str) -> Self {
        Self { code, reason }
    }
}

/// The fields of a link object as stored.
#[derive(Debug)]
struct Record {
    document: ObjectId,
    path: String,
    mode: LoadMode,
    content: Option<[u8; 32]>,
}

fn record(snapshot: &Snapshot, link: ObjectId) -> std::result::Result<Record, Problem> {
    let bad = |reason| Problem::new(codes::LINK_INVALID, reason);
    let handle = snapshot.resolve(link).map_err(|_| bad("missing"))?;
    if snapshot.type_key(handle).map(TypeKey::as_str) != Some(LINK_TYPE) {
        return Err(bad("type"));
    }
    let object = snapshot.get(handle).map_err(|_| bad("missing"))?;
    let value = |at: usize| object.values.get(at).and_then(Option::as_ref);
    let document = match value(0) {
        Some(Value::Opaque(bytes)) => <[u8; 16]>::try_from(bytes.as_slice())
            .ok()
            .map(|raw| ObjectId::from_u128(u128::from_be_bytes(raw))),
        _ => None,
    }
    .ok_or_else(|| bad("document"))?;
    let path = match value(1) {
        Some(Value::String(path)) => path.clone(),
        _ => return Err(bad("path")),
    };
    let mode = match value(3) {
        Some(Value::String(text)) => LoadMode::parse(text).ok_or_else(|| bad("load_mode"))?,
        _ => return Err(bad("load_mode")),
    };
    let content = match value(5) {
        None => None,
        Some(Value::Opaque(bytes)) => {
            Some(<[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| bad("content"))?)
        }
        Some(_) => return Err(bad("content")),
    };
    Ok(Record {
        document,
        path,
        mode,
        content,
    })
}

/// The error that reports `problem` about `link` when a reference through it is read.
fn refusal(link: ObjectId, problem: Problem) -> Error {
    Error::new(problem.code)
        .with_arg("link", link.to_string())
        .with_arg("reason", problem.reason)
}

/// A document opened through a resolver: its location, its identifier, the BLAKE3 hash of its
/// file's content and its snapshot. The location is the caller's name for the document; links
/// inside it resolve their paths relative to it.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The caller's name of the document.
    pub location: String,
    /// The identifier of the document, which the link must name.
    pub document: ObjectId,
    /// The BLAKE3 hash of the document's file as the caller read it.
    pub content: [u8; 32],
    /// The document's state, read-only.
    pub snapshot: Snapshot,
}

/// Turns a path, seen from the document at location `from`, into an opened document. The
/// crate calls it for each link to a document not yet loaded by the same load; a closure of the
/// same signature implements the trait.
pub trait LinkResolver {
    /// Opens the document that `path` names from the document at `from`. A failure makes the
    /// link unresolved, with the failure's code as its reason.
    fn open(&self, from: &str, path: &str) -> Result<Opened>;
}

impl<F> LinkResolver for F
where
    F: Fn(&str, &str) -> Result<Opened>,
{
    fn open(&self, from: &str, path: &str) -> Result<Opened> {
        self(from, path)
    }
}

/// A document of one load: its location, identifier, the content hash of its file (absent for
/// the root document until a link opens it), its snapshot and the states of its links.
#[derive(Debug)]
struct Linked {
    location: String,
    content: Option<[u8; 32]>,
    snapshot: Snapshot,
    states: BTreeMap<ObjectId, LinkState>,
}

/// The state of one link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// The link loads nothing (`not_loaded`).
    NotLoaded,
    /// The linked document (by identifier) is open and matches the content the link recorded.
    Resolved(ObjectId),
    /// The linked document (by identifier) is open but its content differs from the one the
    /// link recorded, or none was recorded yet.
    Outdated(ObjectId),
    /// The link does not open its document; references through it are not readable.
    Unresolved(Problem),
    /// The link is refused (a nesting limit); references through it are not readable.
    Refused(Problem),
}

/// A problem with a link of a document, as [`Links::problems`] reports it.
#[derive(Debug, Clone)]
pub struct LinkProblem {
    /// The link object.
    pub link: ObjectId,
    /// How serious it is: outdated and unresolved links are warnings, refused links errors.
    pub severity: Severity,
    /// The code of the problem.
    pub code: ErrorCode,
    /// The reason, as [`Problem::reason`].
    pub reason: &'static str,
}

/// The link states of one document of a load, and access to the documents it links.
///
/// The documents of a load are shared: [`Links::linked`] returns the view of a linked document,
/// which may be the root itself or a document that links back to an ancestor.
#[derive(Debug, Clone)]
pub struct Links {
    document: ObjectId,
    documents: Arc<BTreeMap<ObjectId, Linked>>,
}

impl Links {
    fn entry(&self) -> Option<&Linked> {
        self.documents.get(&self.document)
    }

    /// The identifier of the document whose links these are.
    #[must_use]
    pub fn document(&self) -> ObjectId {
        self.document
    }

    /// The snapshot of that document.
    #[must_use]
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.entry().map(|entry| &entry.snapshot)
    }

    /// The state of the link object `link`, if the document has it.
    #[must_use]
    pub fn state(&self, link: ObjectId) -> Option<&LinkState> {
        self.entry()?.states.get(&link)
    }

    /// Every link with its state, in identifier order.
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &LinkState)> + '_ {
        self.entry()
            .into_iter()
            .flat_map(|entry| entry.states.iter().map(|(link, state)| (*link, state)))
    }

    /// The view of the document that `link` links, when the link resolves.
    #[must_use]
    pub fn linked(&self, link: ObjectId) -> Option<Links> {
        match self.state(link)? {
            LinkState::Resolved(document) | LinkState::Outdated(document) => Some(Links {
                document: *document,
                documents: Arc::clone(&self.documents),
            }),
            LinkState::NotLoaded | LinkState::Unresolved(_) | LinkState::Refused(_) => None,
        }
    }

    /// The object `reference.target` of the document that `reference.link` links. Fails with
    /// `doc.link.invalid` (reason `missing`) when the document holds no such link, and with
    /// `doc.link.unresolved`, `doc.link.too_deep` or `doc.link.not_loaded` when the link has no
    /// readable document, and with `doc.object.not_found` when the linked document does not hold
    /// the object.
    pub fn object(&self, reference: &ExternalRef) -> Result<&Arc<Object>> {
        let link = reference.link;
        match self.state(link) {
            None => Err(refusal(link, Problem::new(codes::LINK_INVALID, "missing"))),
            Some(LinkState::NotLoaded) => Err(refusal(
                link,
                Problem::new(codes::LINK_NOT_LOADED, "load_mode"),
            )),
            Some(LinkState::Unresolved(problem) | LinkState::Refused(problem)) => {
                Err(refusal(link, *problem))
            }
            Some(LinkState::Resolved(document) | LinkState::Outdated(document)) => {
                let linked = self
                    .documents
                    .get(document)
                    .ok_or_else(|| refusal(link, Problem::new(codes::LINK_INVALID, "missing")))?;
                let handle = linked.snapshot.resolve(reference.target)?;
                linked.snapshot.get(handle)
            }
        }
    }

    /// The problems of the document's own links: outdated, unresolved and refused links, in
    /// identifier order. The links of linked documents report their problems on their own view
    /// ([`Links::linked`]).
    #[must_use]
    pub fn problems(&self) -> Vec<LinkProblem> {
        self.iter()
            .filter_map(|(link, state)| {
                let (severity, code, reason) = match state {
                    LinkState::NotLoaded | LinkState::Resolved(_) => return None,
                    LinkState::Outdated(_) => (Severity::Warning, codes::LINK_OUTDATED, "content"),
                    LinkState::Unresolved(problem) => {
                        (Severity::Warning, problem.code, problem.reason)
                    }
                    LinkState::Refused(problem) => (Severity::Error, problem.code, problem.reason),
                };
                Some(LinkProblem {
                    link,
                    severity,
                    code,
                    reason,
                })
            })
            .collect()
    }
}

/// The documents of one load, by identifier, and the nesting limit.
struct Loader<'a> {
    resolver: &'a dyn LinkResolver,
    /// The number of linked levels a load opens below the root: `Limits::nesting_depth`.
    limit: usize,
    documents: BTreeMap<ObjectId, Linked>,
}

impl Loader<'_> {
    /// Computes the link states of the document `document`, which the map holds, and stores
    /// them. A link to a document the load has not opened yet opens and walks it, one level
    /// deeper; a link to a document already in the map reuses that entry.
    fn walk(&mut self, document: ObjectId, depth: usize) {
        let Some(entry) = self.documents.get(&document) else {
            return;
        };
        let snapshot = entry.snapshot.clone();
        let from = entry.location.clone();
        let ids: Vec<ObjectId> = snapshot
            .iter_type(LINK_TYPE)
            .map(|objects| objects.map(|(_, object)| object.id).collect())
            .unwrap_or_default();
        let states = ids
            .into_iter()
            .map(|link| (link, self.state(&snapshot, &from, link, depth)))
            .collect();
        if let Some(entry) = self.documents.get_mut(&document) {
            entry.states = states;
        }
    }

    fn state(
        &mut self,
        snapshot: &Snapshot,
        from: &str,
        link: ObjectId,
        depth: usize,
    ) -> LinkState {
        let record = match record(snapshot, link) {
            Ok(record) => record,
            Err(problem) => return LinkState::Unresolved(problem),
        };
        if record.mode == LoadMode::NotLoaded {
            return LinkState::NotLoaded;
        }
        let target = record.document;
        let known = self.documents.get(&target).map(|entry| entry.content);
        if let Some(Some(content)) = known {
            return Self::settled(&record, target, content);
        }
        if known.is_none() && depth >= self.limit {
            return LinkState::Refused(Problem::new(codes::LINK_TOO_DEEP, "nesting_depth"));
        }
        let opened = match self.resolver.open(from, &record.path) {
            Ok(opened) => opened,
            Err(error) => {
                return LinkState::Unresolved(Problem::new(codes::LINK_UNRESOLVED, error.code()));
            }
        };
        if opened.document != target {
            return LinkState::Unresolved(Problem::new(codes::LINK_UNRESOLVED, "document"));
        }
        let content = opened.content;
        match known {
            // The root document, reached by a link for the first time: its snapshot is the one
            // the load started from, and this link records its content.
            Some(None) => {
                if let Some(entry) = self.documents.get_mut(&target) {
                    entry.content = Some(content);
                }
            }
            _ => {
                let linked = Linked {
                    location: opened.location,
                    content: Some(content),
                    snapshot: opened.snapshot,
                    states: BTreeMap::new(),
                };
                self.documents.insert(target, linked);
                self.walk(target, depth + 1);
            }
        }
        Self::settled(&record, target, content)
    }

    fn settled(record: &Record, target: ObjectId, content: [u8; 32]) -> LinkState {
        if record.content == Some(content) {
            LinkState::Resolved(target)
        } else {
            LinkState::Outdated(target)
        }
    }
}

/// Loads the links of the document `root`, whose identifier is `root_document` and whose caller's
/// name is `location`. Each link's path is opened through `resolver`, and the linked documents'
/// own links are loaded likewise.
///
/// Each document of the load is opened once and walked once: a link to a document already loaded,
/// including the root and any document on the path to the link, reuses that document, and the
/// resolver is not called for it. So a cycle of links loads and terminates; for A links B and B
/// links A, the resolver is called once for B and once for A, and `root_document` is never opened
/// through the resolver twice. A link whose document would be opened beyond `limits.nesting_depth`
/// levels below the root is refused with `doc.link.too_deep`.
#[must_use]
pub fn load(
    root: &Snapshot,
    root_document: ObjectId,
    location: &str,
    resolver: &dyn LinkResolver,
    limits: &Limits,
) -> Links {
    let limit = usize::try_from(limits.nesting_depth).unwrap_or(usize::MAX);
    let mut loader = Loader {
        resolver,
        limit,
        documents: BTreeMap::new(),
    };
    loader.documents.insert(
        root_document,
        Linked {
            location: location.to_owned(),
            content: None,
            snapshot: root.clone(),
            states: BTreeMap::new(),
        },
    );
    loader.walk(root_document, 0);
    Links {
        document: root_document,
        documents: Arc::new(loader.documents),
    }
}

/// Reloads the link object `link` of the transaction's document: opens its path, seen from
/// `from`, through `resolver`, and records the opened document's revision and content hash in
/// the link. The transaction has kind [`TxnKind::LinkReload`]; its changes are field operations,
/// so undo restores the previously recorded state.
///
/// Fails with `doc.link.invalid` for a link object that is malformed or missing,
/// `doc.link.not_loaded` for a link with the load mode `not_loaded`, `doc.link.unresolved` when
/// the path does not open the linked document, and `doc.value.out_of_range` when the revision
/// does not fit a 64-bit signed field. Nothing is recorded then.
pub fn reload(
    txn: &mut Txn,
    link: ObjectId,
    from: &str,
    resolver: &dyn LinkResolver,
) -> Result<()> {
    let record = record(txn.snapshot(), link).map_err(|problem| refusal(link, problem))?;
    if record.mode == LoadMode::NotLoaded {
        return Err(refusal(
            link,
            Problem::new(codes::LINK_NOT_LOADED, "load_mode"),
        ));
    }
    let opened = resolver.open(from, &record.path).map_err(|error| {
        refusal(link, Problem::new(codes::LINK_UNRESOLVED, error.code())).with_source(error)
    })?;
    if opened.document != record.document {
        return Err(refusal(link, Problem::new(codes::LINK_INVALID, "document")));
    }
    let revision = i64::try_from(opened.snapshot.revision())
        .map_err(|_| Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", REVISION))?;
    txn.meta_mut().kind = TxnKind::LinkReload;
    txn.modify(link, FieldPath::new(REVISION), Some(Value::I64(revision)))?;
    txn.modify(
        link,
        FieldPath::new(CONTENT),
        Some(Value::Opaque(opened.content.to_vec())),
    )
}
