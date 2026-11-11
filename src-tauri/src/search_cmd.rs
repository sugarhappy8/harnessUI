//! Web search for the model (0.0.4; ARCHITECTURE.md 9.3, 13.3, 13.5, 13.6): the built-in
//! `search` server with `web_search` and `read_source`, the chat's stored sources, and the
//! search settings.
//!
//! What must hold:
//!
//! - **Both tools go through the policy gate.** They are outbound network actions with
//!   their own grant scope ([`policy::Scope::WebSearch`]): the first search in a chat asks,
//!   "Allow for this chat" covers later ones, and after untrusted content entered a turn
//!   they ask again (D9), because a search sends the question out.
//! - **Pages are read only through the guarded fetcher** of `crates/search`: it refuses
//!   loopback, private, link-local, shared and unspecified addresses before connecting,
//!   connects to the address it checked and checks every redirect. The model cannot make
//!   `read_source` fetch an address of its choosing: it names the number of a source a
//!   search of this chat stored, and the stored address is what the card shows and what is
//!   fetched.
//! - **Everything retrieved is untrusted data.** Titles, summaries and excerpts are
//!   sanitised like other tool results ([`policy::sanitize`]), capped, stored as plain text
//!   and given to the model as a tool result, which the agent loop wraps and treats as
//!   untrusted for the rest of the turn. Nothing retrieved is ever rendered as HTML.
//! - **What is stored is what the model was given.** A source's stored title, summary and
//!   excerpt are exactly the text that went into tool results, so the Sources tab can show
//!   what an answer could rest on, with the page's address, the time it was read and the
//!   hash of its text.
//! - **API keys live in the keychain** (`search.brave`, `search.tavily`). No command
//!   returns one: the settings say only whether a key is set.
//! - **The project's manifests are read through the path sandbox**, like the file tools
//!   read: only inside the chat's approved folders.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use search::provider::{Provider, ProviderConfig, ProviderError, ProviderKind};
use search::{
    Engine, FetchError, Fetcher, Library, PageRead, Passage, Registry, SearchError, SearchOptions,
    SearchOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tauri::{AppHandle, Emitter, Manager, State};
use url::Url;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The built-in server's name (the "search" group of the Tools panel).
pub const SERVER_NAME: &str = "search";
pub const WEB_SEARCH: &str = "web_search";
pub const READ_SOURCE: &str = "read_source";

/// The setting that holds [`SearchSettings`].
pub const SETTINGS_KEY: &str = "searchSettings";
/// Sent to the main window (with the chat's id) when a chat's sources changed.
pub const SOURCES_CHANGED: &str = "sources-changed";

/// How every search result ends for the model.
pub const CITE_HINT: &str = "Cite a source as [n] right after the sentence it supports. Use only what the excerpts say; if they do not answer the question, say so.";

const WEB_SEARCH_DESCRIPTION: &str = "Search the web and read the pages it finds. Returns numbered sources, each with its address and an excerpt of the page's text. Use it for documentation, for anything that may have changed since you were trained, and for facts you are not sure of. When the question is about a library the project uses, name it in `library`: results are matched to the project's version of it and its official documentation comes first. Cite a source as [n] right after the sentence it supports.";
const READ_SOURCE_DESCRIPTION: &str = "Read more of a source that web_search found. `n` is the source's number. Returns passages of its page that you were not given yet: the ones that fit `query` best, or the next ones on the page when there is no query.";

/// Characters per token used to size results (as the file tools do).
const CHARS_PER_TOKEN: usize = 3;
/// A title as the model is given it, in characters at most.
const TITLE_CHARS: usize = 160;
/// A summary of the search service as the model is given it, at most.
const SNIPPET_CHARS: usize = 300;
/// An excerpt per source: never less room than this, never more than that.
const MIN_EXCERPT_CHARS: usize = 240;
const MAX_EXCERPT_CHARS: usize = 2400;
/// What a hit costs besides its excerpt (number, title, site, address), roughly.
const HIT_OVERHEAD_CHARS: usize = 220;
/// Results per search by default and at most.
const DEFAULT_RESULTS: usize = 6;
/// What `read_source` returns at most, in characters.
const MAX_MORE_CHARS: usize = 8000;

// ---- Settings ----------------------------------------------------------------------

/// The search settings (Settings → Search). No key is in here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchSettings {
    pub provider: ProviderKind,
    /// The address of the user's SearXNG instance; empty when none is set.
    pub searxng_url: String,
    /// How many of the top results are read (0: none, only the summaries).
    pub pages: u8,
    /// Match results to the versions of the project's libraries.
    pub version_matching: bool,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            provider: ProviderKind::DuckDuckGo,
            searxng_url: String::new(),
            pages: search::pipeline::DEFAULT_PAGES as u8,
            version_matching: true,
        }
    }
}

