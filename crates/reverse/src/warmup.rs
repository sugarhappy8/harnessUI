//! Prepares rea's macOS process helper before rea starts (F2, spec correction 12).
//!
//! rea 6.9.0's Kuna probe runs `kuna --version` under a 10 second deadline. On a fresh
//! `HOME` the first command rea owns compiles its Darwin process helper (cached under
//! `$HOME/Library/Caches/rea/process-helper/darwin`) inside that deadline, so Kuna shows as
//! unavailable. rea's own `prepareOwnedCommand` (`dist/process/OwnedCommand.js`) does that
//! compile first, under its own bound, and Ghidra's probe calls it; Kuna's does not. So
//! before each start of rea on macOS the app runs it: `node --eval` imports rea's module and
//! calls `prepareOwnedCommand()` with rea's own environment (so `HOME` and the cache are the
//! pack's), and rea then finds the helper cached. A failure or a timeout changes nothing for
//! the start, which goes on and reports Kuna exactly as before.
//!
//! The run is spawned through the runtime crate like every child, with a limit of
//! [`HELPER_PREPARE_TIMEOUT`], after which its whole process tree is killed.

use std::path::{Path, PathBuf};
use std::time::Duration;

use runtime::{SpawnSpec, StdioMode};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::env::{Flavor, PREPARE_ONLY_VARIABLE};
use crate::process::RunConfig;

/// Longest wait for the prepare run: rea bounds the compile at 120 s itself.
pub const HELPER_PREPARE_TIMEOUT: Duration = Duration::from_secs(180);

/// The script of the prepare run (an ES module: `import()` and top-level `await`).
pub const PREPARE_SCRIPT: &str =
    "const m = await import(process.env.REA_HELPER_MODULE); await m.prepareOwnedCommand();";

/// Lines of the prepare run's stderr kept for the log.
const TAIL_LINES: usize = 10;
/// Longest stretch of stderr read, and longest kept line.
const MAX_STDERR_BYTES: u64 = 256 * 1024;
const MAX_LINE_CHARS: usize = 400;
/// How long the end of stderr is awaited after the process has ended.
const STDERR_DRAIN: Duration = Duration::from_secs(2);

/// Why the prepare run did not succeed.
#[derive(Debug, thiserror::Error)]
pub enum WarmupError {
    /// It outlived its limit and was killed.
    #[error("timed out")]
    TimedOut,
    /// It ended with a failure (`code` is `None` when a signal ended it or it did not start).
    #[error("exited {code:?}")]
    Failed {
        /// The exit code.
        code: Option<i32>,
        /// The last lines it wrote to stderr.
        stderr_tail: Vec<String>,
    },
}

/// rea's `dist/process/OwnedCommand.js`, two folders up from `<rea>/scripts/rea.mjs`, when
/// it exists.
pub fn helper_module(rea_entry: &Path) -> Option<PathBuf> {
    let module = rea_entry
        .parent()?
        .parent()?
        .join("dist")
        .join("process")
        .join("OwnedCommand.js");
    module.is_file().then_some(module)
}

/// Whether rea's helper is already compiled in `config`'s `HOME` (a non-empty cache folder).
pub fn helper_cached(config: &RunConfig) -> bool {
    config.var("HOME").is_some_and(|home| {
        Path::new(home)
            .join("Library/Caches/rea/process-helper/darwin")
            .read_dir()
            .is_ok_and(|mut entries| entries.next().is_some())
    })
}

/// The prepare run for `config`: macOS only, and only when rea has the module. It is rea's
/// own configuration (node, working folder and the whole environment, so `HOME` and the
/// cache are rea's) with the script instead of rea's arguments, and the module's URL in
/// [`PREPARE_ONLY_VARIABLE`].
pub fn prepare_config(config: &RunConfig, rea_entry: &Path, flavor: Flavor) -> Option<RunConfig> {
    if flavor != Flavor::Macos {
        return None;
    }
    let module = helper_module(rea_entry)?;
    Some(RunConfig {
        program: config.program.clone(),
        args: vec![
            "--input-type=module".into(),
            "--eval".into(),
            PREPARE_SCRIPT.into(),
        ],
        env: config
            .env
            .clone()
            .set(PREPARE_ONLY_VARIABLE, file_url(&module)),
        cwd: config.cwd.clone(),
        start_timeout: config.start_timeout,
    })
}

