//! The escape-test suite (CORE-CMD-033): every way out of the granted roots this platform
//! allows creating is tried against `Roots::open_read` and `Roots::create_write`, the file
//! surfaces of every automation session. A canary file outside the roots must never be read,
//! written, or joined by a new file.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[cfg(unix)]
use std::io::BufReader;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

use arc_command::protocol::McpConfig;
#[cfg(unix)]
use arc_command::protocol::{LocalSocket, SocketConfig, SocketStop};
use arc_command::roots::MAX_PATH_LEN;
use arc_command::{
    CommandSpec, NoLog, Permission, Registry, Roots, Server, ServerConfig, Surface, UndoBehavior,
};
use arc_doc::Document;
use serde_json::{Value, json};

const CANARY: &str = "CANARY";

/// A temporary tree: `base/root` is the granted root, `base/outside` holds the canary.
struct Tree {
    base: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "arc-escape-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let (root, outside) = (base.join("root"), base.join("outside"));
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(base.join("root2")).unwrap();
        fs::write(root.join("inside.txt"), "inside").unwrap();
        fs::write(outside.join("canary.txt"), CANARY).unwrap();
        fs::write(base.join("root2").join("canary.txt"), CANARY).unwrap();
        // Canonical paths, so `..` and links inside the base resolve as the system sees them.
        let base = plain(fs::canonicalize(&base).unwrap());
        Self {
            root: base.join("root"),
            outside: base.join("outside"),
            base,
        }
    }

    fn roots(&self) -> Roots {
        Roots::new([&self.root], [&self.root]).unwrap()
    }

    /// Reads and writes `path`; any file it gets must not be the canary.
    fn probe(&self, roots: &Roots, path: &Path) {
        if let Ok(mut file) = roots.open_read(path) {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            assert_ne!(text, CANARY, "read the canary through {}", path.display());
        }
        if let Ok(mut file) = roots.create_write(path) {
            file.write_all(b"written").unwrap();
        }
    }

    /// The canary is intact and nothing new appeared outside the roots.
    fn assert_no_escape(&self) {
        assert_eq!(
            fs::read_to_string(self.outside.join("canary.txt")).unwrap(),
            CANARY
        );
        let entries: Vec<_> = fs::read_dir(&self.outside)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "files appeared outside: {entries:?}");
        assert_eq!(
            fs::read_to_string(self.base.join("root2").join("canary.txt")).unwrap(),
            CANARY
        );
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// `path` without the Windows verbatim prefix, so `..` in joined paths is resolved.
fn plain(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && !rest.starts_with("UNC")
    {
        return PathBuf::from(rest);
    }
    path
}

#[allow(clippy::print_stderr, reason = "a skipped case says why")]
fn skip(case: &str, why: &str) {
    eprintln!("escape suite: skipped {case}: {why}");
}

/// Creates a symbolic link at `at` to `target`; false when the platform refuses.
fn link(target: &Path, at: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at).is_ok()
    }
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, at).is_ok()
        } else {
            std::os::windows::fs::symlink_file(target, at).is_ok()
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, at);
        false
    }
}

fn code(result: std::io::Result<()>, roots_result: arc_base::Result<fs::File>) -> String {
    result.unwrap();
    roots_result.map(drop).unwrap_err().code().to_owned()
}

