# Studio (0.1.0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Studio, a section where people make pictures (OpenRouter, OpenAI) and videos (OpenRouter) with the endpoint keys they already saved, with the price shown and guarded before anything is sent and every result kept on their own computer.

**Architecture:** A new `agent::media` module turns OpenRouter's public catalogues and a bundled OpenAI file into typed controls (`ParamSpec`) and prices, and sends each run through the endpoint's own `OpenAiClient`, at most once. A `StudioManager` in `src-tauri/src/studio/` (modelled on `DownloadManager`) checks the spending rules, queues runs, follows video jobs across restarts, downloads results into a visible gallery folder, writes V3 store rows and one audit row per run. The webview renders every control from the `ParamSpec` list, so there is no React code per company.

**Tech Stack:** Rust (tokio, reqwest 0.13, rusqlite, Tauri 2, async-trait, chrono), React 19 + TypeScript + zustand + Radix, vitest; no new dependency.

**Spec:** docs/dev/0.1.0/studio-design.md

Every commit message ends with the attribution trailer your session requires. Run `cargo fmt --all` before each commit. `crates/agent` and `crates/store` have `#![warn(missing_docs)]`, so every public item there needs a doc comment or clippy (`-D warnings`) fails. A task is done when its own tests pass and `cargo clippy --workspace --all-targets -- -D warnings`, `npm run lint` and `npm run typecheck` stay clean.

---

## Global Constraints

- **Names:** the section is "Studio", its button is "Create"; code never says "generation" for it (`state::Generation` and the store's `generation` mean a chat reply). Rust types use `Creation*`, `Media*`, `Studio*`.
- **Companies:** OpenRouter (pictures and video) and OpenAI (pictures only; its video API shut down Sep 24, 2026). Anthropic, abliteration.ai and Generic endpoints get nothing in 0.1.0.
- **Hosts:** prompts and added pictures go only to the chosen endpoint's host (`openrouter.ai` or `api.openai.com`); results come from that host, or from the company's file host through the guarded fetcher **without the key**.
- **Keys:** no new key or secret; Studio reads `provider-api-key:<id>`. A key goes only to its endpoint's own scheme, host and port, never to a result link on another host.
- **No automatic requests:** opening Studio or Settings → Studio sends nothing. Lists are fetched only on Test, *Get the list*, *Update model lists*/*Update now*, or picking an OpenRouter picture model (its price).
- **Confirm before:** every video, any run estimated at **$1** or more, any run whose price is unknown. The backend refuses with `studio_needs_confirmation` when `confirmed` is false and a rule applies.
- **Daily limit:** **$10**, on by default, per local calendar day, enforced in the backend (`studio_daily_limit`) whatever the webview sends. A run that would take today's spending past it is not sent. An unknown price counts as passing unless at least **$5** remains and the user confirms.
- **Cost colours:** text always; colour is a second cue: under $1 plain, $1–$2.50 amber, over $2.50 red.
- **Never pay twice:** the request that creates a run or job is sent at most once. No retry on a timeout, a 5xx or a dropped connection after sending; one retry only on 429 with `Retry-After` of at most 30 s, or on a connect failure before any byte was sent. Resume, cancel, *Get it anyway* and *Check again* never send a create.
- **Safety refusals:** explained in plain words with the company, the stage when known, whether charged, and the request id; never retried automatically; no "rewrite to get past the filter" helper.
- **Queue:** up to 2 picture runs and 2 video jobs at once per endpoint; more wait, shown as "waiting".
- **Timeouts:** connect 10 s; picture request 5 min whole; video checks every 5 s for the first minute, then every 10 s, then every 20 s, each check limited to 30 s; after 30 min of watching the card says "still not ready" with *Check again*.
- **Caps:** picture response streamed to a temporary file, cap 192 MiB; 50 MiB per picture; 1 GiB per video; 20 MiB per added picture; the simple layer's "How many" chips stop at 4.
- **Events:** `studio-progress` (payload `RunDto`) to the main window, at most 4 per second per run; state is also readable through `studio_runs` and `studio_live`, because events are not replayed.
- **Files:** gallery default `<home>/harnessUI/studio`; `<gallery>/YYYY-MM/<run_id>-<n>.<ext>`, sidecar `<gallery>/YYYY-MM/<run_id>.json` (never the key), `<gallery>/refs/<sha256>.<ext>`, `<gallery>/.thumbs/<output_id>.jpg` (512 px JPEG), `<gallery>/.partial/` for downloads in progress. Allowed types: PNG, JPEG, WebP, GIF, SVG, MP4, WebM. Files are kept exactly as the company sent them.
- **Database:** migration V3 appended to `MIGRATIONS`; V1 and V2 are never edited; at most 20,000 runs.
- **Esc never cancels a Studio job.** In Studio, Esc only closes the viewer or a popup. The global stop hotkey stops chat turns and tools only.
- **CSP:** add `media-src blob:` to `src-tauri/tauri.conf.json` and to the pinned CSP test in `src-tauri/src/commands.rs`; video plays from `blob:` URLs made from `studio_output_bytes`.
- **Audit:** one row per run: server `studio`, tool `picture` or `video`, no chat, decision `allowed_without_prompt` decided by `user`, the prompt's length but never the prompt, completed when the run ends (also after a restart, through `audit_id`).
- **Commands:** each new command is listed in `src-tauri/commands.list`, granted as `allow-<cmd>` in `src-tauri/capabilities/main.json` (main window only) and registered in `generate_handler!` in `src-tauri/src/lib.rs`; each checks `commands::only_main_window`.
- **Navigation:** sidebar button between Models and Settings (sparkle icon), shortcut Ctrl/⌘ G, window event `app:open-studio`.
- **Defaults:** video length = the shortest of at least 4 s, resolution = 720p (or the lowest offered), sound as the company defaults; Studio picks and sends a random seed whenever the model takes one.
- **Chat link:** *Send to chat* for pictures only, through the composer's 20 MB and 8-picture limits; *Open in Studio* on a chat picture. No chat tool in 0.1.0.
- **Copy:** plain English, no raw identifiers on screen, all webview wording in `src/lib/words.ts`, backend wording in `src-tauri/src/studio/errors.rs`. Examples to keep verbatim: "Daily limit reached: $X of $10 spent today", "Cloud: your prompt and added pictures go to openrouter.ai", "price unknown", "Make this video?".
- **Not in 0.1.0:** chat picture tool, fal.ai, mask painting, video extend, side-by-side models, styles, system notifications.

## Review Focus

1. **App closed mid-run, then reopened.** A video in `waiting` or `downloading` is checked and downloaded again with no second create; a run left in `sending` becomes `interrupted` ("may or may not have started"); a `queued` run is never sent after a restart. Pinned in Task A14 (`resume_after_restart_finishes_the_video_without_a_second_create`, `a_run_left_sending_becomes_interrupted_and_is_never_sent_again`, `a_queued_run_is_not_sent_after_a_restart`) and Task D1.
2. **The result is gone before it is downloaded** (signed link expired overnight, content endpoint answers 404/410 after the job finished). The run fails with `studio_result_expired`, keeps its remote id and estimate as "may have been charged", and never re-creates. Pinned in Task A5 (`content_gone_after_done_is_expired`) and Task A14 (`an_expired_result_says_so_and_keeps_the_remote_id`).
3. **Disk full or a result over its cap.** The partial file is removed, the run fails with `studio_disk_full` or `studio_too_large`, and the remote id stays so *Get it anyway* downloads again without a new create. Pinned in Task A5 (`a_download_over_the_cap_removes_the_partial_file`), Task A9 (`a_full_disk_is_named`) and Task A14 (`a_full_disk_keeps_the_remote_id_for_get_it_anyway`).
4. **Money at the edges:** a run created at 23:59 counts on that day; queued and in-flight runs count by their estimate; an unknown price reserves $5; reaching the limit exactly is allowed, a cent over is not. Pinned in Task A3 (`reaching_the_limit_exactly_is_allowed`, `a_run_counts_on_the_day_it_was_created`, `an_unknown_price_needs_five_dollars_left`) and Task A13 (`queued_and_running_estimates_count_against_the_limit`).
5. **Duplicate Create** (double click, Enter plus click, *Again* pressed twice): one run, one charge, and two concurrent starts cannot both slip under the limit. Pinned in Task A13 (`the_same_create_press_twice_makes_one_run`, `two_starts_at_once_cannot_both_pass_the_limit`) and Task B7 (`Create is disabled while a start is on its way`).

Also covered where they belong: switching to a model that lacks a setting (Task B4 notes, Task A1 `validate_refuses_a_field_the_model_does_not_list`), and safety refusals (Tasks A4, A7, A13, B8).

---

## File Structure

**Rust, `crates/agent`**

| File | Responsibility |
|---|---|
| `src/media/mod.rs` (new) | Media types (`MediaKind`, `Company`, `MediaModel`, `MediaRequest`, `Started`, `JobState`, `MediaFailure`, `MediaError`, `MediaLimits`), the `MediaProvider` trait, `media_provider()` |
| `src/media/spec.rs` (new) | `ParamSpec` and its kinds; catalogue → `MediaModel`; `validate` |
| `src/media/price.rs` (new) | `Price`, `pricing_skus` and `/endpoints` parsing, `estimate`, OpenAI cost from usage |
| `src/media/openrouter.rs` (new) | `OpenRouterMedia`: catalogues, picture runs, video jobs, failure mapping |
| `src/media/openai.rs` (new) | `OpenAiImages`: bundled spec matching, generations, edits, failure mapping |
| `src/media/multipart.rs` (new) | Hand-built `multipart/form-data` body for OpenAI edits (reqwest's `multipart` feature would add `mime_guess`) |
| `src/media/specs/openai_images.json` (new) | Dated OpenAI settings, retire dates and prices |
| `src/client.rs` | `pub(crate)` helpers; `post_once`, `download_to_file`, `get_json_within` |
| `src/cloud.rs` | `media_company`, `openai_media_kind` |
| `src/lib.rs` | `pub mod media` and re-exports |
| `tests/media_endpoints.rs` (new) | Both adapters against `tests/common` `MockServer` |
| `tests/media_redirects.rs` (new) | Single-test binary: a non-loopback redirect to another origin carries no key |
| `tests/fixtures/media/` (new) | Catalogue snapshots, recorded `/endpoints` answers, OpenAI sample answers |

**Rust, other crates**

| File | Responsibility |
|---|---|
| `crates/search/src/fetch.rs` | `Fetcher::fetch_to_file`: binary, streamed, capped, same address rules, never a key |
| `crates/search/tests/fetch.rs` | Its tests (test hooks) |
| `crates/store/src/schema.rs` | V3 |
| `crates/store/src/creations.rs` (new) | `creation_runs` / `creation_outputs` accessors |
| `crates/store/src/lib.rs`, `error.rs` | Re-exports; `StoreError::TooManyCreations` |

**Rust, `src-tauri`**

| File | Responsibility |
|---|---|
| `src/studio/mod.rs` (new) | Module root |
| `src/studio/spend.rs` (new) | Confirmation rules, daily limit, day boundary (pure) |
| `src/studio/gallery.rs` (new) | Folder layout, type check and rename, sidecar, references, default folder |
| `src/studio/catalog.rs` (new) | `providerMediaModels:<id>` cache, refresh, `models_for` (no network) |
| `src/studio/prefs.rs` (new) | `StudioPrefs` (`studioPrefs` setting), defaults, validation |
| `src/studio/deps.rs` (new) | `ProviderSource`, `StudioSink`, `ForeignFetch`, `Clock` traits and their app implementations |
| `src/studio/manager.rs` (new) | `StudioManager`: start, queue, run tasks, video jobs, resume, cancel, audit, events |
| `src/studio/manager_tests.rs` (new) | Manager tests with fakes |
| `src/studio/commands.rs` (new) | Tauri commands |
| `src/studio/errors.rs` (new) | Studio `AppError`s and their copy |
| `src/studio/wire_tests.rs` (new) | Phase D: a whole run against a local OpenRouter mock |
| `src/dto.rs` | Studio DTOs |
| `src/providers.rs` | Test caches media lists; delete forgets them; `may_hold_key` `pub(crate)` |
| `src/attachments.rs`, `src/main.rs` | `sniff`/`checked_picture` `pub(crate)`; the picture worker's thumbnail mode |
| `src/artifacts.rs` | `export_target` takes an extension, `pub(crate)` |
| `src/lib.rs`, `commands.list`, `capabilities/main.json` | Registration, `manage`, resume at start |
| `tauri.conf.json`, `src/commands.rs` | `media-src blob:`; version test |

**Webview**

| File | Responsibility |
|---|---|
| `src/components/studio/types.ts` (new) | TS mirrors of the DTOs |
| `src/components/studio/api.ts` (new) | Typed `invoke` wrappers and `onStudioProgress` |
| `src/components/studio/fixtures/models.json` (new, generated) | Six models as the backend serialises them, shared by Rust and vitest |
| `src/store/studio.ts` (new) | zustand store: view, endpoints, drafts, runs, live progress, prefs, usage |
| `src/components/studio/canonical.ts` (new) | Defaults, carrying values between models, the change note, model families |
| `src/components/studio/cost.ts` (new) | Formatting and colour of prices |
| `src/components/studio/StudioView.tsx` (new) | Shell: top bar, tabs, empty states, layout |
| `src/components/studio/ModelPicker.tsx` (new) | Grouped, searchable picker with the Cloud mark and price hint |
| `src/components/studio/ParamField.tsx` (new) | The only widget renderer, driven by `ParamSpec` |
| `src/components/studio/SettingsPanel.tsx` (new) | Right panel: simple layer, My setups, Advanced toggle |
| `src/components/studio/AdvancedDrawer.tsx` (new) | Advanced fields, Extra fields (JSON), routing, request preview |
| `src/components/studio/RefTray.tsx` (new) | Added pictures with role chips |
| `src/components/studio/PromptDock.tsx` (new) | Prompt, chips, Cloud line, Create with price |
| `src/components/studio/ConfirmRun.tsx` (new) | The confirm dialog |
| `src/components/studio/RunCard.tsx` (new) | One run: progress, results, errors, actions |
| `src/components/studio/Grid.tsx`, `Viewer.tsx` (new) | Gallery grid; full-size viewer with keys |
| `src/components/studio/StudioNotice.tsx` (new) | "Your video is ready" while in chat |
| `src/components/settings/StudioSettings.tsx` (new) | Settings → Studio |
| `src/App.tsx`, `Sidebar.tsx`, `ShortcutsDialog.tsx`, `SettingsDialog.tsx` | View switch, button and badge, shortcut rows, the new tab |
| `src/components/settings/endpointPresets.ts`, `Endpoints.tsx` | `media` flags and "Chat · Pictures · Video" |
| `src/components/Composer.tsx`, `MessageItem.tsx` | `app:compose-images`; *Open in Studio* |
| `src/lib/words.ts` | All Studio copy |

**Docs, scripts, e2e:** `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md` (7.5, 12, 13.6, 13.7, 14, 17), `SECURITY.md`, `docs/SPEC.md`, `docs/dev/0.1.0/README.md`, `docs/dev/0.1.0/owner-checklist.md` (new), `scripts/docs_test.py`, `e2e/m8_offline.py`, `e2e/m12_studio.py` (new); version in `Cargo.toml`, `Cargo.lock`, `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`.

---

## Phase A: Backend

### Task A1: Media types and controls from OpenRouter's catalogues

**Files:**
- Create: `crates/agent/src/media/mod.rs`, `crates/agent/src/media/spec.rs`, `crates/agent/src/media/price.rs` (types only here)
- Create: `crates/agent/tests/fixtures/media/openrouter-images-2026-11-17.json`, `openrouter-videos-2026-11-17.json` (copies of `docs/dev/0.1.0/research/catalog/`)
- Modify: `crates/agent/src/lib.rs` (`pub mod media;`), `crates/agent/src/cloud.rs` (`media_company`)
- Test: inline `#[cfg(test)] mod tests` in `spec.rs`

**Interfaces:**
- Consumes: `agent::ProviderKind`, `agent::ProviderError`.
- Produces (`agent::media`):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum MediaKind { Image, Video }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "lowercase")]
pub enum Company { OpenRouter, OpenAi }            // "openrouter" | "openai"
// cloud.rs: pub fn media_company(kind: ProviderKind) -> Option<Company>;

// spec.rs (serde camelCase; mirrored by src/components/studio/types.ts)
pub struct ParamSpec { pub key: String, pub canonical: Option<Canonical>, pub label: String, pub help: Option<String>,
    pub layer: Layer, pub kind: ParamKind, pub company_default: Option<Value>, pub initial: Option<Value>,
    pub affects_price: bool, pub requires: Option<Requires> }
pub enum Canonical { Shape, Count, Quality, Resolution, Length, Sound, Seed, References, StartFrame, EndFrame, Size, Format, Background }
pub enum Layer { Simple, Advanced }
#[serde(tag = "type")] pub enum ParamKind { Enum { values: Vec<Choice> }, Range { min: f64, max: f64, step: Option<f64>, unit: Option<String> },
    Boolean, Seed { max: u64 }, Images { min: u32, max: u32, role: RefRole }, Size { rules: SizeRules }, Extra { allowed_keys: Vec<String> } }
pub struct Choice { pub value: Value, pub label: String }          // value: string or number, sent as is
pub struct Requires { pub key: String, pub values: Vec<Value> }
pub struct SizeRules { pub multiple_of: u32, pub min_ratio: f64, pub max_ratio: f64, pub max_side: u32, pub min_pixels: u64, pub max_pixels: u64 }
pub const OPENAI_SIZE_RULES: SizeRules;                           // 16, 1/3, 3, 3840, 655_360, 8_294_400
pub const SEED_MAX: u64 = 2_147_483_647;

// mod.rs
pub struct MediaModel { pub id: String, pub display_name: String, pub family: String, pub kind: MediaKind,
    pub accepts_images: bool, pub params: Vec<ParamSpec>, pub price: Price, pub notes: Vec<String>,
    pub usable: Option<String>, pub created: Option<i64> }
#[serde(rename_all = "camelCase")] pub enum RefRole { Reference, StartFrame, EndFrame }
pub struct RefImage { pub role: RefRole, pub mime: String, pub path: PathBuf, pub sha256: String, pub size_bytes: u64 }
pub struct Routing { pub order: Vec<String>, pub only: bool, pub allow_fallbacks: bool, pub zdr: bool }
pub struct MediaRequest { pub kind: MediaKind, pub model: String, pub prompt: String,
    pub params: Map<String, Value>, pub extra: Map<String, Value>, pub refs: Vec<RefImage>, pub routing: Option<Routing> }
pub struct RawCatalog { pub images: Value, pub videos: Option<Value> }
pub struct Saved { pub path: PathBuf, pub mime: String, pub size_bytes: u64, pub width: Option<u32>, pub height: Option<u32>, pub duration_ms: Option<u64> }
pub struct Cost { pub usd: f64, pub reported: bool }
pub enum Started { Done { outputs: Vec<Saved>, cost: Option<Cost> }, Job { remote_id: String } }
pub struct JobDone { pub outputs: usize, pub signed: Vec<SignedUrl>, pub cost: Option<Cost> }
pub struct SignedUrl { pub url: Url, pub expires_at: Option<i64> }
pub enum JobState { Pending, Running { progress: Option<f32> }, Done(JobDone), Failed(MediaFailure), Gone }
pub enum Fetched { Saved(Saved), Elsewhere(Url) }
pub struct RequestPreview { pub method: String, pub url: String, pub host: String, pub body: Value }
pub enum Stage { Prompt, Output }
pub enum FailureKind { Blocked, Unauthorized, NoCredit, OrgVerification, RateLimited, BadRequest, NotFound, Expired, Server, Other }
pub struct MediaFailure { pub kind: FailureKind, pub stage: Option<Stage>, pub charged: Option<bool>, pub company_code: Option<String>,
    pub request_id: Option<String>, pub status: Option<u16>, pub message: String, pub retry_after: Option<Duration> }
pub enum MediaError { NotSent(ProviderError), Answered(MediaFailure), Unknown(ProviderError) }
pub struct MediaLimits { pub response_cap: u64, pub picture_cap: u64, pub video_cap: u64, pub picture_timeout: Duration, pub check_timeout: Duration }
impl Default for MediaLimits  // 192 MiB, 50 MiB, 1 GiB, 300 s, 30 s
#[async_trait] pub trait MediaProvider: Send + Sync {
    fn company(&self) -> Company;
    fn host(&self) -> String;
    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError>;
    async fn fetch_image_price(&self, model: &str) -> Result<Value, MediaError>;   // Value::Null for OpenAI
    fn preview(&self, req: &MediaRequest) -> RequestPreview;
    /// Sent at most once. Pictures come back done; a video comes back as a job.
    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError>;
    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError>;
    async fn download(&self, remote_id: &str, job: &JobDone, index: usize, dest: &Path) -> Result<Fetched, MediaError>;
}
// spec.rs
pub fn parse_catalog(company: Company, raw: &RawCatalog, image_prices: &BTreeMap<String, Value>) -> Vec<MediaModel>;
pub fn openrouter_image_models(list: &Value) -> Vec<MediaModel>;
pub fn openrouter_video_models(list: &Value) -> Vec<MediaModel>;
pub fn family_of(id: &str) -> String;
pub enum Invalid { NotOffered(String), BadValue(String), Requires { key: String, needs: String }, TooManyPictures { role: RefRole, max: u32 },
    TooFewPictures { role: RefRole, min: u32 }, Reserved(String), EmptyPrompt, PromptTooLong, NotUsable(String) }
pub fn validate(model: &MediaModel, req: &MediaRequest) -> Result<(), Vec<Invalid>>;
// price.rs (types only in this task; Task A2 fills them)
pub enum Price { PerImage(Table), PerSecond(Table), PerToken, Unknown }
pub struct Table { pub rows: Vec<PriceRow>, pub per_input_image_usd: Option<f64>, pub minimum_usd: Option<f64> }
pub struct PriceRow { pub mode: Option<Mode>, pub resolution: Option<String>, pub quality: Option<String>, pub audio: Option<bool>, pub usd: f64 }
pub enum Mode { TextToVideo, ImageToVideo }
```
`MediaRequest` holds the structured request; the run's `params_json` later stores `{params, extra, routing}` exactly as sent. Every public type above derives `Debug, Clone, PartialEq, Serialize, Deserialize` (camelCase), except the trait and `MediaError`; `RawCatalog` is what Task A11 caches.

- [ ] **Step 1: Write the failing tests** (`crates/agent/src/media/spec.rs`)
```rust
const IMAGES: &str = include_str!("../../tests/fixtures/media/openrouter-images-2026-11-17.json");
const VIDEOS: &str = include_str!("../../tests/fixtures/media/openrouter-videos-2026-11-17.json");

#[test] fn the_picture_catalogue_gives_68_models() {
    let m = openrouter_image_models(&serde_json::from_str(IMAGES).unwrap());
    assert_eq!(m.len(), 68);
    assert!(m.iter().all(|m| m.kind == MediaKind::Image && m.usable.is_none()));
}
#[test] fn gpt_image_2_6_flare_has_its_controls() {
    let m = image("openai/gpt-image-2.6-flare");
    assert_eq!(m.display_name, "GPT Image 2.6 Flare");
    assert_eq!(values(&m, "quality"), ["auto", "low", "medium", "high", "xhigh", "max"]);
    assert_eq!(param(&m, "quality").layer, Layer::Simple);
    assert_eq!(param(&m, "n").kind, ParamKind::Range { min: 1.0, max: 10.0, step: Some(1.0), unit: None });
    assert_eq!(param(&m, "input_references").kind, ParamKind::Images { min: 0, max: 16, role: RefRole::Reference });
    assert_eq!(param(&m, "background").layer, Layer::Advanced);
    assert_eq!(param(&m, "output_compression").requires,
        Some(Requires { key: "output_format".into(), values: vec![json!("jpeg"), json!("webp")] }));
    assert!(maybe(&m, "frame_count").is_some() && maybe(&m, "seed").is_none());
}
#[test] fn nano_banana_has_no_count_and_no_seed() {
    let m = image("google/gemini-nano-banana-2.1");
    assert!(maybe(&m, "n").is_none() && maybe(&m, "seed").is_none());
    assert_eq!(values(&m, "resolution"), ["1K", "2K", "4K"]);
    assert!(param(&m, "resolution").affects_price);
}
#[test] fn k3_edit_needs_a_picture() {
    assert_eq!(param(&image("krea/k3-edit"), "input_references").kind,
        ParamKind::Images { min: 1, max: 8, role: RefRole::Reference });
}
#[test] fn no_catalogue_parameter_is_dropped() {
    // every supported_parameters key is a typed field or an Extra-field hint
}
#[test] fn the_video_catalogue_gives_33_models_and_4_need_a_video() {
    let m = openrouter_video_models(&serde_json::from_str(VIDEOS).unwrap());
    assert_eq!(m.len(), 33);
    let mut unusable: Vec<_> = m.iter().filter(|m| m.usable.is_some()).map(|m| m.id.as_str()).collect();
    unusable.sort();
    assert_eq!(unusable, ["black-forest-labs/flux-video-edit", "black-forest-labs/flux-video-upscale",
                          "heygen/avatar-iv", "runway/aleph-2"]);
}
#[test] fn video_defaults_keep_the_first_try_cheap() {
    let veo = video("google/veo-3.1");
    assert_eq!(values(&veo, "duration"), [json!(4), json!(6), json!(8)]);
    assert_eq!(param(&veo, "duration").initial, Some(json!(4)));
    assert_eq!(param(&veo, "resolution").initial, Some(json!("720p")));
    assert_eq!(param(&veo, "generate_audio").company_default, Some(json!(true)));
    assert_eq!(param(&veo, "first_frame").layer, Layer::Simple);
    assert_eq!(param(&veo, "last_frame").layer, Layer::Advanced);
    assert_eq!(values(&video("bytedance/seedance-2.5-flash"), "duration")[0], json!(4));    // -1 dropped
    assert_eq!(values(&video("google/veo-3.1-lite"), "duration"), [json!(4), json!(6), json!(8)]);  // sorted
    assert_eq!(param(&video("minimax/hailuo-3-max"), "resolution").initial, Some(json!("480p")));  // no 720p
    assert_eq!(param(&video("x-ai/grok-imagine-video-1.5-lite"), "duration").initial, Some(json!(4)));
    assert!(veo.notes.iter().any(|n| n == "OpenRouter video is not covered by zero data retention."));
    assert!(veo.notes.iter().any(|n| n == "Google adds an invisible SynthID watermark."));
}
#[test] fn passthrough_names_become_extra_fields() {
    let veo = video("google/veo-3.1");
    assert!(matches!(&param(&veo, "extra").kind, ParamKind::Extra { allowed_keys } if allowed_keys.contains(&"negativePrompt".to_string())));
}
#[test] fn families_group_versions() {
    assert_eq!(family_of("google/veo-3.1-fast"), "google/veo");
    assert_eq!(family_of("openai/gpt-image-2.6-flare"), "openai/gpt-image");
    assert_eq!(family_of("kwaivgi/kling-v3.0-pro"), "kwaivgi/kling");
}
#[test] fn validate_refuses_a_field_the_model_does_not_list() {
    let req = request("google/gemini-nano-banana-2.1", json!({"seed": 7}));
    assert_eq!(validate(&image("google/gemini-nano-banana-2.1"), &req), Err(vec![Invalid::NotOffered("seed".into())]));
}
#[test] fn validate_checks_values_requires_extra_and_pictures() {
    let flare = image("openai/gpt-image-2.6-flare");
    assert_eq!(validate(&flare, &request_p(json!({"quality": "ultra"}))), Err(vec![Invalid::BadValue("quality".into())]));
    assert!(matches!(validate(&flare, &request_p(json!({"output_compression": 80}))).unwrap_err()[0], Invalid::Requires { .. }));
    assert!(validate(&flare, &request_p(json!({"output_format": "webp", "output_compression": 80}))).is_ok());
    assert_eq!(validate(&image("krea/k3-edit"), &request_p(json!({}))).unwrap_err(),
        vec![Invalid::TooFewPictures { role: RefRole::Reference, min: 1 }]);
    let veo = video("google/veo-3.1");
    assert_eq!(validate(&veo, &request_x(json!({"foo": 1}))), Err(vec![Invalid::NotOffered("foo".into())]));
    assert_eq!(validate(&veo, &request_x(json!({"duration": 4}))), Err(vec![Invalid::Reserved("duration".into())]));
}
```
(`image`, `video`, `param`, `maybe`, `values`, `request*` are small helpers in the test module.)

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --lib media::spec`
Expected: FAIL to compile, `error[E0433]: failed to resolve: could not find 'media'`.

- [ ] **Step 3: Implement** the types above and the parsers in `spec.rs`. Rules the tests do not fully pin:
  - Picture `supported_parameters`: `aspect_ratio` → Shape, simple, enum; `n` → Count, simple, range step 1, **omitted when max is 1**; `resolution` → Resolution, simple, affects price; `quality` → Quality, simple, affects price; `input_references` → References, simple, `Images{min, max}`, omitted when max is 0; `output_format` → Format, advanced; `background` → Background, advanced; `output_compression` → advanced range, requires `output_format` in `["jpeg","webp"]`; `seed: boolean` → Seed, advanced, `Seed{max: SEED_MAX}`; `size: boolean` → Size with `OPENAI_SIZE_RULES` for `openai/` ids, else an Extra hint; any other key → Extra hint. For `openai/gpt-image-2.6-*` with `gif` in `output_format`: `frame_count` range 2–16 and `frame_duration_ms` range 20–2000 (unit `ms`), both requiring `output_format = "gif"`.
  - Video fields: `supported_aspect_ratios` → `aspect_ratio` (Shape); `supported_durations` → `duration` (Length, numbers, `-1` dropped, ascending, labels "4 s", initial = shortest ≥ 4 else the longest); `supported_resolutions` → `resolution` (initial `720p`, else the lowest by its number); `generate_audio == true` → `generate_audio` (Sound, boolean, company default `true`, affects price); `supported_frame_images` → `first_frame` (StartFrame, simple, `Images{0,1}`) and `last_frame` (EndFrame, advanced); `seed == true` → Seed; `supported_sizes` → `size` (Size, advanced enum; when set, `aspect_ratio` and `resolution` are not sent); `allowed_passthrough_parameters` → `extra` (`Extra{allowed_keys}`). `usable = Some("Needs a video to start from, which Studio does not offer yet.")` when `supported_durations` is null or empty.
  - Display name: catalogue `name` without its `"<Maker>: "` prefix. Family: `<maker>/` + the name cut before the first `-` followed by a digit, or by `v` and a digit.
  - `validate`: param keys must be offered non-picture fields; enum values from the list; ranges within bounds and on the step; seeds integers in `0..=max`; sizes pass `SizeRules` or are in the list; `requires` met; extra keys only from `allowed_keys` and never a typed key, `model` or `prompt` (`Reserved`); pictures per role within `min..=max`; prompt 1–32,000 characters; an unusable model is refused.
  - `cloud::media_company`: `OpenRouter → Some(Company::OpenRouter)`, `OpenAi → Some(Company::OpenAi)`, everything else `None`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --lib media::spec && cargo clippy -p agent --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/media crates/agent/src/lib.rs crates/agent/src/cloud.rs crates/agent/tests/fixtures/media
git commit -m "Studio: media types and controls built from OpenRouter's catalogues"
```

### Task A2: Prices and estimates

**Files:**
- Modify: `crates/agent/src/media/price.rs`, `crates/agent/src/media/spec.rs` (fill `MediaModel.price`)
- Create: `crates/agent/tests/fixtures/media/openrouter-image-endpoints-gpt-image-2.6-flare.json`, `...-gemini-nano-banana-2.1.json`, `...-flux-3-image.json`
- Test: inline in `price.rs`

**Interfaces:**
- Consumes: Task A1 types.
- Produces:
```rust
pub struct PriceChoice { pub count: u32, pub seconds: Option<f64>, pub resolution: Option<String>, pub quality: Option<String>,
    pub audio: Option<bool>, pub start_frame: bool, pub input_images: u32 }
pub fn parse_pricing_skus(skus: &Map<String, Value>) -> Price;
pub fn parse_image_endpoints(answer: &Value, pinned: Option<&str>) -> Price;   // pinned = routing's first company
pub fn estimate(price: &Price, choice: &PriceChoice) -> Option<f64>;          // None = price unknown
pub fn choice_for(model: &MediaModel, req: &MediaRequest) -> PriceChoice;
pub fn endpoint_companies(answer: &Value) -> Vec<String>;                      // company names in an `/endpoints` answer (routing choices)
impl Price { pub fn range(&self) -> Option<(f64, f64)>; }                     // for the picker's price hint
```

- [ ] **Step 0: Record the picture price answers.** `curl -sS https://openrouter.ai/api/v1/images/models/openai/gpt-image-2.6-flare/endpoints` (and the same for `google/gemini-nano-banana-2.1` and `black-forest-labs/flux-3-image`; public, no key) into the three fixture files, with `"_recorded": "<date>"` added at the top. If the host cannot be reached, write the fixtures by hand from the doc page and keep the parser returning `Price::Unknown` for any shape it does not recognise, so every picture run asks first.

- [ ] **Step 1: Write the failing tests**
```rust
fn sku(id: &str) -> Price { /* parse_pricing_skus of that model in the video fixture */ }
fn secs(price: &Price, s: f64, res: &str, audio: Option<bool>, start: bool, images: u32) -> Option<f64>;
fn usd(a: Option<f64>, b: f64) { assert!((a.unwrap() - b).abs() < 1e-9, "{a:?} != {b}"); }

