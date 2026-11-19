//! The reference headless CLI (CORE-APP-026). The sample document type set lives in
//! `arc-shell-sample`, shared with the windowed reference shell; it is re-exported here so the
//! binary and its tests use it through this package as before.
//!
//! [`run`] is the program: the product subcommand `render` (module [`render`]) and the shared
//! subcommands of [`arc_command::headless::run_with_renderer`], which serves `view.render` with
//! the same renderer.

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use arc_app::render::SnapshotRenderer;
use arc_command::error::codes;
use arc_command::headless::{self, Exit, Product};

pub use arc_shell_sample::*;

pub mod render;

/// Runs the command-line program: `render` here, every other subcommand through the shared
/// runner. Never panics: a fault is contained and reported as [`Exit::Internal`].
pub fn run<R, W, E>(
    args: impl IntoIterator<Item = OsString>,
    product: &Product,
    stdin: R,
    mut stdout: W,
    mut stderr: E,
) -> Exit
where
    R: BufRead,
    W: Write + Send,
    E: Write + Send,
{
    let mut args: Vec<OsString> = args.into_iter().collect();
    if args.first().and_then(|a| a.to_str()) != Some("render") {
        let renderer = SnapshotRenderer::new(render::extract_sample);
        return headless::run_with_renderer(args, product, Some(&renderer), stdin, stdout, stderr);
    }
    args.remove(0);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        render::render(args, product, &mut stdout)
    }));
    let error = match outcome {
        Ok(Ok(exit)) => return exit,
        Ok(Err(error)) => error,
        Err(_) => arc_base::Error::new(codes::FAULT),
    };
    headless::report_error(error, &mut stderr)
}
