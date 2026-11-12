//! Tests of the comparison's facts (0.0.6 item 4) with a fake Hugging Face, fixture
//! catalog and answers (made-up names), GGUF headers written for the test and an
//! in-memory store. Nothing here touches the network.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::hub::{SavedFacts, register_installed_in};

const CATALOG: &str =
    include_str!("../../../../crates/models/tests/fixtures/catalog/compare-fields.json");
const FULL: &str = include_str!("../../../../crates/models/tests/fixtures/hf/repo-full.json");
const HOSTILE: &str = include_str!("../../../../crates/models/tests/fixtures/hf/repo-hostile.json");

const GB: u64 = 1_000_000_000;
const CTX: u32 = 16_384;

// ---- Fakes --------------------------------------------------------------------------

/// The ways a fake request fails.
#[derive(Debug, Clone, Copy)]
enum Fail {
    Offline,
    Busy,
    Gated,
    Missing,
}

fn error(f: Fail) -> HfError {
    match f {
        Fail::Offline => HfError::Network("connection refused".into()),
        Fail::Busy => HfError::RateLimited(30),
        Fail::Gated => HfError::NeedsToken,
        Fail::Missing => HfError::NotFound,
    }
}

/// Hugging Face as the test says it answers. A repository it was not told about is
/// offline. Counts every request.
#[derive(Default)]
struct FakeHub {
    listings: HashMap<String, Result<RepoDetail, Fail>>,
    heads: HashMap<String, Result<ModelSummary, Fail>>,
    cached_heads: HashMap<String, ModelSummary>,
    listing_calls: AtomicUsize,
    head_calls: AtomicUsize,
}

impl FakeHub {
    fn with_listing(mut self, json: &str) -> Self {
        let d = detail(json);
        self.listings.insert(d.id.clone(), Ok(d));
        self
    }
    fn failing_listing(mut self, repo: &str, f: Fail) -> Self {
        self.listings.insert(repo.into(), Err(f));
        self
    }
    fn with_head(mut self, repo: &str, s: ModelSummary) -> Self {
        self.heads.insert(repo.into(), Ok(s));
        self
    }
    fn failing_head(mut self, repo: &str, f: Fail) -> Self {
        self.heads.insert(repo.into(), Err(f));
        self
    }
    fn requests(&self) -> usize {
        self.listing_calls.load(Ordering::SeqCst) + self.head_calls.load(Ordering::SeqCst)
    }
}

impl Hub for FakeHub {
    async fn listing(
        &self,
        repo: &str,
        _revision: Option<&str>,
    ) -> Result<Arc<RepoDetail>, HfError> {
        self.listing_calls.fetch_add(1, Ordering::SeqCst);
        match self.listings.get(repo) {
            Some(Ok(d)) => Ok(Arc::new(d.clone())),
            Some(Err(f)) => Err(error(*f)),
            None => Err(error(Fail::Offline)),
        }
    }
    async fn head(
        &self,
        repo: &str,
        _commit: &str,
        _file: &str,
    ) -> Result<Arc<ModelSummary>, HfError> {
        self.head_calls.fetch_add(1, Ordering::SeqCst);
        match self.heads.get(repo) {
            Some(Ok(s)) => Ok(Arc::new(s.clone())),
            Some(Err(f)) => Err(error(*f)),
            None => Err(error(Fail::Offline)),
        }
    }
    fn cached_head(&self, repo: &str, _commit: &str, _file: &str) -> Option<Arc<ModelSummary>> {
        self.cached_heads.get(repo).cloned().map(Arc::new)
    }
    fn cached_listing(&self, _repo: &str, _revision: Option<&str>) -> Option<Arc<RepoDetail>> {
        None
    }
    fn page_url(&self, repo: &str) -> String {
        format!("https://huggingface.co/{repo}")
    }
}

fn detail(json: &str) -> RepoDetail {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    let endpoint = url::Url::parse(models::hf::OFFICIAL_ENDPOINT).unwrap();
    models::hf::parse_repo_detail(&id, &v, &endpoint).unwrap()
}

/// GGUF header bytes of a dense 7B-class model (32 layers, 8 KV heads of 128).
fn dense_bytes() -> Vec<u8> {
    models::gguf::testing::Writer::new()
        .str("general.architecture", "sample")
        .str("general.size_label", "7B")
        .u32("general.file_type", 15)
        .u32("sample.context_length", 32_768)
        .u32("sample.block_count", 32)
        .u32("sample.embedding_length", 4096)
        .u32("sample.attention.head_count", 32)
        .u32("sample.attention.head_count_kv", 8)
        .u32("sample.vocab_size", 32_000)
        .str(
            "tokenizer.chat_template",
            "{% if tools %}{% endif %}<think>",
        )
        .build()
}

/// A mixture of experts: 128 experts, 8 used, labelled 30B-A3B.
fn moe_bytes() -> Vec<u8> {
    models::gguf::testing::Writer::new()
        .str("general.architecture", "samplemoe")
        .str("general.size_label", "30B-A3B")
        .u32("general.file_type", 15)
        .u32("samplemoe.context_length", 65_536)
        .u32("samplemoe.block_count", 48)
        .u32("samplemoe.embedding_length", 2048)
        .u32("samplemoe.attention.head_count", 32)
        .u32("samplemoe.attention.head_count_kv", 4)
        .u32("samplemoe.expert_count", 128)
        .u32("samplemoe.expert_used_count", 8)
        .u32("samplemoe.vocab_size", 150_000)
        .build()
}

fn summary(bytes: &[u8]) -> ModelSummary {
    ModelSummary::from_header(&models::gguf::read_prefix(bytes).unwrap())
}