#[test]
fn paths_inside_work() {
    let t = Tree::new("inside");
    let roots = t.roots();
    let mut text = String::new();
    roots
        .open_read(&t.root.join("sub").join("..").join("inside.txt"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "inside");
    roots
        .create_write(&t.root.join("sub").join("new.txt"))
        .unwrap()
        .write_all(b"n")
        .unwrap();
    roots.create_write(&t.root.join("inside.txt")).unwrap();
    assert_eq!(
        fs::read_to_string(t.root.join("inside.txt")).unwrap(),
        "",
        "truncated"
    );
    assert_eq!(
        fs::read_to_string(t.root.join("sub").join("new.txt")).unwrap(),
        "n"
    );
    t.assert_no_escape();
}

#[test]
fn dot_dot_chains_and_prefix_siblings_stay_out() {
    let t = Tree::new("dotdot");
    let roots = t.roots();
    let out = |p: PathBuf| {
        let read = roots.open_read(&p).map(drop).unwrap_err();
        assert_eq!(read.code(), "cmd.path.outside_roots", "{}", p.display());
        let write = roots.create_write(&p).map(drop).unwrap_err();
        assert_eq!(write.code(), "cmd.path.outside_roots", "{}", p.display());
    };
    out(t.root.join("..").join("outside").join("canary.txt"));
    out(t
        .root
        .join("sub")
        .join("..")
        .join("..")
        .join("outside")
        .join("canary.txt"));
    let mut deep = t.root.clone();
    for _ in 0..40 {
        deep.push("sub");
        deep.push("..");
    }
    out(deep.join("..").join("outside").join("canary.txt"));
    out(t.base.join("root2").join("canary.txt"));
    out(t.root.join("..").join("outside").join("new.txt"));
    assert_eq!(
        roots.create_write(&t.root).map(drop).unwrap_err().code(),
        "cmd.path.outside_roots"
    );
    t.assert_no_escape();
}

#[test]
fn malformed_and_long_paths_are_rejected() {
    let t = Tree::new("text");
    let roots = t.roots();
    let rejected = |p: &Path| roots.open_read(p).map(drop).unwrap_err().code().to_owned();
    assert_eq!(rejected(Path::new("root/inside.txt")), "cmd.path.rejected");
    assert_eq!(rejected(&t.root.join("in\0side.txt")), "cmd.path.rejected");
    assert_eq!(rejected(&t.root.join("sub")), "cmd.path.rejected");
    let long = t.root.join("a/".repeat(MAX_PATH_LEN / 2 + 1));
    assert_eq!(rejected(&long), "cmd.path.too_long");
    assert_eq!(
        roots
            .create_write(&long.join("f"))
            .map(drop)
            .unwrap_err()
            .code(),
        "cmd.path.too_long"
    );
    // A single component longer than the system allows.
    let name = t.root.join("n".repeat(300));
    assert!(matches!(
        rejected(&name).as_str(),
        "cmd.path.too_long" | "cmd.path.not_found"
    ));
    assert_eq!(
        roots
            .create_write(&t.root.join("sub").join(".."))
            .map(drop)
            .unwrap_err()
            .code(),
        "cmd.path.rejected"
    );
    #[cfg(windows)]
    {
        for bad in ["inside.txt:stream", "CON", "nul.txt", "COM1.log"] {
            assert_eq!(rejected(&t.root.join(bad)), "cmd.path.rejected", "{bad}");
        }
        assert_eq!(
            rejected(Path::new(r"\\.\PhysicalDrive0")),
            "cmd.path.rejected"
        );
        assert_eq!(
            rejected(Path::new(r"\\server\share\x")),
            "cmd.path.rejected"
        );
    }
    t.assert_no_escape();
}

#[test]
fn symbolic_links_out_are_refused_and_links_in_work() {
    let t = Tree::new("links");
    let roots = t.roots();
    let canary = t.outside.join("canary.txt");
    let cases = [
        (canary.clone(), "abs"),
        (
            PathBuf::from("..").join("outside").join("canary.txt"),
            "rel",
        ),
        (t.outside.clone(), "dir"),
        (t.outside.join("dangling.txt"), "dangling"),
    ];
    for (target, name) in &cases {
        if !link(target, &t.root.join(name)) {
            skip(
                "symbolic links",
                "the system refuses to create them without privileges",
            );
            return;
        }
    }
    assert!(link(&t.root.join("loop_b"), &t.root.join("loop_a")));
    assert!(link(&t.root.join("loop_a"), &t.root.join("loop_b")));
    assert!(link(&t.root.join("inside.txt"), &t.root.join("in_link")));
    for name in ["abs", "rel"] {
        let p = t.root.join(name);
        assert_eq!(code(Ok(()), roots.open_read(&p)), "cmd.path.outside_roots");
        assert_eq!(
            code(Ok(()), roots.create_write(&p)),
            "cmd.path.outside_roots"
        );
    }
    assert_eq!(
        code(
            Ok(()),
            roots.open_read(&t.root.join("dir").join("canary.txt"))
        ),
        "cmd.path.outside_roots"
    );
    assert_eq!(
        code(
            Ok(()),
            roots.create_write(&t.root.join("dir").join("new.txt"))
        ),
        "cmd.path.outside_roots"
    );
    assert!(roots.create_write(&t.root.join("dangling")).is_err());
    assert!(roots.open_read(&t.root.join("loop_a")).is_err());
    assert!(roots.create_write(&t.root.join("loop_a")).is_err());
    let mut text = String::new();
    roots
        .open_read(&t.root.join("in_link"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "inside");
    roots
        .create_write(&t.root.join("in_link"))
        .unwrap()
        .write_all(b"via link")
        .unwrap();
    assert_eq!(
        fs::read_to_string(t.root.join("inside.txt")).unwrap(),
        "via link"
    );
    // A root reached through a link is canonical; a link to the root from outside is a way in.
    assert!(link(&t.root, &t.base.join("alias")));
    roots
        .open_read(&t.base.join("alias").join("inside.txt"))
        .unwrap();
    t.assert_no_escape();
}

#[test]
fn normalization_and_case_variants_resolve_to_what_the_system_opens() {
    let t = Tree::new("unicode");
    let nfc = "caf\u{e9}";
    let nfd = "cafe\u{301}";
    fs::create_dir_all(t.root.join(nfc)).unwrap();
    fs::write(t.root.join(nfc).join("menu.txt"), "inside").unwrap();
    // The decomposed name outside holds a canary; on systems that treat both forms as one name
    // it is the same directory as the root's, which is fine as long as the canary stays unread.
    let outside_variant = t.base.join(nfd);
    if fs::create_dir_all(&outside_variant).is_ok() {
        let _ = fs::write(outside_variant.join("menu.txt"), CANARY);
    }
    let roots = t.roots();
    for name in [nfc, nfd, "CAF\u{c9}", "CAFE\u{301}"] {
        t.probe(&roots, &t.root.join("..").join(name).join("menu.txt"));
        t.probe(&roots, &t.root.join(name).join("menu.txt"));
    }
    for variant in ["ROOT", "Root", "root\u{200b}"] {
        t.probe(&roots, &t.base.join(variant).join("inside.txt"));
        t.probe(
            &roots,
            &t.base
                .join(variant)
                .join("..")
                .join("outside")
                .join("canary.txt"),
        );
    }
    if let Ok(text) = fs::read_to_string(outside_variant.join("menu.txt")) {
        assert_eq!(text, CANARY, "the outside variant was written");
    }
    t.assert_no_escape();
}

#[cfg(windows)]
#[test]
fn junctions_out_are_refused() {
    let t = Tree::new("junction");
    let at = t.root.join("junction");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&at)
        .arg(&t.outside)
        .output()
        .is_ok_and(|o| o.status.success());
    if !made {
        skip("junctions", "mklink /J failed");
        return;
    }
    let roots = t.roots();
    t.probe(&roots, &at.join("canary.txt"));
    t.probe(&roots, &at.join("new.txt"));
    assert!(roots.open_read(&at.join("canary.txt")).is_err());
    t.assert_no_escape();
}

#[cfg(unix)]
/// Swaps `at` between a regular file inside the root and a link to `target` while `probe` runs.
fn race(
    t: &Tree,
    at: &Path,
    target: &Path,
    make_inside: impl Fn(&Path) + Sync,
    probe: impl Fn() + Sync,
) {
    let stop = std::sync::atomic::AtomicBool::new(false);
    let staged_link = t.root.join("staged_link");
    let staged = t.root.join("staged");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            // A link cannot replace a directory in one rename, so a directory goes first.
            let clear = || {
                if at.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                    let _ = fs::remove_dir_all(at);
                }
            };
            for _ in 0..500 {
                let _ = fs::remove_file(&staged_link);
                if link(target, &staged_link) {
                    clear();
                    let _ = fs::rename(&staged_link, at);
                }
                make_inside(&staged);
                clear();
                let _ = fs::rename(&staged, at);
            }
            stop.store(true, Ordering::SeqCst);
        });
        while !stop.load(Ordering::SeqCst) {
            probe();
        }
    });
}

