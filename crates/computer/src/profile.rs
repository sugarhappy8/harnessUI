//! Per-model settings for computer use (ARCHITECTURE.md 9.3 "Model conventions"): how the
//! model writes coordinates, and the exact image size llama.cpp would feed it.
//!
//! **Why the app resizes screenshots itself.** llama.cpp resizes every image to the size
//! its projector expects before the model sees it. A model that answers in pixels answers
//! in pixels of *that* resized image, not of the image the app sent. Sending exactly the
//! size llama.cpp would resize to makes its resize a no-op, so the app knows what the model
//! saw and can map the answer back.
//!
//! [`SmartResize::target_size`] is a line-by-line port of `calc_size_preserved_ratio` in
//! llama.cpp `tools/mtmd/mtmd-image.cpp` at the pinned commit (`llama.lock.json`,
//! cd959bae). It is computed in `f32` exactly as the C++ does, and the unit tests compare
//! it against a table produced by compiling that C++ function verbatim (identical with
//! g++ -O0/-O2 and clang++ -O3 -ffp-contract=fast). The parameters come from the
//! projector:
//!
//! | Projector (llama.cpp) | `align` | tokens (default) | rounding |
//! |---|---|---|---|
//! | Qwen2-VL, Qwen2.5-VL (patch 14, merge 2) | 28 | 8–4096 | half to even |
//! | Qwen3-VL (patch 16, merge 2) | 32 | 8–4096 | half to even |
//!
//! `min_pixels = min_tokens × align²`, `max_pixels = max_tokens × align²`. The server's
//! `--image-min-tokens` / `--image-max-tokens` and the GGUF's own min/max pixel keys
//! replace the defaults; the app must pass the values the server was started with (the
//! research recommends `--image-min-tokens 1024` for Qwen-family grounders).
//! `longest_edge` covers projectors that cap the longest side instead (Pixtral-style).
//! Projectors that use one fixed square size use [`ResizeRule::Fixed`].
//!
//! **Automatic profiles.** Nobody should have to know any of this to use computer use, so
//! [`auto_profile`] picks the convention and the resize rule from the model's name: the
//! families the research documents, and a 0–1000 grid for everything else. The user can
//! still choose another convention per model in Settings.

use serde::{Deserialize, Serialize};

/// How a model writes a screen position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Convention {
    /// Pixels of the resized image the model saw (Qwen2.5-VL lineage: UI-TARS-1.5,
    /// Fara-7B, Holo1.5).
    #[default]
    Pixels,
    /// A 0–1000 grid over the image, `[x, y]` (Qwen3-VL, Qwen3.5, Holo2/3.1/4).
    Grid1000,
    /// 0–1000 boxes `[x1, y1, x2, y2]`; the centre is used.
    Box1000,
    /// 0–1000 boxes in `[y1, x1, y2, x2]` order (Gemma's `box_2d`; the axis order comes
    /// from secondary sources).
    Box1000Yx,
}

impl Convention {
    /// One line for the model, after each screenshot, saying how to answer.
    pub fn instructions(self, image: (u32, u32)) -> String {
        match self {
            Convention::Pixels => format!(
                "Coordinates are pixels of this {}x{} image: [x, y] with x from 0 (left) to {} and y from 0 (top) to {}.",
                image.0,
                image.1,
                image.0.saturating_sub(1),
                image.1.saturating_sub(1)
            ),
            Convention::Grid1000 => "Coordinates are on a 0-1000 grid over this image: [x, y] with [0, 0] the top-left corner and [1000, 1000] the bottom-right corner.".to_string(),
            Convention::Box1000 => "Coordinates are boxes on a 0-1000 grid over this image: [x1, y1, x2, y2] (the centre is used); [0, 0] is the top-left corner and [1000, 1000] the bottom-right corner.".to_string(),
            Convention::Box1000Yx => "Coordinates are boxes on a 0-1000 grid over this image: [y1, x1, y2, x2] (the centre is used); [0, 0] is the top-left corner and [1000, 1000] the bottom-right corner.".to_string(),
        }
    }

    /// Short name for cards and text lines.
    pub fn label(self) -> &'static str {
        match self {
            Convention::Pixels => "pixels of the image",
            Convention::Grid1000 => "0-1000 grid",
            Convention::Box1000 => "0-1000 boxes [x1, y1, x2, y2]",
            Convention::Box1000Yx => "0-1000 boxes [y1, x1, y2, x2]",
        }
    }
}

/// The parameters of llama.cpp's `calc_size_preserved_ratio` (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartResize {
    /// Each side becomes a multiple of this (`patch_size × n_merge`). At least 1.
    pub align: u32,
    /// 0 = no lower bound.
    pub min_pixels: u32,
    /// 0 = no upper bound.
    pub max_pixels: u32,
    /// 0 = off. When set, the longest side is scaled to this first.
    pub longest_edge: u32,
    /// Python's `round()` (half to even) instead of C's `round()` (half away from zero).
    pub round_half_even: bool,
}

impl SmartResize {
    /// llama.cpp's default limit for Qwen-family projectors (`set_limit_image_tokens(8,
    /// 4096)`).
    pub const QWEN_DEFAULT_TOKENS: (u32, u32) = (8, 4096);

