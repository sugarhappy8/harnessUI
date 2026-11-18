//! Picture and video models found on Hugging Face (studio-local-design.md, "Get local
//! models"): which family a repository or a local file is, what a searched file needs
//! beside it, and the licence it is under.
//!
//! The recognition tables are compiled in: a family decides engine flags and quirks, so
//! the names and tensor tables that say which family something is are code. Everything
//! else about a family (its companions, figures and licence) is catalog data
//! ([`FamilyRecord`]).
//!
//! A name never proves what a file holds: [`recognise`] decides what a repository offers
//! from its listing, and the downloaded file's own tensor table ([`family_from_tensors`])
//! decides whether it is added and run.

use std::collections::{BTreeMap, BTreeSet};
use std::iter;

use crate::arch::is_diffusion;
use crate::files::{RepoFile, quant_label};
use crate::gen_catalog::{
    BUNDLED_LICENCE_TEXTS, Commercial, FamilyRecord, GateMode, GenEntry, GenFamily, GenGate,
    GenLicence, GenPart, GenRole, check_licences, check_parts, sha256_hex, strictness,
};
use crate::gguf::TensorShape;
use crate::hf::{Gated, GenSearchKind, OFFICIAL_ENDPOINT, RepoDetail, uploader_text};

/// Why a repository is never offered, whatever it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotOffered {
    /// Its licence excludes some regions (HunyuanVideo, HunyuanImage, MiniMax-H3).
    RegionLimited,
    /// Made without safety limits ("uncensored", "abliterated").
    Uncensored,
    /// Tagged not for all audiences.
    AdultContent,
}

/// What a repository's listing offers in Studio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recognition {
    /// A family harnessUI runs, with the repository's diffusion files that have a
    /// published SHA-256.
    Supported {
        family: GenFamily,
        files: Vec<RepoFile>,
    },
    /// Shown as "Not offered in harnessUI".
    NotOffered(NotOffered),
    /// Shown as "Not supported yet" with this sentence, and never downloaded.
    NotSupportedYet(String),
}

/// What a name says about its family ([`family_of_name`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameMatch {
    /// One family's name.
    Family(GenFamily),
    /// Wan 2.2 TI2V-5B, but not the fast version harnessUI runs.
    UncheckedWan22,
    /// More than one family's name.
    Several,
    /// No family's name.
    None,
}

/// Each family's names, compared after [`normalise`]. A family's token wins over
/// [`UNCHECKED_WAN22_TOKENS`], which is part of the fast Wan's.
const FAMILY_TOKENS: &[(GenFamily, &[&str])] = &[
    (
        GenFamily::Flux2Klein4b,
        &["flux.2-klein-4b", "flux2-klein-4b", "flux-2-klein-4b"],
    ),
    (GenFamily::ZImageTurbo, &["z-image-turbo"]),
    // FastVideo's distillation, the family's upstream (Apache-2.0).
    (GenFamily::Wan22Ti2v5bFast, &["fastwan2.2-ti2v-5b"]),
    (GenFamily::Wan21T2v1_3b, &["wan2.1-t2v-1.3b"]),
];

/// The full Wan 2.2 5B (about 30–50 steps, cfg 5) and other distillations harnessUI has not
/// checked (a "turbo" whose base states no licence). Read only when no family token
/// matches.
const UNCHECKED_WAN22_TOKENS: &[&str] = &["wan2.2-ti2v-5b"];

/// Models whose licence excludes some regions (outputs included).
const REGION_LIMITED_TOKENS: &[&str] = &[
    "hunyuanvideo",
    "hunyuan-video",
    "hunyuanimage",
    "minimax-h3",
];

/// Models made without safety limits.
const UNCENSORED_TOKENS: &[&str] = &["uncensored", "abliterated"];

/// File names that mark an encoder, a decoder or a projector, never the diffusion model
/// (`text_encoder` reads `text-encoder` after [`normalise`]).
const NOT_DIFFUSION_MARKERS: &[&str] = &["vae", "text-encoder", "umt5", "qwen3", "mmproj"];

const UNKNOWN_KIND: &str = "harnessUI does not know this kind of model yet.";
const UNCHECKED_WAN22: &str = "harnessUI runs FastVideo's fast version of Wan 2.2 5B; this one is the full model or a version it has not checked.";
const SEVERAL_KINDS: &str = "This repository holds several kinds of model.";
const OTHER_SIZE: &str = "This file is a different size of model than its name says.";
const NO_CHECKSUM: &str = "No file here has a published checksum.";

/// What sort of search finds a family's repositories.
pub fn search_kind(family: GenFamily) -> GenSearchKind {
    match family {
        GenFamily::Flux2Klein4b | GenFamily::ZImageTurbo => GenSearchKind::Pictures,
        GenFamily::Wan22Ti2v5bFast | GenFamily::Wan21T2v1_3b => GenSearchKind::Video,
    }
}