/// The settings as the window gets them: with whether each key is set, never a key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSettingsDto {
    pub provider: ProviderKind,
    pub searxng_url: String,
    pub pages: u8,
    pub version_matching: bool,
    pub brave_key_set: bool,
    pub tavily_key_set: bool,
}

/// The keychain entry of a provider's key.
fn key_name(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::Brave => Some("search.brave"),
        ProviderKind::Tavily => Some("search.tavily"),
        ProviderKind::DuckDuckGo | ProviderKind::Searxng => None,
    }
}

/// The provider a key is for, as the window names it.
fn keyed_provider(name: &str) -> AppResult<ProviderKind> {
    match name {
        "brave" => Ok(ProviderKind::Brave),
        "tavily" => Ok(ProviderKind::Tavily),
        _ => Err(AppError::bad_request(
            "That search service does not use an API key.",
        )),
    }
}

/// The address of a SearXNG instance as it is stored: http or https, no user name or
/// password in it, nothing after the path.
fn checked_searxng_url(address: &str) -> Result<String, String> {
    let address = address.trim();
    if address.is_empty() {
        return Ok(String::new());
    }
    let url = Url::parse(address).map_err(|_| {
        "The SearXNG address is not a valid address (for example https://searx.example.org)."
            .to_string()
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("The SearXNG address must start with http:// or https://.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("The SearXNG address must not contain a user name or password.".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(
            "Enter only the address of the instance, without anything after a ? or #.".into(),
        );
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

/// Settings as they may be saved, or why not.
fn checked_settings(mut settings: SearchSettings) -> Result<SearchSettings, String> {
    settings.searxng_url = checked_searxng_url(&settings.searxng_url)?;
    if settings.provider == ProviderKind::Searxng && settings.searxng_url.is_empty() {
        return Err("Enter the address of your SearXNG instance first.".into());
    }
    if usize::from(settings.pages) > search::pipeline::MAX_PAGES {
        return Err(format!(
            "The number of pages to read must be from 0 to {}.",
            search::pipeline::MAX_PAGES
        ));
    }
    Ok(settings)
}

/// An API key as it may be saved, or why not.
fn checked_key(key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 512 || key.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("That does not look like an API key.".into());
    }
    Ok(key.to_string())
}

async fn load_settings(store: &store::Store) -> SearchSettings {
    let stored: Option<SearchSettings> = store.get_setting(SETTINGS_KEY).await.ok().flatten();
    // A stored value this version would not save (edited by hand) falls back to the
    // defaults rather than being used.
    stored
        .and_then(|s| checked_settings(s).ok())
        .unwrap_or_default()
}

fn key_is_set(secrets: &dyn store::secrets::SecretStore, kind: ProviderKind) -> bool {
    key_name(kind)
        .and_then(|name| secrets.get(name).ok().flatten())
        .is_some_and(|key| !key.trim().is_empty())
}

fn settings_dto(
    settings: SearchSettings,
    secrets: &dyn store::secrets::SecretStore,
) -> SearchSettingsDto {
    SearchSettingsDto {
        brave_key_set: key_is_set(secrets, ProviderKind::Brave),
        tavily_key_set: key_is_set(secrets, ProviderKind::Tavily),
        provider: settings.provider,
        searxng_url: settings.searxng_url,
        pages: settings.pages,
        version_matching: settings.version_matching,
    }
}

fn keychain_error(e: store::secrets::SecretError) -> AppError {
    AppError::new(
        "keychain",
        "The API key could not be saved to the system keychain.",
        "Unlock the keychain and try again.",
        e.to_string(),
    )
}

/// `<app name>/<version>`: what search services and sites are told about who asks.
fn user_agent(app: &AppHandle) -> String {
    let info = app.package_info();
    format!("{}/{}", info.name, info.version)
}

/// The provider for the saved settings, with its key from the keychain.
fn provider_for(
    settings: &SearchSettings,
    secrets: &dyn store::secrets::SecretStore,
    user_agent: &str,
) -> Result<Arc<dyn Provider>, ProviderError> {
    let mut config = ProviderConfig::new(settings.provider, user_agent);
    if let Some(name) = key_name(settings.provider) {
        config.api_key = secrets.get(name).ok().flatten();
    }
    config.searxng_url = Url::parse(&settings.searxng_url).ok();
    search::provider::build(config)
}

#[tauri::command]
pub async fn get_search_settings(state: State<'_, AppState>) -> AppResult<SearchSettingsDto> {
    Ok(settings_dto(
        load_settings(&state.store).await,
        state.secrets.as_ref(),
    ))
}

/// Saves the provider, the SearXNG address, the number of pages and version matching.
#[tauri::command]
pub async fn set_search_settings(
    state: State<'_, AppState>,
    settings: SearchSettings,
) -> AppResult<SearchSettingsDto> {
    let settings = checked_settings(settings).map_err(AppError::bad_request)?;
    state.store.set_setting(SETTINGS_KEY, &settings).await?;
    Ok(settings_dto(settings, state.secrets.as_ref()))
}

/// Stores a provider's API key in the keychain. Returns the settings, which say only that
/// a key is set.
#[tauri::command]
pub async fn set_search_key(
    state: State<'_, AppState>,
    provider: String,
    key: String,
) -> AppResult<SearchSettingsDto> {
    let kind = keyed_provider(&provider)?;
    let key = checked_key(&key).map_err(AppError::bad_request)?;
    if let Some(name) = key_name(kind) {
        state.secrets.set(name, &key).map_err(keychain_error)?;
    }
    Ok(settings_dto(
        load_settings(&state.store).await,
        state.secrets.as_ref(),
    ))
}

/// Removes a provider's API key from the keychain.
#[tauri::command]
pub async fn clear_search_key(
    state: State<'_, AppState>,
    provider: String,
) -> AppResult<SearchSettingsDto> {
    let kind = keyed_provider(&provider)?;
    if let Some(name) = key_name(kind) {
        state.secrets.delete(name).map_err(keychain_error)?;
    }
    Ok(settings_dto(
        load_settings(&state.store).await,
        state.secrets.as_ref(),
    ))
}

/// What the "Test search" button shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSearchDto {
    /// The service's name.
    pub provider: String,
    pub results: Vec<TestResultDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResultDto {
    pub title: String,
    pub site: String,
}

/// Runs one search with the saved settings and returns the first results' titles. Only
/// the search service is asked; no page is read.
#[tauri::command]
pub async fn test_search(app: AppHandle, state: State<'_, AppState>) -> AppResult<TestSearchDto> {
    let settings = load_settings(&state.store).await;
    let kind = settings.provider;
    let failed = |error: ProviderError| {
        AppError::new(
            "search",
            error.explain(kind),
            "Change the search service or its key above, then test again.",
            error.to_string(),
        )
    };
    let provider =
        provider_for(&settings, state.secrets.as_ref(), &user_agent(&app)).map_err(failed)?;
    let results = provider
        .search("open source software", 5)
        .await
        .map_err(failed)?;
    Ok(TestSearchDto {
        provider: kind.label().to_string(),
        results: results
            .iter()
            .map(|r| TestResultDto {
                title: policy::sanitize::one_line(&r.title, TITLE_CHARS),
                site: Url::parse(&r.url)
                    .map(|u| search::official::site_of(&u))
                    .unwrap_or_default(),
            })
            .collect(),
    })
}

/// A stored source as the window gets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDto {
    pub n: u32,
    pub url: String,
    pub title: String,
    pub site: String,
    /// The search service's summary, when that is what the model was given.
    pub snippet: String,
    /// The text of the page the model was given.
    pub excerpt: String,
    pub content_sha256: Option<String>,
    pub official: bool,
    pub version_note: Option<String>,
    /// The searches that found it, oldest first.
    pub queries: Vec<String>,
    pub fetched_at: Option<i64>,
    pub created_at: i64,
}

impl From<store::Source> for SourceDto {
    fn from(s: store::Source) -> Self {
        Self {
            n: s.n,
            url: s.url,
            title: s.title,
            site: s.site,
            snippet: s.snippet,
            excerpt: s.excerpt,
            content_sha256: s.content_sha256,
            official: s.official,
            version_note: s.version_note,
            queries: s.query.lines().map(str::to_string).collect(),
            fetched_at: s.fetched_at,
            created_at: s.created_at,
        }
    }
}

/// The chat's sources, by number.
#[tauri::command]
pub async fn list_sources(state: State<'_, AppState>, chat_id: i64) -> AppResult<Vec<SourceDto>> {
    Ok(state
        .store
        .list_sources(store::ChatId(chat_id))
        .await?
        .into_iter()
        .map(SourceDto::from)
        .collect())
}

// ---- The project's libraries ------------------------------------------------------------

/// The libraries of the chat's project: the manifests in its working folder, read through
/// the path sandbox exactly as `read_file` would read them. A manifest that is not inside
/// an approved folder (also after following links) is not read.
pub fn project_libraries(chat: &ChatContext) -> Vec<Library> {
    let (sandbox, _skipped) = PathSandbox::new(
        Flavor::native(),
        &chat.approved_folders,
        &chat.forbidden_folders,
        &RealFs,
        &|p| volume_case(p),
    );
    let base = chat
        .working_folder
        .clone()
        .or_else(|| sandbox.roots().first().map(|r| r.canonical.to_string()));
    let Some(base) = base else {
        return Vec::new();
    };
    search::versions::from_manifests(&|name| {
        let checked = sandbox.check(name, Some(&base), &RealFs).ok()?;
        if !checked.exists {
            return None;
        }
        let path = Path::new(&checked.canonical);
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > search::versions::MAX_MANIFEST_BYTES {
            return None;
        }
        // Checked again right before the read (13.4 step 5).
        sandbox.recheck(&checked, &RealFs).ok()?;
        std::fs::read_to_string(path).ok()
    })
}

// ---- What the model is given -------------------------------------------------------------

/// Text from the web as it is stored and shown: invisible and control characters escaped.
fn clean(text: &str) -> String {
    policy::sanitize::visible(text)
}

/// The same on one line, cut to `max` characters.
fn clean_line(text: &str, max: usize) -> String {
    policy::sanitize::one_line(text, max)
}

fn budget_chars(cap_tokens: usize) -> usize {
    cap_tokens.max(500) * CHARS_PER_TOKEN
}

/// One row of a search result: the stored source and why its page was not read, if so.
struct Row<'a> {
    n: u32,
    source: &'a store::NewSource,
    /// The page was tried and could not be read.
    error: Option<String>,
}

