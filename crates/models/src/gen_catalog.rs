//! Picture and video models in the signed catalog (studio-local-design.md, "the catalog
//! entry"): the `generators`, `generatorFamilies` and `generatorTiers` keys that
//! [`Catalog`] keeps as raw JSON, read here one entry at a time.
//!
//! An entry or family record that cannot be used is hidden with a reason, never an
//! error: an unknown family, role, input, kind or quirk (a newer catalog than this
//! build), a newer Studio engine, a part that is not a pinned `.gguf` or `.safetensors`
//! file, a licence that is region-limited or whose text is not the bundled one. The
//! chat catalog never depends on any of it.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::catalog::{Catalog, is_hex};
use crate::hf::{validate_repo_id, validate_repo_path};

/// What a model makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenKind {
    /// Pictures from words (and, with [`GenInput::Reference`], from pictures).
    Image,
    /// Changes to a picture the user adds.
    ImageEdit,
    /// Short clips.
    Video,
}

/// The families harnessUI knows how to run. The list is compiled in because a family
/// decides engine flags and quirks; anything else in the catalog is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GenFamily {
    /// FLUX.2 klein 4B.
    #[serde(rename = "flux2-klein-4b")]
    Flux2Klein4b,
    /// Z-Image Turbo.
    #[serde(rename = "z-image-turbo")]
    ZImageTurbo,
    /// A 4-step distillation of Wan 2.2 TI2V-5B. The full Wan 2.2 5B (about 30–50 steps,
    /// cfg 5) is not a family.
    #[serde(rename = "wan2.2-ti2v-5b-fast")]
    Wan22Ti2v5bFast,
    /// Wan 2.1 T2V 1.3B.
    #[serde(rename = "wan2.1-t2v-1.3b")]
    Wan21T2v1_3b,
}

/// What a part is to the engine; each role has one fixed engine flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenRole {
    /// The diffusion model on its own.
    Diffusion,
    /// The high-noise diffusion model of a two-model video family.
    HighNoiseDiffusion,
    /// A language model used as the text encoder.
    Llm,
    /// The T5-XXL text encoder.
    T5xxl,
    /// The CLIP-L text encoder.
    ClipL,
    /// The CLIP vision encoder.
    ClipVision,
    /// The decoder.
    Vae,
    /// One file holding the whole model.
    Model,
}

impl GenRole {
    /// Whether the part is the model itself (an entry has exactly one).
    fn is_model(self) -> bool {
        matches!(self, Self::Diffusion | Self::Model)
    }
}

/// A picture a model takes beside the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenInput {
    /// Pictures to follow or change.
    Reference,
    /// The first frame of a clip.
    StartFrame,
    /// The last frame of a clip.
    EndFrame,
}

/// A compiled-in adjustment for one platform; never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Quirk {
    /// `GGML_METAL_TENSOR_DISABLE=1` on macOS.
    MetalTensorOff,
    /// Animated WebP instead of WebM for video on macOS.
    VideoWebpOnMac,
}

/// Whether a licence lets people use the outputs commercially, from the least strict
/// to the strictest (the derived order is the strictness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Commercial {
    /// Free for any use, including commercial.
    Yes,
    /// Commercial use only under a yearly revenue ([`GenLicence::revenue_limit_usd`]).
    UnderRevenue,
    /// Commercial use after registering with the publisher.
    Registration,
    /// The licence does not say clearly.
    Unclear,
    /// Non-commercial.
    No,
}

/// The colour of a licence's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Badge {
    /// Free for any use, including commercial.
    Green,
    /// Commercial use with a condition.
    Amber,
    /// Non-commercial, or check the licence.
    Red,
}

/// How a repository is gated on Hugging Face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateMode {
    /// Not gated.
    None,
    /// Access is granted at once after accepting.
    Auto,
    /// The publisher approves each request.
    Manual,
}

/// Which frame counts a video model takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FrameRule {
    /// 4n + 1 frames (Wan).
    #[serde(rename = "4n+1")]
    FourNPlusOne,
    /// 8n + 1 frames (LTX).
    #[serde(rename = "8n+1")]
    EightNPlusOne,
}

/// A licence one or more parts are under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenLicence {
    /// Its key in the entry's `licenses`, which parts name (`apache-2.0`).
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// The upstream page at a pinned revision.
    pub url: String,
    /// SHA-256 of the licence text: the bundled canonical text's when there is one
    /// ([`BUNDLED_LICENCE_TEXTS`]).
    pub sha256: String,
    pub commercial_outputs: Commercial,
    #[serde(default)]
    pub revenue_limit_usd: Option<u64>,
    /// Regions where the licence does not apply; such a licence is never offered.
    #[serde(default)]
    pub excluded_regions: Vec<String>,
    #[serde(default)]
    pub obligations: Vec<String>,
    pub plain_words: String,
}

impl GenLicence {
    /// Green when outputs are free for any use; amber with a condition (a revenue limit
    /// or a registration); red when non-commercial or unclear.
    pub fn badge(&self) -> Badge {
        match self.commercial_outputs {
            Commercial::Yes => Badge::Green,
            Commercial::UnderRevenue | Commercial::Registration => Badge::Amber,
            Commercial::Unclear | Commercial::No => Badge::Red,
        }
    }
}

/// How strict a licence is: by [`Commercial`], then a lower revenue limit is stricter
/// (an unstated limit counts as the lowest).
pub(crate) fn strictness(l: &GenLicence) -> (Commercial, Reverse<u64>) {
    let limit = match l.commercial_outputs {
        Commercial::UnderRevenue => l.revenue_limit_usd.unwrap_or(0),
        _ => 0,
    };
    (l.commercial_outputs, Reverse(limit))
}

