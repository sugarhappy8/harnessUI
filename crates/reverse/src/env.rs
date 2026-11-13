//! rea's exact environment and command line (item5-spec.md WP9b `env.rs`, R8, R9, R10,
//! R20): `node <rea>/scripts/rea.mjs mcp` in `reverse/work`, with `HOME`, temp folders and
//! caches inside the reverse folder, the tool families, the chosen decompiler and nothing
//! else of the app's environment except a fixed few names.
//!
//! The environment is built from nothing ([`runtime::EnvSpec`] clears the child's
//! environment first): only the names in [`WINDOWS_COPIED`] or [`UNIX_COPIED`] are copied
//! from the app, when present, and everything else is set here. So `NODE_OPTIONS`, the Java
//! option variables, proxy variables and any other `REA_*` setting ([`NEVER`]) cannot reach
//! rea, Kuna, Ghidra or Java, whatever the app's own environment holds.
//!
//! `REA_ANALYSIS_PROVIDER` is always set, never `auto`: rea's automatic search would look
//! for Hopper, IDA and other decompilers on the computer (R10). With no decompiler it is
//! `ghidra` with no Ghidra configured, so rea uses none.
//!
//! On Linux with Ghidra, `TMPDIR` is the private folder under `XDG_RUNTIME_DIR` when the
//! layout has one ([`ReverseLayout::run_tmp`]), else `reverse/tmp`, from which rea moves
//! Ghidra's session to `/tmp` because the path has a folder starting with a dot
//! (`~/.local/share`).
//!
//! Known limit (Linux): outside Windows rea empties every Java option variable for Ghidra's
//! Java and takes its options only from Ghidra's own `launch.properties`, so the app cannot
//! pass `-XX:-UsePerfData` (rea passes it itself on Windows). Java keeps a 32 KB performance
//! file in `/tmp/hsperfdata_<user>/` while it runs, whatever its temporary folder is, and
//! removes it when it ends; after a Java ended by force (a stop that outlasts rea's grace),
//! the file stays until the next Java of that user starts and removes it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Decompiler;
use crate::install::Installation;
use crate::layout::ReverseLayout;
use crate::lock::{self, ReverseLock};
use crate::platform::{Os, Platform};
use crate::process::RunConfig;

/// Start limit on the first start after an install or repair (antivirus scans 10,585 new
/// files).
pub const FIRST_START_TIMEOUT: Duration = Duration::from_secs(180);
/// Start limit after that.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);

/// Copied from the app's environment when present, on Windows.
pub const WINDOWS_COPIED: &[&str] = &[
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "SYSTEMDRIVE",
    "USERNAME",
    "PROGRAMFILES",
];

/// Copied from the app's environment when present, on macOS and Linux.
pub const UNIX_COPIED: &[&str] = &["USER", "LOGNAME", "LANG"];

/// `PATH` on macOS.
pub const MACOS_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// `PATH` on Linux.
pub const LINUX_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Never in rea's environment (item5-spec.md WP9b). Proxy variables in any case, and any
/// `REA_*_COMMAND` or `REA_*_URL` other than `REA_KUNA_COMMAND`, are refused too (see
/// [`is_never`]).
pub const NEVER: &[&str] = &[
    "NODE_OPTIONS",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "REA_GHIDRA_PROJECT_PATH",
    "REA_GHIDRA_CHECKPOINT_DIR",
    "REA_DARWIN_SWIFTC_PATH",
    "REA_IDA_MCP_CONFIG",
];

/// Variables that name a folder inside the reverse folder, created before rea starts.
pub const FOLDER_VARIABLES: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "TEMP",
    "TMP",
    "TMPDIR",
    "REA_KUNA_CACHE_DIR",
];

/// The variables of the Kuna provider (never set together with [`GHIDRA_VARIABLES`]).
pub const KUNA_VARIABLES: &[&str] = &[
    "REA_KUNA_COMMAND",
    "REA_KUNA_CACHE_DIR",
    "REA_KUNA_CACHE_MAX_BYTES",
    "REA_KUNA_PROJECT_TIMEOUT_MS",
    "REA_KUNA_TIMEOUT_MS",
];

