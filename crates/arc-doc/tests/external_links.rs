//! External references (CORE-DOC-011) and unresolved and outdated links (CORE-DOC-012): links
//! load read-only through a caller's resolver, references address `(link, ObjectId)`, cycles and
//! nesting beyond the limit are refused, unresolved links keep their references, and a reload of
//! a changed document is an undoable transaction.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code builds valid literal fixtures"
)]

use std::sync::{Arc, Mutex};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};
use arc_doc::error::codes;
use arc_doc::graph::Evaluators;
use arc_doc::link::{self, LinkSpec, LinkState, LoadMode, Opened};
use arc_doc::{
    Document, ExternalRef, FieldDecl, Header, Schema, SchemaVersion, Severity, Snapshot, Txn,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

/// A registry with a number holder, a holder of one external reference and the link type.
fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(
            TypeKey::new("test.cell").unwrap(),
            Schema::new(
                SchemaVersion::new(1, 0),
                vec![FieldDecl::new("v", ValueType::I64)],
            ),
        ))
        .unwrap();
    registry
        .register(TypeDecl::new(
            TypeKey::new("test.holder").unwrap(),
            Schema::new(
                SchemaVersion::new(1, 0),
                vec![FieldDecl::new("ext", ValueType::ExternalReference).optional()],
            ),
        ))
        .unwrap();
    link::register(&mut registry).unwrap();
    Arc::new(registry)
}

fn document() -> Document {
    Document::new(registry(), Arc::new(Evaluators::new())).unwrap()
}

/// A document holding one cell `1` with value `v`.
fn cell_document(v: i64) -> Document {
    let document = document();
    document
        .submit(move |txn: &mut Txn| {
            txn.create(
                "test.cell",
                id(1),
                Header::default(),
                vec![Some(Value::I64(v))],
            )
            .map(drop)
        })
        .wait()
        .unwrap();
    document
}

fn opened(location: &str, document: ObjectId, content: u8, snapshot: &Snapshot) -> Opened {
    Opened {
        location: location.to_owned(),
        document,
        content: [content; 32],
        snapshot: snapshot.clone(),
    }
}

/// Adds the link `link` to `path` of the document `target`, and a holder of a reference through
/// it to object `1` of the target.
fn link_document(
    document: &Document,
    link: ObjectId,
    holder: ObjectId,
    target: ObjectId,
    path: &str,
    mode: LoadMode,
) {
    let path = path.to_owned();
    document
        .submit(move |txn: &mut Txn| {
            link::create(
                txn,
                link,
                &LinkSpec {
                    document: target,
                    path: &path,
                    hub_resource: None,
                    mode,
                },
            )
        })
        .wait()
        .unwrap();
    document
        .submit(move |txn: &mut Txn| {
            txn.create(
                "test.holder",
                holder,
                Header::default(),
                vec![Some(Value::ExternalReference(Box::new(ExternalRef {
                    link,
                    target: id(1),
                })))],
            )
            .map(drop)
        })
        .wait()
        .unwrap();
}

fn reference(link: ObjectId) -> ExternalRef {
    ExternalRef {
        link,
        target: id(1),
    }
}

#[test]
fn a_linked_document_is_read_only_and_its_objects_address_through_the_link() {
    let linked = cell_document(42);
    let root = document();
    let (link_id, holder) = (id(10), id(11));
    link_document(
        &root,
        link_id,
        holder,
        id(100),
        "linked.arc",
        LoadMode::Full,
    );
    let linked_snapshot = linked.snapshot();
    let resolver = move |_: &str, path: &str| -> Result<Opened> {
        match path {
            "linked.arc" => Ok(opened("linked", id(100), 7, &linked_snapshot)),
            _ => Err(Error::new(codes::OBJECT_NOT_FOUND).with_arg("id", path)),
        }
    };
    let links = link::load(
        &root.snapshot(),
        id(500),
        "root",
        &resolver,
        &Limits::DEFAULT,
    );

    let object = links.object(&reference(link_id)).unwrap();
    assert_eq!(object.values[0], Some(Value::I64(42)));
    assert!(matches!(links.state(link_id), Some(LinkState::Outdated(_))));
}

