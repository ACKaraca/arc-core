# arc-hub-api

Typed definitions of the Arc Hub API and the client every product, CLI and worker uses to talk to an
Arc Hub. The server itself lives in the [arc-hub](https://github.com/ACKaraca/arc-hub/blob/main/docs/architecture/overview.md)
repository; this crate is the shared contract.

## Owns

- Request and response types for authentication, projects, folders, files, versions, locks,
  issues, reviews, jobs, events and, later, change sets.
- The asynchronous client with retries, pagination, resumable transfers and cancellation.
- The device sign-in flow against an OpenID Connect provider and secure token storage hooks.
- API versioning and the published API description consumed by the server's tests.
- Mapping of local document identities to Hub resources.

## Stays in products

Product-specific workflows (which states a product offers, how a product shows issues), the user
interface for Hub panels, and Hub-side logic (permissions evaluation, storage, web client).

## Key concepts and types

| Concept | Description |
|---|---|
| `HubClient` | Endpoint, credentials provider, retry policy, runtime handle |
| `Project`, `Folder`, `FileItem`, `Version` | Hub resources with identifiers and metadata |
| `Lock` | Check-out lock with holder and time |
| `LifecycleState` | Work in progress, shared, published, archived, or a project-defined state |
| `Issue`, `Comment`, `Viewpoint` | Issue records with BCF-compatible viewpoints |
| `Review` | Review with participants, files and decisions |
| `Job` | Headless job request (product, command or macro, inputs, outputs) and status |
| `ChangeSetUpload` | Change set with base, author and records (stage 2) |
| `ResourceId` | Stable Hub identifier stored in document links |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-doc` (change set and identifier types) |
| Third-party | `serde`, `serde_json`, `reqwest`, `tokio`, `openidconnect` |

The crate performs network access only through a client constructed with an endpoint; nothing in it
runs at start-up.

## Test strategy

Contract tests against recorded server responses and against the published API description;
round-trip serialization tests for every type; retry and resume tests with a fault-injecting test
server; credential handling tests that assert tokens never appear in logs or errors.

## Capabilities

### Connection and identity

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-HUBAPI-001 | Endpoint configuration | Connects to a Hub by URL, reads its capabilities and API version, and refuses unsupported major versions with a clear error. | T1 | 2 |
| CORE-HUBAPI-002 | OpenID Connect sign-in | Signs in through the provider with the authorization code flow with PKCE in the system browser, or the device flow for headless use. | T1 | 2 |
| CORE-HUBAPI-003 | Token handling | Refreshes tokens before expiry, stores them through a host-provided secure store, and never writes tokens to logs, errors or documents. | T1 | 2 |
| CORE-HUBAPI-004 | Service tokens for jobs | Authenticates headless workers with scoped service tokens supplied by the environment. | T1 | 2 |
| CORE-HUBAPI-005 | Offline behavior | All client calls fail fast with `hub.unreachable` when offline; no local workflow waits on the Hub. | T1 | 2 |

### Files and versions

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-HUBAPI-006 | Projects and folders | Lists, creates and renames projects and folders with permissions visible to the caller. | T1 | 2 |
| CORE-HUBAPI-007 | File versions | Uploads new versions with comment and metadata and downloads any version, verifying content hashes. | T1 | 2 |
| CORE-HUBAPI-008 | Resumable transfers | Uploads and downloads large files in chunks that resume after interruption without resending completed chunks. | T1 | 2 |
| CORE-HUBAPI-009 | Check-out locks | Acquires, releases and inspects file locks; a document opened from a locked file by another user opens read-only. | T1 | 2 |
| CORE-HUBAPI-010 | Lifecycle states | Reads and changes lifecycle states subject to server rules, returning the reason when a transition is refused. | T1 | 2 |
| CORE-HUBAPI-011 | Desktop sync support | Lists changes since a cursor so a client can synchronize a local folder with a project. | T2 | 2 |
| CORE-HUBAPI-012 | Linked document resolution | Resolves document links that carry Hub resource identifiers to local cached files or downloads. | T1 | 2 |

### Issues, reviews and events

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-HUBAPI-013 | Issues | Creates, updates, lists and comments on issues with BCF-compatible viewpoints and object references. | T1 | 2 |
| CORE-HUBAPI-014 | Reviews | Starts reviews, records decisions and reads review status for files and versions. | T2 | 2 |
| CORE-HUBAPI-015 | Event stream | Subscribes over WebSocket to project events (new versions, lock changes, issue updates) with automatic reconnection and resumption from the last event. | T2 | 2 |
| CORE-HUBAPI-016 | Pagination | Iterates all list endpoints with cursor pagination and bounded page sizes. | T1 | 2 |

### Jobs and multi-user editing

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-HUBAPI-017 | Job submission | Submits headless jobs naming product, command or macro, inputs and outputs, and polls or streams status and logs. | T2 | 3 |
| CORE-HUBAPI-018 | Job worker protocol | Lets workers claim jobs, report progress, upload outputs and complete or fail jobs. | T2 | 3 |
| CORE-HUBAPI-019 | Element borrowing | Requests and releases borrowing of elements by `ObjectId` for multi-user editing and reports current holders. | T1 | 5 |
| CORE-HUBAPI-020 | Change set exchange | Uploads change sets and downloads others' change sets since a base with server-assigned order. | T1 | 5 |
| CORE-HUBAPI-021 | API versioning | Sends the client API version; the server's compatibility window is reported and checked at connection. | T1 | 2 |
