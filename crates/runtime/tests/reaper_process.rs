//! End-to-end test of the reaper (ARCHITECTURE.md 4.1), and the reaper process itself.
//!
//! This test target has `harness = false`, so this file's `main` runs for it. Started with
//! [`REAPER_FLAG`] it *is* the reaper, exactly as the app's own binary would be. Started
//! with [`FAKE_APP_FLAG`] it plays the app: it installs a reaper, starts a process group
//! through the spawn module, reports the PIDs and waits to be killed. Started any other way
//! (by `cargo test`) it runs the checks:
//!
//! 1. **Pipe closed:** a reaper started from this process kills a registered group when the
//!    client is dropped.
//! 2. **App killed:** a fake app is killed with `SIGKILL` (a crash or force-quit); the
//!    reaper it started kills the group the fake app registered automatically through
//!    [`runtime::ReaperClient::install_global`], then exits.

/// Hidden flag that turns this binary into the reaper.
const REAPER_FLAG: &str = "--runtime-test-reaper";
/// Hidden flag that turns this binary into a fake app.
const FAKE_APP_FLAG: &str = "--runtime-test-fake-app";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some(REAPER_FLAG) => {
            let parent = args.get(2).and_then(|pid| pid.parse().ok()).unwrap_or(0);
            runtime::reaper_main(parent);
        }
        Some(FAKE_APP_FLAG) => {
            #[cfg(unix)]
            unix::fake_app();
            return;
        }
        _ => {}
    }
    if args.iter().any(|arg| arg == "--list") {
        println!("reaper_process: test");
        return;
    }
    #[cfg(unix)]
    {
        unix::pipe_closed_kills_registered_group();
        println!("reaper_process: pipe_closed_kills_registered_group ... ok");
        unix::killed_app_leaves_no_orphans();
        println!("reaper_process: killed_app_leaves_no_orphans ... ok");
    }
    #[cfg(not(unix))]
    println!(
        "reaper_process: skipped (the reaper is a no-op on Windows, where Job Objects kill children)"
    );
}

#[cfg(unix)]
mod unix {
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use runtime::{ManagedChild, ReaperClient, SpawnSpec, StdioMode, resolve_program, spawn};
    use tokio::io::{AsyncBufReadExt, BufReader};

    use super::{FAKE_APP_FLAG, REAPER_FLAG};

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    fn this_exe() -> PathBuf {
        std::env::current_exe().expect("current exe")
    }

    /// `sh -c 'sleep 30 & echo $!; wait'`: a group with a leader and a grandchild.
    fn sleeper_spec() -> SpawnSpec {
        let sh = resolve_program("sh", None).expect("sh on PATH");
        SpawnSpec::new(sh, std::env::temp_dir())
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdout(StdioMode::Piped)
    }

    async fn first_line(child: &mut ManagedChild) -> String {
        let stdout = child.take_stdout().expect("stdout piped");
        let mut lines = BufReader::new(stdout).lines();
        tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .expect("line within 10 s")
            .expect("read")
            .expect("a line")
    }

    /// Dead, or a zombie nobody has reaped yet (on Linux a container's PID 1 may not reap).
    fn is_dead(pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Err(_) => true,
                Ok(stat) => stat
                    .rfind(')')
                    .and_then(|end| stat[end + 1..].split_whitespace().next())
                    .is_some_and(|state| state == "Z" || state == "X"),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let pid = libc::pid_t::try_from(pid).expect("pid fits");
            // SAFETY: signal 0 only checks for existence.
            let rc = unsafe { libc::kill(pid, 0) };
            rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
    }

