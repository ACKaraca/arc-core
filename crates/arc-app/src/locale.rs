//! Locale selection and message formatting (CORE-APP-014).
//!
//! [`Catalogs`] holds parsed Fluent catalogs by language. [`negotiate`] picks the catalog language
//! for a list of requested tags; [`Localizer`] formats messages with English fallback per locale
//! and per message, and [`Localizer::set_requested`] swaps the active language at run time. The
//! operating-system locale list is an input: this module never queries the system.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::error::Value;
use arc_base::{Error, Result};
use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

use crate::codes;

/// The source language and last fallback.
pub const FALLBACK: &str = "en-US";

/// The largest catalog source accepted, in bytes.
pub const MAX_CATALOG_BYTES: usize = 4 << 20;

/// The setting value that means "use the operating-system list".
pub const SYSTEM: &str = "system";

/// Parses a language tag, tolerating operating-system spellings: `tr_TR.UTF-8`, `en_US@euro`.
fn parse_tag(text: &str) -> Option<LanguageIdentifier> {
    let base = text
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    base.parse().ok()
}

/// The Fluent catalogs of the messages the reference shell formats with, for every shipped
/// locale: the sources of the crates' catalogs in `locales/`, concatenated per locale. A product
/// starts from [`shell_catalogs`] and adds its own with [`Catalogs::append`].
pub const SHELL_CATALOGS: &[(&str, &str)] = &[
    (
        "en-US",
        concat!(
            include_str!("../../../locales/en-US/arc-app.ftl"),
            include_str!("../../../locales/en-US/arc-ui.ftl"),
            include_str!("../../../locales/en-US/arc-command.ftl"),
            include_str!("../../../locales/en-US/arc-doc.ftl"),
            include_str!("../../../locales/en-US/arc-gpu.ftl"),
            include_str!("../../../locales/en-US/arc-base.ftl"),
            include_str!("../../../locales/en-US/arc-shell.ftl"),
            include_str!("../../../locales/en-US/arc-solve.ftl"),
            include_str!("../../../locales/en-US/arc-geom.ftl"),
            include_str!("../../../locales/en-US/arc-draft.ftl"),
        ),
    ),
    (
        "tr-TR",
        concat!(
            include_str!("../../../locales/tr-TR/arc-app.ftl"),
            include_str!("../../../locales/tr-TR/arc-ui.ftl"),
            include_str!("../../../locales/tr-TR/arc-command.ftl"),
            include_str!("../../../locales/tr-TR/arc-doc.ftl"),
            include_str!("../../../locales/tr-TR/arc-gpu.ftl"),
            include_str!("../../../locales/tr-TR/arc-base.ftl"),
            include_str!("../../../locales/tr-TR/arc-shell.ftl"),
            include_str!("../../../locales/tr-TR/arc-solve.ftl"),
            include_str!("../../../locales/tr-TR/arc-geom.ftl"),
            include_str!("../../../locales/tr-TR/arc-draft.ftl"),
        ),
    ),
];

/// A catalog set holding the shell's messages for every shipped locale (see [`SHELL_CATALOGS`]).
/// A syntax error in any of them fails: use [`shell_catalogs_with_fallback`] at run time.
///
/// # Errors
/// `app.locale.catalog_invalid` when a catalog does not parse.
pub fn shell_catalogs() -> Result<Catalogs> {
    let mut catalogs = Catalogs::new();
    for (tag, source) in SHELL_CATALOGS {
        catalogs.add(tag, source)?;
    }
    Ok(catalogs)
}