/// `file://` URL of an absolute path, percent-encoded.
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file://");
    if !text.starts_with('/') {
        url.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(char::from(byte));
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// Runs rea's `prepareOwnedCommand` for `config` (see the module documentation). `Ok` also
/// when there is nothing to prepare (not macOS, or no module in this rea).
pub async fn prepare_process_helper(
    config: &RunConfig,
    rea_entry: &Path,
    flavor: Flavor,
) -> Result<(), WarmupError> {
    prepare_within(config, rea_entry, flavor, HELPER_PREPARE_TIMEOUT).await
}

pub(crate) async fn prepare_within(
    config: &RunConfig,
    rea_entry: &Path,
    flavor: Flavor,
    limit: Duration,
) -> Result<(), WarmupError> {
    let Some(prepare) = prepare_config(config, rea_entry, flavor) else {
        return Ok(());
    };
    let spec = SpawnSpec::new(&prepare.program, &prepare.cwd)
        .args(&prepare.args)
        .env(prepare.env.clone())
        .stdin(StdioMode::Null)
        .stdout(StdioMode::Null)
        .stderr(StdioMode::Piped);
    let mut child = runtime::spawn(spec).map_err(|e| WarmupError::Failed {
        code: None,
        stderr_tail: vec![format!("could not start: {}", e.kind())],
    })?;
    let reader = child.take_stderr().map(|stderr| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr.take(MAX_STDERR_BYTES)).lines();
            let mut tail: Vec<String> = Vec::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if tail.len() == TAIL_LINES {
                    tail.remove(0);
                }
                tail.push(line.chars().take(MAX_LINE_CHARS).collect());
            }
            tail
        })
    });
    let status = match tokio::time::timeout(limit, child.wait()).await {
        Ok(status) => status,
        Err(_) => {
            // The whole tree: the compiler rea started goes with it.
            let _ = child.kill().await;
            if let Some(reader) = reader {
                reader.abort();
            }
            return Err(WarmupError::TimedOut);
        }
    };
    let stderr_tail = match reader {
        Some(reader) => tokio::time::timeout(STDERR_DRAIN, reader)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(WarmupError::Failed {
            code: status.code(),
            stderr_tail,
        }),
        Err(e) => Err(WarmupError::Failed {
            code: None,
            stderr_tail: vec![format!("could not wait: {}", e.kind())],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(config: &RunConfig, name: &str) -> String {
        config.var(name).unwrap_or_default().to_owned()
    }

    /// A temp pack with `rea-6.9.0/scripts/rea.mjs` and `dist/process/OwnedCommand.js`, and
    /// the configuration that starts it (`node` is whatever `program` says).
    fn macos_config_with_pack(program: &Path) -> (tempfile::TempDir, RunConfig, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let rea = dir.path().join("rea-6.9.0");
        std::fs::create_dir_all(rea.join("scripts")).unwrap();
        std::fs::create_dir_all(rea.join("dist/process")).unwrap();
        std::fs::write(rea.join("scripts/rea.mjs"), "").unwrap();
        std::fs::write(rea.join("dist/process/OwnedCommand.js"), "").unwrap();
        let entry = rea.join("scripts/rea.mjs");
        let config = RunConfig {
            program: program.to_path_buf(),
            args: vec![entry.to_string_lossy().into_owned(), "mcp".into()],
            env: runtime::EnvSpec::empty()
                .set(
                    "HOME",
                    dir.path().join("home").to_string_lossy().into_owned(),
                )
                .set("PATH", "/usr/bin:/bin")
                .set("REA_LOG_LEVEL", "error"),
            cwd: dir.path().to_path_buf(),
            start_timeout: Duration::from_secs(60),
        };
        (dir, config, entry)
    }

    #[test]
    fn the_prepare_run_imports_reas_helper_with_reas_environment() {
        let (_dir, config, entry) = macos_config_with_pack(Path::new("/pack/node"));
        let p = prepare_config(&config, &entry, Flavor::Macos).unwrap();
        assert_eq!(p.args, ["--input-type=module", "--eval", PREPARE_SCRIPT]);
        assert_eq!(p.program, config.program);
        assert_eq!(p.cwd, config.cwd);
        assert_eq!(env(&p, "HOME"), env(&config, "HOME"));
        assert_eq!(env(&p, "PATH"), env(&config, "PATH"));
        let module = env(&p, PREPARE_ONLY_VARIABLE);
        assert!(module.starts_with("file://") && module.ends_with("/dist/process/OwnedCommand.js"));
        // Only that one variable is added, and rea's own configuration is untouched.
        assert_eq!(p.env.set.len(), config.env.set.len() + 1);
        assert!(config.var(PREPARE_ONLY_VARIABLE).is_none());
    }

    #[test]
    fn no_prepare_off_macos_or_without_the_module() {
        let (_dir, config, entry) = macos_config_with_pack(Path::new("/pack/node"));
        assert!(prepare_config(&config, &entry, Flavor::Linux).is_none());
        assert!(prepare_config(&config, &entry, Flavor::Windows).is_none());
        assert!(prepare_config(&config, Path::new("/nope/rea.mjs"), Flavor::Macos).is_none());
        std::fs::remove_file(
            entry
                .parent()
                .unwrap()
                .join("../dist/process/OwnedCommand.js"),
        )
        .unwrap();
        assert!(prepare_config(&config, &entry, Flavor::Macos).is_none());
    }

    #[test]
    fn the_module_url_is_percent_encoded() {
        assert_eq!(
            file_url(Path::new("/a b/Application Support/x#y.js")),
            "file:///a%20b/Application%20Support/x%23y.js"
        );
        assert_eq!(file_url(Path::new(r"C:\a\b.js")), "file:///C:/a/b.js");
    }

    #[test]
    fn the_cache_counts_when_it_has_something_in_it() {
        let (dir, config, _entry) = macos_config_with_pack(Path::new("/pack/node"));
        assert!(!helper_cached(&config));
        let cache = dir
            .path()
            .join("home/Library/Caches/rea/process-helper/darwin");
        std::fs::create_dir_all(&cache).unwrap();
        assert!(!helper_cached(&config));
        std::fs::write(cache.join("helper"), b"x").unwrap();
        assert!(helper_cached(&config));
    }

    #[tokio::test]
    async fn nothing_to_prepare_is_ok() {
        let (_dir, config, entry) = macos_config_with_pack(Path::new("/pack/node"));
        prepare_process_helper(&config, &entry, Flavor::Linux)
            .await
            .unwrap();
    }

    /// A fake `node`: a shell script that ignores its arguments.
    #[cfg(unix)]
    fn fake_node(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("node");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_run_that_succeeds_sees_the_module_and_home() {
        let scratch = tempfile::tempdir().unwrap();
        let out = scratch.path().join("seen");
        let node = fake_node(
            scratch.path(),
            &format!(
                "echo \"$REA_HELPER_MODULE|$HOME|$1|$2\" > '{}'",
                out.display()
            ),
        );
        let (_dir, config, entry) = macos_config_with_pack(&node);
        prepare_process_helper(&config, &entry, Flavor::Macos)
            .await
            .unwrap();
        let seen = std::fs::read_to_string(out).unwrap();
        assert!(seen.starts_with("file://"), "{seen}");
        assert!(seen.contains(&format!(
            "|{}|--input-type=module|--eval",
            env(&config, "HOME")
        )));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_run_reports_its_code_and_the_tail_of_its_stderr() {
        let scratch = tempfile::tempdir().unwrap();
        let node = fake_node(
            scratch.path(),
            "i=1; while [ $i -le 15 ]; do echo \"line $i\" >&2; i=$((i+1)); done; exit 3",
        );
        let (_dir, config, entry) = macos_config_with_pack(&node);
        let e = prepare_process_helper(&config, &entry, Flavor::Macos)
            .await
            .unwrap_err();
        match e {
            WarmupError::Failed { code, stderr_tail } => {
                assert_eq!(code, Some(3));
                assert_eq!(stderr_tail.len(), TAIL_LINES);
                assert_eq!(stderr_tail.first().unwrap(), "line 6");
                assert_eq!(stderr_tail.last().unwrap(), "line 15");
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(all(unix, target_os = "linux"))]
    #[tokio::test]
    async fn a_run_that_never_ends_is_killed_at_the_limit() {
        let scratch = tempfile::tempdir().unwrap();
        let pid_file = scratch.path().join("pid");
        let node = fake_node(
            scratch.path(),
            &format!("echo $$ > '{}'\nexec sleep 600", pid_file.display()),
        );
        let (_dir, config, entry) = macos_config_with_pack(&node);
        let started = std::time::Instant::now();
        let e = prepare_within(&config, &entry, Flavor::Macos, Duration::from_millis(500))
            .await
            .unwrap_err();
        assert!(matches!(e, WarmupError::TimedOut), "{e:?}");
        assert!(started.elapsed() < Duration::from_secs(20));
        let pid = std::fs::read_to_string(pid_file).unwrap();
        assert!(
            !Path::new("/proc").join(pid.trim()).exists(),
            "the prepare run outlived its limit"
        );
    }
}
