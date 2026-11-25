//! Version → recipe (design section 7.1). The recipes do not say which detected version
//! they serve, so this keeps the matching table: for each version, a default recipe and
//! rules on the file's own facts and name tokens that pick among several. It also gives
//! each recipe its Studio kind and input pictures, and lists the recipes Studio leaves out
//! (design sections 3 and 11a).

use serde::{Deserialize, Serialize};

use crate::gen_catalog::GenInput;
use crate::sd::CannotRunReason;
use crate::sd::detect::SdVersion;
use crate::sd::index::{PICKLE_EXTENSIONS, READ_EXTENSIONS};
use crate::sd::inspect::MatchFacts;
use crate::sd::recipes::{SlotId, recipe, recipes};

/// What a recipe makes, as Studio shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipeKind {
    Picture,
    /// It takes a picture: references, or a picture to change with its mask.
    PictureEdit,
    Video,
}

/// How many pictures of one role a recipe takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeInput {
    pub role: GenInput,
    pub min: u32,
    pub max: u32,
}

/// The part of a two-model video (Wan 2.2 A14B) a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoisePart {
    High,
    Low,
}

/// The recipe a model file runs with, or why Studio does not run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Recipe {
        id: &'static str,
        /// For a recipe with a high-noise slot, the part the name says the file is.
        part: Option<NoisePart>,
    },
    Refused(CannotRunReason),
}

// ---- The matching table ------------------------------------------------------------------

