//! Facts for Models → Compare (0.0.6 item 4, WP4a): `catalog_list` and `compare_facts`.
//!
//! Where each fact comes from (D6), and the rules that keep them honest:
//! - **The signed catalog** (`From the publisher's model card`): attached to a column only
//!   on an *exact* match: the same SHA-256 for every file (every shard of a split model),
//!   the same repository and file path, or the catalog id (an id saved at download
//!   counts, unless the file's SHA-256 says it is another file). An installed model keeps
//!   only its first file's SHA-256, which decides alone only against a single file. A
//!   Hugging Face repository whose uploader names a catalog model's
//!   source as its base model gets that entry's text only as [`BaseModelFacts`], shown
//!   under "The uploader says this is made from …", stripped of the capabilities, sizes and
//!   licence, and never used for the fit.
//! - **Hugging Face** (`The uploader lists`): saved at download for installed models, or
//!   fetched through [`HfClient`] for the one repository of a column (never a bulk fetch;
//!   at most [`COMPARE_LIMIT`] columns a call). All uploader text is made safe in
//!   `models::hf` (invisible characters shown, capped at 300 characters, unknown tags
//!   dropped). Offline, rate limits (429) and gated repositories become a column's
//!   [`CompareProblem`], never an error for the whole call.
//! - **The GGUF header** (`From the model file`): the installed file itself, or the first
//!   bytes of one file of the repository (the same file the repository page reads).
//! - **The fit** (one verdict per column at the chosen size): an installed file is judged
//!   as Installed and Settings for this model judge it ([`installed_fit`]: its own
//!   settings, `placement::judge_in`, the settings panel's amounts), and so is it on the
//!   repository page and in Change ([`variant_fit`]); a file that is not downloaded gets
//!   the same estimate as the repository page, from that column's own file only. A plan
//!   is made only for the installed files a call shows ([`Plans`]). The screen words the
//!   figures (`fitLine` in src/lib/words.ts).
//! - **Your results**: saved model checks and benchmark runs with the same file name and
//!   size as the column's file.
//!
//! Installed files are read only when they are in the installed list: a path the webview
//! sends that is not an installed model is "not found", and nothing is read from it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use models::Verdict;
use models::catalog::{Catalog, Entry};
use models::fit::{FitDevice, FitInput, Target};
use models::hf::{Gated, HfClient, HfError, RepoDetail, UPLOADER_TEXT_CAP, uploader_text};
use models::placement::{Fits, MemoryKind};
use models::summary::ModelSummary;

use super::StoredSummary;
use crate::dto::{
    BaseModelFacts, CatalogFacts, CatalogListEntry, CompareFacts, CompareItem, CompareProblem,
    FileFacts, HubFacts, LicenseFacts, YourResult, YourResultKind,
};
use crate::evals::bench::BenchRun;
use crate::evals::profile::{Identity, Profile};
use crate::load_settings::Plan;

/// The most models one comparison holds (the screen's limit, `COMPARE_LIMIT` in
/// `src/store/compare.ts`). A longer list is refused: it would be a bulk fetch.
pub const COMPARE_LIMIT: usize = 4;
/// The sizes a comparison may be checked at, in tokens.
const MIN_CTX: u32 = 512;
const MAX_CTX: u32 = 1_048_576;

/// The size a comparison is checked at: the one asked for, kept within sane bounds.
pub fn clamp_ctx(ctx: u32) -> u32 {
    ctx.clamp(MIN_CTX, MAX_CTX)
}

// ---- What the app already knows ----------------------------------------------------

/// One installed model, as the installed list stores it.
#[derive(Debug, Clone)]
pub struct Installed {
    pub path: String,
    pub repo: Option<String>,
    pub file_name: String,
    /// All of the model's files (every shard of a split model).
    pub size: u64,
    pub sha256: Option<String>,
    pub stored: StoredSummary,
}

impl Installed {
    pub fn from_record(m: &store::ModelRecord) -> Self {
        Self {
            path: m.path.clone(),
            repo: m.repo.clone(),
            file_name: m.file_name.clone(),
            size: m.size_bytes,
            sha256: m.sha256.clone().filter(|s| !s.is_empty()),
            stored: serde_json::from_value(m.gguf_summary.clone()).unwrap_or_default(),
        }
    }

    /// The size of the file that is loaded (the first shard), which is what model checks
    /// remember; the stored total when the file cannot be read.
    fn first_file_size(&self) -> u64 {
        std::fs::metadata(&self.path)
            .map(|m| m.len())
            .unwrap_or(self.size)
    }

    /// The repository's path of the file, saved at download, else `None`.
    fn repo_file(&self) -> Option<&str> {
        self.stored.repo_file.as_deref()
    }
}

/// One of the user's results, with the file it was made with.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalResult {
    pub file: String,
    pub size: u64,
    pub result: YourResult,
}

/// Everything a comparison needs that is on this computer.
pub struct Known<P = HashMap<String, Plan>> {
    pub catalog: Catalog,
    pub installed: Vec<Installed>,
    pub target: Target,
    /// Memory the model may use, for the order of Suggested models.
    pub usable_bytes: u64,
    pub results: Vec<LocalResult>,
    /// How a load of an installed model would go now, with its own settings: what
    /// Installed and Settings for this model judge ([`installed_fit`]). Asked only for the
    /// installed files a column or a suggestion shows.
    pub plans: P,
}

/// How a load of the installed model at a path would go now, with its own settings
/// (`load_settings::plan` in the app; plans made beforehand in tests). Asked only for the
/// installed files a comparison, a suggestion or a repository page shows (compare-fit-minors):
/// a plan reads the file's header, and the app remembers only the last 32 headers it read,
/// which Settings for this model relies on too.
pub trait Plans {
    /// `None` when no plan can be made for the file.
    async fn plan(&self, path: &str) -> Option<Plan>;
}

/// Plans made beforehand, by path.
impl Plans for HashMap<String, Plan> {
    async fn plan(&self, path: &str) -> Option<Plan> {
        self.get(path).cloned()
    }
}

