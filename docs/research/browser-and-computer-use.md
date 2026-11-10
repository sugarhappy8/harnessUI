# browser-and-computer-use — research

## Summary

Verified on 2026-11-10 against crates.io, npm, crate sources, Chromium/llama.cpp/Playwright sources, Apple/Microsoft docs and Hugging Face. Nothing was written to the repo.

**Main decisions**

1. **CDP client:** write a thin CDP client over `tokio-tungstenite` 0.30.0 and spawn the installed Chrome/Edge ourselves. `chromiumoxide` 0.9.1 is the only tokio-native high-level crate and covers every required command, but its defaults are wrong for a real-browsing agent and its typed events can silently drop. `headless_chrome` 1.0.22 is synchronous, so it is out.
2. **Browser launch:** use `--remote-debugging-port=0` with our own `--user-data-dir` and read `<user-data-dir>/DevToolsActivePort`. Chrome 136+ only blocks debugging of the default data directory; a custom directory is explicitly the supported path.
3. **Form-submission gate:** layer a tool-level intent check over a `Fetch.requestPaused` backstop for non-GET requests. `Fetch` patterns cannot filter by HTTP method, and WebSocket frames, GET side effects and telemetry noise remain gaps.
4. **Page representation:** an accessibility-tree snapshot with `[ref=eN]` refs mapped to `backendDOMNodeId`. Full snapshots measured 14.5k–19.4k tokens on ordinary pages, so a small local model needs a hard cap, viewport filtering and a find tool.
5. **Computer use:** `xcap` 0.9.10 for capture and `enigo` 0.6.1 for keyboard. On Windows, write our own `SendInput` mouse path, because enigo's absolute move only addresses the primary monitor. On macOS, xcap still uses `CGWindowListCreateImage`, so keep a ScreenCaptureKit fallback.
6. **Coordinates:** a per-model coordinate-mode setting is required. Qwen2.5-VL-lineage models emit absolute pixels of the resized image; Qwen3-VL/Qwen3.5-lineage models (Holo2/3.1/4) emit 0–1000 normalised; Gemma 4 emits 0–1000 boxes, reportedly y-first.
7. **Emergency stop:** enigo tracks and releases held keys only, not mouse buttons, and only if `Drop` runs. We need our own pressed-set for keys and buttons. Injection into elevated windows fails silently under UIPI.

**Not verified today (marked medium/low in findings):** the System Settings deep links on macOS 15/26, whether permissions need an app restart, pipe file-descriptor numbers, the `msedge.exe` App Paths key, uiAccess secure locations, and the coordinate conventions of Holo1.5, UI-Venus-2, Fara1.5 and OS-Atlas.

Crate sources I read are under `<cargo registry>/` (`chromiumoxide-0.9.1`, `headless_chrome-1.0.22`, `enigo-0.6.1`, `xcap-0.9.10`) and `<temp workspace>` (`cdp/chromiumoxide_cdp-0.9.1`, `llama/mtmd-*.cpp`).

## Findings

### CDP crate status: chromiumoxide 0.9.1 is tokio-only and slow-moving; headless_chrome 1.0.22 is synchronous; tokio-tungstenite is 0.30.0.

Confidence: high

chromiumoxide 0.9.1 released 2026-02-25 (0.9.0 on 2026-02-20 removed async-std; 0.8.0 on 2025-11-28). MSRV 1.85, edition 2024, MIT OR Apache-2.0, last commit on main 2026-04-03, bundled CDP PDL r1566079, transport async-tungstenite 0.32 (tokio-runtime). headless_chrome 1.0.22 released 2026-06-11, MSRV 1.85, MIT: blocking tungstenite 0.29 plus std threads, no tokio; defaults `idle_browser_timeout` 30 s and `ignore_certificate_errors: true`. tokio-tungstenite 0.30.0 released 2026-07-11, MSRV 1.85. Forks seen: chromey 2.58.2 (2026-09-12), spider_chrome 2.37.130, rustenium 1.1.15 (2026-10-25).

Source: crates.io API (crates/chromiumoxide, headless_chrome, tokio-tungstenite, chromey, rustenium); chromiumoxide-0.9.1/CHANGELOG.md and Cargo.toml.orig; headless_chrome-1.0.22/src/browser/process.rs; https://github.com/mattsse/chromiumoxide/commits/main/

### chromiumoxide 0.9.1 meets every functional requirement: installed browser, own profile, headful, raw typed CDP.

Confidence: high

`BrowserConfig::builder()` offers `.chrome_executable(path)`, `.user_data_dir(dir)`, `.with_head()`, `.viewport(None)`, `.respect_https_errors()`, `.disable_default_args()`, `.arg()/.args()`, `.port(u16)`, `.launch_timeout()`, `.request_timeout()`. `Browser::launch(config)` and `Browser::connect(url)` / `Browser::connect_with_config(url, HandlerConfig)` return `(Browser, Handler)`; the Handler stream must be polled in a spawned task. Raw CDP is `page.execute<T: Command>(cmd)` and `page.event_listener::<Event>()`. Present in chromiumoxide_cdp 0.9.1: `accessibility::GetFullAxTreeParams`, `page::CaptureScreenshotParams`, `input::DispatchMouseEventParams`, `DispatchKeyEventParams`, `InsertTextParams`, `dom::GetContentQuadsParams`, `GetBoxModelParams`, `ScrollIntoViewIfNeededParams`, `ResolveNodeParams`, `fetch::EventRequestPaused`, `page::EventFrameRequestedNavigation`, `EventJavascriptDialogOpening`.