#[test] fn veo_3_1_prices() {
    let p = sku("google/veo-3.1");
    usd(secs(&p, 8.0, "720p", Some(true), false, 0), 3.20);
    usd(secs(&p, 8.0, "4K", Some(true), false, 0), 4.80);
    usd(secs(&p, 8.0, "1080p", Some(false), false, 0), 1.60);
}
#[test] fn the_resolution_row_comes_before_the_plain_one() {
    let p = sku("google/veo-3.1-fast");
    usd(secs(&p, 8.0, "720p", Some(true), false, 0), 0.80);
    usd(secs(&p, 8.0, "1080p", Some(true), false, 0), 0.96);
}
#[test] fn cents_are_cents() {
    usd(secs(&sku("x-ai/grok-imagine-video-1.5-lite"), 4.0, "480p", Some(true), true, 1), 0.09);
    usd(secs(&sku("tbc/mischief-1"), 5.0, "720p", None, false, 0), 0.05);
}
#[test] fn mode_keys_follow_the_start_frame() {
    let p = sku("alibaba/wan-2.6");
    usd(secs(&p, 5.0, "720p", Some(true), false, 0), 0.40);
    usd(secs(&p, 5.0, "720p", Some(true), true, 0), 0.50);
    assert_eq!(secs(&p, 5.0, "480p", Some(true), true, 0), None);
}
#[test] fn sound_keys() {
    let k = sku("kwaivgi/kling-v3.0-pro");
    usd(secs(&k, 5.0, "720p", Some(true), false, 0), 0.84);
    usd(secs(&k, 5.0, "720p", Some(false), false, 0), 0.56);
    assert_eq!(secs(&sku("google/gemini-omni-1.2-flash"), 8.0, "720p", Some(false), false, 0), None);
}
#[test] fn a_minimum_per_run_applies() {
    usd(secs(&sku("runway/aleph-2"), 1.0, "", None, false, 0), 0.56);
    usd(secs(&sku("runway/aleph-2"), 3.0, "", None, false, 0), 0.84);
}
#[test] fn video_tokens_have_no_estimate() {
    assert_eq!(sku("bytedance/seedance-2.5"), Price::PerToken);
    assert_eq!(secs(&sku("bytedance/seedance-2.5"), 5.0, "720p", Some(true), false, 0), None);
}
#[test] fn keys_it_does_not_know_give_unknown() {
    for id in ["heygen/heygen-video-1", "black-forest-labs/flux-video-upscale", "minimax/hailuo-3"] {
        assert_eq!(sku(id), Price::Unknown, "{id}");
    }
}
#[test] fn features_studio_does_not_offer_are_ignored() {
    usd(secs(&sku("black-forest-labs/flux-3-video"), 5.0, "720p", Some(true), false, 0), 0.85);
}
#[test] fn every_catalogue_video_gets_a_price_or_says_why() {
    // all 33 parse without panic; exactly the four ids above and the five seedance ids are not PerSecond
}
#[test] fn picture_prices_from_the_recorded_endpoints() {
    // gpt-image-2.6-flare: a number per picture for (quality, size) or Unknown; nano banana 1K/2K/4K rows;
    // with several companies the highest price is used unless `pinned` names one
}
#[test] fn count_multiplies_and_pictures_in_add() {
    // PerImage row 0.04, count 4, input_images 2 with per_input_image_usd 0.01 → 0.18
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --lib media::price`
Expected: FAIL to compile, `cannot find function 'parse_pricing_skus'`.

- [ ] **Step 3: Implement.** The lookup order for a per-second price (`res` lower-cased: `4K` → `4k`; dollars unless the key starts with `cents_`):
  1. sound on: `duration_seconds_with_audio_{res}`, `duration_seconds_with_audio`; sound off: `duration_seconds_without_audio_{res}`, `duration_seconds_without_audio`;
  2. then, whatever the sound: `{text_to_video|image_to_video}_duration_seconds_{res}` (image-to-video when a start frame is sent), `duration_seconds_{res}`, `duration_seconds`, `cents_per_video_output_second_{res}`, `cents_per_second_output_{res}`, `cents_per_second_output`;
  3. none found → `None` (unknown). `cents_per_image_input` adds per added picture; `minimum_cents_per_generation` is a floor. Keys containing `video_continuation` or `with_video_input` are ignored (features not in 0.1.0); any `video_tokens*` key makes the price `PerToken`; any other key makes the whole price `Unknown`, never a guess.
  - `choice_for`: count from `n` (1 when absent; always 1 for video), seconds from `duration`, resolution from `resolution` or from `size`'s shorter side (`1280x720` → `720p`), quality from `quality`, sound from `generate_audio` or its company default, `start_frame` when a StartFrame picture is sent, `input_images` = pictures sent.
  - `parse_catalog` now sets `price` from `pricing_skus` (video) and from `image_prices[id]` when cached (pictures), else `Price::Unknown`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --lib media`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/media crates/agent/tests/fixtures/media
git commit -m "Studio: read OpenRouter prices and estimate a run, unknown when unsure"
```

### Task A3: Spending guard

**Files:**
- Create: `src-tauri/src/studio/mod.rs` (`pub mod spend;`), `src-tauri/src/studio/spend.rs`
- Modify: `src-tauri/src/lib.rs` (`#[allow(dead_code, reason = "wired up in Task A15")] mod studio;`; the attribute goes in Task A15)
- Test: inline in `spend.rs`

**Interfaces:**
- Consumes: `agent::media::MediaKind`, `chrono`.
- Produces:
```rust
pub const CONFIRM_AT_USD: f64 = 1.0;
pub const DEFAULT_DAILY_LIMIT_USD: f64 = 10.0;
pub const UNKNOWN_PRICE_RESERVE_USD: f64 = 5.0;
pub struct ConfirmRules { pub every_video: bool, pub at_usd: f64 }          // Default: true, CONFIRM_AT_USD
pub enum ConfirmReason { Video, AtLeast(f64), UnknownPrice }               // serde: "video" | "atLeast" | "unknownPrice"
pub fn confirm_reasons(kind: MediaKind, estimate: Option<f64>, rules: &ConfirmRules) -> Vec<ConfirmReason>;
pub enum Charge { Pending, Final, None, Maybe }
pub struct SpendRow { pub created_at: i64, pub estimate_usd: Option<f64>, pub cost_usd: Option<f64>, pub charge: Charge }
pub fn counted_usd(row: &SpendRow) -> f64;
pub struct Spend { pub settled_usd: f64, pub pending_usd: f64 }
pub fn spent_since(rows: &[SpendRow], day_start_ms: i64) -> Spend;
pub enum LimitRefusal { Over { spent: f64, limit: f64 } }
pub fn check_limit(limit: Option<f64>, spend: &Spend, estimate: Option<f64>) -> Result<(), LimitRefusal>;
pub fn day_start_in<Tz: chrono::TimeZone>(tz: &Tz, at_ms: i64) -> i64;
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn every_video_asks() {
    assert_eq!(confirm_reasons(MediaKind::Video, Some(0.09), &ConfirmRules::default()), [ConfirmReason::Video]);
}
#[test] fn a_dollar_or_more_asks() {
    assert!(confirm_reasons(MediaKind::Image, Some(0.99), &ConfirmRules::default()).is_empty());
    assert_eq!(confirm_reasons(MediaKind::Image, Some(1.00), &ConfirmRules::default()), [ConfirmReason::AtLeast(1.0)]);
}
#[test] fn an_unknown_price_always_asks() {
    let lax = ConfirmRules { every_video: false, at_usd: 100.0 };
    assert_eq!(confirm_reasons(MediaKind::Image, None, &lax), [ConfirmReason::UnknownPrice]);
}
#[test] fn dont_ask_again_for_videos_keeps_the_dollar_rule() {
    let r = ConfirmRules { every_video: false, at_usd: 1.0 };
    assert!(confirm_reasons(MediaKind::Video, Some(0.96), &r).is_empty());
    assert_eq!(confirm_reasons(MediaKind::Video, Some(1.20), &r), [ConfirmReason::AtLeast(1.0)]);
}
#[test] fn reaching_the_limit_exactly_is_allowed() {
    let s = Spend { settled_usd: 9.04, pending_usd: 0.0 };
    assert!(check_limit(Some(10.0), &s, Some(0.96)).is_ok());
    assert_eq!(check_limit(Some(10.0), &s, Some(0.97)), Err(LimitRefusal::Over { spent: 9.04, limit: 10.0 }));
}
#[test] fn an_unknown_price_needs_five_dollars_left() {
    assert!(check_limit(Some(10.0), &Spend { settled_usd: 4.0, pending_usd: 1.0 }, None).is_ok());
    assert!(check_limit(Some(10.0), &Spend { settled_usd: 5.01, pending_usd: 0.0 }, None).is_err());
}
#[test] fn no_limit_refuses_nothing() {
    assert!(check_limit(None, &Spend { settled_usd: 1e6, pending_usd: 0.0 }, None).is_ok());
}
#[test] fn what_each_run_counts() {
    assert_eq!(counted_usd(&row(Some(0.5), Some(0.42), Charge::Final)), 0.42);
    assert_eq!(counted_usd(&row(Some(3.0), None, Charge::None)), 0.0);
    assert_eq!(counted_usd(&row(Some(0.96), None, Charge::Pending)), 0.96);
    assert_eq!(counted_usd(&row(Some(0.96), None, Charge::Maybe)), 0.96);
    assert_eq!(counted_usd(&row(None, None, Charge::Pending)), UNKNOWN_PRICE_RESERVE_USD);
}
#[test] fn a_run_counts_on_the_day_it_was_created() {
    let tz = FixedOffset::east_opt(3600).unwrap();
    let late = ms(&tz, 2026, 11, 17, 23, 59, 30);
    let next_day = day_start_in(&tz, ms(&tz, 2026, 11, 18, 0, 0, 10));
    assert_eq!(spent_since(&[row_at(late, Some(0.96), Charge::Pending)], day_start_in(&tz, late)).pending_usd, 0.96);
    assert_eq!(spent_since(&[row_at(late, Some(0.96), Charge::Pending)], next_day).pending_usd, 0.0);
}
#[test] fn the_day_starts_at_local_midnight() {
    for secs in [14 * 3600, -12 * 3600] {
        let tz = FixedOffset::east_opt(secs).unwrap();
        assert_eq!(day_start_in(&tz, ms(&tz, 2026, 11, 17, 13, 5, 0)), ms(&tz, 2026, 11, 17, 0, 0, 0));
    }
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::spend`
Expected: FAIL to compile, `file not found for module 'studio'` / unresolved names.

