//! Reading well-formed documents: events, namespaces, references and line ends come out as
//! specified.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use arc_io_common::ReadOptions;
use arc_io_common::xml::{Attribute, Element, Event, XmlReader, split_qualified_name};

/// All events of a document, or the code of the first error.
fn events(document: &[u8]) -> Result<Vec<Event>, &'static str> {
    let mut reader = XmlReader::new(document, &ReadOptions::default()).map_err(|e| e.code())?;
    let mut out = Vec::new();
    while let Some(event) = reader.next_event().map_err(|e| e.code())? {
        out.push(event);
    }
    Ok(out)
}

fn start(name: &str, attributes: &[(&str, &str)]) -> Event {
    Event::Start(Element {
        name: name.to_owned(),
        attributes: attributes
            .iter()
            .map(|(n, v)| Attribute {
                name: (*n).to_owned(),
                value: (*v).to_owned(),
            })
            .collect(),
    })
}

#[test]
fn a_small_document_reads_as_its_events() {
    let document = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root a=\"1\"><item>text</item><empty/></root>";
    assert_eq!(
        events(document),
        Ok(vec![
            start("root", &[("a", "1")]),
            start("item", &[]),
            Event::Text("text".to_owned()),
            Event::End("item".to_owned()),
            start("empty", &[]),
            Event::End("empty".to_owned()),
            Event::End("root".to_owned()),
        ])
    );
}

#[test]
fn comments_and_the_declaration_are_not_passed_on() {
    let document = b"<?xml version=\"1.0\"?><!-- note --><a><!-- inner -->x</a>";
    assert_eq!(
        events(document),
        Ok(vec![
            start("a", &[]),
            Event::Text("x".to_owned()),
            Event::End("a".to_owned()),
        ])
    );
}

#[test]
fn the_five_predefined_entities_and_character_references_decode() {
    let document = b"<a t=\"&lt;&amp;&#x41;\">&quot;&apos;&gt;&#66;&#x1F600;</a>";
    assert_eq!(
        events(document),
        Ok(vec![
            start("a", &[("t", "<&A")]),
            Event::Text("\"'>B\u{1F600}".to_owned()),
            Event::End("a".to_owned()),
        ])
    );
}

#[test]
fn cdata_is_literal_text_and_joins_its_neighbours() {
    let document = b"<a>one &amp; <![CDATA[<two> & three]]> four</a>";
    assert_eq!(
        events(document),
        Ok(vec![
            start("a", &[]),
            Event::Text("one & <two> & three four".to_owned()),
            Event::End("a".to_owned()),
        ])
    );
}

#[test]
fn namespace_declarations_resolve_in_scope_and_end_with_their_element() {
    let document = b"<p:a xmlns:p=\"urn:one\" xmlns=\"urn:default\"><b/></p:a>";
    let mut reader = XmlReader::new(document, &ReadOptions::default()).unwrap();
    assert_eq!(reader.resolve_prefix("p"), None);
    let first = reader.next_event().unwrap();
    assert!(matches!(first, Some(Event::Start(ref e)) if e.name == "p:a"));
    assert_eq!(reader.resolve_prefix("p"), Some("urn:one"));
    assert_eq!(reader.resolve_prefix(""), Some("urn:default"));
    assert_eq!(
        reader.resolve_prefix("xml"),
        Some(arc_io_common::xml::XML_NAMESPACE)
    );
    while reader.next_event().unwrap().is_some() {}
    assert_eq!(reader.resolve_prefix("p"), None);
}

#[test]
fn qualified_names_split_into_prefix_and_local_name() {
    assert_eq!(split_qualified_name("p:item"), ("p", "item"));
    assert_eq!(split_qualified_name("item"), ("", "item"));
}

#[test]
fn line_ends_are_normalized_in_text() {
    let document = b"<a>one\r\ntwo\rthree</a>";
    assert_eq!(
        events(document),
        Ok(vec![
            start("a", &[]),
            Event::Text("one\ntwo\nthree".to_owned()),
            Event::End("a".to_owned()),
        ])
    );
}

#[test]
fn a_utf8_byte_order_mark_is_skipped() {
    let document = b"\xEF\xBB\xBF<a/>";
    assert_eq!(
        events(document),
        Ok(vec![start("a", &[]), Event::End("a".to_owned())])
    );
}

#[test]
fn a_declared_latin1_encoding_is_refused() {
    let document = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>";
    assert_eq!(events(document), Err("io.xml.encoding_unsupported"));
}

#[test]
fn a_comment_ends_a_text_node() {
    let document = b"<a>one<!-- c -->two</a>";
    assert_eq!(
        events(document),
        Ok(vec![
            start("a", &[]),
            Event::Text("one".to_owned()),
            Event::Text("two".to_owned()),
            Event::End("a".to_owned()),
        ])
    );
}

#[test]
fn white_space_outside_the_root_is_not_delivered() {
    assert_eq!(
        events(b"\n<a/>\r\n"),
        Ok(vec![start("a", &[]), Event::End("a".to_owned())])
    );
}

#[test]
fn valid_xml_names_are_accepted() {
    assert_eq!(
        events("<caf\u{e9}.x-1 _y:z=\"1\"/>".as_bytes()),
        Ok(vec![
            start("caf\u{e9}.x-1", &[("_y:z", "1")]),
            Event::End("caf\u{e9}.x-1".to_owned()),
        ])
    );
}