Source: chromiumoxide-0.9.1/src/browser/config.rs, src/browser/mod.rs, src/page.rs, examples/interception.rs; chromiumoxide_cdp-0.9.1/src/cdp.rs

### chromiumoxide's defaults are unsafe or wrong for an agent driving the user's real browser.

Confidence: high

Read from source: (a) headless by default (`HeadlessMode::True`). (b) Viewport defaults to `Some(800x600)`, applied via `Emulation.setDeviceMetricsOverride`. (c) `ignore_https_errors: true` in both `BrowserConfigBuilder` and `HandlerConfig::default()` (which `Browser::connect` uses), so it sends `Security.setIgnoreCertificateErrors(true)`. (d) `DEFAULT_ARGS` has 24 flags including `--enable-automation`, `--password-store=basic`, `--use-mock-keychain`, `--lang=en_US`, `--disable-sync`, `--disable-popup-blocking`, `--disable-background-networking`; `--disable-extensions` is added when no extension is configured. (e) Default profile is `temp_dir()/chromiumoxide-runner`. (f) The ws URL is parsed from child stderr (a line containing `listening on ws…devtools/browser`); there is no DevToolsActivePort or pipe support. (g) `LAUNCH_TIMEOUT` is 20000 ms and `REQUEST_TIMEOUT` is 30000 ms, so commands waiting behind a human approval can time out. (h) `enable_request_intercept()` enables Fetch for urlPattern `*` with `handleAuthRequests` and disables the HTTP cache. (i) Windows detection reads only `App Paths\chrome.exe` (HKLM then HKCU) and falls back only to `C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe`.

Source: chromiumoxide-0.9.1/src/browser/config.rs, src/handler/mod.rs, src/handler/network.rs, src/handler/viewport.rs, src/detection.rs

### chromiumoxide's typed events can be dropped silently when an evergreen browser is newer than its pinned protocol, which would hang a Fetch-based gate.

Confidence: medium

Generated enums in chromiumoxide_cdp 0.9.1 are closed: `network::ResourceType` has 19 variants and no `#[serde(other)]`; there are zero catch-all variants in `cdp.rs`. Undeserialisable messages become `CdpError::InvalidMessage` and are ignored by default (`ignore_invalid_events: true`; opt out with `surface_invalid_messages()`). If a newer Chrome/Edge adds an enum value inside `Fetch.requestPaused`, the event is dropped and the paused request is never continued. That last step is my inference from the code, not an observed failure. Also, `chromiumoxide_cdp/src/cdp.rs` is 5.19 MB / 111,037 lines, a noticeable compile-time cost.

Source: chromiumoxide_cdp-0.9.1/src/cdp.rs; chromiumoxide-0.9.1/src/conn.rs, src/handler/mod.rs, src/browser/config.rs

### Installed Chrome/Edge locations on Windows and macOS (Playwright's lookup table).

Confidence: high

Windows: try each prefix `%LOCALAPPDATA%`, `%PROGRAMFILES%`, `%PROGRAMFILES(X86)%`, `%HOMEDRIVE%\Program Files`, `%HOMEDRIVE%\Program Files (x86)` joined with `\Google\Chrome\Application\chrome.exe` or `\Microsoft\Edge\Application\msedge.exe` (Beta/Dev/Canary use `Chrome Beta`, `Chrome Dev`, `Chrome SxS`, `Edge Beta`, `Edge Dev`, `Edge SxS`). Registry: `HKLM` or `HKCU` `\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe`, default value (verified in chromiumoxide source); the equivalent `App Paths\msedge.exe` key is from my own knowledge, not verified today. macOS: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` and `/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge`; also check `~/Applications` (my addition). Default profiles to avoid: `%LOCALAPPDATA%\Google\Chrome\User Data` and `~/Library/Application Support/Google/Chrome`.

Source: https://raw.githubusercontent.com/microsoft/playwright/main/packages/playwright-core/src/server/registry/index.ts; chromiumoxide-0.9.1/src/detection.rs; https://chromium.googlesource.com/chromium/src/+/main/docs/user_data_dir.md

### Chrome 136+ ignores remote-debugging switches only for the default data directory; a custom --user-data-dir is the supported route.

Confidence: high

Chrome blog: "These switches will no longer be respected if attempting to debug the default Chrome data directory. These switches must now be accompanied by the `--user-data-dir` switch to point to a non-standard directory." It applies to both `--remote-debugging-port` and `--remote-debugging-pipe`; Chrome for Testing is unaffected. Current Chromium source enables the default-directory check for Google Chrome branding, treats an unknown result as default (`is_default_user_data_dir.value_or(true)`), and also honours the policy pref `kDevToolsRemoteDebuggingAllowed`. Whether Edge applies the same restriction is unconfirmed, and irrelevant if we always pass our own directory. Chrome 144+ adds a consent-based mode at `chrome://inspect/#remote-debugging` (used by chrome-devtools-mcp `--autoConnect`), which is not what an app-owned profile needs.

Source: https://developer.chrome.com/blog/remote-debugging-port; https://raw.githubusercontent.com/chromium/chromium/main/chrome/browser/devtools/remote_debugging_server.cc; https://developer.chrome.com/docs/devtools/agents/use-cases/auto-connect