/// The strictest of `licences`, the first of equals.
fn strictest<'a>(licences: impl Iterator<Item = &'a GenLicence>) -> Option<&'a GenLicence> {
    licences.reduce(|best, l| {
        if strictness(l) > strictness(best) {
            l
        } else {
            best
        }
    })
}

/// One pinned file of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenPart {
    pub role: GenRole,
    /// `owner/name` on Hugging Face.
    pub repo: String,
    /// 40-hex commit.
    pub revision: String,
    /// Path inside the repository.
    pub path: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    /// A key of the entry's `licenses`.
    pub license: String,
}

/// What Hugging Face asks before it serves the files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenGate {
    pub mode: GateMode,
    /// What the publisher collects when access is asked for.
    #[serde(default)]
    pub collects: Vec<String>,
}

/// The settings a run starts from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenDefaults {
    pub steps: u32,
    pub cfg: f32,
    #[serde(default)]
    pub flow_shift: Option<f32>,
    /// An engine sampler word; `None` is the engine's default.
    #[serde(default)]
    pub sampler: Option<String>,
    /// An engine scheduler word; `None` is the engine's default.
    #[serde(default)]
    pub scheduler: Option<String>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub frames: Option<u32>,
    #[serde(default)]
    pub fps: Option<u32>,
}

/// The sizes a model takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenLimits {
    /// Width and height are multiples of this.
    pub multiple_of: u32,
    pub max_side: u32,
    #[serde(default)]
    pub frame_rule: Option<FrameRule>,
    #[serde(default)]
    pub max_frames: Option<u32>,
}

/// Conservative coefficients for the fit check when no measured point applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitFallback {
    pub act_base_bytes: u64,
    pub act_bytes_per_token: u64,
    pub vae_bytes_per_pixel_frame: u64,
    pub vae_tile_cap_bytes: u64,
    /// Pixels per latent token along each side: 16, or 32 for Wan 2.2 TI2V.
    pub token_div: u32,
    /// Frames per latent frame: 4 for Wan.
    #[serde(default)]
    pub frame_div: Option<u32>,
    /// Frames decoded at once (Wan decodes one latent frame, 4 frames, at a time);
    /// `None` decodes all of them together.
    #[serde(default)]
    pub vae_chunk_frames: Option<u32>,
}

/// A measured peak of memory for one size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPoint {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub peak_bytes: u64,
    pub device: String,
    /// The engine build it was measured with.
    pub build: u32,
}

/// A measured run time for one size on one class of device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedPoint {
    /// `<backend>-<GB>` ("vulkan-8", "metal-16"), or "cpu".
    pub device_class: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub load_seconds: f64,
    pub seconds_per_step: f64,
    pub decode_seconds: f64,
    /// Where and when it was measured.
    pub measured_on: String,
}

/// A picture or video model that can be offered on this build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenEntry {
    pub id: String,
    pub name: String,
    /// One line about what it is good for.
    pub summary: String,
    /// Who made the model (not who converted it).
    pub publisher: String,
    pub kind: GenKind,
    pub family: GenFamily,
    pub inputs: Vec<GenInput>,
    /// Exactly one `diffusion` or `model` part, plus encoders and the decoder.
    pub parts: Vec<GenPart>,
    /// Every licence a part names, by id.
    pub licenses: BTreeMap<String, GenLicence>,
    pub gate: GenGate,
    pub defaults: GenDefaults,
    pub limits: GenLimits,
    pub fallback: FitFallback,
    pub memory: Vec<MemoryPoint>,
    pub speed: Vec<SpeedPoint>,
    pub quirks: Vec<Quirk>,
    /// The oldest Studio engine build that runs it.
    pub min_build: u32,
    pub runs_on_apple: bool,
    /// The date it last passed the catalog test.
    pub tested: Option<String>,
}

impl GenEntry {
    /// Every part's size added up: what a download from nothing takes.
    pub fn total_bytes(&self) -> u64 {
        self.parts.iter().map(|p| p.size).sum()
    }

    /// The part holding the model (`diffusion` or `model`).
    ///
    /// # Panics
    /// When there is none, which [`gen_catalog`] never lets through.
    pub fn diffusion(&self) -> &GenPart {
        self.parts
            .iter()
            .find(|p| p.role.is_model())
            .expect("an entry has a model part (gen_catalog hides one without)")
    }

    /// The strictest licence any part is under: it decides the badge. An entry built by
    /// hand whose parts name none of its licences gets the strictest of them all.
    ///
    /// # Panics
    /// When the entry has no licence at all, which [`gen_catalog`] never lets through.
    pub fn effective_licence(&self) -> &GenLicence {
        let named = self
            .parts
            .iter()
            .filter_map(|p| self.licenses.get(&p.license));
        strictest(named)
            .or_else(|| strictest(self.licenses.values()))
            .expect("an entry has a licence (gen_catalog hides a part without one)")
    }
}

/// What harnessUI knows about a family: what a searched repository of it needs beside
/// its diffusion file, and the figures entries of the family take when they state none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyRecord {
    pub family: GenFamily,
    pub name: String,
    pub publisher: String,
    pub kind: GenKind,
    pub inputs: Vec<GenInput>,
    /// The licence of the family's upstream model (a key of `licenses`), which a
    /// searched diffusion file is under at least.
    pub upstream_license: String,
    /// The encoders and the decoder, never the model itself.
    pub companions: Vec<GenPart>,
    pub licenses: BTreeMap<String, GenLicence>,
    pub defaults: GenDefaults,
    pub limits: GenLimits,
    pub fallback: FitFallback,
    pub memory: Vec<MemoryPoint>,
    pub speed: Vec<SpeedPoint>,
    pub quirks: Vec<Quirk>,
    pub min_build: u32,
    pub runs_on_apple: bool,
    /// The diffusion model's parameter count band, inclusive (4B klein: 3.5e9..=4.5e9),
    /// checked when a listing states it.
    pub parameters: (u64, u64),
}

