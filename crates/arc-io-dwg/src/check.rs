//! The checks made on the first six bytes before the wrapped library reads the file.

use arc_base::{Error, Result};
use arc_io_dxf::Version;

use crate::error::codes;

/// The version signatures this crate reads, each with its release name. R13 and R14 share the
/// pre-R2000 layout; R2010, R2013 and R2018 share the layout the library calls its newest family.
const SUPPORTED: [(&str, Version); 8] = [
    ("AC1012", Version::R13),
    ("AC1014", Version::R14),
    ("AC1015", Version::R2000),
    ("AC1018", Version::R2004),
    ("AC1021", Version::R2007),
    ("AC1024", Version::R2010),
    ("AC1027", Version::R2013),
    ("AC1032", Version::R2018),
];

/// Reads the version signature. Returns the version and the signature text.
pub(crate) fn signature(bytes: &[u8]) -> Result<(Version, &'static str)> {
    let head = bytes
        .get(..6)
        .ok_or_else(|| Error::new(codes::SIGNATURE_INVALID))?;
    let well_formed = head.starts_with(b"AC")
        && head
            .get(2..)
            .is_some_and(|digits| digits.iter().all(u8::is_ascii_digit));
    if !well_formed {
        return Err(Error::new(codes::SIGNATURE_INVALID));
    }
    let text = String::from_utf8_lossy(head).into_owned();
    match SUPPORTED.iter().find(|(code, _)| *code == text) {
        Some((code, version)) => Ok((*version, code)),
        None => Err(Error::new(codes::VERSION_UNSUPPORTED).with_arg("version", text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_signatures_map_to_their_release() {
        assert_eq!(
            signature(b"AC1032xx").map(|(v, _)| v).ok(),
            Some(Version::R2018)
        );
        assert_eq!(
            signature(b"AC1012").map(|(v, _)| v).ok(),
            Some(Version::R13)
        );
    }

    #[test]
    fn malformed_signatures_are_rejected() {
        for bytes in [&b""[..], b"AC10", b"XX1015", b"AC10A5", b"AD1015"] {
            let err = signature(bytes).err();
            assert_eq!(err.map(|e| e.code()), Some("io.dwg.signature_invalid"));
        }
    }

    #[test]
    fn other_versions_are_unsupported() {
        for code in ["AC1009", "AC1006", "AC1040", "AC1031"] {
            let err = signature(code.as_bytes()).err();
            assert_eq!(err.map(|e| e.code()), Some("io.dwg.version_unsupported"));
        }
    }
}