### Port 0 plus the DevToolsActivePort file is the practical transport; the pipe is more secure but harder from Rust.

Confidence: medium

With `--remote-debugging-port=0` Chromium atomically writes `<user-data-dir>/DevToolsActivePort` as `"%d\n%s"`: line 1 is the port, line 2 the browser path (`/devtools/browser/<uuid>`). Connect to `ws://127.0.0.1:<port><path>`. HTTP endpoints `/json/version` and `/json/list` reject a Host header that is not an IP or localhost. A WebSocket request carrying an `Origin` header is rejected unless `--remote-allow-origins` allows it; tungstenite sends no Origin by default (my knowledge). `--remote-debugging-pipe` frames messages as NUL-terminated JSON by default, or CBOR with `=cbor` (verified in source); the fd 3/4 convention and Windows handle inheritance are from my knowledge. chrome-launcher notes that port value 0 also sets `navigator.webdriver` to true, and calls the pipe "more secure than using protocol over a websocket". The loopback port has no authentication, so any local process can attach.

Source: https://raw.githubusercontent.com/chromium/chromium/main/content/browser/devtools/devtools_http_handler.cc; .../devtools_pipe_handler.cc; https://raw.githubusercontent.com/GoogleChrome/chrome-launcher/main/docs/chrome-flags-for-tools.md

### Launch flags worth copying from Playwright/Puppeteer for an installed, headful, app-owned profile.

Confidence: high

Useful: `--user-data-dir=<app dir>`, `--remote-debugging-port=0`, `--no-first-run` ("Skip first run wizards"), `--no-default-browser-check`, `--disable-search-engine-choice-screen`. Edge on Windows: `--edge-skip-compat-layer-relaunch` (Playwright: Edge can relaunch and lose its stdio/CDP handles). If the parent is ever elevated: `--do-not-de-elevate`. Playwright also disables features such as `msForceBrowserSignIn`, `MediaRouter`, `OptimizationHints`. Puppeteer falls back to `--remote-debugging-port=0` when no port or pipe is given. My judgement, not from a source: omit `--enable-automation`, `--use-mock-keychain`, `--password-store=basic`, `--disable-sync`, `--disable-extensions` and `--disable-popup-blocking` for a profile the user logs into.

Source: https://raw.githubusercontent.com/microsoft/playwright/main/packages/playwright-core/src/server/chromium/chromiumSwitches.ts; https://raw.githubusercontent.com/puppeteer/puppeteer/main/packages/puppeteer-core/src/node/ChromeLauncher.ts; chrome-flags-for-tools.md

### What CDP and the DOM actually expose for detecting form submissions.

Confidence: high

`Fetch.enable` takes `RequestPattern {urlPattern, resourceType, requestStage}`; there is no method filter, so gating non-GET means pausing every request of the chosen resource types and continuing GETs at once. `Fetch.requestPaused` carries `requestId`, `request` (a `Network.Request` with `method`, `hasPostData`, `postDataEntries`; post data may be omitted when too long), `frameId`, `resourceType`, optional `networkId`, `redirectedRequestId`; the request stays paused until `continueRequest`, `failRequest` or `fulfillRequest`. `Page.frameRequestedNavigation` (experimental, notification only) reports `reason: ClientNavigationReason` whose values include `formSubmissionGet`, `formSubmissionPost`, `anchorClick`, `scriptInitiated`, and a `disposition` (`currentTab`, `newTab`, `newWindow`, `download`). `Network.requestWillBeSent` has `hasUserGesture` and `initiator`. DOM side (MDN): `submit` fires on the form when the user clicks a submit button, submits implicitly (e.g. Enter in a text input) or script calls `requestSubmit()`; it is "not sent to the form when a script calls the form.submit() method directly"; failed validation fires `invalid` and no `submit`.

Source: chromiumoxide_cdp-0.9.1/pdl/domains/Fetch.pdl, Page.pdl, Network.pdl (r1566079); https://developer.mozilla.org/en-US/docs/Web/API/HTMLFormElement/submit_event

### Playwright MCP and chrome-devtools-mcp both use accessibility-tree text snapshots with per-element refs.

Confidence: high

Playwright MCP 0.0.85 (2026-11-09): YAML-like lines such as `- textbox "What needs to be done?" [ref=e5]`; iframe refs are prefixed (`f1e12`); a ref is "Valid until the page changes"; stale refs return "Ref <ref> not found in the current page snapshot. Try capturing new snapshot." `browser_snapshot` accepts `target`, `depth`, `boxes` (`[box=x,y,width,height]` in viewport CSS px) and `filename`; `browser_find` (text or regex) returns only matching nodes plus context; `--snapshot-mode` is `full` or `none`. In Playwright's source, AI mode gives refs only to visible elements that receive pointer events, caches the ref on the element as `_ariaRef`, and reuses it while role and name are unchanged. chrome-devtools-mcp 1.12.0 (2026-10-22): `take_snapshot` lines look like `uid=<id> role "name" attrs`; uid is `${snapshotId}_${idCounter++}` and is reused across snapshots when `${loaderId}_${backendNodeId}` matches; it calls Puppeteer `accessibility.snapshot({includeIframes: true, interestingOnly: !verbose})` with a 10 s timeout. The CDP primitive for our own version is `Accessibility.getFullAXTree {depth?, frameId?}`, whose `AXNode` has `ignored`, `role`, `name`, `value`, `properties`, `childIds`, `backendDOMNodeId`, `frameId`.