/// Recommended entries by usable memory in GB, best first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenTiers {
    pub image: BTreeMap<u64, Vec<String>>,
    pub video: BTreeMap<u64, Vec<String>>,
}

/// An entry (by id) or family record (`family:<key>`) left out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hidden {
    pub id: String,
    pub reason: String,
}

/// The catalog's picture and video models as this build can use them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenCatalog {
    pub entries: BTreeMap<String, GenEntry>,
    pub families: BTreeMap<GenFamily, FamilyRecord>,
    pub tiers: GenTiers,
    pub hidden: Vec<Hidden>,
}

/// The only file types a part may be (`.ckpt`, `.pt`, `.pth` and other pickle files can
/// run code). Compared without regard to case.
pub const ALLOWED_PART_EXTENSIONS: [&str; 2] = [".gguf", ".safetensors"];

/// Canonical licence texts bundled with the app (third_party/licenses/models/), by licence
/// id: the catalog pins their SHA-256.
pub const BUNDLED_LICENCE_TEXTS: &[(&str, &str)] = &[(
    "apache-2.0",
    include_str!("../../../third_party/licenses/models/apache-2.0.txt"),
)];

const UNKNOWN: &str = "unknown family, role, input, kind or quirk";
const NEWER_ENGINE: &str = "needs a newer Studio engine";
const NOT_ALLOWED_FILE: &str = "not a .gguf or .safetensors file";
const NOT_PINNED: &str = "a part is not pinned";
const NO_LICENCE: &str = "a part has no licence";
const REGION_LIMITED: &str = "region-limited";
const TEXT_MISMATCH: &str = "licence text does not match";
const NO_MODEL_PART: &str = "no model part";
const SEVERAL_MODEL_PARTS: &str = "more than one model part";
const COMPANION_IS_MODEL: &str = "a companion is a model part";
const NO_FAMILY_RECORD: &str = "its family record is missing";

/// An entry as the catalog writes it: the figures its family gives may be left out.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenEntryRaw {
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    publisher: String,
    kind: GenKind,
    family: GenFamily,
    #[serde(default)]
    inputs: Vec<GenInput>,
    parts: Vec<GenPart>,
    licenses: BTreeMap<String, GenLicence>,
    gate: GenGate,
    defaults: GenDefaults,
    #[serde(default)]
    limits: Option<GenLimits>,
    #[serde(default)]
    fallback: Option<FitFallback>,
    #[serde(default)]
    memory: Vec<MemoryPoint>,
    #[serde(default)]
    speed: Option<Vec<SpeedPoint>>,
    #[serde(default)]
    quirks: Vec<Quirk>,
    #[serde(default)]
    min_build: Option<u32>,
    #[serde(default)]
    runs_on_apple: Option<bool>,
    #[serde(default)]
    tested: Option<String>,
}

/// A family record as the catalog writes it; its family is its key.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FamilyRecordRaw {
    name: String,
    #[serde(default)]
    publisher: String,
    kind: GenKind,
    #[serde(default)]
    inputs: Vec<GenInput>,
    upstream_license: String,
    #[serde(default)]
    companions: Vec<GenPart>,
    licenses: BTreeMap<String, GenLicence>,
    defaults: GenDefaults,
    limits: GenLimits,
    fallback: FitFallback,
    #[serde(default)]
    memory: Vec<MemoryPoint>,
    #[serde(default)]
    speed: Vec<SpeedPoint>,
    #[serde(default)]
    quirks: Vec<Quirk>,
    min_build: u32,
    #[serde(default = "runs_on_apple_unless_said")]
    runs_on_apple: bool,
    parameters: (u64, u64),
}

fn runs_on_apple_unless_said() -> bool {
    true
}

/// Reads the catalog's picture and video models for an engine of `engine_build` (the app
/// passes `runtime::sd::lock::embedded().sd_cpp.build`). Never fails: what cannot be
/// used is listed in `hidden`, a family record as `family:<key>`.
pub fn gen_catalog(catalog: &Catalog, engine_build: u32) -> GenCatalog {
    let mut hidden = Vec::new();
    // Every record that reads and passes its checks: entries take their figures from it
    // whatever engine it needs (an entry taking its build is then hidden for it).
    let mut records = BTreeMap::new();
    for (key, raw) in &catalog.generator_families {
        match read_family(key, raw) {
            Ok(record) => {
                records.insert(record.family, record);
            }
            Err(reason) => hidden.push(Hidden {
                id: format!("family:{key}"),
                reason,
            }),
        }
    }
    let mut entries = BTreeMap::new();
    for (id, raw) in &catalog.generators {
        match read_entry(id, raw, &records, engine_build) {
            Ok(entry) => {
                entries.insert(id.clone(), entry);
            }
            Err(reason) => hidden.push(Hidden {
                id: id.clone(),
                reason,
            }),
        }
    }
    let mut families = BTreeMap::new();
    for (family, record) in records {
        if record.min_build <= engine_build {
            families.insert(family, record);
        } else {
            hidden.push(Hidden {
                id: format!("family:{}", family_key(family)),
                reason: NEWER_ENGINE.into(),
            });
        }
    }
    let tiers = read_tiers(&catalog.generator_tiers, &entries);
    GenCatalog {
        entries,
        families,
        tiers,
        hidden,
    }
}

