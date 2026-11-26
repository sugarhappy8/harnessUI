//! Reading a Hugging Face repo's files when it opens (design section 7.3): one header read
//! per group of quantisations ([`models::sd::candidates`]), at most
//! [`MAX_FILES_PER_REPO`] a repo, [`PARALLEL_READS`] at a time, each file read once while it
//! stays in a small cache. Every listed file gets a label, and the repo a verdict the search
//! shows in place of its name's hint.
//!
//! The app keeps one [`DetectService`] in its state (an `Arc`), beside the `StudioManager`
//! (`lib.rs`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;

use futures_util::StreamExt;
use models::files::RepoFile;
use models::hf::{Gated, HfClient, HfError, RepoDetail};
use models::sd::candidates::{QuantGroup, candidates};
use models::sd::detect::FolderPart;
use models::sd::inspect::{inspect, inspect_folder};
use models::sd::names::DiffusersFolder;
use models::sd::remote::{MAX_FILES_PER_REPO, RemoteError, read_remote};
use models::sd::{CannotRunReason, FileFacts, FileLabel, NotRead, Verdict, label_for, verdict_of};

/// The most files whose facts are kept.
pub const CACHE_ENTRIES: usize = 512;
/// Header reads at once.
pub const PARALLEL_READS: usize = 4;

/// A file at a commit, by its content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub repo: String,
    pub revision: String,
    pub path: String,
    pub sha256: String,
}

/// What reading a repo found.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoReading {
    /// Every listed file Studio shows, by path; files it does not list are absent.
    pub labels: BTreeMap<String, FileLabel>,
    /// The facts of every file labelled from a read, by path: each member of a group has
    /// its read file's; a diffusers folder's UNet has the folder's.
    pub facts: BTreeMap<String, FileFacts>,
    /// Files whose header was read, now or before (from the cache).
    pub read: u32,
    /// Files labelled `NotRead`.
    pub unread: u32,
}

/// The facts of the files read lately, the one used longest ago forgotten first.
#[derive(Default)]
struct Cache {
    map: HashMap<CacheKey, FileFacts>,
    /// Oldest use first.
    order: VecDeque<CacheKey>,
}

impl Cache {
    fn get(&mut self, key: &CacheKey) -> Option<FileFacts> {
        let facts = self.map.get(key)?.clone();
        self.touch(key);
        Some(facts)
    }

    fn put(&mut self, key: CacheKey, facts: FileFacts) {
        if self.map.insert(key.clone(), facts).is_some() {
            self.touch(&key);
            return;
        }
        self.order.push_back(key);
        while self.map.len() > CACHE_ENTRIES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.map.remove(&oldest);
        }
    }

    /// Makes `key` the latest used.
    fn touch(&mut self, key: &CacheKey) {
        if let Some(at) = self.order.iter().position(|k| k == key)
            && let Some(k) = self.order.remove(at)
        {
            self.order.push_back(k);
        }
    }
}

/// Reads repos' files when they open, and remembers what it read.
#[derive(Default)]
pub struct DetectService {
    cache: Mutex<Cache>,
    /// The last reading's verdict by repo id, for the search.
    verdicts: Mutex<HashMap<String, Verdict>>,
}

/// Why a read gave no facts.
fn not_read(e: &RemoteError) -> NotRead {
    match e {
        RemoteError::Hf(HfError::NeedsToken) => NotRead::NeedsToken,
        RemoteError::Hf(HfError::NeedsTerms) => NotRead::NeedsTerms,
        _ => NotRead::Failed,
    }
}

/// The parts of `folder` read to tell its version: the UNet and the text encoders.
fn folder_reads(folder: &DiffusersFolder) -> Vec<(FolderPart, &RepoFile)> {
    [
        (FolderPart::Unet, Some(&folder.unet)),
        (FolderPart::TextEncoder, folder.text_encoder.as_ref()),
        (FolderPart::TextEncoder2, folder.text_encoder_2.as_ref()),
    ]
    .into_iter()
    .filter_map(|(part, file)| file.map(|f| (part, &f.file)))
    .collect()
}

