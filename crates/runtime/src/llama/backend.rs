//! Backends and where they live on disk (ARCHITECTURE.md 7.1, 7.2 and 12).
//!
//! A **runtime directory** holds a complete `llama-server` and its libraries. A **backend**
//! names a runtime directory and the device prefix to ask it for. CPU and Vulkan share the
//! installed directory on Windows and are told apart with `--device`; each downloaded pack
//! is a directory of its own.
//!
//! This module is pure path logic: it looks at files, it never starts a process.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::lock::{self, LlamaLock};

/// File that marks a downloaded pack as completely unpacked and verified.
pub const PACK_COMPLETE_MARKER: &str = ".complete";

/// Environment variable naming a development runtime directory (Linux dev builds).
pub const DEV_OVERRIDE_ENV: &str = "APP_DEV_LLAMA_DIR";

/// A kind of llama.cpp backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// No GPU offload (`--device none`).
    Cpu,
    /// Vulkan: every GPU vendor on Windows. The default (D3).
    Vulkan,
    /// NVIDIA CUDA (downloaded pack).
    Cuda,
    /// AMD ROCm / HIP (downloaded pack).
    Rocm,
    /// Apple Metal (macOS).
    Metal,
}

impl BackendKind {
    /// The prefix of this backend's device names in `--list-devices` output (`Vulkan0`,
    /// `CUDA0`, `ROCm0`, `MTL0`). `None` for the CPU, which has no device row.
    pub fn device_prefix(self) -> Option<&'static str> {
        match self {
            Self::Cpu => None,
            Self::Vulkan => Some("Vulkan"),
            Self::Cuda => Some("CUDA"),
            Self::Rocm => Some("ROCm"),
            Self::Metal => Some("MTL"),
        }
    }

    /// Lower-case name, as in the lock file and settings.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Vulkan => "vulkan",
            Self::Cuda => "cuda",
            Self::Rocm => "rocm",
            Self::Metal => "metal",
        }
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cpu => "CPU",
            Self::Vulkan => "Vulkan",
            Self::Cuda => "CUDA",
            Self::Rocm => "ROCm",
            Self::Metal => "Metal",
        })
    }
}

/// Where a backend's runtime directory came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackendSource {
    /// Shipped with the app.
    Installed,
    /// Downloaded in Settings; `id` is the pack's id in the lock file.
    Pack {
        /// Pack id (`cuda`, `rocm`).
        id: String,
    },
    /// A development directory named by [`DEV_OVERRIDE_ENV`].
    DevOverride,
}

/// One usable-looking backend: a runtime directory plus the kind of device to ask it for.
/// Whether it actually works is decided by [`super::devices::probe`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backend {
    /// Which backend this is.
    pub kind: BackendKind,
    /// The runtime directory; always the child's working directory.
    pub dir: PathBuf,
    /// The `llama-server` executable inside [`dir`](Self::dir).
    pub server: PathBuf,
    /// The `llama-fit-params` executable, when present.
    pub fit_params: Option<PathBuf>,
    /// The llama.cpp build in the directory (`b12110`, `lemonade-b1372`, `dev`).
    pub build: String,
    /// Where the directory came from.
    pub source: BackendSource,
}

/// Where to look for runtime directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeLayout {
    /// The installed runtime directory: `resources/llama/main` on Windows; the directory
    /// holding the `externalBin` sidecar on macOS.
    pub installed_dir: Option<PathBuf>,
    /// The folder downloaded packs are unpacked into: `<local data>/backends`.
    pub packs_dir: PathBuf,
    /// A development runtime directory, from [`DEV_OVERRIDE_ENV`] on Linux dev machines.
    pub dev_override: Option<PathBuf>,
}

