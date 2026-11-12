# Item 6: usage view and finer offloading controls (design, work packages WP10a to WP10e)

Written 2026-11-12 for 0.0.6, in the style of `spec.md` Part 3. Nothing here is built yet.
Every developer follows `spec.md` Part 1 (rules), Part 2 (D1 to D12, storage, shared files)
and the amended D4 of Part 3b, Item 6.

**The owner's words (README item 6):** "on 0.0.6 i want more control options as well for the
models seeing how much of my gpu and cpu is currently being used and how offloading would
look like and options for select offloading btw. similar to how LM studios does it."

**The owner's decisions (spec Part 3b, Item 6):** Fine control is a closed section inside
each model's settings (`src/components/ModelSettings.tsx`); live usage is always visible
under the model name in the chat header. D4 is amended: the plain controls stay the default,
unchanged; layers on the graphics card, expert parts on the processor, conversation memory
on the graphics card, processor threads, fast attention and the memory to leave free appear
only under Fine control, each a typed value from a fixed list or range; batch sizes, slots,
rope, tensor overrides, free text and paths stay never shown.

**When it runs:** after WP1a, WP1b and WP2 are finished. WP10a, WP10b and WP10c can run in
parallel; WP10d and WP10e then run in parallel against the contract in section 2 (WP10e
mocks the commands until WP10d lands). Roughly 8,000 lines with tests, five developers.

Background reading, in this order: this file; `spec.md` Part 1, D1 to D4, D12, "Storage";
`item6-usage-offloading.md` (the research sketch this replaces); ARCHITECTURE.md 7.1 to 7.4
(runtime), 11 (fit check), 13.1 to 13.6 (trust, webview, policy, network).

---------------------------------------------------------------------------------------

## 0. What was checked on the pinned build (2026-11-12, this machine, processor only)

Run by hand with `.cache/llama-linux` (b12110, `version: 0.9.0-dev (build 12110, commit
cd959bae9)`) and `.cache/models/Qwen3-0.6B-Q8_0.gguf` (610 MiB), environment emptied
(`env -i`), nothing spawned from code. The captures are copied into the fixtures named in
WP10a.

1. **Every flag Fine control needs exists in b12110** (`llama-server --help`, and the
   [b12110 server README](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/server/README.md)
   flag table): `-ngl, --gpu-layers N|auto|all` (default `auto`), `-cmoe, --cpu-moe`,
   `-ncmoe, --n-cpu-moe N`, `-kvo/-nkvo`, `-t, --threads N` (default -1),
   `-fa, --flash-attn on|off|auto` (default auto), `-fit on|off` (**default on**),
   `-fitt, --fit-target MiB` (default 1024), `-fitc` (default 4096), `--log-jsonl`.
2. **`llama-fit-params` allocates nothing and answers fast.** Its measuring code loads the
   model with `no_alloc = true` and `load_mode = NONE` ([common/fit.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/common/fit.cpp),
   `common_get_device_memory_data_impl`). Measured: 0.20 to 0.37 s per run, peak memory
   65 MiB for a 610 MiB model (so the weights are never read), exit 0. On a graphics card
   this is **unverified** (owner checklist).
3. **Two output modes** ([fit-params.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/tools/fit-params/fit-params.cpp)):
   - fit mode prints the fitted arguments, e.g. `-c 4096 -ngl -1`, or for a mixture of
     experts `-c 4096 -ngl 48 -ot "blk\.14\.ffn_(up|down|gate)_(ch|)exps=CPU,…"`
     ([fit-params README](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/fit-params/README.md));
   - `-fitp on` measures the given arguments **as they are** and prints `device model
     context compute` in MiB, one line per device plus `Host`.
   With `--log-jsonl` both modes also print a structured record at the default verbosity:
   ```
   {"type":"fit_memory_breakdown","data":{"unit":"MiB","rows":[{"kind":"host","name":"Host","self":918,"model":157,"context":448,"compute":312},{"kind":"buffer_type","name":"AMX","self":674,"model":674,"context":0,"compute":0}]}}
   Host 157 448 312
   ```
   The plain `Host 157 448 312` line **leaves out 674 MiB** of repacked weights (the `AMX`
   buffer type; `CPU_REPACK` on ordinary processors): `common_fit_print` skips buffer
   types that belong to no model device. The JSON record has every row. Device rows also
   carry `description`, `total` and `free`. So the app reads the JSON record, never the
   plain lines.