/// The variables of the Ghidra provider (never set together with [`KUNA_VARIABLES`]).
pub const GHIDRA_VARIABLES: &[&str] = &[
    "GHIDRA_INSTALL_DIR",
    "JAVA_HOME",
    "GHIDRA_HEADLESS_MAXMEM",
    "REA_GHIDRA_STARTUP_TIMEOUT_MS",
    "REA_GHIDRA_DISABLED_ANALYZERS",
];

/// Whether `name` may never be in rea's environment ([`NEVER`], proxy variables, another
/// tool's `REA_*_COMMAND` or `REA_*_URL`).
pub fn is_never(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    NEVER.contains(&upper.as_str())
        || upper.ends_with("_PROXY")
        || upper == "PROXY"
        || (upper.starts_with("REA_")
            && (upper.ends_with("_COMMAND") || upper.ends_with("_URL"))
            && upper != "REA_KUNA_COMMAND")
}

/// Which system's variables and `PATH` to build (any flavour can be built on any OS, for
/// the golden tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
}

impl Flavor {
    /// The flavour of a platform (`Linux` for anything that is not Windows or macOS).
    pub fn of(platform: Platform) -> Self {
        match platform.os() {
            Os::Windows => Flavor::Windows,
            Os::Macos => Flavor::Macos,
            Os::Linux | Os::Other => Flavor::Linux,
        }
    }
    /// The flavour of this build.
    pub fn current() -> Self {
        Self::of(Platform::current())
    }
    /// The platform whose lock entries (the disabled Ghidra analyzers) apply: this
    /// computer's when the flavour is its own, else the flavour's main platform.
    fn platform(self) -> Platform {
        let here = Platform::current();
        if Self::of(here) == self {
            return here;
        }
        match self {
            Flavor::Windows => Platform::WindowsX64,
            Flavor::Macos => Platform::MacosArm64,
            Flavor::Linux => Platform::LinuxX64,
        }
    }
}

/// How to start rea for this installation and decompiler, with the app's own lock and
/// environment.
///
/// The program is the pack's `node` and the first argument rea's entry script; when rea is
/// not installed both are empty, and [`crate::process::ReaGuard::ensure_started`] refuses
/// to start. A decompiler that is chosen but not installed counts as none.
pub fn run_config(
    layout: &ReverseLayout,
    installed: &Installation,
    decompiler: Option<Decompiler>,
    flavor: Flavor,
    first_start: bool,
) -> RunConfig {
    run_config_with(
        lock::embedded(),
        layout,
        installed,
        decompiler,
        flavor,
        first_start,
        &|name| std::env::var(name).ok(),
    )
}