/// Lower case, with `_` and spaces read as `-`.
fn normalise(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '_' | ' ' => '-',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// The family `name` (a repository id or a file name) names, by the family tokens only:
/// what the untagged search keeps and what [`recognise`] starts from.
pub fn family_of_name(name: &str) -> NameMatch {
    family_of_names(iter::once(name))
}

/// [`family_of_name`] over several names together: one family between them all, several,
/// or (with none) the unchecked Wan 2.2.
fn family_of_names<'a>(names: impl Iterator<Item = &'a str>) -> NameMatch {
    let mut found = BTreeSet::new();
    let mut unchecked = false;
    for name in names {
        let name = normalise(name);
        for (family, tokens) in FAMILY_TOKENS {
            if tokens.iter().any(|t| name.contains(t)) {
                found.insert(*family);
            }
        }
        unchecked |= UNCHECKED_WAN22_TOKENS.iter().any(|t| name.contains(t));
    }
    let mut found = found.into_iter();
    match (found.next(), found.next()) {
        (Some(family), None) => NameMatch::Family(family),
        (Some(_), Some(_)) => NameMatch::Several,
        (None, _) if unchecked => NameMatch::UncheckedWan22,
        (None, _) => NameMatch::None,
    }
}

/// A `.gguf` file whose name does not mark it as an encoder, a decoder or a projector.
fn is_diffusion_candidate(path: &str) -> bool {
    let name = normalise(path);
    name.ends_with(".gguf") && !NOT_DIFFUSION_MARKERS.iter().any(|m| name.contains(m))
}

fn not_yet(why: &str) -> Recognition {
    Recognition::NotSupportedYet(why.to_string())
}

/// What a repository offers, from its listing and the catalog's family records, in this
/// order: not for all audiences, a region-limited model or an uncensored one is not
/// offered; then it must name exactly one family harnessUI has a record of, of the size
/// the record states (when the listing states one) and an architecture a picture or video
/// model has (when it states one); then its diffusion files are the `.gguf` files that are
/// not encoders, decoders or projectors, whose own name names no other model, and that
/// have a published SHA-256.
///
/// A name cannot prove a file's family: the downloaded file's tensor table is read before
/// it is added and before every run.
pub fn recognise(detail: &RepoDetail, families: &BTreeMap<GenFamily, FamilyRecord>) -> Recognition {
    if detail.not_for_all_audiences {
        return Recognition::NotOffered(NotOffered::AdultContent);
    }
    let names: Vec<String> = iter::once(detail.id.as_str())
        .chain(detail.files.iter().map(|f| f.path.as_str()))
        .map(normalise)
        .collect();
    let named = |tokens: &[&str]| names.iter().any(|n| tokens.iter().any(|t| n.contains(t)));
    if named(REGION_LIMITED_TOKENS) {
        return Recognition::NotOffered(NotOffered::RegionLimited);
    }
    if named(UNCENSORED_TOKENS)
        || detail
            .info
            .tags
            .iter()
            .any(|t| UNCENSORED_TOKENS.contains(&t.as_str()))
    {
        return Recognition::NotOffered(NotOffered::Uncensored);
    }
    let candidates: Vec<&RepoFile> = detail
        .files
        .iter()
        .filter(|f| is_diffusion_candidate(&f.path))
        .collect();
    let names = iter::once(detail.id.as_str()).chain(candidates.iter().map(|f| f.path.as_str()));
    let family = match family_of_names(names) {
        NameMatch::Family(family) => family,
        NameMatch::UncheckedWan22 => return not_yet(UNCHECKED_WAN22),
        NameMatch::Several => return not_yet(SEVERAL_KINDS),
        NameMatch::None => return not_yet(UNKNOWN_KIND),
    };
    let Some(record) = families.get(&family) else {
        return not_yet(UNKNOWN_KIND);
    };
    if let Some(gguf) = &detail.gguf {
        let (low, high) = record.parameters;
        if gguf.parameters.is_some_and(|p| !(low..=high).contains(&p)) {
            return not_yet(OTHER_SIZE);
        }
        if gguf
            .architecture
            .as_deref()
            .is_some_and(|a| !is_diffusion(a))
        {
            return not_yet(UNKNOWN_KIND);
        }
    }
    // One family token decides for the id and every file name together, so a file whose own
    // name says it is another model (the full Wan 2.2 beside FastWan) is not offered under
    // this family; nor, when the other files name the family, one that names none (a 9B
    // klein beside the 4B).
    let own: Vec<(&RepoFile, NameMatch)> = candidates
        .into_iter()
        .map(|f| (f, family_of_name(&f.path)))
        .collect();
    let files_name_it = own.iter().any(|(_, m)| *m == NameMatch::Family(family));
    let (named, other): (Vec<_>, Vec<_>) = own.into_iter().partition(|(_, m)| match m {
        NameMatch::Family(f) => *f == family,
        NameMatch::None => !files_name_it,
        NameMatch::UncheckedWan22 | NameMatch::Several => false,
    });
    if named.is_empty() && !other.is_empty() {
        return not_yet(
            match family_of_names(other.iter().map(|(f, _)| f.path.as_str())) {
                NameMatch::UncheckedWan22 => UNCHECKED_WAN22,
                NameMatch::Family(_) | NameMatch::Several => SEVERAL_KINDS,
                NameMatch::None => UNKNOWN_KIND,
            },
        );
    }
    let files: Vec<RepoFile> = named
        .into_iter()
        .filter(|(f, _)| f.sha256.is_some())
        .map(|(f, _)| f.clone())
        .collect();
    if files.is_empty() {
        return not_yet(NO_CHECKSUM);
    }
    Recognition::Supported { family, files }
}

