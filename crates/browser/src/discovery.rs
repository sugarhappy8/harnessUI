//! Finding the browser to drive (ARCHITECTURE.md 9.3, 15): a user override, installed
//! Chrome or Edge (standard install paths, the Windows registry's App Paths), Chromium,
//! and for tests an environment override and the pre-installed Playwright Chromium.

use std::path::{Path, PathBuf};

/// Environment variable naming a browser executable; read before the standard locations
/// (tests and CI).
pub const ENV_OVERRIDE: &str = "HARNISS_BROWSER_PATH";

/// Which browser was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    /// The path the user chose in Settings.
    UserChoice,
    /// The `HARNISS_BROWSER_PATH` environment variable.
    Environment,
    /// Google Chrome.
    Chrome,
    /// Microsoft Edge.
    Edge,
    /// Chromium (including Playwright's build).
    Chromium,
}

/// A browser executable that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundBrowser {
    /// Absolute path of the executable.
    pub path: PathBuf,
    /// What it is.
    pub kind: BrowserKind,
}

/// No usable browser (section 15: message plus next step).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BrowserUnavailable {
    /// Plain-language message.
    pub message: String,
    /// What the user can do.
    pub next_step: String,
}

impl BrowserUnavailable {
    /// The text a tool returns to the model.
    pub fn for_model(&self) -> String {
        format!(
            "The browser tool is unavailable: {} Tell the user: {}",
            self.message, self.next_step
        )
    }
}

fn not_installed() -> BrowserUnavailable {
    BrowserUnavailable {
        message: "The browser tool needs Chrome or Edge, and neither is installed.".into(),
        next_step: "Install Google Chrome, or choose a browser path in Settings → Browser.".into(),
    }
}

/// What discovery reads from the environment, so tests can supply their own.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryEnv {
    /// `HARNISS_BROWSER_PATH`.
    pub env_override: Option<PathBuf>,
    /// `%ProgramFiles%`, `%ProgramFiles(x86)%`, `%LocalAppData%` (Windows).
    pub program_files: Option<PathBuf>,
    /// `%ProgramFiles(x86)%`.
    pub program_files_x86: Option<PathBuf>,
    /// `%LocalAppData%`.
    pub local_app_data: Option<PathBuf>,
    /// The home folder (macOS `~/Applications`).
    pub home: Option<PathBuf>,
    /// App Paths entries from the Windows registry (chrome.exe, msedge.exe).
    pub registry: Vec<(PathBuf, BrowserKind)>,
    /// `PLAYWRIGHT_BROWSERS_PATH`, else `/opt/pw-browsers` on Linux.
    pub playwright_root: Option<PathBuf>,
    /// Directories of `PATH` (Linux).
    pub path_dirs: Vec<PathBuf>,
}