/// The shell's catalogs for a run, where a shipped language whose catalog does not parse is left
/// out rather than failing the start: English must parse, and every other language that fails
/// is returned with its error, so the caller can report it and that language falls back to
/// English. The shell's own tests use [`shell_catalogs`], which fails on any error.
///
/// # Errors
/// The error of the `en-US` catalog, when it does not parse.
pub fn shell_catalogs_with_fallback() -> Result<(Catalogs, Vec<(&'static str, Error)>)> {
    catalogs_from(SHELL_CATALOGS)
}

fn catalogs_from(
    sources: &[(&'static str, &str)],
) -> Result<(Catalogs, Vec<(&'static str, Error)>)> {
    let mut catalogs = Catalogs::new();
    let mut skipped = Vec::new();
    for (tag, source) in sources {
        match catalogs.add(tag, source) {
            Ok(()) => {}
            Err(error) if *tag == FALLBACK => return Err(error),
            Err(error) => skipped.push((*tag, error)),
        }
    }
    Ok((catalogs, skipped))
}

/// Parsed catalogs by language. A language holds one or more resources: [`Catalogs::add`]
/// replaces them, [`Catalogs::append`] adds one. Where two resources define a message, the one
/// appended last wins (see [`Catalogs::append`]).
#[derive(Default)]
pub struct Catalogs {
    map: BTreeMap<LanguageIdentifier, Vec<Arc<FluentResource>>>,
}

impl std::fmt::Debug for Catalogs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.map.keys()).finish()
    }
}

impl Catalogs {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses and adds the catalog of `tag`, replacing an earlier one.
    ///
    /// # Errors
    /// `app.locale.invalid_tag`, `app.locale.catalog_too_large` or
    /// `app.locale.catalog_invalid` (any syntax error rejects the whole catalog).
    pub fn add(&mut self, tag: &str, source: &str) -> Result<()> {
        let (locale, resource) = Self::parse(tag, source)?;
        self.map.insert(locale, vec![resource]);
        Ok(())
    }

    /// Parses and adds a catalog of `tag` beside the ones the language already has. A message
    /// that the earlier resources define is overridden: the definition of the resource appended
    /// last is the one formatted. Within one catalog a repeated message keeps its first
    /// definition.
    ///
    /// # Errors
    /// The errors of [`Catalogs::add`].
    pub fn append(&mut self, tag: &str, source: &str) -> Result<()> {
        let (locale, resource) = Self::parse(tag, source)?;
        self.map.entry(locale).or_default().push(resource);
        Ok(())
    }

    fn parse(tag: &str, source: &str) -> Result<(LanguageIdentifier, Arc<FluentResource>)> {
        let locale: LanguageIdentifier = tag
            .parse()
            .map_err(|_| Error::new(codes::LOCALE_INVALID_TAG).with_arg("tag", tag))?;
        if source.len() > MAX_CATALOG_BYTES {
            return Err(Error::new(codes::LOCALE_CATALOG_TOO_LARGE).with_arg("tag", tag));
        }
        let resource = FluentResource::try_new(source.to_owned()).map_err(|(_, errors)| {
            Error::new(codes::LOCALE_CATALOG_INVALID)
                .with_arg("tag", tag)
                .with_detail("errors", i64::try_from(errors.len()).unwrap_or(i64::MAX))
        })?;
        Ok((locale, Arc::new(resource)))
    }

    /// The languages that have a catalog.
    pub fn locales(&self) -> impl Iterator<Item = &LanguageIdentifier> {
        self.map.keys()
    }
}

/// The requested tags for a locale setting: the operating-system list for [`SYSTEM`] or an empty
/// value, otherwise the single tag.
#[must_use]
pub fn requested_from_setting(setting: &str, os_locales: &[String]) -> Vec<String> {
    let setting = setting.trim();
    if setting.is_empty() || setting == SYSTEM {
        os_locales.to_vec()
    } else {
        vec![setting.to_owned()]
    }
}

/// The result of [`negotiate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The chosen catalog language.
    pub locale: LanguageIdentifier,
    /// Whether a requested tag was satisfied; false means the English fallback was chosen.
    pub matched: bool,
}

/// Chooses a catalog language for the requested tags, in order. A tag matches an available
/// language exactly, else an available language with the same language subtag, preferring one
/// without a region. Unparsable tags are skipped. With no match the result is [`FALLBACK`].
#[must_use]
pub fn negotiate(requested: &[String], available: &[LanguageIdentifier]) -> Selection {
    for tag in requested.iter().filter_map(|t| parse_tag(t)) {
        if let Some(found) = available.iter().find(|a| **a == tag) {
            return Selection {
                locale: found.clone(),
                matched: true,
            };
        }
        let same: Vec<_> = available
            .iter()
            .filter(|a| a.language == tag.language)
            .collect();
        if let Some(found) = same.iter().find(|a| a.region.is_none()).or(same.first()) {
            return Selection {
                locale: (*found).clone(),
                matched: true,
            };
        }
    }
    Selection {
        locale: fallback_id(),
        matched: false,
    }
}

