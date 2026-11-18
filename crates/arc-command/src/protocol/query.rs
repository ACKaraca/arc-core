//! Query methods (CORE-CMD-023): `query.objects`, `query.references`, `query.diagnostics` and
//! `query.history`. Every answer names the revision of the snapshot it read; a page cursor
//! encodes that revision, the position and a digest of the query, and a cursor of another
//! revision or query is refused.

use std::collections::BTreeSet;
use std::sync::Arc;

use arc_base::{Error, Limits, ObjectId};
use arc_doc::relationship::references;
use arc_doc::txn::TxnKind;
use arc_doc::{Handle, Object, PathSegment, Schema, Snapshot, Value as Field, ValueType};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::encode::{Summary, diagnostic_json};
use super::events::require_read;
use super::{Answer, Failure, Server, decode, invalid_params, limit};
use crate::context::Session;
use crate::error::codes;

/// The page size when a query names none.
const DEFAULT_PAGE: usize = 100;

/// The largest page.
pub const MAX_PAGE: usize = 1000;

/// The longest field path in bytes and in segments.
const MAX_PATH_BYTES: usize = 256;
const MAX_PATH_SEGMENTS: usize = 16;

/// The page size a session may ask for: [`MAX_PAGE`] or its array limit, whichever is smaller.
pub(super) fn max_page(limits: &Limits) -> usize {
    usize::try_from(limits.array_length)
        .unwrap_or(usize::MAX)
        .clamp(1, MAX_PAGE)
}

/// Where a page starts and how long it is.
struct Page {
    offset: usize,
    size: usize,
    revision: u64,
    version: u64,
    digest: u64,
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl Page {
    /// The page of a query with `params`, read at `revision` and `version` (a second counter
    /// of the source, 0 when it has none).
    fn new(
        method: &str,
        params: &Map<String, Value>,
        cursor: Option<&str>,
        limit_param: Option<u64>,
        limits: &Limits,
        revision: u64,
        version: u64,
    ) -> Result<Self, Failure> {
        let max = max_page(limits);
        let size = match limit_param {
            None => DEFAULT_PAGE.min(max),
            Some(0) => return Err(invalid_params(method, "limit")),
            Some(n) => match usize::try_from(n) {
                Ok(n) if n <= max => n,
                _ => return Err(limit("page_size")),
            },
        };
        // The query's identity: its method and every parameter but the page's own.
        let rest: Map<String, Value> = params
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "cursor" | "limit"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let digest = fnv(&json!([method, rest]).to_string());
        let mut page = Self {
            offset: 0,
            size,
            revision,
            version,
            digest,
        };
        if let Some(text) = cursor {
            let invalid = || Failure::from(Error::new(codes::QUERY_CURSOR_INVALID));
            let part = |i: usize| {
                text.get(i * 16..(i + 1) * 16)
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            };
            if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid());
            }
            let (Some(rev), Some(ver), Some(offset), Some(sent)) =
                (part(0), part(1), part(2), part(3))
            else {
                return Err(invalid());
            };
            if sent != digest {
                return Err(invalid());
            }
            if (rev, ver) != (revision, version) {
                let as_int = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
                return Err(Error::new(codes::QUERY_CURSOR_STALE)
                    .with_arg("expected", as_int(rev))
                    .with_detail("expected", as_int(rev))
                    .with_detail("current", as_int(revision))
                    .into());
            }
            page.offset = usize::try_from(offset).map_err(|_| invalid())?;
        }
        Ok(page)
    }

    /// Takes the page out of `items` (the whole sequence, lazily) and the cursor of the next.
    fn take<T>(&self, items: impl Iterator<Item = T>) -> (Vec<T>, Value) {
        // shortcut: the offset is skipped by walking, O(offset) per page; resume by slot if
        // documents beyond a million matching objects are paged through.
        let mut page: Vec<T> = items
            .skip(self.offset)
            .take(self.size.saturating_add(1))
            .collect();
        let more = page.len() > self.size;
        page.truncate(self.size);
        let next = more.then(|| {
            format!(
                "{:016x}{:016x}{:016x}{:016x}",
                self.revision,
                self.version,
                self.offset.saturating_add(self.size),
                self.digest
            )
        });
        (page, json!(next))
    }
}

