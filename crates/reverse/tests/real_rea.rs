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
//!   memory of Ghidra's Java on Linux while it opens the file;
//! - checks that everything rea wrote is under the pack's `home`, `tmp`, `cache` and `work`
//!   (the part folders and the fixtures' folder are as they were), and that rea added
//!   nothing to the runner's `~/.cache/rea`, `~/Library/Caches/rea`, `~/.codex`, `~/.cursor`,
//!   `~/.gemini`, `~/.claude` or `~/.claude.json`. `~/.claude` and `~/.claude.json` are
//!   compared in full only when `CI` is set (on a developer's computer Claude Code itself
//!   writes there while the test runs); otherwise their settings files must not newly name
//!   rea.
//!
//! The test starts no process itself (`scripts/check-spawn.sh`): rea starts through
//! `crates/runtime`, and the workflow (`.github/workflows/reverse.yml`) adds the process and
//! network checks around it. The two tests take turns: one rea, and Ghidra's 2 GB heap, at a
//! time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// What the runner's own folders look like: every file with its size and modification time,
/// at most 50,000 per folder.
fn home_state(path: &Path) -> Option<Vec<(String, u64, Option<SystemTime>)>> {
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
    strict: Vec<(PathBuf, Option<Vec<(String, u64, Option<SystemTime>)>>)>,
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
    text.chars().take(limit).collect::<String>().replace('\n', " | ")
}

/// Calls `tool` the way the `reverse` server forwards it: the arguments given plus the
/// tool's fixed `detail`, its time limit, and the app's list of what may be called.
async fn call(guard: &mut ReaGuard, tool: &str, args: Value) -> ToolOutput {
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
            t.1,
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

async fn end_to_end(decompiler: Decompiler) {
    let (cache, platform) = match needs(decompiler) {
        Ok(found) => found,
        Err(why) => {
            println!("real_rea: {}: {why}; skipped", decompiler.label());
            return;
        }
    };
    let _one_at_a_time = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(dir.path()).unwrap();
    let root = base.join("reverse");
    let files = base.join("files");
    std::fs::create_dir_all(&files).unwrap();
    for name in FIXTURE_FILES {
        std::fs::copy(fixtures_dir().join(name), files.join(name)).unwrap();
    }
    let layout = ReverseLayout::new(&root);
    let lock = reverse::lock::embedded();

    // Install through the real unpacker.
    let installer = Installer::new(layout.clone(), platform)
        .with_fetch(Arc::new(DirFetch::new(&cache)));
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
                let meta = std::fs::metadata(&path)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
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
    let mut watched = part_dirs.clone();
    watched.push(files.clone());
    let before = snapshot(&watched);
    let home = home_before();

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

    // Open the ELF, find main, read its code.
    let hello = files.join("hello-linux-x64");
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
        text_of(&page).contains("\"main\""),
        "main is not among the functions: {}",
        short(&text_of(&page))
    );
    let pseudo = call(
        &mut guard,
        "procedure_pseudo_code",
        json!({"procedure": "main"}),
    )
    .await;
    assert!(!pseudo.is_error, "{}", pseudo.joined_text());
    assert!(
        pseudo.joined_text().contains("main") && pseudo.joined_text().trim().len() > 20,
        "{}",
        pseudo.joined_text()
    );
    stop_sampling.store(true, Ordering::SeqCst);
    if let Some(s) = sampler {
        let _ = s.await;
    }

    // binary_session names nothing outside the pack and the fixtures.
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
    // `/usr/bin/prlimit` for tools the app does not offer, and its list of provider candidates
    // names Hopper's default launcher (checked, never started: REA_ANALYSIS_PROVIDER is fixed).
    let rea_dir = installation
        .rea
        .as_ref()
        .and_then(|r| r.part("rea"))
        .map(|p| p.dir.clone())
        .expect("rea's folder");
    let (constants, unexplained): (Vec<String>, Vec<String>) = outside
        .into_iter()
        .partition(|p| in_package(&rea_dir, p));
    println!("real_rea: binary_session names rea's own constant paths {constants:?}");
    assert!(
        unexplained.is_empty(),
        "binary_session names paths outside the pack and the fixtures: {unexplained:?}"
    );

    // The file checks the app offers, on the three fixtures.
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
    let closed = call(&mut guard, "close_binary", json!({})).await;
    assert!(!closed.is_error, "{}", closed.joined_text());
    drop(guard);

    // A stop, then a second start.
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
    let second_start = started.elapsed();
    assert!(second.fresh && second.generation > rea.generation);
    drop(guard);
    process.restart(RestartReason::Quit).await;
    assert_eq!(process.state(), RunState::Stopped);

    println!(
        "real_rea: {} on {}: first start {:.1} s, second start {:.1} s, open and first analysis {:.1} s{}",
        decompiler.label(),
        platform.key(),
        first_start.as_secs_f64(),
        second_start.as_secs_f64(),
        open_time.as_secs_f64(),
        match peak.load(Ordering::SeqCst) {
            0 => String::new(),
            bytes => format!(", Java's peak memory {} MB", bytes / 1_000_000),
        }
    );

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
    end_to_end(Decompiler::Kuna).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rea_with_ghidra() {
    end_to_end(Decompiler::Ghidra).await;
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
