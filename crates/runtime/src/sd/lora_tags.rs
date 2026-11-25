//! LoRAs in the Studio engine's prompt file (docs/dev/0.2.0/design.md 8): the tag the engine
//! reads each from, the names it reads back, and the removal of the tags typed into a
//! prompt. Re-exported from [`super::args`], whose `prompt_file_text` and `build_args` use
//! them.
//!
//! The engine finds a LoRA in the prompt with the pattern `<lora:([^:>]+):([^>]+)>`, the name
//! taken relative to `--lora-model-dir` and `|high_noise|` before it naming the high-noise
//! part (`extract_and_remove_lora`, `examples/common/common.cpp:2352` at the pinned commit).
//! It reads the weight with `std::stof`, and removes each tag it finds from the prompt.

use std::ops::RangeInclusive;

/// The most LoRAs one run takes: a bound against pathological drafts, well above any real use.
pub const MAX_LORAS: usize = 8;

/// A LoRA's strength: the slider's range (Task S11's `ParamKind::Loras` reads these three
/// constants, one source).
pub const LORA_WEIGHT_RANGE: RangeInclusive<f32> = -1.0..=2.0;

/// A LoRA's strength slider's step.
pub const LORA_WEIGHT_STEP: f32 = 0.05;

/// A LoRA's strength when it is attached.
pub const LORA_WEIGHT_DEFAULT: f32 = 1.0;

/// The start of the engine's LoRA tag, `<lora:([^:>]+):([^>]+)>` (`extract_and_remove_lora`,
/// `examples/common/common.cpp` at the pinned commit).
const LORA_TAG_OPEN: &str = "<lora:";

/// The prefix a tag's name takes for the high-noise part of a two-model video family.
const HIGH_NOISE_PREFIX: &str = "|high_noise|";

/// One attached LoRA, as its tag in the prompt file names it.
#[derive(Debug, Clone, PartialEq)]
pub struct LoraTag {
    /// Its file's name relative to the LoRA folder (`--lora-model-dir`), with or without its
    /// extension: [`valid_lora_name`].
    pub name: String,
    /// Its strength, in [`LORA_WEIGHT_RANGE`].
    pub weight: f32,
    /// For the high-noise part of a two-model video family (`<lora:|high_noise|NAME:W>`).
    pub high_noise: bool,
}

/// The tag the engine reads `tag` from: `<lora:NAME:W>`, or `<lora:|high_noise|NAME:W>` for
/// the high-noise part, with W as Rust prints the `f32` (`0.8`, `1`, `-0.25`), which the
/// engine's `std::stof` reads back.
pub fn lora_tag(tag: &LoraTag) -> String {
    let prefix = if tag.high_noise {
        HIGH_NOISE_PREFIX
    } else {
        ""
    };
    format!("{LORA_TAG_OPEN}{prefix}{}:{}>", tag.name, tag.weight)
}

/// `prompt` without any LoRA tag: every span the engine's pattern
/// `<lora:([^:>]+):([^>]+)>` matches is removed, as the engine finds them, and then every
/// `<lora:` left, including one that removing another joins together (`<lo<lora:ra:`). A
/// `<lora:` left over would make a tag with what follows it: with the tags
/// [`prompt_file_text`](super::args::prompt_file_text) appends (`<lora:b:3` and
/// ` <lora:x:1>` read as `b` at 3), or with the rest of a tag a span removed
/// (`<lo<lora:a:1>ra:b:3>`). The text that comes back holds no `<lora:` at all, so the engine
/// finds only the tags Studio appends. Both passes are linear in the prompt's length.
pub fn strip_lora_tags(prompt: &str) -> String {
    let mut kept = String::with_capacity(prompt.len());
    for c in remove_lora_spans(prompt).chars() {
        kept.push(c);
        if kept.ends_with(LORA_TAG_OPEN) {
            kept.truncate(kept.len() - LORA_TAG_OPEN.len());
        }
    }
    kept
}

