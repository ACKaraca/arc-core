//! Validation of entry names. The bytes are decoded first (UTF-8, or the legacy bytes mapped one
//! to one onto the first 256 code points), then checked for control characters, path safety and
//! empty components. The duplicate check is done by the archive, on [`Checked::key`].

use arc_base::{Limits, Result};

use crate::bytes::to_u64;
use crate::error::codes;
use crate::error::{at_entry, limit};

/// A name that passed the checks.
pub(super) struct Checked {
    /// The name as the archive wrote it, with its trailing `/` for a directory.
    pub(super) name: String,
    /// The name without its trailing `/`, lowercased character by character. Two entries with
    /// equal keys would occupy one path.
    pub(super) key: String,
}

/// Checks the raw name bytes of entry `index`. `utf8` is the entry's UTF-8 flag.
pub(super) fn check(raw: &[u8], utf8: bool, limits: &Limits, index: usize) -> Result<Checked> {
    let length = to_u64(raw.len());
    if length > limits.string_bytes {
        return Err(limit("string_bytes", length));
    }
    let name = if utf8 {
        std::str::from_utf8(raw)
            .map_err(|_| at_entry(codes::ZIP_NAME_INVALID, index))?
            .to_owned()
    } else {
        raw.iter().map(|&byte| char::from(byte)).collect()
    };
    check_characters(&name, index)?;
    let body = name.strip_suffix('/').unwrap_or(name.as_str());
    if body.is_empty() {
        return Err(at_entry(codes::ZIP_NAME_INVALID, index));
    }
    for component in body.split('/') {
        match component {
            "" => return Err(at_entry(codes::ZIP_NAME_INVALID, index)),
            "." | ".." => return Err(at_entry(codes::ZIP_PATH_UNSAFE, index)),
            _ => {}
        }
    }
    let key = body.chars().flat_map(char::to_lowercase).collect();
    Ok(Checked { name, key })
}

fn check_characters(name: &str, index: usize) -> Result<()> {
    if name.chars().any(char::is_control) {
        return Err(at_entry(codes::ZIP_NAME_INVALID, index));
    }
    if name.starts_with('/') || name.contains(['\\', ':']) {
        return Err(at_entry(codes::ZIP_PATH_UNSAFE, index));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use arc_base::{Error, Limits};

    use super::check;
    use crate::error::codes;

    fn code_of(raw: &[u8], utf8: bool) -> Option<&'static str> {
        check(raw, utf8, &Limits::CONSTRAINED, 0)
            .err()
            .map(|error: Error| error.code())
    }

    #[test]
    fn plain_and_nested_names_pass() {
        assert_eq!(code_of(b"a.txt", true), None);
        assert_eq!(code_of(b"dir/sub/file.xml", true), None);
        assert_eq!(code_of(b"dir/", true), None);
    }

    #[test]
    fn path_escapes_are_refused_as_unsafe() {
        for raw in [
            &b"../x"[..],
            b"a/../b",
            b"/x",
            b"a\\b",
            b"C:x",
            b"a/./b",
            b"./a",
        ] {
            assert_eq!(
                code_of(raw, true),
                Some(codes::ZIP_PATH_UNSAFE.code),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn malformed_names_are_refused() {
        assert_eq!(code_of(b"", true), Some(codes::ZIP_NAME_INVALID.code));
        assert_eq!(code_of(b"a//b", true), Some(codes::ZIP_NAME_INVALID.code));
        assert_eq!(code_of(b"a\0b", true), Some(codes::ZIP_NAME_INVALID.code));
        assert_eq!(code_of(b"a\x7fb", true), Some(codes::ZIP_NAME_INVALID.code));
        assert_eq!(
            code_of(&[0xFF, b'a'], true),
            Some(codes::ZIP_NAME_INVALID.code)
        );
    }

    #[test]
    fn legacy_bytes_map_one_to_one() {
        let checked = check(&[b'a', 0xE9], false, &Limits::CONSTRAINED, 0).ok();
        assert_eq!(checked.map(|c| c.name), Some("a\u{e9}".to_owned()));
        assert_eq!(code_of(&[0x85], false), Some(codes::ZIP_NAME_INVALID.code));
    }

    #[test]
    fn keys_ignore_case_and_the_trailing_slash() {
        let file = check(b"Dir/A.txt", true, &Limits::CONSTRAINED, 0).ok();
        let folder = check(b"dir/a.TXT/", true, &Limits::CONSTRAINED, 0).ok();
        assert_eq!(file.map(|c| c.key), Some("dir/a.txt".to_owned()));
        assert_eq!(folder.map(|c| c.key), Some("dir/a.txt".to_owned()));
    }

    #[test]
    fn names_over_the_string_limit_hit_the_limit() {
        let limits = Limits {
            string_bytes: 3,
            ..Limits::CONSTRAINED
        };
        let error = check(b"abcd", true, &limits, 0).err();
        assert_eq!(error.map(|e| e.code()), Some(codes::LIMIT_EXCEEDED.code));
    }
}
