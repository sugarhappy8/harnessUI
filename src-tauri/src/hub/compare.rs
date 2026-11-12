//! Facts for Models → Compare (0.0.6 item 4, WP4a): `catalog_list` and `compare_facts`.
//!
//! Where each fact comes from (D6), and the rules that keep them honest:
//! - **The signed catalog** (`From the publisher's model card`): attached to a column only
//!   on an *exact* match: the same SHA-256, the same repository and file path, or the
//!   catalog id (an id saved at download counts, unless the file's SHA-256 says it is
//!   another file). A Hugging Face repository whose uploader names a catalog model's
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
//! - **The fit** (one verdict per column at the chosen size): the same estimate as the
//!   repository page, from that column's own file only.
//! - **Your results**: saved model checks and benchmark runs with the same file name and
//!   size as the column's file.
//!
//! Installed files are read only when they are in the installed list: a path the webview
//! sends that is not an installed model is "not found", and nothing is read from it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use models::catalog::{Catalog, Entry};
use models::fit::{FitInput, Target};
use models::hf::{Gated, HfClient, HfError, RepoDetail, UPLOADER_TEXT_CAP, uploader_text};
use models::summary::ModelSummary;

use super::StoredSummary;
use crate::dto::{
    BaseModelFacts, CatalogFacts, CatalogListEntry, CompareFacts, CompareItem, CompareProblem,
    FileFacts, HubFacts, LicenseFacts, YourResult, YourResultKind,
};
use crate::evals::bench::BenchRun;
use crate::evals::profile::{Identity, Profile};

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
pub struct Known {
    pub catalog: Catalog,
    pub installed: Vec<Installed>,
    pub target: Target,
    /// Memory the model may use, for the order of Suggested models.
    pub usable_bytes: u64,
    pub results: Vec<LocalResult>,
}

/// Two SHA-256 values that are both known and differ: certainly not the same file.
fn sha_differs(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if !a.eq_ignore_ascii_case(b))
}

/// The same file of the same repository: equal SHA-256 when both are known (the same
/// bytes, wherever they were uploaded), else the same repository and path.
fn same_remote_file(
    (repo_a, file_a, sha_a): (&str, &str, Option<&str>),
    (repo_b, file_b, sha_b): (&str, &str, Option<&str>),
) -> bool {
    if let (Some(a), Some(b)) = (sha_a, sha_b) {
        return a.eq_ignore_ascii_case(b);
    }
    repo_a.eq_ignore_ascii_case(repo_b) && file_a == file_b
}

/// `path` ends with `/<owner>/<name>/<file>`, the place downloads put a repository's file.
fn path_is_download_of(path: &str, repo: &str, file: &str) -> bool {
    let path = path.replace('\\', "/");
    let tail = format!("/{repo}/{file}");
    path.len() >= tail.len()
        && path.is_char_boundary(path.len() - tail.len())
        && path[path.len() - tail.len()..].eq_ignore_ascii_case(&tail)
}