fn target() -> Target {
    Target::Gpu {
        name: "Test graphics card".into(),
        free_bytes: 16 * GB,
        ram_free_bytes: 32 * GB,
    }
}

fn known(installed: Vec<Installed>, results: Vec<LocalResult>) -> Known {
    Known {
        catalog: Catalog::parse(CATALOG).unwrap(),
        installed,
        target: target(),
        usable_bytes: 16 * GB,
        results,
    }
}

/// A folder of its own under the system's temporary folder.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("compare-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A GGUF file on disk with `bytes` as its header.
fn gguf_file(dir: &Path, rel: &str, bytes: &[u8]) -> String {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    p.display().to_string()
}

fn installed(
    path: &str,
    repo: Option<&str>,
    size: u64,
    sha: Option<&str>,
    stored: StoredSummary,
) -> Installed {
    Installed {
        path: path.into(),
        repo: repo.map(str::to_string),
        file_name: file_name(path).to_string(),
        size,
        sha256: sha.map(str::to_string),
        stored,
    }
}

fn catalog_item(id: &str) -> CompareItem {
    CompareItem::Catalog {
        catalog_id: id.into(),
    }
}

fn repo_item(repo: &str, file: &str) -> CompareItem {
    CompareItem::Repo {
        repo: repo.into(),
        file: file.into(),
    }
}

fn result(kind: YourResultKind, id: &str, at: i64, passed: u32, total: u32) -> YourResult {
    YourResult {
        kind,
        id: id.into(),
        at,
        backend: Some("vulkan".into()),
        passed,
        total,
    }
}

const SHA_7B: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// ---- Catalog items ------------------------------------------------------------------

/// A catalog entry not on this computer: the catalog's text, the uploader's facts and
/// one fit at the chosen size from the file's own header, and the user's results.
#[tokio::test]
async fn a_catalog_item_with_the_network() {
    let hub = FakeHub::default()
        .with_listing(FULL)
        .with_head("example-quants/Sample-7B-GGUF", summary(&dense_bytes()));
    let mine = LocalResult {
        file: "Sample-7B-Q4_K_M.gguf".into(),
        size: 4_400_000_000,
        result: result(YourResultKind::ModelCheck, "modelProfile:1", 5, 3, 3),
    };
    let other_size = LocalResult {
        size: 1,
        ..mine.clone()
    };
    let k = known(Vec::new(), vec![mine.clone(), other_size]);
    let f = facts_for(&hub, &k, catalog_item("sample-7b-q4km"), CTX).await;

    assert_eq!(f.item, catalog_item("sample-7b-q4km"));
    assert_eq!(f.title, "Sample 7B");
    assert_eq!(f.file.as_deref(), Some("Sample-7B-Q4_K_M.gguf"));
    assert_eq!(f.quant.as_deref(), Some("Q4_K_M"));
    assert_eq!(f.size_bytes, Some(5_000_000_000), "model and projector");
    assert_eq!(f.ctx, CTX);
    assert_eq!(f.problem, None);
    assert_eq!(f.installed_path, None);
    let c = f.catalog.as_ref().unwrap();
    assert_eq!(c.id, "sample-7b-q4km");
    assert_eq!(c.good_at, ["chat", "coding", "long-documents"]);
    assert_eq!(
        c.source.as_deref(),
        Some("https://huggingface.co/example-labs/Sample-7B")
    );
    assert_eq!(c.capabilities, ["tools", "vision", "thinking"]);
    assert_eq!(c.license.as_ref().unwrap().name, "apache-2.0");
    // The fit: this entry's weights and projector, the header's shape, at CTX.
    let s = summary(&dense_bytes());
    assert_eq!(
        f.fit,
        Some(fit_for(&s, 4_400_000_000, 600_000_000, CTX, &target()))
    );
    let ff = f.file_facts.as_ref().unwrap();
    assert_eq!(
        ff.parameters,
        Some(7_241_732_096),
        "Hugging Face's exact count"
    );
    assert_eq!(ff.context_length, Some(32_768));
    assert!(ff.tools && ff.thinking && ff.vision && !ff.moe);
    let h = f.hub.as_ref().unwrap();
    assert_eq!(h.downloads, Some(123_456));
    assert_eq!(
        h.card_url,
        "https://huggingface.co/example-quants/Sample-7B-GGUF"
    );
    assert_eq!(f.base_catalog, None, "an exact match needs no claim");
    assert_eq!(f.results, vec![mine.result]);
}

/// Offline, a catalog entry keeps its catalog text and says why the rest is missing.
#[tokio::test]
async fn a_catalog_item_offline() {
    let hub = FakeHub::default();
    let k = known(Vec::new(), Vec::new());
    let f = facts_for(&hub, &k, catalog_item("sample-moe-q4km"), CTX).await;
    assert_eq!(f.problem, Some(CompareProblem::Offline));
    let c = f.catalog.unwrap();
    assert_eq!(c.parameters, Some(30 * GB));
    assert_eq!(c.active_parameters, Some(3 * GB));
    assert_eq!(c.languages, ["multilingual"]);
    assert_eq!(
        c.license.unwrap().url.as_deref(),
        Some("https://example.com/model-license")
    );
    assert_eq!(f.size_bytes, Some(18_500_000_000), "both shards");
    assert_eq!(f.fit, None);
    assert_eq!(f.hub, None);
    // The listing failed, so the file was not asked for as well.
    assert_eq!(hub.head_calls.load(Ordering::SeqCst), 0);
    // Rate limited and gated read the same way.
    let busy = FakeHub::default().failing_listing("example-quants/Sample-MoE-30B-GGUF", Fail::Busy);
    let f = facts_for(&busy, &k, catalog_item("sample-moe-q4km"), CTX).await;
    assert_eq!(f.problem, Some(CompareProblem::RateLimited));
    let gated = FakeHub::default()
        .with_listing(FULL)
        .failing_head("example-quants/Sample-7B-GGUF", Fail::Gated);
    let f = facts_for(&gated, &k, catalog_item("sample-7b-q4km"), CTX).await;
    assert_eq!(f.problem, Some(CompareProblem::Gated));
    assert!(f.hub.is_some() && f.fit.is_none());
}