    /// Qwen-family projectors: `align = patch_size × merge`, token limits converted to
    /// pixels as llama.cpp's `set_limit_image_tokens` does.
    pub fn qwen(patch_size: u32, merge: u32, min_tokens: u32, max_tokens: u32) -> Self {
        let align = (patch_size * merge).max(1);
        let area = align.saturating_mul(align);
        Self {
            align,
            min_pixels: min_tokens.saturating_mul(area),
            max_pixels: max_tokens.saturating_mul(area),
            longest_edge: 0,
            round_half_even: true,
        }
    }

    /// Qwen2-VL / Qwen2.5-VL (patch 14, merge 2: align 28).
    pub fn qwen2_vl(min_tokens: u32, max_tokens: u32) -> Self {
        Self::qwen(14, 2, min_tokens, max_tokens)
    }

    /// Qwen3-VL (patch 16, merge 2: align 32).
    pub fn qwen3_vl(min_tokens: u32, max_tokens: u32) -> Self {
        Self::qwen(16, 2, min_tokens, max_tokens)
    }

    /// The size llama.cpp resizes a `width` × `height` image to. Port of
    /// `calc_size_preserved_ratio` (mtmd-image.cpp), in `f32` like the original.
    pub fn target_size(&self, width: u32, height: u32) -> (u32, u32) {
        if width == 0 || height == 0 {
            return (0, 0);
        }
        // Sizes past i32 are not images llama.cpp accepts either.
        let width = width.min(i32::MAX as u32) as i32;
        let height = height.min(i32::MAX as u32) as i32;
        let align = self.align.clamp(1, i32::MAX as u32) as i32;
        let f = align as f32;
        let round_by_factor = |x: f32| -> i32 {
            let q = x / f;
            let r = if self.round_half_even {
                q.round_ties_even()
            } else {
                q.round()
            };
            (r as i32).saturating_mul(align)
        };
        let ceil_by_factor = |x: f32| -> i32 { ((x / f).ceil() as i32).saturating_mul(align) };
        let floor_by_factor = |x: f32| -> i32 { ((x / f).floor() as i32).saturating_mul(align) };

        let (mut w_bar, mut h_bar);
        if self.longest_edge > 0 {
            let le = self.longest_edge as f32;
            let scale = (le / width as f32).min(le / height as f32);
            w_bar = ceil_by_factor(width as f32 * scale);
            h_bar = ceil_by_factor(height as f32 * scale);
        } else {
            // always align up first
            w_bar = align.max(round_by_factor(width as f32));
            h_bar = align.max(round_by_factor(height as f32));
        }

        let area = i64::from(h_bar) * i64::from(w_bar);
        if self.max_pixels > 0 && area > i64::from(self.max_pixels) {
            let beta = (height as f32 * width as f32 / self.max_pixels as f32).sqrt();
            h_bar = align.max(floor_by_factor(height as f32 / beta));
            w_bar = align.max(floor_by_factor(width as f32 / beta));
        } else if self.min_pixels > 0 && area < i64::from(self.min_pixels) {
            let beta = (self.min_pixels as f32 / (height as f32 * width as f32)).sqrt();
            h_bar = ceil_by_factor(height as f32 * beta);
            w_bar = ceil_by_factor(width as f32 * beta);
        }
        (w_bar.max(1) as u32, h_bar.max(1) as u32)
    }
}

/// How the app sizes a screenshot before sending it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ResizeRule {
    /// llama.cpp's aspect-preserving resize (Qwen-family projectors and the like).
    Smart(SmartResize),
    /// One fixed size (projectors that stretch to a square; the image is stretched the same
    /// way so llama.cpp has nothing left to change).
    Fixed { width: u32, height: u32 },
    /// Send the capture as it is (remote providers that do their own resizing).
    Original,
}

impl ResizeRule {
    pub fn target_size(&self, width: u32, height: u32) -> (u32, u32) {
        match *self {
            ResizeRule::Smart(s) => s.target_size(width, height),
            ResizeRule::Fixed { width, height } => (width.max(1), height.max(1)),
            ResizeRule::Original => (width, height),
        }
    }
}

/// What the computer server needs to know about the model of a chat. The app works one
/// out per model ([`auto_profile`], or the convention the user chose in Settings) and sets
/// it per chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    pub convention: Convention,
    pub resize: ResizeRule,
}

impl Default for ModelProfile {
    /// Qwen2.5-VL lineage at llama.cpp's defaults: pixels of the resized image.
    fn default() -> Self {
        let (min, max) = SmartResize::QWEN_DEFAULT_TOKENS;
        Self {
            convention: Convention::Pixels,
            resize: ResizeRule::Smart(SmartResize::qwen2_vl(min, max)),
        }
    }
}

/// A profile picked from a model's name ([`auto_profile`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoProfile {
    pub profile: ModelProfile,
    /// The family the name matched. `None`: nothing matched, and the profile is the best
    /// guess for an unknown model.
    pub family: Option<&'static str>,
}

impl AutoProfile {
    /// Nothing in the name was recognised.
    pub fn is_guess(&self) -> bool {
        self.family.is_none()
    }