impl DetectService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads `detail`'s repo: its diffusers folder, if any, then the first groups of
    /// quantisations, in the order the candidates give (models, then LoRAs, then names a read
    /// cannot make runnable), up to [`MAX_FILES_PER_REPO`] files in all, [`PARALLEL_READS`]
    /// at a time, each through the cache first. Each member of a group gets the label of the
    /// file read, with its own name's part. A gated repo and no token reads nothing: every
    /// file that would be read is `NotRead { NeedsToken }`.
    pub async fn read_repo(&self, hf: &HfClient, detail: &RepoDetail) -> RepoReading {
        let c = candidates(&detail.files);
        let name = |path: &str| format!("{}/{}", detail.id, path);
        let no_token = detail.gated != Gated::No && !hf.has_token();
        let mut labels = c.by_name.clone();
        let mut facts = BTreeMap::new();
        let mut read = 0;
        let mut budget = MAX_FILES_PER_REPO;

        if let Some(folder) = &c.folder {
            let parts = folder_reads(folder);
            budget = budget.saturating_sub(parts.len());
            let unet = &folder.unet.file.path;
            let found = if no_token {
                Err(NotRead::NeedsToken)
            } else {
                self.read_folder(hf, detail, &parts).await
            };
            match found {
                Ok(found) => {
                    read += parts.len();
                    labels.insert(unet.clone(), label_for(&found, &name(unet)));
                    facts.insert(unet.clone(), found);
                }
                Err(reason) => {
                    labels.insert(unet.clone(), FileLabel::NotRead { reason });
                }
            }
        }

        let files: HashMap<&str, &RepoFile> =
            detail.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let (now, later) = c.groups.split_at(budget.min(c.groups.len()));
        let found: Vec<Result<FileFacts, NotRead>> = if no_token {
            vec![Err(NotRead::NeedsToken); now.len()]
        } else {
            // The futures are made before the stream is polled: a stream holding a closure
            // that borrows each group would make the reading's future not `Send`.
            let reads: Vec<_> = now
                .iter()
                .map(|group| self.read_listed(hf, detail, files.get(group.read.as_str()).copied()))
                .collect();
            futures_util::stream::iter(reads)
                .buffered(PARALLEL_READS)
                .collect()
                .await
        };
        let past_budget = if no_token {
            NotRead::NeedsToken
        } else {
            NotRead::Budget
        };
        let unread_groups = later.iter().map(|g| (g, Err(past_budget)));
        for (group, found) in now.iter().zip(found).chain(unread_groups) {
            read += usize::from(found.is_ok());
            label_group(group, found, &name, &c.by_name, &mut labels, &mut facts);
        }

        let unread = labels
            .values()
            .filter(|l| matches!(l, FileLabel::NotRead { .. }))
            .count();
        let all: Vec<FileLabel> = labels.values().cloned().collect();
        let verdict = verdict_of(&all);
        // A verdict is kept only when no unread file could change it: a file that works or a
        // LoRA, or every file labelled. Otherwise names alone (a gated FLUX.1-dev's folder
        // model files) would stand in for the model nobody could read.
        if matches!(verdict, Verdict::Works { .. } | Verdict::Lora { .. }) || unread == 0 {
            self.verdicts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(detail.id.clone(), verdict);
        }
        RepoReading {
            labels,
            facts,
            read: u32::try_from(read).unwrap_or(u32::MAX),
            unread: u32::try_from(unread).unwrap_or(u32::MAX),
        }
    }

    /// The last [`DetectService::read_repo`]'s verdict for `repo`, when that reading found a
    /// file that works or a LoRA, or left no file `NotRead`.
    pub fn verdict(&self, repo: &str) -> Option<Verdict> {
        self.verdicts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(repo)
            .cloned()
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(detail: &RepoDetail, file: &RepoFile) -> Option<CacheKey> {
        Some(CacheKey {
            repo: detail.id.clone(),
            revision: detail.commit.clone(),
            path: file.path.clone(),
            sha256: file.sha256.clone()?,
        })
    }

    /// [`DetectService::read_file`] for a group's file, which the listing has.
    async fn read_listed(
        &self,
        hf: &HfClient,
        detail: &RepoDetail,
        file: Option<&RepoFile>,
    ) -> Result<FileFacts, NotRead> {
        match file {
            Some(file) => self.read_file(hf, detail, file).await,
            None => Err(NotRead::Failed),
        }
    }

    /// The facts of `file`, from the cache or its header.
    async fn read_file(
        &self,
        hf: &HfClient,
        detail: &RepoDetail,
        file: &RepoFile,
    ) -> Result<FileFacts, NotRead> {
        self.read_one(hf, detail, file)
            .await
            .map_err(|e| not_read(&e))
    }

    /// The facts of `detail`'s listed `file`, from the cache or its header (one bounded read,
    /// `read_remote`), with the read's own error: Task S10 reads a LoRA's header before its
    /// download starts. Kept in the cache like a repo's reading.
    pub(crate) async fn read_one(
        &self,
        hf: &HfClient,
        detail: &RepoDetail,
        file: &RepoFile,
    ) -> Result<FileFacts, RemoteError> {
        let key = Self::key(detail, file);
        if let Some(found) = key.as_ref().and_then(|k| self.cache().get(k)) {
            return Ok(found);
        }
        let index = read_remote(hf, &detail.id, &detail.commit, &file.path, file.size).await?;
        let found = inspect(&index);
        if let Some(key) = key {
            self.cache().put(key, found.clone());
        }
        Ok(found)
    }

    /// The facts of a diffusers folder from its `parts`' headers, kept under its UNet's key.
    async fn read_folder(
        &self,
        hf: &HfClient,
        detail: &RepoDetail,
        parts: &[(FolderPart, &RepoFile)],
    ) -> Result<FileFacts, NotRead> {
        let key = parts.first().and_then(|(_, unet)| Self::key(detail, unet));
        if let Some(found) = key.as_ref().and_then(|k| self.cache().get(k)) {
            return Ok(found);
        }
        let reads = parts.iter().map(|(part, file)| async move {
            read_remote(hf, &detail.id, &detail.commit, &file.path, file.size)
                .await
                .map(|index| (*part, index))
        });
        let indexes = futures_util::future::join_all(reads)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| not_read(&e))?;
        let parts: Vec<_> = indexes.iter().map(|(part, index)| (*part, index)).collect();
        let found = inspect_folder(&parts);
        if let Some(key) = key {
            self.cache().put(key, found.clone());
        }
        Ok(found)
    }
}