    /// Waits without blocking the runtime. The reaper started by this process is its own
    /// child: once it exits it stays a zombie until Tokio reaps it, which needs the runtime
    /// to run. On macOS `kill(pid, 0)` reports a zombie as alive, so a blocking sleep here
    /// would wait for a zombie forever.
    async fn wait_dead(pid: u32, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if is_dead(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        is_dead(pid)
    }

    pub(super) fn pipe_closed_kills_registered_group() {
        let rt = runtime();
        rt.block_on(async {
            let client = ReaperClient::start(&this_exe(), REAPER_FLAG).expect("start reaper");
            let reaper_pid = client.pid().expect("reaper pid");
            let mut child = spawn(sleeper_spec()).expect("spawn sleeper");
            let leader = child.id().expect("leader pid");
            let grandchild: u32 = first_line(&mut child)
                .await
                .trim()
                .parse()
                .expect("grandchild pid");
            // Control: a group that is registered and then unregistered must survive.
            let mut control = spawn(sleeper_spec()).expect("spawn control");
            let control_leader = control.id().expect("control pid");
            let control_grandchild: u32 = first_line(&mut control)
                .await
                .trim()
                .parse()
                .expect("control grandchild");
            client.register(control_leader).expect("register control");
            client
                .unregister(control_leader)
                .expect("unregister control");
            client.register(leader).expect("register");
            // A protected or bogus group must be ignored rather than crash the reaper.
            client.register(1).expect("send");
            assert!(
                !is_dead(grandchild),
                "grandchild runs before the pipe closes"
            );

            drop(client);

            let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .expect("leader exits after the pipe closes")
                .expect("wait");
            assert!(
                !status.success(),
                "leader was terminated, not finished: {status:?}"
            );
            assert!(
                wait_dead(grandchild, Duration::from_secs(10)).await,
                "grandchild {grandchild} survived"
            );
            assert!(
                wait_dead(reaper_pid, Duration::from_secs(10)).await,
                "reaper {reaper_pid} did not exit"
            );
            assert!(
                !is_dead(control_grandchild),
                "an unregistered group was killed"
            );
            assert!(
                control.try_wait().expect("try_wait").is_none(),
                "an unregistered leader was killed"
            );
            control.kill().await.expect("kill control");
            assert!(wait_dead(control_grandchild, Duration::from_secs(10)).await);
        });
    }

    pub(super) fn killed_app_leaves_no_orphans() {
        let rt = runtime();
        rt.block_on(async {
            let spec = SpawnSpec::new(this_exe(), std::env::temp_dir())
                .arg(FAKE_APP_FLAG)
                .stdout(StdioMode::Piped);
            let mut app = spawn(spec).expect("spawn fake app");
            let line = first_line(&mut app).await;
            let pids: Vec<u32> = line
                .split_whitespace()
                .map(|p| p.parse().expect("pid"))
                .collect();
            let [leader, grandchild, reaper] = pids[..] else {
                panic!("unexpected line {line:?}")
            };
            assert!(!is_dead(leader) && !is_dead(grandchild) && !is_dead(reaper));

            // Force-quit the app: SIGKILL, no chance to clean up.
            app.kill().await.expect("kill fake app");

            assert!(
                wait_dead(leader, Duration::from_secs(10)).await,
                "leader {leader} survived the app"
            );
            assert!(
                wait_dead(grandchild, Duration::from_secs(10)).await,
                "grandchild {grandchild} survived the app"
            );
            assert!(
                wait_dead(reaper, Duration::from_secs(10)).await,
                "reaper {reaper} did not exit"
            );
        });
    }

    pub(super) fn fake_app() {
        let rt = runtime();
        rt.block_on(async {
            let client = ReaperClient::start(&this_exe(), REAPER_FLAG).expect("start reaper");
            let reaper = client.pid().expect("reaper pid");
            client.install_global().expect("first reaper");
            // Registered with the reaper automatically by the spawn module.
            let mut child = spawn(sleeper_spec()).expect("spawn sleeper");
            let leader = child.id().expect("leader pid");
            let grandchild = first_line(&mut child).await;
            let mut out = std::io::stdout().lock();
            writeln!(out, "{leader} {} {reaper}", grandchild.trim()).expect("report pids");
            out.flush().expect("flush");
            drop(out);
            // Wait to be killed; never drop `child` (that would kill the group ourselves).
            tokio::time::sleep(Duration::from_secs(60)).await;
            std::mem::forget(child);
        });
    }
}
