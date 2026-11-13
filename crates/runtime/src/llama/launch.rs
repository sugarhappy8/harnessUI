//! The llama-server command line and environment (ARCHITECTURE.md 7.3).
//!
//! Every default the pinned build would otherwise apply is overridden explicitly: port
//! 9931, auto-fitted context, four slots, the 8 GB prompt cache, the Web UI, CORS for any
//! origin and downloads from the network. The model's own file name is never the API's
//! model id: `-a` reports the fixed [`MODEL_ALIAS`] instead. The server never starts a
//! program of its own: `--video-ffmpeg-dir` names a folder that does not exist
//! ([`NO_FFMPEG_DIR`]), so it cannot find `ffmpeg` on `PATH`.
//!
//! The 0.0.6 load settings are typed fields with a fixed set of words each (0.0.6 D4):
//! [`LaunchConfig::gpu`], [`LaunchConfig::save_memory`] and
//! [`LaunchConfig::keep_in_memory`], and Fine control ([`LaunchConfig::fine`], item 6:
//! layers, expert parts, where the conversation memory lives, threads, fast attention and
//! the memory As much as fits leaves free). Nothing a user or a model types reaches the
//! command line: every value is a fixed word or a number from a typed choice.
//!
//! The fit estimate before a load is [`super::fit_params`] (item 6); As much as fits still
//! leaves the split to llama-server's own `--fit on` at load time.

use std::io;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::spawn::EnvSpec;

/// Value for `--cors-origins` that matches no browser origin.
///
/// The pinned build treats the argument as a literal comma-separated allow-list and answers
/// with `Access-Control-Allow-Origin: none`, which no page origin equals, so browsers refuse
/// every cross-origin response. Verified against b12110 on 2026-11-10.
pub const CORS_ORIGINS_NONE: &str = "none";

/// Environment variable that puts llama-server in managed-child mode: progress lines on
/// stdout, and exit when stdin closes (after load). The value is not read.
pub const ROUTER_PORT_ENV: &str = "LLAMA_SERVER_ROUTER_PORT";

/// Environment variable carrying the per-launch API key, so it never shows in the process
/// list.
pub const API_KEY_ENV: &str = "LLAMA_API_KEY";

/// Environment variable that restricts which GPUs the HIP runtime sees (ROCm only).
pub const HIP_VISIBLE_DEVICES_ENV: &str = "HIP_VISIBLE_DEVICES";

/// The model name the server reports through its API (`-a`): always this fixed word, so an
/// API response never shows the model file's path. [`LaunchConfig::alias`] holds it.
pub const MODEL_ALIAS: &str = "harnessui-local";

/// How much of the model goes on the graphics card ("Use the graphics card", 0.0.6 D4).
///
/// The arguments ([`build_args`]):
/// - `All`: `-ngl all --fit off`. Every layer on the device, so a model that does not fit
///   fails (the supervisor then retries with `Fit`, see [`super::server::Supervisor`]).
///   The pinned build's fitting would not change an explicit `-ngl` or `-c` anyway (its
///   library logs "n_gpu_layers already set by user ..., abort" and "context size set by
///   user ... -> no change"); `--fit off` only skips its measuring pass.
/// - `Fit`: no `-ngl` and `--fit on`. llama.cpp measures free device memory and puts as
///   many layers as fit on the device, the rest on the processor (for a mixture of experts
///   it may also keep expert weights on the processor). `-c` stays explicit, so only where
///   the weights go changes, never how much the model remembers.
/// - `Off`: `--device none -ngl 0 --fit off`. Nothing on the graphics card, and no device
///   at all, so llama.cpp does not offload the processor's work to it either
///   (`--op-offload`), whatever [`LaunchConfig::device`] holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GpuUse {
    /// Every layer on the graphics card. The default, and the behaviour of 0.0.4.
    #[default]
    All,
    /// As much as fits; the rest runs on the processor.
    Fit,
    /// None: the processor only.
    Off,
}

/// Expert parts kept on the processor, for a mixture-of-experts model (Fine control,
/// item 6). JSON `"none"`, `"all"` or `{"firstLayers":n}`.
///
/// - `None`: no flag; where the expert parts go follows the rest of the placement.
/// - `All`: `--cpu-moe`, every layer's expert parts on the processor.
/// - `FirstLayers(k)`: `--n-cpu-moe k`, the expert parts of the **first** `k` layers
///   (llama.cpp: "keep the MoE weights of the first N layers in the CPU").
///   `FirstLayers(0)` adds no flag and is no custom placement (a defence: the app's
///   resolver never produces it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpertsOnCpu {
    /// No flag.
    #[default]
    None,
    /// Every expert part on the processor (`--cpu-moe`).
    All,
    /// The expert parts of the first `n` layers on the processor (`--n-cpu-moe n`).
    FirstLayers(u32),
}

/// Fast attention (`-fa`), Fine control item 6. JSON `"auto"`, `"on"` or `"off"`. With
/// Save memory on, Save memory's own `-fa on` wins, whatever this says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlashAttn {
    /// No flag: llama.cpp's default (`auto`, on where the device supports it).
    #[default]
    Auto,
    /// `-fa on`.
    On,
    /// `-fa off`.
    Off,
}

/// Fine control as launch arguments (item 6, section 3.1). The default adds no argument
/// at all, so a launch without Fine control is exactly the launch of 0.0.6 without item 6.
/// The app's resolver fills only what reaches the command line for the chosen graphics
/// card use; [`build_args`] still ignores every field that cannot apply (no device, Don't
/// use it, Save memory), so a stray value never changes the arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FineLaunch {
    /// Layers on the graphics card, counted from the back by llama.cpp (`-ngl N`). A
    /// custom placement: replaces All of it and As much as fits, and launches with
    /// `--fit off`.
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    /// Expert parts on the processor (`--cpu-moe` or `--n-cpu-moe N`). Anything but
    /// [`ExpertsOnCpu::None`] (and `FirstLayers(0)`) is a custom placement.
    #[serde(default)]
    pub experts_on_cpu: ExpertsOnCpu,
    /// Keep the conversation memory in computer memory (`-nkvo`), with a graphics device.
    #[serde(default)]
    pub kv_in_computer_memory: bool,
    /// Processor threads (`-t N`). `None` and `Some(0)` add nothing (llama.cpp's default).
    #[serde(default)]
    pub threads: Option<u16>,
    /// Fast attention (`-fa on` / `-fa off`), when Save memory is off.
    #[serde(default)]
    pub flash_attn: FlashAttn,
    /// Graphics memory As much as fits leaves free, in MiB (`-fitt N`). Only with As much
    /// as fits, a device and no custom placement.
    #[serde(default)]
    pub fit_margin_mib: Option<u32>,
}

impl FineLaunch {
    /// Whether layers or expert parts were chosen by hand: a custom placement, which
    /// replaces All of it and As much as fits (`--fit off`, item 6 I6-2 and I6-3).
    pub fn custom_placement(&self) -> bool {
        self.gpu_layers.is_some() || self.experts_flag().is_some()
    }

    /// The expert flag and its number, when one applies.
    fn experts_flag(&self) -> Option<Vec<String>> {
        match self.experts_on_cpu {
            ExpertsOnCpu::None | ExpertsOnCpu::FirstLayers(0) => None,
            ExpertsOnCpu::All => Some(vec!["--cpu-moe".to_owned()]),
            ExpertsOnCpu::FirstLayers(n) => Some(vec!["--n-cpu-moe".to_owned(), n.to_string()]),
        }
    }
}