    /// The profile when the user picks the convention by hand.
    ///
    /// For a known family the projector is known, so only the convention changes. For an
    /// unknown model the resize rule follows the convention too: a model that answers in
    /// pixels answers in pixels of what its projector was fed, and the models that do so
    /// are the Qwen2.5-VL lineage (align 28). The 0–1000 conventions do not depend on the
    /// size, so they keep the rule of the guess.
    pub fn with_convention(&self, convention: Convention) -> ModelProfile {
        let resize = match (self.family, convention) {
            (None, Convention::Pixels) => Lineage::Qwen2.resize(),
            _ => self.profile.resize,
        };
        ModelProfile { convention, resize }
    }
}

/// Which projector a family's screenshots are sized for (see the module docs).
#[derive(Clone, Copy)]
enum Lineage {
    /// Qwen2-VL and Qwen2.5-VL: align 28.
    Qwen2,
    /// Qwen3-VL and later: align 32. Also what is sent to models whose projector is not
    /// known: it keeps the aspect ratio and caps the image at about 4 megapixels, and with
    /// a 0–1000 convention a second resize by the server changes nothing in the mapping.
    Qwen3,
}

impl Lineage {
    fn resize(self) -> ResizeRule {
        let (min, max) = SmartResize::QWEN_DEFAULT_TOKENS;
        ResizeRule::Smart(match self {
            Lineage::Qwen2 => SmartResize::qwen2_vl(min, max),
            Lineage::Qwen3 => SmartResize::qwen3_vl(min, max),
        })
    }
}

/// How a family's name is looked for in a model's name.
#[derive(Clone, Copy)]
enum Needle {
    /// Anywhere in the name with the separators taken out (`qwen25vl` finds
    /// `Qwen2.5-VL`, `qwen2_5_vl` and `Qwen2.5VL`).
    Anywhere(&'static str),
    /// The same, but only where a word starts and not followed by more letters: for short
    /// names that are also parts of other words (`holo` must not find `hologram`).
    Word(&'static str),
}

struct Family {
    name: &'static str,
    needles: &'static [Needle],
    convention: Convention,
    lineage: Lineage,
}

/// The families whose convention is written down in the research behind 9.3
/// (docs/research/browser-and-computer-use.md and its recheck). The first match wins, so
/// fine-tunes come before the base models they are built on, and a more specific name
/// before a shorter one.
///
/// Not every row is equally certain. Documented by the publisher: UI-TARS-1.5, Fara-7B,
/// Qwen3-VL, Holo-3.1 and Holo4. Inferred from the base model or from secondary sources:
/// Holo1/1.5, Holo2, Qwen3.5, Fara1.5 and Gemma's axis order. The user can always choose
/// another convention in Settings.
const FAMILIES: &[Family] = &[
    // Only 1.5 is documented; other UI-TARS versions are not assumed to be the same.
    Family {
        name: "UI-TARS-1.5",
        needles: &[Needle::Anywhere("uitars15")],
        convention: Convention::Pixels,
        lineage: Lineage::Qwen2,
    },
    // Built on Qwen3.5, but its card says it answers in pixels.
    Family {
        name: "Fara1.5",
        needles: &[Needle::Word("fara15")],
        convention: Convention::Pixels,
        lineage: Lineage::Qwen3,
    },
    Family {
        name: "Fara",
        needles: &[Needle::Word("fara")],
        convention: Convention::Pixels,
        lineage: Lineage::Qwen2,
    },
    Family {
        name: "Holo1",
        needles: &[Needle::Word("holo1")],
        convention: Convention::Pixels,
        lineage: Lineage::Qwen2,
    },
    // Holo2, Holo-3.1, Holo4.
    Family {
        name: "Holo",
        needles: &[Needle::Word("holo")],
        convention: Convention::Grid1000,
        lineage: Lineage::Qwen3,
    },
    Family {
        name: "Qwen2-VL",
        needles: &[Needle::Anywhere("qwen2vl"), Needle::Anywhere("qwen25vl")],
        convention: Convention::Pixels,
        lineage: Lineage::Qwen2,
    },
    // Qwen3-VL, Qwen3.5 and later.
    Family {
        name: "Qwen3",
        needles: &[Needle::Anywhere("qwen3")],
        convention: Convention::Grid1000,
        lineage: Lineage::Qwen3,
    },
    // Gemma's projector is not a Qwen one; with a 0–1000 convention that does not matter.
    Family {
        name: "Gemma",
        needles: &[Needle::Anywhere("gemma")],
        convention: Convention::Box1000Yx,
        lineage: Lineage::Qwen3,
    },
];

/// The convention for a model nothing is known about.
///
/// A 0–1000 grid, because it is the one answer that does not depend on what happened to
/// the image on the way to the model. The line after each screenshot
/// ([`Convention::instructions`]) tells the model how to answer, and a general vision
/// model follows it; but "pixels of this image" is only right if the model saw the image
/// at the size the app sent, and an unknown projector or a remote provider may have
/// resized it. A fraction of the image stays the same fraction at any size. It is also
/// what the newer grounding models (Qwen3-VL and later) answer in without being told. And
/// a wrong guess fails in the open more often: a model that answers in pixels of a large
/// image gives values over 1000, which are refused with a message, where a grid answer
/// read as pixels would click quietly near the top-left corner.
const UNKNOWN_CONVENTION: Convention = Convention::Grid1000;

/// A model name with everything but ASCII letters and digits taken out, in lower case,
/// remembering where words started.
struct Squashed {
    text: String,
    /// Per byte of `text`: the character began a word in the original name.
    word_start: Vec<bool>,
}

impl Squashed {
    fn new(name: &str) -> Self {
        let mut text = String::with_capacity(name.len());
        let mut word_start = Vec::with_capacity(name.len());
        let mut after_separator = true;
        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                text.push(c.to_ascii_lowercase());
                word_start.push(after_separator);
                after_separator = false;
            } else {
                after_separator = true;
            }
        }
        Self { text, word_start }
    }

