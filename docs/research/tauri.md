# tauri — research

## Summary

Verified on 2026-11-10 against crates.io, npm, and Tauri source at tag tauri-v2.12.5, plugins-workspace v2, tauri-docs v2 and tauri-action v1. Nothing was built or run; the capability JSON, CSP and CI workflow below are my compositions from verified identifiers and are untested.

**Things that differ from older Tauri 2 knowledge**
- Stable is tauri 2.12.5 (MSRV 1.90). A 3.0.0-alpha.6 line exists and is what `cargo search` shows, so pin 2.x explicitly.
- `macOSPrivateApi` is no longer needed for transparent windows (no-op since 2.12.1).
- tauri-action is now `@v1` (v1.0.0), and `includeUpdaterJson` was renamed to `uploadUpdaterJson`.
- Azure Trusted Signing is now "Azure Artifact Signing"; the CLI is `artifact-signing-cli` 0.12.0.
- App commands are callable from every local window unless `tauri_build::AppManifest::commands` is declared in build.rs.

**Decisions the evidence supports**
- **Sidecar on Windows:** ship llama-server variants through `bundle.resources` into per-variant subfolders, because `externalBin` flattens everything into one directory.
- **Sidecar on macOS:** use `externalBin` (and `bundle.macOS.frameworks` for dylibs). The bundler does not sign anything under `Contents/Resources`, so unsigned Mach-O files there will fail notarization.
- **No shell plugin:** spawn from Rust with `tokio::process::Command`; the frontend then needs no `shell:*` permission.
- **Updater:** the plugin makes no network request until `check()` is called, so opt-in is purely app logic.
- **Installer:** NSIS `currentUser` mode avoids UAC on install; keep optional large backends (CUDA) out of the installer because of the roughly 2 GB NSIS ceiling.

**Linux dev build in this container:** feasible but not ready. The requested `apt-get install -s` fails only because the apt lists are empty; a simulation against a private scratch cache resolves cleanly (147 packages, 62 MB).