#[tokio::test]
async fn an_unknown_catalog_id_is_not_found() {
    let hub = FakeHub::default();
    let f = facts_for(
        &hub,
        &known(Vec::new(), Vec::new()),
        catalog_item("nope\u{202E}"),
        CTX,
    )
    .await;
    assert_eq!(f.problem, Some(CompareProblem::NotFound));
    assert_eq!(f.title, "nope[U+202E]");
    assert!(f.catalog.is_none() && f.fit.is_none());
    assert_eq!(hub.requests(), 0);
}

/// An installed catalog entry is compared from its file on this computer: the fit and
/// facts need no network, and a failed request for the uploader's extras is no warning.
#[tokio::test]
async fn an_installed_catalog_item_needs_no_network() {
    let dir = scratch("installed-catalog");
    let path = gguf_file(
        &dir,
        "example-quants/Sample-7B-GGUF/Sample-7B-Q4_K_M.gguf",
        &dense_bytes(),
    );
    let stored = StoredSummary {
        catalog_id: Some("sample-7b-q4km".into()),
        ..StoredSummary::default()
    };
    let rec = installed(
        &path,
        Some("example-quants/Sample-7B-GGUF"),
        4_400_000_000,
        Some(SHA_7B),
        stored,
    );
    let hub = FakeHub::default();
    let k = known(vec![rec], Vec::new());
    let f = facts_for(&hub, &k, catalog_item("sample-7b-q4km"), CTX).await;
    assert_eq!(f.installed_path.as_deref(), Some(path.as_str()));
    assert_eq!(f.problem, None);
    let s = summary(&dense_bytes());
    assert_eq!(f.fit, Some(fit_for(&s, 4_400_000_000, 0, CTX, &target())));
    let ff = f.file_facts.unwrap();
    assert_eq!(ff.parameters, Some(7 * GB), "from the size label");
    assert!(!ff.vision, "no projector beside it");
    assert_eq!(hub.head_calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).ok();
}

/// A saved catalog id does not make another file a catalog match.
#[test]
fn a_saved_catalog_id_with_another_sha_is_not_a_match() {
    let k = known(Vec::new(), Vec::new());
    let e = &k.catalog.models["sample-7b-q4km"];
    let stored = StoredSummary {
        catalog_id: Some("sample-7b-q4km".into()),
        ..StoredSummary::default()
    };
    let other = installed("/m/x.gguf", None, 1, Some(&"f".repeat(64)), stored.clone());
    assert!(!installed_is_entry(&other, "sample-7b-q4km", e));
    let unknown_sha = installed("/m/x.gguf", None, 1, None, stored);
    assert!(installed_is_entry(&unknown_sha, "sample-7b-q4km", e));
    // The same bytes under any name or folder are the same file.
    let same = installed(
        "/elsewhere/renamed.gguf",
        None,
        1,
        Some(SHA_7B),
        StoredSummary::default(),
    );
    assert!(installed_is_entry(&same, "sample-7b-q4km", e));
    // An older download without its repository path: where downloads put the file.
    let old = installed(
        "/models/example-quants/Sample-7B-GGUF/Sample-7B-Q4_K_M.gguf",
        Some("example-quants/Sample-7B-GGUF"),
        1,
        None,
        StoredSummary::default(),
    );
    assert!(installed_is_entry(&old, "sample-7b-q4km", e));
    let near = installed(
        "/models/example-quants/Sample-7B-GGUF-v2/Sample-7B-Q4_K_M.gguf",
        Some("example-quants/Sample-7B-GGUF"),
        1,
        None,
        StoredSummary::default(),
    );
    assert!(!installed_is_entry(&near, "sample-7b-q4km", e));
    let imported = installed(
        "/m/Sample-7B-Q4_K_M.gguf",
        None,
        4_400_000_000,
        None,
        StoredSummary::default(),
    );
    assert!(
        !installed_is_entry(&imported, "sample-7b-q4km", e),
        "a name alone is not a match"
    );
}

// ---- Repository items -----------------------------------------------------------------

/// A repository file that is a catalog entry's file (same repository and path) gets the
/// catalog's text; the fit is the file's own.
#[tokio::test]
async fn a_repo_item_that_is_a_catalog_file() {
    let hub = FakeHub::default()
        .with_listing(FULL)
        .with_head("example-quants/Sample-7B-GGUF", summary(&dense_bytes()));
    let k = known(Vec::new(), Vec::new());
    let item = repo_item("example-quants/Sample-7B-GGUF", "Sample-7B-Q4_K_M.gguf");
    let f = facts_for(&hub, &k, item.clone(), CTX).await;
    assert_eq!(f.item, item);
    assert_eq!(
        f.catalog.as_ref().map(|c| c.id.as_str()),
        Some("sample-7b-q4km")
    );
    assert_eq!(f.title, "Sample 7B");
    assert_eq!(f.size_bytes, Some(4_400_000_000));
    let s = summary(&dense_bytes());
    assert_eq!(
        f.fit,
        Some(fit_for(&s, 4_400_000_000, 600_000_000, CTX, &target()))
    );
    assert_eq!(f.problem, None);
    // Another file of the same repository is not the catalog's file: only the uploader's
    // claim, labelled as such.
    let q8 = repo_item("example-quants/Sample-7B-GGUF", "Sample-7B-Q8_0.gguf");
    let f = facts_for(&hub, &k, q8, CTX).await;
    assert_eq!(f.catalog, None);
    assert_eq!(f.title, "example-quants/Sample-7B-GGUF");
    assert_eq!(f.quant.as_deref(), Some("Q8_0"));
    assert_eq!(
        f.file_facts.as_ref().unwrap().quantization.as_deref(),
        Some("Q8_0")
    );
    let b = f.base_catalog.unwrap();
    assert_eq!(b.base_model, "example-labs/Sample-7B");
    assert_eq!(b.catalog.id, "sample-7b-q4km");
}