- [ ] **Step 3: Implement** with these rules: `counted_usd` is the reported cost when there is one (any charge but `None`), 0 for `Charge::None`, else the estimate, else `UNKNOWN_PRICE_RESERVE_USD`. `spent_since` sums rows with `created_at >= day_start_ms`, `Pending` into `pending_usd`, the rest into `settled_usd`. `check_limit` refuses when `settled + pending + estimate > limit + 1e-9`, and for an unknown estimate when `limit - settled - pending < UNKNOWN_PRICE_RESERVE_USD`. `day_start_in` takes the local date of `at_ms`, then midnight's `earliest()` instant, or the first valid instant after it when midnight does not exist in that zone. The app uses `day_start_in(&chrono::Local, now)`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::spend`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/lib.rs
git commit -m "Studio: confirmation rules and the daily limit, counted per local day"
```

### Task A4: A create request sent at most once, and OpenRouter pictures

**Files:**
- Modify: `crates/agent/src/client.rs` (make `same_origin_redirects`, `authorize`, `read_capped`, `network_error` `pub(crate)`; add `post_once`)
- Create: `crates/agent/src/media/openrouter.rs` (catalogues, prices, pictures, preview, failure mapping)
- Modify: `crates/agent/src/media/mod.rs` (`media_provider`)
- Test: `crates/agent/tests/media_endpoints.rs` (new, on `tests/common` `MockServer`)

**Interfaces:**
- Consumes: Task A1 trait and types, Task A2 `parse_image_endpoints`.
- Produces:
```rust
// client.rs
pub(crate) enum OnceBody { Json(Value), Multipart { boundary: String, bytes: Vec<u8> } }
pub(crate) enum OnceReply { Success { path: PathBuf, len: u64, headers: HeaderMap }, Error { status: u16, body: Vec<u8>, headers: HeaderMap } }
impl OpenAiClient {
    /// Sends `body` once; one more try only on 429 with Retry-After <= 30 s, or a connect failure.
    /// A success body is streamed to a new file in `scratch` (at most `cap` bytes).
    pub(crate) async fn post_once(&self, url: Url, body: OnceBody, limit: Duration, cap: u64, scratch: &Path) -> Result<OnceReply, MediaError>;
}
// URLs are built with the existing `api_url()` (`.../api/v1` or `.../v1`) and `join`.
// openrouter.rs
pub struct OpenRouterMedia { /* client, limits */ }
impl OpenRouterMedia { pub fn new(client: OpenAiClient) -> Self; pub fn with_limits(self, limits: MediaLimits) -> Self; }
impl MediaProvider for OpenRouterMedia { /* pictures in this task; check/download in Task A5 */ }
pub(crate) fn openrouter_failure(status: u16, headers: &HeaderMap, body: &[u8], key: Option<&str>) -> MediaFailure;
// mod.rs
pub fn media_provider(client: OpenAiClient) -> Option<Arc<dyn MediaProvider>>;   // OpenRouter here; OpenAI in Task A7
```

- [ ] **Step 1: Write the failing tests** (`crates/agent/tests/media_endpoints.rs`)
```rust
const KEY: &str = "sk-or-v1-studio-test-key-0123456789";
fn openrouter(server: &MockServer) -> OpenRouterMedia;          // ClientConfig::new(server.url() + "/api/v1", OpenRouter), key KEY
fn quick() -> MediaLimits { MediaLimits { picture_timeout: Duration::from_secs(1), response_cap: 64 * 1024, ..Default::default() } }

#[tokio::test] async fn the_catalogues_come_from_the_endpoints_own_client() {
    // GET /api/v1/images/models and /api/v1/videos/models → RawCatalog with both; Authorization: Bearer KEY sent to this host only
}
#[tokio::test] async fn a_picture_run_decodes_every_picture() {
    // reply 200 {"data":[{"b64_json":PNG_A,"media_type":"image/png"},{"b64_json":PNG_B}],"usage":{"cost":0.08}}
    let Started::Done { outputs, cost } = media.start(&req_with_ref(), dir.path()).await.unwrap() else { panic!() };
    assert_eq!(outputs.len(), 2);
    assert_eq!(std::fs::read(&outputs[0].path).unwrap(), PNG_A_BYTES);
    assert_eq!(cost, Some(Cost { usd: 0.08, reported: true }));
    let sent = server.requests()[0].json();
    assert_eq!(sent["model"], "openai/gpt-image-2.6-flare");
    assert_eq!(sent["n"], 2);
    assert!(sent["input_references"][0]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert_eq!(server.requests()[0].path(), "/api/v1/images");
}
#[tokio::test] async fn a_500_is_never_sent_again() {
    assert!(matches!(media.start(&req(), dir).await, Err(MediaError::Unknown(_))));
    assert_eq!(server.requests().len(), 1);
}
#[tokio::test] async fn a_timeout_is_never_sent_again()     { /* Reply::Silence(3 s), limit 1 s → Unknown, 1 request */ }
#[tokio::test] async fn a_cut_connection_is_never_sent_again() { /* Reply::Stream{end: Abort} → Unknown, 1 request */ }
#[tokio::test] async fn a_429_with_a_short_wait_is_sent_once_more() {
    // WithHeaders 429 retry-after "1", then 200 → Ok; server.requests().len() == 2
}
#[tokio::test] async fn a_429_with_a_long_wait_is_not_sent_again() {
    // retry-after "31" → Err(Answered(f)) with f.kind == RateLimited and f.retry_after == Some(31 s); 1 request
}
#[tokio::test] async fn a_refused_connection_was_not_sent() {
    // base URL on a port with nothing listening → Err(MediaError::NotSent(_))
}
#[tokio::test] async fn a_502_was_not_charged() {
    // 502 → Answered(MediaFailure { kind: Server, charged: Some(false), .. })
}
#[tokio::test] async fn a_moderation_error_is_blocked() {
    // 403 {"error":{"code":403,"message":"Your prompt was flagged by moderation","metadata":{"reasons":["violence"]}}}
    // → Answered(f): f.kind == Blocked, f.charged == Some(false), f.status == Some(403)
}
#[tokio::test] async fn the_key_never_appears_in_an_error() {
    // 400 body echoing KEY → f.message contains "[API key]" and not KEY
}
#[tokio::test] async fn a_body_over_the_cap_is_refused_and_its_file_removed() {
    // 200 with 100 KiB body, cap 64 KiB → Err(Unknown(Decode(..))); dir has no file left
}
#[tokio::test] async fn the_preview_hides_pictures_and_the_key() {
    let p = media.preview(&req_with_ref());
    assert_eq!(p.host, "127.0.0.1");
    assert!(p.body["input_references"][0].as_str().unwrap().starts_with("picture 1 (sha "));
    assert!(!p.body.to_string().contains(KEY));
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --test media_endpoints`
Expected: FAIL to compile, `cannot find type 'OpenRouterMedia' in module 'agent::media'`.

- [ ] **Step 3: Implement.**
  - `post_once`: build the request with `authorize`, `limit` as the whole-request timeout, no `retry_delay`. Error mapping: `reqwest::Error::is_connect()` → retry once after 1 s, then `NotSent`; timeout, a body read that fails, or any other send error → `Unknown`; a 429 with `cloud::parse_retry_after` ≤ 30 s → wait and send once more; any other non-2xx → `OnceReply::Error` (body read with `read_capped(.., MAX_ERROR_BODY, true)`); 2xx → stream chunks into `scratch/<uuid>.part`, removing the file and returning `Unknown(Decode)` past `cap`. Every error text goes through `cloud::scrub_key`.
  - Picture body: `model`, `prompt`, every `params` entry, every `extra` entry at the top level, `input_references: [{"type":"image_url","image_url":{"url":"data:<mime>;base64,..."}}]`, and `provider: {order, only?, allow_fallbacks, zdr}` when `routing` is set. Response read with a typed struct (`data: Vec<{b64_json: String, media_type: Option<String>}>`, `usage.cost`), one picture decoded and written at a time to `scratch/<uuid>-<n>`; MIME from `media_type`, else from the bytes.
  - `fetch_catalog`: `GET {api}/images/models` and `{api}/videos/models` through the endpoint's client. `fetch_image_price`: `GET {api}/images/models/{id}/endpoints`.
  - `openrouter_failure`: 401 `Unauthorized`; 402 `NoCredit`; 403 with "moderation"/"flagged" or a `moderation` metadata object, or 400 with the same → `Blocked`, `charged: Some(false)`; 429 `RateLimited` with `retry_after`; 404 `NotFound`; 502 `Server` with `charged: Some(false)` ("failed, not charged"); other 5xx `Server`; `request_id` from the body's `id` or the `x-request-id` header; `message` cut by `error::bounded` and scrubbed.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --test media_endpoints && cargo test -p agent --lib`
Expected: PASS (the existing client tests still pass).

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/client.rs crates/agent/src/media crates/agent/tests/media_endpoints.rs
git commit -m "Studio: OpenRouter pictures, with the create request sent at most once"
```

### Task A5: OpenRouter video jobs and streamed downloads

**Files:**
- Modify: `crates/agent/src/client.rs` (`download_to_file`, `get_json_within`), `crates/agent/src/media/openrouter.rs`
- Test: `crates/agent/tests/media_endpoints.rs`; Create: `crates/agent/tests/media_redirects.rs` (one test, sets `NO_PROXY=*` before any thread, like `tests/redirects.rs`)

**Interfaces:**
- Consumes: Task A4.
- Produces:
```rust
impl OpenAiClient {
    pub(crate) async fn get_json_within(&self, url: Url, limit: Duration) -> Result<Value, ProviderError>;
    /// Streams to `dest` (at most `cap` bytes). A 3xx whose Location is another origin comes back
    /// as `Fetched::Elsewhere` and is never followed with the key.
    pub(crate) async fn download_to_file(&self, url: Url, dest: &Path, cap: u64) -> Result<Fetched, MediaError>;
}
// OpenRouterMedia: start (video) → Started::Job; check; download
```

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn a_video_job_starts_checks_and_downloads() {
    // POST /api/v1/videos → 202 {"id":"vid_1","polling_url":"...","status":"pending"}
    assert_eq!(media.start(&video_req(), dir).await.unwrap(), Started::Job { remote_id: "vid_1".into() });
    // GET /api/v1/videos/vid_1: pending → Pending; in_progress {"progress":0.4} → Running{progress: Some(0.4)};
    // completed {"usage":{"cost":0.36},"unsigned_urls":["..."]} → Done(JobDone { outputs: 1, cost: Some(Cost{usd:0.36,reported:true}), .. })
    // download(…, 0, dest) → GET /api/v1/videos/vid_1/content?index=0 (video/mp4) → Fetched::Saved { mime: "video/mp4", .. }
    let sent = server.requests()[0].json();
    assert_eq!(sent["duration"], 4);
    assert_eq!(sent["frame_images"][0]["frame_type"], "first_frame");
}
#[tokio::test] async fn a_failed_job_says_whether_it_was_charged() { /* status failed + error → Failed(f), f.charged == Some(false) */ }
#[tokio::test] async fn a_job_the_company_no_longer_has_is_gone() { /* 404 on check → JobState::Gone */ }
#[tokio::test] async fn a_check_that_times_out_is_an_error_not_a_state() { /* Silence, check_timeout 1 s → Err(Unknown) */ }
#[tokio::test] async fn content_gone_after_done_is_expired() {
    // 404 and 410 on /content → Err(Answered(f)) with f.kind == FailureKind::Expired
}
#[tokio::test] async fn a_download_over_the_cap_removes_the_partial_file() {
    // video_cap 1 KiB, content 4 KiB → Err(Answered(f)) kind Other "larger than", dest does not exist
}
#[tokio::test] async fn a_link_to_another_host_is_handed_back_not_followed() {
    // /content → 302 Location: http://127.0.0.1:<other>/v.mp4 → Ok(Fetched::Elsewhere(url)); `other` saw no request
}
#[tokio::test] async fn signed_urls_are_kept_for_the_guarded_fetcher() {
    // completed with "urls":[{"url":"https://cdn.example/v.mp4","expires_at":1794900000}] → JobDone.signed[0]
}
// media_redirects.rs (loopback = false):
#[test] fn a_redirect_to_another_origin_carries_no_key() {
    // content → 307 to another port → Fetched::Elsewhere; the other server recorded 0 requests
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --test media_endpoints video && cargo test -p agent --test media_redirects`
Expected: FAIL (`start` for video returns an error / no such method).

- [ ] **Step 3: Implement.** Before writing the body, read https://openrouter.ai/docs/guides/overview/multimodal/video-generation and pin the exact `frame_images` item and where passthrough fields go in the test's expected body (defaults used here: `{"type":"image_url","image_url":{"url":"data:…"},"frame_type":"first_frame"|"last_frame"}`; passthrough fields at the top level). Video body: `model`, `prompt`, `duration`, `resolution`, `aspect_ratio` or `size`, `generate_audio`, `seed`, `frame_images`, extra fields, `provider`. Status mapping: `pending` → `Pending`; `in_progress` → `Running{progress}` (a 0–1 number when present, a 0–100 number divided by 100); `completed` → `Done` (outputs = `max(len(unsigned_urls), len(urls), 1)`); `failed`, `cancelled`, `expired` → `Failed` (`expired` with kind `Expired`); 404 → `Gone`. Each check uses `get_json_within(limits.check_timeout)`. `download` prefers `GET {api}/videos/{id}/content?index=N` with the key; when only `signed` URLs exist it returns `Fetched::Elsewhere(signed[N].url)`; 404/410 on content → `Expired`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent --test media_endpoints && cargo test -p agent --test media_redirects`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src/client.rs crates/agent/src/media/openrouter.rs crates/agent/tests/media_endpoints.rs crates/agent/tests/media_redirects.rs
git commit -m "Studio: OpenRouter video jobs, checked and downloaded without following foreign links"
```

### Task A6: Guarded download of a result file (no key)

**Files:**
- Modify: `crates/search/src/fetch.rs`, `crates/search/Cargo.toml` (`tempfile` dev-dependency, already in the lock)
- Test: `crates/search/tests/fetch.rs`

**Interfaces:**
- Produces:
```rust
pub const MEDIA_TYPES: &[&str] = &["image/png","image/jpeg","image/webp","image/gif","image/svg+xml","video/mp4","video/webm"];
pub struct FetchedFile { pub url: Url, pub content_type: String, pub len: u64, pub redirects: usize }
impl Fetcher {
    /// The same address rules as `get`, binary, streamed to `dest`, at most `cap` bytes; never sends a key.
    pub async fn fetch_to_file(&self, url: &Url, cap: u64, dest: &Path) -> Result<FetchedFile, FetchError>;
}
// FetchError gains: NotMedia(String), TooLarge(u64), Disk(String)
```

- [ ] **Step 1: Write the failing tests** (`crates/search/tests/fetch.rs`, with `with_test_loopback_ports`)
```rust
#[tokio::test] async fn fetch_to_file_saves_a_video() { /* 200 video/mp4 → file bytes equal, len matches */ }
#[tokio::test] async fn fetch_to_file_never_sends_authorization() { /* recorded request has no Authorization header */ }
#[tokio::test] async fn fetch_to_file_refuses_a_private_address() {
    let err = Fetcher::new("t").fetch_to_file(&url("http://127.0.0.1:9/v.mp4"), 1 << 20, &dest).await.unwrap_err();
    assert!(err.is_refusal()); assert!(!dest.exists());
}
#[tokio::test] async fn fetch_to_file_refuses_a_page() { /* text/html → FetchError::NotMedia("text/html"), no file */ }
#[tokio::test] async fn fetch_to_file_stops_at_the_cap() { /* 4 KiB body, cap 1 KiB → TooLarge(1024), no file */ }
#[tokio::test] async fn fetch_to_file_follows_redirects_by_hand() { /* allowed port → allowed port: saved; → 10.0.0.1: Blocked */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p search --test fetch fetch_to_file`
Expected: FAIL to compile, `no method named 'fetch_to_file'`.

- [ ] **Step 3: Implement** by reusing `check`, `client_for` and `redirect_target` per hop, with `Accept: <MEDIA_TYPES joined>` and `Accept-Encoding: identity`; the media type must be in `MEDIA_TYPES`; chunks go to `dest.with_extension("part")`, renamed to `dest` at the end, removed on any error; `std::io::ErrorKind::StorageFull` → `Disk`. The caller sets the time limit with `with_timeout` (Studio uses 10 minutes).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p search --test fetch`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/search/src/fetch.rs crates/search/Cargo.toml crates/search/tests/fetch.rs Cargo.lock
git commit -m "Studio: download a result from a company's file host through the guarded fetcher"
```

### Task A7: OpenAI pictures

**Files:**
- Create: `crates/agent/src/media/openai.rs`, `crates/agent/src/media/multipart.rs`, `crates/agent/src/media/specs/openai_images.json`
- Create: `crates/agent/tests/fixtures/media/openai-models.json`, `openai-generation.json`, `openai-moderation-blocked.json`, `openai-org-verification.json` (shapes from OpenAI's API reference; replaced by recorded answers in the owner's real run)
- Modify: `crates/agent/src/cloud.rs` (`openai_media_kind`), `crates/agent/src/media/mod.rs` (`media_provider` covers OpenAI), `crates/agent/src/media/spec.rs` (`parse_catalog` for OpenAI), `crates/agent/src/media/price.rs` (`openai_cost`)
- Test: inline (`openai.rs`, `multipart.rs`, `cloud.rs`) and `crates/agent/tests/media_endpoints.rs`

