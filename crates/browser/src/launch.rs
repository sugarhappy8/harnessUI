//! Starting the browser (ARCHITECTURE.md 9.3, 13.6): the agent's own profile, background
//! networking, sync and component updates off, first-run and default-browser checks off,
//! CDP over a pipe or a loopback port. The child is created through the runtime spawn
//! module, so it lands in the app's Job Object (Windows) or a registered process group
//! (macOS) and dies with the app.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use runtime::spawn::{EnvSpec, ManagedChild, SpawnSpec, StdioMode};
use tokio::io::AsyncReadExt;

use crate::config::{BrowserConfig, ConnectionMode};
use crate::discovery::Os;
use crate::transport::Transport;

/// How the browser is told where its debugging channel is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugChannel {
    /// `--remote-debugging-pipe`, commands on fd 3, replies on fd 4 (macOS, Linux).
    PipeFds,
    /// `--remote-debugging-pipe --remote-debugging-io-pipes=<read>,<write>`: inherited
    /// pipe handles named on the command line (Windows).
    PipeHandles(u32, u32),
    /// `--remote-debugging-port=0`: a port the OS picks, on 127.0.0.1.
    Port,
}

/// Features switched off: each one either talks to Google in the background or opens
/// surfaces the agent should not meet.
const DISABLED_FEATURES: &str = "Translate,MediaRouter,DialMediaRouteProvider,OptimizationHints,AutofillServerCommunication,InterestFeedContentSuggestions,CertificateTransparencyComponentUpdater,Prerender2";

/// The browser's command line, without the executable. Pure.
pub fn browser_args(config: &BrowserConfig, os: Os, channel: &DebugChannel) -> Vec<String> {
    let mut a: Vec<String> = vec![
        format!("--user-data-dir={}", config.profile_dir.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-background-networking".into(),
        "--disable-sync".into(),
        "--disable-component-update".into(),
        "--disable-default-apps".into(),
        "--disable-extensions".into(),
        "--disable-breakpad".into(),
        "--disable-crash-reporter".into(),
        "--disable-domain-reliability".into(),
        "--metrics-recording-only".into(),
        "--no-pings".into(),
        "--disable-search-engine-choice-screen".into(),
        "--deny-permission-prompts".into(),
        format!("--disable-features={DISABLED_FEATURES}"),
        format!(
            "--window-size={},{}",
            config.window_size.0, config.window_size.1
        ),
    ];
    match os {
        Os::Mac => a.push("--use-mock-keychain".into()),
        Os::Linux => {
            a.push("--password-store=basic".into());
            a.push("--disable-dev-shm-usage".into());
        }
        Os::Windows => {}
    }
    match channel {
        DebugChannel::PipeFds => a.push("--remote-debugging-pipe".into()),
        DebugChannel::PipeHandles(read, write) => {
            a.push("--remote-debugging-pipe".into());
            a.push(format!("--remote-debugging-io-pipes={read},{write}"));
        }
        DebugChannel::Port => a.push("--remote-debugging-port=0".into()),
    }
    if config.headless {
        a.push("--headless".into());
        a.push("--hide-scrollbars".into());
        a.push("--mute-audio".into());
    }
    if config.no_sandbox {
        a.push("--no-sandbox".into());
    }
    #[cfg(feature = "test-hooks")]
    a.extend(config.test_extra_args.iter().cloned());
    a.push("about:blank".into());
    a
}

/// A started browser and its connection.
pub struct Launched {
    /// The browser process tree; killed when dropped.
    pub child: ManagedChild,
    /// The CDP connection.
    pub transport: Transport,
    /// The last lines the browser wrote to stderr, for error details.
    pub stderr_tail: Arc<Mutex<String>>,
}

/// Bytes of stderr kept.
const STDERR_TAIL: usize = 8 * 1024;

fn env_for_browser() -> EnvSpec {
    let mut env = EnvSpec::minimal();
    if cfg!(all(unix, not(target_os = "macos"))) {
        for k in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "XAUTHORITY",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
        ] {
            env = env.inherit(k);
        }
    }
    env
}

fn drain_stderr(child: &mut ManagedChild) -> Arc<Mutex<String>> {
    let tail = Arc::new(Mutex::new(String::new()));
    if let Some(mut stderr) = child.take_stderr() {
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut t) = tail.lock() {
                    t.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if t.len() > STDERR_TAIL {
                        let mut cut = t.len() - STDERR_TAIL;
                        while !t.is_char_boundary(cut) {
                            cut += 1;
                        }
                        t.drain(..cut);
                    }
                }
            }
        });
    }
    tail
}

fn spec(exe: &Path, config: &BrowserConfig, args: Vec<String>) -> SpawnSpec {
    SpawnSpec::new(exe, &config.profile_dir)
        .args(args)
        .env(env_for_browser())
        .stdin(StdioMode::Null)
        .stdout(StdioMode::Null)
        .stderr(StdioMode::Piped)
}