/// Two SHA-256 values that are both known and differ: certainly not the same file.
fn sha_differs(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if !a.eq_ignore_ascii_case(b))
}

/// The SHA-256 values of a model's files, in order (every shard of a split model), each
/// `None` when not known.
type Shas<'a> = [Option<&'a str>];

/// What the SHA-256 values of two models' files say: `Some(true)` every file the same
/// bytes, `Some(false)` certainly other bytes (the first files differ, or every value is
/// known and they differ), `None` not known (a value missing on either side).
fn same_bytes(a: &Shas, b: &Shas) -> Option<bool> {
    let (first_a, first_b) = (a.first().copied().flatten(), b.first().copied().flatten());
    if sha_differs(first_a, first_b) {
        return Some(false);
    }
    let all_known = |s: &Shas| !s.is_empty() && s.iter().all(Option::is_some);
    if !(all_known(a) && all_known(b)) {
        return None;
    }
    let sorted = |s: &Shas| {
        let mut v: Vec<String> = s.iter().flatten().map(|x| x.to_ascii_lowercase()).collect();
        v.sort();
        v
    };
    Some(sorted(a) == sorted(b))
}

/// The same file of the same repository: equal SHA-256 for every file when all are
/// known (the same bytes, wherever they were uploaded), else the same repository and the
/// same path of the first file.
fn same_remote_files(
    (repo_a, file_a, shas_a): (&str, &str, &Shas),
    (repo_b, file_b, shas_b): (&str, &str, &Shas),
) -> bool {
    match same_bytes(shas_a, shas_b) {
        Some(same) => same,
        None => repo_a.eq_ignore_ascii_case(repo_b) && file_a == file_b,
    }
}

/// The SHA-256 values of a catalog entry's files (the catalog checks every one is there).
fn entry_shas(e: &Entry) -> Vec<Option<&str>> {
    e.files.iter().map(|f| f.sha256.as_deref()).collect()
}

/// `path` ends with `/<owner>/<name>/<file>`, the place downloads put a repository's file.
fn path_is_download_of(path: &str, repo: &str, file: &str) -> bool {
    let path = path.replace('\\', "/");
    let tail = format!("/{repo}/{file}");
    path.len() >= tail.len()
        && path.is_char_boundary(path.len() - tail.len())
        && path[path.len() - tail.len()..].eq_ignore_ascii_case(&tail)
}

/// Whether an installed model is exactly this repository file, whose files have the
/// SHA-256 values `shas` (empty when not known). The installed list keeps only the
/// first file's SHA-256: different, it is another model; the same, it decides alone only
/// against a single file (a split model's other files could differ). Otherwise the
/// repository and the path decide.
fn installed_is(rec: &Installed, repo: &str, file: &str, shas: &Shas) -> bool {
    if let (Some(a), Some(b)) = (rec.sha256.as_deref(), shas.first().copied().flatten()) {
        if !a.eq_ignore_ascii_case(b) {
            return false;
        }
        if shas.len() == 1 {
            return true;
        }
    }
    let Some(own_repo) = rec.repo.as_deref() else {
        return false;
    };
    if !own_repo.eq_ignore_ascii_case(repo) {
        return false;
    }
    match rec.repo_file() {
        Some(f) => f == file,
        None => path_is_download_of(&rec.path, repo, file),
    }
}

/// Whether an installed model is exactly this catalog entry.
fn installed_is_entry(rec: &Installed, id: &str, e: &Entry) -> bool {
    let first = &e.files[0];
    installed_is(rec, &e.repo, &first.path, &entry_shas(e))
        || (rec.stored.catalog_id.as_deref() == Some(id)
            && !sha_differs(rec.sha256.as_deref(), first.sha256.as_deref()))
}

/// The installed model that is exactly this repository file (Compare's rule, [`installed_is`]),
/// whose files have the SHA-256 values `shas` (empty when not known).
fn installed_file<'a>(
    installed: &'a [Installed],
    repo: &str,
    file: &str,
    shas: &Shas,
) -> Option<&'a Installed> {
    installed.iter().find(|r| installed_is(r, repo, file, shas))
}

impl<P> Known<P> {
    fn installed_by_path(&self, path: &str) -> Option<&Installed> {
        self.installed.iter().find(|r| r.path == path)
    }

    fn installed_for_entry(&self, id: &str, e: &Entry) -> Option<&Installed> {
        self.installed.iter().find(|r| installed_is_entry(r, id, e))
    }

    fn installed_for_file(&self, repo: &str, file: &str, shas: &Shas) -> Option<&Installed> {
        installed_file(&self.installed, repo, file, shas)
    }

    /// The catalog entry that is exactly this repository file, whose files (every shard)
    /// have the SHA-256 values `shas` (empty when not known).
    fn entry_for_file(&self, repo: &str, file: &str, shas: &Shas) -> Option<(&str, &Entry)> {
        self.catalog
            .models
            .iter()
            .find(|(_, e)| {
                same_remote_files(
                    (&e.repo, &e.files[0].path, &entry_shas(e)),
                    (repo, file, shas),
                )
            })
            .map(|(id, e)| (id.as_str(), e))
    }

    /// The catalog entry that is exactly this installed model.
    fn entry_for_installed(&self, rec: &Installed) -> Option<(&str, &Entry)> {
        self.catalog
            .models
            .iter()
            .find(|(id, e)| installed_is_entry(rec, id, e))
            .map(|(id, e)| (id.as_str(), e))
    }

    /// A catalog entry whose publisher's model card is the repository `base`, for the
    /// uploader's claim that a repository is made from it.
    fn entry_with_source(&self, base: &str) -> Option<(&str, &Entry)> {
        self.catalog
            .models
            .iter()
            .find(|(_, e)| {
                e.source_repo()
                    .is_some_and(|s| s.eq_ignore_ascii_case(base))
            })
            .map(|(id, e)| (id.as_str(), e))
    }

