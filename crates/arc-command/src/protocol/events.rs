//! Event subscriptions and the selection (CORE-CMD-024, CORE-CMD-025): the state of one
//! session besides its permissions.
//!
//! A [`Hub`] belongs to one session. Notifications (`event.notify`) go through its bounded
//! queue: when a client reads too slowly the oldest notifications are dropped and the next one
//! sent is `events.dropped` with their number. Document events are read from the documents'
//! [`arc_doc::EventBus`] subscriptions by the session's pump thread, and by the worker right
//! before it answers a run, so the changes of a run precede its response.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use arc_base::{Error, ObjectId};
use arc_doc::events::CommittedEvent;
use arc_doc::{Document, Event, Snapshot, Subscription};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::encode::Summary;
use super::run::DocState;
use super::{Answer, Failure, Server, decode, invalid_params, limit, text};
use crate::error::codes;
use crate::spec::Permission;

/// The most notifications a session holds for its client.
pub const EVENT_QUEUE: usize = 256;

/// The most objects a selection holds.
pub const MAX_SELECTED: usize = 10_000;

/// The most sub-object keys of one selected object, and the longest key in bytes.
const MAX_SUB_KEYS: usize = 1024;
const MAX_KEY_BYTES: usize = 256;

/// The most unknown identifiers `selection.set` names in its error.
const MAX_UNKNOWN_LISTED: usize = 1000;

/// The most document events read ahead per document while a preview holds them back.
const MAX_BACKLOG: usize = 1024;

/// The event names a session can subscribe to.
pub const EVENTS: [&str; 3] = ["command.progress", "document.changed", "selection.changed"];

struct Item {
    /// The request a progress notification belongs to: a newer one replaces it in place.
    progress_of: Option<String>,
    line: String,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    dropped: u64,
}

struct DocSub {
    state: Arc<DocState>,
    changes: Option<Subscription>,
    backlog: VecDeque<Event>,
    selection: bool,
}

/// One selected object and its sub-object keys.
#[derive(Clone)]
struct Selected {
    id: ObjectId,
    keys: Vec<String>,
}

pub(super) struct Hub {
    out: SyncSender<String>,
    queue: Mutex<Queue>,
    /// Held while notifications are flushed, so that they keep their order.
    gate: Mutex<()>,
    /// Progress of the session's own runs goes out. On until the client unsubscribes, so
    /// clients that never subscribe keep receiving it.
    progress: AtomicBool,
    subs: Mutex<BTreeMap<String, DocSub>>,
    selection: Mutex<BTreeMap<String, Vec<Selected>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn notification(params: &Value) -> String {
    text(&json!({"jsonrpc": "2.0", "method": "event.notify", "params": params}))
}

pub(super) fn require_read(
    permissions: &BTreeSet<Permission>,
    method: &str,
) -> Result<(), Failure> {
    if permissions.contains(&Permission::Read) {
        Ok(())
    } else {
        Err(Error::new(codes::PERMISSION_DENIED)
            .with_arg("command", method)
            .with_detail("permission", "read")
            .into())
    }
}

#[derive(Deserialize)]
struct SubscribeParams {
    events: Vec<String>,
    #[serde(default)]
    document: Option<String>,
}

#[derive(Deserialize)]
struct DocumentParams {
    document: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectParam {
    id: ObjectId,
    #[serde(default)]
    sub_objects: Vec<String>,
}

#[derive(Deserialize)]
struct SetParams {
    document: String,
    objects: Vec<SelectParam>,
}

impl Hub {
    pub(super) fn new(out: SyncSender<String>) -> Self {
        Self {
            out,
            queue: Mutex::default(),
            gate: Mutex::new(()),
            progress: AtomicBool::new(true),
            subs: Mutex::default(),
            selection: Mutex::default(),
        }
    }

    fn push(&self, line: String, progress_of: Option<String>) {
        let mut queue = lock(&self.queue);
        if let (Some(key), Some(last)) = (&progress_of, queue.items.back_mut())
            && last.progress_of.as_ref() == Some(key)
        {
            last.line = line;
            return;
        }
        if queue.items.len() >= EVENT_QUEUE {
            queue.items.pop_front();
            queue.dropped = queue.dropped.saturating_add(1);
        }
        queue.items.push_back(Item { progress_of, line });
    }

    /// Queues one `event.notify` with `params`.
    fn notify(&self, params: &Value) {
        self.push(notification(params), None);
    }

    /// Queues the progress of the run `id`, if the session takes progress.
    pub(super) fn progress(&self, id: &Value, fraction: f64, message_id: &str) {
        if self.progress.load(Ordering::Acquire) {
            let params = json!({
                "event": "command.progress",
                "requestId": id,
                "fraction": fraction,
                "messageId": message_id,
            });
            self.push(notification(&params), Some(text(id)));
        }
    }