/// [`run_config`] with a given lock and a given source for the app's own variables (only
/// `SYSTEMROOT` is read, for `PATH` on Windows; the copied names are resolved when rea
/// starts).
pub fn run_config_with(
    lock: &ReverseLock,
    layout: &ReverseLayout,
    installed: &Installation,
    decompiler: Option<Decompiler>,
    flavor: Flavor,
    first_start: bool,
    app_env: &dyn Fn(&str) -> Option<String>,
) -> RunConfig {
    let copied = match flavor {
        Flavor::Windows => WINDOWS_COPIED,
        Flavor::Macos | Flavor::Linux => UNIX_COPIED,
    };
    let mut env = runtime::EnvSpec::empty();
    for name in copied {
        env = env.inherit(*name);
    }
    let path = match flavor {
        Flavor::Windows => {
            let root = system_root(app_env);
            format!(r"{root}\System32;{root};{root}\System32\Wbem")
        }
        Flavor::Macos => MACOS_PATH.to_owned(),
        Flavor::Linux => LINUX_PATH.to_owned(),
    };
    env = env.set("PATH", path);
    let kuna = match decompiler {
        Some(Decompiler::Kuna) => installed.kuna_program(),
        _ => None,
    };
    let ghidra = match decompiler {
        Some(Decompiler::Ghidra) => installed.ghidra_dir().zip(installed.java_home.clone()),
        _ => None,
    };
    let home = layout.home();
    env = env.set("HOME", text(&home));
    match flavor {
        Flavor::Windows => {
            env = env
                .set("USERPROFILE", text(&home))
                .set("APPDATA", text(&home.join("AppData").join("Roaming")))
                .set("LOCALAPPDATA", text(&home.join("AppData").join("Local")))
                .set("TEMP", text(&layout.tmp()))
                .set("TMP", text(&layout.tmp()));
        }
        Flavor::Linux => {
            // With Ghidra, the private folder under XDG_RUNTIME_DIR when there is one: the
            // reverse folder is under ~/.local/share, and from a temporary folder there rea
            // would move Ghidra's session to /tmp (module documentation of `layout`).
            let tmp = match (&ghidra, layout.run_tmp()) {
                (Some(_), Some(_run_tmp)) => layout.tmp(), // TEMP-BEFORE-FIX
                _ => layout.tmp(),
            };
            env = env
                .set("XDG_CONFIG_HOME", text(&home.join(".config")))
                .set("XDG_CACHE_HOME", text(&home.join(".cache")))
                .set("XDG_DATA_HOME", text(&home.join(".local").join("share")))
                .set("XDG_STATE_HOME", text(&home.join(".local").join("state")))
                .set("TMPDIR", text(&tmp));
        }
        Flavor::Macos => {
            env = env.set("TMPDIR", text(&layout.tmp()));
        }
    }
    env = env
        .set("REA_MCP_TOOL_FAMILIES", lock.families.clone())
        .set("REA_MCP_TOOL_CATALOG_PROFILE", "full")
        .set("REA_MCP_INPUT_SCHEMA_PROFILE", "compact")
        .set("REA_MCP_OUTPUT_SCHEMA_PROFILE", "off")
        .set("REA_LOG_LEVEL", "error");
    if let Some(kuna) = kuna {
        env = env
            .set("REA_ANALYSIS_PROVIDER", "kuna")
            .set("REA_KUNA_COMMAND", text(&kuna))
            .set("REA_KUNA_CACHE_DIR", text(&layout.kuna_cache()))
            .set("REA_KUNA_CACHE_MAX_BYTES", "1073741824")
            .set("REA_KUNA_PROJECT_TIMEOUT_MS", "900000")
            .set("REA_KUNA_TIMEOUT_MS", "120000");
    } else if let Some((ghidra, java)) = ghidra {
        env = env
            .set("REA_ANALYSIS_PROVIDER", "ghidra")
            .set("GHIDRA_INSTALL_DIR", text(&ghidra))
            .set("JAVA_HOME", text(&java))
            .set("GHIDRA_HEADLESS_MAXMEM", "2G")
            .set("REA_GHIDRA_STARTUP_TIMEOUT_MS", "900000");
        if let Some(analyzers) = lock
            .disabled_analyzers(flavor.platform())
            .filter(|a| !a.is_empty())
        {
            env = env.set("REA_GHIDRA_DISABLED_ANALYZERS", analyzers);
        }
    } else {
        // No decompiler: rea uses none (never its automatic search).
        env = env.set("REA_ANALYSIS_PROVIDER", "ghidra");
    }
    RunConfig {
        program: installed.node().unwrap_or_default(),
        args: vec![
            installed.rea_entry.as_deref().map(text).unwrap_or_default(),
            "mcp".into(),
        ],
        env,
        cwd: layout.work(),
        start_timeout: if first_start {
            FIRST_START_TIMEOUT
        } else {
            START_TIMEOUT
        },
    }
}

