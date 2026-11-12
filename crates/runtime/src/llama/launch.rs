//! The llama-server command line and environment (ARCHITECTURE.md 7.3).
//!
//! Every default the pinned build would otherwise apply is overridden explicitly: port
//! 9931, auto-fitted context, four slots, the 8 GB prompt cache, the Web UI, CORS for any
//! origin and downloads from the network.

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

/// How much of the model goes on the graphics card ("Use the graphics card", 0.0.5 D4).
///
/// WP1a turns it into arguments: `All` is `-ngl all`, `Fit` leaves `-ngl` out and passes
/// `--fit on`, `Off` is `-ngl 0`. Until then [`build_args`] reads [`LaunchConfig::gpu_layers`]
/// only, and callers set both fields to the same choice.
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

/// How many layers to offload to the GPU (`-ngl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "layers", rename_all = "snake_case")]
pub enum GpuLayers {
    /// Let llama.cpp fit as many layers as free memory allows.
    Auto,
    /// Every layer.
    All,
    /// Exactly this many layers.
    Count(u32),
}

impl GpuLayers {
    /// The `-ngl` argument value.
    pub fn as_arg(self) -> String {
        match self {
            Self::Auto => "auto".to_owned(),
            Self::All => "all".to_owned(),
            Self::Count(n) => n.to_string(),
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
    /// GPU layers (`-ngl`).
    pub gpu_layers: GpuLayers,
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
    /// How much of the model goes on the graphics card. Not an argument yet (WP1a):
    /// [`gpu_layers`](Self::gpu_layers) still decides `-ngl`.
    #[serde(default)]
    pub gpu: GpuUse,
    /// Save memory: flash attention and a q8_0 conversation memory (`-fa on -ctk q8_0
    /// -ctv q8_0`). Not an argument yet (WP1a).
    #[serde(default)]
    pub save_memory: bool,
    /// Keep the model in memory (`-lm mmap+mlock`). Not an argument yet (WP1a).
    #[serde(default)]
    pub keep_in_memory: bool,
    /// The model id the API reports (`-a`); always [`MODEL_ALIAS`]. Not an argument yet
    /// (WP1a).
    #[serde(default = "default_alias")]
    pub alias: String,
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
            gpu_layers: GpuLayers::All,
            cache_ram_mib: 512,
            reasoning_budget: None,
            port: 0,
            api_key: String::new(),
            hip_visible_devices: None,
            gpu: GpuUse::All,
            save_memory: false,
            keep_in_memory: false,
            alias: default_alias(),
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
            .field("gpu_layers", &self.gpu_layers)
            .field("cache_ram_mib", &self.cache_ram_mib)
            .field("reasoning_budget", &self.reasoning_budget)
            .field("port", &self.port)
            .field("api_key", &REDACTED)
            .field("hip_visible_devices", &self.hip_visible_devices)
            .field("gpu", &self.gpu)
            .field("save_memory", &self.save_memory)
            .field("keep_in_memory", &self.keep_in_memory)
            .field("alias", &self.alias)
            .finish()
    }
}

/// The exact argument list for `cfg`:
///
/// ```text
/// --host 127.0.0.1 --port P --no-webui --no-slots --offline --jinja --cors-origins none
/// --no-context-shift -m M [--mmproj X] -c C -np 1 --device D -ngl auto|all|N
/// --cache-ram R [--reasoning-budget B]
/// ```
///
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
    let device = cfg
        .device
        .as_deref()
        .filter(|d| !d.is_empty())
        .unwrap_or("none");
    args.extend([
        "-c".to_owned(),
        cfg.ctx.to_string(),
        "-np".to_owned(),
        "1".to_owned(),
        "--device".to_owned(),
        device.to_owned(),
        "-ngl".to_owned(),
        cfg.gpu_layers.as_arg(),
        "--cache-ram".to_owned(),
        cfg.cache_ram_mib.to_string(),
    ]);
    if let Some(budget) = cfg.reasoning_budget {
        args.push("--reasoning-budget".to_owned());
        args.push(budget.to_string());
    }
    args
}

/// The environment for `cfg`: the platform allowlist, the API key, managed-child mode and,
/// for ROCm, `HIP_VISIBLE_DEVICES`. Nothing else from the app's environment is passed, so
/// `LLAMA_ARG_*`, `GGML_*`, `HF_TOKEN` and `HF_ENDPOINT` never reach the server.
pub fn build_env(cfg: &LaunchConfig) -> EnvSpec {
    build_env_on(EnvSpec::minimal(), cfg)
}

/// [`build_env`] on top of a backend's own base environment
/// ([`Backend::base_env`](crate::Backend::base_env)).
pub fn build_env_on(base: EnvSpec, cfg: &LaunchConfig) -> EnvSpec {
    let mut env = base
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
            gpu_layers: GpuLayers::Auto,
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
        assert_eq!(d.gpu_layers, GpuLayers::All);
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

    /// WP0 adds the 0.0.5 fields without turning them into arguments: whatever they hold,
    /// the command line is the one of 0.0.4 (WP1a changes this test when it adds them).
    #[test]
    fn new_fields_are_not_arguments_yet() {
        let changed = LaunchConfig {
            gpu: GpuUse::Fit,
            save_memory: true,
            keep_in_memory: true,
            alias: "other".into(),
            ..config()
        };
        assert_eq!(build_args(&changed), build_args(&config()));
    }

    /// A config saved before 0.0.5 still reads, with the new fields at their defaults.
    #[test]
    fn old_json_reads_with_defaults() {
        let mut v = serde_json::to_value(config()).expect("json");
        let obj = v.as_object_mut().expect("object");
        for key in ["gpu", "save_memory", "keep_in_memory", "alias"] {
            assert!(obj.remove(key).is_some(), "{key}");
        }
        let back: LaunchConfig = serde_json::from_value(v).expect("reads");
        assert_eq!(back, config());
        assert_eq!(
            serde_json::to_value(GpuUse::Fit).expect("json"),
            serde_json::json!("fit")
        );
    }

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
                "-m",
                "/models/m.gguf",
                "-c",
                "8192",
                "-np",
                "1",
                "--device",
                "Vulkan0",
                "-ngl",
                "auto",
                "--cache-ram",
                "512",
            ]
        );
    }

    #[test]
    fn optional_args_and_cpu_device() {
        let cfg = LaunchConfig {
            mmproj: Some(PathBuf::from("/models/mmproj.gguf")),
            device: None,
            gpu_layers: GpuLayers::Count(12),
            reasoning_budget: Some(0),
            ..config()
        };
        let args = build_args(&cfg);
        let joined = args.join(" ");
        assert!(joined.contains("-m /models/m.gguf --mmproj /models/mmproj.gguf -c 8192"));
        assert!(joined.contains("--device none -ngl 12"));
        assert!(joined.ends_with("--cache-ram 512 --reasoning-budget 0"));
        let empty_device = LaunchConfig {
            device: Some(String::new()),
            gpu_layers: GpuLayers::All,
            ..config()
        };
        assert!(
            build_args(&empty_device)
                .join(" ")
                .contains("--device none -ngl all")
        );
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

    #[test]
    fn gpu_layers_serialise() {
        assert_eq!(
            serde_json::to_string(&GpuLayers::Count(3)).expect("json"),
            r#"{"mode":"count","layers":3}"#
        );
        assert_eq!(
            serde_json::to_string(&GpuLayers::Auto).expect("json"),
            r#"{"mode":"auto"}"#
        );
    }
}