/// The spoofed base model: a stranger's repository that names a catalog model as its base
/// gets the publisher's text only as the uploader's claim, without the catalog's
/// capabilities, sizes, licence or fit. The same bytes as the catalog's file (equal
/// SHA-256) are an exact match wherever they were uploaded.
#[tokio::test]
async fn the_spoofed_base_model() {
    let repo = "evil-uploads/Totally-Official-GGUF";
    let hub = FakeHub::default()
        .with_listing(HOSTILE)
        .with_head(repo, summary(&moe_bytes()));
    let k = known(Vec::new(), Vec::new());
    let f = facts_for(
        &hub,
        &k,
        repo_item(repo, "Totally-Official-Q5_K_M.gguf"),
        CTX,
    )
    .await;
    assert_eq!(f.catalog, None, "never a plain catalog match");
    assert_eq!(f.title, repo);
    let b = f.base_catalog.clone().unwrap();
    assert_eq!(b.base_model, "example-labs/Sample-7B");
    assert_eq!(
        b.catalog.summary.as_deref(),
        Some("Sample 7B is a small model made for testing. It answers questions and writes code.")
    );
    assert!(b.catalog.capabilities.is_empty());
    assert_eq!(b.catalog.parameters, None);
    assert_eq!(b.catalog.license, None);
    assert!(b.catalog.languages.is_empty() && b.catalog.released.is_none());
    // The fit and the file's facts are this repository's own file's.
    let s = summary(&moe_bytes());
    assert_eq!(f.fit, Some(fit_for(&s, 5_100_000_000, 0, CTX, &target())));
    let ff = f.file_facts.as_ref().unwrap();
    assert!(ff.moe && !ff.vision);
    assert_eq!(ff.expert_count, Some(128));
    assert_eq!(ff.expert_used_count, Some(8));
    assert_eq!(ff.parameters, Some(30 * GB));
    assert_eq!(ff.active_parameters, Some(3 * GB));
    // The uploader's text is made safe.
    let h = f.hub.as_ref().unwrap();
    assert_eq!(h.author.as_deref(), Some("evil[U+202E]uploads"));
    assert!(h.gated);
    assert_eq!(h.tags, ["uncensored"]);
    let json = serde_json::to_string(&f).unwrap();
    assert!(!json.contains('\u{202E}') && !json.contains('\u{200B}'));

    // The repository's other file has the catalog file's SHA-256: the same bytes.
    let f = facts_for(
        &hub,
        &k,
        repo_item(repo, "Totally-Official-Q4_K_M.gguf"),
        CTX,
    )
    .await;
    assert_eq!(f.catalog.map(|c| c.id), Some("sample-7b-q4km".to_string()));
    assert_eq!(f.base_catalog, None);
}

/// Offline, a repository file keeps what needs no network: an exact catalog match by
/// repository and path. Rate limits, gated repositories and missing files are named.
#[tokio::test]
async fn repo_item_problems() {
    let k = known(Vec::new(), Vec::new());
    let item = repo_item("example-quants/Sample-7B-GGUF", "Sample-7B-Q4_K_M.gguf");
    let f = facts_for(&FakeHub::default(), &k, item.clone(), CTX).await;
    assert_eq!(f.problem, Some(CompareProblem::Offline));
    assert_eq!(f.catalog.map(|c| c.id), Some("sample-7b-q4km".to_string()));
    assert_eq!(f.size_bytes, Some(5_000_000_000));
    assert!(f.hub.is_none() && f.fit.is_none());

    let busy = FakeHub::default().failing_listing("example-quants/Sample-7B-GGUF", Fail::Busy);
    let f = facts_for(&busy, &k, item.clone(), CTX).await;
    assert_eq!(f.problem, Some(CompareProblem::RateLimited));
    assert_eq!(busy.head_calls.load(Ordering::SeqCst), 0);

    // Gated: the listing is public, the file needs a token. What the listing says about
    // the files still shows.
    let gated = FakeHub::default()
        .with_listing(HOSTILE)
        .failing_head("evil-uploads/Totally-Official-GGUF", Fail::Gated);
    let f = facts_for(
        &gated,
        &k,
        repo_item(
            "evil-uploads/Totally-Official-GGUF",
            "Totally-Official-Q5_K_M.gguf",
        ),
        CTX,
    )
    .await;
    assert_eq!(f.problem, Some(CompareProblem::Gated));
    assert!(f.hub.as_ref().unwrap().gated);
    assert_eq!(f.fit, None);

    let full = FakeHub::default()
        .with_listing(FULL)
        .with_head("example-quants/Sample-7B-GGUF", summary(&dense_bytes()))
        .failing_listing("example-quants/Removed-GGUF", Fail::Missing);
    let f = facts_for(
        &full,
        &k,
        repo_item("example-quants/Removed-GGUF", "x.gguf"),
        CTX,
    )
    .await;
    assert_eq!(f.problem, Some(CompareProblem::NotFound));
    let f = facts_for(
        &full,
        &k,
        repo_item("example-quants/Sample-7B-GGUF", "Not-There-Q4_K_M.gguf"),
        CTX,
    )
    .await;
    assert_eq!(f.problem, Some(CompareProblem::NotFound));
    assert!(f.hub.is_some(), "the repository's facts are still shown");
}

