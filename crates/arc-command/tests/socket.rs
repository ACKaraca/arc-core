//! Local socket sessions (CORE-CMD-020): the automation protocol over a user-scoped Unix
//! socket with token authentication, client and server on threads of one test.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]
#![cfg(unix)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Barrier, Mutex};
use std::time::Duration;

use arc_command::protocol::{
    ConnectionObserver, LocalSocket, MAX_CLIENTS, MAX_HANDSHAKE_BYTES, MAX_HANDSHAKING, NoObserver,
    SocketConfig, SocketStop, ViewRenderer, ViewRequest,
};
use arc_command::{
    CommandSpec, NoLog, Permission, Registry, Roots, Server, ServerConfig, UndoBehavior,
};
use arc_doc::Document;
use common::{document, registry};
use serde_json::{Value, json};

/// A fresh directory under the system temporary directory, removed on drop.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "arc-sock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn socket_dir(&self) -> PathBuf {
        self.0.join("run")
    }

    fn config(&self, enabled: bool) -> SocketConfig {
        SocketConfig {
            enabled,
            directory: self.socket_dir(),
            name: "arc".into(),
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

struct Conn {
    reader: BufReader<UnixStream>,
    stream: UnixStream,
}

impl Conn {
    fn open(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
        }
    }

    fn send(&mut self, line: &str) {
        self.stream.write_all(line.as_bytes()).unwrap();
        // The server may close a connection that exceeded the handshake limit before the newline.
        let _ = self.stream.write_all(b"\n");
    }

    /// The next line, or `None` at the end of the stream.
    fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        (self.reader.read_line(&mut line).unwrap() > 0)
            .then(|| serde_json::from_str(&line).unwrap())
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string(),
        );
        self.read().unwrap()
    }

    fn authenticate(&mut self, token: &str) -> Option<Value> {
        self.call_auth(token);
        self.read()
    }

    fn call_auth(&mut self, token: &str) {
        self.send(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate",
                    "params": {"token": token}})
            .to_string(),
        );
    }
}

fn documents() -> BTreeMap<String, Document> {
    BTreeMap::from([("main".to_owned(), document())])
}

/// Runs `client` against a serving socket and stops the server afterwards.
fn serve(temp: &Temp, max_message: u64, client: impl FnOnce(&LocalSocket)) {
    serve_config(temp.config(true), max_message, client);
}

/// [`serve`] with the given configuration.
fn serve_config(config: SocketConfig, max_message: u64, client: impl FnOnce(&LocalSocket)) {
    let socket = LocalSocket::bind(&config).unwrap();
    let mut config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read, Permission::Edit]);
    config.max_message_bytes = max_message;
    let registry = registry();
    let docs = documents();
    let server = Server::new(&registry, &docs, config, &NoLog);
    let stop = socket.stopper();
    std::thread::scope(|scope| {
        let served = scope.spawn(|| socket.serve(&server));
        // Stops the server even when the client panics, so a failure is not a hang.
        struct StopOnDrop(arc_command::protocol::SocketStop);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        let guard = StopOnDrop(stop);
        client(&socket);
        drop(guard);
        served.join().unwrap().unwrap();
    });
}

fn token(socket: &LocalSocket) -> String {
    std::fs::read_to_string(socket.token_path()).unwrap()
}

fn assert_refused_and_closed(conn: &mut Conn) {
    let refusal = conn.read().unwrap();
    assert_eq!(refusal["error"]["message"], "cmd.socket.unauthorized");
    assert_eq!(refusal["error"]["code"], -32004);
    assert_eq!(refusal["id"], Value::Null);
    assert!(refusal.get("result").is_none());
    assert!(conn.read().is_none(), "the connection stays open");
}

#[test]
fn nothing_is_created_unless_automation_is_enabled() {
    let temp = Temp::new();
    let error = LocalSocket::bind(&temp.config(false)).unwrap_err();
    assert_eq!(error.code(), "cmd.socket.disabled");
    assert!(!temp.socket_dir().exists());
}

#[test]
fn the_files_are_private_and_removed_with_the_socket() {
    let temp = Temp::new();
    let (socket_path, token_path);
    {
        let socket = LocalSocket::bind(&temp.config(true)).unwrap();
        socket_path = socket.socket_path().to_owned();
        token_path = socket.token_path().to_owned();
        assert_eq!(mode(&temp.socket_dir()), 0o700);
        assert_eq!(mode(&socket_path), 0o600);
        assert_eq!(mode(&token_path), 0o600);
        let token = token(&socket);
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        // Another token for another socket: the token is random.
        let other = Temp::new();
        let second = LocalSocket::bind(&other.config(true)).unwrap();
        assert_ne!(token, self::token(&second));
    }
    assert!(!socket_path.exists() && !token_path.exists());
}

