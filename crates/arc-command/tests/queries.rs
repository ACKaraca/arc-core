//! Query methods (CORE-CMD-023) against a reference model: random documents of nodes and edges
//! (references, owners), every filter, projection and page size compared with the model that
//! the test keeps alongside the document.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "tests"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_command::schema::{check_schema, validate};
use arc_command::{NoLog, Permission, Registry, Server, ServerConfig};
use arc_doc::graph::Evaluators;
use arc_doc::{
    Cardinality, Document, FieldDecl, FieldPath, Header, OnCopy, OnDelete, ReferenceSpec,
    RelationshipRule, Schema, SchemaVersion, Strength, TypeDecl, TypeKey, TypeRegistry, Value,
    ValueType,
};
use proptest::prelude::*;
use serde_json::{Map, json};

const RESPONSES: &str = include_str!("../../../schemas/automation/responses.json");

const NODE: &str = "q.node";
const EDGE: &str = "q.edge";

fn reference(cardinality: Cardinality) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new(NODE).unwrap()],
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality,
        },
    })
}

fn types() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    let version = SchemaVersion::new(1, 0);
    let node = Schema::new(
        version,
        vec![
            FieldDecl::new("label", ValueType::String),
            FieldDecl::new("n", ValueType::I64),
            FieldDecl::new("tags", ValueType::List(Box::new(ValueType::String))),
        ],
    );
    let edge = Schema::new(
        version,
        vec![
            FieldDecl::new("to", reference(Cardinality::Optional)).optional(),
            FieldDecl::new(
                "peers",
                ValueType::List(Box::new(reference(Cardinality::Many { max: 4 }))),
            ),
        ],
    );
    registry
        .register(TypeDecl::new(TypeKey::new(NODE).unwrap(), node))
        .unwrap();
    registry
        .register(TypeDecl::new(TypeKey::new(EDGE).unwrap(), edge))
        .unwrap();
    Arc::new(registry)
}

/// What the test knows about one object.
#[derive(Debug, Clone)]
struct Model {
    node: bool,
    owner: Option<ObjectId>,
    label: String,
    n: i64,
    tags: Vec<String>,
    to: Option<ObjectId>,
    peers: Vec<ObjectId>,
}

#[derive(Debug, Clone)]
enum Op {
    Node(String, i64, Vec<String>, Option<usize>),
    Edge(Option<usize>, Vec<usize>),
    SetN(usize, i64),
    DeleteEdge(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => ("[a-c]{0,3}", -5..5_i64, prop::collection::vec("[xy]{0,2}", 0..3), prop::option::of(any::<usize>()))
            .prop_map(|(l, n, t, o)| Op::Node(l, n, t, o)),
        3 => (prop::option::of(any::<usize>()), prop::collection::vec(any::<usize>(), 0..4))
            .prop_map(|(t, p)| Op::Edge(t, p)),
        1 => (any::<usize>(), -5..5_i64).prop_map(|(i, n)| Op::SetN(i, n)),
        1 => any::<usize>().prop_map(Op::DeleteEdge),
    ]
}

fn pick(ids: &[ObjectId], i: usize) -> Option<ObjectId> {
    (!ids.is_empty()).then(|| ids[i % ids.len()])
}