4. **`--fit` changes only arguments left at their defaults** (fit.h: "only parameters that
   have the same value as in llama_default_model_params are modified"). With `-ngl N` set,
   or `--cpu-moe`/`--n-cpu-moe` set (they become tensor overrides), the fit stops with
   "n_gpu_layers already set by user" / "tensor_buft_overrides already set by user" and the
   server loads the arguments unchanged. As much as fits and a chosen placement therefore
   cannot be combined; Fine control's placement means `--fit off`.
5. **As much as fits moves mixture-of-experts experts first.** fit.cpp step 3: for a model
   with experts it first measures "only dense weights in device memory" (all experts on the
   processor) and then fills full layers back to front. So As much as fits already does what
   LM Studio's "Force model expert weights onto CPU" does, when needed.
6. **"offloaded N/M layers to GPU" is not printed at the default verbosity.** With `-lv 3`
   (the default, and what the app runs) stderr has no `load_tensors:` lines at all; with
   `-lv 4` it has `load_tensors: offloaded 0/29 layers to GPU` and per-buffer sizes. So
   `ServerEvent::Offloaded` (`crates/runtime/src/llama/protocol.rs`) never fires on b12110,
   and WP2's "Part of the model runs on the processor" note can never show today. The
   existing fixture `crates/runtime/tests/fixtures/server_stderr_cpu.txt` confirms it.
7. **The ready line already says where the model went.** The managed-child `ready` payload
   on stdout carries the memory llama.cpp allocated, per device, in bytes:
   ```
   "memory":{"n_ctx":4096,"devices":[{"kind":"host","name":"Host","model":165568512,"context":469762048,"compute":33572896},{"kind":"buffer_type","name":"AMX","model":707731456,"context":0,"compute":0}]}
   ```
   The same shape is documented for the router as `memory.effective` (server README,
   "Memory-based unloading"), and WP1a's contract test already reads it for Save memory.
8. **Projection versus allocation:** fit-params projected 312 MiB of working space
   (`compute`) where the server then allocated 32 MiB on the processor. The projection is on
   the safe side; the picture says "about".
9. **Image readers are not measured** (server README: "Multimodal projectors (mmproj) are
   not included in the measurements yet"; server.cpp: "TODO: mmproj is not in
   llama_get_memory_breakdown"). The picture adds the projector's file size, labelled.
10. **A crash spawns a debugger on Linux.** Given a file that is not a model, fit-params
    aborts (exit -6) and ggml's `ggml_print_backtrace` forks and runs `gdb` (then `lldb`)
    found on `PATH` ([ggml.c](https://github.com/ggml-org/llama.cpp/blob/b12110/ggml/src/ggml.c),
    lines 158 to 225); it attached gdb here. That is a process the app did not start
    through `crates/runtime`. `GGML_NO_BACKTRACE` (any value) turns it off; macOS uses it
    only with `GGML_BACKTRACE_LLDB`, which the scrubbed environment never passes.
11. **fit-params accepts the measuring arguments the app needs** (run here, exit 0):
    `-m M -c 8192 -np 1 --device none -ngl all --cpu-moe -nkvo -fa on -ctk q8_0 -ctv q8_0
    --fit off -fitp on --log-jsonl --offline`. A device that the build does not have is
    refused at once (`invalid device: Vulkan0`, exit 1, 0.02 s).
12. `llama-fit-params` is shipped on every system: `keep_files` in `llama.lock.json` keeps
    `llama-fit-params.exe` on Windows; macOS builds it as the second `externalBin`
    (ARCHITECTURE.md 7.1); `runtime::Backend::fit_params` already finds it
    (`crates/runtime/src/llama/backend.rs`). Whether the ROCm pack has it is unknown; without
    it the picture is the app's own estimate.

---------------------------------------------------------------------------------------

## 1. Decisions taken for this item (with the reason)

- **I6-1 Fine control is per model only.** Stored as a nested `fine` object in that model's
  `ModelLoadSettings`; `LocalModelSettings` (defaults for all models) does not change. The
  owner chose "inside each model's settings", and a layer count only means something for
  one model. Each Fine control field has an explicit `Auto` default variant meaning "what
  the app does without Fine control" (there is no global default to follow, so no
  `Default` variant).
- **I6-2 One source of truth for where the model goes.** Choosing layers or expert parts in
  Fine control is a *custom placement*. It replaces All of it / As much as fits, and the
  plain control then reads "Custom: 28 of 37 layers (set in Fine control)". Choosing All of
  it, As much as fits or Don't use it in the plain control clears the custom placement.
  Don't use it always wins: with it, the layers, expert parts and conversation memory
  controls are greyed out and add no argument (their saved values are kept).
- **I6-3 A custom placement launches with `--fit off`.** llama.cpp's fit would stop anyway
  (finding 4); saying it explicitly keeps the arguments honest and the log quiet.
- **I6-4 Save memory needs fast attention.** With Save memory on, Fast attention shows "On
  (Save memory needs it)" and cannot be changed; the arguments are Save memory's.
- **I6-5 The step-down learns two steps** (`supervisor.rs`), each once per load, the saved
  values unchanged (as D2c): fast attention On that fails the way Save memory can fail is
  retried with Automatic; a custom placement that runs out of graphics memory is retried
  with As much as fits. Then the existing order.
- **I6-6 The before-load picture has two tiers.** An instant estimate from the model file
  (the GGUF tensor table: exact weights per layer and per expert part; the conversation
  memory from the header; the working-space allowance of `fit.rs`), and, while Fine control
  is open, llama.cpp's own measurement (`llama-fit-params -fitp on --log-jsonl`) for the
  exact placement, which replaces the estimate when it arrives. For As much as fits the app
  works out the layer count itself with the same rule llama.cpp uses (finding 5) and labels
  it "About": fit mode would count the loaded model's memory as taken, and at load time
  llama.cpp measures again anyway.
- **I6-7 "Where the model went" comes from the ready line's memory report** (finding 7),
  not from log lines (finding 6). The layer count is exact for All, a chosen number and the
  processor; for As much as fits it is inferred from the bytes on the graphics card and the
  tensor table ("About"). This also brings back WP2's "part on the processor" note.
- **I6-8 `GGML_NO_BACKTRACE=1`** goes into the environment of llama-server and
  llama-fit-params (finding 10), so a crash never starts a debugger.
- **I6-9 Usage is read on this computer, on request only.** A new crate `crates/usage` with
  no new third-party crate (the `windows` 0.62 crate with two more features, `libc`,
  `objc2-core-foundation`, all already in `Cargo.lock`). No background polling: the webview
  asks every 1.5 seconds while the usage line is on screen; the reader lives on its own
  thread and ends 30 seconds after the last request.
- **I6-10 Memory amounts are shown the way Task Manager and Activity Monitor show them**:
  binary gigabytes written "GB" with one decimal (".0" dropped), so a 16 GB card reads
  "16 GB", not "17.2 GB". Download and file sizes keep the app's decimal "GB" (as the
  systems show file sizes).
- **I6-11 The usage line shows only when the chat's model is a local one** (loaded or not).
  A remote endpoint's work is not on this computer's graphics card.
- **I6-12 Windows reads PDH, the counters Task Manager's figures come from**, for every
  vendor (no ADLX: proprietary licence; no NVML on Windows). Graphics card busy is the
  busiest engine, Task Manager's rule ([GPUs in the Task Manager](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/)).
- **I6-13 No per-process graphics reading.** What the model itself takes comes from the
  ready line's memory report (exact and the same on every system).
- **I6-14 One measurement at a time.** A newer request cancels the older run; 20-second
  timeout; results cached (32 entries); output capped; a failure falls back to the estimate
  and says why.
- **I6-15 Splitting across several graphics cards stays out** (D4 amendment: `-ts`, `-sm`,
  `-mg` never shown) until someone has two cards to test.
- **I6-16 Modes (D12):** nothing here is a tool action. Fine control, the picture and the
  usage line are the same in Ask first, Allow all and Full access; no tool and no model
  output can read or change them (`set_setting`'s allow-list unchanged; the new commands are
  main-window only); the model is never told the usage figures.

---------------------------------------------------------------------------------------

## 2. Contract (shared by WP10a to WP10e)

Owners in brackets build these first in their package; dependants build against this text.

### 2.1 Runtime crate [WP10a] (`crates/runtime/src/llama/launch.rs`, `fit_params.rs`, `protocol.rs`)
```rust
/// Expert parts kept on the processor (mixture-of-experts models).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpertsOnCpu { #[default] None, All, FirstLayers(u32) }   // "none" | "all" | {"firstLayers":n}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlashAttn { #[default] Auto, On, Off }                       // "auto" | "on" | "off"

/// Fine control as launch arguments. Default = no argument at all (0.0.6 without item 6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FineLaunch {
    #[serde(default)] pub gpu_layers: Option<u32>,          // -ngl N
    #[serde(default)] pub experts_on_cpu: ExpertsOnCpu,     // --cpu-moe | --n-cpu-moe N
    #[serde(default)] pub kv_in_computer_memory: bool,      // -nkvo
    #[serde(default)] pub threads: Option<u16>,             // -t N
    #[serde(default)] pub flash_attn: FlashAttn,            // -fa on|off
    #[serde(default)] pub fit_margin_mib: Option<u32>,      // -fitt N (As much as fits only)
}
impl FineLaunch { pub fn custom_placement(&self) -> bool; /* gpu_layers.is_some() || experts != None */ }

// LaunchConfig gains:  #[serde(default)] pub fine: FineLaunch,

/// One row of a memory report, in bytes (fit-params' MiB are multiplied by 1,048,576).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRow {
    pub kind: MemoryRowKind,            // Device | Host | BufferType  ("device"|"host"|"bufferType")
    pub name: String,                   // "Vulkan0", "Host", "CPU_REPACK" (escaped, at most 64 chars)
    pub description: String,            // device rows: "Radeon RX 9070 XT" (escaped, at most 80 chars)
    pub total_bytes: Option<u64>,       // device rows only
    pub free_bytes: Option<u64>,        // device rows only, at the moment of the run
    pub model_bytes: u64,
    pub context_bytes: u64,
    pub compute_bytes: u64,
}
pub struct MemoryReport { pub n_ctx: Option<u32>, pub rows: Vec<MemoryRow> }

// protocol.rs: ChildState::Ready gains  memory: Option<MemoryReport>
// server.rs:   ServerEvent::Memory(MemoryReport), sent once per Ready that carries one.

// fit_params.rs
pub enum MeasureLayers { All, Count(u32) }            // the device is MeasureConfig::device
pub struct MeasureConfig {
    pub model: PathBuf, pub ctx: u32, pub device: Option<String>, pub layers: MeasureLayers,
    pub experts_on_cpu: ExpertsOnCpu, pub kv_in_computer_memory: bool,
    pub save_memory: bool, pub flash_attn: FlashAttn, pub hip_visible_devices: Option<String>,
}
pub fn build_measure_args(cfg: &MeasureConfig) -> Vec<String>;
pub fn parse_measure_output(stdout: &[String]) -> Result<Vec<MemoryRow>, FitParamsError>;
pub async fn measure(backend: &Backend, cfg: &MeasureConfig, timeout: Duration,
                     cancel: CancellationToken) -> Result<Vec<MemoryRow>, FitParamsError>;
pub enum FitParamsError { NoTool, Start { message: String }, Timeout, Failed { code: Option<i32> },
                          NoReport, Cancelled }

// stage.rs: LoadStage gains  RetryingWithoutFastAttention { backend: BackendKind }
//           JSON {"kind":"retryingWithoutFastAttention","backend":"vulkan"}
```

### 2.2 Models crate [WP10b] (`crates/models/src/gguf.rs`, `layers.rs`, `placement.rs`, `fit.rs`)
```rust
// gguf.rs
pub struct TensorEntry { pub name: String, pub bytes: u64 }
pub fn read_tensor_table(path: &Path) -> Result<Vec<TensorEntry>, GgufError>;

// layers.rs: what each part of the model weighs, from the tensor tables of all split files
pub struct LayerSize { pub dense_bytes: u64, pub expert_bytes: u64 }
pub struct LayerSizes {
    pub input_bytes: u64,        // token_embd.*  (always in computer memory)
    pub output_bytes: u64,       // output.*, output_norm.* (llama.cpp's last "layer"); tied
                                 // embeddings: the size of token_embd.weight
    pub layers: Vec<LayerSize>,  // blk.0 … blk.(block_count-1)
    pub other_bytes: u64,        // anything else (rope tables, nextn, unknown names)
}
impl LayerSizes {
    pub fn from_tables(tables: &[Vec<TensorEntry>], block_count: u64) -> Self;
    pub fn total(&self) -> u64;
    pub fn offload_units(&self) -> u32;       // layers.len() + 1, llama.cpp's "N/M layers"
}

// placement.rs: pure arithmetic, no I/O
pub struct PlacementInput<'a> {
    pub sizes: &'a LayerSizes,
    pub kv_per_layer: Vec<u64>,        // bytes per layer at the context and cache type chosen
    pub compute_gpu: u64, pub compute_cpu: u64, pub projector: u64,
}
pub enum Layers { All, Count(u32) }
pub struct Where { pub graphics: bool, pub layers: Layers, pub experts_on_cpu: ExpertsPick, pub kv_on_gpu: bool }
pub enum ExpertsPick { None, All, FirstLayers(u32) }
pub struct Parts { pub model: u64, pub context: u64, pub compute: u64, pub image_reader: u64 }
pub struct Split { pub gpu: Parts, pub cpu: Parts, pub layers_on_gpu: u32, pub units: u32,
                   pub expert_layers_on_cpu: u32 }
pub fn split(input: &PlacementInput, place: &Where) -> Split;
/// The rule of llama.cpp's fit (common/fit.cpp step 3) for one device: everything when it
/// fits; else, for experts, all experts on the processor and then full layers back to front;
/// else whole layers back to front. Returns (layers_on_gpu, expert_layers_on_cpu).
pub fn fit_layers(input: &PlacementInput, target_bytes: u64, moe: bool) -> (u32, u32);
/// The layer count that best explains `gpu_model_bytes`; None when no count is within one
/// layer's size.
pub fn infer_layers(sizes: &LayerSizes, gpu_model_bytes: u64, experts: ExpertsPick) -> Option<u32>;

// fit.rs (WP1b's file, one new function; no signature changes)
pub fn auto_ctx_with_margin(input: &FitInput, room: Room, spare_bytes: u64) -> Option<AutoCtx>;
// auto_ctx(input, room) == auto_ctx_with_margin(input, room, SPARE_BYTES)
```

### 2.3 Usage crate [WP10c] (`crates/usage`)
```rust
pub enum Gap { NoGraphicsCard, SystemGivesNone, DriverGivesNone, ReadFailed } // camelCase JSON
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reading<T> { Value { value: T }, Measuring, NotAvailable { why: Gap } }
pub struct Focus { pub description: Option<String>, pub total_hint_bytes: Option<u64> }
pub struct GraphicsSample { pub name: String, pub busy_percent: Reading<f32>,
                            pub memory_used_bytes: Reading<u64>, pub memory_total_bytes: Option<u64> }
pub struct MemUse { pub used_bytes: u64, pub total_bytes: u64 }
pub struct Sample {
    pub graphics: Reading<GraphicsSample>,   // NotAvailable{NoGraphicsCard} when none is known
    pub processor_percent: Reading<f32>,     // Measuring on the first sample
    pub cores: u32,                          // logical processors
    pub memory: Reading<MemUse>,
    pub unified: bool,                       // Apple silicon: graphics memory is computer memory
}
pub struct Sampler { /* platform state; lives on one thread */ }
impl Sampler { pub fn new() -> Self; pub fn sample(&mut self, focus: &Focus) -> Sample; }
```

### 2.4 App [WP10d] (`src-tauri/src/load_settings.rs`, `offload.rs`, `usage_cmd.rs`, `dto.rs`)
```rust
pub enum LayersChoice  { #[default] Auto, Count(u32) }              // "auto" | {"count":n}
pub enum ExpertsChoice { #[default] Auto, All, FirstLayers(u32) }   // "auto" | "all" | {"firstLayers":n}
pub enum KvPlace       { #[default] Auto, ComputerMemory }          // "auto" | "computerMemory"
pub enum ThreadsChoice { #[default] Auto, Count(u16) }              // "auto" | {"count":n}
pub enum FastAttention { #[default] Auto, On, Off }                 // "auto" | "on" | "off"
pub enum LeaveFree     { #[default] Auto, Mib(u32) }                // "auto" | {"mib":512|1024|2048|4096}
#[serde(rename_all = "camelCase")]
pub struct FineControl {       // every field #[serde(default)]; derives Default; is_default()
    pub layers: LayersChoice, pub experts: ExpertsChoice, pub kv: KvPlace,
    pub threads: ThreadsChoice, pub fast_attention: FastAttention, pub leave_free: LeaveFree,
}
// ModelLoadSettings gains:
//   #[serde(default, skip_serializing_if = "FineControl::is_default")] pub fine: FineControl,
// ResolvedLoad gains:  #[serde(default)] pub fine: runtime::FineLaunch,   (compared by same_launch)
// ModelLoadInfo gains: pub fine_facts: FineFacts,
pub struct FineFacts {
    pub units: Option<u32>,          // block_count + 1 ("of 37 layers"); None without a header
    pub expert_layers: Option<u32>,  // block_count when the model has experts, else None
    pub cores: u32,                  // logical processors of this computer
    pub graphics: bool,              // a graphics device is known for this load
    pub apple: bool,                 // Apple graphics (unified memory)
}

// offload.rs: the picture
pub enum PictureSource { LlamaCpp, Estimate { why: EstimateWhy } } // tag "kind"
pub enum EstimateWhy { Quick, NoTool, Failed, TimedOut, NotReadable }
pub enum Fits { Fits, Tight, WontFitGraphics, WontFitAtAll }
pub struct MemoryBar { pub name: String, pub total_bytes: u64, pub free_bytes: u64,
                       pub free_after_unload: bool, pub other_bytes: u64, pub need: Parts }
pub struct OffloadPreview {
    pub source: PictureSource, pub unified: bool,
    pub units: Option<u32>, pub layers_on_graphics: u32, pub layers_about: bool,
    pub expert_layers_on_processor: Option<u32>,
    pub graphics: Option<MemoryBar>,       // None on the processor only
    pub computer: MemoryBar,
    pub graphics_limit_bytes: Option<u64>, // Apple: the most Apple graphics may use
    pub fits: Fits, pub custom: bool,
}
// dto.rs: RuntimeStatusDto gains  pub placement: Option<Placement>  (None until Ready)
pub struct GraphicsUse { pub name: String /* device description */, #[serde(rename = "use")] pub parts: Parts }
pub struct Placement {
    pub graphics: Option<GraphicsUse>,     // JSON {"name":…, "use":{…}}
    pub computer: Parts,
    pub units: Option<u32>, pub layers_on_graphics: Option<u32>, pub layers_about: bool,
    pub custom: bool,
}
```
**Commands** (main window only, `crate::commands::only_main_window(window.label())?`, in
`generate_handler!`, `src-tauri/commands.list`, `allow-<name>` in
`src-tauri/capabilities/main.json`):
- `offload_preview(path: String, overrides: ModelLoadSettings, measure: bool) -> OffloadPreview`
  validates `overrides` exactly as `set_model_load_settings` does, saves nothing.
- `usage_now() -> usage::Sample`

### 2.5 Frontend [WP10e] (`src/lib/types.ts`, `src/lib/ipc.ts`)
```ts
export type LayersChoice = "auto" | { count: number };
export type ExpertsChoice = "auto" | "all" | { firstLayers: number };
export type KvPlace = "auto" | "computerMemory";
export type ThreadsChoice = "auto" | { count: number };
export type FastAttention = "auto" | "on" | "off";
export type LeaveFree = "auto" | { mib: 512 | 1024 | 2048 | 4096 };
export interface FineControl { layers: LayersChoice; experts: ExpertsChoice; kv: KvPlace;
  threads: ThreadsChoice; fastAttention: FastAttention; leaveFree: LeaveFree }
// ModelLoadSettings gains  fine?: FineControl   (absent = every field "auto")
export interface FineFacts { units: number | null; expertLayers: number | null; cores: number;
  graphics: boolean; apple: boolean }
export interface Parts { model: number; context: number; compute: number; imageReader: number }
export interface MemoryBar { name: string; totalBytes: number; freeBytes: number;
  freeAfterUnload: boolean; otherBytes: number; need: Parts }
export type PictureSource = { kind: "llamaCpp" }
  | { kind: "estimate"; why: "quick" | "noTool" | "failed" | "timedOut" | "notReadable" };
export interface OffloadPreview { source: PictureSource; unified: boolean; units: number | null;
  layersOnGraphics: number; layersAbout: boolean; expertLayersOnProcessor: number | null;
  graphics: MemoryBar | null; computer: MemoryBar; graphicsLimitBytes: number | null;
  fits: "fits" | "tight" | "wontFitGraphics" | "wontFitAtAll"; custom: boolean }
export interface Placement { graphics: { name: string; use: Parts } | null; computer: Parts;
  units: number | null; layersOnGraphics: number | null; layersAbout: boolean; custom: boolean }
// RuntimeStatus gains  placement: Placement | null;   LoadStage gains retryingWithoutFastAttention
export type UsageGap = "noGraphicsCard" | "systemGivesNone" | "driverGivesNone" | "readFailed";
export type Reading<T> = { kind: "value"; value: T } | { kind: "measuring" }
  | { kind: "notAvailable"; why: UsageGap };
export interface UsageSample { graphics: Reading<{ name: string; busyPercent: Reading<number>;
  memoryUsedBytes: Reading<number>; memoryTotalBytes: number | null }>;
  processorPercent: Reading<number>; cores: number;
  memory: Reading<{ usedBytes: number; totalBytes: number }>; unified: boolean }
// ipc.ts: offloadPreview(path, overrides, measure): Promise<OffloadPreview>; usageNow(): Promise<UsageSample>
```

---------------------------------------------------------------------------------------

## 3. Fine control: the controls, their values and their arguments

Shown only in "Settings for this model", in a closed `<details>` section **Fine control**
after **More options**. The help lines below are exact. On a Mac "graphics card" reads
"Apple graphics" in labels and help, as D4 does.

| # | Label | Choices (typed value) | Shown when |
|---|---|---|---|
| 1 | **Layers on the graphics card** | Automatic (follows Use the graphics card) · Choose how many: slider 0 … units, "28 of 37 layers" (`LayersChoice`) | a graphics device is known |
| 2 | **Expert parts on the processor** | Automatic · All of them · Some: slider 1 … expert layers, "the first 20 of 48 layers" (`ExpertsChoice`) | the model has experts |
| 3 | **Conversation memory on the graphics card** | Yes, faster (Automatic) · No, keep it in computer memory (`KvPlace`) | a graphics device is known |
| 4 | **Processor threads** | Automatic · Choose how many: slider 1 … cores, "8 of 16" (`ThreadsChoice`) | always |
| 5 | **Fast attention** | Automatic · On · Off (`FastAttention`) | always |
| 6 | **Leave free on the graphics card** | Automatic (1 GB) · 0.5 GB · 1 GB · 2 GB · 4 GB (`LeaveFree`) | a graphics device is known |

Help lines:
1. "The model is built from layers. Layers on the graphics card run much faster than on the
   processor, as long as they fit in its memory. The rest run on the processor."
2. "This model is a mixture of experts: it has many expert parts but uses only a few of them
   for each word. Keeping expert parts on the processor frees a lot of graphics memory and is
   often only a little slower. Automatic keeps them on the graphics card with All of it, and
   moves them first when As much as fits needs room."
3. "Where the model keeps the chat it is working on. On the graphics card is faster. Keeping
   it in computer memory leaves more room for layers on the graphics card, but replies slow
   down as the chat grows."
4. "How many processor cores the model may use for the parts that run on the processor.
   Automatic is usually fastest. Fewer keeps the computer more responsive for other work."
5. "A faster way for the model to look back over the chat that also uses less memory.
   Automatic turns it on where the graphics card supports it."
6. "As much as fits leaves this much graphics memory free for the screen and other programs,
   such as games or video. More free memory means fewer layers on the graphics card."

Notes under a control (text, never only a tooltip):
- With Use the graphics card = Don't use it, under 1, 2, 3 and 6: "Not used while Use the
  graphics card is Don't use it."
- With a custom placement, under 6: "Not used while you choose the layers or expert parts
  yourself."
- With Save memory on, control 5 shows the single choice "On (Save memory needs it)",
  disabled.
- Below the controls: **Reset fine control** (a text button; sets all six to Automatic).

### 3.1 Arguments (`build_args`, after the context and before Save memory)
```
-c C -np 1 --device D <placement> [--cpu-moe | --n-cpu-moe K] [-nkvo] [-t T]
[-fa on | -fa off | -fa on -ctk q8_0 -ctv q8_0] [-lm mmap+mlock] --cache-ram R [--reasoning-budget B]
```
| GPU use (resolved) | device | Fine control | `<placement>` and extras |
|---|---|---|---|
| Off, or no device | — | anything | `-ngl 0 --fit off` with `--device none`; no experts, `-nkvo`, `-fitt` |
| All | yes | no custom placement | `-ngl all --fit off` |
| Fit | yes | no custom placement | `--fit on`, plus `-fitt M` when Leave free is set |
| All or Fit | yes | layers N | `-ngl N --fit off` |
| All or Fit | yes | experts only | `-ngl all --fit off` + `--cpu-moe` or `--n-cpu-moe K` |
| All or Fit | yes | layers N and experts | `-ngl N --fit off` + `--cpu-moe` or `--n-cpu-moe K` |
Always, independent of the placement: `-t T` when threads are chosen; `-fa on` / `-fa off`
when fast attention is chosen and Save memory is off; Save memory gives `-fa on -ctk q8_0
-ctv q8_0` whatever Fast attention says. `-nkvo` only with a device. Every value is a number
from the typed choice; nothing else reaches the command line.

### 3.2 What the resolver does with stored values (`load_settings::resolve`)
- Layers above `units` launch as `-ngl all`; experts on a model without experts are dropped;
  experts `FirstLayers(k)` above `expert_layers` become `All`; threads above this computer's
  cores become the cores (settings copied from another computer).
- Automatic context (D2b) with a custom placement uses the partial room (as As much as fits
  does) and, with Leave free set, that margin instead of `SPARE_BYTES`
  (`auto_ctx_with_margin`).
- `same_launch` compares `fine`, so a change shows "Settings changed. Restart the model to use
  them." and the no-op check sees it.

### 3.3 Step-down (supervisor), each step once per load
1. Save memory off (existing, D2c).
2. **New:** fast attention On (Save memory off) and a failure Save memory could also cause
   (`LoadFailure::save_memory_may_be_the_cause`) → retry with fast attention Automatic,
   `LoadStage::RetryingWithoutFastAttention`. Words: "Fast attention doesn't work with this
   graphics card, so it was turned off for this load."
3. As much as fits on a graphics-card out-of-memory, **now also when the placement is
   custom**: layers and experts cleared, `gpu = Fit` (Leave free kept),
   `LoadStage::RetryingWithLessGpu` (existing words).
4. The next backend down (existing); on the processor the placement adds nothing (3.1).

---------------------------------------------------------------------------------------

## 4. The before-load picture

**Where:** at the top of Fine control, under the heading "Where the model goes". The
`<summary>` of the closed section carries the one-line result, so it is useful closed:
"Fine control · All 37 layers on the graphics card" (plus " · changed" when any Fine control
value is set). When the picture says it will not fit, the warning line also shows outside
Fine control, above the buttons.

**How it is worked out** (`src-tauri/src/offload.rs`):
1. Resolve the unsaved draft for this computer (`load_settings::resolve`, with `fine`).
2. Facts, cached per file version (as `HEADER_CACHE`): header (block count, expert count,
   conversation memory per layer for f16 and q8_0) and `LayerSizes` from the tensor tables
   of every split file. Without a readable header: `source = estimate { notReadable }`,
   layers unknown, bars from the file size only.
3. Room: graphics total and free now from the usage sampler (Windows PDH with DXGI totals,
   Linux sysfs or NVML), else the device row of the measurement, else the probe; computer
   memory from `models::system_memory()`. The loaded model is replaced by any load (one model
   at a time), so its bytes from `RuntimeStatusDto.placement` count as free on both bars,
   and the bar says "free once the loaded model is unloaded". Margin = Leave free (Automatic:
   1,024 MiB, llama.cpp's default).
4. Placement: processor (Off, no device); custom (exact layers and experts); All (all
   units); Fit (`placement::fit_layers` with target = free − margin → "About").
5. Instant estimate: `placement::split` with the working-space allowance of `fit.rs`
   (`compute_bytes`), the projector's file size as "Image reader" on the graphics card when
   it is loaded with the model. Returned at once with `measure = false`
   (`source = estimate { quick }`).
6. With `measure = true` (Fine control open): one `llama-fit-params` run for exactly that
   placement (section 2.1; Fit uses the layer and expert counts of step 4). Device rows go to
   the graphics bar; Host and every buffer-type row go to the computer bar; the projector is
   added as in step 5. Cached by (file, size, modified time, backend directory, device,
   context, cache type, fast attention, conversation memory place, layers, experts). Falls
   back to step 5 with the reason (`noTool`, `failed`, `timedOut`).
7. Verdict: `wontFitAtAll` when the computer part exceeds free computer memory less
   `RAM_RESERVE_BYTES` (and the graphics part cannot take it); `wontFitGraphics` with All or
   a custom placement when the graphics part exceeds free graphics memory; `tight` when it
   fits but not with the margin; else `fits`. As much as fits is never `wontFitGraphics`.
8. Apple silicon (`unified`): one bar for the computer's memory, with the most Apple
   graphics may use (the Metal working set, from the probe's `MTL0` total) as a marker; the
   graphics part must fit under the marker.

**Fit-params run** (`runtime::fit_params::measure`): `SpawnSpec` through
`crates/runtime/src/spawn.rs` on `backend.fit_params` in `backend.dir`, environment
`backend.base_env()` + `HIP_VISIBLE_DEVICES` for ROCm + `GGML_NO_BACKTRACE=1` (no
`LLAMA_ARG_*`, no tokens), stdin null, stdout read for at most 512 lines of at most 64 KiB
(only lines starting with `{` are parsed; the last `fit_memory_breakdown` record wins),
stderr kept as a 40-line tail for the log, killed on timeout (20 s) or cancel. Arguments
(all fixed words, numbers, the model path and the probe's device name):
```
-m M -c C -np 1 --device D (-ngl all | -ngl N | -ngl 0) [--cpu-moe | --n-cpu-moe K] [-nkvo]
[-fa on | -fa off] [-ctk q8_0 -ctv q8_0] --fit off -fitp on --log-jsonl --offline
```
One run at a time app-wide (a newer request cancels the older one). The webview asks again
600 ms after the last change, never more than once per change.

---------------------------------------------------------------------------------------

## 5. Live usage

**Command:** `usage_now()` hands the request to one reader thread (`src-tauri/src/usage_cmd.rs`)
that owns a `usage::Sampler`, takes a sample and answers within 2 s (else
`readFailed` for every figure). The thread is started by the first request and ends 30 s
after the last one, closing every handle. Rates (busy percentages) are the difference to the
previous sample; the first sample reports `measuring`, and a gap of more than 10 s starts
again. The focus device is the loaded model's device, else the device a load would use
(`load_settings::Machine`), with its description and total from the probe.

| Figure | Windows | macOS (Apple silicon) | Linux |
|---|---|---|---|
| Graphics card busy | PDH `\GPU Engine(*)\Utilization Percentage`: sum per engine over processes, busiest engine of the focus adapter (Task Manager's rule) | IORegistry `IOAccelerator` → `PerformanceStatistics` → `Device Utilization %` | AMD: sysfs `…/device/gpu_busy_percent`; NVIDIA: NVML `nvmlDeviceGetUtilizationRates`; else not available |
| Graphics memory used / total | PDH `\GPU Adapter Memory(*)\Dedicated Usage` of the focus adapter; total from DXGI `DXGI_ADAPTER_DESC1.DedicatedVideoMemory`; adapter matched by LUID and by description | `In use system memory`; total = the probe's Metal working set | AMD: `mem_info_vram_used` / `mem_info_vram_total`; NVIDIA: `nvmlDeviceGetMemoryInfo` |
| Processor busy | PDH `\Processor Information(_Total)\% Processor Utility` capped at 100 (what Task Manager shows), else `GetSystemTimes` | `host_statistics(HOST_CPU_LOAD_INFO)` | `/proc/stat` first `cpu` line |
| Computer memory | `GlobalMemoryStatusEx` | `models::system_memory()` (`hw.memsize`, `host_statistics64`) | `/proc/meminfo` |

- **No admin rights, no new crate:** PDH (`windows` feature `Win32_System_Performance`), DXGI
  enumeration without creating a device (`Win32_Graphics_Dxgi`), IOKit through a few
  `extern "C"` declarations (`IOServiceMatching`, `IOServiceGetMatchingServices`,
  `IOIteratorNext`, `IORegistryEntryCreateCFProperty`, `IOObjectRelease`) linked with
  `#[link(name = "IOKit", kind = "framework")]` and `objc2-core-foundation` for the
  dictionary. Read only the one `PerformanceStatistics` property with
  `IORegistryEntryCreateCFProperty`, never all properties (reported to cost much more
  processor time when called often), NVML through `libc::dlopen("libnvidia-ml.so.1")`
  on Linux only (a missing library is `driverGivesNone`).
- **Matching the focus device:** Windows by DXGI description equal to the probe's
  description (the RX 9070 XT, not the Ryzen's integrated "AMD Radeon(TM) Graphics"), else
  the adapter with the most dedicated memory; Linux AMD by the largest `mem_info_vram_total`;
  NVML by name, else index 0.
- **Not available per figure:** no graphics device at all → `noGraphicsCard` (the line then
  leaves graphics out); counters or keys missing → `systemGivesNone`; NVIDIA on Linux
  without the NVML library → `driverGivesNone`; a call that fails → `readFailed`.
- **What the model itself takes** comes from `RuntimeStatusDto.placement` (the ready line's
  report), not from a per-process reading (I6-13).
- **Where the model went** (after Ready): `Placement` built by `runtime_manager` from
  `ServerEvent::Memory`: device rows → graphics; Host and buffer types → computer;
  `layers_on_graphics` exact for All (units), a chosen count and the processor (0), else
  `placement::infer_layers` ("About"); `custom` from the launch. It also fills
  `offloaded_layers` / `total_layers`, so WP2's partial note works again, except with a
  custom placement (the owner chose it; no note).

---------------------------------------------------------------------------------------

## 6. UI text and layout

### 6.1 The usage line (chat header, under the model name)
In `ChatView.tsx` the header's `<ModelPicker />` is wrapped in a column with the new
`<UsageLine />` under it (`flex min-w-0 flex-col`); the line is 11px, muted, `tabular-nums`,
one line, truncated, aligned with the picker's text (`px-2.5`). Shown when the chat's model is
local (I6-11), loaded or not, also while it loads. Polled every 1,500 ms while the page is
visible (`document.visibilityState`) and the line is mounted; never otherwise.

By the header's width (its `@container`):
- `@5xl` and wider: "Graphics card 63% · 11.2 of 16 GB · Processor 12% · Memory 14.1 of 32 GB"
- `@3xl` to `@5xl`: "Graphics card 63% · 11.2/16 GB · Processor 12% · Memory 14/32 GB"
- `@xl` to `@3xl`: "Graphics card 63% · Processor 12% · Memory 44%"
- narrower: three icons (lucide `Gpu`, `Cpu`, `MemoryStick`) each followed by its percentage;
  each icon has `aria-label` and `title` in words ("Graphics card 63% busy").
- Mac: "Apple graphics 40% · Processor 12% · Memory 11.2 of 16 GB" (graphics memory is part
  of Memory and is named in the menu).
- First sample: "…" in place of a percentage. Not available: "Graphics card not available"
  (narrow: the icon with "–"). No graphics card: the graphics part is left out.

The whole line is one button (`aria-label="Computer usage"`) that opens a menu:
```
This computer right now
Graphics card · AMD Radeon RX 9070 XT
  Busy: 63%
  Memory: 11.2 of 16 GB in use by all programs
  The model: 9.8 GB (model 8.1 GB, conversation memory 1.2 GB, working space 0.5 GB)
Processor: 12% busy · 16 cores
Memory: 14.1 of 32 GB in use · the model: 0.6 GB
Where the model went: 35 of 37 layers on the graphics card, 2 on the processor.
Read on this computer only while you can see it. Nothing is sent anywhere.
[Model settings…]
```
Variants: "All 37 layers on the graphics card." · "About 30 of 37 layers on the graphics
card, the rest on the processor." · "On the processor only." · MoE: "Expert parts of 20
layers are on the processor." · no model loaded: the "The model" and "Where the model went"
rows are left out. Not available, as visible text: `systemGivesNone` "This computer doesn't
tell apps how busy its graphics card is."; `driverGivesNone` "The NVIDIA driver's
monitoring library isn't installed."; `readFailed` "Couldn't read this just now."; Mac graphics
memory: "Apple graphics: 6.1 GB of the computer's memory".

### 6.2 The picture (top of Fine control)
```
Where the model goes
Graphics card · AMD Radeon RX 9070 XT · 16 GB
[████ model ███ conversation memory ██ working space ▒ other programs ░ free]
Model 8.1 GB · Conversation memory 1.2 GB · Working space 0.5 GB · Other programs 0.9 GB · Free 5.3 GB
Needs about 9.8 GB. 15.1 GB is free now.
Computer memory · 32 GB
[██ model ▒▒▒▒▒ other programs ░░░░ free]
Model 0.6 GB · Other programs 12.0 GB · Free 19.4 GB
28 of 37 layers on the graphics card, 9 on the processor.
From llama.cpp's own calculation for these settings.
```
- Bars are stacked `div`s; every part has its word and number in the legend (never colour
  alone); the bar has `role="img"` and an `aria-label` with the whole sentence. "Image
  reader 0.9 GB" appears when the model has one.
- "15.1 GB is free now." becomes "15.1 GB is free once the loaded model is unloaded." when the
  loaded model's memory was counted.
- Layer lines: "All 37 layers on the graphics card." · "28 of 37 layers on the graphics card,
  9 on the processor." · "About 30 of 37 layers on the graphics card. The rest run on the
  processor. The exact number is decided when the model loads." · "On the processor only." ·
  MoE addition: "Expert parts of 20 layers stay on the processor."
- Source lines: "From llama.cpp's own calculation for these settings." · "Rough estimate from
  the model file. This backend has no calculation tool." (`noTool`) · "Rough estimate from
  the model file. llama.cpp couldn't work it out for this file." (`failed`) · "Rough
  estimate from the model file. llama.cpp took too long to work it out." (`timedOut`) ·
  "Rough estimate. The model file's details couldn't be read." (`notReadable`). While a
  measurement runs, the estimate stays shown, dimmed, with "Working it out…".
- Apple: one bar "Memory · shared by the processor and Apple graphics · 16 GB" with a marker
  and the caption "Apple graphics can use up to 10.7 GB".
- Warnings (`role="alert"`, also shown outside Fine control):
  - `wontFitGraphics`: "This probably won't fit in the graphics card's memory: it needs about
    17.2 GB and 15.1 GB is free. Choose fewer layers, keep expert parts on the processor, or
    turn on Save memory."
  - `wontFitAtAll`: "This probably needs more memory than this computer has free: about 38.0
    GB, with 30.2 GB free."
  - `tight`: "It only just fits. Programs that use the graphics card, such as games or video,
    may stop it from loading."
- **Load anyway:** Apply and restart and Load with these settings check the latest preview.
  `wontFitGraphics` asks, in the same dialog as WP2's questions (no default focus on the
  button that loads): "This
  probably won't fit in the graphics card's memory. If it doesn't, the model won't load, or
  part of it moves to the processor, which is slower." — **Cancel** / **Load anyway**.
  `wontFitAtAll` asks WP2's existing question ("This model probably needs more memory than
  this computer has. Load it anyway?").

### 6.3 The plain control with a custom placement
"Use the graphics card" shows one more, disabled and selected option "Custom: 28 of 37
layers (set in Fine control)" (with experts only: "Custom: expert parts on the processor (set
in Fine control)"), and its help line becomes: "Fine control sets where the model goes.
Choose another option here to go back to it." Choosing All of it, As much as fits or Don't
use it writes `fine.layers = "auto"` and `fine.experts = "auto"` with it.

---------------------------------------------------------------------------------------

## Part 3c. Work packages

### WP10a. Fine control arguments, step-down, fit-params runner and the ready report (`crates/runtime`)
**Owns:** extends WP1a's `crates/runtime/src/llama/launch.rs`,
`crates/runtime/src/llama/server/supervisor.rs`, `crates/runtime/src/llama/stage.rs`,
`crates/runtime/src/llama/server.rs` (the Memory event), `crates/runtime/tests/llama_server.rs`
(append), and WP1a's optional `crates/runtime/src/llama/fit_params.rs` (WP1a did not build it;
if it exists at the start, extend it); `crates/runtime/src/llama/protocol.rs` (no other 0.0.6
owner); new fixtures `crates/runtime/tests/fixtures/fit_params_measure_cpu.jsonl` (the capture
of section 0, item 3), `fit_params_measure_vulkan.jsonl` (written by hand from the README's
table and marked "shape only until the owner's capture"), `managed_ready_memory_vulkan.txt`
(same); re-export lines in `crates/runtime/src/lib.rs` and `src/llama/mod.rs`.
**Build:**
- `ExpertsOnCpu`, `FlashAttn`, `FineLaunch`, `LaunchConfig.fine` and the arguments of
  section 3.1, in that order; `Debug` prints `fine`.
- `build_env` / `build_env_on` and the fit-params environment set `GGML_NO_BACKTRACE=1` (I6-8).
- Supervisor: the two new steps of section 3.3; `LoadStage::RetryingWithoutFastAttention`.
- `fit_params.rs`: `build_measure_args`, `parse_measure_output` (JSON record; rows in bytes;
  names and descriptions escaped and capped; refuse a record without rows), `measure` as in
  section 4, with its own tail and caps.
- `protocol.rs`: `ChildState::Ready.memory` from `payload.memory` (unknown kinds ignored,
  missing numbers as 0, at most 16 rows); `server.rs` sends `ServerEvent::Memory` once per
  Ready that carries it.
**Tests:** `args_are_exact_for_every_fine_control` (a table: each row of 3.1, Save memory with
Fast attention Off, Off with every Fine control set adds nothing but `-t` and `-fa`, no device
behaves as Off, layers 0 with a device, `-fitt` only with Fit), `fine_launch_old_json_reads`
(a 0.0.6-without-item-6 `LaunchConfig` JSON reads with defaults; the new JSON reads into a
struct without `fine`), `env_turns_off_debugger_backtraces`, supervisor:
`fast_attention_failure_retries_once_with_automatic`,
`custom_placement_out_of_memory_retries_with_as_much_as_fits`, `the_chain_still_ends` (every
retry once, with all settings on); `measure_args_are_exact` (every combination),
`parses_the_captured_measure_output` (Host + AMX rows, MiB to bytes, the plain line ignored),
`parses_a_device_row_with_total_and_free`, `a_record_without_rows_is_no_report`,
`hostile_names_are_escaped_and_capped`; `ready_line_memory_is_read` (the existing
`managed_stdout.txt` and the Vulkan shape). Contract tests against the pinned binary on the
processor (`LLAMA_SERVER_BIN`, `TEST_GGUF`; fit-params is its sibling):
`fit_params_measures_without_reading_the_weights` (rows parse; peak memory of the child below
half the model's size, from `getrusage(RUSAGE_CHILDREN)` on Linux), `fit_params_q8_0_halves_the_context`,
`fit_params_on_a_broken_file_fails_without_a_debugger` (exit is not 0, `measure` reports
`Failed`, stderr has no `gdb` or `LWP` line), `fine_control_flags_start` (a server with `-t 2
-fa off --cpu-moe -nkvo` on the processor reaches Ready), `ready_reports_where_the_model_went`.
**Unverified (owner):** fit-params timing and "allocates nothing" on Vulkan and Metal; the
device row's total and free on the RX 9070 XT.
**Size:** about 1,500 lines with tests.

### WP10b. Tensor table and placement arithmetic (`crates/models`)
**Owns:** `crates/models/src/gguf.rs` (no other 0.0.6 owner; additions only), new
`crates/models/src/layers.rs` and `crates/models/src/placement.rs`, one new function in WP1b's
`crates/models/src/fit.rs` (`auto_ctx_with_margin`; `auto_ctx` calls it), re-exports in
`crates/models/src/lib.rs`.
**Build:**
- `read_tensor_table`: continue after the metadata: per tensor the name (at most 256 bytes),
  `n_dims` (at most 4), the dimensions, the type and the offset; data start = position
  aligned to `general.alignment` (default 32). Sizes from the gaps between sorted offsets,
  the last one to the end of the file. Refuse a tensor count above 65,536, offsets outside
  the file or not increasing, and names that are not UTF-8. No type table is needed.
- `LayerSizes::from_tables`: group `token_embd.*` (input), `output.*` and `output_norm.*`
  (output; with tied embeddings the output weighs `token_embd.weight`), `blk.N.*` with
  `blk.N.ffn_(up|down|gate|gate_up)_(ch|)exps.*` as expert bytes (the patterns llama.cpp
  uses, fit.cpp), the rest as other.
- `placement::split`, `fit_layers` (llama.cpp's step 3, one device, whole layers only),
  `infer_layers`. Conversation memory per layer from `ModelSummary` (`head_count_kv` per
  layer; recurrent layers 0).
**Tests:** a tensor table written with `gguf::testing::Writer` (extended with tensors) for a
dense and an expert model, split files summed, tied embeddings, hostile tables (count too
large, offsets backwards, beyond the end, a name of 10 KB, a truncated file) refused without
panicking; the real small model when `TEST_GGUF` is set (the sum of sizes is within 1% of the
file size less the header); `split` for every `Where` (all, none, N, experts all and first K
with N, conversation memory in computer memory) on fixed numbers; `fit_layers` against
hand-worked cases (fits whole; experts first then full layers; dense back to front; target
smaller than one layer gives 0); `infer_layers` round-trips `split`; `auto_ctx_with_margin`
equals `auto_ctx` at `SPARE_BYTES`.
**Size:** about 1,300 lines with tests.

### WP10c. Usage readers (`crates/usage`, new crate)
**Owns:** new `crates/usage/` (`Cargo.toml`, `src/lib.rs`, `src/windows.rs`, `src/macos.rs`,
`src/linux.rs`, `src/nvml.rs`, `tests/fixtures/…`), the members line in the workspace
`Cargo.toml` (shared file, one line), `Cargo.lock`. Touches no file of WP1a, WP1b or WP2 (it
could start before them).
**Build:** section 2.3 and the table of section 5. Pure parsers kept apart from the system
calls so they run in tests on every system: `parse_engine_instance` (`pid_1234_luid_0x00000000_0x0000D1E5_phys_0_eng_0_engtype_3D`),
`busiest_engine_per_adapter`, `parse_adapter_instance`, `parse_proc_stat` and
`processor_percent(prev, now)`, `amdgpu_from_files` (a directory of sysfs files),
`apple_from_stats` (a map of `PerformanceStatistics` keys), `cap_percent`. Dependencies in
the crate's `Cargo.toml`: `serde`; on Windows `windows = { version = "0.62", features =
["Win32_Foundation", "Win32_System_Performance", "Win32_Graphics_Dxgi",
"Win32_System_SystemInformation", "Win32_System_Threading"] }`; on macOS
`objc2-core-foundation` (features already used by `crates/computer`) and `libc`; on Linux
`libc`. Every `unsafe` block has a `SAFETY:` comment. Run `python3 scripts/notices.py` and
`cargo deny check` (no new package expected; say so if one appears).
**Tests:** fixtures: PDH instance lists (two adapters, many processes, an engine busier than
the sum of another's), `/proc/stat` pairs (including counters that wrap), sysfs trees (AMD
card + integrated card; missing `gpu_busy_percent`), a `PerformanceStatistics` map with and
without the keys, NVML absent; `reports_plausible_values_here` on the build machine
(processor and memory values, graphics `noGraphicsCard` in this container); the first sample
is `measuring`. CI builds the crate on Windows and macOS (`ci.yml` runs all three).
**Unverified (owner):** the PDH counter names and no admin prompt on Windows 11 with the RX
9070 XT; the IORegistry keys on the owner's macOS version.
**Size:** about 1,700 lines with tests.

### WP10d. Storage, resolver, picture and usage commands (app)
**Owns:** extends WP1b's `src-tauri/src/load_settings.rs` (Fine control types, validation,
resolve, `same_launch`, `FineFacts`), `src-tauri/src/runtime_manager.rs` (`launch_config`
maps `fine`; `ServerEvent::Memory` → `placement` and the offloaded fields; `placement` cleared
when a load starts), `src-tauri/src/models_cmd.rs` (registers the two commands next to
WP1b's); new `src-tauri/src/offload.rs` (+ tests), new `src-tauri/src/usage_cmd.rs` (+ tests);
appends in `src-tauri/src/dto.rs` (`placement`), `src-tauri/src/state.rs` (the usage thread
handle, the fit-params lock and cache), `src-tauri/src/lib.rs`, `src-tauri/commands.list`,
`src-tauri/capabilities/main.json` (`allow-offload-preview`, `allow-usage-now`),
`src-tauri/Cargo.toml` (`usage` dependency).
**Build:** sections 2.4, 3.2, 4 and 5 (the command side). `set_model_load_settings` and
`offload_preview` refuse values outside the ranges (layers 0 to 1,000 and at most `units`
when known; experts 1 to 1,000 and at most `expert_layers`; threads 1 to 1,024; Leave free
one of 512, 1,024, 2,048, 4,096) with "This setting isn't one of the choices."; resolve clamps
as 3.2. The picture never runs while a measurement for the same key is running (it joins
it).
**Tests:** `fine_control_old_json_both_ways` (0.0.6 JSON without `fine` reads as Automatic;
JSON with `fine` read by the struct without it keeps the other fields; a model without Fine
control serialises without the key), validation per field, resolve per row of 3.1 (including
Off wins, no device, a model without experts, layers above units, threads above cores),
`same_launch_sees_fine_control`, Automatic with a custom placement and with Leave free;
`offload.rs` on fixed facts and rows: processor only, All, custom, Fit ("About"), the loaded
model counted as free, the projector added, every verdict, Apple's single bar and limit,
estimate reasons (no tool, failure, timeout); cache hit and cancel; `usage_cmd` with a fake
sampler: the thread starts on request and ends after 30 s of none, a slow sampler answers
`readFailed` within 2 s; both commands refuse a window other than main;
`placement_from_memory_report` (Vulkan shape, processor shape, custom flag, partial note
fields).
**e2e:** new `e2e/m11_fine_control.py` with the real small model (`LLAMA_SERVER_BIN`,
`TEST_GGUF`): open Settings for this model, open Fine control, see "From llama.cpp's own
calculation for these settings.", choose Processor threads 2 and Fast attention Off, Load
with these settings, the model becomes ready, the usage line shows "Processor" and
"Memory … of … GB" and no graphics part, its menu says "On the processor only."
**Size:** about 1,700 lines with tests.

### WP10e. Fine control section, picture and usage line (frontend)
**Owns:** new `src/components/FineControl.tsx` (+ test), `src/components/OffloadPicture.tsx`
(+ test), `src/components/UsageLine.tsx` (+ test); extends WP2's
`src/components/ModelSettings.tsx` (+ test: mount the section, the Custom option and help of
6.3, the warning line, the Load anyway question, `same()` comparing `fine` with absent =
Automatic), `src/components/RuntimeIndicator.tsx` (+ test: the partial note from
`placement`, not with a custom placement) and `src/lib/words.ts` (+ test: every new phrase,
`retryingWithoutFastAttention` in `loadStageWords` and `adjustmentWords`, memory amounts in
binary GB per I6-10); one wrapping edit around `<ModelPicker />` in `src/components/ChatView.tsx`
(WP3b's file; shared mount, as WP7's FolderChip); appends in `src/lib/types.ts` and
`src/lib/ipc.ts`; the `placement: null` field in the runtime-status mocks of
`Welcome.test.tsx`, `OtherApps.test.tsx`, `Installed.test.tsx`, `ModelSettings.test.tsx`,
`RuntimeIndicator.test.tsx` and `store/modelChange.test.ts`; a `fine-control` scenario and its
`PROBES` entry in `e2e/scripted_agent.py`.
**Build:** sections 3 (controls and help), 6.1, 6.2, 6.3 with the exact wording above.
`offloadPreview(path, draft, open)` is called when the panel opens and 600 ms after each draft
change (`measure` only while Fine control is open); a newer answer replaces an older one.
`usageNow` is polled by a module-level hook like `usePoll` in `RuntimeIndicator.tsx`.
Sliders are native `input type="range"` with a visible number and keyboard steps.
**Tests:** FineControl: each control writes its typed value, Don't use it greys out 1, 2, 3
and 6 with the note, Save memory fixes Fast attention, the expert control only for models with
experts, Reset fine control; ModelSettings: Custom option appears and clearing it by choosing
All of it, the warning line, Load anyway for `wontFitGraphics`, WP2's question for
`wontFitAtAll`; OffloadPicture: every source line, every layer line, Apple bar, legend numbers,
`aria-label`; UsageLine: the four widths, Mac wording, `measuring`, each gap text,
`noGraphicsCard` leaves graphics out, polling stops when hidden and for a remote model, menu
rows with and without a loaded model. **e2e:** scenario `fine-control` in
`e2e/scripted_agent.py` (no real model needed): open the panel from the picker gear, open
Fine control, choose 10 layers, see "Custom: 10 of … layers (set in Fine control)", Reset fine
control, close; the usage line is present under the model name for a local model and absent
for a remote one.
**Size:** about 2,000 lines with tests.

### Order and integration
1. WP10a, WP10b, WP10c in parallel (WP10c may start earlier: it touches nothing of WP1/WP2).
2. WP10d and WP10e in parallel against section 2 (WP10e mocks `offloadPreview`, `usageNow`
   and `placement`).
3. Reviews (security, correctness) and fixes, as for the other packages. Security reviewers:
   the fit-params runner (arguments, environment, caps, timeout, cancel), the tensor-table
   parser (hostile files), the FFI blocks, the two commands (main window only), that no
   setting can be reached from a tool.
4. The lead runs `m10`, `m11`, `settings-pages`, `model-settings`, `fine-control`; updates
   ARCHITECTURE.md 7.3 (arguments), 7.4 (the two new retries, the ready report), 11 (the
   picture), 13.6 (no network use added), 18 (no new dependency; new crate) and the user
   guide.

---------------------------------------------------------------------------------------

## Owner checklist (things only your computers can show)

**Windows, AMD Radeon RX 9070 XT (Vulkan):**
1. With no model loaded, the usage line shows "Graphics card N% · x of 16 GB · Processor N% ·
   Memory x of y GB" and no window asks for administrator rights.
2. While a reply is written, the graphics card figure is within about 10 points of Task
   Manager's GPU figure, and the memory within about 0.3 GB of Task Manager's "Dedicated GPU
   memory"; the processor within about 10 points of Task Manager.
3. The usage line names the RX 9070 XT in its menu, not "AMD Radeon(TM) Graphics".
4. Open Settings for a model → Fine control: the picture appears within 5 seconds and says
   "From llama.cpp's own calculation for these settings."; Task Manager's dedicated memory does
   not rise by more than 0.5 GB while it is worked out.
5. Layers on the graphics card = 20 of N, Load: the menu says "20 of N layers on the graphics
   card"; Task Manager shows less dedicated memory; replies are slower.
6. A mixture-of-experts model larger than 16 GB (for example one with about 30 billion parameters, 3 billion of them used at a time, at Q4): with All of it
   the load steps down; with Expert parts on the processor = All of them it loads with all
   layers on the graphics card and replies at a usable speed.
7. Conversation memory on the graphics card = No: dedicated memory drops by about the
   conversation memory figure of the picture.
8. Processor threads = 4 with Don't use it: Task Manager's processor figure is lower than
   with Automatic.
9. Fast attention On and Off both load on Vulkan (if On fails, the note says it was turned
   off for this load).
10. Leave free on the graphics card = 4 GB with As much as fits: fewer layers than with 1 GB.
11. Too many layers for the card: the warning shows, Load anyway, then the load either works
    or says "The graphics card ran out of memory, so part of the model runs on the
    processor."
12. After a load with As much as fits, the menu's "About N of M layers" matches what you
    expect from the picture before loading (within a layer or two).
13. With the ROCm pack: the picture works, or says "This backend has no calculation tool."

**Mac, 16 GB, Apple silicon:**
14. The usage line shows "Apple graphics N% · Processor N% · Memory x of 16 GB"; no password
    prompt; graphics busy roughly follows Activity Monitor's GPU History while a reply is
    written; memory roughly matches Activity Monitor's "Memory Used".
15. The picture shows one memory bar with "Apple graphics can use up to … GB" and appears
    within 5 seconds.
16. Layers on Apple graphics = half: loads, the menu says so, replies are slower.

---------------------------------------------------------------------------------------

## Open questions for the owner

1. **Where should the "where the model goes" picture be?**
   - **A. At the top of Fine control, with its one-line result on the closed section and the
     warning outside it (recommended).** The panel stays short for everyday use.
   - B. Always visible in Settings for this model.
2. **When the layers you chose probably won't fit:**
   - **A. Warn, offer Load anyway, and if the card runs out of memory use As much as fits for
     that load and say so (recommended).**
   - B. Warn and refuse to load until you change the setting.
3. **How often should the usage line update?**
   - **A. Every 1.5 seconds (recommended).** Close to Task Manager, little work for the
     computer.
   - B. Every second.
   - C. Every 3 seconds.
4. **Chats that use a model from another app or a server (not a local model):**
   - **A. No usage line (recommended).** That model's work is not on this computer's graphics
     card, or the app cannot tell.
   - B. Show it anyway.

---------------------------------------------------------------------------------------

## Sources

In this repository: `docs/dev/0.0.6/README.md` (item 6), `docs/dev/0.0.6/spec.md` (Part 1,
D2, D4, D12, Storage, WP1a, WP1b, WP2, Part 3b Item 6), `docs/dev/0.0.6/item6-usage-offloading.md`,
`docs/dev/0.0.6/report-WP0-fix.md` ("CONTRACT (updated)"), ARCHITECTURE.md 7.1 to 7.4, 11, 13,
SECURITY.md, and the code read for this design: `crates/runtime/src/llama/launch.rs`,
`server/supervisor.rs`, `protocol.rs`, `devices.rs`, `backend.rs`, `crates/runtime/src/spawn.rs`,
`crates/runtime/tests/llama_server.rs` and `fixtures/`, `crates/models/src/fit.rs`,
`gguf.rs`, `summary.rs`, `hardware.rs`, `src-tauri/src/load_settings.rs`,
`runtime_manager.rs`, `dto.rs`, `gpu.rs`, `models_cmd.rs`, `src/components/ModelSettings.tsx`,
`RuntimeIndicator.tsx`, `ChatView.tsx`, `ModelPicker.tsx`, `src/lib/words.ts`,
`scripts/check-spawn.sh`, `llama.lock.json`.

llama.cpp, pinned b12110:
- [tools/server/README.md](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/server/README.md):
  the flag table (`-ngl`, `-cmoe`, `-ncmoe`, `-kvo`, `-t`, `-fa`, `-fit`, `-fitt`, `-fitc`,
  `--log-jsonl`) and "Memory-based unloading" (the `memory` report's shape; projectors not
  measured).
- [tools/fit-params/README.md](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/fit-params/README.md)
  and [fit-params.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/tools/fit-params/fit-params.cpp):
  the two output modes.
- [common/fit.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/common/fit.cpp) and
  [common/fit.h](https://github.com/ggml-org/llama.cpp/blob/b12110/common/fit.h): `no_alloc`,
  only default-valued arguments changed, experts moved first, the JSON breakdown rows.
- [tools/server/server.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/tools/server/server.cpp):
  the router's measuring child (internal; not used), the projector TODO.
- [ggml/src/ggml.c](https://github.com/ggml-org/llama.cpp/blob/b12110/ggml/src/ggml.c):
  `ggml_print_backtrace` and `GGML_NO_BACKTRACE`.
- The pinned binaries' own `--help` output and the runs of section 0.

LM Studio (what the owner compares with), as collected in `item6-usage-offloading.md`:
[load](https://lmstudio.ai/docs/cli/local-models/load),
[per-model settings](https://lmstudio.ai/docs/app/advanced/per-model),
[0.3.27 memory estimate and guardrails](https://lmstudio.ai/blog/lmstudio-v0.3.27),
[0.3.23 expert weights on the processor](https://lmstudio.ai/blog/lmstudio-v0.3.23),
[REST load options](https://lmstudio.ai/docs/developer/rest/load),
[SDK load config](https://lmstudio.ai/docs/typescript/api-reference/llm-load-model-config),
[0.3.14 hardware page](https://lmstudio.ai/blog/lmstudio-v0.3.14).

Windows:
[PdhAddEnglishCounterW](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhaddenglishcounterw),
[PdhGetFormattedCounterArrayW](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhgetformattedcounterarrayw),
[GetSystemTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes),
[GlobalMemoryStatusEx](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-globalmemorystatusex),
[DXGI_ADAPTER_DESC1](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_adapter_desc1),
[GPUs in the Task Manager](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/)
(busiest engine; WDDM 2.0 drivers). The GPU counter set names (`GPU Engine`, `GPU Adapter
Memory`) and their instance-name format are from Windows itself and community sources, not
from Microsoft documentation: unverified until the owner's check.

macOS:
[IOKit](https://developer.apple.com/documentation/iokit),
Apple's own `ioreg` source as an example of reading registry properties from user space
([ioreg.c](https://github.com/apple-oss-distributions/IOKitTools/blob/main/ioreg.tproj/ioreg.c)),
the cost of reading all properties often ([cocoa-dev, 2008](https://lists.apple.com/archives/cocoa-dev/2008/Dec/msg00810.html)),
[host_statistics](https://developer.apple.com/documentation/kernel/1502546-host_statistics).
The `PerformanceStatistics` keys are not documented by Apple: unverified on the owner's macOS.

Linux:
[amdgpu memory accounting (`mem_info_vram_used`, `mem_info_vram_total`)](https://docs.kernel.org/gpu/amdgpu/driver-misc.html),
[amdgpu `gpu_busy_percent`](https://docs.kernel.org/gpu/amdgpu/thermal.html),
[proc_stat(5)](https://man7.org/linux/man-pages/man5/proc_stat.5.html),
[NVML API reference](https://docs.nvidia.com/deploy/nvml-api/index.html) (`nvmlInit_v2`, `nvmlDeviceGetHandleByIndex_v2`, `nvmlDeviceGetName`, `nvmlDeviceGetUtilizationRates`, `nvmlDeviceGetMemoryInfo`, `nvmlShutdown`).
