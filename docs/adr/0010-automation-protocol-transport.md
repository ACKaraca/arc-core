# 0010. Automation protocol transport: newline-delimited JSON-RPC over stdio and user-scoped local sockets

- Status: Accepted

## Context

The suite decision defines the Arc Automation Protocol as JSON-RPC over stdio or a local socket.
The framing, the local-socket security model and the handling of large payloads must be fixed so
that clients in any language can implement it and so that the protocol does not open the user's
machine to other local users or remote access.

## Decision

- Messages are JSON-RPC 2.0, UTF-8, one message per line (newline-delimited JSON), at most 64 MiB.
  Payloads larger than that are exchanged as files inside the session's write roots.
- Headless sessions use stdio; logs go to stderr only.
- Running applications listen only when the user enables automation, on a Unix domain socket or a
  Windows named pipe inside a per-user directory with user-only permissions. Clients present a
  session token read from a user-only file in the same directory. No TCP listener exists.
- `arc.initialize` negotiates a protocol version (`major.minor`) and permission classes; the server
  supports the current and previous major.
- The Arc Scene session and `arc-worker` reuse the same framing; binary bulk data travels on a
  separate length-prefixed data channel where needed.

## Consequences

- Any language with JSON and pipes can be a client; the Model Context Protocol endpoint shares the
  framing.
- Binary data (images) is base64-encoded inline up to a limit or passed as files, which costs size
  but keeps one framing.
- Remote automation requires an explicit tunnel owned by the user, never a built-in listener.

## Amendment: local socket handshake and Model Context Protocol framing

Additive; the protocol version stays 1.0 and no method, parameter or result of the protocol
changes.

- A connection to the local socket starts with one transport message before the protocol:
  `{"jsonrpc":"2.0","id":<id>,"method":"arc.authenticate","params":{"token":"<token>"}}`, at most
  1,024 bytes before its newline, answered with `{"result":{}}`. The whole line must arrive within
  10 seconds of the connection being accepted: one absolute deadline, not a timeout per read. A
  connection that fails it receives one `cmd.socket.unauthorized` error (-32004) and is closed. The
  message is not a protocol method: stdio sessions have no handshake, and the published method
  schemas do not describe it.
- On Windows the same handshake applies to named pipes when they are provided.
- The Model Context Protocol endpoint uses the same newline-delimited framing and size limits.

Alternative rejected: carrying the token in `arc.initialize`. It would let an unauthenticated
peer reach the protocol parser and would put a secret in a method every transport shares.

## Alternatives considered

- **Content-Length header framing.** Robust for embedded newlines, but JSON never contains raw
  newlines inside strings, and line framing is simpler for scripting tools.
- **HTTP on localhost.** Exposes the session to any local process and to browser-based attacks
  without extra measures.
- **gRPC.** Requires generated stubs and an HTTP/2 stack in every client.

## Reopen when

A required client environment cannot use local sockets or pipes, or measured throughput of
inline payloads limits a documented workflow.
