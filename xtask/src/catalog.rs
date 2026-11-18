//! The localization catalog gate: the message identifiers a crate's catalog defines and the
//! `message_id` strings its sources reference must be the same set.

use std::collections::{BTreeMap, BTreeSet};

/// The message identifiers defined by a Fluent catalog, in order, duplicates included. Only lines
/// of the form `identifier = text` at the start of a line count; comments (`#`), terms (`-name`),
/// attributes and continuation lines are skipped.
fn defined(catalog: &str) -> Vec<&str> {
    catalog
        .lines()
        .filter(|l| !l.starts_with(['#', ' ', '\t', '-', '.']))
        .filter_map(|l| l.split_once('=').map(|(id, _)| id.trim_end()))
        .filter(|id| {
            id.starts_with(|c: char| c.is_ascii_alphabetic())
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .collect()
}

/// The identifiers referenced as `message_id: "..."` in Rust source text. By convention the test
/// module closes a file, so everything from `#[cfg(test)]` on is ignored: tests may use
/// deliberately invalid identifiers. Comment lines are ignored.
pub fn referenced(source: &str) -> Vec<String> {
    let code: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let source = code.split("#[cfg(test)]").next().unwrap_or_default();
    source
        .match_indices("message_id:")
        .filter_map(|(at, key)| {
            let rest = source[at + key.len()..].trim_start().strip_prefix('"')?;
            rest.split_once('"').map(|(id, _)| id.to_owned())
        })
        .collect()
}

/// Compares one crate's catalog (`None` when it has none) with its referenced identifiers and
/// returns one message per problem.
pub fn check(krate: &str, catalog: Option<&str>, referenced: &[String]) -> Vec<String> {
    let Some(catalog) = catalog else {
        return if referenced.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "{krate}: references message identifiers but has no catalog locales/en-US/{krate}.ftl"
            )]
        };
    };
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for id in defined(catalog) {
        if !seen.insert(id) {
            out.push(format!(
                "{krate}: duplicate message identifier `{id}` in the catalog"
            ));
        }
    }
    let used: BTreeSet<&str> = referenced.iter().map(String::as_str).collect();
    for id in used.difference(&seen) {
        out.push(format!(
            "{krate}: message identifier `{id}` is referenced but not defined in the catalog"
        ));
    }
    for id in seen.difference(&used) {
        out.push(format!(
            "{krate}: message identifier `{id}` is defined but never referenced"
        ));
    }
    out
}

/// What a translation must share with the English source of a message: its attributes and the
/// variables used anywhere in it, attributes included.
#[derive(Debug, Default, PartialEq, Eq)]
struct Shape {
    attributes: BTreeSet<String>,
    variables: BTreeSet<String>,
}

