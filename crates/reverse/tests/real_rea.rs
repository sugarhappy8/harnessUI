//! Contract tests with the real rea, Node.js, Kuna, Ghidra and Java (item5-spec.md WP9e).
//!
//! They run only when `REVERSE_PACK_CACHE` names a folder filled by
//! `python3 -I scripts/pin_reverse.py --download-to <dir>` (each archive is checked against
//! the lock there and again here); without it, or without one decompiler's archives for this
//! computer, that decompiler's test says so and passes without checking anything. Each test:
//!
//! - installs rea and the decompiler through `reverse::install` with a `Fetch` that reads the
//!   cache, so the real archives go through the real unpacker on every system, and checks
//!   that each program the app or rea runs has the lock's size (and is executable on Unix)
//!   and that each part takes the lock's unpacked size;
//! - starts rea through `ReaProcess` with the environment the app builds (`env::run_config`);
//!   every allow-listed tool passes the live check against the pinned fixture, and the tools
//!   of the families the app turns off are absent from rea's `tools/list`;
//! - opens `hello-linux-x64`, finds `main` among its functions and gets readable code for it,
//!   runs the file checks the app offers on the Windows program, the .NET library and the
//!   ELF, and reads `binary_session`, which names no path outside the pack and the fixtures;
//! - stops rea and starts it again (both start times are printed), and measures the peak
//!   memory of Ghidra's Java on Linux while it opens the file; with Ghidra, stops a call in
//!   the middle of the analysis (it ends as cancelled); after the last stop nothing from the
//!   pack still runs (Linux; the workflow checks Windows and macOS with their own tools);
//! - checks that everything rea wrote is under the pack's `home`, `tmp`, `cache` and `work`
//!   (the part folders and the fixtures' folder are as they were, and nothing of rea's, Kuna's
//!   or Ghidra's appeared in the system's temporary folders at any time during the run: they
//!   are watched every 200 ms), and that rea added nothing to the runner's `~/.cache/rea`,
//!   `~/Library/Caches/rea`, `~/.codex`, `~/.cursor`, `~/.gemini`, `~/.claude` or
//!   `~/.claude.json`. `~/.claude` and `~/.claude.json` are compared in full only when `CI` is
//!   set (on a developer's computer Claude Code itself writes there while the test runs);
//!   otherwise their settings files must not newly name rea.
//!
//! The pack goes into a temporary folder named `real-rea-…`, or into
//! `REAL_REA_DIR/<kuna|ghidra>` when that is set (the Windows job blocks the pack's programs in
//! the firewall by path before the run). Neither may have a folder whose name starts with a
//! dot, as the app's folders on Windows and macOS have none: rea moves Ghidra's session (its
//! project, logs, a copy of the opened file and the bridge socket) to `/tmp` when the temporary
//! folder it is given has one, because Ghidra refuses such project paths. On Linux a third test
//! puts the pack under `.local/share`, as the app's own folder is on Linux, and checks that
//! whatever rea puts in the system's temporary folder there is gone after the stop (it prints
//! what it saw: a known limit of the app on Linux).
//!
//! `REAL_REA_REQUIRE` lists the decompilers this computer must test (`kuna,ghidra`): a test
//! that would be skipped fails instead. `REAL_REA_LARGE=decompile` (or a program's absolute
//! path) also opens a larger program with Ghidra (the pack's own decompiler, 3.4 MB on Linux)
//! and prints how long its analysis took and Java's peak memory. `REAL_REA_VERBOSE` prints
//! rea's results in full.
//!
//! The test starts no process itself (`scripts/check-spawn.sh`): rea starts through
//! `crates/runtime`, and the workflow (`.github/workflows/reverse.yml`) adds the process and
//! network checks around it. The tests take turns: one rea, and Ghidra's 2 GB heap, at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component as PathPart, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use mcp_host::ToolOutput;
use reverse::env::{self, Flavor};
use reverse::process::STOP_GRACE;
use reverse::{
    Component, ComponentState, Decompiler, DirFetch, Installer, Platform, ReaGuard, ReaProcess,
    RestartReason, ReverseLayout, RunState, allow, results,
};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

/// One real rea at a time (and one Ghidra heap).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const FIXTURE_FILES: [&str; 3] = ["hello-linux-x64", "hello-win-x64.exe", "hello-dotnet.dll"];

