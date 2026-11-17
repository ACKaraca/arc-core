//! The shared error model (CORE-BASE-015 to CORE-BASE-018).
//!
//! Every crate defines its own error enum and converts it into [`Error`], which carries a
//! category, a stable dotted code, a Fluent message identifier with typed arguments, structured
//! details and the chain of underlying causes. Libraries never format user-facing sentences: the
//! application resolves the message identifier in the user's locale.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The result type of fallible Arc functions.
pub type Result<T> = std::result::Result<T, Error>;

/// The eleven error categories. A category tells a caller how to react without knowing the code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// The caller supplied a value that cannot be accepted.
    InvalidInput,
    /// The named thing does not exist.
    NotFound,
    /// The request contradicts the current state.
    Conflict,
    /// The operation or data is not supported.
    Unsupported,
    /// A configured limit was exceeded.
    LimitExceeded,
    /// The caller may not perform the operation.
    PermissionDenied,
    /// Reading or writing failed.
    Io,
    /// Stored or received data is damaged.
    CorruptData,
    /// The operation was cancelled.
    Cancelled,
    /// A valid operation could not produce a result.
    FailedOperation,
    /// A defect in Arc.
    Internal,
}

impl Category {
    /// Every category, in declaration order.
    pub const ALL: [Self; 11] = [
        Self::InvalidInput,
        Self::NotFound,
        Self::Conflict,
        Self::Unsupported,
        Self::LimitExceeded,
        Self::PermissionDenied,
        Self::Io,
        Self::CorruptData,
        Self::Cancelled,
        Self::FailedOperation,
        Self::Internal,
    ];

    /// The stable lowercase name used in protocols and CLI output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Unsupported => "unsupported",
            Self::LimitExceeded => "limit_exceeded",
            Self::PermissionDenied => "permission_denied",
            Self::Io => "io",
            Self::CorruptData => "corrupt_data",
            Self::Cancelled => "cancelled",
            Self::FailedOperation => "failed_operation",
            Self::Internal => "internal",
        }
    }
}

/// A registered error: its stable code, category and message identifier.
///
/// Codes are dotted lowercase strings owned by the crate named in the first segment. They are
/// public API: a code is never reused for a different meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode {
    /// The stable dotted code, for example `base.id.invalid`.
    pub code: &'static str,
    /// The category of every error with this code.
    pub category: Category,
    /// The Fluent message identifier that describes the error to a user.
    pub message_id: &'static str,
}

/// The error codes owned by `arc-base`.
pub mod codes {
    use super::{Category, ErrorCode};

    /// A text is not an identifier in hyphenated or plain hexadecimal form.
    pub const ID_INVALID: ErrorCode = ErrorCode {
        code: "base.id.invalid",
        category: Category::InvalidInput,
        message_id: "base-id-invalid",
    };
    /// Two quantities of different dimensions were added, subtracted, compared or converted.
    pub const UNIT_DIMENSION_MISMATCH: ErrorCode = ErrorCode {
        code: "base.unit.dimension_mismatch",
        category: Category::InvalidInput,
        message_id: "base-unit-dimension-mismatch",
    };
    /// A unit name is not in the built-in registry.
    pub const UNIT_UNKNOWN: ErrorCode = ErrorCode {
        code: "base.unit.unknown",
        category: Category::NotFound,
        message_id: "base-unit-unknown",
    };
    /// A display rounding increment is not a positive finite number.
    pub const DISPLAY_INVALID_INCREMENT: ErrorCode = ErrorCode {
        code: "base.display.invalid_increment",
        category: Category::InvalidInput,
        message_id: "base-display-invalid-increment",
    };
    /// The operation observed a cancelled token.
    pub const CANCELLED: ErrorCode = ErrorCode {
        code: "base.cancelled",
        category: Category::Cancelled,
        message_id: "base-cancelled",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        ID_INVALID,
        UNIT_DIMENSION_MISMATCH,
        UNIT_UNKNOWN,
        DISPLAY_INVALID_INCREMENT,
        CANCELLED,
    ];
}

/// A typed message argument or detail value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Value {
    /// A truth value.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A real number.
    Float(f64),
    /// A text.
    Text(String),
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

/// The error every Arc crate error converts into.
///
/// Its `Display` form is the stable code, for logs. Serializing it yields the machine-readable
/// form used by protocols and CLI output: `category`, `code`, `message_id`, `arguments` and
/// `details`. The source chain is deliberately not serialized, so internal details never reach a
/// client; read it with [`Error::source_chain`] for logs and diagnostics.
#[derive(Debug, thiserror::Error, Serialize)]
#[error("{code}")]
pub struct Error {
    category: Category,
    code: &'static str,
    message_id: &'static str,
    arguments: BTreeMap<&'static str, Value>,
    details: BTreeMap<&'static str, Value>,
    #[serde(skip)]
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    /// The longest source chain [`Error::source_chain`] reports.
    pub const MAX_CHAIN: usize = 32;

    /// Creates an error from a registered code.
    #[must_use]
    pub fn new(code: ErrorCode) -> Self {
        Self {
            category: code.category,
            code: code.code,
            message_id: code.message_id,
            arguments: BTreeMap::new(),
            details: BTreeMap::new(),
            source: None,
        }
    }

    /// Adds an argument that the localized message refers to by `name`.
    #[must_use]
    pub fn with_arg(mut self, name: &'static str, value: impl Into<Value>) -> Self {
        self.arguments.insert(name, value.into());
        self
    }

    /// Adds a structured detail for clients and diagnostics; details are not shown in messages.
    #[must_use]
    pub fn with_detail(mut self, name: &'static str, value: impl Into<Value>) -> Self {
        self.details.insert(name, value.into());
        self
    }

