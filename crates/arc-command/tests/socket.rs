//! Local socket sessions (CORE-CMD-020): the automation protocol over a user-scoped Unix
//! socket with token authentication, client and server on threads of one test.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]
#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use arc_command::protocol::{LocalSocket, SocketConfig};
use arc_command::{NoLog, Permission, Server, ServerConfig};
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
        self.stream.write_all(b"\n").unwrap();
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
    let socket = LocalSocket::bind(&temp.config(true)).unwrap();
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
