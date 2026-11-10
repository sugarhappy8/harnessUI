# rmcp-mcp — research

## Summary

rmcp is at 3.9.0 (crates.io, released 2026-11-05; MSRV 1.88, edition 2024) and now targets MCP spec 2026-07-28, which removed the initialize handshake and server-to-client requests. Almost everything recalled from the 0.x/1.x API is wrong for this version, so ARCHITECTURE.md should be written against the names below.

How this was checked: I read the rmcp-3.9.0 crate source and the rust-sdk repo at HEAD (2026-11-08), and compiled and ran four probe programs against rmcp =3.9.0 on Linux. Three things were NOT exercised: Windows behaviour, macOS behaviour (no such hosts), and real `npx`/`uvx` servers (running external npm code was blocked by the session permission policy). Findings in those areas rest on source and docs only and are marked medium or low.

The points that most affect the design:
- **Lifecycle:** `.serve(transport)` still performs the legacy `initialize` handshake and negotiates at most 2025-11-25. To also reach modern-only servers the host must use `serve_with_lifecycle(transport, ClientLifecycleMode::Auto { .. })`, with older versions included in `preferred_versions`.
- **In-process servers:** there is no named in-memory transport, but two patterns work without a subprocess: `tokio::io::duplex`, or a typed mpsc `(Sink, Stream)` pair with no serialization. Both ran successfully.
- **Dynamic tools:** a server with a fully dynamic tool list is supported by implementing `ServerHandler::list_tools` / `call_tool` / `get_tool` by hand; `call_tool` now returns `CallToolResponse`.
- **Cancellation and progress:** dropping the call future, `RequestHandle::cancel`, and `PeerRequestOptions::with_timeout` all send `notifications/cancelled`, and the server-side `CancellationToken` fires. Every outgoing request automatically carries a progress token.
- **Sampling, elicitation, roots:** the default client handler never hangs: sampling returns -32601, roots returns an empty list, elicitation returns Decline. On 2026-07-28 these arrive embedded in an `InputRequiredResult`, and only `RunningService::call_tool` (not `Peer::call_tool`) handles that.
- **Process trees:** with a plain `tokio::process::Command`, rmcp shutdown kills only the direct child; a grandchild (the real server under an npx/uvx shim) survived in my test. Wrapping with `process_wrap` `ProcessGroup::leader()` fixed it on Linux; `JobObject` is the Windows analogue (untested).
- **No legacy HTTP+SSE client:** rmcp removed it in 0.11.0 and will not add it back, so config entries with `"type": "sse"` need a custom transport, a bridge, or an explicit "unsupported" error.
- **TLS:** `transport-streamable-http-client-reqwest` alone builds reqwest without TLS; one of `reqwest`, `reqwest-native-tls` or `reqwest-tls-no-provider` must be added.

Probe sources are in <temp workspace> and, under <temp workspace> legacy.rs, chan.rs and tree.rs. The repository working directory was not touched.

## Findings

### Current version is rmcp 3.9.0 (published 2026-11-05) with rmcp-macros 3.9.0; MSRV is Rust 1.88, edition 2024. The crate moves fast, with minor releases roughly weekly.

Confidence: high

`cargo search rmcp` returns rmcp = "3.9.0" and rmcp-macros = "3.9.0"; `cargo info rmcp` shows rust-version 1.88, license Apache-2.0. crates.io history: 2.0.0 on 2026-06-29, 3.0.0 on 2026-07-28, then 3.1 through 3.9 between 2026-07-31 and 2026-11-05. Minor releases have changed behaviour (3.5.0 moved `ProtocolVersion::LATEST` to 2026-07-28; 3.6.0 added cancel-on-drop). Key dependency versions: schemars 1.x (re-exported as `rmcp::schemars`), reqwest 0.13.2+ (not re-exported), process-wrap 10 (not re-exported), oauth2 5, tokio 1, which 8. docs.rs/rmcp/3.9.0 builds, but its feature list omits `which-command` and `local`, so `which_command` may not appear there.

Source: cargo search / cargo info (crates.io); https://crates.io/api/v1/crates/rmcp; <cargo registry>/rmcp-3.9.0/Cargo.toml; https://docs.rs/rmcp/3.9.0/rmcp/

### Feature flags: default is [base64, macros, server]. The host needs client, server, macros, schemars, transport-child-process, which-command, transport-streamable-http-client-reqwest, a TLS feature, and auth.

Confidence: high

What each does:
- `client`: ClientHandler, Peer<RoleClient>, lifecycle modes.
- `server`: implies transport-async-rw, schemars, uuid.
- `macros`: #[tool], #[tool_router], #[tool_handler].
- `schemars`: JSON Schema generation.
- `transport-child-process`: TokioChildProcess (pulls process-wrap).
- `which-command`: `rmcp::transport::which_command`.
- `transport-async-rw`: (AsyncRead, AsyncWrite) transport, used for tokio::io::duplex.
- `transport-streamable-http-client-reqwest`: StreamableHttpClientTransport::from_uri / from_config.
- `auth`: OAuth 2.1 (OAuthState, AuthorizationManager, AuthClient, CredentialStore).
- `elicitation`: gates only server-side helpers (`Peer<RoleServer>::elicit`, `elicit_url`); client-side `ClientHandler::create_elicitation` needs no feature.
- Not needed: transport-io (server-side stdio), transport-streamable-http-server, local, request-state, auth-enterprise-managed.