    /// Records the underlying cause.
    #[must_use]
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// The category.
    #[must_use]
    pub fn category(&self) -> Category {
        self.category
    }

    /// The stable dotted code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// The Fluent message identifier.
    #[must_use]
    pub fn message_id(&self) -> &'static str {
        self.message_id
    }

    /// The message arguments by name.
    #[must_use]
    pub fn arguments(&self) -> &BTreeMap<&'static str, Value> {
        &self.arguments
    }

    /// The structured details by name.
    #[must_use]
    pub fn details(&self) -> &BTreeMap<&'static str, Value> {
        &self.details
    }

    /// The underlying causes, outermost first, as text for logs and diagnostics. The chain is cut
    /// after [`Error::MAX_CHAIN`] entries.
    #[must_use]
    pub fn source_chain(&self) -> Vec<String> {
        let first = std::error::Error::source(self);
        std::iter::successors(first, |cause| cause.source())
            .take(Self::MAX_CHAIN)
            .map(ToString::to_string)
            .collect()
    }
}

/// A defect in an error-code registry, found by [`registry_problems`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryProblem {
    /// Two entries register the same code.
    DuplicateCode(&'static str),
    /// The code is not two or more dot-separated segments of lowercase letters, digits and
    /// underscores, each starting with a letter.
    MalformedCode(&'static str),
    /// The entry with this code has no message identifier.
    MissingMessageId(&'static str),
    /// The message identifier of the entry with this code is not a Fluent identifier.
    MalformedMessageId(&'static str),
}

/// Checks a registry of error codes: the concatenated `codes::ALL` tables of the crates under
/// test. An empty result means every code is unique and well-formed and has a message identifier.
#[must_use]
pub fn registry_problems(registry: &[ErrorCode]) -> Vec<RegistryProblem> {
    fn segment_ok(segment: &str) -> bool {
        segment.starts_with(|c: char| c.is_ascii_lowercase())
            && segment
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }
    fn message_id_ok(id: &str) -> bool {
        id.starts_with(|c: char| c.is_ascii_alphabetic())
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in registry {
        if !seen.insert(entry.code) {
            problems.push(RegistryProblem::DuplicateCode(entry.code));
        }
        if !entry.code.contains('.') || !entry.code.split('.').all(segment_ok) {
            problems.push(RegistryProblem::MalformedCode(entry.code));
        }
        if entry.message_id.is_empty() {
            problems.push(RegistryProblem::MissingMessageId(entry.code));
        } else if !message_id_ok(entry.message_id) {
            problems.push(RegistryProblem::MalformedMessageId(entry.code));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-base.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-base.md");

    #[test]
    fn base_registry_has_no_problems() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
    }

    #[test]
    fn duplicate_code_is_reported() {
        let registry = [codes::ID_INVALID, codes::CANCELLED, codes::ID_INVALID];
        assert_eq!(
            registry_problems(&registry),
            vec![RegistryProblem::DuplicateCode("base.id.invalid")]
        );
    }

    #[test]
    fn missing_message_identifier_is_reported() {
        let entry = ErrorCode {
            message_id: "",
            ..codes::ID_INVALID
        };
        assert_eq!(
            registry_problems(&[entry]),
            vec![RegistryProblem::MissingMessageId("base.id.invalid")]
        );
    }

    #[test]
    fn malformed_codes_and_message_identifiers_are_reported() {
        for code in ["base", "Base.id", "base..id", "base.1d", "base.id-invalid"] {
            let entry = ErrorCode {
                code,
                ..codes::ID_INVALID
            };
            assert_eq!(
                registry_problems(&[entry]),
                vec![RegistryProblem::MalformedCode(code)]
            );
        }
        let entry = ErrorCode {
            message_id: "base.id.invalid",
            ..codes::ID_INVALID
        };
        assert_eq!(
            registry_problems(&[entry]),
            vec![RegistryProblem::MalformedMessageId("base.id.invalid")]
        );
    }

    #[test]
    fn english_catalog_contains_every_registered_message() {
        let messages: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
        for entry in codes::ALL {
            assert!(
                messages.contains(entry.message_id),
                "{} is not in the catalog",
                entry.code
            );
        }
    }

    #[test]
    fn every_code_is_documented_in_the_specification() {
        for entry in codes::ALL {
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(
                SPECIFICATION.contains(&cell),
                "{cell} is not in docs/crates/arc-base.md"
            );
        }
    }

    #[test]
    fn category_names_match_their_serialized_form() {
        for category in Category::ALL {
            let json = serde_json::to_string(&category).unwrap();
            assert_eq!(json, format!("\"{}\"", category.as_str()));
        }
    }

    #[test]
    fn machine_readable_form_has_the_stable_fields_and_no_sources() {
        let error = Error::new(codes::UNIT_DIMENSION_MISMATCH)
            .with_arg("expected", "length")
            .with_arg("count", 2_i64)
            .with_detail("exact", true)
            .with_source(std::io::Error::other("secret path"));
        let json = serde_json::to_value(&error).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "category": "invalid_input",
                "code": "base.unit.dimension_mismatch",
                "message_id": "base-unit-dimension-mismatch",
                "arguments": { "count": 2, "expected": "length" },
                "details": { "exact": true },
            })
        );
    }

    #[test]
    fn source_chain_lists_causes_outermost_first() {
        let inner = Error::new(codes::ID_INVALID).with_source(std::io::Error::other("disk"));
        let outer = Error::new(codes::CANCELLED).with_source(inner);
        assert_eq!(outer.to_string(), "base.cancelled");
        assert_eq!(
            outer.source_chain(),
            vec!["base.id.invalid".to_owned(), "disk".to_owned()]
        );
        assert!(Error::new(codes::CANCELLED).source_chain().is_empty());
    }
}
