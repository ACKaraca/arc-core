//! Font providers and font substitution (CORE-DRAFT-036).
//!
//! Products register font sources through a [`FontProvider`]: stroke fonts by name and outline
//! font bytes by family name. [`FontRegistry`] is the provider of this crate; it always holds
//! the shipped stroke font. [`Fonts`] is what layout and display lists receive: the provider,
//! the per-user substitution table and the name of the default stroke font. The per-document
//! table is the [`FONT_TABLE`] object of the document.
//!
//! [`resolve`] maps a requested font name to an available font: the name itself when the
//! provider has it, else the substitute the document table names, else the one the user table
//! names, else the default stroke font. Every step taken is reported as a [`Substitution`].
//! Names compare without regard to case (Unicode lowercase). Nothing here looks at the fonts
//! installed on the system: fonts come only from the provider.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Error, ObjectId, Result};
use arc_doc::{
    Change, DocumentValidator, FieldDecl, Finding, Severity, Snapshot, Value, ValueType,
};

use crate::error::codes;
use crate::stroke_font::{BUILTIN_NAME, StrokeFont, builtin};
use crate::types::{NAME_BYTES, text_at};

/// The type of the document's font substitution table: `entries`, a list of records
/// `requested`, `substitute`.
pub const FONT_TABLE: &str = "core.draft.font_table";

/// The identifier of a document's font substitution table: one per document, created by
/// `core.draft.font.substitute`. Objects of [`FONT_TABLE`] with another identifier are ignored.
pub const FONT_TABLE_ID: u128 = 0x0000_0000_0000_7000_8000_0000_0000_0f01;

/// The most entries of a substitution table.
pub const MAX_SUBSTITUTIONS: u64 = 4096;

/// The identifier of the document's font substitution table.
#[must_use]
pub fn font_table_id() -> ObjectId {
    ObjectId::from_u128(FONT_TABLE_ID)
}

/// The family name of the bundled outline font (feature `outline-text`).
#[cfg(feature = "outline-text")]
pub const BUNDLED_OUTLINE_FAMILY: &str = "Noto Sans";

/// The bundled outline font: Noto Sans, regular weight only, under the SIL Open Font License 1.1
/// (`fonts/NotoSans-OFL.txt`). It is the substitute for an outline font whose bytes cannot be
/// read, and the default outline family of [`FontRegistry::with_builtin`].
#[cfg(feature = "outline-text")]
#[must_use]
pub fn bundled_outline() -> OutlineFont {
    static BYTES: std::sync::OnceLock<Arc<[u8]>> = std::sync::OnceLock::new();
    let data =
        BYTES.get_or_init(|| Arc::from(&include_bytes!("../fonts/NotoSans-Regular.ttf")[..]));
    OutlineFont {
        family: BUNDLED_OUTLINE_FAMILY.to_owned(),
        data: Arc::clone(data),
        index: 0,
    }
}

/// Outline font bytes registered under a family name.
#[derive(Clone, PartialEq, Eq)]
pub struct OutlineFont {
    /// The family name text refers to.
    pub family: String,
    /// The font file (OpenType or TrueType).
    pub data: Arc<[u8]>,
    /// The face index in a collection file; 0 otherwise.
    pub index: u32,
}

impl fmt::Debug for OutlineFont {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutlineFont")
            .field("family", &self.family)
            .field("bytes", &self.data.len())
            .field("index", &self.index)
            .finish()
    }
}

/// A source of fonts that products implement or fill ([`FontRegistry`]).
pub trait FontProvider: Send + Sync + fmt::Debug {
    /// The stroke font named `name`, if the provider has it.
    fn stroke_font(&self, name: &str) -> Option<Arc<StrokeFont>>;
    /// The outline font of the family `family`, if the provider has it.
    fn outline_font(&self, family: &str) -> Option<OutlineFont>;
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// A provider holding registered fonts, keyed by their lowercase names.
#[derive(Debug, Clone, Default)]
pub struct FontRegistry {
    stroke: BTreeMap<String, Arc<StrokeFont>>,
    outline: BTreeMap<String, OutlineFont>,
}

impl FontRegistry {
    /// A registry holding the shipped stroke font under [`BUILTIN_NAME`] and, with the feature
    /// `outline-text`, the bundled outline font under [`BUNDLED_OUTLINE_FAMILY`].
    ///
    /// # Errors
    ///
    /// `draft.font.default_missing` if the shipped font cannot be read.
    pub fn with_builtin() -> Result<Self> {
        let mut registry = Self::default();
        registry.add_stroke(BUILTIN_NAME, builtin()?);
        #[cfg(feature = "outline-text")]
        {
            let bundled = bundled_outline();
            registry.add_outline(&bundled.family, bundled.data, bundled.index);
        }
        Ok(registry)
    }