    /// The user's results with this file (name and size), newest first.
    fn results_for(&self, file: &str, size: u64) -> Vec<YourResult> {
        let name = file_name(file);
        let mut out: Vec<YourResult> = self
            .results
            .iter()
            .filter(|r| r.file == name && r.size == size)
            .map(|r| r.result.clone())
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.at));
        out
    }
}

/// The last part of a `/`-separated (or `\`-separated) path.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

// ---- Your results ----------------------------------------------------------------

/// The backends a result can name.
const BACKENDS: [&str; 5] = ["vulkan", "cuda", "rocm", "metal", "cpu"];

/// "file.gguf (vulkan)" → "vulkan", when it is a backend.
fn backend_of_label(label: &str) -> Option<String> {
    let inner = label.trim_end().strip_suffix(')')?;
    let (_, b) = inner.rsplit_once(" (")?;
    let b = b.to_ascii_lowercase();
    BACKENDS.contains(&b.as_str()).then_some(b)
}

/// The user's results, from saved model checks and benchmark runs. A benchmark run is
/// matched by its file name and the size the file has now (`bench`: each run with the
/// size of the file at its path, or `None` when it is gone); cancelled runs and runs
/// that ran nothing are left out.
pub fn local_results(
    profiles: &[(String, Profile)],
    bench: &[(BenchRun, Option<u64>)],
) -> Vec<LocalResult> {
    let mut out = Vec::new();
    for (key, p) in profiles {
        let Identity::Local {
            file,
            size,
            backend,
        } = &p.identity
        else {
            continue;
        };
        let chosen = p
            .chosen
            .as_ref()
            .and_then(|c| p.results.iter().find(|v| &v.label == c))
            .or_else(|| p.results.iter().max_by_key(|v| v.passed));
        let Some(v) = chosen else {
            continue;
        };
        let total = v.probes.len() as u32;
        if total == 0 {
            continue;
        }
        out.push(LocalResult {
            file: file.clone(),
            size: *size,
            result: YourResult {
                kind: YourResultKind::ModelCheck,
                id: key.clone(),
                at: p.checked_at,
                backend: Some(backend.to_ascii_lowercase()),
                passed: v.passed.min(total),
                total,
            },
        });
    }
    for (run, size) in bench {
        let (Some(path), Some(size)) = (run.model_ref.strip_prefix("local:"), size) else {
            continue;
        };
        if run.cancelled || run.totals.ran == 0 {
            continue;
        }
        out.push(LocalResult {
            file: file_name(path).to_string(),
            size: *size,
            result: YourResult {
                kind: YourResultKind::Benchmark,
                id: run.id.clone(),
                at: run.started_at,
                backend: backend_of_label(&run.model),
                passed: run.totals.completed.min(run.totals.ran),
                total: run.totals.ran,
            },
        });
    }
    out
}

// ---- Hugging Face ---------------------------------------------------------------

/// What the comparison asks of Hugging Face: one repository's listing and the first
/// bytes of one of its files. [`LiveHub`] in the app; a fake in tests.
pub trait Hub {
    /// The listing of a repository (at `revision`, or its latest commit).
    async fn listing(&self, repo: &str, revision: Option<&str>)
    -> Result<Arc<RepoDetail>, HfError>;
    /// The header of one file at `commit`, read from its first bytes.
    async fn head(
        &self,
        repo: &str,
        commit: &str,
        file: &str,
    ) -> Result<Arc<ModelSummary>, HfError>;
    /// A header read earlier in this session, without asking the network.
    fn cached_head(&self, repo: &str, commit: &str, file: &str) -> Option<Arc<ModelSummary>>;
    /// A listing read earlier in this session, without asking the network.
    fn cached_listing(&self, repo: &str, revision: Option<&str>) -> Option<Arc<RepoDetail>>;
    /// The repository's page ("Show model card").
    fn page_url(&self, repo: &str) -> String;
}

/// How long a repository listing (download counts, latest commit) is reused.
const LISTING_TTL: Duration = Duration::from_secs(600);
/// Listings and headers remembered at most; the oldest goes first.
const CACHE_MAX: usize = 64;

/// What was fetched in this session, so changing "Compare at" or opening a repository
/// page again does not ask Hugging Face again. In memory only.
#[derive(Default)]
struct Cache {
    listings: HashMap<String, (Instant, Arc<RepoDetail>)>,
    /// Headers by commit, which never change.
    heads: HashMap<String, (Instant, Arc<ModelSummary>)>,
    /// Headers of installed files, by path, size and modification time.
    local: HashMap<String, (Instant, Arc<ModelSummary>)>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

fn remember<T>(map: &mut HashMap<String, (Instant, T)>, key: String, value: T) {
    if map.len() >= CACHE_MAX
        && !map.contains_key(&key)
        && let Some(oldest) = map
            .iter()
            .min_by_key(|(_, (t, _))| *t)
            .map(|(k, _)| k.clone())
    {
        map.remove(&oldest);
    }
    map.insert(key, (Instant::now(), value));
}

/// A 40-hex commit (listings and headers at a commit never change).
fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Hugging Face through the user's [`HfClient`] (token and mirror as everywhere else),
/// with the session cache. Without a client every request answers "offline".
pub struct LiveHub {
    client: Option<HfClient>,
}

impl LiveHub {
    pub fn new(client: Option<HfClient>) -> Self {
        Self { client }
    }

    fn client(&self) -> Result<&HfClient, HfError> {
        self.client
            .as_ref()
            .ok_or_else(|| HfError::Network("no Hugging Face client".into()))
    }

    fn key(&self, repo: &str, rest: &[&str]) -> Option<String> {
        let c = self.client.as_ref()?;
        Some(format!(
            "{}|{}|{}|{}",
            c.endpoint(),
            c.has_token(),
            repo.to_ascii_lowercase(),
            rest.join("|")
        ))
    }