Source: npm view @playwright/mcp, chrome-devtools-mcp; https://playwright.dev/mcp/snapshots; playwright packages/injected/src/ariaSnapshot.ts; chrome-devtools-mcp src/TextSnapshot.ts and src/formatters/SnapshotFormatter.ts; Accessibility.pdl

### Full accessibility snapshots are too large for small local contexts; image inputs cost roughly 900–2,700 tokens per screenshot on Qwen-family models.

Confidence: medium

A third-party measurement (2026-02-26, @playwright/mcp v0.0.68, tokens estimated as chars/4) found full Playwright MCP snapshots of 16,044 tokens (a Wikipedia article), 19,409 (a GitHub repo page) and 14,547 (Hacker News front page); a more compact formatter cut these by 51–79%. Neither official doc publishes a budget. Image cost, computed by me from llama.cpp's smart-resize code with its default 8–4096 token limits: Qwen2.5-VL lineage (28 px per token side) 1280x720 → 1288x728 = 1,196 tokens, 1440x900 → 1428x896 = 1,632, 1920x1080 → 1932x1092 = 2,691; Qwen3-VL lineage (32 px) 1280x720 → 1280x704 = 880, 1280x800 = 1,000, 1920x1080 → 1920x1088 = 2,040. GGUF metadata can override these limits. Gemma 4 uses fixed budgets of 70/140/280/560/1120 tokens.

Source: https://dev.to/kuroko1t/how-accessibility-tree-formatting-affects-token-cost-in-browser-mcps-n2a; llama.cpp tools/mtmd/mtmd-image.cpp and mtmd-model.cpp (master, fetched 2026-11-10); https://huggingface.co/blog/gemma4

### enigo 0.6.1 API, and three pitfalls that matter for this app.

Confidence: high

enigo 0.6.1 released 2025-08-28 (no release in about 14 months), MIT, MSRV 1.85. API: `Enigo::new(&Settings::default())`; trait `Keyboard { text(&str), key(Key, Direction), raw(u16, Direction), fast_text }`; trait `Mouse { button(Button, Direction), move_mouse(x, y, Coordinate::{Abs,Rel}), scroll(len, Axis), main_display(), location() }`; `Direction::{Press, Release, Click}`; `Settings` includes `release_keys_when_dropped: true`, `open_prompt_to_get_permissions: true`, `windows_dw_extra_info`, `event_source_user_data`; `EVENT_MARKER = 100`. Pitfall 1: on Windows an absolute move sends `MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE` without `MOUSEEVENTF_VIRTUALDESK`, normalised against `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`; Microsoft documents that such coordinates "map to the primary monitor". Pitfall 2: `held()` and `Drop` cover keys and raw keycodes only, never mouse buttons. Pitfall 3: on macOS `Enigo::new` calls `AXIsProcessTrustedWithOptions` and shows the system prompt unless `open_prompt_to_get_permissions` is false; it also calls `TISCopyCurrentKeyboardInputSource`/`TISGetInputSourceProperty`, and one third-party report says macOS 26 aborts when TSM/TIS is called off the main thread. enigo depends on `windows` 0.61 while xcap uses 0.62.

Source: crates.io API; enigo-0.6.1/src/lib.rs, src/win/win_impl.rs, src/macos/macos_impl.rs, Cargo.toml.orig; https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput

### xcap 0.9.10 is the maintained capture crate, with coordinate units that differ per OS and a deprecated macOS capture call.

Confidence: high

xcap 0.9.10 released 2026-11-09, Apache-2.0, `image` 0.25. `Monitor::all()`, `Monitor::from_point(x, y)`, then `id/name/friendly_name/x/y/width/height/rotation/scale_factor/frequency/is_primary/is_builtin`, `capture_image() -> RgbaImage`, `capture_region(x, y, w, h)`; `Window::all()` with `capture_image()`. macOS: x/y/width/height come from `CGDisplayBounds` (points, global display space); `scale_factor` = display-mode pixel width / point width; capture calls `CGWindowListCreateImage` with the default image option, which I expect returns backing pixels (not run here). That function is marked obsoleted in the macOS 15 SDK; reports say it still works at runtime, and its status on macOS 26+ is contested. Windows: geometry from `DEVMODE` (`dmPosition`, `dmPelsWidth/Height`), i.e. physical pixels; capture is GDI by default, Windows.Graphics.Capture behind the `wgc` feature. `capture_region` therefore takes points on macOS and pixels on Windows. Alternatives: `screenshots` 0.8.10 (2024-03-01) is deprecated ("Move to XCap"); `rdev` 0.5.3 dates from 2023-06-26; `scap` stable is 0.0.8 with 0.1.0-beta.1 from 2025-08-04; `windows-capture` 2.0.1 (2026-08-08) is Windows-only; `screencapturekit` 11.1.2 (2026-11-02) is active. `SCScreenshotManager` needs macOS 14.0+.

Source: crates.io API; xcap-0.9.10/src/monitor.rs, src/macos/impl_monitor.rs, src/macos/capture.rs, src/windows/impl_monitor.rs, src/windows/capture.rs; Apple docs JSON for SCScreenshotManager; web search results on CGWindowListCreateImage

### macOS permission APIs are confirmed; deep links and restart behaviour are only partly confirmed.

Confidence: medium