    /// Registers a stroke font under `name`, replacing one of the same name.
    pub fn add_stroke(&mut self, name: &str, font: Arc<StrokeFont>) {
        self.stroke.insert(key(name), font);
    }

    /// Registers outline font bytes under `family`, replacing one of the same family. The bytes
    /// are checked when text is laid out with them.
    pub fn add_outline(&mut self, family: &str, data: Arc<[u8]>, index: u32) {
        self.outline.insert(
            key(family),
            OutlineFont {
                family: family.to_owned(),
                data,
                index,
            },
        );
    }
}

impl FontProvider for FontRegistry {
    fn stroke_font(&self, name: &str) -> Option<Arc<StrokeFont>> {
        self.stroke.get(&key(name)).cloned()
    }
    fn outline_font(&self, family: &str) -> Option<OutlineFont> {
        self.outline.get(&key(family)).cloned()
    }
}

/// A substitution table: requested name to substitute name, compared without regard to case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Substitutions {
    entries: BTreeMap<String, String>,
}

impl Substitutions {
    /// A table of `(requested, substitute)` pairs; a later pair for the same name wins.
    #[must_use]
    pub fn new<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            entries: pairs
                .into_iter()
                .map(|(r, s)| (key(r), s.to_owned()))
                .collect(),
        }
    }

    /// The substitute for `requested`, if the table names one.
    #[must_use]
    pub fn get(&self, requested: &str) -> Option<&str> {
        self.entries.get(&key(requested)).map(String::as_str)
    }

    /// The table held in the `entries` value of a [`FONT_TABLE`] object.
    #[must_use]
    pub fn from_value(entries: Option<&Value>) -> Self {
        let mut pairs = Vec::new();
        if let Some(Value::List(items)) = entries {
            for item in items {
                if let Value::Record(fields) = item
                    && let (Some(r), Some(s)) = (text_at(fields, 0), text_at(fields, 1))
                {
                    pairs.push((r, s));
                }
            }
        }
        Self::new(pairs)
    }

    /// The `entries` value of a [`FONT_TABLE`] object holding this table.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::List(
            self.entries
                .iter()
                .map(|(r, s)| {
                    Value::Record(vec![
                        Some(Value::String(r.clone())),
                        Some(Value::String(s.clone())),
                    ])
                })
                .collect(),
        )
    }

    /// Sets (`Some`) or removes (`None`) the substitute of `requested`.
    pub fn set(&mut self, requested: &str, substitute: Option<&str>) {
        match substitute {
            Some(s) => self.entries.insert(key(requested), s.to_owned()),
            None => self.entries.remove(&key(requested)),
        };
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The document's substitution table read from `snapshot`; empty when it has none.
#[must_use]
pub fn document_table(snapshot: &Snapshot) -> Substitutions {
    let entries = snapshot
        .resolve(font_table_id())
        .ok()
        .and_then(|h| snapshot.get(h).ok())
        .and_then(|o| o.values.first().cloned().flatten());
    Substitutions::from_value(entries.as_ref())
}

/// What layout receives: the provider, the per-user table and the default stroke font.
#[derive(Clone)]
pub struct Fonts {
    /// The font sources.
    pub provider: Arc<dyn FontProvider>,
    /// The per-user substitution table, consulted after the document's.
    pub user: Substitutions,
    /// The name of the stroke font every unresolved name ends at.
    pub default_stroke: String,
}

impl fmt::Debug for Fonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fonts")
            .field("provider", &self.provider)
            .field("user", &self.user)
            .field("default_stroke", &self.default_stroke)
            .finish()
    }
}

impl PartialEq for Fonts {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.provider, &other.provider)
            && self.user == other.user
            && self.default_stroke == other.default_stroke
    }
}