#[test]
fn a_directory_others_can_enter_is_refused() {
    let temp = Temp::new();
    std::fs::create_dir(temp.socket_dir()).unwrap();
    std::fs::set_permissions(temp.socket_dir(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = LocalSocket::bind(&temp.config(true)).unwrap_err();
    assert_eq!(error.code(), "cmd.socket.directory");
    // A file, and a bad name.
    let file = temp.0.join("file");
    std::fs::write(&file, b"x").unwrap();
    let mut config = temp.config(true);
    config.directory = file;
    assert_eq!(
        LocalSocket::bind(&config).unwrap_err().code(),
        "cmd.socket.directory"
    );
    let mut config = temp.config(true);
    config.name = "../x".into();
    assert_eq!(
        LocalSocket::bind(&config).unwrap_err().code(),
        "cmd.socket.directory"
    );
}

#[test]
fn a_live_socket_is_not_taken_over_and_a_stale_one_is_replaced() {
    let temp = Temp::new();
    let first = LocalSocket::bind(&temp.config(true)).unwrap();
    let error = LocalSocket::bind(&temp.config(true)).unwrap_err();
    assert_eq!(error.code(), "cmd.socket.io");
    drop(first);
    // A socket file left behind by a process that died.
    let path = temp.socket_dir().join("arc.sock");
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    assert!(LocalSocket::bind(&temp.config(true)).is_ok());
}

#[test]
fn a_client_with_the_token_runs_a_session() {
    let temp = Temp::new();
    serve(&temp, 1 << 20, |socket| {
        let mut conn = Conn::open(socket.socket_path());
        let accepted = conn.authenticate(&token(socket)).unwrap();
        assert_eq!(accepted["result"], json!({}));
        assert_eq!(accepted["id"], 1);
        // The protocol is the one of the stdio transport.
        let init = conn.call(
            2,
            "arc.initialize",
            json!({"clientName": "t", "protocolVersions": {"min": "1.0", "max": "1.0"},
                   "permissions": ["read", "edit"]}),
        );
        assert_eq!(init["result"]["protocolVersion"], "1.0");
        let run = conn.call(
            3,
            "command.run",
            json!({"document": "main", "command": "test.item.create",
                   "params": {"name": "wall", "length": "2400 mm"}}),
        );
        assert_eq!(run["result"]["summary"]["counts"]["created"], 1);
        let found = conn.call(4, "query.objects", json!({"document": "main"}));
        assert_eq!(found["result"]["objects"].as_array().unwrap().len(), 1);
        // A second client in parallel, and an oversize message that is refused.
        let mut other = Conn::open(socket.socket_path());
        assert!(other.authenticate(&token(socket)).is_some());
        let init = json!({"clientName": "o", "protocolVersions": {"min": "1.0", "max": "1.0"}});
        assert_eq!(other.call(2, "arc.initialize", init).get("error"), None);
        assert_eq!(other.call(3, "arc.ping", json!({}))["result"], json!({}));
        let big = json!({"jsonrpc": "2.0", "id": 9, "method": "arc.ping",
                         "params": {"pad": "x".repeat(2 << 20)}});
        conn.send(&big.to_string());
        let refused = conn.read().unwrap();
        assert_eq!(refused["error"]["code"], -32008);
        assert_eq!(conn.call(5, "arc.ping", json!({}))["result"], json!({}));
        assert_eq!(
            conn.call(6, "arc.shutdown", json!({}))["result"],
            Value::Null
        );
    });
}

#[test]
fn a_connection_without_the_right_token_gets_only_the_refusal() {
    let temp = Temp::new();
    serve(&temp, 1 << 20, |socket| {
        let good = token(socket);
        let wrong = "0".repeat(64);
        let cases = [
            // Wrong token, a prefix and an extension of the right one.
            json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate", "params": {"token": wrong}}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate", "params": {"token": &good[..63]}}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate", "params": {"token": format!("{good}0")}}),
            // No token, a token of the wrong type, another method first, a missing id.
            json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate", "params": {}}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate", "params": {"token": 5}}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "arc.ping", "params": {"token": good}}),
            json!({"jsonrpc": "2.0", "method": "arc.authenticate", "params": {"token": good}}),
            json!([1]),
        ];
        for case in cases {
            let mut conn = Conn::open(socket.socket_path());
            conn.send(&case.to_string());
            assert_refused_and_closed(&mut conn);
        }
        for raw in ["garbage", "", &"x".repeat(5000)] {
            let mut conn = Conn::open(socket.socket_path());
            conn.send(raw);
            assert_refused_and_closed(&mut conn);
        }
        // A client that never authenticates cannot reach a method, and the server still
        // serves clients that do.
        let mut conn = Conn::open(socket.socket_path());
        conn.send(r#"{"jsonrpc":"2.0","id":1,"method":"command.list","params":{}}"#);
        assert_refused_and_closed(&mut conn);
        let mut conn = Conn::open(socket.socket_path());
        assert_eq!(conn.authenticate(&good).unwrap()["result"], json!({}));
    });
}