/// The program each test opens, and the function it reads: the ELF and its `main`, except
/// on Windows, where rea's Ghidra support opens Windows programs only (the PE's entry point,
/// which Ghidra names `entry`). `REAL_REA_OPEN` names another fixture to open.
fn open_target() -> (&'static str, &'static str) {
    let pe = ("hello-win-x64.exe", "entry");
    let elf = ("hello-linux-x64", "main");
    match std::env::var("REAL_REA_OPEN").as_deref() {
        Ok("hello-win-x64.exe") => pe,
        Ok("hello-linux-x64") => elf,
        _ if cfg!(windows) => pe,
        _ => elf,
    }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// What a test needs on this computer, or why it is skipped.
fn needs(decompiler: Decompiler) -> Result<(PathBuf, Platform), String> {
    let Some(cache) = std::env::var_os("REVERSE_PACK_CACHE").map(PathBuf::from) else {
        return Err("REVERSE_PACK_CACHE is not set".into());
    };
    let platform = Platform::current();
    let installer = Installer::new(ReverseLayout::new(cache.join("unused")), platform);
    let status = installer.status(decompiler.component());
    if !status.available {
        return Err(format!(
            "{} is not offered on {}: {}",
            decompiler.label(),
            platform.key(),
            status.reason.unwrap_or_default()
        ));
    }
    let lock = reverse::lock::embedded();
    let parts: &[&str] = match decompiler {
        Decompiler::Kuna => &["rea", "node", "kuna"],
        Decompiler::Ghidra => &["rea", "node", "ghidra", "java"],
    };
    let missing: Vec<String> = parts
        .iter()
        .filter_map(|p| lock.part(p)?.build_for(platform))
        .map(|b| b.file.name.clone())
        .filter(|name| !cache.join(name).is_file())
        .collect();
    if !missing.is_empty() {
        return Err(format!("the cache lacks {missing:?}"));
    }
    Ok((cache, platform))
}

/// Whether `REAL_REA_REQUIRE` (a comma list such as `kuna,ghidra`) names this decompiler: the
/// workflow sets it per system, so a test that would be skipped fails the job instead.
fn required(decompiler: Decompiler, list: Option<&str>) -> bool {
    list.unwrap_or_default()
        .split(',')
        .any(|d| d.trim().eq_ignore_ascii_case(decompiler.label()))
}

/// Where the test puts the pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    /// A folder with no element whose name starts with a dot, like the app's own folder on
    /// Windows (`AppData\Local\<id>`) and macOS (`Library/Application Support/<id>`).
    Plain,
    /// Under `.local/share`, like the app's own folder on Linux.
    DotFolder,
}

/// Whether a path has a folder whose name starts with a dot (rea then moves Ghidra's session
/// out of the temporary folder it was given).
fn dotted(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, PathPart::Normal(n) if n.to_string_lossy().starts_with('.')))
}

/// The system's temporary folders: `std::env::temp_dir()`, and on Unix also `/tmp`, where
/// rea's fallbacks point.
fn system_temp_dirs() -> BTreeSet<PathBuf> {
    let mut dirs = vec![std::env::temp_dir()];
    if cfg!(unix) {
        dirs.push(PathBuf::from("/tmp"));
    }
    dirs.into_iter()
        .filter_map(|d| dunce::canonicalize(d).ok())
        .collect()
}

/// Whether a name in a temporary folder is rea's, Kuna's or Ghidra's: rea names its private
/// folders `rea-…` or `.rea-…` (`PrivateRuntimeRoot`, `mkdtemp` in its package), and anything
/// named after Kuna, Ghidra or JNA (Java's native-library loader that Ghidra uses).
fn rea_temp_name(name: &str) -> bool {
    let name = name.to_lowercase();
    name.starts_with("rea-")
        || name.starts_with(".rea-")
        || ["kuna", "ghidra", "jna"].iter().any(|w| name.contains(w))
}

/// rea's, Kuna's and Ghidra's entries in the system's temporary folders, plus the entries of
/// Java's performance counters (`hsperfdata_<user>/<pid>`, which the JVM writes to the
/// system's temporary folder whatever its own temporary folder is).
fn temp_entries(dirs: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for dir in dirs {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("hsperfdata_") {
                for f in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    out.insert(f.path());
                }
            } else if rea_temp_name(&name) {
                out.insert(e.path());
            }
        }
    }
    out
}

/// Whether an entry is one of Java's performance counters.
fn java_counters(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n.to_string_lossy().starts_with("hsperfdata_"))
}