/// Whether an installed model is exactly this repository file.
fn installed_is(rec: &Installed, repo: &str, file: &str, sha: Option<&str>) -> bool {
    if let (Some(a), Some(b)) = (rec.sha256.as_deref(), sha) {
        return a.eq_ignore_ascii_case(b);
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
    installed_is(rec, &e.repo, &first.path, first.sha256.as_deref())
        || (rec.stored.catalog_id.as_deref() == Some(id)
            && !sha_differs(rec.sha256.as_deref(), first.sha256.as_deref()))
}

impl Known {
    fn installed_by_path(&self, path: &str) -> Option<&Installed> {
        self.installed.iter().find(|r| r.path == path)
    }

    fn installed_for_entry(&self, id: &str, e: &Entry) -> Option<&Installed> {
        self.installed.iter().find(|r| installed_is_entry(r, id, e))
    }

    fn installed_for_file(&self, repo: &str, file: &str, sha: Option<&str>) -> Option<&Installed> {
        self.installed
            .iter()
            .find(|r| installed_is(r, repo, file, sha))
    }

    /// The catalog entry that is exactly this repository file.
    fn entry_for_file(&self, repo: &str, file: &str, sha: Option<&str>) -> Option<(&str, &Entry)> {
        self.catalog
            .models
            .iter()
            .find(|(_, e)| {
                let first = &e.files[0];
                same_remote_file(
                    (&e.repo, &first.path, first.sha256.as_deref()),
                    (repo, file, sha),
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
fn base_claim(known: &Known, hub: &HubFacts) -> Option<BaseModelFacts> {
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

/// One fit verdict at `ctx`: the same estimate as the repository page.
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

/// The projector beside an installed model, when its file is there: (size, it exists).
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
fn fill_from_installed(f: &mut CompareFacts, rec: &Installed, known: &Known, ctx: u32) {
    f.installed_path = Some(rec.path.clone());
    let vision = installed_projector_bytes(rec) > 0;
    let quant = rec
        .stored
        .quantization
        .clone()
        .or_else(|| models::quant_label(&rec.file_name));
    match local_header(rec) {
        Some(s) => {
            f.fit = Some(fit_for(
                &s,
                rec.size,
                installed_projector_bytes(rec),
                ctx,
                &known.target,
            ));
            f.file_facts = Some(file_facts(&s, rec.stored.parameters, vision, quant));
        }
        None => f.file_facts = stored_file_facts(&rec.stored, vision),
    }
    if f.hub.is_none() {
        f.hub = rec.stored.hub.clone();
    }
}

/// Everything known about one model to compare, at `ctx`. Never fails: what cannot be
/// found is left out and named in `problem`.
pub async fn facts_for(hub: &impl Hub, known: &Known, item: CompareItem, ctx: u32) -> CompareFacts {
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

async fn catalog_item(
    hub: &impl Hub,
    known: &Known,
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
        fill_from_installed(&mut f, rec, known, ctx);
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

async fn repo_item(
    hub: &impl Hub,
    known: &Known,
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
    // The file in the listing: its variant (all shards), SHA-256 and projector.
    let mut sha: Option<String> = None;
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
                sha = v.first_file().sha256.clone();
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
    let entry = known.entry_for_file(repo, &file, sha.as_deref());
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
    f.size_bytes = variant_size.or_else(|| entry.map(|(_, e)| e.total_bytes()));
    if let Some(size) = first_size {
        f.results = known.results_for(&file, size);
    }

    if let Some(rec) = known.installed_for_file(repo, &file, sha.as_deref()) {
        if f.results.is_empty() {
            f.results = known.results_for(&rec.file_name, rec.first_file_size());
        }
        f.size_bytes.get_or_insert(rec.size);
        fill_from_installed(&mut f, rec, known, ctx);
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

async fn installed_item(
    hub: &impl Hub,
    known: &Known,
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
    fill_from_installed(&mut f, rec, known, ctx);
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
/// user's results. Never fetches anything.
pub fn catalog_list(hub: &impl Hub, known: &Known, ctx: u32) -> Vec<CatalogListEntry> {
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
    order
        .iter()
        .filter_map(|id| known.catalog.models.get(id).map(|e| (id, e)))
        .map(|(id, e)| {
            let first = &e.files[0];
            let installed = known.installed_for_entry(id, e);
            let projector = e.mmproj.as_ref().map_or(0, |m| m.size);
            let fit = match installed {
                Some(rec) => local_header(rec).map(|s| {
                    fit_for(
                        &s,
                        rec.size,
                        installed_projector_bytes(rec),
                        ctx,
                        &known.target,
                    )
                }),
                None => hub
                    .cached_head(&e.repo, &e.revision, &first.path)
                    .map(|s| fit_for(&s, e.weights_bytes(), projector, ctx, &known.target)),
            };
            CatalogListEntry {
                catalog: catalog_facts(id, e),
                repo: e.repo.clone(),
                file: first.path.clone(),
                quant: e.quant.clone(),
                size_bytes: e.total_bytes(),
                fit,
                installed_path: installed.map(|r| r.path.clone()),
                results: known.results_for(&first.path, first.size),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