// ---- tensor tables ------------------------------------------------------------------------

/// What a family's diffusion file holds, from the pinned files' tensor tables
/// (tests/fixtures/gguf/).
struct TensorSignature {
    family: GenFamily,
    /// Name prefixes at least one tensor has.
    required: &'static [&'static str],
    /// The tensor the width is read from, the index of the dimension (ggml's order) and the
    /// width.
    width: (&'static str, usize, u64),
    /// Each block prefix and how many numbered blocks (`<prefix><n>.…`) it has.
    blocks: &'static [(&'static str, usize)],
    /// A patch embedding's input channels: the tensor, its kernel's volume and the
    /// channels (its elements are width × channels × volume, however a converter stacked
    /// the dimensions).
    channels: Option<(&'static str, u64, u64)>,
}

const SIGNATURES: &[TensorSignature] = &[
    TensorSignature {
        family: GenFamily::Flux2Klein4b,
        required: &["double_blocks.", "single_blocks."],
        width: ("img_in.weight", 1, 3072),
        blocks: &[("double_blocks.", 5), ("single_blocks.", 20)],
        channels: None,
    },
    TensorSignature {
        family: GenFamily::ZImageTurbo,
        required: &["cap_embedder."],
        width: ("cap_embedder.1.weight", 1, 3840),
        blocks: &[
            ("layers.", 30),
            ("noise_refiner.", 2),
            ("context_refiner.", 2),
        ],
        channels: None,
    },
    // The fast and full versions share it: the name decides between them.
    TensorSignature {
        family: GenFamily::Wan22Ti2v5bFast,
        required: &["blocks.", "patch_embedding."],
        width: ("patch_embedding.bias", 0, 3072),
        blocks: &[("blocks.", 30)],
        channels: Some(("patch_embedding.weight", 4, 48)),
    },
    TensorSignature {
        family: GenFamily::Wan21T2v1_3b,
        required: &["blocks.", "patch_embedding."],
        width: ("patch_embedding.bias", 0, 1536),
        blocks: &[("blocks.", 30)],
        channels: Some(("patch_embedding.weight", 4, 16)),
    },
];

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
];

/// A tensor's name without a converter's prefix.
fn bare(name: &str) -> &str {
    CONVERTER_PREFIXES
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name)
}

/// How many distinct `<n>` there are in names `<prefix><n>.…`.
fn block_count(shapes: &[TensorShape], prefix: &str) -> usize {
    shapes
        .iter()
        .filter_map(|t| {
            let rest = bare(&t.name).strip_prefix(prefix)?;
            rest.split('.').next()?.parse::<u64>().ok()
        })
        .collect::<BTreeSet<_>>()
        .len()
}

fn has_prefixes(shapes: &[TensorShape], prefixes: &[&str]) -> bool {
    prefixes
        .iter()
        .all(|p| shapes.iter().any(|t| bare(&t.name).starts_with(p)))
}

fn dims_of<'a>(shapes: &'a [TensorShape], name: &str) -> Option<&'a [u64]> {
    shapes
        .iter()
        .find(|t| bare(&t.name) == name)
        .map(|t| t.dims.as_slice())
}

fn matches_signature(sig: &TensorSignature, shapes: &[TensorShape]) -> bool {
    let (tensor, index, width) = sig.width;
    has_prefixes(shapes, sig.required)
        && dims_of(shapes, tensor).and_then(|d| d.get(index)) == Some(&width)
        && sig
            .blocks
            .iter()
            .all(|(prefix, count)| block_count(shapes, prefix) == *count)
        && sig.channels.is_none_or(|(tensor, volume, channels)| {
            let elements = dims_of(shapes, tensor)
                .and_then(|d| d.iter().try_fold(1u64, |n, &x| n.checked_mul(x)));
            elements == Some(width * channels * volume)
        })
}

