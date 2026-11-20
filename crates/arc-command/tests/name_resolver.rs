#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

//! The product's name resolver (CORE-CMD-009): a typed name owned by the product resolves to a
//! command with flags on every surface that resolves names through the registry.

mod common;

use std::collections::BTreeMap;
use std::sync::Mutex;

use arc_base::Limits;
use arc_command::macros::{Recorder, parse_macro, play};
use arc_command::protocol::{NoLog, Server, ServerConfig};
use arc_command::{
    CommandLine, CommandSpec, Invocation, Invoker, LineStep, NameFlags, NameMatch, NameResolver,
    Outcome, Permission, Registry, Session, Submitted, Surface, Target, UndoBehavior, Via,
    format_line, parse_line,
};
use arc_doc::Document;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// What the probe saw in its context.
#[derive(Debug, Serialize, JsonSchema)]
struct Seen {
    no_dialogs: bool,
    builtin_only: bool,
}

fn probe(id: &str, surfaces: &[Surface]) -> CommandSpec {
    CommandSpec::new(
        id,
        "test-probe",
        "test-probe-description",
        |context, _: NoParams| -> arc_base::Result<Seen> {
            let flags = context.name_flags();
            Ok(Seen {
                no_dialogs: flags.no_dialogs,
                builtin_only: flags.builtin_only,
            })
        },
    )
    .with_undo(UndoBehavior::NotUndoable)
    .with_permissions([Permission::Read])
    .with_exposure(surfaces.iter().copied())
}

const ALL: [Surface; 5] = [
    Surface::Ui,
    Surface::CommandLine,
    Surface::Automation,
    Surface::Mcp,
    Surface::Plugin,
];

const BOTH: NameFlags = NameFlags {
    no_dialogs: true,
    builtin_only: true,
};

/// The product's names: `!probe` and `!create` (prompt-driven), `!hidden` (exposed only to
/// automation) and `!absent` (names a command that is not registered).
fn resolver() -> NameResolver {
    NameResolver::new(|name| {
        let (command, flags) = match name {
            "!probe" => ("test.probe.flags", BOTH),
            "!hidden" => ("test.probe.hidden", BOTH),
            "!create" => (
                "test.item.create",
                NameFlags {
                    no_dialogs: true,
                    builtin_only: false,
                },
            ),
            "!absent" => ("test.absent", NameFlags::default()),
            _ => return None,
        };
        Some(NameMatch {
            command: command.to_owned(),
            flags,
        })
    })
}

/// The common registry, the two probes, a user alias `probe` that the product name bypasses,
/// and the product's resolver.
fn base() -> Registry {
    let mut r = common::registry();
    r.register(probe("test.probe.flags", &ALL)).unwrap();
    r.register(probe("test.probe.hidden", &[Surface::Automation]))
        .unwrap();
    r.add_user_alias("probe", "test.probe.flags").unwrap();
    r
}

fn registry() -> Registry {
    let mut r = base();
    r.set_name_resolver(resolver()).unwrap();
    r
}

fn document() -> Document {
    common::document()
}

fn flags(value: &Value) -> (bool, bool) {
    (
        value["no_dialogs"].as_bool().unwrap(),
        value["builtin_only"].as_bool().unwrap(),
    )
}

fn code(result: arc_base::Result<Outcome>) -> String {
    result.map(drop).unwrap_err().code().to_owned()
}

#[test]
fn resolution_gives_the_command_its_flags_and_the_via() {
    let r = registry();
    let product = r.resolve("  !probe ", &BTreeMap::new()).unwrap();
    assert_eq!(product.spec.id(), "test.probe.flags");
    assert_eq!(product.via, Via::Product);
    assert_eq!(product.flags, BOTH);
    assert_eq!(product.call_name(" !probe "), "!probe");

    let identifier = r.resolve("TEST.Probe.Flags", &BTreeMap::new()).unwrap();
    assert_eq!(identifier.via, Via::Identifier);
    assert_eq!(identifier.flags, NameFlags::default());
    assert_eq!(identifier.call_name("TEST.Probe.Flags"), "test.probe.flags");
}

#[test]
fn a_product_name_bypasses_the_user_alias_of_the_same_text() {
    let r = registry();
    let alias = r.resolve("probe", &BTreeMap::new()).unwrap();
    assert_eq!(alias.via, Via::UserAlias);
    assert_eq!(alias.flags, NameFlags::default());
    let product = r.resolve("!probe", &BTreeMap::new()).unwrap();
    assert_eq!(product.via, Via::Product);
    assert_eq!(product.flags, BOTH);
}

#[test]
fn unknown_names_keep_the_existing_error() {
    let r = registry();
    let localized = BTreeMap::new();
    assert_eq!(
        r.resolve("!absent", &localized).unwrap_err().code(),
        "cmd.unknown"
    );
    assert_eq!(
        r.resolve("!nothing", &localized).unwrap_err().code(),
        "cmd.unknown"
    );
    assert_eq!(
        r.resolve("nothing-at-all", &localized).unwrap_err().code(),
        "cmd.unknown"
    );
    let doc = document();
    let session = Session::new(Surface::Automation, [Permission::Read]);
    let invoker = Invoker::new(&r);
    let invocation = Invocation::new("!absent", json!({}));
    assert_eq!(
        code(invoker.invoke(&doc, &session, invocation)),
        "cmd.unknown"
    );
}