/// " (official documentation; matches tokio 1.38)".
fn marks(official: bool, version_note: Option<&str>) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if official {
        parts.push("official documentation");
    }
    if let Some(note) = version_note.filter(|n| !n.is_empty()) {
        parts.push(note);
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join("; "))
    }
}

fn heading(n: u32, title: &str, site: &str, official: bool, version_note: Option<&str>) -> String {
    format!("[{n}] {title} — {site}{}", marks(official, version_note))
}

/// The text of a `web_search` result.
fn format_search(query: &str, matched: Option<&Library>, rows: &[Row<'_>]) -> String {
    let numbers: Vec<String> = rows.iter().map(|r| format!("[{}]", r.n)).collect();
    let mut out = format!(
        "Sources {} for: {}\n",
        numbers.join(" "),
        clean_line(query, 200)
    );
    if let Some(library) = matched.filter(|l| !l.version.is_empty()) {
        out.push_str(&format!(
            "Matched to {} {}, which this project uses.\n",
            clean_line(library.short_name(), 80),
            clean_line(&library.version, 40)
        ));
    }
    let tried = rows
        .iter()
        .filter(|r| r.error.is_some() || !r.source.excerpt.is_empty())
        .count();
    let read = rows.iter().filter(|r| !r.source.excerpt.is_empty()).count();
    if tried > 0 && read == 0 {
        out.push_str(
            "None of the pages could be read, so only the search service's summaries are given.\n",
        );
    }
    for row in rows {
        let s = row.source;
        out.push('\n');
        out.push_str(&heading(
            row.n,
            &s.title,
            &s.site,
            s.official,
            s.version_note.as_deref(),
        ));
        out.push('\n');
        out.push_str(&s.url);
        out.push('\n');
        if !s.excerpt.is_empty() {
            out.push_str(&s.excerpt);
        } else {
            match &row.error {
                Some(reason) => out.push_str(&format!(
                    "(This page could not be read: {reason}. The search service's summary:)\n"
                )),
                None => out.push_str(&format!(
                    "(Not read. The search service's summary; read_source {{\"n\": {}}} reads the page.)\n",
                    row.n
                )),
            }
            out.push_str(if s.snippet.is_empty() {
                "(no summary)"
            } else {
                &s.snippet
            });
        }
        out.push('\n');
    }
    out.push('\n');
    out.push_str(CITE_HINT);
    out
}

/// What a search is asked to do, from the tool's arguments and the settings.
pub(crate) struct SearchCall<'a> {
    pub query: &'a str,
    pub library: Option<&'a str>,
    pub max_results: usize,
    pub pages: usize,
    pub libraries: Vec<Library>,
    pub ask_registry: bool,
    pub cap_tokens: usize,
}

