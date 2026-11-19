//! Deep nesting on a small stack: chains of block references and of placed external references at
//! the default nesting limit are listed on a thread with a 512 KB stack, and chains one level deeper
//! report the nesting error. The documents are built on the test thread, because building a document
//! through commands needs more stack than the display build; only the display build runs on the small
//! thread. A recursion that uses more stack than the bound overflows in these tests, not in a product.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::math::Point2;
use arc_base::{Limits, ObjectId, Result};
use arc_doc::Snapshot;
use arc_doc::link::{self, LinkSpec, LoadMode, Opened};
use arc_draft::display::{self, DisplayOptions, Lists, space_list_with_links};
use arc_geom::{Curve2, Segment2};
use serde_json::{Value, json};
use support::{Fixture, curve_json};

/// The stack of the thread the display build runs on.
const SMALL_STACK: usize = 512 * 1024;

const HOST: u128 = 1;
const LINK: u128 = 10;
/// Document `k` of a chain has the identifier `FIRST + k`.
const FIRST: u128 = 100;
/// The link from document `k` to document `k + 1` has the identifier `LINK_BASE + k`.
const LINK_BASE: u128 = 1000;

const NESTING: &str = "draft.block.nesting_too_deep";

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

/// Runs `run` on a thread with [`SMALL_STACK`] bytes of stack and returns its result.
fn on_small_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap()
}

fn segment() -> Value {
    curve_json(&Curve2::Segment(
        Segment2::new(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)).unwrap(),
    ))
}

/// Block definitions `b0 .. b{count-1}`, each with one line and a reference to the next (the last
/// holds only the line), and a space whose reference to `b0` is the returned entity. The snapshot
/// of the document is returned with the entity.
fn block_chain(count: usize, limit: u32) -> (Snapshot, ObjectId) {
    let mut f = Fixture::with_nesting(limit);
    let space = f.space("model");
    let blocks: Vec<ObjectId> = (0..count)
        .map(|k| f.create("core.draft.block.define", json!({"name": format!("b{k}")})))
        .collect();
    for (k, &block) in blocks.iter().enumerate() {
        f.entity(block, segment());
        if let Some(&next) = blocks.get(k + 1) {
            insert_block(&mut f, block, next);
        }
    }
    let top = insert_block(&mut f, space, blocks[0]);
    (f.document.snapshot(), top)
}

fn insert_block(f: &mut Fixture, owner: ObjectId, block: ObjectId) -> ObjectId {
    f.create(
        "core.draft.block.insert",
        json!({"owner": owner.to_string(), "block": block.to_string(), "position": [0.0, 0.0]}),
    )
}

/// A link object `link` in the drawing `f` to the document `document` at `path`.
fn link_to(f: &mut Fixture, link: ObjectId, document: ObjectId, path: String) {
    f.document
        .submit(move |txn: &mut arc_doc::Txn| {
            let spec = LinkSpec {
                document,
                path: &path,
                hub_resource: None,
                mode: LoadMode::Full,
            };
            link::create(txn, link, &spec)
        })
        .wait()
        .unwrap();
}

/// A placed external reference in `owner` through `link`.
fn place(f: &mut Fixture, owner: ObjectId, link: ObjectId) {
    f.create(
        "core.draft.external_ref.create",
        json!({"owner": owner.to_string(), "link": link.to_string(), "position": [0.0, 0.0]}),
    );
}

/// The documents of a chain of placed references, with the host that places the first one.
struct Chain {
    host: Snapshot,
    host_space: ObjectId,
    /// The identifier and the snapshot of each linked document, in chain order.
    documents: Vec<(ObjectId, Snapshot)>,
}

