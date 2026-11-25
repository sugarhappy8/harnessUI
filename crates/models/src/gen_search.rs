//! Picture and video models found on Hugging Face (studio-local-design.md, "Get local
//! models"): what a search result offers from its name, and the licence a searched file is
//! under.
//!
//! A name never proves what a file holds: [`verdict_hint`] says what a search result offers
//! from its name alone, until the repo is opened and its files' headers are read
//! (`sd::candidates`, and the app's `studio::detect`); the engine's detection on the
//! downloaded file's own header (`downloads::check_main_file` in the app) decides whether it
//! is added and run.
//!
//! Licences: a 0.1.x family's searched file is under the stricter of the family's licence and
//! the repo's label ([`searched_licence`]); a generic model, which has no family upstream, is
//! under its repo's label ([`repo_licence`]).

use crate::files::RepoFile;
use crate::gen_catalog::{
    BUNDLED_LICENCE_TEXTS, Commercial, FamilyRecord, GenLicence, sha256_hex, strictness,
};
use crate::gguf::TensorShape;
use crate::hf::{OFFICIAL_ENDPOINT, RepoDetail, uploader_text};
use crate::sd::detect::SdVersion;
use crate::sd::inspect::MatchFacts;
use crate::sd::matching::{
    Match, RecipeKind, kind_of, match_recipe, normalise as name_text, recipes_for, tokens,
};
use crate::sd::{CannotRunReason, Verdict};

// ---- search verdicts from names (0.2.0) --------------------------------------------------

/// A repo name read for [`verdict_hint`]'s words.
struct HintName {
    /// Lower case, `_` and spaces read as `-` ([`crate::sd::matching::normalise`]).
    text: String,
    words: Vec<String>,
}

impl HintName {
    fn new(name: &str) -> Self {
        HintName {
            text: name_text(name),
            words: tokens(name),
        }
    }

    /// Whether the name has `word`: a whole word ([`tokens`]), or, for a word with `-` or
    /// `.`, a part of the whole name. R7's words keep a `.` only between digits, so
    /// `flux.1` is two words there and is read in the whole name.
    fn has(&self, word: &str) -> bool {
        if word.contains(['-', '.']) {
            self.text.contains(word)
        } else {
            self.words.iter().any(|w| w == word)
        }
    }

    fn has_any(&self, words: &[&str]) -> bool {
        words.iter().any(|w| self.has(w))
    }
}

/// Names of models Studio can't run, after `lora`; the first match wins.
const CANNOT_RUN_WORDS: [(&[&str], CannotRunReason); 4] = [
    (&["lumina", "neta"], CannotRunReason::Lumina2),
    (&["ltx-video", "ltxv"], CannotRunReason::LtxVideo09),
    (&["hidream-i1"], CannotRunReason::UnknownWeights),
    (&["svd", "stable-video"], CannotRunReason::Svd),
];

const WAN_WORDS: &[&str] = &["wan2.1", "wan2.2", "wan"];

/// The version words: any of the first, with all of the second; the first match wins.
const VERSION_WORDS: [(&[&str], &[&str], SdVersion); 19] = [
    (&["sdxl"], &[], SdVersion::Sdxl),
    (&["sd1.5", "v1-5", "sd15"], &[], SdVersion::Sd1),
    (&["sd2", "v2-1"], &[], SdVersion::Sd2),
    (&["sd3", "sd3.5"], &[], SdVersion::Sd3),
    (
        &["flux.1", "flux1", "kontext", "schnell"],
        &[],
        SdVersion::Flux,
    ),
    (&["flux.2"], &["klein"], SdVersion::Flux2Klein),
    (&["flux.2"], &[], SdVersion::Flux2),
    (&["flux.3"], &[], SdVersion::Flux3),
    (&["chroma"], &[], SdVersion::Chroma),
    (&["qwen-image"], &[], SdVersion::QwenImage),
    (&["z-image"], &[], SdVersion::ZImage),
    (WAN_WORDS, &["ti2v"], SdVersion::Wan22Ti2v),
    (WAN_WORDS, &[], SdVersion::Wan2),
    (
        &["hunyuanvideo-1.5", "hunyuan-video-1.5"],
        &[],
        SdVersion::HunyuanVideo,
    ),
    (&["ltx-2.3", "ltx-2.5"], &[], SdVersion::Ltxav),
    (&["ovis"], &[], SdVersion::OvisImage),
    (&["longcat"], &[], SdVersion::Longcat),
    (&["pixart"], &[], SdVersion::Pixart),
    (&["anima"], &[], SdVersion::Anima),
];

/// The pipeline tags of picture and video models, and the kind each says.
const MEDIA_TAGS: [(&str, RecipeKind); 4] = [
    ("text-to-image", RecipeKind::Picture),
    ("image-to-image", RecipeKind::PictureEdit),
    ("text-to-video", RecipeKind::Video),
    ("image-to-video", RecipeKind::Video),
];

fn version_of(name: &HintName) -> Option<SdVersion> {
    VERSION_WORDS
        .iter()
        .find(|(any, all, _)| name.has_any(any) && all.iter().all(|w| name.has(w)))
        .map(|(.., version)| *version)
}

/// What a model of `version` named `name` makes: from its media pipeline tag
/// ([`MEDIA_TAGS`]), else from the recipe its name picks ([`match_recipe`]), else from the
/// version's default recipe.
fn kind_hint(version: SdVersion, name: &str, pipeline_tag: Option<&str>) -> RecipeKind {
    if let Some((_, kind)) = MEDIA_TAGS
        .iter()
        .find(|(tag, _)| Some(*tag) == pipeline_tag)
    {
        return *kind;
    }
    match match_recipe(version, &MatchFacts::default(), &[], name) {
        Match::Recipe { id, .. } => kind_of(id),
        // A name the table refuses (VACE, LTX-2) is still its version's kind.
        Match::Refused(_) => recipes_for(version)
            .first()
            .map_or(RecipeKind::Picture, |id| kind_of(id)),
    }
}