/// What runs a search for `web_search`, so that tests can stand in for it.
#[async_trait]
pub(crate) trait Searcher: Send + Sync {
    async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchOutcome, SearchError>;
}

#[async_trait]
impl Searcher for Engine {
    async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchOutcome, SearchError> {
        Engine::search(self, query, options).await
    }
}

/// What reads a page for `read_source`, so that tests can stand in for it.
#[async_trait]
pub(crate) trait PageReader: Send + Sync {
    async fn read_page(&self, url: &Url, query: &str) -> Result<PageRead, FetchError>;
}

#[async_trait]
impl PageReader for Fetcher {
    async fn read_page(&self, url: &Url, query: &str) -> Result<PageRead, FetchError> {
        search::pipeline::read_page(self, url, query).await
    }
}

/// Runs one `web_search`: searches, stores each hit under its number and returns the text
/// for the model. The second value says whether the chat's sources changed.
pub(crate) async fn run_web_search(
    store: &store::Store,
    backend: &dyn Searcher,
    chat_id: i64,
    call: SearchCall<'_>,
) -> (ToolOutput, bool) {
    let budget = budget_chars(call.cap_tokens);
    // No more hits than the result has room for.
    let max_results = call
        .max_results
        .clamp(1, search::provider::MAX_RESULTS)
        .min((budget / (HIT_OVERHEAD_CHARS + MIN_EXCERPT_CHARS)).max(2));
    let options = SearchOptions {
        library: call.library.map(str::to_string),
        libraries: call.libraries,
        max_results,
        pages: call.pages.min(max_results),
        passages_per_page: search::pipeline::DEFAULT_PASSAGES,
        ask_registry: call.ask_registry,
    };
    let outcome = match backend.search(call.query, &options).await {
        Ok(outcome) => outcome,
        Err(error) => {
            return (
                ToolOutput::error(format!("The search could not be run. {}", error.explain())),
                false,
            );
        }
    };
    if outcome.hits.is_empty() {
        return (
            ToolOutput::text(format!(
                "No results for: {}\nTry other words, or fewer of them.",
                clean_line(call.query, 200)
            )),
            false,
        );
    }
    // Room for each excerpt: what is left after the fixed parts, shared by the pages read.
    let read = outcome.hits.iter().filter(|h| h.was_read()).count();
    let unread = outcome.hits.len() - read;
    let fixed = 400 + outcome.hits.len() * HIT_OVERHEAD_CHARS + unread * SNIPPET_CHARS;
    let excerpt_chars =
        (budget.saturating_sub(fixed) / read.max(1)).clamp(MIN_EXCERPT_CHARS, MAX_EXCERPT_CHARS);

    let mut errors: Vec<Option<String>> = Vec::new();
    let new_sources: Vec<store::NewSource> = outcome
        .hits
        .iter()
        .map(|hit| {
            let excerpt = clean(&hit.excerpt(excerpt_chars));
            errors.push(
                hit.page_error
                    .as_deref()
                    .map(|reason| clean_line(reason, 240)),
            );
            store::NewSource {
                url: hit.url.clone(),
                title: clean_line(&hit.title, TITLE_CHARS),
                site: clean_line(&hit.site, 120),
                // The summary is stored when it is what the model is given: for a page
                // that was not read.
                snippet: if excerpt.is_empty() {
                    clean_line(&hit.snippet, SNIPPET_CHARS)
                } else {
                    String::new()
                },
                excerpt,
                content_sha256: hit.content_sha256.clone(),
                official: hit.official,
                version_note: hit
                    .version_note
                    .as_deref()
                    .map(|note| clean_line(note, 120)),
                fetched_at: hit.fetched_at,
            }
        })
        .collect();
    let stored = match store
        .record_sources(
            store::ChatId(chat_id),
            clean_line(call.query, 200),
            new_sources.clone(),
        )
        .await
    {
        Ok(stored) => stored,
        Err(error) => {
            tracing::error!("could not store the sources of a search: {error}");
            return (
                ToolOutput::error(
                    "The search ran, but its sources could not be saved, so they cannot be cited. Try the search again.",
                ),
                false,
            );
        }
    };
    // A hit is given to the model only under the number it was stored with.
    let rows: Vec<Row<'_>> = new_sources
        .iter()
        .zip(errors)
        .filter_map(|(source, error)| {
            let n = stored.iter().find(|s| s.url == source.url)?.n;
            Some(Row { n, source, error })
        })
        .collect();
    if rows.is_empty() {
        return (
            ToolOutput::error(
                "The search ran, but this chat cannot hold more sources. Start a new chat to go on searching.",
            ),
            false,
        );
    }
    (
        ToolOutput::text(format_search(call.query, outcome.library.as_ref(), &rows)),
        true,
    )
}