**Interfaces:**
- Produces:
```rust
// cloud.rs: pub(crate) fn openai_media_kind(id: &str) -> Option<MediaKind>;
pub struct OpenAiImages; impl OpenAiImages { pub fn new(client: OpenAiClient) -> Self; pub fn with_limits(self, l: MediaLimits) -> Self; }
pub struct OpenAiRates { pub text_input: Option<f64>, pub image_input: f64, pub image_output: f64 }   // dollars per 1M tokens
pub fn openai_cost(usage: &Value, rates: &OpenAiRates) -> Option<f64>;
pub(crate) fn openai_failure(status: u16, headers: &HeaderMap, body: &[u8], key: Option<&str>) -> MediaFailure;
pub fn check_size(size: &str, rules: &SizeRules) -> Result<(), SizeProblem>;   // SizeProblem: NotWxH, MultipleOf, Ratio, LongestSide, TooFewPixels, TooManyPixels
// multipart.rs: pub(crate) fn form(fields: &[(&str, String)], files: &[FilePart]) -> (String, Vec<u8>);   // (boundary, body)
```
`openai_images.json` shape: `{"checked":"2026-11-17","sources":[…],"rates_per_million":{"text_input":null,"image_input":8.0,"image_output":30.0},"models":[{"prefix","label","note","retires","maxN","maxRefs","qualities","formats","svg","gif","perImage":{"<quality>|<size>":usd}}]}`, led by `gpt-image-2.6-sunburst` and `gpt-image-2.6-flare`, then `gpt-image-2` (retires `2027-02-10`), `gpt-image-1.5`, `gpt-image-1-mini`, `chatgpt-image-latest` (retire `2026-12-01`). `perImage` holds only figures OpenAI's image guide and pricing page state on the day; anything missing stays unknown (asks first).

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn openai_media_kind_is_the_reverse_of_chat() {
    for id in ["gpt-image-2.6-flare", "gpt-image-2.6-sunburst", "gpt-image-1.5", "chatgpt-image-latest"] {
        assert_eq!(openai_media_kind(id), Some(MediaKind::Image)); assert!(!openai_can_chat(id));
    }
    for id in ["gpt-5.5", "dall-e-3", "sora-2"] { assert_eq!(openai_media_kind(id), None); }
}
#[test] fn the_bundled_spec_leads_with_2_6() {
    let s = bundled(); assert_eq!(s.checked, "2026-11-17");
    assert_eq!([s.models[0].prefix.as_str(), s.models[1].prefix.as_str()], ["gpt-image-2.6-sunburst", "gpt-image-2.6-flare"]);
}
#[test] fn the_longest_prefix_wins_and_a_new_model_gets_basic_fields() {
    assert_eq!(match_spec("gpt-image-2.6-flare-2026-11-04").unwrap().prefix, "gpt-image-2.6-flare");
    let m = model_for("gpt-image-3");
    assert_eq!(keys(&m), ["size", "quality", "n"]);
    assert!(m.notes.contains(&"Settings may not match this new model.".to_string()));
}
#[test] fn custom_sizes_are_checked() {
    assert!(check_size("1536x1024", &OPENAI_SIZE_RULES).is_ok());
    assert!(check_size("3840x2160", &OPENAI_SIZE_RULES).is_ok());
    assert_eq!(check_size("1000x1000", &OPENAI_SIZE_RULES), Err(SizeProblem::MultipleOf));
    assert_eq!(check_size("3840x1024", &OPENAI_SIZE_RULES), Err(SizeProblem::Ratio));
    assert_eq!(check_size("4096x2048", &OPENAI_SIZE_RULES), Err(SizeProblem::LongestSide));
    assert_eq!(check_size("512x512", &OPENAI_SIZE_RULES), Err(SizeProblem::TooFewPixels));
}
#[test] fn svg_is_one_picture_and_never_an_edit() { /* validate n=2 + svg → Err; svg + a reference → Err */ }
#[test] fn a_multipart_body_has_each_part() { /* boundary not in any part; "Content-Disposition: form-data; name=\"image[]\"; filename=\"picture-1.png\"" */ }
#[test] fn cost_from_usage() {
    let u = json!({"input_tokens_details":{"text_tokens":0,"image_tokens":1000},"output_tokens":2000});
    assert!((openai_cost(&u, &rates()).unwrap() - 0.068).abs() < 1e-9);       // 1000×8/1e6 + 2000×30/1e6
    assert_eq!(openai_cost(&json!({"input_tokens_details":{"text_tokens":20},"output_tokens":1}), &rates()), None);  // no text rate
}
// media_endpoints.rs
#[tokio::test] async fn an_openai_generation_is_one_json_post() { /* POST /v1/images/generations; body model/prompt/n/size/quality; data[0].b64_json decoded */ }
#[tokio::test] async fn an_openai_edit_is_one_multipart_post() { /* with a reference: POST /v1/images/edits, content-type multipart/form-data; boundary=…, 1 request */ }
#[tokio::test] async fn moderation_blocked_is_explained() {
    // 400 {"error":{"code":"moderation_blocked","message":"…","moderation_details":{…}}} + x-request-id req_8f2
    // → Answered(f): f.kind == Blocked, f.company_code == Some("moderation_blocked"), f.request_id == Some("req_8f2")
}
#[tokio::test] async fn organization_verification_is_named() { /* 403 "…organization must be verified…" → kind OrgVerification */ }
#[tokio::test] async fn the_openai_list_keeps_picture_models_only() { /* /v1/models fixture → ids gpt-image-2.6-flare, gpt-image-2.6-sunburst */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p agent --lib media && cargo test -p agent --lib cloud::tests::openai_media_kind && cargo test -p agent --test media_endpoints openai`
Expected: FAIL to compile, `cannot find type 'OpenAiImages'`.

- [ ] **Step 3: Implement.** `fetch_catalog` = `GET {api}/models` (key needed) as `RawCatalog{images: list, videos: None}`; `parse_catalog(OpenAi, …)` keeps ids with `openai_media_kind == Some(Image)` and builds params from the longest matching prefix: `size` (enum `auto`, `1024x1024`, `1536x1024`, `1024x1536`, plus Size rules in Advanced), `quality`, `n` (1–`maxN`, chips stop at 4), `output_format` (adds `svg`/`gif` when the entry says so), `output_compression` (requires jpeg/webp), `background`, `moderation` (Auto or Low; help "Low makes OpenAI's filter less strict; it still refuses what its rules forbid."), `frame_count`/`frame_duration_ms` (gif), `image[]` references up to `maxRefs`. A retiring model gets the note "OpenAI retires this model on <date>." Price: `PerImage` rows from `perImage`. With no references the run is `POST {api}/images/generations` (JSON); with references, `POST {api}/images/edits` (multipart, files as `image[]`). Actual cost: `openai_cost(usage, rates)`, `reported: false`. `openai_failure`: `moderation_blocked` → `Blocked` (stage from `moderation_details` when it says input or output, else none; `charged: None`); 403 whose message mentions organization verification → `OrgVerification`; 401/402/429/5xx as in Task A4; `request_id` from `x-request-id`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p agent`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/agent/src crates/agent/tests
git commit -m "Studio: OpenAI pictures and edits from a dated settings file"
```

### Task A8: Storage (schema V3)

**Files:**
- Modify: `crates/store/src/schema.rs` (V3 exactly as in the design §5, appended), `crates/store/src/lib.rs`, `crates/store/src/error.rs` (`TooManyCreations`)
- Create: `crates/store/src/creations.rs`
- Test: inline in `schema.rs` and `creations.rs`

**Interfaces:**
- Produces (`store`):
```rust
sql_enum!(CreationKind { Image => "image", Video => "video" });
sql_enum!(CreationStatus { Queued, Sending, Waiting, Downloading, Done, Failed, Blocked, Cancelled, Interrupted });  // lower-case names
pub const MAX_CREATION_RUNS: usize = 20_000;
pub struct NewCreationRun { pub id: String, pub kind: CreationKind, pub provider_id: Option<ProviderId>, pub provider_name: String,
    pub provider_host: String, pub model: String, pub model_label: String, pub prompt: String, pub params: Value, pub refs: Value, pub estimate_usd: Option<f64> }
pub struct CreationPatch { pub status: Option<CreationStatus>, pub remote_id: Option<Option<String>>, pub progress: Option<Option<f64>>,
    pub cost_usd: Option<Option<f64>>, pub error: Option<Option<Value>>, pub audit_id: Option<Option<AuditId>>,
    pub started_at: Option<Option<i64>>, pub finished_at: Option<Option<i64>> }
pub struct CreationOutput { pub id: String, pub run_id: String, pub idx: u32, pub path: String, pub thumb_path: Option<String>, pub mime: String,
    pub size_bytes: u64, pub width: Option<u32>, pub height: Option<u32>, pub duration_ms: Option<u64>, pub seed: Option<u64>, pub starred: bool, pub created_at: i64 }
pub struct NewCreationOutput { /* CreationOutput without starred and created_at */ }
pub struct CreationRun { /* every creation_runs column, params/refs/error as Value */ pub outputs: Vec<CreationOutput> }
pub struct CreationQuery { pub kind: Option<CreationKind>, pub starred: bool, pub text: Option<String>, pub model: Option<String>,
    pub since: Option<i64>, pub until: Option<i64>, pub before: Option<i64>, pub limit: u32 }
pub struct CreationUsage { pub pictures: u64, pub videos: u64, pub bytes: u64 }
impl Store {
    pub async fn insert_creation_run(&self, run: NewCreationRun) -> Result<(), StoreError>;     // TooManyCreations at the cap
    pub async fn update_creation_run(&self, id: &str, patch: CreationPatch) -> Result<(), StoreError>;
    pub async fn get_creation_run(&self, id: &str) -> Result<Option<CreationRun>, StoreError>;
    pub async fn list_creation_runs(&self, q: CreationQuery) -> Result<Vec<CreationRun>, StoreError>;  // newest first
    pub async fn live_creation_runs(&self) -> Result<Vec<CreationRun>, StoreError>;           // queued, sending, waiting, downloading
    pub async fn creation_runs_since(&self, since_ms: i64) -> Result<Vec<CreationRun>, StoreError>;
    pub async fn insert_creation_output(&self, out: NewCreationOutput) -> Result<(), StoreError>;
    pub async fn get_creation_output(&self, id: &str) -> Result<Option<CreationOutput>, StoreError>;
    pub async fn set_creation_thumb(&self, id: &str, thumb_path: &str) -> Result<(), StoreError>;
    pub async fn set_creation_starred(&self, id: &str, starred: bool) -> Result<(), StoreError>;
    pub async fn delete_creation_outputs(&self, ids: Vec<String>) -> Result<Vec<CreationOutput>, StoreError>;  // a finished run left with none goes too
    pub async fn delete_creation_runs(&self, ids: Vec<String>) -> Result<Vec<CreationOutput>, StoreError>;
    pub async fn unfinished_creation_run_ids(&self) -> Result<Vec<String>, StoreError>;      // failed, blocked, cancelled, interrupted
    pub async fn creation_usage(&self) -> Result<CreationUsage, StoreError>;
}
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn version_3_adds_the_studio_tables_to_a_version_2_database() {
    // Migrations(&[V1, V2]).to_latest; insert a provider and a chat; MIGRATIONS.to_latest;
    // user_version == 3; both tables exist; the provider and chat rows are unchanged
}
#[tokio::test] async fn deleting_an_endpoint_keeps_its_runs_with_their_names() {
    // run with provider_id 1; delete_provider(1); run.provider_id == None; provider_name and provider_host unchanged
}
#[tokio::test] async fn deleting_a_run_deletes_its_outputs() { /* delete_creation_runs → outputs gone, returned for file removal */ }
#[tokio::test] async fn a_status_outside_the_list_is_refused() { /* raw UPDATE status = 'generating' fails the CHECK */ }
#[tokio::test] async fn runs_list_newest_first_with_filters() {
    // kind, starred (a run with a starred output), text (LIKE with % and _ escaped), model, before cursor, limit
}
#[tokio::test] async fn live_runs_are_the_four_working_states() {}
#[tokio::test] async fn the_last_output_takes_its_finished_run_along() {}
#[tokio::test] async fn the_cap_is_twenty_thousand_runs() {
    assert_eq!(MAX_CREATION_RUNS, 20_000);
    // 20,000 rows through one INSERT … SELECT over a recursive CTE, then insert_creation_run → Err(StoreError::TooManyCreations)
}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p store creations && cargo test -p store schema`
Expected: FAIL to compile, `no method named 'insert_creation_run'`.

- [ ] **Step 3: Implement** with the existing helpers (`sql_enum!`, `JsonColumn`, `json_text`, `now_ms`, `like_contains`, `sql_limit`, `update_columns`). The insert checks the count inside the same transaction. JSON columns are written with `json_text` so `json_valid` holds.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p store`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add crates/store/src
git commit -m "Studio: schema V3 with runs and their outputs"
```

### Task A9: The gallery folder and added pictures

**Files:**
- Create: `src-tauri/src/studio/gallery.rs`
- Modify: `src-tauri/src/studio/mod.rs`, `src-tauri/src/attachments.rs` (`sniff`, `checked_picture`, `PictureWorker`, `ImageInput` become `pub(crate)`)
- Test: inline in `gallery.rs`

**Interfaces:**
- Consumes: `attachments::{checked_picture, PictureWorker, ImageInput, MAX_IMAGE_BYTES}`, `store::CreationRun`.
- Produces:
```rust
pub struct Gallery { root: PathBuf }
impl Gallery {
    pub fn new(root: PathBuf) -> Self;
    pub fn default_root(home: &Path, app_name: &str) -> PathBuf;                  // <home>/<app_name>/studio
    pub fn partial_dir(&self) -> PathBuf;                                         // <root>/.partial (created)
    pub fn output_path(&self, created_ms: i64, run_id: &str, idx: u32, ext: &str) -> PathBuf;   // <root>/YYYY-MM/<run>-<idx>.<ext>
    pub fn sidecar_path(&self, created_ms: i64, run_id: &str) -> PathBuf;          // <root>/YYYY-MM/<run>.json
    pub fn thumb_path(&self, output_id: &str) -> PathBuf;                         // <root>/.thumbs/<id>.jpg
    pub fn finalize(&self, partial: &Path, claimed: Option<&str>, target_no_ext: &Path, cap: u64) -> Result<(PathBuf, String), GalleryError>;
    pub fn write_sidecar(&self, run: &store::CreationRun) -> Result<(), GalleryError>;
    pub async fn add_ref(&self, worker: &PictureWorker, img: &ImageInput) -> AppResult<RefStored>;
    pub fn ref_file(&self, sha256: &str) -> Result<(PathBuf, String), GalleryError>;   // refuses a name that is not 64 hex characters
    pub fn remove_files(&self, outputs: &[store::CreationOutput]);                // outputs, thumbs, and a sidecar left alone
}
pub struct RefStored { pub sha256: String, pub mime: String, pub size_bytes: u64 }
pub fn sniff_media(bytes: &[u8]) -> Option<&'static str>;
pub enum GalleryError { DiskFull, TooLarge(u64), NotMedia, Mismatch { claimed: String, found: String }, BadName, Io(String) }
pub fn io_error(e: std::io::Error) -> GalleryError;                               // StorageFull → DiskFull
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn files_go_under_their_month() {
    let g = Gallery::new("/g".into());
    assert_eq!(g.output_path(ms(2026, 11, 17), "r1", 0, "png"), Path::new("/g/2026-11/r1-0.png"));
    assert_eq!(g.sidecar_path(ms(2026, 11, 17), "r1"), Path::new("/g/2026-11/r1.json"));
}
#[test] fn the_default_folder_is_next_to_the_models() {
    assert_eq!(Gallery::default_root(Path::new("/home/u"), "harnessUI"), Path::new("/home/u/harnessUI/studio"));
}
#[test] fn the_type_is_read_from_the_bytes() {
    assert_eq!(sniff_media(PNG), Some("image/png"));   // also JPEG, WebP, GIF, SVG, MP4 (ftyp at 4), WebM (1A 45 DF A3)
    assert_eq!(sniff_media(b"<!doctype html><p>"), None);
}
#[test] fn a_file_that_is_not_what_it_claims_is_refused_and_removed() {
    // partial holds HTML, claimed "video/mp4" → Err(NotMedia); partial no longer exists
    // partial holds PNG, claimed "video/mp4" → Err(Mismatch{..})
}
#[test] fn a_full_disk_is_named() {
    assert!(matches!(io_error(std::io::Error::from(std::io::ErrorKind::StorageFull)), GalleryError::DiskFull));
}
#[test] fn the_sidecar_is_the_run_record() { /* JSON has id, prompt, params, outputs; no "Authorization", no "api_key" */ }
#[test] fn a_reference_name_that_is_not_a_hash_is_refused() {
    assert!(matches!(Gallery::new(tmp()).ref_file("../../etc/passwd"), Err(GalleryError::BadName)));
}
#[tokio::test] async fn references_are_kept_once_by_hash() { /* add_ref twice → same refs/<sha>.png, one file */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::gallery`
Expected: FAIL to compile, `cannot find type 'Gallery'`.

- [ ] **Step 3: Implement.** `finalize` sniffs the first bytes, refuses a claimed type that differs (a WebP called `image/png` is a mismatch too), refuses anything outside the seven types, renames into `<target>.<ext>` (ext from the sniffed type) and removes the partial on every refusal. `add_ref` runs `checked_picture` (20 MiB, sniff, WebP → JPEG/PNG) and writes `refs/<sha>.<ext>` through a `.tmp` and rename, like `save_images`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::gallery`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/attachments.rs
git commit -m "Studio: gallery folder layout, type checks, sidecars and added pictures"
```

### Task A10: Thumbnails in the picture worker

**Files:**
- Modify: `src-tauri/src/attachments.rs` (thumbnail mode), `src-tauri/src/main.rs` (pass the mode argument)
- Test: inline in `attachments.rs` (its existing `test_worker` helper)

**Interfaces:**
- Produces:
```rust
pub const THUMB_MODE: &str = "thumb";
pub const THUMB_SIDE: u32 = 512;
pub fn picture_worker_main(mode: Option<&str>) -> !;             // None: today's WebP conversion; Some("thumb"): a thumbnail
pub(crate) fn thumbnail(bytes: &[u8]) -> Result<Vec<u8>, NotConverted>;   // PNG, JPEG or WebP in, JPEG out, longest side 512
impl PictureWorker { pub(crate) fn thumbs() -> Self; pub(crate) async fn thumbnail(&self, bytes: &[u8]) -> Result<Vec<u8>, NotConverted>; }
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn a_thumbnail_is_a_jpeg_at_most_512_px() {
    let jpg = thumbnail(&png(1536, 1024)).unwrap();
    assert!(jpg.starts_with(&[0xFF, 0xD8, 0xFF]));
    let (w, h) = image::load_from_memory(&jpg).unwrap().dimensions();
    assert_eq!((w, h), (512, 341));
}
#[test] fn a_small_picture_is_not_enlarged() { /* 300x200 → 300x200 */ }
#[tokio::test] async fn the_worker_makes_thumbnails() { /* test_worker("thumb").thumbnail(png) → JPEG */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib attachments::tests::a_thumbnail`
Expected: FAIL to compile, `cannot find function 'thumbnail'`.

- [ ] **Step 3: Implement.** The worker keeps its 1 GiB memory cap and 60 s limit; in thumbnail mode it reads at most 50 MiB, decodes with the existing size checks (at most 8,192 px a side), resizes with `image::imageops::thumbnail`, and writes JPEG at quality 80 after `PICTURE_MARK`. `main.rs` passes `args.next().as_deref()`. GIF and SVG get no thumbnail (the grid shows the original).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib attachments && bash scripts/check-spawn.sh`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/attachments.rs src-tauri/src/main.rs
git commit -m "Studio: 512 px thumbnails made in the picture worker"
```

### Task A11: Saved model lists (Test, Update, delete)