impl Default for Fonts {
    /// A [`FontRegistry`] with the shipped font, no user table, and the shipped font as the
    /// default. If the shipped font cannot be read the registry is empty, and resolution fails
    /// with `draft.font.default_missing`.
    fn default() -> Self {
        Self {
            provider: Arc::new(FontRegistry::with_builtin().unwrap_or_default()),
            user: Substitutions::default(),
            default_stroke: BUILTIN_NAME.to_owned(),
        }
    }
}

/// A font ready for layout.
#[derive(Debug, Clone, PartialEq)]
pub enum FontSource {
    /// A stroke font.
    Stroke(Arc<StrokeFont>),
    /// Outline font bytes.
    Outline(OutlineFont),
}

impl FontSource {
    /// Whether it is a stroke font.
    #[must_use]
    pub fn is_stroke(&self) -> bool {
        matches!(self, Self::Stroke(_))
    }
}

/// Which rule chose a substitute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubstitutedBy {
    /// The document's table.
    Document,
    /// The user's table.
    User,
    /// The default stroke font.
    Default,
    /// The bundled outline family, which stands in for an outline font whose bytes do not parse
    /// as a font (feature `outline-text`).
    Bundled,
    /// The text's base font: a text lays out with one kind of font (stroke or outline), the
    /// kind of its base font, so a run naming the other kind uses the base font.
    Base,
}

/// A font name that was not used as requested.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Substitution {
    /// The requested name.
    pub requested: String,
    /// The name used instead.
    pub used: String,
    /// The rule that chose it.
    pub by: SubstitutedBy,
}

/// The font a requested name resolved to.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The name of the font used.
    pub name: String,
    /// The font.
    pub source: FontSource,
    /// The substitution, when the requested name was not available.
    pub substitution: Option<Substitution>,
}

/// The font of `name` in the provider: a stroke font first, then outline bytes when the
/// `outline-text` feature lays them out.
fn available(fonts: &Fonts, name: &str) -> Option<FontSource> {
    if let Some(font) = fonts.provider.stroke_font(name) {
        return Some(FontSource::Stroke(font));
    }
    if cfg!(feature = "outline-text") {
        return fonts.provider.outline_font(name).map(FontSource::Outline);
    }
    None
}

/// The bundled outline font in place of the requested outline font `source` when its bytes do not
/// parse (the provider's font cannot be loaded); `None` when it reads or the feature is off.
#[cfg(feature = "outline-text")]
fn unreadable(requested: &str, source: &FontSource) -> Option<Resolution> {
    let FontSource::Outline(font) = source else {
        return None;
    };
    if skrifa::FontRef::from_index(&font.data, font.index).is_ok() {
        return None;
    }
    Some(Resolution {
        name: BUNDLED_OUTLINE_FAMILY.to_owned(),
        source: FontSource::Outline(bundled_outline()),
        substitution: Some(Substitution {
            requested: requested.to_owned(),
            used: BUNDLED_OUTLINE_FAMILY.to_owned(),
            by: SubstitutedBy::Bundled,
        }),
    })
}

#[cfg(not(feature = "outline-text"))]
fn unreadable(_: &str, _: &FontSource) -> Option<Resolution> {
    None
}

/// Resolves `requested` (absent or blank: the default stroke font, with no substitution) through
/// the document table `document`, then the user table, then the default stroke font. A table
/// entry whose substitute is not available is skipped.
///
/// # Errors
///
/// `draft.font.default_missing` (argument `name`) when the provider lacks the default stroke
/// font and it is needed.
pub fn resolve(
    requested: Option<&str>,
    document: &Substitutions,
    fonts: &Fonts,
) -> Result<Resolution> {
    let default = || -> Result<FontSource> {
        fonts
            .provider
            .stroke_font(&fonts.default_stroke)
            .map(FontSource::Stroke)
            .ok_or_else(|| {
                Error::new(codes::FONT_DEFAULT_MISSING)
                    .with_arg("name", fonts.default_stroke.clone())
            })
    };
    let requested = requested.map(str::trim).filter(|r| !r.is_empty());
    let Some(requested) = requested else {
        return Ok(Resolution {
            name: fonts.default_stroke.clone(),
            source: default()?,
            substitution: None,
        });
    };
    if let Some(source) = available(fonts, requested) {
        if let Some(bundled) = unreadable(requested, &source) {
            return Ok(bundled);
        }
        return Ok(Resolution {
            name: requested.to_owned(),
            source,
            substitution: None,
        });
    }
    let substituted = |used: &str, by, source| Resolution {
        name: used.to_owned(),
        source,
        substitution: Some(Substitution {
            requested: requested.to_owned(),
            used: used.to_owned(),
            by,
        }),
    };
    for (table, by) in [
        (document, SubstitutedBy::Document),
        (&fonts.user, SubstitutedBy::User),
    ] {
        if let Some(used) = table.get(requested)
            && let Some(source) = available(fonts, used)
        {
            return Ok(substituted(used, by, source));
        }
    }
    Ok(substituted(
        &fonts.default_stroke,
        SubstitutedBy::Default,
        default()?,
    ))
}