/// Runs one `read_source`: reads the page of source `n` again and returns passages the
/// model was not given yet, which are then added to the stored excerpt.
pub(crate) async fn run_read_source(
    store: &store::Store,
    reader: &dyn PageReader,
    chat_id: i64,
    n: u32,
    query: Option<&str>,
    cap_tokens: usize,
) -> (ToolOutput, bool) {
    let chat = store::ChatId(chat_id);
    let source = match store.get_source(chat, n).await {
        Ok(Some(source)) => source,
        Ok(None) => {
            return (
                ToolOutput::error(no_such_source(store, chat, n).await),
                false,
            );
        }
        Err(error) => {
            tracing::error!("could not read a source: {error}");
            return (
                ToolOutput::error("The source could not be looked up. Try again."),
                false,
            );
        }
    };
    let Ok(url) = Url::parse(&source.url) else {
        return (
            ToolOutput::error(format!("Source [{n}] has no address that can be read.")),
            false,
        );
    };
    let asked = query.map(str::trim).filter(|q| !q.is_empty());
    // Ranked for what is asked now; with no question, the page is read on in its own order.
    let page = match reader.read_page(&url, asked.unwrap_or("")).await {
        Ok(page) => page,
        Err(error) => {
            return (
                ToolOutput::error(format!(
                    "Source [{n}] could not be read: {}. What you were given of it before still stands.",
                    clean_line(&error.to_string(), 240)
                )),
                false,
            );
        }
    };
    let total = page.passages.len();
    let mut fresh: Vec<Passage> = page
        .passages
        .into_iter()
        .map(|mut passage| {
            passage.text = clean(&passage.text);
            passage
        })
        .filter(|passage| !source.excerpt.contains(passage.text.as_str()))
        .collect();
    if asked.is_none() {
        fresh.sort_by_key(|passage| passage.index);
    }
    let head = heading(
        n,
        &source.title,
        &source.site,
        source.official,
        source.version_note.as_deref(),
    );
    if fresh.is_empty() {
        return (
            ToolOutput::text(format!(
                "{head}\n{}\nNothing more: you were already given all {total} passages of this page.",
                source.url
            )),
            false,
        );
    }
    let room = budget_chars(cap_tokens)
        .saturating_sub(500)
        .clamp(MIN_EXCERPT_CHARS, MAX_MORE_CHARS);
    let more = search::rank::excerpt(&fresh, room);
    let changed = source
        .content_sha256
        .as_deref()
        .is_some_and(|before| before != page.content_sha256);
    if let Err(error) = store
        .extend_source(
            chat,
            n,
            more.clone(),
            page.content_sha256.clone(),
            page.fetched_at,
        )
        .await
    {
        tracing::error!("could not store more of a source: {error}");
        return (
            ToolOutput::error(
                "The page was read, but the text could not be saved with the source, so it is not given. Try again.",
            ),
            false,
        );
    }
    let mut out = format!("{head}\n{}\n", source.url);
    out.push_str(&match asked {
        Some(asked) => format!("More of this page, for: {}\n", clean_line(asked, 200)),
        None => "More of this page:\n".to_string(),
    });
    if changed {
        out.push_str("(The page has changed since it was first read.)\n");
    }
    out.push('\n');
    out.push_str(&more);
    out.push_str(&format!(
        "\n\nCite it as [{n}]. Use only what the text says."
    ));
    (ToolOutput::text(out), true)
}