    fn has(&self, needle: Needle) -> bool {
        match needle {
            Needle::Anywhere(n) => self.text.contains(n),
            Needle::Word(n) => self.text.match_indices(n).any(|(at, _)| {
                let end = at + n.len();
                let ends_word = match self.text.as_bytes().get(end) {
                    None => true,
                    Some(next) => next.is_ascii_digit() || self.word_start[end],
                };
                self.word_start[at] && ends_word
            }),
        }
    }

    fn family(&self) -> Option<&'static Family> {
        FAMILIES
            .iter()
            .find(|f| f.needles.iter().any(|&n| self.has(n)))
    }
}

/// The parts of a model reference that name the model, most telling first.
///
/// - `remote:<provider number>:<model id>`: the model id.
/// - `local:<path>` or anything ending in `.gguf`: the file name, then the folder it is
///   in. Downloads are stored as `<owner>/<repository>/<file>`, and some publishers name
///   every file the same (`q4_k_m.gguf`), so the repository folder is the only place the
///   model's name appears. Folders further up are not read: they are the publisher's or
///   the user's own names.
/// - anything else: the whole text.
fn name_parts(model: &str) -> Vec<&str> {
    let remote = model.strip_prefix("remote:").and_then(|rest| {
        let (provider, id) = rest.split_once(':')?;
        (!provider.is_empty() && provider.bytes().all(|b| b.is_ascii_digit())).then_some(id)
    });
    let local = model.strip_prefix("local:");
    let name = remote.or(local).unwrap_or(model);
    let is_file = name
        .len()
        .checked_sub(5)
        .and_then(|at| name.get(at..))
        .is_some_and(|ext| ext.eq_ignore_ascii_case(".gguf"));
    if local.is_none() && !is_file {
        return vec![name];
    }
    name.rsplit(['/', '\\'])
        .filter(|part| !part.is_empty())
        .take(2)
        .collect()
}