#[test]
fn stopping_closes_open_connections() {
    let temp = Temp::new();
    serve(&temp, 1 << 20, |socket| {
        let mut conn = Conn::open(socket.socket_path());
        assert!(conn.authenticate(&token(socket)).is_some());
        socket.stopper().stop();
        assert!(conn.read().is_none());
    });
}

#[test]
fn a_failed_bind_leaves_the_first_instance_serving() {
    let temp = Temp::new();
    serve(&temp, 1 << 20, |socket| {
        let before = token(socket);
        let error = LocalSocket::bind(&temp.config(true)).unwrap_err();
        assert_eq!(error.code(), "cmd.socket.io");
        assert_eq!(token(socket), before);
        assert!(socket.socket_path().exists());
        let mut conn = Conn::open(socket.socket_path());
        assert_eq!(conn.authenticate(&before).unwrap()["result"], json!({}));
    });
}

#[test]
fn a_failed_bind_removes_only_what_it_created() {
    let temp = Temp::new();
    let first = LocalSocket::bind(&temp.config(true)).unwrap();
    let before = std::fs::read_to_string(first.token_path()).unwrap();
    // The socket file vanishes while the first instance lives; its token file stays.
    std::fs::remove_file(first.socket_path()).unwrap();
    let error = LocalSocket::bind(&temp.config(true)).unwrap_err();
    assert_eq!(error.code(), "cmd.socket.io");
    assert_eq!(std::fs::read_to_string(first.token_path()).unwrap(), before);
    assert!(!first.socket_path().exists());
}

#[test]
fn the_debug_output_omits_the_token() {
    let temp = Temp::new();
    let socket = LocalSocket::bind(&temp.config(true)).unwrap();
    let text = format!("{socket:?}");
    assert!(text.contains("LocalSocket"));
    assert!(!text.contains(&token(&socket)));
}

#[test]
fn a_handshake_line_is_bounded_and_a_line_at_the_limit_is_read() {
    let temp = Temp::new();
    serve(&temp, 1 << 20, |socket| {
        let good = token(socket);
        // More than the limit without a newline: refused and closed.
        let mut conn = Conn::open(socket.socket_path());
        conn.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let _ = conn
            .stream
            .write_all(&vec![b'a'; MAX_HANDSHAKE_BYTES + 4096]);
        assert_refused_and_closed(&mut conn);
        // A handshake of exactly the limit, newline excluded, is accepted.
        let base = json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate",
                          "params": {"token": good, "pad": ""}})
        .to_string();
        let pad = "x".repeat(MAX_HANDSHAKE_BYTES - base.len());
        let line = base.replace(r#""pad":"""#, &format!(r#""pad":"{pad}""#));
        assert_eq!(line.len(), MAX_HANDSHAKE_BYTES);
        let mut conn = Conn::open(socket.socket_path());
        conn.send(&line);
        assert_eq!(conn.read().unwrap()["result"], json!({}));
    });
}