/// Starts the browser at `exe` with `config` and connects to it.
pub async fn launch(exe: &Path, config: &BrowserConfig) -> Result<Launched, String> {
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        && is_users_own_profile(
            &config.profile_dir,
            Path::new(&home),
            std::env::var_os("LOCALAPPDATA").as_deref().map(Path::new),
        )
    {
        return Err(format!(
            "{} is the user's own browser profile; the agent must use its own",
            config.profile_dir.display()
        ));
    }
    std::fs::create_dir_all(&config.profile_dir).map_err(|e| {
        format!(
            "could not create the browser profile folder {}: {e}",
            config.profile_dir.display()
        )
    })?;
    match config.connection {
        ConnectionMode::Pipe => launch_pipe(exe, config),
        ConnectionMode::LoopbackPort => launch_port(exe, config).await,
    }
}

#[cfg(unix)]
fn launch_pipe(exe: &Path, config: &BrowserConfig) -> Result<Launched, String> {
    use runtime::spawn::InheritedFd;
    let io = |e: std::io::Error| format!("could not create the browser's pipes: {e}");
    // fd 3: the browser reads commands; fd 4: the browser writes replies and events.
    let (child_reads, app_writes) = std::io::pipe().map_err(io)?;
    let (app_reads, child_writes) = std::io::pipe().map_err(io)?;
    let args = browser_args(config, Os::current(), &DebugChannel::PipeFds);
    let mut child = runtime::spawn::spawn_with_fds(
        spec(exe, config, args),
        vec![
            InheritedFd {
                child_fd: 3,
                fd: child_reads.into(),
            },
            InheritedFd {
                child_fd: 4,
                fd: child_writes.into(),
            },
        ],
    )
    .map_err(|e| format!("the browser could not start: {e}"))?;
    let stderr_tail = drain_stderr(&mut child);
    let transport = crate::transport::pipe_transport(app_reads, app_writes).map_err(io)?;
    Ok(Launched {
        child,
        transport,
        stderr_tail,
    })
}

// TODO(M6): compile-checked from Linux only; run the end-to-end tests on Windows CI (or
// the owner's PC) before relying on it. Chromium reads the two handle values with
// `--remote-debugging-io-pipes` (content/browser/devtools/devtools_agent_host_impl.cc).
#[cfg(windows)]
fn launch_pipe(exe: &Path, config: &BrowserConfig) -> Result<Launched, String> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::CreatePipe;

    fn pipe() -> Result<(OwnedHandle, OwnedHandle), String> {
        let mut read = HANDLE::default();
        let mut write = HANDLE::default();
        // SAFETY: both out-pointers are valid; with no security attributes the handles
        // are not inheritable. Ownership moves into OwnedHandle right away.
        unsafe { CreatePipe(&mut read, &mut write, None, 0) }
            .map_err(|e| format!("could not create the browser's pipes: {e}"))?;
        // SAFETY: CreatePipe returned two fresh handles owned by nothing else.
        Ok(unsafe {
            (
                OwnedHandle::from_raw_handle(read.0),
                OwnedHandle::from_raw_handle(write.0),
            )
        })
    }

    use std::os::windows::io::AsRawHandle;
    let (child_reads, app_writes) = pipe()?;
    let (app_reads, child_writes) = pipe()?;
    // Chrome parses the values as 32-bit unsigned integers; kernel handle values fit.
    let value = |h: &OwnedHandle| h.as_raw_handle() as usize as u32;
    let channel = DebugChannel::PipeHandles(value(&child_reads), value(&child_writes));
    let args = browser_args(config, Os::current(), &channel);
    let mut child = runtime::spawn::spawn_with_handles(
        spec(exe, config, args),
        vec![child_reads, child_writes],
    )
    .map_err(|e| format!("the browser could not start: {e}"))?;
    let stderr_tail = drain_stderr(&mut child);
    let transport = crate::transport::pipe_transport(
        std::fs::File::from(app_reads),
        std::fs::File::from(app_writes),
    )
    .map_err(|e| format!("could not start the browser connection: {e}"))?;
    Ok(Launched {
        child,
        transport,
        stderr_tail,
    })
}