/// What a search result offers, from its repo id and pipeline tag alone, before its files
/// are read. A pipeline tag that is not a picture or video model's ([`MEDIA_TAGS`]: a chat
/// model's `text-generation`) is `Unknown`. Otherwise the first match: `lora` (a LoRA, for
/// the version its name says, `xl` read as SDXL); a model Studio can't run
/// ([`CANNOT_RUN_WORDS`]); a version ([`VERSION_WORDS`]), whose kind comes from the
/// pipeline tag, else from the name; otherwise `Unknown`.
///
/// `xl` names SDXL only in a LoRA's name or beside a picture model's pipeline tag
/// ([`XL_TAGS`]: `stabilityai/stable-diffusion-xl-base-1.0`, `cagliostrolab/animagine-xl-3.1`):
/// an untagged `google/flan-t5-xl` is no SDXL model.
pub fn verdict_hint(repo_id: &str, pipeline_tag: Option<&str>) -> Verdict {
    if pipeline_tag.is_some_and(|tag| MEDIA_TAGS.iter().all(|(media, _)| *media != tag)) {
        return Verdict::Unknown;
    }
    let name = HintName::new(repo_id);
    if name.has("lora") {
        return Verdict::Lora {
            base: version_of(&name).or_else(|| name.has("xl").then_some(SdVersion::Sdxl)),
        };
    }
    if let Some((_, reason)) = CANNOT_RUN_WORDS
        .iter()
        .find(|(words, _)| name.has_any(words))
    {
        return Verdict::CannotRun {
            reason: *reason,
            version: None,
        };
    }
    let xl = pipeline_tag.is_some_and(|tag| XL_TAGS.contains(&tag)) && name.has("xl");
    match version_of(&name).or_else(|| xl.then_some(SdVersion::Sdxl)) {
        Some(version) => Verdict::Works {
            version,
            kind: kind_hint(version, repo_id, pipeline_tag),
        },
        None => Verdict::Unknown,
    }
}

/// The pipeline tags beside which a name's `xl` says SDXL (ruling S5-xl-3).
const XL_TAGS: [&str; 2] = ["text-to-image", "image-to-image"];

// ---- tensor tables ------------------------------------------------------------------------

/// Prefixes a converter puts before every name (the engine's own converter writes the
/// checkpoint's `model.diffusion_model.`).
const CONVERTER_PREFIXES: [&str; 2] = ["model.diffusion_model.", "diffusion_model."];

/// Name prefixes only picture and video models use (llama.cpp's own are `blk.`,
/// `token_embd`, `output`, and `v.`, `mm.` or `a.` in projectors).
const DIFFUSION_NAMES: &[&str] = &[
    "double_blocks.",
    "single_blocks.",
    "joint_blocks.",
    "transformer_blocks.",
    "single_transformer_blocks.",
    "input_blocks.",
    "middle_block.",
    "output_blocks.",
    "down_blocks.",
    "up_blocks.",
    "mid_block.",
    "time_embed.",
    "time_in.",
    "img_in.",
    "txt_in.",
    "x_embedder.",
    "t_embedder.",
    "cap_embedder.",
    "context_embedder.",
    "patch_embedding.",
    "time_embedding.",
    "noise_refiner.",
    "context_refiner.",
    "first_stage_model.",
    "cond_stage_model.",
    // FLUX.3 Image Klein's image embedding.
    "emb_in.image.",
];

/// Whether a tensor table is a picture or video model's: a converter's prefix, or a name only
/// such models use. Whether it is the model it was downloaded as is the engine's detection's
/// to say (`downloads::check_main_file` in the app).
pub fn looks_like_diffusion(shapes: &[TensorShape]) -> bool {
    shapes.iter().any(|t| {
        CONVERTER_PREFIXES.iter().any(|p| t.name.starts_with(p))
            || DIFFUSION_NAMES.iter().any(|p| t.name.starts_with(p))
    })
}

// ---- licences -----------------------------------------------------------------------------

/// The licence a searched file is under, and why it differs from the repository's label.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchedLicence {
    pub licence: GenLicence,
    /// "The repository says MIT; FLUX.2 klein 4B is Apache 2.0." when the label is not the
    /// family's upstream licence.
    pub label_mismatch: Option<String>,
}

/// A licence id Hugging Face labels use that harnessUI knows: its short words for a
/// sentence, its name and what it allows for outputs.
struct KnownLabel {
    id: &'static str,
    words: &'static str,
    name: &'static str,
    commercial: Commercial,
}

const KNOWN_LABELS: &[KnownLabel] = &[
    KnownLabel {
        id: "apache-2.0",
        words: "Apache 2.0",
        name: "Apache License 2.0",
        commercial: Commercial::Yes,
    },
    KnownLabel {
        id: "mit",
        words: "MIT",
        name: "MIT License",
        commercial: Commercial::Yes,
    },
    KnownLabel {
        id: "bsd-3-clause",
        words: "BSD 3-Clause",
        name: "BSD 3-Clause License",
        commercial: Commercial::Yes,
    },
    KnownLabel {
        id: "bsd-2-clause",
        words: "BSD 2-Clause",
        name: "BSD 2-Clause License",
        commercial: Commercial::Yes,
    },
    KnownLabel {
        id: "cc0-1.0",
        words: "CC0 1.0",
        name: "Creative Commons Zero 1.0",
        commercial: Commercial::Yes,
    },
    KnownLabel {
        id: "cc-by-4.0",
        words: "CC BY 4.0",
        name: "Creative Commons Attribution 4.0",
        commercial: Commercial::Yes,
    },
    // The OpenRAIL licences most SD 1.x and SDXL models carry: commercial use is allowed,
    // with a list of uses they forbid (design 11a).
    KnownLabel {
        id: "creativeml-openrail-m",
        words: "CreativeML Open RAIL-M",
        name: "CreativeML Open RAIL-M",
        commercial: Commercial::WithUseRules,
    },
    KnownLabel {
        id: "openrail",
        words: "Open RAIL",
        name: "Open RAIL",
        commercial: Commercial::WithUseRules,
    },
    KnownLabel {
        id: "openrail++",
        words: "Open RAIL++-M",
        name: "Open RAIL++-M",
        commercial: Commercial::WithUseRules,
    },
    KnownLabel {
        id: "bigscience-openrail-m",
        words: "BigScience Open RAIL-M",
        name: "BigScience Open RAIL-M",
        commercial: Commercial::WithUseRules,
    },
    KnownLabel {
        id: "cc-by-nc-4.0",
        words: "CC BY-NC 4.0",
        name: "Creative Commons Attribution-NonCommercial 4.0",
        commercial: Commercial::No,
    },
    KnownLabel {
        id: "cc-by-nc-sa-4.0",
        words: "CC BY-NC-SA 4.0",
        name: "Creative Commons Attribution-NonCommercial-ShareAlike 4.0",
        commercial: Commercial::No,
    },
    KnownLabel {
        id: "cc-by-nc-nd-4.0",
        words: "CC BY-NC-ND 4.0",
        name: "Creative Commons Attribution-NonCommercial-NoDerivatives 4.0",
        commercial: Commercial::No,
    },
    KnownLabel {
        id: "cc-by-nc-3.0",
        words: "CC BY-NC 3.0",
        name: "Creative Commons Attribution-NonCommercial 3.0",
        commercial: Commercial::No,
    },
    KnownLabel {
        id: "cc-by-nc-sa-3.0",
        words: "CC BY-NC-SA 3.0",
        name: "Creative Commons Attribution-NonCommercial-ShareAlike 3.0",
        commercial: Commercial::No,
    },
    KnownLabel {
        id: "flux-1-dev-non-commercial-license",
        words: "the FLUX.1 [dev] Non-Commercial License",
        name: "FLUX.1 [dev] Non-Commercial License",
        commercial: Commercial::No,
    },
];