/// The family of a local GGUF diffusion file, from its tensor table (most such files carry
/// no `general.architecture`): the names its signature needs, the width one tensor's
/// dimensions give, the block counts and, for Wan, the patch embedding's input channels.
/// Anything else, a 9B klein named like the 4B included, is `None`.
pub fn family_from_tensors(shapes: &[TensorShape]) -> Option<GenFamily> {
    SIGNATURES
        .iter()
        .find(|sig| matches_signature(sig, shapes))
        .map(|sig| sig.family)
}

/// Whether a tensor table is a picture or video model's: a known family, the names a known
/// signature needs (another size of it), or names only such models use.
pub fn looks_like_diffusion(shapes: &[TensorShape]) -> bool {
    family_from_tensors(shapes).is_some()
        || SIGNATURES
            .iter()
            .any(|sig| has_prefixes(shapes, sig.required))
        || shapes.iter().any(|t| {
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

fn known_label(id: &str) -> Option<&'static KnownLabel> {
    KNOWN_LABELS.iter().find(|k| k.id.eq_ignore_ascii_case(id))
}

/// A licence the repository's label names, accepted per repository revision: its SHA-256
/// is the bundled canonical text's when there is one, else that of
/// `<repo>@<commit>:<label>`, and its link is the repository at that commit.
fn label_licence(
    detail: &RepoDetail,
    id: &str,
    name: &str,
    commercial: Commercial,
    plain_words: &str,
) -> GenLicence {
    let sha256 = BUNDLED_LICENCE_TEXTS
        .iter()
        .find(|(bundled, _)| bundled.eq_ignore_ascii_case(id))
        .map(|(_, text)| sha256_hex(text.as_bytes()))
        .unwrap_or_else(|| sha256_hex(format!("{}@{}:{id}", detail.id, detail.commit).as_bytes()));
    GenLicence {
        id: id.to_string(),
        name: name.to_string(),
        url: format!("{OFFICIAL_ENDPOINT}/{}/tree/{}", detail.id, detail.commit),
        sha256,
        commercial_outputs: commercial,
        revenue_limit_usd: None,
        excluded_regions: Vec::new(),
        obligations: Vec::new(),
        plain_words: plain_words.to_string(),
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
        );
        return SearchedLicence {
            licence,
            label_mismatch: None,
        };
    };
    let label = detail
        .license
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_ascii_lowercase);
    // A licence Hugging Face has no id for is labelled `other` with its own name, which
    // HubInfo keeps (made safe): FLUX.1 [dev]'s is `flux-1-dev-non-commercial-license`.
    let own_name = detail
        .info
        .license
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("other"))
        .map(str::to_ascii_lowercase);
    let label = match label.as_deref() {
        Some("other") => own_name.or(label),
        _ => label,
    };
    let Some(label) = label.filter(|l| *l != upstream.id.to_ascii_lowercase()) else {
        return SearchedLicence {
            licence: upstream.clone(),
            label_mismatch: None,
        };
    };
    let upstream_words = known_label(&upstream.id).map_or(upstream.name.as_str(), |k| k.words);
    let family = &record.name;
    if let Some(known) = known_label(&label) {
        let plain = match known.commercial {
            Commercial::No => "For non-commercial use only.",
            _ => "Free for any use, including commercial.",
        };
        let theirs = label_licence(detail, known.id, known.name, known.commercial, plain);
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
        licence: label_licence(detail, &id, &name, Commercial::Unclear, CHECK_THE_LICENCE),
        label_mismatch: Some(sentence),
    }
}

// ---- the entry a searched file makes ------------------------------------------------------