#[test]
fn an_unresolved_link_keeps_its_references_and_is_reported() {
    let root = document();
    let (link_id, holder) = (id(10), id(11));
    link_document(
        &root,
        link_id,
        holder,
        id(100),
        "missing.arc",
        LoadMode::Full,
    );
    let before = root.snapshot();
    let resolver = |_: &str, path: &str| -> Result<Opened> {
        Err(Error::new(codes::OBJECT_NOT_FOUND).with_arg("id", path))
    };
    let links = link::load(&before, id(500), "root", &resolver, &Limits::DEFAULT);

    assert!(matches!(
        links.state(link_id),
        Some(LinkState::Unresolved(_))
    ));
    let problems = links.problems();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].code.code, codes::LINK_UNRESOLVED.code);
    assert_eq!(problems[0].severity, Severity::Warning);
    assert_eq!(problems[0].reason, codes::OBJECT_NOT_FOUND.code);
    let error = links.object(&reference(link_id)).unwrap_err();
    assert_eq!(error.code(), codes::LINK_UNRESOLVED.code);
    // The document still holds the link object and the external reference.
    let after = root.snapshot();
    assert_eq!(
        after.get(after.resolve(holder).unwrap()).unwrap().values,
        vec![Some(Value::ExternalReference(Box::new(reference(link_id))))]
    );
    assert!(after.resolve(link_id).is_ok());
}

#[test]
fn a_changed_linked_document_is_outdated_and_a_reload_is_undoable() {
    let root = cell_document(1);
    let (link_id, holder) = (id(10), id(11));
    link_document(
        &root,
        link_id,
        holder,
        id(100),
        "linked.arc",
        LoadMode::Full,
    );
    let linked = cell_document(5);
    let linked_snapshot = linked.snapshot();
    // The link has never recorded content, so the first load is outdated.
    let resolver = {
        let snapshot = linked_snapshot.clone();
        move |_: &str, _: &str| -> Result<Opened> { Ok(opened("linked", id(100), 9, &snapshot)) }
    };
    let links = link::load(
        &root.snapshot(),
        id(500),
        "root",
        &resolver,
        &Limits::DEFAULT,
    );
    assert!(matches!(links.state(link_id), Some(LinkState::Outdated(_))));
    assert_eq!(links.problems()[0].code.code, codes::LINK_OUTDATED.code);
    // An outdated link still reads the file as it is now.
    assert_eq!(
        links.object(&reference(link_id)).unwrap().values[0],
        Some(Value::I64(5))
    );

    // Reloading records the new content in a transaction; undo restores the old record.
    let reload_resolver = {
        let snapshot = linked_snapshot.clone();
        move |_: &str, _: &str| -> Result<Opened> { Ok(opened("linked", id(100), 9, &snapshot)) }
    };
    root.submit(move |txn: &mut Txn| link::reload(txn, link_id, "root", &reload_resolver))
        .wait()
        .unwrap();
    let reloaded = link::load(
        &root.snapshot(),
        id(500),
        "root",
        &resolver,
        &Limits::DEFAULT,
    );
    assert!(matches!(
        reloaded.state(link_id),
        Some(LinkState::Resolved(_))
    ));
    assert!(reloaded.problems().is_empty());

    root.undo().wait().unwrap();
    let undone = link::load(
        &root.snapshot(),
        id(500),
        "root",
        &resolver,
        &Limits::DEFAULT,
    );
    assert!(matches!(
        undone.state(link_id),
        Some(LinkState::Outdated(_))
    ));
}

#[test]
fn a_cycle_of_links_loads_once_per_document_and_terminates() {
    // A links B and B links A. Each document is opened once, and A (the root) is not opened twice.
    let (a_id, b_id) = (id(100), id(200));
    let (to_b, to_a) = (id(10), id(20));
    let a = cell_document(7);
    link_document(&a, to_b, id(11), b_id, "b.arc", LoadMode::Full);
    let b = cell_document(9);
    link_document(&b, to_a, id(21), a_id, "a.arc", LoadMode::Full);
    let (a_snapshot, b_snapshot) = (a.snapshot(), b.snapshot());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&calls);
    let resolver_a = a_snapshot.clone();
    let resolver = move |_from: &str, path: &str| -> Result<Opened> {
        log.lock().unwrap().push(path.to_owned());
        match path {
            "b.arc" => Ok(opened("b", b_id, 2, &b_snapshot)),
            "a.arc" => Ok(opened("a", a_id, 1, &resolver_a)),
            _ => Err(Error::new(codes::OBJECT_NOT_FOUND).with_arg("id", path)),
        }
    };
    let links = link::load(&a_snapshot, a_id, "a", &resolver, &Limits::DEFAULT);

    assert_eq!(*calls.lock().unwrap(), vec!["b.arc", "a.arc"]);
    assert_eq!(links.state(to_b), Some(&LinkState::Outdated(b_id)));
    let view_of_b = links.linked(to_b).unwrap();
    assert_eq!(view_of_b.document(), b_id);
    assert_eq!(view_of_b.state(to_a), Some(&LinkState::Outdated(a_id)));
    // An object of A is addressable through the link in B.
    let object = view_of_b
        .object(&ExternalRef {
            link: to_a,
            target: id(1),
        })
        .unwrap();
    assert_eq!(object.values[0], Some(Value::I64(7)));
}