const CHECK_THE_LICENCE: &str = "Check the licence before you use what you make.";

/// What a known label allows for what you make, in plain words.
fn plain_words(commercial: Commercial) -> &'static str {
    match commercial {
        Commercial::Yes => "Free for any use, including commercial.",
        Commercial::WithUseRules => "Commercial use allowed, with use rules",
        Commercial::No => "For non-commercial use only.",
        // No known label is one of these.
        Commercial::UnderRevenue | Commercial::Registration | Commercial::Unclear => {
            CHECK_THE_LICENCE
        }
    }
}

fn known_label(id: &str) -> Option<&'static KnownLabel> {
    KNOWN_LABELS.iter().find(|k| k.id.eq_ignore_ascii_case(id))
}

/// A licence the repository's label names, identified by repository revision: its SHA-256
/// is the bundled canonical text's when there is one, else that of
/// `<repo>@<commit>:<label>`; its link is `url`.
fn label_licence(
    detail: &RepoDetail,
    id: &str,
    name: &str,
    commercial: Commercial,
    plain_words: &str,
    url: String,
) -> GenLicence {
    let sha256 = BUNDLED_LICENCE_TEXTS
        .iter()
        .find(|(bundled, _)| bundled.eq_ignore_ascii_case(id))
        .map(|(_, text)| sha256_hex(text.as_bytes()))
        .unwrap_or_else(|| sha256_hex(format!("{}@{}:{id}", detail.id, detail.commit).as_bytes()));
    GenLicence {
        id: id.to_string(),
        name: name.to_string(),
        url,
        sha256,
        commercial_outputs: commercial,
        revenue_limit_usd: None,
        excluded_regions: Vec::new(),
        obligations: Vec::new(),
        plain_words: plain_words.to_string(),
    }
}

/// The repository's page at its commit.
fn tree_url(detail: &RepoDetail) -> String {
    format!("{OFFICIAL_ENDPOINT}/{}/tree/{}", detail.id, detail.commit)
}

/// The repository's label, lower case, as harnessUI reads it: a licence Hugging Face has no
/// id for is labelled `other` with its own name, which HubInfo keeps (made safe): FLUX.1
/// [dev]'s is `flux-1-dev-non-commercial-license`.
fn label_of(detail: &RepoDetail) -> Option<String> {
    let label = detail
        .license
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_ascii_lowercase);
    let own_name = detail
        .info
        .license
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("other"))
        .map(str::to_ascii_lowercase);
    match label.as_deref() {
        Some("other") => own_name.or(label),
        _ => label,
    }
}

/// The licence a searched file of `record`'s family is under: the stricter of the family's
/// upstream licence and the repository's own label. No label, or the same one, is the
/// upstream licence; a known stricter label (a non-commercial one) is that licence; a known
/// looser one (MIT) is the upstream licence; `other` or a label harnessUI does not know is
/// "Check the licence" (red). A label that is not the upstream licence is told in a
/// sentence.
pub fn searched_licence(detail: &RepoDetail, record: &FamilyRecord) -> SearchedLicence {
    let Some(upstream) = record.licenses.get(&record.upstream_license) else {
        // A record without its own licence, which gen_catalog never lets through.
        let licence = label_licence(
            detail,
            "unknown",
            "Unknown licence",
            Commercial::Unclear,
            CHECK_THE_LICENCE,
            tree_url(detail),
        );
        return SearchedLicence {
            licence,
            label_mismatch: None,
        };
    };
    let Some(label) = label_of(detail).filter(|l| *l != upstream.id.to_ascii_lowercase()) else {
        return SearchedLicence {
            licence: upstream.clone(),
            label_mismatch: None,
        };
    };
    let upstream_words = known_label(&upstream.id).map_or(upstream.name.as_str(), |k| k.words);
    let family = &record.name;
    if let Some(known) = known_label(&label) {
        let theirs = label_licence(
            detail,
            known.id,
            known.name,
            known.commercial,
            plain_words(known.commercial),
            tree_url(detail),
        );
        let licence = if strictness(&theirs) > strictness(upstream) {
            theirs
        } else {
            upstream.clone()
        };
        return SearchedLicence {
            licence,
            label_mismatch: Some(format!(
                "The repository says {}; {family} is {upstream_words}.",
                known.words
            )),
        };
    }
    // Uploader text: made safe before it is kept or shown.
    let id = uploader_text(&label, 64);
    let (name, sentence) = if label == "other" {
        (
            "The repository's own licence".to_string(),
            format!("The repository has its own licence; {family} is {upstream_words}."),
        )
    } else {
        (
            id.clone(),
            format!("The repository says {id}; {family} is {upstream_words}."),
        )
    };
    SearchedLicence {
        licence: label_licence(
            detail,
            &id,
            &name,
            Commercial::Unclear,
            CHECK_THE_LICENCE,
            tree_url(detail),
        ),
        label_mismatch: Some(sentence),
    }
}