#[test]
fn the_handshake_has_one_deadline_for_the_whole_line() {
    let temp = Temp::new();
    let mut config = temp.config(true);
    config.handshake_timeout = Duration::from_millis(200);
    serve_config(config, 1 << 20, |socket| {
        // One byte, then nothing: the deadline ends the connection.
        let mut silent = Conn::open(socket.socket_path());
        silent
            .stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        silent.stream.write_all(b"{").unwrap();
        assert_refused_and_closed(&mut silent);
        // Bytes that keep arriving do not extend the deadline.
        let mut trickle = Conn::open(socket.socket_path());
        trickle
            .stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        for _ in 0..40 {
            let _ = trickle.stream.write_all(b"{");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_refused_and_closed(&mut trickle);
        // A correct client is served after both.
        let mut conn = Conn::open(socket.socket_path());
        assert_eq!(
            conn.authenticate(&token(socket)).unwrap()["result"],
            json!({})
        );
    });
}

/// Serves an already bound socket with `observer` while `client` runs, then stops it.
fn serve_bound(
    socket: &LocalSocket,
    observer: &dyn ConnectionObserver,
    client: impl FnOnce(&LocalSocket),
) {
    serve_with(socket, &registry(), &documents(), observer, client);
}

/// [`serve_bound`] over the given commands and documents, granting the server Read and Edit.
fn serve_with(
    socket: &LocalSocket,
    registry: &Registry,
    docs: &BTreeMap<String, Document>,
    observer: &dyn ConnectionObserver,
    client: impl FnOnce(&LocalSocket),
) {
    let config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read, Permission::Edit]);
    let server = Server::new(registry, docs, config, &NoLog);
    let stop = socket.stopper();
    std::thread::scope(|scope| {
        let served = scope.spawn(|| socket.serve_observed(&server, observer));
        // Stops the server even when the client panics, so a failure is not a hang.
        struct StopOnDrop(SocketStop);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        let guard = StopOnDrop(stop);
        client(socket);
        drop(guard);
        served.join().unwrap().unwrap();
    });
}

/// Sends an `arc.authenticate` line naming `client` (when given) and presenting `token`.
fn send_hello(conn: &mut Conn, client: Option<&str>, token: &str) {
    let mut params = json!({"token": token});
    if let Some(client) = client {
        params["clientName"] = json!(client);
    }
    conn.send(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "arc.authenticate", "params": params})
            .to_string(),
    );
}

