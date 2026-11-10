//! The built-in `browser` tool server (ARCHITECTURE.md 9.3, 13.3–13.6, D8).
//!
//! The agent drives its own Chrome or Edge, with its own profile, over the Chrome DevTools
//! Protocol (a pipe by default, a loopback port as a setting). Where the agent may go is
//! enforced on the browser's actual requests, not only on the tool's arguments:
//!
//! - `http`/`https` only; loopback, private, link-local, CGNAT, unique-local and
//!   unspecified addresses blocked on the resolved address, for every document request on
//!   every attached target and every redirect hop ([`netpolicy`], `session`).
//! - Form submissions (D8): intent makes the action always-ask; every non-GET and every
//!   form-submission navigation is held until approved ([`intent`]); held requests that do
//!   not match the approved intent go to the app's [`NavigationApprover`].
//! - Each read, click and type re-checks the frame's actual origin before acting.
//!
//! Entry point: [`BrowserServer`], an `mcp_host::BuiltinServer`.

pub mod approval;
pub mod cdp;
pub mod config;
pub mod discovery;
pub mod intent;
pub mod keys;
pub mod launch;
pub mod netpolicy;
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