/// The files a repository's licence text can be in, at its top.
const LICENCE_FILES: [&str; 3] = ["LICENSE", "LICENSE.md", "LICENSE.txt"];

/// The licence a generic model's file is under (design 11a): its repository's label, read
/// through the labels harnessUI knows (the OpenRAIL ones amber "Commercial use allowed, with
/// use rules"). `other` or a label harnessUI does not know, and no label at all, are "Check
/// the licence" (red). Its link is the licence text where the publisher ships one at the
/// repository's top (`LICENSE`, `LICENSE.md` or `LICENSE.txt`, at the pinned commit), else
/// the repository's page at that commit.
pub fn repo_licence(detail: &RepoDetail) -> GenLicence {
    let url = detail
        .files
        .iter()
        .find(|f| LICENCE_FILES.contains(&f.path.as_str()))
        .map_or_else(
            || tree_url(detail),
            |f: &RepoFile| {
                format!(
                    "{OFFICIAL_ENDPOINT}/{}/blob/{}/{}",
                    detail.id, detail.commit, f.path
                )
            },
        );
    match label_of(detail) {
        Some(label) => match known_label(&label) {
            Some(known) => label_licence(
                detail,
                known.id,
                known.name,
                known.commercial,
                plain_words(known.commercial),
                url,
            ),
            None => {
                // Uploader text: made safe before it is kept or shown.
                let id = uploader_text(&label, 64);
                let name = if label == "other" {
                    "The repository's own licence".to_string()
                } else {
                    id.clone()
                };
                label_licence(
                    detail,
                    &id,
                    &name,
                    Commercial::Unclear,
                    CHECK_THE_LICENCE,
                    url,
                )
            }
        },
        None => unstated_licence(detail, url),
    }
}

/// "Check the licence" (red) for a repository that states none.
fn unstated_licence(detail: &RepoDetail, url: String) -> GenLicence {
    label_licence(
        detail,
        "unknown",
        "Unknown licence",
        Commercial::Unclear,
        CHECK_THE_LICENCE,
        url,
    )
}

