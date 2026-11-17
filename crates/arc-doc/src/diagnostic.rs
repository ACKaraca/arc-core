//! Diagnostics: findings that do not block (validator warnings, load findings, failed nodes,
//! faults) and the sink that receives them.
//!
//! The maintained document diagnostics list with its change events is CORE-DOC-035; this module
//! provides the [`Diagnostic`] value, the [`DiagnosticSink`] a document delivers diagnostics to,
//! and a simple [`CollectingSink`].

use std::sync::{Arc, Mutex, PoisonError};

use arc_base::{Error, ObjectId};

use crate::graph::NodeKey;
use crate::load::{DiagnosticKind, LoadDiagnostic};
use crate::schema::{Finding, Severity};

/// Where a diagnostic comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A type validator or a document validator at commit.
    Validator,
    /// A node of the dependency graph.
    Node(NodeKey),
    /// Loading, with the kind of the load finding.
    Load(DiagnosticKind),
    /// The document executor: a job that unwound, or a step after publication that unwound.
    Executor,
}

/// A finding about a document: severity, stable code, message identifier and arguments (all
/// in the [`Error`]), the objects involved and its origin.
///
/// Clones share the error.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// How serious it is.
    pub severity: Severity,
    /// The code, message identifier, arguments and details.
    pub error: Arc<Error>,
    /// The objects involved, possibly none.
    pub objects: Vec<ObjectId>,
    /// Where it comes from.
    pub origin: Origin,
}

impl Diagnostic {
    /// A diagnostic of `error` about `objects`.
    #[must_use]
    pub fn new(severity: Severity, error: Error, objects: Vec<ObjectId>, origin: Origin) -> Self {
        Self {
            severity,
            error: Arc::new(error),
            objects,
            origin,
        }
    }

    /// The diagnostic of a validator finding about `objects`.
    #[must_use]
    pub fn from_finding(finding: Finding, objects: Vec<ObjectId>) -> Self {
        Self::new(finding.severity, finding.error, objects, Origin::Validator)
    }

    /// The stable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.error.code()
    }
}

impl From<LoadDiagnostic> for Diagnostic {
    /// Quarantined and unaddressable rows are errors (the object or row cannot be used);
    /// preserved objects, validator warnings and dangling references are warnings.
    fn from(diagnostic: LoadDiagnostic) -> Self {
        let severity = match diagnostic.kind {
            DiagnosticKind::Quarantined | DiagnosticKind::Unaddressable => Severity::Error,
            DiagnosticKind::Preserved | DiagnosticKind::Warning | DiagnosticKind::Dangling => {
                Severity::Warning
            }
        };
        Self::new(
            severity,
            diagnostic.error,
            diagnostic.object.into_iter().collect(),
            Origin::Load(diagnostic.kind),
        )
    }
}

/// Receives the diagnostics of a document: validator warnings of each commit and faults. It is
/// called on the document executor after publication and must return quickly.
pub trait DiagnosticSink: Send + Sync {
    /// Takes the diagnostics of one commit or fault, in the order they were found.
    fn deliver(&self, diagnostics: &[Diagnostic]);
}

/// A sink that keeps the first `capacity` diagnostics delivered to it and counts the rest.
#[derive(Debug)]
pub struct CollectingSink {
    capacity: usize,
    state: Mutex<(Vec<Diagnostic>, u64)>,
}

impl CollectingSink {
    /// A sink that keeps at most `capacity` diagnostics.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new((Vec::new(), 0)),
        }
    }

    /// Takes the kept diagnostics and the number dropped since the last call.
    pub fn take(&self) -> (Vec<Diagnostic>, u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *state)
    }
}

impl DiagnosticSink for CollectingSink {
    fn deliver(&self, diagnostics: &[Diagnostic]) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let room = self.capacity.saturating_sub(state.0.len());
        let kept = diagnostics.len().min(room);
        state.0.extend(diagnostics.iter().take(kept).cloned());
        let dropped = u64::try_from(diagnostics.len().saturating_sub(kept)).unwrap_or(u64::MAX);
        state.1 = state.1.saturating_add(dropped);
    }
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;

    use super::*;
    use crate::error::codes;

    #[test]
    fn collecting_sink_keeps_capacity_and_counts_the_rest() {
        let sink = CollectingSink::new(2);
        let one = || {
            Diagnostic::new(
                Severity::Warning,
                Error::new(codes::VALUE_MISSING),
                vec![],
                Origin::Validator,
            )
        };
        sink.deliver(&[one(), one(), one()]);
        let (kept, dropped) = sink.take();
        assert_eq!((kept.len(), dropped), (2, 1));
        assert_eq!(sink.take().0.len(), 0);
    }

    #[test]
    fn load_diagnostics_convert_with_severity_and_object() {
        let id = ObjectId::from_u128(7);
        let quarantined = LoadDiagnostic {
            kind: DiagnosticKind::Quarantined,
            object: Some(id),
            error: Error::new(codes::PAYLOAD_INVALID),
        };
        let diagnostic = Diagnostic::from(quarantined);
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.objects, vec![id]);
        assert_eq!(diagnostic.origin, Origin::Load(DiagnosticKind::Quarantined));
        assert_eq!(diagnostic.code(), "doc.payload.invalid");
        let dangling = LoadDiagnostic {
            kind: DiagnosticKind::Dangling,
            object: None,
            error: Error::new(codes::REFERENCE_DANGLING),
        };
        assert_eq!(Diagnostic::from(dangling).severity, Severity::Warning);
    }
}
