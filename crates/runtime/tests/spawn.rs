//! Integration tests for the spawn module and the stale-child registry: the child
//! environment is cleared, the working directory is applied, and killing or dropping a
//! child kills its whole tree.

use std::time::{Duration, Instant};

use runtime::{EnvSpec, SpawnSpec, process_alive, resolve_program, spawn};
use tokio::io::AsyncReadExt;

/// A variable present in this process's environment that the allowlist does not pass on.
/// Cargo sets `CARGO_MANIFEST_DIR` (and friends) for test processes; fall back to any
/// other non-allowlisted name.
fn non_allowlisted_parent_var() -> Option<(String, String)> {
    let allow = EnvSpec::minimal().inherit;
    std::env::vars().find(|(name, _)| {
        !allow
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name))
            && name.starts_with("CARGO")
    })
}

async fn read_all(child: &mut runtime::ManagedChild) -> String {
    let mut stdout = child.take_stdout().expect("stdout piped");
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stdout.read_to_end(&mut out))
        .await
        .expect("output within 20 s")
        .expect("read stdout");
    // Lossy: cmd.exe prints in the OEM code page.
    String::from_utf8_lossy(&out).into_owned()
}

fn wait_gone(pid: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !process_alive(pid)
}

#[cfg(unix)]
mod unix {
    use runtime::StdioMode;

    use super::*;

    fn sh() -> std::path::PathBuf {
        resolve_program("sh", None).expect("sh on PATH")
    }

