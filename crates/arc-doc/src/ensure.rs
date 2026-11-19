//! Synchronous ensure (CORE-DOC-033): a running command brings derived nodes to a settled state
//! for the uncommitted state of its transaction and reads their values before it commits.
//!
//! The nodes are evaluated on the calling thread by a graph of their own, built on the candidate
//! snapshot of the transaction ([`Txn::snapshot`]). The document's graph and scheduler never see
//! the evaluation, so nothing computed from uncommitted changes is stored where readers can find
//! it: when the transaction rolls back, the results are simply dropped with the graph, and the
//! document's nodes keep the values of the committed state. The graph of the command starts
//! empty, so every node is computed from its inputs in this transaction; a commit does not keep
//! these results.

use std::sync::Arc;

use arc_base::Result;

use crate::graph::{Evaluators, Graph, NodeKey, NodeView};
use crate::txn::Txn;

/// Brings each node of `nodes` to a settled state (clean, failed, blocked or cyclic) for the
/// uncommitted state of `txn` and returns their views in the order given.
///
/// Fails like [`Graph::evaluate_now`]: `doc.object.not_found` when a node names an object the
/// transaction does not hold live, `doc.graph.unknown_key` when no evaluator computes the key,
/// and `doc.graph.stale` when the graph cannot settle the node. A node whose evaluator returns an
/// error is returned as its failed view and does not fail the call. An evaluator that unwinds
/// unwinds through this call; the executor then reports the job as `doc.executor.fault` and rolls
/// the transaction back.
pub fn ensure(txn: &Txn, evaluators: Arc<Evaluators>, nodes: &[NodeKey]) -> Result<Vec<NodeView>> {
    let graph = Graph::new(evaluators, txn.snapshot().clone());
    nodes.iter().map(|node| graph.evaluate_now(node)).collect()
}