/// Names that are not a repository and a GGUF file in it are refused before any request.
#[tokio::test]
async fn bad_repo_names_ask_nothing() {
    let hub = FakeHub::default().with_listing(FULL);
    let k = known(Vec::new(), Vec::new());
    for (repo, file) in [
        ("../etc", "passwd.gguf"),
        ("a/b/c", "x.gguf"),
        ("example-quants/Sample-7B-GGUF", "../x.gguf"),
        ("example-quants/Sample-7B-GGUF", "README.md"),
        ("example-quants/Sample-7B-GGUF", "C:\\x.gguf"),
    ] {
        let f = facts_for(&hub, &k, repo_item(repo, file), CTX).await;
        assert_eq!(f.problem, Some(CompareProblem::NotFound), "{repo} {file}");
    }
    assert_eq!(hub.requests(), 0);
}

/// A repository file that is installed is compared from the file on this computer, with
/// no warning when the network is down.
#[tokio::test]
async fn an_installed_repo_item_offline() {
    let dir = scratch("installed-repo");
    let path = gguf_file(
        &dir,
        "example-quants/Sample-7B-GGUF/Sample-7B-Q8_0.gguf",
        &moe_bytes(),
    );
    let stored = StoredSummary {
        repo_file: Some("Sample-7B-Q8_0.gguf".into()),
        ..StoredSummary::default()
    };
    let rec = installed(
        &path,
        Some("example-quants/Sample-7B-GGUF"),
        7_700_000_000,
        None,
        stored,
    );
    let k = known(vec![rec], Vec::new());
    let f = facts_for(
        &FakeHub::default(),
        &k,
        repo_item("example-quants/Sample-7B-GGUF", "Sample-7B-Q8_0.gguf"),
        CTX,
    )
    .await;
    assert_eq!(f.installed_path.as_deref(), Some(path.as_str()));
    assert_eq!(f.problem, None);
    assert!(f.fit.is_some() && f.file_facts.is_some());
    assert_eq!(f.size_bytes, Some(7_700_000_000));
    std::fs::remove_dir_all(dir).ok();
}

// ---- Installed items ------------------------------------------------------------------

/// A path that is not an installed model is "not found", and nothing is read from it.
#[tokio::test]
async fn an_unknown_path_is_not_read() {
    let dir = scratch("not-installed");
    let path = gguf_file(&dir, "secret.gguf", &dense_bytes());
    let hub = FakeHub::default();
    let f = facts_for(
        &hub,
        &known(Vec::new(), Vec::new()),
        CompareItem::Installed {
            installed_path: path.clone(),
        },
        CTX,
    )
    .await;
    assert_eq!(f.problem, Some(CompareProblem::NotFound));
    assert_eq!(f.title, "secret.gguf");
    assert!(f.file_facts.is_none() && f.fit.is_none() && f.installed_path.is_none());
    assert_eq!(hub.requests(), 0);
    std::fs::remove_dir_all(dir).ok();
}

/// An installed model with facts saved at download is compared offline: the saved
/// uploader's facts, the file's header, the base-model claim, and the user's results
/// matched by file name and size (the first shard's size, as model checks remember it).
#[tokio::test]
async fn an_installed_item_with_saved_facts() {
    let dir = scratch("installed-saved");
    let bytes = dense_bytes();
    let path = gguf_file(&dir, "Sample-7B-Q8_0.gguf", &bytes);
    let saved_hub = hub_facts(
        &detail(FULL),
        "https://huggingface.co/example-quants/Sample-7B-GGUF".into(),
    );
    let stored = StoredSummary {
        quantization: Some("Q8_0".into()),
        repo_file: Some("Sample-7B-Q8_0.gguf".into()),
        hub: Some(saved_hub.clone()),
        parameters: Some(7_241_732_096),
        ..StoredSummary::default()
    };
    let rec = installed(
        &path,
        Some("example-quants/Sample-7B-GGUF"),
        7_700_000_000,
        None,
        stored,
    );
    let first_size = bytes.len() as u64;
    let check = LocalResult {
        file: "Sample-7B-Q8_0.gguf".into(),
        size: first_size,
        result: result(YourResultKind::ModelCheck, "modelProfile:a", 10, 2, 3),
    };
    let bench = LocalResult {
        file: "Sample-7B-Q8_0.gguf".into(),
        size: first_size,
        result: result(YourResultKind::Benchmark, "20261101-101010-abcd", 20, 4, 5),
    };
    let elsewhere = LocalResult {
        file: "Other.gguf".into(),
        ..check.clone()
    };
    let k = known(vec![rec], vec![check.clone(), bench.clone(), elsewhere]);
    let hub = FakeHub::default();
    let item = CompareItem::Installed {
        installed_path: path.clone(),
    };
    let f = facts_for(&hub, &k, item.clone(), CTX).await;
    assert_eq!(f.item, item);
    assert_eq!(f.title, "Sample-7B-Q8_0.gguf");
    assert_eq!(f.installed_path.as_deref(), Some(path.as_str()));
    assert_eq!(f.quant.as_deref(), Some("Q8_0"));
    assert_eq!(f.size_bytes, Some(7_700_000_000));
    assert_eq!(f.hub, Some(saved_hub));
    assert_eq!(f.catalog, None);
    assert_eq!(f.base_catalog.unwrap().catalog.id, "sample-7b-q4km");
    assert_eq!(f.file_facts.unwrap().parameters, Some(7_241_732_096));
    assert_eq!(f.results, vec![bench.result, check.result], "newest first");
    assert_eq!(f.problem, None);
    assert_eq!(hub.requests(), 0, "everything was saved");
    std::fs::remove_dir_all(dir).ok();
}

