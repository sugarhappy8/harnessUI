# Item 6: usage view and finer offloading controls (design, work packages WP10a to WP10e)

Written 2026-11-12 for 0.0.6, in the style of `spec.md` Part 3; revision 2 after three
critiques (security, owner intent, code fit; see "Critique log" at the end). Nothing here is
built yet. Every developer follows `spec.md` Part 1 (rules), Part 2 (D1 to D12, storage,
shared files) and the amended D4 of Part 3b, Item 6.

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

**The owner's answers to the open questions (2026-11-12, binding):** 1 = A (the picture is
always in Settings for this model as a rough estimate that follows the choices; exact figures
are worked out when Fine control is opened); 2 = A (warn, offer Load anyway, and on a real
out-of-memory use As much as fits for that load and say so); 3 = A (memory counted as Task
Manager does, download sizes as Hugging Face shows them, with a note under the picture).

**When it runs:** after WP1a, WP1b and WP2 are finished. WP10a, WP10b and WP10c can run in
parallel; WP10d and WP10e then run in parallel against the contract in section 2 (WP10e
mocks the commands until WP10d lands). Roughly 9,800 lines with tests, five developers.

Background reading, in this order: this file; `spec.md` Part 1, D1 to D4, D12, "Storage";
`item6-usage-offloading.md` (the research sketch this replaces); ARCHITECTURE.md 4 (process
table), 7.1 to 7.4 (runtime), 11 (fit check), 13.1 to 13.6 (trust, webview, policy,
network).

---------------------------------------------------------------------------------------

## 0. What was checked on the pinned build (2026-11-12, this machine, processor only)

Run by hand with `.cache/llama-linux` (b12110, `version: 0.9.0-dev (build 12110, commit
cd959bae9)`) and `.cache/models/Qwen3-0.6B-Q8_0.gguf` (610 MiB), environment emptied
(`env -i`), nothing spawned from code. The captures are copied into the fixtures named in
WP10a.

1. **Every flag Fine control needs exists in b12110** (`llama-server --help`, and the
   [b12110 server README](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/server/README.md)
   flag table): `-ngl, --gpu-layers N|auto|all` (default `auto`), `-cmoe, --cpu-moe`,
   `-ncmoe, --n-cpu-moe N` ("keep the MoE weights of the **first** N layers in the CPU"),
   `-kvo/-nkvo`, `-t, --threads N` (default -1), `-fa, --flash-attn on|off|auto` (default
   auto), `-fit on|off` (**default on**), `-fitt, --fit-target MiB` (default 1024), `-fitc`
   (default 4096), `--log-jsonl`. `llama-fit-params --help` lists the same, plus `-fitp`.
2. **`llama-fit-params` allocates nothing and answers fast.** Its measuring code loads the
   model with `no_alloc = true` and `load_mode = NONE` ([common/fit.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/common/fit.cpp),
   lines 70-71 and 176-177). Measured: 0.20 to 0.55 s per run (0.28 s for the run below),
   peak memory 65 MiB for a 610 MiB model (so the weights are never read), exit 0. On a
   graphics card this is **unverified** (owner checklist).
3. **Two output modes** ([fit-params.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/tools/fit-params/fit-params.cpp)):
   - fit mode prints the fitted arguments, e.g. `-c 4096 -ngl -1`, or for a mixture of
     experts `-c 4096 -ngl 48 -ot "blk\.14\.ffn_(up|down|gate)_(ch|)exps=CPU,…"`
     ([fit-params README](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/fit-params/README.md));
   - `-fitp on` measures the given arguments **as they are** and prints `device model
     context compute` in MiB, one line per device plus `Host`.
   With `--log-jsonl`, llama.cpp's **whole log stream goes to stdout** as JSON lines, and
   the measurement is one record among them. The measuring run on the small model printed
   exactly three lines:
   ```
   {"type":"log","time":11632,"level":"info","msg":"llama_fit_params: printing estimated memory in MiB to stdout (device, model, context, compute) ...\n"}
   {"type":"fit_memory_breakdown","data":{"unit":"MiB","rows":[{"kind":"host","name":"Host","self":918,"model":157,"context":448,"compute":312},{"kind":"buffer_type","name":"AMX","self":674,"model":674,"context":0,"compute":0}]}}
   Host 157 448 312
   ```
   The plain `Host 157 448 312` line **leaves out 674 MiB** of repacked weights (the `AMX`
   buffer type; `CPU_REPACK` on ordinary processors): `common_fit_print` skips buffer
   types that belong to no model device. The JSON record has every row. Device rows carry
   `kind:"device"`, `name`, `description`, `total`, `free`, `self`, `model`, `context`,
   `compute` and `unaccounted` (which may be negative); host and buffer-type rows carry
   `kind:"host"` or `kind:"buffer_type"` (snake case), `name`, `self`, `model`, `context`,
   `compute` (fit.cpp lines 1125-1158). So the app reads the JSON record, never the plain
   lines.