    /// Sends what is queued, oldest first and the drop notice before the rest, and returns
    /// whether anything went out. A `blocking` flush waits for the output; the other leaves
    /// what does not fit queued.
    fn flush(&self, blocking: bool) -> bool {
        let mut sent = false;
        loop {
            let (line, restore) = {
                let mut queue = lock(&self.queue);
                if queue.dropped > 0 {
                    let dropped = std::mem::take(&mut queue.dropped);
                    let params = json!({"event": "events.dropped", "dropped": dropped});
                    (notification(&params), Err(dropped))
                } else if let Some(item) = queue.items.pop_front() {
                    (item.line.clone(), Ok(item))
                } else {
                    return sent;
                }
            };
            let failed = if blocking {
                self.out.send(line).err().map(|e| e.0)
            } else {
                match self.out.try_send(line) {
                    Ok(()) => None,
                    Err(TrySendError::Full(line)) => Some(line),
                    Err(TrySendError::Disconnected(_)) => {
                        // The session is over: drop everything.
                        lock(&self.queue).items.clear();
                        return sent;
                    }
                }
            };
            if failed.is_some() {
                let mut queue = lock(&self.queue);
                match restore {
                    Ok(item) => queue.items.push_front(item),
                    Err(dropped) => queue.dropped = queue.dropped.saturating_add(dropped),
                }
                return sent;
            }
            sent = true;
        }
    }

    /// Moves the document events of the subscriptions into the queue (those of a document
    /// with a preview in flight wait) and flushes. Returns whether anything happened. A
    /// non-blocking pump gives way when another pump is flushing.
    pub(super) fn pump(&self, blocking: bool) -> bool {
        let _gate = if blocking {
            lock(&self.gate)
        } else if let Ok(gate) = self.gate.try_lock() {
            gate
        } else {
            return false;
        };
        let moved = self.collect();
        self.flush(blocking) || moved
    }

    fn collect(&self) -> bool {
        let mut moved = false;
        let mut subs = lock(&self.subs);
        for (document, sub) in subs.iter_mut() {
            if let Some(changes) = &sub.changes {
                while sub.backlog.len() < MAX_BACKLOG
                    && let Some(event) = changes.try_recv()
                {
                    sub.backlog.push_back(event);
                }
            }
            while let Some(event) = sub.backlog.front() {
                if let Event::Committed(committed) = event {
                    if sub.state.previewing() {
                        break;
                    }
                    // A coalesced event cannot be told apart from its previews' share.
                    if committed.commits > 1 || !sub.state.is_hidden(committed.txn_id) {
                        self.notify(&document_changed(document, committed));
                        moved = true;
                    }
                }
                sub.backlog.pop_front();
            }
        }
        moved
    }