#[derive(Deserialize)]
struct ObjectsParams {
    document: String,
    #[serde(default)]
    types: Option<Vec<String>>,
    #[serde(default)]
    ids: Option<Vec<ObjectId>>,
    #[serde(default)]
    owner: Option<ObjectId>,
    #[serde(default)]
    fields: Option<Vec<String>>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Deserialize)]
struct ReferencesParams {
    document: String,
    id: ObjectId,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Deserialize)]
struct ListParams {
    document: String,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u64>,
}

/// Parses a field path as [`arc_doc::FieldPath`] prints it: `name`, `.name`, `[index]` and
/// `["key"]` steps.
fn parse_path(text: &str) -> Option<Vec<PathSegment>> {
    if text.len() > MAX_PATH_BYTES {
        return None;
    }
    let mut segments = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            if after.starts_with('"') {
                // A JSON string; the stream reader tells where it ends.
                let mut stream = serde_json::Deserializer::from_str(after).into_iter::<String>();
                let key = stream.next()?.ok()?;
                rest = after.get(stream.byte_offset()..)?.strip_prefix(']')?;
                segments.push(PathSegment::Key(key));
            } else {
                let (digits, tail) = after.split_once(']')?;
                segments.push(PathSegment::Index(digits.parse().ok()?));
                rest = tail;
            }
        } else {
            let after = if segments.is_empty() {
                rest
            } else {
                rest.strip_prefix('.')?
            };
            let end = after.find(['.', '[']).unwrap_or(after.len());
            let name = after.get(..end).filter(|n| !n.is_empty())?;
            segments.push(PathSegment::Field(name.to_owned()));
            rest = after.get(end..)?;
        }
        if segments.len() > MAX_PATH_SEGMENTS {
            return None;
        }
    }
    matches!(segments.first(), Some(PathSegment::Field(_))).then_some(segments)
}

/// The value at `path` in `values`, the field values of an object of `schema` at its minor
/// version `minor`.
fn value_at<'a>(
    schema: &'a Schema,
    minor: u32,
    values: &'a [Option<Field>],
    path: &'a [PathSegment],
) -> Option<&'a Field> {
    let (PathSegment::Field(name), steps) = path.split_first()? else {
        return None;
    };
    let index = schema
        .fields_at(minor)
        .iter()
        .position(|f| f.name == *name)?;
    let mut ty = &schema.fields.get(index)?.value_type;
    let mut value = values.get(index)?.as_ref()?;
    for step in steps {
        (ty, value) = match (ty, value, step) {
            (ValueType::Record(fields), Field::Record(items), PathSegment::Field(name)) => {
                let i = fields.iter().position(|f| f.name == *name)?;
                (&fields.get(i)?.value_type, items.get(i)?.as_ref()?)
            }
            (ValueType::List(element), Field::List(items), PathSegment::Index(i)) => {
                (element.as_ref(), items.get(usize::try_from(*i).ok()?)?)
            }
            (ValueType::Map(element), Field::Map(map), PathSegment::Key(key)) => {
                (element.as_ref(), map.get(key)?)
            }
            _ => return None,
        };
    }
    Some(value)
}

/// The canonical JSON of a field value: the serialization `arc-doc` defines for values (the
/// one the package inspection prints).
fn field_json(value: Option<&Field>) -> Value {
    value
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or(Value::Null)
}