**Not directly verifiable:** the NSIS wiki page on the 2 GB limit returned 403 (confidence medium, backed by NSIS's own changelog). The GitHub REST API was unavailable, so I used raw files and `git ls-remote`. No files were created in the repository.

## Findings

### Current stable core versions: tauri 2.12.5, tauri-build 2.7.4, tauri-cli 2.12.4, @tauri-apps/cli 2.12.4, @tauri-apps/api 2.12.4. A 3.0.0-alpha line is being published in parallel.

Confidence: high

crates.io max_stable: tauri 2.12.5 (2026-10-21, rust-version 1.90, edition 2024), tauri-build 2.7.4, tauri-cli 2.12.4, tauri-bundler 2.10.4, tauri-utils 2.10.2, wry 0.57.1, tao 0.37.2. npm dist-tags: @tauri-apps/cli latest 2.12.4 / next 3.0.0-alpha.6; @tauri-apps/api latest 2.12.4 / next 3.0.0-alpha.3. tauri 3.0.0-alpha.6 was published 2026-11-06 with MSRV 1.95. `cargo search tauri` prints the alpha, so do not copy versions from it; write `tauri = "2.12"`. Changelog 2.12.0: MSRV raised to 1.90, `windows` crate 0.62 / webview2-com 0.39 "drops Windows 7 support", security fix GHSA-w28w-mhc8-qvjv (channel data readable across webviews). Default cargo features: wry, compression, common-controls-v6, dynamic-acl, x11, dbus. Optional features relevant here: tray-icon, devtools, protocol-asset, isolation, image-png.

Source: https://crates.io/api/v1/crates/tauri ; `npm view @tauri-apps/cli dist-tags` ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/CHANGELOG.md ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/Cargo.toml

### Current stable official plugin versions (crate / npm), all MSRV 1.90.

Confidence: high

shell 2.6.0 / 2.6.0; global-shortcut 2.4.1 / 2.4.1; updater 2.14.1 / 2.14.1; dialog 2.8.4 / 2.8.4; single-instance 2.5.5 / no npm package (404); process 2.4.0 / 2.4.0; opener 2.7.2 / 2.7.2; window-state 2.5.0 / 2.5.0; os 2.4.0 / 2.4.0; clipboard-manager 2.5.0 / 2.5.0; deep-link 2.6.3 / 2.6.3. Also seen: fs 2.7.1, store 2.5.0, log 2.10.1. Several plugins also have 3.0.0-alpha.2/3 pre-releases on crates.io. single-instance has cargo features `semver` and `deep-link`, and its README says it must be the first plugin registered.

Source: https://crates.io/api/v1/crates/<name> ; `npm view @tauri-apps/plugin-<name> version` ; https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/single-instance/README.md

### Exact permission identifiers and default sets per plugin.

Confidence: high

Every `allow-x` has a matching `deny-x`.
- shell: allow-execute, allow-kill, allow-open, allow-spawn, allow-stdin-write. default = [allow-open].
- global-shortcut: allow-is-registered, allow-register, allow-register-all, allow-unregister, allow-unregister-all. default = [] (nothing).
- updater: allow-check, allow-download, allow-install, allow-download-and-install. default = all four.
- dialog: allow-ask, allow-confirm, allow-message, allow-open, allow-save. default = [allow-message, allow-save, allow-open].
- process: allow-exit, allow-restart. default = both.
- opener: allow-open-url, allow-open-path, allow-reveal-item-in-dir, allow-default-urls (mailto:, tel:, http(s)://). default = [allow-open-url, allow-reveal-item-in-dir, allow-default-urls]. Scopes use `{"url": glob}` / `{"path": glob}`.
- window-state: allow-filename, allow-restore-state, allow-save-window-state. default = all three.
- os: allow-arch, allow-exe-extension, allow-family, allow-hostname, allow-locale, allow-os-type, allow-platform, allow-version. default = all except hostname.
- clipboard-manager: allow-clear, allow-read-image, allow-read-text, allow-write-html, allow-write-image, allow-write-secret, allow-write-text. default = [].
- deep-link: allow-get-current, allow-is-registered, allow-register, allow-unregister. default = [allow-get-current].
- single-instance: no permissions directory and no JS API (Rust only).

Source: https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins/<name>/permissions (default.toml and autogenerated/reference.md, fetched raw)

### ACL semantics: plugin commands always need a capability; app commands are open to all local windows unless an AppManifest is declared; remote origins are always blocked without a `remote` capability.

Confidence: high

webview/mod.rs: the ACL check runs when `plugin_command.is_some() || has_app_acl_manifest || !is_local`. Without `tauri_build::AppManifest::new().commands(&[...])` in build.rs, every `#[tauri::command]` is callable from every local window, including an overlay window. With it, `allow-<command>` / `deny-<command>` permissions are generated and must be granted per capability. The Channel fetch command `plugin:__TAURI_CHANNEL__|fetch` is exempt. `core:window:default` contains only getters; NOT default are create, show/hide/close, set_always_on_top, set_ignore_cursor_events, set_focus, start_dragging and others. `core:webview:default` excludes create_webview_window. `core:app:default` excludes exit (`core:app:allow-exit` is new in 2.12.0). `core:event:default` = listen, unlisten, emit, emit_to. Capability fields: identifier, description, local (default true), remote.urls, windows (label globs), webviews, permissions, platforms. Windows matching several capabilities get the union. The boundary is the window label.

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/webview/mod.rs (L2044-2107) ; .../crates/tauri/build.rs ; .../crates/tauri-build/src/acl.rs ; .../crates/tauri-utils/src/acl/capability.rs ; https://v2.tauri.app/security/capabilities/

### Proposed minimal capabilities (my composition from verified identifiers; not compiled).

Confidence: medium

Only plugins the webview calls need permissions. global-shortcut, single-instance, window-state, deep-link and os can be driven entirely from Rust with zero permissions; the shell plugin can be left out.

src-tauri/capabilities/main.json:
{"$schema":"../gen/schemas/desktop-schema.json","identifier":"main","windows":["main"],"permissions":["core:default","dialog:allow-open","dialog:allow-save","opener:allow-open-url","opener:allow-default-urls","opener:allow-reveal-item-in-dir","clipboard-manager:allow-write-text"]}

Add `core:window:allow-start-dragging` etc. only for a custom titlebar. Add `updater:allow-check` + `updater:allow-download-and-install` and `process:allow-restart` only if the update flow runs in JS; otherwise do it in a Rust command (`app.updater()?.check()`, `app.restart()`) and grant nothing.

src-tauri/capabilities/overlay.json:
{"identifier":"overlay","windows":["overlay"],"permissions":["core:event:allow-listen","core:event:allow-unlisten"]}

If AppManifest::commands is used, add the generated `allow-<command>` entries to main.json only. List capabilities explicitly in `app.security.capabilities` so a stray file in capabilities/ is not auto-enabled.

Source: Derived from the plugin permission files and https://v2.tauri.app/security/capabilities/ ; https://v2.tauri.app/learn/security/capabilities-for-windows-and-platforms/

### externalBin: file must be named `<name>-<target-triple>[.exe]`; all sidecars are flattened into the main executable's directory.

Confidence: high

Example: `"externalBin": ["binaries/llama-server"]` needs `src-tauri/binaries/llama-server-x86_64-pc-windows-msvc.exe` and `llama-server-aarch64-apple-darwin`. Get the triple with `rustc --print host-tuple` (Rust 1.84+). The bundler copies only `file_name()` with `-<triple>` stripped: next to the app exe on Windows, into `Contents/MacOS/` on macOS, and into `target/<profile>/` during dev (tauri-build build script). Consequences: (1) two entries with the same file name in different folders collide, and all sidecars share one DLL directory, so Vulkan/CPU/CUDA builds whose DLLs have identical names cannot coexist via externalBin; (2) a sidecar may not have the same name as the Cargo package; (3) the file must exist at `cargo build`/`cargo check` time.

Source: https://v2.tauri.app/develop/sidecar/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-bundler/src/bundle/settings.rs (copy_binaries) ; .../crates/tauri-build/src/lib.rs (copy_binaries)

### bundle.resources can ship several variants plus their DLLs in separate subfolders; `$RESOURCE` resolves per OS.

Confidence: high

Use the map form to control targets, e.g. `"resources": {"llama/win-vulkan/": "llama/vulkan/", "llama/win-cpu/": "llama/cpu/"}`. A `dir/` source copies recursively and keeps structure; a glob source with a map target flattens. In list form `..` becomes `_up_` and an absolute root becomes `_root_`. `resource_dir()` returns the exe directory on Windows (always) and when running from a cargo `target/` dir in dev; `Contents/Resources` on macOS; `/usr/lib/<name>` (or `$APPDIR/usr/lib/<name>`) on Linux. tauri-build copies resources into `target/<profile>/` in dev, so resolution is the same in dev and production. Platform files `tauri.windows.conf.json` / `tauri.macos.conf.json` / `tauri.linux.conf.json` are merged with JSON Merge Patch: objects merge key by key, arrays are replaced wholesale. Put platform-specific resources/externalBin there so a Linux dev build does not require Windows binaries.

Source: https://v2.tauri.app/develop/resources/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-utils/src/platform.rs (resource_dir_from) ; https://v2.tauri.app/develop/configuration-files/

### Spawning from Rust with std/tokio process needs no plugin and no capability; the shell plugin does nothing special beyond path resolution and CREATE_NO_WINDOW.

Confidence: high

Capabilities only gate IPC from the webview. Sketch: `let exe = app.path().resolve("llama/vulkan/llama-server.exe", tauri::path::BaseDirectory::Resource)?;` then `tokio::process::Command::new(&exe).current_dir(exe.parent().unwrap()).args([...]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true)`, plus `creation_flags(0x0800_0000)` (CREATE_NO_WINDOW) on Windows. For an externalBin sidecar the path is `current_exe()?.parent().join("llama-server")`; that is exactly what tauri-plugin-shell's `relative_command_path` does (it appends `.exe` on Windows and sets CREATE_NO_WINDOW). Windows loads DLLs from the exe's own folder first, so per-variant folders isolate DLL sets. shell 2.6.0 added `Command::kill_on_drop` and 2.5.0 added `processGroup` (job object / process group); if you skip the plugin you must supply equivalent tree-kill yourself (e.g. crate process-wrap 10.0.1).

Source: https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/shell/src/process/mod.rs ; https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/shell/CHANGELOG.md ; https://v2.tauri.app/develop/resources/

### macOS: the bundler signs frameworks/dylibs listed in bundle.macOS.frameworks, externalBin sidecars and the main binary, then the .app. It does NOT sign anything copied via bundle.resources or bundle.macOS.files.

Confidence: high

macos/app.rs builds `sign_paths` from: `copy_frameworks_to_bundle` (entries ending in `.framework` or `.dylib` go to `Contents/Frameworks/`), `settings.copy_binaries` (externalBin to `Contents/MacOS/`), the app binaries, then the bundle itself. `copy_resources` and `copy_custom_files_to_bundle` add nothing to the list. codesign is invoked per target as `codesign --force -s <identity> [--options runtime] [--keychain ...] [--entitlements <file>] <path>`: no `--deep`, hardened runtime only for executables when `hardenedRuntime` (default true), and the same entitlements file for every target. Apple's bundle rules: helper tool in `Contents/MacOS/` or `Contents/Helpers/`; framework or dynamic library in `Contents/Frameworks/`; "incorrectly placed code might work during day-to-day development, but might cause problems during notarization". Notary errors include "The binary is not signed with a valid Developer ID certificate.", "The signature does not include a secure timestamp.", "The executable does not have the hardened runtime enabled." So Mach-O files under Resources/ must be pre-signed by you before `tauri build`, or moved to externalBin/frameworks. Dylibs moved to Contents/Frameworks need an rpath such as `@executable_path/../Frameworks` on the sidecar (Tauri's own doc comment says so).

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-bundler/src/bundle/macos/app.rs ; .../crates/tauri-macos-sign/src/keychain.rs ; https://developer.apple.com/documentation/bundleresources/placing-content-in-a-bundle ; https://developer.apple.com/documentation/security/resolving-common-notarization-issues

### Windows: when signing is configured, sidecars and any .exe/.dll in resources are signed automatically.

Confidence: high

Signing is active only if `bundle.windows.signCommand` or `certificateThumbprint` is set (`can_sign`). The bundler signs non-main binaries, externalBin sidecars (skipped if already signed, or if `TAURI_SKIP_SIDECAR_SIGNATURE_CHECK=true`), every resource whose extension is `exe` or `dll` and is not already signed (`should_sign`), the NSIS plugin DLLs, the uninstaller and the final `-setup.exe`. `bundle.windows.bundleVCRuntime: true` (bundler 2.10.0) copies the Visual C++ runtime DLLs into the installer, which is useful if llama.cpp binaries are not statically linked against the VC runtime.

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-bundler/src/bundle.rs (sign_binaries_if_needed) ; .../bundle/windows/nsis/mod.rs (generate_resource_data) ; .../bundle/windows/sign.rs ; .../crates/tauri-bundler/CHANGELOG.md

### CSP is only active when `app.security.csp` is set, and on desktop it is only applied to assets served by Tauri's protocol (i.e. production builds, not the Vite dev server).

Confidence: medium

At build time Tauri adds sha256 hashes of bundled JS and nonces for `<style>` elements and remote `<script src=http...>`, then appends them (and `'self'`) to script-src / style-src. The header is sent on Windows/macOS and injected as a meta tag on Linux. The dev-server proxy that would apply it exists only for `cfg(all(dev, mobile))`, so test CSP with `tauri build --debug`. The official example allows `connect-src ipc: http://ipc.localhost` and `img-src 'self' asset: http://asset.localhost blob: data:`. Origins: Windows uses `http://<scheme>.localhost` (https if `useHttpsScheme`), macOS/Linux use `<scheme>://localhost`. Changing `useHttpsScheme` between releases loses IndexedDB/localStorage.

Proposed CSP for a no-remote-content app (my composition): {"default-src":"'self'","script-src":"'self'","style-src":"'self' 'unsafe-inline'","img-src":"'self' data: blob: asset: http://asset.localhost","font-src":"'self' data:","media-src":"'self' blob: asset: http://asset.localhost","connect-src":"'self' ipc: http://ipc.localhost","worker-src":"'self' blob:","object-src":"'none'","base-uri":"'self'","form-action":"'none'","frame-src":"'none'"}.

Add `'wasm-unsafe-eval'` to script-src only if the frontend uses WASM. `asset:` needs cargo feature `protocol-asset` plus `app.security.assetProtocol {enable: true, scope: [...]}`. Tailwind v4 compiles to a static CSS file covered by `'self'`; `'unsafe-inline'` in style-src is only for libraries that inject `<style>` tags or inline style attributes.

Source: https://v2.tauri.app/security/csp/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/manager/mod.rs (set_csp, replace_csp_nonce) ; .../crates/tauri/src/protocol/tauri.rs ; .../examples/api/src-tauri/tauri.conf.json ; .../crates/tauri-utils/src/config.rs

### `freezePrototype` and `dangerousDisableAssetCspModification` semantics.

Confidence: high

`app.security.freezePrototype` (default false) runs `Object.freeze(Object.prototype)` as an init script on every webview before app code; the config docs warn that libraries extending built-in prototypes stop working, so enable it early and test. `dangerousDisableAssetCspModification` accepts `true`/`false` or a list of directive names (e.g. `["style-src"]`) that Tauri must not modify. Tauri only appends style nonces if the built HTML contains `<style>` elements; if it does, the added nonce makes browsers ignore `'unsafe-inline'`, and listing `style-src` here is the fix. Leave script-src modification enabled. `withGlobalTauri` defaults to false. `pattern` defaults to `brownfield`; `isolation` routes IPC through a sandboxed iframe app and needs the `isolation` cargo feature.

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-utils/src/config.rs (SecurityConfig, DisabledCspModificationKind) ; .../crates/tauri/scripts/freeze_prototype.js ; .../crates/tauri-utils/src/html.rs

### Tauri does not block navigation to remote URLs by default; you must add `on_navigation` / `on_new_window` handlers. Remote pages get no IPC unless a `remote` capability exists.

Confidence: high

The internal navigation handler only calls the user's `WebviewWindowBuilder::on_navigation(|url| -> bool)` and plugin hooks; with none set, navigation is allowed. Documented example: `.on_navigation(|url| url.scheme() == "tauri" || (cfg!(dev) && url.host_str() == Some("localhost")))`. On Windows the production origin is `http://tauri.localhost`, so also allow host `tauri.localhost`. Use `on_new_window` to deny `window.open` (desktop only). Windows defined in tauri.conf.json are auto-created; to attach handlers set `"create": false` and build with `WebviewWindowBuilder::from_config(app.handle(), &app.config().app.windows[i])`. Defence in depth: since 2.12, requests from a non-local origin are rejected for custom commands ("Command … not allowed by ACL") unless a capability with `remote.urls` matches. `limitNavigationsToAppBoundDomains` (2.12.0) is iOS-only.

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/manager/webview.rs (L601-630) ; .../crates/tauri/src/webview/webview_window.rs (on_navigation, on_new_window) ; .../crates/tauri/src/webview/mod.rs (is_local_url) ; .../crates/tauri-utils/src/config.rs

### Use `tauri::ipc::Channel<T>` for token streaming; events are documented as unsuitable for high throughput.

Confidence: high

Docs: the event system "is not designed for low latency or high throughput situations", payloads are always JSON strings, and there is no capability control; channels "are designed to be fast and deliver ordered data". Rust: `#[tauri::command] fn chat(on_event: Channel<ChatEvent>)` then `on_event.send(..)?`. JS: `import { invoke, Channel } from '@tauri-apps/api/core'; const ch = new Channel<ChatEvent>(); ch.onmessage = (m) => {...}; await invoke('chat', { onEvent: ch })`. Implementation: JSON payloads under 8192 bytes (raw under 1024) are delivered by direct eval, larger ones are queued and fetched via `plugin:__TAURI_CHANNEL__|fetch`. Binary chunks use `Channel<InvokeResponseBody>` with `InvokeResponseBody::Raw`. Channel needs no permission. Fixes to note: 2.12.3 keeps delivering after an `onmessage` handler throws; 2.12.0 scopes queued channel data per webview and stops queuing once the webview is closed. Global `emit` reaches all windows; `emit_to` targets one label.

Source: https://v2.tauri.app/develop/calling-frontend/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/ipc/channel.rs ; .../crates/tauri/CHANGELOG.md

### Overlay window: transparency no longer needs `macOSPrivateApi`; click-through is a runtime call only; macOS coverage of fullscreen Spaces is limited.

Confidence: high

Changelog 2.12.1: "The `macos-private-api` feature flag / `macOSPrivateAPI` tauri.conf.json value is no longer required to use transparency or fullscreen on macOS" (the config docs call it a no-op in 2.12.1+). There is no config key for click-through: call `window.set_ignore_cursor_events(true)` in Rust after creation, or JS `setIgnoreCursorEvents` with `core:window:allow-set-ignore-cursor-events`.

Windows (tao 0.37.2): click-through sets `WS_EX_TRANSPARENT | WS_EX_LAYERED` on the whole window; `focusable: false` sets `WS_EX_NOACTIVATE`; `noRedirectionBitmap: true` (2.12.0) avoids the white flash on transparent windows; `shadow: true` on an undecorated window draws a 1px border.

macOS: `alwaysOnTop` is `NSFloatingWindowLevel`; `visibleOnAllWorkspaces` only sets `CanJoinAllSpaces` (no FullScreenAuxiliary), so the overlay will not appear over another app's native-fullscreen Space without extra native code (third-party tauri-nspanel 2.1.1 exists). `skipTaskbar` is unsupported on macOS; `visibleOnAllWorkspaces` is unsupported on Windows. `contentProtected` maps to `WDA_EXCLUDEFROMCAPTURE` on Windows and `NSWindowSharingType::None` on macOS.

Suggested window config: {"label":"overlay","create":false,"url":"overlay.html","transparent":true,"decorations":false,"shadow":false,"alwaysOnTop":true,"skipTaskbar":true,"focus":false,"focusable":false,"resizable":false,"visibleOnAllWorkspaces":true,"noRedirectionBitmap":true}.

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/CHANGELOG.md ; .../crates/tauri-utils/src/config.rs (WindowConfig, AppConfig.macos_private_api) ; https://github.com/tauri-apps/tao/blob/tao-v0.37.2/src/platform_impl/macos/window.rs ; .../src/platform_impl/windows/window_state.rs

### Updater plugin never checks on its own; config lives under `plugins.updater`, and `pubkey` is mandatory once the plugin is registered.

Confidence: high

`Builder::build()` only stores state and registers the commands check / download / install / download_and_install. A request happens only when JS `check()` or Rust `app.updater()?.check().await` is called, so "opt-in" is entirely app logic. Config keys: `pubkey` (key content, not a path; required), `endpoints` (https enforced in release unless `dangerousInsecureTransportProtocol`), `windows.installMode` (`passive` default | `basicUi` | `quiet`), `windows.installerArgs`, `allowDowngrades` (moved into config in 2.12.0, default false), `requireSignedVersion` (default false; rejects an update whose signed version differs from the announced one), `dangerousAcceptInvalidCerts`, `dangerousAcceptInvalidHostnames`. Endpoint variables: `{{current_version}}`, `{{target}}`, `{{arch}}`, `{{bundle_type}}`. Static GitHub endpoint: `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`. If the plugin is registered without a `plugins.updater` block, initialization fails with "Error deserializing 'plugins.updater' within your Tauri configuration". Signature verification cannot be disabled. 2.14.0 verifies package bytes again in `Update::install`.

Source: https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/updater/src/lib.rs ; .../plugins/updater/src/config.rs ; .../plugins/updater/CHANGELOG.md ; https://v2.tauri.app/plugin/updater/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri/src/plugin.rs

### Updater artifacts and signing at build time: `bundle.createUpdaterArtifacts: true` plus `TAURI_SIGNING_PRIVATE_KEY` (and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`); the build fails without the key.

Confidence: high

Generate keys with `tauri signer generate -w ~/.tauri/myapp.key`. `TAURI_SIGNING_PRIVATE_KEY` may be the key content or a file path for build/bundle; `.env` files do not work. With updater artifacts enabled and the variable absent the CLI errors: "A public key has been found, but no private key. Make sure to set `TAURI_SIGNING_PRIVATE_KEY` environment variable." Outputs with `true`: Windows reuses `<productName>_<version>_<arch>-setup.exe` and adds `-setup.exe.sig`; macOS produces `<App>.app.tar.gz` + `.app.tar.gz.sig`. The macOS archive is only produced when the `app` bundle target is built; `dmg` alone triggers the warning "no updater-enabled targets were built". `"v1Compatible"` is deprecated and will be removed in v3. Docs: losing the private key means you cannot ship updates to existing installs. The CLI also records the version in the signature (used by `requireSignedVersion`).

Source: https://v2.tauri.app/plugin/updater/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-cli/src/bundle.rs (sign_updaters) ; .../crates/tauri-bundler/src/bundle.rs (L221-262) ; .../crates/tauri-cli/ENVIRONMENT_VARIABLES.md

### tauri-action current major is v1 (tags v1, v1.0, v1.0.0), running on node24; `includeUpdaterJson` no longer exists.

Confidence: high

Inputs in action.yml at v1: releaseId, tagName, releaseName, releaseBody, releaseDraft (default false), prerelease (false), releaseCommitish, projectPath ('.'), uploadUpdaterJson (true), updaterJsonPreferNsis (false), tauriScript, args, retryAttempts, owner, repo, githubBaseUrl, releaseAssetNamePattern, uploadPlainBinary (false), uploadWorkflowArtifacts (false), workflowArtifactNamePattern, uploadUpdaterSignatures (true), generateReleaseNotes (false), mobile. Outputs: releaseId, releaseHtmlUrl, releaseUploadUrl, artifactPaths, appVersion. `__VERSION__` in tagName/releaseName is replaced with the app version. Breaking changes in 1.0.0: `includeUpdaterJson` renamed to `uploadUpdaterJson`; `assetNamePattern` renamed to `releaseAssetNamePattern`; `includeRelease`/`includeDebug` removed (use `args: --debug`); `updaterJsonKeepUniversal` removed; Tauri v1 support dropped; project auto-init removed; `.app.tar.gz` assets now include the version; the action fails if `releaseDraft: true` but the existing release is not a draft; node24 needs runner >= v2.327.1. Omit tagName/releaseName/releaseId to build without uploading.

Source: https://github.com/tauri-apps/tauri-action/blob/v1/action.yml ; https://github.com/tauri-apps/tauri-action/blob/dev/CHANGELOG.md ; https://github.com/tauri-apps/tauri-action/blob/dev/README.md ; `git ls-remote --tags https://github.com/tauri-apps/tauri-action`

### latest.json from tauri-action v1: keys `{os}-{arch}` and `{os}-{arch}-{bundle}`, URLs pointing at the GitHub API asset endpoint.

Confidence: high

File shape: {version, notes, pub_date, platforms:{<key>:{signature,url}}}. Keys written: `windows-x86_64`, `windows-x86_64-nsis`, `darwin-aarch64`, `darwin-aarch64-app` and so on. The updater looks up `{os}-{arch}-{bundle_type}` first, then `{os}-{arch}`; the suffixed keys need tauri-plugin-updater >= 2.10.0. If both MSI and NSIS are built, the plain `windows-x86_64` entry is the MSI unless `updaterJsonPreferNsis: true`. URLs are `https://api.github.com/repos/<owner>/<repo>/releases/assets/<id>` (changed in 1.0.0); the updater sends `Accept: application/octet-stream` and User-Agent `tauri-plugin-updater/<version>` by default. Each matrix job downloads the existing latest.json from the release and merges its own platforms. `releases/latest/download/latest.json` does not resolve for draft or prerelease releases, so clients see nothing until the draft is published.

Source: https://github.com/tauri-apps/tauri-action/blob/v1/src/upload-version-json.ts ; https://github.com/tauri-apps/plugins-workspace/blob/v2/plugins/updater/src/updater.rs (L536, L838) ; https://github.com/tauri-apps/tauri-action/blob/dev/CHANGELOG.md

### Example release matrix for windows-latest (NSIS) and macos-latest (aarch64, app+dmg), adapted from the official example (not run).

Confidence: medium

jobs.publish: permissions: {contents: write}; strategy.fail-fast: false; matrix.include: [{platform: windows-latest, args: '--bundles nsis'}, {platform: macos-latest, args: '--target aarch64-apple-darwin --bundles app,dmg'}]; runs-on: ${{ matrix.platform }}.
Steps:
1. actions/checkout@v7
2. actions/setup-node@v6 with node-version: lts/*, cache: npm
3. dtolnay/rust-toolchain@stable (targets: aarch64-apple-darwin on macOS)
4. swatinem/rust-cache@v2 with workspaces: './src-tauri -> target'
5. npm ci
6. a script that downloads the llama.cpp binaries (must run before the build, because externalBin/resources must exist)
7. tauri-apps/tauri-action@v1 with env GITHUB_TOKEN, TAURI_SIGNING_PRIVATE_KEY, TAURI_SIGNING_PRIVATE_KEY_PASSWORD (plus APPLE_* on macOS, AZURE_* on Windows) and with: tagName: v__VERSION__, releaseName: 'App v__VERSION__', releaseDraft: true, prerelease: false, uploadUpdaterJson: true, updaterJsonPreferNsis: true, args: ${{ matrix.args }}.

The Tauri docs example uses checkout@v7 and setup-node@v6 (latest tags today: checkout v7.0.1, setup-node v7.1.0, rust-cache v2.9.3, upload-artifact v7.0.2). Runner labels today: windows-latest = Windows Server 2025 (VS2026 image), macos-latest = macOS 26 arm64, ubuntu-latest = 24.04. With `--target <triple>` output is under `src-tauri/target/<triple>/release/bundle/`, and the externalBin suffix must match that triple.

Source: https://v2.tauri.app/distribute/pipelines/github/ ; https://github.com/tauri-apps/tauri-action/blob/v1/examples/publish-to-auto-release.yml ; https://github.com/actions/runner-images/blob/main/README.md ; `git ls-remote --tags` for each action

### macOS signing and notarization are driven purely by environment variables read by the Tauri CLI; no manual keychain step is needed.

Confidence: high

Signing: `APPLE_CERTIFICATE` (base64 of the .p12: `openssl base64 -A -in cert.p12`), `APPLE_CERTIFICATE_PASSWORD`; the bundler creates a temporary keychain and imports it. `APPLE_SIGNING_IDENTITY` overrides `bundle.macOS.signingIdentity` and is inferred from the certificate if absent. Use a "Developer ID Application" certificate for DMG distribution.
Notarization (attempted automatically after signing): either `APPLE_ID` + `APPLE_PASSWORD` (app-specific password) + `APPLE_TEAM_ID` (missing team id is a hard error), or `APPLE_API_KEY` (key id) + `APPLE_API_ISSUER` + `APPLE_API_KEY_PATH` (.p8; otherwise searched as `AuthKey_<id>.p8` in `./private_keys`, `~/private_keys`, `~/.private_keys`, `~/.appstoreconnect/private_keys`). With neither set the build logs "skipping app notarization" and continues. `--skip-stapling` exists for a slow first notarization. `APPLE_PROVIDER_SHORT_NAME` is optional.
Placeholder: `"signingIdentity": "-"` gives an ad-hoc signature (the DMG is then left unsigned); docs say users must still approve the app in Privacy & Security, and it avoids Apple Silicon builds being treated as damaged.
Other: `minimumSystemVersion` defaults to "10.13" (set "13.0"); `hardenedRuntime` defaults to true. With `CI=true` the DMG script is run with `--skip-jenkins` (Finder window layout/background is skipped) unless `TAURI_BUNDLER_DMG_IGNORE_CI=true`.

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-bundler/src/bundle/macos/sign.rs ; .../crates/tauri-cli/ENVIRONMENT_VARIABLES.md ; https://v2.tauri.app/distribute/sign/macos/ ; .../crates/tauri-bundler/src/bundle/macos/dmg/mod.rs ; .../crates/tauri-utils/src/config.rs (MacConfig)

### Windows signing options today: legacy `certificateThumbprint` (signtool + cert store), or `signCommand` (any tool; Azure Key Vault via relic; Azure Artifact Signing via artifact-signing-cli).

Confidence: high

Thumbprint route: `bundle.windows {certificateThumbprint, digestAlgorithm: "sha256", timestampUrl, tsp}` runs `signtool /fd <alg> /sha1 <thumb> /tr <url> /td <alg>`; `TAURI_WINDOWS_SIGNTOOL_PATH` overrides the tool location. The docs flag it as applying only to OV certificates acquired before June 1st 2023 (exportable PFX).
`signCommand` is a string, or `{cmd, args}`, with `%1` as the file placeholder; it is required when cross-compiling.
Azure Artifact Signing ("previously called Azure Code Signing/Azure Trusted Signing"): `cargo install artifact-signing-cli` (0.12.0; `trusted-signing-cli` 0.11.0 is marked DEPRECATED), env `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID`, config `"signCommand": "artifact-signing-cli -e https://wus2.codesigning.azure.net -a MyAccount -c MyProfile -d MyApp %1"`. Prerequisites (.NET 8, Azure CLI, signtool) are stated to be preinstalled on GitHub runners.
Docs also note that since 2024 EV certificates no longer give immediate SmartScreen reputation, and that Windows 11 Smart App Control blocks unsigned apps outright.
Most practical placeholder: ship unsigned with no signing keys in the base config, and keep a separate config file containing only the `signCommand`, passed via `args: --config <file>` once an Artifact Signing account exists.

Source: https://v2.tauri.app/distribute/sign/windows/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-utils/src/config.rs (WindowsConfig, CustomSignCommandConfig) ; .../crates/tauri-bundler/src/bundle/windows/sign.rs ; `cargo search artifact-signing-cli`

### NSIS specifics: `installMode` currentUser (default) | perMachine | both; bundled NSIS 3.13 with solid LZMA; installer size ceiling about 2 GB.

Confidence: medium

`bundle.windows.nsis.installMode`:
- `currentUser`: `RequestExecutionLevel user`, installs to `%LOCALAPPDATA%\<productName>`, registry under HKCU, no admin.
- `perMachine`: admin, `Program Files`, HKLM.
- `both`: user chooses, but admin is always required.
The updater's `quiet` mode cannot elevate, so per-user installs update without UAC.
Bundler: NSIS 3.13 (since tauri-bundler 2.10.3), nsis_tauri_utils v0.5.3, default `SetCompressor /SOLID lzma`; `nsis.compression` = zlib | bzip2 | lzma | none. Output: `target/release/bundle/nsis/<productName>_<version>_<arch>-setup.exe`. Installer flags: `/S` silent, `/P` passive, `/R` restart app, `/NS` no shortcuts, `/UPDATE`, `/ARGS`. Hooks via `nsis.installerHooks`: NSIS_HOOK_PREINSTALL / POSTINSTALL / PREUNINSTALL / POSTUNINSTALL. `allowDowngrades` defaults to true. The uninstaller's "delete app data" checkbox removes `%APPDATA%\<identifier>` and `%LOCALAPPDATA%\<identifier>` (not during updates), which would delete downloaded models stored there.
2 GB limit: the NSIS wiki (per search snippet; page itself returned 403) says NSIS "can theoretically compile installers as large as 2GB"; NSIS's own history lists "Support for compression of files up to 2GB". Resources are embedded in the installer, so all bundled llama variants count toward it.

Source: https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi ; .../nsis/mod.rs ; .../crates/tauri-utils/src/config.rs (NsisConfig) ; https://v2.tauri.app/distribute/windows-installer/ ; https://nsis.sourceforge.io/I_get_an_error_when_compiling_large_installers (search snippet only) ; https://github.com/kichik/nsis/blob/master/Docs/src/history.but

### WebView2 install modes: `downloadBootstrapper` (default) and `embedBootstrapper` both need internet when WebView2 is missing; only `offlineInstaller` (+~127 MB) or `fixedRuntime` (+~180 MB) are truly offline.

Confidence: high

`bundle.windows.webviewInstallMode.type`: `skip` | `downloadBootstrapper` {silent: true} (0 MB; downloads `https://go.microsoft.com/fwlink/p/?LinkId=2124703` at install time) | `embedBootstrapper` (+~1.8 MB, still downloads the runtime) | `offlineInstaller` (embeds the full installer) | `fixedRuntime` {path} (pinned runtime you must patch yourself). The docs table calls the last one "fixedVersion" but the config value is `fixedRuntime`. The installer first reads the registry (`EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}` in HKLM/HKCU) and skips the step when WebView2 is present. Docs: "On Windows 10 (April 2018 release or later) and Windows 11, the WebView2 runtime is distributed as part of the operating system". So on the stated targets the default normally makes no network call; `offlineInstaller` only matters for machines where the runtime was removed. `bundle.windows.minimumWebview2Version` makes the installer run the bootstrapper if the installed runtime is older (needs internet). The offline installer is downloaded from Microsoft at build time.

Source: https://v2.tauri.app/distribute/windows-installer/#webview2-installation-options ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-utils/src/config.rs (WebviewInstallMode) ; .../bundle/windows/nsis/installer.nsi (L592-670) ; .../bundle/windows/util.rs

### Linux dev build in this container is feasible but the dependencies are not installed; the requested simulation fails only because apt lists are empty.

Confidence: high

Requested command output: `E: Couldn't find any package by glob 'libwebkit2gtk-4.1-dev'` / `E: Unable to locate package libayatana-appindicator3-dev` / `librsvg2-dev` / `libxdo-dev`; `pkg-config --modversion webkit2gtk-4.1` prints "Package webkit2gtk-4.1 was not found in the pkg-config search path". Cause: `/var/lib/apt/lists` is empty (no `apt-get update` has run).
I repeated the simulation against a private apt state in the scratchpad (no system change): `apt-get update` fetched 28.6 MB in 2 s, and `apt-get install -s` of the five packages resolves: "0 upgraded, 147 newly installed", "Need to get 62.0 MB of archives", "After this operation, 272 MB of additional disk space will be used". Candidate libwebkit2gtk-4.1-dev is 2.52.6-0ubuntu0.24.04.1 (Tauri needs webkit2gtk >= 2.40 via feature `v2_40`).
Environment: Ubuntu 24.04.5, root, rustc/cargo 1.97.0 (>= MSRV 1.90), Node v22.23.4, npm 10.9.9, gcc and pkg-config present, libssl-dev 3.0.13 already installed, 4 cores, 15 GB RAM, 29 GB free disk. No DISPLAY, but `xvfb-run` is installed.
The official Debian list is: libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev. So `apt-get update && apt-get install -y ...` followed by `cargo check` / `tauri build --no-bundle` should work; running the GUI needs xvfb.

Source: Commands run in this container on 2026-11-10 (`apt-get install -s`, `pkg-config`, `apt-get -o Dir::State::lists=<scratch> update/install -s/--assume-no`) ; https://v2.tauri.app/start/prerequisites/ ; https://github.com/tauri-apps/tauri/blob/tauri-v2.12.5/crates/tauri-runtime-wry/Cargo.toml

### Frontend toolchain versions on npm today (context, outside the Tauri packages).

Confidence: high

vite 8.3.7, react 19.3.1, typescript 7.0.2, @vitejs/plugin-react 6.3.0, tailwindcss 4.3.5, create-tauri-app 4.7.5. The current create-tauri-app React-TS template pins `typescript ~6.0.3`, `vite ^8.0.16`, `@vitejs/plugin-react ^6.0.2`, `react ^19.1.0`, `@tauri-apps/api ^2`, `@tauri-apps/cli ^2`, and ships `@tauri-apps/plugin-opener ^2` by default. The official template has not moved to TypeScript 7.

Source: `npm view <pkg> version` ; https://github.com/tauri-apps/create-tauri-app/blob/dev/templates/template-react-ts/package.json.lte

## Recommendations

- Pin the 2.x line explicitly: `tauri = "2.12"`, `tauri-build = "2.7"`, `@tauri-apps/cli` and `@tauri-apps/api` `^2.12`, plugins at the 2.x versions listed. Set `rust-version = "1.90"`. Do not take versions from `cargo search` (it shows 3.0.0-alpha) or npm's `next` tag.
- Do not add tauri-plugin-shell. Spawn llama-server from Rust with `tokio::process::Command` (CREATE_NO_WINDOW on Windows, `kill_on_drop(true)`, plus a job object / process group for tree-kill). The webview then has no process-spawning surface at all.
- Windows packaging: ship each llama.cpp variant with its DLLs through a `bundle.resources` map into `llama/<variant>/`, declared in `tauri.windows.conf.json`. Bundle Vulkan and CPU; download CUDA on demand into the app data dir to keep the installer and every update small and clear of the NSIS ceiling.
- macOS packaging: prefer a single statically linked llama-server shipped as `externalBin` (declared in `tauri.macos.conf.json`) so Tauri signs it with hardened runtime and notarizes it with no extra steps. If dylibs are unavoidable, list them in `bundle.macOS.frameworks` and add rpath `@executable_path/../Frameworks`, or pre-sign everything yourself before `tauri build`. Do not put unsigned Mach-O files in Resources.
- Declare all app commands with `tauri_build::AppManifest::new().commands(&[...])` in build.rs and grant `allow-<command>` only to the `main` capability, so the overlay window (and any future window) cannot invoke agent/tool commands. Enforce tool-approval decisions in Rust, not in the webview.
- Use two explicit capability files (main, overlay), reference them in `app.security.capabilities`, and keep global-shortcut, single-instance, window-state, deep-link and os Rust-only so they need no permissions. Exclude the overlay from window-state with `with_denylist(&["overlay"])`.
- Set `app.security.csp` to the strict policy in the findings, keep `withGlobalTauri` false, try `freezePrototype: true` from day one, and proxy all llama-server traffic through Rust (Channel) so `connect-src` stays `'self' ipc: http://ipc.localhost` with no `http://127.0.0.1`. Verify CSP with `tauri build --debug`, since the dev server bypasses it.
- Create both windows with `"create": false` and build them in `setup` via `WebviewWindowBuilder::from_config(...)`, adding `.on_navigation(...)` that only allows the app origin (`tauri://localhost`, `http://tauri.localhost`, and the dev URL in dev) and an `.on_new_window(...)` that denies. Open external links through the opener plugin or a Rust command.
- Stream tokens with `tauri::ipc::Channel<T>` and coalesce tokens in Rust (flush every ~16-33 ms) rather than one message per token. Use events only for low-rate broadcasts such as toggling the overlay.
- Updater: register the plugin with `pubkey` and the GitHub `latest.json` endpoint, but perform `check()` only from a Rust command behind an explicit user action or setting, and grant no `updater:*` permission to the webview. Consider `requireSignedVersion: true` from the first release. Generate the updater key now and back it up.
- CI: `tauri-apps/tauri-action@v1` with `uploadUpdaterJson: true`, `updaterJsonPreferNsis: true`, `releaseDraft: true`; Windows `--bundles nsis`, macOS `--target aarch64-apple-darwin --bundles app,dmg` (the `app` target is required for the updater archive). Fetch llama.cpp binaries before the build step. For PR builds, omit tagName/releaseName and pass a config override that turns `createUpdaterArtifacts` off.
- Signing placeholders: macOS `"signingIdentity": "-"` in config, overridden by `APPLE_SIGNING_IDENTITY` when the Developer ID secrets exist; Windows unsigned until an Azure Artifact Signing account exists, then add `signCommand` via a separate `--config` file. Only define the APPLE_* / AZURE_* env vars in the workflow when the secrets are actually present.
- Installer: NSIS only, `installMode: "currentUser"`, `webviewInstallMode: {"type": "embedBootstrapper"}` as the default; switch to `offlineInstaller` only if a fully offline first install on machines lacking WebView2 is a hard requirement (+~127 MB). Set `bundle.macOS.minimumSystemVersion: "13.0"`.
- For Linux dev in this container: `apt-get update && apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev` (62 MB), keep llama resources out of the base tauri.conf.json so `cargo check` does not need Windows/macOS binaries, and use `xvfb-run` for anything that opens a window.

## Risks

- Notarization failure if any Mach-O (llama-server, libllama/libggml dylibs) ends up under Contents/Resources or `bundle.macOS.files`: Tauri does not sign those paths. This typically only shows up at the first real notarized release.
- Ad-hoc macOS signing (`-`) combined with the default `hardenedRuntime: true` may stop a sidecar from loading separately shipped dylibs (library validation with no Team ID). Tauri's own docs suggest `hardenedRuntime: false` for ad-hoc signatures. Not tested here.
- Windows Authenticode signing must happen inside `tauri build` (thumbprint or `signCommand`). Signing the `-setup.exe` afterwards changes the file and would invalidate the updater `.sig` and latest.json signature (my inference from the build order; not tested).
- With `createUpdaterArtifacts: true`, a missing or empty `TAURI_SIGNING_PRIVATE_KEY` breaks the build. Unset GitHub secrets arrive as empty strings, and empty APPLE_CERTIFICATE / APPLE_SIGNING_IDENTITY values are likely to be treated as set and fail signing (inferred from `var_os` usage; not tested).
- Without `AppManifest::commands`, any XSS in the main window (model output rendered as markdown/HTML) or any second window can call every app command, including file/terminal/computer-use tools. CSP reduces but does not remove this.
- tauri-action v1 writes api.github.com asset URLs into latest.json. Unauthenticated GitHub API rate limits (per IP) could make update downloads fail behind shared NATs, and private repositories would need an auth header. I did not verify the rate-limit behaviour for asset downloads.
- The updater always downloads the full installer (no delta updates). Every MB of bundled llama.cpp binaries is re-downloaded on each update, and NSIS installers have a roughly 2 GB ceiling (limit confirmed only indirectly; the NSIS wiki was unreachable).
- macOS overlay limitations: always-on-top is only NSFloatingWindowLevel and `visibleOnAllWorkspaces` does not add FullScreenAuxiliary, so the indicator will not show over other apps' fullscreen Spaces without native code. The overlay will also appear in the agent's own screenshots unless excluded; `contentProtected` behaviour with newer macOS capture APIs was not verified.
- Uninstalling on Windows with the "delete app data" box ticked removes `%APPDATA%\<identifier>` and `%LOCALAPPDATA%\<identifier>`, which would delete multi-GB downloaded models if they are stored there.
- CSP is not enforced under `tauri dev` on desktop, so CSP breakage (inline styles from UI libraries, WASM, blob workers) only appears in built apps. `freezePrototype` can break third-party libraries at runtime.
- Tauri 3 is in alpha with a higher MSRV (1.95). Tauri marks `createUpdaterArtifacts`, the `macos-private-api` flag and the channel plugin naming for removal or change in v3, so a later migration is likely.
- Runner drift: `macos-latest` is now macOS 26 arm64 and `windows-latest` is Server 2025 with VS2026. Building for a macOS 13 deployment target on a macOS 26 SDK depends on `minimumSystemVersion` being set and on the llama.cpp binaries' own minimum OS version.
