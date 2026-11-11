//! Settings for the browser tool (ARCHITECTURE.md 9.3, 13.6).

use std::path::PathBuf;
use std::time::Duration;

/// How the app talks to the browser over CDP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionMode {
    /// `--remote-debugging-pipe`: no listening port another local program could attach to.
    /// Chrome marks a pipe-driven browser as automated, and some sites refuse it.
    #[default]
    Pipe,
    /// `--remote-debugging-port` on 127.0.0.1 with a port the browser picks at random.
    /// Hides the automation flag but leaves an unauthenticated local port open while the
    /// browser runs (the setting for sites that refuse automated browsers).
    LoopbackPort,
}

/// Everything the browser server needs from the app.
#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// The agent's own browser profile (`--user-data-dir`), inside the app's data folder.
    /// Never the user's profile; it is also one of the folders the file tools can never
    /// reach (13.4).
    pub profile_dir: PathBuf,
    /// A browser executable the user chose in Settings, tried before any other.
    pub browser_path: Option<PathBuf>,
    /// The folder the agent's own browser is downloaded into ([`crate::own`]), in the
    /// app's data folder. Used when no installed browser is found.
    pub own_browser_dir: Option<PathBuf>,
    /// Pipe (default) or loopback port.
    pub connection: ConnectionMode,
    /// Run without a window. The app shows a real window; tests run headless.
    pub headless: bool,
    /// Lets pages load from private networks (10/8, 172.16/12, 192.168/16, 100.64/10,
    /// fc00::/7). Loopback, link-local, unspecified, multicast and reserved addresses stay
    /// blocked whatever this says.
    pub allow_private_networks: bool,
    /// Origins (`scheme://host[:port]`) in strict mode: background non-GET requests (XHR,
    /// fetch, beacons) from their pages are held for approval like form submissions (D8).
    pub strict_sites: Vec<String>,
    /// How long a navigation may take before the tool reports it as still loading.
    pub navigation_timeout: Duration,
    /// How long a held navigation waits for the user before it is failed.
    pub approval_timeout: Duration,
    /// Window (and headless viewport) size in CSS pixels.
    pub window_size: (u32, u32),
    /// Adds `--no-sandbox`. Only for containers that run tests as root; the app never
    /// sets it.
    pub no_sandbox: bool,
    /// Tests only (feature `test-hooks`): 127.0.0.1 is reachable on exactly these ports,
    /// so the integration tests can serve pages locally while every other loopback
    /// address stays blocked.
    #[cfg(feature = "test-hooks")]
    pub test_allowed_loopback_ports: Vec<u16>,
    /// Tests only (feature `test-hooks`): extra browser switches, for example
    /// `--isolate-origins` to put a test frame in its own process.
    #[cfg(feature = "test-hooks")]
    pub test_extra_args: Vec<String>,
    /// Tests only (feature `test-hooks`): host names the address check resolves to fixed
    /// addresses (pair with `--host-resolver-rules` in `test_extra_args`).
    #[cfg(feature = "test-hooks")]
    pub test_dns_overrides: Vec<(String, std::net::IpAddr)>,
    /// Tests only (feature `test-hooks`): a wait before a new tab or frame is put under
    /// the request checks, to hold open the moment between "attached" and "checked".
    #[cfg(feature = "test-hooks")]
    pub test_setup_delay: Duration,
}

impl BrowserConfig {
    /// Defaults: pipe mode, a real window, private networks blocked, no strict sites, 30 s
    /// navigation timeout, 120 s approval timeout, 1280×800.
    pub fn new(profile_dir: impl Into<PathBuf>) -> Self {
        Self {
            profile_dir: profile_dir.into(),
            browser_path: None,
            own_browser_dir: None,
            connection: ConnectionMode::Pipe,
            headless: false,
            allow_private_networks: false,
            strict_sites: Vec::new(),
            navigation_timeout: Duration::from_secs(30),
            approval_timeout: Duration::from_secs(120),
            window_size: (1280, 800),
            no_sandbox: false,
            #[cfg(feature = "test-hooks")]
            test_allowed_loopback_ports: Vec::new(),
            #[cfg(feature = "test-hooks")]
            test_extra_args: Vec::new(),
            #[cfg(feature = "test-hooks")]
            test_dns_overrides: Vec::new(),
            #[cfg(feature = "test-hooks")]
            test_setup_delay: Duration::ZERO,
        }
    }

    /// Whether strict mode is on for `origin`.
    pub fn is_strict(&self, origin: &str) -> bool {
        self.strict_sites
            .iter()
            .any(|s| s.eq_ignore_ascii_case(origin))
    }

    /// Loopback ports the test hooks open (always empty without the feature).
    pub(crate) fn loopback_ports(&self) -> Vec<u16> {
        #[cfg(feature = "test-hooks")]
        {
            self.test_allowed_loopback_ports.clone()
        }
        #[cfg(not(feature = "test-hooks"))]
        {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_safe_ones() {
        let c = BrowserConfig::new("/p");
        assert_eq!(c.connection, ConnectionMode::Pipe);
        assert!(!c.headless);
        assert!(!c.allow_private_networks);
        assert!(!c.no_sandbox);
        assert!(c.strict_sites.is_empty());
        assert_eq!(c.window_size, (1280, 800));
    }

    #[test]
    fn strict_sites_match_case_insensitively() {
        let mut c = BrowserConfig::new("/p");
        c.strict_sites.push("https://Bank.example".into());
        assert!(c.is_strict("https://bank.example"));
        assert!(!c.is_strict("https://other.example"));
    }
}