`CGPreflightScreenCaptureAccess()` and `CGRequestScreenCaptureAccess()` exist from macOS 10.15. `AXIsProcessTrustedWithOptions(options)` exists from 10.9 with key `kAXTrustedCheckOptionPrompt`; Apple: "Prompting occurs asynchronously and does not affect the return value." `CGPreflightPostEventAccess()` / `CGRequestPostEventAccess()` exist from 10.15. Deep links listed by a community reference for Ventura through Sonoma 14.2: `x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture` and `...?Privacy_Accessibility`; I found no source confirming them, or the `com.apple.settings.PrivacySecurity.extension` form, on macOS 15 or 26. Restart: support pages and user reports describe the system's "Quit & Reopen" step for Screen Recording; that Accessibility takes effect without relaunch is my understanding, not confirmed today. macOS 15 re-prompts for screen recording about monthly (press report). On macOS 26.1, developers report plain executables do not appear in the Screen Recording list, so capture must come from the .app bundle.

Source: Apple docs JSON (developer.apple.com/tutorials/data/documentation/coregraphics/..., applicationservices/1459186-axisprocesstrustedwithoptions); https://github.com/bvanpeski/SystemPreferences/blob/main/macos_preferencepanes-Ventura.md (via search); https://developer.apple.com/forums/thread/807898

### Ad-hoc or unsigned macOS builds lose TCC grants on every build.

Confidence: high

Apple DTS (Quinn "The Eskimo!"): "macOS tracks code identity using the code's designated requirement. Ad hoc signed code does not include a stable DR, and thus macOS is unable to tell that version N+1 of your app is the 'same code' as version N." The stated fix is a stable signing identity: Apple Development during development, Developer ID for direct distribution (see TN3127). Community reports add that the old toggle can still look enabled while not applying, and that `tccutil reset ScreenCapture <bundle id>` or `tccutil reset Accessibility <bundle id>` clears it.

Source: https://developer.apple.com/forums/thread/795739

### On Windows, Tauri becomes Per-Monitor-V2 DPI aware at runtime, not via the manifest, and requests no elevation.

Confidence: high

tao calls `become_dpi_aware()` once in `EventLoop::new` when `attributes.dpi_aware` is true (the default). It tries `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`, retries with `..._PER_MONITOR_AWARE`, and uses `SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE)` or `SetProcessDPIAware()` only when the newer API is missing. tauri-build's default `windows-app-manifest.xml` declares only Common-Controls 6.0.0.0: no `dpiAwareness` and no `requestedExecutionLevel`. Microsoft: a PMv2 process sees "the raw pixels of each display" (Windows 10 1703+), while calls from unaware or system-aware threads may return virtualised values. So DPI-sensitive calls made before the event loop exists can return scaled numbers. Versions today: tauri 2.12.5 stable (3.0.0-alpha.6 published 2026-11-06), tao 0.37.2, wry 0.57.1.

Source: https://raw.githubusercontent.com/tauri-apps/tao/dev/src/platform_impl/windows/dpi.rs and event_loop.rs; https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-build/src/windows-app-manifest.xml; https://learn.microsoft.com/en-us/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows; crates.io API

### Correct mapping from a downscaled screenshot back to input coordinates on multi-monitor setups.

Confidence: medium

Capture one monitor and keep its origin (Mx, My), captured size (Wp, Hp) and scale s. If the image sent to the model is (Wm, Hm), first get model-image pixels: absolute models give (xm, ym) directly; 0–1000 models give xm = xn/1000*Wm. Then px = xm*Wp/Wm, py = ym*Hp/Hm. Windows (process PMv2-aware, xcap geometry in physical pixels): global = (Mx+px, My+py); for `SendInput` use `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` with dx = round((gx - SM_XVIRTUALSCREEN) * 65535 / (SM_CXVIRTUALSCREEN - 1)), likewise dy. Microsoft documents the 0–65535 range and that VIRTUALDESK maps to the whole virtual desktop; the exact rounding is mine. macOS: events use the global display coordinate space in points, origin at the upper-left of the main display, so global = (Mx_pt + px/s, My_pt + py/s) where xcap's x/y are already points. Secondary monitors can have negative origins on both systems.

Source: https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput; Apple docs JSON for CGDisplayBounds, CGEvent init(mouseEventSource:...), NSScreen.backingScaleFactor; xcap-0.9.10 sources

### llama.cpp resizes Qwen-family images itself and warns that grounding needs at least 1024 image tokens.

Confidence: high