/// What the model is told when it names a source the chat does not have.
async fn no_such_source(store: &store::Store, chat: store::ChatId, n: u32) -> String {
    let known = store.list_sources(chat).await.unwrap_or_default();
    match (known.first(), known.last()) {
        (Some(first), Some(last)) if first.n == last.n => format!(
            "There is no source [{n}] in this chat. The only source is [{}].",
            first.n
        ),
        (Some(first), Some(last)) => format!(
            "There is no source [{n}] in this chat. The sources are [{}] to [{}].",
            first.n, last.n
        ),
        _ => format!(
            "There is no source [{n}] in this chat: no search has found anything yet. Use web_search first."
        ),
    }
}

// ---- Describing calls for the policy gate ------------------------------------------------

fn text_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn action(tool: &str, summary: String, preview: String, payload: Value) -> policy::Action {
    policy::Action {
        server: SERVER_NAME.into(),
        tool: tool.into(),
        summary,
        preview,
        scope: policy::Scope::WebSearch,
        risk: policy::Risk::Normal,
        // It changes nothing anywhere, and it sends the question (or a request for a page)
        // off this computer.
        read_only: true,
        outbound: true,
        keep_asking: None,
        network: true,
        hard_limit: None,
        payload,
    }
}

/// The action of a `web_search` call, or why the call cannot run.
fn describe_search(
    args: &Map<String, Value>,
    settings: &SearchSettings,
    key_set: bool,
) -> Described {
    let Some(query) = text_arg(args, "query") else {
        return Described::Invalid("query is empty: say what to search for".into());
    };
    let query = search::provider::clean_query(query);
    if query.is_empty() {
        return Described::Invalid("query is empty: say what to search for".into());
    }
    let kind = settings.provider;
    // Refused here, before a card is shown for a search that cannot run.
    if kind.needs_key() && !key_set {
        return Described::Invalid(ProviderError::NoKey.explain(kind));
    }
    if kind == ProviderKind::Searxng && settings.searxng_url.is_empty() {
        return Described::Invalid(ProviderError::NoAddress.explain(kind));
    }
    let library = text_arg(args, "library").map(|name| clean_line(name, 80));
    let goes_to = match kind {
        ProviderKind::Searxng => format!(
            "your SearXNG instance at {}",
            Url::parse(&settings.searxng_url)
                .ok()
                .and_then(|u| policy::sanitize::display_host(&u))
                .unwrap_or_default()
        ),
        other => other.label().to_string(),
    };
    let mut preview = format!("Search the web for:\n{}\n", clean(&query));
    if let Some(library) = &library {
        preview.push_str(&format!("\nLibrary: {library}\n"));
    }
    preview.push_str(&format!("\nThe search is sent to {goes_to}."));
    preview.push_str(&match settings.pages {
        0 => " No page is read.".to_string(),
        1 => " The first page it finds is then read from its site.".to_string(),
        pages => {
            format!(" Up to {pages} of the pages it finds are then read from their sites.")
        }
    });
    Described::Action(action(
        WEB_SEARCH,
        format!("Search the web: {}", clean_line(&query, 120)),
        preview,
        json!({
            "query": query,
            "library": library,
            "provider": kind.as_str(),
            "source": format!("web search ({})", kind.label()),
        }),
    ))
}