/// Watches the system's temporary folders while rea runs: every 200 ms, and at each phase,
/// it notes each new entry of rea's, Kuna's, Ghidra's or Java's, with the phase it was first
/// seen in. It reads folder listings only.
struct TempWatch {
    dirs: Arc<BTreeSet<PathBuf>>,
    before: Arc<BTreeSet<PathBuf>>,
    seen: Arc<Mutex<BTreeMap<PathBuf, &'static str>>>,
    phase: Arc<Mutex<&'static str>>,
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl TempWatch {
    fn start() -> Self {
        let dirs = Arc::new(system_temp_dirs());
        let before = Arc::new(temp_entries(&dirs));
        let seen = Arc::new(Mutex::new(BTreeMap::new()));
        let phase = Arc::new(Mutex::new("start"));
        let stop = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn({
            let (dirs, before, seen, phase, stop) = (
                dirs.clone(),
                before.clone(),
                seen.clone(),
                phase.clone(),
                stop.clone(),
            );
            async move {
                while !stop.load(Ordering::SeqCst) {
                    note(&dirs, &before, &seen, &phase);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        });
        Self {
            dirs,
            before,
            seen,
            phase,
            stop,
            task,
        }
    }

    fn phase(&self, name: &'static str) {
        note(&self.dirs, &self.before, &self.seen, &self.phase);
        *self.phase.lock().unwrap() = name;
    }

    /// Everything that appeared during the run (with its phase), and what is still there.
    async fn finish(self) -> (BTreeMap<PathBuf, &'static str>, BTreeSet<PathBuf>) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.task.await;
        let left = note(&self.dirs, &self.before, &self.seen, &self.phase);
        let seen = self.seen.lock().unwrap().clone();
        (seen, left)
    }
}

/// Notes the new entries now, and returns them.
fn note(
    dirs: &BTreeSet<PathBuf>,
    before: &BTreeSet<PathBuf>,
    seen: &Mutex<BTreeMap<PathBuf, &'static str>>,
    phase: &Mutex<&'static str>,
) -> BTreeSet<PathBuf> {
    let new: BTreeSet<PathBuf> = temp_entries(dirs).difference(before).cloned().collect();
    let phase = *phase.lock().unwrap();
    let mut seen = seen.lock().unwrap();
    for p in &new {
        seen.entry(p.clone()).or_insert(phase);
    }
    new
}

/// Every file under `dir` with its size (relative paths with `/`).
fn listing(dir: &Path) -> BTreeMap<String, u64> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, u64>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if meta.is_dir() {
                out.insert(format!("{rel}/"), 0);
                walk(base, &path, out);
            } else {
                out.insert(rel, meta.len());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// A part folder's files (sizes), or the files of a folder outside the pack, for "nothing
/// changed here" checks.
fn snapshot(dirs: &[PathBuf]) -> BTreeMap<PathBuf, BTreeMap<String, u64>> {
    dirs.iter().map(|d| (d.clone(), listing(d))).collect()
}

/// Every file of a folder with its size and modification time.
type FolderState = Vec<(String, u64, Option<SystemTime>)>;

/// What the runner's own folders look like: every file with its size and modification time,
/// at most 50,000 per folder.
fn home_state(path: &Path) -> Option<FolderState> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return Some(vec![(String::new(), meta.len(), meta.modified().ok())]);
    }
    let mut out = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if out.len() >= 50_000 {
                return Some(out);
            }
            let p = e.path();
            let Ok(m) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if m.is_dir() {
                stack.push(p.clone());
            }
            let rel = p.strip_prefix(path).unwrap_or(&p).display().to_string();
            out.push((rel, m.len(), m.modified().ok()));
        }
    }
    out.sort();
    Some(out)
}

/// How often a settings file names rea (rea's own `setup` would add an MCP entry there).
fn rea_mentions(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|t| t.matches("rea-agents").count() + t.matches("rea.mjs").count())
        .unwrap_or(0)
}

struct Home {
    strict: Vec<(PathBuf, Option<FolderState>)>,
    mentions: Vec<(PathBuf, usize)>,
}

fn home_before() -> Option<Home> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    let ci = std::env::var_os("CI").is_some();
    let mut strict: Vec<PathBuf> = [
        ".cache/rea",
        "Library/Caches/rea",
        ".codex",
        ".cursor",
        ".gemini",
    ]
    .iter()
    .map(|p| home.join(p))
    .collect();
    let settings = [
        ".claude.json",
        ".claude/settings.json",
        ".claude/settings.local.json",
        ".codex/config.toml",
        ".cursor/mcp.json",
        ".gemini/settings.json",
    ];
    if ci {
        strict.push(home.join(".claude"));
        strict.push(home.join(".claude.json"));
    }
    Some(Home {
        strict: strict
            .into_iter()
            .map(|p| {
                let s = home_state(&p);
                (p, s)
            })
            .collect(),
        mentions: settings
            .iter()
            .map(|p| home.join(p))
            .map(|p| {
                let n = rea_mentions(&p);
                (p, n)
            })
            .collect(),
    })
}

fn home_unchanged(before: &Home) {
    for (path, state) in &before.strict {
        assert!(
            &home_state(path) == state,
            "rea changed the runner's {}",
            path.display()
        );
    }
    for (path, n) in &before.mentions {
        assert_eq!(
            rea_mentions(path),
            *n,
            "the runner's {} names rea now",
            path.display()
        );
    }
}

/// Absolute paths a plain text names (POSIX, Windows drive and UNC forms).
fn absolute_paths(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let stop = |c: char| c.is_whitespace() || "\"'`,;)]}>|".contains(c);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let before = if i == 0 { ' ' } else { chars[i - 1] };
        let boundary = stop(before) || "([{=:<".contains(before);
        let next = |k: usize| chars.get(i + k).copied().unwrap_or(' ');
        let posix = chars[i] == '/' && boundary && !stop(next(1)) && next(1) != '/';
        let drive = chars[i].is_ascii_alphabetic()
            && boundary
            && next(1) == ':'
            && (next(2) == '\\' || next(2) == '/');
        let unc = chars[i] == '\\' && boundary && next(1) == '\\';
        if posix || drive || unc {
            let start = i;
            while i < chars.len() && !stop(chars[i]) {
                i += 1;
            }
            let p: String = chars[start..i].iter().collect();
            out.push(p.trim_end_matches(['.', ':']).to_string());
        } else {
            i += 1;
        }
    }
    out
}

/// Every string a result holds: its text blocks (each JSON value's strings and keys when the
/// block is JSON, as rea's are) and the structured content.
fn strings_of(out: &ToolOutput) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Value::Object(map) => {
                for (k, v) in map {
                    out.push(k.clone());
                    walk(v, out);
                }
            }
            _ => {}
        }
    }
    let mut all = Vec::new();
    let text = out.joined_text();
    match serde_json::from_str::<Value>(&text) {
        Ok(v) => walk(&v, &mut all),
        Err(_) => {
            for line in text.lines() {
                match serde_json::from_str::<Value>(line) {
                    Ok(v) => walk(&v, &mut all),
                    Err(_) => all.push(line.to_string()),
                }
            }
        }
    }
    if let Some(ui) = &out.ui {
        walk(ui, &mut all);
    }
    all
}

