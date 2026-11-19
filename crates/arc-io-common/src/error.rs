//! The error codes owned by `arc-io-common`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]. Messages are in
//! `locales/en-US/arc-io-common.ftl`.

use arc_base::{Error, ErrorCode};

/// The error `io.limit_exceeded` naming the field of [`arc_base::Limits`] that was exceeded.
pub(crate) fn limit(name: &'static str, value: u64) -> Error {
    Error::new(codes::LIMIT_EXCEEDED)
        .with_arg("limit", name)
        .with_detail("value", clamp(value))
}

/// An error that names the index of the ZIP entry it concerns.
pub(crate) fn at_entry(code: ErrorCode, index: usize) -> Error {
    Error::new(code).with_detail("index", clamp(crate::bytes::to_u64(index)))
}

fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The error codes owned by `arc-io-common`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A limit of the caller's limits profile was exceeded; the argument `limit` names it.
    pub const LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "io.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "io-limit-exceeded",
    };
    /// The archive ends inside a record, a header, a name or an entry's data.
    pub const ZIP_TRUNCATED: ErrorCode = ErrorCode {
        code: "io.zip.truncated",
        category: Category::CorruptData,
        message_id: "io-zip-truncated",
    };
    /// No end-of-central-directory record matches the end of the buffer.
    pub const ZIP_END_MISSING: ErrorCode = ErrorCode {
        code: "io.zip.end_missing",
        category: Category::CorruptData,
        message_id: "io-zip-end-missing",
    };
    /// A signature, count, offset or size of the archive's structures is inconsistent.
    pub const ZIP_STRUCTURE_INVALID: ErrorCode = ErrorCode {
        code: "io.zip.structure_invalid",
        category: Category::CorruptData,
        message_id: "io-zip-structure-invalid",
    };
    /// The archive spans several disks, or refers to a disk other than the first.
    pub const ZIP_MULTI_DISK: ErrorCode = ErrorCode {
        code: "io.zip.multi_disk",
        category: Category::Unsupported,
        message_id: "io-zip-multi-disk",
    };
    /// An entry is encrypted, which this reader does not support.
    pub const ZIP_ENCRYPTED: ErrorCode = ErrorCode {
        code: "io.zip.encrypted",
        category: Category::Unsupported,
        message_id: "io-zip-encrypted",
    };
    /// An entry uses a compression method other than stored or deflate.
    pub const ZIP_METHOD_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "io.zip.method_unsupported",
        category: Category::Unsupported,
        message_id: "io-zip-method-unsupported",
    };
    /// An entry's data descriptor does not agree with its central directory record.
    pub const ZIP_DESCRIPTOR_UNVERIFIED: ErrorCode = ErrorCode {
        code: "io.zip.data_descriptor_unverified",
        category: Category::CorruptData,
        message_id: "io-zip-data-descriptor-unverified",
    };
    /// A local file header does not agree with the central directory record of its entry.
    pub const ZIP_HEADER_MISMATCH: ErrorCode = ErrorCode {
        code: "io.zip.header_mismatch",
        category: Category::CorruptData,
        message_id: "io-zip-header-mismatch",
    };
    /// Two entries, or an entry and the central directory, occupy the same bytes.
    pub const ZIP_OVERLAP: ErrorCode = ErrorCode {
        code: "io.zip.overlap",
        category: Category::CorruptData,
        message_id: "io-zip-overlap",
    };
    /// An entry name is empty, is not valid UTF-8, contains a control character or an empty
    /// component.
    pub const ZIP_NAME_INVALID: ErrorCode = ErrorCode {
        code: "io.zip.name_invalid",
        category: Category::CorruptData,
        message_id: "io-zip-name-invalid",
    };
    /// An entry name is absolute, contains a backslash, a colon or a `.` or `..` component.
    pub const ZIP_PATH_UNSAFE: ErrorCode = ErrorCode {
        code: "io.zip.path_unsafe",
        category: Category::CorruptData,
        message_id: "io-zip-path-unsafe",
    };
    /// Two entry names are equal after normalization, ignoring letter case.
    pub const ZIP_NAME_DUPLICATE: ErrorCode = ErrorCode {
        code: "io.zip.name_duplicate",
        category: Category::CorruptData,
        message_id: "io-zip-name-duplicate",
    };
    /// A deflate stream produces more or fewer bytes than declared, or leaves compressed bytes
    /// unread.
    pub const ZIP_SIZE_MISMATCH: ErrorCode = ErrorCode {
        code: "io.zip.size_mismatch",
        category: Category::CorruptData,
        message_id: "io-zip-size-mismatch",
    };
    /// A deflate stream is malformed.
    pub const ZIP_INFLATE_FAILED: ErrorCode = ErrorCode {
        code: "io.zip.inflate_failed",
        category: Category::CorruptData,
        message_id: "io-zip-inflate-failed",
    };
    /// The CRC-32 of the decompressed data differs from the one in the archive.
    pub const ZIP_CRC_MISMATCH: ErrorCode = ErrorCode {
        code: "io.zip.crc_mismatch",
        category: Category::CorruptData,
        message_id: "io-zip-crc-mismatch",
    };
    /// The index does not name an entry of the archive.
    pub const ZIP_ENTRY_MISSING: ErrorCode = ErrorCode {
        code: "io.zip.entry_missing",
        category: Category::NotFound,
        message_id: "io-zip-entry-missing",
    };
    /// The document ends inside a tag, a text node, a declaration or an open element, or it
    /// has no root element.
    pub const XML_TRUNCATED: ErrorCode = ErrorCode {
        code: "io.xml.truncated",
        category: Category::CorruptData,
        message_id: "io-xml-truncated",
    };
    /// The markup is not well formed.
    pub const XML_SYNTAX: ErrorCode = ErrorCode {
        code: "io.xml.syntax",
        category: Category::CorruptData,
        message_id: "io-xml-syntax",
    };
    /// Elements do not nest, a second root element starts, or text appears outside the root.
    pub const XML_STRUCTURE_INVALID: ErrorCode = ErrorCode {
        code: "io.xml.structure_invalid",
        category: Category::CorruptData,
        message_id: "io-xml-structure-invalid",
    };
    /// A name, attribute value or text is not valid UTF-8, or contains a character XML does not
    /// allow.
    pub const XML_TEXT_INVALID: ErrorCode = ErrorCode {
        code: "io.xml.text_invalid",
        category: Category::CorruptData,
        message_id: "io-xml-text-invalid",
    };
    /// The document has a document type declaration, which this reader refuses.
    pub const XML_DOCTYPE_REFUSED: ErrorCode = ErrorCode {
        code: "io.xml.doctype_refused",
        category: Category::Unsupported,
        message_id: "io-xml-doctype-refused",
    };
    /// The document has a processing instruction other than the XML declaration.
    pub const XML_PROCESSING_INSTRUCTION_REFUSED: ErrorCode = ErrorCode {
        code: "io.xml.processing_instruction_refused",
        category: Category::Unsupported,
        message_id: "io-xml-processing-instruction-refused",
    };
    /// An entity reference is neither one of the five predefined entities nor a character
    /// reference.
    pub const XML_ENTITY_REFUSED: ErrorCode = ErrorCode {
        code: "io.xml.entity_refused",
        category: Category::Unsupported,
        message_id: "io-xml-entity-refused",
    };
    /// A character reference names no character of XML, or is not terminated.
    pub const XML_REFERENCE_INVALID: ErrorCode = ErrorCode {
        code: "io.xml.reference_invalid",
        category: Category::CorruptData,
        message_id: "io-xml-reference-invalid",
    };
    /// The declared encoding is not UTF-8, or the document starts with a byte-order mark of
    /// another encoding.
    pub const XML_ENCODING_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "io.xml.encoding_unsupported",
        category: Category::Unsupported,
        message_id: "io-xml-encoding-unsupported",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        LIMIT_EXCEEDED,
        ZIP_TRUNCATED,
        ZIP_END_MISSING,
        ZIP_STRUCTURE_INVALID,
        ZIP_MULTI_DISK,
        ZIP_ENCRYPTED,
        ZIP_METHOD_UNSUPPORTED,
        ZIP_DESCRIPTOR_UNVERIFIED,
        ZIP_HEADER_MISMATCH,
        ZIP_OVERLAP,
        ZIP_NAME_INVALID,
        ZIP_PATH_UNSAFE,
        ZIP_NAME_DUPLICATE,
        ZIP_SIZE_MISMATCH,
        ZIP_INFLATE_FAILED,
        ZIP_CRC_MISMATCH,
        ZIP_ENTRY_MISSING,
        XML_TRUNCATED,
        XML_SYNTAX,
        XML_STRUCTURE_INVALID,
        XML_TEXT_INVALID,
        XML_DOCTYPE_REFUSED,
        XML_PROCESSING_INSTRUCTION_REFUSED,
        XML_ENTITY_REFUSED,
        XML_REFERENCE_INVALID,
        XML_ENCODING_UNSUPPORTED,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-io-common.ftl");
    const CATALOG_TR: &str = include_str!("../../../locales/tr-TR/arc-io-common.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-io.md");

    fn catalog_ids(catalog: &str) -> BTreeSet<&str> {
        catalog
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn registry_has_no_problems_alone_or_with_arc_base() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(codes::ALL.iter().all(|entry| entry.code.starts_with("io.")));
    }

    #[test]
    fn both_catalogs_contain_exactly_the_registered_messages() {
        let ids: Vec<&str> = codes::ALL.iter().map(|c| c.message_id).collect();
        for catalog in [CATALOG, CATALOG_TR] {
            let defined = catalog_ids(catalog);
            for id in &ids {
                assert!(defined.contains(id), "{id} is not in a catalog");
            }
            assert_eq!(defined.len(), ids.len(), "catalog and registry differ");
        }
    }

    #[test]
    fn every_code_is_documented_in_the_specification() {
        for entry in codes::ALL {
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(SPECIFICATION.contains(&cell), "{cell} is not documented");
        }
    }
}