/// The action of a `read_source` call for a stored source.
fn describe_read(n: u32, query: Option<&str>, source: &store::Source) -> Described {
    let mut preview = format!(
        "Read more of source [{n}]:\n{}\n{}",
        clean(&source.title),
        clean(&source.url)
    );
    if let Some(query) = query {
        preview.push_str(&format!("\n\nLooking for: {}", clean_line(query, 200)));
    }
    preview.push_str("\n\nThe page is read from its site again.");
    Described::Action(action(
        READ_SOURCE,
        format!(
            "Read more of source [{n}]: {}",
            clean_line(&source.title, 100)
        ),
        preview,
        json!({ "n": n, "url": source.url }),
    ))
}

// ---- The built-in server --------------------------------------------------------------

/// The two tools as they are offered to the model.
fn tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: WEB_SEARCH.into(),
            description: WEB_SEARCH_DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "description": "What to search for, in a few words."
                    },
                    "library": {
                        "type": "string",
                        "description": "The library or framework the question is about, if any (for example tokio or react)."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 10,
                        "description": "How many results to return (default 6)."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            destructive: Some(false),
            read_only: Some(true),
        },
        ToolDef {
            name: READ_SOURCE.into(),
            description: READ_SOURCE_DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "n": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "The number of the source, as in [n]."
                    },
                    "query": {
                        "type": "string",
                        "description": "What to look for on the page."
                    }
                },
                "required": ["n"],
                "additionalProperties": false
            }),
            destructive: Some(false),
            read_only: Some(true),
        },
    ]
}

/// The `search` server. It holds what package indexes said about libraries for as long as
/// the app runs.
pub struct SearchServer {
    app: AppHandle,
    registry: Arc<Registry>,
}