**Files:**
- Create: `src-tauri/src/studio/catalog.rs`
- Modify: `src-tauri/src/providers.rs` (`test_provider` also refreshes media lists, failures only logged; `delete_provider` calls `catalog::forget`; `may_hold_key` `pub(crate)`; the client-building half of `client_for` becomes `pub(crate) fn client_from(provider: &store::Provider, key: Option<String>) -> AppResult<OpenAiClient>` so Task A13's `StoreProviders` builds clients the same way)
- Test: inline in `catalog.rs`

**Interfaces:**
- Consumes: `agent::media::{MediaProvider, RawCatalog, parse_catalog, media_company}`, `store::Store` settings.
- Produces:
```rust
pub fn cache_key(id: store::ProviderId) -> String;                        // "providerMediaModels:<id>"
#[derive(Serialize, Deserialize)] pub struct CachedCatalog { pub fetched_at: i64, pub raw: RawCatalog, pub image_prices: BTreeMap<String, CachedPrice> }
pub struct CachedPrice { pub fetched_at: i64, pub raw: Value }
pub async fn refresh(store: &store::Store, id: store::ProviderId, media: &dyn MediaProvider, now: i64) -> Result<(), MediaError>;
pub async fn refresh_price(store: &store::Store, id: store::ProviderId, media: &dyn MediaProvider, model: &str, now: i64) -> Result<(), MediaError>;
pub async fn forget(store: &store::Store, id: store::ProviderId);
pub struct EndpointModels { pub provider: store::Provider, pub company: Company, pub host: String, pub cloud: bool,
    pub has_key: bool, pub fetched_at: Option<i64>, pub models: Vec<MediaModel> }
/// From the saved lists only: never builds a client or touches the network.
pub async fn models_for(store: &store::Store, providers: &[store::Provider], has_key: impl Fn(store::ProviderId) -> bool) -> Vec<EndpointModels>;
pub async fn model(store: &store::Store, id: store::ProviderId, model: &str) -> Option<(EndpointModels, MediaModel)>;
```

- [ ] **Step 1: Write the failing tests**
```rust
#[tokio::test] async fn models_come_from_the_saved_lists_only() {
    // save CachedCatalog from the two fixtures for an "openrouter" provider; models_for → 101 models, fetched_at Some
}
#[tokio::test] async fn an_endpoint_without_a_list_offers_to_get_it() { /* fetched_at None, models empty */ }
#[tokio::test] async fn only_openrouter_and_openai_endpoints_are_listed() { /* anthropic, generic, ollama left out */ }
#[tokio::test] async fn a_saved_picture_price_is_used() { /* image_prices entry → model.price is PerImage */ }
#[tokio::test] async fn a_failed_refresh_keeps_the_old_list() { /* fake provider fails → Err, cache unchanged */ }
#[tokio::test] async fn forget_removes_the_list() {}
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::catalog`
Expected: FAIL to compile.

- [ ] **Step 3: Implement.** `models_for` keeps providers whose `kind_of(kind)` has a `media_company`; `host` from `agent::endpoint_host`, `cloud` from `endpoint_place == Internet`. In `test_provider`, after the chat list is saved: `if let Some(media) = agent::media::media_provider(client.clone()) { if let Err(e) = catalog::refresh(..).await { tracing::warn!(..) } }`, so a media list failure never fails Test.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::catalog && cargo test -p app --lib providers`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio/catalog.rs src-tauri/src/studio/mod.rs src-tauri/src/providers.rs
git commit -m "Studio: keep each endpoint's picture and video lists, fetched only on request"
```

### Task A12: Studio preferences

**Files:**
- Create: `src-tauri/src/studio/prefs.rs`
- Test: inline

**Interfaces:**
- Consumes: `spend::{CONFIRM_AT_USD, DEFAULT_DAILY_LIMIT_USD, ConfirmRules}`.
- Produces:
```rust
pub const PREFS_KEY: &str = "studioPrefs";
pub const MAX_SETUPS: usize = 50;
pub const DAILY_LIMIT_RANGE: RangeInclusive<f64> = 1.0..=1000.0;
pub const CONFIRM_AT_RANGE: RangeInclusive<f64> = 0.25..=100.0;
#[serde(rename_all = "camelCase", default)]
pub struct StudioPrefs { pub last_model: BTreeMap<MediaKind, ModelChoice>, pub advanced_open: bool, pub confirm_every_video: bool,
    pub confirm_at_usd: f64, pub daily_limit_usd: Option<f64>, pub setups: Vec<Setup>, pub gallery_dir: Option<String> }
pub struct ModelChoice { pub provider_id: i64, pub model: String }
pub struct Setup { pub name: String, pub kind: MediaKind, pub provider_id: i64, pub model: String,
    pub params: Map<String, Value>, pub extra: Map<String, Value>, pub routing: Option<Routing> }
impl StudioPrefs { pub fn confirm_rules(&self) -> ConfirmRules; pub fn validate(&self) -> Result<(), String>; }
pub async fn load(store: &store::Store) -> StudioPrefs;                  // a missing or unreadable value gives the defaults
pub async fn save(store: &store::Store, prefs: &StudioPrefs) -> AppResult<()>;   // validates first
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn the_defaults_are_the_owners() {
    let p = StudioPrefs::default();
    assert_eq!(p.daily_limit_usd, Some(10.0));
    assert!(p.confirm_every_video);
    assert_eq!(p.confirm_at_usd, 1.0);
}
#[test] fn the_limit_is_between_1_and_1000_dollars_or_off() {
    for bad in [0.5, 1000.01, f64::NAN] { assert!(with_limit(Some(bad)).validate().is_err()); }
    for good in [Some(1.0), Some(1000.0), None] { assert!(with_limit(good).validate().is_ok()); }
}
#[test] fn setups_stop_at_50_and_need_a_name() {}
#[tokio::test] async fn prefs_saved_without_new_fields_read_with_defaults() {
    // store {"advancedOpen":true} → load: advanced_open true, daily_limit_usd Some(10.0)
}
#[tokio::test] async fn a_gallery_folder_must_exist_and_take_files() { /* missing folder → Err "Pick an existing folder." */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::prefs`
Expected: FAIL to compile.

- [ ] **Step 3: Implement.** The folder check copies `hub::set_models_dir` (exists, `.write-test` probe). Changing the folder affects new results only; existing results keep their absolute paths and stay where they are (said on the settings page).

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::prefs`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio
git commit -m "Studio: preferences with a \$10 daily limit and confirmation rules on by default"
```

### Task A13: StudioManager: starting runs, pictures, queue, cancel, audit

**Files:**
- Create: `src-tauri/src/studio/deps.rs`, `src-tauri/src/studio/manager.rs`, `src-tauri/src/studio/manager_tests.rs`, `src-tauri/src/studio/errors.rs`
- Test: `manager_tests.rs` (fakes: `FakeProvider` counting `start` calls with scripted answers, `FakeSink`, `FakeFetch`, `FixedClock`; `store::Store::open_in_memory()`; a temporary gallery)

**Interfaces:**
- Consumes: Tasks A1–A12, `store::{AuditEntry, AuditDecision}`.
- Produces:
```rust
// deps.rs
#[async_trait] pub trait ProviderSource: Send + Sync {
    async fn media(&self, id: store::ProviderId) -> AppResult<(Arc<dyn MediaProvider>, Endpoint)>;   // studio_no_endpoint / studio_no_key
}
pub struct Endpoint { pub id: store::ProviderId, pub name: String, pub host: String, pub company: Company }
pub trait StudioSink: Send + Sync { fn progress(&self, run: &RunDto); }
#[async_trait] pub trait ForeignFetch: Send + Sync { async fn fetch_to_file(&self, url: &Url, cap: u64, dest: &Path) -> Result<String, GalleryError>; }  // returns the content type
pub trait Clock: Send + Sync { fn now_ms(&self) -> i64; fn day_start_ms(&self, at_ms: i64) -> i64; }
pub struct StoreProviders { pub store: store::Store, pub secrets: Arc<dyn store::secrets::SecretStore> }   // reads the endpoint and its key like providers::client_for
pub struct AppSink(pub AppHandle); pub struct GuardedFetch(pub search::Fetcher); pub struct SystemClock;
// manager.rs
pub const PICTURES_AT_ONCE: usize = 2;
pub const VIDEOS_AT_ONCE: usize = 2;
pub struct StudioDeps { pub store: store::Store, pub providers: Arc<dyn ProviderSource>, pub sink: Arc<dyn StudioSink>,
    pub fetch: Arc<dyn ForeignFetch>, pub clock: Arc<dyn Clock>, pub gallery: Arc<dyn Fn() -> Gallery + Send + Sync>,
    pub thumbs: Option<PictureWorker>, pub poll: PollPlan, pub limits: MediaLimits }
pub struct StudioManager { /* Arc<Inner>: running, queues, starting: tokio::sync::Mutex<()>, recent_tokens, live */ }
impl StudioManager {
    pub fn new(deps: StudioDeps) -> Self;
    pub async fn estimate(&self, req: &StartRequest) -> AppResult<EstimateDto>;
    pub async fn preview(&self, req: &StartRequest) -> AppResult<RequestPreview>;
    pub async fn start(&self, req: StartRequest, confirmed: bool) -> AppResult<RunDto>;
    pub async fn cancel(&self, run_id: &str) -> AppResult<RunDto>;
    pub async fn live(&self) -> AppResult<Vec<RunDto>>;
    pub async fn run_dto(&self, run: &store::CreationRun) -> RunDto;
    pub async fn usage(&self) -> AppResult<UsageDto>;
}
// errors.rs (each a four-part AppError; copy in plain English)
pub fn needs_confirmation(reasons: &[ConfirmReason]) -> AppError;             // "studio_needs_confirmation"
pub fn daily_limit(spent: f64, limit: f64) -> AppError;                       // "studio_daily_limit": "Daily limit reached: $9.50 of $10 spent today"
pub fn blocked(company: Company, stage: Option<Stage>, charged: Option<bool>, request_id: Option<&str>) -> AppError;  // "studio_blocked"
pub fn org_verification() -> AppError; pub fn no_endpoint() -> AppError; pub fn no_key(name: &str) -> AppError;
pub fn job_lost(company: Company) -> AppError; pub fn interrupted(company: Company) -> AppError; pub fn result_expired(company: Company) -> AppError;
pub fn disk_full() -> AppError; pub fn too_large(cap: u64) -> AppError; pub fn gallery_full() -> AppError;
pub fn not_answered(company: Company, charged: Option<bool>) -> AppError;      // "studio_not_answered": timeouts and cut connections, "may still charge"
pub fn from_failure(company: Company, f: &MediaFailure) -> AppError;
```
`StartRequest`, `RunDto`, `EstimateDto`, `UsageDto` are defined in `src-tauri/src/dto.rs` in this task (shapes in Task A15).

- [ ] **Step 1: Write the failing tests** (`manager_tests.rs`)
```rust
#[tokio::test] async fn a_picture_run_goes_from_queued_to_done_with_its_files() {
    let h = harness().await;                                     // fake answers Done with 2 PNGs, cost 0.04
    let run = h.manager.start(h.picture_req("lighthouse at dusk"), false).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 2);
    assert!(Path::new(&done.outputs[0].path).starts_with(h.gallery_root.join("2026-11")));
    assert_eq!(done.cost_usd, Some(0.04));
    assert_eq!(h.statuses(&run.id), ["queued", "sending", "done"]);
}
#[tokio::test] async fn a_video_without_confirmation_is_refused() {
    let err = h.manager.start(h.video_req(), false).await.unwrap_err();
    assert_eq!(err.code, "studio_needs_confirmation");
    assert_eq!(h.provider.starts(), 0); assert!(h.store_runs().await.is_empty());
}
#[tokio::test] async fn a_run_over_the_daily_limit_is_refused_whatever_the_webview_says() {
    h.seed_done_run_costing(9.50).await;
    let err = h.manager.start(h.video_req_estimated(0.96), true).await.unwrap_err();
    assert_eq!(err.code, "studio_daily_limit");
    assert_eq!(err.message, "Daily limit reached: $9.50 of $10 spent today");
    assert_eq!(h.provider.starts(), 0);
}
#[tokio::test] async fn queued_and_running_estimates_count_against_the_limit() {
    // two video jobs held in `waiting` at 4.50 each (confirmed) → a third at 1.50 is refused
}
#[tokio::test] async fn two_starts_at_once_cannot_both_pass_the_limit() {
    h.seed_done_run_costing(8.50).await;
    let (a, b) = tokio::join!(h.manager.start(h.req_estimated(1.00, "t1"), true), h.manager.start(h.req_estimated(1.00, "t2"), true));
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
}
#[tokio::test] async fn the_same_create_press_twice_makes_one_run() {
    let a = h.manager.start(h.req_token("press-1"), false).await.unwrap();
    let b = h.manager.start(h.req_token("press-1"), false).await.unwrap();
    assert_eq!(a.id, b.id); h.wait_status(&a.id, "done").await; assert_eq!(h.provider.starts(), 1);
}
#[tokio::test] async fn studio_picks_a_seed_when_the_model_takes_one() {
    // seedream-5-5 request without seed → stored params.params.seed in 0..=2_147_483_647; nano banana → no seed key
}
#[tokio::test] async fn a_field_the_model_does_not_list_is_refused_before_anything_is_sent() {
    // {"seed": 1} for nano banana → bad_request naming the setting; starts() == 0
}
#[tokio::test] async fn two_pictures_run_at_once_and_the_third_waits() {
    // provider holds each start until released; 3 starts on one endpoint → [sending, sending, queued], queue_position Some(1)
}
#[tokio::test] async fn cancel_while_sending_says_the_company_may_charge() {
    // status cancelled; error.charged None; error.message contains "may still charge"
}
#[tokio::test] async fn a_timeout_fails_the_run_and_it_is_never_sent_again() {
    // fake → Err(Unknown); status failed, code "studio_not_answered"; after 2 s of waiting starts() == 1
}
#[tokio::test] async fn a_blocked_run_is_never_sent_again() {
    // fake → Answered(Blocked, charged Some(false)); status blocked; error.message "OpenAI's safety filter refused the prompt."; starts() == 1
}
#[tokio::test] async fn the_audit_row_has_the_prompt_length_not_the_prompt() {
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!((row.server.as_str(), row.tool.as_str()), ("studio", "picture"));
    assert_eq!(row.chat_id, None);
    assert_eq!(row.decision, AuditDecision::AllowedWithoutPrompt); assert_eq!(row.decided_by, "user");
    assert_eq!(row.arguments["prompt_chars"], 18);
    assert!(!row.arguments.to_string().contains("lighthouse"));
    assert_eq!(row.result_status.as_deref(), Some("done"));
    assert!(row.result_summary.unwrap().contains("$0.04"));
}
#[tokio::test] async fn extra_field_values_are_not_audited() {
    // extra {"negativePrompt":"secret words"} → arguments["extra_fields"] == ["negativePrompt"]; no "secret words"
}
#[tokio::test] async fn the_gallery_cap_is_said_plainly() { /* store at MAX_CREATION_RUNS → "studio_gallery_full" */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::manager`
Expected: FAIL to compile, `cannot find type 'StudioManager'`.

- [ ] **Step 3: Implement.** `start`, under the `starting` lock: a known `client_token` (last 64 kept in memory) returns its run; load the model from `catalog::model` (else `studio_no_endpoint`); add a random seed when the model has a Seed field and none is set; `spec::validate`; `price::estimate(choice_for(..))`; `spend::check_limit` with `spent_since(rows of creation_runs_since(day_start))` (refuse `studio_daily_limit`); `spend::confirm_reasons` with `prefs.confirm_rules()` (refuse `studio_needs_confirmation` when not empty and `!confirmed`); insert the run (`queued`, `params = {params, extra, routing}`, `refs = [{sha256, role, mime, sizeBytes}]`); append the audit row (arguments: `endpoint`, `host`, `model`, `changed` = typed settings with their values, `extra_fields` = names only, `pictures_sent`, `estimate_usd`, `confirmed`, `prompt_chars`; preview "Made a picture with <model> through <endpoint>" / "Started a video …"; risk `normal`) and store its id in `audit_id`; enqueue. A run task: status `sending` is written **before** `provider.start`; `Started::Done` → each output through `gallery.finalize` (cap 50 MiB) and `insert_creation_output`, a thumbnail when a worker is set, sidecar, `done` with cost; errors through `errors::from_failure` / `not_answered` into `error` JSON with `charged`, `request_id`, `company`, `stage`. A slot frees on every exit through a `TaskEnd` drop guard as in `DownloadManager`. Cancel cancels the token; the run's future is dropped (the connection closes). Every exit completes the audit row with the status and `cost $X.XX` or `cost not reported`.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::manager`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio src-tauri/src/dto.rs
git commit -m "Studio: start runs under the spending rules, queue them and make pictures"
```

### Task A14: StudioManager: video jobs and restarts

**Files:**
- Modify: `src-tauri/src/studio/manager.rs`, `src-tauri/src/studio/manager_tests.rs`

**Interfaces:**
- Produces:
```rust
pub struct PollPlan { pub first: Duration, pub second: Duration, pub third: Duration, pub second_from: Duration, pub third_from: Duration, pub watch_limit: Duration }
impl Default for PollPlan   // 5 s; 10 s from 60 s; 20 s from 300 s; watch_limit 30 min
pub fn poll_delay(plan: &PollPlan, watched_for: Duration) -> Duration;
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);
impl StudioManager {
    pub async fn resume_interrupted(&self);
    /// "Get it anyway" on a stopped video, and "Check again" on one still not ready: checks and downloads; never creates.
    pub async fn fetch_anyway(&self, run_id: &str) -> AppResult<RunDto>;
}
```

- [ ] **Step 1: Write the failing tests**
```rust
#[test] fn checks_come_every_5_then_10_then_20_seconds() {
    let p = PollPlan::default();
    assert_eq!(poll_delay(&p, secs(0)), secs(5));   assert_eq!(poll_delay(&p, secs(59)), secs(5));
    assert_eq!(poll_delay(&p, secs(60)), secs(10)); assert_eq!(poll_delay(&p, secs(299)), secs(10));
    assert_eq!(poll_delay(&p, secs(300)), secs(20)); assert_eq!(p.watch_limit, secs(30 * 60));
}
#[tokio::test] async fn the_remote_id_is_saved_before_the_first_check() { /* fake.check reads the row: remote_id == Some("vid_1") */ }
#[tokio::test] async fn a_network_error_while_checking_keeps_waiting() { /* check Err(Unknown) twice then Done → done */ }
#[tokio::test] async fn resume_after_restart_finishes_the_video_without_a_second_create() {
    let first = h.manager(); first.start(h.video_req(), true).await.unwrap(); h.wait_status_any("waiting").await;
    drop(first);
    let second = h.manager_with(fake_done_on_first_check()); second.resume_interrupted().await;
    h.wait_status_any("done").await;
    assert_eq!(h.provider.starts() + h.provider2.starts(), 1);
}
#[tokio::test] async fn a_run_left_sending_becomes_interrupted_and_is_never_sent_again() {
    // seeded row: status sending, no remote id → resume → interrupted, code "studio_interrupted",
    // message contains "may or may not have started"; starts() == 0
}
#[tokio::test] async fn a_queued_run_is_not_sent_after_a_restart() {
    // seeded row: queued → resume → cancelled, message "Not sent: the app closed before its turn. Nothing was charged."; starts() == 0
}
#[tokio::test] async fn a_download_left_half_done_starts_over_from_the_result() { /* downloading row → check → download; starts() == 0 */ }
#[tokio::test] async fn stop_watching_keeps_the_remote_id_and_get_it_anyway_downloads() {
    // cancel while waiting → cancelled, remote_id kept, error.message contains "OpenRouter may still finish and charge for this";
    // fetch_anyway → done; starts() == 1
}
#[tokio::test] async fn after_30_minutes_it_says_still_not_ready() {
    // watch_limit 50 ms → status waiting, watching false, stale true; fetch_anyway resumes checking
}
#[tokio::test] async fn a_job_the_company_no_longer_has_says_so() { /* Gone → failed "studio_job_lost", remote_id kept */ }
#[tokio::test] async fn an_expired_result_says_so_and_keeps_the_remote_id() {
    // download → Answered(Expired) → failed "studio_result_expired"; remote_id kept; counted as maybe charged; starts() == 1
}
#[tokio::test] async fn a_full_disk_keeps_the_remote_id_for_get_it_anyway() {
    // finalize → DiskFull → failed "studio_disk_full"; .partial empty; fetch_anyway after space is back → done; starts() == 1
}
#[tokio::test] async fn a_video_over_its_cap_is_refused() { /* video_cap 1 KiB → failed "studio_too_large", partial removed */ }
#[tokio::test] async fn a_foreign_result_link_goes_through_the_guarded_fetcher() {
    // provider.download → Elsewhere(url) → FakeFetch saw exactly that url; output saved (ForeignFetch has no key parameter)
}
#[tokio::test] async fn progress_is_sent_at_most_four_times_a_second() {
    // 100 Running{progress} answers within 1 s → progress-only events <= 4; every status change still sent
}
#[tokio::test] async fn the_audit_row_is_completed_after_a_restart() { /* audit row of the resumed run gets "done" and the cost */ }
#[tokio::test] async fn the_third_video_waits_for_a_slot() { /* VIDEOS_AT_ONCE = 2 per endpoint; a stale job frees its slot */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::manager`
Expected: FAIL (`resume_interrupted`, `fetch_anyway`, `poll_delay` missing).

- [ ] **Step 3: Implement.** Video task: `sending` → `provider.start` → write `remote_id` and `waiting` → loop `check` with `poll_delay(watched_for)` until `Done`/`Failed`/`Gone` or `watch_limit` (then `watching = false`, row stays `waiting`); a check error keeps waiting. `Done` → `downloading` → `provider.download` into `.partial/<output id>`; `Elsewhere(url)` → `deps.fetch.fetch_to_file(url, video_cap, partial)`; `gallery.finalize`; output row; `done` with the job's cost. `resume_interrupted` at start: `sending` → `interrupted`; `queued` → `cancelled` (not sent); `waiting` with a remote id → watch again (counting `watched_for` from now); `downloading` → check and download again. Charge stored for `spend`: `done` → Final; `failed`/`blocked` → from `charged` (Some(false) → None, otherwise Maybe); `cancelled` with a remote id, `interrupted` → Maybe; `cancelled` before sending → None.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib studio::manager`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio
git commit -m "Studio: follow video jobs across restarts without paying twice"
```

### Task A15: Commands, DTOs, events and wiring

**Files:**
- Create: `src-tauri/src/studio/commands.rs`
- Modify: `src-tauri/src/dto.rs`, `src-tauri/src/lib.rs` (remove the `dead_code` allow; register handlers; `manage(Arc<StudioManager>)`; spawn `resume_interrupted` next to the downloads resume), `src-tauri/commands.list`, `src-tauri/capabilities/main.json`, `src-tauri/src/artifacts.rs` (`export_target(path, ext, forbidden)`, `pub(crate)`)
- Create (generated by a test): `src/components/studio/fixtures/models.json`
- Test: inline in `commands.rs`

**Interfaces:**
- Produces the commands (each `async`, first checks `only_main_window(window.label())`):
```text
studio_models() -> StudioModelsDto                          // saved lists only
studio_refresh_models(provider_id: i64) -> StudioModelsDto
studio_model_price(provider_id: i64, model: String) -> MediaModelDto
studio_estimate(req: StartRequest) -> EstimateDto            // no network
studio_preview(req: StartRequest) -> RequestPreviewDto
studio_start(req: StartRequest, confirmed: bool) -> RunDto
studio_cancel(run_id: String) -> RunDto
studio_fetch_anyway(run_id: String) -> RunDto
studio_runs(filter: RunsFilter, before: Option<i64>, limit: u32) -> RunsPage
studio_live() -> Vec<RunDto>
studio_output_bytes(output_id: String, thumb: bool) -> tauri::ipc::Response
studio_save_thumb(output_id: String, jpeg_base64: String) -> OutputDto     // JPEG only, at most 1 MiB
studio_star(output_id: String, starred: bool) -> OutputDto
studio_delete(output_ids: Vec<String>) -> ()
studio_delete_unfinished() -> u32                            // "Delete stopped and failed runs"
studio_export(output_id: String, path: String, with_settings: bool) -> String
studio_show_in_folder(output_id: String) -> ()
studio_open_folder() -> ()
studio_usage() -> UsageDto
studio_add_ref(image: ImageInput) -> RefDto
studio_ref_from_output(output_id: String) -> RefDto
studio_ref_bytes(sha256: String) -> tauri::ipc::Response
studio_output_for_chat(output_id: String) -> ImageInput      // pictures under 20 MB in PNG, JPEG, GIF or WebP
studio_get_prefs() -> StudioPrefs
studio_set_prefs(prefs: StudioPrefs) -> StudioPrefs
```
Event `studio-progress` → `RunDto`. DTOs (serde camelCase; the TS mirrors in Task B1 use the same names):
```rust
pub struct StartRequest { pub client_token: String, pub provider_id: i64, pub kind: MediaKind, pub model: String, pub prompt: String,
    pub params: Map<String, Value>, pub extra: Map<String, Value>, pub refs: Vec<RefInput>, pub routing: Option<Routing> }
