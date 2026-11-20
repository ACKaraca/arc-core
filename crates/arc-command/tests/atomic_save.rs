#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

//! Exclusive creation and replacement on the command context (CORE-CMD-032): a product's atomic
//! save is built from `create_new`, a write and `replace_file`, all inside the write roots.

use std::io::Write;
use std::path::{Path, PathBuf};

use arc_command::error::codes;
use arc_command::{
    CommandSpec, Invocation, Invoker, Permission, Registry, Roots, Session, Surface, UndoBehavior,
};
use arc_doc::{Document, TypeRegistry, graph::Evaluators};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Save {
    target: String,
    temporary: String,
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Create {
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Replace {
    temporary: String,
    target: String,
}

fn io() -> arc_base::Error {
    arc_base::Error::new(codes::PATH_IO)
}

/// The product's save: a new temporary file, written, then renamed over the target.
fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(
        CommandSpec::new(
            "test.save.atomic",
            "test-save",
            "test-save-description",
            |context, p: Save| -> arc_base::Result<()> {
                let mut file = context.create_new(Path::new(&p.temporary))?;
                file.write_all(p.text.as_bytes()).map_err(|_| io())?;
                file.sync_all().map_err(|_| io())?;
                drop(file);
                context.replace_file(Path::new(&p.temporary), Path::new(&p.target))
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::FileWrite]),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.create.new",
            "test-create-new",
            "test-create-new-description",
            |context, p: Create| -> arc_base::Result<()> {
                drop(context.create_new(Path::new(&p.path))?);
                Ok(())
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::FileWrite]),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.replace.file",
            "test-replace-file",
            "test-replace-file-description",
            |context, p: Replace| -> arc_base::Result<()> {
                context.replace_file(Path::new(&p.temporary), Path::new(&p.target))
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::FileWrite]),
    )
    .unwrap();
    r
}

fn document() -> Document {
    Document::new(Arc::new(TypeRegistry::new()), Arc::new(Evaluators::new())).unwrap()
}

/// A scratch directory with a short, test-specific name, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("arc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A session with `dir` as its read and write root.
    fn session(&self, granted: &[Permission]) -> Session {
        let mut session = Session::new(Surface::Automation, granted.iter().copied());
        session.roots = Roots::new([self.0.as_path()], [self.0.as_path()]).unwrap();
        session
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn code(result: arc_base::Result<arc_command::Outcome>) -> String {
    result.map(drop).unwrap_err().code().to_owned()
}

fn reason(error: &arc_base::Error) -> String {
    error
        .details()
        .get("reason")
        .map(|v| format!("{v:?}"))
        .unwrap_or_default()
}

#[test]
fn an_atomic_save_replaces_the_target_and_leaves_no_temporary_file() {
    let scratch = Scratch::new("save-ok");
    let (target, temporary) = (scratch.join("doc.txt"), scratch.join("doc.tmp"));
    std::fs::write(&target, "old").unwrap();
    let session = scratch.session(&[Permission::FileWrite]);
    let registry = registry();
    let outcome = Invoker::new(&registry).invoke(
        &document(),
        &session,
        Invocation::new(
            "test.save.atomic",
            json!({
                "target": target,
                "temporary": temporary,
                "text": "new",
            }),
        ),
    );
    outcome.unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    assert!(!temporary.exists());
}

#[test]
fn an_exclusive_create_refuses_an_existing_file_and_a_link_at_the_name() {
    let scratch = Scratch::new("save-excl");
    let existing = scratch.join("taken.txt");
    std::fs::write(&existing, "keep").unwrap();
    let session = scratch.session(&[Permission::FileWrite]);
    let registry = registry();
    let invoker = Invoker::new(&registry);
    let doc = document();
    let create = |path: &Path| {
        invoker.invoke(
            &doc,
            &session,
            Invocation::new("test.create.new", json!({ "path": path })),
        )
    };
    let refused = create(&existing).unwrap_err();
    assert_eq!(refused.code(), "cmd.path.io");
    assert_eq!(reason(&refused), "Text(\"exists\")");
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "keep");

    #[cfg(unix)]
    {
        let link = scratch.join("link.txt");
        std::os::unix::fs::symlink(scratch.join("elsewhere.txt"), &link).unwrap();
        assert_eq!(code(create(&link)), "cmd.path.io");
        assert!(!scratch.join("elsewhere.txt").exists());
    }

    let fresh = scratch.join("fresh.txt");
    create(&fresh).unwrap();
    assert!(fresh.is_file());
}

#[test]
fn replace_file_cannot_move_a_file_outside_the_write_roots() {
    let scratch = Scratch::new("save-move");
    let inside = scratch.join("inside");
    let outside = scratch.join("outside");
    std::fs::create_dir_all(&inside).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("moved.tmp"), "data").unwrap();
    std::fs::write(inside.join("moved.tmp"), "data").unwrap();
    let mut session = Session::new(Surface::Automation, [Permission::FileWrite]);
    session.roots = Roots::new([inside.as_path()], [inside.as_path()]).unwrap();
    let registry = registry();
    let invoker = Invoker::new(&registry);
    let doc = document();
    let replace = |temporary: PathBuf, target: PathBuf| {
        invoker.invoke(
            &doc,
            &session,
            Invocation::new(
                "test.replace.file",
                json!({ "temporary": temporary, "target": target }),
            ),
        )
    };
    // The target lies outside the roots: the file does not move.
    assert_eq!(
        code(replace(inside.join("moved.tmp"), outside.join("doc.txt"))),
        "cmd.path.rejected"
    );
    // The temporary file lies outside the roots.
    assert_eq!(
        code(replace(outside.join("moved.tmp"), inside.join("doc.txt"))),
        "cmd.path.rejected"
    );
    // Both lie in one directory outside the roots.
    assert_eq!(
        code(replace(outside.join("moved.tmp"), outside.join("doc.txt"))),
        "cmd.path.outside_roots"
    );
    assert!(outside.join("moved.tmp").is_file());
    assert!(!outside.join("doc.txt").exists());
    assert!(!inside.join("doc.txt").exists());
    // Inside the roots the same call succeeds.
    replace(inside.join("moved.tmp"), inside.join("doc.txt")).unwrap();
    assert!(inside.join("doc.txt").is_file());
}

#[test]
fn the_save_and_the_replace_need_the_file_write_class() {
    let scratch = Scratch::new("save-perm");
    let session = scratch.session(&[Permission::FileRead]);
    let registry = registry();
    let invoker = Invoker::new(&registry);
    let doc = document();
    let error = invoker
        .invoke(
            &doc,
            &session,
            Invocation::new(
                "test.save.atomic",
                json!({
                    "target": scratch.join("a.txt"),
                    "temporary": scratch.join("a.tmp"),
                    "text": "x",
                }),
            ),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.permission_denied");
    assert!(!scratch.join("a.tmp").exists());
}