    /// The live selected identifiers of `document`, in selection order.
    pub(super) fn selection_ids(&self, document: &str, snapshot: &Snapshot) -> Vec<ObjectId> {
        lock(&self.selection)
            .get(document)
            .map(|list| {
                list.iter()
                    .map(|s| s.id)
                    .filter(|id| snapshot.resolve(*id).is_ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Replaces the selection of `document` by `ids` (the live ones) after a command, and
    /// reports it.
    pub(super) fn selection_after(&self, document: &str, snapshot: &Snapshot, ids: Vec<ObjectId>) {
        let list: Vec<Selected> = ids
            .into_iter()
            .filter(|id| snapshot.resolve(*id).is_ok())
            .take(MAX_SELECTED)
            .map(|id| Selected {
                id,
                keys: Vec::new(),
            })
            .collect();
        self.store_selection(document, snapshot.revision(), list, "command");
    }

    fn store_selection(&self, document: &str, revision: u64, list: Vec<Selected>, source: &str) {
        let objects = objects_json(&list);
        lock(&self.selection).insert(document.to_owned(), list);
        if lock(&self.subs)
            .get(document)
            .is_some_and(|sub| sub.selection)
        {
            self.notify(&json!({
                "event": "selection.changed",
                "document": document,
                "revision": revision,
                "source": source,
                "objects": objects,
            }));
        }
    }

    fn subscriptions(&self) -> Value {
        let mut all = BTreeSet::new();
        if self.progress.load(Ordering::Acquire) {
            all.insert(("command.progress", String::new()));
        }
        for (document, sub) in lock(&self.subs).iter() {
            if sub.changes.is_some() {
                all.insert(("document.changed", document.clone()));
            }
            if sub.selection {
                all.insert(("selection.changed", document.clone()));
            }
        }
        let list: Vec<Value> = all
            .into_iter()
            .map(|(event, document)| {
                if document.is_empty() {
                    json!({"event": event})
                } else {
                    json!({"event": event, "document": document})
                }
            })
            .collect();
        json!({"subscriptions": list})
    }

    /// `event.subscribe` (`on`) and `event.unsubscribe` of the named events, for one document
    /// or for every document; the answer lists the session's subscriptions.
    pub(super) fn subscribe(
        &self,
        server: &Server<'_>,
        permissions: &BTreeSet<Permission>,
        method: &str,
        params: Map<String, Value>,
        on: bool,
    ) -> Answer {
        require_read(permissions, method)?;
        let p: SubscribeParams = decode(method, params)?;
        if p.events.is_empty()
            || p.events.len() > EVENTS.len()
            || p.events.iter().any(|e| !EVENTS.contains(&e.as_str()))
        {
            return Err(invalid_params(method, "events"));
        }
        let documents: Vec<(&String, &Document)> = match &p.document {
            Some(id) => {
                server.document(id)?;
                server.documents.get_key_value(id).into_iter().collect()
            }
            None => server.documents.iter().collect(),
        };
        for event in &p.events {
            match event.as_str() {
                "command.progress" => self.progress.store(on, Ordering::Release),
                kind => {
                    let mut subs = lock(&self.subs);
                    for (id, document) in &documents {
                        let Some(state) = server.states.get(*id) else {
                            continue;
                        };
                        let sub = subs.entry((*id).clone()).or_insert_with(|| DocSub {
                            state: Arc::clone(state),
                            changes: None,
                            backlog: VecDeque::new(),
                            selection: false,
                        });
                        if kind == "document.changed" {
                            sub.changes = on.then(|| document.subscribe());
                            sub.backlog.clear();
                        } else {
                            sub.selection = on;
                        }
                    }
                    subs.retain(|_, sub| sub.changes.is_some() || sub.selection);
                }
            }
        }
        Ok(self.subscriptions())
    }

    /// `selection.get`: the live selection of one document.
    pub(super) fn selection_get(
        &self,
        server: &Server<'_>,
        permissions: &BTreeSet<Permission>,
        params: Map<String, Value>,
    ) -> Answer {
        const METHOD: &str = "selection.get";
        require_read(permissions, METHOD)?;
        let p: DocumentParams = decode(METHOD, params)?;
        let snapshot = server.document(&p.document)?.snapshot();
        let mut all = lock(&self.selection);
        let list = all.entry(p.document.clone()).or_default();
        list.retain(|s| snapshot.resolve(s.id).is_ok());
        Ok(selection_json(&p.document, snapshot.revision(), list))
    }

    /// `selection.set`: replaces the selection of one document; every identifier must name an
    /// object of the latest revision.
    pub(super) fn selection_set(
        &self,
        server: &Server<'_>,
        permissions: &BTreeSet<Permission>,
        params: Map<String, Value>,
    ) -> Answer {
        const METHOD: &str = "selection.set";
        require_read(permissions, METHOD)?;
        let p: SetParams = decode(METHOD, params)?;
        let snapshot = server.document(&p.document)?.snapshot();
        if p.objects.len() > MAX_SELECTED {
            return Err(limit("selection"));
        }
        let mut list: Vec<Selected> = Vec::new();
        let mut unknown = Vec::new();
        for object in p.objects {
            if object.sub_objects.len() > MAX_SUB_KEYS
                || object.sub_objects.iter().any(|k| k.len() > MAX_KEY_BYTES)
            {
                return Err(limit("sub_objects"));
            }
            if snapshot.resolve(object.id).is_err() {
                unknown.push(object.id);
                continue;
            }
            match list.iter_mut().find(|s| s.id == object.id) {
                Some(known) => {
                    for key in object.sub_objects {
                        if !known.keys.contains(&key) {
                            known.keys.push(key);
                        }
                    }
                }
                None => list.push(Selected {
                    id: object.id,
                    keys: dedup(object.sub_objects),
                }),
            }
        }
        if !unknown.is_empty() {
            let mut extra = Map::new();
            let listed: Vec<String> = unknown
                .iter()
                .take(MAX_UNKNOWN_LISTED)
                .map(ToString::to_string)
                .collect();
            extra.insert("unknown".into(), json!(listed));
            let count = i64::try_from(unknown.len()).unwrap_or(i64::MAX);
            return Err(Failure(Box::new((
                Error::new(codes::SELECTION_UNKNOWN).with_arg("count", count),
                extra,
            ))));
        }
        let answer = selection_json(&p.document, snapshot.revision(), &list);
        self.store_selection(&p.document, snapshot.revision(), list, "client");
        Ok(answer)
    }
}

fn dedup(keys: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    keys.into_iter()
        .filter(|k| seen.insert(k.clone()))
        .collect()
}

fn objects_json(list: &[Selected]) -> Vec<Value> {
    list.iter()
        .map(|s| {
            if s.keys.is_empty() {
                json!({"id": s.id.to_string()})
            } else {
                json!({"id": s.id.to_string(), "subObjects": s.keys})
            }
        })
        .collect()
}

fn selection_json(document: &str, revision: u64, list: &[Selected]) -> Value {
    json!({"document": document, "revision": revision, "objects": objects_json(list)})
}

fn document_changed(document: &str, event: &CommittedEvent) -> Value {
    json!({
        "event": "document.changed",
        "document": document,
        "revision": event.revision,
        "transaction": event.txn_id.to_string(),
        "commits": event.commits,
        "changes": Summary::of_changes(&event.changes).json(),
    })
}