TLS pitfall: `transport-streamable-http-client-reqwest` and `auth` enable only `__reqwest`, which is reqwest with default-features = false and features [json, stream], so no TLS. Add exactly one of:
- `reqwest`: rustls with the aws-lc-rs provider and rustls-platform-verifier (confirmed in the probe's Cargo.lock: aws-lc-rs 1.19.0, aws-lc-sys 0.47.0).
- `reqwest-native-tls`: SChannel / Secure Transport.
- `reqwest-tls-no-provider`.

`ClientCapabilities::builder()` is gated on `server` or `macros`, not `client`.

Suggested line: rmcp = { version = "3.9", features = ["client","server","macros","schemars","transport-child-process","which-command","transport-streamable-http-client-reqwest","reqwest","auth"] }

Source: <cargo registry>/rmcp-3.9.0/Cargo.toml and README.md; reqwest-0.13.5/Cargo.toml; probe build (cargo 1.97.0)

### The 0.x/1.x API has been renamed: `CallToolRequestParam`, `Content`, `Meta`, `ServerInfo`/`ClientInfo` and struct-literal construction are gone or deprecated.

Confidence: high

Changes that break old code:
- `*RequestParam` aliases removed; use `CallToolRequestParams` and friends.
- `Content` no longer exists; use `ContentBlock` (Text | Image | Audio | Resource | ResourceLink).
- `Meta` is split into `MetaObject`, `RequestMetaObject`, `NotificationMetaObject`.
- `ServerInfo` / `ClientInfo` are deprecated aliases; use `ServerConfig` (= InitializeResult) and `ClientConfig` (= InitializeRequestParams).
- `peer_info()` on a client returns `Option<Arc<ServerPeerInfo>>`, whose `server_info` is `Option<Implementation>`.
- `ServerHandler::call_tool` returns `Result<CallToolResponse, ErrorData>`, where CallToolResponse is Complete(CallToolResult) | InputRequired(InputRequiredResult) | Task(CreateTaskResult); wrap with `.into()`.
- Most model structs and the protocol enums are `#[non_exhaustive]`: construct with `::new(..)` plus `with_*` builders, and add wildcard match arms.
- `TokioChildProcess::split` is removed.
- Tasks moved to an extension; `#[task_handler]` is removed.

Two doc traps: the repo README still shows struct literals such as `ListResourcesResult { resources, next_cursor: None, meta: None }` that lack the newer fields (result_type, ttl_ms, cache_scope); use `ListToolsResult::with_all_items(vec)`. The migration guide (discussion #969) says LATEST is still 2025-11-25, but the 3.9.0 source has `pub const LATEST: Self = Self::V_2026_07_28` (changed in 3.5.0).

Source: rmcp-3.9.0 src/model.rs, src/model/content.rs, src/handler/server.rs, CHANGELOG.md; https://github.com/modelcontextprotocol/rust-sdk/discussions/969

### rmcp 3.9.0 knows protocol revisions 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25 and 2026-07-28. `ProtocolVersion::LATEST` is 2026-07-28, which is also the spec's current revision.

Confidence: high

Constants: `ProtocolVersion::{V_2024_11_05, V_2025_03_26, V_2025_06_18, V_2025_11_25, V_2026_07_28}`, `KNOWN_VERSIONS`, `LATEST = V_2026_07_28`, `LATEST_WITH_INITIALIZE = V_2025_11_25`, `NO_INITIALIZE = V_2026_07_28`, plus `has_initialize()` and `known_up_to(&max)`.

modelcontextprotocol.io/specification/versioning states the current protocol version is 2026-07-28. Its changelog against 2025-11-25:
- Removes `initialize` / `notifications/initialized`; every request carries `_meta` keys `io.modelcontextprotocol/protocolVersion`, `clientCapabilities`, `clientInfo`.
- Adds mandatory `server/discover`.
- Replaces server-initiated `roots/list`, `sampling/createMessage`, `elicitation/create` with Multi Round-Trip Requests (`InputRequiredResult`, `resultType: "input_required"`).
- Replaces the GET stream and `resources/subscribe` with `subscriptions/listen`.
- Removes `ping`, `logging/setLevel`, `Mcp-Session-Id`, and SSE resumability.
- Requires `resultType`, `ttlMs`, `cacheScope` on results.
- Deprecates Roots, Sampling, Logging (SEP-2577), HTTP+SSE, and OAuth Dynamic Client Registration.
- Moves tasks into the extension `io.modelcontextprotocol/tasks`.

Source: rmcp-3.9.0 src/model.rs lines 154-270; https://modelcontextprotocol.io/specification/versioning; https://modelcontextprotocol.io/specification/2026-07-28/changelog

### `.serve(transport)` is legacy-only. A host that must reach both legacy and modern servers needs `ClientLifecycleMode::Auto`, and `preferred_versions` must list older versions too.

Confidence: high

API: `use rmcp::{ClientServiceExt, ClientLifecycleMode}; handler.serve_with_lifecycle(transport, mode)`. Modes: `Initialize` (what `ServiceExt::serve` and `serve_with_ct` do), `Discover { preferred_versions }` (no fallback), `Auto { preferred_versions, legacy_version: Option<ProtocolVersion> }`.

Auto sends `server/discover` and falls back to `initialize` on the same connection when it gets a correlated non-modern JSON-RPC error or no reply within `DEFAULT_AUTO_DISCOVER_TIMEOUT` = 10 s. That constant is private and not configurable.

Observed in-process:
- `serve()` with default ClientConfig against an rmcp server negotiated 2025-11-25.
- `Auto { preferred_versions: [V_2026_07_28] }` negotiated 2026-07-28.
- `Auto` with only [V_2026_07_28] against a server whose `supported_protocol_versions()` is capped at 2025-11-25 FAILED with `ClientInitializeError::NoCompatibleProtocolVersion`. The server answered discover with UnsupportedProtocolVersion, a modern error, so there is no fallback.
- With [V_2026_07_28, V_2025_11_25] the same server negotiated 2025-11-25.

The spec's compatibility matrix says a legacy client against a modern-only server fails, and that clients SHOULD cache a server's era for the life of the process.

`serve()` has no startup timeout: a silent server blocks forever unless you pass a CancellationToken via `serve_with_ct` or wrap it in `tokio::time::timeout`.

`ClientInitializeError` variants include ConnectionClosed(String), TransportError{error, context}, JsonRpcError(ErrorData), NoCompatibleProtocolVersion{..}, Cancelled, LegacyFallbackFailed{discover, fallback}. Helpers: `is_authorization_required()`, `auth_challenge()`.

Source: rmcp-3.9.0 src/service/client.rs lines 702-1100; probe src/bin/legacy.rs output; https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning (Backward Compatibility); https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio

### Stdio client: `TokioChildProcess::new(cmd)` or `TokioChildProcess::builder(cmd).stderr(Stdio::piped()).spawn()`, which returns `(TokioChildProcess, Option<ChildStderr>)`.

Confidence: high

Imports: `rmcp::transport::{TokioChildProcess, ConfigureCommandExt}` (also `which_command` with the feature). `new(command: impl Into<process_wrap::tokio::CommandWrap>)` accepts a `tokio::process::Command` directly.

README pattern: `().serve(TokioChildProcess::new(Command::new("npx").configure(|cmd| { cmd.arg("-y").arg("@modelcontextprotocol/server-everything"); }))?).await?`

Builder defaults: stdin piped, stdout piped, stderr `Stdio::inherit()`. In a GUI app inherited stderr is lost, so pipe it. The builder has `.stdin()`, `.stdout()`, `.stderr()`, `.spawn()`. Other methods: `id() -> Option<u32>`, `graceful_shutdown()`, `into_inner()`.

stderr capture ran successfully: reading the returned `ChildStderr` with `BufReader::lines()` in a spawned task yielded the child's output. The handle must be drained continuously or a chatty server will block on a full pipe (general pipe behaviour; not measured).

The spec says stderr is for any logging and clients should not treat it as an error signal. Claude Desktop writes each server's stderr to `mcp-server-SERVERNAME.log`.

Source: rmcp-3.9.0 src/transport/child_process.rs; probe src/main.rs section 5 output; https://modelcontextprotocol.io/docs/develop/connect-local-servers; stdio spec page

### Spawn failures surface in two distinct places: a synchronous `std::io::Error` from spawn, or `ClientInitializeError::ConnectionClosed` from `serve()` when the process starts and then exits.

Confidence: high

Observed on Linux:
- Missing binary: `TokioChildProcess::new(Command::new("definitely-not-a-real-binary-xyz"))` returned Err with kind NotFound, "No such file or directory (os error 2)".
- `which_command("missing")` returned Err with kind NotFound, "cannot find binary path".
- Child that prints to stderr and exits 1 (simulating an npx 404): `().serve(transport).await` returned `ConnectionClosed("initialize response")`, displayed as "connection closed: initialize response". The only useful diagnostic ("npm ERR! 404 not found") was on the captured stderr.

So the per-server log view must attach stderr before calling serve and show its tail alongside the init error. rmcp does not expose the child's exit status on init failure; if you need the exit code, hold the pid or wrap the process yourself.

Not exercised: a real `npx -y <missing-package>` run (blocked by policy). On Windows, issue #456 reports "The system cannot find the file specified. (os error 2)" for `Command::new("npx")`.

Source: probe src/main.rs sections 4-5 output; rmcp-3.9.0 src/service/client.rs (expect_next_message); https://github.com/modelcontextprotocol/rust-sdk/issues/456

### Streamable HTTP client: `StreamableHttpClientTransport::from_uri(url)`, `from_config(config)`, or `with_client(reqwest::Client, config)`. rmcp has no legacy HTTP+SSE client.

Confidence: high

`StreamableHttpClientTransportConfig::with_uri(url)` has builder methods:
- `.auth_header(token)`: the bearer token WITHOUT the "Bearer " prefix.
- `.custom_headers(HashMap<http::HeaderName, http::HeaderValue>)`.
- `.max_concurrent_requests(n)`: default 16.
- `.control_request_timeout(d)`: default 5 s.
- `.session_recovery_timeout(d)`: default 5 s.
- `.max_sse_event_size(bytes)`: default 16 MiB.
- `.reinit_on_expired_session(bool)`: default true.
- `allow_stateless` defaults to true.

The default reqwest client sets `pool_max_idle_per_host(0)` and follows only same-origin 307/308 redirects, up to 10 hops; 301/302/303 or cross-origin redirects are errors.

Error types: `StreamableHttpError`, `AuthRequiredError { www_authenticate_header }`, `InsufficientScopeError`, `HttpStatusError { status, retry_after }`.

reqwest and http are not re-exported, so the app must depend on reqwest 0.13 and http 1 itself for `with_client` and custom headers.

The README section "Legacy HTTP+SSE transport (2024-11-05) — intentionally not provided" says rmcp "will not add the legacy transport back"; it was removed in 0.11.0 (2025-12-08).

No HTTP connection was exercised in my probes; this is from source and README.

Source: rmcp-3.9.0 src/transport/streamable_http_client.rs lines 2016-2247; src/transport/common/reqwest/streamable_http_client.rs lines 425-500; repo README.md (Transports); CHANGELOG.md 0.11.0

### Client calls: `list_tools(Option<PaginatedRequestParams>)`, `list_all_tools()`, `call_tool(CallToolRequestParams)`. `RunningService::call_tool` and `Peer::call_tool` behave differently.

Confidence: high

`CallToolRequestParams` is non_exhaustive with fields `meta: Option<RequestMetaObject>`, `name: Cow<'static, str>`, `arguments: Option<JsonObject>`, `input_responses`, `request_state`. Build it as `CallToolRequestParams::new("name").with_arguments(json.as_object().unwrap().clone())`.

`CallToolResult` fields: `result_type: Option<ResultType>`, `content: Vec<ContentBlock>`, `structured_content: Option<Value>`, `is_error: Option<bool>`, `meta`.

The difference:
- `RunningService<RoleClient, H>::call_tool` drives MRTR `input_required` rounds through the local ClientHandler, up to `DEFAULT_MRTR_MAX_ROUNDS` = 10; `call_tool_with_mrtr_max_rounds` sets the cap.
- `Peer<RoleClient>::call_tool` (what you get from a cloned `client.peer()`) does not: an InputRequiredResult becomes `ServiceError::UnexpectedResponse`.
- `call_tool_once` returns `CallToolResponse::{Complete, InputRequired, Task}`.

`list_tools` responses are cached only when the server sends a positive `ttlMs`, and are invalidated by list_changed. The default `serve_stale_on_error` returns the last cached list as Ok on a failed refetch; tune with `peer.set_response_cache_config(ClientCacheConfig::disabled())`.

`ServiceError` variants: McpError(ErrorData), TransportSend, TransportClosed, UnexpectedResponse, Cancelled{reason}, Timeout{timeout}, InputRequiredRoundsExceeded{max_rounds}, SubscriptionLagged{capacity}.

list_all_tools and call_tool ran end to end in the probes.

Source: rmcp-3.9.0 src/service/client.rs lines 1440-2330, src/model.rs lines 4139-4470, src/service/client/cache.rs; probe src/main.rs output

### There are three ways to cancel an in-flight request, and all of them send `notifications/cancelled`. There is no default request timeout.

Confidence: high

1. Drop the call future (for example a `tokio::time::timeout` or a losing `select!` branch). Since 3.6.0 an `AbandonedRequestGuard` sends a best-effort cancelled notification with reason "request abandoned by the caller".
2. Explicit handle: `let h = client.peer().send_cancellable_request(ClientRequest::CallToolRequest(CallToolRequest::new(params)), PeerRequestOptions::no_options()).await?;` then `h.cancel(Some("user pressed stop".into())).await?` or `h.await_response().await`. `RequestHandle` has public fields `id`, `progress_token`, `rx`, `peer`, `options`. A request sent this way does not get MRTR driving.
3. `PeerRequestOptions::with_timeout(d)`, optionally `.reset_timeout_on_progress()` and `.with_max_total_timeout(d)`. On expiry it sends cancelled with reason "request timeout" and returns `ServiceError::Timeout`.

All three ran in-process, and in each case the server tool's `CancellationToken` fired within one 100 ms tick.

`notify_cancelled(CancelledNotificationParam::new(Some(id), Some(reason)))` also exists. The connection-wide token is `client.cancellation_token()`, which closes the whole service, not one request.

Source: rmcp-3.9.0 src/service.rs lines 541-690 and 771-860, src/service/client.rs (AbandonedRequestGuard, send_request_cancel_on_drop); CHANGELOG 3.6.0 (#1344); probe src/main.rs output

### Progress, logging and list_changed arrive through `ClientHandler` callbacks, and rmcp attaches a progress token to every outgoing request automatically.

Confidence: high

Callbacks: `on_progress(ProgressNotificationParam, NotificationContext<RoleClient>)`, `on_logging_message(LoggingMessageNotificationParam, ..)` (deprecated by SEP-2577 but functional), `on_tool_list_changed(..)`, `on_prompt_list_changed`, `on_resource_list_changed`, `on_resource_updated`, `on_cancelled`, `on_task_status`, `on_custom_notification`.

`ProgressNotificationParam` has `progress_token`, `progress: f64`, `total: Option<f64>`, `message: Option<String>`.

In the probe every request got a numeric progressToken in `_meta`, readable as `RequestHandle::progress_token`, so progress can be matched to a tool call by token. The helper `rmcp::handler::client::progress::ProgressDispatcher` offers `subscribe(token) -> ProgressSubscriber` (a Stream, channel size 16) and `handle_notification`.

Both lifecycles ran: `on_tool_list_changed` fired after the server called `context.peer.notify_tool_list_changed()`, and a following `list_all_tools()` returned the new tool.

A 2026-07-28 caveat from the spec and rmcp docs: compliant modern servers send list-changed only on a `subscriptions/listen` stream. Open one with `client.listen(SubscriptionFilter::builder().tools_list_changed().build())`, which returns a `Subscription` with `next()`, `cancel()`, `end()` (buffer 64). Notifications routed to a Subscription are NOT also delivered to the ClientHandler callbacks.

Logging in 2026-07-28 is opt-in per request via the `_meta` key `io.modelcontextprotocol/logLevel` (`RequestMetaObject::set_log_level`); `set_level` (logging/setLevel) is legacy-only and deprecated.

Source: rmcp-3.9.0 src/handler/client.rs, src/handler/client/progress.rs, src/service.rs lines 860-880, src/service/client.rs (listen); probe src/main.rs output; spec changelog items 4-5

### Graceful shutdown is `client.close().await`, `close_with_timeout(d)` or `cancel().await`. For a child process rmcp closes stdin, waits 3 s, sends SIGTERM (Unix only), waits 3 s, then kills, but only the direct child.

Confidence: high

`RunningService` methods: `close(&mut self) -> Result<QuitReason, JoinError>`, `close_with_timeout(Duration) -> Result<Option<QuitReason>, JoinError>`, `cancel(self)`, `waiting(self)`, `is_closed()`, `cancellation_token()`. QuitReason is Cancelled | Closed | JoinError. Dropping a RunningService cancels asynchronously via a DropGuard. `ChildWithCleanup::drop` calls `tokio::spawn` to kill, so it must be dropped inside a tokio runtime.

`TokioChildProcess::graceful_shutdown()` uses `MAX_WAIT_ON_DROP_SECS = 3` per stage, so the worst case is about 6 s per server on Unix and about 3 s on Windows, where there is no SIGTERM stage.

Process-tree test on Linux, using `sh -c 'sleep N & wait'` as a stand-in for an npx/uvx shim:
- Plain `tokio::process::Command`: shutdown took 3.5 s and the grandchild was still alive afterwards.
- `let mut w = process_wrap::tokio::CommandWrap::from(cmd); w.wrap(process_wrap::tokio::ProcessGroup::leader()); TokioChildProcess::new(w)`: the grandchild was killed.

On Windows the equivalent wrappers are `process_wrap::tokio::JobObject` and `CreationFlags`; that is from source only. The app must add `process-wrap = { version = "10", features = ["tokio1"] }` itself, matching rmcp's major version.

Source: rmcp-3.9.0 src/service.rs lines 1064-1200, src/transport/child_process.rs; process-wrap-10.0.1 src/tokio/{job_object,creation_flags,process_group}.rs; probe src/bin/tree.rs output

### An in-process server needs no subprocess. There is no named in-memory transport type, but `tokio::io::duplex` and a typed channel pair both work.

Confidence: high

Pattern A, a byte stream with newline-delimited JSON (used by more than 30 rmcp tests):
```rust
let (server_io, client_io) = tokio::io::duplex(64 * 1024);
tokio::spawn(async move { let svc = Builtin.serve(server_io).await.expect("init"); let _ = svc.waiting().await; });
let client = host_handler.serve(client_io).await?;   // RunningService<RoleClient, _>
let tools = client.list_all_tools().await?;
```
It needs `transport-async-rw`, which `server` implies. Incoming lines are capped at `DEFAULT_MAX_LINE_LENGTH` = 16 MiB; an oversized message is dropped and logged, not an error, and the caller then waits forever for its response. That matters for base64 screenshots. The duplex buffer size only affects backpressure.

Pattern B, typed channels with no serialization, via the always-compiled `(Sink, Stream)` IntoTransport impl:
```rust
let (c2s_tx, c2s_rx) = tokio::sync::mpsc::channel::<ClientJsonRpcMessage>(64);
let (s2c_tx, s2c_rx) = tokio::sync::mpsc::channel::<ServerJsonRpcMessage>(64);
let server_t = (tokio_util::sync::PollSender::new(s2c_tx), tokio_stream::wrappers::ReceiverStream::new(c2s_rx));
let client_t = (tokio_util::sync::PollSender::new(c2s_tx), tokio_stream::wrappers::ReceiverStream::new(s2c_rx));
```
Both sides then call `.serve(..)` as usual.

Both patterns ran initialize, tools/list and tools/call successfully. `serve_directly(service, transport, peer_info)` exists to skip the handshake. The README table lists "Worker / in-process | transport-worker", but that is the `Worker` trait abstraction, not a ready-made in-memory pipe.

Source: rust-sdk crates/rmcp/tests/test_tool_macros.rs lines 314-330 and test_notification.rs; rmcp-3.9.0 src/transport/sink_stream.rs, src/transport/async_rw.rs lines 50-120; probe src/main.rs and src/bin/chan.rs output

### Server macros: `#[tool_router]` on the impl block, `#[tool(...)]` on each function, `#[tool_handler(...)] impl ServerHandler for T {}`. Annotation syntax is `annotations(read_only_hint = true, ...)`.

Confidence: high

`#[tool]` attributes: `name` (defaults to the fn name), `title`, `description` (defaults to the doc comment), `input_schema`, `output_schema`, `annotations(title = "..", read_only_hint = bool, destructive_hint = bool, idempotent_hint = bool, open_world_hint = bool)`, `icons`, `meta`, `local`.

`#[tool_router]` attributes: `router = ident`, `vis = "pub"`, `server_handler` (emits the #[tool_handler] impl for tools-only servers), `allow_empty`. A router with no #[tool] fn is otherwise a compile error, and #[tool] fns generated by a macro_rules! inside the impl are invisible to it.

`#[tool_handler]` attributes: `router = expr` (default `Self::tool_router()`, so no router field is required), `meta`, `name`, `version`, `instructions`. It generates call_tool, list_tools, get_tool, and get_info if absent.

Parameters: `Parameters<T>` from `rmcp::handler::server::wrapper::Parameters`, with T: Deserialize + schemars::JsonSchema. The schema comes from T's fields. `Option<T>` renders as type ["T", "null"] in JSON Schema 2020-12.

Return types (anything implementing IntoCallToolResult): String, ContentBlock, `Json<T>` (also generates outputSchema), CallToolResult, CallToolResponse, InputRequiredResult, Result<T, E>.

The probe's wire output for an annotated tool was `{"title":"Echo","readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}`.

The programmatic form is `Tool::new(name, desc, Arc<JsonObject>).with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false))`. In ToolAnnotations an unset destructiveHint defaults to true (`is_destructive()`), and an unset idempotentHint to false.

Routers compose with `+`; `ToolRouter::with_sync_tool::<T>()` / `with_async_tool::<T>()` support one tool per file via the `ToolBase` / `SyncTool` / `AsyncTool` traits.

Source: rust-sdk crates/rmcp-macros/src/lib.rs and src/tool.rs; rmcp-3.9.0 src/model/tool.rs, src/handler/server/router/tool.rs; probe src/main.rs output

### A tool function gets cancellation and progress by adding extractor arguments: `tokio_util::sync::CancellationToken` and `RequestContext<RoleServer>`.

Confidence: high

Extractors available on #[tool] fns (FromContextPart impls): `Parameters<P>`, `JsonObject`, `RequestContext<RoleServer>`, `tokio_util::sync::CancellationToken`, `Peer<RoleServer>`, `RequestMetaObject`, `Extensions`, `Extension<T>`, `RequestId`, `ToolName`, `RequestState`, `InputResponses`.

`RequestContext` fields: `ct` (cancelled when the client's notifications/cancelled arrives), `id`, `meta`, `extensions`, `peer`.

Pattern that ran:
```rust
async fn slow(&self, Parameters(a): Parameters<Args>, ct: CancellationToken, ctx: RequestContext<RoleServer>) -> Result<CallToolResult, McpError> {
  let token = ctx.meta.get_progress_token();
  if let Some(t) = token.clone() { ctx.peer.notify_progress(ProgressNotificationParam::new(t, i as f64).with_total(n).with_message("..")).await.ok(); }
  tokio::select! { _ = ct.cancelled() => return Ok(CallToolResult::error(vec![ContentBlock::text("cancelled")])), _ = work => {} }
}
```
rmcp does not abort the handler on cancel; the handler must observe `ct` itself. The spec says a server must not send further messages for a cancelled request.

Source: rmcp-3.9.0 src/handler/server/common.rs lines 168-250, src/handler/server/tool.rs, src/service.rs lines 1210-1235; rust-sdk examples/servers/src/common/progress_demo.rs; probe src/main.rs output

### A fully dynamic tool list is supported in two ways: implement `ServerHandler::{get_info, list_tools, call_tool, get_tool}` by hand, or mutate a `ToolRouter` at runtime.

Confidence: high

Hand-written signatures:
```rust
async fn list_tools(&self, _r: Option<PaginatedRequestParams>, _c: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError>  // Ok(ListToolsResult::with_all_items(vec_of_Tool))
async fn call_tool(&self, request: CallToolRequestParams, context: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError>  // Ok(CallToolResult::success(..).into())
fn get_tool(&self, name: &str) -> Option<Tool>
fn get_info(&self) -> ServerConfig  // ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_tool_list_changed().build()).with_server_info(Implementation::new("name","ver"))
```
This ran: a server holding `Arc<Mutex<Vec<Tool>>>` added a tool at runtime, called `context.peer.notify_tool_list_changed().await`, the client's `on_tool_list_changed` fired, and the re-list showed the new tool, in both lifecycles.

Router-based: `ToolRouter::{new, add_route(ToolRoute), remove_route(name), disable_route(name), enable_route(name), has_route, list_all, get, call(ToolCallContext), merge, set_notifier, bind_peer_notifier(&peer)}`. A route is built with `ToolRoute::new_dyn(tool: impl Into<Tool>, |ctx: ToolCallContext<'_, S>| -> BoxFuture<Result<CallToolResponse, ErrorData>>)`. `disable_route` / `enable_route` call the notifier; `bind_peer_notifier` makes it send tools/list_changed.

In the default router, an argument deserialization failure is converted to an in-band `CallToolResult::error` ("failed to deserialize parameters: ..."), and an unknown tool is `invalid_params("tool not found")`.

Tool names are checked by `validate_and_warn_tool_name`, which warns and does not reject.

Source: rmcp-3.9.0 src/handler/server.rs lines 560-620, src/handler/server/router/tool.rs lines 184-600, src/model.rs (paginated_result!); probe src/main.rs Dynamic server output

### Results are built with `CallToolResult::{success, error, structured, structured_error}` from `ContentBlock::{text, image, audio, resource, resource_link}`. Tool failures should be `Ok(CallToolResult::error(..))`, not `Err`.

Confidence: high

- `ContentBlock::image(base64_data, "image/png")` serialized in the probe as `{"type":"image","data":"..","mimeType":"image/png"}`.
- `CallToolResult::structured(json)` sets `structuredContent` and also a text block holding the JSON string; the probe's output was `{"content":[{"type":"text","text":"{...}"}],"structuredContent":{..},"isError":false}`.
- Returning `Json<T>` from a #[tool] fn generates `outputSchema` as well.
- From 2026-07-28 `structuredContent` may be any JSON value and `outputSchema` any schema type (SEP-2106).
- `resultType: "complete"` is emitted only to 2026-07-28 peers and stripped for legacy peers; the probe showed both forms.
- `Err(McpError::invalid_params(..) / internal_error(..))` is a JSON-RPC protocol error; the rmcp docs say clients typically render those opaquely, so the model would not see the message.
- `CallToolResult::into_typed::<T>()` deserializes structured_content, or else the first text block.

Source: rmcp-3.9.0 src/model.rs lines 4139-4400, src/model/content.rs; repo README.md (Tools); probe src/main.rs output

### Sampling, elicitation and roots do not hang when the host does not implement them. In 2026-07-28 they arrive as MRTR input requests rather than server-initiated requests.

Confidence: high

`ClientHandler` defaults (also `impl ClientHandler for ()` and `for ClientConfig`):
- `create_message` returns Err(method_not_found), i.e. -32601 "sampling/createMessage".
- `list_roots` returns Ok(ListRootsResult::default()), an empty list.
- `create_elicitation` returns Ok(ElicitResult { action: Decline, .. }).
- `ping` returns Ok.

All three were observed in a legacy session with a server that sends real requests.

`ClientCapabilities::default()` advertises none of them, so compliant servers should not ask.

In a 2026-07-28 session, rmcp servers are blocked from sending server-to-client requests at all; the probe saw -32600 "server-to-client requests are not allowed on protocol 2026-07-28 or later; return InputRequiredResult instead". The rmcp client also rejects restricted server requests that are not associated with an in-flight client request (SEP-2260). The server instead returns `InputRequiredResult { input_requests, request_state }`, and `RunningService::call_tool` fulfils each `InputRequest::{CreateMessage, Elicitation, ListRoots}` through the same ClientHandler methods and retries. A handler Err, such as the sampling default, fails the whole call_tool with `ServiceError::McpError`.

To support elicitation, override `create_elicitation(ElicitRequestParams, RequestContext<RoleClient>)`, matching `FormElicitationParams { message, requested_schema, .. }` and `UrlElicitationParams { url, .. }`, and advertise it via `ClientCapabilities::builder().enable_elicitation().build()`.

Elicitation is NOT deprecated. Sampling, Roots and Logging are (SEP-2577, removal no sooner than 12 months).

Source: rmcp-3.9.0 src/handler/client.rs, src/service/client.rs lines 339-375 and 2330-2420, src/service/server.rs lines 55-70; probe src/bin/legacy.rs output; spec changelog (MRTR, Deprecated)

### Hard limits and built-in timeouts in rmcp 3.9.0.

Confidence: high

- stdio / AsyncRw incoming line: `DEFAULT_MAX_LINE_LENGTH` = 16 MiB; oversized lines are dropped and logged. Override requires building `AsyncRwTransport::new(r, w).with_max_line_length(n)` yourself; TokioChildProcess does not expose it.
- Lines that are not valid JSON are ignored at debug level, so a server printing banners to stdout does not kill the connection. Well-formed JSON that is not valid JSON-RPC gets an "Invalid request" error reply.
- SSE event: `DEFAULT_MAX_SSE_EVENT_SIZE` = 16 MiB.
- HTTP: 16 concurrent POSTs (`max_concurrent_requests`), `control_request_timeout` 5 s, `session_recovery_timeout` 5 s, at most 10 same-origin 307/308 redirects.
- Auto-lifecycle discover probe: 10 s, not configurable.
- Child shutdown: 3 s per stage (`MAX_WAIT_ON_DROP_SECS`).
- MRTR: `DEFAULT_MRTR_MAX_ROUNDS` = 10.
- Subscription buffer: `DEFAULT_SUBSCRIPTION_CHANNEL_CAPACITY` = 64.
- Peer outbound queue: 1024 messages.
- ProgressDispatcher channel: 16.
- Client cache: `MAX_CLIENT_CACHE_TTL` = 24 h.
- There is no default per-request timeout and no initialize timeout.

Source: rmcp-3.9.0 src/transport/async_rw.rs, src/transport/common/client_side_sse.rs line 18, src/transport/streamable_http_client.rs, src/service/client.rs line 723, src/transport/child_process.rs line 10, src/model/mrtr.rs line 53, src/service.rs, src/service/client/cache.rs

### Windows (not tested): `Command::new("npx")` fails because Rust resolves only `.exe` without an extension. Use `which_command("npx")` or an explicit `npx.cmd`, set CREATE_NO_WINDOW, and use a Job Object for tree kill.

Confidence: medium

Rust std docs for Command::new on Windows: "Rust resolves the executable path before spawning... searched in order: 1. The child's PATH, if explicitly set via env. 2. The directory of the current executable. 3. The system directory. 4. The Windows directory. 5. The parent process's PATH... For executable files, the .exe extension may be omitted. Files with other extensions must include the extension, otherwise they will not be found." npx, npm and pnpm are `.cmd` shims; uvx and uv are real `.exe` files.

rmcp ships `rmcp::transport::which_command(name) -> io::Result<tokio::process::Command>` (feature `which-command`, added in 1.4.0 via PR #774, closing issue #456). It resolves through the `which` crate; its doc says `.cmd` shims such as npx.cmd "are not reliably found by tokio::process::Command without a fully-qualified path". It searches the current process PATH, not a PATH you set on the child env.

Spawning a `.bat` goes through `cmd.exe /c` implicitly. The std docs say arguments are escaped by cmd.exe rules and that some special characters make spawn return an error; they name only `.bat`. I did not confirm that `.cmd` gets identical treatment in the current std, nor the exact error kind. An explicit `cmd /c npx ...` wrapper is the alternative, and Claude Desktop-style configs sometimes already contain `"command": "cmd", "args": ["/c", "npx", ...]`.

Console flash: a GUI-subsystem parent spawning a console child shows a window unless CREATE_NO_WINDOW (0x08000000, `PROCESS_CREATION_FLAGS(134217728)`) is set. Use `tokio::process::Command::creation_flags(0x0800_0000)`, or when combined with a job object, `process_wrap::tokio::CreationFlags(CREATE_NO_WINDOW)` with `.wrap(JobObject)`. process-wrap documents that CreationFlags is the only way to combine flags with JobObject.

rmcp's graceful_shutdown has no SIGTERM stage on Windows: stdin close, 3 s, then kill. Without JobObject, kill hits only cmd.exe and leaves node.exe behind (inferred from my Linux tree test).

The Claude Desktop docs note an ENOENT `${APPDATA}` failure fixed by passing `"env": {"APPDATA": "C:\\Users\\user\\AppData\\Roaming\\"}`. If you use env_clear, preserve APPDATA, LOCALAPPDATA, USERPROFILE, SystemRoot, PATH and PATHEXT (that list is my inference).

Source: https://doc.rust-lang.org/std/process/struct.Command.html; https://doc.rust-lang.org/std/process/index.html#windows-argument-splitting; rmcp-3.9.0 src/transport/child_process.rs (which_command); https://github.com/modelcontextprotocol/rust-sdk/issues/456; process-wrap-10.0.1 src/tokio/creation_flags.rs; tokio-1.54.1 src/process/mod.rs line 675; windows-0.62.2 CREATE_NO_WINDOW; https://modelcontextprotocol.io/docs/develop/connect-local-servers

### macOS (not tested): apps launched from Finder get a minimal launchd PATH, so npx/uvx/node installed via Homebrew, nvm, volta or ~/.local/bin are not found unless the app resolves the login-shell PATH itself.

Confidence: medium

Tauri's `fix-path-env` crate (tauri-apps/fix-path-env-rs) is git-only: its Cargo.toml says version 0.0.0 and `cargo search` finds nothing on crates.io, so pin a git rev. `fix_path_env::fix()` runs `$SHELL -ilc 'echo -n "_SHELL_ENV_DELIMITER_"; env; echo -n "_SHELL_ENV_DELIMITER_"; exit'` (default /bin/zsh on macOS), with DISABLE_AUTO_UPDATE=true and cwd = home, then `std::env::set_var`s the result.

Caveats:
- An interactive login shell can be slow or hang on heavy dotfiles; run it once at startup with a timeout, off the main thread.
- It mutates the process environment globally. `set_var` is unsafe in edition 2024 and racy once threads exist, so call it before starting the tokio runtime, or compute the PATH and pass it per child via `Command::env("PATH", ..)`.
- `rmcp::transport::which_command` and the `which` crate search the current process PATH. If you only set PATH on the child env, resolve the binary yourself with `which::which_in(name, Some(path), cwd)`.

Suggested design: compute the shell PATH once; append well-known directories (/opt/homebrew/bin, /usr/local/bin, ~/.local/bin, ~/.cargo/bin, ~/.volta/bin, the nvm current bin); resolve the command to an absolute path; pass that PATH in the child env so `#!/usr/bin/env node` shebangs inside npx work; and let users override the command path per server. The minimal-PATH premise is corroborated by a KDE merge request found in search results, and the directory list is my suggestion. I did not fetch an Apple primary source.

Source: https://raw.githubusercontent.com/tauri-apps/fix-path-env-rs/dev/README.md and src/lib.rs and Cargo.toml; cargo search fix-path-env; https://invent.kde.org/utilities/kate/-/merge_requests/2143

### Claude Desktop config: the top-level key is `mcpServers`, mapping a name to `{ command, args, env }`, stdio only. Remote servers are added through Settings > Connectors, not this file.

Confidence: medium

File locations: macOS `~/Library/Application Support/Claude/claude_desktop_config.json`; Windows `%APPDATA%\Claude\claude_desktop_config.json`; Linux `~/.config/Claude/claude_desktop_config.json`.

Example from the official docs, identical on macOS and Windows apart from paths:
```json
{ "mcpServers": { "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/username/Desktop"] } } }
```
and with env: `"env": { "APPDATA": "C:\\Users\\user\\AppData\\Roaming\\", "BRAVE_API_KEY": "..." }`.

There is no `type` field; `args` and `env` are optional; paths must be absolute.

Logs: `~/Library/Logs/Claude/mcp.log` and `mcp-server-SERVERNAME.log` (the server's stderr); on Windows `%APPDATA%\Claude\logs`.

The official remote-server guide describes only Settings > Connectors > "Add custom connector" with a URL and OAuth. A search summary of Anthropic's support article says Desktop will not connect to remote servers configured in the JSON file; I did not open that article. The community workaround is a stdio bridge: `"command": "npx", "args": ["mcp-remote", "https://..."]` (mcp-remote is 0.16.5 on npm).

Current npm versions: @modelcontextprotocol/server-filesystem 2.2.0, server-everything 2.2.1, @modelcontextprotocol/sdk 1.32.2.

Source: https://modelcontextprotocol.io/docs/develop/connect-local-servers; https://modelcontextprotocol.io/docs/develop/connect-remote-servers; https://support.anthropic.com/en/articles/11503834-building-custom-connectors-via-remote-mcp-servers (via search summary); npm view

### Other hosts express remote servers differently. Claude Code uses `mcpServers` with `"type": "http"` (alias `"streamable-http"`), `"sse"` or `"ws"` plus `url` and `headers`. VS Code uses a top-level `servers` key. Cursor uses `mcpServers` with a bare `url`.

Confidence: medium

Claude Code (`.mcp.json`, `~/.claude.json`, `claude mcp add-json`):
- stdio: `{ "command": "npx", "args": [..], "env": {..} }`. An entry with no `type` is read as stdio.
- Remote: `{ "type": "http", "url": "https://mcp.stripe.com", "headers": { "Authorization": "Bearer ${API_KEY}" } }`.
- Docs text: "the `type` field accepts `streamable-http` as an alias for `http`"; "A JSON entry that has a `url` but no `type` is a configuration error"; "The SSE (Server-Sent Events) transport is deprecated."
- Extras: `${VAR}` and `${VAR:-default}` expansion in command, args, env, url, headers; `headersHelper` (a shell command printing a JSON header object); an `oauth` object { clientId, callbackPort, authServerMetadataUrl, scopes }; per-server `timeout` in ms; env vars MCP_TIMEOUT (startup), MCP_TOOL_TIMEOUT, MAX_MCP_OUTPUT_TOKENS (default 25,000 tokens, warning at 10,000). These extras came through a summarizing fetch, so treat the exact numbers as medium confidence.

VS Code (`.vscode/mcp.json`): `{ "servers": { "name": { "type": "stdio", "command", "args", "env", "envFile" } | { "type": "http" | "sse", "url", "headers" } }, "inputs": [..] }`. It tries HTTP first and falls back to SSE.

Cursor (`.cursor/mcp.json`, `~/.cursor/mcp.json`): `{ "mcpServers": { "name": { "command", "args", "env", "envFile" } | { "url", "headers", "auth": { "CLIENT_ID", "CLIENT_SECRET", "scopes" } } } }`. Remote entries have no `type`.

Implication for our importer: accept `mcpServers` and `servers`. Treat an entry with `command` as stdio. Treat an entry with `url` as Streamable HTTP when `type` is absent or one of `http`, `streamable-http`, `streamableHttp`. Flag `type: "sse"` as legacy, since rmcp has no SSE client. The `streamableHttp` spelling is not from these docs; I added it defensively.

Source: https://code.claude.com/docs/en/mcp.md (lines 81-95 quoted); https://code.visualstudio.com/docs/copilot/reference/mcp-configuration; https://cursor.com/docs/context/mcp

### OAuth for remote servers is built in behind the `auth` feature. The flow is `OAuthState` then `AuthorizationManager` then `AuthClient`, wrapped around the Streamable HTTP transport. rmcp never opens a browser or runs a callback listener.

Confidence: medium

Flow from docs/OAUTH_SUPPORT.md:
```rust
let mut st = OAuthState::new(&server_url, None /* or Some(reqwest::Client) */).await?;
st.start_authorization(AuthorizationRequest::new(REDIRECT_URI).with_client_name("My MCP Client")).await?;   // optional: .with_scopes([..]) .with_client_metadata_url(..) .with_preregistered_client(id).with_client_secret(..)
let url = st.get_authorization_url().await?;      // app opens the browser and runs a loopback listener
st.handle_callback(code, csrf_state).await?;
let am = st.into_authorization_manager().unwrap();
let transport = StreamableHttpClientTransport::with_client(AuthClient::new(reqwest::Client::default(), am), StreamableHttpClientTransportConfig::with_uri(url));
```
It covers PKCE S256, RFC 9728 and RFC 8414 discovery, Dynamic Client Registration (now deprecated by the spec in favour of Client ID Metadata Documents), CIMD, automatic refresh, and scope upgrade via `request_scope_upgrade`.

Persist tokens by implementing `CredentialStore` (`load`, `save(StoredCredentials)`, `clear`, optional `on_refresh_token_rejected`); the built-in store is `InMemoryCredentialStore`. Use the OS keychain rather than plain files. The spec says clients MUST key persisted credentials by issuer.

To detect that auth is needed, check `err.is_authorization_required()` or `err.auth_challenge()` on the `ClientInitializeError` from serve(). The SDK does not start the flow automatically.

This was not exercised in a probe; it is from docs and source.

Source: rust-sdk docs/OAUTH_SUPPORT.md; rmcp-3.9.0 src/transport/auth.rs (CredentialStore at line 349, AuthClient at 573, OAuthState at 4104), src/service/client.rs lines 100-150; spec changelog (SEP-2352, DCR deprecation)

### Not verified: behaviour against real `npx`/`uvx` MCP servers, and everything specific to Windows or macOS.

Confidence: high

A probe that launched `npx -y @modelcontextprotocol/server-everything` was blocked by the session permission policy (downloading and running external npm code), and I did not work around it. So I have no measurement of:
- how TypeScript-SDK 1.32.x servers answer a pre-initialize `server/discover` (an immediate -32601, or silence that triggers the 10 s wait);
- first-run `npx -y` download latency;
- real stderr chatter volume.

No Windows or macOS host was available, so .cmd resolution, console-window flashes, JobObject tree kill and Finder PATH are source-and-docs findings only. No HTTP or OAuth connection was made either.

Everything else marked high confidence was compiled and run on Linux x86_64 (cargo 1.97.0) against rmcp =3.9.0, all in-process or with local `sh` / `sleep` only.

Source: Session tool denial; probe sources under <temp workspace>

## Recommendations

- Pin `rmcp = "=3.9.0"` (or `~3.9`) with features client, server, macros, schemars, transport-child-process, which-command, transport-streamable-http-client-reqwest, auth, plus exactly one TLS feature. Commit Cargo.lock and upgrade deliberately: minor releases have changed behaviour. Set the workspace rust-version to at least 1.88 and add direct dependencies on process-wrap 10 (tokio1), reqwest 0.13, http 1 and tokio-util.
- Choose the TLS feature by trying both in CI on windows-msvc and aarch64-apple-darwin: `reqwest` (rustls with aws-lc-rs, which needs a C toolchain) or `reqwest-native-tls` (SChannel / Secure Transport, simpler to build). Do not ship with only `transport-streamable-http-client-reqwest`, because https URLs will then fail at runtime.
- Connect external servers with `serve_with_lifecycle(transport, ClientLifecycleMode::Auto { preferred_versions: vec![V_2026_07_28, V_2025_11_25, V_2025_06_18, V_2025_03_26, V_2024_11_05], legacy_version: Some(V_2025_11_25) })`, wrapped in a startup timeout (for example 30-60 s, user-configurable) via `serve_with_ct` or `tokio::time::timeout`. Persist the detected era per server config and use `ClientLifecycleMode::Initialize` directly for servers known to be legacy, to avoid the 10 s probe wait. Before finalizing, measure Auto against real TypeScript-SDK and Python-SDK servers on a developer machine; I could not run them here.
- For built-in tools, run in-process servers over `tokio::io::duplex(64 * 1024)` (the pattern rmcp's own tests use), or over typed mpsc channels if base64 screenshots could approach the 16 MiB line cap or serialization cost matters. Keep built-ins behind the same `RunningService<RoleClient, H>` abstraction as external servers so one code path handles list, call, cancel and progress.
- Implement built-in servers whose tool set changes at runtime (skills, per-workspace toggles) with a hand-written `ServerHandler` or a `ToolRouter` using `disable_route` / `enable_route` and `bind_peer_notifier`. Use the #[tool_router] / #[tool] macros for static tool groups (files, git, terminal) with explicit `annotations(read_only_hint, destructive_hint, idempotent_hint, open_world_hint)`. Treat annotations from external servers as untrusted hints when deciding approval prompts.
- Always call tools through `RunningService::call_tool` (or `call_tool_once` with your own loop), not a cloned `Peer::call_tool`, so that 2026-07-28 `InputRequiredResult` rounds are handled. For user Stop, either drop the call future or keep a `RequestHandle` from `send_cancellable_request` and call `.cancel(reason)`. Add your own per-call timeout via `PeerRequestOptions::with_timeout(..).reset_timeout_on_progress().with_max_total_timeout(..)`, since rmcp has none by default.
- Write one host `ClientHandler` per connection that forwards `on_progress` (keyed by progress token), `on_logging_message` and `on_tool_list_changed` to the UI and tool registry. In modern sessions also open `client.listen(SubscriptionFilter::builder().tools_list_changed().build())` and re-list when it yields. Start with empty `ClientCapabilities` (no sampling, roots or elicitation) and add form elicitation later by overriding `create_elicitation`. Do not build on Sampling, Roots or MCP Logging; they are deprecated by SEP-2577.
- Spawn stdio servers through one helper that: resolves the command to an absolute path (`which_command` or `which::which_in` against the computed PATH); builds a `process_wrap::tokio::CommandWrap` with `ProcessGroup::leader()` on Unix and `JobObject` plus `CreationFlags(CREATE_NO_WINDOW)` on Windows; uses `TokioChildProcess::builder(..).stderr(Stdio::piped()).spawn()`; drains stderr into a bounded per-server ring buffer and log file; and shows the stderr tail next to any `ClientInitializeError`. On app exit call `close_with_timeout` on every client, budgeting about 6 s per server on Unix and 3 s on Windows.
- On macOS, compute the login-shell PATH once at startup (the fix-path-env approach, pinned to a git rev, or your own `$SHELL -ilc env` with a timeout), merge in well-known tool directories, and pass it explicitly as the child's PATH instead of mutating the global environment after threads start. Let users set an absolute command path per server and show the resolved path in the UI.
- Use the Claude Desktop `mcpServers` shape as the native config format, extended with `type` (`stdio` | `http` | `streamable-http`), `url` and `headers`, and accept VS Code's `servers` key on import. For `"type": "sse"` entries, first try the URL as Streamable HTTP; if that fails, show a clear "legacy HTTP+SSE not supported, use mcp-remote as a stdio bridge" message rather than failing silently.
- Add CI smoke tests on Windows and macOS runners that spawn `npx -y @modelcontextprotocol/server-everything` and a `uvx` server through the real spawn helper. Check for no console flash, clean tree kill, PATH resolution from a GUI launch, and Auto-lifecycle fallback timing. These are exactly the areas this research could not exercise.

## Risks

- API churn: rmcp shipped three major versions in five months (1.0 in March 2026, 2.0 in June, 3.0 in July) and near-weekly minors that alter behaviour. Examples, blog posts and model-recalled code for 0.x/1.x will not compile. The repo README and migration guide are themselves partly stale (struct literals for non_exhaustive types; the LATEST value).
- Protocol transition: the ecosystem is mid-migration from the initialize-based revisions to stateless 2026-07-28. A legacy-only client (`serve()`) will fail against future modern-only servers. A modern probe can add up to 10 s of startup per silent legacy stdio server, and may confuse servers that mishandle pre-initialize requests. This was not tested against real servers.
- Orphaned processes: with a plain tokio Command, rmcp kills only the direct child. npx (cmd.exe then node) and uvx (then python) trees will leak processes on shutdown or crash unless `process_wrap` ProcessGroup or JobObject is used. Reproduced on Linux; the Windows JobObject path is untested.
- Dropped large messages: a single JSON-RPC line or SSE event over 16 MiB is dropped and logged, not surfaced as an error, and the caller then waits forever unless it has its own timeout. Large base64 screenshots or file contents from the computer-use, browser or files tools could hit this over the duplex or stdio transports.
- No built-in timeouts on initialize or tool calls: a hung server, or `npx -y` blocked on a first-run download or prompt, stalls the caller indefinitely unless the app adds timeouts and cancellation.
- Windows spawn fragility (untested): `.cmd` shims need explicit resolution; batch-file argument escaping can reject some special characters at spawn; a missing CREATE_NO_WINDOW flashes consoles; stripping the environment breaks npx (APPDATA).
- macOS PATH (untested): GUI launches will not find node, npx or uvx without login-shell PATH resolution. Running an interactive login shell at startup can be slow or hang on unusual dotfiles, and mutating the process environment after threads start is unsafe in edition 2024.
- Legacy HTTP+SSE servers are unsupported by rmcp by design. Users importing configs with `"type": "sse"` will hit a gap unless a custom Transport or the mcp-remote bridge is offered; the bridge needs Node, which matters for a fully-offline app.
- The offline claim versus MCP reality: `npx -y` and `uvx` fetch packages from the network on first run, and sometimes on later runs. External stdio servers are therefore not offline unless pre-installed. Remote HTTP and OAuth servers are inherently online. Built-in in-process servers are the only fully offline path.
- TLS build risk: the default `reqwest` feature pulls aws-lc-rs / aws-lc-sys (seen in the lockfile), which needs a C toolchain and may need extra build tools on Windows MSVC. I did not confirm the exact Windows requirements; validate in CI or use `reqwest-native-tls`.
- Deprecated MCP features (Sampling, Roots, Logging, DCR) still work but emit Rust deprecation warnings (`#[deprecated]` on `on_logging_message` types, `set_level`, `create_message`). They are scheduled for removal no sooner than 12 months out, so code built on them needs `#[allow(deprecated)]` and has a limited lifetime.
- Tool annotations and server instructions from external MCP servers are untrusted. Using readOnlyHint or destructiveHint from third-party servers to skip user approval is a security risk; rmcp's own docs warn that clients should never make tool-use decisions based on annotations from untrusted servers.