/// The primitive count of the host's space and the codes of its problems.
type Built = (usize, Vec<&'static str>);

/// Builds the host and the linked documents `d0 .. d{count-1}`. `fill(k, f, space, next)` fills
/// document `k` in the drawing `f` with `space`; `next` is the link to document `k + 1`, when there
/// is one. The host places `d0`.
fn chain(count: usize, fill: impl Fn(usize, &mut Fixture, ObjectId, Option<ObjectId>)) -> Chain {
    let mut documents = Vec::with_capacity(count);
    for k in 0..count {
        let mut f = Fixture::drafting_with_links();
        let space = f.space("Model");
        let next = (k + 1 < count).then(|| {
            let link = id(LINK_BASE + k as u128);
            link_to(
                &mut f,
                link,
                id(FIRST + k as u128 + 1),
                format!("d{}.arc", k + 1),
            );
            link
        });
        fill(k, &mut f, space, next);
        documents.push((id(FIRST + k as u128), f.document.snapshot()));
    }
    let mut host = Fixture::drafting_with_links();
    let host_space = host.space("Host");
    link_to(&mut host, id(LINK), id(FIRST), "d0.arc".to_owned());
    place(&mut host, host_space, id(LINK));
    Chain {
        host: host.document.snapshot(),
        host_space,
        documents,
    }
}

/// Lists the host's space of `chain` through the loaded links.
fn list_through_links(chain: Chain) -> Built {
    let Chain {
        host,
        host_space,
        documents,
    } = chain;
    let resolver = move |_: &str, path: &str| -> Result<Opened> {
        let k: usize = path
            .trim_start_matches('d')
            .trim_end_matches(".arc")
            .parse()
            .unwrap();
        let (document, snapshot) = &documents[k];
        Ok(Opened {
            location: path.to_owned(),
            document: *document,
            content: [7; 32],
            snapshot: snapshot.clone(),
        })
    };
    // The loader's own depth bound is raised above the display bound, so that the display's
    // nesting check decides the chains one level past the limit.
    let limits = Limits {
        nesting_depth: 128,
        ..Limits::DEFAULT
    };
    let links = link::load(&host, id(HOST), "host.arc", &resolver, &limits);
    let out = space_list_with_links(
        &host,
        Lists::Direct,
        Some(&links),
        host_space,
        &DisplayOptions::default(),
    )
    .unwrap();
    let codes = out.problems.iter().map(|p| p.code).collect();
    (out.primitives.len(), codes)
}

/// Each document holds one line and, except the last, a placed reference to the next one.
fn placed_chain(count: usize) -> Chain {
    chain(count, |_, f, space, next| {
        f.entity(space, segment());
        if let Some(link) = next {
            place(f, space, link);
        }
    })
}

/// Document `k` holds a block reference to `B{k}`; `B{k}` holds one line and, except in the last
/// document, a placed reference to the next document. Each document adds two levels of nesting.
fn mixed_chain(documents: usize) -> Chain {
    chain(documents, |k, f, space, next| {
        let block = f.create("core.draft.block.define", json!({"name": format!("B{k}")}));
        f.entity(block, segment());
        insert_block(f, space, block);
        if let Some(link) = next {
            place(f, block, link);
        }
    })
}

#[test]
fn a_chain_of_block_references_at_the_nesting_limit_lists_on_a_small_stack() {
    let (snapshot, top) = block_chain(64, DisplayOptions::default().max_nesting);
    let count = on_small_stack(move || {
        let list = display::entity_list(&snapshot, top, &DisplayOptions::default()).unwrap();
        list.primitives.len()
    });
    assert_eq!(count, 64);
}

#[test]
fn a_chain_of_block_references_one_past_the_nesting_limit_is_refused_on_a_small_stack() {
    let limit = DisplayOptions::default().max_nesting;
    let (snapshot, top) = block_chain(limit as usize + 1, limit + 1);
    let code = on_small_stack(move || {
        display::entity_list(&snapshot, top, &DisplayOptions::default())
            .unwrap_err()
            .code()
    });
    assert_eq!(code, NESTING);
}

#[test]
fn a_chain_of_placed_references_at_the_nesting_limit_lists_on_a_small_stack() {
    let input = placed_chain(64);
    let (count, codes) = on_small_stack(move || list_through_links(input));
    assert_eq!(count, 64);
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_chain_of_placed_references_one_past_the_nesting_limit_reports_the_nesting_error() {
    let input = placed_chain(65);
    let (count, codes) = on_small_stack(move || list_through_links(input));
    assert_eq!(codes, [NESTING]);
    assert_eq!(count, 64);
}

#[test]
fn a_mixed_chain_of_blocks_and_placed_references_at_the_nesting_limit_lists_on_a_small_stack() {
    // Thirty-two documents, each with a block reference and a placed reference: 64 levels.
    let input = mixed_chain(32);
    let (count, codes) = on_small_stack(move || list_through_links(input));
    assert_eq!(count, 32);
    assert!(codes.is_empty(), "{codes:?}");
}

#[test]
fn a_mixed_chain_one_past_the_nesting_limit_reports_the_nesting_error() {
    let input = mixed_chain(33);
    let (_, codes) = on_small_stack(move || list_through_links(input));
    assert_eq!(codes, [NESTING]);
}