#[cfg(unix)]
#[test]
fn a_file_swapped_for_a_link_never_exposes_the_canary() {
    let t = Tree::new("race-file");
    let roots = t.roots();
    let at = t.root.join("swap");
    fs::write(&at, "inside").unwrap();
    let canary = t.outside.join("canary.txt");
    race(
        &t,
        &at,
        &canary,
        |staged| {
            let _ = fs::remove_dir_all(staged);
            let _ = fs::write(staged, "inside");
        },
        || t.probe(&roots, &at),
    );
    t.assert_no_escape();
}

#[cfg(unix)]
#[test]
fn a_directory_swapped_for_a_link_never_receives_new_files() {
    let t = Tree::new("race-dir");
    let roots = t.roots();
    let at = t.root.join("d");
    fs::create_dir(&at).unwrap();
    let n = AtomicUsize::new(0);
    race(
        &t,
        &at,
        &t.outside,
        |staged| {
            let _ = fs::remove_file(staged);
            let _ = fs::create_dir(staged);
        },
        || {
            let i = n.fetch_add(1, Ordering::SeqCst);
            t.probe(&roots, &at.join(format!("new{i}.txt")));
            t.probe(&roots, &at.join("canary.txt"));
        },
    );
    t.assert_no_escape();
}

// Every surface: the same escape cases through the stdio protocol, the Model Context Protocol
// endpoint and the local socket, each reading and writing through a registered command.