/// The reason a value did not read: an enum word this build does not know, or another
/// mistake (`what` names the thing, with serde's message).
fn unreadable(e: &serde_json::Error, what: &str) -> String {
    if e.to_string().starts_with("unknown variant") {
        UNKNOWN.into()
    } else {
        format!("not a valid {what}: {e}")
    }
}

fn family_key(family: GenFamily) -> String {
    match serde_json::to_value(family) {
        Ok(Value::String(s)) => s,
        _ => format!("{family:?}"),
    }
}

fn read_entry(
    id: &str,
    raw: &Value,
    records: &BTreeMap<GenFamily, FamilyRecord>,
    engine_build: u32,
) -> Result<GenEntry, String> {
    let raw: GenEntryRaw =
        serde_json::from_value(raw.clone()).map_err(|e| unreadable(&e, "entry"))?;
    let record = records.get(&raw.family);
    let limits = own_or_family(raw.limits, record, |r| r.limits.clone())?;
    let fallback = own_or_family(raw.fallback, record, |r| r.fallback.clone())?;
    let min_build = own_or_family(raw.min_build, record, |r| r.min_build)?;
    let speed = raw
        .speed
        .or_else(|| record.map(|r| r.speed.clone()))
        .unwrap_or_default();
    let runs_on_apple = raw
        .runs_on_apple
        .or_else(|| record.map(|r| r.runs_on_apple))
        .unwrap_or(true);
    if min_build > engine_build {
        return Err(NEWER_ENGINE.into());
    }
    let licenses = keyed(raw.licenses);
    check_parts(&raw.parts, &licenses)?;
    check_licences(&licenses)?;
    match raw.parts.iter().filter(|p| p.role.is_model()).count() {
        0 => return Err(NO_MODEL_PART.into()),
        1 => {}
        _ => return Err(SEVERAL_MODEL_PARTS.into()),
    }
    Ok(GenEntry {
        id: id.to_string(),
        name: raw.name,
        summary: raw.summary,
        publisher: raw.publisher,
        kind: raw.kind,
        family: raw.family,
        inputs: raw.inputs,
        parts: raw.parts,
        licenses,
        gate: raw.gate,
        defaults: raw.defaults,
        limits,
        fallback,
        memory: raw.memory,
        speed,
        quirks: raw.quirks,
        min_build,
        runs_on_apple,
        tested: raw.tested,
    })
}

/// The entry's own value, else its family record's; hidden when it has neither.
fn own_or_family<T>(
    own: Option<T>,
    record: Option<&FamilyRecord>,
    take: impl FnOnce(&FamilyRecord) -> T,
) -> Result<T, String> {
    own.or_else(|| record.map(take))
        .ok_or_else(|| NO_FAMILY_RECORD.to_string())
}

/// A family record with every check an entry gets except its engine build, which
/// [`gen_catalog`] applies after entries have taken their figures.
fn read_family(key: &str, raw: &Value) -> Result<FamilyRecord, String> {
    let family: GenFamily = serde_json::from_value(Value::String(key.to_string()))
        .map_err(|e| unreadable(&e, "family"))?;
    let raw: FamilyRecordRaw =
        serde_json::from_value(raw.clone()).map_err(|e| unreadable(&e, "family record"))?;
    let licenses = keyed(raw.licenses);
    check_parts(&raw.companions, &licenses)?;
    if !licenses.contains_key(&raw.upstream_license) {
        return Err(NO_LICENCE.into());
    }
    check_licences(&licenses)?;
    if raw.companions.iter().any(|p| p.role.is_model()) {
        return Err(COMPANION_IS_MODEL.into());
    }
    Ok(FamilyRecord {
        family,
        name: raw.name,
        publisher: raw.publisher,
        kind: raw.kind,
        inputs: raw.inputs,
        upstream_license: raw.upstream_license,
        companions: raw.companions,
        licenses,
        defaults: raw.defaults,
        limits: raw.limits,
        fallback: raw.fallback,
        memory: raw.memory,
        speed: raw.speed,
        quirks: raw.quirks,
        min_build: raw.min_build,
        runs_on_apple: raw.runs_on_apple,
        parameters: raw.parameters,
    })
}

/// Each licence with its id set to its key.
fn keyed(licenses: BTreeMap<String, GenLicence>) -> BTreeMap<String, GenLicence> {
    licenses
        .into_iter()
        .map(|(id, mut l)| {
            l.id.clone_from(&id);
            (id, l)
        })
        .collect()
}

/// Every part is an allowed file, pinned, and names one of `licenses`.
pub(crate) fn check_parts(
    parts: &[GenPart],
    licenses: &BTreeMap<String, GenLicence>,
) -> Result<(), String> {
    for p in parts {
        if validate_repo_id(&p.repo).is_err() || validate_repo_path(&p.path).is_err() {
            return Err(NOT_PINNED.into());
        }
        let path = p.path.to_ascii_lowercase();
        if !ALLOWED_PART_EXTENSIONS
            .iter()
            .any(|ext| path.ends_with(ext))
        {
            return Err(NOT_ALLOWED_FILE.into());
        }
        if !is_hex(&p.revision, 40) || !is_hex(&p.sha256, 64) || p.size == 0 {
            return Err(NOT_PINNED.into());
        }
        if !licenses.contains_key(&p.license) {
            return Err(NO_LICENCE.into());
        }
    }
    Ok(())
}