/// An installed model whose file is gone still shows what was saved for it.
#[tokio::test]
async fn an_installed_item_whose_file_is_gone() {
    let stored = StoredSummary {
        architecture: Some("sample".into()),
        size_label: Some("30B-A3B".into()),
        is_moe: true,
        expert_count: Some(128),
        expert_used_count: Some(8),
        context_length: Some(65_536),
        ..StoredSummary::default()
    };
    let rec = installed("/nowhere/Gone-Q4_K_M.gguf", None, 9, None, stored);
    let k = known(vec![rec], Vec::new());
    let f = facts_for(
        &FakeHub::default(),
        &k,
        CompareItem::Installed {
            installed_path: "/nowhere/Gone-Q4_K_M.gguf".into(),
        },
        CTX,
    )
    .await;
    let ff = f.file_facts.unwrap();
    assert_eq!(ff.parameters, Some(30 * GB));
    assert_eq!(ff.active_parameters, Some(3 * GB));
    assert_eq!(ff.expert_count, Some(128));
    assert_eq!(f.quant.as_deref(), Some("Q4_K_M"), "from the file name");
    assert_eq!(f.fit, None);
}

// ---- Suggested models -----------------------------------------------------------------

/// The catalog list asks nothing of the network: a fit where the header is known
/// (installed, or read earlier this session), none otherwise.
#[test]
fn catalog_list_fetches_nothing() {
    let dir = scratch("catalog-list");
    let path = gguf_file(&dir, "Sample-Plain-Q8_0.gguf", &dense_bytes());
    let rec = installed(
        &path,
        None,
        1_200_000_000,
        Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
        StoredSummary::default(),
    );
    let mine = LocalResult {
        file: "Sample-Plain-Q8_0.gguf".into(),
        size: 1_200_000_000,
        result: result(YourResultKind::Benchmark, "20261101-101010-abcd", 1, 1, 2),
    };
    let k = known(vec![rec], vec![mine.clone()]);
    let mut hub = FakeHub::default();
    hub.cached_heads.insert(
        "example-quants/Sample-MoE-30B-GGUF".into(),
        summary(&moe_bytes()),
    );
    let list = catalog_list(&hub, &k, CTX);
    assert_eq!(hub.requests(), 0);
    let ids: Vec<&str> = list.iter().map(|e| e.catalog.id.as_str()).collect();
    // The entries that fit 16 GB first (by the catalog's own estimate), then the rest.
    assert_eq!(
        ids,
        ["sample-7b-q4km", "sample-plain-q8", "sample-moe-q4km"]
    );
    let by_id = |id: &str| list.iter().find(|e| e.catalog.id == id).unwrap();
    let plain = by_id("sample-plain-q8");
    assert_eq!(plain.installed_path.as_deref(), Some(path.as_str()));
    assert!(plain.fit.is_some());
    assert_eq!(plain.results, vec![mine.result]);
    assert_eq!(plain.quant, "Q8_0");
    assert_eq!(plain.catalog.source, None, "an http link is not shown");
    let moe = by_id("sample-moe-q4km");
    assert_eq!(
        moe.fit,
        Some(fit_for(
            &summary(&moe_bytes()),
            18_500_000_000,
            0,
            CTX,
            &target()
        ))
    );
    assert_eq!(moe.file, "Q4_K_M/Sample-MoE-30B-Q4_K_M-00001-of-00002.gguf");
    let small = by_id("sample-7b-q4km");
    assert_eq!(small.fit, None, "not known without the network");
    assert_eq!(small.size_bytes, 5_000_000_000);
    assert_eq!(small.repo, "example-quants/Sample-7B-GGUF");
    assert_eq!(small.installed_path, None);
    std::fs::remove_dir_all(dir).ok();
}

// ---- Your results ---------------------------------------------------------------------