/// Records every connection event, in the order the observer received them.
#[derive(Default)]
struct Events(Mutex<Vec<(&'static str, u64, String)>>);

impl ConnectionObserver for Events {
    fn opened(&self, connection: u64, client_name: String) {
        self.0
            .lock()
            .unwrap()
            .push(("opened", connection, client_name));
    }

    fn closed(&self, connection: u64) {
        self.0
            .lock()
            .unwrap()
            .push(("closed", connection, String::new()));
    }
}

#[test]
fn opened_and_closed_pair_once_per_connection_and_refusals_emit_nothing() {
    let temp = Temp::new();
    let socket = LocalSocket::bind(&temp.config(true)).unwrap();
    let good = token(&socket);
    let events = Events::default();
    serve_bound(&socket, &events, |socket| {
        let mut named = Conn::open(socket.socket_path());
        send_hello(&mut named, Some("tester"), &good);
        assert_eq!(named.read().unwrap()["result"], json!({}));
        let mut wrong = Conn::open(socket.socket_path());
        send_hello(&mut wrong, Some("tester"), &"0".repeat(64));
        assert_refused_and_closed(&mut wrong);
        // A client that never names itself is admitted in single-token mode.
        let mut anonymous = Conn::open(socket.socket_path());
        send_hello(&mut anonymous, None, &good);
        assert_eq!(anonymous.read().unwrap()["result"], json!({}));
        // The named client disconnects; the anonymous one is closed by the stop.
        drop(named);
    });
    let events = events.0.lock().unwrap();
    assert_eq!(events.len(), 4, "{events:?}");
    let opened: Vec<&(&str, u64, String)> = events.iter().filter(|e| e.0 == "opened").collect();
    let names: BTreeSet<&str> = opened.iter().map(|e| e.2.as_str()).collect();
    assert_eq!(names, BTreeSet::from(["tester", ""]));
    let ids: BTreeSet<u64> = opened.iter().map(|e| e.1).collect();
    assert_eq!(ids.len(), 2);
    for id in ids {
        let at = |kind: &str| events.iter().position(|e| e.0 == kind && e.1 == id);
        let closed: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.0 == "closed" && e.1 == id)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(closed.len(), 1, "connection {id} closed once");
        assert!(at("opened").unwrap() < closed[0], "opened before closed");
    }
}

#[test]
fn concurrent_connections_get_different_identifiers() {
    let temp = Temp::new();
    let socket = LocalSocket::bind(&temp.config(true)).unwrap();
    let good = token(&socket);
    let events = Events::default();
    serve_bound(&socket, &events, |socket| {
        let mut first = Conn::open(socket.socket_path());
        let mut second = Conn::open(socket.socket_path());
        send_hello(&mut first, None, &good);
        assert_eq!(first.read().unwrap()["result"], json!({}));
        send_hello(&mut second, None, &good);
        assert_eq!(second.read().unwrap()["result"], json!({}));
    });
    let events = events.0.lock().unwrap();
    let ids: BTreeSet<u64> = events
        .iter()
        .filter(|e| e.0 == "opened")
        .map(|e| e.1)
        .collect();
    assert_eq!(ids.len(), 2);
}

#[test]
fn each_client_runs_with_its_own_ceiling_and_token() {
    let temp = Temp::new();
    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    let reader_file = socket
        .add_client("reader", BTreeSet::from([Permission::Read]))
        .unwrap();
    let editor_file = socket
        .add_client(
            "editor",
            BTreeSet::from([Permission::Read, Permission::Edit]),
        )
        .unwrap();
    let reader_token = std::fs::read_to_string(&reader_file).unwrap();
    let editor_token = std::fs::read_to_string(&editor_file).unwrap();
    assert_eq!(mode(&reader_file), 0o600);
    assert_ne!(reader_token, editor_token);
    serve_bound(&socket, &NoObserver, |socket| {
        let edit = json!({"document": "main", "command": "test.item.create",
                          "params": {"name": "wall", "length": "2400 mm"}});
        let mut reader = Conn::open(socket.socket_path());
        send_hello(&mut reader, Some("reader"), &reader_token);
        assert_eq!(reader.read().unwrap()["result"], json!({}));
        let init = json!({"clientName": "r", "protocolVersions": {"min": "1.0", "max": "1.0"},
                          "permissions": ["read", "edit"]});
        assert_eq!(
            reader.call(2, "arc.initialize", init)["result"]["permissions"],
            json!(["read"])
        );
        let refused = reader.call(3, "command.run", edit.clone());
        assert_eq!(refused["error"]["message"], "cmd.permission_denied");
        assert_eq!(reader.call(4, "arc.ping", json!({}))["result"], json!({}));

        let mut editor = Conn::open(socket.socket_path());
        send_hello(&mut editor, Some("editor"), &editor_token);
        assert_eq!(editor.read().unwrap()["result"], json!({}));
        let init = json!({"clientName": "e", "protocolVersions": {"min": "1.0", "max": "1.0"},
                          "permissions": ["read", "edit"]});
        let granted = editor.call(2, "arc.initialize", init);
        assert!(
            granted["result"]["permissions"]
                .as_array()
                .unwrap()
                .contains(&json!("edit"))
        );
        let run = editor.call(3, "command.run", edit);
        assert_eq!(run["result"]["summary"]["counts"]["created"], 1);

        // A client presenting another client's token, an unknown name and no name are refused.
        let mut swapped = Conn::open(socket.socket_path());
        send_hello(&mut swapped, Some("reader"), &editor_token);
        assert_refused_and_closed(&mut swapped);
        let mut stranger = Conn::open(socket.socket_path());
        send_hello(&mut stranger, Some("nobody"), &reader_token);
        assert_refused_and_closed(&mut stranger);
        let mut nameless = Conn::open(socket.socket_path());
        send_hello(&mut nameless, None, &reader_token);
        assert_refused_and_closed(&mut nameless);

        // Removing the editor closes its connection, deletes its token and refuses it again.
        assert!(socket.remove_client("editor"));
        assert!(
            editor.read().is_none(),
            "the removed client's connection closes"
        );
        assert!(!editor_file.exists());
        let mut again = Conn::open(socket.socket_path());
        send_hello(&mut again, Some("editor"), &editor_token);
        assert_refused_and_closed(&mut again);
        assert!(!socket.remove_client("editor"));
    });
}

#[test]
fn client_table_names_and_duplicates_are_refused() {
    let temp = Temp::new();
    let single = LocalSocket::bind(&temp.config(true)).unwrap();
    assert_eq!(
        single.add_client("x", BTreeSet::new()).unwrap_err().code(),
        "cmd.socket.io"
    );
    drop(single);
    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    for bad in ["", "../x", "Upper"] {
        assert_eq!(
            socket.add_client(bad, BTreeSet::new()).unwrap_err().code(),
            "cmd.socket.io",
            "{bad}"
        );
    }
    let path = socket.add_client("ok", BTreeSet::new()).unwrap();
    assert!(socket.add_client("ok", BTreeSet::new()).is_err());
    drop(socket);
    assert!(
        !path.exists(),
        "dropping the socket removes its client files"
    );
}

#[test]
fn client_files_do_not_collide_with_a_single_token_socket_of_a_similar_name() {
    let temp = Temp::new();
    let config = |name: &str| SocketConfig {
        enabled: true,
        directory: temp.socket_dir(),
        name: name.into(),
        handshake_timeout: Duration::from_secs(10),
    };
    let outer = LocalSocket::bind_clients(&config("x")).unwrap();
    // The separator is outside the name alphabet, so no client name can make the file of another.
    assert_eq!(
        outer.add_client("y@z", BTreeSet::new()).unwrap_err().code(),
        "cmd.socket.io"
    );
    let client_file = outer
        .add_client("y", BTreeSet::from([Permission::Read]))
        .unwrap();
    let client_token = std::fs::read_to_string(&client_file).unwrap();
    let single = LocalSocket::bind(&config("x.y")).unwrap();
    assert_ne!(client_file, single.token_path());
    let single_token = token(&single);
    drop(outer);
    assert!(!client_file.exists());
    assert_eq!(token(&single), single_token);
    assert_ne!(client_token, single_token);
    serve_bound(&single, &NoObserver, |socket| {
        let mut conn = Conn::open(socket.socket_path());
        assert_eq!(
            conn.authenticate(&single_token).unwrap()["result"],
            json!({})
        );
    });
}

#[test]
fn the_client_table_holds_at_most_max_clients() {
    let temp = Temp::new();
    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    for i in 0..MAX_CLIENTS {
        socket
            .add_client(&format!("c{i}"), BTreeSet::new())
            .unwrap();
    }
    let error = socket.add_client("extra", BTreeSet::new()).unwrap_err();
    assert_eq!(error.code(), "cmd.socket.io");
    assert!(format!("{:?}", error.details().get("reason")).contains("clients"));
    assert!(!socket.token_path().exists());
    // Removing one client makes room for another.
    assert!(socket.remove_client("c0"));
    socket.add_client("extra", BTreeSet::new()).unwrap();
}

#[test]
fn silent_connections_cannot_take_every_connection_slot() {
    let temp = Temp::new();
    let mut config = temp.config(true);
    // No deadline in this test is reached: the silent connections are closed at the stop.
    config.handshake_timeout = Duration::from_secs(3600);
    let socket = LocalSocket::bind(&config).unwrap();
    let good = token(&socket);
    serve_bound(&socket, &NoObserver, |socket| {
        let mut authed = Conn::open(socket.socket_path());
        assert_eq!(authed.authenticate(&good).unwrap()["result"], json!({}));
        let init = json!({"clientName": "a", "protocolVersions": {"min": "1.0", "max": "1.0"}});
        assert_eq!(authed.call(2, "arc.initialize", init).get("error"), None);
        // The handshake share is full of connections that never send a line.
        let silent: Vec<UnixStream> = (0..MAX_HANDSHAKING)
            .map(|_| UnixStream::connect(socket.socket_path()).unwrap())
            .collect();
        // The next connection is closed at once: its peer reads the end of the stream, no answer.
        let mut late = Conn::open(socket.socket_path());
        late.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert!(
            late.read().is_none(),
            "a connection beyond the share was served"
        );
        // The connection that already authenticated keeps working.
        assert_eq!(authed.call(3, "arc.ping", json!({}))["result"], json!({}));
        drop(silent);
    });
}

#[test]
fn a_client_is_granted_no_permission_its_server_lacks() {
    let temp = Temp::new();
    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    let file = socket
        .add_client(
            "wide",
            BTreeSet::from([Permission::Read, Permission::Edit, Permission::FileWrite]),
        )
        .unwrap();
    let token = std::fs::read_to_string(&file).unwrap();
    // serve_bound grants the server Read and Edit only.
    serve_bound(&socket, &NoObserver, |socket| {
        let mut conn = Conn::open(socket.socket_path());
        send_hello(&mut conn, Some("wide"), &token);
        assert_eq!(conn.read().unwrap()["result"], json!({}));
        let init = json!({"clientName": "w", "protocolVersions": {"min": "1.0", "max": "1.0"},
                          "permissions": ["read", "edit", "file_write"]});
        assert_eq!(
            conn.call(2, "arc.initialize", init)["result"]["permissions"],
            json!(["read", "edit"])
        );
    });
}

/// Closes the observer's channel with the identifier of every connection that ends.
struct OnClosed(Mutex<Sender<u64>>);

impl ConnectionObserver for OnClosed {
    fn opened(&self, _: u64, _: String) {}

    fn closed(&self, connection: u64) {
        let _ = self.0.lock().unwrap().send(connection);
    }
}

/// The registry of the tests plus two commands. `test.item.block` (Read) announces that it started
/// and waits until it is released. `test.item.mark` (Edit) announces that it started and then
/// creates an item. The announcements come before any cancellation check, so a run that starts
/// is always observed.
fn blocking_registry(started: Sender<()>, release: Receiver<()>, marked: Sender<()>) -> Registry {
    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Nothing {}

    let started = Mutex::new(started);
    let release = Mutex::new(release);
    let marked = Mutex::new(marked);
    let mut registry = registry();
    registry
        .register(
            CommandSpec::new(
                "test.item.block",
                "test-block",
                "test-block-description",
                move |_ctx, _: Nothing| -> arc_base::Result<String> {
                    let _ = started.lock().unwrap().send(());
                    let _ = release.lock().unwrap().recv();
                    Ok(String::from("released"))
                },
            )
            .with_undo(UndoBehavior::NotUndoable)
            .with_permissions([Permission::Read]),
        )
        .unwrap();
    registry
        .register(
            CommandSpec::new(
                "test.item.mark",
                "test-mark",
                "test-mark-description",
                move |ctx, _: Nothing| -> arc_base::Result<String> {
                    let _ = marked.lock().unwrap().send(());
                    let id = ctx.new_id();
                    ctx.txn()?.create(
                        common::ITEM,
                        id,
                        arc_doc::Header::default(),
                        vec![Some(arc_doc::Value::String("late".into()))],
                    )?;
                    Ok(String::from("marked"))
                },
            )
            .with_permissions([Permission::Edit]),
        )
        .unwrap();
    registry
}

#[test]
fn a_revoked_client_runs_no_request_it_had_already_received() {
    let temp = Temp::new();
    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    let file = socket
        .add_client(
            "editor",
            BTreeSet::from([Permission::Read, Permission::Edit]),
        )
        .unwrap();
    let token = std::fs::read_to_string(&file).unwrap();
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let (marked_tx, marked_rx) = channel();
    let registry = blocking_registry(started_tx, release_rx, marked_tx);
    let docs = documents();
    let revision = docs["main"].snapshot().revision();
    let (closed_tx, closed_rx) = channel();
    serve_with(
        &socket,
        &registry,
        &docs,
        &OnClosed(Mutex::new(closed_tx)),
        |socket| {
            let mut conn = Conn::open(socket.socket_path());
            send_hello(&mut conn, Some("editor"), &token);
            assert_eq!(conn.read().unwrap()["result"], json!({}));
            let init = json!({"clientName": "e", "protocolVersions": {"min": "1.0", "max": "1.0"},
                              "permissions": ["read", "edit"]});
            assert_eq!(conn.call(2, "arc.initialize", init).get("error"), None);
            let block = json!({"jsonrpc": "2.0", "id": 3, "method": "command.run",
                               "params": {"document": "main", "command": "test.item.block",
                                          "params": {}}});
            let mark = json!({"jsonrpc": "2.0", "id": 4, "method": "command.run",
                              "params": {"document": "main", "command": "test.item.mark",
                                         "params": {}}});
            // Both requests in one write: the session holds the second while the first runs.
            conn.stream
                .write_all(format!("{block}\n{mark}\n").as_bytes())
                .unwrap();
            started_rx.recv().unwrap();
            assert!(socket.remove_client("editor"));
            release_tx.send(()).unwrap();
            // The session has ended, after its worker finished or dropped every queued run.
            closed_rx.recv().unwrap();
        },
    );
    assert!(
        marked_rx.try_recv().is_err(),
        "the request buffered before the revocation started"
    );
    assert_eq!(
        docs["main"].snapshot().revision(),
        revision,
        "a revoked client changed the document"
    );
}

#[test]
fn a_removed_client_name_can_be_added_again_at_once() {
    let temp = Temp::new();
    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    socket.add_client("again", BTreeSet::new()).unwrap();
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..200 {
                barrier.wait();
                assert!(socket.remove_client("again"));
                barrier.wait();
            }
        });
        for _ in 0..200 {
            barrier.wait();
            // Each add waits for the removal: it may be refused as existing until the removal
            // lands, and must succeed after that. Any other refusal is a failure.
            loop {
                match socket.add_client("again", BTreeSet::new()) {
                    Ok(_) => break,
                    Err(error) => {
                        assert_eq!(error.code(), "cmd.socket.io");
                        assert!(
                            format!("{:?}", error.details().get("reason")).contains("exists"),
                            "{error:?}"
                        );
                    }
                }
            }
            barrier.wait();
        }
    });
}