/// Whether `profile` is (or is inside, or contains) one of the user's own Chrome, Edge or
/// Chromium profiles, which the agent must never use (9.3). Compared lexically.
pub fn is_users_own_profile(profile: &Path, home: &Path, local_app_data: Option<&Path>) -> bool {
    let mut own: Vec<std::path::PathBuf> = [
        ".config/google-chrome",
        ".config/google-chrome-beta",
        ".config/chromium",
        ".config/microsoft-edge",
        "Library/Application Support/Google/Chrome",
        "Library/Application Support/Microsoft Edge",
        "Library/Application Support/Chromium",
    ]
    .iter()
    .map(|rel| home.join(rel))
    .collect();
    if let Some(local) = local_app_data {
        for rel in [
            "Google/Chrome/User Data",
            "Microsoft/Edge/User Data",
            "Chromium/User Data",
        ] {
            own.push(local.join(rel));
        }
    }
    let norm = |p: &Path| -> String {
        p.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join("/")
    };
    let profile = norm(profile);
    own.iter().any(|o| {
        let o = norm(o);
        profile == o
            || profile.starts_with(&format!("{o}/"))
            || o.starts_with(&format!("{profile}/"))
    })
}

/// Reads `DevToolsActivePort` (port, then the browser endpoint path).
pub fn parse_active_port(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    (port != 0 && path.starts_with("/devtools/browser/")).then(|| (port, path.to_string()))
}

async fn launch_port(exe: &Path, config: &BrowserConfig) -> Result<Launched, String> {
    let port_file = config.profile_dir.join("DevToolsActivePort");
    let _ = std::fs::remove_file(&port_file);
    let args = browser_args(config, Os::current(), &DebugChannel::Port);
    let mut child = runtime::spawn::spawn(spec(exe, config, args))
        .map_err(|e| format!("the browser could not start: {e}"))?;
    let stderr_tail = drain_stderr(&mut child);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let (port, path) = loop {
        if let Some(found) = std::fs::read_to_string(&port_file)
            .ok()
            .as_deref()
            .and_then(parse_active_port)
        {
            break found;
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the browser exited at start ({status})"));
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the browser did not open its debugging port within 20 s".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let transport =
        crate::transport::websocket_transport(&format!("ws://127.0.0.1:{port}{path}")).await?;
    Ok(Launched {
        child,
        transport,
        stderr_tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(args: &[String], a: &str) -> bool {
        args.iter().any(|x| x == a)
    }

    #[test]
    fn args_use_the_agent_profile_and_switch_background_traffic_off() {
        let config = BrowserConfig::new("/data/browser-profile");
        let args = browser_args(&config, Os::Linux, &DebugChannel::PipeFds);
        assert_eq!(args[0], "--user-data-dir=/data/browser-profile");
        for a in [
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-sync",
            "--disable-component-update",
            "--remote-debugging-pipe",
            "--window-size=1280,800",
        ] {
            assert!(has(&args, a), "{a} missing from {args:?}");
        }
        assert!(!has(&args, "--headless"), "a real window by default");
        assert!(!has(&args, "--no-sandbox"));
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("--remote-debugging-port"))
        );
        assert_eq!(args.last().unwrap(), "about:blank");
    }

    #[test]
    fn channels_and_platform_switches() {
        let mut config = BrowserConfig::new("/p");
        config.headless = true;
        config.no_sandbox = true;
        let win = browser_args(&config, Os::Windows, &DebugChannel::PipeHandles(12, 34));
        assert!(has(&win, "--remote-debugging-pipe"));
        assert!(has(&win, "--remote-debugging-io-pipes=12,34"));
        assert!(has(&win, "--headless"));
        assert!(has(&win, "--no-sandbox"));
        let mac = browser_args(&config, Os::Mac, &DebugChannel::Port);
        assert!(has(&mac, "--use-mock-keychain"));
        assert!(has(&mac, "--remote-debugging-port=0"));
        assert!(!has(&mac, "--remote-debugging-pipe"));
    }

    #[test]
    fn the_users_own_profiles_are_refused() {
        let home = Path::new("/home/u");
        assert!(is_users_own_profile(
            Path::new("/home/u/.config/google-chrome"),
            home,
            None
        ));
        assert!(is_users_own_profile(
            Path::new("/home/u/.config/google-chrome/Default"),
            home,
            None
        ));
        assert!(is_users_own_profile(
            Path::new("/home/u/Library/Application Support/Google/Chrome"),
            home,
            None
        ));
        assert!(is_users_own_profile(
            Path::new("C:/Users/u/AppData/Local/Google/Chrome/User Data"),
            Path::new("C:/Users/u"),
            Some(Path::new("C:/Users/u/AppData/Local"))
        ));
        // A folder holding the user's profile is refused too.
        assert!(is_users_own_profile(
            Path::new("/home/u/.config"),
            home,
            None
        ));
        assert!(!is_users_own_profile(
            Path::new("/home/u/.local/share/app/browser-profile"),
            home,
            None
        ));
    }

    #[test]
    fn active_port_file() {
        assert_eq!(
            parse_active_port("53211\n/devtools/browser/abc-def\n"),
            Some((53211, "/devtools/browser/abc-def".into()))
        );
        assert_eq!(parse_active_port("0\n/devtools/browser/x"), None);
        assert_eq!(parse_active_port("123\n/json"), None);
        assert_eq!(parse_active_port(""), None);
    }
}