/// The declaration of [`FONT_TABLE`].
pub(crate) fn table_fields() -> Vec<FieldDecl> {
    let entry = ValueType::Record(vec![
        FieldDecl::new("requested", ValueType::String)
            .with_bound(arc_doc::Bound::Length { max: NAME_BYTES }),
        FieldDecl::new("substitute", ValueType::String)
            .with_bound(arc_doc::Bound::Length { max: NAME_BYTES }),
    ]);
    vec![
        FieldDecl::new("entries", ValueType::List(Box::new(entry))).with_bound(
            arc_doc::Bound::Length {
                max: MAX_SUBSTITUTIONS,
            },
        ),
    ]
}

/// The requested font names of the document: the `font` of every text style and of every run
/// of every rich text, without repetition, in name order.
#[must_use]
pub fn requested_names(snapshot: &Snapshot) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    if let Ok(styles) = snapshot.iter_type(crate::style::TEXT_STYLE) {
        for (_, style) in styles {
            if let Some(font) = text_at(&style.values, crate::text::style_field::FONT) {
                names.insert(font.to_owned());
            }
        }
    }
    if let Ok(texts) = snapshot.iter_type(crate::text::RICH_TEXT) {
        for (_, text) in texts {
            for paragraph in crate::text::paragraphs_of(&text.values) {
                for run in paragraph.runs {
                    names.extend(run.font);
                }
            }
        }
    }
    names.into_iter().collect()
}

/// The substitutions the document needs: one per requested name that resolves through a
/// substitution, in name order.
///
/// # Errors
///
/// `draft.font.default_missing` as [`resolve`].
pub fn substitutions(snapshot: &Snapshot, fonts: &Fonts) -> Result<Vec<Substitution>> {
    let table = document_table(snapshot);
    let mut out = Vec::new();
    for name in requested_names(snapshot) {
        out.extend(resolve(Some(&name), &table, fonts)?.substitution);
    }
    Ok(out)
}

/// The warning that reports `substitution`: `draft.font.substituted` with the arguments
/// `requested` and `used`.
#[must_use]
pub fn substitution_error(substitution: &Substitution) -> Error {
    Error::new(codes::FONT_SUBSTITUTED)
        .with_arg("requested", substitution.requested.clone())
        .with_arg("used", substitution.used.clone())
}

/// The document validator that lists every substitution of the document as a warning
/// (`draft.font.substituted`) on every commit, so the document's diagnostics list always holds
/// the substitutions of the current revision. It never rejects a commit.
#[must_use]
pub fn validator(fonts: Fonts) -> DocumentValidator {
    Arc::new(
        move |_: &Snapshot, candidate: &Snapshot, _: &BTreeMap<ObjectId, Change>| {
            substitutions(candidate, &fonts)
                .map(|list| {
                    list.iter()
                        .map(|s| Finding {
                            severity: Severity::Warning,
                            error: substitution_error(s),
                        })
                        .collect()
                })
                .unwrap_or_else(|error| {
                    vec![Finding {
                        severity: Severity::Warning,
                        error,
                    }]
                })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_compare_without_case_and_round_trip_as_values() {
        let mut table = Substitutions::new([("Old Font", "Arc Stroke")]);
        assert_eq!(table.get("old font"), Some("Arc Stroke"));
        table.set("x", Some("y"));
        assert_eq!(Substitutions::from_value(Some(&table.to_value())), table);
        table.set("X", None);
        assert_eq!(table.len(), 1);
    }
}
