# rmcp-mcp — re-check

An independent second pass over the research file of the same name. Where the two disagree, this file is the later and better-checked one.

## Corrections

### `.serve(transport)` still performs the legacy initialize handshake and negotiates at most 2025-11-25.

Status: corrected

`.serve()` is legacy-only, but the 2025-11-25 ceiling is wrong. The default `ClientConfig` has `protocol_version = LATEST` (2026-07-28), so `.serve()` sends `initialize` with protocolVersion 2026-07-28 and accepts a 2026-07-28 answer. My probe: the rmcp 3.9.0 `()` handler with `.serve()` against a real rmcp 2.2.0 stdio server negotiated "2026-07-28", a hybrid session where an initialize-era server runs under no-handshake revision rules. tools/list still worked; the modern-rule side effects (server-initiated requests, list_changed delivery) were not exercised. The researcher's 2025-11-25 result holds only against servers that cap `initialize`, such as rmcp 3.5+. Fix: for any legacy handshake, set `ClientConfig.protocol_version = ProtocolVersion::LATEST_WITH_INITIALIZE` in the handler's `get_info()`. With that pin the same server negotiated 2025-11-25.

Source: rmcp-3.9.0 src/model.rs (InitializeRequestParams::default) and src/service/client.rs (legacy_startup); probe scratchpad/skeptic_rmcp/eraprobe/client/src/bin/hybrid.rs against rmcp =2.2.0

### `ClientLifecycleMode::Auto` probes with server/discover and falls back to initialize on a correlated non-modern error or after a private 10 s timeout; recommended as the way to connect all external servers.

Status: corrected

The mechanics are as described: private `DEFAULT_AUTO_DISCOVER_TIMEOUT` = 10 s, fallback on a correlated non-modern JSON-RPC error or timeout, no fallback on UnsupportedProtocolVersion, no overall startup timeout. But Auto is not a safe universal default. Two failure modes reproduced with rmcp =3.9.0:
- **Server exits on a pre-initialize request.** A real rmcp 2.2.0 stdio server exits with `ExpectedInitializeRequest`; the client returns `ConnectionClosed("discover response")` in about 4 ms with no fallback. The same code path exists in rmcp 0.8.5 and 1.2.0.
- **Server starts reading stdin after more than 10 s** (first-run `npx -y`, `uvx`, `docker run`). The stale discover reply arrives during the fallback handshake and Auto fails with `UncorrelatedErrorResponse { expected: 1, received: 0 }`. A modern or dual-era server would hit `ConflictInitResponseId` by the same logic (from source, not run). Plain Initialize against the same slow server succeeded.
Both cases need the host to respawn the process and retry with `ClientLifecycleMode::Initialize`.

Source: rmcp-3.9.0 src/service/client.rs lines 150-230 and 830-910; rmcp-{0.8.5,1.2.0,2.2.0} src/service/server.rs; probes scratchpad/skeptic_rmcp/eraprobe/client/src/bin/auto_oldsrv.rs and client/src/main.rs with scratchpad/skeptic_rmcp/fake/legacy_server.py

### Not verified: how TypeScript-SDK 1.32.x servers answer a pre-initialize server/discover (immediate -32601 or silence that triggers the 10 s wait). npm: @modelcontextprotocol/sdk 1.32.2, server-filesystem 2.2.0, server-everything 2.2.1, mcp-remote 0.16.5.

Status: corrected

The npm versions are right, but the framing is stale. server-filesystem 2.2.0 and server-everything 2.2.1 depend on `@modelcontextprotocol/server` ^2.3.1 (TS SDK v2, latest 2.4.0), not on `@modelcontextprotocol/sdk` 1.x. v2's `serveStdio` is dual-era by default (`legacy: "serve"`; it answers both server/discover and initialize) and can be set to `legacy: "reject"` for modern-only. Both reference servers declare `engines.node >= 24`. For legacy SDKs, source reading shows an immediate reply with no 10 s wait: TS SDK 1.32.2 returns -32601 for an unknown method before init, and Python mcp 1.30.0 returns -32602 "Invalid request parameters". The Python SDK is at 2.3.0 with 2026-07-28 support. None of these servers were executed.