#[test]
fn client_files_left_by_a_dead_instance_do_not_block_the_name() {
    let temp = Temp::new();
    let dir = temp.socket_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Left by an instance that died: a client file of this socket name, and a file of another
    // socket name that only resembles it.
    std::fs::write(dir.join("arc@client-1.token"), "stale").unwrap();
    std::fs::write(dir.join("other@x.token"), "foreign").unwrap();
    // A link of the pattern is neither followed nor removed.
    let target = temp.0.join("target");
    std::fs::write(&target, "keep").unwrap();
    std::os::unix::fs::symlink(&target, dir.join("arc@link.token")).unwrap();

    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    assert!(
        !dir.join("arc@client-1.token").exists(),
        "the stale file is removed"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("other@x.token")).unwrap(),
        "foreign"
    );
    assert!(std::fs::symlink_metadata(dir.join("arc@link.token")).is_ok());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");

    // The name is free again: the counter's first client gets a new token that authenticates.
    let file = socket
        .add_client("client-1", BTreeSet::from([Permission::Read]))
        .unwrap();
    let token = std::fs::read_to_string(&file).unwrap();
    assert_ne!(token, "stale");
    serve_bound(&socket, &NoObserver, |socket| {
        let mut conn = Conn::open(socket.socket_path());
        send_hello(&mut conn, Some("client-1"), &token);
        assert_eq!(conn.read().unwrap()["result"], json!({}));
    });
}