/// Whether `path` is written in rea's own package (a constant of its code or texts), as is
/// or JSON-escaped.
fn in_package(rea_dir: &Path, path: &str) -> bool {
    let escaped = path.replace('\\', "\\\\");
    let mut stack = vec![rea_dir.join("dist")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .is_some_and(|x| x == "js" || x == "json" || x == "mjs")
                && std::fs::read_to_string(&p)
                    .is_ok_and(|t| t.contains(path) || t.contains(&escaped))
            {
                return true;
            }
        }
    }
    false
}

fn text_of(out: &ToolOutput) -> String {
    let mut all = out.joined_text();
    if let Some(ui) = &out.ui {
        all.push('\n');
        all.push_str(&ui.to_string());
    }
    all
}

/// A result for the log: the first 500 characters, or all of it with `REAL_REA_VERBOSE`.
fn short(text: &str) -> String {
    let limit = if std::env::var_os("REAL_REA_VERBOSE").is_some() {
        usize::MAX
    } else {
        500
    };
    text.chars()
        .take(limit)
        .collect::<String>()
        .replace('\n', " | ")
}

/// Calls `tool` the way the `reverse` server forwards it: the arguments given plus the
/// tool's fixed `detail`, its time limit, and the app's list of what may be called.
async fn call(guard: &mut ReaGuard, tool: &str, args: Value) -> ToolOutput {
    call_with(guard, tool, args, None).await
}

/// As `call`, with another time limit (a larger program's analysis).
async fn call_with(
    guard: &mut ReaGuard,
    tool: &str,
    args: Value,
    timeout: Option<Duration>,
) -> ToolOutput {
    let t = allow::get(tool)
        .map(|t| (t.fixed_detail, t.timeout))
        .unwrap_or((None, Duration::from_secs(180)));
    let mut args: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
    if let Some(detail) = t.0 {
        args.insert("detail".into(), json!(detail));
    }
    let started = Instant::now();
    let out = guard
        .call(
            &allow::app_list(),
            tool,
            args,
            timeout.unwrap_or(t.1),
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    println!(
        "real_rea: {tool} ({:.1} s, error {}): {}",
        started.elapsed().as_secs_f64(),
        out.is_error,
        short(&out.joined_text())
    );
    out
}

/// The processes (pid and program) running a program from under `root`, zombies left out
/// (Linux; reads /proc).
fn running_from(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for p in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(exe) = std::fs::read_link(p.path().join("exe")) else {
            continue;
        };
        let zombie = std::fs::read_to_string(p.path().join("stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(')')
                    .map(|(_, rest)| rest.trim_start().starts_with('Z'))
            })
            .unwrap_or(true);
        if exe.starts_with(root) && !zombie {
            out.push((p.file_name().to_string_lossy().into_owned(), exe));
        }
    }
    out
}