/// The refusal codes of the cases.
const OUTSIDE: &str = "cmd.path.outside_roots";
const REJECTED: &str = "cmd.path.rejected";
const TOO_LONG: &str = "cmd.path.too_long";
#[cfg(unix)]
const NOT_FOUND: &str = "cmd.path.not_found";

/// What a read or write of a path that is being swapped may answer besides its content: a path
/// refusal. Anything else (another code, a transport failure) fails the race tests.
#[cfg(unix)]
const REFUSALS: [&str; 4] = [OUTSIDE, REJECTED, TOO_LONG, NOT_FOUND];

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathParam {
    path: String,
}

/// The two file commands of the suite: one reads a file, one writes `written` to a file.
fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(
        CommandSpec::new(
            "test.file.read",
            "test-read",
            "test-read-description",
            |ctx, p: PathParam| -> arc_base::Result<String> {
                let mut text = String::new();
                ctx.open_read(Path::new(&p.path))?
                    .read_to_string(&mut text)
                    .map_err(|_| arc_base::Error::new(arc_command::error::codes::PATH_IO))?;
                Ok(text)
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read, Permission::FileRead]),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.file.write",
            "test-write",
            "test-write-description",
            |ctx, p: PathParam| -> arc_base::Result<()> {
                ctx.create_write(Path::new(&p.path))?
                    .write_all(b"written")
                    .map_err(|_| arc_base::Error::new(arc_command::error::codes::PATH_IO))
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Edit, Permission::FileWrite]),
    )
    .unwrap();
    r
}

/// What every surface serves: the commands, the document and a session granted file access.
struct Fixture {
    registry: Registry,
    docs: BTreeMap<String, Document>,
    config: ServerConfig,
}

fn fixture(root: &Path) -> Fixture {
    let mut config = ServerConfig::new(
        "arc-escape",
        "0.1.0",
        [
            Permission::Read,
            Permission::Edit,
            Permission::FileRead,
            Permission::FileWrite,
        ],
    );
    config.roots = Roots::new([root], [root]).unwrap();
    Fixture {
        registry: registry(),
        docs: BTreeMap::from([("main".to_owned(), common::document())]),
        config,
    }
}

/// The reply of request `id` among the lines of `output`.
fn reply(output: &[u8], id: u64) -> Value {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|v| v["id"] == json!(id))
        .unwrap()
}