/// The members of one object: header, and the requested field values (all top-level fields
/// when `paths` is `None`).
fn object_json(
    snapshot: &Snapshot,
    handle: Handle,
    object: &Object,
    paths: Option<&[(String, Vec<PathSegment>)]>,
) -> Value {
    let key = snapshot.type_key(handle).map(ToString::to_string);
    let schema = key
        .as_deref()
        .and_then(|key| snapshot.registry().get(key))
        .and_then(|decl| decl.schema(object.version.major));
    let mut fields = Map::new();
    if let Some(schema) = schema {
        match paths {
            None => {
                for (decl, value) in schema
                    .fields_at(object.version.minor)
                    .iter()
                    .zip(&object.values)
                {
                    fields.insert(decl.name.clone(), field_json(value.as_ref()));
                }
            }
            Some(paths) => {
                for (text, path) in paths {
                    let value = value_at(schema, object.version.minor, &object.values, path);
                    fields.insert(text.clone(), field_json(value));
                }
            }
        }
    }
    let mut entry = Map::new();
    entry.insert("id".into(), json!(object.id.to_string()));
    entry.insert("type".into(), json!(key));
    if let Some(owner) = object.header.owner {
        entry.insert("owner".into(), json!(owner.to_string()));
    }
    if let Some(name) = &object.header.name {
        entry.insert("name".into(), json!(name));
    }
    if let Some(partition) = &object.header.partition {
        entry.insert("partition".into(), json!(partition));
    }
    entry.insert("fields".into(), Value::Object(fields));
    Value::Object(entry)
}

fn check_len(items: usize, limits: &Limits) -> Result<(), Failure> {
    if u64::try_from(items).is_ok_and(|n| n <= limits.array_length) {
        Ok(())
    } else {
        Err(limit("array_length"))
    }
}