fn profile_json(
    file: &str,
    size: u64,
    backend: &str,
    chosen: Option<&str>,
    passed: &[(&str, &[bool])],
) -> Profile {
    let probe = |ok: &bool| {
        serde_json::json!({
            "probe": "toolCalling", "passed": ok, "arrival": "native", "toolFailures": 0,
            "failedEdits": 0, "blocked": 0, "steps": 1, "tokens": 1, "timeMs": 1,
            "status": "completed", "note": ""
        })
    };
    let results: Vec<serde_json::Value> = passed
        .iter()
        .map(|(label, oks)| {
            serde_json::json!({
                "label": label, "settings": {}, "toolFailures": 0,
                "passed": oks.iter().filter(|o| **o).count(),
                "probes": oks.iter().map(probe).collect::<Vec<_>>()
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "version": 1,
        "identity": {"kind": "local", "file": file, "size": size, "backend": backend},
        "label": format!("{file} ({backend})"),
        "checkedAt": 1000,
        "chosen": chosen,
        "defaults": null,
        "results": results
    }))
    .unwrap()
}

fn bench_json(
    id: &str,
    model: &str,
    model_ref: &str,
    cancelled: bool,
    ran: u32,
    completed: u32,
) -> BenchRun {
    serde_json::from_value(serde_json::json!({
        "id": id, "startedAt": 2000, "finishedAt": 3000, "model": model, "modelRef": model_ref,
        "settings": {}, "toolchains": [], "cancelled": cancelled, "tasks": [],
        "totals": {"tasks": 5, "ran": ran, "completed": completed, "skipped": 5 - ran,
                   "brokenEdits": 0, "toolFailures": 0, "blocked": 0, "steps": 0,
                   "tokens": 0, "timeMs": 0}
    }))
    .unwrap()
}

#[test]
fn your_results_from_checks_and_benchmarks() {
    let chosen = profile_json(
        "m.gguf",
        42,
        "Vulkan",
        Some("Thinking off"),
        &[
            ("Default", &[true, false, false]),
            ("Thinking off", &[true, true, false]),
        ],
    );
    let none_chosen = profile_json(
        "n.gguf",
        7,
        "cpu",
        None,
        &[("Default", &[false, false, false])],
    );
    let empty = profile_json("e.gguf", 1, "cpu", None, &[]);
    let remote: Profile = serde_json::from_value(serde_json::json!({
        "version": 1, "identity": {"kind": "remote", "url": "http://h/v1", "model": "m.gguf"},
        "label": "m", "checkedAt": 1, "chosen": null, "defaults": null, "results": []
    }))
    .unwrap();
    let profiles = vec![
        ("modelProfile:1".to_string(), chosen),
        ("modelProfile:2".to_string(), none_chosen),
        ("modelProfile:3".to_string(), empty),
        ("modelProfile:4".to_string(), remote),
    ];
    let bench = vec![
        (
            bench_json(
                "20261101-101010-aaaa",
                "m.gguf (rocm)",
                "local:/models/m.gguf",
                false,
                4,
                3,
            ),
            Some(42),
        ),
        (
            bench_json(
                "20261101-101010-bbbb",
                "m.gguf (vulkan)",
                "local:/models/m.gguf",
                true,
                4,
                3,
            ),
            Some(42),
        ),
        (
            bench_json(
                "20261101-101010-cccc",
                "m.gguf (cpu)",
                "local:/gone/m.gguf",
                false,
                4,
                3,
            ),
            None,
        ),
        (
            bench_json(
                "20261101-101010-dddd",
                "m.gguf (cpu)",
                "local:/models/m.gguf",
                false,
                0,
                0,
            ),
            Some(42),
        ),
        (
            bench_json("20261101-101010-eeee", "remote", "remote:1:m", false, 4, 4),
            Some(42),
        ),
        (
            bench_json(
                "20261101-101010-ffff",
                "m.gguf (weird)",
                "local:C:\\models\\m.gguf",
                false,
                2,
                2,
            ),
            Some(42),
        ),
    ];
    let got = local_results(&profiles, &bench);
    // (file, size, id, backend, passed, total)
    type Row<'a> = (&'a str, u64, &'a str, Option<&'a str>, u32, u32);
    let summary: Vec<Row> = got
        .iter()
        .map(|r| {
            (
                r.file.as_str(),
                r.size,
                r.result.id.as_str(),
                r.result.backend.as_deref(),
                r.result.passed,
                r.result.total,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("m.gguf", 42, "modelProfile:1", Some("vulkan"), 2, 3),
            ("n.gguf", 7, "modelProfile:2", Some("cpu"), 0, 3),
            ("m.gguf", 42, "20261101-101010-aaaa", Some("rocm"), 3, 4),
            ("m.gguf", 42, "20261101-101010-ffff", None, 2, 2),
        ]
    );
    assert_eq!(got[0].result.kind, YourResultKind::ModelCheck);
    assert_eq!(got[0].result.at, 1000);
    assert_eq!(got[2].result.kind, YourResultKind::Benchmark);
    assert_eq!(got[2].result.at, 2000);
}

// ---- Small rules ----------------------------------------------------------------------

#[test]
fn size_labels() {
    assert_eq!(size_label_parameters(Some("7B")), (Some(7 * GB), None));
    assert_eq!(
        size_label_parameters(Some("0.6B")),
        (Some(600_000_000), None)
    );
    assert_eq!(
        size_label_parameters(Some("596M")),
        (Some(596_000_000), None)
    );
    assert_eq!(
        size_label_parameters(Some("1.2T")),
        (Some(1_200 * GB), None)
    );
    assert_eq!(
        size_label_parameters(Some("30B-A3B")),
        (Some(30 * GB), Some(3 * GB))
    );
    assert_eq!(size_label_parameters(Some("3B-A30B")), (Some(3 * GB), None));
    for odd in ["8x7B", "E4B", "B", "7", "-7B", "1e3B", "7 B", "", "７B"] {
        assert_eq!(size_label_parameters(Some(odd)), (None, None), "{odd:?}");
    }
    assert_eq!(size_label_parameters(None), (None, None));
}

#[test]
fn problems_from_errors() {
    assert_eq!(
        problem_of(&HfError::Network("x".into())),
        CompareProblem::Offline
    );
    assert_eq!(
        problem_of(&HfError::RateLimited(5)),
        CompareProblem::RateLimited
    );
    assert_eq!(problem_of(&HfError::NeedsToken), CompareProblem::Gated);
    assert_eq!(problem_of(&HfError::NeedsTerms), CompareProblem::Gated);
    assert_eq!(problem_of(&HfError::NotFound), CompareProblem::NotFound);
    assert_eq!(problem_of(&HfError::FileNotFound), CompareProblem::NotFound);
    assert_eq!(
        problem_of(&HfError::Http {
            status: 500,
            message: String::new()
        }),
        CompareProblem::Failed
    );
    assert_eq!(
        problem_of(&HfError::BlockedHost("x".into())),
        CompareProblem::Failed
    );
}

#[test]
fn sizes_are_clamped() {
    assert_eq!(clamp_ctx(0), 512);
    assert_eq!(clamp_ctx(8192), 8192);
    assert_eq!(clamp_ctx(u32::MAX), 1_048_576);
}

/// The JSON the comparison screen reads: every field of the contract, camelCase.
#[tokio::test]
async fn the_json_has_every_contract_field() {
    let hub = FakeHub::default()
        .with_listing(FULL)
        .with_head("example-quants/Sample-7B-GGUF", summary(&dense_bytes()));
    let f = facts_for(
        &hub,
        &known(Vec::new(), Vec::new()),
        catalog_item("sample-7b-q4km"),
        CTX,
    )
    .await;
    let v = serde_json::to_value(&f).unwrap();
    for key in [
        "item",
        "title",
        "file",
        "quant",
        "sizeBytes",
        "installedPath",
        "ctx",
        "fit",
        "catalog",
        "baseCatalog",
        "hub",
        "fileFacts",
        "results",
        "problem",
    ] {
        assert!(v.get(key).is_some(), "{key} missing: {v}");
    }
    assert_eq!(
        v["item"],
        serde_json::json!({"catalogId": "sample-7b-q4km"})
    );
    assert_eq!(
        v["catalog"]["goodAt"],
        serde_json::json!(["chat", "coding", "long-documents"])
    );
    assert_eq!(v["fileFacts"]["expertCount"], serde_json::Value::Null);
    assert_eq!(v["hub"]["pipelineTag"], "text-generation");
}

// ---- Saved at download ----------------------------------------------------------------

/// A download saves its catalog id and the uploader's facts in `gguf_summary`; an import
/// of the same file later keeps them; a different file at that path does not.
#[tokio::test]
async fn facts_saved_at_download() {
    let dir = scratch("saved");
    let path = gguf_file(&dir, "Sample-7B-Q4_K_M.gguf", &moe_bytes());
    let store = store::Store::open_in_memory().unwrap();
    let hub = hub_facts(
        &detail(FULL),
        "https://huggingface.co/example-quants/Sample-7B-GGUF".into(),
    );
    let saved = SavedFacts {
        catalog_id: Some("sample-7b-q4km".into()),
        repo_file: Some("Sample-7B-Q4_K_M.gguf".into()),
        hub: Some(hub.clone()),
        parameters: Some(7_241_732_096),
    };
    let size = std::fs::metadata(&path).unwrap().len();
    register_installed_in(
        &store,
        Path::new(&path),
        Some("example-quants/Sample-7B-GGUF".into()),
        size,
        Some(SHA_7B.into()),
        None,
        store::ModelSource::Download,
        saved,
    )
    .await
    .unwrap();
    let rec = Installed::from_record(&store.model_by_path(&path).await.unwrap().unwrap());
    assert_eq!(rec.stored.catalog_id.as_deref(), Some("sample-7b-q4km"));
    assert_eq!(
        rec.stored.repo_file.as_deref(),
        Some("Sample-7B-Q4_K_M.gguf")
    );
    assert_eq!(rec.stored.hub.as_ref(), Some(&hub));
    assert_eq!(rec.stored.parameters, Some(7_241_732_096));
    assert_eq!(rec.stored.expert_count, Some(128));
    assert_eq!(rec.stored.expert_used_count, Some(8));
    assert!(rec.stored.is_moe);

    // Offline comparison of the installed model: everything from what was saved.
    let k = known(vec![rec], Vec::new());
    let f = facts_for(
        &FakeHub::default(),
        &k,
        CompareItem::Installed {
            installed_path: path.clone(),
        },
        CTX,
    )
    .await;
    assert_eq!(f.hub, Some(hub.clone()));
    assert_eq!(f.catalog.map(|c| c.id), Some("sample-7b-q4km".to_string()));
    assert_eq!(f.title, "Sample 7B");

    // Imported again (same size): the saved facts stay.
    register_installed_in(
        &store,
        Path::new(&path),
        None,
        size,
        None,
        None,
        store::ModelSource::Import,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    let rec = Installed::from_record(&store.model_by_path(&path).await.unwrap().unwrap());
    assert_eq!(rec.stored.catalog_id.as_deref(), Some("sample-7b-q4km"));
    assert_eq!(rec.stored.hub.as_ref(), Some(&hub));

    // Another file at the same path (another size): nothing carried over.
    std::fs::write(&path, dense_bytes()).unwrap();
    let new_size = std::fs::metadata(&path).unwrap().len();
    assert_ne!(new_size, size);
    register_installed_in(
        &store,
        Path::new(&path),
        None,
        new_size,
        None,
        None,
        store::ModelSource::Import,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    let rec = Installed::from_record(&store.model_by_path(&path).await.unwrap().unwrap());
    assert!(rec.stored.catalog_id.is_none() && rec.stored.hub.is_none());
    assert!(!rec.stored.is_moe);
    std::fs::remove_dir_all(dir).ok();
}

/// The session cache: a header at a commit is kept, the oldest goes first when full.
#[test]
fn the_session_cache_keeps_at_most_its_limit() {
    let mut map: HashMap<String, (Instant, u32)> = HashMap::new();
    for n in 0..(CACHE_MAX as u32 + 5) {
        remember(&mut map, format!("k{n}"), n);
    }
    assert_eq!(map.len(), CACHE_MAX);
    assert!(map.contains_key(&format!("k{}", CACHE_MAX + 4)));
    assert!(is_commit(&"a".repeat(40)) && !is_commit("main"));
    // Without a client nothing is cached or fetched, and the card link is still built.
    let live = LiveHub::new(None);
    assert!(live.cached_head("a/b", &"a".repeat(40), "f.gguf").is_none());
    assert_eq!(live.page_url("a/b"), "https://huggingface.co/a/b");
}