Source: npm view (dependencies, engines); unpacked @modelcontextprotocol/server@2.4.0 dist/stdio.mjs, server-everything@2.2.1 dist/transports/stdio.js, @modelcontextprotocol/sdk@1.32.2 dist/esm/shared/protocol.js; PyPI mcp 2.3.0 and 1.30.0 wheels (mcp/shared/session.py, mcp/server/session.py)

### `RunningService::call_tool` drives MRTR input_required rounds (cap 10); `Peer::call_tool` turns InputRequiredResult into UnexpectedResponse; recommendation: always use `RunningService::call_tool` and add per-call timeouts via `PeerRequestOptions::with_timeout(..).reset_timeout_on_progress()`.

Status: corrected

The behavioural difference is confirmed. The recommendation cannot be implemented as written. `RunningService::call_tool`, `call_tool_with_mrtr_max_rounds` and `call_tool_once` take only `CallToolRequestParams`, always send with `PeerRequestOptions::no_options()`, and return no `RequestHandle`. So the MRTR helper gives no per-request timeout, no reset-on-progress and no extra `_meta`. `PeerRequestOptions` works only with `send_request_with_option` / `send_cancellable_request`, which do not drive MRTR, and the MRTR helpers (`prepare_input_required_retry`, `fulfill_input_requests`) are private. Either wrap `call_tool` in `tokio::time::timeout` or `select!` (dropping it sends notifications/cancelled), or write your own MRTR loop over `send_cancellable_request`.

Source: rmcp-3.9.0 src/service/client.rs lines 1460-1495, 1720, 2139-2215, 2302; src/service.rs lines 771-880

### Every outgoing request automatically carries a progress token, readable as `RequestHandle::progress_token`, so progress can be matched to a tool call by token via `on_progress`.

Status: corrected

The automatic token is confirmed: `send_request_with_option_and_subscription` always calls `set_progress_token`, overwriting any caller-supplied token. But the token and request id are exposed only through `RequestHandle`. The recommended `RunningService::call_tool` path never returns one, and `ProgressNotificationParam` carries only the token. So per-call progress correlation is possible only with `send_cancellable_request` (no MRTR), or by allowing one in-flight call per connection. The callbacks, `listen(SubscriptionFilter)` and the note that Subscription-routed notifications skip the handler are confirmed.

Source: rmcp-3.9.0 src/service.rs lines 868-900; src/service/client.rs lines 1280-1330; src/handler/client.rs; src/handler/client/progress.rs

### OAuth behind the `auth` feature: OAuthState, then AuthorizationManager, then AuthClient wrapped around the Streamable HTTP transport; CredentialStore for persistence; `is_authorization_required()` / `auth_challenge()` on ClientInitializeError; the example uses `AuthClient::new(reqwest::Client::default(), am)`.

Status: corrected

The type and method names are confirmed (`OAuthState` enum at auth.rs:4104, `AuthorizationRequest::with_*`, `CredentialStore` with `on_refresh_token_rejected`, `AuthClient`, `AuthorizationManager::new_allowing_private_addresses`). The security problem is the copied snippet. `reqwest::Client::default()` bypasses rmcp's same-origin 307/308 redirect policy, which exists only on the SDK's private default client; the regression test is named `default_http_client_does_not_leak_custom_headers_to_redirect_target`. reqwest's own default follows cross-origin redirects and strips only Authorization, Cookie and similar, so custom headers such as X-API-Key and the session id would be replayed. That is the class of GHSA-9g45-5xwm-f3wc. Any caller-supplied client must set `.redirect(Policy::none())` or an equivalent same-origin policy, plus timeouts. Source reading only; no HTTP was exercised.

Source: rmcp-3.9.0 src/transport/auth.rs; src/transport/common/reqwest/streamable_http_client.rs lines 447-530 and 682; rust-sdk docs/OAUTH_SUPPORT.md line 249; https://github.com/modelcontextprotocol/rust-sdk/security/advisories/GHSA-9g45-5xwm-f3wc

### Recommendation: pin `rmcp = "=3.9.0"` (or `~3.9`) and upgrade deliberately.

Status: corrected