#[test]
fn every_surface_runs_a_product_name_with_its_flags() {
    let r = registry();
    let doc = document();
    for surface in ALL {
        let session = Session::new(surface, [Permission::Read]);
        let invoker = Invoker::new(&r);
        let outcome = invoker
            .invoke(&doc, &session, Invocation::new("!probe", json!({})))
            .unwrap_or_else(|e| panic!("{surface:?}: {e:?}"));
        assert_eq!(flags(&outcome.value), (true, true), "{surface:?}");
        let plain = invoker
            .invoke(
                &doc,
                &session,
                Invocation::new("test.probe.flags", json!({})),
            )
            .unwrap();
        assert_eq!(flags(&plain.value), (false, false), "{surface:?}");
    }
}

#[test]
fn a_product_name_cannot_reach_a_command_hidden_from_the_surface() {
    let r = registry();
    let doc = document();
    for surface in ALL {
        let session = Session::new(surface, [Permission::Read]);
        let result = Invoker::new(&r).invoke(&doc, &session, Invocation::new("!hidden", json!({})));
        if surface == Surface::Automation {
            assert_eq!(flags(&result.unwrap().value), (true, true));
        } else {
            assert_eq!(code(result), "cmd.not_exposed", "{surface:?}");
        }
    }
}

#[test]
fn the_command_line_runs_a_product_name_with_its_flags() {
    let r = registry();
    let doc = document();
    let localized = BTreeMap::new();
    let session = Session::new(Surface::CommandLine, [Permission::Read]);
    let target = Target {
        registry: &r,
        document: &doc,
        session: &session,
        selection: &[],
        localized: &localized,
        seed: 0,
    };
    let mut line = CommandLine::new();
    let Submitted::Ran(outcome) = line.submit(&target, "!probe").unwrap() else {
        panic!("the command did not run");
    };
    assert_eq!(flags(&outcome.value), (true, true));
    let Submitted::Ran(outcome) = line
        .start(&target, "!probe", serde_json::Map::new())
        .unwrap()
    else {
        panic!("the started command did not run");
    };
    assert_eq!(flags(&outcome.value), (true, true));
    // The repeat runs the same name, so the flags are kept.
    let Submitted::Ran(outcome) = line.submit(&target, "").unwrap() else {
        panic!("the repeat did not run");
    };
    assert_eq!(flags(&outcome.value), (true, true));
}

#[test]
fn a_prompt_keeps_the_product_name_for_the_run() {
    let r = registry();
    let doc = document();
    let localized = BTreeMap::new();
    let session = Session::new(Surface::CommandLine, [Permission::Read, Permission::Edit]);
    let target = Target {
        registry: &r,
        document: &doc,
        session: &session,
        selection: &[],
        localized: &localized,
        seed: 0,
    };
    let mut line = CommandLine::new();
    // The length is missing, so the line asks for it.
    assert!(matches!(
        line.submit(&target, "!create c").unwrap(),
        Submitted::Prompt(_)
    ));
    let seen = Mutex::new(Vec::new());
    let invoker = Invoker::new(&r);
    let submitted = line
        .drive(&target, LineStep::Submit("1200 mm"), &mut |invocation| {
            seen.lock().unwrap().push(invocation.command.clone());
            invoker.invoke(&doc, &session, invocation)
        })
        .unwrap();
    assert!(matches!(submitted, Submitted::Ran(_)));
    assert_eq!(*seen.lock().unwrap(), vec!["!create".to_owned()]);
}

#[test]
fn the_protocol_runs_a_product_name_through_command_run() {
    let r = registry();
    let documents = BTreeMap::from([("main".to_owned(), document())]);
    let config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read]);
    let server = Server::new(&r, &documents, config, &NoLog);
    let answer = server
        .call(
            "command.run",
            json!({"document": "main", "command": "!probe", "params": {}}),
        )
        .unwrap();
    assert_eq!(flags(&answer["value"]), (true, true));
    let hidden = server
        .call(
            "command.run",
            json!({"document": "main", "command": "!absent", "params": {}}),
        )
        .unwrap_err();
    assert_eq!(hidden["data"]["code"], "cmd.unknown");
}

