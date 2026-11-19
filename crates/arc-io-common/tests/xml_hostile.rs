//! Hostile XML input: each violation is refused with its specific code, and arbitrary or mutated
//! bytes never panic.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use arc_base::Limits;
use arc_io_common::ReadOptions;
use arc_io_common::xml::XmlReader;
use proptest::prelude::*;

/// The code of the first error of `document` under `limits`, or `None` for a valid document.
fn code(document: &[u8], limits: Limits) -> Option<&'static str> {
    let options = ReadOptions::with_limits(limits);
    let mut reader = match XmlReader::new(document, &options) {
        Ok(reader) => reader,
        Err(error) => return Some(error.code()),
    };
    loop {
        match reader.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => return None,
            Err(error) => return Some(error.code()),
        }
    }
}

fn constrained() -> Limits {
    Limits::CONSTRAINED
}

#[test]
fn a_depth_bomb_is_refused_by_nesting_depth() {
    let limits = Limits {
        nesting_depth: 32,
        ..constrained()
    };
    let mut document = String::new();
    for _ in 0..100 {
        document.push_str("<a>");
    }
    assert_eq!(code(document.as_bytes(), limits), Some("io.limit_exceeded"));
}

#[test]
fn an_attribute_flood_is_refused_by_array_length() {
    let limits = Limits {
        array_length: 100,
        ..constrained()
    };
    let mut document = String::from("<a");
    for index in 0..200 {
        document.push_str(&format!(" a{index}=\"v\""));
    }
    document.push_str("/>");
    assert_eq!(code(document.as_bytes(), limits), Some("io.limit_exceeded"));
}

#[test]
fn oversized_attribute_bytes_are_refused_by_string_bytes() {
    let limits = Limits {
        string_bytes: 64,
        ..constrained()
    };
    let document = format!("<a x=\"{}\"/>", "y".repeat(100));
    assert_eq!(code(document.as_bytes(), limits), Some("io.limit_exceeded"));
}

#[test]
fn a_huge_text_node_is_refused_by_string_bytes() {
    let limits = Limits {
        string_bytes: 1024,
        ..constrained()
    };
    let document = format!("<a>{}</a>", "t".repeat(5000));
    assert_eq!(code(document.as_bytes(), limits), Some("io.limit_exceeded"));
}

#[test]
fn a_document_type_declaration_is_refused() {
    let document = b"<!DOCTYPE a [<!ENTITY e \"value\">]><a>&e;</a>";
    assert_eq!(
        code(document, constrained()),
        Some("io.xml.doctype_refused")
    );
}

#[test]
fn a_billion_laughs_document_is_refused_at_its_declaration() {
    let document = b"<?xml version=\"1.0\"?>\n<!DOCTYPE lolz [\n\
        <!ENTITY lol \"lol\">\n\
        <!ENTITY lol2 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\">\n\
        <!ENTITY lol3 \"&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;\">\n\
        ]>\n<lolz>&lol3;</lolz>";
    assert_eq!(
        code(document, constrained()),
        Some("io.xml.doctype_refused")
    );
}

#[test]
fn a_custom_entity_reference_is_refused() {
    assert_eq!(
        code(b"<a>&custom;</a>", constrained()),
        Some("io.xml.entity_refused")
    );
}

#[test]
fn a_processing_instruction_is_refused() {
    assert_eq!(
        code(b"<?style sheet?><a/>", constrained()),
        Some("io.xml.processing_instruction_refused")
    );
}

#[test]
fn a_numeric_reference_to_a_non_character_is_refused() {
    assert_eq!(
        code(b"<a>&#0;</a>", constrained()),
        Some("io.xml.reference_invalid")
    );
}

#[test]
fn unclosed_and_mismatched_elements_are_refused() {
    assert_eq!(
        code(b"<a><b></a>", constrained()),
        Some("io.xml.structure_invalid")
    );
    assert_eq!(code(b"<a><b>", constrained()), Some("io.xml.truncated"));
    assert_eq!(code(b"", constrained()), Some("io.xml.truncated"));
}

#[test]
fn a_second_root_and_text_outside_the_root_are_refused() {
    assert_eq!(
        code(b"<a/><b/>", constrained()),
        Some("io.xml.structure_invalid")
    );
    assert_eq!(
        code(b"text<a/>", constrained()),
        Some("io.xml.structure_invalid")
    );
}

#[test]
fn duplicate_attributes_are_refused() {
    assert_eq!(
        code(b"<a x=\"1\" x=\"2\"/>", constrained()),
        Some("io.xml.structure_invalid")
    );
}