/// Everything that varies between launches.
///
/// `Debug` shows the API key as `<redacted>`, so a logged config never leaks it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchConfig {
    /// The model file (`-m`).
    pub model: PathBuf,
    /// The vision projector (`--mmproj`), when the model has one.
    pub mmproj: Option<PathBuf>,
    /// Context length (`-c`). Explicit, so llama.cpp never shrinks it silently.
    pub ctx: u32,
    /// Device name from the probe (`Vulkan0`, `ROCm0`, `MTL0`); `None` means CPU (`none`).
    pub device: Option<String>,
    /// Host prompt cache in MiB (`--cache-ram`).
    pub cache_ram_mib: u32,
    /// Thinking budget in tokens (`--reasoning-budget`); `None` keeps the build's default.
    pub reasoning_budget: Option<i32>,
    /// Loopback port, chosen by the app.
    pub port: u16,
    /// Per-launch API key, passed in [`API_KEY_ENV`].
    pub api_key: String,
    /// `HIP_VISIBLE_DEVICES` for ROCm, so the HIP runtime never sees the integrated GPU.
    pub hip_visible_devices: Option<String>,
    /// How much of the model goes on the graphics card (`-ngl`, `--fit`; see [`GpuUse`]).
    #[serde(default)]
    pub gpu: GpuUse,
    /// Save memory: flash attention and a q8_0 conversation memory (`-fa on -ctk q8_0
    /// -ctv q8_0`), half the size of the default f16 one. Off adds nothing, so llama.cpp's
    /// defaults (flash attention when supported, f16) apply as in 0.0.4.
    #[serde(default)]
    pub save_memory: bool,
    /// Keep the model in memory (`-lm mmap+mlock`). Off adds nothing (llama.cpp's `auto`,
    /// as in 0.0.4).
    #[serde(default)]
    pub keep_in_memory: bool,
    /// The model id the API reports (`-a`): [`MODEL_ALIAS`]. Only one plain word reaches
    /// the command line; anything else is replaced by [`MODEL_ALIAS`] (see [`build_args`]).
    #[serde(default = "default_alias")]
    pub alias: String,
    /// Fine control (item 6): placement, conversation memory place, threads, fast
    /// attention and the margin As much as fits leaves free (see [`FineLaunch`]). The
    /// default adds nothing.
    #[serde(default)]
    pub fine: FineLaunch,
}

fn default_alias() -> String {
    MODEL_ALIAS.to_owned()
}

impl Default for LaunchConfig {
    /// The app's fixed choices (`-ngl all`, a 512 MiB prompt cache, no thinking budget,
    /// the fixed alias) with no model, port or key. Port 0 and an empty key are refused by
    /// [`super::server::start`], so a config must get them from [`LaunchConfig::new`] or
    /// struct update syntax.
    fn default() -> Self {
        Self {
            model: PathBuf::new(),
            mmproj: None,
            ctx: 8192,
            device: None,
            cache_ram_mib: 512,
            reasoning_budget: None,
            port: 0,
            api_key: String::new(),
            hip_visible_devices: None,
            gpu: GpuUse::All,
            save_memory: false,
            keep_in_memory: false,
            alias: default_alias(),
            fine: FineLaunch::default(),
        }
    }
}

impl LaunchConfig {
    /// A launch of `model` with `ctx` tokens of context on `port` with `api_key`, and the
    /// defaults of [`LaunchConfig::default`] for everything else (CPU device, every layer).
    pub fn new(model: PathBuf, ctx: u32, port: u16, api_key: String) -> Self {
        Self {
            model,
            ctx,
            port,
            api_key,
            ..Self::default()
        }
    }

    /// The `--device` word: the probed device, or `none` for the processor, for an empty
    /// name and for [`GpuUse::Off`].
    fn device_arg(&self) -> &str {
        match self.gpu {
            GpuUse::Off => "none",
            GpuUse::All | GpuUse::Fit => self
                .device
                .as_deref()
                .filter(|d| !d.is_empty())
                .unwrap_or("none"),
        }
    }

    /// Whether the command line names a device (`--device` is not `none`), so a graphics
    /// card can get part of the work. The supervisor decides its retries and step-downs by
    /// this, not by the backend's label.
    pub fn uses_device(&self) -> bool {
        self.device_arg() != "none"
    }
}

/// What `Debug` prints instead of a secret.
pub(crate) const REDACTED: &str = "<redacted>";

impl std::fmt::Debug for LaunchConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchConfig")
            .field("model", &self.model)
            .field("mmproj", &self.mmproj)
            .field("ctx", &self.ctx)
            .field("device", &self.device)
            .field("cache_ram_mib", &self.cache_ram_mib)
            .field("reasoning_budget", &self.reasoning_budget)
            .field("port", &self.port)
            .field("api_key", &REDACTED)
            .field("hip_visible_devices", &self.hip_visible_devices)
            .field("gpu", &self.gpu)
            .field("save_memory", &self.save_memory)
            .field("keep_in_memory", &self.keep_in_memory)
            .field("alias", &self.alias)
            .field("fine", &self.fine)
            .finish()
    }
}

/// The conversation memory's number type with Save memory on (`-ctk`, `-ctv`): 8 bits per
/// value plus a scale per 32 values, 34 bytes where f16 takes 64.
pub const SAVE_MEMORY_CACHE_TYPE: &str = "q8_0";

/// The folder passed to `--video-ffmpeg-dir`: one that never exists, so llama-server never
/// starts `ffprobe` or `ffmpeg`.
///
/// The pinned build's media helper (`tools/mtmd/mtmd-helper.cpp`) hands video, and every
/// image its own decoder cannot read (WebP, or any undecodable bytes sent as an image), to
/// `ffprobe` and `ffmpeg` through `subprocess_create` with PATH search, unless this option
/// names a folder: then it runs `<folder>/ffprobe` (`.exe` on Windows) and nothing else.
/// Such a process would bypass `spawn.rs` (ARCHITECTURE.md 7.3) and feed outside programs
/// whatever a request carries. A relative path with a folder part is resolved in the
/// server's working directory, the backend's runtime folder, and is never looked up on
/// `PATH` by the system calls it uses on Unix and Windows; the runtime folder holds no such
/// folder, so the start fails and the request gets an error. Checked against the pinned
/// `libmtmd` with a fake `ffprobe` on `PATH`: started without the option, not with it.
///
/// So the in-app chat cannot send WebP images to the model either; they fail the same way
/// on every computer, where today they work only when ffmpeg happens to be installed.
pub const NO_FFMPEG_DIR: &str = "no-ffmpeg";

/// The load mode with Keep the model in memory on (`-lm`).
pub const KEEP_IN_MEMORY_LOAD_MODE: &str = "mmap+mlock";

/// Longest alias passed to `-a`.
const MAX_ALIAS_LEN: usize = 64;