pub struct RefInput { pub sha256: String, pub role: RefRole }
pub struct RefDto { pub sha256: String, pub mime: String, pub size_bytes: u64, pub role: Option<RefRole> }
pub struct PriceHintDto { pub unit: String /* image|second|token|unknown */, pub low: Option<f64>, pub high: Option<f64> }
pub struct MediaModelDto { pub id: String, pub display_name: String, pub family: String, pub kind: MediaKind, pub accepts_images: bool,
    pub params: Vec<ParamSpec>, pub price: PriceHintDto, pub price_known: bool, pub companies: Vec<String>, pub notes: Vec<String>,
    pub usable: Option<String>, pub created: Option<i64> }
pub struct StudioEndpointDto { pub provider_id: i64, pub name: String, pub company: Company, pub host: String, pub cloud: bool,
    pub has_key: bool, pub fetched_at: Option<i64>, pub models: Vec<MediaModelDto> }
pub struct StudioModelsDto { pub endpoints: Vec<StudioEndpointDto> }
pub struct RunErrorDto { pub code: String, pub message: String, pub next_step: String, pub details: String,
    pub charged: Option<bool>, pub request_id: Option<String>, pub company: String, pub stage: Option<String> }
pub struct OutputDto { pub id: String, pub run_id: String, pub idx: u32, pub mime: String, pub size_bytes: u64, pub width: Option<u32>,
    pub height: Option<u32>, pub duration_ms: Option<u64>, pub seed: Option<u64>, pub starred: bool, pub has_thumb: bool, pub path: String }
pub struct RunDto { pub id: String, pub kind: MediaKind, pub provider_id: Option<i64>, pub provider_name: String, pub provider_host: String,
    pub model: String, pub model_label: String, pub prompt: String, pub params: Map<String, Value>, pub extra: Map<String, Value>,
    pub routing: Option<Routing>, pub refs: Vec<RefDto>, pub status: String, pub watching: bool, pub stale: bool, pub remote_id: Option<String>,
    pub progress: Option<f64>, pub estimate_usd: Option<f64>, pub cost_usd: Option<f64>, pub error: Option<RunErrorDto>,
    pub created_at: i64, pub started_at: Option<i64>, pub finished_at: Option<i64>, pub outputs: Vec<OutputDto>, pub queue_position: Option<u32> }
pub struct LimitDto { pub limit_usd: Option<f64>, pub spent_today_usd: f64, pub pending_usd: f64, pub would_pass: bool }
pub struct EstimateDto { pub usd: Option<f64>, pub reasons: Vec<ConfirmReason>, pub limit: LimitDto, pub invalid: Vec<String> }
pub struct RequestPreviewDto { pub method: String, pub url: String, pub host: String, pub body: Value }
pub struct RunsFilter { pub kind: Option<MediaKind>, pub starred: bool, pub text: String, pub model: Option<String> }
pub struct RunsPage { pub runs: Vec<RunDto>, pub next: Option<i64> }
pub struct UsageDto { pub spent_today_usd: f64, pub pending_usd: f64, pub limit_usd: Option<f64>, pub folder: String,
    pub bytes: u64, pub pictures: u64, pub videos: u64 }