impl RuntimeLayout {
    /// A layout with `dev_override` taken from [`DEV_OVERRIDE_ENV`] when it is set and not
    /// empty.
    pub fn with_env_override(installed_dir: Option<PathBuf>, packs_dir: PathBuf) -> Self {
        let dev_override = std::env::var_os(DEV_OVERRIDE_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        Self {
            installed_dir,
            packs_dir,
            dev_override,
        }
    }
}

/// File name of the server executable on this platform. On macOS Tauri strips the target
/// triple from `externalBin` names when bundling, so it is plain `llama-server` there too.
pub fn server_file_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// File name of the fit-params executable on this platform.
pub fn fit_params_file_name() -> &'static str {
    if cfg!(windows) {
        "llama-fit-params.exe"
    } else {
        "llama-fit-params"
    }
}

/// Lists the backends whose files are present, using the lock file compiled into the app.
///
/// Order: installed backends, then complete packs for this platform, then the development
/// override.
pub fn discover(layout: &RuntimeLayout) -> Vec<Backend> {
    discover_with(layout, lock::embedded(), lock::current_platform())
}

/// [`discover`] against an explicit lock file and platform key (for tests).
pub fn discover_with(layout: &RuntimeLayout, lock: &LlamaLock, platform: &str) -> Vec<Backend> {
    let mut found = Vec::new();

    if let Some(dir) = &layout.installed_dir
        && let Some(server) = existing_file(dir, server_file_name())
    {
        let kinds = match lock.installer.get(platform) {
            Some(entry) if !entry.backends.is_empty() => entry.backends.clone(),
            // A platform the lock has no installer for (Linux dev): CPU only.
            _ => vec![BackendKind::Cpu],
        };
        for kind in kinds {
            found.push(Backend {
                kind,
                dir: dir.clone(),
                server: server.clone(),
                fit_params: existing_file(dir, fit_params_file_name()),
                build: lock.llama_cpp.tag.clone(),
                source: BackendSource::Installed,
            });
        }
    }

    for pack in lock.packs.iter().filter(|pack| pack.platform == platform) {
        if !is_plain_dir_name(&pack.dir) {
            tracing::warn!(pack = %pack.id, dir = %pack.dir, "ignoring a pack whose directory is not a plain name");
            continue;
        }
        let dir = layout.packs_dir.join(&pack.dir);
        if !dir.join(PACK_COMPLETE_MARKER).is_file() {
            continue;
        }
        let Some(server) = existing_file(&dir, server_file_name()) else {
            continue;
        };
        found.push(Backend {
            kind: pack.backend,
            fit_params: existing_file(&dir, fit_params_file_name()),
            dir,
            server,
            build: pack.build.clone(),
            source: BackendSource::Pack {
                id: pack.id.clone(),
            },
        });
    }

    if let Some(dir) = &layout.dev_override
        && let Some(server) = existing_file(dir, server_file_name())
    {
        for kind in dev_kinds(dir) {
            found.push(Backend {
                kind,
                dir: dir.clone(),
                server: server.clone(),
                fit_params: existing_file(dir, fit_params_file_name()),
                build: "dev".to_owned(),
                source: BackendSource::DevOverride,
            });
        }
    }

    found
}

/// Backends a development directory can serve: always CPU, plus each GPU backend whose
/// library sits in the directory (upstream builds load backends as `ggml-<name>` libraries),
/// plus Metal on macOS, where it is linked in.
fn dev_kinds(dir: &Path) -> Vec<BackendKind> {
    let mut kinds = vec![BackendKind::Cpu];
    let names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let has_lib = |backend: &str| {
        names
            .iter()
            .any(|name| name.contains(&format!("ggml-{backend}")))
    };
    if has_lib("vulkan") {
        kinds.push(BackendKind::Vulkan);
    }
    if has_lib("cuda") {
        kinds.push(BackendKind::Cuda);
    }
    if has_lib("hip") {
        kinds.push(BackendKind::Rocm);
    }
    if cfg!(target_os = "macos") || has_lib("metal") {
        kinds.push(BackendKind::Metal);
    }
    kinds
}

