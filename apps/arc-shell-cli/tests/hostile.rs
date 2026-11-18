//! Hostile arguments and standard input never panic the runner: every invocation ends with an
//! exit code of the documented table, and standard output holds only JSON lines.

#![allow(clippy::unwrap_used, reason = "tests")]

use std::ffi::OsString;
use std::io::Cursor;
use std::sync::Arc;

use arc_command::FixedClock;
use arc_command::headless::{Exit, Product, run};
use arc_doc::package::LockOwner;
use proptest::prelude::*;

fn product() -> Product {
    let owner = LockOwner {
        user: "test".into(),
        host: "test".into(),
        application: "hostile".into(),
        time_unix_ms: 0,
    };
    arc_shell_cli::product(Arc::new(FixedClock(0)), owner).unwrap()
}

/// Words that reach every subcommand and flag, mixed with arbitrary text.
fn word() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(vec![
            "commands",
            "describe",
            "validate-params",
            "run",
            "inspect",
            "validate",
            "serve",
            "help",
            "--stdio",
            "--save",
            "--check",
            "--json",
            "--allow-edit",
            "--allow-network",
            "--command",
            "--params",
            "--seed",
            "--read-root",
            "--write-root",
            "--",
            "--params=@",
            "shell.point.create",
            "shell.object.delete",
            "{}",
            "{\"name\":\"a\"}",
            "-1",
            "18446744073709551616",
            "/nonexistent/x.arc",
            ".",
            "",
        ])
        .prop_map(str::to_owned),
        ".{0,24}",
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn hostile_arguments_and_input_end_with_a_table_code(
        args in prop::collection::vec(word(), 0..8),
        input in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let product = product();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let exit = run(
            args.iter().map(OsString::from),
            &product,
            Cursor::new(input),
            &mut out,
            &mut err,
        );
        prop_assert!(Exit::ALL.contains(&exit));
        prop_assert!(exit != Exit::Internal, "{:?}: {}", args, String::from_utf8_lossy(&err));
        for line in String::from_utf8(out).unwrap().lines() {
            prop_assert!(serde_json::from_str::<serde_json::Value>(line).is_ok(), "{}", line);
        }
    }
}