/// The entry a searched file makes: its diffusion part (this repository, this commit, the
/// file's published SHA-256, under [`searched_licence`]) plus the record's companions,
/// defaults, limits, fallback and speed. Its id is `hf:<owner>/<repo>:<file>`. Refused
/// for a file that is not one of the listing's diffusion files with a published SHA-256,
/// or a part that is not pinned.
pub fn entry_for(
    detail: &RepoDetail,
    file: &RepoFile,
    record: &FamilyRecord,
) -> Result<GenEntry, String> {
    if !detail.files.contains(file) || !is_diffusion_candidate(&file.path) {
        return Err("not one of the repository's model files".into());
    }
    let sha256 = file.sha256.clone().ok_or_else(|| NO_CHECKSUM.to_string())?;
    let SearchedLicence { licence, .. } = searched_licence(detail, record);
    let diffusion = GenPart {
        role: GenRole::Diffusion,
        repo: detail.id.clone(),
        revision: detail.commit.clone(),
        path: file.path.clone(),
        size: file.size,
        sha256,
        license: licence.id.clone(),
    };
    let mut licenses = record.licenses.clone();
    licenses.insert(licence.id.clone(), licence);
    let parts: Vec<GenPart> = iter::once(diffusion)
        .chain(record.companions.iter().cloned())
        .collect();
    check_parts(&parts, &licenses)?;
    check_licences(&licenses)?;
    let name = match quant_label(&file.path) {
        Some(quant) => format!("{} ({quant})", record.name),
        None => record.name.clone(),
    };
    Ok(GenEntry {
        id: format!("hf:{}:{}", detail.id, file.path),
        name,
        summary: String::new(),
        publisher: record.publisher.clone(),
        kind: record.kind,
        family: record.family,
        inputs: record.inputs.clone(),
        parts,
        licenses,
        gate: GenGate {
            mode: match detail.gated {
                Gated::No => GateMode::None,
                Gated::Auto => GateMode::Auto,
                Gated::Manual => GateMode::Manual,
            },
            collects: Vec::new(),
        },
        defaults: record.defaults.clone(),
        limits: record.limits.clone(),
        fallback: record.fallback.clone(),
        memory: record.memory.clone(),
        speed: record.speed.clone(),
        quirks: record.quirks.clone(),
        min_build: record.min_build,
        runs_on_apple: record.runs_on_apple,
        tested: None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use reqwest::Url;
    use serde_json::Value;

    use super::*;
    use crate::catalog::Catalog;
    use crate::gen_catalog::{
        Badge, Commercial, FamilyRecord, GateMode, GenFamily, GenKind, GenRole, gen_catalog,
    };
    use crate::gguf::{TensorShape, read_tensor_shapes, testing};
    use crate::hf::{OFFICIAL_ENDPOINT, RepoDetail, parse_repo_detail};

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

    fn detail(name: &str) -> RepoDetail {
        parse(&listing(name))
    }

    /// The listing under another repository id.
    fn renamed(name: &str, id: &str) -> RepoDetail {
        let mut v = listing(name);
        v["id"] = id.into();
        parse(&v)
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

    fn supported_files(name: &str) -> Vec<crate::files::RepoFile> {
        match recognise(&detail(name), &families()) {
            Recognition::Supported { files, .. } => files,
            other => panic!("{name}: {other:?}"),
        }
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
        assert_eq!(g.families.len(), 4);
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
        // Three entries of three parts, four records of two companions.
        assert!(checked >= 17, "{checked}");
    }

    #[test]
    fn a_recognised_repo_lists_its_ggufs_that_have_a_published_hash() {
        let Recognition::Supported { family, files } =
            recognise(&detail("repo-flux2-klein-4b-gguf.json"), &families())
        else {
            panic!()
        };
        assert_eq!(family, GenFamily::Flux2Klein4b);
        assert!(
            !files.is_empty()
                && files
                    .iter()
                    .all(|f| f.path.ends_with(".gguf") && f.sha256.is_some())
        );
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["flux-2-klein-4b-Q4_0.gguf", "flux-2-klein-4b-Q8_0.gguf"]
        );
        assert_eq!(
            supported_files("repo-z-image-turbo-gguf.json").len(),
            7,
            "the GGUFs, not the sample pictures"
        );
    }

    #[test]
    fn encoder_decoder_and_projector_files_are_not_offered() {
        let mut v = listing("repo-z-image-turbo-gguf.json");
        let first = v["siblings"][10].clone();
        assert_eq!(first["rfilename"], "z_image_turbo-Q2_K.gguf");
        for name in [
            "Qwen3-4B-Q4_K_M.gguf",
            "vae/ae.gguf",
            "text_encoder/model.gguf",
            "umt5-xxl-Q8_0.gguf",
            "mmproj-F16.gguf",
        ] {
            let mut extra = first.clone();
            extra["rfilename"] = name.into();
            v["siblings"].as_array_mut().unwrap().push(extra);
        }
        let Recognition::Supported { files, .. } = recognise(&parse(&v), &families()) else {
            panic!()
        };
        assert_eq!(files.len(), 7);
        assert!(files.iter().all(|f| f.path.starts_with("z_image_turbo-")));
    }

    #[test]
    fn a_searched_file_gets_the_familys_companions() {
        let d = detail("repo-z-image-turbo-gguf.json");
        let rec = &families()[&GenFamily::ZImageTurbo];
        let file = &supported_files("repo-z-image-turbo-gguf.json")[0];
        let e = entry_for(&d, file, rec).unwrap();
        assert_eq!(e.id, format!("hf:{}:{}", d.id, file.path));
        assert_eq!(
            e.parts.iter().map(|p| p.role).collect::<Vec<_>>(),
            [GenRole::Diffusion, GenRole::Llm, GenRole::Vae]
        );
        assert_eq!(
            (e.parts[0].revision.as_str(), e.total_bytes()),
            (
                d.commit.as_str(),
                file.size + rec.companions.iter().map(|p| p.size).sum::<u64>()
            )
        );
        assert_eq!(e.parts[1..], rec.companions[..]);
        assert_eq!(e.parts[0].sha256, file.sha256.clone().unwrap());
        assert_eq!(
            (e.family, e.kind, e.defaults.steps, e.quirks.clone()),
            (rec.family, rec.kind, rec.defaults.steps, rec.quirks.clone())
        );
        assert_eq!(e.name, "Z-Image Turbo (Q2_K)");
        assert_eq!(e.effective_licence().id, "apache-2.0");
        // Not a pinned file of the listing: no entry.
        let mut other = file.clone();
        other.sha256 = Some("0".repeat(64));
        assert!(entry_for(&d, &other, rec).is_err());
        let mut unhashed = file.clone();
        unhashed.sha256 = None;
        assert!(entry_for(&d, &unhashed, rec).is_err());
        let readme = d.files.iter().find(|f| f.path == "README.md").unwrap();
        assert!(entry_for(&d, readme, rec).is_err());
    }

    /// The licence the repository's label makes stricter is on the diffusion part, so it
    /// decides the badge and is shown and accepted with the others.
    #[test]
    fn a_searched_entry_carries_the_stricter_label() {
        let d = relabelled("repo-z-image-turbo-gguf.json", Some("cc-by-nc-4.0"));
        let rec = &families()[&GenFamily::ZImageTurbo];
        let file = &supported_files("repo-z-image-turbo-gguf.json")[0];
        let e = entry_for(&d, file, rec).unwrap();
        assert_eq!(e.parts[0].license, "cc-by-nc-4.0");
        assert_eq!(e.effective_licence().badge(), Badge::Red);
        assert_eq!(
            e.licenses.keys().map(String::as_str).collect::<Vec<_>>(),
            ["apache-2.0", "cc-by-nc-4.0"]
        );
        let unclear = entry_for(
            &relabelled("repo-z-image-turbo-gguf.json", Some("other")),
            file,
            rec,
        )
        .unwrap();
        let l = &unclear.licenses[&unclear.parts[0].license];
        assert_eq!(l.commercial_outputs, Commercial::Unclear);
        // Accepted per repository revision.
        assert_eq!(
            l.url,
            format!("https://huggingface.co/{}/tree/{}", d.id, d.commit)
        );
        assert_eq!(l.sha256.len(), 64);
        assert_ne!(
            l.sha256,
            entry_for(
                &RepoDetail {
                    commit: "1".repeat(40),
                    ..relabelled("repo-z-image-turbo-gguf.json", Some("other"))
                },
                file,
                rec
            )
            .map(|e| e.licenses[&e.parts[0].license].sha256.clone())
            .unwrap()
        );
    }

    #[test]
    fn a_9b_klein_is_not_supported_yet() {
        assert!(matches!(
            recognise(&detail("repo-flux2-klein-9b-gguf.json"), &families()),
            Recognition::NotSupportedYet(_)
        ));
        // The listing's gguf.parameters is outside the 4B band.
        assert!(matches!(
            recognise(&detail("repo-klein-4b-name-9b-parameters.json"), &families()),
            Recognition::NotSupportedYet(r) if r.contains("different size")
        ));
    }

    #[test]
    fn the_full_wan_2_2_is_not_the_fast_one() {
        assert!(matches!(
            recognise(&detail("repo-wan22-ti2v-5b-full-gguf.json"), &families()),
            Recognition::NotSupportedYet(r) if r.contains("fast version")
        ));
        assert!(matches!(
            recognise(&detail("repo-fastwan22-ti2v-5b-gguf.json"), &families()),
            Recognition::Supported {
                family: GenFamily::Wan22Ti2v5bFast,
                ..
            }
        ));
    }

    /// The listing `name` with its `.gguf` files replaced by `files`, each taken from the
    /// recorded listing it is in (`(listing, file)`), kept as they are otherwise.
    fn with_ggufs(name: &str, files: &[(&str, &str)]) -> RepoDetail {
        let mut v = listing(name);
        let siblings = v["siblings"].as_array_mut().unwrap();
        siblings.retain(|s| !s["rfilename"].as_str().unwrap().ends_with(".gguf"));
        for (from, file) in files {
            let s = listing(from)["siblings"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["rfilename"] == *file)
                .unwrap_or_else(|| panic!("{from} has no {file}"))
                .clone();
            siblings.push(s);
        }
        parse(&v)
    }

    fn offered(d: &RepoDetail) -> Vec<String> {
        match recognise(d, &families()) {
            Recognition::Supported { files, .. } => files.into_iter().map(|f| f.path).collect(),
            other => panic!("{}: {other:?}", d.id),
        }
    }

    /// One family token decides for a whole repository, but a file whose own name says it is
    /// another model is not offered under that family.
    #[test]
    fn a_mixed_repo_offers_only_its_familys_files() {
        const FAST: &str = "repo-fastwan22-ti2v-5b-gguf.json";
        const FULL: &str = "repo-wan22-ti2v-5b-full-gguf.json";
        // FastWan and the full Wan 2.2 in one repository: only the FastWan file.
        let mixed = with_ggufs(
            FAST,
            &[
                (FAST, "FastWan2.2-TI2V-5B-q6_k.gguf"),
                (FULL, "Wan2.2-TI2V-5B-Q4_K_M.gguf"),
            ],
        );
        assert_eq!(
            family_of_name("Wan2.2-TI2V-5B-Q4_K_M.gguf"),
            NameMatch::UncheckedWan22
        );
        assert_eq!(offered(&mixed), ["FastWan2.2-TI2V-5B-q6_k.gguf"]);
        // Only the full Wan 2.2 under a FastWan name: nothing, and why.
        assert!(matches!(
            recognise(&with_ggufs(FAST, &[(FULL, "Wan2.2-TI2V-5B-Q4_K_M.gguf")]), &families()),
            Recognition::NotSupportedYet(r) if r.contains("fast version")
        ));
        // A 9B klein beside the 4B files of a 4B repository (its name has no family token).
        const KLEIN: &str = "repo-flux2-klein-4b-gguf.json";
        let klein = with_ggufs(
            KLEIN,
            &[
                (KLEIN, "flux-2-klein-4b-Q4_0.gguf"),
                ("repo-flux2-klein-9b-gguf.json", "flux-2-klein-9b-Q4_0.gguf"),
                (KLEIN, "flux-2-klein-4b-Q8_0.gguf"),
            ],
        );
        assert_eq!(
            offered(&klein),
            ["flux-2-klein-4b-Q4_0.gguf", "flux-2-klein-4b-Q8_0.gguf"]
        );
        // Files that name no family are offered when the repository's name is the family's
        // and no file names it.
        let mut v = listing(KLEIN);
        for s in v["siblings"].as_array_mut().unwrap() {
            if let Some(n) = s["rfilename"]
                .as_str()
                .and_then(|n| n.strip_prefix("flux-2-klein-4b-"))
            {
                s["rfilename"] = format!("model-{n}").into();
            }
        }
        assert_eq!(offered(&parse(&v)), ["model-Q4_0.gguf", "model-Q8_0.gguf"]);
    }

    #[test]
    fn names_are_read_by_their_family_tokens() {
        assert_eq!(
            family_of_name("Green-Sky/FastWan2.2-TI2V-5B-FullAttn-GGUF"),
            NameMatch::Family(GenFamily::Wan22Ti2v5bFast),
            "a family token wins over the unchecked one inside it"
        );
        assert_eq!(
            family_of_name("hum-ma/Wan2.2-TI2V-5B-Turbo-GGUF"),
            NameMatch::UncheckedWan22
        );
        assert_eq!(
            family_of_name("z_image_turbo-Q4_K.gguf"),
            NameMatch::Family(GenFamily::ZImageTurbo)
        );
        assert_eq!(
            family_of_name("FLUX 2 klein 4b q4.gguf"),
            NameMatch::Family(GenFamily::Flux2Klein4b)
        );
        assert_eq!(
            family_of_name("Wan2.1_T2V_1.3B_fp16.safetensors"),
            NameMatch::Family(GenFamily::Wan21T2v1_3b)
        );
        assert_eq!(
            family_of_name("someone/Z-Image-Turbo-and-FLUX.2-klein-4B"),
            NameMatch::Several
        );
        assert_eq!(
            family_of_name("leejet/FLUX.2-klein-base-4B-GGUF"),
            NameMatch::None
        );
        assert_eq!(family_of_name("unsloth/Qwen3-4B-GGUF"), NameMatch::None);
    }

    #[test]
    fn families_are_told_apart_by_their_tensor_tables() {
        assert_eq!(
            family_from_tensors(&table("klein4b-tensors.txt")),
            Some(GenFamily::Flux2Klein4b)
        );
        // A 9B file named like the 4B is never taken for it.
        assert_eq!(family_from_tensors(&table("klein9b-tensors.txt")), None);
        assert_eq!(
            family_from_tensors(&table("zimage-tensors.txt")),
            Some(GenFamily::ZImageTurbo)
        );
        assert_eq!(
            family_from_tensors(&table("wan22-ti2v-5b-tensors.txt")),
            Some(GenFamily::Wan22Ti2v5bFast)
        );
        // Converted with the engine's own tool: every name starts `model.diffusion_model.`.
        assert_eq!(
            family_from_tensors(&table("wan21-t2v-1.3b-tensors.txt")),
            Some(GenFamily::Wan21T2v1_3b)
        );
        assert!(
            looks_like_diffusion(&table("klein9b-tensors.txt"))
                && !looks_like_diffusion(&table("qwen3-4b-tensors.txt"))
        );
        assert_eq!(family_from_tensors(&table("qwen3-4b-tensors.txt")), None);
        for fixture in testing::TENSOR_FIXTURES {
            assert_eq!(
                looks_like_diffusion(&table(fixture)),
                fixture != "qwen3-4b-tensors.txt",
                "{fixture}"
            );
        }
    }

    /// One block more or less, another width, other input channels: not the family.
    #[test]
    fn a_signature_needs_every_part_of_it() {
        let wan = table("wan22-ti2v-5b-tensors.txt");
        let without_last_block: Vec<TensorShape> = wan
            .iter()
            .filter(|t| !t.name.starts_with("blocks.29."))
            .cloned()
            .collect();
        assert_eq!(family_from_tensors(&without_last_block), None);
        assert!(looks_like_diffusion(&without_last_block));
        let mut other_channels = wan.clone();
        for t in &mut other_channels {
            if t.name == "patch_embedding.weight" {
                t.dims = vec![2, 2, 1, 16 * 3072];
            }
        }
        assert_eq!(family_from_tensors(&other_channels), None);
        let mut other_width = table("klein4b-tensors.txt");
        for t in &mut other_width {
            if t.name == "img_in.weight" {
                t.dims = vec![128, 3584];
            }
        }
        assert_eq!(family_from_tensors(&other_width), None);
        assert_eq!(family_from_tensors(&[]), None);
        assert!(!looks_like_diffusion(&[]));
    }

    #[test]
    fn an_unknown_family_says_not_supported_yet() {
        assert_eq!(
            recognise(&detail("repo-unknown-diffusion.json"), &families()),
            Recognition::NotSupportedYet("harnessUI does not know this kind of model yet.".into())
        );
        // A family this catalog has no record of is not known either.
        let mut few = families();
        few.remove(&GenFamily::Flux2Klein4b);
        assert!(matches!(
            recognise(&detail("repo-flux2-klein-4b-gguf.json"), &few),
            Recognition::NotSupportedYet(_)
        ));
        // A stated architecture that is a chat model's.
        let mut v = listing("repo-fastwan22-ti2v-5b-gguf.json");
        v["gguf"]["architecture"] = "qwen3".into();
        assert_eq!(
            recognise(&parse(&v), &families()),
            Recognition::NotSupportedYet("harnessUI does not know this kind of model yet.".into())
        );
        assert!(matches!(
            recognise(
                &renamed(
                    "repo-flux2-klein-4b-gguf.json",
                    "someone/Z-Image-Turbo-GGUF"
                ),
                &families()
            ),
            Recognition::NotSupportedYet(r) if r.contains("several kinds")
        ));
    }

    #[test]
    fn an_uncensored_or_region_limited_repo_is_not_offered() {
        assert_eq!(
            recognise(&detail("repo-uncensored.json"), &families()),
            Recognition::NotOffered(NotOffered::Uncensored)
        );
        assert_eq!(
            recognise(
                &renamed(
                    "repo-unknown-diffusion.json",
                    "someone/HunyuanVideo-1.5-GGUF"
                ),
                &families()
            ),
            Recognition::NotOffered(NotOffered::RegionLimited)
        );
        assert_eq!(
            recognise(
                &renamed(
                    "repo-flux2-klein-4b-gguf.json",
                    "someone/FLUX.2-klein-4B-abliterated"
                ),
                &families()
            ),
            Recognition::NotOffered(NotOffered::Uncensored)
        );
        let adult = RepoDetail {
            not_for_all_audiences: true,
            ..detail("repo-flux2-klein-4b-gguf.json")
        };
        assert_eq!(
            recognise(&adult, &families()),
            Recognition::NotOffered(NotOffered::AdultContent)
        );
    }

    #[test]
    fn a_repo_without_any_published_hash_offers_nothing() {
        assert!(matches!(
            recognise(&detail("repo-no-hashes.json"), &families()),
            Recognition::NotSupportedYet(r) if r.contains("checksum")
        ));
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
    }
}