/// The text of a protocol reply: the value, or the stable error code.
fn outcome(reply: &Value) -> Result<String, String> {
    if let Some(error) = reply.get("error") {
        return Err(error["message"].as_str().unwrap_or("?").to_owned());
    }
    Ok(reply["result"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn initialize() -> String {
    request(
        1,
        "arc.initialize",
        json!({"clientName": "escape", "protocolVersions": {"min": "1.0", "max": "1.0"},
               "permissions": ["read", "edit", "file_read", "file_write"]}),
    )
}

fn run(command: &str, path: &Path) -> String {
    request(
        2,
        "command.run",
        json!({"document": "main", "command": command,
               "params": {"path": path.to_string_lossy()}}),
    )
}

/// The reply of request `id` read from a session's output.
fn read_reply(from: &mut impl BufRead, id: u64) -> Value {
    loop {
        let mut line = String::new();
        assert!(from.read_line(&mut line).unwrap() > 0, "the session ended");
        let message: Value = serde_json::from_str(&line).unwrap();
        if message["id"] == json!(id) {
            return message;
        }
    }
}

/// One command run in a stdio protocol session. The session is driven over the crate's pipe and
/// its input is closed only after the answer: a stream that ends earlier cancels the run.
fn stdio_run(fx: &Fixture, command: &str, path: &Path) -> Result<String, String> {
    let server = Server::new(&fx.registry, &fx.docs, fx.config.clone(), &NoLog);
    let (mut to_server, server_in) = arc_command::protocol::pipe();
    let (server_out, from_server) = arc_command::protocol::pipe();
    let mut from_server = from_server.with_timeout(Duration::from_secs(30));
    std::thread::scope(|scope| {
        let served = scope.spawn(|| server.serve(server_in, server_out));
        writeln!(to_server, "{}", initialize()).unwrap();
        let _ = read_reply(&mut from_server, 1);
        writeln!(to_server, "{}", run(command, path)).unwrap();
        let answer = read_reply(&mut from_server, 2);
        drop(to_server);
        served.join().unwrap().unwrap();
        outcome(&answer)
    })
}

/// One tool call in a Model Context Protocol session.
fn mcp_run(fx: &Fixture, tool: &str, path: &Path) -> Result<String, String> {
    let mut config = fx.config.clone();
    config.surface = Surface::Mcp;
    let server = Server::new(&fx.registry, &fx.docs, config, &NoLog);
    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                      "params": {"name": tool, "arguments": {"path": path.to_string_lossy()}}});
    let input = format!(
        "{}\n{}\n{call}\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    let mut out = Vec::new();
    server
        .serve_mcp(
            &McpConfig::new("main"),
            Cursor::new(input.into_bytes()),
            &mut out,
        )
        .unwrap();
    let answer = reply(&out, 2);
    if let Some(error) = answer.get("error") {
        return Err(error["message"].as_str().unwrap_or("?").to_owned());
    }
    let result = &answer["result"];
    let value: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    if result["isError"] == json!(true) {
        Err(value["code"].as_str().unwrap_or("?").to_owned())
    } else {
        // The text is the run's result, whose `value` is the command's answer.
        Ok(value["value"].as_str().unwrap_or_default().to_owned())
    }
}

/// A surface that reads and writes a path through the file commands.
trait Gateway: Sync {
    fn name(&self) -> &'static str;
    fn read(&self, path: &Path) -> Result<String, String>;
    fn write(&self, path: &Path) -> Result<(), String>;
}

struct Stdio<'a>(&'a Fixture);

impl Gateway for Stdio<'_> {
    fn name(&self) -> &'static str {
        "stdio"
    }
    fn read(&self, path: &Path) -> Result<String, String> {
        stdio_run(self.0, "test.file.read", path)
    }
    fn write(&self, path: &Path) -> Result<(), String> {
        stdio_run(self.0, "test.file.write", path).map(drop)
    }
}

struct Mcp<'a>(&'a Fixture);

impl Gateway for Mcp<'_> {
    fn name(&self) -> &'static str {
        "mcp"
    }
    fn read(&self, path: &Path) -> Result<String, String> {
        mcp_run(self.0, "test_file_read", path)
    }
    fn write(&self, path: &Path) -> Result<(), String> {
        mcp_run(self.0, "test_file_write", path).map(drop)
    }
}

/// A connection to the local socket, authenticated with its token, one session per call.
#[cfg(unix)]
struct Socket {
    path: PathBuf,
    token: String,
}

#[cfg(unix)]
impl Socket {
    fn run(&self, command: &str, path: &Path) -> Result<String, String> {
        let mut stream = UnixStream::connect(&self.path).map_err(|_| "connect".to_owned())?;
        let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "arc.authenticate",
                           "params": {"token": self.token}});
        let input = format!("{hello}\n{}\n{}\n", initialize(), run(command, path));
        stream
            .write_all(input.as_bytes())
            .map_err(|_| "write".to_owned())?;
        let mut replies = Vec::new();
        let mut lines = BufReader::new(stream);
        loop {
            let mut line = String::new();
            if lines.read_line(&mut line).unwrap() == 0 {
                break;
            }
            replies.extend_from_slice(line.as_bytes());
            if serde_json::from_str::<Value>(&line).unwrap()["id"] == json!(2) {
                break;
            }
        }
        outcome(&reply(&replies, 2))
    }
}