#[test]
fn nesting_beyond_the_limit_is_refused() {
    // root -> d1 -> d2 -> d3, each link to the next document. With nesting_depth 2 the link
    // from d2 to d3 is refused; with 3 every level opens.
    let (d1, d2, d3) = (id(101), id(102), id(103));
    let d3_doc = cell_document(3);
    let d2_doc = document();
    link_document(&d2_doc, id(12), id(13), d3, "d3.arc", LoadMode::Full);
    let d1_doc = document();
    link_document(&d1_doc, id(14), id(15), d2, "d2.arc", LoadMode::Full);
    let root = document();
    link_document(&root, id(16), id(17), d1, "d1.arc", LoadMode::Full);
    let (s1, s2, s3) = (d1_doc.snapshot(), d2_doc.snapshot(), d3_doc.snapshot());
    let resolver = move |_: &str, path: &str| -> Result<Opened> {
        match path {
            "d1.arc" => Ok(opened("d1", d1, 1, &s1)),
            "d2.arc" => Ok(opened("d2", d2, 2, &s2)),
            "d3.arc" => Ok(opened("d3", d3, 3, &s3)),
            _ => Err(Error::new(codes::OBJECT_NOT_FOUND).with_arg("id", path)),
        }
    };
    let root_snapshot = root.snapshot();

    let limited = Limits {
        nesting_depth: 2,
        ..Limits::DEFAULT
    };
    let links = link::load(&root_snapshot, id(500), "root", &resolver, &limited);
    let level_one = links.linked(id(16)).unwrap();
    let level_two = level_one.linked(id(14)).unwrap();
    let Some(LinkState::Refused(problem)) = level_two.state(id(12)) else {
        panic!("the link to the third level is refused");
    };
    assert_eq!(problem.code.code, codes::LINK_TOO_DEEP.code);

    let deep = Limits {
        nesting_depth: 3,
        ..Limits::DEFAULT
    };
    let links = link::load(&root_snapshot, id(500), "root", &resolver, &deep);
    let level_three = links
        .linked(id(16))
        .unwrap()
        .linked(id(14))
        .unwrap()
        .linked(id(12))
        .unwrap();
    assert!(level_three.snapshot().unwrap().resolve(id(1)).is_ok());
}

#[test]
fn a_not_loaded_link_reads_nothing_and_a_reload_of_it_is_refused() {
    let root = document();
    let (link_id, holder) = (id(10), id(11));
    link_document(
        &root,
        link_id,
        holder,
        id(100),
        "linked.arc",
        LoadMode::NotLoaded,
    );
    let resolver =
        |_: &str, _: &str| -> Result<Opened> { panic!("a not_loaded link is never opened") };
    let links = link::load(
        &root.snapshot(),
        id(500),
        "root",
        &resolver,
        &Limits::DEFAULT,
    );
    assert!(matches!(links.state(link_id), Some(LinkState::NotLoaded)));
    assert_eq!(
        links.object(&reference(link_id)).unwrap_err().code(),
        codes::LINK_NOT_LOADED.code
    );
    let error = root
        .submit(move |txn: &mut Txn| link::reload(txn, link_id, "root", &resolver))
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), codes::LINK_NOT_LOADED.code);
}

#[test]
fn a_document_with_another_identifier_does_not_resolve_the_link() {
    let root = document();
    let (link_id, holder) = (id(10), id(11));
    link_document(
        &root,
        link_id,
        holder,
        id(100),
        "linked.arc",
        LoadMode::Full,
    );
    let other = cell_document(1);
    let snapshot = other.snapshot();
    let resolver =
        move |_: &str, _: &str| -> Result<Opened> { Ok(opened("other", id(999), 1, &snapshot)) };
    let links = link::load(
        &root.snapshot(),
        id(500),
        "root",
        &resolver,
        &Limits::DEFAULT,
    );
    assert!(matches!(
        links.state(link_id),
        Some(LinkState::Unresolved(_))
    ));
    assert_eq!(links.problems()[0].reason, "document");
}
