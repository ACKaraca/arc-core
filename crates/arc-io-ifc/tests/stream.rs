//! Streaming read: the same instances, spans and header as the in-memory reader, whatever the
//! chunking of the source; a window that stays small on a large file; and bounded failures.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "test code: fixtures and assertions"
)]

use std::io::{Cursor, Read};

use arc_base::{Cancel, Limits};
use arc_io_ifc::stream::{StreamFile, StreamIndex, StreamLimits, scan};
use arc_io_ifc::{IfcFile, ReadOptions, read};

/// A file with the awkward corners of the grammar: comments in every place, strings with
/// semicolons, apostrophes and comment markers, a complex instance, several data sections, an
/// anchor section, a binary literal, and trailing bytes.
fn awkward() -> Vec<u8> {
    let mut text = String::from(
        "ISO-10303-21;\n/* a ; comment */\nHEADER;\nFILE_DESCRIPTION(('x;y'),'2;1');\n\
         FILE_NAME('a''b.ifc','',(),(),'','','');\nFILE_SCHEMA(('IFC4X3_ADD2'));\nENDSEC;\n\
         ANCHOR;\n#99=IFCPROJECT('skipped');\nENDSEC;\n\
         DATA;\n/* c */ #1=IFCPROJECT('0YvctVUKr0kugbFTf53O9L',$,'a;b''c /* not a comment */',$,$,$,$,$,$);\n\
         #2 = ( IFCSITE('s',$,$,$,$,$,$,$,$,$,$,$,$,$) IFCSPATIALELEMENT($) );\n\
         #3=IFCLABELHOLDER(\"0A1B\",(1.5E-3,-2,.T.,*),IFCLABEL('é'));\nENDSEC;\n\
         DATA;\n#10=IFCWALL('w',$,$,$,$,$,$,$,$);\n/* last */ ENDSEC;\nEND-ISO-10303-21;\ntrailing\n",
    );
    for i in 11..200 {
        text.push_str("");
        let _ = i;
    }
    // Move the trailing instances into the second data section by rebuilding it.
    let body: String = (11..200)
        .map(|i| format!("#{i}=IFCCARTESIANPOINT(({i}.,0.,0.));\n"))
        .collect();
    text = text.replace("/* last */ ENDSEC;", &format!("{body}/* last */ ENDSEC;"));
    text.into_bytes()
}

fn limits() -> (Limits, StreamLimits, Cancel) {
    (Limits::DEFAULT, StreamLimits::default(), Cancel::new())
}

/// Delivers a source in pieces of `step` bytes.
struct Choppy<'a>(&'a [u8], usize);

impl Read for Choppy<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.1.min(self.0.len()).min(out.len());
        out[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}

fn assert_same(file: &IfcFile, bytes: &[u8], chunk: usize) {
    let (l, s, c) = limits();
    let mut stream = StreamFile::open(Cursor::new(bytes.to_vec()), &l, &s, &c).unwrap();
    assert_eq!(stream.index().len(), file.instances().len());
    assert_eq!(stream.index().schema(), &file.schema);
    let header: Vec<_> = stream
        .index()
        .summary()
        .header
        .iter()
        .map(|(r, _)| r.clone())
        .collect();
    assert_eq!(header, file.header);
    for instance in file.instances() {
        assert_eq!(
            stream.instance(instance.name).unwrap().as_ref(),
            Some(instance)
        );
        assert_eq!(stream.index().span(instance.name), file.span(instance.name));
    }
    assert_eq!(stream.instance(123_456).unwrap(), None);
    for type_name in [
        "IFCPROJECT",
        "IFCSITE",
        "IFCSPATIALELEMENT",
        "IFCWALL",
        "IFCCARTESIANPOINT",
    ] {
        let streamed: Vec<u64> = stream.index().instances_of(type_name).to_vec();
        let loaded: Vec<u64> = file.instances_of(type_name).map(|i| i.name).collect();
        assert_eq!(streamed, loaded, "{type_name}");
    }
    // The same through a source that hands out `chunk` bytes at a time, and by scan.
    let mut seen = Vec::new();
    let summary = scan(&mut Choppy(bytes, chunk), &l, &s, &c, |i, span| {
        seen.push((i.clone(), span));
        Ok(())
    })
    .unwrap();
    assert_eq!(summary.instances as usize, file.instances().len());
    assert_eq!(seen.len(), file.instances().len());
    // `scan` visits in file order, which the in-memory reader keeps.
    for ((instance, span), expected) in seen.iter().zip(file.instances()) {
        assert_eq!(instance, expected);
        assert_eq!(Some(span.clone()), file.span(expected.name));
    }
}