impl Server<'_> {
    /// Answers a query method; permission class `read` is required.
    pub(super) fn read_method(
        &self,
        session: &Session,
        method: &str,
        params: Map<String, Value>,
    ) -> Answer {
        require_read(&session.permissions, method)?;
        match method {
            "query.objects" => self.query_objects(session, params),
            "query.references" => self.query_references(session, params),
            "query.diagnostics" => self.query_diagnostics(session, params),
            _ => self.query_history(session, params),
        }
    }

    fn query_objects(&self, session: &Session, params: Map<String, Value>) -> Answer {
        const METHOD: &str = "query.objects";
        let page_params = params.clone();
        let p: ObjectsParams = decode(METHOD, params)?;
        let limits = &session.limits;
        for len in [
            p.types.as_ref().map(Vec::len),
            p.ids.as_ref().map(Vec::len),
            p.fields.as_ref().map(Vec::len),
        ]
        .into_iter()
        .flatten()
        {
            check_len(len, limits)?;
        }
        let paths: Option<Vec<(String, Vec<PathSegment>)>> = p
            .fields
            .map(|fields| {
                fields
                    .into_iter()
                    .map(|text| {
                        parse_path(&text)
                            .map(|path| (text, path))
                            .ok_or_else(|| invalid_params(METHOD, "fields"))
                    })
                    .collect()
            })
            .transpose()?;
        let document = self.document(&p.document)?;
        let snapshot = document.snapshot();
        let page = Page::new(
            METHOD,
            &page_params,
            p.cursor.as_deref(),
            p.limit,
            limits,
            snapshot.revision(),
            0,
        )?;
        let types: Option<BTreeSet<String>> = p.types.map(|t| t.into_iter().collect());
        let owned = |object: &Arc<Object>| p.owner.is_none_or(|o| object.header.owner == Some(o));
        let of_type = |handle: Handle| {
            types.as_ref().is_none_or(|types| {
                snapshot
                    .type_key(handle)
                    .is_some_and(|key| types.contains(key.as_str()))
            })
        };
        // An identifier named twice is visited once.
        let ids = p.ids.as_ref().map(|ids| {
            let mut seen = BTreeSet::new();
            ids.iter()
                .copied()
                .filter(|id| seen.insert(*id))
                .collect::<Vec<_>>()
        });
        let found: Box<dyn Iterator<Item = (Handle, &Arc<Object>)> + '_> = match (&ids, &types) {
            (Some(ids), _) => Box::new(ids.iter().filter_map(|id| {
                let handle = snapshot.resolve(*id).ok()?;
                Some((handle, snapshot.get(handle).ok()?))
            })),
            (None, Some(types)) => {
                let mut per_type = Vec::new();
                for key in types {
                    per_type.push(snapshot.iter_type(key)?);
                }
                Box::new(per_type.into_iter().flatten())
            }
            (None, None) => Box::new(snapshot.iter()),
        };
        let matching = found.filter(|(handle, object)| of_type(*handle) && owned(object));
        let (objects, next) = page.take(matching);
        let objects: Vec<Value> = objects
            .into_iter()
            .map(|(handle, object)| object_json(&snapshot, handle, object, paths.as_deref()))
            .collect();
        Ok(json!({
            "document": p.document,
            "revision": snapshot.revision(),
            "objects": objects,
            "nextCursor": next,
        }))
    }

    fn query_references(&self, session: &Session, params: Map<String, Value>) -> Answer {
        const METHOD: &str = "query.references";
        let page_params = params.clone();
        let p: ReferencesParams = decode(METHOD, params)?;
        let (outgoing, incoming) = match p.direction.as_deref() {
            None | Some("both") => (true, true),
            Some("outgoing") => (true, false),
            Some("incoming") => (false, true),
            Some(_) => return Err(invalid_params(METHOD, "direction")),
        };
        let snapshot = self.document(&p.document)?.snapshot();
        let page = Page::new(
            METHOD,
            &page_params,
            p.cursor.as_deref(),
            p.limit,
            &session.limits,
            snapshot.revision(),
            0,
        )?;
        let handle = snapshot.resolve(p.id)?;
        let object = snapshot.get(handle)?;
        let key = snapshot
            .type_key(handle)
            .map(ToString::to_string)
            .unwrap_or_default();
        let entry = |direction: &str, other: ObjectId, field: String| {
            let kind = snapshot
                .resolve(other)
                .ok()
                .and_then(|h| snapshot.type_key(h))
                .map(ToString::to_string);
            json!({"direction": direction, "object": other.to_string(), "field": field, "type": kind})
        };
        let out = outgoing
            .then(|| references(snapshot.registry(), &key, object))
            .into_iter()
            .flatten()
            .map(|r| entry("outgoing", r.target, r.field.to_string()));
        let into = incoming
            .then(|| snapshot.referrers(p.id))
            .into_iter()
            .flatten()
            .map(|r| entry("incoming", r.source, r.field.to_string()));
        let (references, next) = page.take(out.chain(into));
        Ok(json!({
            "document": p.document,
            "revision": snapshot.revision(),
            "id": p.id.to_string(),
            "references": references,
            "nextCursor": next,
        }))
    }

    fn query_diagnostics(&self, session: &Session, params: Map<String, Value>) -> Answer {
        const METHOD: &str = "query.diagnostics";
        let page_params = params.clone();
        let p: ListParams = decode(METHOD, params)?;
        let severity = match p.severity.as_deref() {
            None => None,
            Some(s @ ("error" | "warning")) => Some(s),
            Some(_) => return Err(invalid_params(METHOD, "severity")),
        };
        let document = self.document(&p.document)?;
        let revision = document.snapshot().revision();
        let list = document.diagnostics_snapshot();
        let page = Page::new(
            METHOD,
            &page_params,
            p.cursor.as_deref(),
            p.limit,
            &session.limits,
            revision,
            list.version,
        )?;
        let matching = list
            .diagnostics
            .iter()
            .map(diagnostic_json)
            .filter(|d| severity.is_none_or(|s| d["severity"] == s));
        let (diagnostics, next) = page.take(matching);
        Ok(json!({
            "document": p.document,
            "revision": revision,
            "diagnosticsVersion": list.version,
            "dropped": list.dropped,
            "diagnostics": diagnostics,
            "nextCursor": next,
        }))
    }

    fn query_history(&self, session: &Session, params: Map<String, Value>) -> Answer {
        const METHOD: &str = "query.history";
        let page_params = params.clone();
        let p: ListParams = decode(METHOD, params)?;
        let document = self.document(&p.document)?;
        let revision = document.snapshot().revision();
        let page = Page::new(
            METHOD,
            &page_params,
            p.cursor.as_deref(),
            p.limit,
            &session.limits,
            revision,
            0,
        )?;
        // The journal may already hold a transaction newer than the snapshot read above.
        let (transactions, next) = document.with_journal(|journal| {
            page.take(
                journal
                    .iter()
                    .take_while(|entry| entry.revision() <= revision)
                    .map(|entry| {
                        let record = entry.record();
                        let field = |text: &str, name: &str| {
                            serde_json::from_str::<Value>(text).ok().and_then(|v| {
                                v.get(name).and_then(Value::as_str).map(str::to_owned)
                            })
                        };
                        let (kind, target) = match record.kind {
                            TxnKind::Normal => ("normal", None),
                            TxnKind::UndoOf(t) => ("undo_of", Some(t.to_string())),
                            TxnKind::RedoOf(t) => ("redo_of", Some(t.to_string())),
                            TxnKind::Merge => ("merge", None),
                            TxnKind::Migration => ("migration", None),
                            TxnKind::LinkReload => ("link_reload", None),
                        };
                        let summary = Summary::of_ops(&record.ops).json();
                        json!({
                            "id": record.txn_id.to_string(),
                            "revision": entry.revision(),
                            "command": field(&record.command, "command"),
                            "label": field(&record.label, "message_id"),
                            "time": record.time_unix_millis,
                            "kind": kind,
                            "target": target,
                            "group": record.group,
                            "counts": summary["counts"],
                        })
                    }),
            )
        });
        Ok(json!({
            "document": p.document,
            "revision": revision,
            "transactions": transactions,
            "nextCursor": next,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_paths_parse_as_they_print() {
        use arc_doc::FieldPath;
        let paths = [
            FieldPath::new("layers").index(2).field("tags").index(0),
            FieldPath::new("costs").key("a \"b\" ]"),
            FieldPath::new("flag"),
            FieldPath::new("a").key("").index(7),
        ];
        for path in paths {
            let parsed = parse_path(&path.to_string());
            assert_eq!(parsed.as_deref(), Some(path.segments()), "{path}");
        }
        for bad in [
            "",
            ".a",
            "a.",
            "a..b",
            "[0]",
            "a[",
            "a[x]",
            "a[-1]",
            "a[\"k\"",
            "a[\"k\"]x",
            "a b.",
            "a[\"k\"]]",
        ] {
            // "a b." ends in a dot, which names no field.
            assert_eq!(parse_path(bad), None, "{bad:?}");
        }
        assert_eq!(parse_path(&"a".repeat(MAX_PATH_BYTES + 1)), None);
        assert_eq!(
            parse_path(&format!("a{}", "[0]".repeat(MAX_PATH_SEGMENTS))),
            None
        );
    }

    #[test]
    fn page_cursors_round_trip_and_refuse_other_queries() {
        let limits = Limits::DEFAULT;
        let params: Map<String, Value> = json!({"document": "d", "types": ["a"]})
            .as_object()
            .cloned()
            .unwrap_or_default();
        let page = Page::new("query.objects", &params, None, None, &limits, 7, 0)
            .unwrap_or_else(|_| unreachable_page());
        let (_, next) = page.take(0..1000);
        let Some(cursor) = next.as_str() else {
            panic!("no cursor");
        };
        let again = Page::new("query.objects", &params, Some(cursor), None, &limits, 7, 0);
        assert_eq!(again.map(|p| p.offset).ok(), Some(DEFAULT_PAGE));
        let other: Map<String, Value> = json!({"document": "d"})
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (params, revision, code) in [
            (&other, 7, "cmd.query.cursor_invalid"),
            (&params, 8, "cmd.query.cursor_stale"),
        ] {
            let refused = Page::new(
                "query.objects",
                params,
                Some(cursor),
                None,
                &limits,
                revision,
                0,
            );
            let code_of = |r: Result<Page, Failure>| r.err().map(|f| f.0.0.code());
            assert_eq!(code_of(refused), Some(code));
        }
    }

    fn unreachable_page() -> Page {
        panic!("page")
    }
}