fn fallback_id() -> LanguageIdentifier {
    // The constant is a valid tag; the default (`und`) is never reached.
    FALLBACK.parse().unwrap_or_default()
}

/// A formatted message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    /// The text; the message identifier itself when no catalog defines it.
    pub text: String,
    /// The language whose catalog supplied the message, or `None` when none did.
    pub locale: Option<LanguageIdentifier>,
}

/// Formats messages in the active language with fallback.
pub struct Localizer {
    catalogs: Catalogs,
    active: LanguageIdentifier,
    chain: Vec<(LanguageIdentifier, FluentBundle<Arc<FluentResource>>)>,
}

impl std::fmt::Debug for Localizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Localizer")
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl Localizer {
    /// Builds a localizer over `catalogs` for the requested tags.
    ///
    /// # Errors
    /// `app.locale.fallback_missing` when there is no [`FALLBACK`] catalog.
    pub fn new(catalogs: Catalogs, requested: &[String]) -> Result<Self> {
        if !catalogs.map.contains_key(&fallback_id()) {
            return Err(Error::new(codes::LOCALE_FALLBACK_MISSING).with_arg("tag", FALLBACK));
        }
        let mut localizer = Self {
            catalogs,
            active: fallback_id(),
            chain: Vec::new(),
        };
        localizer.set_requested(requested);
        Ok(localizer)
    }

    /// Switches the active language without restarting: negotiates again and rebuilds the
    /// fallback chain, which is the selected language, its region-less sibling and English.
    /// Returns the selection.
    pub fn set_requested(&mut self, requested: &[String]) -> Selection {
        let available: Vec<_> = self.catalogs.locales().cloned().collect();
        let selection = negotiate(requested, &available);
        let mut order = vec![selection.locale.clone()];
        let mut sibling = selection.locale.clone();
        sibling.region = None;
        order.extend([sibling, fallback_id()]);
        let mut chain: Vec<(LanguageIdentifier, FluentBundle<Arc<FluentResource>>)> = Vec::new();
        for id in order {
            let Some(resources) = self.catalogs.map.get(&id) else {
                continue;
            };
            if chain.iter().any(|(seen, _)| *seen == id) {
                continue;
            }
            let mut bundle = FluentBundle::new_concurrent(vec![id.clone()]);
            // Fluent keeps the first definition of a message, so the last appended resource is
            // added first and its definitions win over the earlier ones.
            for resource in resources.iter().rev() {
                let _ = bundle.add_resource(Arc::clone(resource));
            }
            chain.push((id, bundle));
        }
        self.active = selection.locale.clone();
        self.chain = chain;
        selection
    }

    /// The active catalog language.
    #[must_use]
    pub fn active(&self) -> &LanguageIdentifier {
        &self.active
    }

    /// Formats message `id` with named arguments, trying each language of the chain in turn.
    #[must_use]
    pub fn format(&self, id: &str, arguments: &[(&str, Value)]) -> Formatted {
        let mut args = FluentArgs::new();
        for (name, value) in arguments {
            args.set(*name, to_fluent(value));
        }
        for (locale, bundle) in &self.chain {
            let Some(pattern) = bundle.get_message(id).and_then(|m| m.value()) else {
                continue;
            };
            let mut errors = Vec::new();
            let text = bundle
                .format_pattern(pattern, Some(&args), &mut errors)
                .into_owned();
            return Formatted {
                text,
                locale: Some(locale.clone()),
            };
        }
        Formatted {
            text: id.to_owned(),
            locale: None,
        }
    }

    /// Formats an error from its message identifier and arguments.
    #[must_use]
    pub fn format_error(&self, error: &Error) -> Formatted {
        let args: Vec<(&str, Value)> = error
            .arguments()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        self.format(error.message_id(), &args)
    }
}