4. **`--fit` changes only arguments left at their defaults** (fit.h: "only parameters that
   have the same value as in llama_default_model_params are modified"). With `-ngl N` set,
   or `--cpu-moe`/`--n-cpu-moe` set (they become tensor overrides), the fit stops with
   "n_gpu_layers already set by user" / "tensor_buft_overrides already set by user"
   (fit.cpp 545 and 566) and the server loads the arguments unchanged. As much as fits and
   a chosen placement therefore cannot be combined; Fine control's placement means
   `--fit off`.
5. **How As much as fits places a mixture of experts** (fit.cpp, one device). Step 3 (line
   569) fills layers back to front with only their dense parts on the card and **every
   expert part on the processor**. Step 4 (line 816), when all dense parts fit, turns
   dense-only layers into full layers **from the front**, and tries part of one more layer.
   The result keeps the expert parts of the **last** layers on the processor: the
   README's example prints `-ngl 48 -ot "blk\.14 … blk\.47 …exps=CPU"`. `--n-cpu-moe K`
   instead moves the **first** K layers. The two move the same amount only when the layers
   weigh the same. As much as fits already does what LM Studio's "Force model expert
   weights onto CPU" does, when needed.
6. **`-ngl N` counts from the back.** The output layer is llama.cpp's last unit and goes on
   the card first; then `blk.(B-1)`, `blk.(B-2)`, … (fit.cpp: "the first layer is the
   output layer which must always be full", "filling … back-to-front"). With `N` of
   `B + 1` units on the card, blocks `0 … B−N` run on the processor.
7. **"offloaded N/M layers to GPU" is not printed at the default verbosity.** With `-lv 3`
   (the default, and what the app runs) stderr has no `load_tensors:` lines at all; with
   `-lv 4` it has `load_tensors: offloaded 0/29 layers to GPU`. So
   `ServerEvent::Offloaded` (`crates/runtime/src/llama/protocol.rs`) never fires on b12110,
   and WP2's "Part of the model runs on the processor" note can never show today. The
   existing fixture `crates/runtime/tests/fixtures/server_stderr_cpu.txt` confirms it.
8. **The ready line already says where the model went.** The managed-child `ready` payload
   on stdout carries the memory llama.cpp allocated, per device, in bytes:
   ```
   "memory":{"n_ctx":4096,"devices":[{"kind":"host","name":"Host","model":165568512,"context":469762048,"compute":33572896},{"kind":"buffer_type","name":"AMX","model":707731456,"context":0,"compute":0}]}
   ```
   Device rows there carry `name` (`Vulkan0`, `MTL0`), `total`, `free`, `model`, `context`
   and `compute`, but **no description** (server README, "Memory-based unloading").
9. **Projection versus allocation:** fit-params projected 312 MiB of working space
   (`compute`) where the server then allocated 32 MiB on the processor. The gap is close to
   the output buffer for 512 tokens (151,936 × 512 × 4 bytes = 297 MiB). The app's own
   allowance (`fit.rs` `compute_bytes` = 256 MiB + vocabulary × 512 × 4) includes the same
   buffer and is larger (553 MiB here), so both are on the safe side; the picture says
   "about".
10. **Image readers are not measured** (server README: "Multimodal projectors (mmproj) are
    not included in the measurements yet"). The picture adds the projector's file size.
11. **A crash starts a debugger on Linux.** Given a file that is not a model, fit-params
    aborts (exit 134) and ggml's `ggml_print_backtrace` forks and runs `gdb` (then `lldb`)
    found on `PATH` ([ggml.c](https://github.com/ggml-org/llama.cpp/blob/b12110/ggml/src/ggml.c),
    lines 158 to 225), which attaches to the crashing process and prints argument values
    into stderr. That is a process the app did not start through `crates/runtime`, and
    llama-server's memory holds chat text and its API key. `GGML_NO_BACKTRACE` (any value)
    turns it off; macOS uses the debugger only with `GGML_BACKTRACE_LLDB`, which the
    scrubbed environment never passes. Three llama.cpp programs are started today:
    llama-server (`launch::build_env_on`), the device probe (`devices.rs`, from
    `Backend::base_env()`) and the backend check (`models_cmd::full_environment`).
12. **On a broken file the reason is on stdout, not stderr.** With `--log-jsonl`, the run on
    a truncated copy of the model printed `{"type":"log","level":"error","msg":"llama_model_load:
    error loading model: …"}` lines on stdout and only `terminate called after throwing an
    instance of 'std::runtime_error'` on stderr.
13. **fit-params accepts the measuring arguments the app needs** (run here, exit 0):
    `-m M -c 8192 -np 1 --device none -ngl all --cpu-moe -nkvo -fa on -ctk q8_0 -ctv q8_0
    --fit off -fitp on --log-jsonl --offline`, and `-ngl 10 --n-cpu-moe 5 -nkvo -t 2 -fa
    off`. A device that the build does not have is refused at once (`invalid device:
    Vulkan0`, exit 1, 0.02 s).
14. **Where `llama-fit-params` ships.** `keep_files` in `llama.lock.json` keeps
    `llama-fit-params.exe` and its `-impl.dll` on Windows; macOS builds it as the second
    `externalBin` (ARCHITECTURE.md 7.1); the CUDA pack is built from the same tag (`build:
    b12110`); `runtime::Backend::fit_params` finds it. The **ROCm pack has none**: its zip
    holds `llama-server.exe` and its DLLs only (`docs/research/rocm-amd-runtime.md`, line
    130), and it is built from another commit (`lemonade-b1372`, upstream `0eebb6edd`). With
    ROCm the picture is always the app's own estimate.

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
  plain control then reads "Custom: 30 of 41 layers (set in Fine control)". Choosing **any**
  value in the plain control, "Same as default (…)" included, clears the custom placement
  (layers and expert parts back to Automatic). Don't use it always wins, also when it comes
  from the defaults for all models: the layers, expert parts and conversation memory
  controls are greyed out with a visible note, the plain control shows no Custom option,
  and nothing of the placement reaches the command line.
- **I6-3 A custom placement launches with `--fit off`.** llama.cpp's fit would stop anyway
  (finding 4); saying it explicitly keeps the arguments honest and the log quiet.
- **I6-4 Save memory needs fast attention.** With Save memory on, Fast attention shows "On
  (Save memory needs it)" and cannot be changed; the arguments are Save memory's.
- **I6-5 The step-down learns two steps** (`supervisor.rs`), each once per load, the saved
  values unchanged (as D2c): fast attention On that fails the way Save memory can fail is
  retried with fast attention **Off**; a custom placement that runs out of graphics memory
  is retried with As much as fits, and the note names the owner's choice. Then the existing
  order.
- **I6-6 One answer to "will it fit?" per panel.** Every fit mark, the "too large" line,
  the picture and its warning come from one function (`placement::judge`, section 4.1) on
  the same memory figures (`load_settings::Machine`). The picture has two tiers: an instant
  estimate from the model file (the GGUF tensor table: exact weights per layer and per
  expert part; the conversation memory from the header; the working-space allowance of
  `fit.rs`), and llama.cpp's own measurement (`llama-fit-params -fitp on --log-jsonl`) for
  the exact placement, which replaces the estimate and the marks of the selected choices
  when it arrives. For As much as fits the app works out the layer count itself with
  llama.cpp's rule (finding 5) and labels it "About": fit mode would count the loaded
  model's memory as taken, and at load time llama.cpp measures again anyway.
- **I6-7 "Where the model went" comes from the ready line's memory report** (finding 8),
  not from log lines (finding 7). The layer count is exact for All, a chosen number and the
  processor; for As much as fits it is explained from the bytes on the graphics card in
  fit's own order (finding 5), and when the bytes cannot tell, only amounts are given.
  This also brings back WP2's "part on the processor" note.
- **I6-8 `GGML_NO_BACKTRACE=1` for every llama.cpp program the app starts**: in
  `Backend::base_env()` (device probe, llama-server, fit-params), in `launch::build_env`,
  and in `models_cmd::full_environment` (backend check), so a crash never starts a
  debugger (finding 11).
- **I6-9 Usage is read on this computer, on request only.** A new crate `crates/usage` with
  no new third-party crate (the `windows` 0.62 crate with two more features, `libc`,
  `objc2-core-foundation`, all already in `Cargo.lock`). No background polling: the webview
  asks every 1.5 seconds while the window is visible; the reader lives on its own thread
  and ends 30 seconds after the last request. No driver library is loaded into the app.
- **I6-10 Memory is counted the way Task Manager and Activity Monitor count it**: binary
  gigabytes written "GB". Capacities (a card's or the computer's total) are rounded like
  WP2's device label (`words.ts` `gigabytes`: whole from 10 GB up, so "16 GB", "32 GB");
  amounts in use or needed have one decimal (".0" dropped). One formatter pair in
  `words.ts` serves every item 6 figure and the device label. A caption under the picture
  says so. Download and file sizes keep the app's decimal "GB" (see "Not changed" in 6.2).
- **I6-11 The usage line is always visible under the model picker**, whatever the chat's
  model is (local, from another app, or none chosen) and on the Welcome screen: it is this
  computer's usage, and the owner asked for it always visible. The rows about the model
  itself appear only when a local model is loaded.
- **I6-12 Windows reads PDH, the counters Task Manager's figures come from**, for every
  vendor (no ADLX: proprietary licence; no NVML), always added with
  `PdhAddEnglishCounterW` so non-English Windows works. Graphics card busy is the busiest
  engine, Task Manager's rule ([GPUs in the Task Manager](https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/)).
- **I6-13 No per-process graphics reading.** What the model itself takes comes from the
  ready line's memory report. Process ids in PDH instance names are dropped inside
  `crates/usage`, never returned and never logged.
- **I6-14 One measurement at a time.** A newer request cancels the older run; one timeout
  (`FIT_PARAMS_TIMEOUT`, 30 s, enough for macOS's first-start malware scan); results
  cached (32 entries); failures and time-outs cached per file version and backend build
  until the panel is opened again, so a file that crashes fit-params is run at most once
  per opening; output capped; a failure falls back to the estimate and says why.
- **I6-15 Only pinned programs are measured.** `fit_params::measurable(backend)`: the
  backend has a fit-params file **and** its `build` equals the lock's `llama_cpp.tag`
  (the installed runtime and the CUDA pack), or it is the development folder in a debug
  build. The ROCm pack, other builds and the development folder in release builds get the
  estimate.
- **I6-16 Splitting across several graphics cards stays out** (D4 amendment: `-ts`, `-sm`,
  `-mg` never shown) until someone has two cards to test.
- **I6-17 Modes (D12):** nothing here is a tool action. Fine control, the picture and the
  usage line are the same in Ask first, Allow all and Full access; no tool and no model
  output can read or change them (`set_setting`'s allow-list unchanged; the new commands are
  main-window only); the model is never told the usage figures.

---------------------------------------------------------------------------------------

## 2. Contract (shared by WP10a to WP10e)

Owners in brackets build these first in their package; dependants build against this text.

### 2.0 Numbers and text from other programs (every package)
- **Arithmetic:** every sum, product and difference on numbers from llama.cpp's output, the
  model file, the system or a driver uses checked or saturating operations (no wrapping, no
  truncating `as` casts; `f64` only for display). Debug builds, which the e2e runs use,
  must never panic on such input.
  - fit-params rows: only `total`, `free`, `self`, `model`, `context` and `compute` are
    read; each must be a JSON integer from 0 to 1,073,741,824 MiB (1 PiB). A negative
    value, a fraction or a larger number makes the whole result `NoReport`. MiB become
    bytes with `checked_mul(1_048_576)`. Other fields (`unaccounted`, which can be
    negative) are ignored.
  - Ready-line rows: byte values above 2^50 drop that row.
  - Free memory minus a margin saturates: a free amount below the margin gives a target
    of 0.
  - Tensor tables: data start plus offset plus size is checked; an overflow refuses the
    file (`NotReadable`).
- **Text:** names and descriptions from llama.cpp, the model file or the system go through
  one rule: every control character (Unicode Cc), bidirectional mark or override (U+061C,
  U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069) and zero-width or invisible character
  (U+200B to U+200D, U+2060 to U+2064, U+FEFF) is written as `[U+XXXX]`, then the text is
  cut to a fixed number of characters. This is the rule of `models::hf::uploader_text`;
  `crates/runtime` (`fit_params.rs`, `pub(crate) fn escape_text`) and `crates/usage` keep
  their own copy, since neither depends on `models`. Caps: row names 64, descriptions 80,
  graphics card names 80, log messages 300.

### 2.1 Runtime crate [WP10a] (`crates/runtime/src/llama/launch.rs`, `fit_params.rs`, `protocol.rs`, `server.rs`, `stage.rs`)
```rust
/// Expert parts kept on the processor (mixture-of-experts models).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpertsOnCpu { #[default] None, All, FirstLayers(u32) }   // "none" | "all" | {"firstLayers":n}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlashAttn { #[default] Auto, On, Off }                       // "auto" | "on" | "off"

/// Fine control as launch arguments. Default = no argument at all (0.0.6 without item 6).
/// The resolver fills only what reaches the command line (section 3.2).
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryRowKind { Device, Host, BufferType }   // JSON out: "device"|"host"|"bufferType"
// Parsed from llama.cpp's words explicitly: "device", "host", "buffer_type" (never through
// a derived Deserialize, which would drop "buffer_type"); any other kind skips the row.

/// One row of a memory report, in bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRow {
    pub kind: MemoryRowKind,
    pub name: String,                   // "Vulkan0", "Host", "CPU_REPACK" (2.0, cap 64)
    pub description: String,            // fit-params device rows: "AMD Radeon RX 9070 XT"
                                        // (2.0, cap 80); empty in the ready line
    pub total_bytes: Option<u64>,       // device rows only
    pub free_bytes: Option<u64>,        // device rows only, at the moment of the run
    pub model_bytes: u64,
    pub context_bytes: u64,
    pub compute_bytes: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReport { pub n_ctx: Option<u32>, pub rows: Vec<MemoryRow> }   // at most 16 rows

// protocol.rs: ChildState::Ready gains  memory: Option<MemoryReport>
// server.rs:   ServerEvent::Memory(MemoryReport), sent once per Ready that carries one
//              (ServerEvent keeps its derives: Clone, Debug, PartialEq, Serialize).

// fit_params.rs
pub const FIT_PARAMS_TIMEOUT: Duration = Duration::from_secs(30);
pub enum MeasureLayers { All, Count(u32) }            // the device is MeasureConfig::device
pub struct MeasureConfig {
    pub model: PathBuf, pub ctx: u32, pub device: Option<String>, pub layers: MeasureLayers,
    pub experts_on_cpu: ExpertsOnCpu, pub kv_in_computer_memory: bool,
    pub save_memory: bool, pub flash_attn: FlashAttn, pub hip_visible_devices: Option<String>,
}
pub fn measurable(backend: &Backend) -> bool;          // I6-15
pub fn build_measure_args(cfg: &MeasureConfig) -> Vec<String>;
pub fn parse_measure_output(stdout: &[String]) -> Result<Vec<MemoryRow>, FitParamsError>;
pub async fn measure(backend: &Backend, cfg: &MeasureConfig, timeout: Duration,
                     cancel: CancellationToken) -> Result<Vec<MemoryRow>, FitParamsError>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FitParamsError { NoTool, Start { message: String }, Timeout, Failed { code: Option<i32> },
                          NoReport, Cancelled }

// stage.rs: LoadStage gains  RetryingWithoutFastAttention { backend: BackendKind }
//           JSON {"kind":"retryingWithoutFastAttention","backend":"vulkan"}
//           LoadStage::RetryingWithLessGpu gains two fields that old JSON reads without:
//             #[serde(default, skip_serializing_if = "is_false")] custom: bool,
//             #[serde(default, skip_serializing_if = "Option::is_none")] chosen_layers: Option<u32>,
//           JSON {"kind":"retryingWithLessGpu","backend":"vulkan","custom":true,"chosenLayers":40}
```

### 2.2 Models crate [WP10b] (`crates/models/src/gguf.rs`, `layers.rs`, `placement.rs`, `fit.rs`)
```rust
// gguf.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TensorEntry { pub name: String, pub bytes: u64 }
pub fn read_tensor_table(path: &Path) -> Result<Vec<TensorEntry>, GgufError>;

// layers.rs: what each part of the model weighs, from the tensor tables of all split files
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LayerSize { pub dense_bytes: u64, pub expert_bytes: u64 }
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LayerSizes {
    pub input_bytes: u64,        // token_embd.*  (always in computer memory)
    pub output_bytes: u64,       // output.*, output_norm.* (llama.cpp's last unit); tied
                                 // embeddings: the size of token_embd.weight
    pub layers: Vec<LayerSize>,  // blk.0 … blk.(block_count-1)
    pub other_bytes: u64,        // anything else (rope tables, nextn, unknown names)
}
impl LayerSizes {
    pub fn from_tables(tables: &[Vec<TensorEntry>], block_count: u64) -> Self;
    pub fn total(&self) -> u64;
    pub fn units(&self) -> u32;               // layers.len() + 1, llama.cpp's "N/M layers"
    pub fn has_experts(&self) -> bool;
}

// placement.rs: pure arithmetic, no I/O. Every operation saturates (2.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementInput<'a> {
    pub sizes: &'a LayerSizes,
    pub kv_per_layer: Vec<u64>,        // bytes per layer at the context and cache type chosen
    pub compute_gpu: u64, pub compute_cpu: u64, pub projector: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layers { All, Count(u32) }    // units counted from the back (finding 6)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpertsPick { None, All, FirstLayers(u32), LastLayers(u32) }
                                       // LastLayers: As much as fits' own result (finding 5);
                                       // never a launch value
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Where { pub graphics: bool, pub layers: Layers, pub experts_on_cpu: ExpertsPick,
                   pub kv_on_gpu: bool }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Parts { pub model: u64, pub context: u64, pub compute: u64, pub image_reader: u64 }
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Split { pub gpu: Parts, pub cpu: Parts, pub layers_on_gpu: u32, pub units: u32,
                   pub expert_layers_on_cpu: u32 }   // counted only for layers on the card
pub fn split(input: &PlacementInput, place: &Where) -> Split;
/// llama.cpp's fit for one device (common/fit.cpp steps 3 and 4), whole layers only: all
/// units when everything fits under `target_bytes`; for a model with experts, all expert
/// parts on the processor and dense-only layers back to front (step 3), then, when every
/// dense part fits, full layers from the front so that the expert parts of the last K
/// layers stay on the processor (step 4); for a dense model, whole layers back to front.
pub fn fit_layers(input: &PlacementInput, target_bytes: u64) -> (Layers, ExpertsPick);
/// The placement in fit's own order that explains `gpu_model_bytes` within one layer's
/// size, or `None` (the menu then gives amounts only). Shares its order with fit_layers.
pub fn explain(sizes: &LayerSizes, gpu_model_bytes: u64) -> Option<(Layers, ExpertsPick)>;
/// Expert layers that `FirstLayers(k)` actually takes off the card when `layers` of
/// `units` are on it: max(0, min(k, blocks) − blocks_on_processor).
pub fn effective_expert_layers(units: u32, layers: Layers, k: u32) -> u32;

/// The one verdict (section 4.1), with fit.rs's margins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fits { Fits, Tight, Partial, WontFitGraphics, WontFitAtAll }
pub fn judge(split: &Split, room: fit::Room, gpu_choice_is_fit: bool) -> (fit::Verdict, Fits);
/// Automatic (D2b) for a custom placement or conversation memory in computer memory: the
/// largest context, floor to cap as fit::auto_ctx, whose judge is Green, else Yellow.
pub fn auto_ctx_placed(at: impl Fn(u64) -> Split, room: fit::Room, floor: u64, cap: u64)
    -> Option<fit::AutoCtx>;

// fit.rs (WP1b's file, one new function; no signature changes)
pub fn auto_ctx_with_margin(input: &FitInput, room: Room, spare_bytes: u64) -> Option<AutoCtx>;
// auto_ctx(input, room) == auto_ctx_with_margin(input, room, SPARE_BYTES)
```

### 2.3 Usage crate [WP10c] (`crates/usage`)
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Gap { NoGraphicsCard, SystemGivesNone, NotReadHere, ReadFailed }
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Reading<T> { Value { value: T }, Measuring, NotAvailable { why: Gap } }
pub struct Focus { pub description: Option<String>, pub total_hint_bytes: Option<u64> }
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphicsSample { pub name: String,            // 2.0, cap 80
                            pub busy_percent: Reading<f32>,
                            pub memory_used_bytes: Reading<u64>, pub memory_total_bytes: Option<u64> }
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub graphics: Reading<GraphicsSample>,   // NotAvailable{NoGraphicsCard} when none is known
    pub processor_percent: Reading<f32>,     // Measuring on the first sample
    pub threads: u32,                        // logical processors
    pub unified: bool,                       // Apple silicon: graphics memory is computer memory
}
pub trait Sampler { fn sample(&mut self, focus: &Focus) -> Sample; }
pub fn system_sampler() -> Box<dyn Sampler + Send>;   // platform state; lives on one thread
```
Computer memory is not read here: the app fills it from `models::hardware::system_memory()`
(which already exists on all three systems), so `crates/usage` does not depend on `models`.

### 2.4 App [WP10d] (`src-tauri/src/load_settings.rs`, `offload.rs`, `usage_cmd.rs`, `dto.rs`)
```rust
pub enum LayersChoice  { #[default] Auto, Count(u32) }              // "auto" | {"count":n}
pub enum ExpertsChoice { #[default] Auto, All, FirstLayers(u32) }   // "auto" | "all" | {"firstLayers":n}
pub enum KvPlace       { #[default] Auto, ComputerMemory }          // "auto" | "computerMemory"
pub enum ThreadsChoice { #[default] Auto, Count(u16) }              // "auto" | {"count":n}
pub enum FastAttention { #[default] Auto, On, Off }                 // "auto" | "on" | "off"
pub enum LeaveFree     { #[default] Auto, Mib(u32) }                // "auto" | {"mib":512|2048|4096}
#[serde(rename_all = "camelCase")]
pub struct FineControl {       // every field #[serde(default)]; derives Default; is_default()
    pub layers: LayersChoice, pub experts: ExpertsChoice, pub kv: KvPlace,
    pub threads: ThreadsChoice, pub fast_attention: FastAttention, pub leave_free: LeaveFree,
}
// ModelLoadSettings gains:
//   #[serde(default, skip_serializing_if = "FineControl::is_default")] pub fine: FineControl,
// ResolvedLoad gains:  #[serde(default)] pub fine: runtime::FineLaunch,   (compared by same_launch)
// ModelLoadInfo gains: pub fine_facts: FineFacts, pub picture: OffloadPreview,
//   and its marks (fit, too_large, context_options, gpu_options) come from placement::judge
//   whenever the model's LayerSizes are known (section 4.1).
pub struct FineFacts {
    pub units: Option<u32>,          // block_count + 1 ("of 41 layers"); None without a header
    pub expert_layers: Option<u32>,  // block_count when the model has experts, else None
    pub threads: u32,                // logical processors of this computer
    pub graphics: bool,              // a graphics device is known for this load
    pub apple: bool,                 // Apple graphics (unified memory)
    pub graphics_limit_bytes: Option<u64>, // Apple: the most Apple graphics may use
}

// offload.rs: the picture
pub enum PictureSource { Measured,                                  // tag "kind"
    Estimate { why: EstimateWhy,
               #[serde(skip_serializing_if = "Option::is_none")] backend: Option<BackendKind> } }
pub enum EstimateWhy { Quick, NoTool, Failed, TimedOut, NotReadable } // backend set with NoTool
pub struct MemoryBar { pub name: String, pub total_bytes: u64, pub free_bytes: u64,
                       pub free_after_unload: bool, pub other_bytes: u64, pub need: Parts }
pub struct OffloadPreview {
    pub key: String,                       // the draft it answers (the webview drops stale ones)
    pub source: PictureSource, pub unified: bool,
    pub units: Option<u32>, pub layers_on_graphics: u32, pub layers_about: bool,
    pub expert_layers_on_processor: Option<u32>,   // effective count (placement 2.2)
    pub graphics: Option<MemoryBar>,       // None on the processor only, and on a Mac
    pub computer: MemoryBar,
    pub graphics_limit_bytes: Option<u64>, // Apple: the marker on the single bar
    pub mark: Verdict, pub fits: Fits, pub custom: bool,
}
// dto.rs: RuntimeStatusDto gains  pub placement: Option<Placement>  (None until Ready)
pub struct GraphicsUse { pub name: String /* probe description */, #[serde(rename = "use")] pub parts: Parts }
pub struct Placement {
    pub graphics: Option<GraphicsUse>,     // JSON {"name":…, "use":{…}}
    pub computer: Parts,
    pub units: Option<u32>,
    pub layers_on_graphics: Option<u32>,   // None: only amounts are known
    pub expert_layers_on_processor: Option<u32>,
    pub layers_about: bool, pub custom: bool,
}
// usage_cmd.rs
pub struct UsageNow { #[serde(flatten)] pub sample: usage::Sample,
                      pub memory: usage::Reading<MemUse> }
pub struct MemUse { pub used_bytes: u64, pub total_bytes: u64 }
```
**Commands** (main window only, `crate::commands::only_main_window(window.label())?`,
registered in `generate_handler!` in `src-tauri/src/lib.rs`, in `src-tauri/commands.list`,
`allow-<name>` in `src-tauri/capabilities/main.json`):
- `offload_measure(path: String, overrides: ModelLoadSettings) -> OffloadPreview`: first
  `existing_model(&path)?` (as `model_load_info`), then validates `overrides` exactly as
  `set_model_load_settings` does, saves nothing, runs one measurement (section 4.3). The
  instant estimate needs no command of its own: it is `ModelLoadInfo.picture`, which
  `model_load_info(path, overrides)` (WP1b's draft preview) already returns for the unsaved
  choices.
- `usage_now() -> UsageNow`

### 2.5 Frontend [WP10e] (`src/lib/types.ts`, `src/lib/ipc.ts`)
```ts
export type LayersChoice = "auto" | { count: number };
export type ExpertsChoice = "auto" | "all" | { firstLayers: number };
export type KvPlace = "auto" | "computerMemory";
export type ThreadsChoice = "auto" | { count: number };
export type FastAttention = "auto" | "on" | "off";
export type LeaveFree = "auto" | { mib: 512 | 2048 | 4096 };
export interface FineControl { layers: LayersChoice; experts: ExpertsChoice; kv: KvPlace;
  threads: ThreadsChoice; fastAttention: FastAttention; leaveFree: LeaveFree }
// ModelLoadSettings gains  fine?: FineControl   (absent = every field "auto")
export interface FineLaunch { gpuLayers: number | null;
  expertsOnCpu: "none" | "all" | { firstLayers: number }; kvInComputerMemory: boolean;
  threads: number | null; flashAttn: "auto" | "on" | "off"; fitMarginMib: number | null }
// ResolvedLoad gains  fine: FineLaunch
export interface FineFacts { units: number | null; expertLayers: number | null; threads: number;
  graphics: boolean; apple: boolean; graphicsLimitBytes: number | null }
export interface Parts { model: number; context: number; compute: number; imageReader: number }
export interface MemoryBar { name: string; totalBytes: number; freeBytes: number;
  freeAfterUnload: boolean; otherBytes: number; need: Parts }
export type PictureSource = { kind: "measured" }
  | { kind: "estimate"; why: "quick" | "noTool" | "failed" | "timedOut" | "notReadable";
      backend?: BackendKind };   // backend comes with "noTool"
export type Fits = "fits" | "tight" | "partial" | "wontFitGraphics" | "wontFitAtAll";
export interface OffloadPreview { key: string; source: PictureSource; unified: boolean;
  units: number | null; layersOnGraphics: number; layersAbout: boolean;
  expertLayersOnProcessor: number | null; graphics: MemoryBar | null; computer: MemoryBar;
  graphicsLimitBytes: number | null; mark: Verdict; fits: Fits; custom: boolean }
// ModelLoadInfo gains  fineFacts: FineFacts; picture: OffloadPreview
export interface Placement { graphics: { name: string; use: Parts } | null; computer: Parts;
  units: number | null; layersOnGraphics: number | null; expertLayersOnProcessor: number | null;
  layersAbout: boolean; custom: boolean }
// RuntimeStatus gains  placement: Placement | null;
// LoadStage gains  { kind: "retryingWithoutFastAttention"; backend: BackendKind }, and
// retryingWithLessGpu gains  custom?: boolean; chosenLayers?: number
export type UsageGap = "noGraphicsCard" | "systemGivesNone" | "notReadHere" | "readFailed";
export type Reading<T> = { kind: "value"; value: T } | { kind: "measuring" }
  | { kind: "notAvailable"; why: UsageGap };
export interface UsageNow { graphics: Reading<{ name: string; busyPercent: Reading<number>;
  memoryUsedBytes: Reading<number>; memoryTotalBytes: number | null }>;
  processorPercent: Reading<number>; threads: number;
  memory: Reading<{ usedBytes: number; totalBytes: number }>; unified: boolean }
// ipc.ts: offloadMeasure(path, overrides): Promise<OffloadPreview>; usageNow(): Promise<UsageNow>
```

---------------------------------------------------------------------------------------

## 3. Fine control: the controls, their values and their arguments

Shown only in "Settings for this model", in a closed `<details>` section **Fine control**
after **More options**. The help lines below are exact; where a Mac version is given, a Mac
(`FineFacts.apple`) shows it instead.

| # | Label (Mac) | Choices (typed value) | Shown when |
|---|---|---|---|
| 1 | **Layers on the graphics card** (**Layers on Apple graphics**) | Automatic (follows Use the graphics card) · Choose how many: slider 0 … units, "30 of 41 layers" (`LayersChoice`) | a graphics device is known |
| 2 | **Expert parts on the processor** | Automatic · All of them · Some: slider 1 … expert layers, "the first 20 of 48 layers" (`ExpertsChoice`) | the model has experts |
| 3 | **Conversation memory on the graphics card** (**Conversation memory with Apple graphics**) | Yes, faster (Automatic) · No, keep it in computer memory (Mac: No, keep it with the processor) (`KvPlace`) | a graphics device is known |
| 4 | **Processor threads** | Automatic · Choose how many: slider 1 … threads, "8 of 16 threads" (`ThreadsChoice`) | always |
| 5 | **Fast attention** | Automatic · On · Off (`FastAttention`) | always |
| 6 | **Leave free on the graphics card** (**Memory to leave free**) | Automatic (1 GB) · 0.5 GB · 2 GB · 4 GB (`LeaveFree`) | a graphics device is known |

Help lines:
1. "The model is built from layers. Layers on the graphics card run much faster than on the
   processor, as long as they fit in its memory. The rest run on the processor."
   Mac: "The model is built from layers. Layers on Apple graphics run much faster than on the
   processor, as long as they fit in Apple graphics' share of memory (up to 10.7 GB here).
   The rest run on the processor." (the figure is `graphicsLimitBytes`)
2. "This model is a mixture of experts: it has many expert parts but uses only a few of them
   for each word. Keeping expert parts on the processor frees a lot of graphics memory and is
   often only a little slower. Automatic keeps them on the graphics card with All of it, and
   moves them first when As much as fits needs room."
3. "Where the model keeps the chat it is working on. On the graphics card is faster. Keeping
   it in computer memory leaves more room for layers on the graphics card, but replies slow
   down as the chat grows."
   Mac: "On a Mac the processor and Apple graphics share one memory. Keeping the
   conversation with the processor leaves Apple graphics more of its share (up to 10.7 GB
   here) for layers, but replies slow down as the chat grows."
4. "How many of the processor's threads the model may use for the parts that run on the
   processor. Automatic is usually fastest. Fewer keeps the computer more responsive for
   other work."
5. "A faster way for the model to look back over the chat that also uses less memory.
   Automatic turns it on where the graphics card supports it." Mac: "… where Apple graphics
   supports it."
6. "With As much as fits, this much graphics memory is left free for the screen and other
   programs, such as games or video. More free memory means fewer layers on the graphics
   card." Mac: "With As much as fits, this much of Apple graphics' share of memory is left
   free for other programs. More free memory means fewer layers on Apple graphics."

Notes under a control (visible text, never only a tooltip; the control is greyed out):
- Use the graphics card resolves to Don't use it (saved here or in the defaults for all
  models), under 1, 2, 3 and 6: "Not used while Use the graphics card is Don't use it."
  (Mac: "… Use Apple graphics is Don't use it.")
- Under 6, when the graphics choice resolves to All of it with no custom placement: "Only
  used with As much as fits."; with a custom placement: "Not used while you choose the
  layers or expert parts yourself." Leave free affects nothing else: not the marks, not the
  warnings, and Automatic only with As much as fits (3.2).
- Under 2, when layers is a count N with N < units: "The first 9 layers already run on the
  processor. Only expert parts of later layers free graphics memory." (9 = units − N; with
  N = 0: "Not used while no layers are on the graphics card.")
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
| Off | `none` | anything | `-ngl 0 --fit off` (WP1a's, unchanged) |
| All, no device | `none` | anything | `-ngl all --fit off` (WP1a's, unchanged) |
| Fit, no device | `none` | anything | `--fit on` (WP1a's, unchanged) |
| All | yes | no custom placement | `-ngl all --fit off` |
| Fit | yes | no custom placement | `--fit on`, plus `-fitt M` when Leave free is set |
| All or Fit | yes | layers N | `-ngl N --fit off` |
| All or Fit | yes | experts only | `-ngl all --fit off` + `--cpu-moe` or `--n-cpu-moe K` |
| All or Fit | yes | layers N and experts | `-ngl N --fit off` + `--cpu-moe` or `--n-cpu-moe K` |

Without a device (the first three rows) the placement, the expert flags, `-nkvo` and `-fitt`
add nothing, and WP1a's tests `optional_args_and_cpu_device` and
`args_are_exact_for_every_combination` keep their expectations. Always, independent of the
placement: `-t T` when threads are chosen; `-fa on` / `-fa off` when fast attention is
chosen and Save memory is off; Save memory gives `-fa on -ctk q8_0 -ctv q8_0` whatever Fast
attention says. `ExpertsOnCpu::FirstLayers(0)` adds no flag (defence; the resolver never
produces it). Every value is a number from the typed choice; nothing else reaches the
command line.

### 3.2 What the resolver does with stored values (`load_settings::resolve`, `sane_overrides`)
- **Read back checked.** `sane_overrides` applies `validate`'s ranges to every `fine` field
  one by one, because a program can write the settings row (ARCHITECTURE.md 13.3): layers
  above 1,000, threads 0 or above 1,024, experts 0 or above 1,000, and a Leave free that is
  not 512, 2,048 or 4,096 MiB each become Automatic.
- **Clamped for this computer and this file.** Layers at or above `units` launch as `-ngl
  all`; experts on a model without experts are dropped; `FirstLayers(k)` at or above
  `expert_layers` becomes `All`; threads above this computer's threads become its threads
  (settings copied from another computer).
- **Only what reaches the command line goes into `ResolvedLoad.fine`:** with Off or no
  device, no layers, experts or `-nkvo`; `fit_margin_mib` only with As much as fits and no
  custom placement; `flash_attn` Auto when Save memory is on. So `same_launch` (which
  compares `fine`) never says "Settings changed. Restart the model to use them." for a
  change that launches the same arguments, and does say it for one that does not.
- **Automatic context (D2b)** with a custom placement, or with conversation memory in
  computer memory: `placement::auto_ctx_placed`, the largest size whose split fits (judge
  Green, else Yellow), floor and cap as D2b. With As much as fits and Leave free set:
  `fit::auto_ctx_with_margin` with that margin instead of `SPARE_BYTES`. Otherwise WP1b's
  `auto_ctx`, unchanged.

### 3.3 Step-down (supervisor), each step once per load
1. Save memory off (existing, D2c).
2. **New:** fast attention On (Save memory off) and a failure Save memory could also cause
   (`LoadFailure::save_memory_may_be_the_cause`) → retry with fast attention **Off**,
   `LoadStage::RetryingWithoutFastAttention`. Loading line: "Trying again without Fast
   attention". Note after the load: "Fast attention doesn't work with this graphics card,
   so it was turned off for this load." (Mac: "… with Apple graphics, …").
3. As much as fits on a graphics-card out-of-memory, **now also when the placement is
   custom**: layers and experts cleared, `gpu = Fit` (conversation memory place kept; the
   memory to leave free is llama.cpp's default, because Leave free applies only to As much
   as fits without a custom placement, 3.2; the lead's decision after WP10a's review), `LoadStage::RetryingWithLessGpu { custom: true, chosen_layers }`. Note after
   the load with a chosen count: "The 40 layers you chose didn't fit on the graphics card,
   so As much as fits was used for this load. Choose fewer layers in Fine control." With
   expert parts only: "Your Fine control choices didn't fit on the graphics card, so As
   much as fits was used for this load." Without `custom`, WP2's existing words.
4. The next backend down (existing); on the processor the placement adds nothing (3.1).

---------------------------------------------------------------------------------------

## 4. The before-load picture

**Where (recommended answer to open question 1):** always visible in "Settings for this
model", under the heading **Where the model goes**, after the plain controls and before
More options; it shows the instant estimate and follows every change at once. The exact
figures (llama.cpp's measurement) are worked out while Fine control is open. The closed
Fine control `<summary>` reads "Fine control · 30 of 41 layers on the graphics card" (plus
" · changed" when any Fine control value is set). If the owner picks C instead, the picture
moves to the top of Fine control and the summary adds " · needs about 11.7 of 15.0 GB".

### 4.1 One answer to "will it fit?"
- **One function.** `placement::judge(split, room, gpu_choice_is_fit)` gives both the
  three-way mark of WP2's panel (`Verdict`: Fits / Fits with limits / Too large) and the
  picture's detail (`Fits`), with fit.rs's margins (`SPARE_BYTES` on the main device,
  `RAM_RESERVE_BYTES` in computer memory) and fit.rs's `Room`:
  - Green / `fits`: the graphics part plus the spare fits `room.whole` and the computer part
    fits computer memory, and with As much as fits every unit is on the card.
  - Yellow / `partial`: As much as fits had to leave part on the processor (no warning; the
    layer line explains it).
  - Yellow / `tight`: All of it or a custom placement; the graphics part fits the free
    memory but not with the spare.
  - Yellow / `wontFitGraphics`: All of it or a custom placement; the graphics part is
    larger than the free memory, but the model fits `room.partial` (the step-down to As much
    as fits would load it).
  - Red / `wontFitAtAll`: it does not fit `room.partial`.
  On the processor only, `room.whole` is computer memory less the reserve, so judge is
  fit.rs's `Room::verdict` of the total.
- **Every mark uses it.** `load_settings::build_info` computes `fit`, `too_large`,
  `context_options` and `gpu_options` with `judge` on `placement::split` whenever the
  model's `LayerSizes` are known (every readable real file); only without a tensor table
  does it keep WP1b's fit.rs rule, and then the picture is the `notReadable` estimate made
  from the same fit.rs numbers. WP1b's fixed-number tests build facts without a tensor
  table and keep their expectations. The marks therefore include Fine control: change the
  layers in the draft and the "How much it can remember" marks follow.
- **The picture is part of the same answer.** `ModelLoadInfo.picture` is built in the same
  `build_info` call from the same split, room and judge, so the picture's verdict is the
  mark of the selected context and graphics choices.
- **The measurement replaces the selected choices' marks.** When `offload_measure` answers
  for the draft on screen, its `mark` replaces the marks of the selected context choice and
  the selected graphics choice, and its picture and warning replace the estimate's. The
  other choices keep the estimate's marks (they were not measured).
- **One warning line**, at the top of the controls where WP2's `TOO_LARGE_LINE` is today,
  in this order of precedence: `too_large` (WP2's line, unchanged) > `wontFitAtAll` >
  `wontFitGraphics` > `tight`. The picture repeats no warning.
- **One set of free figures.** Free memory comes from `load_settings::Machine` (the probe's
  free memory plus `Freed`) for the marks, Automatic and the picture alike; the live usage
  reader does not feed the picture.

### 4.2 How the picture is worked out (`src-tauri/src/offload.rs`, arithmetic in `placement.rs`)
1. Resolve the draft for this computer (`load_settings::plan_draft`, with `fine`).
2. Facts, cached per file version in `HEADER_CACHE`: header (block count, expert count,
   conversation memory per layer for f16 and q8_0) and `LayerSizes`. Split models: the
   other files are found only from the first file's own `-00001-of-0000N.gguf` name, in the
   same folder; N must be at most 256 and equal the header's `split.count`; each file is
   parsed with the same limits. A missing or unreadable part, or no readable header, gives
   `estimate { notReadable }`, layers unknown, bars from fit.rs's numbers.
3. Room: `Machine` (graphics free from the probe, the Metal working set on a Mac, computer
   memory from `models::hardware::system_memory()`), with `Freed` counting the loaded
   model as free. `Freed` is extended, not duplicated: once the ready report is known,
   `RuntimeManager::loaded_on_device` uses the report's device bytes instead of the
   estimated footprint, and on the processor `Freed.ram_bytes` adds the report's
   buffer-type model bytes (repacked weights are not file-mapped). A bar whose free figure
   includes a credit says so ("free once the loaded model is unloaded").
4. Placement: processor (Off, no device); custom (exact layers and experts); All (all
   units); Fit (`placement::fit_layers` with target = graphics free − margin, saturating;
   margin = Leave free, Automatic 1,024 MiB, llama.cpp's default) → "About".
5. Instant estimate: `placement::split` with fit.rs's working-space allowance on the
   device holding the output layer and fit.rs's fixed part (`COMPUTE_BASE_BYTES`) on the
   processor when any layer runs there; the projector's file size as "Image reader" on the
   graphics card when it is loaded with the model (on the computer bar otherwise). Source
   `estimate { quick }`.
6. Measurement (`offload_measure`, while Fine control is open): one `llama-fit-params` run
   for exactly that placement (4.3). Device rows go to the graphics bar; Host and every
   buffer-type row go to the computer bar; the projector is added as in step 5. Free
   figures stay `Machine`'s (4.1). Cached by (file, size, modified time, backend build,
   device, context, cache type, fast attention, conversation memory place, layers,
   experts). Not measurable (I6-15) → `noTool` at once. Failure → step 5's estimate with
   `failed` or `timedOut`, cached as in I6-14.
7. Verdict: `judge` (4.1). For As much as fits the picture is labelled About and is never
   `wontFitGraphics` (llama.cpp fits again at load time).
8. Apple silicon (`unified`): one bar for the computer's memory, with the most Apple
   graphics may use (the probe's `MTL0` total, the Metal working set) as a marker; judge
   uses `Room::apple`.

### 4.3 The fit-params run (`runtime::fit_params::measure`)
- Only when `fit_params::measurable(backend)` (I6-15); otherwise `NoTool`. A model path
  that is not valid Unicode returns `Start` (launch refuses such paths too; never a lossy
  conversion).
- `SpawnSpec` through `crates/runtime/src/spawn.rs` on `backend.fit_params` in
  `backend.dir`; environment `backend.base_env()` (which now carries `GGML_NO_BACKTRACE=1`)
  + `HIP_VISIBLE_DEVICES` for ROCm (no `LLAMA_ARG_*`, no keys); stdin null; one run at a
  time app-wide (a newer request cancels the older one); killed on `FIT_PARAMS_TIMEOUT` or
  cancel.
- Arguments (all fixed words, numbers, the model path and the probe's device name):
  ```
  -m M -c C -np 1 --device D (-ngl all | -ngl N | -ngl 0) [--cpu-moe | --n-cpu-moe K] [-nkvo]
  [-fa on | -fa off] [-ctk q8_0 -ctv q8_0] --fit off -fitp on --log-jsonl --offline
  ```
  For As much as fits on a model with experts, step 4's result (expert parts of the last K
  layers on the processor) is measured as `-ngl all --n-cpu-moe K`: the same amount when
  the layers weigh the same, slightly different for models whose first blocks have no
  experts. Hence "About".
- **Reading stdout** (crate-private `LineReader`, 16 KiB per line, at most 512 lines): each
  line is parsed as a JSON object. Exactly one record with a top-level `"type"` of exactly
  `"fit_memory_breakdown"`, `data.unit` exactly `"MiB"` and 1 to 16 rows is required; zero
  or two or more give `NoReport`. A record inside a log message's text is just text. Rows
  follow 2.0. Plain lines are ignored.
- **For the log:** the last 40 records with `"type":"log"` and level `warn` or `error`
  (message escaped and capped per 2.0) and the last 40 stderr lines, written once per run
  as one `tracing` entry. A reading failure is never logged per webview request.

---------------------------------------------------------------------------------------

## 5. Live usage

**Command:** `usage_now()` hands the request to one reader thread
(`src-tauri/src/usage_cmd.rs`) that owns a `usage::Sampler`, takes a sample and answers
within 2 s. The thread is started by the first request and ends 30 s after the last one,
closing every handle. A sample that runs past 2 s marks the reader **stuck**: that request
and every later one answer `readFailed` for every figure at once, without queuing, and at
most one replacement thread starts per app run. Rates (busy percentages) are the
difference to the previous sample; the first sample reports `measuring`, and a gap of more
than 10 s starts again. Computer memory is added by `usage_cmd` from
`models::hardware::system_memory()` (used = total − available). The focus device is the
loaded model's device, else the device a load would use (`load_settings::Machine`), with
its description and total from the probe.

| Figure | Windows | macOS (Apple silicon) | Linux (development only) |
|---|---|---|---|
| Graphics card busy | PDH `\GPU Engine(*)\Utilization Percentage`: sum per engine over processes, busiest engine of the focus adapter (Task Manager's rule) | IORegistry `IOAccelerator` → `PerformanceStatistics` → `Device Utilization %` | AMD: sysfs `…/device/gpu_busy_percent`; others `notReadHere` |
| Graphics memory used / total | PDH `\GPU Adapter Memory(*)\Dedicated Usage` of the focus adapter; total from DXGI `DXGI_ADAPTER_DESC1.DedicatedVideoMemory` | `In use system memory`; total = the probe's Metal working set | AMD: `mem_info_vram_used` / `mem_info_vram_total`; others `notReadHere` |
| Processor busy | PDH `\Processor Information(_Total)\% Processor Utility` capped at 100 (what Task Manager shows), else `GetSystemTimes` | `host_statistics(HOST_CPU_LOAD_INFO)` | `/proc/stat` first `cpu` line |
| Computer memory (app) | `models::hardware::system_memory()` | same | same |

- **No admin rights, no new crate, no driver library in the app:** PDH (`windows` feature
  `Win32_System_Performance`), DXGI enumeration without creating a device
  (`Win32_Graphics_Dxgi`), IOKit through a few `extern "C"` declarations
  (`IOServiceMatching`, `IOServiceGetMatchingServices`, `IOIteratorNext`,
  `IORegistryEntryCreateCFProperty`, `IOObjectRelease`) linked with `#[link(name = "IOKit",
  kind = "framework")]` and `objc2-core-foundation` for the dictionary. Read only the one
  `PerformanceStatistics` property, never all properties (reported to cost much more
  processor time when called often). NVIDIA on Linux is not read (Linux is a development
  platform; loading NVML would put a proprietary library into the process that holds the
  approval state and keychain secrets).
- **FFI rules (every `unsafe` block has a `SAFETY:` comment):**
  - PDH counters are added only with `PdhAddEnglishCounterW`. `PdhGetFormattedCounterArrayW`
    is called once for the size, then with a buffer of that size; on `PDH_MORE_DATA` (the
    instances changed between calls) it retries at most 3 times; items past the returned
    `dwItemCount` are never read.
  - Every CoreFoundation object created or copied is released exactly once, through an
    owning wrapper (`CFRetained`); `IOObjectRelease` on the iterator and on every entry.
  - Process ids in PDH instance names (`pid_1234_luid_0x…`) are dropped while parsing;
    per-process values are summed per engine and never leave `crates/usage`.
  - A reader failure is logged once per reader thread.
- **Matching the focus device:** Windows by DXGI description equal to the probe's
  description (the RX 9070 XT, not the Ryzen's integrated "AMD Radeon(TM) Graphics"), else
  the adapter with the most dedicated memory; adapters with `DXGI_ADAPTER_FLAG_SOFTWARE`
  and names that `src-tauri/src/gpu.rs` `is_virtual` treats as virtual ("Microsoft Basic
  Render Driver" and others; the list is copied, with a comment naming gpu.rs) are skipped.
  PDH instance names write the LUID as `luid_0x<HighPart>_0x<LowPart>` (8 hex digits each),
  matched against `DXGI_ADAPTER_DESC1.AdapterLuid`. Linux AMD by the largest
  `mem_info_vram_total`.
- **Not available per figure:** no graphics device at all → `noGraphicsCard` (the line then
  leaves graphics out); counters or keys missing → `systemGivesNone`; a graphics card this
  system reader does not handle (NVIDIA or Intel on Linux) → `notReadHere`; a call that
  fails, or a stuck reader → `readFailed`. Busy and memory of the graphics card are separate
  readings, so half a reading is shown as half (6.1).
- **What the model itself takes** comes from `RuntimeStatusDto.placement` (the ready line's
  report), not from a per-process reading (I6-13).
- **Where the model went** (after Ready): `Placement` built by `runtime_manager` from
  `ServerEvent::Memory`: device rows → graphics, named by mapping the row's `name`
  (`Vulkan0`, `MTL0`) to the probe's `Device.description` for the running backend (else
  "Graphics card"; Mac "Apple graphics"); Host and buffer types → computer;
  `layers_on_graphics` exact for All (units), a chosen count and the processor (0); for As
  much as fits `placement::explain` gives layers and, for experts, expert layers on the
  processor ("About"), or `None` (amounts only); `custom` from the launch. It also fills
  `offloaded_layers` / `total_layers`. WP2's partial note shows when fewer than all layers
  or any expert parts are on the processor, except with a custom placement (the owner chose
  it).

---------------------------------------------------------------------------------------

## 6. UI text and layout

All amounts follow I6-10 (`words.ts`: `capacityGb` = WP2's `gigabytes` rule, exported;
`amountGb` one decimal). The worked example below is one consistent case: a model of 14.0
GB in memory with 41 units (40 blocks of 0.32 GB, input and output 0.6 GB each), 30 units
chosen for an RX 9070 XT (15.9 GB, shown "16 GB") with 0.9 GB used by other programs, 16,384
tokens of conversation memory (1.6 GB, 0.04 GB per block), and 32 GB of computer memory with
12.0 GB used by other programs.

### 6.1 The usage line (chat header, under the model picker)
In `ChatView.tsx` the header's `<ModelPicker />` is wrapped in a column with the new
`<UsageLine />` under it (`flex min-w-0 flex-col`); the line is 11px, muted, `tabular-nums`,
one line, truncated, aligned with the picker's text (`px-2.5`). Shown always (I6-11),
whatever model is chosen, also while a model loads and on the Welcome screen. Polled every
1,500 ms while the page is visible (`document.visibilityState`); never otherwise.

By the header's width (its `@container`):
- `@5xl` and wider: "Graphics card 63% · 12.6 of 16 GB · Processor 12% · Memory 16.8 of 32 GB"
- `@3xl` to `@5xl`: "Graphics card 63% · 12.6/16 GB · Processor 12% · Memory 16.8/32 GB"
- `@xl` to `@3xl`: "Graphics card 63% · Processor 12% · Memory 53%"
- narrower: three icons (lucide `Gpu`, `Cpu`, `MemoryStick`) each followed by its
  percentage; each icon has `aria-label` and `title` in words ("Graphics card 63% busy").
- Mac: "Apple graphics 40% · Processor 12% · Memory 11.2 of 16 GB" (graphics memory is part
  of Memory and is named in the menu).
- First sample: "…" in place of a percentage. One figure missing: "–" in its place, the
  rest stays ("Graphics card – · 12.6 of 16 GB", "Graphics card 63% · – of 16 GB"; narrow:
  the icon with "–"); the reason is on that figure's row in the menu. No graphics card: the
  graphics part is left out.

The whole line is one button (`aria-label="Computer usage"`) that opens a menu:
```
This computer right now
Graphics card · AMD Radeon RX 9070 XT
  Busy: 63%
  Memory: 12.6 of 16 GB in use by all programs
  The model uses 11.7 GB: 9.9 GB for itself, 1.2 GB conversation memory, 0.6 GB working space
Processor: 12% busy · 16 threads
Memory: 16.8 of 32 GB in use · the model: 4.8 GB
Where the model went: 30 of 41 layers on the graphics card, 11 on the processor.
Read on this computer only while you can see it. Nothing is sent anywhere.
[Model settings…]
```
- "Where the model went" variants: "All 41 layers on the graphics card." · "About 30 of 41
  layers on the graphics card, the rest on the processor." · "On the processor only." ·
  with experts (As much as fits): "All 49 layers on the graphics card. Expert parts of about
  19 layers are on the processor." · chosen expert parts: "… Expert parts of 20 layers are
  on the processor." · amounts only: "About 12.1 GB of the model is on the graphics card,
  the rest on the processor."
- No local model loaded (a model from another app, none chosen, or not loaded): the "The
  model …", "the model: …" and "Where the model went" parts are left out; **Model
  settings…** shows only for a local model.
- Not available, as visible text on the figure's row: `systemGivesNone` "This computer
  doesn't tell apps how busy its graphics card is." (memory: "… how much of its graphics
  memory is in use."; Mac: "This Mac doesn't tell apps how busy Apple graphics is.");
  `notReadHere` "harnessUI can't read this graphics card on this system yet.";
  `readFailed` "Couldn't read this just now."; Mac graphics memory: "Apple graphics: 6.1 GB
  of the computer's memory".

### 6.2 The picture ("Where the model goes")
```
Where the model goes
Graphics card · AMD Radeon RX 9070 XT · 16 GB
[█████ model ██ conversation memory █ working space ▒ other programs ░ free]
Model 9.9 GB · Conversation memory 1.2 GB · Working space 0.6 GB · Other programs 0.9 GB · Free after loading 3.3 GB
Needs about 11.7 GB of the 15.0 GB free now.
Computer memory · 32 GB
[██ model ▒▒▒▒▒▒ other programs ░░░░░░ free]
Model 4.1 GB · Conversation memory 0.4 GB · Working space 0.3 GB · Other programs 12.0 GB · Free after loading 15.1 GB
30 of 41 layers on the graphics card, 11 on the processor.
Worked out for these settings.
Memory is counted the way Task Manager counts it, so it can differ from the download size.
```
(Graphics: 9.9 + 1.2 + 0.6 + 0.9 + 3.3 = 15.9 GB; computer: 4.1 + 0.4 + 0.3 + 12.0 + 15.1 =
31.9 GB; the model 9.9 + 4.1 = 14.0 GB. All 41 layers would need about 15.6 GB, more than
the 15.0 GB free, which is why 30 were chosen.)
- Bars are stacked `div`s; every part has its word and number in the legend (never colour
  alone); the bar has `role="img"` and an `aria-label` with the whole sentence. "Image
  reader 0.9 GB" appears when the model has one. The last caption says "Activity Monitor"
  on a Mac.
- "Needs about 11.7 GB of the 15.0 GB free now." becomes "… of the 15.0 GB free once the
  loaded model is unloaded." when the loaded model's memory was counted.
- Layer lines: "All 41 layers on the graphics card." · "30 of 41 layers on the graphics
  card, 11 on the processor." · "About 30 of 41 layers on the graphics card. The rest run
  on the processor. The exact number is decided when the model loads." · "On the processor
  only." · experts: "Expert parts of 20 layers stay on the processor." (the effective count
  of 2.2) · As much as fits with experts: "All 49 layers on the graphics card. Expert parts
  of about 19 layers stay on the processor. The exact split is decided when the model
  loads."
- Source lines: "Worked out for these settings." (`measured`) · "Rough estimate from the
  model file. Open Fine control to work out exact figures." (`quick`, Fine control closed)
  · while a measurement runs: the estimate stays shown, following the controls, with
  "Working out exact figures…" · "Rough estimate from the model file. Exact figures
  couldn't be worked out for this file." (`failed`) · "Rough estimate from the model file.
  Working them out took too long." (`timedOut`) · "Rough estimate from the model file. The
  AMD graphics card (ROCm) download can't work out exact figures." (`noTool`, ROCm; other
  backends: "Rough estimate from the model file. Exact figures can't be worked out with
  this download.") · "Rough estimate. The model file's details couldn't be read."
  (`notReadable`).
- Apple: one bar "Memory · shared by the processor and Apple graphics · 16 GB" with a marker
  and the caption "Apple graphics can use up to 10.7 GB".
- The warning line (`role="alert"`, one at a time, at the top of the controls, 4.1):
  - `wontFitGraphics`, no custom placement: "This probably won't fit in the graphics card's
    memory: it needs about 15.6 GB and 15.0 GB is free. Choose As much as fits, or turn on
    Save memory."
  - `wontFitGraphics`, custom placement: "This probably won't fit in the graphics card's
    memory: it needs about 15.6 GB and 15.0 GB is free. Choose fewer layers or keep expert
    parts on the processor in Fine control, or turn on Save memory."
  - `wontFitAtAll`: "This probably needs more memory than this computer has free: about 38.0
    GB, with 30.2 GB free. Choose less to remember, or a smaller download."
  - `tight`: "It only just fits. Programs that use the graphics card, such as games or video,
    may stop it from loading."
  - Mac versions say "Apple graphics' share of memory" for "the graphics card's memory".
- **Load anyway:** Apply and restart and Load with these settings check the latest answer
  (the measurement when there is one for the draft, else the estimate). `wontFitGraphics`
  asks, in the same dialog as WP2's questions (no default focus on the button that loads):
  "This probably won't fit in the graphics card's memory. If it doesn't, the model won't
  load, or part of it moves to the processor, which is slower." — **Cancel** / **Load
  anyway**. `wontFitAtAll` and `too_large` ask WP2's existing question ("This model
  probably needs more memory than this computer has. Load it anyway?").

**Not changed (units):** the owner-intent critique proposed switching the whole app to
binary GB (`src/lib/format.ts`) or showing the file size beside the picture. Download and
file sizes stay decimal because that is how Hugging Face, where downloads are chosen, shows
them (`formatBytes`' own comment), and `format.ts` is outside item 6. The picture never
shows a file size; the caption explains the difference in one line, and capacities now
round exactly like WP2's device label, so the header and the usage line say "16 GB" alike.
Open question 3 lets the owner choose otherwise.

### 6.3 The plain control with a custom placement
When a custom placement is stored and Use the graphics card does not resolve to Don't use
it, "Use the graphics card" shows one more, disabled and selected option "Custom: 30 of 41
layers (set in Fine control)" (with experts only: "Custom: expert parts on the processor
(set in Fine control)"), and its help line becomes: "Fine control sets where the model
goes. Choose another option here to go back to it." Choosing any option, "Same as default
(…)" included, writes `fine.layers = "auto"` and `fine.experts = "auto"` with it. When the
graphics choice resolves to Don't use it (here or in the defaults), the plain control shows
its own value and Fine control shows its notes (section 3).

---------------------------------------------------------------------------------------

## Part 3c. Work packages

### WP10a. Fine control arguments, step-down, fit-params runner and the ready report (`crates/runtime`)
**Owns:** extends WP1a's `crates/runtime/src/llama/launch.rs`,
`crates/runtime/src/llama/server/supervisor.rs`, `crates/runtime/src/llama/stage.rs`,
`crates/runtime/src/llama/server.rs` (the Memory event and its test literal),
`crates/runtime/tests/llama_server.rs` (append), and WP1a's optional
`crates/runtime/src/llama/fit_params.rs` (WP1a did not build it; if it exists at the start,
extend it); `crates/runtime/src/llama/protocol.rs` and `crates/runtime/src/llama/backend.rs`
(`base_env` only; no other 0.0.6 owner); new `crates/runtime/tests/fit_params.rs`; **two
lines in WP1b's `src-tauri/src/runtime_manager.rs` `on_event`**: the arm
`ServerEvent::Memory(_) => {}` (so the app keeps compiling; WP10d replaces it) and the
adjustments test changed from its own `matches!` list to `stage.is_adjustment()`; new
fixtures `crates/runtime/tests/fixtures/fit_params_measure_cpu.jsonl` (section 0, item 3),
`fit_params_broken_file.jsonl` (item 12), `fit_params_measure_vulkan.jsonl` (written by
hand from fit.cpp's row shape and marked "shape only until the owner's capture"),
`managed_ready_memory_vulkan.txt` (same); re-export lines in `crates/runtime/src/lib.rs` and
`src/llama/mod.rs`.
**Build:**
- `ExpertsOnCpu`, `FlashAttn`, `FineLaunch`, `LaunchConfig.fine` and the arguments of
  section 3.1, in that order; `Debug` prints `fine`.
- `GGML_NO_BACKTRACE=1` in `Backend::base_env()` and in `build_env` (I6-8).
- Supervisor: the two new steps of section 3.3; `LoadStage::RetryingWithoutFastAttention`
  (in `is_adjustment` and `of_event`); the two new fields of `RetryingWithLessGpu`.
- `fit_params.rs`: `measurable`, `build_measure_args`, `parse_measure_output`, `measure` as
  in section 4.3; `escape_text` (2.0).
- `protocol.rs`: `ChildState::Ready.memory` from `payload.memory` (kinds mapped explicitly,
  unknown kinds skipped, missing numbers as 0, rules of 2.0, at most 16 rows);
  `server.rs` sends `ServerEvent::Memory` once per Ready that carries it.
**Tests:** `args_are_exact_for_every_fine_control` (a table: each row of 3.1; Save memory
with Fast attention Off; Off with every Fine control set adds nothing but `-t` and `-fa`; no
device with All and with Fit keeps WP1a's words; layers 0 with a device; `FirstLayers(0)`
adds nothing; `-fitt` only with Fit), `fine_launch_old_json_reads` (a 0.0.6-without-item-6
`LaunchConfig` JSON reads with defaults; the new JSON reads into a struct without `fine`),
`env_turns_off_debugger_backtraces` (the probe's, llama-server's and fit-params' spawn
environments all carry it), `retrying_with_less_gpu_old_json_reads`, supervisor:
`fast_attention_failure_retries_once_with_off`,
`custom_placement_out_of_memory_retries_with_as_much_as_fits` (stage carries `custom` and
`chosen_layers`), `the_chain_still_ends` (every retry once, with all settings on);
`measure_args_are_exact` (every combination), `parses_the_captured_measure_output` (Host +
AMX rows, MiB to bytes, the plain line ignored), `parses_a_device_row_with_total_and_free`
(negative `unaccounted` ignored), `a_record_without_rows_is_no_report`,
`two_records_are_no_report`, `a_record_inside_a_log_message_is_ignored`,
`more_than_16_rows_is_no_report`, `values_out_of_range_are_no_report` (u64::MAX MiB,
-1, 1.5, 2^30 + 1 MiB), `hostile_names_are_escaped_and_capped`,
`broken_file_log_records_are_kept_for_the_log`, `measure_refuses_a_path_that_is_not_unicode`
(Unix), `only_pinned_builds_are_measurable` (installed, CUDA pack, ROCm pack, dev folder in
debug and release); `ready_line_memory_is_read` (the existing `managed_stdout.txt` and the
Vulkan shape; a byte value above 2^50 drops the row). Contract tests against the pinned
binary on the processor (`LLAMA_SERVER_BIN`, `TEST_GGUF`; fit-params is its sibling), in
`tests/fit_params.rs`, a binary of its own so `getrusage(RUSAGE_CHILDREN)` sees only
fit-params children: `fit_params_measures_without_reading_the_weights` (rows parse; on
Linux the children's peak memory is below half the model's size),
`fit_params_q8_0_halves_the_context`, `fit_params_accepts_the_fine_control_flags` (the exact
words `--device none -ngl all --cpu-moe -nkvo` and `-ngl 10 --n-cpu-moe 5 -t 2 -fa off`
through the crate's spawn: exit 0 and one record),
`fit_params_on_a_broken_file_fails_without_a_debugger` (exit is not 0, `measure` reports
`Failed` within the timeout on all three CI systems, stderr has no `gdb` or `LWP` line). In
`llama_server.rs`: `fine_control_flags_start` (a server with `-t 2 -fa off`, and one with
`-fa on`, on the processor reaches Ready; the expert flags and `-nkvo` add nothing without a
device, so their server start is on the owner checklist), `ready_reports_where_the_model_went`.
**Unverified (owner):** fit-params timing and "allocates nothing" on Vulkan and Metal; the
device row's total and free on the RX 9070 XT; `--cpu-moe`, `--n-cpu-moe` and `-nkvo` on a
real graphics card.
**Size:** about 1,800 lines with tests.

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
  the file or not increasing, names that are not UTF-8, and any overflow of data start +
  offset (2.0). No type table is needed.
- `LayerSizes::from_tables`: group `token_embd.*` (input), `output.*` and `output_norm.*`
  (output; with tied embeddings the output weighs `token_embd.weight`), `blk.N.*` with
  `blk.N.ffn_(up|down|gate|gate_up)_(ch|)exps.*` as expert bytes (the patterns llama.cpp
  uses, fit.cpp), the rest as other.
- `placement.rs`: `split` with the index rule of finding 6 (units counted from the back;
  `FirstLayers(k)` moves blocks `0 … k−1`, `LastLayers(k)` the last k; expert layers on the
  processor counted only for blocks on the card), `effective_expert_layers`, `fit_layers`
  (steps 3 and 4, whole layers), `explain` (the same order), `judge` (section 4.1),
  `auto_ctx_placed`, and the bars' arithmetic (need per bar, other programs, free after
  loading) so that WP10d's `offload.rs` only gathers facts. Conversation memory per layer
  from `ModelSummary` (`head_count_kv` per layer; recurrent layers 0). Saturating
  throughout.
**Tests:** a tensor table written with `gguf::testing::Writer` (extended with tensors) for a
dense and an expert model, split files summed, tied embeddings, hostile tables (count too
large, offsets backwards, beyond the end, an offset that overflows with the data start, a
name of 10 KB, a truncated file) refused without panicking; the real small model when
`TEST_GGUF` is set (the sum of sizes is within 1% of the file size less the header);
`split` for every `Where` (all, none, N, experts all, first K and last K with N, the overlap
cases K ≤ units − N and K > units − N, conversation memory in computer memory) on fixed
numbers; `fit_layers` against hand-worked cases (fits whole; experts: dense-only back to
front when dense does not fit; all layers with the experts of the last K when it does;
dense back to front; target smaller than one layer gives 0; free below the margin gives
target 0); `explain` round-trips `split` for dense and expert tables and returns `None`
between steps; `judge` for each `Fits` value and the processor-only case equals
`Room::verdict`; `auto_ctx_placed` against `fit::auto_ctx` when everything is on one device;
`auto_ctx_with_margin` equals `auto_ctx` at `SPARE_BYTES`; u64::MAX inputs never panic.
**Size:** about 1,700 lines with tests.

### WP10c. Usage readers (`crates/usage`, new crate)
**Owns:** new `crates/usage/` (`Cargo.toml` with `[lints] workspace = true`, `src/lib.rs`,
`src/windows.rs`, `src/macos.rs`, `src/linux.rs`, `tests/fixtures/…`); in the workspace
`Cargo.toml` (shared file) the members line and `usage = { path = "crates/usage" }` under
`[workspace.dependencies]`; `Cargo.lock`; one line in `.gitattributes`
(`crates/usage/tests/fixtures/** -text`); one added check in `scripts/check-spawn.sh`.
Touches no file of WP1a, WP1b or WP2 (it could start before them).
**Build:** section 2.3 and the table and rules of section 5. Pure parsers kept apart from
the system calls so they run in tests on every system: `parse_engine_instance`
(`pid_1234_luid_0x00000000_0x0000D1E5_phys_0_eng_0_engtype_3D`; the process id is dropped),
`busiest_engine_per_adapter`, `parse_adapter_instance`, `luid_matches`,
`parse_proc_stat` and `processor_percent(prev, now)`, `amdgpu_from_files` (a directory of
sysfs files), `apple_from_stats` (a map of `PerformanceStatistics` keys),
`is_software_adapter`, `cap_percent`, `escape_text` (2.0); every parser trims a trailing
CR. Dependencies in the crate's `Cargo.toml`: `serde`; on Windows `windows = { version =
"0.62", features = ["Win32_Foundation", "Win32_System_Performance", "Win32_Graphics_Dxgi",
"Win32_System_SystemInformation", "Win32_System_Threading"] }`; on macOS
`objc2-core-foundation` (features already used by `crates/computer`) and `libc`; on Linux
`libc`. `check-spawn.sh` also fails on `CreateProcess|ShellExecute|WinExec|posix_spawn|fork\(|execv|execl|popen|libc::system|dlopen`
outside `crates/runtime/src/spawn*` (no file has them today). Run `python3
scripts/notices.py` and `cargo deny check` (no new package expected; say so if one appears).
**Tests:** fixtures: PDH instance lists (two adapters, many processes, an engine busier than
the sum of another's, an instance count that changes between calls, LUID high and low
parts), `/proc/stat` pairs (including counters that wrap, CRLF endings), sysfs trees (AMD
card + integrated card; missing `gpu_busy_percent`), a `PerformanceStatistics` map with and
without the keys, a software adapter and a basic render driver skipped, hostile device
names escaped and capped; `reports_plausible_values_here` runs on all three CI runners
(`ci.yml` already runs `cargo test --workspace` on windows-2025 and macos-26): processor
percent within 0 to 100 after two samples and threads above 0; graphics may be any reading,
and `noGraphicsCard` is asserted only on Linux without `/sys/class/drm/card*`; the first
sample is `measuring`.
**Unverified (owner):** the PDH counter names and no admin prompt on Windows 11 with the RX
9070 XT; the IORegistry keys on the owner's macOS version.
**Size:** about 1,600 lines with tests.

### WP10d. Storage, resolver, picture and usage commands (app)
**Owns:** extends WP1b's `src-tauri/src/load_settings.rs` (Fine control types, validation,
`sane_overrides`, resolve, `same_launch`, `FineFacts`, `ModelFacts` with `LayerSizes`,
`build_info`'s marks through `judge`, `ModelLoadInfo.picture`), WP1b's
`src-tauri/src/runtime_manager.rs` (`launch_config` maps `fine`; the `ServerEvent::Memory`
arm WP10a left empty → `placement` and the offloaded fields; `placement` cleared when a load
starts; `loaded_on_device` and `freed_memory` from the report), WP1b's
`src-tauri/src/models_cmd.rs` (`fine` in `diagnostic_load`'s `ResolvedLoad`;
`GGML_NO_BACKTRACE=1` in `full_environment`; the two command functions), WP1b's
`src-tauri/src/load_tracker.rs` (the test helper `settings()` only: the new `fine` field);
new `src-tauri/src/offload.rs` (+ tests), new `src-tauri/src/usage_cmd.rs` (+ tests);
appends in `src-tauri/src/dto.rs` (`placement`), `src-tauri/src/state.rs` (the usage thread
handle, the fit-params lock and caches), `src-tauri/src/lib.rs` (`generate_handler!`
entries), `src-tauri/commands.list`, `src-tauri/capabilities/main.json`
(`allow-offload-measure`, `allow-usage-now`), `src-tauri/Cargo.toml` (`usage = { workspace =
true }`); new `e2e/m11_fine_control.py`.
**Build:** sections 2.4, 3.2, 4 and 5 (the command side). `set_model_load_settings`,
`model_load_info` with a draft and `offload_measure` refuse values outside the ranges
(layers 0 to 1,000 and at most `units` when known; experts 1 to 1,000 and at most
`expert_layers`; threads 1 to 1,024; Leave free one of 512, 2,048, 4,096) with "This setting
isn't one of the choices."; `sane_overrides` turns stored values outside them into
Automatic; resolve clamps as 3.2. A measurement for a key already running is joined, not
started again. `fit_params::measurable` decides `noTool`.
**Tests:** `fine_control_old_json_both_ways` (0.0.6 JSON without `fine` reads as Automatic;
JSON with `fine` read by the struct without it keeps the other fields; a model without Fine
control serialises without the key), validation per field,
`a_stored_fine_value_outside_the_lists_launches_as_automatic`, resolve per row of 3.1
(including Off wins from the defaults, no device, a model without experts, layers above
units, threads above this computer's), `same_launch_sees_fine_control`,
`a_change_that_launches_the_same_is_not_settings_changed` (Leave free with All; layers with
Off; Fast attention with Save memory), Automatic with a custom placement and with Leave
free under As much as fits; `marks_follow_fine_control` (a draft with fewer layers changes
the context marks, and the picture's mark equals the selected options' marks),
`facts_without_a_tensor_table_keep_wp1b_marks`; `offload.rs` on fixed facts and rows:
processor only, All, custom, Fit ("About"), experts with Fit, the loaded model counted as
free (from the report when known), the projector added, every `Fits`, Apple's single bar
and limit, estimate reasons (not measurable, failure, timeout), a failed file is run once
per opening, cache hit, join and cancel, split parts found only from the first file's name
(index 1, at most 256, equal to `split.count`, one missing → `notReadable`), free below the
margin; `usage_cmd` with a fake sampler: the thread starts on request and ends after 30 s
of none, a sampler slower than 2 s marks the reader stuck so the next request answers
`readFailed` at once and a replacement starts once per run, computer memory filled from
`system_memory`; both commands refuse a window other than main; `offload_measure` refuses a
path that is not a model; `placement_from_memory_report` (Vulkan shape mapped to the probe's
description, processor shape, custom flag, experts explained, amounts only, partial note
fields); `diagnostics_environment_turns_off_debugger_backtraces`.
**e2e:** new `e2e/m11_fine_control.py` with the real small model (`LLAMA_SERVER_BIN`,
`TEST_GGUF`): open Settings for this model, see "Where the model goes", open Fine control,
see "Worked out for these settings.", choose Processor threads 2 and Fast attention Off,
Load with these settings, the model becomes ready; open the usage menu (its text does not
depend on the window width): a "Processor:" row with "threads", "Memory: … of … GB in use",
no graphics row, "On the processor only."
**Size:** about 2,500 lines with tests.

### WP10e. Fine control section, picture and usage line (frontend)
**Owns:** new `src/components/FineControl.tsx` (+ test), `src/components/OffloadPicture.tsx`
(+ test), `src/components/UsageLine.tsx` (+ test), new `src/lib/usePoll.ts` (moved from
`RuntimeIndicator.tsx`, which imports it); extends WP2's `src/components/ModelSettings.tsx`
(+ test: the picture mounted after the plain controls, the section, the Custom option and
help of 6.3, the one warning line and its precedence, marks replaced by a measurement for
the selected choices, the Load anyway question, `same()` comparing `fine` with absent =
Automatic), `src/components/RuntimeIndicator.tsx` (+ test: the partial note from
`placement`, including expert parts, not with a custom placement) and `src/lib/words.ts`
(+ test: every new phrase, Mac versions, `retryingWithoutFastAttention` in `loadStageWords`
and `adjustmentWords`, the custom variant of `retryingWithLessGpu`, `capacityGb` and
`amountGb` per I6-10, `deviceLabel` using `capacityGb`); one wrapping edit around
`<ModelPicker />` in `src/components/ChatView.tsx` (WP3b's file; shared mount, as WP7's
FolderChip); appends in `src/lib/types.ts` and `src/lib/ipc.ts`; the new fields
(`placement: null`, `fine`, `fineFacts`, `picture`) in the mocks of `Welcome.test.tsx`,
`OtherApps.test.tsx`, `Installed.test.tsx`, `ModelSettings.test.tsx`,
`RuntimeIndicator.test.tsx` and `store/modelChange.test.ts`; a `fine-control` scenario and
its `PROBES` entry in `e2e/scripted_agent.py`.
**Build:** sections 3 (controls, help, notes), 4 (where), 6.1, 6.2, 6.3 with the exact
wording above. The picture is `ModelLoadInfo.picture` from WP2's existing draft preview
(`previewModelLoadSettings`, 150 ms after each change), so it follows the sliders; while
Fine control is open, `offloadMeasure(path, draft)` is called 600 ms after the last change
and its answer is used only while its `key` matches the draft on screen. `usageNow` is
polled with `usePoll`. Sliders are native `input type="range"` with a visible number and
keyboard steps.
**Tests:** FineControl: each control writes its typed value; with mocked `fineFacts.graphics
= true` the layer slider, Custom option and greying with Don't use it (also when it comes
from the defaults) with the note; Save memory fixes Fast attention; the expert control only
for models with experts, and its note when layers are a count; the Leave free note with All
of it and with a custom placement; Mac labels and help; Reset fine control; ModelSettings:
Custom option appears and choosing All of it or "Same as default (…)" clears it, no Custom
option when the defaults say Don't use it, one warning line, Load anyway for
`wontFitGraphics`, WP2's question for `wontFitAtAll`; OffloadPicture: every source line,
every layer line, Apple bar, legend numbers that add up, `aria-label`; UsageLine: each width
variant's text and its `@container` classes rendered (jsdom has no layout; real widths are
in e2e), Mac wording, `measuring`, half readings, each gap text, `noGraphicsCard` leaves
graphics out, polling stops when hidden, shown for a remote model and with none chosen, menu
rows with and without a loaded model.
**e2e:** scenario `fine-control` in `e2e/scripted_agent.py` (the real Linux backend is
processor only, so no layers control appears there): open the panel from the picker gear,
see "Where the model goes", open Fine control, choose Processor threads 2 and Fast attention
Off, see " · changed" on the summary, Reset fine control, close; the usage line is present
under the model name for a local model and for a remote one, and the remote one's menu has
no "Where the model went" row.
**Size:** about 2,200 lines with tests.

### Order and integration
1. WP10a, WP10b, WP10c in parallel (WP10c may start earlier: it touches nothing of WP1/WP2).
   WP10a's two lines in `runtime_manager.rs` keep `-p app` compiling for the packages still
   running e2e checks.
2. WP10d and WP10e in parallel against section 2 (WP10e mocks `offloadMeasure`, `usageNow`,
   `picture`, `fineFacts` and `placement`).
3. Reviews (security, correctness) and fixes, as for the other packages. Security reviewers:
   the fit-params runner (arguments, environment, `measurable`, parsing, caps, timeout,
   cancel), the tensor-table parser (hostile files, split parts), the FFI blocks and the
   extended `check-spawn.sh`, the two commands (main window only), `sane_overrides`, that no
   setting can be reached from a tool.
4. The lead runs `m10`, `m11`, `settings-pages`, `model-settings`, `fine-control`; updates
   ARCHITECTURE.md 4 (the process table: fit-params is started by the settings panel while
   Fine control is open, one at a time, cancellable, `FIT_PARAMS_TIMEOUT`), 7.3
   (arguments), 7.4 (the two new retries, the ready report), 11 (the picture and the one
   verdict), 13.1 (llama.cpp now parses a model file when its Fine control is opened, not
   only when it loads), 13.6 (no network use added; whether crashes of llama.cpp's
   programs can reach Windows Error Reporting, whose reports could hold llama-server's
   memory: `init_parent_error_mode` sets `SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX`
   but not `SEM_NOGPFAULTERRORBOX`; deciding that is a `spawn.rs` change outside item 6),
   18 (no new third-party dependency; new crate) and the user guide.

---------------------------------------------------------------------------------------

## Owner checklist (things only your computers can show)

**Windows, AMD Radeon RX 9070 XT (Vulkan):**
1. With no model loaded, the usage line shows "Graphics card N% · x of 16 GB · Processor N% ·
   Memory x of y GB" and no window asks for administrator rights.
2. While a reply is written, the graphics card figure is within about 10 points of Task
   Manager's GPU figure, and the memory within about 0.3 GB of Task Manager's "Dedicated GPU
   memory"; the processor within about 10 points of Task Manager.
3. The usage line names the RX 9070 XT in its menu, not "AMD Radeon(TM) Graphics".
4. With a chat open and nothing else running, harnessUI uses under 2% of the processor in
   Task Manager's Processes tab.
5. Make the window narrow: the usage line shortens to icons with numbers and nothing
   overlaps.
6. Open Settings for a model: "Where the model goes" appears at once; open Fine control:
   within 5 seconds it says "Worked out for these settings."
7. Compare the picture's "Working space" on the graphics card with the menu's figure after
   loading the same model with the same settings; note both numbers.
8. Layers on the graphics card = 20 of N, Load: the menu says "20 of N layers on the
   graphics card"; Task Manager shows less dedicated memory; replies are slower.
9. A mixture-of-experts model larger than 16 GB: in Browse, search for a model whose name
   has "A3B" (about 30 billion parameters, 3 billion used at a time) and pick its
   "Q4_K_M" file (about 18 GB). This needs at least 32 GB of computer memory. With All of
   it the picture warns and the load steps down; with Expert parts on the processor = All
   of them it loads with all layers on the graphics card and replies at a usable speed.
10. Conversation memory on the graphics card = No: dedicated memory drops by about the
    conversation memory figure of the picture.
11. Processor threads = 4 with Don't use it: Task Manager's processor figure is lower than
    with Automatic.
12. Fast attention On and Off both load on Vulkan (if On fails, the note says it was turned
    off for this load).
13. Leave free on the graphics card = 4 GB with As much as fits: fewer layers than with
    Automatic.
14. Too many layers for the card: the warning shows, Load anyway, then the load either works
    or says "The N layers you chose didn't fit on the graphics card, so As much as fits was
    used for this load."
15. After a load with As much as fits, the menu's "About N of M layers" (or, for a
    mixture of experts, "Expert parts of about K layers") matches the picture before loading
    within a layer or two.
16. With the ROCm download: the picture says "Rough estimate from the model file. The AMD
    graphics card (ROCm) download can't work out exact figures."

**Mac, 16 GB, Apple silicon:**
17. The usage line shows "Apple graphics N% · Processor N% · Memory x of 16 GB"; no password
    prompt; graphics busy roughly follows Activity Monitor's GPU History while a reply is
    written; memory roughly matches Activity Monitor's "Memory Used"; harnessUI uses under
    2% of the processor in Activity Monitor with a chat open.
18. The picture shows one memory bar with "Apple graphics can use up to … GB"; Fine control's
    figures appear within 5 seconds.
19. Layers on Apple graphics = half: loads, the menu says so, replies are slower. The help
    lines and notes say "Apple graphics", never "graphics card".

---------------------------------------------------------------------------------------

## Open questions for the owner

1. **Where should the "where the model goes" picture be?**
   - **A. Always in Settings for this model, as a rough estimate that follows your choices;
     the exact figures are worked out when you open Fine control (recommended).** You see
     what offloading would look like without opening the advanced controls.
   - B. Always in Settings for this model, with the exact figures worked out as soon as the
     settings open (it takes under a second and loads nothing, but runs a llama.cpp
     program each time).
   - C. Only inside Fine control, with a one-line summary on the closed section.
2. **When the layers you chose probably won't fit:**
   - **A. Warn, offer Load anyway, and if the card runs out of memory use As much as fits for
     that load and say so (recommended).**
   - B. Warn and refuse to load until you change the setting.
3. **Memory sizes:**
   - **A. Memory as Task Manager counts it (a 16 GB card reads 16 GB), download sizes as
     Hugging Face shows them, with a note under the picture (recommended).**
   - B. Every size the way Windows Explorer counts it, download sizes included (a "9.0 GB"
     download would read "8.4 GB").

(Decided without asking, from your own words: the usage line updates every 1.5 seconds, like
the other status lines, and is shown for every chat, whatever model it uses.)

---------------------------------------------------------------------------------------

## Sources

In this repository: `docs/dev/0.0.6/README.md` (item 6), `docs/dev/0.0.6/spec.md` (Part 1,
D2, D4, D12, Storage, WP1a, WP1b, WP2, Part 3b Item 6), `docs/dev/0.0.6/item6-usage-offloading.md`,
`docs/dev/0.0.6/report-WP0-fix.md` ("CONTRACT (updated)"), `docs/research/rocm-amd-runtime.md`,
ARCHITECTURE.md 4, 7.1 to 7.4, 11, 13, SECURITY.md, and the code read for this design:
`crates/runtime/src/llama/launch.rs`, `server.rs`, `server/supervisor.rs`, `stage.rs`,
`protocol.rs`, `devices.rs`, `backend.rs`, `lock.rs`, `lines.rs`, `crates/runtime/src/spawn.rs`,
`crates/runtime/tests/llama_server.rs` and `fixtures/`, `crates/models/src/fit.rs`,
`gguf.rs`, `summary.rs`, `hardware.rs`, `hf.rs` (`uploader_text`),
`src-tauri/src/load_settings.rs` (`build_info`, `mark`, `sane_overrides`, `Freed`,
`Machine`, `plan_draft`, `split_parts`), `runtime_manager.rs` (`on_event`, `freed_memory`),
`load_tracker.rs`, `models_cmd.rs` (`model_load_info`, `full_environment`,
`diagnostic_load`), `dto.rs`, `gpu.rs`, `src/components/ModelSettings.tsx`,
`RuntimeIndicator.tsx`, `ChatView.tsx`, `ModelPicker.tsx`, `src/lib/words.ts`,
`src/lib/format.ts`, `scripts/check-spawn.sh`, `.gitattributes`, `Cargo.toml`,
`.github/workflows/ci.yml`, `llama.lock.json`.

llama.cpp, pinned b12110:
- [tools/server/README.md](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/server/README.md):
  the flag table (`-ngl`, `-cmoe`, `-ncmoe`, `-kvo`, `-t`, `-fa`, `-fit`, `-fitt`, `-fitc`,
  `--log-jsonl`) and "Memory-based unloading" (the `memory` report's shape; projectors not
  measured).
- [tools/fit-params/README.md](https://raw.githubusercontent.com/ggml-org/llama.cpp/b12110/tools/fit-params/README.md)
  and [fit-params.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/tools/fit-params/fit-params.cpp):
  the two output modes.
- [common/fit.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/common/fit.cpp) and
  [common/fit.h](https://github.com/ggml-org/llama.cpp/blob/b12110/common/fit.h): `no_alloc`
  (70-71, 176-177), only default-valued arguments changed (545, 566), step 3 (569) and step
  4 (816), the JSON breakdown rows (1125-1158).
- [tools/server/server.cpp](https://github.com/ggml-org/llama.cpp/blob/b12110/tools/server/server.cpp):
  the projector TODO.
- [ggml/src/ggml.c](https://github.com/ggml-org/llama.cpp/blob/b12110/ggml/src/ggml.c):
  `ggml_print_backtrace` and `GGML_NO_BACKTRACE` (158-225).
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
[proc_stat(5)](https://man7.org/linux/man-pages/man5/proc_stat.5.html).

---------------------------------------------------------------------------------------

## Critique log

Each problem raised, by critique, and what this revision did. "Not changed" notes give the
reason and evidence; there are two, plus one partly changed.

**Security**
1. *Debugger switch-off missed the probe and the backend check* → `GGML_NO_BACKTRACE=1` in
   `Backend::base_env()`, `build_env` and `models_cmd::full_environment` (I6-8, finding 11);
   tests cover the probe, server, fit-params (WP10a) and diagnostics (WP10d).
2. *Stored Fine control values not checked on read* → `sane_overrides` applies every range;
   out of range becomes Automatic; test `a_stored_fine_value_outside_the_lists_launches_as_automatic`
   (3.2, WP10d).
3. *No overflow rule* → section 2.0: checked or saturating arithmetic, fit-params value
   limits, free below margin gives target 0, tensor offset overflow refused; tests in
   WP10a, WP10b, WP10d.
4. *"Last record wins" was weak* → exactly one `fit_memory_breakdown` record by top-level
   `type`, at most 16 rows, else `NoReport`; tests for a forged record in a log message and
   for two records (4.3).
5. *Unpinned programs would be measured* → `fit_params::measurable` (I6-15): build equal to
   the lock tag, or the dev folder only in debug builds; ROCm gets the estimate. Noted for
   the lead that llama-server from the dev folder has the same question today.
6. *NVML in the app process; a stuck reader* → NVML dropped (NVIDIA on Linux is
   `notReadHere`); a sample past 2 s marks the reader stuck, later requests answer at once,
   one replacement per run (section 5).
7. *FFI rules unwritten; Windows/macOS code first runs on the owner's computers* → PDH
   two-call rule with at most 3 retries and `dwItemCount`, owning CF wrappers,
   `IOObjectRelease`; `reports_plausible_values_here` on all three CI runners (5, WP10c).
8. *Device names not escaped; PIDs; log flooding* → `GraphicsSample.name` escaped and capped
   (2.0); PIDs dropped inside the crate (I6-13); failures logged once per thread.
9. *Path checks* → `offload_measure` calls `existing_model` first; non-Unicode path →
   `Start`; split parts only from the first file's own name, at most 256, equal to
   `split.count` (2.4, 4.2, 4.3).
10. *Crashing files run again and again* → failures and time-outs cached per file version
    and build until the panel is opened again (I6-14). **Partly changed:** whether
    llama.cpp's crashes reach Windows Error Reporting goes into ARCHITECTURE.md 13.6 at
    integration; adding `SEM_NOGPFAULTERRORBOX` would change `spawn.rs`'s error mode for
    every child and the app itself (`init_parent_error_mode` sets it on the parent), which
    is outside item 6, so the lead decides.
11. *New crate could bring in low-level process calls* → `[lints] workspace = true`;
    `check-spawn.sh` extended with the low-level pattern list (WP10c; no file matches
    today).
12. *ARCHITECTURE.md 4 and 13.1 missing; two timeouts* → both added to integration step 4;
    one `FIT_PARAMS_TIMEOUT` of 30 s.

**Owner intent and plain words**
1. *Two answers to "will it fit?"* → `placement::judge` is the only verdict; `build_info`'s
   marks, `too_large`, the picture and the warning use it on the same `Machine` figures;
   the measurement replaces the selected choices' marks; one warning line with a precedence
   (4.1); test `marks_follow_fine_control`. (WP2's TODO was already closed by WP1b's draft
   preview, which this builds on.)
2. *Leave free did nothing with All of it* → note "Only used with As much as fits."; Leave
   free now affects only As much as fits (launch, picture, Automatic); help 6 says so.
3. *Warning pointed into the closed section* → two texts; without a custom placement it
   names As much as fits and Save memory (6.2).
4. *Custom option contradictions* → shown only when the graphics choice does not resolve
   to Don't use it; any plain choice, "Same as default (…)" included, clears the placement;
   "saved values are kept" removed for layers and experts; tests for defaults that say
   Don't use it (I6-2, 6.3).
5. *After-load line contradicted the picture for experts* → `placement::explain` in fit's
   own order returns layers and expert layers; amounts only when the bytes cannot tell;
   round-trip test (I6-7, 5).
6. *The picture was hidden in the closed section* → recommendation changed: the picture is
   always shown (estimate), exact figures with Fine control open (open question 1 A).
7. *Program names in source lines* → plain lines without "llama.cpp" or "backend" (6.2).
8. *Two "free" figures; "Model" meaning two things; example did not add up* → "Free after
   loading", "Needs about X of the Y free now", "The model uses 11.7 GB: 9.9 GB for itself,
   …"; a worked example whose parts add up (6.2).
9. *"Cores" for logical processors* → "threads" everywhere (`Sample.threads`,
   `FineFacts.threads`, "8 of 16 threads", menu, help 4).
10. *Two sizes for one file; rounding differed from the device label* → capacities round
    like `deviceLabel`, one formatter pair, caption under the picture. **Not changed:**
    file sizes stay decimal (Hugging Face's unit, `format.ts` outside item 6); open
    question 3 asks the owner.
11. *Mac wording only in labels* → Mac versions of help 1, 3, 5, 6, the notes, the gap text,
    the warnings and the step-down note; tests and checklist item 19.
12. *No words for half a reading; non-English Windows* → "–" per missing figure with the
    reason on its row (6.1); `PdhAddEnglishCounterW` is a rule (I6-12, 5).
13. *Fast attention retry used Automatic* → retries with Off; stage text "Trying again
    without Fast attention" (3.3).
14. *Duplicate 1 GB choice; Q3 and Q4* → plain 1 GB removed; 1.5 s decided; the usage line
    is shown for every chat and on Welcome (I6-11); Q3 and Q4 removed.
15. *Custom placement set aside silently* → `RetryingWithLessGpu { custom, chosen_layers }`
    and its own note (3.3).
16. *Picture did not follow the slider* → the instant estimate is `ModelLoadInfo.picture`
    from the existing 150 ms draft preview; the measurement follows after 600 ms (WP10e).
17. *Checklist gaps* → items 4 and 17 (processor use), 5 (narrow window), 9 (which download,
    32 GB of computer memory), 6 now timing only, 7 (working space).

**Code fit and feasibility**
1. *New ServerEvent variant breaks the app between WP10a and WP10d* → WP10a owns two lines
   in `runtime_manager.rs` (`ServerEvent::Memory(_) => {}` and `is_adjustment()`), WP10d
   replaces the first; the new stage is in `is_adjustment` and `of_event`.
2. *`ResolvedLoad` literals in files WP10d did not own* → `load_tracker.rs` (test helper)
   added to WP10d's Owns; `diagnostic_load` named as the reason for `models_cmd.rs`;
   registration is in `lib.rs`.
3. *`getrusage` would see llama-server children* → the fit-params contract tests move to
   their own binary `tests/fit_params.rs`.
4. *Expert flags and `-nkvo` cannot start without a device* → split:
   `fine_control_flags_start` covers `-t` and `-fa`; the other flags go through fit-params
   with `--device none`; their server start is on the owner checklist.
5. *e2e scenario needed a graphics device* → layer slider, Custom option and greying in
   vitest with mocked `fineFacts.graphics`; e2e exercises threads, fast attention, Reset,
   the picture and the usage line; m11 reads the menu, not the width-dependent line.
6. *`fit_layers` mirrored only step 3* → steps 3 and 4 with whole layers; `LastLayers` for
   fit's result; the Fit measurement approximates with `--n-cpu-moe K`, labelled About
   (finding 5, 4.3). The alternative (fit mode for As much as fits) is not taken: fit mode
   counts the loaded model's memory as taken (I6-6).
7. *Layers and expert parts overlap undefined* → index rule (finding 6),
   `effective_expert_layers`, the note under control 2, overlap tests.
8. *Free figures and precedence undefined* → one source, `Machine` with `Freed`, for marks
   and picture; the sampler no longer feeds the picture; `Freed` extended from the ready
   report; precedence of the warning line (4.1, 4.2).
9. *No-device arguments changed WP1a's tests* → today's no-device arguments kept; WP1a's
   tests unchanged (3.1).
10. *`same_launch` would see changes that launch the same* → only command-line values in
    `ResolvedLoad.fine`; test `a_change_that_launches_the_same_is_not_settings_changed`;
    3.2 split into separate points.
11. *Contract gaps* → TypeScript `fineFacts`, `picture`, `fine` and the mock files in
    WP10e's Owns; derives written; explicit mapping of `buffer_type`; escaping defined
    (2.0, 2.1, 2.5).
12. *Usage crate wiring* → computer memory filled by `usage_cmd` from
    `models::hardware::system_memory()`; `[workspace.dependencies]` line; `.gitattributes`
    line; parsers trim CR (WP10c).
13. *Error reason is on stdout; line cap* → the last 40 warn/error log records kept for the
    log; `LineReader`'s 16 KiB cap (4.3, finding 12).
14. *Probe also forks gdb* → covered by `base_env` (security 1).
15. *ROCm has no fit-params* → stated (finding 14); checklist item 16 expects the estimate
    wording.
16. *Software adapters; LUID order* → `DXGI_ADAPTER_FLAG_SOFTWARE` and gpu.rs's virtual
    names skipped; `noGraphicsCard` asserted only on Linux without DRM cards;
    `luid_0x<High>_0x<Low>` with a parser test.
17. *Ready-line device rows have no description* → `runtime_manager` maps the row's name to
    the probe's description, fallback "Graphics card" (5).
18. *fit-params' working space is about 10 times the allocation* → owner checklist item 7.
    **Not changed:** the verdict keeps the measured working space. fit.rs's allowance, the
    proposed replacement, includes the same output buffer and is larger (256 MiB +
    151,936 × 512 × 4 bytes = 553 MiB against fit-params' 312 MiB for the small model,
    finding 9), so switching would not remove the excess.
19. *`usePoll` private; widths untestable in jsdom* → `usePoll` moved to `src/lib/usePoll.ts`;
    width tests assert each variant's text and container classes; real widths in e2e and the
    checklist.
20. *WP10d too small* → budget 2,500 lines; the verdict and bar arithmetic moved into WP10b's
    `placement.rs`.