/// No licence is region-limited, and each one with a bundled text pins that text.
pub(crate) fn check_licences(licenses: &BTreeMap<String, GenLicence>) -> Result<(), String> {
    for l in licenses.values() {
        if !l.excluded_regions.is_empty() {
            return Err(REGION_LIMITED.into());
        }
        let bundled = BUNDLED_LICENCE_TEXTS
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(&l.id));
        if let Some((_, text)) = bundled
            && sha256_hex(text.as_bytes()) != l.sha256
        {
            return Err(TEXT_MISMATCH.into());
        }
    }
    Ok(())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `generatorTiers` as `{"image": {"<GB>": [ids]}, "video": {…}}`: keys that are not
/// numbers, ids that are not usable entries and tiers left empty are dropped.
fn read_tiers(raw: &Value, entries: &BTreeMap<String, GenEntry>) -> GenTiers {
    let kind = |name: &str| -> BTreeMap<u64, Vec<String>> {
        let Some(tiers) = raw.get(name).and_then(Value::as_object) else {
            return BTreeMap::new();
        };
        tiers
            .iter()
            .filter_map(|(gb, ids)| {
                let gb = gb.parse::<u64>().ok()?;
                let ids: Vec<String> = ids
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|id| entries.contains_key(*id))
                    .map(str::to_string)
                    .collect();
                (!ids.is_empty()).then_some((gb, ids))
            })
            .collect()
    };
    GenTiers {
        image: kind("image"),
        video: kind("video"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// One chat model, two usable entries (`sample-image`, `sample-video`), three broken
    /// ones, family records (one of a family this build does not know) and tiers. Every
    /// name, repository and number is made up; the licence hash is the bundled text's.
    const GEN: &str = include_str!("../tests/fixtures/catalog/generators.json");

    /// `json` with its top-level keys replaced by `fields` (`"key": value, …`).
    fn with(json: &str, fields: &str) -> String {
        let mut v: Value = serde_json::from_str(json).unwrap();
        let extra: Value = serde_json::from_str(&format!("{{{fields}}}")).unwrap();
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v.to_string()
    }

    /// The fixture read at build 1106 after `change` to its whole JSON.
    fn read_with(change: impl FnOnce(&mut Value)) -> GenCatalog {
        let mut v: Value = serde_json::from_str(GEN).unwrap();
        change(&mut v);
        gen_catalog(&Catalog::parse(&v.to_string()).unwrap(), 1106)
    }

    /// The fixture read at build 1106 after `change` to `sample-image`.
    fn read_changed(change: impl FnOnce(&mut Value)) -> GenCatalog {
        read_with(|v| change(&mut v["generators"]["sample-image"]))
    }

    fn reason(g: &GenCatalog, id: &str) -> Option<String> {
        g.hidden
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.reason.clone())
    }

    /// `sample-image` with `change` is hidden, for exactly `why`.
    fn hidden_for(why: &str, change: impl FnOnce(&mut Value)) {
        let g = read_changed(change);
        assert!(!g.entries.contains_key("sample-image"), "{why}");
        assert_eq!(reason(&g, "sample-image").as_deref(), Some(why));
        assert!(g.entries.contains_key("sample-video"), "only that entry");
    }

    fn hidden_with_part_path(path: &str) -> bool {
        let g = read_changed(|e| e["parts"][0]["path"] = path.into());
        if g.entries.contains_key("sample-image") {
            return false;
        }
        assert_eq!(
            reason(&g, "sample-image").as_deref(),
            Some("not a .gguf or .safetensors file")
        );
        true
    }

    fn licence(id: &str, commercial: Commercial, limit: Option<u64>) -> GenLicence {
        GenLicence {
            id: id.into(),
            name: id.into(),
            url: format!("https://example.com/{id}"),
            sha256: "ab".repeat(32),
            commercial_outputs: commercial,
            revenue_limit_usd: limit,
            excluded_regions: Vec::new(),
            obligations: Vec::new(),
            plain_words: String::new(),
        }
    }

    /// `sample-image` with its parts naming `ids` in turn (the first part the first id).
    fn entry_with_licences(ids: &[&str]) -> GenEntry {
        let mut e = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106)
            .entries
            .remove("sample-image")
            .unwrap();
        assert!(ids.len() <= e.parts.len());
        let known = [
            licence("apache-2.0", Commercial::Yes, None),
            licence("kommunity", Commercial::UnderRevenue, Some(5_000_000)),
            licence("tiny-revenue", Commercial::UnderRevenue, Some(1_000_000)),
            licence("sign-up", Commercial::Registration, None),
            licence("unclear", Commercial::Unclear, None),
            licence("flux-nc", Commercial::No, None),
        ];
        e.licenses = known
            .into_iter()
            .filter(|l| ids.contains(&l.id.as_str()))
            .map(|l| (l.id.clone(), l))
            .collect();
        for (part, id) in e.parts.iter_mut().zip(ids.iter().cycle()) {
            part.license = (*id).to_string();
        }
        e
    }

    #[test]
    fn generators_are_read_beside_the_chat_models() {
        let c = Catalog::parse(GEN).unwrap();
        assert_eq!(c.models.len(), 1);
        let g = gen_catalog(&c, 1106);
        assert_eq!(
            g.entries.keys().map(String::as_str).collect::<Vec<_>>(),
            ["sample-image", "sample-video"]
        );
        assert_eq!(
            (
                g.entries["sample-video"].kind,
                g.entries["sample-video"].limits.frame_rule
            ),
            (GenKind::Video, Some(FrameRule::FourNPlusOne))
        );
        assert_eq!(g.tiers.image[&8], ["sample-image"]);
    }

    /// Every field of a usable entry, as the fixture writes it (and its family gives).
    #[test]
    fn an_entry_is_read_field_by_field() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        let v = &g.entries["sample-video"];
        assert_eq!(v.id, "sample-video");
        assert_eq!(
            (v.name.as_str(), v.publisher.as_str()),
            ("Sample Clip 5B, fast", "Example Labs")
        );
        assert_eq!(v.family, GenFamily::Wan22Ti2v5bFast);
        assert_eq!(v.inputs, [GenInput::StartFrame]);
        assert_eq!(
            v.parts.iter().map(|p| p.role).collect::<Vec<_>>(),
            [GenRole::Diffusion, GenRole::T5xxl, GenRole::Vae]
        );
        assert_eq!(v.diffusion().path, "Sample-Clip-5B-q6_k.gguf");
        assert_eq!(
            v.total_bytes(),
            4_212_000_000 + 4_148_000_000 + 1_409_400_000
        );
        assert_eq!(
            v.gate,
            GenGate {
                mode: GateMode::None,
                collects: vec![]
            }
        );
        assert_eq!(
            (
                v.defaults.flow_shift,
                v.defaults.frames,
                v.defaults.fps,
                v.defaults.sampler.as_deref()
            ),
            (Some(3.0), Some(49), Some(24), None)
        );
        assert_eq!((v.limits.multiple_of, v.limits.max_frames), (32, Some(121)));
        assert_eq!(
            (
                v.fallback.token_div,
                v.fallback.frame_div,
                v.fallback.vae_chunk_frames
            ),
            (32, Some(4), Some(4))
        );
        assert_eq!(
            v.fallback.act_base_bytes, 1_000_000_000,
            "its own, not the family's"
        );
        assert_eq!(v.speed[0].device_class, "vulkan-8");
        assert_eq!(v.quirks, [Quirk::VideoWebpOnMac]);
        assert_eq!(
            (v.min_build, v.tested.as_deref()),
            (1100, Some("2026-11-20"))
        );
        assert!(!v.runs_on_apple, "its family's");
        let licence = v.effective_licence();
        assert_eq!(
            (licence.id.as_str(), licence.badge()),
            ("apache-2.0", Badge::Green)
        );

        let i = &g.entries["sample-image"];
        assert_eq!(
            (i.kind, i.family),
            (GenKind::Image, GenFamily::Flux2Klein4b)
        );
        assert_eq!(i.inputs, [GenInput::Reference]);
        assert_eq!(i.memory[0].peak_bytes, 6_200_000_000);
        assert!(i.runs_on_apple);
        assert_eq!(g.tiers.video[&8], ["sample-video"]);
    }

    /// The wire words downstream code and the catalog share.
    #[test]
    fn enums_read_and_write_the_catalog_words() {
        let words = |v: Value| serde_json::to_string(&v).unwrap();
        for (value, word) in [
            (json!(GenKind::Image), "image"),
            (json!(GenKind::ImageEdit), "image-edit"),
            (json!(GenKind::Video), "video"),
            (json!(GenFamily::Flux2Klein4b), "flux2-klein-4b"),
            (json!(GenFamily::ZImageTurbo), "z-image-turbo"),
            (json!(GenFamily::Wan22Ti2v5bFast), "wan2.2-ti2v-5b-fast"),
            (json!(GenFamily::Wan21T2v1_3b), "wan2.1-t2v-1.3b"),
            (json!(GenRole::HighNoiseDiffusion), "high-noise-diffusion"),
            (json!(GenRole::T5xxl), "t5xxl"),
            (json!(GenRole::ClipL), "clip-l"),
            (json!(GenRole::ClipVision), "clip-vision"),
            (json!(GenRole::Model), "model"),
            (json!(GenInput::Reference), "reference"),
            (json!(GenInput::StartFrame), "startFrame"),
            (json!(GenInput::EndFrame), "endFrame"),
            (json!(Quirk::MetalTensorOff), "metal-tensor-off"),
            (json!(Quirk::VideoWebpOnMac), "video-webp-on-mac"),
            (json!(Commercial::UnderRevenue), "underRevenue"),
            (json!(Commercial::Registration), "registration"),
            (json!(Badge::Amber), "amber"),
            (json!(GateMode::Manual), "manual"),
            (json!(FrameRule::FourNPlusOne), "4n+1"),
            (json!(FrameRule::EightNPlusOne), "8n+1"),
        ] {
            assert_eq!(words(value), format!("\"{word}\""));
        }
        assert_eq!(
            serde_json::from_str::<GenFamily>("\"wan2.1-t2v-1.3b\"").unwrap(),
            GenFamily::Wan21T2v1_3b
        );
    }

    #[test]
    fn an_unknown_family_role_or_quirk_hides_only_that_entry() {
        let reasons: BTreeMap<_, _> = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106)
            .hidden
            .into_iter()
            .map(|h| (h.id, h.reason))
            .collect();
        for id in ["bad-family", "bad-role", "bad-quirk"] {
            assert_eq!(
                reasons.get(id).map(String::as_str),
                Some("unknown family, role, input, kind or quirk"),
                "{id}"
            );
        }
        hidden_for("unknown family, role, input, kind or quirk", |e| {
            e["inputs"] = json!(["mask"])
        });
        hidden_for("unknown family, role, input, kind or quirk", |e| {
            e["kind"] = json!("audio")
        });
    }

    #[test]
    fn an_entry_that_cannot_be_read_says_why() {
        let g = read_changed(|e| e["defaults"]["steps"] = json!("four"));
        let why = reason(&g, "sample-image").unwrap();
        assert!(why.starts_with("not a valid entry: "), "{why}");
    }

    #[test]
    fn an_entry_needing_a_newer_engine_is_hidden() {
        // sample-image has "minBuild": 1106.
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1105);
        assert!(
            g.hidden
                .iter()
                .any(|h| h.id == "sample-image" && h.reason == "needs a newer Studio engine")
        );
        assert!(g.entries.contains_key("sample-video"), "minBuild 1100");
    }

    #[test]
    fn pickle_files_are_refused() {
        for ext in [".ckpt", ".pt", ".pth", ".bin"] {
            assert!(hidden_with_part_path(&format!("model{ext}")), "{ext}");
        }
        assert!(!hidden_with_part_path("Model.SAFETENSORS"));
        assert!(!hidden_with_part_path("dir/model.GGUF"));
    }

    #[test]
    fn every_part_is_pinned() {
        for (part, field, value) in [
            (0, "revision", json!("2".repeat(39))),
            (2, "revision", json!("4".repeat(41))),
            (1, "revision", json!("main")),
            (1, "revision", json!("A".repeat(40))),
            (0, "sha256", json!("b".repeat(63))),
            (2, "sha256", json!("g".repeat(64))),
            (1, "size", json!(0)),
            (2, "path", json!("split_files/../../vae.safetensors")),
            (0, "path", json!("/sample.gguf")),
            (1, "repo", json!("no-owner")),
            (2, "repo", json!("example/../evil")),
        ] {
            hidden_for("a part is not pinned", |e| e["parts"][part][field] = value);
        }
    }

    #[test]
    fn every_part_names_one_of_its_licences() {
        hidden_for("a part has no licence", |e| {
            e["parts"][1]["license"] = json!("mit")
        });
    }

    #[test]
    fn an_entry_has_exactly_one_model_part() {
        hidden_for("no model part", |e| {
            e["parts"][0]["role"] = json!("high-noise-diffusion")
        });
        hidden_for("more than one model part", |e| {
            e["parts"][1]["role"] = json!("model")
        });
        let g = read_changed(|e| e["parts"][0]["role"] = json!("model"));
        assert_eq!(g.entries["sample-image"].diffusion().role, GenRole::Model);
    }

    #[test]
    fn the_strictest_licence_of_all_parts_decides() {
        // kommunity: UnderRevenue, 5_000_000.
        let e = entry_with_licences(&["apache-2.0", "kommunity"]);
        assert_eq!(
            (
                e.effective_licence().id.as_str(),
                e.effective_licence().badge()
            ),
            ("kommunity", Badge::Amber)
        );
        assert_eq!(
            entry_with_licences(&["apache-2.0", "kommunity", "flux-nc"])
                .effective_licence()
                .badge(),
            Badge::Red
        );
        let strictest = |ids: &[&str]| entry_with_licences(ids).effective_licence().id.clone();
        assert_eq!(
            strictest(&["kommunity", "tiny-revenue"]),
            "tiny-revenue",
            "a lower limit"
        );
        assert_eq!(
            strictest(&["kommunity", "sign-up", "apache-2.0"]),
            "sign-up"
        );
        assert_eq!(strictest(&["unclear", "sign-up"]), "unclear");
        assert_eq!(strictest(&["unclear", "flux-nc"]), "flux-nc");
        assert_eq!(strictest(&["apache-2.0"]), "apache-2.0");
    }

    #[test]
    fn each_licence_has_its_badge() {
        for (commercial, badge) in [
            (Commercial::Yes, Badge::Green),
            (Commercial::UnderRevenue, Badge::Amber),
            (Commercial::Registration, Badge::Amber),
            (Commercial::Unclear, Badge::Red),
            (Commercial::No, Badge::Red),
        ] {
            assert_eq!(
                licence("x", commercial, None).badge(),
                badge,
                "{commercial:?}"
            );
        }
    }

    #[test]
    fn a_region_limited_licence_is_never_offered() {
        hidden_for("region-limited", |e| {
            e["licenses"]["apache-2.0"]["excludedRegions"] = json!(["EU"])
        });
    }

    #[test]
    fn a_licence_hash_is_the_bundled_texts() {
        hidden_for("licence text does not match", |e| {
            e["licenses"]["apache-2.0"]["sha256"] = json!("0".repeat(64))
        });
        // A licence id with no bundled text keeps its own pinned SHA-256.
        let g = read_changed(|e| {
            let mut l = e["licenses"]["apache-2.0"].take();
            l["sha256"] = json!("ab".repeat(32));
            e["licenses"] = json!({ "example-licence": l });
            for p in e["parts"].as_array_mut().unwrap() {
                p["license"] = json!("example-licence");
            }
        });
        let e = &g.entries["sample-image"];
        assert_eq!(e.effective_licence().id, "example-licence");
        assert_eq!(e.effective_licence().sha256, "ab".repeat(32));
    }

    /// The canonical Apache-2.0 text from apache.org, byte for byte (`.gitattributes` keeps
    /// Git from changing its line endings on Windows, which would change the hash).
    #[test]
    fn the_bundled_apache_text_is_the_canonical_one() {
        let (id, text) = BUNDLED_LICENCE_TEXTS[0];
        assert_eq!(id, "apache-2.0");
        assert_eq!(
            sha256_hex(text.as_bytes()),
            "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"
        );
    }

    #[test]
    fn broken_generators_never_fail_the_catalog() {
        let c = Catalog::parse(&with(
            GEN,
            r#""generators": {"x": 5}, "generatorTiers": "nope""#,
        ))
        .unwrap();
        let g = gen_catalog(&c, 1106);
        assert!(
            g.entries.is_empty()
                && g.tiers.image.is_empty()
                && g.hidden.iter().any(|h| h.id == "x")
        );
        assert_eq!(c.models.len(), 1);
    }

    #[test]
    fn tiers_keep_numbers_and_known_entries() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        // "lots" is not a number, "16" is not a list, "bad-family" is hidden and
        // "no-such-entry" and 42 are not entries.
        assert_eq!(g.tiers.image.keys().copied().collect::<Vec<_>>(), [0, 8]);
        assert_eq!(g.tiers.image[&0], ["sample-image"]);
        assert_eq!(g.tiers.video.keys().copied().collect::<Vec<_>>(), [8]);
        // A tier left with no entry is dropped.
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1105);
        assert!(g.tiers.image.is_empty(), "{:?}", g.tiers.image);
    }

    #[test]
    fn an_entry_without_a_fallback_takes_its_familys() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        let e = &g.entries["sample-image"];
        let family = &g.families[&GenFamily::Flux2Klein4b];
        assert_eq!(e.fallback, family.fallback);
        assert_eq!(e.speed, family.speed, "speed too");
        assert_eq!(e.runs_on_apple, family.runs_on_apple);
    }

    #[test]
    fn an_entry_without_limits_or_min_build_takes_its_familys() {
        let g = read_with(|v| {
            let e = v["generators"]["sample-image"].as_object_mut().unwrap();
            e.remove("limits");
            e.remove("minBuild");
            v["generatorFamilies"]["flux2-klein-4b"]["limits"]["maxSide"] = json!(1536);
            v["generatorFamilies"]["flux2-klein-4b"]["minBuild"] = json!(1104);
        });
        let e = &g.entries["sample-image"];
        assert_eq!((e.limits.max_side, e.min_build), (1536, 1104));
        assert_eq!(e.limits, g.families[&GenFamily::Flux2Klein4b].limits);
    }

    #[test]
    fn an_entry_with_nothing_to_take_from_is_hidden() {
        let g = read_with(|v| {
            v["generatorFamilies"]
                .as_object_mut()
                .unwrap()
                .remove("flux2-klein-4b");
        });
        assert_eq!(
            reason(&g, "sample-image").as_deref(),
            Some("its family record is missing")
        );
        // With its own figures it needs no record.
        let g = read_with(|v| {
            v["generatorFamilies"]
                .as_object_mut()
                .unwrap()
                .remove("flux2-klein-4b");
            v["generators"]["sample-image"]["fallback"] =
                v["generators"]["sample-video"]["fallback"].clone();
        });
        let e = &g.entries["sample-image"];
        assert!(e.speed.is_empty() && e.runs_on_apple, "the defaults");
    }

    #[test]
    fn family_records_are_read_by_family() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        assert_eq!(
            g.families.keys().copied().collect::<Vec<_>>(),
            [GenFamily::Flux2Klein4b, GenFamily::Wan22Ti2v5bFast]
        );
        let klein = &g.families[&GenFamily::Flux2Klein4b];
        assert_eq!(klein.family, GenFamily::Flux2Klein4b);
        assert_eq!(klein.parameters, (3_500_000_000, 4_500_000_000));
        assert_eq!(klein.upstream_license, "apache-2.0");
        assert_eq!(klein.licenses["apache-2.0"].id, "apache-2.0");
        assert_eq!(
            klein.companions.iter().map(|p| p.role).collect::<Vec<_>>(),
            [GenRole::Llm, GenRole::Vae]
        );
        assert_eq!(
            (klein.kind, klein.inputs.as_slice()),
            (GenKind::Image, [GenInput::Reference].as_slice())
        );
        assert_eq!(
            reason(&g, "family:sd3.5-medium").as_deref(),
            Some("unknown family, role, input, kind or quirk")
        );
    }

    #[test]
    fn a_family_record_is_checked_like_an_entry() {
        let family = |change: fn(&mut Value)| {
            let g = read_with(|v| change(&mut v["generatorFamilies"]["flux2-klein-4b"]));
            assert!(!g.families.contains_key(&GenFamily::Flux2Klein4b));
            reason(&g, "family:flux2-klein-4b").unwrap()
        };
        assert_eq!(
            family(|f| f["companions"][0]["size"] = json!(0)),
            "a part is not pinned"
        );
        assert_eq!(
            family(|f| f["companions"][1]["path"] = json!("vae.pt")),
            "not a .gguf or .safetensors file"
        );
        assert_eq!(
            family(|f| f["upstreamLicense"] = json!("mit")),
            "a part has no licence"
        );
        assert_eq!(
            family(|f| f["companions"][0]["license"] = json!("mit")),
            "a part has no licence"
        );
        assert_eq!(
            family(|f| f["licenses"]["apache-2.0"]["excludedRegions"] = json!(["EU"])),
            "region-limited"
        );
        assert_eq!(
            family(|f| f["licenses"]["apache-2.0"]["sha256"] = json!("0".repeat(64))),
            "licence text does not match"
        );
        assert_eq!(
            family(|f| f["companions"][0]["role"] = json!("diffusion")),
            "a companion is a model part"
        );
        assert!(family(|f| f["parameters"] = json!(4)).starts_with("not a valid family record: "));
        // A record needing a newer engine is left out; so is an entry that takes its build.
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1105);
        assert!(!g.families.contains_key(&GenFamily::Flux2Klein4b));
        assert_eq!(
            reason(&g, "family:flux2-klein-4b").as_deref(),
            Some("needs a newer Studio engine")
        );
    }
}