3.9.0 is the minimum safe version, not just a convenient pin. The repo has 8 published advisories and RustSec lists 4. Those touching the client path:
- GHSA-c9f9-cp8h-jcj5: unbounded response-body buffering in Streamable HTTP clients; affects 0.2.0 up to but not including 3.9.0; patched in 3.9.0.
- GHSA-5p52-q7pq-9fq2: OAuth metadata SSRF filter; affects 2.0.0 to 3.6.0; patched in 3.7.0.
- GHSA-9g45-5xwm-f3wc / CVE-2026-64684: custom headers leaked on cross-origin redirect; patched in 2.1.0.
Three advisories landed in the three weeks before 2026-11-05, so an exact `=3.9.0` pin would block the next fix. Use `rmcp = "3.9"` with a committed Cargo.lock and cargo-audit or cargo-deny in CI, and treat 3.9.0 as a floor.

Source: https://github.com/modelcontextprotocol/rust-sdk/security/advisories (and the three advisory pages); https://rustsec.org/packages/rmcp.html; rmcp CHANGELOG 3.7.0 and 3.9.0 ("Merge commit from fork")

### TLS build risk: the `reqwest` feature pulls aws-lc-rs / aws-lc-sys, which needs a C toolchain and may need extra build tools on Windows MSVC; `reqwest-native-tls` is simpler to build.

Status: unverifiable

The dependency chain is confirmed (rmcp `reqwest` → reqwest/rustls → aws-lc-rs + rustls-platform-verifier). The actual build requirements on x86_64-pc-windows-msvc and aarch64-apple-darwin could not be checked: this session has only a Linux host, with no Windows or macOS toolchain. Settle it with a CI build on both targets.

Source: rmcp-3.9.0/Cargo.toml; reqwest-0.13.5/Cargo.toml features

## Things the research missed

### rmcp's Auto lifecycle needs a respawn-and-retry wrapper; it is not a drop-in universal connect mode for stdio servers.

Reproduced with rmcp =3.9.0. (1) Servers built on rmcp 2.x and earlier exit when the first request is not `initialize`, so the probe kills the process and Auto returns `ConnectionClosed("discover response")` with no fallback. Run against a real rmcp 2.2.0 server; the same code is in 0.8.5 and 1.2.0. (2) If the child takes more than 10 s to start reading stdin (first-run `npx -y`, `uvx`, `docker run -i`), the late reply to the abandoned probe breaks the fallback handshake with `UncorrelatedErrorResponse` (legacy server) or `ConflictInitResponseId` (dual-era server, from source). The 10 s window is a private constant. The host needs a per-server era state machine: on these error signatures, kill, respawn and connect with `ClientLifecycleMode::Initialize`, then persist the era. The alternative is legacy-first: start with `Initialize` and respawn with `Discover` only when `initialize` is rejected with a JSON-RPC error. That gives up modern-only features, such as extension capabilities, on dual-era servers.

Source: probes: scratchpad/skeptic_rmcp/eraprobe/client/src/bin/auto_oldsrv.rs, client/src/main.rs, scratchpad/skeptic_rmcp/fake/legacy_server.py; rmcp-3.9.0 src/service/client.rs lines 830-910

### A legacy handshake with the default ClientConfig offers 2026-07-28 and can land on it.

`ClientConfig::default().protocol_version` is `LATEST` (2026-07-28). Against an rmcp 2.2.0 server, `.serve()` negotiated "2026-07-28" through `initialize`; rmcp 3.9 then seeds per-request metadata and applies modern-revision rules to an initialize-era server. The host's `ClientHandler::get_info()` should return `protocol_version = ProtocolVersion::LATEST_WITH_INITIALIZE` whenever it connects in `Initialize` mode. Auto's `legacy_version: Some(V_2025_11_25)` already does this for the fallback path only. It should also set its own `client_info` rather than the default `Implementation::from_build_env()`.

Source: probe scratchpad/skeptic_rmcp/eraprobe/client/src/bin/hybrid.rs; rmcp-3.9.0 src/model.rs lines 1604-1613; src/service/client.rs legacy_startup

### rmcp has an active security-advisory stream; this changes the dependency policy.

There are 8 published GHSAs on the rust-sdk repo and 4 RUSTSEC entries. Client-relevant ones were fixed as recently as 3.7.0 (OAuth SSRF) and 3.9.0 (HTTP client memory exhaustion by a malicious server). ARCHITECTURE.md should state a version floor of 3.9.0, a caret requirement with a lockfile, cargo-audit or cargo-deny in CI, and a short patch-release path for the desktop app.

Source: https://github.com/modelcontextprotocol/rust-sdk/security/advisories; https://rustsec.org/packages/rmcp.html

### rmcp's HTTP redirect protection and its (absent) timeouts apply only to its private default client.