/// Applies `ops` to a new document and to the model.
fn build(ops: &[Op]) -> (Document, BTreeMap<ObjectId, Model>) {
    let document = Document::new(types(), Arc::new(Evaluators::new())).unwrap();
    let mut model: BTreeMap<ObjectId, Model> = BTreeMap::new();
    let mut next = 0x0190_f3c4_0a11_7c2e_8f00_5d6e_7f8a_0000_u128;
    for op in ops {
        let nodes: Vec<ObjectId> = model
            .iter()
            .filter(|(_, m)| m.node)
            .map(|(id, _)| *id)
            .collect();
        let edges: Vec<ObjectId> = model
            .iter()
            .filter(|(_, m)| !m.node)
            .map(|(id, _)| *id)
            .collect();
        next += 0x9e37;
        let id = ObjectId::from_u128(next);
        let blank = Model {
            node: true,
            owner: None,
            label: String::new(),
            n: 0,
            tags: Vec::new(),
            to: None,
            peers: Vec::new(),
        };
        match op {
            Op::Node(label, n, tags, owner) => {
                let owner = owner.and_then(|i| pick(&nodes, i));
                let values = vec![
                    Some(Value::String(label.clone())),
                    Some(Value::I64(*n)),
                    Some(Value::List(
                        tags.iter().cloned().map(Value::String).collect(),
                    )),
                ];
                document
                    .submit(move |txn| {
                        txn.create(
                            NODE,
                            id,
                            Header {
                                owner,
                                ..Header::default()
                            },
                            values,
                        )?;
                        Ok(())
                    })
                    .wait()
                    .unwrap();
                model.insert(
                    id,
                    Model {
                        owner,
                        label: label.clone(),
                        n: *n,
                        tags: tags.clone(),
                        ..blank
                    },
                );
            }
            Op::Edge(to, peers) => {
                let to = to.and_then(|i| pick(&nodes, i));
                let peers: Vec<ObjectId> = peers.iter().filter_map(|i| pick(&nodes, *i)).collect();
                let values = vec![
                    to.map(Value::Reference),
                    Some(Value::List(
                        peers.iter().copied().map(Value::Reference).collect(),
                    )),
                ];
                document
                    .submit(move |txn| {
                        txn.create(EDGE, id, Header::default(), values)?;
                        Ok(())
                    })
                    .wait()
                    .unwrap();
                model.insert(
                    id,
                    Model {
                        node: false,
                        to,
                        peers,
                        ..blank
                    },
                );
            }
            Op::SetN(i, n) => {
                let Some(target) = pick(&nodes, *i) else {
                    continue;
                };
                let n = *n;
                document
                    .submit(move |txn| txn.modify(target, FieldPath::new("n"), Some(Value::I64(n))))
                    .wait()
                    .unwrap();
                model.get_mut(&target).unwrap().n = n;
            }
            Op::DeleteEdge(i) => {
                let Some(target) = pick(&edges, *i) else {
                    continue;
                };
                document
                    .submit(move |txn| txn.delete(target))
                    .wait()
                    .unwrap();
                model.remove(&target);
            }
        }
    }
    (document, model)
}

fn schema() -> serde_json::Value {
    let mut value: serde_json::Value = serde_json::from_str(RESPONSES).unwrap();
    value.as_object_mut().unwrap().remove("$id");
    check_schema(&value).unwrap();
    value
}

struct Harness<'a> {
    server: &'a Server<'a>,
    schema: serde_json::Value,
}

impl Harness<'_> {
    fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, serde_json::Value> {
        let result = self.server.call(method, params);
        if let Ok(result) = &result {
            let root = json!({"$defs": self.schema["$defs"], "$ref": format!("#/$defs/{method}")});
            let violations = validate(&root, result).unwrap();
            assert!(
                violations.is_empty(),
                "{method}: {violations:?} in {result}"
            );
        }
        result
    }

    /// Every page of a query with `params` (page size `limit`), and the revision they name.
    fn pages(
        &self,
        method: &str,
        key: &str,
        mut params: serde_json::Value,
        limit: usize,
    ) -> (Vec<serde_json::Value>, u64) {
        params["limit"] = json!(limit);
        let (mut all, mut revision) = (Vec::new(), None);
        for _ in 0..2000 {
            let page = self.call(method, params.clone()).unwrap();
            let items = page[key].as_array().unwrap();
            assert!(
                items.len() <= limit,
                "a page of {} for limit {limit}",
                items.len()
            );
            assert_eq!(
                *revision.get_or_insert(page["revision"].as_u64().unwrap()),
                page["revision"]
            );
            all.extend(items.iter().cloned());
            match page["nextCursor"].as_str() {
                Some(cursor) => {
                    assert_eq!(items.len(), limit, "only the last page is short");
                    params["cursor"] = json!(cursor);
                }
                None => return (all, revision.unwrap()),
            }
        }
        panic!("the cursor never ended");
    }
}

fn ids_of(objects: &[serde_json::Value]) -> Vec<String> {
    objects
        .iter()
        .map(|o| o["id"].as_str().unwrap().to_owned())
        .collect()
}

