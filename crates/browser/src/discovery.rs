//! Finding the browser to drive (ARCHITECTURE.md 9.3, 15): a user override, an installed
//! Chrome, Edge, Brave, Vivaldi or Chromium (standard install paths, the Windows
//! registry's App Paths), then the agent's own downloaded browser ([`crate::own`]), and
//! for tests an environment override and the pre-installed Playwright Chromium.

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
    /// Brave.
    Brave,
    /// Vivaldi.
    Vivaldi,
    /// Chromium (including Playwright's build).
    Chromium,
    /// The browser the app downloaded for the agent ([`crate::own`]).
    Own,
}

impl BrowserKind {
    /// The name shown in Settings.
    pub fn label(self) -> &'static str {
        match self {
            Self::UserChoice => "The browser you chose",
            Self::Environment => "The browser named by HARNISS_BROWSER_PATH",
            Self::Chrome => "Google Chrome",
            Self::Edge => "Microsoft Edge",
            Self::Brave => "Brave",
            Self::Vivaldi => "Vivaldi",
            Self::Chromium => "Chromium",
            Self::Own => "The agent's own browser (Chrome for Testing)",
        }
    }
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

fn not_installed(os: Os) -> BrowserUnavailable {
    let next_step = if crate::own::offered_on(os) {
        "Install Google Chrome or Microsoft Edge, or download a browser for the agent in Settings → Browser."
    } else {
        "Install Google Chrome or Microsoft Edge, or choose a browser path in Settings → Browser."
    };
    BrowserUnavailable {
        message:
            "The browser tool needs Chrome, Edge, Brave, Vivaldi or Chromium, and none was found."
                .into(),
        next_step: next_step.into(),
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
    /// App Paths entries from the Windows registry (chrome.exe, msedge.exe, brave.exe,
    /// vivaldi.exe).
    pub registry: Vec<(PathBuf, BrowserKind)>,
    /// The executable of the browser the app downloaded for the agent, when installed.
    pub own_browser: Option<PathBuf>,
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
            own_browser: None,
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

/// Where to look, in order: Chrome, Edge, Brave, Vivaldi, then Chromium. Pure: no
/// file-system access.
pub fn candidates(os: Os, env: &DiscoveryEnv) -> Vec<(PathBuf, BrowserKind)> {
    let mut out: Vec<(PathBuf, BrowserKind)> = Vec::new();
    match os {
        Os::Windows => {
            let pf = env.program_files.as_ref();
            let pf86 = env.program_files_x86.as_ref();
            let local = env.local_app_data.as_ref();
            // Per browser: where it installs (`\`-separated, under a base folder) and
            // the bases in the order its installer prefers them.
            let installs: [(BrowserKind, &str, [Option<&PathBuf>; 3]); 5] = [
                (
                    BrowserKind::Chrome,
                    r"Google\Chrome\Application\chrome.exe",
                    [pf, pf86, local],
                ),
                (
                    BrowserKind::Edge,
                    r"Microsoft\Edge\Application\msedge.exe",
                    [pf86, pf, None],
                ),
                (
                    BrowserKind::Brave,
                    r"BraveSoftware\Brave-Browser\Application\brave.exe",
                    [pf, pf86, local],
                ),
                (
                    BrowserKind::Vivaldi,
                    r"Vivaldi\Application\vivaldi.exe",
                    [local, pf, pf86],
                ),
                (
                    BrowserKind::Chromium,
                    r"Chromium\Application\chrome.exe",
                    [local, None, None],
                ),
            ];
            for (kind, install, bases) in installs {
                out.extend(env.registry.iter().filter(|(_, k)| *k == kind).cloned());
                for base in bases.into_iter().flatten() {
                    let mut path = base.clone();
                    path.extend(install.split('\\'));
                    out.push((path, kind));
                }
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
                (apps("Brave Browser", "Brave Browser"), BrowserKind::Brave),
                (apps("Vivaldi", "Vivaldi"), BrowserKind::Vivaldi),
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
            // Per browser: names searched in `PATH`, then fixed install paths.
            let installs: [(BrowserKind, &[&str], &[&str]); 5] = [
                (
                    BrowserKind::Chrome,
                    &["google-chrome-stable", "google-chrome"],
                    &["/opt/google/chrome/chrome"],
                ),
                (
                    BrowserKind::Edge,
                    &["microsoft-edge-stable", "microsoft-edge"],
                    &["/opt/microsoft/msedge/msedge"],
                ),
                (
                    BrowserKind::Brave,
                    &["brave-browser-stable", "brave-browser", "brave"],
                    &["/opt/brave.com/brave/brave", "/snap/bin/brave"],
                ),
                (
                    BrowserKind::Vivaldi,
                    &["vivaldi-stable", "vivaldi"],
                    &["/opt/vivaldi/vivaldi"],
                ),
                (
                    BrowserKind::Chromium,
                    &["chromium", "chromium-browser"],
                    &["/snap/bin/chromium"],
                ),
            ];
            for (kind, names, fixed) in installs {
                for name in names {
                    for dir in &env.path_dirs {
                        out.push((dir.join(name), kind));
                    }
                }
                out.extend(fixed.iter().map(|p| (PathBuf::from(p), kind)));
            }
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

/// Finds the browser: the user's choice, `HARNISS_BROWSER_PATH` (tests and CI), the
/// standard locations for `os` (Chrome, Edge, Brave, Vivaldi, Chromium), the agent's own
/// downloaded browser, then Playwright's Chromium (tests).
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
                next_step: "Choose another browser path in Settings → Browser, or clear it to use an installed browser.".into(),
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
    if let Some(path) = env.own_browser.as_deref().and_then(usable) {
        return Ok(FoundBrowser {
            path,
            kind: BrowserKind::Own,
        });
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
    Err(not_installed(os))
}

/// [`find_browser_in`] with the real environment. `own_root` is the folder the agent's
/// own browser is downloaded into ([`crate::own`]), when the app has one.
pub fn find_browser(
    user_choice: Option<&Path>,
    own_root: Option<&Path>,
) -> Result<FoundBrowser, BrowserUnavailable> {
    let mut env = DiscoveryEnv::from_system();
    // Read only when it can matter: a chosen path wins or fails on its own.
    if user_choice.is_none() {
        env.own_browser = own_root.and_then(crate::own::installed).map(|b| b.exe);
    }
    find_browser_in(user_choice, Os::current(), &env)
}

/// `chrome.exe`, `msedge.exe`, `brave.exe` and `vivaldi.exe` from `HKCU` and `HKLM`
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
        ("brave.exe", BrowserKind::Brave),
        ("vivaldi.exe", BrowserKind::Vivaldi),
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
            "The browser tool needs Chrome, Edge, Brave, Vivaldi or Chromium, and none was found."
        );
        assert!(err.for_model().contains("Install Google Chrome"));
        assert!(err.next_step.contains("choose a browser path"));
        assert!(
            !err.next_step.contains("download"),
            "no download is offered on Windows"
        );
    }

    /// The order of the kinds in a candidate list, each kind once.
    fn kind_order(c: &[(PathBuf, BrowserKind)]) -> Vec<BrowserKind> {
        let mut out: Vec<BrowserKind> = Vec::new();
        for (_, k) in c {
            if out.last() != Some(k) {
                assert!(!out.contains(k), "{k:?} appears in two places: {c:?}");
                out.push(*k);
            }
        }
        out
    }

    const ORDER: [BrowserKind; 5] = [
        BrowserKind::Chrome,
        BrowserKind::Edge,
        BrowserKind::Brave,
        BrowserKind::Vivaldi,
        BrowserKind::Chromium,
    ];

    #[test]
    fn every_os_looks_for_chrome_edge_brave_vivaldi_then_chromium() {
        let env = DiscoveryEnv {
            program_files: Some(PathBuf::from("PF")),
            program_files_x86: Some(PathBuf::from("PF86")),
            local_app_data: Some(PathBuf::from("L")),
            home: Some(PathBuf::from("/Users/u")),
            path_dirs: vec![PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin")],
            registry: vec![
                (PathBuf::from("R/vivaldi.exe"), BrowserKind::Vivaldi),
                (PathBuf::from("R/brave.exe"), BrowserKind::Brave),
                (PathBuf::from("R/msedge.exe"), BrowserKind::Edge),
                (PathBuf::from("R/chrome.exe"), BrowserKind::Chrome),
            ],
            ..Default::default()
        };
        for os in [Os::Windows, Os::Mac, Os::Linux] {
            assert_eq!(kind_order(&candidates(os, &env)), ORDER, "{os:?}");
        }
    }

    #[test]
    fn brave_and_vivaldi_standard_locations() {
        let env = DiscoveryEnv {
            program_files: Some(PathBuf::from("PF")),
            program_files_x86: Some(PathBuf::from("PF86")),
            local_app_data: Some(PathBuf::from("L")),
            home: Some(PathBuf::from("/Users/u")),
            path_dirs: vec![PathBuf::from("/usr/bin")],
            registry: vec![
                (PathBuf::from("R/brave.exe"), BrowserKind::Brave),
                (PathBuf::from("R/vivaldi.exe"), BrowserKind::Vivaldi),
            ],
            ..Default::default()
        };
        let has = |os: Os, kind: BrowserKind, path: &str| {
            let c = candidates(os, &env);
            assert!(
                c.contains(&(PathBuf::from(path), kind)),
                "{path} missing from {c:?}"
            );
        };
        use BrowserKind::{Brave, Vivaldi};
        for base in ["PF", "PF86", "L"] {
            has(
                Os::Windows,
                Brave,
                &format!("{base}/BraveSoftware/Brave-Browser/Application/brave.exe"),
            );
            has(
                Os::Windows,
                Vivaldi,
                &format!("{base}/Vivaldi/Application/vivaldi.exe"),
            );
        }
        // The registry's App Paths entry comes before the install folders.
        let win = candidates(Os::Windows, &env);
        let first = |kind: BrowserKind| win.iter().find(|(_, k)| *k == kind).unwrap().0.clone();
        assert_eq!(first(Brave), PathBuf::from("R/brave.exe"));
        assert_eq!(first(Vivaldi), PathBuf::from("R/vivaldi.exe"));
        has(
            Os::Mac,
            Brave,
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        );
        has(
            Os::Mac,
            Brave,
            "/Users/u/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        );
        has(
            Os::Mac,
            Vivaldi,
            "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
        );
        has(
            Os::Mac,
            Vivaldi,
            "/Users/u/Applications/Vivaldi.app/Contents/MacOS/Vivaldi",
        );
        has(Os::Linux, Brave, "/usr/bin/brave-browser");
        has(Os::Linux, Brave, "/opt/brave.com/brave/brave");
        has(Os::Linux, Vivaldi, "/usr/bin/vivaldi-stable");
        has(Os::Linux, Vivaldi, "/opt/vivaldi/vivaldi");
    }

    #[test]
    fn the_agents_own_browser_comes_after_installed_ones_and_before_the_test_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |rel: &str| {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "x").unwrap();
            dunce::canonicalize(&p).unwrap()
        };
        let own = mk("agent-browser/1.2.3.4/app/own");
        let playwright = mk("pw/chromium-1000/chrome-linux64/chrome");
        let brave = mk("L/BraveSoftware/Brave-Browser/Application/brave.exe");
        // Windows locations all come from the environment, so nothing installed on the
        // machine running the test is found.
        let mut env = DiscoveryEnv {
            own_browser: Some(own.clone()),
            playwright_root: Some(dir.path().join("pw")),
            ..Default::default()
        };
        // Only the own browser and the test fallback exist: the own browser is used.
        let found = find_browser_in(None, Os::Windows, &env).unwrap();
        assert_eq!((found.path, found.kind), (own.clone(), BrowserKind::Own));
        // An installed browser wins over it.
        env.local_app_data = Some(dir.path().join("L"));
        let found = find_browser_in(None, Os::Windows, &env).unwrap();
        assert_eq!((found.path, found.kind), (brave, BrowserKind::Brave));
        // Removed: back to the fallback.
        env.local_app_data = None;
        std::fs::remove_file(&own).unwrap();
        let found = find_browser_in(None, Os::Windows, &env).unwrap();
        assert_eq!(
            (found.path, found.kind),
            (playwright, BrowserKind::Chromium)
        );
    }

    #[test]
    fn the_next_step_offers_the_download_on_macos_only() {
        let none = DiscoveryEnv {
            // Nothing on this machine is under this home folder; /Applications does not
            // exist on the Linux test machines and holds no browser on a Mac in CI only
            // by luck, so the Mac case is checked through the message function.
            home: Some(PathBuf::from("/nonexistent-home")),
            ..Default::default()
        };
        let mac = not_installed(Os::Mac);
        assert!(
            mac.next_step
                .contains("download a browser for the agent in Settings → Browser"),
            "{}",
            mac.next_step
        );
        for os in [Os::Windows, Os::Linux] {
            let e = not_installed(os);
            assert!(!e.next_step.contains("download"), "{os:?}: {}", e.next_step);
            assert_eq!(e.message, mac.message);
        }
        let err = find_browser_in(None, Os::Windows, &none).unwrap_err();
        assert_eq!(err, not_installed(Os::Windows));
    }

    #[test]
    fn kinds_have_plain_names() {
        assert_eq!(BrowserKind::Brave.label(), "Brave");
        assert_eq!(BrowserKind::Chrome.label(), "Google Chrome");
        assert!(BrowserKind::Own.label().contains("Chrome for Testing"));
    }
}