/// "Check the licence" (red) for a file of repository `repo` at `revision` whose listing
/// could not be read, so its label is not known (ruling S4-licence-ref).
pub fn unread_licence(repo: &str, revision: &str) -> GenLicence {
    let detail = RepoDetail {
        id: repo.to_string(),
        commit: revision.to_string(),
        quantized_from: None,
        gated: crate::hf::Gated::No,
        license: None,
        files: Vec::new(),
        gguf: None,
        template_tools: false,
        template_thinking: false,
        info: crate::hf::HubInfo::default(),
        not_for_all_audiences: false,
    };
    let url = tree_url(&detail);
    unstated_licence(&detail, url)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use reqwest::Url;
    use serde_json::Value;

    use super::*;
    use crate::catalog::Catalog;
    use crate::gen_catalog::{
        Badge, Commercial, FamilyRecord, GateMode, GenFamily, GenKind, gen_catalog,
    };
    use crate::gguf::{TensorShape, read_tensor_shapes, testing};
    use crate::hf::{Gated, HubInfo, OFFICIAL_ENDPOINT, RepoDetail, parse_repo_detail};

    /// The Studio engine build the app pins (`runtime::sd::lock`, which this crate cannot
    /// depend on).
    fn runtime_build() -> u32 {
        let lock: Value = serde_json::from_str(include_str!("../../../sd.lock.json")).unwrap();
        u32::try_from(lock["sd_cpp"]["build"].as_u64().unwrap()).unwrap()
    }

    fn families() -> BTreeMap<GenFamily, FamilyRecord> {
        gen_catalog(&Catalog::bundled(), runtime_build()).families
    }

    /// Every repository listing in tests/fixtures/hf/: recorded on the day of pinning, or
    /// made by hand in the same shape (README.md there says which).
    const LISTINGS: &[(&str, &str)] = &[
        (
            "repo-flux2-klein-4b-gguf.json",
            include_str!("../tests/fixtures/hf/repo-flux2-klein-4b-gguf.json"),
        ),
        (
            "repo-flux3-klein-gguf.json",
            include_str!("../tests/fixtures/hf/repo-flux3-klein-gguf.json"),
        ),
        (
            "repo-qwen3-vl-4b-instruct-gguf.json",
            include_str!("../tests/fixtures/hf/repo-qwen3-vl-4b-instruct-gguf.json"),
        ),
        (
            "repo-flux2-klein-9b-gguf.json",
            include_str!("../tests/fixtures/hf/repo-flux2-klein-9b-gguf.json"),
        ),
        (
            "repo-z-image-turbo-gguf.json",
            include_str!("../tests/fixtures/hf/repo-z-image-turbo-gguf.json"),
        ),
        (
            "repo-fastwan22-ti2v-5b-gguf.json",
            include_str!("../tests/fixtures/hf/repo-fastwan22-ti2v-5b-gguf.json"),
        ),
        (
            "repo-wan22-ti2v-5b-full-gguf.json",
            include_str!("../tests/fixtures/hf/repo-wan22-ti2v-5b-full-gguf.json"),
        ),
        (
            "repo-qwen3-4b-gguf.json",
            include_str!("../tests/fixtures/hf/repo-qwen3-4b-gguf.json"),
        ),
        (
            "repo-qwen3-4b-instruct-2507-gguf.json",
            include_str!("../tests/fixtures/hf/repo-qwen3-4b-instruct-2507-gguf.json"),
        ),
        (
            "repo-comfy-flux-2-klein-4b.json",
            include_str!("../tests/fixtures/hf/repo-comfy-flux-2-klein-4b.json"),
        ),
        (
            "repo-comfy-z-image-turbo.json",
            include_str!("../tests/fixtures/hf/repo-comfy-z-image-turbo.json"),
        ),
        (
            "repo-comfy-wan-2.1.json",
            include_str!("../tests/fixtures/hf/repo-comfy-wan-2.1.json"),
        ),
        (
            "repo-umt5-xxl-encoder-gguf.json",
            include_str!("../tests/fixtures/hf/repo-umt5-xxl-encoder-gguf.json"),
        ),
        (
            "repo-klein-4b-name-9b-parameters.json",
            include_str!("../tests/fixtures/hf/repo-klein-4b-name-9b-parameters.json"),
        ),
        (
            "repo-unknown-diffusion.json",
            include_str!("../tests/fixtures/hf/repo-unknown-diffusion.json"),
        ),
        (
            "repo-uncensored.json",
            include_str!("../tests/fixtures/hf/repo-uncensored.json"),
        ),
        (
            "repo-no-hashes.json",
            include_str!("../tests/fixtures/hf/repo-no-hashes.json"),
        ),
    ];

    fn listing(name: &str) -> Value {
        let (_, text) = LISTINGS
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no listing fixture {name}"));
        serde_json::from_str(text).unwrap()
    }

    fn parse(v: &Value) -> RepoDetail {
        let id = v["id"].as_str().unwrap();
        parse_repo_detail(id, v, &Url::parse(OFFICIAL_ENDPOINT).unwrap()).unwrap()
    }

    /// The listing with the licence label `label` (its `license:` tag and the card's), and
    /// the card's `license_name` when given.
    fn relabelled_named(name: &str, label: Option<&str>, license_name: Option<&str>) -> RepoDetail {
        let mut v = listing(name);
        let tags = v["tags"].as_array_mut().unwrap();
        tags.retain(|t| !t.as_str().unwrap().starts_with("license:"));
        if let Some(label) = label {
            tags.push(format!("license:{label}").into());
        }
        let card = v["cardData"].as_object_mut().unwrap();
        card.remove("license");
        if let Some(label) = label {
            card.insert("license".into(), label.into());
        }
        if let Some(n) = license_name {
            card.insert("license_name".into(), n.into());
        }
        parse(&v)
    }

    fn relabelled(name: &str, label: Option<&str>) -> RepoDetail {
        relabelled_named(name, label, None)
    }

    /// A fixture's tensor table written as the tiny model file (no metadata keys) and read
    /// back from disk.
    fn table(fixture: &str) -> Vec<TensorShape> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        std::fs::write(&path, testing::diffusion_gguf(fixture)).unwrap();
        read_tensor_shapes(&path).unwrap()
    }

    #[test]
    fn the_recommended_list_is_free_for_any_use_and_ungated() {
        let g = gen_catalog(&Catalog::bundled(), runtime_build());
        let ids: BTreeSet<_> = g.entries.keys().map(String::as_str).collect();
        assert!(
            ids.is_superset(&BTreeSet::from(["flux2-klein-4b-q4", "z-image-turbo-q4"])),
            "{ids:?}"
        );
        let video: Vec<_> = ids
            .iter()
            .filter(|id| g.entries[**id].kind == GenKind::Video)
            .collect();
        assert!(
            matches!(video.len(), 1 | 2)
                && video
                    .iter()
                    .all(|id| id.starts_with("wan22-ti2v-5b-fast") || **id == "wan21-t2v-1.3b"),
            "{video:?}"
        );
        // Wan 2.1 only as Task L17 records it.
        assert!(matches!(ids.len(), 3 | 4));
        assert!(g.hidden.is_empty(), "{:?}", g.hidden);
        for e in g.entries.values() {
            assert_eq!(
                (
                    e.effective_licence().badge(),
                    e.effective_licence().commercial_outputs
                ),
                (Badge::Green, Commercial::Yes),
                "{}",
                e.id
            );
            assert_eq!(e.gate.mode, GateMode::None);
            assert!(
                e.speed.iter().all(|p| !p.measured_on.is_empty()),
                "{}: a speed point without where it was measured",
                e.id
            );
        }
        assert_eq!(g.families.len(), 6);
        assert_eq!(g.tiers.image[&0], ["flux2-klein-4b-q4"]);
        assert_eq!(g.tiers.image[&8], ["z-image-turbo-q4", "flux2-klein-4b-q4"]);
    }

    #[test]
    fn every_part_repo_is_its_listings_canonical_id() {
        let g = gen_catalog(&Catalog::bundled(), runtime_build());
        let parts = g
            .entries
            .values()
            .flat_map(|e| e.parts.iter())
            .chain(g.families.values().flat_map(|r| r.companions.iter()));
        let mut checked = 0;
        for p in parts {
            // The listing whose own `id` is the part's repository, spelled the same: a
            // renamed repository answers its old name with a redirect, and its files would
            // go under a second folder.
            let v = LISTINGS
                .iter()
                .map(|(_, text)| serde_json::from_str::<Value>(text).unwrap())
                .find(|v| v["id"] == p.repo.as_str())
                .unwrap_or_else(|| panic!("no recorded listing whose id is {}", p.repo));
            assert_eq!(v["sha"], p.revision.as_str(), "{} {}", p.repo, p.path);
            let d = parse(&v);
            let f = d
                .files
                .iter()
                .find(|f| f.path == p.path)
                .unwrap_or_else(|| panic!("{} has no {}", p.repo, p.path));
            assert_eq!(
                (f.size, f.sha256.as_deref()),
                (p.size, Some(p.sha256.as_str())),
                "{} {}",
                p.repo,
                p.path
            );
            checked += 1;
        }
        // Every catalog entry and family's companion must have a recorded pinned listing.
        assert!(checked >= 17, "{checked}");
    }

    /// A picture or video model's table is told from a chat model's: every recorded table of
    /// one (a 9B klein's, one missing a block) and FLUX.3 Image Klein's image embedding alone.
    /// Which model it is, the engine's detection says (`downloads::check_main_file`).
    #[test]
    fn a_picture_or_video_models_table_is_told_from_a_chat_models() {
        for fixture in testing::TENSOR_FIXTURES {
            assert_eq!(
                looks_like_diffusion(&table(fixture)),
                fixture != "qwen3-4b-tensors.txt",
                "{fixture}"
            );
        }
        let without_last_block: Vec<TensorShape> = table("wan22-ti2v-5b-tensors.txt")
            .into_iter()
            .filter(|t| !t.name.starts_with("blocks.29."))
            .collect();
        assert!(looks_like_diffusion(&without_last_block));
        let embedding: Vec<TensorShape> = table("flux3-klein-tensors.txt")
            .into_iter()
            .filter(|t| t.name.starts_with("emb_in.image."))
            .collect();
        assert!(!embedding.is_empty() && looks_like_diffusion(&embedding));
        assert!(!looks_like_diffusion(&[]));
    }

    #[test]
    fn the_licence_is_the_stricter_of_the_family_and_the_label() {
        let rec = &families()[&GenFamily::ZImageTurbo];
        let l = searched_licence(
            &relabelled("repo-z-image-turbo-gguf.json", Some("mit")),
            rec,
        );
        assert_eq!(l.licence.id, "apache-2.0");
        assert_eq!(
            l.label_mismatch.as_deref(),
            Some("The repository says MIT; Z-Image Turbo is Apache 2.0.")
        );
        let nc = searched_licence(
            &relabelled("repo-z-image-turbo-gguf.json", Some("cc-by-nc-4.0")),
            rec,
        );
        assert_eq!(nc.licence.badge(), Badge::Red);
        assert_eq!(nc.licence.commercial_outputs, Commercial::No);
        assert!(nc.label_mismatch.is_some());
        let other = searched_licence(
            &relabelled("repo-z-image-turbo-gguf.json", Some("other")),
            rec,
        );
        assert_eq!(
            (
                other.licence.commercial_outputs,
                other.licence.plain_words.as_str()
            ),
            (
                Commercial::Unclear,
                "Check the licence before you use what you make."
            )
        );
        assert!(other.label_mismatch.is_some());
        // States none: the original's.
        let none = searched_licence(&relabelled("repo-z-image-turbo-gguf.json", None), rec);
        assert_eq!(
            (none.licence.id.as_str(), none.label_mismatch),
            ("apache-2.0", None)
        );
        let same = searched_licence(
            &relabelled("repo-z-image-turbo-gguf.json", Some("Apache-2.0")),
            rec,
        );
        assert_eq!(
            (same.licence, same.label_mismatch),
            (rec.licenses["apache-2.0"].clone(), None)
        );
        // `other` naming a licence harnessUI knows is that licence.
        let named = searched_licence(
            &relabelled_named(
                "repo-z-image-turbo-gguf.json",
                Some("other"),
                Some("flux-1-dev-non-commercial-license"),
            ),
            rec,
        );
        assert_eq!(
            (named.licence.id.as_str(), named.licence.commercial_outputs),
            ("flux-1-dev-non-commercial-license", Commercial::No)
        );
        assert_eq!(
            named.label_mismatch.as_deref(),
            Some(
                "The repository says the FLUX.1 [dev] Non-Commercial License; Z-Image Turbo is Apache 2.0."
            )
        );
        assert_eq!(
            other.label_mismatch.as_deref(),
            Some("The repository has its own licence; Z-Image Turbo is Apache 2.0.")
        );
        // An unknown label is uploader text: shown made safe, and red.
        let odd = searched_licence(
            &relabelled("repo-z-image-turbo-gguf.json", Some("my\u{202E}licence")),
            rec,
        );
        assert_eq!(odd.licence.badge(), Badge::Red);
        let sentence = odd.label_mismatch.unwrap();
        assert!(!sentence.contains('\u{202E}'), "{sentence}");
        // A label's own licence is accepted per repository revision.
        let d = relabelled("repo-z-image-turbo-gguf.json", Some("other"));
        assert_eq!(
            other.licence.url,
            format!("https://huggingface.co/{}/tree/{}", d.id, d.commit)
        );
        let later = searched_licence(
            &RepoDetail {
                commit: "1".repeat(40),
                ..d
            },
            rec,
        );
        assert_ne!(later.licence.sha256, other.licence.sha256);
    }

    // ---- a generic model's licence ------------------------------------------------------

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A listing of `someone/model` at [`COMMIT`] labelled `label`, with `files`.
    fn detail_labelled(label: &str, files: &[&str]) -> RepoDetail {
        RepoDetail {
            id: "someone/model".into(),
            commit: COMMIT.into(),
            quantized_from: None,
            gated: Gated::No,
            license: Some(label.into()),
            files: files
                .iter()
                .map(|p| RepoFile {
                    path: (*p).into(),
                    size: 1_000,
                    sha256: None,
                })
                .collect(),
            gguf: None,
            template_tools: false,
            template_thinking: false,
            info: HubInfo::default(),
            not_for_all_audiences: false,
        }
    }

    #[test]
    fn an_openrail_label_reads_amber_with_use_rules_and_links_its_licence() {
        for label in [
            "creativeml-openrail-m",
            "openrail",
            "openrail++",
            "bigscience-openrail-m",
        ] {
            let l = repo_licence(&detail_labelled(
                label,
                &["LICENSE.md", "v1-5-pruned-emaonly.safetensors"],
            ));
            assert_eq!(
                (l.badge(), l.commercial_outputs, l.plain_words.as_str()),
                (
                    Badge::Amber,
                    Commercial::WithUseRules,
                    "Commercial use allowed, with use rules"
                ),
                "{label}"
            );
            assert!(
                l.url.ends_with(&format!("/blob/{COMMIT}/LICENSE.md")),
                "{label}"
            );
            assert_eq!(l.id, label);
        }
        assert_eq!(
            repo_licence(&detail_labelled("other", &[])).badge(),
            Badge::Red
        );
    }

    #[test]
    fn a_generic_models_licence_is_its_repos_label() {
        // A known label, its text in a plain LICENSE file.
        let mit = repo_licence(&detail_labelled("MIT", &["LICENSE", "model.safetensors"]));
        assert_eq!(
            (mit.id.as_str(), mit.badge(), mit.name.as_str()),
            ("mit", Badge::Green, "MIT License")
        );
        assert_eq!(
            mit.url,
            format!("https://huggingface.co/someone/model/blob/{COMMIT}/LICENSE")
        );
        // Apache 2.0 is the bundled text, whatever the repo.
        let apache = repo_licence(&detail_labelled("apache-2.0", &[]));
        assert_eq!(
            apache.sha256,
            families()[&GenFamily::ZImageTurbo].licenses["apache-2.0"].sha256
        );
        // No licence file: the repo's page at the commit. Non-commercial is red.
        let nc = repo_licence(&detail_labelled("cc-by-nc-4.0", &["model.safetensors"]));
        assert_eq!(
            (nc.badge(), nc.url.as_str()),
            (
                Badge::Red,
                format!("https://huggingface.co/someone/model/tree/{COMMIT}").as_str()
            )
        );
        // A licence file in a folder is not the repo's.
        let deep = repo_licence(&detail_labelled("mit", &["docs/LICENSE.md"]));
        assert!(deep.url.contains("/tree/"), "{}", deep.url);
        // No label, or one harnessUI does not know (shown made safe): check the licence.
        let none = RepoDetail {
            license: None,
            ..detail_labelled("x", &[])
        };
        for l in [
            repo_licence(&none),
            repo_licence(&detail_labelled("my\u{202E}licence", &[])),
        ] {
            assert_eq!(
                (l.badge(), l.plain_words.as_str()),
                (
                    Badge::Red,
                    "Check the licence before you use what you make."
                )
            );
            assert!(!l.id.contains('\u{202E}'), "{}", l.id);
        }
        // `other` naming a licence harnessUI knows is that licence.
        let named = RepoDetail {
            info: HubInfo {
                license: Some("flux-1-dev-non-commercial-license".into()),
                ..HubInfo::default()
            },
            ..detail_labelled("other", &[])
        };
        assert_eq!(repo_licence(&named).commercial_outputs, Commercial::No);
        // A repo whose listing could not be read: check the licence, linked to its page.
        let unread = unread_licence("someone/other", COMMIT);
        assert_eq!(unread.badge(), Badge::Red);
        assert_eq!(
            unread.url,
            format!("https://huggingface.co/someone/other/tree/{COMMIT}")
        );
    }

    // ---- search verdicts from names (0.2.0) ----------------------------------------------

    fn works(version: SdVersion, kind: RecipeKind) -> Verdict {
        Verdict::Works { version, kind }
    }

    #[test]
    fn search_hints_from_names() {
        assert_eq!(
            verdict_hint("nerijs/pixel-art-xl-lora", Some("text-to-image")),
            Verdict::Lora {
                base: Some(SdVersion::Sdxl)
            }
        );
        assert_eq!(
            verdict_hint("Alpha-VLLM/Lumina-Image-2.0", Some("text-to-image")),
            Verdict::CannotRun {
                reason: CannotRunReason::Lumina2,
                version: None
            }
        );
        assert_eq!(
            verdict_hint("QuantStack/Wan2.2-T2V-A14B-GGUF", Some("text-to-video")),
            works(SdVersion::Wan2, RecipeKind::Video)
        );
    }

    /// R7's words keep a `.` only between digits, so `flux.1` is two words there: the FLUX
    /// names are read in the whole name.
    #[test]
    fn flux_names_with_a_dot_are_read() {
        assert_eq!(
            verdict_hint("black-forest-labs/FLUX.1-dev", Some("text-to-image")),
            works(SdVersion::Flux, RecipeKind::Picture)
        );
        assert_eq!(
            verdict_hint("black-forest-labs/FLUX.2-klein-4B", Some("text-to-image")),
            works(SdVersion::Flux2Klein, RecipeKind::Picture)
        );
        assert_eq!(
            verdict_hint("unsloth/FLUX.2-klein-base-9B-GGUF", None),
            works(SdVersion::Flux2Klein, RecipeKind::Picture)
        );
        assert_eq!(
            verdict_hint("black-forest-labs/FLUX.2-dev", Some("text-to-image")),
            works(SdVersion::Flux2, RecipeKind::Picture)
        );
        assert_eq!(
            verdict_hint(
                "webmp3/Sakura-FLUX.3-Image-Klein-GGUF",
                Some("text-to-image")
            ),
            works(SdVersion::Flux3, RecipeKind::Picture)
        );
        assert_eq!(
            verdict_hint("city96/FLUX.1-schnell-gguf", None),
            works(SdVersion::Flux, RecipeKind::Picture)
        );
    }

    #[test]
    fn search_hints_take_the_first_row_that_matches() {
        let hint = |name| verdict_hint(name, None);
        // `lora` first, then the reasons Studio can't run a repo, then the versions.
        assert_eq!(
            hint("XLabs-AI/flux-lora-collection"),
            Verdict::Lora { base: None }
        );
        assert_eq!(
            hint("Kijai/Wan2.2_lightx2v_lora"),
            Verdict::Lora {
                base: Some(SdVersion::Wan2)
            }
        );
        for (name, reason) in [
            ("neta-art/Neta-Lumina", CannotRunReason::Lumina2),
            ("Lightricks/LTX-Video", CannotRunReason::LtxVideo09),
            ("Lightricks/ltxv-13b-0.9.7-dev", CannotRunReason::LtxVideo09),
            (
                "HiDream-ai/HiDream-I1-Full",
                CannotRunReason::UnknownWeights,
            ),
            (
                "stabilityai/stable-video-diffusion-img2vid-xt",
                CannotRunReason::Svd,
            ),
            ("second-state/svd-xt-GGUF", CannotRunReason::Svd),
        ] {
            assert_eq!(
                hint(name),
                Verdict::CannotRun {
                    reason,
                    version: None
                },
                "{name}"
            );
        }
        for (name, version) in [
            ("stabilityai/sdxl-turbo", SdVersion::Sdxl),
            ("runwayml/stable-diffusion-v1-5", SdVersion::Sd1),
            ("someone/sd15-anime", SdVersion::Sd1),
            ("someone/my_sd1.5_merge", SdVersion::Sd1),
            ("someone/sd2-768-merge", SdVersion::Sd2),
            ("someone/stable-diffusion-v2-1-base", SdVersion::Sd2),
            ("someone/sd3-medium-merge", SdVersion::Sd3),
            ("someone/sd3.5_large_turbo", SdVersion::Sd3),
            ("black-forest-labs/FLUX.1-Kontext-dev", SdVersion::Flux),
            ("lodestones/Chroma", SdVersion::Chroma),
            ("Qwen/Qwen-Image", SdVersion::QwenImage),
            ("Tongyi-MAI/Z-Image-Turbo", SdVersion::ZImage),
            ("Wan-AI/Wan2.1-T2V-1.3B", SdVersion::Wan2),
            ("Wan-AI/Wan2.2-TI2V-5B", SdVersion::Wan22Ti2v),
            ("tencent/HunyuanVideo-1.5", SdVersion::HunyuanVideo),
            ("Lightricks/LTX-2.3", SdVersion::Ltxav),
            ("AIDC-AI/Ovis-Image-7B", SdVersion::OvisImage),
            ("meituan-longcat/LongCat-Image", SdVersion::Longcat),
            ("PixArt-alpha/PixArt-Sigma-XL-2-1024-MS", SdVersion::Pixart),
            ("circlestone-labs/Anima", SdVersion::Anima),
        ] {
            assert!(
                matches!(hint(name), Verdict::Works { version: v, .. } if v == version),
                "{name}: {:?}",
                hint(name)
            );
        }
        // Chat models and words inside other words say nothing; nor does `xl` outside a
        // LoRA's name.
        for name in [
            "Qwen/Qwen3-8B",
            "unsloth/gemma-3-12b-it-GGUF",
            "someone/flora-wanderer",
            "someone/swan-song",
            "google/flan-t5-xl",
            "Rostlab/prot_t5_xl_uniref50",
            "stabilityai/stable-diffusion-xl-base-1.0",
        ] {
            assert_eq!(hint(name), Verdict::Unknown, "{name}");
        }
    }

    #[test]
    fn the_kind_comes_from_the_tag_and_else_from_the_name() {
        assert_eq!(
            verdict_hint(
                "black-forest-labs/FLUX.1-Kontext-dev",
                Some("image-to-image")
            ),
            works(SdVersion::Flux, RecipeKind::PictureEdit)
        );
        assert_eq!(
            verdict_hint("black-forest-labs/FLUX.1-Kontext-dev", None),
            works(SdVersion::Flux, RecipeKind::PictureEdit)
        );
        assert_eq!(
            verdict_hint("Qwen/Qwen-Image-Edit-2509", None),
            works(SdVersion::QwenImage, RecipeKind::PictureEdit)
        );
        assert_eq!(
            verdict_hint("Wan-AI/Wan2.2-TI2V-5B", Some("image-to-video")),
            works(SdVersion::Wan22Ti2v, RecipeKind::Video)
        );
        assert_eq!(
            verdict_hint("Wan-AI/Wan2.1-T2V-1.3B", None),
            works(SdVersion::Wan2, RecipeKind::Video)
        );
        // A name the matching table refuses (VACE) is still a video model's.
        assert_eq!(
            verdict_hint("Wan-AI/Wan2.1-VACE-14B", None),
            works(SdVersion::Wan2, RecipeKind::Video)
        );
    }

    /// A pipeline tag of another kind of model says what the repo is, whatever its name.
    #[test]
    fn a_tag_that_is_no_picture_or_video_models_says_nothing() {
        for (name, tag) in [
            ("meituan-longcat/LongCat-Flash-Chat", "text-generation"),
            ("meituan-longcat/LongCat-Flash-Thinking", "text-generation"),
            ("meituan-longcat/LongCat-2.5", "image-text-to-text"),
            ("someone/llama-3-8b-lora", "text-generation"),
            ("o/my-sdxl-merge", "other"),
        ] {
            assert_eq!(verdict_hint(name, Some(tag)), Verdict::Unknown, "{name}");
        }
        assert_eq!(
            verdict_hint("meituan-longcat/LongCat-Image", Some("text-to-image")),
            works(SdVersion::Longcat, RecipeKind::Picture)
        );
        // `xl` is SDXL in a LoRA's name, whatever its tag.
        assert_eq!(
            verdict_hint("nerijs/pixel-art-xl-lora", Some("text-to-image")),
            Verdict::Lora {
                base: Some(SdVersion::Sdxl)
            }
        );
        assert_eq!(
            verdict_hint("nerijs/pixel-art-xl-lora", None),
            Verdict::Lora {
                base: Some(SdVersion::Sdxl)
            }
        );
        assert_eq!(verdict_hint("google/flan-t5-xl", None), Verdict::Unknown);
    }

    /// Ruling S5-xl-3: beside a picture model's pipeline tag, `xl` is SDXL; untagged, or
    /// beside another tag, it says nothing.
    #[test]
    fn xl_beside_a_picture_tag_is_sdxl() {
        for name in [
            "stabilityai/stable-diffusion-xl-base-1.0",
            "cagliostrolab/animagine-xl-3.1",
        ] {
            assert_eq!(
                verdict_hint(name, Some("text-to-image")),
                works(SdVersion::Sdxl, RecipeKind::Picture),
                "{name}"
            );
        }
        assert_eq!(
            verdict_hint("someone/my-xl-edit", Some("image-to-image")),
            works(SdVersion::Sdxl, RecipeKind::PictureEdit)
        );
        assert_eq!(
            verdict_hint("stabilityai/stable-diffusion-xl-base-1.0", None),
            Verdict::Unknown
        );
        assert_eq!(verdict_hint("google/flan-t5-xl", None), Verdict::Unknown);
        assert_eq!(
            verdict_hint("google/flan-t5-xl", Some("text2text-generation")),
            Verdict::Unknown
        );
        assert_eq!(
            verdict_hint("someone/wan-xl", Some("text-to-video")),
            works(SdVersion::Wan2, RecipeKind::Video),
            "a version word wins"
        );
        assert_eq!(
            verdict_hint("someone/xl-clip", Some("text-to-video")),
            Verdict::Unknown
        );
        assert_eq!(
            verdict_hint("nerijs/pixel-art-xl-lora", Some("text-to-image")),
            Verdict::Lora {
                base: Some(SdVersion::Sdxl)
            }
        );
    }
}