impl DiscoveryEnv {
    /// Reads the real environment.
    pub fn from_system() -> Self {
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
        Self {
            env_override: var(ENV_OVERRIDE).filter(|p| !p.as_os_str().is_empty()),
            program_files: var("ProgramFiles"),
            program_files_x86: var("ProgramFiles(x86)"),
            local_app_data: var("LOCALAPPDATA"),
            home: var("HOME").or_else(|| var("USERPROFILE")),
            registry: registry_app_paths(),
            playwright_root: var("PLAYWRIGHT_BROWSERS_PATH")
                .or_else(|| cfg!(target_os = "linux").then(|| PathBuf::from("/opt/pw-browsers"))),
            path_dirs: std::env::var_os("PATH")
                .map(|p| {
                    std::env::split_paths(&p)
                        .filter(|d| d.is_absolute())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// The operating system whose install locations to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows.
    Windows,
    /// macOS.
    Mac,
    /// Linux and other Unix.
    Linux,
}

impl Os {
    /// The OS this build runs on.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Linux
        }
    }
}

/// Where to look, in order (Chrome first, then Edge, then Chromium). Pure: no file-system
/// access.
pub fn candidates(os: Os, env: &DiscoveryEnv) -> Vec<(PathBuf, BrowserKind)> {
    let mut out: Vec<(PathBuf, BrowserKind)> = Vec::new();
    match os {
        Os::Windows => {
            out.extend(
                env.registry
                    .iter()
                    .filter(|(_, k)| *k == BrowserKind::Chrome)
                    .cloned(),
            );
            let chrome = Path::new("Google")
                .join("Chrome")
                .join("Application")
                .join("chrome.exe");
            let edge = Path::new("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe");
            for base in [
                &env.program_files,
                &env.program_files_x86,
                &env.local_app_data,
            ]
            .into_iter()
            .flatten()
            {
                out.push((base.join(&chrome), BrowserKind::Chrome));
            }
            out.extend(
                env.registry
                    .iter()
                    .filter(|(_, k)| *k == BrowserKind::Edge)
                    .cloned(),
            );
            for base in [&env.program_files_x86, &env.program_files]
                .into_iter()
                .flatten()
            {
                out.push((base.join(&edge), BrowserKind::Edge));
            }
            if let Some(local) = &env.local_app_data {
                out.push((
                    local
                        .join("Chromium")
                        .join("Application")
                        .join("chrome.exe"),
                    BrowserKind::Chromium,
                ));
            }
        }
        Os::Mac => {
            let apps = |name: &str, exe: &str| {
                Path::new(&format!("{name}.app"))
                    .join("Contents")
                    .join("MacOS")
                    .join(exe)
            };
            let entries = [
                (apps("Google Chrome", "Google Chrome"), BrowserKind::Chrome),
                (apps("Microsoft Edge", "Microsoft Edge"), BrowserKind::Edge),
                (apps("Chromium", "Chromium"), BrowserKind::Chromium),
            ];
            for (rel, kind) in entries {
                out.push((Path::new("/Applications").join(&rel), kind));
                if let Some(home) = &env.home {
                    out.push((home.join("Applications").join(&rel), kind));
                }
            }
        }
        Os::Linux => {
            let names = [
                ("google-chrome-stable", BrowserKind::Chrome),
                ("google-chrome", BrowserKind::Chrome),
                ("microsoft-edge-stable", BrowserKind::Edge),
                ("microsoft-edge", BrowserKind::Edge),
                ("chromium", BrowserKind::Chromium),
                ("chromium-browser", BrowserKind::Chromium),
            ];
            for (name, kind) in names {
                for dir in &env.path_dirs {
                    out.push((dir.join(name), kind));
                }
            }
            out.push((
                PathBuf::from("/opt/google/chrome/chrome"),
                BrowserKind::Chrome,
            ));
            out.push((PathBuf::from("/snap/bin/chromium"), BrowserKind::Chromium));
        }
    }
    out
}

/// The real Chromium executable inside a Playwright browsers folder
/// (`chromium-<build>/chrome-<platform>/chrome`), newest build first; the headless shell
/// is skipped.
pub fn playwright_chromium(root: &Path) -> Option<PathBuf> {
    let mut builds: Vec<(u64, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let build = name.strip_prefix("chromium-")?.parse::<u64>().ok()?;
            Some((build, e.path()))
        })
        .collect();
    builds.sort_by_key(|b| std::cmp::Reverse(b.0));
    for (_, dir) in builds {
        for rel in [
            Path::new("chrome-linux64").join("chrome"),
            Path::new("chrome-linux").join("chrome"),
            Path::new("chrome-win").join("chrome.exe"),
            Path::new("chrome-win64").join("chrome.exe"),
            Path::new("chrome-mac")
                .join("Chromium.app")
                .join("Contents")
                .join("MacOS")
                .join("Chromium"),
            Path::new("chrome-mac-arm64")
                .join("Chromium.app")
                .join("Contents")
                .join("MacOS")
                .join("Chromium"),
        ] {
            let candidate = dir.join(rel);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn usable(path: &Path) -> Option<PathBuf> {
    let canonical = dunce::canonicalize(path).ok()?;
    canonical.is_file().then_some(canonical)
}

/// Finds the browser: the user's choice, `HARNISS_BROWSER_PATH`, then the standard
/// locations for `os`, then Playwright's Chromium.
pub fn find_browser_in(
    user_choice: Option<&Path>,
    os: Os,
    env: &DiscoveryEnv,
) -> Result<FoundBrowser, BrowserUnavailable> {
    if let Some(choice) = user_choice {
        return usable(choice)
            .map(|path| FoundBrowser {
                path,
                kind: BrowserKind::UserChoice,
            })
            .ok_or_else(|| BrowserUnavailable {
                message: format!(
                    "The browser chosen in Settings ({}) was not found.",
                    choice.display()
                ),
                next_step: "Choose another browser path in Settings → Browser, or clear it to use Chrome or Edge.".into(),
            });
    }
    if let Some(path) = env.env_override.as_deref().and_then(usable) {
        return Ok(FoundBrowser {
            path,
            kind: BrowserKind::Environment,
        });
    }
    for (path, kind) in candidates(os, env) {
        if let Some(path) = usable(&path) {
            return Ok(FoundBrowser { path, kind });
        }
    }
    if let Some(path) = env
        .playwright_root
        .as_deref()
        .and_then(playwright_chromium)
        .and_then(|p| usable(&p))
    {
        return Ok(FoundBrowser {
            path,
            kind: BrowserKind::Chromium,
        });
    }
    Err(not_installed())
}

/// [`find_browser_in`] with the real environment.
pub fn find_browser(user_choice: Option<&Path>) -> Result<FoundBrowser, BrowserUnavailable> {
    find_browser_in(user_choice, Os::current(), &DiscoveryEnv::from_system())
}

/// `chrome.exe` and `msedge.exe` from `HKCU` and `HKLM`
/// `SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths`.
#[cfg(windows)]
fn registry_app_paths() -> Vec<(PathBuf, BrowserKind)> {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_default(root: HKEY, subkey: &str) -> Option<PathBuf> {
        let key = wide(subkey);
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `key` is NUL-terminated and outlives the call; `buf` has `size` bytes
        // of space and RegGetValueW writes at most that many, NUL-terminated (RRF_RT_REG_SZ).
        let status = unsafe {
            RegGetValueW(
                root,
                PCWSTR(key.as_ptr()),
                PCWSTR::null(),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status.is_err() {
            return None;
        }
        let chars = (size as usize / 2).saturating_sub(1).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..chars]);
        let s = s.trim().trim_matches('"').to_string();
        (!s.is_empty()).then(|| PathBuf::from(s))
    }

    let base = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\";
    let mut out = Vec::new();
    for (exe, kind) in [
        ("chrome.exe", BrowserKind::Chrome),
        ("msedge.exe", BrowserKind::Edge),
    ] {
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            if let Some(p) = read_default(root, &format!("{base}{exe}")) {
                out.push((p, kind));
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn registry_app_paths() -> Vec<(PathBuf, BrowserKind)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_order_is_registry_then_install_paths_chrome_before_edge() {
        let env = DiscoveryEnv {
            program_files: Some(PathBuf::from(r"C:\PF")),
            program_files_x86: Some(PathBuf::from(r"C:\PF86")),
            local_app_data: Some(PathBuf::from(r"C:\L")),
            registry: vec![
                (PathBuf::from(r"C:\R\msedge.exe"), BrowserKind::Edge),
                (PathBuf::from(r"C:\R\chrome.exe"), BrowserKind::Chrome),
            ],
            ..Default::default()
        };
        let c = candidates(Os::Windows, &env);
        let kinds: Vec<BrowserKind> = c.iter().map(|(_, k)| *k).collect();
        assert_eq!(c[0].0, PathBuf::from(r"C:\R\chrome.exe"));
        let first_edge = kinds.iter().position(|k| *k == BrowserKind::Edge).unwrap();
        let last_chrome = kinds
            .iter()
            .rposition(|k| *k == BrowserKind::Chrome)
            .unwrap();
        assert!(last_chrome < first_edge);
        assert!(
            c.iter()
                .any(|(p, _)| p.ends_with("msedge.exe") && p.starts_with(r"C:\PF86"))
        );
        assert_eq!(c.last().unwrap().1, BrowserKind::Chromium);
    }

    #[test]
    fn mac_looks_in_both_application_folders() {
        let env = DiscoveryEnv {
            home: Some(PathBuf::from("/Users/u")),
            ..Default::default()
        };
        let c = candidates(Os::Mac, &env);
        assert_eq!(
            c[0].0,
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        );
        assert!(c.iter().any(|(p, k)| *k == BrowserKind::Edge
            && p == Path::new(
                "/Users/u/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"
            )));
    }

    #[test]
    fn linux_searches_path_directories() {
        let env = DiscoveryEnv {
            path_dirs: vec![PathBuf::from("/usr/bin")],
            ..Default::default()
        };
        let c = candidates(Os::Linux, &env);
        assert_eq!(
            c[0],
            (
                PathBuf::from("/usr/bin/google-chrome-stable"),
                BrowserKind::Chrome
            )
        );
    }

    #[test]
    fn playwright_layout_newest_build_and_never_the_headless_shell() {
        let root = tempfile::tempdir().unwrap();
        let mk = |rel: &str| {
            let p = root.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
            p
        };
        mk("chromium_headless_shell-2000/chrome-headless-shell-linux64/chrome-headless-shell");
        mk("chromium-1000/chrome-linux64/chrome");
        let newest = mk("chromium-1200/chrome-linux64/chrome");
        assert_eq!(playwright_chromium(root.path()), Some(newest));
        assert_eq!(playwright_chromium(&root.path().join("missing")), None);
    }

    #[test]
    fn user_choice_wins_and_a_missing_choice_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("my-browser");
        std::fs::write(&exe, "x").unwrap();
        let env = DiscoveryEnv::default();
        let found = find_browser_in(Some(&exe), Os::Linux, &env).unwrap();
        assert_eq!(found.kind, BrowserKind::UserChoice);
        let err = find_browser_in(Some(&dir.path().join("nope")), Os::Linux, &env).unwrap_err();
        assert!(err.message.contains("was not found"), "{err}");
        assert!(err.next_step.contains("Settings"));
    }

    #[test]
    fn env_override_then_nothing_found() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("chrome");
        std::fs::write(&exe, "x").unwrap();
        let env = DiscoveryEnv {
            env_override: Some(exe.clone()),
            ..Default::default()
        };
        let found = find_browser_in(None, Os::Linux, &env).unwrap();
        assert_eq!(found.kind, BrowserKind::Environment);
        let none = DiscoveryEnv {
            path_dirs: vec![dir.path().join("empty")],
            ..Default::default()
        };
        // Windows locations all come from the environment, so nothing on this machine matches.
        let err = find_browser_in(None, Os::Windows, &none).unwrap_err();
        assert_eq!(
            err.message,
            "The browser tool needs Chrome or Edge, and neither is installed."
        );
        assert!(err.for_model().contains("Install Google Chrome"));
    }
}
