//! Web search for the model (ARCHITECTURE.md 9.3, 13.5, 13.6).
//!
//! A search runs in these steps, each its own module:
//!
//! 1. [`versions`]: the libraries the project uses and their versions, read from its
//!    manifests. When the question is about one of them, its version goes into the search.
//! 2. [`provider`]: one search service answers with titles, addresses and snippets
//!    (DuckDuckGo, Brave, Tavily or the user's own SearXNG instance).
//! 3. [`official`]: which of the results are official documentation, and whether a page
//!    is for the version the project uses.
//! 4. [`fetch`]: the top pages are read. This is a security boundary: the fetcher refuses
//!    loopback, private, link-local, shared (CGNAT) and unspecified addresses before it
//!    connects, connects only to the address it checked, and checks every redirect again
//!    ([`netguard`]).
//! 5. [`extract`] and [`rank`]: each page becomes plain text, cut into passages, and the
//!    passages that fit the question best are kept.
//!
//! [`pipeline::Engine`] ties them together.
//!
//! Everything a search returns is untrusted data: text written by whoever runs the site.
//! This crate returns it as plain text only and never interprets it. The app sanitises it
//! once more, stores it, and gives it to the model as a tool result.

pub mod extract;
pub mod fetch;
pub mod netguard;
pub mod official;
pub mod pipeline;
pub mod provider;
pub mod rank;
pub mod versions;

pub use fetch::{Accept, FetchError, Fetched, Fetcher};
pub use official::{OfficialSource, Registry, version_note};
pub use pipeline::{Engine, PageRead, SearchError, SearchOptions, SearchOutcome, SourceHit};
pub use provider::{Provider, ProviderConfig, ProviderError, ProviderKind, SearchResult};
pub use rank::Passage;
pub use versions::{Ecosystem, Library};