    /// Remembers a listing another command fetched (the repository page).
    pub fn remember_listing(&self, revision: Option<&str>, detail: Arc<RepoDetail>) {
        let Some(key) = self.key(&detail.id, &[revision.unwrap_or("")]) else {
            return;
        };
        if let Ok(mut c) = CACHE.lock() {
            remember(&mut c.listings, key, detail);
        }
    }
}

impl Hub for LiveHub {
    async fn listing(
        &self,
        repo: &str,
        revision: Option<&str>,
    ) -> Result<Arc<RepoDetail>, HfError> {
        if let Some(d) = self.cached_listing(repo, revision) {
            return Ok(d);
        }
        let detail = Arc::new(self.client()?.repo(repo, revision).await?);
        self.remember_listing(revision, detail.clone());
        Ok(detail)
    }

    async fn head(
        &self,
        repo: &str,
        commit: &str,
        file: &str,
    ) -> Result<Arc<ModelSummary>, HfError> {
        if let Some(s) = self.cached_head(repo, commit, file) {
            return Ok(s);
        }
        let bytes = self
            .client()?
            .fetch_head(repo, commit, file, super::HEAD_BYTES)
            .await?;
        let header =
            models::gguf::read_prefix(&bytes).map_err(|e| HfError::Parse(e.to_string()))?;
        let summary = Arc::new(ModelSummary::from_header(&header));
        if is_commit(commit)
            && let Some(key) = self.key(repo, &[commit, file])
            && let Ok(mut c) = CACHE.lock()
        {
            remember(&mut c.heads, key, summary.clone());
        }
        Ok(summary)
    }

    fn cached_head(&self, repo: &str, commit: &str, file: &str) -> Option<Arc<ModelSummary>> {
        let key = self.key(repo, &[commit, file])?;
        let c = CACHE.lock().ok()?;
        c.heads.get(&key).map(|(_, s)| s.clone())
    }

    fn cached_listing(&self, repo: &str, revision: Option<&str>) -> Option<Arc<RepoDetail>> {
        let key = self.key(repo, &[revision.unwrap_or("")])?;
        let c = CACHE.lock().ok()?;
        let (at, d) = c.listings.get(&key)?;
        (at.elapsed() < LISTING_TTL || revision.is_some_and(is_commit)).then(|| d.clone())
    }

    fn page_url(&self, repo: &str) -> String {
        self.client
            .as_ref()
            .and_then(|c| c.page_url(repo))
            .unwrap_or_else(|| format!("{}/{repo}", models::hf::OFFICIAL_ENDPOINT))
    }
}

/// What a failed Hugging Face request means for a column.
pub fn problem_of(e: &HfError) -> CompareProblem {
    match e {
        HfError::Network(_) => CompareProblem::Offline,
        HfError::RateLimited(_) => CompareProblem::RateLimited,
        HfError::NeedsToken | HfError::NeedsTerms => CompareProblem::Gated,
        HfError::NotFound | HfError::FileNotFound | HfError::InvalidName(_) => {
            CompareProblem::NotFound
        }
        _ => CompareProblem::Failed,
    }
}

// ---- Turning what is known into facts ----------------------------------------------

/// The capabilities the catalog may name.
const CAPABILITIES: [&str; 3] = ["tools", "vision", "thinking"];

/// A catalog entry as the comparison shows it; values it cannot show are left out.
pub fn catalog_facts(id: &str, e: &Entry) -> CatalogFacts {
    let text = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let mut capabilities: Vec<String> = Vec::new();
    for c in &e.capabilities {
        if CAPABILITIES.contains(&c.as_str()) && !capabilities.contains(c) {
            capabilities.push(c.clone());
        }
    }
    CatalogFacts {
        id: id.to_string(),
        name: e.name.clone(),
        summary: text(&e.summary),
        source: e.source_url(),
        good_at: e.good_at.clone(),
        publisher: text(&e.publisher),
        parameters: e.parameters.filter(|&p| p > 0),
        active_parameters: e.active_parameters_checked(),
        languages: e
            .languages
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && l.len() <= 20)
            .map(str::to_string)
            .collect(),
        released: e.released_date().map(str::to_string),
        capabilities,
        license: Some(LicenseFacts {
            name: e
                .license
                .name
                .clone()
                .unwrap_or_else(|| e.license.spdx.clone()),
            url: e
                .license
                .url
                .as_deref()
                .filter(|u| url::Url::parse(u).is_ok_and(|u| u.scheme() == "https"))
                .map(str::to_string),
        }),
    }
}

/// The publisher's text of a catalog entry for the uploader's base-model claim: only
/// what the publisher says the model is and is good at. Its capabilities, sizes,
/// languages, dates and licence are not this repository's and are left out.
fn claimed_base_facts(id: &str, e: &Entry) -> CatalogFacts {
    let c = catalog_facts(id, e);
    CatalogFacts {
        capabilities: Vec::new(),
        parameters: None,
        active_parameters: None,
        languages: Vec::new(),
        released: None,
        license: None,
        ..c
    }
}

/// The uploader's base-model claim, when it names a catalog model's publisher card.
fn base_claim<P>(known: &Known<P>, hub: &HubFacts) -> Option<BaseModelFacts> {
    let bases = hub.base_model.as_deref()?;
    bases.split(", ").find_map(|base| {
        let (id, e) = known.entry_with_source(base)?;
        Some(BaseModelFacts {
            base_model: base.to_string(),
            catalog: claimed_base_facts(id, e),
        })
    })
}

/// What the uploader lists, as the comparison carries it.
pub fn hub_facts(d: &RepoDetail, card_url: String) -> HubFacts {
    let i = &d.info;
    HubFacts {
        repo: d.id.clone(),
        author: i.author.clone(),
        pipeline_tag: i.pipeline_tag.clone(),
        base_model: (!i.base_models.is_empty())
            .then(|| uploader_text(&i.base_models.join(", "), UPLOADER_TEXT_CAP)),
        languages: i.languages.clone(),
        license: i.license.clone().map(|name| LicenseFacts {
            name,
            url: i.license_url.clone(),
        }),
        tags: i.tags.clone(),
        downloads: i.downloads,
        likes: i.likes,
        last_modified: i.last_modified.clone(),
        gated: d.gated != Gated::No,
        card_url,
    }
}