fn to_fluent(value: &Value) -> FluentValue<'static> {
    match value {
        Value::Bool(b) => FluentValue::from(b.to_string()),
        Value::Int(i) => FluentValue::from(*i),
        Value::Float(f) => FluentValue::from(*f),
        Value::Text(t) => FluentValue::from(t.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = "hello = Hello\nonly-en = English only\ncount = { $n } files\n";
    const TR: &str = "hello = Merhaba\ncount = { $n } dosya\n";
    const TR_TR: &str = "hello = Merhaba (TR)\n";

    fn localizer(requested: &[&str]) -> Localizer {
        let mut c = Catalogs::new();
        c.add("en-US", EN).unwrap();
        c.add("tr", TR).unwrap();
        c.add("tr-TR", TR_TR).unwrap();
        Localizer::new(
            c,
            &requested
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn plain(f: &Formatted) -> String {
        f.text.replace(['\u{2068}', '\u{2069}'], "")
    }

    #[test]
    fn negotiation_prefers_exact_then_language_then_english() {
        let avail: Vec<LanguageIdentifier> = ["en-US", "tr", "de-AT", "de-CH"]
            .iter()
            .map(|t| t.parse().unwrap())
            .collect();
        let pick = |r: &[&str]| {
            negotiate(
                &r.iter().map(ToString::to_string).collect::<Vec<_>>(),
                &avail,
            )
        };
        assert_eq!(pick(&["tr"]).locale.to_string(), "tr");
        assert_eq!(pick(&["tr_TR.UTF-8"]).locale.to_string(), "tr");
        assert_eq!(pick(&["de-CH"]).locale.to_string(), "de-CH");
        assert_eq!(pick(&["de"]).locale.to_string(), "de-AT");
        assert_eq!(pick(&["xx", "tr"]).locale.to_string(), "tr");
        let none = pick(&["ja", "!!"]);
        assert_eq!(
            (none.locale.to_string(), none.matched),
            ("en-US".to_owned(), false)
        );
        assert!(!pick(&[]).matched);
    }

    #[test]
    fn setting_selects_os_list_or_explicit_tag() {
        let os = vec!["tr_TR".to_owned()];
        assert_eq!(requested_from_setting("system", &os), os);
        assert_eq!(requested_from_setting("", &os), os);
        assert_eq!(requested_from_setting("de", &os), vec!["de".to_owned()]);
    }

    #[test]
    fn missing_locale_falls_back_to_english() {
        let l = localizer(&["ja"]);
        assert_eq!(l.active().to_string(), "en-US");
        assert_eq!(plain(&l.format("hello", &[])), "Hello");
    }

    #[test]
    fn missing_message_falls_back_per_message() {
        let l = localizer(&["tr-TR"]);
        assert_eq!(plain(&l.format("hello", &[])), "Merhaba (TR)");
        // Not in tr-TR: the region-less sibling answers.
        let f = l.format("count", &[("n", Value::Int(3))]);
        assert_eq!(
            (plain(&f), f.locale.unwrap().to_string()),
            ("3 dosya".to_owned(), "tr".to_owned())
        );
        // Only English has it.
        let f = l.format("only-en", &[]);
        assert_eq!(
            (f.text, f.locale.unwrap().to_string()),
            ("English only".to_owned(), "en-US".to_owned())
        );
        let none = l.format("nowhere", &[]);
        assert_eq!((none.text.as_str(), none.locale), ("nowhere", None));
    }

    #[test]
    fn switching_at_run_time_needs_no_restart() {
        let mut l = localizer(&["en-US"]);
        assert_eq!(plain(&l.format("hello", &[])), "Hello");
        assert!(l.set_requested(&["tr".to_owned()]).matched);
        assert_eq!(plain(&l.format("hello", &[])), "Merhaba");
        l.set_requested(&["en".to_owned()]);
        assert_eq!(plain(&l.format("hello", &[])), "Hello");
    }

    #[test]
    fn catalogs_are_validated_and_english_is_required() {
        let mut c = Catalogs::new();
        assert_eq!(
            c.add("not a tag!", EN).unwrap_err().code(),
            "app.locale.invalid_tag"
        );
        assert_eq!(
            c.add("en-US", "broken = { $\n").unwrap_err().code(),
            "app.locale.catalog_invalid"
        );
        let big = "#".repeat(MAX_CATALOG_BYTES + 1);
        assert_eq!(
            c.add("en-US", &big).unwrap_err().code(),
            "app.locale.catalog_too_large"
        );
        c.add("tr", TR).unwrap();
        assert_eq!(
            Localizer::new(c, &[]).unwrap_err().code(),
            "app.locale.fallback_missing"
        );
    }

    #[test]
    fn errors_are_formatted_from_message_id_and_arguments() {
        let mut c = Catalogs::new();
        c.add(
            "en-US",
            "app-settings-unknown-key = Unknown setting { $key }.\n",
        )
        .unwrap();
        let l = Localizer::new(c, &[]).unwrap();
        let e = Error::new(codes::SETTINGS_UNKNOWN_KEY).with_arg("key", "a.b");
        assert_eq!(plain(&l.format_error(&e)), "Unknown setting a.b.");
    }

    #[test]
    fn a_product_catalog_set_formats_shell_messages_in_en_us_and_tr_tr() {
        // The product appends its own catalog to the shell's, per locale.
        let product = |greeting: &str| format!("product-greeting = {greeting}\n");
        let build = || -> Catalogs {
            let mut catalogs = shell_catalogs().unwrap();
            catalogs
                .append("en-US", &product("Hello from the product"))
                .unwrap();
            catalogs
                .append("tr-TR", &product("Üründen merhaba"))
                .unwrap();
            catalogs
        };
        let english = Localizer::new(build(), &["en-US".to_owned()]).unwrap();
        assert_eq!(plain(&english.format("shell-title", &[])), "Arc Shell");
        assert_eq!(
            plain(&english.format("product-greeting", &[])),
            "Hello from the product"
        );
        let turkish = Localizer::new(build(), &["tr-TR".to_owned()]).unwrap();
        assert_eq!(plain(&turkish.format("shell-title", &[])), "Arc Kabuğu");
        assert_eq!(
            plain(&turkish.format("shell-panel-settings", &[])),
            "Ayarlar"
        );
        assert_eq!(
            plain(&turkish.format("product-greeting", &[])),
            "Üründen merhaba"
        );
    }

    #[test]
    fn a_later_appended_catalog_overrides_an_earlier_message() {
        let mut catalogs = shell_catalogs().unwrap();
        catalogs
            .append("en-US", "shell-title = Product title\n")
            .unwrap();
        let localizer = Localizer::new(catalogs, &["en-US".to_owned()]).unwrap();
        assert_eq!(
            plain(&localizer.format("shell-title", &[])),
            "Product title"
        );
        assert_eq!(
            plain(&localizer.format("shell-panel-settings", &[])),
            "Settings",
            "messages the product does not define keep the shell's text"
        );
    }

    #[test]
    fn a_broken_shipped_language_is_left_out_and_english_must_parse() {
        let sources: &[(&str, &str)] = &[
            ("en-US", "shell-title = Arc Shell\n"),
            ("tr-TR", "shell-title = Kabuk\nbroken = {\n"),
        ];
        let (catalogs, skipped) = catalogs_from(sources).unwrap();
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].0, "tr-TR");
        assert_eq!(skipped[0].1.code(), "app.locale.catalog_invalid");
        let localizer = Localizer::new(catalogs, &["tr-TR".to_owned()]).unwrap();
        assert_eq!(plain(&localizer.format("shell-title", &[])), "Arc Shell");
        let broken_english: &[(&str, &str)] = &[("en-US", "broken = {\n")];
        assert!(catalogs_from(broken_english).is_err());
    }

    #[test]
    fn shipped_catalog_parses_and_formats() {
        let mut c = Catalogs::new();
        c.add("en-US", include_str!("../../../locales/en-US/arc-app.ftl"))
            .unwrap();
        let l = Localizer::new(c, &[]).unwrap();
        for entry in codes::ALL {
            assert!(
                l.format(entry.message_id, &[]).locale.is_some(),
                "{}",
                entry.message_id
            );
        }
    }
}