impl SearchServer {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            registry: Arc::new(Registry::new()),
        }
    }

    fn engine(&self, settings: &SearchSettings) -> Result<Engine, ProviderError> {
        let state = self.app.state::<AppState>();
        let user_agent = user_agent(&self.app);
        let provider = provider_for(settings, state.secrets.as_ref(), &user_agent)?;
        Ok(Engine::new(
            provider,
            Arc::new(Fetcher::new(user_agent)),
            self.registry.clone(),
        ))
    }

    async fn web_search(
        &self,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: &CallContext,
    ) -> (ToolOutput, bool) {
        let state = self.app.state::<AppState>();
        let settings = load_settings(&state.store).await;
        // What runs is what was shown: the card named the service the search goes to.
        if action.payload.get("provider").and_then(Value::as_str)
            != Some(settings.provider.as_str())
        {
            return (
                ToolOutput::error(
                    "The search service was changed in Settings after this search was approved. Search again.",
                ),
                false,
            );
        }
        let Some(query) = action.payload.get("query").and_then(Value::as_str) else {
            return (
                ToolOutput::error("internal error: the approved search has no question"),
                false,
            );
        };
        let engine = match self.engine(&settings) {
            Ok(engine) => engine,
            Err(error) => {
                return (
                    ToolOutput::error(format!(
                        "The search could not be run. {}",
                        error.explain(settings.provider)
                    )),
                    false,
                );
            }
        };
        let libraries = if settings.version_matching {
            let chat = ctx.chat.clone();
            tokio::task::spawn_blocking(move || project_libraries(&chat))
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if let Some(progress) = &ctx.progress {
            progress(format!("Searching with {}…\n", settings.provider.label()));
        }
        let call = SearchCall {
            query,
            library: text_arg(args, "library"),
            max_results: args
                .get("max_results")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_RESULTS, |n| n as usize),
            pages: usize::from(settings.pages),
            libraries,
            ask_registry: settings.version_matching,
            cap_tokens: ctx.chat.result_cap_tokens,
        };
        run_web_search(&state.store, &engine, ctx.chat.chat_id, call).await
    }

    async fn read_source(
        &self,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: &CallContext,
    ) -> (ToolOutput, bool) {
        let state = self.app.state::<AppState>();
        let Some(n) = action
            .payload
            .get("n")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
        else {
            return (
                ToolOutput::error("internal error: the approved call names no source"),
                false,
            );
        };
        // What runs is what was shown: the address on the card is the one that is fetched.
        let stored = state
            .store
            .get_source(store::ChatId(ctx.chat.chat_id), n)
            .await
            .ok()
            .flatten();
        if stored.map(|s| s.url).as_deref() != action.payload.get("url").and_then(Value::as_str) {
            return (
                ToolOutput::error(format!(
                    "Source [{n}] changed after this call was approved. Call read_source again."
                )),
                false,
            );
        }
        // Reading a page needs no search service: only the guarded fetcher.
        let reader = Fetcher::new(user_agent(&self.app));
        run_read_source(
            &state.store,
            &reader,
            ctx.chat.chat_id,
            n,
            text_arg(args, "query"),
            ctx.chat.result_cap_tokens,
        )
        .await
    }
}

#[async_trait]
impl BuiltinServer for SearchServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        tool_defs()
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let state = self.app.state::<AppState>();
        match tool {
            WEB_SEARCH => {
                let settings = load_settings(&state.store).await;
                let key_set = key_is_set(state.secrets.as_ref(), settings.provider);
                describe_search(args, &settings, key_set)
            }
            READ_SOURCE => {
                let n = args
                    .get("n")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n >= 1);
                let Some(n) = n else {
                    return Described::Invalid(
                        "n must be the number of a source, as in [n]".into(),
                    );
                };
                let chat_id = store::ChatId(chat.chat_id);
                match state.store.get_source(chat_id, n).await {
                    Ok(Some(source)) => describe_read(n, text_arg(args, "query"), &source),
                    Ok(None) => Described::Invalid(no_such_source(&state.store, chat_id, n).await),
                    Err(error) => {
                        tracing::error!("could not read a source: {error}");
                        Described::Invalid("The source could not be looked up. Try again.".into())
                    }
                }
            }
            other => Described::Invalid(format!("the search server has no tool named {other}")),
        }
    }

    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        let chat_id = ctx.chat.chat_id;
        let work = async {
            match tool {
                WEB_SEARCH => self.web_search(args, action, &ctx).await,
                READ_SOURCE => self.read_source(args, action, &ctx).await,
                other => (
                    ToolOutput::error(format!("the search server has no tool named {other}")),
                    false,
                ),
            }
        };
        let (output, changed) = tokio::select! {
            done = work => done,
            () = ctx.cancel.cancelled() => (ToolOutput::error("Stopped by the user."), false),
        };
        if changed {
            let _ = self
                .app
                .emit_to(crate::MAIN_WINDOW, SOURCES_CHANGED, chat_id);
        }
        output
    }
}

#[cfg(test)]
mod tests;