/// The highest peak resident memory (`VmHWM`) of any process running a program from under
/// `root`, sampled until `stop` (Linux only; reads /proc, starts nothing).
async fn peak_memory(root: PathBuf, stop: Arc<AtomicBool>, peak: Arc<AtomicU64>) {
    while !stop.load(Ordering::SeqCst) {
        if let Ok(procs) = std::fs::read_dir("/proc") {
            for p in procs.flatten() {
                let exe = std::fs::read_link(p.path().join("exe")).unwrap_or_default();
                if !exe.starts_with(&root) {
                    continue;
                }
                let status = std::fs::read_to_string(p.path().join("status")).unwrap_or_default();
                if let Some(kb) = status
                    .lines()
                    .find_map(|l| l.strip_prefix("VmHWM:"))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
                    && exe.file_name().is_some_and(|n| n == "java")
                {
                    peak.fetch_max(kb * 1024, Ordering::SeqCst);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// The larger program `REAL_REA_LARGE` names: `decompile` for the pack's own Ghidra
/// decompiler for this system (a C++ program of a few megabytes), or a program's absolute
/// path.
fn large_program(ghidra_dir: Option<&Path>) -> Option<PathBuf> {
    let wanted = std::env::var_os("REAL_REA_LARGE")?;
    if wanted == "decompile" {
        let os = ghidra_dir?.join("Ghidra/Features/Decompiler/os");
        let own = if cfg!(windows) {
            "decompile.exe"
        } else {
            "decompile"
        };
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&os)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        dirs.sort();
        let found = dirs.iter().map(|d| d.join(own)).find(|p| p.is_file());
        assert!(
            found.is_some(),
            "no decompile program under {}",
            os.display()
        );
        return found;
    }
    let path = PathBuf::from(wanted);
    assert!(
        path.is_absolute() && path.is_file(),
        "REAL_REA_LARGE: {} is not a program file",
        path.display()
    );
    Some(path)
}

/// The pack, the fixtures and anything rea writes go into `base`: a temporary folder named
/// `real-rea-…`, or `REAL_REA_DIR/<kuna|ghidra>`; with `Base::DotFolder`, its `.local/share`.
fn make_base(decompiler: Decompiler, kind: Base) -> (Option<tempfile::TempDir>, PathBuf) {
    let label = decompiler.label().to_lowercase();
    let (temp, top) = match std::env::var_os("REAL_REA_DIR") {
        Some(dir) => {
            let name = match kind {
                Base::Plain => label,
                Base::DotFolder => format!("{label}-dot"),
            };
            let top = PathBuf::from(dir).join(name);
            let _ = std::fs::remove_dir_all(&top);
            std::fs::create_dir_all(&top).unwrap();
            (None, top)
        }
        None => {
            let dir = tempfile::Builder::new()
                .prefix("real-rea-")
                .tempdir()
                .unwrap();
            let top = dir.path().to_path_buf();
            (Some(dir), top)
        }
    };
    let top = dunce::canonicalize(&top).unwrap();
    assert!(
        !dotted(&top),
        "the test's folder {} has a folder whose name starts with a dot, so rea would move \
         Ghidra's session out of the pack; set REAL_REA_DIR or TMPDIR to a folder without one",
        top.display()
    );
    let base = match kind {
        Base::Plain => top,
        Base::DotFolder => {
            let base = top.join(".local").join("share");
            std::fs::create_dir_all(&base).unwrap();
            base
        }
    };
    (temp, base)
}

async fn end_to_end(decompiler: Decompiler, kind: Base) {
    let (cache, platform) = match needs(decompiler) {
        Ok(found) => found,
        Err(why) => {
            let list = std::env::var("REAL_REA_REQUIRE").ok();
            assert!(
                !required(decompiler, list.as_deref()),
                "real_rea: {}: {why}, but REAL_REA_REQUIRE names it",
                decompiler.label()
            );
            println!("real_rea: {}: {why}; skipped", decompiler.label());
            return;
        }
    };
    let _one_at_a_time = SERIAL.lock().await;
    let full = kind == Base::Plain;
    let (_temp, base) = make_base(decompiler, kind);
    println!("real_rea: the pack goes under {}", base.display());
    let root = base.join("reverse");
    let files = base.join("files");
    std::fs::create_dir_all(&files).unwrap();
    for name in FIXTURE_FILES {
        std::fs::copy(fixtures_dir().join(name), files.join(name)).unwrap();
    }
    let layout = ReverseLayout::new(&root);
    let lock = reverse::lock::embedded();

    // Install through the real unpacker.
    let installer =
        Installer::new(layout.clone(), platform).with_fetch(Arc::new(DirFetch::new(&cache)));
    let started = Instant::now();
    let installed = installer
        .download(decompiler.component(), &CancellationToken::new(), &|_| {})
        .await
        .unwrap_or_else(|e| panic!("install rea and {}: {e}", decompiler.label()));
    println!(
        "real_rea: installed rea and {} in {:.1} s",
        decompiler.label(),
        started.elapsed().as_secs_f64()
    );
    for c in [Component::Rea, decompiler.component()] {
        assert_eq!(installer.state(c), ComponentState::Installed, "{c:?}");
    }
    let installation = installer.installation();
    let mut part_dirs = Vec::new();
    for row in [installation.rea.as_ref(), Some(&installed)]
        .into_iter()
        .flatten()
    {
        for part in &row.parts {
            let pinned = lock.part(&part.name).unwrap().build_for(platform).unwrap();
            assert_eq!(part.disk_bytes, pinned.unpacked_bytes, "{}", part.name);
            for p in &pinned.programs {
                let path = part.dir.join(&p.path);
                let meta =
                    std::fs::metadata(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert_eq!(meta.len(), p.size, "{}", p.path);
                #[cfg(unix)]
                if p.exec {
                    use std::os::unix::fs::PermissionsExt;
                    assert!(
                        meta.permissions().mode() & 0o100 != 0,
                        "{} is not executable",
                        p.path
                    );
                }
            }
            part_dirs.push(part.dir.clone());
        }
    }
    part_dirs.sort();
    part_dirs.dedup();
    // A larger program for Ghidra's memory figure, copied beside the fixtures before the
    // "nothing changed here" snapshot.
    let ghidra_dir = installed.part("ghidra").map(|p| p.dir.clone());
    let large = (full && decompiler == Decompiler::Ghidra)
        .then(|| large_program(ghidra_dir.as_deref()))
        .flatten()
        .map(|from| {
            let to = files.join(format!(
                "large-{}",
                from.file_name().unwrap().to_string_lossy()
            ));
            std::fs::copy(&from, &to).unwrap();
            to
        });
    let mut watched = part_dirs.clone();
    watched.push(files.clone());
    let before = snapshot(&watched);
    let home = home_before();
    let temp = TempWatch::start();

    // Start rea with the app's own environment.
    let process = ReaProcess::new(layout.clone(), platform);
    process.set_log_dir(Some(&base));
    let config = env::run_config(
        &layout,
        &installation,
        Some(decompiler),
        Flavor::current(),
        true,
    );
    let mut guard = process.lock().await;
    let started = Instant::now();
    let rea = guard
        .ensure_started(config.clone())
        .await
        .unwrap_or_else(|e| panic!("rea did not start: {e}"));
    let first_start = started.elapsed();
    assert!(rea.fresh);

    // Every allow-listed tool as reviewed; the turned-off families are not there.
    let rea_version = &lock.part("rea").unwrap().version;
    let live = allow::live_check(rea_version, &rea.tools);
    assert!(
        live.changed.is_empty() && live.missing.is_empty(),
        "live check: {live:?}"
    );
    assert!(live.housekeeping_ok());
    let fixture = allow::fixture(rea_version).expect("the pinned fixture");
    let kept: Vec<&str> = lock.families.split(',').collect();
    let listed: Vec<&str> = rea.tools.iter().map(|(n, _)| n.as_str()).collect();
    let mut enabled_not_offered = 0;
    for (name, family) in fixture["tool_families"].as_object().unwrap() {
        match family.as_str() {
            Some(f) if !kept.contains(&f) => assert!(
                !listed.contains(&name.as_str()),
                "{name} (family {f}) is listed although its family is off"
            ),
            Some(_)
                if listed.contains(&name.as_str())
                    && !allow::MODEL_TOOLS.contains(&name.as_str())
                    && !allow::APP_ONLY_TOOLS.contains(&name.as_str()) =>
            {
                enabled_not_offered += 1;
            }
            _ => {}
        }
    }
    println!(
        "real_rea: rea lists {} tools; {} of them are not offered by the app and stay enabled inside rea",
        listed.len(),
        enabled_not_offered
    );

    // Open the test program, find its function, read its code.
    temp.phase("open and analysis");
    let (target, function) = open_target();
    println!("real_rea: opening {target}, looking for {function}");
    let hello = files.join(target);
    let stop_sampling = Arc::new(AtomicBool::new(false));
    let peak = Arc::new(AtomicU64::new(0));
    let sampler = (cfg!(target_os = "linux") && decompiler == Decompiler::Ghidra).then(|| {
        tokio::spawn(peak_memory(
            root.clone(),
            stop_sampling.clone(),
            peak.clone(),
        ))
    });
    let started = Instant::now();
    let opened = call(
        &mut guard,
        "open_binary",
        json!({"path": hello.display().to_string()}),
    )
    .await;
    assert!(!opened.is_error, "{}", opened.joined_text());
    // The decompiler works the file out at the first step that needs it (Ghidra's
    // auto-analysis), so the open is timed with that step.
    let listed = call(&mut guard, "list_procedures", json!({})).await;
    let open_time = started.elapsed();
    assert!(!listed.is_error, "{}", listed.joined_text());
    // The summary is a receipt; the list itself is its parent, read through a view.
    let receipt: Value = serde_json::from_str(&listed.joined_text()).expect("rea's JSON");
    let parent = receipt["parameters"]["parent_evidence_id"]
        .as_str()
        .expect("the list's id")
        .to_string();
    assert!(results::is_evidence_id(&parent), "{parent}");
    let page = call(
        &mut guard,
        "inspect_analysis_view",
        json!({"source": {"kind": "retained-evidence", "evidence_id": parent}, "view": {"kind": "native-inventory", "offset": 0, "limit": 64}}),
    )
    .await;
    assert!(!page.is_error, "{}", page.joined_text());
    assert!(
        text_of(&page).contains(&format!("\"{function}\"")),
        "{function} is not among the functions: {}",
        short(&text_of(&page))
    );
    let pseudo = call(
        &mut guard,
        "procedure_pseudo_code",
        json!({"procedure": function}),
    )
    .await;
    assert!(!pseudo.is_error, "{}", pseudo.joined_text());
    assert!(
        pseudo.joined_text().contains(function) && pseudo.joined_text().trim().len() > 20,
        "{}",
        pseudo.joined_text()
    );
    stop_sampling.store(true, Ordering::SeqCst);
    if let Some(s) = sampler {
        let _ = s.await;
    }

    let mut second_start = None;
    let mut large_figures = None;
    if full {
        // binary_session names nothing outside the pack and the fixtures.
        temp.phase("binary_session");
        let session = call(&mut guard, "binary_session", json!({})).await;
        assert!(!session.is_error, "{}", session.joined_text());
        let mut outside: Vec<String> = strings_of(&session)
            .iter()
            .flat_map(|s| absolute_paths(s))
            .filter(|p| {
                let p = Path::new(p);
                !p.starts_with(&root) && !p.starts_with(&files)
            })
            .collect();
        outside.sort();
        outside.dedup();
        // rea's own constants are not about this computer: its remediation texts name
        // `/usr/bin/prlimit` for tools the app does not offer, and its list of provider
        // candidates names Hopper's default launcher (checked, never started:
        // REA_ANALYSIS_PROVIDER is fixed).
        let rea_dir = installation
            .rea
            .as_ref()
            .and_then(|r| r.part("rea"))
            .map(|p| p.dir.clone())
            .expect("rea's folder");
        let (constants, unexplained): (Vec<String>, Vec<String>) =
            outside.into_iter().partition(|p| in_package(&rea_dir, p));
        println!("real_rea: binary_session names rea's own constant paths {constants:?}");
        assert!(
            unexplained.is_empty(),
            "binary_session names paths outside the pack and the fixtures: {unexplained:?}"
        );

        // The file checks the app offers, on the three fixtures.
        temp.phase("file checks");
        let imports = call(
            &mut guard,
            "inspect_pe_imports",
            json!({"path": files.join("hello-win-x64.exe").display().to_string()}),
        )
        .await;
        assert!(!imports.is_error, "{}", imports.joined_text());
        let managed = call(
            &mut guard,
            "inspect_managed_members",
            json!({"path": files.join("hello-dotnet.dll").display().to_string()}),
        )
        .await;
        assert!(!managed.is_error, "{}", managed.joined_text());
        let exports = call(
            &mut guard,
            "inspect_native_exports",
            json!({"path": hello.display().to_string()}),
        )
        .await;
        assert!(!exports.is_error, "{}", exports.joined_text());

        // A larger program: how long Ghidra takes and how much memory Java uses.
        if let Some(large) = &large {
            temp.phase("larger program");
            let size = std::fs::metadata(large).unwrap().len();
            let stop = Arc::new(AtomicBool::new(false));
            let peak = Arc::new(AtomicU64::new(0));
            let sampler = cfg!(target_os = "linux")
                .then(|| tokio::spawn(peak_memory(root.clone(), stop.clone(), peak.clone())));
            let started = Instant::now();
            let opened = call(
                &mut guard,
                "open_binary",
                json!({"path": large.display().to_string()}),
            )
            .await;
            assert!(!opened.is_error, "{}", opened.joined_text());
            let listed = call_with(
                &mut guard,
                "list_procedures",
                json!({}),
                Some(Duration::from_secs(1800)),
            )
            .await;
            assert!(!listed.is_error, "{}", listed.joined_text());
            let took = started.elapsed();
            stop.store(true, Ordering::SeqCst);
            if let Some(s) = sampler {
                let _ = s.await;
            }
            large_figures = Some((size, took, peak.load(Ordering::SeqCst)));
        }
    }
    let closed = call(&mut guard, "close_binary", json!({})).await;
    assert!(!closed.is_error, "{}", closed.joined_text());
    drop(guard);

    if full {
        // A stop, then a second start.
        temp.phase("second start");
        process.stop(STOP_GRACE).await;
        assert_eq!(process.state(), RunState::Stopped);
        let mut guard = process.lock().await;
        let mut again = config.clone();
        again.start_timeout = env::START_TIMEOUT;
        let started = Instant::now();
        let second = guard
            .ensure_started(again)
            .await
            .unwrap_or_else(|e| panic!("rea did not start again: {e}"));
        second_start = Some(started.elapsed());
        assert!(second.fresh && second.generation > rea.generation);
        // Stop in the middle of Ghidra's analysis (Java running): the call ends as cancelled.
        if decompiler == Decompiler::Ghidra {
            temp.phase("a call stopped during the analysis");
            let opened = call(
                &mut guard,
                "open_binary",
                json!({"path": hello.display().to_string()}),
            )
            .await;
            assert!(!opened.is_error, "{}", opened.joined_text());
            let cancel = CancellationToken::new();
            let stop = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                stop.cancel();
            });
            let started = Instant::now();
            let mut args = Map::new();
            args.insert("detail".into(), json!("summary"));
            let outcome = guard
                .call(
                    &allow::app_list(),
                    "list_procedures",
                    args,
                    Duration::from_secs(600),
                    &cancel,
                    None,
                )
                .await;
            println!(
                "real_rea: list_procedures stopped after 3 s: {:?} after {:.1} s",
                outcome.as_ref().map(|o| o.is_error),
                started.elapsed().as_secs_f64()
            );
            match outcome {
                Err(reverse::ReverseError::Cancelled) => {}
                Ok(_) => {
                    println!("real_rea: the analysis finished within 3 s; nothing was stopped")
                }
                Err(other) => panic!("a stopped call ended as {other}"),
            }
        }
        drop(guard);
    }
    temp.phase("stop");
    process.restart(RestartReason::Quit).await;
    assert_eq!(process.state(), RunState::Stopped);
    // After the stop nothing from the pack runs any more: not rea, not Kuna, not Java or
    // Ghidra's decompiler (Linux reads /proc; the workflow checks the other systems).
    if cfg!(target_os = "linux") {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = running_from(&root);
            if left.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "still running after the stop: {left:?}"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    println!(
        "real_rea: {} on {}: first start {:.1} s{}, open and first analysis {:.1} s{}",
        decompiler.label(),
        platform.key(),
        first_start.as_secs_f64(),
        second_start
            .map(|t| format!(", second start {:.1} s", t.as_secs_f64()))
            .unwrap_or_default(),
        open_time.as_secs_f64(),
        match peak.load(Ordering::SeqCst) {
            0 => String::new(),
            bytes => format!(", Java's peak memory {} MB", bytes / 1_000_000),
        }
    );
    if let Some((size, took, peak)) = large_figures {
        println!(
            "real_rea: a larger program ({} KB) opened and analysed by Ghidra in {:.1} s{}",
            size / 1000,
            took.as_secs_f64(),
            match peak {
                0 => String::new(),
                bytes => format!(", Java's peak memory {} MB", bytes / 1_000_000),
            }
        );
    }

    // Nothing of rea's appeared in the system's temporary folders (watched during the run),
    // and nothing is left there after the stop.
    let (appeared, left) = temp.finish().await;
    let (java, others): (Vec<_>, Vec<_>) = appeared.iter().partition(|(p, _)| java_counters(p));
    if !java.is_empty() {
        println!(
            "real_rea: Java's performance counters in the system's temporary folder (the JVM \
             writes them there whatever its own temporary folder is, and removes them when it \
             ends): {java:?}"
        );
    }
    let left_over: Vec<&PathBuf> = left.iter().filter(|p| !java_counters(p)).collect();
    assert!(
        left_over.is_empty(),
        "left in the system's temporary folder after the stop: {left_over:?}"
    );
    match kind {
        Base::Plain => assert!(
            others.is_empty(),
            "rea wrote outside the pack, into the system's temporary folder (entry and the \
             phase it appeared in): {others:?}. A `rea-ghidra-socket-` entry means the socket \
             path inside the pack was longer than 106 bytes (use a shorter REAL_REA_DIR)"
        ),
        Base::DotFolder if !others.is_empty() => println!(
            "real_rea: KNOWN LIMIT: with the pack under a folder whose name starts with a dot \
             (the app's own folder on Linux is under ~/.local/share), rea puts Ghidra's session \
             (its project, logs, a copy of the opened file and the bridge socket) in the \
             system's temporary folder instead of the pack's tmp; it was removed at the stop: \
             {others:?}"
        ),
        Base::DotFolder => println!(
            "real_rea: with the pack under a folder whose name starts with a dot, nothing of \
             rea's appeared in the system's temporary folder"
        ),
    }

    // Everything rea wrote is under home, tmp, cache and work.
    let after = snapshot(&watched);
    for dir in &watched {
        let (b, a) = (&before[dir], &after[dir]);
        let added: Vec<_> = a.keys().filter(|k| !b.contains_key(*k)).collect();
        let changed: Vec<_> = b.iter().filter(|(k, v)| a.get(*k) != Some(v)).collect();
        assert!(
            added.is_empty() && changed.is_empty(),
            "rea wrote into {}: added {added:?}, changed or removed {changed:?}",
            dir.display()
        );
    }
    let top: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let allowed: Vec<String> = part_dirs
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .chain(
            ["home", "tmp", "cache", "work", ".downloads"]
                .iter()
                .map(|s| s.to_string()),
        )
        .collect();
    let stray: Vec<&String> = top.iter().filter(|n| !allowed.contains(n)).collect();
    assert!(stray.is_empty(), "rea wrote {stray:?} into the pack's root");
    let base_top: Vec<String> = std::fs::read_dir(&base)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "reverse" && n != "files" && n != reverse::process::LOG_FILE_NAME)
        .collect();
    assert!(
        base_top.is_empty(),
        "something wrote {base_top:?} beside the pack"
    );
    if let Some(home) = &home {
        home_unchanged(home);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rea_with_kuna() {
    end_to_end(Decompiler::Kuna, Base::Plain).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rea_with_ghidra() {
    end_to_end(Decompiler::Ghidra, Base::Plain).await;
}

/// The app's own folder on Linux is under `~/.local/share`: rea then moves Ghidra's session to
/// `/tmp`. Whatever it puts there must be gone after the stop (the test prints what it saw).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rea_with_ghidra_under_a_dot_folder() {
    end_to_end(Decompiler::Ghidra, Base::DotFolder).await;
}

#[test]
fn paths_are_found_in_rea_text() {
    let text = r"see /tmp/a b and C:\Users\ann\x.exe, (\\server\share\f) or https://example.com/p and/or /usr/bin/ok.";
    assert_eq!(
        absolute_paths(text),
        [
            "/tmp/a",
            r"C:\Users\ann\x.exe",
            r"\\server\share\f",
            "/usr/bin/ok"
        ]
    );
    assert!(absolute_paths("no paths here, and/or 3/4").is_empty());
    let out = ToolOutput::text(r#"{"session":{"path":"/srv/x","note":"in C:\\A\\b"}}"#);
    let mut found: Vec<String> = strings_of(&out)
        .iter()
        .flat_map(|s| absolute_paths(s))
        .collect();
    found.sort();
    assert_eq!(found, ["/srv/x", r"C:\A\b"]);
}

#[test]
fn the_required_list_and_the_temporary_names() {
    assert!(required(Decompiler::Ghidra, Some("kuna, ghidra")));
    assert!(required(Decompiler::Kuna, Some("Kuna")));
    assert!(!required(Decompiler::Ghidra, Some("kuna")));
    assert!(!required(Decompiler::Kuna, None));
    assert!(!required(Decompiler::Kuna, Some("")));
    for name in [
        "rea-ghidra-Skj3tD",
        "rea-ghidra-socket-AbC123",
        ".rea-config-x",
        "rea-kuna-1",
        "ghidra_root_Temp",
        "jna-12345",
        "Kuna.lock",
    ] {
        assert!(rea_temp_name(name), "{name}");
    }
    for name in [
        "real-rea-abc",
        "thread-dump",
        "systemd-private-1",
        "area-51",
    ] {
        assert!(!rea_temp_name(name), "{name}");
    }
    assert!(java_counters(Path::new("/tmp/hsperfdata_root/1234")));
    assert!(!java_counters(Path::new("/tmp/rea-ghidra-x")));
    assert!(dotted(Path::new("/home/u/.local/share/x/reverse")));
    assert!(!dotted(Path::new("/tmp/real-rea-abc/reverse")));
    assert!(!dotted(Path::new("/tmp/a.b/reverse")));
}
