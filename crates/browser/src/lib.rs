//! The built-in `browser` tool server (ARCHITECTURE.md 9.3, 13.3–13.6, D8).
//!
//! The agent drives a Chromium-family browser (Chrome, Edge, Brave, Vivaldi, Chromium, or
//! the build the app can download for it, [`own`]), with its own profile, over the Chrome
//! DevTools Protocol (a pipe by default, a loopback port as a setting). Where the agent may go is
//! enforced on the browser's actual requests, not only on the tool's arguments:
//!
//! - `http`/`https` only; loopback, private, link-local, CGNAT, unique-local and
//!   unspecified addresses blocked on the resolved address, for every document request on
//!   every attached target and every redirect hop ([`netpolicy`], `session`).
//! - Form submissions (D8): intent makes the action always-ask; every non-GET and every
//!   form-submission navigation is held until approved ([`intent`]); held requests that do
//!   not match the approved intent go to the app's [`NavigationApprover`].
//! - Each read, click and type re-checks the frame's actual origin before acting.
//! - [0.0.7 P7] `browser_capture_site` ([`capture`]) saves a copy of a page's front end
//!   into an approved folder for studying its design; while it runs, every web request of
//!   the browser (not only page loads) goes through the address checks, and the captured
//!   tab's WebSockets are refused (one that starts anyway stops the capture).
//!
//! Entry point: [`BrowserServer`], an `mcp_host::BuiltinServer`.

pub mod approval;
pub mod capture;
pub mod cdp;
pub mod config;
pub mod discovery;
pub mod intent;
pub mod keys;
pub mod launch;
pub mod netpolicy;
pub mod own;
mod page;
mod server;
mod session;
pub mod snapshot;
pub mod transport;

pub use approval::{DenyAll, HeldNavigation, NavigationApprover};
pub use config::{BrowserConfig, ConnectionMode};
pub use discovery::{BrowserKind, BrowserUnavailable, ENV_OVERRIDE, FoundBrowser, find_browser};
pub use intent::HoldKind;
pub use page::device_to_css;
pub use server::BrowserServer;
