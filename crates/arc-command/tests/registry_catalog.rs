//! The registry (identifiers, aliases, resolution, duplicates, the registration check), the
//! examples of every command, and the reviewed catalog snapshot.

#![allow(clippy::unwrap_used, reason = "tests")]

mod common;

use std::collections::BTreeMap;

use arc_command::{CommandSpec, Registry, Via};
use common::{Nothing, registry};
use serde_json::json;

fn noop(id: &str) -> CommandSpec {
    CommandSpec::new(id, "test-noop", "test-noop-description", |_, _: Nothing| {
        Ok(())
    })
}

#[test]
fn every_registered_command_validates_its_own_examples() {
    let r = registry();
    assert!(r.iter().count() > 10);
    for spec in r.iter() {
        spec.validate_examples().unwrap();
    }
}

#[test]
fn a_command_whose_example_violates_its_schema_is_not_registered() {
    let mut r = Registry::new();
    let error = r
        .register(noop("test.bad.example").with_example(json!({"extra": 1})))
        .unwrap_err();
    assert_eq!(error.code(), "cmd.spec.invalid");
    assert_eq!(
        error.details().get("violations"),
        Some(&arc_base::error::Value::Text(
            r#"[{"pointer":"/extra","kind":"additional_property"}]"#.into()
        ))
    );
    assert_eq!(r.iter().count(), 0);
}

#[test]
fn malformed_specifications_are_rejected() {
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct Open {
        _a: u8,
    }
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct WithSet {
        _s: std::collections::BTreeSet<u8>,
    }
    let mut r = Registry::new();
    for spec in [
        noop("test.two"),
        noop("Test.a.b"),
        noop("test..a.b"),
        CommandSpec::new("test.a.b", "bad label", "x", |_, _: Nothing| Ok(())),
        noop("test.a.b").with_permissions([]),
        CommandSpec::new("test.a.b", "x", "y", |_, _: Open| Ok(())),
        CommandSpec::new("test.a.b", "x", "y", |_, _: WithSet| Ok(())),
        CommandSpec::new("test.a.b", "x", "y", |_, _: ()| Ok(())),
    ] {
        assert_eq!(r.register(spec).unwrap_err().code(), "cmd.spec.invalid");
    }
    assert_eq!(r.iter().count(), 0);
}

#[test]
fn duplicates_fail_and_leave_the_registry_unchanged() {
    let mut r = registry();
    let before = r.catalog_text();
    let aliases: Vec<_> = r
        .user_aliases()
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
        .collect();
    assert_eq!(
        r.register(noop("test.item.create")).unwrap_err().code(),
        "cmd.duplicate"
    );
    // A new command cannot take a deprecated alias.
    assert_eq!(
        r.register(noop("test.item.make")).unwrap_err().code(),
        "cmd.duplicate"
    );
    assert_eq!(
        r.add_deprecated_alias("test.item.summary", "test.item.create")
            .unwrap_err()
            .code(),
        "cmd.duplicate"
    );
    assert_eq!(r.catalog_text(), before);
    assert_eq!(
        r.user_aliases()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect::<Vec<_>>(),
        aliases
    );
}

#[test]
fn names_resolve_by_identifier_alias_and_localized_name() {
    let mut r = registry();
    let none = BTreeMap::new();
    let via = |r: &Registry, name: &str, localized: &BTreeMap<String, String>| {
        let resolved = r.resolve(name, localized).unwrap();
        (resolved.spec.id().to_owned(), resolved.via)
    };
    assert_eq!(
        via(&r, " Test.Item.Create ", &none),
        ("test.item.create".into(), Via::Identifier)
    );
    assert_eq!(
        via(&r, "test.item.make", &none),
        ("test.item.create".into(), Via::DeprecatedAlias)
    );
    let deprecation = r
        .resolve("test.item.make", &none)
        .unwrap()
        .deprecation("test.item.make");
    assert_eq!(deprecation.unwrap().code(), "cmd.alias.deprecated");

    r.add_user_alias("mk", "test.item.create").unwrap();
    assert_eq!(
        via(&r, "MK", &none),
        ("test.item.create".into(), Via::UserAlias)
    );
    for (alias, code) in [
        ("test.item.delete", "cmd.duplicate"),
        ("test.item.make", "cmd.duplicate"),
        ("mk", "cmd.duplicate"),
        ("", "cmd.alias.invalid"),
        ("has space", "cmd.alias.invalid"),
        ("Upper", "cmd.alias.invalid"),
    ] {
        assert_eq!(
            r.add_user_alias(alias, "test.item.delete")
                .unwrap_err()
                .code(),
            code,
            "{alias}"
        );
    }
    assert_eq!(
        r.add_user_alias("zz", "test.missing.cmd")
            .unwrap_err()
            .code(),
        "cmd.unknown"
    );
    assert_eq!(
        r.add_deprecated_alias("old", "test.item.create")
            .unwrap_err()
            .code(),
        "cmd.alias.invalid"
    );
    r.remove_user_alias("mk").unwrap();
    assert_eq!(
        r.remove_user_alias("mk").unwrap_err().code(),
        "cmd.alias.unknown"
    );
    assert_eq!(r.resolve("mk", &none).unwrap_err().code(), "cmd.unknown");

    let localized: BTreeMap<String, String> = [
        (
            "test.item.create".to_owned(),
            "Element erstellen".to_owned(),
        ),
        ("test.item.delete".to_owned(), "Löschen".to_owned()),
        ("test.item.summary".to_owned(), "Löschen".to_owned()),
    ]
    .into();
    assert_eq!(
        via(&r, "element ERSTELLEN", &localized),
        ("test.item.create".into(), Via::LocalizedName)
    );
    assert_eq!(
        r.resolve("löschen", &localized).unwrap_err().code(),
        "cmd.name.ambiguous"
    );
}

#[test]
fn the_catalog_is_deterministic_and_matches_the_reviewed_snapshot() {
    let text = registry().catalog_text();
    assert_eq!(text, registry().catalog_text());
    let catalog: serde_json::Value = serde_json::from_str(&text).unwrap();
    let ids: Vec<&str> = catalog["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&"test.hidden.noop"));
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted);
    insta::assert_snapshot!("catalog", text);
}