```

- [ ] **Step 1: Write the failing tests**
```rust
const STUDIO_COMMANDS: &[&str] = &["studio_models", /* … all 25 above … */ "studio_set_prefs"];
#[test] fn every_studio_command_is_listed_granted_and_registered() {
    let list = include_str!("../../commands.list"); let main = include_str!("../../capabilities/main.json");
    let lib = include_str!("../lib.rs");
    for c in STUDIO_COMMANDS {
        assert!(list.lines().any(|l| l.trim() == *c), "{c}");
        assert!(main.contains(&format!("\"allow-{}\"", c.replace('_', "-"))), "{c}");
        assert!(lib.contains(&format!("studio::commands::{c},")), "{c}");
    }
}
#[test] fn studio_errors_have_four_parts() { /* every errors.rs constructor: non-empty code, message, next_step */ }
#[test] fn the_safety_message_names_the_company_and_the_charge() {
    let e = errors::blocked(Company::OpenAi, Some(Stage::Prompt), Some(false), Some("req_8f2"));
    assert_eq!(e.message, "OpenAI's safety filter refused the prompt.");
    assert!(e.next_step.contains("Not charged") && e.details.contains("req_8f2"));
}
#[test] fn the_stop_hotkey_never_reaches_studio() { assert!(!include_str!("../hotkey.rs").contains("studio")); }
#[test] fn the_webview_fixture_is_current() {
    // six models (gpt-image-2.6-flare, gemini-nano-banana-2.1, krea/k3-edit, veo-3.1, grok-imagine-video-1.5-lite,
    // seedance-2.5-flash) as MediaModelDto from the catalogue fixtures, plus one RunDto, compared with
    // src/components/studio/fixtures/models.json; with STUDIO_WRITE_FIXTURE=1 it writes the file instead
}
#[test] fn a_video_or_a_big_picture_cannot_go_to_a_chat() {
    // for_chat(&CreationOutput) -> AppResult<()>: "video/mp4", "image/svg+xml" and a 21 MiB PNG are refused; a 2 MiB PNG passes
}
#[test] fn export_keeps_the_original_extension_and_refuses_app_folders() { /* export_target(path, "mp4", forbidden) */ }
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib studio::commands && cargo test -p app --lib commands::tests::commands_list_matches_capabilities`
Expected: FAIL (commands missing from the list and lib.rs).

- [ ] **Step 3: Implement.** Wire each command to `StudioManager`, `catalog`, `prefs`, `Gallery`. `studio_output_bytes` reads the recorded file (or its thumbnail) up to 1 GiB into `tauri::ipc::Response::new(bytes)`. `studio_export` copies the original, and with `with_settings` writes `<name>.json` (the run record: prompt, model, settings, cost; never a key). `studio_show_in_folder` uses `tauri_plugin_opener::OpenerExt::opener().reveal_item_in_dir`. `studio_delete` removes rows, then files, thumbnails and an orphaned sidecar; the audit rows stay. In `lib.rs`: build `StudioManager::new(StudioDeps{ StoreProviders { store, secrets }, AppSink, GuardedFetch(search::Fetcher::new(user_agent).with_timeout(Duration::from_secs(600))), SystemClock, gallery from prefs or `Gallery::default_root(home, package name)`, thumbs: Some(PictureWorker::thumbs()), PollPlan::default(), MediaLimits::default() })`, `app.manage(studio.clone())`, then spawn `studio.resume_interrupted()` beside `downloads.resume_interrupted`. Then run `STUDIO_WRITE_FIXTURE=1 cargo test -p app --lib studio::commands::tests::the_webview_fixture_is_current` once to write the fixture.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**
```bash
git add src-tauri src/components/studio/fixtures/models.json
git commit -m "Studio: commands, progress events and start-up resume"
```

---

## Phase B: Webview

All webview tests mock `src/components/studio/api.ts` with `vi.mock("./api")` (or the relative path) and build data from `src/components/studio/fixtures/models.json`, as `ChatView.cloud.test.tsx` and `settings/Endpoints.test.tsx` mock `lib/ipc`.

### Task B1: Studio store and navigation

**Files:**
- Create: `src/components/studio/types.ts`, `src/components/studio/api.ts`, `src/store/studio.ts`, `src/store/studio.test.ts`
- Modify: `src/App.tsx`, `src/App.test.tsx`, `src/components/Sidebar.tsx`, `src/components/Sidebar.test.tsx`, `src/components/ShortcutsDialog.tsx`, `src/lib/words.ts`

**Interfaces:**
- Consumes: Task A15 commands, event and DTOs.
- Produces:
```ts
// types.ts: TS mirrors with the DTO names, camelCase fields exactly as Task A15:
// MediaKind, Company, Choice, SizeRules, ParamKind, Canonical, ParamSpec, RefRole, PriceHint, MediaModel, StudioEndpoint,
// RunStatus, RunError, Output, RefItem, Run, Limit, ConfirmReason, Estimate, Routing, StartRequest, Setup, StudioPrefs,
// Usage, RunsFilter, RunsPage, RequestPreview, ImageInput (from lib/types)
// api.ts: one wrapper per command, e.g.
export const studioModels = () => invoke<{ endpoints: StudioEndpoint[] }>("studio_models");
export const studioStart = (req: StartRequest, confirmed: boolean) => invoke<Run>("studio_start", { req, confirmed });
export const studioOutputBytes = (outputId: string, thumb: boolean) => invoke<ArrayBuffer>("studio_output_bytes", { outputId, thumb });
export function onStudioProgress(handler: (run: Run) => void): Promise<UnlistenFn>;     // "studio-progress"
// store/studio.ts
export type StudioTab = MediaKind;
export interface StudioState {
  view: "chat" | "studio"; tab: StudioTab; live: Record<string, Run>; readyNotice: string | null;
  showStudio(tab?: StudioTab): void; showChat(): void; applyProgress(run: Run): void;
}
export const useStudio: UseBoundStore<StoreApi<StudioState>>;
export function runningCount(s: StudioState): number;       // queued, sending, downloading, and waiting while watched
export function followStudio(): () => void;                 // studio-progress through listenInEffect, plus studioLive() once
```

- [ ] **Step 1: Write the failing tests**
```ts
// App.test.tsx (StudioView mocked to a marker; studio api mocked)
it("Ctrl+G opens Studio and Esc there stops nothing", async () => {
  fireEvent.keyDown(window, { key: "g", ctrlKey: true });
  expect(useStudio.getState().view).toBe("studio");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(ipcMock.stopGeneration).not.toHaveBeenCalled();
});
it("app:open-studio opens Studio on the tab it names", () => {
  window.dispatchEvent(new CustomEvent("app:open-studio", { detail: "video" }));
  expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video" });
});
it("the chat stays mounted, hidden, while Studio is shown", () => { /* ChatView marker still in the DOM, inside a hidden element */ });
it("Ctrl+N goes back to the chat", () => { /* view "chat"; newChat called */ });
// Sidebar.test.tsx
it("the Studio button sits between Models and Settings and counts working runs", () => {
  useStudio.setState({ live: { r1: run({ status: "waiting", watching: true }) } });
  const buttons = screen.getAllByRole("button").map((b) => b.getAttribute("aria-label")).filter(Boolean);
  expect(buttons.slice(0, 3)).toEqual(["Models", "Studio (1 working)", "Settings"]);
});
it("a chat click or New chat leaves Studio", () => { /* view becomes "chat" */ });
// studio.test.ts
it("a video that finishes while the chat is shown raises the ready notice", () => {
  useStudio.getState().applyProgress(run({ id: "v", kind: "video", status: "done" }));
  expect(useStudio.getState().readyNotice).toBe("v");
});
it("runningCount ignores stopped and stale runs", () => {});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/App.test.tsx src/components/Sidebar.test.tsx src/store/studio.test.ts`
Expected: FAIL (`Failed to resolve import "./store/studio"`).

- [ ] **Step 3: Implement.** App keeps `<ChatView/>` and `<Workbench/>` mounted inside a wrapper with `hidden` while `view === "studio"` (the composer's draft survives, and `app:compose-images` reaches it), and mounts `<StudioView/>` on first open and keeps it. Key handler: `mod+g` → `showStudio()`; `Escape` returns early when `view === "studio"`; `mod+n`, `mod+l` call `showChat()` first. App's effect calls `followStudio()`. Sidebar: `Sparkles` icon button, `aria-label` "Studio" or "Studio (N working)", title "Studio (Ctrl+G)", with a small count badge; chat rows and New chat call `showChat()`. Shortcut row `[\`${MOD} G\`, "Studio"]`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/App.test.tsx src/components/Sidebar.test.tsx src/store/studio.test.ts && npm run typecheck`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio/types.ts src/components/studio/api.ts src/store/studio.ts src/store/studio.test.ts src/App.tsx src/App.test.tsx src/components/Sidebar.tsx src/components/Sidebar.test.tsx src/components/ShortcutsDialog.tsx src/lib/words.ts
git commit -m "Studio: sidebar button, Ctrl+G, and a view that leaves the chat running"
```

### Task B2: Studio view shell

**Files:**
- Create: `src/components/studio/StudioView.tsx`, `src/components/studio/StudioView.test.tsx`
- Modify: `src/store/studio.ts` (endpoints, drafts, runs, prefs, usage, `load`, `refreshModels`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export interface Draft { providerId: number | null; model: string | null; prompt: string; values: Record<string, unknown>;
  extra: Record<string, unknown>; refs: RefItem[]; seedLocked: boolean; routing: Routing | null; note: string | null }
// added to StudioState:
endpoints: StudioEndpoint[]; loaded: boolean; drafts: Record<StudioTab, Draft>; runs: Run[]; next: number | null;
filter: RunsFilter; prefs: StudioPrefs | null; usage: Usage | null; grid: boolean;
load(): Promise<void>;                        // studioModels, studioRuns, studioLive, studioGetPrefs, studioUsage; never a refresh
refreshModels(providerId: number): Promise<void>;
setTab(tab: StudioTab): void;
export function StudioView(): JSX.Element;
```

- [ ] **Step 1: Write the failing tests**
```ts
it("opening Studio reads only what is saved", async () => {
  await show();
  expect(api.studioModels).toHaveBeenCalledTimes(1);
  expect(api.studioRefreshModels).not.toHaveBeenCalled();
  expect(api.studioModelPrice).not.toHaveBeenCalled();
});
it("with no OpenRouter or OpenAI endpoint it says how to add one", async () => {
  await show({ endpoints: [] });
  screen.getByText("Studio makes pictures and videos with OpenRouter or OpenAI. Add one in Settings → Endpoints.");
  fireEvent.click(screen.getByRole("button", { name: "Open Endpoints" }));
  expect(opened).toEqual(["endpoints"]);           // app:open-settings detail
});
it("an endpoint without a saved list offers to get it", async () => {
  await show({ endpoints: [endpoint({ fetchedAt: null, models: [] })] });
  screen.getByText("Get OpenRouter's picture and video models");
  fireEvent.click(screen.getByRole("button", { name: "Get the list" }));
  expect(api.studioRefreshModels).toHaveBeenCalledWith(1);
});
it("the top bar shows Pictures and Video, the Cloud mark and spent today", async () => {
  await show({ usage: usage({ spentTodayUsd: 0.84 }) });
  screen.getByRole("tab", { name: "Pictures" }); screen.getByRole("tab", { name: "Video" });
  screen.getByText("$0.84 today"); expect(document.querySelector("[data-cloud-mark]")).not.toBeNull();
});
it("each tab keeps its own draft", async () => {});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/StudioView.test.tsx`
Expected: FAIL (`Failed to resolve import "./StudioView"`).

- [ ] **Step 3: Implement** the layout of Wireframe 1: top bar (title, `[Pictures | Video]` Radix tabs, search, Starred, Grid toggle, `CloudMark`-style chip naming the chosen endpoint's host, "$X today"), the card stream in the middle, the right panel slot (Task B5), the prompt dock slot (Task B7). The first model of a tab is `prefs.lastModel[tab]`, else the first usable model of the first endpoint with a list (OpenAI's `gpt-image-2.6-flare` or OpenRouter's `openai/gpt-image-2.6-flare` for pictures, `google/veo-3.1-fast` for video, when present), never an "Auto" choice.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: the view's frame, empty states and the saved lists"
```

### Task B3: Model picker

**Files:**
- Create: `src/components/studio/ModelPicker.tsx`, `src/components/studio/ModelPicker.test.tsx`
- Modify: `src/store/studio.ts` (`chooseModel`, picture price on pick), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export type PickerFilter = "edit" | "references" | "startFrame" | "endFrame" | "sound" | "transparent";
export function pickerGroups(endpoints: StudioEndpoint[], kind: StudioTab, query: string, filters: PickerFilter[], recent: string[]):
  { endpoint: StudioEndpoint; family: string; models: MediaModel[] }[];
export function priceHint(m: MediaModel): string;          // "from $0.04 a picture", "$0.10–$0.40 a second", "price shown when picked", "price unknown"
export function ModelPicker(props: { tab: StudioTab }): JSX.Element;
// store: chooseModel(tab, providerId, modelId): void      // applies canonical.carry (Task B4) and asks studioModelPrice once for an
//                                                         // OpenRouter picture model without a saved price
```

- [ ] **Step 1: Write the failing tests**
```ts
it("groups by endpoint and family, recent models first", () => {});
it("names where the prompt goes", () => {
  render(<ModelPicker tab="image" />);
  screen.getByText("via OpenRouter"); expect(screen.getByTitle("Your prompt and added pictures go to openrouter.ai.")).toBeTruthy();
});
it("greys out an endpoint without a key", () => {
  // hasKey false → option disabled with "Add a key in Settings → Endpoints"
});
it("greys out a video model that needs a video, and says why", () => {});
it("filters by Edit, References, Start frame, End frame, Sound and Transparent", () => {});
it("picking an OpenRouter picture model asks for its price once", async () => {
  // before: "price shown when picked"; pick → api.studioModelPrice(1, "openai/gpt-image-2.6-flare") once; pick again → still once
});
it("never offers an Auto model", () => { expect(screen.queryByText(/auto/i)).toBeNull(); });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/ModelPicker.test.tsx`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement** as a Radix Popover with a search box, filter chips, groups ("Recent" first, then each endpoint, then each family), one line per model: name, badges from its params (Edit when it accepts pictures, Refs N, Start/End frame, Sound, Transparent, GIF, SVG), the price hint and model notes in the tooltip. `filters`: `edit`/`references` = References field with max ≥ 1; `startFrame`/`endFrame` = those fields; `sound` = Sound field; `transparent` = `background` offers `transparent`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio/ModelPicker.tsx src/components/studio/ModelPicker.test.tsx src/store/studio.ts src/lib/words.ts
git commit -m "Studio: model picker grouped by endpoint and family, with the Cloud mark"
```

### Task B4: Carrying settings between models

**Files:**
- Create: `src/components/studio/canonical.ts`, `src/components/studio/canonical.test.ts`

**Interfaces:**
- Produces:
```ts
export type Values = Record<string, unknown>;
export function defaultsFor(model: MediaModel): Values;               // the `initial` values only (video length and resolution)
export function carry(from: MediaModel | null, to: MediaModel, values: Values, extra: Values, refs: RefItem[]):
  { values: Values; extra: Values; refs: RefItem[]; note: string | null };
export function familyOf(id: string): string;                         // the same rule as Rust `family_of`
export function closestModel(models: MediaModel[], retired: { id: string; family: string }): MediaModel | null;  // same family, newest `created`
export function changedCount(model: MediaModel, values: Values, extra: Values): number;   // Advanced fields set, plus each extra key
```

- [ ] **Step 1: Write the failing tests** (models from `fixtures/models.json`)
```ts
it("names what changed, in one note", () => {
  const r = carry(flare, nanoBanana, { aspect_ratio: "21:9", seed: 81723 }, {}, []);
  expect(r.values).toEqual({ aspect_ratio: "21:9" });
  expect(r.note).toBe("Nano Banana 2.1 has no seed setting, so the seed was removed.");
});
it("moves a shape to the nearest ratio", () => {
  expect(carry(a, withoutWide, { aspect_ratio: "21:9" }, {}, []).values.aspect_ratio).toBe("16:9");
  // note "Shape 21:9 became 16:9."
});
it("lowers how many to the new maximum", () => { /* n 4 → model without n: removed; max 2: 2, "How many 4 became 2." */ });
it("keeps the quality name or moves one step cheaper", () => {
  // "xhigh" → [low, medium, high] → "high" ("Quality xhigh became high."); "max" → "high"; "low" → "low"
});
it("moves a length to the nearest one, the shorter on a tie", () => { /* 7 → [4, 6, 8] → 6, "Length 7 s became 6 s." */ });
it("moves a resolution to the same or the next lower one", () => { /* "4K" → [720p, 1080p] → "1080p" */ });
it("keeps added pictures up to the new limit", () => {
  // 5 references → max 3 → 3, "Only 3 added pictures fit, so 2 were left out."
});
it("drops an extra field the new model does not accept", () => { /* "Veo 3.1 does not accept foo, so it was removed." */ });
it("defaults give a cheap first video", () => { expect(defaultsFor(veo)).toEqual({ duration: 4, resolution: "720p" }); });
it("counts Advanced changes", () => { expect(changedCount(flare, { seed: 1, background: "transparent", quality: "high" }, {})).toBe(2); });
it("offers the newest model of the same family for a retired one", () => {});
it("agrees with the Rust family rule", () => {
  expect(familyOf("google/veo-3.1-fast")).toBe("google/veo"); expect(familyOf("kwaivgi/kling-v3.0-pro")).toBe("kwaivgi/kling");
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/canonical.test.ts`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement.** Values are carried by `canonical` role, not by key, so `aspect_ratio` on one model fills the Shape field of another. Shape: nearest by `|ln(w/h) − ln(w'/h')|`; `auto` only when offered. Quality ladder: low < medium < high < xhigh < max (`auto` kept when offered). Resolution: compare by the number in the label (`4K` = 2160, `1K` = 1024). Anything else not offered is removed and named. Sentences joined with one space, in field order.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio/canonical.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio/canonical.ts src/components/studio/canonical.test.ts
git commit -m "Studio: carry settings to another model and say what changed"
```

### Task B5: Simple controls and added pictures

**Files:**
- Create: `src/components/studio/ParamField.tsx`, `SettingsPanel.tsx`, `RefTray.tsx` and their tests
- Modify: `src/store/studio.ts` (`setValue`, `resetValue`, `addRef`, `removeRef`, `setRefRole`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export type Widget = "segmented" | "select" | "slider" | "switch" | "seed" | "drop" | "size" | "json";
export function widgetFor(spec: ParamSpec): Widget;     // enum ≤ 5 values: segmented; longer: select; range: slider + number box; …
export function ParamField(props: { spec: ParamSpec; value: unknown; onChange(v: unknown): void; onReset(): void }): JSX.Element;
export function SettingsPanel(props: { tab: StudioTab }): JSX.Element;
export function RefTray(props: { tab: StudioTab }): JSX.Element;
// store: addRef(tab, ref: RefItem): string | null   (returns why it was not added)
```

- [ ] **Step 1: Write the failing tests**
```ts
it("draws each kind with its widget", () => {
  expect(widgetFor(spec({ kind: { type: "enum", values: choices(5) } }))).toBe("segmented");
  expect(widgetFor(spec({ kind: { type: "enum", values: choices(6) } }))).toBe("select");
  expect(widgetFor(spec({ kind: { type: "range", min: 0, max: 100 } }))).toBe("slider");
});
it("shows only the simple fields the model declares", () => { /* nano banana: no How many, no Quality; Resolution 1K 2K 4K */ });
it("shows five shapes and the rest under +", () => {});
it("How many offers 1, 2 and 4, and more under + up to the model's maximum", () => {});
it("starts a video at the shortest length of at least 4 s and 720p", () => { /* veo: [4 s] pressed, [720p] pressed */ });
it("hides compression unless the file type is JPEG or WebP", () => {});
it("choosing a transparent background switches to PNG and says so", () => {
  // output_format "jpeg" → background "transparent" → output_format "png", note "Transparent needs PNG or WebP, so the file type is now PNG."
});
it("the tray refuses a picture past the model's limit", () => { /* max 1: second add → "This model takes 1 added picture." */ });
it("a start frame drop zone appears only when the model takes one", () => {});
it("an added picture goes to studioAddRef and shows a role chip", async () => {});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/ParamField.test.tsx src/components/studio/SettingsPanel.test.tsx src/components/studio/RefTray.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement.** Every label reads "Automatic (company default)" until set; an unset field is not sent. The tray accepts drop, paste and a file (through `readImage` from `Composer.tsx`, then `studioAddRef`); role chips Reference / Start frame / End frame follow the model's Images fields. The panel holds Model (Task B3), the simple fields in spec order, My setups (Task B6) and the Advanced toggle with its change count.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: controls drawn from the model's own settings, and added pictures"
```

### Task B6: Advanced drawer, chips, Extra fields, request preview, My setups

**Files:**
- Create: `src/components/studio/AdvancedDrawer.tsx`, `AdvancedDrawer.test.tsx`
- Modify: `src/components/studio/SettingsPanel.tsx` (My setups), `src/store/studio.ts` (`saveSetup`, `applySetup`, `newSeed`, `lockSeed`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function chipsFor(model: MediaModel, values: Values, extra: Values): { key: string; text: string }[];   // "seed 81723", "background: transparent"
export function parseExtra(text: string, allowed: string[], typed: string[]): { ok: true; value: Values } | { ok: false; message: string };
export function AdvancedDrawer(props: { tab: StudioTab }): JSX.Element;
```

- [ ] **Step 1: Write the failing tests**
```ts
it("counts changed fields in the header", () => { screen.getByText("Advanced · 2 changed"); });
it("each changed field shows as a chip that removes it", () => {});
it("Reset all and each field's reset go back to Automatic", () => {});
it("Extra fields accept only the names the model lists", () => {
  expect(parseExtra('{"negativePrompt":"rain"}', ["negativePrompt"], [])).toEqual({ ok: true, value: { negativePrompt: "rain" } });
  expect(parseExtra('{"foo":1}', ["negativePrompt"], [])).toEqual({ ok: false, message: "This model does not accept foo." });
  expect(parseExtra("{nope", ["negativePrompt"], [])).toEqual({ ok: false, message: "This is not valid JSON." });
  expect(parseExtra('{"duration":4}', ["duration"], ["duration"])).toEqual({ ok: false, message: "Set duration with its own field above." });
});
it("lists the accepted names as hints", () => { screen.getByText("This model accepts: negativePrompt, personGeneration, enhancePrompt"); });
it("See the exact request shows where it goes, without the key", async () => {
  // api.studioPreview resolves {host:"openrouter.ai", body:{…}} → "goes to openrouter.ai" and "key removed"
});
it("the dice gives a new seed and Keep it locks it", () => {});
it("a setup brings back the model and every setting", () => {});
it("a routing choice is offered for OpenRouter pictures only", () => {});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/AdvancedDrawer.test.tsx`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement** Wireframe 2: Advanced fields through `ParamField`, each with a reset; seed with Dice and Keep it and the help "A seed is not a saved copy; the same seed on another model or version gives a different picture."; the OpenRouter section (company that runs it from `MediaModelDto.companies`, *Allow another company if that one is busy*, *Only companies that keep no data*) for pictures; Extra fields (JSON) with hints; *See the exact request* (read-only JSON, "key removed · goes to <host>"). `prefs.advancedOpen` remembers the drawer. My setups: a select plus *Save as setup…* (name), at most 50.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: every setting one click away, shown as chips, with the exact request"
```

### Task B7: Price on Create, the confirm dialog and the daily limit

**Files:**
- Create: `src/components/studio/cost.ts`, `cost.test.ts`, `PromptDock.tsx`, `PromptDock.test.tsx`, `ConfirmRun.tsx`, `ConfirmRun.test.tsx`
- Modify: `src/store/studio.ts` (`estimate` debounced 150 ms, `start`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function formatUsd(usd: number): string;                         // "$0.16"; below $0.01 "< $0.01"
export function costTone(usd: number | null): "plain" | "amber" | "red" | "unknown";
export function createLabel(estimate: Estimate | null): string;         // "~ $0.16" | "price unknown"
export function PromptDock(props: { tab: StudioTab }): JSX.Element;
export function ConfirmRun(props: { open: boolean; tab: StudioTab; onCancel(): void; onConfirm(dontAskVideos: boolean): void }): JSX.Element;
// store: estimate(tab): Promise<void>; start(tab, confirmed: boolean): Promise<Run | null>; starting: boolean
```

- [ ] **Step 1: Write the failing tests**
```ts
// cost.test.ts
it("formats and colours prices", () => {
  expect(formatUsd(0.16)).toBe("$0.16"); expect(formatUsd(0.004)).toBe("< $0.01");
  expect(costTone(0.99)).toBe("plain"); expect(costTone(1)).toBe("amber"); expect(costTone(2.5)).toBe("amber");
  expect(costTone(2.51)).toBe("red"); expect(costTone(null)).toBe("unknown");
  expect(createLabel(est({ usd: null }))).toBe("price unknown");
});
// PromptDock.test.tsx
it("Create shows the price for the whole run", () => { screen.getByRole("button", { name: /Create/ }); screen.getByText("~ $0.16"); });
it("names where the prompt goes", () => { screen.getByText("Cloud: your prompt and added pictures go to openrouter.ai"); });
it("Enter creates and Shift+Enter adds a line", () => {});
it("Create is disabled while a start is on its way", async () => {
  // api.studioStart pending; click Create twice and press Enter → studioStart called once
});
it("the daily limit replaces Create", () => {
  // estimate.limit = {limitUsd:10, spentTodayUsd:9.5, wouldPass:true}
  screen.getByText("Daily limit reached: $9.50 of $10 spent today");
  fireEvent.click(screen.getByRole("button", { name: "Raise the limit" }));     // app:open-settings "studio"
  expect(screen.queryByRole("button", { name: /Create/ })).toBeNull();
});
// ConfirmRun.test.tsx
it.each([["a video", "video", 0.09, ["video"]], ["a $1.00 picture run", "image", 1, ["atLeast"]],
         ["an unknown price", "image", null, ["unknownPrice"]]])("asks before %s", () => { /* dialog shown */ });
it("does not ask for a $0.99 picture run", () => { /* studioStart(req, false) at once */ });
it("says a started video is charged even if you stop watching", () => {
  screen.getByText("Make this video?");
  screen.getByText("OpenRouter charges for a video once it has started, even if you stop watching it.");
});
it("Don't ask again for videos under $1.00 saves the choice and starts", async () => {
  // → api.studioSetPrefs({…, confirmEveryVideo: false}) then api.studioStart(req, true)
});
it("Cancel sends nothing", () => { expect(api.studioStart).not.toHaveBeenCalled(); });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/cost.test.ts src/components/studio/PromptDock.test.tsx src/components/studio/ConfirmRun.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement.** The dock: *+ Add picture*, the prompt box, the chips row (Task B6), the Cloud line, the price in text with `costTone` colour as a second cue, and Create. The store's `start` makes one `clientToken` (`crypto.randomUUID()`) per press, sets `starting`, calls `studioStart(req, false)` unless `estimate.reasons` is not empty (then `ConfirmRun`), and on `studio_needs_confirmation` from the backend opens the dialog anyway. The dialog lists model, company, length, size, sound, count, the estimate with "(<company>'s price list, <date of fetchedAt>)", where the data goes, and for video the charge sentence; the checkbox appears only when the only reason is `video`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: price on Create, confirm before videos and dear runs, and the daily limit"
```

### Task B8: Run cards and progress

**Files:**
- Create: `src/components/studio/RunCard.tsx`, `RunCard.test.tsx`, `src/components/studio/StudioNotice.tsx`, `StudioNotice.test.tsx`
- Modify: `src/components/studio/StudioView.tsx` (queue strip, card stream), `src/App.tsx` (render `StudioNotice` in chat view), `src/store/studio.ts` (`cancel`, `fetchAnyway`), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function RunCard(props: { run: Run }): JSX.Element;
export function usualSeconds(runs: Run[], model: string): number | null;   // median of the last 10 finished runs of that model
export function queueSummary(runs: Run[]): string;                         // "1 making · 1 waiting · 3 done · 1 stopped"
export function StudioNotice(): JSX.Element | null;
```

- [ ] **Step 1: Write the failing tests**
```ts
it("a picture card shows making it, a timer and Cancel", () => { screen.getByText(/Making it · 0:0\d/); screen.getByRole("button", { name: "Cancel" }); });
it("a cancelled picture says the company may still charge", () => { screen.getByText("Stopped waiting. The company may still charge for it."); });
it("a video card shows time so far and the usual time", () => {
  screen.getByText("Making it · 1:12 so far · usually about 2 min");
  screen.getByText("You can keep working or close the app; it will appear here.");
});
it("Stop watching keeps a way to get the video", async () => {
  // cancel → card "OpenRouter may still finish and charge for this." + "Get it anyway" → api.studioFetchAnyway(id)
});
it("a still-not-ready video offers Check again", () => {});
it("a refused run explains it and offers no retry", () => {
  // error {code:"studio_blocked", message:"OpenAI's safety filter refused the prompt.", charged:false, requestId:"req_8f2"}
  screen.getByText("OpenAI's safety filter refused the prompt."); screen.getByText("Not charged · request req_8f2");
  screen.getByRole("button", { name: "Edit prompt" }); expect(screen.queryByRole("button", { name: /try again/i })).toBeNull();
});
it("an interrupted run says it may or may not have started", () => {});
it("progress events update the card", () => { /* applyProgress → progress bar 40% */ });
it("the queue strip counts", () => { expect(queueSummary(runs)).toBe("1 making · 1 waiting · 3 done · 1 stopped"); });
it("a video finished while in chat says so, with a way to open it", () => {
  // readyNotice "v" → "Your video is ready" + "Open" → view studio, viewer on its first output
});
it("Esc does not cancel a running card", () => { fireEvent.keyDown(window, { key: "Escape" }); expect(api.studioCancel).not.toHaveBeenCalled(); });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/RunCard.test.tsx src/components/studio/StudioNotice.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement** Wireframe 3's card: prompt, the settings line ("Veo 3.1 Fast · OpenRouter · 8 s · 16:9 · 720p · sound on · start frame"), estimate then actual cost, status line, results row (Task B9 tiles), actions (Again, Reuse, Star, …). The charge line reads "Not charged", "May have been charged" or "Charged $0.04". Parts of the strip with 0 are left out; failed and blocked runs count as "failed".

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio src/App.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/App.tsx src/store/studio.ts src/lib/words.ts
git commit -m "Studio: run cards with progress, honest charge lines and the ready notice"
```

### Task B9: Gallery, viewer and actions

**Files:**
- Create: `src/components/studio/Grid.tsx`, `Grid.test.tsx`, `Viewer.tsx`, `Viewer.test.tsx`
- Modify: `src/components/studio/RunCard.tsx` (tiles, actions), `src/store/studio.ts` (`again`, `reuse`, `animate`, `editWithWords`, `useAsReference`, `star`, `remove`, `openViewer`, filters), `src/components/ShortcutsDialog.tsx` (viewer keys), `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
export function useOutputUrl(outputId: string, thumb: boolean): string | null;   // blob: URL from studioOutputBytes, revoked on unmount
export function Grid(props: { onOpen(outputId: string): void }): JSX.Element;
export function Viewer(props: { outputId: string; onClose(): void }): JSX.Element;
// store
again(run: Run): Promise<void>;               // same settings, new seed unless locked; goes through start (confirm rules apply)
reuse(run: Run, withSeed: boolean): void;     // fills the draft; never starts
animate(output: Output): void; editWithWords(output: Output): void; useAsReference(output: Output): Promise<void>;
```

- [ ] **Step 1: Write the failing tests**
```ts
it("arrow keys move, F stars, R reuses all, Shift+R reuses without seed, Esc closes the viewer only", () => {
  fireEvent.keyDown(window, { key: "ArrowRight" }); screen.getByText("2 / 4");
  fireEvent.keyDown(window, { key: "f" }); expect(api.studioStar).toHaveBeenCalledWith("o2", true);
  fireEvent.keyDown(window, { key: "R", shiftKey: true }); expect(useStudio.getState().drafts.image.values.seed).not.toBe(81723);
  fireEvent.keyDown(window, { key: "Escape" }); expect(onClose).toHaveBeenCalled(); expect(api.studioCancel).not.toHaveBeenCalled();
});
it("shows every setting, the cost, the time taken and the file", () => { screen.getByText("$0.04 · took 13.8 s"); });
it("Delete says what the audit log keeps", async () => {
  screen.getByText("The audit log keeps a line saying a run happened (model and cost), but not your prompt.");
});
it("Save as leaves the settings file off unless asked", async () => {
  // saveDialog mocked → path; api.studioExport("o1", path, false)
});
it("Reuse all fills the prompt and every setting and starts nothing", () => {});
it("Reuse on a retired model picks the closest one and says what changed", () => {
  // none in the family → "OpenRouter no longer offers this model."
});
it("Animate switches to Video with the picture as the start frame", async () => {
  // current video model without a start frame → "Veo 3.1 Lite takes no start frame. Use Veo 3.1 Fast instead?"
});
it("Edit puts the picture in the tray and focuses the prompt", () => {});
it("Copy falls back to Save as when the clipboard refuses", async () => {
  // navigator.clipboard.write rejects → "Copy is not available here. Use Save as… instead."
});
it("the grid filters by Pictures, Video, Starred and search", async () => { /* api.studioRuns called with the filter */ });
it("several can be selected and deleted together", () => {});
it("an SVG result is shown with an img element only", () => { expect(container.querySelector("object, iframe, embed")).toBeNull(); });
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/studio/Grid.test.tsx src/components/studio/Viewer.test.tsx`
Expected: FAIL (modules missing).

- [ ] **Step 3: Implement** Wireframe 4. Tiles use the thumbnail when `hasThumb`, else the original (GIF, SVG), else for video the length and the start frame. Viewer notes come from the model's notes (for example "Carries Google's SynthID watermark"). Copy draws non-PNG pictures to a canvas and writes `image/png` with `navigator.clipboard.write`. Viewer keys: ←/→, F, R, Shift R, A, S, C, Del, Space, Esc.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/studio src/store/studio.ts src/components/ShortcutsDialog.tsx src/lib/words.ts
git commit -m "Studio: gallery grid, full-size viewer and the actions on each result"
```

### Task B10: Settings → Studio and the endpoint badges

**Files:**
- Create: `src/components/settings/StudioSettings.tsx`, `StudioSettings.test.tsx`
- Modify: `src/components/SettingsDialog.tsx` (tab `studio` after Endpoints), `src/components/settings/endpointPresets.ts` (`media?: { image: boolean; video: boolean }`), `src/components/settings/Endpoints.tsx`, `Endpoints.test.tsx`, `e2e/m8_offline.py` (`"Studio"` after `"Endpoints"` in `SETTINGS_PAGES`), `src/lib/words.ts`

**Interfaces:**
- Produces: `export function StudioSettings(): JSX.Element;`, `export function abilitiesOf(kind: ProviderKind): string;` ("Chat · Pictures · Video").

- [ ] **Step 1: Write the failing tests**
```ts
it("says how much Studio holds and where", async () => {
  screen.getByText("Studio uses 1.2 GB in ~/harnessUI/studio (312 pictures, 14 videos)");
  screen.getByRole("button", { name: "Open folder" });
});
it("opening the page sends nothing", async () => { expect(api.studioRefreshModels).not.toHaveBeenCalled(); });
it("the daily limit is on at $10 and can be changed or turned off", async () => {
  // input "20" → api.studioSetPrefs({…, dailyLimitUsd: 20}); untick → dailyLimitUsd: null; "0.5" → "Choose an amount from $1 to $1,000."
});
it("the confirmation choices can be changed", () => {});
it("Delete stopped and failed runs asks first", async () => {});
it("each model list says when it was updated, and Update now refreshes it", async () => {});
it("changing the folder says existing results stay where they are", () => {});
// Endpoints.test.tsx
it("cards say what each company can do", () => {
  expect(abilitiesOf("openrouter")).toBe("Chat · Pictures · Video");
  expect(abilitiesOf("openai")).toBe("Chat · Pictures"); expect(abilitiesOf("anthropic")).toBe("Chat");
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/settings/StudioSettings.test.tsx src/components/settings/Endpoints.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement** the page from design §5 and §4: usage line and Open folder; Delete stopped and failed runs; *Ask before every video*; *Ask before runs of $X or more*; *Daily limit* (switch and amount); model lists with dates and Update now; gallery folder with *Change…* (folder picker through the dialog plugin) and the note "New results go to the new folder; existing ones stay where they are." `endpointPresets`: openrouter `{image: true, video: true}`, openai `{image: true, video: false}`.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components/settings src/components/SettingsDialog.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/settings src/components/SettingsDialog.tsx e2e/m8_offline.py src/lib/words.ts
git commit -m "Studio: its settings page, and what each endpoint can make"
```

### Task B11: Send to chat and Open in Studio

**Files:**
- Modify: `src/components/Composer.tsx`, `Composer.test.tsx` (`app:compose-images`), `src/components/MessageItem.tsx`, `MessageItem.test.tsx` (*Open in Studio*), `src/store/studio.ts` (`sendToChat`), `src/components/studio/Viewer.tsx`, `RunCard.tsx`, `src/lib/words.ts`

**Interfaces:**
- Produces:
```ts
// window event "app:compose-images", detail: ImageInput[] → Composer adds them, up to MAX_IMAGES (8)
sendToChat(output: Output): Promise<void>;    // studioOutputForChat → showChat() (new chat when none is open) → app:compose-images
```

- [ ] **Step 1: Write the failing tests**
```ts
it("Send to chat puts the picture in the message box", async () => {
  // api.studioOutputForChat → {mime:"image/png", fileName:"lighthouse-1.png", data:"…"}; view becomes "chat"; event detail has it
});
it("Send to chat is hidden for videos", () => {});
it("a picture over 20 MB cannot be sent and says why", async () => {});
// Composer.test.tsx
it("app:compose-images adds pictures up to 8", () => {});
// MessageItem.test.tsx
it("Open in Studio puts an attached picture in the tray", async () => {
  // click → api.studioAddRef({mime, fileName, data}) → drafts.image.refs has it; view "studio"
});
```

- [ ] **Step 2: Run to see them fail**
Run: `npx vitest run src/components/Composer.test.tsx src/components/MessageItem.test.tsx src/components/studio`
Expected: FAIL.

- [ ] **Step 3: Implement.** The picture goes through the normal send path (`save_images`: 20 MB, 8 pictures, type check, WebP conversion, copied into `attachments/`), so deleting it from the gallery never breaks the chat.

- [ ] **Step 4: Run to see them pass**
Run: `npx vitest run src/components`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/components/Composer.tsx src/components/Composer.test.tsx src/components/MessageItem.tsx src/components/MessageItem.test.tsx src/components/studio src/store/studio.ts src/lib/words.ts
git commit -m "Studio: send a picture to a chat, and open a chat picture in Studio"
```

### Task B12: Video playback (CSP) and poster frames

**Files:**
- Modify: `src-tauri/tauri.conf.json` (add `media-src blob:` after `img-src 'self' data: blob:;`), `src-tauri/src/commands.rs` (`capabilities_are_listed_explicitly`), `src/components/studio/Viewer.tsx`, `Grid.tsx`, `Viewer.test.tsx`

**Interfaces:**
- Produces: `export function capturePoster(video: HTMLVideoElement): string | null;` (base64 JPEG, longest side 512) and a call to `studioSaveThumb(outputId, base64)` the first time a video without a thumbnail plays.

- [ ] **Step 1: Write the failing tests**
```rust
// commands.rs: the pinned policy becomes
"default-src 'self'; script-src 'self'; connect-src 'self' ipc: http://ipc.localhost; \
 img-src 'self' data: blob:; media-src blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; \
 object-src 'none'; frame-src artifact://sandbox.localhost http://artifact.sandbox.localhost; \
 form-action 'none'; base-uri 'self'; require-trusted-types-for 'script'"
```
```ts
it("a video plays from a blob: address", () => { expect(screen.getByTestId("studio-video").getAttribute("src")).toMatch(/^blob:/); });
it("the first play saves a poster frame", async () => { /* canvas mocked → api.studioSaveThumb("o9", "…") once */ });
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib commands::tests::capabilities_are_listed_explicitly && npx vitest run src/components/studio/Viewer.test.tsx`
Expected: FAIL (the policy string differs; no poster call).

- [ ] **Step 3: Implement** the CSP change in `tauri.conf.json` and the poster capture (draw the current frame to a canvas scaled to 512 px, `toDataURL("image/jpeg", 0.8)`). A very large video stays on the blob route in 0.1.0; the main-window-only `studio:` scheme is left for later.

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib commands && npx vitest run src/components/studio`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/tauri.conf.json src-tauri/src/commands.rs src/components/studio
git commit -m "Studio: let videos play from blob: addresses, and save their poster frames"
```

---

## Phase C: Docs, privacy and version

### Task C1: Docs and their checks

**Files:**
- Modify: `README.md`, `ARCHITECTURE.md` (7.5 providers: `agent::media`; 12 storage: the gallery folder and V3; 13.6: a `| Studio |` row; 13.7: a *Studio (0.1.0)* bullet; 14 frontend: the Studio view; 17 testing: the new tests), `SECURITY.md` ("Secrets and privacy": prompts, added pictures, results, "OpenRouter passes it on", the key never goes to a file host; "The webview": `media-src blob:`; "Audit log": the Studio row keeps the prompt's length only), `docs/SPEC.md` (line 121), `docs/dev/0.1.0/README.md` (owner's words kept verbatim; links to this plan and the owner checklist), `scripts/docs_test.py`
- Test: `scripts/docs_test.py` (new class `StudioDocs`)

**Interfaces:** none (docs).

- [ ] **Step 1: Write the failing tests** (`scripts/docs_test.py`)
```python
class StudioDocs(unittest.TestCase):
    """What the docs say about Studio matches the code: hosts, money rules, audit row, CSP, scope note."""

    def test_13_6_has_a_studio_row_with_its_hosts(self):
        row = paragraph(section("### 13.6 Network use"), "| Studio |")
        for words in ("`openrouter.ai`", "`api.openai.com`", "guarded fetcher", "without the key"):
            self.assertIn(words, row)

    def test_the_money_rules_are_the_codes(self):
        spend = read("src-tauri/src/studio/spend.rs")
        for name, value in (("CONFIRM_AT_USD", "1.0"), ("DEFAULT_DAILY_LIMIT_USD", "10.0"), ("UNKNOWN_PRICE_RESERVE_USD", "5.0")):
            self.assertRegex(spend, rf"pub const {name}: f64 = {re.escape(value)};")
        for doc in ("SECURITY.md", "README.md"):
            self.assertIn("$10", read(doc), doc)
            self.assertIn("$1 or more", read(doc), doc)

    def test_the_audit_row_keeps_the_prompt_length_only(self):
        bullet = paragraph(section("### 13.7 Audit log"), "- *Studio* (0.1.0)")
        self.assertIn("length", bullet)
        self.assertIn("not the prompt", bullet)

    def test_the_spec_says_the_scope_changed(self):
        self.assertIn("(Changed in 0.1.0 at the owner's request: cloud image and video generation, see "
                      "docs/dev/0.1.0/README.md)", read("docs/SPEC.md"))

    def test_the_csp_the_docs_describe_is_the_apps(self):
        csp = json.loads(read("src-tauri/tauri.conf.json"))["app"]["security"]["csp"]
        self.assertIn("media-src blob:", csp)
        self.assertIn("media-src blob:", read("SECURITY.md"))

    def test_the_offline_test_visits_studio(self):
        self.assertIn('"Studio"', read("e2e/m8_offline.py"))
```
(`import json` at the top of the script.)

- [ ] **Step 2: Run to see them fail**
Run: `python3 -I scripts/docs_test.py`
Expected: FAIL (`expected one line starting with '| Studio |', found 0`, …).

- [ ] **Step 3: Write the docs.** The 13.6 row: `| Studio | The user clicks Create, Get the list, Update model lists or Test, or picks an OpenRouter picture model (its price) | the chosen endpoint: \`openrouter.ai\` or \`api.openai.com\`; a result on the company's file host through the guarded fetcher, without the key |`. README: a short "Studio" section in plain words (what it does, the $10 daily limit, confirmation for every video and any run of $1 or more, where files go). The SPEC line gets the annotation in the test, the way 0.0.8 annotated the updater lines. ARCHITECTURE 17 names `crates/agent/tests/media_endpoints.rs`, `media_redirects.rs`, `src-tauri/src/studio/manager_tests.rs`, `wire_tests.rs` and `e2e/m12_studio.py` (by hand). **Separate website task (other repository):** add the Studio row (what goes to OpenRouter or OpenAI, results stay on the computer, no telemetry) to the harnessui.site privacy policy page added in 0.0.8.

- [ ] **Step 4: Run to see them pass**
Run: `python3 -I scripts/docs_test.py`
Expected: PASS (`OK`).

- [ ] **Step 5: Commit**
```bash
git add README.md ARCHITECTURE.md SECURITY.md docs/SPEC.md docs/dev/0.1.0/README.md scripts/docs_test.py
git commit -m "Studio: architecture, security and readme, checked against the code"
```

### Task C2: CHANGELOG and version 0.1.0

**Files:**
- Modify: `CHANGELOG.md` (new `## 0.1.0 (<date>)` entry on top), `Cargo.toml` (`[workspace.package] version`), `Cargo.lock`, `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, `src-tauri/src/commands.rs` (version test), `scripts/docs_test.py`

**Interfaces:** none.

- [ ] **Step 1: Write the failing tests**
```rust
// src-tauri/src/commands.rs
#[test] fn the_version_is_the_same_everywhere() {
    let v = env!("CARGO_PKG_VERSION");
    assert_eq!(v, "0.1.0");
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let pkg: serde_json::Value = serde_json::from_str(include_str!("../../package.json")).unwrap();
    let lock: serde_json::Value = serde_json::from_str(include_str!("../../package-lock.json")).unwrap();
    assert_eq!(conf["version"], v); assert_eq!(pkg["version"], v);
    assert_eq!(lock["version"], v); assert_eq!(lock["packages"][""]["version"], v);
}
```
```python
# scripts/docs_test.py, in StudioDocs
def test_the_changelog_tells_users_what_0_1_0_does(self):
    entry = read("CHANGELOG.md").split("\n## ")[1]
    self.assertTrue(entry.startswith("0.1.0 ("))
    for words in ("Studio", "$10", "$1 or more", "### Added", "### Known limits"):
        self.assertIn(words, entry)
```

- [ ] **Step 2: Run to see them fail**
Run: `cargo test -p app --lib commands::tests::the_version_is_the_same_everywhere; python3 -I scripts/docs_test.py`
Expected: FAIL (`left: "0.0.8"`; the changelog's first entry is 0.0.8).

- [ ] **Step 3: Write the entry and bump the version.** The entry opens with a paragraph for users that quotes the owner's request, then *Added* (Studio and how to open it, the companies, controls that follow the model with Advanced and chips, prices before you run, confirmation and the $10 daily limit, videos that keep going after you close the app, the gallery folder, Send to chat / Open in Studio, the audit row without the prompt), *Changed* (Test on an OpenRouter or OpenAI endpoint also fetches its picture and video lists; endpoint cards say "Chat · Pictures · Video"), *Known limits* (OpenAI publishes no per-picture price for some 2.6 settings, so those runs say "price unknown" and ask; how long OpenRouter keeps a finished video is not documented; going back to 0.0.8 after 0.1.0 has opened the database is refused, because 0.0.8 cannot read schema 3; built and tested on Linux, Windows and macOS from CI). Then:
```bash
sed -i 's/^version = "0.0.8"$/version = "0.1.0"/' Cargo.toml
npm version 0.1.0 --no-git-tag-version
# src-tauri/tauri.conf.json: "version": "0.1.0"
cargo check -p app            # rewrites the workspace crates' versions in Cargo.lock
```

- [ ] **Step 4: Run to see them pass**
Run: `cargo test -p app --lib commands && python3 -I scripts/docs_test.py && python3 -I scripts/release_notes_test.py`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add CHANGELOG.md Cargo.toml Cargo.lock package.json package-lock.json src-tauri/tauri.conf.json src-tauri/src/commands.rs scripts/docs_test.py
git commit -m "Release 0.1.0: Studio"
```

---

## Phase D: End-to-end check against a mock provider

### Task D1: A whole run against a local OpenRouter mock

**Files:**
- Create: `src-tauri/src/studio/wire_tests.rs` (`#[cfg(test)] mod wire_tests;` in `studio/mod.rs`)

**Interfaces:**
- Consumes: the real `OpenRouterMedia`, `StudioManager`, `StoreProviders`, `Gallery`, `catalog`, `store::Store::open_in_memory()`, `store::MemorySecretStore`.
- Produces (test-only): `struct MockOpenRouter { fn start() -> Self; fn url(&self) -> String; fn count(&self, method: &str, path: &str) -> usize; fn requests(&self) -> Vec<(String, String)> }` on a `tokio::net::TcpListener`, serving the catalogue fixtures, a recorded `/endpoints` answer, `POST /api/v1/images` (two PNGs, `usage.cost` 0.04), `POST /api/v1/videos` (202 `vid_1`), `GET /api/v1/videos/vid_1` (pending twice, then completed with `usage.cost` 0.36) and `GET /api/v1/videos/vid_1/content?index=0` (a small MP4 starting with an `ftyp` box).

- [ ] **Step 1: Write the tests**
```rust
#[tokio::test] async fn a_picture_and_a_video_through_a_mock_openrouter() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::new(&mock).await;          // provider kind "openrouter" at mock.url() + "/api/v1", key in MemorySecretStore
    h.refresh_lists().await;
    assert_eq!(h.models().await.len(), 101);
    let pic = h.manager.start(h.picture("openai/gpt-image-2.6-flare", json!({"n": 2})), true).await.unwrap();
    let pic = h.wait(&pic.id, "done").await;
    assert_eq!(pic.outputs.len(), 2);
    assert!(h.gallery_file_exists(&pic.outputs[0].path) && h.sidecar_exists(&pic));
    let vid = h.manager.start(h.video("x-ai/grok-imagine-video-1.5-lite", json!({"duration": 4, "resolution": "480p"})), true).await.unwrap();
    h.wait(&vid.id, "waiting").await;
    let h = h.restart().await;                      // a new StudioManager on the same store and gallery
    h.manager.resume_interrupted().await;
    let vid = h.wait(&vid.id, "done").await;
    assert_eq!(vid.outputs[0].mime, "video/mp4");
    assert_eq!(mock.count("POST", "/api/v1/images"), 1);
    assert_eq!(mock.count("POST", "/api/v1/videos"), 1);
    assert!((h.manager.usage().await.unwrap().spent_today_usd - 0.40).abs() < 1e-9);
    assert!(h.audit_rows().await.iter().all(|r| !r.arguments.to_string().contains(&h.prompt)));
}
#[tokio::test] async fn opening_studio_makes_no_request() {
    // saved lists in the store; catalog::models_for, list runs, usage, prefs::load → mock.requests().is_empty()
}
#[tokio::test] async fn a_video_past_the_daily_limit_never_reaches_the_mock() {
    // spent 9.95 today; a 4 s 480p Grok Imagine Video 1.5 Lite run (estimate $0.09) → "studio_daily_limit"; no POST /api/v1/videos
}
#[tokio::test] async fn export_then_delete_leaves_the_audit_rows() {
    // studio export path gets the file; delete removes files, rows and sidecar; the two audit rows stay
}
```

- [ ] **Step 2: Run them**
Run: `cargo test -p app --lib studio::wire_tests`
Expected: PASS once Phases A–C are in. A failure names the owning task (A4/A5 adapter, A13/A14 manager, A9 gallery); fix it there, with a test in that task.

- [ ] **Step 3: Implement** only the mock and the harness (no product code). The harness builds `StudioDeps` with `StoreProviders`, a recording `StudioSink`, `GuardedFetch` (unused here), `SystemClock`, a temporary gallery and `PollPlan { first: 20 ms, second: 20 ms, third: 20 ms, .. }`.

- [ ] **Step 4: Run the whole workspace**
Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src-tauri/src/studio/wire_tests.rs src-tauri/src/studio/mod.rs
git commit -m "Studio: a whole picture and video run against a mock OpenRouter"
```

### Task D2: The real app against the mock, offline check, owner checklist

**Files:**
- Create: `e2e/m12_studio.py`, `docs/dev/0.1.0/owner-checklist.md`
- Modify: `e2e/m8_offline.py` (open Studio from the sidebar, both tabs, and Settings → Studio; still zero requests), `e2e/README.md`

**Interfaces:** `e2e/m12_studio.py` reuses `e2e/webdriver.py` and a `ThreadingHTTPServer` mock with the same answers as Task D1, on `http://127.0.0.1:<port>/api/v1`.

- [ ] **Step 1: Write the scenario.** Checks, each PASS or FAIL like `scripted_agent.py`: add an OpenRouter-kind endpoint at the mock (Test fetches the lists) → open Studio with Ctrl+G → create a picture (no dialog under $1) → two tiles appear → start a 4 s video (dialog "Make this video?") → restart the app while it is waiting → the card finishes by itself and the mock saw one `POST /api/v1/videos` → Save as with settings off → Delete → the file is gone and Settings → Audit log still shows two Studio rows without the prompt → set the daily limit to $1 and see "Daily limit reached".

- [ ] **Step 2: Run them**
Run: `python3 -I e2e/m12_studio.py && python3 -I e2e/m8_offline.py`
Expected: every check PASS; m8_offline sees no request while Studio and Settings → Studio are open. Both run by hand on Linux with tauri-driver, as the other e2e scripts do.

- [ ] **Step 3: Write the owner checklist** (`docs/dev/0.1.0/owner-checklist.md`): one real run on each company, under $1 in total: a low-quality GPT Image 2.6 Flare picture on OpenAI; a picture and a 4 s 480p Grok Imagine Video 1.5 Lite video on OpenRouter (about $0.09); one safety refusal on each. Compare each shown cost with the company's activity page. Save the real answers (OpenAI generation, its `moderation_blocked` error, OpenRouter's video status and content answers, and any redirect the content endpoint gives) over the hand-written fixtures in `crates/agent/tests/fixtures/media/` and re-run the tests. Note how long OpenRouter keeps a finished video. Play a video on Windows and on a Mac (blob playback and seeking).

- [ ] **Step 4: Run the full check list**
Run: `npm run lint && npm run typecheck && npx vitest run && cargo test --workspace && python3 -I scripts/docs_test.py`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add e2e/m12_studio.py e2e/m8_offline.py e2e/README.md docs/dev/0.1.0/owner-checklist.md
git commit -m "Studio: app-level scenario against a mock, offline check, owner checklist"
```

---

## Spec coverage

| Design section | Tasks |
|---|---|
| §1 What and where (navigation, shortcut, event, badge, ready notice, name) | B1, B2, B8 |
| §2 Companies, keys, endpoint badges, lists fetched only on request, greyed models | A1, A4, A7, A11, B3, B10 |
| §3 Easy but not dumbed down (simple layer, Advanced, chips, Extra fields, request preview, seeds, setups, references, Animate, Edit, carrying values) | A1, B4, B5, B6, B9 |
| §4 Jobs, cost, confirmation, daily limit, queue, never pay twice, resume, downloads, prices | A2, A3, A4, A5, A6, A13, A14, B7, B8 |
| §5 Gallery, viewer, actions, Settings → Studio, files on disk, V3, showing media, export, folder | A8, A9, A10, A15, B9, B10, B12 |
| §6 Send to chat, Open in Studio | A15, B11 |
| §7 Privacy, Cloud mark, no automatic requests, safety messages, audit row, docs | A13, A15, B2, B3, B7, C1, D1, D2 |
| §8 Architecture mapped to files | File Structure, all tasks |
| §9 Scope, risks, tests, owner checklist | Review Focus, D1, D2 |

## Decisions this plan makes where the design was open

- **Raw lists are cached, not parsed models:** `providerMediaModels:<id>` keeps OpenRouter's (or OpenAI's) answers, and the controls are built when read, so a parser fix applies to lists saved earlier. The trait therefore has `fetch_catalog`/`fetch_image_price` instead of `list_models`/`model_price`, plus `preview` for *See the exact request*.
- **`MediaError` says whether the company may have the request** (`NotSent`, `Answered`, `Unknown`), which drives both the never-retry rule and the charge line.
- **`send_json` itself does not retry** (the 408/5xx/529 retries live in `send_stream_request`); `post_once` is still needed for the streamed body and to make "at most once" explicit and tested.
- **Polling:** 5 s for the first minute, 10 s until 5 minutes, then 20 s; after 30 minutes the run stays `waiting`, unwatched, with *Check again*.
- **A queued run is never sent after a restart**; it becomes "Not sent … Nothing was charged."
- **Spending:** a run counts on the day it was created; queued and running runs count by their estimate; an unknown price reserves $5; cancelled-after-start and interrupted runs count as maybe charged; "not charged" failures count 0.
- **Duplicate presses:** a `clientToken` per Create press (kept in memory) plus a start lock, no schema change.
- **At 20,000 runs Studio refuses new runs** (`studio_gallery_full`) instead of deleting old results.
- **`studio_attach_to_chat` is replaced** by `studio_output_for_chat` plus the composer's `app:compose-images` event, so the picture goes through the normal send path (which already copies into `attachments/`); ChatView stays mounted, hidden, while Studio is shown, so drafts survive.
- **Added commands the design implied:** `studio_preview`, `studio_live`, `studio_add_ref`, `studio_ref_from_output`, `studio_ref_bytes`, `studio_delete_unfinished`, `studio_open_folder`.
- **Video models that need a video input** (4 on 2026-11-17) are listed greyed with the reason; `-1` (automatic length) is not offered; `size` on non-OpenAI picture models is an Extra-field hint.
- **OpenAI edits use a hand-built multipart body** (no new dependency). OpenAI per-picture prices fill only what OpenAI states; the rest is "price unknown" and asks.
- **Copy to clipboard uses the webview's clipboard API** (no new Tauri permission), with a Save as fallback.
- **Changing the gallery folder affects new results only;** nothing is moved in 0.1.0.
- **Routing in Advanced is offered for OpenRouter pictures only** (their `/endpoints` answer names the companies); OpenRouter chooses for video.