/// How long a catalog download waits for the uploader's facts before it starts without
/// them.
const DOWNLOAD_FACTS_WAIT: Duration = Duration::from_secs(15);

/// What the uploader lists for a catalog entry about to be downloaded, to save with the
/// model for comparing it offline (and Hugging Face's parameter count): read earlier in
/// this session, else one listing request at the catalog's pinned commit (the same
/// request a download from Browse makes). `None` when it cannot be read: the download
/// goes ahead without it.
pub async fn catalog_download_facts(hub: &impl Hub, e: &Entry) -> Option<(HubFacts, Option<u64>)> {
    let d = tokio::time::timeout(DOWNLOAD_FACTS_WAIT, hub.listing(&e.repo, Some(&e.revision)))
        .await
        .ok()?
        .ok()?;
    Some((
        hub_facts(&d, hub.page_url(&e.repo)),
        d.gguf.as_ref().and_then(|g| g.parameters),
    ))
}

/// `"7B"` → 7,000,000,000; `"0.6B"`, `"596M"`, `"1.2T"` alike; anything else `None`.
fn label_number(s: &str) -> Option<u64> {
    let s = s.trim();
    let unit = s.chars().last()?;
    let mult: f64 = match unit.to_ascii_uppercase() {
        'K' => 1e3,
        'M' => 1e6,
        'B' => 1e9,
        'T' => 1e12,
        _ => return None,
    };
    let num = &s[..s.len() - unit.len_utf8()];
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let n: f64 = num.parse().ok()?;
    (n > 0.0 && n.is_finite() && n < 1e6).then(|| (n * mult).round() as u64)
}

/// The parameters a GGUF's `general.size_label` names: `"8B"` → (8 billion, none),
/// `"30B-A3B"` → (30 billion, 3 billion at a time). Labels such as `"8x7B"` give nothing.
pub fn size_label_parameters(label: Option<&str>) -> (Option<u64>, Option<u64>) {
    let Some(label) = label.map(str::trim).filter(|l| !l.is_empty()) else {
        return (None, None);
    };
    match label.split_once("-A") {
        Some((total, active)) => match (label_number(total), label_number(active)) {
            (Some(t), Some(a)) if a < t => (Some(t), Some(a)),
            (t, _) => (t, None),
        },
        None => (label_number(label), None),
    }
}

/// An architecture name from a file, shown as text.
fn architecture_text(a: &str) -> Option<String> {
    let a = uploader_text(a, 64);
    (!a.is_empty() && a != "unknown").then_some(a)
}

/// Facts from a header. `parameters`: the exact count when known (Hugging Face counts
/// it), else the file's size label. `quant`: the column's file's own quantisation.
fn file_facts(
    s: &ModelSummary,
    parameters: Option<u64>,
    vision: bool,
    quant: Option<String>,
) -> FileFacts {
    let (label_total, label_active) = size_label_parameters(s.size_label.as_deref());
    let moe = s.is_moe();
    FileFacts {
        architecture: architecture_text(&s.architecture),
        parameters: parameters.or(label_total),
        context_length: s.context_length,
        tools: s.template_tools,
        thinking: s.template_thinking,
        vision,
        moe,
        quantization: quant.or_else(|| s.quantization.clone()),
        expert_count: s.expert_count,
        expert_used_count: s.expert_used_count,
        active_parameters: if moe { label_active } else { None },
    }
}

/// Facts from what was saved when a model was installed, when its file cannot be read.
fn stored_file_facts(st: &StoredSummary, vision: bool) -> Option<FileFacts> {
    st.architecture.as_ref()?;
    let (label_total, label_active) = size_label_parameters(st.size_label.as_deref());
    Some(FileFacts {
        architecture: st.architecture.as_deref().and_then(architecture_text),
        parameters: st.parameters.or(label_total),
        context_length: st.context_length,
        tools: st.template_tools,
        thinking: st.template_thinking,
        vision,
        moe: st.is_moe,
        quantization: st.quantization.clone(),
        expert_count: st.expert_count,
        expert_used_count: st.expert_used_count,
        active_parameters: if st.is_moe { label_active } else { None },
    })
}