/// The profile for a model, from its name alone: a GGUF file name or path, a remote
/// model id, or the app's `local:<path>` / `remote:<n>:<id>` reference. Case and
/// separators do not matter.
///
/// This is what computer use runs with unless the user chose a convention for the model
/// in Settings.
pub fn auto_profile(model: &str) -> AutoProfile {
    let family = name_parts(model)
        .into_iter()
        .find_map(|part| Squashed::new(part).family());
    match family {
        Some(f) => AutoProfile {
            profile: ModelProfile {
                convention: f.convention,
                resize: f.lineage.resize(),
            },
            family: Some(f.name),
        },
        None => AutoProfile {
            profile: ModelProfile {
                convention: UNKNOWN_CONVENTION,
                resize: Lineage::Qwen3.resize(),
            },
            family: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (align, min_pixels, max_pixels, longest_edge, round_half_even, w, h, out_w, out_h),
    /// produced by compiling llama.cpp's calc_size_preserved_ratio (pinned commit) verbatim.
    type Row = (u32, u32, u32, u32, bool, u32, u32, u32, u32);
    #[rustfmt::skip]
    const GOLDEN: &[Row] = &[
        (28, 6272, 3211264, 0, true, 1440, 900, 1428, 896),
        (28, 6272, 3211264, 0, true, 1920, 1080, 1932, 1092),
        (28, 6272, 3211264, 0, true, 2560, 1440, 2380, 1344),
        (28, 6272, 3211264, 0, true, 3840, 2160, 2380, 1344),
        (28, 6272, 3211264, 0, true, 1280, 800, 1288, 812),
        (28, 6272, 3211264, 0, true, 1366, 768, 1372, 756),
        (28, 6272, 3211264, 0, true, 2880, 1800, 2240, 1400),
        (28, 6272, 3211264, 0, true, 3024, 1964, 2212, 1428),
        (28, 6272, 3211264, 0, true, 3456, 2234, 2212, 1428),
        (28, 6272, 3211264, 0, true, 5120, 2880, 2380, 1344),
        (28, 6272, 3211264, 0, true, 1680, 1050, 1680, 1064),
        (28, 6272, 3211264, 0, true, 1024, 768, 1036, 756),
        (28, 6272, 3211264, 0, true, 800, 600, 812, 588),
        (28, 6272, 3211264, 0, true, 3440, 1440, 2744, 1148),
        (28, 6272, 3211264, 0, true, 2560, 1600, 2240, 1400),
        (28, 6272, 3211264, 0, true, 1600, 900, 1596, 896),
        (28, 6272, 3211264, 0, true, 6016, 3384, 2380, 1344),
        (28, 6272, 3211264, 0, true, 100, 50, 112, 56),
        (28, 6272, 3211264, 0, true, 13, 7, 112, 84),
        (28, 6272, 3211264, 0, true, 1920, 1200, 1932, 1204),
        (28, 802816, 3211264, 0, true, 1440, 900, 1428, 896),
        (28, 802816, 3211264, 0, true, 1920, 1080, 1932, 1092),
        (28, 802816, 3211264, 0, true, 2560, 1440, 2380, 1344),
        (28, 802816, 3211264, 0, true, 3840, 2160, 2380, 1344),
        (28, 802816, 3211264, 0, true, 1280, 800, 1288, 812),
        (28, 802816, 3211264, 0, true, 1366, 768, 1372, 756),
        (28, 802816, 3211264, 0, true, 2880, 1800, 2240, 1400),
        (28, 802816, 3211264, 0, true, 3024, 1964, 2212, 1428),
        (28, 802816, 3211264, 0, true, 3456, 2234, 2212, 1428),
        (28, 802816, 3211264, 0, true, 5120, 2880, 2380, 1344),
        (28, 802816, 3211264, 0, true, 1680, 1050, 1680, 1064),
        (28, 802816, 3211264, 0, true, 1024, 768, 1036, 784),
        (28, 802816, 3211264, 0, true, 800, 600, 1036, 784),
        (28, 802816, 3211264, 0, true, 3440, 1440, 2744, 1148),
        (28, 802816, 3211264, 0, true, 2560, 1600, 2240, 1400),
        (28, 802816, 3211264, 0, true, 1600, 900, 1596, 896),
        (28, 802816, 3211264, 0, true, 6016, 3384, 2380, 1344),
        (28, 802816, 3211264, 0, true, 100, 50, 1288, 644),
        (28, 802816, 3211264, 0, true, 13, 7, 1232, 672),
        (28, 802816, 3211264, 0, true, 1920, 1200, 1932, 1204),
        (32, 8192, 4194304, 0, true, 1440, 900, 1440, 896),
        (32, 8192, 4194304, 0, true, 1920, 1080, 1920, 1088),
        (32, 8192, 4194304, 0, true, 2560, 1440, 2560, 1440),
        (32, 8192, 4194304, 0, true, 3840, 2160, 2720, 1536),
        (32, 8192, 4194304, 0, true, 1280, 800, 1280, 800),
        (32, 8192, 4194304, 0, true, 1366, 768, 1376, 768),
        (32, 8192, 4194304, 0, true, 2880, 1800, 2560, 1600),
        (32, 8192, 4194304, 0, true, 3024, 1964, 2528, 1632),
        (32, 8192, 4194304, 0, true, 3456, 2234, 2528, 1632),
        (32, 8192, 4194304, 0, true, 5120, 2880, 2720, 1536),
        (32, 8192, 4194304, 0, true, 1680, 1050, 1664, 1056),
        (32, 8192, 4194304, 0, true, 1024, 768, 1024, 768),
        (32, 8192, 4194304, 0, true, 800, 600, 800, 608),
        (32, 8192, 4194304, 0, true, 3440, 1440, 3136, 1312),
        (32, 8192, 4194304, 0, true, 2560, 1600, 2560, 1600),
        (32, 8192, 4194304, 0, true, 1600, 900, 1600, 896),
        (32, 8192, 4194304, 0, true, 6016, 3384, 2720, 1536),
        (32, 8192, 4194304, 0, true, 100, 50, 128, 64),
        (32, 8192, 4194304, 0, true, 13, 7, 128, 96),
        (32, 8192, 4194304, 0, true, 1920, 1200, 1920, 1216),
        (32, 1048576, 4194304, 0, true, 1440, 900, 1440, 896),
        (32, 1048576, 4194304, 0, true, 1920, 1080, 1920, 1088),
        (32, 1048576, 4194304, 0, true, 2560, 1440, 2560, 1440),
        (32, 1048576, 4194304, 0, true, 3840, 2160, 2720, 1536),
        (32, 1048576, 4194304, 0, true, 1280, 800, 1312, 832),
        (32, 1048576, 4194304, 0, true, 1366, 768, 1376, 768),
        (32, 1048576, 4194304, 0, true, 2880, 1800, 2560, 1600),
        (32, 1048576, 4194304, 0, true, 3024, 1964, 2528, 1632),
        (32, 1048576, 4194304, 0, true, 3456, 2234, 2528, 1632),
        (32, 1048576, 4194304, 0, true, 5120, 2880, 2720, 1536),
        (32, 1048576, 4194304, 0, true, 1680, 1050, 1664, 1056),
        (32, 1048576, 4194304, 0, true, 1024, 768, 1184, 896),
        (32, 1048576, 4194304, 0, true, 800, 600, 1184, 896),
        (32, 1048576, 4194304, 0, true, 3440, 1440, 3136, 1312),
        (32, 1048576, 4194304, 0, true, 2560, 1600, 2560, 1600),
        (32, 1048576, 4194304, 0, true, 1600, 900, 1600, 896),
        (32, 1048576, 4194304, 0, true, 6016, 3384, 2720, 1536),
        (32, 1048576, 4194304, 0, true, 100, 50, 1472, 736),
        (32, 1048576, 4194304, 0, true, 13, 7, 1408, 768),
        (32, 1048576, 4194304, 0, true, 1920, 1200, 1920, 1216),
        (28, 78400, 12845056, 0, false, 1440, 900, 1428, 896),
        (28, 78400, 12845056, 0, false, 1920, 1080, 1932, 1092),
        (28, 78400, 12845056, 0, false, 2560, 1440, 2548, 1428),
        (28, 78400, 12845056, 0, false, 3840, 2160, 3836, 2156),
        (28, 78400, 12845056, 0, false, 1280, 800, 1288, 812),
        (28, 78400, 12845056, 0, false, 1366, 768, 1372, 756),
        (28, 78400, 12845056, 0, false, 2880, 1800, 2884, 1792),
        (28, 78400, 12845056, 0, false, 3024, 1964, 3024, 1960),
        (28, 78400, 12845056, 0, false, 3456, 2234, 3444, 2240),
        (28, 78400, 12845056, 0, false, 5120, 2880, 4760, 2688),
        (28, 78400, 12845056, 0, false, 1680, 1050, 1680, 1064),
        (28, 78400, 12845056, 0, false, 1024, 768, 1036, 756),
        (28, 78400, 12845056, 0, false, 800, 600, 812, 588),
        (28, 78400, 12845056, 0, false, 3440, 1440, 3444, 1428),
        (28, 78400, 12845056, 0, false, 2560, 1600, 2548, 1596),
        (28, 78400, 12845056, 0, false, 1600, 900, 1596, 896),
        (28, 78400, 12845056, 0, false, 6016, 3384, 4760, 2688),
        (28, 78400, 12845056, 0, false, 100, 50, 420, 224),
        (28, 78400, 12845056, 0, false, 13, 7, 392, 224),
        (28, 78400, 12845056, 0, false, 1920, 1200, 1932, 1204),
        (16, 0, 0, 1024, false, 1440, 900, 1024, 640),
        (16, 0, 0, 1024, false, 1920, 1080, 1024, 576),
        (16, 0, 0, 1024, false, 2560, 1440, 1024, 576),
        (16, 0, 0, 1024, false, 3840, 2160, 1024, 576),
        (16, 0, 0, 1024, false, 1280, 800, 1024, 640),
        (16, 0, 0, 1024, false, 1366, 768, 1024, 576),
        (16, 0, 0, 1024, false, 2880, 1800, 1024, 640),
        (16, 0, 0, 1024, false, 3024, 1964, 1024, 672),
        (16, 0, 0, 1024, false, 3456, 2234, 1024, 672),
        (16, 0, 0, 1024, false, 5120, 2880, 1024, 576),
        (16, 0, 0, 1024, false, 1680, 1050, 1024, 640),
        (16, 0, 0, 1024, false, 1024, 768, 1024, 768),
        (16, 0, 0, 1024, false, 800, 600, 1024, 768),
        (16, 0, 0, 1024, false, 3440, 1440, 1024, 432),
        (16, 0, 0, 1024, false, 2560, 1600, 1024, 640),
        (16, 0, 0, 1024, false, 1600, 900, 1024, 576),
        (16, 0, 0, 1024, false, 6016, 3384, 1024, 576),
        (16, 0, 0, 1024, false, 100, 50, 1024, 512),
        (16, 0, 0, 1024, false, 13, 7, 1024, 560),
        (16, 0, 0, 1024, false, 1920, 1200, 1024, 656),
        (14, 0, 1000000, 0, true, 1440, 900, 1260, 784),
        (14, 0, 1000000, 0, true, 1920, 1080, 1330, 742),
        (14, 0, 1000000, 0, true, 2560, 1440, 1330, 742),
        (14, 0, 1000000, 0, true, 3840, 2160, 1330, 742),
        (14, 0, 1000000, 0, true, 1280, 800, 1260, 784),
        (14, 0, 1000000, 0, true, 1366, 768, 1330, 742),
        (14, 0, 1000000, 0, true, 2880, 1800, 1260, 784),
        (14, 0, 1000000, 0, true, 3024, 1964, 1232, 798),
        (14, 0, 1000000, 0, true, 3456, 2234, 1232, 798),
        (14, 0, 1000000, 0, true, 5120, 2880, 1330, 742),
        (14, 0, 1000000, 0, true, 1680, 1050, 1260, 784),
        (14, 0, 1000000, 0, true, 1024, 768, 1022, 770),
        (14, 0, 1000000, 0, true, 800, 600, 798, 602),
        (14, 0, 1000000, 0, true, 3440, 1440, 1540, 644),
        (14, 0, 1000000, 0, true, 2560, 1600, 1260, 784),
        (14, 0, 1000000, 0, true, 1600, 900, 1330, 742),
        (14, 0, 1000000, 0, true, 6016, 3384, 1330, 742),
        (14, 0, 1000000, 0, true, 100, 50, 98, 56),
        (14, 0, 1000000, 0, true, 13, 7, 14, 14),
        (14, 0, 1000000, 0, true, 1920, 1200, 1260, 784),
    ];

    #[test]
    fn target_size_matches_llama_cpp_bit_for_bit() {
        for &(align, min_pixels, max_pixels, longest_edge, round_half_even, w, h, ow, oh) in GOLDEN
        {
            let s = SmartResize {
                align,
                min_pixels,
                max_pixels,
                longest_edge,
                round_half_even,
            };
            assert_eq!(s.target_size(w, h), (ow, oh), "{w}x{h} with {s:?}");
        }
    }

    #[test]
    fn known_published_sizes() {
        // Fara-7B's card: "The screen's resolution is 1428x896" for a 1440x900 screen.
        let q25 = SmartResize::qwen2_vl(8, 4096);
        assert_eq!(q25.target_size(1440, 900), (1428, 896));
        assert_eq!(q25.align, 28);
        assert_eq!(q25.max_pixels, 4096 * 28 * 28);
        // Qwen3-VL: align 32.
        let q3 = SmartResize::qwen3_vl(1024, 4096);
        assert_eq!(q3.align, 32);
        assert_eq!(q3.min_pixels, 1024 * 32 * 32);
        assert_eq!(q3.target_size(1920, 1080), (1920, 1088));
        // Degenerate input.
        assert_eq!(q25.target_size(0, 100), (0, 0));
    }

    #[test]
    fn output_is_aligned_and_within_the_token_budget() {
        let s = SmartResize::qwen2_vl(1024, 4096);
        for &(w, h) in &[
            (1920, 1080),
            (3840, 2160),
            (5120, 1440),
            (1024, 768),
            (800, 600),
        ] {
            let (ow, oh) = s.target_size(w, h);
            assert_eq!(ow % 28, 0);
            assert_eq!(oh % 28, 0);
            assert!(ow * oh <= s.max_pixels, "{w}x{h} -> {ow}x{oh}");
            assert!(ow * oh >= s.min_pixels, "{w}x{h} -> {ow}x{oh}");
        }
    }

    #[test]
    fn rules_and_conventions() {
        assert_eq!(
            ResizeRule::Fixed {
                width: 896,
                height: 896
            }
            .target_size(1920, 1080),
            (896, 896)
        );
        assert_eq!(ResizeRule::Original.target_size(1920, 1080), (1920, 1080));
        assert!(
            Convention::Pixels
                .instructions((1428, 896))
                .contains("0 (left) to 1427")
        );
        assert!(Convention::Grid1000.instructions((1, 1)).contains("0-1000"));
        assert!(
            Convention::Box1000Yx
                .instructions((1, 1))
                .contains("[y1, x1, y2, x2]")
        );
        let p = ModelProfile::default();
        assert_eq!(p.convention, Convention::Pixels);
        // Profiles round-trip through the settings JSON.
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<ModelProfile>(&json).unwrap(), p);
    }

    fn q2() -> ResizeRule {
        ResizeRule::Smart(SmartResize::qwen2_vl(8, 4096))
    }

    fn q3() -> ResizeRule {
        ResizeRule::Smart(SmartResize::qwen3_vl(8, 4096))
    }

    /// The name → profile table (9.3 "Model conventions"): each family named in the module
    /// docs, written the ways file names and remote ids write them.
    #[test]
    fn a_known_family_is_recognised_by_name() {
        use Convention::{Box1000Yx, Grid1000, Pixels};
        let table: &[(&str, Convention, ResizeRule, &str)] = &[
            // Qwen2-VL / Qwen2.5-VL lineage: pixels of the resized image, align 28.
            ("UI-TARS-1.5-7B-Q4_K_M.gguf", Pixels, q2(), "UI-TARS-1.5"),
            ("ui_tars_1.5_7b.q4_k_m.gguf", Pixels, q2(), "UI-TARS-1.5"),
            ("Ui.Tars 1.5 7B", Pixels, q2(), "UI-TARS-1.5"),
            (
                "Qwen2.5-VL-7B-Instruct-Q4_K_M.gguf",
                Pixels,
                q2(),
                "Qwen2-VL",
            ),
            ("QWEN2.5VL-3B", Pixels, q2(), "Qwen2-VL"),
            ("qwen2_5_vl_7b", Pixels, q2(), "Qwen2-VL"),
            ("Qwen2-VL-2B-Instruct-Q8_0.gguf", Pixels, q2(), "Qwen2-VL"),
            ("Fara-7B-Q4_K_M.gguf", Pixels, q2(), "Fara"),
            ("Holo1.5-7B.Q4_K_M.gguf", Pixels, q2(), "Holo1"),
            ("holo-1.5-3b", Pixels, q2(), "Holo1"),
            ("Holo1-7B", Pixels, q2(), "Holo1"),
            // Qwen3-VL, Qwen3.5 and the newer Holo models: a 0-1000 grid, align 32.
            ("Qwen3VL-8B-Instruct-Q4_K_M.gguf", Grid1000, q3(), "Qwen3"),
            ("qwen3-vl:8b", Grid1000, q3(), "Qwen3"),
            ("Qwen3.5-9B-Q4_K_M.gguf", Grid1000, q3(), "Qwen3"),
            ("Holo2-8B-Q4_K_M.gguf", Grid1000, q3(), "Holo"),
            ("Holo-3.1-35B-A3B", Grid1000, q3(), "Holo"),
            ("HOLO4-27B.gguf", Grid1000, q3(), "Holo"),
            // Built on Qwen3.5, but its card says pixels.
            ("Fara1.5-9B-Q4_K_M.gguf", Pixels, q3(), "Fara1.5"),
            // Gemma: boxes, y first.
            ("gemma-3-12b-it-Q4_K_M.gguf", Box1000Yx, q3(), "Gemma"),
            ("gemma-4-E4B-it-Q4_0.gguf", Box1000Yx, q3(), "Gemma"),
            ("Gemma3:4b", Box1000Yx, q3(), "Gemma"),
            ("medgemma-4b-it", Box1000Yx, q3(), "Gemma"),
        ];
        for &(name, convention, resize, family) in table {
            let auto = auto_profile(name);
            assert_eq!(auto.family, Some(family), "{name}");
            assert!(!auto.is_guess(), "{name}");
            assert_eq!(auto.profile.convention, convention, "{name}");
            assert_eq!(auto.profile.resize, resize, "{name}");
        }
    }

    #[test]
    fn an_unknown_model_gets_the_grid_as_a_guess() {
        for name in [
            "llava-v1.6-mistral-7b.Q4_K_M.gguf",
            "some-new-vision-model-12b",
            "remote:3:big-vision-mini",
            "local:/home/u/models/someone/model.gguf",
            "q4_k_m.gguf",
            "",
            // A short family name inside another word is not that family.
            "hologram-7b.gguf",
            "faraday-12b",
            "wolf-ara-7b",
            "UI-TARS-7B-DPO.gguf",
        ] {
            let auto = auto_profile(name);
            assert!(auto.is_guess(), "{name}: {:?}", auto.family);
            assert_eq!(auto.profile.convention, Convention::Grid1000, "{name}");
            assert_eq!(auto.profile.resize, q3(), "{name}");
        }
    }

    #[test]
    fn model_references_are_read_by_their_name() {
        use Convention::{Grid1000, Pixels};
        let known = |r: &str| {
            let a = auto_profile(r);
            assert!(!a.is_guess(), "{r}");
            a.profile.convention
        };
        // Remote: the provider's number is not part of the name.
        assert_eq!(known("remote:3:qwen2.5-vl-7b"), Pixels);
        assert_eq!(known("remote:12:qwen/qwen3-vl-8b-instruct"), Grid1000);
        assert_eq!(
            known("remote:1:hf.co/bartowski/Qwen2.5-VL-7B-Instruct-GGUF:Q4_K_M"),
            Pixels
        );
        // Local: the file name.
        assert_eq!(
            known(
                "local:/home/u/app/models/Mungert/UI-TARS-1.5-7B-GGUF/UI-TARS-1.5-7B-q4_k_m.gguf"
            ),
            Pixels
        );
        // Some publishers name every file the same; then the folder (the repository) says
        // which model it is. Windows paths too.
        assert_eq!(
            known("local:/home/u/app/models/Hcompany/Holo4-35B-A3B-GGUF/q4_k_m.gguf"),
            Grid1000
        );
        assert_eq!(
            known("local:C:\\Users\\u\\app\\models\\Hcompany\\Holo1.5-7B-GGUF\\q4_k_m.gguf"),
            Pixels
        );
        // The file name wins over the folder, and folders further up are not read: a user
        // or a publisher may be called anything.
        assert_eq!(
            known("local:/models/unsloth/gemma-quants/Qwen3VL-8B-Instruct-Q4_K_M.gguf"),
            Grid1000
        );
        assert!(auto_profile("local:/Users/gemma/Downloads/model.gguf").is_guess());
        assert!(auto_profile("local:/home/holo/qwen3-vl/files/model.gguf").is_guess());
        // A server that reports a file path as the model's id.
        assert_eq!(
            known("remote:2:/srv/models/Qwen/qwen3-vl-8b.gguf"),
            Grid1000
        );
    }

    #[test]
    fn choosing_a_convention_by_hand() {
        use Convention::{Box1000, Grid1000, Pixels};
        // Unknown model: the resize rule follows the convention's lineage, so "pixels"
        // means pixels of what a Qwen2.5-VL projector would be fed.
        let unknown = auto_profile("some-new-vision-model-12b");
        assert_eq!(
            unknown.with_convention(Pixels),
            ModelProfile {
                convention: Pixels,
                resize: q2()
            }
        );
        assert_eq!(unknown.with_convention(Grid1000), unknown.profile);
        assert_eq!(
            unknown.with_convention(Box1000),
            ModelProfile {
                convention: Box1000,
                resize: q3()
            }
        );
        // Known family: its projector is known, so only the convention changes.
        let q3vl = auto_profile("Qwen3VL-8B-Instruct-Q4_K_M.gguf");
        assert_eq!(
            q3vl.with_convention(Pixels),
            ModelProfile {
                convention: Pixels,
                resize: q3()
            }
        );
        let tars = auto_profile("UI-TARS-1.5-7B-Q4_K_M.gguf");
        assert_eq!(
            tars.with_convention(Grid1000),
            ModelProfile {
                convention: Grid1000,
                resize: q2()
            }
        );
    }
}