    #[tokio::test]
    async fn environment_is_cleared_then_built_from_the_allowlist() {
        let (secret_name, _) =
            non_allowlisted_parent_var().expect("cargo sets CARGO_* variables for tests");
        let env = resolve_program("env", None).expect("env on PATH");
        let spec = SpawnSpec::new(env, std::env::temp_dir())
            .env(EnvSpec::minimal().set("EXTRA_FOR_CHILD", "42"));
        let mut child = spawn(spec).expect("spawn env");
        let out = read_all(&mut child).await;
        assert!(child.wait().await.expect("wait").success());
        let names: Vec<&str> = out
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name))
            .collect();
        assert!(
            !names.contains(&secret_name.as_str()),
            "{secret_name} leaked into {names:?}"
        );
        assert!(names.contains(&"PATH"), "{names:?}");
        assert!(out.lines().any(|line| line == "EXTRA_FOR_CHILD=42"));
        let allowed = EnvSpec::minimal().inherit;
        for name in names {
            assert!(
                allowed.iter().any(|a| a == name) || name == "EXTRA_FOR_CHILD",
                "unexpected variable {name}"
            );
        }
    }

    #[tokio::test]
    async fn empty_environment_really_is_empty() {
        let env = resolve_program("env", None).expect("env on PATH");
        let mut child = spawn(SpawnSpec::new(env, std::env::temp_dir()).env(EnvSpec::empty()))
            .expect("spawn env");
        let out = read_all(&mut child).await;
        assert!(child.wait().await.expect("wait").success());
        assert_eq!(out.trim(), "", "expected no variables, got {out:?}");
    }

    #[tokio::test]
    async fn working_directory_is_applied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = SpawnSpec::new(sh(), dir.path()).args(["-c", "pwd -P"]);
        let mut child = spawn(spec).expect("spawn pwd");
        let out = read_all(&mut child).await;
        assert!(child.wait().await.expect("wait").success());
        let expected = std::fs::canonicalize(dir.path()).expect("canonical tempdir");
        assert_eq!(std::path::Path::new(out.trim()), expected);
    }

    #[tokio::test]
    async fn start_kill_kills_the_grandchild_too() {
        let spec =
            SpawnSpec::new(sh(), std::env::temp_dir()).args(["-c", "sleep 30 & echo $!; wait"]);
        let mut child = spawn(spec).expect("spawn sh");
        let leader = child.id().expect("pid");
        let mut stdout = tokio::io::BufReader::new(child.take_stdout().expect("stdout"));
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(10),
            tokio::io::AsyncBufReadExt::read_line(&mut stdout, &mut line),
        )
        .await
        .expect("pid line")
        .expect("read");
        let grandchild: u32 = line.trim().parse().expect("grandchild pid");
        assert!(process_alive(grandchild));

        child.start_kill().expect("kill tree");
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("exit")
            .expect("wait");
        assert!(!status.success());
        assert!(
            wait_gone(grandchild, Duration::from_secs(10)),
            "grandchild {grandchild} survived"
        );
        assert!(wait_gone(leader, Duration::from_secs(10)));
        // Killing again after the tree is gone is fine.
        child.start_kill().expect("second kill is a no-op");
    }

    #[tokio::test]
    async fn dropping_the_handle_kills_the_tree() {
        let spec =
            SpawnSpec::new(sh(), std::env::temp_dir()).args(["-c", "sleep 30 & echo $!; wait"]);
        let mut child = spawn(spec).expect("spawn sh");
        let mut stdout = tokio::io::BufReader::new(child.take_stdout().expect("stdout"));
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(10),
            tokio::io::AsyncBufReadExt::read_line(&mut stdout, &mut line),
        )
        .await
        .expect("pid line")
        .expect("read");
        let grandchild: u32 = line.trim().parse().expect("grandchild pid");
        drop(child);
        assert!(
            wait_gone(grandchild, Duration::from_secs(10)),
            "grandchild {grandchild} survived the drop"
        );
    }

    #[tokio::test]
    async fn signal_reaches_the_group() {
        let spec = SpawnSpec::new(sh(), std::env::temp_dir())
            .args(["-c", "trap 'exit 7' TERM; sleep 30 & wait"]);
        let mut child = spawn(spec).expect("spawn sh");
        tokio::time::sleep(Duration::from_millis(200)).await;
        child.signal(libc::SIGTERM).expect("signal");
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("exit")
            .expect("wait");
        assert_eq!(status.code(), Some(7));
    }

    #[tokio::test]
    async fn stdin_can_be_piped() {
        use tokio::io::AsyncWriteExt;
        let spec = SpawnSpec::new(sh(), std::env::temp_dir())
            .args(["-c", "read line; echo got:$line"])
            .stdin(StdioMode::Piped);
        let mut child = spawn(spec).expect("spawn sh");
        let mut stdin = child.take_stdin().expect("stdin");
        stdin.write_all(b"hello\n").await.expect("write");
        drop(stdin);
        assert_eq!(read_all(&mut child).await.trim(), "got:hello");
        assert!(child.wait().await.expect("wait").success());
    }

    #[tokio::test]
    async fn command_wrap_builds_the_same_contained_command() {
        let env = resolve_program("env", None).expect("env on PATH");
        let spec = SpawnSpec::new(env, std::env::temp_dir()).env(EnvSpec::empty().set("ONLY", "1"));
        let wrap = runtime::command_wrap(&spec);
        let command = wrap.command().as_std();
        assert_eq!(
            command.get_current_dir(),
            Some(std::env::temp_dir().as_path())
        );
        let envs: Vec<_> = command
            .get_envs()
            .filter_map(|(k, v)| v.map(|v| (k.to_owned(), v.to_owned())))
            .collect();
        assert_eq!(envs, vec![("ONLY".into(), "1".into())]);
    }

    #[tokio::test]
    async fn stale_child_sweep_kills_a_matching_survivor_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry_path = dir.path().join("children.json");
        let sleep = resolve_program("sleep", None).expect("sleep on PATH");

        let mut matching =
            spawn(SpawnSpec::new(&sleep, std::env::temp_dir()).arg("30")).expect("spawn sleep");
        let mut other =
            spawn(SpawnSpec::new(&sleep, std::env::temp_dir()).arg("30")).expect("spawn sleep");
        let matching_pid = matching.id().expect("pid");
        let other_pid = other.id().expect("pid");

        let registry = runtime::ChildRegistry::open(&registry_path);
        registry.register(matching_pid, &sleep).expect("register");
        // Recorded with a different executable: must never be killed.
        registry
            .register(other_pid, std::path::Path::new("/definitely/not/sleep"))
            .expect("register");
        drop(registry);

        // Next launch.
        let killed = runtime::ChildRegistry::open(&registry_path).sweep_stale();
        assert_eq!(killed, vec![matching_pid]);
        let status = tokio::time::timeout(Duration::from_secs(10), matching.wait())
            .await
            .expect("exit")
            .expect("wait");
        assert!(!status.success());
        assert!(
            other.try_wait().expect("try_wait").is_none(),
            "the non-matching process must survive"
        );
        assert!(
            runtime::ChildRegistry::open(&registry_path)
                .records()
                .is_empty()
        );
        other.kill().await.expect("cleanup");
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    fn cmd() -> std::path::PathBuf {
        resolve_program("cmd", None).expect("cmd.exe on PATH")
    }

    #[tokio::test]
    async fn environment_is_cleared_then_built_from_the_allowlist() {
        let (secret_name, _) =
            non_allowlisted_parent_var().expect("cargo sets CARGO_* variables for tests");
        let spec = SpawnSpec::new(cmd(), std::env::temp_dir())
            .args(["/d", "/c", "set"])
            .env(EnvSpec::minimal().set("EXTRA_FOR_CHILD", "42"));
        let mut child = spawn(spec).expect("spawn cmd");
        let out = read_all(&mut child).await;
        assert!(child.wait().await.expect("wait").success());
        let names: Vec<String> = out
            .lines()
            .filter_map(|line| {
                line.split_once('=')
                    .map(|(name, _)| name.to_ascii_uppercase())
            })
            .collect();
        assert!(
            !names.contains(&secret_name.to_ascii_uppercase()),
            "{secret_name} leaked into {names:?}"
        );
        assert!(names.contains(&"SYSTEMROOT".to_owned()), "{names:?}");
        assert!(out.lines().any(|line| line.trim() == "EXTRA_FOR_CHILD=42"));
    }

    #[tokio::test]
    async fn working_directory_is_applied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = SpawnSpec::new(cmd(), dir.path()).args(["/d", "/c", "cd"]);
        let mut child = spawn(spec).expect("spawn cmd");
        let out = read_all(&mut child).await;
        assert!(child.wait().await.expect("wait").success());
        let leaf = dir
            .path()
            .file_name()
            .expect("leaf")
            .to_string_lossy()
            .to_ascii_lowercase();
        assert!(
            out.trim().to_ascii_lowercase().ends_with(&leaf),
            "{out:?} does not end with {leaf}"
        );
    }

    #[tokio::test]
    async fn start_kill_terminates_the_job() {
        let spec = SpawnSpec::new(cmd(), std::env::temp_dir()).args([
            "/d",
            "/c",
            "ping -n 30 127.0.0.1 >NUL",
        ]);
        let mut child = spawn(spec).expect("spawn cmd");
        let pid = child.id().expect("pid");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(process_alive(pid));
        child.start_kill().expect("terminate job");
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("exit")
            .expect("wait");
        assert!(!status.success());
        drop(child);
        assert!(wait_gone(pid, Duration::from_secs(10)));
    }
}