/// The exact argument list for `cfg`:
///
/// ```text
/// --host 127.0.0.1 --port P --no-webui --no-slots --offline --jinja --cors-origins none
/// --no-context-shift --video-ffmpeg-dir no-ffmpeg -a harnessui-local -m M [--mmproj X]
/// -c C -np 1 --device D <placement> [--cpu-moe | --n-cpu-moe K] [-nkvo] [-t T]
/// [-fa on | -fa off | -fa on -ctk q8_0 -ctv q8_0] [-lm mmap+mlock] --cache-ram R
/// [--reasoning-budget B]
/// ```
///
/// `D` is the device, or `none` for the processor and for [`GpuUse::Off`]. The placement
/// and its extras (item 6, section 3.1; [`placement_args`]):
///
/// | Card use | device | Fine control           | placement and extras                 |
/// |----------|--------|------------------------|--------------------------------------|
/// | Off      | `none` | anything               | `-ngl 0 --fit off`                   |
/// | All      | `none` | anything               | `-ngl all --fit off`                 |
/// | Fit      | `none` | anything               | `--fit on`                           |
/// | All      | yes    | no custom placement    | `-ngl all --fit off`                 |
/// | Fit      | yes    | no custom placement    | `--fit on` [`-fitt M`]               |
/// | All, Fit | yes    | layers N               | `-ngl N --fit off`                   |
/// | All, Fit | yes    | expert parts only      | `-ngl all --fit off` + expert flag   |
/// | All, Fit | yes    | layers N, expert parts | `-ngl N --fit off` + expert flag     |
///
/// `-nkvo` only with a device. Independent of the placement: `-t T` when threads are
/// chosen; `-fa on` / `-fa off` when fast attention is chosen and Save memory is off; Save
/// memory gives `-fa on -ctk q8_0 -ctv q8_0` whatever fast attention says.
///
/// Every word is fixed except the paths, the numbers, the device name from the probe and
/// the alias, which is used only when it is one plain word (otherwise [`MODEL_ALIAS`]).
/// Paths are converted lossily; [`super::server::start`] refuses paths that are not valid
/// Unicode before it gets here.
pub fn build_args(cfg: &LaunchConfig) -> Vec<String> {
    let port = cfg.port.to_string();
    let model = cfg.model.to_string_lossy();
    let mut args: Vec<String> = [
        "--host",
        "127.0.0.1",
        "--port",
        port.as_str(),
        "--no-webui",
        "--no-slots",
        "--offline",
        "--jinja",
        "--cors-origins",
        CORS_ORIGINS_NONE,
        "--no-context-shift",
        "--video-ffmpeg-dir",
        NO_FFMPEG_DIR,
        "-a",
        alias_arg(&cfg.alias),
        "-m",
        model.as_ref(),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    if let Some(mmproj) = &cfg.mmproj {
        args.push("--mmproj".to_owned());
        args.push(mmproj.to_string_lossy().into_owned());
    }
    let device = cfg.device_arg();
    args.extend([
        "-c".to_owned(),
        cfg.ctx.to_string(),
        "-np".to_owned(),
        "1".to_owned(),
        "--device".to_owned(),
        device.to_owned(),
    ]);
    args.extend(placement_args(cfg));
    if let Some(threads) = cfg.fine.threads.filter(|&t| t > 0) {
        args.push("-t".to_owned());
        args.push(threads.to_string());
    }
    if !cfg.save_memory {
        match cfg.fine.flash_attn {
            FlashAttn::Auto => {}
            FlashAttn::On => args.extend(["-fa", "on"].map(str::to_owned)),
            FlashAttn::Off => args.extend(["-fa", "off"].map(str::to_owned)),
        }
    }
    if cfg.save_memory {
        args.extend(
            [
                "-fa",
                "on",
                "-ctk",
                SAVE_MEMORY_CACHE_TYPE,
                "-ctv",
                SAVE_MEMORY_CACHE_TYPE,
            ]
            .map(str::to_owned),
        );
    }
    if cfg.keep_in_memory {
        args.extend(["-lm", KEEP_IN_MEMORY_LOAD_MODE].map(str::to_owned));
    }
    args.extend(["--cache-ram".to_owned(), cfg.cache_ram_mib.to_string()]);
    if let Some(budget) = cfg.reasoning_budget {
        args.push("--reasoning-budget".to_owned());
        args.push(budget.to_string());
    }
    args
}

/// The placement words after `--device D` (see the table of [`build_args`]): where the
/// layers and expert parts go, `--fit`, the margin As much as fits leaves free and where
/// the conversation memory lives. Without a device (the processor, Don't use it, or no
/// probed device) only the card choice's own words of 0.0.6 without item 6 appear: Fine
/// control's placement, expert flags, `-nkvo` and `-fitt` have nothing to act on.
fn placement_args(cfg: &LaunchConfig) -> Vec<String> {
    let words = |list: &[&str]| -> Vec<String> { list.iter().map(|s| (*s).to_owned()).collect() };
    if !cfg.uses_device() {
        return match cfg.gpu {
            GpuUse::All => words(&["-ngl", "all", "--fit", "off"]),
            GpuUse::Fit => words(&["--fit", "on"]),
            GpuUse::Off => words(&["-ngl", "0", "--fit", "off"]),
        };
    }
    let fine = &cfg.fine;
    let mut args = if fine.custom_placement() {
        let layers = fine
            .gpu_layers
            .map_or_else(|| "all".to_owned(), |n| n.to_string());
        let mut args = vec!["-ngl".to_owned(), layers, "--fit".to_owned(), "off".to_owned()];
        args.extend(fine.experts_flag().unwrap_or_default());
        args
    } else {
        match cfg.gpu {
            GpuUse::All => words(&["-ngl", "all", "--fit", "off"]),
            GpuUse::Fit => {
                let mut args = words(&["--fit", "on"]);
                if let Some(margin) = fine.fit_margin_mib {
                    args.push("-fitt".to_owned());
                    args.push(margin.to_string());
                }
                args
            }
            // `uses_device` is false for Off; kept for completeness.
            GpuUse::Off => words(&["-ngl", "0", "--fit", "off"]),
        }
    };
    if fine.kv_in_computer_memory {
        args.push("-nkvo".to_owned());
    }
    args
}

/// The `-a` value: `alias` when it is one plain word (lower-case ASCII letters, digits,
/// `-`, `_` and `.`, starting with a letter or digit, at most [`MAX_ALIAS_LEN`]
/// characters), otherwise [`MODEL_ALIAS`]. llama-server splits the value at commas into
/// several names and reports it in every response, so only a predictable word goes there.
fn alias_arg(alias: &str) -> &str {
    let plain = !alias.is_empty()
        && alias.len() <= MAX_ALIAS_LEN
        && alias
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && alias.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        });
    if plain { alias } else { MODEL_ALIAS }
}

/// Environment variable that stops ggml from starting a debugger when a llama.cpp program
/// crashes (item 6, I6-8). Without it, ggml's `ggml_print_backtrace` on Linux starts `gdb`
/// (then `lldb`) found on `PATH`, which attaches to the crashing process and prints its
/// argument values: a process the app did not start through `crate::spawn`, reading the
/// memory of a server that holds chat text and its API key. Any value turns it off; macOS
/// uses a debugger only with `GGML_BACKTRACE_LLDB`, which the scrubbed environment never
/// passes.
pub const NO_BACKTRACE_ENV: &str = "GGML_NO_BACKTRACE";

/// The environment for `cfg`: the platform allowlist, the API key, managed-child mode,
/// [`NO_BACKTRACE_ENV`] and, for ROCm, `HIP_VISIBLE_DEVICES`. Nothing else from the app's
/// environment is passed, so `LLAMA_ARG_*`, other `GGML_*`, `HF_TOKEN` and `HF_ENDPOINT`
/// never reach the server.
pub fn build_env(cfg: &LaunchConfig) -> EnvSpec {
    build_env_on(EnvSpec::minimal(), cfg)
}