`same_origin_redirect_policy()` and `pool_max_idle_per_host(0)` are applied only in the private `default_http_client()`. Any `with_client(reqwest::Client, ..)` path — needed for OAuth via `AuthClient`, custom CA or proxy, or timeouts — gets reqwest's default policy, which follows up to 10 redirects including cross-origin and forwards custom headers. The default client also has no connect or read timeout. The app should own one `reqwest::Client` builder with a none or same-origin redirect policy and a connect timeout, and should not copy `reqwest::Client::default()` from docs/OAUTH_SUPPORT.md. Source reading only.

Source: rmcp-3.9.0 src/transport/common/reqwest/streamable_http_client.rs lines 447-530 and 682; rust-sdk docs/OAUTH_SUPPORT.md

### rmcp passes the host's entire environment to stdio servers; the reference TypeScript client does not.

`TokioChildProcess` sets no environment, so a tokio `Command` inherits everything, including any API keys or tokens in the user's environment, and hands it to third-party `npx`/`uvx` servers. The official TS client spawns with an allowlist (`DEFAULT_INHERITED_ENV_VARS`), `shell: false` and `windowsHide`:
- Windows: APPDATA, HOMEDRIVE, HOMEPATH, LOCALAPPDATA, PATH, PROCESSOR_ARCHITECTURE, SYSTEMDRIVE, SYSTEMROOT, TEMP, USERNAME, USERPROFILE, PROGRAMFILES.
- Unix: HOME, LOGNAME, PATH, SHELL, TERM, USER.
The spawn helper should `env_clear()`, apply this allowlist, add the computed PATH, then add the config's `env`. This replaces the researcher's self-described inferred list. On macOS, import only PATH from the login shell, not the whole shell environment.

Source: @modelcontextprotocol/sdk@1.32.2 dist/esm/client/stdio.js lines 8-42 and 72-73; rmcp-3.9.0 src/transport/child_process.rs

### MCP has an official Skills extension built on the agentskills.io format, and rmcp 3.9.0 implements it.