/// `SYSTEMROOT` when it is a plain absolute drive path (`C:\Windows`), else `C:\Windows`.
fn system_root(app_env: &dyn Fn(&str) -> Option<String>) -> String {
    let plain = |v: &str| {
        let b = v.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && b[2] == b'\\'
            && !v.contains([';', '"', '%'])
            && !v.chars().any(char::is_control)
    };
    ["SYSTEMROOT", "SystemRoot"]
        .iter()
        .filter_map(|n| app_env(n))
        .find(|v| plain(v))
        .map(|v| v.trim_end_matches('\\').to_owned())
        .unwrap_or_else(|| r"C:\Windows".to_owned())
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The folders a configuration names inside the reverse folder ([`FOLDER_VARIABLES`]),
/// which are created before rea starts.
pub fn folders(config: &RunConfig, layout: &ReverseLayout) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = config
        .env
        .set
        .iter()
        .filter(|(k, _)| FOLDER_VARIABLES.contains(&k.as_str()))
        .map(|(_, v)| PathBuf::from(v))
        .filter(|p| p.starts_with(layout.root()))
        .collect();
    out.push(config.cwd.clone());
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::install::{Installed, InstalledPart, ProgramFile};
    use crate::{Component, lock::ReverseLock};

    const ROOT: &str = "/data/app/reverse";

    fn part(name: &str, version: &str, dir: &str, program: Option<&str>) -> InstalledPart {
        InstalledPart {
            name: name.into(),
            version: version.into(),
            dir: Path::new(ROOT).join(dir),
            programs: program
                .map(|p| {
                    vec![ProgramFile {
                        path: p.into(),
                        size: 1,
                    }]
                })
                .unwrap_or_default(),
            archive_sha256: Vec::new(),
            disk_bytes: 1,
        }
    }

    fn installed(component: Component, parts: Vec<InstalledPart>) -> Installed {
        Installed {
            component,
            version: parts[0].version.clone(),
            parts,
            disk_bytes: 2,
        }
    }

    /// rea, Kuna and Ghidra all installed.
    fn everything() -> Installation {
        Installation {
            rea: Some(installed(
                Component::Rea,
                vec![
                    part("rea", "6.9.0", "rea-6.9.0", Some("scripts/rea.mjs")),
                    part("node", "24.22.3", "node-24.22.3", Some("bin/node")),
                ],
            )),
            kuna: Some(installed(
                Component::Kuna,
                vec![part("kuna", "1.990", "kuna-1.990", Some("kuna"))],
            )),
            ghidra: Some(installed(
                Component::Ghidra,
                vec![
                    part(
                        "ghidra",
                        "12.1.4",
                        "ghidra-12.1.4",
                        Some("support/analyzeHeadless"),
                    ),
                    part("java", "21.0.13+9", "java-21.0.13+9", Some("bin/java")),
                ],
            )),
            rea_entry: Some(path("rea-6.9.0/scripts/rea.mjs")),
            java_home: Some(Path::new(ROOT).join("java-21.0.13+9")),
        }
    }

    fn lock() -> ReverseLock {
        let mut lock = lock::embedded().clone();
        lock.families =
            "direct,enhanced,native,artifact,managed,session,electron,application".into();
        lock.disabled_ghidra_analyzers = BTreeMap::from([
            ("windows-x86_64".to_owned(), "PDB Universal".to_owned()),
            ("linux-x86_64".to_owned(), "PDB Universal".to_owned()),
        ]);
        lock
    }

    fn app_env(name: &str) -> Option<String> {
        match name {
            "SYSTEMROOT" => Some(r"D:\Win".into()),
            "NODE_OPTIONS" => Some("--require /evil.js".into()),
            _ => None,
        }
    }

    fn build(d: Option<Decompiler>, flavor: Flavor, first: bool) -> RunConfig {
        run_config_with(
            &lock(),
            &ReverseLayout::new(ROOT),
            &everything(),
            d,
            flavor,
            first,
            &app_env,
        )
    }

    fn set(config: &RunConfig) -> BTreeMap<String, String> {
        let map: BTreeMap<String, String> = config.env.set.iter().cloned().collect();
        assert_eq!(map.len(), config.env.set.len(), "a variable is set twice");
        map
    }

    /// `ROOT` and `rel`, one part at a time, as the installer joins its relative paths.
    fn path(rel: &str) -> PathBuf {
        let mut path = PathBuf::from(ROOT);
        for c in rel.split('/') {
            path.push(c);
        }
        path
    }

    fn p(rel: &str) -> String {
        path(rel).to_string_lossy().into_owned()
    }

    fn common(flavor: Flavor) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        let mut put = |k: &str, v: String| {
            m.insert(k.to_owned(), v);
        };
        put(
            "REA_MCP_TOOL_FAMILIES",
            "direct,enhanced,native,artifact,managed,session,electron,application".into(),
        );
        put("REA_MCP_TOOL_CATALOG_PROFILE", "full".into());
        put("REA_MCP_INPUT_SCHEMA_PROFILE", "compact".into());
        put("REA_MCP_OUTPUT_SCHEMA_PROFILE", "off".into());
        put("REA_LOG_LEVEL", "error".into());
        put("HOME", p("home"));
        match flavor {
            Flavor::Windows => {
                put(
                    "PATH",
                    r"D:\Win\System32;D:\Win;D:\Win\System32\Wbem".into(),
                );
                put("USERPROFILE", p("home"));
                put("APPDATA", p("home/AppData/Roaming"));
                put("LOCALAPPDATA", p("home/AppData/Local"));
                put("TEMP", p("tmp"));
                put("TMP", p("tmp"));
            }
            Flavor::Macos => {
                put("PATH", "/usr/bin:/bin:/usr/sbin:/sbin".into());
                put("TMPDIR", p("tmp"));
            }
            Flavor::Linux => {
                put("PATH", "/usr/local/bin:/usr/bin:/bin".into());
                put("XDG_CONFIG_HOME", p("home/.config"));
                put("XDG_CACHE_HOME", p("home/.cache"));
                put("XDG_DATA_HOME", p("home/.local/share"));
                put("XDG_STATE_HOME", p("home/.local/state"));
                put("TMPDIR", p("tmp"));
            }
        }
        m
    }

    fn kuna_vars() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("REA_ANALYSIS_PROVIDER".to_owned(), "kuna".to_owned()),
            ("REA_KUNA_COMMAND".to_owned(), p("kuna-1.990/kuna")),
            ("REA_KUNA_CACHE_DIR".to_owned(), p("cache/kuna")),
            (
                "REA_KUNA_CACHE_MAX_BYTES".to_owned(),
                "1073741824".to_owned(),
            ),
            (
                "REA_KUNA_PROJECT_TIMEOUT_MS".to_owned(),
                "900000".to_owned(),
            ),
            ("REA_KUNA_TIMEOUT_MS".to_owned(), "120000".to_owned()),
        ])
    }

    fn ghidra_vars(analyzers: bool) -> BTreeMap<String, String> {
        let mut m = BTreeMap::from([
            ("REA_ANALYSIS_PROVIDER".to_owned(), "ghidra".to_owned()),
            ("GHIDRA_INSTALL_DIR".to_owned(), p("ghidra-12.1.4")),
            ("JAVA_HOME".to_owned(), p("java-21.0.13+9")),
            ("GHIDRA_HEADLESS_MAXMEM".to_owned(), "2G".to_owned()),
            (
                "REA_GHIDRA_STARTUP_TIMEOUT_MS".to_owned(),
                "900000".to_owned(),
            ),
        ]);
        if analyzers {
            m.insert(
                "REA_GHIDRA_DISABLED_ANALYZERS".into(),
                "PDB Universal".into(),
            );
        }
        m
    }

    #[test]
    fn golden_per_flavour_and_decompiler() {
        for flavor in [Flavor::Windows, Flavor::Macos, Flavor::Linux] {
            for d in [None, Some(Decompiler::Kuna), Some(Decompiler::Ghidra)] {
                let config = build(d, flavor, false);
                let mut want = common(flavor);
                match d {
                    Some(Decompiler::Kuna) => want.extend(kuna_vars()),
                    // Only Windows and Linux have analyzers to turn off in the lock.
                    Some(Decompiler::Ghidra) => {
                        let listed = lock().disabled_analyzers(flavor.platform()).is_some();
                        want.extend(ghidra_vars(listed));
                    }
                    None => {
                        want.insert("REA_ANALYSIS_PROVIDER".into(), "ghidra".into());
                    }
                }
                assert_eq!(set(&config), want, "{flavor:?} {d:?}");
                let copied: Vec<&str> = config.env.inherit.iter().map(String::as_str).collect();
                match flavor {
                    Flavor::Windows => assert_eq!(copied, WINDOWS_COPIED, "{flavor:?}"),
                    _ => assert_eq!(copied, UNIX_COPIED, "{flavor:?}"),
                }
                assert_eq!(config.program, path("node-24.22.3/bin/node"));
                assert_eq!(
                    config.args,
                    vec![p("rea-6.9.0/scripts/rea.mjs"), "mcp".into()]
                );
                assert_eq!(config.cwd, Path::new(ROOT).join("work"));
                assert_eq!(config.start_timeout, Duration::from_secs(60));
            }
        }
    }

    /// Fixture paths are built one part at a time, as the installer builds them: on Windows
    /// a `join` of a text holding '/' keeps that '/' next to the '\' of `push`, and the
    /// golden test above compares `args` and the variables as text, so it would fail there
    /// only.
    #[test]
    fn fixture_paths_are_built_one_part_at_a_time() {
        let source = include_str!("env.rs");
        let tests = &source[source.find("mod tests {").expect("the tests")..];
        let needle = concat!(".join", "(\"");
        for line in tests.lines().filter(|l| !l.trim_start().starts_with("//")) {
            let mut rest = line;
            while let Some(at) = rest.find(needle) {
                rest = &rest[at + needle.len()..];
                let literal = rest.split('"').next().unwrap_or_default();
                assert!(!literal.contains('/'), "{}", line.trim());
            }
        }
    }

    /// On Linux with Ghidra, a usable runtime folder replaces `reverse/tmp` as `TMPDIR` (the
    /// golden test above is the case without one): rea then keeps Ghidra's session there
    /// instead of moving it to /tmp. Nothing else changes, in no other case.
    #[test]
    fn on_linux_ghidra_gets_the_private_runtime_folder_when_there_is_one() {
        const RUN: &str = "/run/user/1000/app-reverse";
        let layout = ReverseLayout::new(ROOT).with_run_tmp(Some(RUN.into()));
        for flavor in [Flavor::Windows, Flavor::Macos, Flavor::Linux] {
            for d in [None, Some(Decompiler::Kuna), Some(Decompiler::Ghidra)] {
                let with = run_config_with(
                    &lock(),
                    &layout,
                    &everything(),
                    d,
                    flavor,
                    false,
                    &app_env,
                );
                let mut want = set(&build(d, flavor, false));
                if flavor == Flavor::Linux && d == Some(Decompiler::Ghidra) {
                    want.insert("TMPDIR".into(), RUN.into());
                }
                assert_eq!(set(&with), want, "{flavor:?} {d:?}");
                assert_eq!(with.env.inherit, build(d, flavor, false).env.inherit);
                // Created by rea's process itself (private, outside the reverse folder).
                assert!(!folders(&with, &layout).contains(&PathBuf::from(RUN)));
            }
        }
        // Ghidra chosen but not installed counts as none: reverse/tmp.
        let mut only_rea = everything();
        only_rea.ghidra = None;
        only_rea.java_home = None;
        let config = run_config_with(
            &lock(),
            &layout,
            &only_rea,
            Some(Decompiler::Ghidra),
            Flavor::Linux,
            false,
            &app_env,
        );
        assert_eq!(set(&config)["TMPDIR"], p("tmp"));
    }

    #[test]
    fn the_first_start_after_an_install_may_take_180_seconds() {
        assert_eq!(
            build(None, Flavor::Windows, true).start_timeout,
            Duration::from_secs(180)
        );
        assert_eq!(
            build(None, Flavor::Windows, false).start_timeout,
            Duration::from_secs(60)
        );
    }

    #[test]
    fn nothing_forbidden_ever_reaches_rea() {
        // Even with every forbidden name in the app's environment.
        let hostile = |name: &str| -> Option<String> {
            if name == "SYSTEMROOT" {
                return Some(r"C:\Windows".into());
            }
            Some(format!("hostile value of {name}"))
        };
        for flavor in [Flavor::Windows, Flavor::Macos, Flavor::Linux] {
            for d in [None, Some(Decompiler::Kuna), Some(Decompiler::Ghidra)] {
                let config = run_config_with(
                    &lock(),
                    &ReverseLayout::new(ROOT),
                    &everything(),
                    d,
                    flavor,
                    false,
                    &hostile,
                );
                let names = config
                    .env
                    .set
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .chain(config.env.inherit.iter().map(String::as_str));
                for name in names {
                    assert!(!is_never(name), "{name} in {flavor:?} {d:?}");
                }
                // The copied names resolve against the app's environment at start:
                // even an app full of forbidden variables passes none of them on.
                let parent: Vec<(std::ffi::OsString, std::ffi::OsString)> = NEVER
                    .iter()
                    .chain(
                        [
                            "HTTPS_PROXY",
                            "http_proxy",
                            "ALL_PROXY",
                            "REA_JEB_MCP_URL",
                            "REA_ILSPY_CMD_PATH",
                        ]
                        .iter(),
                    )
                    .map(|n| ((*n).into(), "x".into()))
                    .chain([
                        ("USER".into(), "me".into()),
                        ("SYSTEMROOT".into(), r"C:\Windows".into()),
                    ])
                    .collect();
                for (name, _) in config.env.resolve_from(parent) {
                    let name = name.to_string_lossy().into_owned();
                    assert!(!is_never(&name) && name != "REA_ILSPY_CMD_PATH", "{name}");
                }
            }
        }
        assert!(is_never("NODE_OPTIONS") && is_never("https_proxy") && is_never("NO_PROXY"));
        assert!(is_never("REA_JEB_MCP_URL") && is_never("REA_HOPPER_COMMAND"));
        assert!(!is_never("REA_KUNA_COMMAND") && !is_never("PATH"));
    }

    #[test]
    fn kuna_and_ghidra_variables_are_never_both_set() {
        for flavor in [Flavor::Windows, Flavor::Macos, Flavor::Linux] {
            for d in [None, Some(Decompiler::Kuna), Some(Decompiler::Ghidra)] {
                let names = set(&build(d, flavor, false));
                let kuna = KUNA_VARIABLES.iter().any(|v| names.contains_key(*v));
                let ghidra = GHIDRA_VARIABLES.iter().any(|v| names.contains_key(*v));
                assert!(!(kuna && ghidra), "{flavor:?} {d:?}");
                assert_eq!(kuna, d == Some(Decompiler::Kuna));
                assert_eq!(ghidra, d == Some(Decompiler::Ghidra));
                let provider = names.get("REA_ANALYSIS_PROVIDER").map(String::as_str);
                assert!(matches!(provider, Some("kuna" | "ghidra")), "{provider:?}");
            }
        }
    }

    #[test]
    fn a_chosen_decompiler_that_is_not_installed_counts_as_none() {
        let mut only_rea = everything();
        only_rea.kuna = None;
        only_rea.ghidra = None;
        only_rea.java_home = None;
        for d in [Some(Decompiler::Kuna), Some(Decompiler::Ghidra)] {
            let config = run_config_with(
                &lock(),
                &ReverseLayout::new(ROOT),
                &only_rea,
                d,
                Flavor::Linux,
                false,
                &app_env,
            );
            let names = set(&config);
            assert_eq!(names["REA_ANALYSIS_PROVIDER"], "ghidra");
            assert!(KUNA_VARIABLES.iter().all(|v| !names.contains_key(*v)));
            assert!(GHIDRA_VARIABLES.iter().all(|v| !names.contains_key(*v)));
        }
    }

    #[test]
    fn without_rea_the_program_is_empty_so_nothing_can_start() {
        let config = run_config_with(
            &lock(),
            &ReverseLayout::new(ROOT),
            &Installation::default(),
            None,
            Flavor::Linux,
            false,
            &app_env,
        );
        assert_eq!(config.program, PathBuf::new());
        assert_eq!(config.args, vec![String::new(), "mcp".to_owned()]);
    }

    #[test]
    fn a_strange_systemroot_falls_back_to_the_usual_one() {
        for bad in [
            r"\\server\share",
            "C:\\Win;C:\\evil",
            "relative",
            "C:\\%X%",
            "C:\\a\nb",
        ] {
            let env = move |n: &str| (n == "SYSTEMROOT").then(|| bad.to_owned());
            assert_eq!(system_root(&env), r"C:\Windows", "{bad:?}");
        }
        assert_eq!(system_root(&|_| None), r"C:\Windows");
        assert_eq!(
            system_root(&|n| (n == "SystemRoot").then(|| r"E:\WINNT\".into())),
            r"E:\WINNT"
        );
    }

    #[test]
    fn the_folders_to_create_are_inside_the_reverse_folder() {
        let layout = ReverseLayout::new(ROOT);
        for flavor in [Flavor::Windows, Flavor::Macos, Flavor::Linux] {
            let config = build(Some(Decompiler::Kuna), flavor, false);
            let folders = folders(&config, &layout);
            assert!(folders.iter().all(|f| f.starts_with(ROOT)), "{folders:?}");
            assert!(folders.contains(&layout.home()));
            assert!(folders.contains(&layout.tmp()));
            assert!(folders.contains(&layout.work()));
            assert!(folders.contains(&layout.kuna_cache()));
            // Program folders are never among them.
            assert!(!folders.iter().any(|f| f.ends_with("kuna-1.990")));
        }
    }

    #[test]
    fn the_embedded_lock_names_the_families_of_section_5() {
        let config = run_config(
            &ReverseLayout::new(ROOT),
            &everything(),
            Some(Decompiler::Ghidra),
            Flavor::Linux,
            false,
        );
        let names = set(&config);
        assert_eq!(
            names["REA_MCP_TOOL_FAMILIES"],
            "direct,enhanced,native,artifact,managed,session,electron,application"
        );
        assert_eq!(names["REA_GHIDRA_DISABLED_ANALYZERS"], "PDB Universal");
    }
}