/// One thing a rule can ask of a file.
#[derive(Debug, Clone, Copy)]
enum Fact {
    /// A name token ([`Name::has`]).
    W(&'static str),
    /// `sd3_blocks` is this (`None`: unknown).
    Blocks(Option<u32>),
    /// The file embeds all of these.
    Embeds(&'static [SlotId]),
    /// `flux2_width` is this.
    Width(u64),
    ZeroCond,
    ImgEmbPos,
    HasImgEmb,
    /// The name says which noise part the file is ([`noise_part`]).
    Part,
}

/// A condition of a rule; a rule holds when all its terms do.
#[derive(Debug, Clone, Copy)]
enum Term {
    Is(Fact),
    Any(&'static [Fact]),
    NoneOf(&'static [Fact]),
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    To(&'static str),
    Refuse(CannotRunReason),
}

type Rule = (&'static [Term], Outcome);

/// Versions, the default, and the rules; the first rule that holds wins.
const TABLE: &[(&[SdVersion], Outcome, &[Rule])] = {
    use Fact::*;
    use Outcome::*;
    use SdVersion::*;
    use Term::*;
    const SD3_ENCODERS: &[SlotId] = &[SlotId::ClipL, SlotId::ClipG, SlotId::T5xxl];
    &[
        (&[Sd1, Sd1TinyUnet], To("sd1-checkpoint"), &[]),
        (&[Sdxs512Ds, Sdxs09], To("sdxs"), &[]),
        (&[Sd1Pix2pix], To("instruct-pix2pix"), &[]),
        (
            &[Sd2, Sd2TinyUnet],
            To("sd2-512-checkpoint"),
            &[
                (&[Is(W("turbo"))], To("sd-turbo")),
                (&[Any(&[W("768"), W("v")])], To("sd2-768-checkpoint")),
            ],
        ),
        (
            &[Sdxl, SdxlVega, SdxlSsd1b],
            To("sdxl-checkpoint"),
            &[
                (&[Is(W("turbo"))], To("sdxl-turbo")),
                // NitroFusion's files are `nitrosd-realism_unet`, `nitrosd-vibrant_unet`.
                (&[Any(&[W("nitro"), W("nitrosd")])], To("nitrosd-realism")),
            ],
        ),
        (&[SdxlPix2pix], To("cosxl-edit"), &[]),
        (
            &[Sd3],
            To("sd35-large"),
            &[
                (
                    &[Is(Blocks(Some(24))), Is(Embeds(SD3_ENCODERS))],
                    To("sd3-medium"),
                ),
                (&[Is(Blocks(Some(24)))], To("sd35-medium")),
                (&[Is(Blocks(Some(38)))], To("sd35-large")),
                (&[Is(Blocks(None)), Is(W("medium"))], To("sd35-medium")),
                (&[Is(Blocks(None)), Is(W("large"))], To("sd35-large")),
            ],
        ),
        // Diffusers-layout Fill, Canny and Depth files read as flux (Task R5).
        (
            &[Flux],
            To("flux1-dev"),
            &[
                (&[Is(W("kontext"))], To("flux1-kontext")),
                (&[Is(W("schnell"))], To("flux1-schnell")),
                (&[Is(W("fill"))], To("flux1-fill")),
                (&[Is(W("canny"))], To("flux1-canny")),
                (&[Is(W("depth"))], To("flux1-depth")),
            ],
        ),
        (&[FluxFill], To("flux1-fill"), &[]),
        (
            &[FluxControls],
            To("flux1-canny"),
            &[(&[Is(W("depth"))], To("flux1-depth"))],
        ),
        (&[Sd1Inpaint], To("sd1-inpainting"), &[]),
        (&[Sd2Inpaint], To("sd2-inpainting"), &[]),
        (&[SdxlInpaint], To("sdxl-inpainting"), &[]),
        (&[Flex2], To("flex2"), &[]),
        (&[Chroma], To("chroma"), &[]),
        (&[ChromaRadiance], To("chroma-radiance"), &[]),
        (&[OvisImage], To("ovis-image"), &[]),
        (
            &[Longcat],
            To("longcat-image"),
            &[(&[Is(W("edit"))], To("longcat-image-edit"))],
        ),
        (&[Flux2], To("flux2-dev"), &[]),
        (
            &[Flux2Klein],
            To("flux2-klein-4b"),
            &[
                (
                    &[Any(&[Width(4096), W("9b")]), Is(W("base"))],
                    To("flux2-klein-base-9b"),
                ),
                (&[Any(&[Width(4096), W("9b")])], To("flux2-klein-9b")),
                (&[Is(W("base"))], To("flux2-klein-base-4b")),
            ],
        ),
        (
            &[Flux3],
            To("flux3-klein"),
            &[(&[Is(W("base"))], To("flux3-klein-base"))],
        ),
        (&[SefiImage], To("sefi-image"), &[]),
        (
            &[QwenImage],
            To("qwen-image"),
            &[
                (
                    &[Is(W("edit")), Any(&[W("2511"), ZeroCond])],
                    To("qwen-image-edit-2511"),
                ),
                (&[Is(W("edit")), Is(W("2509"))], To("qwen-image-edit-2509")),
                (&[Is(W("edit"))], To("qwen-image-edit")),
            ],
        ),
        (&[QwenImageLayered], To("qwen-image-layered"), &[]),
        (
            &[QwenImage21],
            To("qwen-image-21"),
            &[(&[Is(W("turbo"))], To("qwen-image-21-turbo"))],
        ),
        (
            &[MageFlow],
            To("mage-flow"),
            &[
                (&[Is(W("edit"))], To("mage-flow-edit")),
                (&[Is(W("turbo"))], To("mage-flow-turbo")),
            ],
        ),
        (
            &[ZImage],
            To("z-image"),
            &[(&[Is(W("turbo"))], To("z-image-turbo"))],
        ),
        (&[ZImageL2p], To("z-image-l2p"), &[]),
        (&[MingImage], To("ming-image"), &[]),
        (
            &[Anima],
            To("anima"),
            &[(&[Is(W("edit"))], To("anima-edit"))],
        ),
        (
            &[BooguImage],
            To("boogu-image"),
            &[(&[Is(W("edit"))], To("boogu-image-edit"))],
        ),
        (
            &[ErnieImage],
            To("ernie-image"),
            &[(&[Is(W("turbo"))], To("ernie-image-turbo"))],
        ),
        (
            &[Fibo2],
            To("fibo2-dev"),
            &[(&[Is(W("edit"))], To("fibo2-edit-turbo"))],
        ),
        (&[HidreamO1], To("hidream-o1-dev"), &[]),
        (&[Ideogram4], To("ideogram4"), &[]),
        (&[Iris], To("iris-3b"), &[]),
        (
            &[Krea2],
            To("krea2-raw"),
            &[
                (&[Is(W("turbo")), Is(W("edit"))], To("krea2-turbo-edit")),
                (&[Is(W("edit"))], To("krea2-raw-edit")),
                (&[Is(W("turbo"))], To("krea2-turbo")),
            ],
        ),
        (
            &[Lens],
            To("lens"),
            &[(&[Is(W("turbo"))], To("lens-turbo"))],
        ),
        (
            &[LladaImage],
            To("llada-image"),
            &[(&[Is(W("turbo"))], To("llada-image-turbo"))],
        ),
        (&[Minit2i], To("minit2i"), &[]),
        (
            &[MossImage],
            To("moss-image-base"),
            &[(&[Is(W("sft"))], To("moss-image-sft"))],
        ),
        (
            &[Pixart],
            To("pixart-sigma"),
            &[
                // Every official PixArt repo is under `PixArt-alpha/`, Sigma's too, and the
                // 512 one is `PixArt-XL-2-512x512`.
                (
                    &[
                        Is(W("alpha")),
                        Any(&[W("512"), W("512x512")]),
                        NoneOf(&[W("sigma")]),
                    ],
                    To("pixart-alpha-512"),
                ),
                (
                    &[Is(W("alpha")), NoneOf(&[W("sigma")])],
                    To("pixart-alpha-1024"),
                ),
            ],
        ),
        (
            &[MinimaxH3Image],
            To("minimax-h3-image"),
            &[(&[Any(&[W("p-h3"), W("p-image")])], To("p-h3-image"))],
        ),
        (&[PH3Image], To("p-h3-image"), &[]),
        (
            &[Wan2],
            To("wan21-t2v"),
            &[
                (
                    &[Is(W("vace"))],
                    Refuse(CannotRunReason::NeedsControlFrames),
                ),
                (&[Any(&[ImgEmbPos, W("flf2v")])], To("wan21-flf2v")),
                (&[Is(HasImgEmb)], To("wan21-i2v")),
                (
                    // `wan2-2`: Kijai's `Wan2_2-T2V-A14B-HIGH_…`.
                    &[Any(&[W("wan2.2"), W("wan22"), W("wan2-2")]), Is(Part)],
                    To("wan22-t2v"),
                ),
            ],
        ),
        (&[Wan22I2v], To("wan22-i2v"), &[]),
        (&[Wan22Ti2v], To("wan22-ti2v-5b"), &[]),
        (
            &[LingbotVideo],
            To("lingbot-video-dense"),
            &[(
                &[Any(&[W("moe"), W("30b"), W("a3b")])],
                To("lingbot-video-moe"),
            )],
        ),
        (
            &[HunyuanVideo],
            To("hunyuan-video-15-t2v"),
            &[(&[Is(W("i2v"))], To("hunyuan-video-15-i2v"))],
        ),
        (
            &[Ltxav],
            To("ltx23-dev"),
            &[
                (
                    &[
                        Any(&[W("ltx-2"), W("ltx2")]),
                        // `ltx-2-3`: Kijai/LTX2.3_comfy's `ltx-2-3-22b-dev_transformer_only`.
                        NoneOf(&[W("2.3"), W("23"), W("2.5"), W("25"), W("ltx-2-3")]),
                    ],
                    Refuse(CannotRunReason::Ltx2),
                ),
                (&[Any(&[W("2.5"), W("ltx25")])], To("ltx25-dev")),
                (&[Is(W("distilled"))], To("ltx23-distilled")),
            ],
        ),
        (
            &[MinimaxH3],
            To("minimax-h3-fl2va"),
            &[
                (&[Any(&[W("p-h3"), W("p-image")])], To("p-h3-image")),
                (
                    &[Is(W("hyperflow")), Is(W("ref2va"))],
                    To("hyperflow-h3-flash-ref2va"),
                ),
                (&[Is(W("hyperflow"))], To("hyperflow-h3-flash-fl2va")),
                (&[Is(W("ref2va"))], To("minimax-h3-ref2va")),
            ],
        ),
        // Versions Studio does not run (design section 3).
        (&[Svd], Refuse(CannotRunReason::NotThisEngine), &[]),
        (&[Wan22S2v], Refuse(CannotRunReason::NeedsAudio), &[]),
        (&[Pid], Refuse(CannotRunReason::Upscaler), &[]),
        (&[SensenovaU15], Refuse(CannotRunReason::FolderModel), &[]),
    ]
};

/// The recipes Studio leaves out, and why: 18 of the engine's 112. Adapter files, upscalers
/// and audio-driven video are out (design section 3); so is VACE, which needs a folder of
/// control frames rather than a picture (a plan decision).
const OUT_OF_SCOPE: &[(CannotRunReason, &[&str])] = &[
    (
        CannotRunReason::Adapter,
        &[
            "sd1-controlnet",
            "sdxl-controlnet",
            "sd1-ip-adapter",
            "sdxl-ip-adapter",
            "sdxl-photomaker",
            "animatediff-sd15",
            "flux1-pulid",
        ],
    ),
    (CannotRunReason::NeedsControlFrames, &["wan21-vace"]),
    (CannotRunReason::NeedsAudio, &["wan22-s2v"]),
    (
        CannotRunReason::Upscaler,
        &[
            "pid",
            "pid-15",
            "pid-sd3",
            "pid-15-sd3",
            "pid-flux2",
            "pid-15-flux2",
            "pid-qwen-image",
            "pid-15-qwen-image",
        ],
    ),
    (CannotRunReason::FolderModel, &["sensenova-u15"]),
];

/// The picture recipes that take a picture: references, or a picture to change with its
/// mask (design 11a for the inpainting and Fill ones).
const PICTURE_EDITS: [&str; 20] = [
    "instruct-pix2pix",
    "cosxl-edit",
    "flux1-kontext",
    "qwen-image-edit",
    "qwen-image-edit-2509",
    "qwen-image-edit-2511",
    "qwen-image-layered",
    "longcat-image-edit",
    "mage-flow-edit",
    "anima-edit",
    "boogu-image-edit",
    "fibo2-edit-turbo",
    "krea2-raw-edit",
    "krea2-turbo-edit",
    "minimax-h3-image",
    "p-h3-image",
    "sd1-inpainting",
    "sd2-inpainting",
    "sdxl-inpainting",
    "flux1-fill",
];

/// At most this many reference pictures (`-r`), as 0.1.x's `MAX_REFERENCES`.
const MAX_REFERENCES: u32 = 4;

const fn input(role: GenInput, min: u32, max: u32) -> RecipeInput {
    RecipeInput { role, min, max }
}

/// The recipes whose input pictures are not read from their features, with the engine's
/// flags (`examples/common/common.cpp:1088-1107`). A Flex.2 mask needs a source: without
/// one the engine repaints everything (`src/pipeline/image.cpp:274-281`).
const INPUTS: &[(&[&str], &[RecipeInput])] = {
    use GenInput::*;
    &[
        (
            &[
                "sd1-inpainting",
                "sd2-inpainting",
                "sdxl-inpainting",
                "flux1-fill",
            ],
            &[input(Source, 1, 1), input(Mask, 1, 1)],
        ),
        (&["flux1-canny", "flux1-depth"], &[input(Control, 1, 1)]),
        (
            &["flex2"],
            &[input(Source, 0, 1), input(Mask, 0, 1), input(Control, 0, 1)],
        ),
        // UNet edit models read only the first reference (`src/pipeline/image.cpp:494-495`).
        (
            &["instruct-pix2pix", "cosxl-edit"],
            &[input(Reference, 1, 1)],
        ),
        (
            &["wan21-i2v", "hunyuan-video-15-i2v"],
            &[input(StartFrame, 1, 1)],
        ),
        // Wan 2.2 I2V also takes an end frame: the engine puts it in the last conditioning
        // frame and marks it in the mask (`src/pipeline/video.cpp:934-937, 978-980, 1000-1004`).
        (
            &["wan22-i2v"],
            &[input(StartFrame, 1, 1), input(EndFrame, 0, 1)],
        ),
        (
            &["wan21-flf2v"],
            &[input(StartFrame, 1, 1), input(EndFrame, 1, 1)],
        ),
    ]
};

// ---- Names ---------------------------------------------------------------------------

/// The extensions a name sheds before it is read for words: the weights files Studio reads
/// or refuses, and `.json`.
const EXTENSIONS: [&[&str]; 3] = [&READ_EXTENSIONS, &PICKLE_EXTENSIONS, &[".json"]];

/// `text` without one trailing extension of [`EXTENSIONS`], in any case.
fn without_extension(text: &str) -> &str {
    EXTENSIONS
        .iter()
        .flat_map(|group| group.iter())
        .find_map(|ext| {
            let cut = text.len().checked_sub(ext.len())?;
            text.get(cut..)
                .filter(|end| end.eq_ignore_ascii_case(ext))
                .and_then(|_| text.get(..cut))
        })
        .unwrap_or(text)
}

/// `text` without its extension, lower case, with `_` and ` ` read as `-`.
pub(crate) fn normalise(text: &str) -> String {
    without_extension(text)
        .to_lowercase()
        .replace(['_', ' '], "-")
}

/// The words of `text`. A trailing weights or config extension (`.safetensors`, `.sft`,
/// `.gguf`, `.ckpt`, `.pt`, `.pth`, `.bin`, `.json`, in any case) is dropped first; then
/// the text is lower-cased, `_` and ` ` read as `-`, and split at every character other
/// than an ASCII letter or digit, except a `.` between two digits (so `wan2.1`, `0.9` and
/// `v1.5` stay whole, and `ae.safetensors` gives `ae`).
pub fn tokens(text: &str) -> Vec<String> {
    let chars: Vec<char> = normalise(text).chars().collect();
    let digit = |i: Option<usize>| {
        i.and_then(|i| chars.get(i))
            .is_some_and(|c| c.is_ascii_digit())
    };
    let mut words = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let joins = c == '.' && digit(i.checked_sub(1)) && digit(Some(i + 1));
        if c.is_ascii_alphanumeric() || joins {
            word.push(c);
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// A name read for tokens.
struct Name {
    text: String,
    words: Vec<String>,
}

impl Name {
    fn new(name: &str) -> Self {
        Name {
            text: normalise(name),
            words: tokens(name),
        }
    }

    /// Whether the name has `token`: a whole word, or, for a multi-word token (`ltx-2`,
    /// `high-noise`), a substring of the normalised text.
    fn has(&self, token: &str) -> bool {
        if token.contains('-') {
            self.text.contains(token)
        } else {
            self.words.iter().any(|w| w == token)
        }
    }

    fn has_any(&self, tokens: &[&str]) -> bool {
        tokens.iter().any(|t| self.has(t))
    }

    fn part(&self) -> Option<NoisePart> {
        if self.has_any(&["high", "highnoise", "high-noise"]) {
            Some(NoisePart::High)
        } else if self.has_any(&["low", "lownoise", "low-noise"]) {
            Some(NoisePart::Low)
        } else {
            None
        }
    }
}

/// The noise part a name says a file is: the tokens `high`, `highnoise`, `high-noise`, or
/// `low`, `lownoise`, `low-noise` (high first, when a name has both).
pub fn noise_part(name: &str) -> Option<NoisePart> {
    Name::new(name).part()
}

// ---- Matching ------------------------------------------------------------------------

struct File<'a> {
    facts: &'a MatchFacts,
    embeds: &'a [SlotId],
    name: Name,
}

impl File<'_> {
    fn holds(&self, fact: Fact) -> bool {
        match fact {
            Fact::W(token) => self.name.has(token),
            Fact::Blocks(blocks) => self.facts.sd3_blocks == blocks,
            Fact::Embeds(slots) => slots.iter().all(|s| self.embeds.contains(s)),
            Fact::Width(width) => self.facts.flux2_width == Some(width),
            Fact::ZeroCond => self.facts.zero_cond,
            Fact::ImgEmbPos => self.facts.img_emb_pos,
            Fact::HasImgEmb => self.facts.has_img_emb,
            Fact::Part => self.name.part().is_some(),
        }
    }

    fn meets(&self, term: Term) -> bool {
        match term {
            Term::Is(fact) => self.holds(fact),
            Term::Any(facts) => facts.iter().any(|f| self.holds(*f)),
            Term::NoneOf(facts) => !facts.iter().any(|f| self.holds(*f)),
        }
    }
}

fn row(version: SdVersion) -> (Outcome, &'static [Rule]) {
    TABLE
        .iter()
        .find(|(versions, ..)| versions.contains(&version))
        .map(|(_, default, rules)| (*default, *rules))
        // Every version has a row (`every_version_has_exactly_one_row`).
        .unwrap_or((Outcome::Refuse(CannotRunReason::UnknownWeights), &[]))
}

/// The recipe for a model file the engine detects as `version`, from its facts, the slots
/// it embeds and its name (`"<repo>/<path>"`).
pub fn match_recipe(
    version: SdVersion,
    facts: &MatchFacts,
    embeds: &[SlotId],
    name: &str,
) -> Match {
    let file = File {
        facts,
        embeds,
        name: Name::new(name),
    };
    let (default, rules) = row(version);
    let outcome = rules
        .iter()
        .find(|(terms, _)| terms.iter().all(|t| file.meets(*t)))
        .map_or(default, |(_, outcome)| *outcome);
    match outcome {
        Outcome::To(id) => {
            let paired =
                recipe(id).is_some_and(|r| r.slot(SlotId::HighNoiseDiffusionModel).is_some());
            Match::Recipe {
                id,
                part: if paired { file.name.part() } else { None },
            }
        }
        Outcome::Refuse(reason) => Match::Refused(reason),
    }
}

/// Every in-scope recipe a file of `version` can take, the default first: the Model type
/// menu. Empty for a version Studio does not run.
pub fn recipes_for(version: SdVersion) -> Vec<&'static str> {
    let (default, rules) = row(version);
    let mut ids = Vec::new();
    for outcome in std::iter::once(default).chain(rules.iter().map(|(_, o)| *o)) {
        if let Outcome::To(id) = outcome
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// What a recipe makes.
pub fn kind_of(recipe_id: &str) -> RecipeKind {
    if recipe(recipe_id).is_some_and(|r| r.is_video()) {
        RecipeKind::Video
    } else if PICTURE_EDITS.contains(&recipe_id) {
        RecipeKind::PictureEdit
    } else {
        RecipeKind::Picture
    }
}

/// Why Studio leaves a recipe out, if it does.
pub fn out_of_scope(recipe_id: &str) -> Option<CannotRunReason> {
    OUT_OF_SCOPE
        .iter()
        .find(|(_, ids)| ids.contains(&recipe_id))
        .map(|(reason, _)| *reason)
}

/// The recipes Studio runs: 94, sorted by id.
pub fn in_scope_recipes() -> Vec<&'static str> {
    recipes()
        .iter()
        .map(|r| r.id.as_str())
        .filter(|id| out_of_scope(id).is_none())
        .collect()
}

/// The input pictures a recipe takes, by role. Reference pictures (`-r`) for a recipe with
/// `ref_images`, at least one for a picture edit; else, for a video, an optional start
/// (`-i`) and end frame (`--end-img`) by its features.
pub fn inputs_of(recipe_id: &str) -> Vec<RecipeInput> {
    if let Some((_, inputs)) = INPUTS.iter().find(|(ids, _)| ids.contains(&recipe_id)) {
        return inputs.to_vec();
    }
    let Some(r) = recipe(recipe_id) else {
        return Vec::new();
    };
    if r.feature("ref_images") {
        let min = u32::from(kind_of(recipe_id) == RecipeKind::PictureEdit);
        return vec![input(GenInput::Reference, min, MAX_REFERENCES)];
    }
    if !r.is_video() {
        return Vec::new();
    }
    [
        ("init_image", GenInput::StartFrame),
        ("end_image", GenInput::EndFrame),
    ]
    .into_iter()
    .filter(|(feature, _)| r.feature(feature))
    .map(|(_, role)| input(role, 0, 1))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn m(v: SdVersion, name: &str) -> Match {
        match_recipe(v, &MatchFacts::default(), &[], name)
    }

    #[test]
    fn the_design_examples_match() {
        assert_eq!(
            m(
                SdVersion::Flux,
                "black-forest-labs/FLUX.1-dev/flux1-dev.safetensors"
            ),
            Match::Recipe {
                id: "flux1-dev",
                part: None
            }
        );
        assert_eq!(
            m(
                SdVersion::Flux,
                "o/FLUX.1-schnell-gguf/flux1-schnell-Q4_0.gguf"
            ),
            Match::Recipe {
                id: "flux1-schnell",
                part: None
            }
        );
        assert_eq!(
            m(
                SdVersion::Flux,
                "QuantStack/FLUX.1-Kontext-dev-GGUF/flux1-kontext-dev-Q4_K_M.gguf"
            ),
            Match::Recipe {
                id: "flux1-kontext",
                part: None
            }
        );
        assert_eq!(
            m(SdVersion::QwenImage, "o/Qwen-Image-Edit-2511-GGUF/x.gguf"),
            Match::Recipe {
                id: "qwen-image-edit-2511",
                part: None
            }
        );
        assert_eq!(
            m(
                SdVersion::QwenImage,
                "QuantStack/Qwen-Image-Edit-2509-GGUF/Qwen-Image-Edit-2509-Q4_0.gguf"
            ),
            Match::Recipe {
                id: "qwen-image-edit-2509",
                part: None
            }
        );
        assert_eq!(
            m(SdVersion::QwenImage, "o/Qwen-Image-Edit/x.gguf"),
            Match::Recipe {
                id: "qwen-image-edit",
                part: None
            }
        );
    }

    #[test]
    fn wan_parts_come_from_the_name() {
        assert_eq!(
            m(
                SdVersion::Wan2,
                "QuantStack/Wan2.2-T2V-A14B-GGUF/HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf"
            ),
            Match::Recipe {
                id: "wan22-t2v",
                part: Some(NoisePart::High)
            }
        );
        assert_eq!(
            m(
                SdVersion::Wan2,
                "QuantStack/Wan2.2-T2V-A14B-GGUF/LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf"
            ),
            Match::Recipe {
                id: "wan22-t2v",
                part: Some(NoisePart::Low)
            }
        );
        assert_eq!(
            match_recipe(
                SdVersion::Wan2,
                &MatchFacts {
                    has_img_emb: true,
                    ..Default::default()
                },
                &[],
                "city96/Wan2.1-I2V-14B-480P-gguf/w.gguf"
            ),
            Match::Recipe {
                id: "wan21-i2v",
                part: None
            }
        );
        assert_eq!(
            m(
                SdVersion::Wan2,
                "Comfy-Org/Wan-2.1/wan2.1_t2v_1.3B_fp16.safetensors"
            ),
            Match::Recipe {
                id: "wan21-t2v",
                part: None
            }
        );
        assert_eq!(
            m(SdVersion::Wan2, "o/Wan2.1-VACE-14B/x.gguf"),
            Match::Refused(CannotRunReason::NeedsControlFrames)
        );
    }

    #[test]
    fn sd3_and_klein_use_the_files_own_facts() {
        let f = |b| MatchFacts {
            sd3_blocks: Some(b),
            ..Default::default()
        };
        assert_eq!(
            match_recipe(
                SdVersion::Sd3,
                &f(24),
                &[SlotId::Vae],
                "o/r/sd3.5_medium.safetensors"
            ),
            Match::Recipe {
                id: "sd35-medium",
                part: None
            }
        );
        assert_eq!(
            match_recipe(
                SdVersion::Sd3,
                &f(24),
                &[SlotId::ClipL, SlotId::ClipG, SlotId::T5xxl, SlotId::Vae],
                "o/r/sd3_medium_incl_clips_t5xxlfp8.safetensors"
            ),
            Match::Recipe {
                id: "sd3-medium",
                part: None
            }
        );
        assert_eq!(
            match_recipe(
                SdVersion::Flux2Klein,
                &MatchFacts {
                    flux2_width: Some(4096),
                    ..Default::default()
                },
                &[],
                "o/r/k.gguf"
            ),
            Match::Recipe {
                id: "flux2-klein-9b",
                part: None
            }
        );
    }

    #[test]
    fn ltx_2_is_refused_and_ltx_2_3_runs() {
        assert_eq!(
            m(
                SdVersion::Ltxav,
                "unsloth/LTX-2.3-GGUF/ltx-2.3-22b-dev-Q4_K_M.gguf"
            ),
            Match::Recipe {
                id: "ltx23-dev",
                part: None
            }
        );
        assert_eq!(
            m(
                SdVersion::Ltxav,
                "Lightricks/LTX-2/ltx-2-19b-dev.safetensors"
            ),
            Match::Refused(CannotRunReason::Ltx2)
        );
    }

    #[test]
    fn every_in_scope_recipe_is_reachable_and_exists() {
        assert_eq!(in_scope_recipes().len(), 94);
        let reached: BTreeSet<_> = SdVersion::ALL
            .iter()
            .flat_map(|v| recipes_for(*v))
            .collect();
        assert_eq!(reached, in_scope_recipes().into_iter().collect());
        assert!(
            recipes()
                .iter()
                .all(|r| out_of_scope(&r.id).is_some()
                    != in_scope_recipes().contains(&r.id.as_str()))
        );
        assert!(
            recipes()
                .iter()
                .filter(|r| r.slots.iter().any(|s| s.id.is_adapter()))
                .all(|r| out_of_scope(&r.id) == Some(CannotRunReason::Adapter))
        );
    }

    #[test]
    fn kinds() {
        assert_eq!(
            (
                kind_of("flux1-kontext"),
                kind_of("wan22-t2v"),
                kind_of("sdxl-checkpoint")
            ),
            (
                RecipeKind::PictureEdit,
                RecipeKind::Video,
                RecipeKind::Picture
            )
        );
        assert_eq!(
            (
                kind_of("sdxl-inpainting"),
                kind_of("flux1-fill"),
                kind_of("flux1-canny")
            ),
            (
                RecipeKind::PictureEdit,
                RecipeKind::PictureEdit,
                RecipeKind::Picture
            )
        );
    }

    #[test]
    fn inpainting_fill_and_control_recipes_are_offered_with_their_input_pictures() {
        assert_eq!(
            m(SdVersion::SdxlInpaint, "o/r/sdxl-inpainting.safetensors"),
            Match::Recipe {
                id: "sdxl-inpainting",
                part: None
            }
        );
        assert_eq!(
            m(SdVersion::FluxFill, "o/r/flux1-fill-dev.safetensors"),
            Match::Recipe {
                id: "flux1-fill",
                part: None
            }
        );
        assert_eq!(
            m(SdVersion::FluxControls, "o/FLUX.1-Depth-dev/x.safetensors"),
            Match::Recipe {
                id: "flux1-depth",
                part: None
            }
        );
        let ins = |id| {
            inputs_of(id)
                .into_iter()
                .map(|i| (i.role, i.min, i.max))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ins("sdxl-inpainting"),
            [(GenInput::Source, 1, 1), (GenInput::Mask, 1, 1)]
        );
        assert_eq!(ins("flux1-canny"), [(GenInput::Control, 1, 1)]);
        assert_eq!(
            ins("flex2"),
            [
                (GenInput::Source, 0, 1),
                (GenInput::Mask, 0, 1),
                (GenInput::Control, 0, 1)
            ]
        );
        assert_eq!(ins("flux1-kontext"), [(GenInput::Reference, 1, 4)]);
        assert_eq!(
            ins("wan21-flf2v"),
            [(GenInput::StartFrame, 1, 1), (GenInput::EndFrame, 1, 1)]
        );
        assert_eq!(
            (out_of_scope("sd1-controlnet"), out_of_scope("flux1-fill")),
            (Some(CannotRunReason::Adapter), None)
        );
    }

    #[test]
    fn tokens_split_like_the_spec() {
        assert_eq!(
            tokens("Wan2.2_T2V-A14B HighNoise"),
            ["wan2.2", "t2v", "a14b", "highnoise"]
        );
    }

    // ---- Beyond the brief's examples -------------------------------------------------

    #[test]
    fn every_version_has_exactly_one_row() {
        for v in SdVersion::ALL {
            let rows = TABLE
                .iter()
                .filter(|(versions, ..)| versions.contains(&v))
                .count();
            assert_eq!(rows, 1, "{}", v.word());
        }
    }

    #[test]
    fn versions_studio_does_not_run_are_refused_with_no_recipes() {
        for (v, reason) in [
            (SdVersion::Svd, CannotRunReason::NotThisEngine),
            (SdVersion::Wan22S2v, CannotRunReason::NeedsAudio),
            (SdVersion::Pid, CannotRunReason::Upscaler),
            (SdVersion::SensenovaU15, CannotRunReason::FolderModel),
        ] {
            assert_eq!(m(v, "o/r/x.safetensors"), Match::Refused(reason), "{v:?}");
            assert!(recipes_for(v).is_empty(), "{v:?}");
        }
    }

    #[test]
    fn every_rule_of_the_table_picks_its_recipe() {
        use SdVersion::*;
        // Rules on name tokens alone.
        let by_name: &[(SdVersion, &str, &str)] = &[
            (Sd1TinyUnet, "o/r/x", "sd1-checkpoint"),
            (Sdxs09, "o/r/x", "sdxs"),
            (Sd1Pix2pix, "o/r/x", "instruct-pix2pix"),
            (Sd2, "o/r/x", "sd2-512-checkpoint"),
            (Sd2, "stabilityai/sd-turbo/x", "sd-turbo"),
            (Sd2, "o/r/v2-1_768-ema.x", "sd2-768-checkpoint"),
            (Sd2TinyUnet, "o/v/x", "sd2-768-checkpoint"),
            (SdxlVega, "o/r/x", "sdxl-checkpoint"),
            (Sdxl, "stabilityai/sdxl-turbo/x", "sdxl-turbo"),
            (Sdxl, "o/SDXL-Nitro/x", "nitrosd-realism"),
            (SdxlPix2pix, "o/r/x", "cosxl-edit"),
            (Sd3, "o/r/x", "sd35-large"),
            (Sd3, "o/r/medium", "sd35-medium"),
            (Sd3, "o/r/large", "sd35-large"),
            (Flux, "o/FLUX.1-Fill-dev/x", "flux1-fill"),
            (Flux, "o/FLUX.1-Canny-dev/x", "flux1-canny"),
            (Flux, "o/FLUX.1-Depth-dev/x", "flux1-depth"),
            (FluxFill, "o/r/x", "flux1-fill"),
            (FluxControls, "o/FLUX.1-Canny-dev/x", "flux1-canny"),
            (Sd1Inpaint, "o/r/x", "sd1-inpainting"),
            (Sd2Inpaint, "o/r/x", "sd2-inpainting"),
            (Flex2, "o/r/x", "flex2"),
            (Chroma, "o/r/x", "chroma"),
            (ChromaRadiance, "o/r/x", "chroma-radiance"),
            (OvisImage, "o/r/x", "ovis-image"),
            (Longcat, "o/LongCat-Image/x", "longcat-image"),
            (Longcat, "o/LongCat-Image-Edit/x", "longcat-image-edit"),
            (Flux2, "o/r/x", "flux2-dev"),
            (Flux2Klein, "o/FLUX.2-klein-4B/x", "flux2-klein-4b"),
            (Flux2Klein, "o/FLUX.2-klein-9B/x", "flux2-klein-9b"),
            (
                Flux2Klein,
                "o/FLUX.2-klein-base-9B/x",
                "flux2-klein-base-9b",
            ),
            (
                Flux2Klein,
                "o/FLUX.2-klein-base-4B/x",
                "flux2-klein-base-4b",
            ),
            (Flux3, "o/r/x", "flux3-klein"),
            (Flux3, "o/r-base/x", "flux3-klein-base"),
            (SefiImage, "o/r/x", "sefi-image"),
            (QwenImage, "o/Qwen-Image/x", "qwen-image"),
            (QwenImageLayered, "o/r/x", "qwen-image-layered"),
            (QwenImage21, "o/r/x", "qwen-image-21"),
            (QwenImage21, "o/r-turbo/x", "qwen-image-21-turbo"),
            (MageFlow, "o/r/x", "mage-flow"),
            (MageFlow, "o/r-edit/x", "mage-flow-edit"),
            (MageFlow, "o/r-turbo/x", "mage-flow-turbo"),
            (ZImage, "o/r/x", "z-image"),
            (ZImage, "o/Z-Image-Turbo/x", "z-image-turbo"),
            (ZImageL2p, "o/r/x", "z-image-l2p"),
            (MingImage, "o/r/x", "ming-image"),
            (Anima, "o/r/x", "anima"),
            (Anima, "o/r-edit/x", "anima-edit"),
            (BooguImage, "o/r/x", "boogu-image"),
            (BooguImage, "o/r-edit/x", "boogu-image-edit"),
            (ErnieImage, "o/r/x", "ernie-image"),
            (ErnieImage, "o/r-turbo/x", "ernie-image-turbo"),
            (Fibo2, "o/r/x", "fibo2-dev"),
            (Fibo2, "o/r-edit/x", "fibo2-edit-turbo"),
            (HidreamO1, "o/r/x", "hidream-o1-dev"),
            (Ideogram4, "o/r/x", "ideogram4"),
            (Iris, "o/r/x", "iris-3b"),
            (Krea2, "o/r/x", "krea2-raw"),
            (Krea2, "o/r-edit-turbo/x", "krea2-turbo-edit"),
            (Krea2, "o/r-edit/x", "krea2-raw-edit"),
            (Krea2, "o/r-turbo/x", "krea2-turbo"),
            (Lens, "o/r/x", "lens"),
            (Lens, "o/r-turbo/x", "lens-turbo"),
            (LladaImage, "o/r/x", "llada-image"),
            (LladaImage, "o/r-turbo/x", "llada-image-turbo"),
            (Minit2i, "o/r/x", "minit2i"),
            (MossImage, "o/r/x", "moss-image-base"),
            (MossImage, "o/r-sft/x", "moss-image-sft"),
            (Pixart, "o/r/x", "pixart-sigma"),
            (Pixart, "o/PixArt-alpha-512/x", "pixart-alpha-512"),
            (Pixart, "o/PixArt-alpha/x", "pixart-alpha-1024"),
            (MinimaxH3Image, "o/r/x", "minimax-h3-image"),
            (MinimaxH3Image, "o/P-Image/x", "p-h3-image"),
            (PH3Image, "o/r/x", "p-h3-image"),
            (Wan2, "o/Wan2.1-FLF2V-14B/x", "wan21-flf2v"),
            (Wan2, "o/Wan2.2-T2V-A14B/x", "wan21-t2v"),
            (Wan22I2v, "o/r/x", "wan22-i2v"),
            (Wan22Ti2v, "o/r/x", "wan22-ti2v-5b"),
            (LingbotVideo, "o/r/x", "lingbot-video-dense"),
            (LingbotVideo, "o/r-A3B/x", "lingbot-video-moe"),
            (HunyuanVideo, "o/r/x", "hunyuan-video-15-t2v"),
            (
                HunyuanVideo,
                "o/r/hunyuan_720p_i2v_fp16.x",
                "hunyuan-video-15-i2v",
            ),
            (Ltxav, "o/LTX-2.5/x", "ltx25-dev"),
            (Ltxav, "o/LTX-2.3/ltx-2.3-distilled-Q4.x", "ltx23-distilled"),
            (MinimaxH3, "o/r/x", "minimax-h3-fl2va"),
            (MinimaxH3, "o/P-H3/x", "p-h3-image"),
            (
                MinimaxH3,
                "o/HyperFlow-H3-Ref2VA/x",
                "hyperflow-h3-flash-ref2va",
            ),
            (
                MinimaxH3,
                "o/HyperFlow-H3-Flash/x",
                "hyperflow-h3-flash-fl2va",
            ),
            (MinimaxH3, "o/MiniMax-H3-Ref2VA/x", "minimax-h3-ref2va"),
        ];
        let recipe = |id| Match::Recipe { id, part: None };
        for &(v, name, id) in by_name {
            assert_eq!(m(v, name), recipe(id), "{} {name}", v.word());
            assert!(recipes_for(v).contains(&id), "{} offers {id}", v.word());
        }
        for name in ["o/ltx2/x", "o/LTX-2-dev/x"] {
            assert_eq!(
                m(Ltxav, name),
                Match::Refused(CannotRunReason::Ltx2),
                "{name}"
            );
        }
        // Rules on the file's own facts.
        let encoders = [SlotId::ClipL, SlotId::ClipG, SlotId::T5xxl];
        let sd3 = |blocks, embeds: &[SlotId], name| {
            let facts = MatchFacts {
                sd3_blocks: blocks,
                ..Default::default()
            };
            match_recipe(Sd3, &facts, embeds, name)
        };
        assert_eq!(sd3(Some(24), &encoders, "o/r/x"), recipe("sd3-medium"));
        assert_eq!(
            sd3(Some(24), &encoders[..2], "o/r/x"),
            recipe("sd35-medium")
        );
        assert_eq!(sd3(Some(38), &[], "o/r/medium"), recipe("sd35-large"));
        assert_eq!(sd3(Some(30), &[], "o/r/medium"), recipe("sd35-large"));
        let width = MatchFacts {
            flux2_width: Some(4096),
            ..Default::default()
        };
        assert_eq!(
            match_recipe(Flux2Klein, &width, &[], "o/FLUX.2-klein-base/x"),
            recipe("flux2-klein-base-9b")
        );
        let zero = MatchFacts {
            zero_cond: true,
            ..Default::default()
        };
        assert_eq!(
            match_recipe(QwenImage, &zero, &[], "o/Qwen-Image-Edit/x"),
            recipe("qwen-image-edit-2511")
        );
        assert_eq!(
            match_recipe(QwenImage, &zero, &[], "o/Qwen-Image/x"),
            recipe("qwen-image")
        );
        let pos = MatchFacts {
            img_emb_pos: true,
            has_img_emb: true,
            ..Default::default()
        };
        assert_eq!(
            match_recipe(Wan2, &pos, &[], "o/r/x"),
            recipe("wan21-flf2v")
        );
    }

    #[test]
    fn the_part_is_set_only_for_recipes_with_a_high_noise_slot() {
        assert_eq!(
            m(SdVersion::Wan22I2v, "o/Wan2.2-I2V-A14B/HighNoise/x.gguf"),
            Match::Recipe {
                id: "wan22-i2v",
                part: Some(NoisePart::High)
            }
        );
        assert_eq!(
            m(SdVersion::Wan22Ti2v, "o/Wan2.2-TI2V-5B/low-noise/x.gguf"),
            Match::Recipe {
                id: "wan22-ti2v-5b",
                part: None
            }
        );
    }

    #[test]
    fn noise_parts_are_read_from_tokens_and_multi_word_tokens() {
        assert_eq!(
            noise_part("o/r/wan2.2_t2v_high_noise_14B_fp8.safetensors"),
            Some(NoisePart::High)
        );
        assert_eq!(
            noise_part("o/r/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf"),
            Some(NoisePart::High)
        );
        assert_eq!(noise_part("o/High/x.gguf"), Some(NoisePart::High));
        assert_eq!(
            noise_part("o/r/wan2.2_t2v_low_noise_14B_fp8.safetensors"),
            Some(NoisePart::Low)
        );
        assert_eq!(noise_part("o/LowNoise/x.gguf"), Some(NoisePart::Low));
        assert_eq!(noise_part("o/low/x.gguf"), Some(NoisePart::Low));
        assert_eq!(noise_part("o/Wan2.2-TI2V-5B-GGUF/x.gguf"), None);
        assert_eq!(noise_part("o/highres-fix/x.gguf"), None, "not a whole word");
    }

    #[test]
    fn tokens_lower_case_and_keep_dots_in_versions() {
        assert_eq!(
            tokens("unsloth/LTX-2.3-GGUF/ltx-2.3-22b-dev-Q4_K_M.gguf"),
            [
                "unsloth", "ltx", "2.3", "gguf", "ltx", "2.3", "22b", "dev", "q4", "k", "m"
            ]
        );
        assert_eq!(
            tokens("Comfy-Org/Wan-2.1/wan2.1_t2v_1.3B_fp16.safetensors"),
            [
                "comfy", "org", "wan", "2.1", "wan2.1", "t2v", "1.3b", "fp16"
            ]
        );
        assert!(tokens(" _-/ ").is_empty());
    }

    // ---- Pre-review round: the extension, real names, the Wan 2.2 end frame ----------

    #[test]
    fn tokens_drop_the_extension_and_split_dots_between_letters() {
        // The FLUX.1 VAE, and the engine docs' renamed copy (`docs/models/flux.md`).
        assert_eq!(
            tokens("black-forest-labs/FLUX.1-dev/ae.safetensors"),
            ["black", "forest", "labs", "flux", "1", "dev", "ae"]
        );
        assert_eq!(tokens("ae.sft"), ["ae"]);
        assert_eq!(tokens("o/sd-v1.5/x.CKPT"), ["o", "sd", "v1.5", "x"]);
        assert_eq!(tokens("model.pth"), ["model"]);
        assert_eq!(tokens("model.pt"), ["model"]);
        assert_eq!(tokens("config.json"), ["config"]);
        // Only the very end, and only once.
        assert_eq!(tokens("x.gguf.part"), ["x", "gguf", "part"]);
        assert_eq!(tokens("x.safetensors.json"), ["x", "safetensors"]);
        assert_eq!(tokens("ltx-0.9.8"), ["ltx", "0.9.8"]);
        assert_eq!(tokens("a.b"), ["a", "b"]);
        // `.sft` goes too, or a renamed MOSS-Image base file would read as SFT.
        assert_eq!(
            m(SdVersion::MossImage, "o/r/moss_image_base.sft"),
            Match::Recipe {
                id: "moss-image-base",
                part: None
            }
        );
    }

    #[test]
    fn a_rule_word_right_before_the_extension_counts() {
        let recipe = |id| Match::Recipe { id, part: None };
        assert_eq!(
            m(
                SdVersion::Ltxav,
                "Lightricks/LTX-2.3/ltx-2.3-22b-distilled.safetensors"
            ),
            recipe("ltx23-distilled")
        );
        assert_eq!(
            m(
                SdVersion::ErnieImage,
                "Comfy-Org/ERNIE-Image/diffusion_models/ernie-image-turbo.safetensors"
            ),
            recipe("ernie-image-turbo")
        );
        assert_eq!(
            noise_part("o/r/Wan2.2-T2V-A14B-HighNoise.gguf"),
            Some(NoisePart::High)
        );
    }

    #[test]
    fn nitrosd_files_match_the_nitrosd_recipe() {
        for name in [
            "ChenDY/NitroFusion/nitrosd-realism_unet.safetensors",
            "ChenDY/NitroFusion/nitrosd-realism_comfyui.safetensors",
        ] {
            assert_eq!(
                m(SdVersion::Sdxl, name),
                Match::Recipe {
                    id: "nitrosd-realism",
                    part: None
                },
                "{name}"
            );
        }
    }

    #[test]
    fn pixart_reads_512x512_and_sigma_under_the_pixart_alpha_owner() {
        let pixart = |name| m(SdVersion::Pixart, name);
        let recipe = |id| Match::Recipe { id, part: None };
        assert_eq!(
            pixart(
                "PixArt-alpha/PixArt-XL-2-512x512/transformer/diffusion_pytorch_model.safetensors"
            ),
            recipe("pixart-alpha-512")
        );
        assert_eq!(
            pixart(
                "PixArt-alpha/PixArt-XL-2-1024-MS/transformer/diffusion_pytorch_model.safetensors"
            ),
            recipe("pixart-alpha-1024")
        );
        assert_eq!(
            pixart(
                "PixArt-alpha/PixArt-Sigma-XL-2-1024-MS/transformer/diffusion_pytorch_model.safetensors"
            ),
            recipe("pixart-sigma")
        );
    }

    #[test]
    fn kijais_ltx_2_3_file_runs_and_ltx_2_files_are_still_refused() {
        assert_eq!(
            m(
                SdVersion::Ltxav,
                "Kijai/LTX2.3_comfy/diffusion_models/ltx-2-3-22b-dev_transformer_only_fp8_input_scaled.safetensors"
            ),
            Match::Recipe {
                id: "ltx23-dev",
                part: None
            }
        );
        for name in [
            "Kijai/LTXV2_comfy/diffusion_models/ltx-2-19b-dev_fp4_transformer_only.safetensors",
            "Lightricks/LTX-2/ltx-2-19b-distilled.safetensors",
        ] {
            assert_eq!(
                m(SdVersion::Ltxav, name),
                Match::Refused(CannotRunReason::Ltx2),
                "{name}"
            );
        }
    }

    #[test]
    fn wan22_i2v_takes_an_optional_end_frame() {
        // `src/pipeline/video.cpp:934-937, 978-980, 1000-1004` at the pin.
        assert!(recipe("wan22-i2v").unwrap().feature("end_image"));
        let ins = inputs_of("wan22-i2v")
            .into_iter()
            .map(|i| (i.role, i.min, i.max))
            .collect::<Vec<_>>();
        assert_eq!(
            ins,
            [(GenInput::StartFrame, 1, 1), (GenInput::EndFrame, 0, 1)]
        );
    }

    // ---- Fix round 1 -------------------------------------------------------------------

    #[test]
    fn kijais_wan2_2_names_match_wan_2_2_with_their_part() {
        use NoisePart::*;
        let kijai = "Kijai/WanVideo_comfy_fp8_scaled";
        for (v, file, id, part) in [
            (
                SdVersion::Wan2,
                "T2V/Wan2_2-T2V-A14B-HIGH_fp8_e5m2_scaled_KJ.safetensors",
                "wan22-t2v",
                Some(High),
            ),
            (
                SdVersion::Wan2,
                "T2V/Wan2_2-T2V-A14B-LOW_fp8_e4m3fn_scaled_KJ.safetensors",
                "wan22-t2v",
                Some(Low),
            ),
            (
                SdVersion::Wan2,
                "T2V/Wan2_2-T2V-A14B_HIGH_fp8_e4m3fn_scaled_KJ.safetensors",
                "wan22-t2v",
                Some(High),
            ),
            (
                SdVersion::Wan22I2v,
                "I2V/Wan2_2-I2V-A14B-HIGH_fp8_e4m3fn_scaled_KJ.safetensors",
                "wan22-i2v",
                Some(High),
            ),
            (
                SdVersion::Wan22I2v,
                "I2V/Wan2_2-I2V-A14B-LOW_fp8_e5m2_scaled_KJ.safetensors",
                "wan22-i2v",
                Some(Low),
            ),
            (
                SdVersion::Wan22Ti2v,
                "TI2V/Wan2_2-TI2V-5B_fp8_e4m3fn_scaled_KJ.safetensors",
                "wan22-ti2v-5b",
                None,
            ),
            (
                SdVersion::Wan2,
                "T2V/Wan2_1-T2V-14B_fp8_e4m3fn_scaled_KJ.safetensors",
                "wan21-t2v",
                None,
            ),
        ] {
            let name = format!("{kijai}/{file}");
            assert_eq!(m(v, &name), Match::Recipe { id, part }, "{name}");
        }
        // `wan2-2` is never read in a Wan 2.1 name.
        for name in [
            format!("{kijai}/T2V/Wan2_1-T2V-14B_fp8_e4m3fn_scaled_KJ.safetensors"),
            format!("{kijai}/T2V/Wan2_1-T2V-14B-FastWan-480p_fp8_e4m3fn_scaled_KJ.safetensors"),
            format!("{kijai}/I2V/Wan2_1-I2V-14B-720p_fp8_e5m2_scaled_KJ.safetensors"),
            format!("{kijai}/VACE/Wan2_1-VACE-module-14B_fp8_e4m3fn_scaled_KJ.safetensors"),
            "Comfy-Org/Wan_2.1_ComfyUI_repackaged/split_files/diffusion_models/wan2.1_t2v_14B_fp16.safetensors".into(),
        ] {
            assert!(!Name::new(&name).has("wan2-2"), "{name}");
        }
    }

    #[test]
    fn unet_edit_recipes_take_one_reference() {
        // The engine reads only `ref_latents[0]` for these (`src/pipeline/image.cpp:494-495`;
        // `is_unet_edit` in `src/model/integrations/sd.cpp:79-82, 194-198`).
        for id in ["instruct-pix2pix", "cosxl-edit"] {
            assert_eq!(
                inputs_of(id),
                [RecipeInput {
                    role: GenInput::Reference,
                    min: 1,
                    max: 1
                }],
                "{id}"
            );
        }
        assert_eq!(inputs_of("qwen-image-edit")[0].max, 4);
    }

    #[test]
    fn picture_edits_take_a_picture_and_references_follow_the_recipe() {
        for id in in_scope_recipes() {
            let ins = inputs_of(id);
            if kind_of(id) == RecipeKind::PictureEdit {
                assert!(ins.iter().any(|i| i.min >= 1), "{id} takes a picture");
            }
            let refs = ins.iter().any(|i| i.role == GenInput::Reference);
            assert_eq!(refs, recipe(id).unwrap().feature("ref_images"), "{id}");
            assert!(ins.iter().all(|i| i.min <= i.max && i.max >= 1), "{id}");
        }
        let ins = |id| {
            inputs_of(id)
                .into_iter()
                .map(|i| (i.role, i.min, i.max))
                .collect::<Vec<_>>()
        };
        assert_eq!(ins("flux2-klein-4b"), [(GenInput::Reference, 0, 4)]);
        assert_eq!(ins("minimax-h3-ref2va"), [(GenInput::Reference, 0, 4)]);
        assert_eq!(ins("wan21-i2v"), [(GenInput::StartFrame, 1, 1)]);
        assert_eq!(
            ins("ltx23-dev"),
            [(GenInput::StartFrame, 0, 1), (GenInput::EndFrame, 0, 1)]
        );
        assert_eq!(ins("wan22-ti2v-5b"), [(GenInput::StartFrame, 0, 1)]);
        assert_eq!(ins("wan22-t2v"), []);
        assert_eq!(ins("sdxl-checkpoint"), []);
    }

    #[test]
    fn the_fixture_files_match_their_recipes() {
        use crate::sd::inspect::{FileKind, inspect};
        use crate::sd::testing::{fixture, fixture_source};
        for (name, id, part) in [
            ("flux1-dev", "flux1-dev", None),
            ("flux1-kontext", "flux1-kontext", None),
            ("flux2-klein-4b", "flux2-klein-4b", None),
            ("chroma", "chroma", None),
            ("hunyuan-video-15", "hunyuan-video-15-t2v", None),
            ("ltx23", "ltx23-dev", None),
            ("qwen-image", "qwen-image", None),
            ("qwen-image-edit-2509", "qwen-image-edit-2509", None),
            ("sd15", "sd1-checkpoint", None),
            ("sd35-medium", "sd35-medium", None),
            ("sdxl", "sdxl-checkpoint", None),
            ("wan21-i2v-14b", "wan21-i2v", None),
            ("wan21-t2v-1.3b", "wan21-t2v", None),
            ("wan22-t2v-a14b-high", "wan22-t2v", Some(NoisePart::High)),
            ("wan22-t2v-a14b-low", "wan22-t2v", Some(NoisePart::Low)),
            ("wan22-ti2v-5b", "wan22-ti2v-5b", None),
            ("z-image-turbo", "z-image-turbo", None),
        ] {
            let facts = inspect(&fixture(name));
            let FileKind::Model {
                version, embeds, ..
            } = facts.kind
            else {
                panic!("{name} is a model: {:?}", facts.kind)
            };
            let source = fixture_source(name);
            let full = format!("{}/{}", source.repo, source.path);
            assert_eq!(
                match_recipe(version, &facts.facts, &embeds, &full),
                Match::Recipe { id, part },
                "{name}"
            );
        }
    }

    #[test]
    fn kinds_and_inputs_have_their_wire_words() {
        use serde_json::json;
        assert_eq!(json!(RecipeKind::PictureEdit), "pictureEdit");
        assert_eq!(json!(NoisePart::High), "high");
        assert_eq!(
            json!(inputs_of("flux1-fill")),
            json!([{"role": "source", "min": 1, "max": 1}, {"role": "mask", "min": 1, "max": 1}])
        );
    }
}