#[cfg(unix)]
impl Gateway for Socket {
    fn name(&self) -> &'static str {
        "socket"
    }
    fn read(&self, path: &Path) -> Result<String, String> {
        self.run("test.file.read", path)
    }
    fn write(&self, path: &Path) -> Result<(), String> {
        self.run("test.file.write", path).map(drop)
    }
}

/// Runs `body` with every surface. The socket serves the fixture for the duration of `body`.
#[cfg(unix)]
fn with_gateways(t: &Tree, fx: &Fixture, body: impl FnOnce(&[&dyn Gateway])) {
    let config = SocketConfig {
        enabled: true,
        directory: t.base.join("run"),
        name: "arc".into(),
        handshake_timeout: Duration::from_secs(10),
    };
    let socket = LocalSocket::bind(&config).unwrap();
    let socket_gateway = Socket {
        path: socket.socket_path().to_owned(),
        token: fs::read_to_string(socket.token_path()).unwrap(),
    };
    let server = Server::new(&fx.registry, &fx.docs, fx.config.clone(), &NoLog);
    let stop = socket.stopper();
    let (stdio, mcp) = (Stdio(fx), Mcp(fx));
    std::thread::scope(|scope| {
        let served = scope.spawn(|| socket.serve(&server));
        // Stops the socket even when a case panics, so a failure is not a hang.
        struct StopOnDrop(SocketStop);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        let guard = StopOnDrop(stop);
        body(&[&stdio, &mcp, &socket_gateway]);
        drop(guard);
        served.join().unwrap().unwrap();
    });
}

#[cfg(not(unix))]
fn with_gateways(_: &Tree, fx: &Fixture, body: impl FnOnce(&[&dyn Gateway])) {
    let (stdio, mcp) = (Stdio(fx), Mcp(fx));
    body(&[&stdio, &mcp]);
}

/// Links the escape cases need inside the root: each points outside, or nowhere.
fn plant_links(t: &Tree) {
    #[cfg(unix)]
    {
        let canary = t.outside.join("canary.txt");
        assert!(link(&canary, &t.root.join("abs_link")));
        assert!(link(
            Path::new("../outside/canary.txt"),
            &t.root.join("rel_link")
        ));
        assert!(link(&t.outside, &t.root.join("dir_link")));
        assert!(link(
            &t.outside.join("dangling.txt"),
            &t.root.join("dangling_link")
        ));
    }
    #[cfg(not(unix))]
    let _ = t;
}

/// One escape attempt and the code every surface must answer for reading and for writing.
struct Case {
    name: &'static str,
    path: PathBuf,
    read: &'static str,
    write: &'static str,
}

fn case(name: &'static str, path: PathBuf, code: &'static str) -> Case {
    Case {
        name,
        path,
        read: code,
        write: code,
    }
}

fn cases(t: &Tree) -> Vec<Case> {
    let mut deep = t.root.clone();
    for _ in 0..40 {
        deep.push("sub");
        deep.push("..");
    }
    let long = t.root.join("a/".repeat(MAX_PATH_LEN / 2 + 1));
    let mut cases = vec![
        case(
            "absolute path outside the roots",
            t.outside.join("canary.txt"),
            OUTSIDE,
        ),
        case(
            "dot-dot out of the root",
            t.root.join("..").join("outside").join("canary.txt"),
            OUTSIDE,
        ),
        case(
            "dot-dot chain past the root",
            deep.join("..").join("outside").join("canary.txt"),
            OUTSIDE,
        ),
        case(
            "sibling root",
            t.base.join("root2").join("canary.txt"),
            OUTSIDE,
        ),
        case("relative path", PathBuf::from("inside.txt"), REJECTED),
        case("NUL in the name", t.root.join("in\0side.txt"), REJECTED),
        case("a directory", t.root.join("sub"), REJECTED),
        case("over-long path", long, TOO_LONG),
    ];
    #[cfg(unix)]
    {
        cases.push(case("absolute link out", t.root.join("abs_link"), OUTSIDE));
        cases.push(case("relative link out", t.root.join("rel_link"), OUTSIDE));
        cases.push(case(
            "file through a link to a directory out",
            t.root.join("dir_link").join("canary.txt"),
            OUTSIDE,
        ));
        cases.push(Case {
            name: "dangling link out",
            path: t.root.join("dangling_link"),
            read: NOT_FOUND,
            write: NOT_FOUND,
        });
        cases.push(case("device node", PathBuf::from("/dev/null"), OUTSIDE));
    }
    #[cfg(windows)]
    {
        cases.push(case("reserved device name", t.root.join("CON"), REJECTED));
        cases.push(case(
            "alternate data stream",
            t.root.join("inside.txt:stream"),
            REJECTED,
        ));
    }
    cases
}