#[test]
fn macros_record_and_replay_the_product_name_with_its_flags() {
    let r = registry();
    let doc = document();
    let session = Session::new(Surface::Automation, [Permission::Read]);
    let invocation = Invocation::new("!probe", json!({}));
    let recorder = Recorder::new("arc-test", "0.1.0", &Limits::DEFAULT);
    let outcome = Invoker::new(&r)
        .invoke(&doc, &session, invocation.clone())
        .unwrap();
    recorder.record(&r, &invocation, outcome.revision).unwrap();
    let recorded = parse_macro(&recorder.text(), &Limits::DEFAULT).unwrap();
    assert_eq!(recorded.steps[0].line, "test.probe.flags");
    let outcomes = play(&r, &doc, &session, &recorded.steps, "macro").unwrap();
    // The identifier replays; the name flags are not recorded, so the replay runs with defaults.
    assert_eq!(outcomes[0].command, "test.probe.flags");
    assert_eq!(flags(&outcomes[0].value), (false, false));
}

#[test]
fn lines_carry_the_typed_name_only_for_a_product_name() {
    let r = registry();
    let product = parse_line(&r, &BTreeMap::new(), "!probe").unwrap().unwrap();
    assert_eq!(product.command, "test.probe.flags");
    assert_eq!(product.invoke_as, "!probe");
    let alias = parse_line(&r, &BTreeMap::new(), "probe").unwrap().unwrap();
    assert_eq!(alias.invoke_as, "test.probe.flags");
    // A recorded line always names the identifier: replay must not depend on the resolver.
    let line = format_line(&r, &Invocation::new("!probe", json!({}))).unwrap();
    assert_eq!(line, "test.probe.flags");
    let line = format_line(&r, &Invocation::new("probe", json!({}))).unwrap();
    assert_eq!(line, "test.probe.flags");
}

/// Recording with the product's resolver and replaying with another resolver, and with none, runs
/// the same command: the recorded line names the identifier.
#[test]
fn replay_runs_the_recorded_identifier_whatever_resolver_exists() {
    let doc = document();
    let session = Session::new(Surface::Automation, [Permission::Read]);
    let invocation = Invocation::new("!probe", json!({}));
    let recorder = Recorder::new("arc-test", "0.1.0", &Limits::DEFAULT);
    let writer = registry();
    let outcome = Invoker::new(&writer)
        .invoke(&doc, &session, invocation.clone())
        .unwrap();
    recorder
        .record(&writer, &invocation, outcome.revision)
        .unwrap();
    let recorded = parse_macro(&recorder.text(), &Limits::DEFAULT).unwrap();
    assert_eq!(recorded.steps[0].line, "test.probe.flags");
    // A resolver that maps the same text elsewhere.
    let mut other = base();
    other
        .set_name_resolver(NameResolver::new(|name| {
            (name == "!probe").then(|| NameMatch {
                command: "test.probe.hidden".to_owned(),
                flags: NameFlags::default(),
            })
        }))
        .unwrap();
    let replays = [&writer, &other, &base()];
    for replay in replays {
        let outcomes = play(replay, &doc, &session, &recorded.steps, "macro").unwrap();
        assert_eq!(outcomes[0].command, "test.probe.flags");
    }
}

/// The resolver cannot shadow a registered identifier, and cannot own the text of an alias.
#[test]
fn identifiers_come_first_and_an_alias_the_resolver_owns_is_refused() {
    let mut r = base();
    let owns_identifier = NameResolver::new(|name| {
        (name == "test.probe.flags").then(|| NameMatch {
            command: "test.probe.hidden".to_owned(),
            flags: BOTH,
        })
    });
    r.set_name_resolver(owns_identifier).unwrap();
    let identifier = r.resolve("test.probe.flags", &BTreeMap::new()).unwrap();
    assert_eq!(identifier.via, Via::Identifier);
    assert_eq!(identifier.flags, NameFlags::default());

    let mut fresh = base();
    fresh
        .set_name_resolver(NameResolver::new(|name| {
            (name == "shadow").then(|| NameMatch {
                command: "test.probe.flags".to_owned(),
                flags: BOTH,
            })
        }))
        .unwrap();
    let refused = fresh
        .add_user_alias("shadow", "test.probe.flags")
        .unwrap_err();
    assert_eq!(refused.code(), "cmd.duplicate");
    // A resolver set after an alias it owns is refused, and the registry keeps its resolver.
    let mut late = base();
    let error = late
        .set_name_resolver(NameResolver::new(|name| {
            (name == "probe").then(|| NameMatch {
                command: "test.probe.flags".to_owned(),
                flags: BOTH,
            })
        }))
        .unwrap_err();
    assert_eq!(error.code(), "cmd.duplicate");
    let alias = late.resolve("probe", &BTreeMap::new()).unwrap();
    assert_eq!(alias.via, Via::UserAlias);
}

/// Completion offers no parameters of a command the command line does not expose, and still offers
/// them for an exposed one.
#[test]
fn completion_skips_a_command_not_exposed_on_the_command_line() {
    let r = registry();
    let localized = BTreeMap::new();
    let hidden = "!hidden ";
    let done = arc_command::complete(&r, &localized, hidden, hidden.len());
    assert!(done.candidates.is_empty());
    let exposed = "test.item.create ";
    let done = arc_command::complete(&r, &localized, exposed, exposed.len());
    assert!(!done.candidates.is_empty());
}
