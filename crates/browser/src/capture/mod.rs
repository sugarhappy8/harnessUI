//! `browser_capture_site` (0.0.7, P7; ARCHITECTURE.md 9.3, SECURITY.md "Website capture"):
//! saves a copy of a web page's front end, so a model can study its design and rebuild it
//! with the user's own content.
//!
//! The capture opens the page in the agent's own browser (its own profile, every web
//! request under the network rules, WebSockets refused and other unchecked connections
//! watched), scrolls through it, reads it from an isolated world and saves, into a new
//! folder inside an approved folder:
//!
//! - `index.html`: the page as shown, from an inert copy of its DOM, parsed again as a
//!   browser parses the file and cleaned as a tree ([`inert`]): nothing that runs or loads
//!   documents, every resource address one of the copy's own files, a policy that runs
//!   nothing and loads nothing from the web first in the head ([`html::COPY_POLICY`]), and a
//!   last check before it is written;
//! - `styles/`: every style sheet (linked, inline, CSS-in-JS rules, adopted), addresses
//!   rewritten ([`css`]);
//! - `assets/`: pictures, fonts, icons, Lottie files and small videos, from the bodies the
//!   browser already received ([`assets`] decides what is kept and the limits; SVG pictures
//!   are parsed as XML and written again with drawing elements only);
//! - `scripts/`: the page's scripts as `.js.txt`, for reference, never linked;
//! - `screenshots/`: the first screen and the full page at each width;
//! - `animations.json` ([`anim`]) and `report.md` ([`report`]), which is also the result.
//!
//! Only responses to requests that passed the browser's request checks during the capture
//! are saved ([`netlog`]); a WebSocket, WebTransport or direct-socket connection that
//! starts during the capture stops it before anything is written. The page is never
//! clicked, typed into or submitted.

pub mod anim;
pub mod assets;
pub mod css;
pub(crate) mod describe;
pub mod guide;
pub mod html;
pub mod inert;
pub mod names;
pub(crate) mod netlog;
pub mod report;
pub(crate) mod run;
pub mod write;

/// The page script that serialises the page and reads its facts.
pub(crate) const COLLECT_JS: &str = include_str!("js/collect.js");
/// The page script that lists running animations.
pub(crate) const ANIMATIONS_JS: &str = include_str!("js/animations.js");
/// The page script that scrolls.
pub(crate) const SCROLL_JS: &str = include_str!("js/scroll.js");
/// The expression that lists animation libraries on `window` (main world).
pub(crate) const GLOBALS_JS: &str = include_str!("js/globals.js");
/// The page script that asks for pictures the style sheets name.
pub(crate) const FETCH_MISSING_JS: &str = include_str!("js/fetch_missing.js");

/// The tool's name.
pub const TOOL: &str = "browser_capture_site";