/// Announces that a render started, then waits until it is released and answers a small PNG-like
/// payload.
struct BlockingRenderer {
    started: Mutex<Sender<()>>,
    release: Mutex<Receiver<()>>,
}

impl ViewRenderer for BlockingRenderer {
    fn render(&self, _: &ViewRequest<'_>) -> arc_base::Result<Vec<u8>> {
        let _ = self.started.lock().unwrap().send(());
        let _ = self.release.lock().unwrap().recv();
        Ok(vec![137, 80, 78, 71])
    }
}

#[test]
fn a_render_in_flight_writes_nothing_once_its_client_is_revoked() {
    let temp = Temp::new();
    let out_dir = temp.0.join("out");
    std::fs::create_dir(&out_dir).unwrap();
    let socket = LocalSocket::bind_clients(&temp.config(true)).unwrap();
    let file = socket
        .add_client(
            "viewer",
            BTreeSet::from([Permission::Read, Permission::FileWrite]),
        )
        .unwrap();
    let token = std::fs::read_to_string(&file).unwrap();
    let (started_tx, started_rx) = channel();
    let (release_tx, release_rx) = channel();
    let renderer = BlockingRenderer {
        started: Mutex::new(started_tx),
        release: Mutex::new(release_rx),
    };
    let mut config = ServerConfig::new(
        "arc-test",
        "0.1.0",
        [Permission::Read, Permission::Edit, Permission::FileWrite],
    );
    config.roots = Roots::new(Vec::<&Path>::new(), vec![out_dir.as_path()]).unwrap();
    let registry = registry();
    let docs = documents();
    let server = Server::new(&registry, &docs, config, &NoLog).with_view_renderer(&renderer);
    let target = out_dir.join("view.png");
    let stop = socket.stopper();
    std::thread::scope(|scope| {
        let served = scope.spawn(|| socket.serve(&server));
        struct StopOnDrop(SocketStop);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        let guard = StopOnDrop(stop);
        let mut conn = Conn::open(socket.socket_path());
        send_hello(&mut conn, Some("viewer"), &token);
        assert_eq!(conn.read().unwrap()["result"], json!({}));
        let init = json!({"clientName": "v", "protocolVersions": {"min": "1.0", "max": "1.0"},
                          "permissions": ["read", "file_write"]});
        assert_eq!(conn.call(2, "arc.initialize", init).get("error"), None);
        conn.send(
            &json!({"jsonrpc": "2.0", "id": 3, "method": "view.render",
                    "params": {"document": "main", "width": 8, "height": 8,
                               "path": target}})
            .to_string(),
        );
        started_rx.recv().unwrap();
        assert!(socket.remove_client("viewer"));
        release_tx.send(()).unwrap();
        // The connection was closed by the revocation, so the session answers nothing.
        assert!(conn.read().is_none(), "no answer reaches a revoked client");
        drop(guard);
        served.join().unwrap().unwrap();
    });
    assert!(!target.exists(), "the revoked render wrote no file");
}