#[test]
fn an_event_flood_is_refused_by_record_count() {
    let limits = Limits {
        record_count: 10,
        ..constrained()
    };
    let document = "<a/>".repeat(100);
    let document = format!("<r>{document}</r>");
    assert_eq!(code(document.as_bytes(), limits), Some("io.limit_exceeded"));
}

#[test]
fn an_input_beyond_the_input_limit_is_refused() {
    let limits = Limits {
        input_bytes: 8,
        ..constrained()
    };
    assert_eq!(
        code(b"<a>0123456789</a>", limits),
        Some("io.limit_exceeded")
    );
}

fn valid_document() -> Vec<u8> {
    b"<?xml version=\"1.0\"?><root xmlns:p=\"urn:p\" id=\"1\"><p:item k=\"&amp;v\">text &lt; more</p:item><empty/></root>".to_vec()
}

fn read_to_end(document: &[u8]) {
    let limits = Limits {
        input_bytes: 1 << 16,
        string_bytes: 1 << 12,
        record_count: 10_000,
        nesting_depth: 32,
        array_length: 1000,
        ..constrained()
    };
    let code = code(document, limits);
    if let Some(code) = code {
        assert!(
            code.starts_with("io.") || code.starts_with("base."),
            "{code}"
        );
    }
}

#[test]
fn the_valid_document_reads_to_its_end() {
    assert_eq!(code(&valid_document(), constrained()), None);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
        read_to_end(&bytes);
    }

    #[test]
    fn mutations_of_a_valid_document_never_panic(
        edits in proptest::collection::vec((any::<proptest::sample::Index>(), any::<u8>()), 1..6),
        cut in any::<proptest::sample::Index>(),
    ) {
        let mut bytes = valid_document();
        for (at, value) in edits {
            let position = at.index(bytes.len());
            if let Some(byte) = bytes.get_mut(position) {
                *byte = value;
            }
        }
        let keep = cut.index(bytes.len() + 1);
        bytes.truncate(keep);
        read_to_end(&bytes);
    }
}

const SYNTAX: &str = "io.xml.syntax";
const STRUCTURE: &str = "io.xml.structure_invalid";
const PROCESSING: &str = "io.xml.processing_instruction_refused";

#[test]
fn an_empty_namespace_prefix_is_a_structure_error() {
    assert_eq!(code(b"<a xmlns:=\"u\"/>", constrained()), Some(STRUCTURE));
}

#[test]
fn a_declaration_is_accepted_only_as_the_first_event() {
    assert_eq!(
        code(b"<a><?xml version=\"1.0\"?></a>", constrained()),
        Some(PROCESSING)
    );
    assert_eq!(
        code(
            b"<?xml version=\"1.0\"?><?xml version=\"1.0\"?><a/>",
            constrained()
        ),
        Some(PROCESSING)
    );
    assert_eq!(
        code(b" <?xml version=\"1.0\"?><a/>", constrained()),
        Some(PROCESSING)
    );
}

#[test]
fn a_less_than_sign_in_an_attribute_value_is_refused() {
    assert_eq!(code(b"<a x=\"a<b\"/>", constrained()), Some(SYNTAX));
}

#[test]
fn the_cdata_end_marker_in_character_data_is_refused() {
    assert_eq!(code(b"<a>x]]>y</a>", constrained()), Some(SYNTAX));
    assert_eq!(code(b"<a><![CDATA[x]]></a>", constrained()), None);
    assert_eq!(code(b"<a>x]]&gt;y</a>", constrained()), None);
}

#[test]
fn names_outside_the_xml_name_grammar_are_refused() {
    for document in [
        &b"<a\x01/>"[..],
        b"<1a/>",
        b"<a!b/>",
        b"<a x\x01=\"1\"/>",
        b"<-a/>",
        b"<a\xC2\xA0b/>",
    ] {
        assert_eq!(code(document, constrained()), Some(SYNTAX), "{document:?}");
    }
}

#[test]
fn a_misplaced_colon_in_a_qualified_name_is_a_structure_error() {
    for document in [&b"<a:/>"[..], b"<:a/>", b"<a:b:c/>", b"<a x:=\"1\"/>"] {
        assert_eq!(
            code(document, constrained()),
            Some(STRUCTURE),
            "{document:?}"
        );
    }
}

#[test]
fn content_other_than_white_space_outside_the_root_is_refused() {
    for document in [
        &b"<![CDATA[ ]]><a/>"[..],
        b"&#32;<a/>",
        b"<a/>&#32;",
        b"<a/><![CDATA[x]]>",
        b"<a/>text",
    ] {
        assert_eq!(
            code(document, constrained()),
            Some(STRUCTURE),
            "{document:?}"
        );
    }
}