/// The variable names (`$name`) occurring in `text`.
fn variables(text: &str, out: &mut BTreeSet<String>) {
    for (at, _) in text.match_indices('$') {
        let name: String = text[at + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
}

/// The shape of every message of a catalog, by identifier (the first definition wins).
fn shapes(catalog: &str) -> BTreeMap<String, Shape> {
    let mut out = BTreeMap::new();
    let mut current: Option<(String, Shape)> = None;
    let mut finish = |c: &mut Option<(String, Shape)>| {
        if let Some((id, shape)) = c.take() {
            out.entry(id).or_insert(shape);
        }
    };
    for line in catalog.lines() {
        if line.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t']) && !line.is_empty() {
            finish(&mut current);
            let id = defined(line).first().map(|id| (*id).to_owned());
            current = id.map(|id| (id, Shape::default()));
        }
        if let Some((_, shape)) = &mut current {
            if let Some((name, _)) = line
                .trim_start()
                .strip_prefix('.')
                .and_then(|attr| attr.split_once('='))
            {
                shape.attributes.insert(name.trim().to_owned());
            }
            variables(line, &mut shape.variables);
        }
    }
    finish(&mut current);
    out
}

/// Checks one translated catalog against its English source: valid Fluent syntax, the same
/// message identifiers, the same attributes and the same variables per message.
pub fn check_translation(name: &str, english: &str, translated: &str) -> Vec<String> {
    let mut out = Vec::new();
    if fluent_bundle::FluentResource::try_new(translated.to_owned()).is_err() {
        out.push(format!("{name}: not valid Fluent syntax"));
    }
    let en = shapes(english);
    let tr = shapes(translated);
    for id in en.keys().filter(|id| !tr.contains_key(*id)) {
        out.push(format!("{name}: message `{id}` is missing"));
    }
    for id in tr.keys().filter(|id| !en.contains_key(*id)) {
        out.push(format!("{name}: message `{id}` does not exist in en-US"));
    }
    for (id, shape) in &en {
        let Some(other) = tr.get(id) else { continue };
        if shape.attributes != other.attributes {
            out.push(format!(
                "{name}: message `{id}` has different attributes than en-US"
            ));
        }
        if shape.variables != other.variables {
            out.push(format!(
                "{name}: message `{id}` uses different variables than en-US"
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str =
        "# comment\nfoo-a = Text { $x }\n  more text\n-term = T\nfoo-b = B\n    .attr = A\n";

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn matching_catalog_and_references_pass() {
        assert!(check("foo", Some(CATALOG), &ids(&["foo-a", "foo-b", "foo-a"])).is_empty());
    }

    #[test]
    fn missing_unused_and_duplicate_identifiers_are_reported() {
        let catalog = format!("{CATALOG}foo-a = again\n");
        let found = check("foo", Some(&catalog), &ids(&["foo-a", "foo-c"]));
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(
            found
                .iter()
                .any(|m| m.contains("duplicate") && m.contains("foo-a"))
        );
        assert!(
            found
                .iter()
                .any(|m| m.contains("referenced but not defined") && m.contains("foo-c"))
        );
        assert!(
            found
                .iter()
                .any(|m| m.contains("never referenced") && m.contains("foo-b"))
        );
    }

    #[test]
    fn references_without_a_catalog_fail_and_no_references_pass() {
        assert_eq!(check("foo", None, &ids(&["foo-a"])).len(), 1);
        assert!(check("foo", None, &[]).is_empty());
    }

    #[test]
    fn references_are_read_from_source_text() {
        let source = "message_id: \"a-b\",\n/// `message_id`, x\nmessage_id: &'static str,\nmessage_id:\"c\"\n#[cfg(test)]\nmessage_id: \"bad\"";
        assert_eq!(referenced(source), ids(&["a-b", "c"]));
    }

    const EN: &str = "a = Hi { $x }\nb = B\n    .title = T { $y }\n";

    #[test]
    fn matching_translation_passes() {
        let tr = "# c\na = Selam { $x }\nb = Be\n    .title = Ti { $y }\n";
        assert!(check_translation("t", EN, tr).is_empty());
    }

    #[test]
    fn translation_gate_reports_each_kind_of_difference() {
        let missing = check_translation("t", EN, "a = Selam { $x }\n");
        assert!(
            missing.iter().any(|m| m.contains("`b` is missing")),
            "{missing:?}"
        );
        let extra = check_translation("t", EN, &format!("{EN}c = C\n"));
        assert!(extra.iter().any(|m| m.contains("`c` does not exist")));
        let vars = check_translation("t", EN, "a = Selam\nb = B\n    .title = T { $y }\n");
        assert!(vars.iter().any(|m| m.contains("variables")));
        let attrs = check_translation("t", EN, "a = Selam { $x }\nb = B\n");
        assert!(attrs.iter().any(|m| m.contains("attributes")));
        let syntax = check_translation("t", EN, "a = { $x\nb = B\n    .title = T { $y }\n");
        assert!(syntax.iter().any(|m| m.contains("syntax")));
    }
}