fn sorted(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    ids.into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn field_names(model: &Model) -> Vec<&'static str> {
    if model.node {
        vec!["label", "n", "tags"]
    } else {
        vec!["to", "peers"]
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn queries_agree_with_the_reference_model(
        ops in prop::collection::vec(op(), 1..40),
        limit in 1..9_usize,
        subset in prop::collection::vec(any::<usize>(), 0..6),
    ) {
        // At least one object, so that the document has a history.
        let mut ops = ops;
        ops.insert(0, Op::Node(String::new(), 0, Vec::new(), None));
        let (document, model) = build(&ops);
        let registry = Registry::new();
        let documents = BTreeMap::from([("d".to_owned(), document)]);
        let server = Server::new(&registry, &documents, ServerConfig::new("t", "0", [Permission::Read]), &NoLog);
        let h = Harness { server: &server, schema: schema() };
        let revision = documents["d"].snapshot().revision();
        let all: Vec<String> = sorted(model.keys().map(ToString::to_string));

        // Pagination visits every object exactly once, whatever the page size, at one revision.
        let (objects, at) = h.pages("query.objects", "objects", json!({"document": "d"}), limit);
        prop_assert_eq!(at, revision);
        let seen = ids_of(&objects);
        prop_assert_eq!(seen.len(), model.len());
        prop_assert_eq!(sorted(seen), all.clone());

        // Default projection: every top-level field, equal to the model.
        for object in &objects {
            let id: ObjectId = object["id"].as_str().unwrap().parse().unwrap();
            let m = &model[&id];
            prop_assert_eq!(object["type"].as_str().unwrap(), if m.node { NODE } else { EDGE });
            let fields = object["fields"].as_object().unwrap();
            prop_assert_eq!(fields.keys().map(String::as_str).collect::<BTreeSet<_>>(), field_names(m).into_iter().collect::<BTreeSet<_>>());
            if m.node {
                prop_assert_eq!(&fields["label"], &json!(m.label));
                prop_assert_eq!(&fields["n"], &json!(m.n));
                prop_assert_eq!(&fields["tags"], &json!(m.tags));
            } else {
                prop_assert_eq!(&fields["to"], &json!(m.to.map(|t| t.to_string())));
                let peers: Vec<String> = m.peers.iter().map(ToString::to_string).collect();
                prop_assert_eq!(&fields["peers"], &json!(peers));
            }
            prop_assert_eq!(object.get("owner").and_then(|o| o.as_str()).map(str::to_owned), m.owner.map(|o| o.to_string()));
        }

        // Projection of paths: present, absent, out of range and of another type.
        let (projected, _) = h.pages("query.objects", "objects", json!({"document": "d", "fields": ["n", "tags[0]", "peers[1]", "to", "nope", "n.x"]}), limit);
        for object in &projected {
            let id: ObjectId = object["id"].as_str().unwrap().parse().unwrap();
            let m = &model[&id];
            let f = &object["fields"];
            prop_assert_eq!(f.as_object().unwrap().len(), 6);
            prop_assert_eq!(&f["n"], &if m.node { json!(m.n) } else { json!(null) });
            prop_assert_eq!(&f["tags[0]"], &json!(m.tags.first()));
            prop_assert_eq!(&f["peers[1]"], &json!(m.peers.get(1).map(ToString::to_string)));
            prop_assert_eq!(&f["to"], &json!(m.to.map(|t| t.to_string())));
            prop_assert_eq!(&f["nope"], &json!(null));
            prop_assert_eq!(&f["n.x"], &json!(null));
        }
        let none = h.call("query.objects", json!({"document": "d", "fields": []})).unwrap();
        prop_assert!(none["objects"].as_array().unwrap().iter().all(|o| o["fields"] == json!({})), "fields: [] projects nothing");

        // Filters by type, owner and identifiers, alone and together.
        let of_type = |edge: bool| sorted(model.iter().filter(|(_, m)| m.node != edge).map(|(id, _)| id.to_string()));
        let (edges, _) = h.pages("query.objects", "objects", json!({"document": "d", "types": [EDGE]}), limit);
        prop_assert_eq!(sorted(ids_of(&edges)), of_type(true));
        let (both, _) = h.pages("query.objects", "objects", json!({"document": "d", "types": [NODE, EDGE, NODE]}), limit);
        prop_assert_eq!(sorted(ids_of(&both)), all.clone());
        let owners: BTreeSet<ObjectId> = model.values().filter_map(|m| m.owner).collect();
        for owner in owners {
            let (owned, _) = h.pages("query.objects", "objects", json!({"document": "d", "owner": owner.to_string()}), limit);
            let expected = sorted(model.iter().filter(|(_, m)| m.owner == Some(owner)).map(|(id, _)| id.to_string()));
            prop_assert_eq!(sorted(ids_of(&owned)), expected);
        }
        let keys: Vec<ObjectId> = model.keys().copied().collect();
        let mut wanted: Vec<String> = subset.iter().map(|i| keys[i % keys.len()].to_string()).collect();
        wanted.push(ObjectId::from_u128(1).to_string());
        if let Some(first) = wanted.first().cloned() {
            wanted.push(first);
        }
        let (by_id, _) = h.pages("query.objects", "objects", json!({"document": "d", "ids": wanted}), limit);
        let mut expected: Vec<String> = Vec::new();
        for id in &wanted {
            if model.contains_key(&id.parse::<ObjectId>().unwrap()) && !expected.contains(id) {
                expected.push(id.clone());
            }
        }
        prop_assert_eq!(ids_of(&by_id), expected.clone());
        let (nodes_by_id, _) = h.pages("query.objects", "objects", json!({"document": "d", "ids": wanted, "types": [NODE]}), limit);
        let expected_nodes: Vec<String> = expected.iter().filter(|id| model[&id.parse::<ObjectId>().unwrap()].node).cloned().collect();
        prop_assert_eq!(ids_of(&nodes_by_id), expected_nodes);

        // References: what an object refers to and what refers to it.
        for (id, m) in &model {
            let mut outgoing: BTreeSet<(String, String)> = BTreeSet::new();
            if let Some(owner) = m.owner { outgoing.insert(("(owner)".into(), owner.to_string())); }
            if let Some(to) = m.to { outgoing.insert(("to".into(), to.to_string())); }
            for (i, peer) in m.peers.iter().enumerate() { outgoing.insert((format!("peers[{i}]"), peer.to_string())); }
            let mut incoming: BTreeSet<(String, String)> = BTreeSet::new();
            for (other, o) in &model {
                if o.owner == Some(*id) { incoming.insert(("(owner)".into(), other.to_string())); }
                if o.to == Some(*id) { incoming.insert(("to".into(), other.to_string())); }
                for (i, peer) in o.peers.iter().enumerate() {
                    if peer == id { incoming.insert((format!("peers[{i}]"), other.to_string())); }
                }
            }
            for (direction, expected) in [("outgoing", &outgoing), ("incoming", &incoming), ("both", &outgoing.union(&incoming).cloned().collect())] {
                let (found, _) = h.pages("query.references", "references", json!({"document": "d", "id": id.to_string(), "direction": direction}), limit);
                let mut seen: Vec<(String, String)> = Vec::new();
                for r in &found {
                    seen.push((r["field"].as_str().unwrap().to_owned(), r["object"].as_str().unwrap().to_owned()));
                    let other: ObjectId = r["object"].as_str().unwrap().parse().unwrap();
                    prop_assert_eq!(r["type"].as_str().unwrap(), if model[&other].node { NODE } else { EDGE });
                }
                if direction == "both" {
                    // Distinct entries: a field both ways is two entries of different direction.
                    prop_assert_eq!(seen.len(), outgoing.len() + incoming.len());
                }
                let seen: BTreeSet<_> = seen.into_iter().collect();
                prop_assert_eq!(&seen, expected, "{} {}", direction, id);
            }
        }

        // History: one transaction per operation that changed something, in order.
        let (history, _) = h.pages("query.history", "transactions", json!({"document": "d"}), limit);
        let total: u64 = history.iter().map(|t| t["counts"]["created"].as_u64().unwrap()).sum();
        let deleted: u64 = history.iter().map(|t| t["counts"]["deleted"].as_u64().unwrap()).sum();
        prop_assert_eq!(total - deleted, model.len() as u64);
        let revisions: Vec<u64> = history.iter().map(|t| t["revision"].as_u64().unwrap()).collect();
        prop_assert!(revisions.windows(2).all(|w| w[0] < w[1]));
        prop_assert_eq!(revisions.last().copied(), Some(revision));

        // A cursor of another revision or another query is refused with its own code.
        if model.len() > limit {
            let page = h.call("query.objects", json!({"document": "d", "limit": limit})).unwrap();
            let cursor = page["nextCursor"].as_str().unwrap().to_owned();
            let other = h.call("query.objects", json!({"document": "d", "limit": limit, "types": [NODE], "cursor": cursor}));
            let other = other.unwrap_err();
            prop_assert_eq!(other["data"]["code"].as_str().unwrap(), "cmd.query.cursor_invalid");
            let node = model.iter().find(|(_, m)| m.node).map(|(id, _)| *id).unwrap();
            documents["d"].submit(move |txn| txn.modify(node, FieldPath::new("n"), Some(Value::I64(99)))).wait().unwrap();
            let stale = h.call("query.objects", json!({"document": "d", "limit": limit, "cursor": cursor})).unwrap_err();
            prop_assert_eq!(stale["data"]["code"].as_str().unwrap(), "cmd.query.cursor_stale");
            prop_assert_eq!(stale["code"].as_i64(), Some(-32006));
            prop_assert_eq!(stale["data"]["details"]["current"].as_u64(), Some(revision + 1));
        }
    }
}

#[test]
fn page_size_and_cursor_errors() {
    let (document, _) = build(&[Op::Node("a".into(), 1, vec![], None)]);
    let registry = Registry::new();
    let documents = BTreeMap::from([("d".to_owned(), document)]);
    let mut config = ServerConfig::new("t", "0", [Permission::Read]);
    config.limits = Limits {
        array_length: 8,
        ..Limits::DEFAULT
    };
    let server = Server::new(&registry, &documents, config, &NoLog);
    let h = Harness {
        server: &server,
        schema: schema(),
    };
    let code = |r: Result<serde_json::Value, serde_json::Value>| {
        r.unwrap_err()["data"]["code"].as_str().unwrap().to_owned()
    };
    // The page bound is the smaller of the protocol's and the session's array limit.
    assert!(
        h.call("query.objects", json!({"document": "d", "limit": 8}))
            .is_ok()
    );
    let over = h.call("query.objects", json!({"document": "d", "limit": 9}));
    assert_eq!(over.as_ref().unwrap_err()["code"], -32008);
    assert_eq!(over.unwrap_err()["data"]["details"]["limit"], "page_size");
    assert_eq!(
        code(h.call("query.objects", json!({"document": "d", "limit": 0}))),
        "cmd.protocol.invalid_params"
    );
    assert_eq!(
        code(h.call("query.objects", json!({"document": "d", "fields": ["a["]}))),
        "cmd.protocol.invalid_params"
    );
    assert_eq!(
        code(h.call("query.objects", json!({"document": "d", "ids": [1]}))),
        "cmd.protocol.invalid_params"
    );
    assert_eq!(
        code(h.call(
            "query.objects",
            json!({"document": "d", "ids": vec![ObjectId::from_u128(1).to_string(); 9]})
        )),
        "cmd.protocol.limit_exceeded"
    );
    assert_eq!(
        code(h.call("query.objects", json!({"document": "x"}))),
        "cmd.document.unknown"
    );
    assert_eq!(
        code(h.call("query.objects", json!({"document": "d", "cursor": "zz"}))),
        "cmd.query.cursor_invalid"
    );
    let nonhex = "g".repeat(64);
    assert_eq!(
        code(h.call("query.objects", json!({"document": "d", "cursor": nonhex}))),
        "cmd.query.cursor_invalid"
    );
    assert_eq!(
        code(h.call(
            "query.references",
            json!({"document": "d", "id": ObjectId::from_u128(5).to_string()})
        )),
        "doc.object.not_found"
    );
    assert_eq!(
        code(h.call(
            "query.references",
            json!({"document": "d", "id": ObjectId::from_u128(5).to_string(), "direction": "up"})
        )),
        "cmd.protocol.invalid_params"
    );
    assert_eq!(
        code(h.call(
            "query.diagnostics",
            json!({"document": "d", "severity": "info"})
        )),
        "cmd.protocol.invalid_params"
    );
    let diagnostics = h
        .call("query.diagnostics", json!({"document": "d"}))
        .unwrap();
    assert_eq!(diagnostics["diagnostics"], json!([]));
    // Without the read class every query is refused.
    let none = Server::new(
        &registry,
        &documents,
        ServerConfig::new("t", "0", []),
        &NoLog,
    );
    let denied = none
        .call("query.objects", json!({"document": "d"}))
        .unwrap_err();
    assert_eq!(denied["data"]["code"], "cmd.permission_denied");
    let _: Map<String, serde_json::Value> = Map::new();
}
