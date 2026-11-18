//! Groups, their typed values, handles and format versions.

/// The type of the value of a group code, per the published group-code ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// A string (names, text, handles in hexadecimal).
    Text,
    /// A 64-bit floating-point number.
    Double,
    /// An 8-bit integer, 0 to 255 (codes 280–289).
    Int8,
    /// A 16-bit integer.
    Int16,
    /// A 32-bit integer.
    Int32,
    /// A 64-bit integer.
    Int64,
    /// A Boolean flag (codes 290–299).
    Bool,
    /// A binary chunk, hexadecimal in ASCII files.
    Bytes,
}

/// The value type of a group code, or `None` for a code outside the published ranges. ASCII
/// files keep unknown codes as text; binary files cannot be read past them.
#[must_use]
pub fn value_kind(code: i16) -> Option<ValueKind> {
    use ValueKind::{Bool, Bytes, Double, Int8, Int16, Int32, Int64, Text};
    Some(match code {
        0..=9
        | 100..=102
        | 105
        | 300..=309
        | 320..=369
        | 390..=399
        | 410..=419
        | 430..=439
        | 470..=481
        | 999
        | 1000..=1003
        | 1005..=1009 => Text,
        10..=59 | 110..=149 | 210..=239 | 460..=469 | 1010..=1059 => Double,
        60..=79 | 170..=179 | 270..=279 | 370..=389 | 400..=409 | 1060..=1070 => Int16,
        90..=99 | 420..=429 | 440..=459 | 1071 => Int32,
        160..=169 => Int64,
        280..=289 => Int8,
        290..=299 => Bool,
        310..=319 | 1004 => Bytes,
        _ => return None,
    })
}

/// A typed group value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Text, decoded to UTF-8.
    Text(String),
    /// A finite number.
    Double(f64),
    /// An integer of any integer kind.
    Int(i64),
    /// A Boolean flag.
    Bool(bool),
    /// A binary chunk.
    Bytes(Vec<u8>),
}

/// One group: a group code and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// The group code.
    pub code: i16,
    /// The value, typed by the code's [`ValueKind`].
    pub value: Value,
}

impl Group {
    /// A text group.
    #[must_use]
    pub fn text(code: i16, text: impl Into<String>) -> Self {
        Self {
            code,
            value: Value::Text(text.into()),
        }
    }

    /// An integer group.
    #[must_use]
    pub fn int(code: i16, value: i64) -> Self {
        Self {
            code,
            value: Value::Int(value),
        }
    }

    /// A number group.
    #[must_use]
    pub fn double(code: i16, value: f64) -> Self {
        Self {
            code,
            value: Value::Double(value),
        }
    }

    /// The text of a text group.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match &self.value {
            Value::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The handle a text group holds in hexadecimal.
    #[must_use]
    pub fn as_handle(&self) -> Option<Handle> {
        let text = self.as_text()?.trim();
        if text.is_empty() || text.len() > 16 {
            return None;
        }
        u64::from_str_radix(text, 16).ok().map(Handle)
    }

    pub(crate) fn marker(name: &str) -> Self {
        Self::text(100, name)
    }
}

/// A DXF handle: the source identifier of a record in its file, written in hexadecimal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(pub u64);

impl Handle {
    /// The hexadecimal form used in files and loss reports.
    #[must_use]
    pub fn to_hex(self) -> String {
        format!("{:X}", self.0)
    }
}

/// The format versions this crate knows, identified by the `$ACADVER` header variable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Version {
    /// `AC1009`, also assumed when the header has no version.
    #[default]
    R12,
    /// `AC1012`.
    R13,
    /// `AC1014`.
    R14,
    /// `AC1015`.
    R2000,
    /// `AC1018`.
    R2004,
    /// `AC1021`, the first version with UTF-8 text.
    R2007,
    /// `AC1024`.
    R2010,
    /// `AC1027`.
    R2013,
    /// `AC1032`.
    R2018,
}

impl Version {
    /// Every version, oldest first.
    pub const ALL: [Self; 9] = [
        Self::R12,
        Self::R13,
        Self::R14,
        Self::R2000,
        Self::R2004,
        Self::R2007,
        Self::R2010,
        Self::R2013,
        Self::R2018,
    ];

    /// The versions the writer produces.
    pub const WRITABLE: [Self; 6] = [
        Self::R2000,
        Self::R2004,
        Self::R2007,
        Self::R2010,
        Self::R2013,
        Self::R2018,
    ];

    /// The `$ACADVER` value.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::R12 => "AC1009",
            Self::R13 => "AC1012",
            Self::R14 => "AC1014",
            Self::R2000 => "AC1015",
            Self::R2004 => "AC1018",
            Self::R2007 => "AC1021",
            Self::R2010 => "AC1024",
            Self::R2013 => "AC1027",
            Self::R2018 => "AC1032",
        }
    }

    /// The release name used in loss reports.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::R12 => "R12",
            Self::R13 => "R13",
            Self::R14 => "R14",
            Self::R2000 => "2000",
            Self::R2004 => "2004",
            Self::R2007 => "2007",
            Self::R2010 => "2010",
            Self::R2013 => "2013",
            Self::R2018 => "2018",
        }
    }

    /// The version of an `$ACADVER` value.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.code() == code.trim())
    }

    /// Whether text is UTF-8 rather than a legacy code page.
    #[must_use]
    pub fn is_unicode(self) -> bool {
        self >= Self::R2007
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_kinds_follow_the_published_ranges() {
        assert_eq!(value_kind(0), Some(ValueKind::Text));
        assert_eq!(value_kind(39), Some(ValueKind::Double));
        assert_eq!(value_kind(62), Some(ValueKind::Int16));
        assert_eq!(value_kind(90), Some(ValueKind::Int32));
        assert_eq!(value_kind(160), Some(ValueKind::Int64));
        assert_eq!(value_kind(281), Some(ValueKind::Int8));
        assert_eq!(value_kind(290), Some(ValueKind::Bool));
        assert_eq!(value_kind(310), Some(ValueKind::Bytes));
        assert_eq!(value_kind(330), Some(ValueKind::Text));
        assert_eq!(value_kind(1071), Some(ValueKind::Int32));
        assert_eq!(value_kind(80), None);
        assert_eq!(value_kind(-1), None);
    }

    #[test]
    fn versions_parse_by_code() {
        for v in Version::ALL {
            assert_eq!(Version::from_code(v.code()), Some(v));
        }
        assert_eq!(Version::from_code("AC1006"), None);
        assert!(Version::R2007.is_unicode() && !Version::R2004.is_unicode());
    }

    #[test]
    fn handles_parse_from_hex_text() {
        assert_eq!(Group::text(5, "1A").as_handle(), Some(Handle(26)));
        assert_eq!(Group::text(5, "zz").as_handle(), None);
        assert_eq!(Group::text(5, "").as_handle(), None);
        assert_eq!(Group::text(5, "11112222333344445").as_handle(), None);
        assert_eq!(Handle(255).to_hex(), "FF");
    }
}