/// "Will it fit on this computer?" for an installed file at `ctx` tokens, judged as
/// Installed and Settings for this model judge it (`load_settings::build_info`'s marks):
/// with the model's own settings on this computer (Use the graphics card, Save memory,
/// Fine control), through `placement::judge_in` when the model's sizes are known, else
/// fit.rs's rule on the same memory. The amounts are the settings panel's picture's: what
/// the model needs in the memory that holds it (a graphics card's own memory, else all of
/// it) and what is free there, and on a graphics card the part in the computer's memory
/// and the free memory of its second bar, with whether that part is what makes it too
/// large. The smaller size that leaves room is Automatic's pick, when
/// the same judge marks it green. `None` without the file's header: Settings for this
/// model shows no mark then either.
pub fn installed_fit(plan: &Plan, ctx: u32) -> Option<models::FitResult> {
    let facts = plan.facts.as_ref()?;
    let (r, machine) = (&plan.resolved, &plan.machine);
    let input = facts.fit_input(r.save_memory)?;
    let (gpu, fine) = (r.gpu, &r.fine);
    let kind = machine.memory_kind(gpu);
    let placer = crate::offload::Placer::new(facts, machine, r.save_memory);
    let tokens = u64::from(ctx);
    let mark = |n: u64| match &placer {
        Some(p) => p.judge(n, gpu, fine).0,
        None => models::fit::verdict_at(&input, n, machine.room(gpu)),
    };
    let mut computer_free_bytes = 0;
    let mut computer_short = false;
    let (verdict, partial_offload, needed_bytes, computer_bytes) = match &placer {
        Some(p) => {
            let split = p.work(tokens, gpu, fine).split;
            let (verdict, fits) = p.judge_split(&split, gpu, fine);
            // On a graphics card, the card's share and, as the panel's second bar, the part
            // in the computer's memory (compare-fit-minors: the screen gives both and the
            // whole, never only the card's share).
            let (needed, computer) = match kind {
                MemoryKind::Separate => (split.gpu.total(), split.cpu.total()),
                MemoryKind::Unified | MemoryKind::Processor => {
                    (split.gpu.total().saturating_add(split.cpu.total()), 0)
                }
            };
            if kind == MemoryKind::Separate {
                // The second bar's free figure, and whether the computer's part is what does
                // not fit: `judge_in`'s own check, its free memory less the reserve.
                let room = machine.room(gpu);
                computer_free_bytes = machine.ram_free_bytes;
                computer_short =
                    verdict == Verdict::Red && computer > room.partial.saturating_sub(room.whole);
            }
            let partial = matches!(fits, Fits::Partial | Fits::WontFitGraphics);
            (verdict, partial, needed, computer)
        }
        // The picture without the model's sizes (`offload::not_readable`): everything on the
        // device that holds it; yellow is part on the processor with As much as fits or when
        // it is larger than the free memory, else only just fitting.
        None => {
            let verdict = mark(tokens);
            let needed = input.needed_at(tokens);
            let fit_choice = gpu == runtime::GpuUse::Fit && !fine.custom_placement();
            let partial =
                verdict == Verdict::Yellow && (fit_choice || needed > machine.room(gpu).whole);
            (verdict, partial, needed, 0)
        }
    };
    let suggested_ctx = if verdict == Verdict::Green {
        None
    } else {
        crate::load_settings::automatic(Some(facts), machine, gpu, r.save_memory, fine)
            .map(|a| a.ctx)
            .filter(|&n| n < tokens && mark(n) == Verdict::Green)
    };
    let (device, available_bytes) = match kind {
        MemoryKind::Separate => (FitDevice::GraphicsCard, machine.gpu_free_bytes),
        MemoryKind::Unified => (FitDevice::AppleGraphics, machine.gpu_free_bytes),
        MemoryKind::Processor => (FitDevice::Processor, machine.ram_free_bytes),
    };
    Some(models::FitResult {
        verdict,
        needed_bytes,
        available_bytes,
        suggested_ctx,
        partial_offload,
        device,
        computer_bytes,
        computer_free_bytes,
        computer_short,
        sentence: String::new(),
    })
}

/// One fit verdict at `ctx` for a file that is not on this computer: the same estimate as
/// the repository page.
fn fit_for(
    s: &ModelSummary,
    weights_bytes: u64,
    projector_bytes: u64,
    ctx: u32,
    target: &Target,
) -> models::FitResult {
    // Every field is set today; `..Default::default()` keeps this call unchanged when
    // fields are added (WP1b).
    #[allow(clippy::needless_update)]
    let input = FitInput {
        weights_bytes,
        projector_bytes,
        kv_bytes_per_token: s.kv_bytes_per_token(),
        vocab_size: s.vocab_size.unwrap_or(0),
        ctx: u64::from(ctx),
        max_ctx: s.context_length,
        ..Default::default()
    };
    models::check(&input, target)
}