#[test]
fn the_stream_gives_what_the_reader_gives() {
    let bytes = awkward();
    let file = read(&bytes, &mut ReadOptions::default()).unwrap().file;
    assert_eq!(file.instances().len(), 193);
    for chunk in [1, 2, 3, 7, 64, 1000, 1 << 20] {
        assert_same(&file, &bytes, chunk);
    }
}

#[test]
fn the_window_stays_small_on_a_large_file() {
    // About 60 MB of instances; the window holds a few chunks and the index a few bytes each.
    let mut text = String::from(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('a','',(),(),'','','');\n\
         FILE_SCHEMA(('IFC4X3_ADD2'));\nENDSEC;\nDATA;\n",
    );
    let count = 250_000;
    for i in 1..=count {
        text.push_str(&format!(
            "#{i}=IFCWALL('0YvctVUKr0kugbFTf53O{}',$,'Wall {i}',$,$,$,$,$,$);\n",
            i % 10
        ));
    }
    text.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    let bytes = text.into_bytes();
    let (l, s, c) = limits();
    let index = StreamIndex::build(&mut Choppy(&bytes, 100_000), &l, &s, &c).unwrap();
    assert_eq!(index.len(), count);
    let window = index.summary().peak_window_bytes;
    assert!(window < 400_000, "window {window}");
    let file_memory = read(&bytes, &mut ReadOptions::default())
        .unwrap()
        .file
        .parsed_bytes();
    assert!(
        index.estimated_bytes() < file_memory / 3,
        "{} vs {file_memory}",
        index.estimated_bytes()
    );
    // The memory limit applies to the index.
    let tight = Limits {
        memory_bytes: Some(1_000_000),
        ..Limits::DEFAULT
    };
    let error = StreamIndex::build(&mut Choppy(&bytes, 100_000), &tight, &s, &c).unwrap_err();
    assert_eq!(error.code(), "io.limit_exceeded");
}

#[test]
fn malformed_and_hostile_sources_fail_with_codes() {
    let (l, s, c) = limits();
    let good = awkward();
    let code = |bytes: &[u8], l: &Limits, s: &StreamLimits| {
        StreamIndex::build(&mut Cursor::new(bytes.to_vec()), l, s, &c)
            .unwrap_err()
            .code()
    };
    // Truncated anywhere inside the data.
    for cut in [10, 120, 300, good.len() - 100] {
        assert!(
            code(&good[..cut], &l, &s).starts_with("io.ifc."),
            "cut {cut}"
        );
    }
    // Not a physical file.
    assert_eq!(code(b"hello;", &l, &s), "io.ifc.structure_invalid");
    // Duplicate names.
    let dup = String::from_utf8(good.clone())
        .unwrap()
        .replace("#10=", "#1=");
    assert_eq!(code(dup.as_bytes(), &l, &s), "io.ifc.instance_duplicate");
    // A syntax error inside a statement.
    let bad = String::from_utf8(good.clone())
        .unwrap()
        .replace("#10=IFCWALL('w',$", "#10=IFCWALL('w',(,$");
    assert!(code(bad.as_bytes(), &l, &s).starts_with("io.ifc."));
    // A statement that never ends: refused at the window limit, not buffered whole.
    let mut endless = b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1=IFCWALL('".to_vec();
    endless.extend(std::iter::repeat_n(b'x', 5_000_000));
    let small = StreamLimits {
        statement_bytes: 1_000_000,
    };
    assert_eq!(code(&endless, &l, &small), "io.limit_exceeded");
    // Limits of the profile.
    let few = Limits {
        record_count: 5,
        ..Limits::DEFAULT
    };
    assert_eq!(code(&good, &few, &s), "io.limit_exceeded");
    let short = Limits {
        input_bytes: 100,
        ..Limits::DEFAULT
    };
    assert_eq!(code(&good, &short, &s), "io.limit_exceeded");
    let stop = Cancel::new();
    stop.cancel();
    let error = StreamIndex::build(&mut Cursor::new(good), &l, &s, &stop).unwrap_err();
    assert_eq!(error.code(), "base.cancelled");
}