/// Labels `group`'s members from what reading it `found`. A member whose name already
/// decided its label (a split file's shards and index) keeps it, with the version read.
fn label_group(
    group: &QuantGroup,
    found: Result<FileFacts, NotRead>,
    name: &impl Fn(&str) -> String,
    by_name: &BTreeMap<String, FileLabel>,
    labels: &mut BTreeMap<String, FileLabel>,
    facts: &mut BTreeMap<String, FileFacts>,
) {
    for member in &group.members {
        if let Some(FileLabel::CannotRun {
            reason: CannotRunReason::Sharded,
            ..
        }) = by_name.get(member)
        {
            if let Ok(found) = &found {
                let version = match label_for(found, &name(&group.read)) {
                    FileLabel::Works { version, .. } => Some(version),
                    FileLabel::CannotRun { version, .. } => version,
                    _ => None,
                };
                labels.insert(
                    member.clone(),
                    FileLabel::CannotRun {
                        reason: CannotRunReason::Sharded,
                        version,
                    },
                );
            }
            continue;
        }
        match &found {
            Ok(found) => {
                labels.insert(member.clone(), label_for(found, &name(member)));
                facts.insert(member.clone(), found.clone());
            }
            Err(reason) => {
                labels.insert(member.clone(), FileLabel::NotRead { reason: *reason });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::hash::{DefaultHasher, Hash, Hasher};

    use models::files::RepoFile;
    use models::gen_catalog::GenRole;
    use models::hf::{Gated, HfClient, HubInfo, RepoDetail};
    use models::sd::detect::SdVersion;
    use models::sd::matching::{NoisePart, RecipeKind};
    use models::sd::testing::{HubServer, Served, file_of, fixture, hub_server};
    use models::sd::{CannotRunReason, FileLabel, NotRead, Verdict};

    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A repo's listing, and what the Hub serves for its files.
    struct TestRepo {
        detail: RepoDetail,
        served: BTreeMap<String, Served>,
    }

    impl TestRepo {
        fn new(id: &str) -> Self {
            TestRepo {
                detail: RepoDetail {
                    id: id.into(),
                    commit: COMMIT.into(),
                    quantized_from: None,
                    gated: Gated::No,
                    license: None,
                    files: Vec::new(),
                    gguf: None,
                    template_tools: false,
                    template_thinking: false,
                    info: HubInfo::default(),
                    not_for_all_audiences: false,
                },
                served: BTreeMap::new(),
            }
        }

        /// Lists `path` (`size` bytes, with a made-up SHA-256) and serves the header of the
        /// detection fixture `header`, if any.
        fn file(mut self, path: &str, size: f64, header: Option<&str>) -> Self {
            let mut h = DefaultHasher::new();
            path.hash(&mut h);
            self.detail.files.push(RepoFile {
                path: path.into(),
                size: size as u64,
                sha256: Some(format!("{:016x}", h.finish()).repeat(4)),
            });
            if let Some(name) = header {
                self.served
                    .insert(path.into(), Served::Bytes(file_of(&fixture(name))));
            }
            self
        }

        /// Lists a file that is not stored with LFS (no SHA-256), and serves nothing for it.
        fn plain(mut self, path: &str) -> Self {
            self.detail.files.push(RepoFile {
                path: path.into(),
                size: 1_000,
                sha256: None,
            });
            self
        }

        fn served(mut self, path: &str, size: f64, served: Served) -> Self {
            self = self.file(path, size, None);
            self.served.insert(path.into(), served);
            self
        }
    }

    fn gated(mut repo: TestRepo) -> TestRepo {
        repo.detail.gated = Gated::Auto;
        repo
    }

    struct S {
        hub: HubServer,
        detail: RepoDetail,
    }

    impl S {
        fn client(&self) -> HfClient {
            self.hub.client()
        }
        fn detail(&self) -> RepoDetail {
            self.detail.clone()
        }
        fn header_requests(&self) -> usize {
            self.hub.header_requests()
        }
        fn requests(&self) -> usize {
            self.hub.requests()
        }
    }

    async fn hub_server_with(repo: TestRepo) -> S {
        S {
            hub: hub_server(repo.served).await,
            detail: repo.detail,
        }
    }

    /// QuantStack's Wan 2.2 T2V A14B: each quantisation of the high-noise and the low-noise
    /// part, each serving its part's header.
    fn wan_a14b_repo(quants: &[&str]) -> TestRepo {
        let mut repo = TestRepo::new("QuantStack/Wan2.2-T2V-A14B-GGUF")
            .plain("README.md")
            .plain(".gitattributes");
        for (i, q) in quants.iter().enumerate() {
            let size = 5e9 + 1e9 * i as f64;
            repo = repo
                .file(
                    &format!("HighNoise/Wan2.2-T2V-A14B-HighNoise-{q}.gguf"),
                    size,
                    Some("wan22-t2v-a14b-high"),
                )
                .file(
                    &format!("LowNoise/Wan2.2-T2V-A14B-LowNoise-{q}.gguf"),
                    size,
                    Some("wan22-t2v-a14b-low"),
                );
        }
        repo
    }

    fn flux_dev_repo() -> TestRepo {
        TestRepo::new("black-forest-labs/FLUX.1-dev")
            .file("flux1-dev.safetensors", 23.8e9, Some("flux1-dev"))
            .file("ae.safetensors", 335e6, Some("ae-flux1"))
            .plain("README.md")
    }

    /// black-forest-labs/FLUX.1-dev as Hugging Face lists it (gated): the checkpoint and its
    /// VAE at the top, beside a diffusers folder model with no UNet.
    fn real_flux_dev_listing() -> TestRepo {
        TestRepo::new("black-forest-labs/FLUX.1-dev")
            .plain(".gitattributes")
            .plain("LICENSE.md")
            .plain("README.md")
            .file("ae.safetensors", 335_304_388.0, Some("ae-flux1"))
            .file("dev_grid.jpg", 1_334_007.0, None)
            .file("flux1-dev.safetensors", 23_802_932_552.0, Some("flux1-dev"))
            .plain("model_index.json")
            .plain("scheduler/scheduler_config.json")
            .plain("text_encoder/config.json")
            .file("text_encoder/model.safetensors", 246_144_352.0, None)
            .plain("text_encoder_2/config.json")
            .file(
                "text_encoder_2/model-00001-of-00002.safetensors",
                4_994_582_224.0,
                None,
            )
            .file(
                "text_encoder_2/model-00002-of-00002.safetensors",
                4_530_066_360.0,
                None,
            )
            .plain("text_encoder_2/model.safetensors.index.json")
            .plain("tokenizer/merges.txt")
            .plain("tokenizer/vocab.json")
            .file("tokenizer_2/spiece.model", 791_656.0, None)
            .plain("tokenizer_2/tokenizer.json")
            .plain("transformer/config.json")
            .file(
                "transformer/diffusion_pytorch_model-00001-of-00003.safetensors",
                9_983_040_304.0,
                None,
            )
            .file(
                "transformer/diffusion_pytorch_model-00002-of-00003.safetensors",
                9_949_328_904.0,
                None,
            )
            .file(
                "transformer/diffusion_pytorch_model-00003-of-00003.safetensors",
                3_870_584_832.0,
                None,
            )
            .plain("transformer/diffusion_pytorch_model.safetensors.index.json")
            .plain("vae/config.json")
            .file(
                "vae/diffusion_pytorch_model.safetensors",
                167_666_902.0,
                None,
            )
    }

    fn sdxl_repo() -> TestRepo {
        TestRepo::new("stabilityai/stable-diffusion-xl-base-1.0")
            .file("sd_xl_base_1.0.safetensors", 6.94e9, Some("sdxl"))
            .plain("README.md")
    }

    /// `n` files of different models (none a quantisation of another), each 6.9 GB.
    fn repo_of_distinct_models(n: usize) -> TestRepo {
        (0..n).fold(TestRepo::new("someone/merges"), |repo, i| {
            repo.file(&format!("merge-{i:02}.safetensors"), 6.9e9, Some("sdxl"))
        })
    }

    /// CompVis/stable-diffusion-v1-4 as a diffusers folder: plain and fp16 files, the UNet
    /// and text encoder serving the sd14 fixtures' headers, and its other folders.
    fn sd14_diffusers_repo() -> TestRepo {
        let unet = Some("sd14-diffusers-unet");
        let text_encoder = Some("sd14-diffusers-text-encoder");
        TestRepo::new("CompVis/stable-diffusion-v1-4")
            .plain("model_index.json")
            .file(
                "unet/diffusion_pytorch_model.safetensors",
                3_438_167_534.0,
                unet,
            )
            .file(
                "unet/diffusion_pytorch_model.fp16.safetensors",
                1_719_125_304.0,
                unet,
            )
            .file(
                "text_encoder/model.safetensors",
                492_265_879.0,
                text_encoder,
            )
            .file(
                "text_encoder/model.fp16.safetensors",
                246_144_352.0,
                text_encoder,
            )
            .file(
                "vae/diffusion_pytorch_model.safetensors",
                334_643_268.0,
                None,
            )
            .file(
                "vae/diffusion_pytorch_model.fp16.safetensors",
                167_335_342.0,
                None,
            )
            .file("safety_checker/model.safetensors", 1_215_981_830.0, None)
            .file("safety_checker/model.fp16.safetensors", 608_018_440.0, None)
            .plain("feature_extractor/preprocessor_config.json")
            .plain("scheduler/scheduler_config.json")
            .plain("tokenizer/vocab.json")
            .file("sd-v1-4.ckpt", 4_265_380_512.0, None)
    }

    #[tokio::test]
    async fn one_header_read_labels_every_quantisation_of_a_model() {
        let quants = [
            "Q2_K", "Q3_K_S", "Q3_K_M", "Q4_0", "Q4_1", "Q4_K_S", "Q4_K_M", "Q5_0", "Q5_1",
            "Q5_K_S", "Q5_K_M", "Q6_K", "Q8_0", "F16", "BF16",
        ];
        let s = hub_server_with(wan_a14b_repo(&quants)).await; // serves both fixtures' headers for every name
        let reading = DetectService::new()
            .read_repo(&s.client(), &s.detail())
            .await;
        assert_eq!(s.header_requests(), 2);
        assert_eq!(reading.labels.len(), 30);
        assert!(reading.labels.iter().all(
            |(path, l)| matches!(l, FileLabel::Works { recipe, part: Some(p), .. }
            if recipe == "wan22-t2v" && (*p == NoisePart::High) == path.starts_with("HighNoise/"))
        ));
        assert_eq!(reading.unread, 0);
        assert_eq!(reading.read, 2);
        // Every quantisation has its part's facts, for the set-up planner.
        assert_eq!(reading.facts.len(), 30);
    }

    #[tokio::test]
    async fn a_gated_repo_without_a_token_labels_files_not_read() {
        let s = hub_server_with(gated(flux_dev_repo())).await; // client without a token
        let d = DetectService::new();
        let reading = d.read_repo(&s.client(), &s.detail()).await;
        assert_eq!(s.requests(), 0);
        assert!(reading.labels.values().all(|l| *l
            == FileLabel::NotRead {
                reason: NotRead::NeedsToken
            }
            || matches!(l, FileLabel::Extra { .. })));
        assert_eq!(
            reading.labels["flux1-dev.safetensors"],
            FileLabel::NotRead {
                reason: NotRead::NeedsToken
            }
        );
        assert_eq!((reading.read, reading.unread), (0, 1));
        // Nothing was learnt, so no verdict is remembered: the search keeps its name hint.
        assert_eq!(d.verdict("black-forest-labs/FLUX.1-dev"), None);
    }

    /// The folder model's files say "can't run" by name, but the checkpoint Studio runs was
    /// not read: no verdict is kept, so the search does not call FLUX.1-dev unrunnable.
    #[tokio::test]
    async fn the_real_flux_dev_listing_gated_without_a_token_keeps_no_verdict() {
        let s = hub_server_with(gated(real_flux_dev_listing())).await;
        let d = DetectService::new();
        let r = d.read_repo(&s.client(), &s.detail()).await;
        assert_eq!(s.requests(), 0);
        assert_eq!(
            r.labels["flux1-dev.safetensors"],
            FileLabel::NotRead {
                reason: NotRead::NeedsToken
            }
        );
        assert_eq!(
            r.labels["ae.safetensors"],
            FileLabel::Extra { role: GenRole::Vae }
        );
        assert_eq!(
            r.labels["transformer/diffusion_pytorch_model-00001-of-00003.safetensors"],
            FileLabel::CannotRun {
                reason: CannotRunReason::FolderModel,
                version: None
            }
        );
        assert_eq!(d.verdict("black-forest-labs/FLUX.1-dev"), None);
        // With every file read the same listing keeps its verdict: the checkpoint works.
        let s = hub_server_with(real_flux_dev_listing()).await;
        d.read_repo(&s.client(), &s.detail()).await;
        assert_eq!(
            d.verdict("black-forest-labs/FLUX.1-dev"),
            Some(Verdict::Works {
                version: SdVersion::Flux,
                kind: RecipeKind::Picture
            })
        );
    }

    #[tokio::test]
    async fn a_file_read_once_is_not_read_again() {
        let s = hub_server_with(sdxl_repo()).await;
        let d = DetectService::new();
        d.read_repo(&s.client(), &s.detail()).await;
        let again = d.read_repo(&s.client(), &s.detail()).await;
        assert_eq!(s.header_requests(), 1);
        assert!(matches!(
            &again.labels["sd_xl_base_1.0.safetensors"],
            FileLabel::Works { version: SdVersion::Sdxl, recipe, .. } if recipe == "sdxl-checkpoint"
        ));
        assert_eq!(again.read, 1);
        assert_eq!(
            d.verdict("stabilityai/stable-diffusion-xl-base-1.0"),
            Some(Verdict::Works {
                version: SdVersion::Sdxl,
                kind: RecipeKind::Picture
            })
        );
    }

    #[tokio::test]
    async fn past_24_files_the_rest_are_labelled_not_read() {
        let s = hub_server_with(repo_of_distinct_models(30)).await;
        let d = DetectService::new();
        let r = d.read_repo(&s.client(), &s.detail()).await;
        assert_eq!((s.header_requests(), r.read, r.unread), (24, 24, 6));
        // Files not read cannot take away a model that works.
        assert!(matches!(
            d.verdict("someone/merges"),
            Some(Verdict::Works { .. })
        ));
        assert_eq!(
            r.labels
                .values()
                .filter(|l| **l
                    == FileLabel::NotRead {
                        reason: NotRead::Budget
                    })
                .count(),
            6
        );
    }

    /// Comfy-Org/Wan-2.2 as recorded (final review, area 1, I-2): its T2V A14B pair, its
    /// TI2V-5B and its LoRAs are read within the 24-file budget, each fp8 file labelled by
    /// its fp16 file's group, and nothing is left "not read" for the budget.
    #[tokio::test]
    async fn comfys_wan_2_2_repo_reads_its_t2v_ti2v_and_loras_within_the_budget() {
        let mut repo = TestRepo::new("Comfy-Org/Wan-2.2");
        repo.detail.files = models::sd::testing::comfy_wan22_listing();
        let header = |path: &str| {
            if path.contains("/loras/") {
                Some("flux-lora-realism")
            } else if path.contains("wan2.2_t2v_high_noise") {
                Some("wan22-t2v-a14b-high")
            } else if path.contains("wan2.2_t2v_low_noise") {
                Some("wan22-t2v-a14b-low")
            } else if path.contains("wan2.2_ti2v_5B") {
                Some("wan22-ti2v-5b")
            } else {
                None
            }
        };
        for f in &repo.detail.files {
            if let Some(name) = header(&f.path) {
                repo.served
                    .insert(f.path.clone(), Served::Bytes(file_of(&fixture(name))));
            }
        }
        let s = hub_server_with(repo).await;
        let r = DetectService::new()
            .read_repo(&s.client(), &s.detail())
            .await;
        let d = "split_files/diffusion_models/";
        for (file, part) in [
            ("wan2.2_t2v_high_noise_14B_fp16", NoisePart::High),
            ("wan2.2_t2v_high_noise_14B_fp8_scaled", NoisePart::High),
            ("wan2.2_t2v_low_noise_14B_fp16", NoisePart::Low),
            ("wan2.2_t2v_low_noise_14B_fp8_scaled", NoisePart::Low),
        ] {
            assert!(
                matches!(&r.labels[&format!("{d}{file}.safetensors")],
                FileLabel::Works { recipe, part: Some(p), .. } if recipe == "wan22-t2v" && *p == part),
                "{file}: {:?}",
                r.labels[&format!("{d}{file}.safetensors")]
            );
        }
        assert!(matches!(
            &r.labels[&format!("{d}wan2.2_ti2v_5B_fp16.safetensors")],
            FileLabel::Works { recipe, .. } if recipe == "wan22-ti2v-5b"
        ));
        let loras: Vec<_> = r
            .labels
            .iter()
            .filter(|(path, _)| path.contains("/loras/"))
            .collect();
        assert_eq!(loras.len(), 6);
        assert!(
            loras
                .iter()
                .all(|(_, l)| matches!(l, FileLabel::Lora { .. })),
            "{loras:?}"
        );
        assert!(
            !r.labels.values().any(|l| *l
                == FileLabel::NotRead {
                    reason: NotRead::Budget
                }),
            "{:?}",
            r.labels
        );
        assert!(s.header_requests() <= MAX_FILES_PER_REPO);
    }

    #[tokio::test]
    async fn a_diffusers_repo_reads_its_folder_and_labels_the_unet() {
        let s = hub_server_with(sd14_diffusers_repo()).await; // unet and text_encoder headers from the sd14 fixtures, plain and fp16 files
        let r = DetectService::new()
            .read_repo(&s.client(), &s.detail())
            .await;
        assert_eq!(s.header_requests(), 2);
        assert!(
            matches!(&r.labels["unet/diffusion_pytorch_model.fp16.safetensors"],
            FileLabel::Works { version: SdVersion::Sd1, recipe, folder: true, .. } if recipe == "sd1-checkpoint")
        );
        assert_eq!(
            r.labels["text_encoder/model.fp16.safetensors"],
            FileLabel::Extra {
                role: GenRole::ClipL
            }
        );
        assert!(!r.labels.contains_key("safety_checker/model.safetensors"));
        assert_eq!(
            r.labels["sd-v1-4.ckpt"],
            FileLabel::CannotRun {
                reason: CannotRunReason::Pickle,
                version: None
            }
        );
        assert_eq!((r.read, r.unread), (2, 0));
        assert!(
            r.facts
                .contains_key("unet/diffusion_pytorch_model.fp16.safetensors")
        );
    }

    #[tokio::test]
    async fn a_read_the_hub_refuses_labels_its_group_not_read() {
        let repo = TestRepo::new("someone/models")
            .file("model-a.safetensors", 6.9e9, Some("sdxl"))
            .served("model-b.safetensors", 6.9e9, Served::Gated(403))
            .served("model-c.safetensors", 6.9e9, Served::Gated(401))
            // Listed, but the Hub has no such file.
            .file("model-d.safetensors", 6.9e9, None)
            // A split file: labelled by its name, with the version its first shard says.
            .file("big-00001-of-00002.safetensors", 5e9, Some("flux1-dev"))
            .file("big-00002-of-00002.safetensors", 5e9, None)
            .file("big.safetensors.index.json", 1e5, None);
        let s = hub_server_with(repo).await;
        let r = DetectService::new()
            .read_repo(&s.client(), &s.detail())
            .await;
        let not_read = |reason| FileLabel::NotRead { reason };
        assert!(matches!(
            r.labels["model-a.safetensors"],
            FileLabel::Works { .. }
        ));
        assert_eq!(
            r.labels["model-b.safetensors"],
            not_read(NotRead::NeedsTerms)
        );
        assert_eq!(
            r.labels["model-c.safetensors"],
            not_read(NotRead::NeedsToken)
        );
        assert_eq!(r.labels["model-d.safetensors"], not_read(NotRead::Failed));
        for path in [
            "big-00001-of-00002.safetensors",
            "big-00002-of-00002.safetensors",
            "big.safetensors.index.json",
        ] {
            assert_eq!(
                r.labels[path],
                FileLabel::CannotRun {
                    reason: CannotRunReason::Sharded,
                    version: Some(SdVersion::Flux)
                },
                "{path}"
            );
        }
        assert_eq!((r.read, r.unread), (2, 3));
        // Read files are kept, refusals are not: the next opening asks for those three again
        // (one `HEAD` each, refused), and for nothing else.
        let d = DetectService::new();
        d.read_repo(&s.client(), &s.detail()).await;
        let before = s.requests();
        d.read_repo(&s.client(), &s.detail()).await;
        assert_eq!(s.requests() - before, 3);
    }

    /// A Tauri command awaits the reading on any thread.
    #[test]
    fn reading_a_repo_can_move_between_threads() {
        fn send<T: Send>(_: &T) {}
        let (d, client, detail) = (
            DetectService::new(),
            HfClient::for_test("http://127.0.0.1:9"),
            sdxl_repo().detail,
        );
        send(&d.read_repo(&client, &detail));
    }

    #[test]
    fn the_cache_forgets_the_file_used_longest_ago() {
        let key = |i: usize| CacheKey {
            repo: "o/r".into(),
            revision: COMMIT.into(),
            path: format!("{i}.gguf"),
            sha256: format!("{i:064}"),
        };
        let facts = models::sd::inspect::inspect(&fixture("sdxl"));
        let mut cache = Cache::default();
        for i in 0..CACHE_ENTRIES {
            cache.put(key(i), facts.clone());
        }
        // Using the first one makes the second the oldest.
        assert!(cache.get(&key(0)).is_some());
        cache.put(key(CACHE_ENTRIES), facts.clone());
        assert_eq!(cache.map.len(), CACHE_ENTRIES);
        assert_eq!(cache.order.len(), CACHE_ENTRIES);
        assert!(cache.get(&key(1)).is_none());
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(CACHE_ENTRIES)).is_some());
        // Putting a key again does not count it twice.
        cache.put(key(0), facts);
        assert_eq!(cache.order.len(), CACHE_ENTRIES);
    }
}
