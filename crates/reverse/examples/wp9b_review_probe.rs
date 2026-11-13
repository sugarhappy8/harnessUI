//! Throwaway review probe (WP9b security review). Deleted after the review.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mcp_host::ToolAllowList;
use reverse::env::{Flavor, run_config};
use reverse::install::{DirFetch, Installer};
use reverse::process::ReaProcess;
use reverse::{Component, Decompiler, Platform, ReverseLayout};
use tokio_util::sync::CancellationToken;

fn procs_under(root: &Path) -> Vec<(u32, String, Vec<String>)> {
    let mut out = Vec::new();
    let root = root.to_string_lossy().to_string();
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(cmd) = std::fs::read(e.path().join("cmdline")) else { continue };
        let cmd = String::from_utf8_lossy(&cmd).replace('\0', " ");
        let exe = std::fs::read_link(e.path().join("exe")).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        if cmd.contains(&root) || exe.contains(&root) {
            let env = std::fs::read(e.path().join("environ"))
                .map(|b| String::from_utf8_lossy(&b).split('\0').filter(|s| !s.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default();
            out.push((pid, cmd, env));
        }
    }
    out
}

fn show(label: &str, root: &Path, env: bool) {
    let ps = procs_under(root);
    eprintln!("== {label}: {} processes", ps.len());
    for (pid, cmd, e) in ps {
        let pgid = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| {
            let after = s.rsplit_once(')')?.1.to_owned();
            after.split_whitespace().nth(2).map(str::to_owned)
        });
        eprintln!("  pid {pid} pgid {pgid:?}: {}", cmd.chars().take(200).collect::<String>());
        if env {
            for v in e {
                eprintln!("      {v}");
            }
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let base = PathBuf::from(&args[1]);
    let cache = PathBuf::from(&args[2]);
    let target = PathBuf::from(&args[3]);
    let cancel_after: u64 = args[4].parse().unwrap();
    let platform = Platform::current();
    let layout = ReverseLayout::new(base.join("reverse"));
    let installer = Arc::new(
        Installer::new(layout.clone(), platform)
            .with_fetch(Arc::new(DirFetch::new(cache)))
            .with_release_rules(false),
    );
    if installer.usable(Component::Kuna).is_err() {
        installer.download(Component::Kuna, &CancellationToken::new(), &|_| {}).await.expect("install");
    }
    let process = ReaProcess::new(layout.clone(), platform);
    process.set_installer(installer.clone());
    process.set_log_dir(Some(&base.join("logs")));
    let config = run_config(&layout, &installer.installation(), Some(Decompiler::Kuna), Flavor::current(), true);
    let mut g = process.lock().await;
    let started = g.ensure_started(config.clone()).await.expect("starts");
    eprintln!("started with {} tools", started.tools.len());
    show("after start", layout.root(), true);
    let allow = ToolAllowList::new(["open_binary", "list_procedures"]);
    let o = g
        .call(&allow, "open_binary", serde_json::json!({"path": target}).as_object().cloned().unwrap(), Duration::from_secs(900), &CancellationToken::new(), None)
        .await;
    eprintln!("open -> {:?}", o.as_ref().map(|o| o.joined_text().chars().take(120).collect::<String>()));
    let tool = args.get(5).cloned().unwrap_or("list_procedures".into());
    let cancel = CancellationToken::new();
    let trig = cancel.clone();
    let root = layout.root().to_path_buf();
    let watcher = tokio::spawn(async move {
        for i in 0..(cancel_after * 2) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if i == cancel_after * 2 - 2 {
                show("just before cancel", &root, false);
                if std::env::var_os("KILL_REA_GROUP").is_some() {
                    for (pid, cmd, _) in procs_under(&root) {
                        if cmd.contains("rea.mjs") {
                            eprintln!("SIGKILL to rea's group {pid}");
                            unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
                        }
                    }
                }
            }
        }
        trig.cancel();
    });
    let begun = std::time::Instant::now();
    let r = g
        .call(&allow, &tool, serde_json::json!({"detail": "summary"}).as_object().cloned().unwrap(), Duration::from_secs(900), &cancel, None)
        .await;
    eprintln!("call -> {:?} after {:.1}s; state {:?}", r.as_ref().map(|o| o.joined_text().chars().take(200).collect::<String>()), begun.elapsed().as_secs_f64(), process.state());
    watcher.abort();
    show("right after the call returned", layout.root(), false);
    for s in [2u64, 5, 10, 20] {
        tokio::time::sleep(Duration::from_secs(s)).await;
        show(&format!("{s} s later"), layout.root(), false);
    }
    drop(g);
    process.stop(Duration::from_secs(3)).await;
    show("after process.stop", layout.root(), false);
    let log = std::fs::read_to_string(base.join("logs").join("mcp-reverse.log")).unwrap_or_default();
    eprintln!("log tail:\n{}", log.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
}