/// Whether `name` is one ordinary path component, so joining it onto the packs folder
/// cannot leave that folder (`..`, an absolute path, a drive prefix or a separator).
pub(crate) fn is_plain_dir_name(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(_)), None)
    ) && !name.contains(['/', '\\', ':'])
}

fn existing_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"{
      "llama_cpp": {"tag": "b1", "version": "0.1.0", "commit": "c", "repository": "r"},
      "installer": {
        "windows-x86_64": {"dir": "main", "backends": ["cpu", "vulkan"]},
        "macos-aarch64": {"dir": "main", "backends": ["metal"]}
      },
      "packs": [
        {"id": "cuda", "backend": "cuda", "platform": "windows-x86_64", "build": "b1", "dir": "cuda-b1",
         "title": "t", "requirements": "r", "license_name": "l", "license_url": "u", "files": []},
        {"id": "rocm", "backend": "rocm", "platform": "windows-x86_64", "build": "x9", "dir": "rocm-x9",
         "title": "t", "requirements": "r", "license_name": "l", "license_url": "u", "files": []}
      ]
    }"#;

    fn lock() -> LlamaLock {
        LlamaLock::parse(LOCK).expect("test lock")
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, b"").expect("touch");
    }

    #[test]
    fn device_prefixes() {
        assert_eq!(BackendKind::Cpu.device_prefix(), None);
        assert_eq!(BackendKind::Vulkan.device_prefix(), Some("Vulkan"));
        assert_eq!(BackendKind::Cuda.device_prefix(), Some("CUDA"));
        assert_eq!(BackendKind::Rocm.device_prefix(), Some("ROCm"));
        assert_eq!(BackendKind::Metal.device_prefix(), Some("MTL"));
    }

    #[test]
    fn kinds_serialise_lowercase() {
        assert_eq!(
            serde_json::to_string(&BackendKind::Rocm).expect("json"),
            "\"rocm\""
        );
        assert_eq!(
            serde_json::from_str::<BackendKind>("\"vulkan\"").expect("json"),
            BackendKind::Vulkan
        );
        assert_eq!(BackendKind::Cuda.as_str(), "cuda");
        assert_eq!(BackendKind::Metal.to_string(), "Metal");
    }

    #[test]
    fn windows_installed_dir_yields_cpu_and_vulkan_from_the_same_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let main = tmp.path().join("resources/llama/main");
        touch(&main.join(server_file_name()));
        touch(&main.join(fit_params_file_name()));
        let layout = RuntimeLayout {
            installed_dir: Some(main.clone()),
            packs_dir: tmp.path().join("backends"),
            dev_override: None,
        };
        let found = discover_with(&layout, &lock(), "windows-x86_64");
        let kinds: Vec<_> = found.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, vec![BackendKind::Cpu, BackendKind::Vulkan]);
        for backend in &found {
            assert_eq!(backend.dir, main);
            assert_eq!(backend.server, main.join(server_file_name()));
            assert_eq!(backend.fit_params, Some(main.join(fit_params_file_name())));
            assert_eq!(backend.build, "b1");
            assert_eq!(backend.source, BackendSource::Installed);
        }
    }

    #[test]
    fn macos_installed_dir_yields_metal() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join(server_file_name()));
        let layout = RuntimeLayout {
            installed_dir: Some(tmp.path().to_path_buf()),
            packs_dir: tmp.path().join("backends"),
            dev_override: None,
        };
        let found = discover_with(&layout, &lock(), "macos-aarch64");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, BackendKind::Metal);
        assert_eq!(found[0].fit_params, None);
    }

    #[test]
    fn missing_server_yields_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let layout = RuntimeLayout {
            installed_dir: Some(tmp.path().to_path_buf()),
            packs_dir: tmp.path().join("backends"),
            dev_override: Some(tmp.path().join("dev")),
        };
        assert!(discover_with(&layout, &lock(), "windows-x86_64").is_empty());
    }

    #[test]
    fn packs_need_the_complete_marker_a_server_and_a_matching_dir_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let packs = tmp.path().join("backends");
        // Complete CUDA pack.
        touch(&packs.join("cuda-b1").join(server_file_name()));
        touch(&packs.join("cuda-b1").join(PACK_COMPLETE_MARKER));
        // ROCm pack still unpacking: no marker.
        touch(&packs.join("rocm-x9").join(server_file_name()));
        // A leftover pack from an older build: not in the lock.
        touch(&packs.join("cuda-b0").join(server_file_name()));
        touch(&packs.join("cuda-b0").join(PACK_COMPLETE_MARKER));
        let layout = RuntimeLayout {
            installed_dir: None,
            packs_dir: packs.clone(),
            dev_override: None,
        };

        let found = discover_with(&layout, &lock(), "windows-x86_64");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, BackendKind::Cuda);
        assert_eq!(found[0].dir, packs.join("cuda-b1"));
        assert_eq!(found[0].source, BackendSource::Pack { id: "cuda".into() });

        // Packs for another platform are ignored.
        assert!(discover_with(&layout, &lock(), "macos-aarch64").is_empty());

        // Marker present but no server: ignored.
        touch(&packs.join("rocm-x9").join(PACK_COMPLETE_MARKER));
        std::fs::remove_file(packs.join("rocm-x9").join(server_file_name())).expect("rm");
        assert_eq!(discover_with(&layout, &lock(), "windows-x86_64").len(), 1);
    }

    #[test]
    fn plain_dir_names() {
        for good in ["cuda-b12110", "rocm-b1372", "main"] {
            assert!(is_plain_dir_name(good), "{good}");
        }
        for bad in [
            "", ".", "..", "../x", "a/b", "a\\b", "/abs", "C:\\x", "C:x", "x/..",
        ] {
            assert!(!is_plain_dir_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_pack_dir_that_escapes_the_packs_folder_is_ignored() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let packs = tmp.path().join("backends");
        // A complete runtime outside the packs folder, named by a hostile dir entry.
        let outside = tmp.path().join("outside");
        touch(&outside.join(server_file_name()));
        touch(&outside.join(PACK_COMPLETE_MARKER));
        let mut lock = lock();
        lock.packs[0].dir = "../outside".into();
        let layout = RuntimeLayout {
            installed_dir: None,
            packs_dir: packs,
            dev_override: None,
        };
        assert!(discover_with(&layout, &lock, "windows-x86_64").is_empty());
    }

    #[test]
    fn dev_override_yields_cpu_plus_present_gpu_libraries() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dev = tmp.path().join("dev");
        touch(&dev.join(server_file_name()));
        touch(&dev.join("libggml-vulkan.so"));
        let layout = RuntimeLayout {
            installed_dir: None,
            packs_dir: tmp.path().join("backends"),
            dev_override: Some(dev.clone()),
        };
        let found = discover_with(&layout, &lock(), "linux-x86_64");
        let kinds: Vec<_> = found.iter().map(|b| b.kind).collect();
        assert_eq!(kinds[0], BackendKind::Cpu);
        assert!(kinds.contains(&BackendKind::Vulkan));
        assert!(
            found.iter().all(|b| b.source == BackendSource::DevOverride
                && b.build == "dev"
                && b.dir == dev)
        );
    }

    #[test]
    fn unknown_platform_installed_dir_is_cpu_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join(server_file_name()));
        let layout = RuntimeLayout {
            installed_dir: Some(tmp.path().to_path_buf()),
            packs_dir: tmp.path().join("backends"),
            dev_override: None,
        };
        let kinds: Vec<_> = discover_with(&layout, &lock(), "linux-x86_64")
            .iter()
            .map(|b| b.kind)
            .collect();
        assert_eq!(kinds, vec![BackendKind::Cpu]);
    }
}