On llama.cpp master, Qwen2-VL, Qwen2.5-VL and Qwen3-VL projectors are resized to a multiple of patch_size*n_merge with Python-style rounding, bicubic, no padding, under `set_limit_image_tokens(8, 4096)` unless the GGUF carries min/max pixel keys or the user passes `--image-min-tokens N` / `--image-max-tokens N` (env `LLAMA_ARG_IMAGE_MIN_TOKENS` / `LLAMA_ARG_IMAGE_MAX_TOKENS`). At load it logs: "Qwen-VL models require at minimum 1024 image tokens to function correctly on grounding tasks … try adding --image-min-tokens 1024" (pointing to issue #16842). H Company's own llama.cpp recipe for Holo-3.1 uses `--image-min-tokens 1024`. Consequence: an absolute-pixel model answers in the coordinates of llama.cpp's resized image, not of the image we sent, unless we send exactly that size. `/v1/chat/completions` accepts `image_url.url` as base64 or a data URI.

Source: llama.cpp tools/mtmd/mtmd-image.cpp and mtmd-model.cpp (master, fetched 2026-11-10); https://raw.githubusercontent.com/ggml-org/llama.cpp/master/tools/server/README.md; https://hub.hcompany.ai/models-api/local-inference.md; https://github.com/ggml-org/llama.cpp/issues/16842

### Qwen2.5-VL-lineage grounding models emit absolute pixels of the smart-resized image (factor 28).

Confidence: medium

UI-TARS-1.5-7B: Apache-2.0, tagged `qwen2_5_vl`, ScreenSpot-Pro 49.6, OSWorld 27.5. Its coordinate guide sets `IMAGE_FACTOR = 28`, `MIN_PIXELS = 100*28*28`, `MAX_PIXELS = 16384*28*28`, `MAX_RATIO = 200` and maps back with x = x_model / new_width * width; the README says "Qwen 2.5vl based models ultilizes absolute coordinates". GGUF: `Mungert/UI-TARS-1.5-7B-GGUF` lists 27 .gguf files and none is an mmproj, so the projector must come from another repo (not checked). UI-TARS-2 was announced 2025-09-04; I found no open weights under ByteDance-Seed. Qwen2.5-VL itself: `ggml-org/Qwen2.5-VL-7B-Instruct-GGUF` ships `Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf` plus `mmproj-Qwen2.5-VL-7B-Instruct-Q8_0.gguf` / `-f16.gguf`; visual tokens per image default to 4–16384. Fara-7B (MIT, Qwen2.5-VL 7B): pixel coordinates, tool spec says "The screen's resolution is 1428x896", which equals smart-resize of 1440x900 at factor 28. Holo1.5 (3B/7B/72B, 7B Apache-2.0, ScreenSpot-Pro 57.94 for 7B) is Qwen2.5-VL-based; its card does not state the convention, so absolute is inferred.

Source: https://raw.githubusercontent.com/bytedance/UI-TARS/main/README.md and README_coordinates.md; https://huggingface.co/ByteDance-Seed/UI-TARS-1.5-7B; HF API for Mungert/UI-TARS-1.5-7B-GGUF, ggml-org/Qwen2.5-VL-7B-Instruct-GGUF; HF READMEs for Qwen2.5-VL-7B-Instruct, microsoft/Fara-7B, Hcompany/Holo1.5-7B

### Qwen3-VL, Qwen3.5 and the current Holo models emit 0–1000 normalised coordinates.

Confidence: medium

Qwen3-VL cookbook: system prompt says "The screen's resolution is 1000x1000", conversion is `coordinate_relative / 1000 * resized_width`, smart_resize factor 32. Official GGUFs exist, e.g. `Qwen3VL-8B-Instruct-Q4_K_M.gguf` with `mmproj-Qwen3VL-8B-Instruct-F16.gguf` or `-Q8_0.gguf` (Apache-2.0). Qwen3.5 (natively multimodal, Apache-2.0; the 9B card reports ScreenSpot-Pro 65.2 and OSWorld-Verified 41.8; `unsloth/Qwen3.5-9B-GGUF` has `Qwen3.5-9B-Q4_K_M.gguf` and `mmproj-F16.gguf`) uses 0–1000 according to a QwenLM discussion and ms-swift docs found by search, not the model card. H Company's element-localization doc specifies JSON `{x, y}` integers constrained to 0–1000, temperature 0.0, thinking off, with the prompt "Localize an element on the GUI image according to the provided target and output a click position." Holo4 (2026-09-25): 27B is CC-BY-NC-4.0 on Qwen3.8-27B, 35B-A3B is Apache-2.0 on Qwen3.6-35B-A3B; official GGUF repos contain `q4_k_m.gguf` and `mmproj.f16.gguf` (21.3 GB for 35B-A3B; Mac guidance "M3 or newer, 36 GB or more"). Holo-3.1 (2026-05/06, Apache-2.0, sizes 0.8B/4B/9B/35B-A3B): the only official GGUF is 35B-A3B; 0.8B/4B/9B GGUFs are community conversions (mradermacher). Holo2 (4B and 8B Apache-2.0, 30B-A3B CC-BY-NC, Qwen3-VL-Thinking base, ScreenSpot-Pro 57.2 / 58.9 / 66.1): the card does not state the convention; 0–1000 is inferred from the base model.

Source: https://raw.githubusercontent.com/QwenLM/Qwen3-VL/main/cookbooks/computer_use.ipynb; HF API (Qwen/Qwen3-VL-8B-Instruct-GGUF, unsloth/Qwen3.5-9B-GGUF, Hcompany/*); https://huggingface.co/Qwen/Qwen3.5-9B; https://hub.hcompany.ai/models-api/element-localization.md and local-inference.md

### Gemma, UI-Venus-2, Fara1.5 and OS-Atlas each need their own handling or a test before use.

Confidence: low

Gemma 4 (E2B, E4B, 26B-A4B, 31B; ggml-org GGUFs such as `gemma-4-E4B-it-Q4_0.gguf` with `mmproj-gemma-4-E4B-it-Q8_0.gguf`, Apache-2.0): the HF blog shows native JSON `[{"box_2d": [171, 75, 245, 308], "label": ...}]` with coordinates that "refer to an image size of 1000x1000"; a third-party integration says the order is [y1, x1, y2, x2]. No ScreenSpot numbers were found. Gemma 3 has llama.cpp GGUFs, but I found no evidence it is trained for GUI grounding. UI-Venus-2-9B (Qwen3.5-9B base, ScreenSpot-Pro 73.0, AndroidWorld 80.2, `bartowski/UI-Venus-2-9B-GGUF` with `mmproj-UI-Venus-2-9B-f16.gguf`): the card says the weight licence is pending and does not state the coordinate convention. Fara1.5 (4B/9B/27B, MIT, 9B on Qwen3.5-9B, WebVoyager 86.6, bartowski GGUFs): trained mostly at 1440x900, convention not stated, and the card says to use it only with Microsoft's own harness. OS-Atlas-Pro-7B (Qwen2-VL-7B base, Apache-2.0, last modified 2024-11) emits `CLICK <point>[[x,y]]</point>`; its scale is not stated on the card.

Source: https://huggingface.co/blog/gemma4; https://docs.voxel51.com/plugins/plugins_ecosystem/gemma4.html (via search); HF READMEs for inclusionAI/UI-Venus-2-9B, microsoft/Fara1.5-9B, OS-Copilot/OS-Atlas-Pro-7B; llama.cpp docs/multimodal.md

### Neither enigo nor the OS guarantees that input is released on stop; UIPI blocks elevated targets silently.

Confidence: medium

enigo's `Drop` releases only the keys and raw keycodes it recorded, only when `release_keys_when_dropped` is true and only if drop actually runs; mouse buttons are never tracked. Microsoft on `SendInput`: "This function fails when it is blocked by UIPI. Note that neither GetLastError nor the return value will indicate the failure was caused by UIPI blocking"; "Applications are permitted to inject input only into applications that are at an equal or lesser integrity level"; and it "does not reset the keyboard's current state" (check with `GetAsyncKeyState`). Tauri's manifest requests no elevation, so the app cannot drive elevated windows such as Task Manager; the same applies to UAC's secure desktop (my knowledge). Tauri's NSIS installer defaults to a per-user install under `%LOCALAPPDATA%`; `perMachine` installs to Program Files and needs admin. A `uiAccess="true"` manifest needs a signed binary in a secure location such as Program Files (my knowledge; the Microsoft page returned 404 today). macOS equivalents for querying state: `CGEventSource.keyState(_:key:)` exists; button state is from my knowledge.

Source: enigo-0.6.1/src/win/win_impl.rs and src/macos/macos_impl.rs; https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput; https://v2.tauri.app/distribute/windows-installer/; enigo-0.6.1/Permissions.md

## Recommendations

- CDP client: build a small in-house client on tokio-tungstenite 0.30.0 using flat sessions (`Target.setAutoAttach {autoAttach: true, flatten: true, waitForDebuggerOnStart: true}`), with `serde_json::Value` or lenient hand-written structs for the roughly 25 methods and 10 events needed. Reasons: gate-critical events must never be dropped by a pinned protocol, no hidden defaults, small dependency and compile footprint. If speed matters more, use chromiumoxide 0.9.1 behind our own trait with `Browser::connect_with_config(ws, HandlerConfig { ignore_https_errors: false, viewport: None, .. })`, our own `Fetch.enable`, and a watchdog for paused requests. Do not use headless_chrome.
- Browser launch: write our own detector (Playwright's prefix and suffix table, App Paths for chrome.exe and msedge.exe, a user override). Spawn with `--user-data-dir=<app data>/browser-profile --remote-debugging-port=0 --no-first-run --no-default-browser-check --disable-search-engine-choice-screen`, plus `--edge-skip-compat-layer-relaunch` for Edge. Delete any stale `DevToolsActivePort` before spawning, poll for the file, then connect to `ws://127.0.0.1:<port><path>`. If the profile is already running, attach to it instead of relaunching. Never point at the default profile.
- Form-submission gate, layer 1 (intent, before dispatch): classify each tool call. Prompt when a click resolves to `button[type=submit]`, `input[type=submit|image]`, a default-type `<button>` inside a form or with a `form` attribute; when Enter is pressed with focus in a form field; and when evaluated JS calls `submit()` or `requestSubmit()`.
- Form-submission gate, layer 2 (enforcement): run our own `Fetch.enable` at request stage for resource types Document, XHR, Fetch and Ping on every attached session. Continue GETs immediately. Hold non-GET requests that fall inside a short window after an agent action until approved, and `Fetch.failRequest` on denial. Use `Page.frameRequestedNavigation` reasons `formSubmissionGet` / `formSubmissionPost` to label prompts and to catch GET forms. Optionally add a capture-phase `submit` listener through `Page.addScriptToEvaluateOnNewDocument` and `Runtime.addBinding`.
- Document the gate's gaps in ARCHITECTURE.md: WebSocket frames, GET links with side effects, background or telemetry POSTs (allow-list or auto-continue outside the action window), service-worker and other-target requests unless attached, page-side fetch timeouts while the user decides, and post data omitted when large. Paused requests must always be resolved on stop, timeout or crash.
- Page representation: default to our own accessibility snapshot from `Accessibility.getFullAXTree` per frame. Drop ignored and structural nodes, emit `- role "name" [ref=eN]`, and keep ref → (frameId, backendDOMNodeId, loaderId) so refs stay stable until navigation. Act through `DOM.scrollIntoViewIfNeeded`, `DOM.getContentQuads`, then `Input.dispatchMouseEvent`. Set a hard budget (about 2–4k tokens by default, configurable per model context), prefer viewport-only output, and add `find(text)` and `snapshot(ref, depth)` tools. Offer innerText or readability extraction as a separate read-page tool with its own cap, and screenshots only on request or for vision models.
- Computer use stack: put capture and input behind `ScreenCapture` and `InputInjector` traits. Capture with xcap 0.9.10; on macOS 14+ keep a ScreenCaptureKit fallback (screencapturekit 11.1.2), since xcap still calls `CGWindowListCreateImage`. Input with enigo 0.6.1 for keys and text, created lazily with `open_prompt_to_get_permissions: false` after onboarding and called on the main thread on macOS. On Windows, implement mouse move and buttons directly with `SendInput` and `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`.
- macOS permissions UX: preflight with `CGPreflightScreenCaptureAccess()` and `AXIsProcessTrustedWithOptions` (prompt false), request only on an explicit user click, open the Privacy panes via `x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture` and `?Privacy_Accessibility`, poll for the grant, and offer a Restart button for Screen Recording. Test the deep links on macOS 15 and 26 before release. Sign every build, including dev and CI, with one stable identity (Developer ID for release) so grants survive updates.
- Coordinates: add a per-model `coordinate_mode` to the model registry, with at least `absolute_resized` (Qwen2.5-VL, UI-TARS-1.5, Holo1.5, Fara-7B), `norm_1000_xy` (Qwen3-VL, Qwen3.5, Holo2/3.1/4) and `norm_1000_box_yx` (Gemma 4), plus `smart_resize_factor` (28 or 32) and min/max image tokens. Always pre-resize the screenshot ourselves to the exact smart-resize size so llama.cpp does not resize again, start llama-server with `--image-min-tokens 1024` for Qwen-family grounders, and ship a one-click calibration test so users can validate any GGUF.
- DPI: perform all capture and input after Tauri's event loop exists (or add a `dpiAwareness` PerMonitorV2 entry to a custom Windows manifest) so every Win32 call sees physical pixels. Capture one monitor per screenshot and carry its origin, size and scale with the image.
- Emergency stop: route all input through one injector that records every pressed key and mouse button. Prefer atomic click and tap operations, and put a watchdog on any hold or drag. On stop, panic hook or exit, release everything recorded plus all modifiers and all mouse buttons unconditionally. Register the stop control as a global shortcut plus an always-on-top button. A hard crash cannot be covered, so say so in the docs.
- Elevated windows: before injecting on Windows, compare the foreground window's process integrity level with ours and return a clear tool error ("target window is elevated; cannot control") instead of failing silently. Do not ship uiAccess or run-as-admin in v1.

## Risks

- The remote-debugging port on 127.0.0.1 has no authentication: any local process can attach to the agent's browser profile and its logged-in sessions. The pipe transport avoids this but is harder from Rust, especially on Windows.
- An evergreen installed Chrome or Edge can change CDP, flags or policy at any time, and several events we rely on (`Page.frameRequestedNavigation`, `Accessibility.getFullAXTree`) are marked experimental. Enterprise policy can disable remote debugging entirely. Whether Edge mirrors Chrome's default-profile restriction is unconfirmed.
- chromiumoxide (if chosen) has had no release since 2026-02-25 and no commit since 2026-04-03; its pinned protocol and closed enums can silently drop events from newer browsers. enigo has had no release since 2025-08-28.
- The form gate cannot be complete: WebSocket traffic, GET side effects and requests from unattached targets bypass it, while gating every POST causes approval fatigue and page timeouts. It should be described as defence in depth, not a guarantee.
- xcap's macOS capture uses `CGWindowListCreateImage`, obsoleted in the macOS 15 SDK, with conflicting reports about macOS 26 and later. macOS 15 also re-prompts for screen recording about monthly, and macOS 26.1 reportedly hides non-bundle executables from the permission list.
- The System Settings deep links and the restart-after-grant behaviour were not confirmed on macOS 15 or 26; a wrong link or missing restart prompt would break onboarding.
- Ad-hoc or unsigned macOS builds lose Screen Recording and Accessibility grants on every build, which will look like random permission bugs in development and to users of unsigned releases.
- Mixed-DPI multi-monitor setups are the most likely source of misclicks: enigo's Windows absolute move targets only the primary monitor, xcap's units differ by OS, and DPI calls made before Tauri's event loop may be virtualised.
- Coordinate conventions for several models (Holo1.5, Holo2, Qwen3.5, UI-Venus-2, Fara1.5, OS-Atlas, Gemma 4 axis order) are inferred or from secondary sources. A wrong mode produces confident clicks in the wrong place, and quantised GGUFs may ground worse than published scores.
- Capable local grounders are large (the official Holo GGUFs are 21.3 GB Q4 with a 36 GB Mac recommendation). The 4B–9B options rely on community GGUF conversions, and some licences are non-commercial (Holo4-27B, Holo2-30B-A3B) or pending (UI-Venus-2-9B).
- A screenshot costs roughly 900–2,700 image tokens and a full accessibility snapshot can exceed 15k tokens, so small-context local models can be exhausted in a few steps without strict budgets and history trimming.
- Synthetic input cannot reach elevated windows or the UAC secure desktop and fails without an error; a crash while a key or button is held leaves it logically down until the user presses it.