/// [`build_env`] on top of a backend's own base environment
/// ([`Backend::base_env`](crate::Backend::base_env), which carries [`NO_BACKTRACE_ENV`]
/// too; it is set here again so a caller's other base cannot lose it).
pub fn build_env_on(base: EnvSpec, cfg: &LaunchConfig) -> EnvSpec {
    let mut env = base
        .set(NO_BACKTRACE_ENV, "1")
        .set(API_KEY_ENV, cfg.api_key.clone())
        .set(ROUTER_PORT_ENV, "1");
    if let Some(devices) = &cfg.hip_visible_devices {
        env = env.set(HIP_VISIBLE_DEVICES_ENV, devices.clone());
    }
    env
}

/// A fresh API key: 32 URL-safe characters (192 bits) from the thread-local CSPRNG.
pub fn random_api_key() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut rng = rand::rng();
    (0..32)
        .map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())]))
        .collect()
}

/// A loopback port that was free a moment ago. The OS picks it; another process could take
/// it before llama-server binds, which [`super::server::start`] detects.
pub fn free_loopback_port() -> io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn config() -> LaunchConfig {
        LaunchConfig {
            model: PathBuf::from("/models/m.gguf"),
            mmproj: None,
            ctx: 8192,
            device: Some("Vulkan0".into()),
            cache_ram_mib: 512,
            reasoning_budget: None,
            port: 40123,
            api_key: "k".into(),
            hip_visible_devices: None,
            ..LaunchConfig::default()
        }
    }

    /// The defaults are the app's fixed choices; the alias is the fixed model id.
    #[test]
    fn defaults_and_constructor() {
        let d = LaunchConfig::default();
        assert_eq!(d.gpu, GpuUse::All);
        assert_eq!(d.cache_ram_mib, 512);
        assert_eq!(d.reasoning_budget, None);
        assert!(!d.save_memory && !d.keep_in_memory);
        assert_eq!(d.alias, "harnessui-local");
        assert_eq!(d.alias, MODEL_ALIAS);
        let n = LaunchConfig::new(PathBuf::from("/m.gguf"), 4096, 40000, "key".into());
        assert_eq!(n.model, PathBuf::from("/m.gguf"));
        assert_eq!((n.ctx, n.port, n.api_key.as_str()), (4096, 40000, "key"));
        assert_eq!(n.alias, MODEL_ALIAS);
        assert_eq!(n.device, None);
    }

    /// A config written by 0.0.5 still reads: the new fields take their defaults, and its
    /// `gpu_layers` (removed in 0.0.6, when [`LaunchConfig::gpu`] took over `-ngl`) is
    /// ignored, whatever it says.
    #[test]
    fn old_json_reads_with_defaults() {
        let mut v = serde_json::to_value(config()).expect("json");
        let obj = v.as_object_mut().expect("object");
        for key in ["gpu", "save_memory", "keep_in_memory", "alias"] {
            assert!(obj.remove(key).is_some(), "{key}");
        }
        assert!(obj.get("gpu_layers").is_none(), "no longer a field");
        obj.insert(
            "gpu_layers".into(),
            serde_json::json!({"mode": "count", "layers": 7}),
        );
        let back: LaunchConfig = serde_json::from_value(v).expect("reads");
        assert_eq!(back, config());
        assert_eq!(
            serde_json::to_value(GpuUse::Fit).expect("json"),
            serde_json::json!("fit")
        );
    }

    /// Every argument of the default launch: every layer on the graphics card, nothing
    /// else changed from 0.0.4, the fixed alias, and `--fit off` so llama.cpp never
    /// second-guesses the explicit choices.
    #[test]
    fn args_are_exact() {
        assert_eq!(
            build_args(&config()),
            vec![
                "--host",
                "127.0.0.1",
                "--port",
                "40123",
                "--no-webui",
                "--no-slots",
                "--offline",
                "--jinja",
                "--cors-origins",
                "none",
                "--no-context-shift",
                "--video-ffmpeg-dir",
                "no-ffmpeg",
                "-a",
                "harnessui-local",
                "-m",
                "/models/m.gguf",
                "-c",
                "8192",
                "-np",
                "1",
                "--device",
                "Vulkan0",
                "-ngl",
                "all",
                "--fit",
                "off",
                "--cache-ram",
                "512",
            ]
        );
    }

    /// The arguments between `--device` and `--cache-ram`, where the 0.0.6 choices go.
    fn choice_args(cfg: &LaunchConfig) -> Vec<String> {
        let args = build_args(cfg);
        let from = args.iter().position(|a| a == "--device").expect("--device");
        let to = args
            .iter()
            .position(|a| a == "--cache-ram")
            .expect("--cache-ram");
        args[from..to].to_vec()
    }

    /// Use the graphics card: All of it, As much as fits, Don't use it.
    #[test]
    fn args_are_exact_for_every_gpu_choice() {
        let all = LaunchConfig {
            gpu: GpuUse::All,
            ..config()
        };
        assert_eq!(
            choice_args(&all),
            ["--device", "Vulkan0", "-ngl", "all", "--fit", "off"]
        );
        // As much as fits: no -ngl at all, so llama.cpp's fitting decides it.
        let fit = LaunchConfig {
            gpu: GpuUse::Fit,
            ..config()
        };
        assert_eq!(choice_args(&fit), ["--device", "Vulkan0", "--fit", "on"]);
        assert!(!build_args(&fit).iter().any(|a| a == "-ngl"));
        // Don't use it: nothing on the graphics card, and no graphics card at all, so
        // llama.cpp does not move work there behind the user's back.
        let off = LaunchConfig {
            gpu: GpuUse::Off,
            ..config()
        };
        assert_eq!(
            choice_args(&off),
            ["--device", "none", "-ngl", "0", "--fit", "off"]
        );
    }

    /// Save memory and Keep the model in memory, alone and together.
    #[test]
    fn args_are_exact_for_save_memory_and_keep_in_memory() {
        let save = LaunchConfig {
            save_memory: true,
            ..config()
        };
        assert_eq!(
            choice_args(&save),
            [
                "--device", "Vulkan0", "-ngl", "all", "--fit", "off", "-fa", "on", "-ctk", "q8_0",
                "-ctv", "q8_0"
            ]
        );
        let keep = LaunchConfig {
            keep_in_memory: true,
            ..config()
        };
        assert_eq!(
            choice_args(&keep),
            [
                "--device",
                "Vulkan0",
                "-ngl",
                "all",
                "--fit",
                "off",
                "-lm",
                "mmap+mlock"
            ]
        );
        let both = LaunchConfig {
            gpu: GpuUse::Fit,
            save_memory: true,
            keep_in_memory: true,
            mmproj: Some(PathBuf::from("/models/mmproj.gguf")),
            reasoning_budget: Some(0),
            ..config()
        };
        assert_eq!(
            build_args(&both),
            vec![
                "--host",
                "127.0.0.1",
                "--port",
                "40123",
                "--no-webui",
                "--no-slots",
                "--offline",
                "--jinja",
                "--cors-origins",
                "none",
                "--no-context-shift",
                "--video-ffmpeg-dir",
                "no-ffmpeg",
                "-a",
                "harnessui-local",
                "-m",
                "/models/m.gguf",
                "--mmproj",
                "/models/mmproj.gguf",
                "-c",
                "8192",
                "-np",
                "1",
                "--device",
                "Vulkan0",
                "--fit",
                "on",
                "-fa",
                "on",
                "-ctk",
                "q8_0",
                "-ctv",
                "q8_0",
                "-lm",
                "mmap+mlock",
                "--cache-ram",
                "512",
                "--reasoning-budget",
                "0",
            ]
        );
    }

    /// Whatever the choices, the fixed protections stay and the context stays explicit.
    #[test]
    fn every_choice_keeps_the_fixed_arguments() {
        for gpu in [GpuUse::All, GpuUse::Fit, GpuUse::Off] {
            for save_memory in [false, true] {
                for keep_in_memory in [false, true] {
                    let cfg = LaunchConfig {
                        gpu,
                        save_memory,
                        keep_in_memory,
                        ..config()
                    };
                    let joined = build_args(&cfg).join(" ");
                    for fixed in [
                        "--host 127.0.0.1 --port 40123 --no-webui --no-slots --offline --jinja",
                        "--cors-origins none --no-context-shift --video-ffmpeg-dir no-ffmpeg",
                        "-a harnessui-local",
                        "-c 8192 -np 1 --device",
                        "--cache-ram 512",
                    ] {
                        assert!(joined.contains(fixed), "{fixed} missing from {joined}");
                    }
                    assert_eq!(joined.contains("-fa on"), save_memory, "{joined}");
                    assert_eq!(joined.contains("mmap+mlock"), keep_in_memory, "{joined}");
                }
            }
        }
    }

    /// Every combination of the choices, as the exact list the spec's table gives: the
    /// card choice, Save memory, Keep in memory, a graphics device or the processor (and an
    /// empty device name), with and without an image reader and a thinking budget. Each
    /// flag appears once, and `-ngl` only where a choice sets it.
    #[test]
    fn args_are_exact_for_every_combination() {
        let gpu_words = |gpu: GpuUse| -> &'static [&'static str] {
            match gpu {
                GpuUse::All => &["-ngl", "all", "--fit", "off"],
                GpuUse::Fit => &["--fit", "on"],
                GpuUse::Off => &["-ngl", "0", "--fit", "off"],
            }
        };
        let mut seen = 0;
        for gpu in [GpuUse::All, GpuUse::Fit, GpuUse::Off] {
            for save_memory in [false, true] {
                for keep_in_memory in [false, true] {
                    for device in [Some("Vulkan0"), None, Some("")] {
                        for mmproj in [false, true] {
                            for budget in [None, Some(0)] {
                                let cfg = LaunchConfig {
                                    gpu,
                                    save_memory,
                                    keep_in_memory,
                                    device: device.map(str::to_owned),
                                    mmproj: mmproj.then(|| PathBuf::from("/models/p.gguf")),
                                    reasoning_budget: budget,
                                    ..config()
                                };
                                let mut want: Vec<&str> = vec![
                                    "--host",
                                    "127.0.0.1",
                                    "--port",
                                    "40123",
                                    "--no-webui",
                                    "--no-slots",
                                    "--offline",
                                    "--jinja",
                                    "--cors-origins",
                                    "none",
                                    "--no-context-shift",
                                    "--video-ffmpeg-dir",
                                    "no-ffmpeg",
                                    "-a",
                                    "harnessui-local",
                                    "-m",
                                    "/models/m.gguf",
                                ];
                                if mmproj {
                                    want.extend(["--mmproj", "/models/p.gguf"]);
                                }
                                // Don't use it never names a device, whatever was probed.
                                let shown = match (gpu, device) {
                                    (GpuUse::Off, _) | (_, None | Some("")) => "none",
                                    (_, Some(name)) => name,
                                };
                                want.extend(["-c", "8192", "-np", "1", "--device", shown]);
                                want.extend(gpu_words(gpu));
                                if save_memory {
                                    want.extend(["-fa", "on", "-ctk", "q8_0", "-ctv", "q8_0"]);
                                }
                                if keep_in_memory {
                                    want.extend(["-lm", "mmap+mlock"]);
                                }
                                want.extend(["--cache-ram", "512"]);
                                if budget.is_some() {
                                    want.extend(["--reasoning-budget", "0"]);
                                }
                                let args = build_args(&cfg);
                                assert_eq!(args, want, "{cfg:?}");
                                assert_eq!(cfg.uses_device(), shown != "none", "{cfg:?}");
                                for flag in [
                                    "--host",
                                    "--port",
                                    "--video-ffmpeg-dir",
                                    "-a",
                                    "-m",
                                    "-c",
                                    "-np",
                                    "--device",
                                    "--fit",
                                    "--cache-ram",
                                ] {
                                    let n = args.iter().filter(|a| *a == flag).count();
                                    assert_eq!(n, 1, "{flag} in {args:?}");
                                }
                                let ngl = args.iter().filter(|a| *a == "-ngl").count();
                                assert_eq!(ngl, usize::from(gpu != GpuUse::Fit), "{args:?}");
                                seen += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(seen, 3 * 2 * 2 * 3 * 2 * 2);
    }

    /// Fine control (item 6, section 3.1): every row of the table, as the exact words
    /// between `--device` and `--cache-ram`. Each flag appears once at most, and the
    /// fixed protections stay.
    #[test]
    fn args_are_exact_for_every_fine_control() {
        let with = |gpu: GpuUse, fine: FineLaunch| LaunchConfig {
            gpu,
            fine,
            ..config()
        };
        let layers = |n: u32| FineLaunch {
            gpu_layers: Some(n),
            ..FineLaunch::default()
        };
        let experts = |e: ExpertsOnCpu| FineLaunch {
            experts_on_cpu: e,
            ..FineLaunch::default()
        };
        let margin = |mib: u32| FineLaunch {
            fit_margin_mib: Some(mib),
            ..FineLaunch::default()
        };
        // Every Fine control value set at once.
        let everything = FineLaunch {
            gpu_layers: Some(30),
            experts_on_cpu: ExpertsOnCpu::FirstLayers(5),
            kv_in_computer_memory: true,
            threads: Some(4),
            flash_attn: FlashAttn::Off,
            fit_margin_mib: Some(2048),
        };
        let cases: Vec<(&str, LaunchConfig, Vec<&str>)> = vec![
            (
                "All, no custom placement",
                with(GpuUse::All, FineLaunch::default()),
                vec!["--device", "Vulkan0", "-ngl", "all", "--fit", "off"],
            ),
            (
                "As much as fits, no custom placement",
                with(GpuUse::Fit, FineLaunch::default()),
                vec!["--device", "Vulkan0", "--fit", "on"],
            ),
            (
                "As much as fits with Leave free",
                with(GpuUse::Fit, margin(2048)),
                vec!["--device", "Vulkan0", "--fit", "on", "-fitt", "2048"],
            ),
            (
                "Leave free with All of it adds nothing",
                with(GpuUse::All, margin(4096)),
                vec!["--device", "Vulkan0", "-ngl", "all", "--fit", "off"],
            ),
            (
                "layers with All",
                with(GpuUse::All, layers(30)),
                vec!["--device", "Vulkan0", "-ngl", "30", "--fit", "off"],
            ),
            (
                "layers with As much as fits: no -fitt with a custom placement",
                with(
                    GpuUse::Fit,
                    FineLaunch {
                        fit_margin_mib: Some(512),
                        ..layers(30)
                    },
                ),
                vec!["--device", "Vulkan0", "-ngl", "30", "--fit", "off"],
            ),
            (
                "no layers on the card, with a device",
                with(GpuUse::All, layers(0)),
                vec!["--device", "Vulkan0", "-ngl", "0", "--fit", "off"],
            ),
            (
                "every expert part with All",
                with(GpuUse::All, experts(ExpertsOnCpu::All)),
                vec![
                    "--device",
                    "Vulkan0",
                    "-ngl",
                    "all",
                    "--fit",
                    "off",
                    "--cpu-moe",
                ],
            ),
            (
                "the first expert parts with As much as fits",
                with(GpuUse::Fit, experts(ExpertsOnCpu::FirstLayers(20))),
                vec![
                    "--device",
                    "Vulkan0",
                    "-ngl",
                    "all",
                    "--fit",
                    "off",
                    "--n-cpu-moe",
                    "20",
                ],
            ),
            (
                "layers and expert parts",
                with(
                    GpuUse::Fit,
                    FineLaunch {
                        experts_on_cpu: ExpertsOnCpu::FirstLayers(5),
                        ..layers(40)
                    },
                ),
                vec![
                    "--device",
                    "Vulkan0",
                    "-ngl",
                    "40",
                    "--fit",
                    "off",
                    "--n-cpu-moe",
                    "5",
                ],
            ),
            (
                "layers and every expert part",
                with(
                    GpuUse::All,
                    FineLaunch {
                        experts_on_cpu: ExpertsOnCpu::All,
                        ..layers(40)
                    },
                ),
                vec![
                    "--device",
                    "Vulkan0",
                    "-ngl",
                    "40",
                    "--fit",
                    "off",
                    "--cpu-moe",
                ],
            ),
            (
                "FirstLayers(0) adds nothing with As much as fits",
                with(GpuUse::Fit, experts(ExpertsOnCpu::FirstLayers(0))),
                vec!["--device", "Vulkan0", "--fit", "on"],
            ),
            (
                "FirstLayers(0) adds nothing with All",
                with(GpuUse::All, experts(ExpertsOnCpu::FirstLayers(0))),
                vec!["--device", "Vulkan0", "-ngl", "all", "--fit", "off"],
            ),
            (
                "conversation memory in computer memory with All",
                with(
                    GpuUse::All,
                    FineLaunch {
                        kv_in_computer_memory: true,
                        ..FineLaunch::default()
                    },
                ),
                vec!["--device", "Vulkan0", "-ngl", "all", "--fit", "off", "-nkvo"],
            ),
            (
                "conversation memory in computer memory with As much as fits",
                with(
                    GpuUse::Fit,
                    FineLaunch {
                        kv_in_computer_memory: true,
                        ..margin(512)
                    },
                ),
                vec!["--device", "Vulkan0", "--fit", "on", "-fitt", "512", "-nkvo"],
            ),
            (
                "threads",
                with(
                    GpuUse::All,
                    FineLaunch {
                        threads: Some(8),
                        ..FineLaunch::default()
                    },
                ),
                vec![
                    "--device", "Vulkan0", "-ngl", "all", "--fit", "off", "-t", "8",
                ],
            ),
            (
                "zero threads adds nothing",
                with(
                    GpuUse::All,
                    FineLaunch {
                        threads: Some(0),
                        ..FineLaunch::default()
                    },
                ),
                vec!["--device", "Vulkan0", "-ngl", "all", "--fit", "off"],
            ),
            (
                "fast attention on",
                with(
                    GpuUse::All,
                    FineLaunch {
                        flash_attn: FlashAttn::On,
                        ..FineLaunch::default()
                    },
                ),
                vec![
                    "--device", "Vulkan0", "-ngl", "all", "--fit", "off", "-fa", "on",
                ],
            ),
            (
                "fast attention off",
                with(
                    GpuUse::All,
                    FineLaunch {
                        flash_attn: FlashAttn::Off,
                        ..FineLaunch::default()
                    },
                ),
                vec![
                    "--device", "Vulkan0", "-ngl", "all", "--fit", "off", "-fa", "off",
                ],
            ),
            (
                "Save memory wins over fast attention off",
                LaunchConfig {
                    save_memory: true,
                    ..with(
                        GpuUse::All,
                        FineLaunch {
                            flash_attn: FlashAttn::Off,
                            ..FineLaunch::default()
                        },
                    )
                },
                vec![
                    "--device", "Vulkan0", "-ngl", "all", "--fit", "off", "-fa", "on", "-ctk",
                    "q8_0", "-ctv", "q8_0",
                ],
            ),
            (
                "everything with a device",
                with(GpuUse::Fit, everything.clone()),
                vec![
                    "--device",
                    "Vulkan0",
                    "-ngl",
                    "30",
                    "--fit",
                    "off",
                    "--n-cpu-moe",
                    "5",
                    "-nkvo",
                    "-t",
                    "4",
                    "-fa",
                    "off",
                ],
            ),
            (
                "Don't use it with everything set adds only -t and -fa",
                with(GpuUse::Off, everything.clone()),
                vec![
                    "--device", "none", "-ngl", "0", "--fit", "off", "-t", "4", "-fa", "off",
                ],
            ),
            (
                "no device with All keeps the words of 0.0.6 without item 6",
                LaunchConfig {
                    device: None,
                    ..with(GpuUse::All, everything.clone())
                },
                vec![
                    "--device", "none", "-ngl", "all", "--fit", "off", "-t", "4", "-fa", "off",
                ],
            ),
            (
                "no device with As much as fits keeps the words of 0.0.6 without item 6",
                LaunchConfig {
                    device: None,
                    ..with(GpuUse::Fit, everything.clone())
                },
                vec!["--device", "none", "--fit", "on", "-t", "4", "-fa", "off"],
            ),
            (
                "an empty device name is no device",
                LaunchConfig {
                    device: Some(String::new()),
                    ..with(GpuUse::Fit, everything.clone())
                },
                vec!["--device", "none", "--fit", "on", "-t", "4", "-fa", "off"],
            ),
        ];
        for (name, cfg, want) in cases {
            assert_eq!(choice_args(&cfg), want, "{name}");
            let args = build_args(&cfg);
            for flag in [
                "-ngl",
                "--fit",
                "--cpu-moe",
                "--n-cpu-moe",
                "-nkvo",
                "-t",
                "-fa",
                "-fitt",
                "--device",
                "-c",
            ] {
                let n = args.iter().filter(|a| *a == flag).count();
                assert!(n <= 1, "{name}: {flag} {n} times in {args:?}");
            }
            let joined = args.join(" ");
            for fixed in [
                "--host 127.0.0.1 --port 40123 --no-webui --no-slots --offline --jinja",
                "--cors-origins none --no-context-shift --video-ffmpeg-dir no-ffmpeg",
                "-a harnessui-local",
                "-c 8192 -np 1 --device",
                "--cache-ram 512",
            ] {
                assert!(joined.contains(fixed), "{name}: {fixed} missing from {joined}");
            }
        }
    }

    /// The whole list with every Fine control value and every other option, so the order
    /// of section 3.1 is pinned end to end.
    #[test]
    fn fine_control_args_come_in_the_documented_order() {
        let cfg = LaunchConfig {
            mmproj: Some(PathBuf::from("/models/p.gguf")),
            keep_in_memory: true,
            reasoning_budget: Some(0),
            fine: FineLaunch {
                gpu_layers: Some(12),
                experts_on_cpu: ExpertsOnCpu::All,
                kv_in_computer_memory: true,
                threads: Some(6),
                flash_attn: FlashAttn::On,
                fit_margin_mib: None,
            },
            ..config()
        };
        assert_eq!(
            build_args(&cfg).join(" "),
            "--host 127.0.0.1 --port 40123 --no-webui --no-slots --offline --jinja \
             --cors-origins none --no-context-shift --video-ffmpeg-dir no-ffmpeg \
             -a harnessui-local -m /models/m.gguf --mmproj /models/p.gguf -c 8192 -np 1 \
             --device Vulkan0 -ngl 12 --fit off --cpu-moe -nkvo -t 6 -fa on -lm mmap+mlock \
             --cache-ram 512 --reasoning-budget 0"
        );
    }

    /// A custom placement is chosen layers or expert parts that move at least one layer.
    #[test]
    fn what_counts_as_a_custom_placement() {
        assert!(!FineLaunch::default().custom_placement());
        for custom in [
            FineLaunch {
                gpu_layers: Some(0),
                ..FineLaunch::default()
            },
            FineLaunch {
                experts_on_cpu: ExpertsOnCpu::All,
                ..FineLaunch::default()
            },
            FineLaunch {
                experts_on_cpu: ExpertsOnCpu::FirstLayers(1),
                ..FineLaunch::default()
            },
        ] {
            assert!(custom.custom_placement(), "{custom:?}");
        }
        for plain in [
            FineLaunch {
                experts_on_cpu: ExpertsOnCpu::FirstLayers(0),
                ..FineLaunch::default()
            },
            FineLaunch {
                kv_in_computer_memory: true,
                threads: Some(4),
                flash_attn: FlashAttn::Off,
                fit_margin_mib: Some(1024),
                ..FineLaunch::default()
            },
        ] {
            assert!(!plain.custom_placement(), "{plain:?}");
        }
    }

    /// A launch config written by 0.0.6 without item 6 still reads (Fine control at its
    /// defaults, so the same arguments), and one written with Fine control reads into a
    /// struct that has no `fine` (the other fields kept). The JSON shapes are the
    /// contract's (item 6, section 2.1).
    #[test]
    fn fine_launch_old_json_reads() {
        let mut v = serde_json::to_value(config()).expect("json");
        assert!(
            v.as_object_mut()
                .expect("object")
                .remove("fine")
                .is_some()
        );
        let back: LaunchConfig = serde_json::from_value(v).expect("reads");
        assert_eq!(back, config());
        assert_eq!(back.fine, FineLaunch::default());
        assert_eq!(build_args(&back), build_args(&config()));

        /// `LaunchConfig` as 0.0.6 had it before item 6.
        #[derive(Deserialize)]
        #[allow(dead_code)] // only some fields are compared
        struct Before {
            model: PathBuf,
            mmproj: Option<PathBuf>,
            ctx: u32,
            device: Option<String>,
            cache_ram_mib: u32,
            reasoning_budget: Option<i32>,
            port: u16,
            api_key: String,
            hip_visible_devices: Option<String>,
            #[serde(default)]
            gpu: GpuUse,
            #[serde(default)]
            save_memory: bool,
            #[serde(default)]
            keep_in_memory: bool,
            #[serde(default = "default_alias")]
            alias: String,
        }
        let with_fine = LaunchConfig {
            gpu: GpuUse::Fit,
            save_memory: true,
            fine: FineLaunch {
                gpu_layers: Some(30),
                experts_on_cpu: ExpertsOnCpu::FirstLayers(20),
                kv_in_computer_memory: true,
                threads: Some(8),
                flash_attn: FlashAttn::Off,
                fit_margin_mib: Some(2048),
            },
            ..config()
        };
        let json = serde_json::to_value(&with_fine).expect("json");
        assert_eq!(
            json["fine"],
            serde_json::json!({
                "gpuLayers": 30,
                "expertsOnCpu": {"firstLayers": 20},
                "kvInComputerMemory": true,
                "threads": 8,
                "flashAttn": "off",
                "fitMarginMib": 2048,
            })
        );
        let before: Before = serde_json::from_value(json.clone()).expect("reads");
        assert_eq!(before.model, with_fine.model);
        assert_eq!(before.ctx, 8192);
        assert_eq!(before.gpu, GpuUse::Fit);
        assert!(before.save_memory);
        assert_eq!(before.alias, MODEL_ALIAS);
        let again: LaunchConfig = serde_json::from_value(json).expect("reads");
        assert_eq!(again, with_fine);

        // Each field may be absent; the shapes of the two enums.
        let empty: FineLaunch = serde_json::from_str("{}").expect("reads");
        assert_eq!(empty, FineLaunch::default());
        for (value, json) in [
            (ExpertsOnCpu::None, r#""none""#),
            (ExpertsOnCpu::All, r#""all""#),
            (ExpertsOnCpu::FirstLayers(3), r#"{"firstLayers":3}"#),
        ] {
            assert_eq!(serde_json::to_string(&value).expect("json"), json);
            assert_eq!(serde_json::from_str::<ExpertsOnCpu>(json).expect("reads"), value);
        }
        for (value, json) in [
            (FlashAttn::Auto, r#""auto""#),
            (FlashAttn::On, r#""on""#),
            (FlashAttn::Off, r#""off""#),
        ] {
            assert_eq!(serde_json::to_string(&value).expect("json"), json);
            assert_eq!(serde_json::from_str::<FlashAttn>(json).expect("reads"), value);
        }
    }

    /// `Debug` shows Fine control (and still hides the key).
    #[test]
    fn debug_output_shows_fine_control() {
        let cfg = LaunchConfig {
            fine: FineLaunch {
                threads: Some(3),
                ..FineLaunch::default()
            },
            ..config()
        };
        let shown = format!("{cfg:?}");
        assert!(shown.contains("fine: FineLaunch {"), "{shown}");
        assert!(shown.contains("threads: Some(3)"), "{shown}");
        assert!(!shown.contains("api_key: \"k\""), "{shown}");
    }

    /// Every llama.cpp program the app starts gets `GGML_NO_BACKTRACE=1`, so a crash never
    /// starts a debugger that reads its memory (item 6, I6-8): the device probe and
    /// fit-params (both from [`Backend::base_env`](crate::Backend::base_env)), llama-server
    /// on top of it, and llama-server through [`build_env`] alone. Even when the app's own
    /// environment asks for a debugger.
    #[test]
    fn env_turns_off_debugger_backtraces() {
        use crate::llama::backend::{Backend, BackendKind, BackendSource};
        let parent: Vec<(OsString, OsString)> = [
            ("PATH", "/usr/bin"),
            ("GGML_BACKTRACE_LLDB", "1"),
            ("GGML_NO_BACKTRACE", ""),
        ]
        .into_iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect();
        let backend = Backend {
            kind: BackendKind::Rocm,
            dir: PathBuf::from("/rt"),
            server: PathBuf::from("/rt/llama-server"),
            fit_params: Some(PathBuf::from("/rt/llama-fit-params")),
            build: "b1".into(),
            source: BackendSource::Installed,
        };
        let measure = crate::llama::fit_params::MeasureConfig {
            model: PathBuf::from("/m.gguf"),
            ctx: 4096,
            device: Some("ROCm0".into()),
            layers: crate::llama::fit_params::MeasureLayers::All,
            experts_on_cpu: ExpertsOnCpu::None,
            kv_in_computer_memory: false,
            save_memory: false,
            flash_attn: FlashAttn::Auto,
            hip_visible_devices: Some("1".into()),
        };
        let environments = [
            ("probe", backend.base_env()),
            ("llama-server", build_env_on(backend.base_env(), &config())),
            ("llama-server alone", build_env(&config())),
            (
                "fit-params",
                crate::llama::fit_params::measure_env(&backend, &measure),
            ),
        ];
        for (name, env) in environments {
            let resolved = env.resolve_from(parent.clone());
            let get = |key: &str| {
                resolved
                    .iter()
                    .filter(|(k, _)| k == key)
                    .map(|(_, v)| v.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            };
            assert_eq!(get("GGML_NO_BACKTRACE"), vec!["1"], "{name}");
            assert!(get("GGML_BACKTRACE_LLDB").is_empty(), "{name}");
        }
    }

    #[test]
    fn optional_args_and_cpu_device() {
        let cfg = LaunchConfig {
            mmproj: Some(PathBuf::from("/models/mmproj.gguf")),
            device: None,
            reasoning_budget: Some(0),
            ..config()
        };
        let args = build_args(&cfg);
        let joined = args.join(" ");
        assert!(joined.contains("-m /models/m.gguf --mmproj /models/mmproj.gguf -c 8192"));
        assert!(joined.contains("--device none -ngl all --fit off"));
        assert!(joined.ends_with("--cache-ram 512 --reasoning-budget 0"));
        let empty_device = LaunchConfig {
            device: Some(String::new()),
            ..config()
        };
        assert!(
            build_args(&empty_device)
                .join(" ")
                .contains("--device none -ngl all")
        );
    }

    /// The alias goes on the command line only as one plain word; anything else (a comma
    /// would make several names, a leading dash looks like a flag) falls back to the fixed
    /// word.
    #[test]
    fn the_alias_is_one_plain_word() {
        let alias_of = |alias: &str| {
            let args = build_args(&LaunchConfig {
                alias: alias.to_owned(),
                ..config()
            });
            let at = args.iter().position(|a| a == "-a").expect("-a");
            args[at + 1].clone()
        };
        assert_eq!(alias_of("harnessui-local"), "harnessui-local");
        assert_eq!(alias_of("other.name_2"), "other.name_2");
        for odd in [
            "",
            "two,names",
            "-flag",
            "with space",
            "Upper",
            "line\nbreak",
            "../path",
            "a/b",
            "\u{202e}rtl",
            &"x".repeat(65),
        ] {
            assert_eq!(alias_of(odd), MODEL_ALIAS, "{odd:?}");
        }
        assert_eq!(
            build_args(&config()).iter().filter(|a| *a == "-a").count(),
            1
        );
    }

    /// The pinned build decodes video, and any image its own decoder cannot read (WebP
    /// included), by starting `ffprobe` and `ffmpeg`, which it looks for on `PATH` unless
    /// `--video-ffmpeg-dir` names a folder. It always names one that does not exist, as a
    /// relative path (resolved in the runtime folder, the working directory), so those
    /// programs never start outside `spawn.rs`.
    #[test]
    fn llama_server_never_looks_for_ffmpeg_on_the_path() {
        for gpu in [GpuUse::All, GpuUse::Fit, GpuUse::Off] {
            let args = build_args(&LaunchConfig {
                gpu,
                mmproj: Some(PathBuf::from("/models/mmproj.gguf")),
                ..config()
            });
            let at = args
                .iter()
                .position(|a| a == "--video-ffmpeg-dir")
                .expect("--video-ffmpeg-dir");
            assert_eq!(args[at + 1], NO_FFMPEG_DIR);
        }
        assert_eq!(NO_FFMPEG_DIR, "no-ffmpeg");
        let dir = std::path::Path::new(NO_FFMPEG_DIR);
        assert!(dir.is_relative());
        assert_eq!(dir.components().count(), 1, "one plain folder name");
    }

    #[test]
    fn the_key_never_appears_in_args() {
        let cfg = LaunchConfig {
            api_key: "very-secret-key".into(),
            ..config()
        };
        assert!(
            build_args(&cfg)
                .iter()
                .all(|arg| !arg.contains("very-secret-key"))
        );
    }

    #[test]
    fn debug_output_redacts_the_key() {
        let cfg = LaunchConfig {
            api_key: "very-secret-key".into(),
            ..config()
        };
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("very-secret-key"), "{shown}");
        assert!(shown.contains("api_key: \"<redacted>\""), "{shown}");
        assert!(shown.contains("port: 40123"), "{shown}");
    }

    #[test]
    fn env_scrubs_llama_ggml_and_hf_variables() {
        let parent: Vec<(OsString, OsString)> = [
            ("PATH", "/usr/bin"),
            ("SYSTEMROOT", "C:\\Windows"),
            ("HOME", "/home/u"),
            ("LLAMA_ARG_TOOLS", "all"),
            ("LLAMA_ARG_CONNECT", "x"),
            ("LLAMA_ARG_HOST", "0.0.0.0"),
            ("GGML_BACKEND_PATH", "/tmp/evil.so"),
            ("GGML_VK_VISIBLE_DEVICES", "1"),
            ("HF_TOKEN", "hf_secret"),
            ("HF_ENDPOINT", "https://mirror"),
            ("LLAMA_API_KEY", "parent-key"),
            ("HIP_VISIBLE_DEVICES", "0"),
        ]
        .into_iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect();
        let cfg = LaunchConfig {
            api_key: "launch-key".into(),
            ..config()
        };
        let env = build_env(&cfg).resolve_from(parent);
        let get = |name: &str| {
            env.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        for banned in [
            "LLAMA_ARG_TOOLS",
            "LLAMA_ARG_CONNECT",
            "LLAMA_ARG_HOST",
            "GGML_BACKEND_PATH",
            "GGML_VK_VISIBLE_DEVICES",
            "HF_TOKEN",
            "HF_ENDPOINT",
            "HIP_VISIBLE_DEVICES",
        ] {
            assert_eq!(get(banned), None, "{banned} leaked");
        }
        assert_eq!(get("LLAMA_API_KEY").as_deref(), Some("launch-key"));
        assert_eq!(get("LLAMA_SERVER_ROUTER_PORT").as_deref(), Some("1"));
        assert_eq!(get("PATH").as_deref(), Some("/usr/bin"));
    }

    #[test]
    fn env_sets_hip_visible_devices_only_when_asked() {
        let cfg = LaunchConfig {
            hip_visible_devices: Some("1".into()),
            ..config()
        };
        let env = build_env(&cfg).resolve_from(Vec::new());
        assert!(
            env.iter()
                .any(|(k, v)| k == "HIP_VISIBLE_DEVICES" && v == "1")
        );
    }

    #[test]
    fn api_keys_are_url_safe_and_distinct() {
        let a = random_api_key();
        let b = random_api_key();
        assert_eq!(a.len(), 32);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_ne!(a, b);
    }

    #[test]
    fn free_port_is_bindable() {
        let port = free_loopback_port().expect("port");
        assert_ne!(port, 0);
        TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("port is free");
    }
}