/// Whether the engine reads `name` back from `<lora:NAME:W>` as the file `name` names under
/// the LoRA folder: not empty; relative (no leading `/`, and no `:`, so no Windows drive);
/// `/` its only separator; none of `:` `<` `>` `|` `\`, nor a control character; and no
/// empty, `.` or `..` component, so it stays in the folder. Spaces and any other Unicode are
/// kept.
///
/// This is the authority on LoRA names. The store's `NOT_IN_A_NAME`
/// (`crates/store/src/gen_loras.rs`) refuses `:` `<` `>` `|` and an empty name when a LoRA is
/// saved; keep the two in step.
pub fn valid_lora_name(name: &str) -> bool {
    const NOT_IN_A_NAME: [char; 5] = [':', '<', '>', '|', '\\'];
    !name.is_empty()
        && !name.contains(NOT_IN_A_NAME)
        && !name.chars().any(char::is_control)
        && name
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

/// `text` without the spans the engine's LoRA pattern matches, found as `std::regex_search`
/// finds them: the leftmost first, then on after it. No span ends after the last `>`, so the
/// search stops there; before it, a name stops at the next `<lora:`'s colon at the latest and
/// a weight at a `>`, so each byte is read a bounded number of times.
fn remove_lora_spans(text: &str) -> String {
    let Some(last_close) = text.rfind('>') else {
        return text.to_owned();
    };
    let mut kept = String::with_capacity(text.len());
    let (mut copied, mut from) = (0, 0);
    while let Some(found) = text
        .get(from..last_close)
        .and_then(|before| before.find(LORA_TAG_OPEN))
    {
        let at = from + found;
        match lora_span_end(&text[at..]) {
            Some(end) => {
                kept.push_str(&text[copied..at]);
                copied = at + end;
                from = copied;
            }
            None => from = at + LORA_TAG_OPEN.len(),
        }
    }
    kept.push_str(&text[copied..]);
    kept
}

/// The length of the tag `<lora:([^:>]+):([^>]+)>` at the start of `text`, if one is there.
/// Every byte the pattern stops at is ASCII, so the length falls on a character boundary.
fn lora_span_end(text: &str) -> Option<usize> {
    let after_open = text.strip_prefix(LORA_TAG_OPEN)?;
    let name_len = after_open.find([':', '>'])?;
    let after_name = &after_open[name_len..];
    if name_len == 0 || !after_name.starts_with(':') {
        return None;
    }
    let weight_len = after_name[1..].find('>')?;
    (weight_len > 0).then(|| LORA_TAG_OPEN.len() + name_len + 1 + weight_len + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lora_tag_names_its_part_and_weight_as_the_engine_reads_them() {
        let tag = |name: &str, weight: f32, high_noise: bool| {
            lora_tag(&LoraTag {
                name: name.into(),
                weight,
                high_noise,
            })
        };
        assert_eq!(tag("x", 1.0, false), "<lora:x:1>");
        assert_eq!(tag("a/b c", -0.25, false), "<lora:a/b c:-0.25>");
        assert_eq!(
            tag("o/r/日本語 スタイル", 0.05, true),
            "<lora:|high_noise|o/r/日本語 スタイル:0.05>"
        );
        assert_eq!(
            (LORA_WEIGHT_RANGE, LORA_WEIGHT_STEP, LORA_WEIGHT_DEFAULT),
            (-1.0..=2.0, 0.05, 1.0)
        );
    }

    /// The engine finds every span its pattern matches, so no `<lora:` the person typed may
    /// be left, alone or joined with what is around it or with the tags Studio appends.
    #[test]
    fn no_typed_lora_tag_survives_the_strip() {
        for (typed, kept) in [
            ("<lora:a:1>", ""),
            ("x <lora:a:1><lora:b:2> y", "x  y"),
            // a span removed joins its two sides into a new opener
            ("<lo<lora:a:1>ra:evil:3>", "evil:3>"),
            ("<lo<lora:ra:x", "x"),
            ("<lo<lora:ra:x:1>", "<lo"),
            ("<lora:<lora:a:1>b:1>", "b:1>"),
            // openers without a whole tag: an appended tag would close them
            ("a fox <lora:evil:3", "a fox evil:3"),
            ("a fox <lora:evil", "a fox evil"),
            ("<lora:>x", ">x"),
            ("<lora::1>", ":1>"),
            // the weight may hold anything but `>`, a line break too
            ("<lora:a:\n1>z", "z"),
            ("<LORA:a:1>", "<LORA:a:1>"),
            ("風 <lora:日本:1> 景", "風  景"),
        ] {
            let stripped = strip_lora_tags(typed);
            assert_eq!(stripped, kept, "{typed:?}");
            assert!(!stripped.contains("<lora:"), "{typed:?}");
        }
    }

    /// A prompt built to make a repeat-until-stable strip, or a scan to the end for each
    /// `<lora:`, take quadratic time is stripped in one pass each (a few milliseconds here;
    /// minutes if either came back).
    #[test]
    fn a_pathological_prompt_strips_in_linear_time() {
        let n = 100_000;
        // openers that never close, each name ended by the next opener's colon
        let open = "<lora:a:".repeat(n);
        assert_eq!(strip_lora_tags(&open), "a:".repeat(n));
        // openers with empty names, one `>` at the very end
        let empty = format!("{}>", "<lora::".repeat(n));
        assert_eq!(strip_lora_tags(&empty), format!("{}>", ":".repeat(n)));
        // nested joins, each removal making a new opener
        let nested = format!("{}<lora:a:1>{}", "<lo".repeat(n), "ra:".repeat(n));
        assert_eq!(strip_lora_tags(&nested), "");
    }
}