/// The header of an installed model's file, read from disk once for each size and
/// modification time of the file in a session (reading a large tokenizer takes a moment).
fn local_header(rec: &Installed) -> Option<Arc<ModelSummary>> {
    let meta = std::fs::metadata(&rec.path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let key = format!("{}|{}|{modified}", rec.path, meta.len());
    if let Some(s) = CACHE
        .lock()
        .ok()
        .and_then(|c| c.local.get(&key).map(|(_, s)| s.clone()))
    {
        return Some(s);
    }
    let header = models::read_file(Path::new(&rec.path)).ok()?;
    let summary = Arc::new(ModelSummary::from_header(&header));
    if let Ok(mut c) = CACHE.lock() {
        remember(&mut c.local, key, summary.clone());
    }
    Some(summary)
}

/// The size of the projector beside an installed model; 0 when there is none.
fn installed_projector_bytes(rec: &Installed) -> u64 {
    rec.stored
        .projector
        .as_deref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or(0, |m| m.len())
}

fn empty_facts(item: CompareItem, ctx: u32, title: String) -> CompareFacts {
    CompareFacts {
        item,
        title,
        file: None,
        quant: None,
        size_bytes: None,
        installed_path: None,
        ctx,
        fit: None,
        catalog: None,
        base_catalog: None,
        hub: None,
        file_facts: None,
        results: Vec::new(),
        problem: None,
    }
}

/// Fills what an installed model gives a column: its path, its file's facts and fit,
/// and its saved Hugging Face facts.
async fn fill_from_installed<P: Plans>(
    f: &mut CompareFacts,
    rec: &Installed,
    known: &Known<P>,
    ctx: u32,
) {
    f.installed_path = Some(rec.path.clone());
    let vision = installed_projector_bytes(rec) > 0;
    let quant = rec
        .stored
        .quantization
        .clone()
        .or_else(|| models::quant_label(&rec.file_name));
    f.fit = known
        .plans
        .plan(&rec.path)
        .await
        .and_then(|p| installed_fit(&p, ctx));
    f.file_facts = match local_header(rec) {
        Some(s) => Some(file_facts(&s, rec.stored.parameters, vision, quant)),
        None => stored_file_facts(&rec.stored, vision),
    };
    if f.hub.is_none() {
        f.hub = rec.stored.hub.clone();
    }
}

/// Everything known about one model to compare, at `ctx`. Never fails: what cannot be
/// found is left out and named in `problem`.
pub async fn facts_for<P: Plans>(
    hub: &impl Hub,
    known: &Known<P>,
    item: CompareItem,
    ctx: u32,
) -> CompareFacts {
    match item.clone() {
        CompareItem::Catalog { catalog_id } => {
            catalog_item(hub, known, item, &catalog_id, ctx).await
        }
        CompareItem::Repo { repo, file } => repo_item(hub, known, item, &repo, &file, ctx).await,
        CompareItem::Installed { installed_path } => {
            installed_item(hub, known, item, &installed_path, ctx).await
        }
    }
}

async fn catalog_item<P: Plans>(
    hub: &impl Hub,
    known: &Known<P>,
    item: CompareItem,
    id: &str,
    ctx: u32,
) -> CompareFacts {
    let Some(e) = known.catalog.models.get(id) else {
        let mut f = empty_facts(item, ctx, uploader_text(id, 100));
        f.problem = Some(CompareProblem::NotFound);
        return f;
    };
    let first = &e.files[0];
    let mut f = empty_facts(item, ctx, e.name.clone());
    f.file = Some(first.path.clone());
    f.quant = Some(e.quant.clone());
    f.size_bytes = Some(e.total_bytes());
    f.catalog = Some(catalog_facts(id, e));
    f.results = known.results_for(&first.path, first.size);
    if let Some(rec) = known.installed_for_entry(id, e) {
        if f.results.is_empty() {
            // The same bytes under another name.
            f.results = known.results_for(&rec.file_name, rec.first_file_size());
        }
        fill_from_installed(&mut f, rec, known, ctx).await;
        if f.hub.is_none() {
            // Downloads and dates are extras for an installed model: a failure is not
            // worth a warning on its column.
            if let Ok(d) = hub.listing(&e.repo, Some(&e.revision)).await {
                f.hub = Some(hub_facts(&d, hub.page_url(&e.repo)));
            }
        }
        return f;
    }
    // Not on this computer: the repository's listing and the file's first bytes, at the
    // catalog's pinned commit.
    let listing = hub.listing(&e.repo, Some(&e.revision)).await;
    // When the listing failed (offline, busy, removed) the file is not asked for too;
    // a header read earlier in this session still counts.
    let head = match &listing {
        Ok(_) => hub.head(&e.repo, &e.revision, &first.path).await,
        Err(_) => hub
            .cached_head(&e.repo, &e.revision, &first.path)
            .ok_or(HfError::Network("not asked".into())),
    };
    let mut parameters = None;
    match &listing {
        Ok(d) => {
            f.hub = Some(hub_facts(d, hub.page_url(&e.repo)));
            parameters = d.gguf.as_ref().and_then(|g| g.parameters);
        }
        Err(err) => f.problem = Some(problem_of(err)),
    }
    match head {
        Ok(s) => {
            let projector = e.mmproj.as_ref().map_or(0, |m| m.size);
            f.fit = Some(fit_for(
                &s,
                e.weights_bytes(),
                projector,
                ctx,
                &known.target,
            ));
            f.file_facts = Some(file_facts(
                &s,
                parameters,
                e.mmproj.is_some(),
                Some(e.quant.clone()),
            ));
        }
        Err(err) => {
            f.problem.get_or_insert(problem_of(&err));
        }
    }
    f
}

async fn repo_item<P: Plans>(
    hub: &impl Hub,
    known: &Known<P>,
    item: CompareItem,
    repo: &str,
    file: &str,
    ctx: u32,
) -> CompareFacts {
    let valid = models::hf::validate_repo_id(repo).is_ok()
        && models::hf::validate_repo_path(file).is_ok()
        && file.to_ascii_lowercase().ends_with(".gguf");
    if !valid {
        let mut f = empty_facts(item, ctx, uploader_text(repo, 100));
        f.problem = Some(CompareProblem::NotFound);
        return f;
    }
    let mut f = empty_facts(item, ctx, repo.to_string());
    f.file = Some(file.to_string());
    f.quant = models::quant_label(file);

    let listing = hub.listing(repo, None).await;
    let detail = match &listing {
        Ok(d) => Some(d.clone()),
        Err(err) => {
            f.problem = Some(problem_of(err));
            None
        }
    };
    // The file in the listing: its variant (all shards), their SHA-256 and the projector.
    let mut shas: Vec<Option<String>> = Vec::new();
    let mut variant_size = None;
    let mut projector_bytes = 0;
    let mut smallest: Option<String> = None;
    if let Some(d) = &detail {
        let classified = models::classify(&d.files);
        let variant = classified
            .variants
            .iter()
            .find(|v| v.first_file().path == file)
            .or_else(|| {
                classified
                    .variants
                    .iter()
                    .find(|v| v.files.iter().any(|x| x.path == file))
            });
        match variant {
            Some(v) => {
                shas = v.files.iter().map(|x| x.sha256.clone()).collect();
                variant_size = Some(v.total_size);
                f.file = Some(v.first_file().path.clone());
            }
            None => f.problem = Some(CompareProblem::NotFound),
        }
        projector_bytes = classified.projectors.first().map_or(0, |p| p.size);
        smallest = classified
            .variants
            .iter()
            .min_by_key(|v| v.total_size)
            .map(|v| v.first_file().path.clone());
        f.hub = Some(hub_facts(d, hub.page_url(repo)));
    }
    let file = f.file.clone().unwrap_or_else(|| file.to_string());

    // Exact matches need no network: the catalog and the installed list.
    let shas: Vec<Option<&str>> = shas.iter().map(Option::as_deref).collect();
    let entry = known.entry_for_file(repo, &file, &shas);
    if let Some((id, e)) = entry {
        f.catalog = Some(catalog_facts(id, e));
        f.title = e.name.clone();
    } else if let Some(h) = &f.hub {
        f.base_catalog = base_claim(known, h);
    }
    let first_size = detail
        .as_ref()
        .and_then(|d| d.files.iter().find(|x| x.path == file).map(|x| x.size))
        .or_else(|| entry.map(|(_, e)| e.files[0].size));
    // The repository's file without a projector, as the listing counts it online.
    f.size_bytes = variant_size.or_else(|| entry.map(|(_, e)| e.weights_bytes()));
    if let Some(size) = first_size {
        f.results = known.results_for(&file, size);
    }

    if let Some(rec) = known.installed_for_file(repo, &file, &shas) {
        if f.results.is_empty() {
            f.results = known.results_for(&rec.file_name, rec.first_file_size());
        }
        f.size_bytes.get_or_insert(rec.size);
        fill_from_installed(&mut f, rec, known, ctx).await;
        // Facts from the file on this computer: nothing is missing for this column.
        if f.problem != Some(CompareProblem::NotFound) || detail.is_none() {
            f.problem = None;
        }
        return f;
    }

    let Some(d) = detail else {
        return f;
    };
    // The header of the repository's smallest file (the repository page reads the same
    // one): the model's shape is the same in every file of the repository.
    let Some(head_file) = smallest else {
        return f;
    };
    match hub.head(repo, &d.commit, &head_file).await {
        Ok(s) => {
            let quant = f.quant.clone();
            if let Some(weights) = variant_size {
                f.fit = Some(fit_for(&s, weights, projector_bytes, ctx, &known.target));
            }
            let parameters = d.gguf.as_ref().and_then(|g| g.parameters);
            let mut ff = file_facts(&s, parameters, projector_bytes > 0, quant);
            if head_file != file {
                // Another file's quantisation is not this one's.
                ff.quantization = f.quant.clone();
            }
            f.file_facts = Some(ff);
        }
        Err(err) => {
            f.problem.get_or_insert(problem_of(&err));
            // What the listing says about the files, when the header cannot be read.
            if let Some(g) = &d.gguf {
                f.file_facts = Some(FileFacts {
                    architecture: g.architecture.as_deref().and_then(architecture_text),
                    parameters: g.parameters,
                    context_length: g.context_length,
                    tools: d.template_tools,
                    thinking: d.template_thinking,
                    vision: projector_bytes > 0,
                    quantization: f.quant.clone(),
                    ..FileFacts::default()
                });
            }
        }
    }
    f
}

async fn installed_item<P: Plans>(
    hub: &impl Hub,
    known: &Known<P>,
    item: CompareItem,
    path: &str,
    ctx: u32,
) -> CompareFacts {
    let Some(rec) = known.installed_by_path(path) else {
        let mut f = empty_facts(item, ctx, file_name(path).to_string());
        f.problem = Some(CompareProblem::NotFound);
        return f;
    };
    let mut f = empty_facts(item, ctx, rec.file_name.clone());
    f.file = Some(rec.repo_file().unwrap_or(&rec.file_name).to_string());
    f.quant = rec
        .stored
        .quantization
        .clone()
        .or_else(|| models::quant_label(&rec.file_name));
    f.size_bytes = Some(rec.size);
    f.results = known.results_for(&rec.file_name, rec.first_file_size());
    if let Some((id, e)) = known.entry_for_installed(rec) {
        f.catalog = Some(catalog_facts(id, e));
        f.title = e.name.clone();
    }
    fill_from_installed(&mut f, rec, known, ctx).await;
    // A model downloaded before its facts were saved: its repository's listing, once. An
    // imported file has no repository. A failure leaves the uploader's rows empty
    // without a warning: everything else about an installed model is on this computer.
    if f.hub.is_none()
        && let Some(repo) = rec.repo.as_deref()
        && let Ok(d) = hub.listing(repo, None).await
    {
        f.hub = Some(hub_facts(&d, hub.page_url(repo)));
    }
    if f.catalog.is_none()
        && let Some(h) = &f.hub
    {
        f.base_catalog = base_claim(known, h);
    }
    f
}

/// The catalog for Suggested models: the best for this computer first (the first-run
/// order), each with its fit at `ctx` when the file's header is known without asking
/// the network (installed, or read earlier in this session), its installed path and the
/// user's results. Never fetches anything; a plan is asked for only for the catalog's
/// installed files.
pub async fn catalog_list<P: Plans>(
    hub: &impl Hub,
    known: &Known<P>,
    ctx: u32,
) -> Vec<CatalogListEntry> {
    let mut order: Vec<String> = models::catalog::first_run(&known.catalog, known.usable_bytes)
        .suggestions
        .into_iter()
        .map(|s| s.id)
        .collect();
    for id in known.catalog.models.keys() {
        if !order.contains(id) {
            order.push(id.clone());
        }
    }
    let mut out = Vec::with_capacity(order.len());
    for (id, e) in order
        .iter()
        .filter_map(|id| known.catalog.models.get(id).map(|e| (id, e)))
    {
        let first = &e.files[0];
        let installed = known.installed_for_entry(id, e);
        let projector = e.mmproj.as_ref().map_or(0, |m| m.size);
        let fit = match installed {
            Some(rec) => known
                .plans
                .plan(&rec.path)
                .await
                .and_then(|p| installed_fit(&p, ctx)),
            None => hub
                .cached_head(&e.repo, &e.revision, &first.path)
                .map(|s| fit_for(&s, e.weights_bytes(), projector, ctx, &known.target)),
        };
        out.push(CatalogListEntry {
            catalog: catalog_facts(id, e),
            repo: e.repo.clone(),
            file: first.path.clone(),
            quant: e.quant.clone(),
            size_bytes: e.total_bytes(),
            fit,
            installed_path: installed.map(|r| r.path.clone()),
            results: known.results_for(&first.path, first.size),
        });
    }
    out
}

/// The fit of one file of a repository page, which Compare's Change menu also lists (its
/// files at the comparison size): a file that is installed is judged as its Compare column
/// judges it (the installed model that is exactly this file by Compare's rule, its own plan,
/// [`installed_fit`]), so one column never gives two verdicts for one file; any other file
/// gets `estimate`, the page's estimate from the repository's header. A plan is asked for
/// only for an installed file.
pub async fn variant_fit(
    installed: &[Installed],
    plans: &impl Plans,
    repo: &str,
    v: &models::Variant,
    ctx: u32,
    estimate: Option<models::FitResult>,
) -> Option<models::FitResult> {
    let shas: Vec<Option<&str>> = v.files.iter().map(|f| f.sha256.as_deref()).collect();
    match installed_file(installed, repo, &v.first_file().path, &shas) {
        Some(rec) => plans
            .plan(&rec.path)
            .await
            .and_then(|p| installed_fit(&p, ctx)),
        None => estimate,
    }
}

#[cfg(test)]
mod tests;