SEP-2640 is Final; the identifier is `io.modelcontextprotocol/skills`, with methods `skills/list`, `skills/get` and optional `resources/directory/read`, and files served through `resources/read`. rmcp 3.9.0 added `Peer::list_skills`, `get_skill`, `list_all_skills`, `read_resource_directory`, `ServerHandler::list_skills` / `get_skill`, `ServerCapabilities::builder().enable_skills()` and `SkillResource::verify` (#1392, #1412). TS SDK v2 ships `@modelcontextprotocol/server/ext/skills`. Host obligations that shape the skills subsystem:
- Key skills by (server identity, URI), never by name alone.
- Verify size, SHA-256 digest and frontmatter before loading.
- Bind persisted approval to the full manifest.
- Do not prefetch files.
- Keep cached MCP skills out of filesystem-skill discovery paths.
- Support up to 512 files or 16 MiB per skill.
The extension capability is declared via server/discover, which is another reason era handling matters.

Source: https://modelcontextprotocol.io/extensions/skills/overview; https://modelcontextprotocol.io/extensions/client-matrix; rmcp-3.9.0 src/model/skill.rs, src/service/client.rs lines 1536-1626 and 2001-2015, CHANGELOG 3.9.0; @modelcontextprotocol/server@2.4.0 package.json exports

### The MRTR-driving `call_tool` helper and per-call progress, timeouts and metadata are mutually exclusive in rmcp 3.9.0.

`RunningService::call_tool` returns no request id or progress token and accepts no `PeerRequestOptions`. `send_cancellable_request` gives both but does not drive `input_required` rounds, and the MRTR helpers are private. The tool-call layer therefore has three options: (a) its own MRTR loop over `send_cancellable_request`, which fits anyway because elicitation must go through the UI; (b) `call_tool` wrapped in `timeout`/`select!` with at most one in-flight call per connection for progress attribution; (c) no progress correlation. `RunningService` is also not `Clone` and `close` takes `&mut self`, which affects how connections are stored in Tauri state.

Source: rmcp-3.9.0 src/service/client.rs lines 1460-1495, 2139-2215, 2302; src/service.rs lines 541-560 and 852-900 and 1129-1175

### process-wrap's JobObject does not kill the tree when the host crashes unless `KillOnDrop` is also wrapped.

`make_job_object(handle, kill_on_drop)` sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` only when `core.has_wrap::<KillOnDrop>()` is true. Without it, a crashed or force-killed host on Windows leaves cmd.exe and node.exe trees running. The Windows spawn helper should register `CreationFlags(CREATE_NO_WINDOW)`, `KillOnDrop` and `JobObject` together. On Unix there is no equivalent; `ProcessGroup::leader()` helps only when rmcp's shutdown runs, so crash cleanup relies on servers exiting at stdin EOF. Source reading only; not run on Windows.

Source: process-wrap-10.0.1 src/tokio/job_object.rs lines 84-110, src/windows.rs lines 118-160, src/tokio/kill_on_drop.rs

### The server ecosystem has already moved to dual-era SDKs, and the reference servers need a recent Node.

`@modelcontextprotocol/server` and `@modelcontextprotocol/client` 2.4.0 (2026-10-23) are the TS SDK v2 packages; `@modelcontextprotocol/sdk` 1.32.2 is the legacy line and tops out at 2025-11-25. server-filesystem 2.2.0 and server-everything 2.2.1 use v2 and declare `engines.node >= 24`. Python `mcp` is at 2.3.0, with a maintained 1.30 line. The host's test matrix should therefore cover: a TS v2 dual-era server, a TS v1 legacy server, a Python 1.x legacy server, an rmcp 2.x-or-earlier server, and a slow first-run start. Server diagnostics should show the resolved Node version.

Source: npm view (versions, dependencies, engines, time); PyPI JSON for mcp; unpacked package sources under scratchpad/skeptic_rmcp

### MCPB bundles (.mcpb, formerly .dxt) are the existing packaging format for installable local servers.

The researcher correctly notes that `npx -y` and `uvx` are not offline. The ecosystem's answer is MCPB: a bundle with a `manifest.json` and server types including node, python, uv and binary. Claude Desktop ships its own Node runtime to run node bundles. If the app wants offline-capable third-party servers, supporting .mcpb import (and deciding whether to bundle a Node runtime) is a design question the research did not raise. Lower priority than the items above; only the repo landing page was checked, and no release version was visible there.

Source: https://github.com/modelcontextprotocol/mcpb

## Final recommendations

- Depend on `rmcp = "3.9"` (floor 3.9.0, not an exact pin) with features client, server, macros, schemars, transport-child-process, which-command, transport-streamable-http-client-reqwest, auth and one TLS feature. Commit Cargo.lock, run cargo-audit or cargo-deny in CI, and plan for fast patch releases: 3.9.0 is the first version with the HTTP-client memory-exhaustion fix (GHSA-c9f9-cp8h-jcj5).
- Do not rely on `ClientLifecycleMode::Auto` alone for stdio servers. Build a connect wrapper that owns spawn, era choice and retry. On `ConnectionClosed("discover response")`, `UncorrelatedErrorResponse` or `ConflictInitResponseId`, kill the child, respawn, and connect with `Initialize`. Persist the detected era per server config. Wrap every attempt in an app-level startup timeout, since rmcp has none.
- For every legacy handshake, return a `ClientConfig` whose `protocol_version` is `ProtocolVersion::LATEST_WITH_INITIALIZE` and whose `client_info` names the app. The default offers 2026-07-28 in `initialize`, and an rmcp 2.2.0 server accepted it.
- Decide the first-attempt era explicitly in ARCHITECTURE.md. Auto-first keeps modern features (extension capabilities, including Skills over MCP) but costs a failed 10 s attempt on slow first-run starts. Initialize-first is the most compatible today but keeps dual-era servers on 2025-11-25. Either way the respawn path is mandatory.
- Design the tool-call layer around the actual API split. To get per-call progress, reset-on-progress timeouts and a Stop button with a custom reason, issue calls with `send_cancellable_request` and implement the MRTR `input_required` loop in the host, routing elicitation to the UI. Otherwise use `RunningService::call_tool` inside `select!`/`timeout` and allow one in-flight call per connection. Store connections so that `close(&mut self)` remains possible.
- Spawn stdio servers through one helper that does all of the following: resolve the command against the computed PATH (`which`-style, PATHEXT-aware); `env_clear()` and apply the TS-SDK allowlist plus the config's `env`; wrap with `ProcessGroup::leader()` on Unix and `CreationFlags(CREATE_NO_WINDOW)` + `KillOnDrop` + `JobObject` on Windows; pipe and continuously drain stderr into a bounded per-server log; reject or escape arguments containing CR/LF for .cmd/.bat targets.
- Own a single `reqwest::Client` builder for all MCP HTTP and OAuth traffic, with a none or same-origin redirect policy, connect and read timeouts, and the chosen TLS backend. Never pass `reqwest::Client::default()` to `with_client` or `AuthClient::new`. Store OAuth credentials in the OS keychain, keyed by issuer.
- Keep built-in tools as in-process rmcp servers. Prefer the typed `(Sink, Stream)` channel pair over `tokio::io::duplex` for anything that can return large payloads (screenshots, file contents): the 16 MiB line cap drops the message without an error, and the caller hangs unless it has its own timeout.
- Plan the skills subsystem with one registry keyed by (origin, URI) that can hold both filesystem skills (agentskills.io) and MCP-served skills (SEP-2640 via rmcp's `list_skills` / `get_skill` / `SkillResource::verify`). Even if MCP skills ship later, the data model and approval rules (manifest-bound approval, digest verification, no prefetch) should be fixed now.
- Start with empty client capabilities and add form elicitation later. Do not build on Sampling, Roots or MCP Logging (deprecated by SEP-2577). Treat tool annotations, instructions and skill content from external servers as untrusted when deciding approvals.
- Use the Claude Desktop `mcpServers` shape as the native config, extended with `type` (stdio | http | streamable-http), `url` and `headers`, and accept VS Code's `servers` on import. For `type: "sse"`, try Streamable HTTP first and otherwise show an explicit "legacy HTTP+SSE not supported" error, since rmcp ships no SSE client by design.
- Before freezing the architecture, run a real interop matrix on Windows and macOS runners, which this review and the original research could only cover by source reading. Servers: server-everything 2.2.x (TS v2, dual-era, Node 24+), a TS-SDK-1.x server, a Python 1.x server via uvx, an rmcp 2.x-or-earlier server, and a cold-cache first run. Checks: era fallback, no console flash, tree kill on both clean exit and host crash, PATH resolution from a GUI launch, and the aws-lc-rs versus native-tls build.

## Claims confirmed unchanged

- rmcp is at 3.9.0 (published 2026-11-05) with rmcp-macros 3.9.0; MSRV 1.88, edition 2024; near-weekly minors; 2.0.0 on 2026-06-29, 3.0.0 on 2026-07-28. — Confirmed. `cargo search`/`cargo info` show rmcp 3.9.0 and rmcp-macros 3.9.0, rust-version 1.88, Apache-2.0. The crates.io API shows 3.9.0 on 2026-11-05, 3.8.0 on 10-26, 3.7.0 on 10-22, 3.6.0 on 10-20, 3.0.0 on 07-28, 2.0.0 on 06-29 and 1.0.0 on 03-03, none yanked. docs.rs builds 3.9.0. The repo ROADMAP states SEP-1730 Tier 1 and 100% conformance on the 2025-11-25 and 2026-07-28 suites, so the SDK is actively maintained.
- Feature flags: default = [base64, macros, server]; the host needs client, server, macros, schemars, transport-child-process, which-command, transport-streamable-http-client-reqwest, auth, plus one TLS feature, because transport-streamable-http-client-reqwest alone builds reqwest without TLS. — Confirmed from the published Cargo.toml. `__reqwest` is reqwest 0.13.2+ with default-features = false and [json, stream]. The TLS choices are `reqwest` (reqwest/rustls), `reqwest-native-tls` and `reqwest-tls-no-provider`. In reqwest 0.13.5, `rustls` means aws-lc-rs plus rustls-platform-verifier. `which-command` and `local` are missing from the docs.rs feature list; the docs.rs page for `which_command` returns 404. `ClientCapabilities::builder()` is gated on server or macros.
- rmcp 3.9.0 knows 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25 and 2026-07-28; LATEST = 2026-07-28, LATEST_WITH_INITIALIZE = 2025-11-25; the spec's current revision is 2026-07-28, which removed initialize, added mandatory server/discover, replaced server-initiated requests with MRTR, and deprecated Roots, Sampling, Logging, HTTP+SSE and DCR. — Confirmed in src/model.rs and on modelcontextprotocol.io (versioning page and 2026-07-28 changelog). Elicitation is not deprecated. One nit: the changelog attributes the initialize removal and server/discover to SEP-2575 and session removal to SEP-2567. The repo README says rmcp "targets current spec revisions (2025-11-25 and 2026-07-28)".
- Stdio client: `TokioChildProcess::new(cmd)` or `TokioChildProcess::builder(cmd).stderr(Stdio::piped()).spawn()` returning `(TokioChildProcess, Option<ChildStderr>)`; stderr defaults to inherit; graceful shutdown closes stdin, waits 3 s, sends SIGTERM (Unix only), waits 3 s, then kills only the direct child. — Confirmed by reading all of src/transport/child_process.rs. `MAX_WAIT_ON_DROP_SECS` = 3. The SIGTERM stage is new in 3.9.0 (#1379). `ChildWithCleanup::drop` calls `tokio::spawn`, so it must drop inside a runtime. rmcp sets no environment on the child, so the child inherits the host's full environment. My own probe captured stderr through the builder as described. Tree kill depends entirely on the process-wrap wrappers the caller adds.
- process_wrap `ProcessGroup::leader()` (Unix) and `JobObject` plus `CreationFlags(CREATE_NO_WINDOW)` (Windows) give tree kill; CreationFlags is the only way to combine creation flags with JobObject; the app must depend on process-wrap 10 (tokio1). — Confirmed in process-wrap 10.0.1: rmcp depends on version 10.0 with tokio1, and JobObject's pre_spawn overwrites flags unless they come through the `CreationFlags` wrapper. Two details to add. Constructing `PROCESS_CREATION_FLAGS` needs the `windows` crate at 0.62.x. And JobObject sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` only when the `KillOnDrop` wrapper is also registered (see missed items). Not run on Windows.
- Windows: `Command::new("npx")` fails because only .exe is resolved without an extension; use `which_command("npx")` (feature which-command, added 1.4.0) or explicit npx.cmd; it was not confirmed that .cmd gets the same cmd.exe treatment as .bat or what the error kind is. — Confirmed, and the open question is resolved from Rust 1.97.0 std source. `.bat` and `.cmd` are detected identically (case-insensitive) and run as `cmd.exe /e:ON /v:OFF /d /c ""script" args"` with std's batch escaping. The only rejected argument content is CR or LF, which gives `InvalidInput` "batch file arguments are invalid". `which` 8.0.6 skips the extensionless `npx` shell shim on Windows (it must pass `GetBinaryTypeW`) and then tries PATHEXT, so `which_command("npx")` should return npx.cmd. `which_command` was added in 1.4.0 (#774). None of this was run on a Windows host.
- Streamable HTTP client: `from_uri` / `from_config` / `with_client`; config defaults (16 concurrent POSTs, 5 s control and session-recovery timeouts, 16 MiB SSE event, reinit_on_expired_session true, allow_stateless true); default client follows only same-origin 307/308 up to 10 hops; rmcp has no legacy HTTP+SSE client (removed 0.11.0, will not return). — Confirmed in source, the repo README ("Legacy HTTP+SSE transport (2024-11-05) — intentionally not provided … rmcp will not add the legacy transport back") and CHANGELOG 0.11.0 (#562). Additions:
- The built-in clients cap non-streaming JSON bodies at 16 MiB and error bodies at 64 KiB (new in 3.9.0).
- For a discover probe, a sessionless 4xx other than 401/403 is turned into a JSON-RPC error so Auto can fall back over HTTP.
- The default reqwest client sets no connect or request timeout.
- The same-origin redirect policy is a private function applied only to the default client (see missed items).
- Three ways to cancel (drop the future since 3.6.0, `RequestHandle::cancel`, `PeerRequestOptions::with_timeout`); all send notifications/cancelled; no default request timeout. — Confirmed in source: `AbandonedRequestGuard` sends reason "request abandoned by the caller"; `REQUEST_TIMEOUT_REASON` is "request timeout"; CHANGELOG 3.6.0 (#1344). Note that `RequestHandle::cancel(self)` and `await_response(self)` both consume the handle, so a Stop button needs a `select!` around the awaiting future or a separate `notify_cancelled` with the saved id.
- In-process servers work over `tokio::io::duplex` (newline JSON, 16 MiB line cap, oversized lines dropped and logged) or a typed mpsc `(Sink, Stream)` pair; there is no named in-memory transport; non-JSON lines are ignored. — Confirmed. `DEFAULT_MAX_LINE_LENGTH` = 16 MiB, added in 3.6.0 (#1049). An oversized line logs an error and the loop continues, so the caller gets no response. Syntax-invalid lines are ignored at debug level. Well-formed JSON that is not a valid message gets an "Invalid request" reply. The `IntoTransport` impl for `(Si, St)` is compiled unconditionally. `tokio::io::duplex` appears about 100 times in rmcp's own tests.
- Server macros (#[tool_router], #[tool], #[tool_handler]) with the listed attributes and `annotations(read_only_hint, destructive_hint, idempotent_hint, open_world_hint)`; extractors (Parameters, CancellationToken, RequestContext, Peer, …); dynamic tools via hand-written ServerHandler or ToolRouter add/remove/disable/enable_route and bind_peer_notifier; `call_tool` returns `CallToolResponse`. — Confirmed against rmcp-macros 3.9.0 (`ToolAttribute`, `ToolAnnotationsAttribute`, `ToolRouterAttribute`, `ToolHandlerAttribute`) and rmcp 3.9.0 (ToolRouter methods at router/tool.rs 389-600, `FromContextPart` impls, the `ServerHandler::call_tool` signature, and the default `supported_protocol_versions` = KNOWN_VERSIONS).
- Default ClientHandler never hangs: sampling returns -32601, roots returns an empty list, elicitation returns Decline; in 2026-07-28 these arrive as MRTR input requests; Elicitation is not deprecated, while Sampling, Roots and Logging are (SEP-2577). — Confirmed in src/handler/client.rs (`method_not_found` for create_message, `ListRootsResult::default()`, `ElicitationAction::Decline`) and in the spec changelog (Deprecated item 1 and Major change 7). The `elicitation` feature gates only server-side helpers.
- Hard limits: 16 MiB stdio line; 16 MiB SSE event; 16 concurrent POSTs; 5 s control and recovery timeouts; 10 redirects; 10 s Auto probe (not configurable); 3 s per shutdown stage; 10 MRTR rounds; subscription buffer 64; peer queue 1024; cache TTL cap 24 h; no default per-request or initialize timeout. — Each constant was found in source with the stated value. Add the 16 MiB JSON-body and 64 KiB error-body caps in the built-in HTTP clients. My probe confirmed the missing startup timeout: a silent server under Auto was still pending after 14 s (10 s probe, then an unbounded initialize wait).
- macOS: Finder-launched apps get a minimal PATH; Tauri's fix-path-env is git-only (version 0.0.0, not on crates.io), runs `$SHELL -ilc 'echo …; env; …'` and set_vars the result with no timeout. — The fix-path-env facts are confirmed: crates.io reports that the crate does not exist; Cargo.toml on the dev branch says 0.0.0, edition 2021; lib.rs uses `cmd.output()` with no timeout, sets DISABLE_AUTO_UPDATE and calls `std::env::set_var`. It splits the env output on newlines, so multi-line values get mangled. The minimal-GUI-PATH premise could not be exercised here (no macOS host) but matches the crate's stated purpose.
- Claude Desktop config: top-level `mcpServers` mapping name to { command, args, env }, stdio only; file paths per OS; logs mcp.log and mcp-server-SERVERNAME.log; APPDATA env workaround; remote servers are added via Settings > Connectors. — Confirmed word for word on modelcontextprotocol.io: the three file paths, both JSON examples, the log locations, and the `${APPDATA}` ENOENT workaround. The remote-server guide describes only Settings > Connectors > "Add custom connector". The specific statement that Desktop ignores remote entries in the JSON file comes from a support article I did not open.
- Claude Code uses `mcpServers` with type http (alias streamable-http), sse or ws plus url and headers; a url without type is a config error; SSE is deprecated; ${VAR} and ${VAR:-default} expansion; headersHelper; MAX_MCP_OUTPUT_TOKENS 25,000 with a warning at 10,000. VS Code uses `servers`; Cursor uses a bare `url`. — The Claude Code part is confirmed with exact quotes from code.claude.com/docs/en/mcp.md. Extra details there: Claude Code tries HTTP first and switches to SSE (v2.1.265+); `MCP_TOOL_TIMEOUT` defaults to about 28 hours when unset; headersHelper is abandoned after 10 s. The VS Code and Cursor shapes were not re-checked in this review.