#[test]
fn every_surface_refuses_every_escape_case_with_the_same_code() {
    let t = Tree::new("surfaces");
    plant_links(&t);
    let fx = fixture(&t.root);
    with_gateways(&t, &fx, |gateways| {
        for case in cases(&t) {
            for gateway in gateways {
                let read = gateway.read(&case.path).err();
                assert_eq!(
                    read.as_deref(),
                    Some(case.read),
                    "read: {} via {}",
                    case.name,
                    gateway.name()
                );
                let write = gateway.write(&case.path).err();
                assert_eq!(
                    write.as_deref(),
                    Some(case.write),
                    "write: {} via {}",
                    case.name,
                    gateway.name()
                );
            }
        }
    });
    t.assert_no_escape();
}

#[cfg(unix)]
#[test]
fn a_file_swapped_for_a_link_is_never_read_on_any_surface() {
    let t = Tree::new("surfaces-race-file");
    let fx = fixture(&t.root);
    let at = t.root.join("swap");
    fs::write(&at, "inside").unwrap();
    with_gateways(&t, &fx, |gateways| {
        // The positive control: before the race, each surface reads the file inside the root.
        for gateway in gateways {
            assert_eq!(
                gateway.read(&at),
                Ok("inside".to_owned()),
                "{} cannot read a file inside the root",
                gateway.name()
            );
        }
        race(
            &t,
            &at,
            &t.outside.join("canary.txt"),
            |staged| {
                let _ = fs::remove_dir_all(staged);
                let _ = fs::write(staged, "inside");
            },
            || {
                for gateway in gateways {
                    match gateway.read(&at) {
                        Ok(text) => {
                            assert_ne!(text, CANARY, "{} read the canary", gateway.name());
                        }
                        Err(code) => assert!(
                            REFUSALS.contains(&code.as_str()),
                            "{} answered {code}",
                            gateway.name()
                        ),
                    }
                }
            },
        );
    });
    t.assert_no_escape();
}

#[cfg(unix)]
#[test]
fn a_directory_swapped_for_a_link_receives_no_file_on_any_surface() {
    let t = Tree::new("surfaces-race-dir");
    let fx = fixture(&t.root);
    let at = t.root.join("d");
    fs::create_dir(&at).unwrap();
    fs::write(at.join("ok.txt"), "inside").unwrap();
    let n = AtomicUsize::new(0);
    with_gateways(&t, &fx, |gateways| {
        // The positive control: before the race, each surface reads the file inside the root.
        for gateway in gateways {
            assert_eq!(
                gateway.read(&at.join("ok.txt")),
                Ok("inside".to_owned()),
                "{} cannot read a file inside the root",
                gateway.name()
            );
        }
        race(
            &t,
            &at,
            &t.outside,
            |staged| {
                let _ = fs::remove_file(staged);
                let _ = fs::create_dir(staged);
            },
            || {
                let i = n.fetch_add(1, Ordering::SeqCst);
                for gateway in gateways {
                    match gateway.write(&at.join(format!("new{i}.txt"))) {
                        Ok(()) => {}
                        Err(code) => assert!(
                            REFUSALS.contains(&code.as_str()),
                            "write: {} answered {code}",
                            gateway.name()
                        ),
                    }
                    match gateway.read(&at.join("canary.txt")) {
                        Ok(text) => {
                            assert_ne!(text, CANARY, "{} read the canary", gateway.name());
                        }
                        Err(code) => assert!(
                            REFUSALS.contains(&code.as_str()),
                            "read: {} answered {code}",
                            gateway.name()
                        ),
                    }
                }
            },
        );
    });
    t.assert_no_escape();
}
