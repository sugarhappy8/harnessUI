//! The small, always-on prompt check for local runs (0.1.0): a prompt that pairs a sexual word
//! with a word for children is refused ("Studio won't make this."), never retried, with no way
//! round it. It is a narrow check, not a filter: it knows only the words in [`SEXUAL`] and
//! [`MINOR`] and the tricks below.
//!
//! The prompt is lower-cased; look-alike letters (accented, full-width, circled, mathematical,
//! small capitals, and Cyrillic and Greek letters that look like Latin ones) become the ASCII
//! letter they look like; invisible characters and combining marks go; and anything else that
//! is not an ASCII letter or digit separates words. So that punctuation and spacing tricks
//! still meet the lists, letters spelled out one by one (`s e x y`, `n.u.d.e`) are read
//! together, and two words in a row are read as a pair (`under-age`, `little girl`). A prompt
//! is refused when a word of [`SEXUAL`] and a word of [`MINOR`] both occur, each as a whole
//! word ("Essex" holds no sexual word). A few words name a sexualised child by themselves, so
//! they are in both lists and refused alone.
//!
//! The check reads the prompt four times, with the same rules each time (rulings S2-safety,
//! S2-attention and S2-attention-2):
//!
//! 1. as typed;
//! 2. as the engine receives it, with the LoRA tags and image markers removed
//!    (`runtime::sd::args::strip_prompt_markers`). Removing a tag joins the text on either side
//!    of it, so `se<lora:a:1>xy` reaches the engine as "sexy", and a tag between two words
//!    breaks a pair rule (`little <lora:a:1> girl`);
//! 3. as the model reads that text through the engine's weighting syntax
//!    (`runtime::sd::args::attention_text`): brackets and `:<number>)` go and the pieces
//!    join, so `se()xy`, `se[]xy`, `se:1)xy` and `(se)(xy)` all read "sexy". The pieces are
//!    joined across weights too, as an LLM encoder reads them side by side;
//! 4. as the engine's own segments (`runtime::sd::args::parse_prompt_attention`), one word
//!    apart: it joins only pieces of equal weight and encodes each segment on its own (CLIP
//!    ends each with `</w>`, T5 starts each with `▁`), so `se()xy(children)` reaches the model
//!    as "sexy" and "children", which reading 3 glues into "sexychildren".
//!
//! The fourth reading costs the engine's weighting, which grows with the square of the
//! brackets, so it reads a prompt of at most `LOCAL_PROMPT_MAX_CHARS` (16,384) characters and
//! no longer: [`refuses`] expects its caller to have refused a longer prompt first, as the
//! local lane does (`studio_prompt_too_long_here`). At that length the worst bracket nesting
//! is read in about 0.2 s (ruling G3-safety-bound); the 4,000-character cap of 0.1.x is gone,
//! and every prompt up to the bound is read all four ways. The negative prompt is not
//! checked (since 0.1.x: "no children"-style negatives would be refused), and is bounded the
//! same way.
//!
//! The lists trade misses against harmless prompts refused. Bare "baby" and "minor" are left
//! out of [`MINOR`] because they refuse too much that is harmless ("a sexy baby-blue dress",
//! "baby doll lingerie", "a sexy saxophone solo in A minor", "sexy dress, minor alterations"),
//! so "a sexy baby" passes while "sexy babies", "infant" and "minors" are still caught. Harmless
//! prompts known to be refused still, for whoever tunes the lists:
//!
//! - "a birthday card to the kids, love xxx" ("xxx" is kept for what it usually means);
//! - "kids looking at the moon with the naked eye";
//! - "nude-colour ballet shoes for kids";
//! - "a sexy sports car poster in a teenager's bedroom";
//! - "the naked emperor in a children's book".

use std::collections::BTreeSet;

use runtime::sd::args::{
    LOCAL_PROMPT_MAX_CHARS, attention_text, parse_prompt_attention, strip_prompt_markers,
};

/// Sexual words, as they read after folding (see the module's notes).
const SEXUAL: &[&str] = &[
    "sex",
    "sexy",
    "sexier",
    "sexiest",
    "sexual",
    "sexually",
    "sexualised",
    "sexualized",
    "seductive",
    "seductively",
    "sensual",
    "erotic",
    "erotica",
    "porn",
    "porno",
    "pornographic",
    "pornography",
    "nsfw",
    "lewd",
    "hentai",
    "ecchi",
    "xxx",
    "nude",
    "nudes",
    "nudity",
    "naked",
    "topless",
    "undressed",
    "undressing",
    "unclothed",
    "lingerie",
    "panties",
    "fetish",
    "bdsm",
    "bondage",
    "horny",
    "aroused",
    "orgasm",
    "masturbating",
    "masturbation",
    "intercourse",
    "fuck",
    "fucking",
    "fucked",
    "blowjob",
    "genitals",
    "genitalia",
    "penis",
    "vagina",
    "boobs",
    "busty",
    "cleavage",
    "stripper",
    "striptease",
    "rape",
    "raped",
    "molested",
    "molesting",
    "sexting",
    "onlyfans",
    "lolicon",
    "shotacon",
    "jailbait",
];

/// Words for children, as they read after folding; two words are matched as a pair in a row.
const MINOR: &[&str] = &[
    "child",
    "children",
    "childs",
    "kid",
    "kids",
    "kiddie",
    "kiddies",
    "kiddo",
    "minors",
    "underage",
    "under age",
    "preteen",
    "preteens",
    "teen",
    "teens",
    "teenage",
    "teenaged",
    "teenager",
    "teenagers",
    "tween",
    "tweens",
    "toddler",
    "toddlers",
    "infant",
    "infants",
    "babies",
    "newborn",
    "newborns",
    "juvenile",
    "juveniles",
    "pubescent",
    "prepubescent",
    "schoolgirl",
    "schoolgirls",
    "schoolboy",
    "schoolboys",
    "school girl",
    "school girls",
    "school boy",
    "school boys",
    "little girl",
    "little girls",
    "little boy",
    "little boys",
    "young girl",
    "young girls",
    "young boy",
    "young boys",
    "kindergartener",
    "kindergartner",
    "preschooler",
    "preschoolers",
    "loli",
    "lolis",
    "lolita",
    "shota",
    "lolicon",
    "shotacon",
    "jailbait",
];

/// Letters that look like an ASCII letter, after lower-casing, with the letter.
const LOOK_ALIKES: [(&str, char); 25] = [
    ("àáâãäåāăąǎαаᴀ", 'a'),
    ("ƀβвʙ", 'b'),
    ("çćĉċčϲсᴄ", 'c'),
    ("ďđԁᴅ", 'd'),
    ("èéêëēĕėęěεеёᴇ", 'e'),
    ("ĝğġģɢ", 'g'),
    ("ĥħнһʜ", 'h'),
    ("ìíîïĩīĭįıǐιіїɪ", 'i'),
    ("ĵјᴊ", 'j'),
    ("ķκкᴋ", 'k'),
    ("ĺļľŀłʟ", 'l'),
    ("мᴍ", 'm'),
    ("ñńņňηпɴ", 'n'),
    ("òóôõöøōŏőǒοоᴏ", 'o'),
    ("ρрᴘ", 'p'),
    ("ԛ", 'q'),
    ("ŕŗřгʀ", 'r'),
    ("śŝşšѕꜱ", 's'),
    ("ţťŧτтᴛ", 't'),
    ("ùúûüũūŭůűųǔυᴜ", 'u'),
    ("νѵᴠ", 'v'),
    ("ŵԝᴡ", 'w'),
    ("χх", 'x'),
    ("ýÿŷуүʏ", 'y'),
    ("źżžᴢ", 'z'),
];

/// Whether `prompt` pairs a sexual word with a word for children, as typed, as the engine
/// receives it ([`strip_prompt_markers`]), as the model reads that joined
/// ([`attention_text`]), or as the engine's segments one word apart ([`segment_words`]). The
/// last reading is made only for a prompt of at most [`LOCAL_PROMPT_MAX_CHARS`] characters,
/// which the caller checks first (see the module's notes).
pub fn refuses(prompt: &str) -> bool {
    let received = strip_prompt_markers(prompt);
    pairs(prompt)
        || pairs(&received)
        || pairs(&attention_text(&received))
        || (prompt.chars().count() <= LOCAL_PROMPT_MAX_CHARS && pairs(&segment_words(&received)))
}

/// The segments the engine encodes from `text` ([`parse_prompt_attention`]), joined with one
/// space: its own merge of equal weights, each segment a word apart from the next.
fn segment_words(text: &str) -> String {
    parse_prompt_attention(text)
        .into_iter()
        .map(|(segment, _)| segment)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `text` pairs a sexual word with a word for children: the rules of the module's
/// notes, on one reading of the prompt.
fn pairs(text: &str) -> bool {
    let tokens = tokens(&folded(text));
    let mut words: BTreeSet<String> = tokens.windows(2).map(|pair| pair.join(" ")).collect();
    words.extend(spelled_out(&tokens));
    words.extend(tokens);
    let has = |list: &[&str]| list.iter().any(|word| words.contains(*word));
    has(SEXUAL) && has(MINOR)
}

/// `prompt` lower-cased, look-alike letters as their ASCII letter, and invisible characters
/// and combining marks gone.
fn folded(prompt: &str) -> String {
    prompt
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| !invisible(*c))
        .map(|c| look_alike(c).unwrap_or(c))
        .collect()
}

/// Characters that show nothing: zero-width ones, direction marks, variation selectors and
/// combining marks.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{FEFF}'
            | '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// The ASCII letter or digit `c` looks like, if any.
fn look_alike(c: char) -> Option<char> {
    let from = |base: u8, offset: u32| char::from(base + offset as u8);
    let cp = u32::from(c);
    match cp {
        // Full-width digits and letters.
        0xFF10..=0xFF19 => Some(from(b'0', cp - 0xFF10)),
        0xFF41..=0xFF5A => Some(from(b'a', cp - 0xFF41)),
        // Circled and parenthesised letters.
        0x24D0..=0x24E9 => Some(from(b'a', cp - 0x24D0)),
        0x249C..=0x24B5 => Some(from(b'a', cp - 0x249C)),
        // Mathematical letters (alphabets of 26 capitals then 26 small letters) and digits.
        0x1D400..=0x1D6A3 => Some(from(b'a', (cp - 0x1D400) % 26)),
        0x1D7CE..=0x1D7FF => Some(from(b'0', (cp - 0x1D7CE) % 10)),
        _ => LOOK_ALIKES
            .iter()
            .find(|(set, _)| set.contains(c))
            .map(|(_, letter)| *letter),
    }
}

/// The words of `text`: runs of ASCII letters and digits.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The listed words found inside runs of one-letter words read together: `a s e x y` reads
/// as "asexy", which holds "sexy".
fn spelled_out(tokens: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for run in tokens
        .split(|t| t.len() != 1)
        .filter(|run| run.len() > 1)
        .map(|run| run.concat())
    {
        let inside = SEXUAL.iter().chain(MINOR).filter(|w| run.contains(**w));
        found.extend(inside.map(|w| (*w).to_owned()));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_refuses_sexual_words_paired_with_words_for_children() {
        let refused = [
            "sexy child",
            "SeXy ChIlD in a garden",
            "nude kids on a beach",
            "n.u.d.e child",
            "s e x y teen",
            "naked toddler",
            "erotic photo of minors",
            "lewd loli",
            "hentai schoolgirl",
            "porn with children",
            "se\u{200B}xy ch\u{200B}ild",
            "\u{FF53}\u{FF45}\u{FF58}\u{FF59}\u{3000}\u{FF43}\u{FF48}\u{FF49}\u{FF4C}\u{FF44}",
            "\u{0455}\u{0435}\u{0445}\u{0443} \u{0441}hild",
            "n\u{00FC}de k\u{00ED}ds",
            "nu\u{0301}de child",
            "topless\u{2014}preteen",
            "sexy_child",
            "child-in-lingerie",
            "\u{1D42C}\u{1D41E}\u{1D431}\u{1D432} \u{1D424}\u{1D422}\u{1D41D}",
            "a kid's sexy pose",
            "Infant, nude.",
            "s*e*x*y underage",
            "nsfw\t\n   teenager",
            "a sexy little girl",
            "under-age porno",
            "a s e x y infant",
            "\u{24E2}\u{24D4}\u{24E7}\u{24E8} \u{24DA}\u{24D8}\u{24D3}",
            "SEXY SCHOOL-GIRL",
        ];
        let passed = [
            "a child's birthday party",
            "a sexy sports car",
            "school portrait of a class",
            "kids playing football in the park at sunset",
            "a baby elephant walking through the savanna",
            "a classical nude statue in a marble museum hall",
            "a sexy red evening dress on a mannequin",
            "children reading books in a cozy library",
            "a teenage astronaut floating above the earth",
            "a toddler hugging a teddy bear",
            "Essex countryside with children flying kites",
            "Middlesex school class photo, 1950s",
            "a 30 year old woman in a sexy dress",
            "a kid goat on a farm",
            "a comic strip about kids and their dog",
            "a naked mole rat in a zoo",
            "a family picnic with kids and grandparents",
            "a mother holding her newborn in a hospital",
            "a children's book illustration of a fox in the snow",
            "the infant Hercules strangling snakes, classical painting",
            "a kindergarten classroom full of colourful toys",
            "a sexy perfume bottle advert, studio lighting",
            "a 5 year warranty sticker on a sexy laptop",
            "teens skateboarding in an empty pool",
            "Sussex babies' swimming class",
            "a sextant on a ship's deck, children watching the stars",
            "a babysitter reading to two children",
            "sexy 18 year old sports car",
            "a sexy bottle of 12 year old whisky on a bar",
            "a sexy 2 year old sports car for sale",
            "a naked oak tree, 5 years old, in winter",
            "a sexy baby-blue dress",
            "baby doll lingerie",
            "a sexy saxophone solo in A minor",
            "sexy dress, minor alterations",
        ];
        assert!(refused.len() >= 20 && passed.len() >= 20);
        for prompt in refused {
            assert!(refuses(prompt), "should be refused: {prompt:?}");
        }
        for prompt in passed {
            assert!(!refuses(prompt), "should pass: {prompt:?}");
        }
    }

    /// The engine receives the prompt without its LoRA tags, and removing one joins the text
    /// around it: what it receives is checked too (ruling S2-safety).
    #[test]
    fn a_lora_tag_cannot_hide_what_the_engine_receives() {
        for prompt in [
            "se<lora:xy chil<lora:dren",
            "se<lora:a:1>xy chi<lora:b:1>ldren",
            "sexy little <lora:a:1> girl",
        ] {
            assert!(
                tokens(&folded(prompt)).len() > 1 && !pairs(prompt),
                "the rules alone pass it: {prompt:?}"
            );
            assert!(pairs(&strip_prompt_markers(prompt)), "{prompt:?}");
            assert!(refuses(prompt), "should be refused: {prompt:?}");
        }
        // a harmless prompt with a tag still passes
        for prompt in [
            "a fox in the snow <lora:style:0.8>",
            "children <lora:watercolour:1> reading in a library",
        ] {
            assert!(!refuses(prompt), "should pass: {prompt:?}");
        }
    }

    /// The model reads the prompt through the engine's weighting syntax, which drops brackets
    /// and `:<number>)` and joins the pieces (ruling S2-attention); an image marker joins the
    /// text around it as a LoRA tag does (ruling S2-image-markers).
    #[test]
    fn the_weighting_syntax_cannot_hide_what_the_model_reads() {
        for prompt in [
            "se()xy chil()dren",
            "se[]xy chil[]dren",
            "se:1)xy chil:1)dren",
            "se(xy:1) chil(dren:1)",
            "nu[]de kids",
            "naked t()ee()n",
            "erotic tod()dler",
            "sexy little(:1) girl",
            "(se)(xy) (chil)(dren)",
            "se<lora:a:1>(xy) chil[]dren",
        ] {
            assert!(
                !pairs(prompt) && !pairs(&strip_prompt_markers(prompt)),
                "the first two readings alone pass it: {prompt:?}"
            );
            assert!(refuses(prompt), "should be refused: {prompt:?}");
        }
        // an image marker is removed as a LoRA tag is: the second reading joins the text
        for prompt in ["se<image:x>xy children", "se<image:x xy chil<image:dren"] {
            assert!(!pairs(prompt), "the rules alone pass it: {prompt:?}");
            assert!(refuses(prompt), "should be refused: {prompt:?}");
        }
        // ordinary weighting stays ordinary
        for prompt in [
            "a (red:1.2) car on a [wet] road",
            "portrait, (soft light), BREAK, city at night",
            "children (playing:1.3) in a [sunny] park",
            "a (sexy:1.2) red sports car, BREAK, studio lighting",
            "(masterpiece), kids' (birthday party:1.1), [balloons]",
        ] {
            assert!(!refuses(prompt), "should pass: {prompt:?}");
        }
    }

    /// The engine encodes segments of different weights one by one, so the words either side
    /// of a weight change stay apart (ruling S2-attention-2): reading 3 glues them into one
    /// non-word, and reading 4 reads them apart.
    #[test]
    fn a_weight_change_cannot_glue_words_out_of_sight() {
        for prompt in [
            "se()xy(children)",
            "(sexy)chil()dren",
            "nu[]de(kids)",
            "(nu)(de)kids",
            "naked(t()een)",
            "erotic(tod()dler)",
            "(nude:1.2)tee()n",
            "sexy little(:1)(girl)",
            "n[]ude kids(B",
            "a ph()oto of nu()de(children) at the beach",
        ] {
            let received = strip_prompt_markers(prompt);
            assert!(
                !pairs(prompt) && !pairs(&received) && !pairs(&attention_text(&received)),
                "the first three readings alone pass it: {prompt:?}"
            );
            assert!(pairs(&segment_words(&received)), "{prompt:?}");
            assert!(refuses(prompt), "should be refused: {prompt:?}");
        }
        assert_eq!(segment_words("se()xy(children)"), "sexy children");
        // ordinary weighting still passes
        for prompt in [
            "a (red:1.2) car on a [wet] road",
            "portrait, (soft light), BREAK, city at night",
            "(masterpiece:1.2), a (cozy:0.8) cabin, [fog], children's (book:1.1) style",
        ] {
            assert!(!refuses(prompt), "should pass: {prompt:?}");
        }
    }

    /// 0.2.0 (ruling G3-safety-bound): the 4,000-character cap is gone, and every local
    /// prompt the lane accepts, up to LOCAL_PROMPT_MAX_CHARS characters, is read all four
    /// ways: a phrase split across weights past the old cap is still refused.
    #[test]
    fn a_weight_change_past_4000_characters_is_still_read() {
        let tail = " se()xy(children)";
        let past = format!("{}{tail}", "a ".repeat(2_500));
        let longest = format!("{}{tail}", "a".repeat(LOCAL_PROMPT_MAX_CHARS - tail.len()));
        assert_eq!(longest.chars().count(), LOCAL_PROMPT_MAX_CHARS);
        for prompt in [&past, &longest] {
            assert!(prompt.chars().count() > 4_000);
            let received = strip_prompt_markers(prompt);
            assert!(
                !pairs(prompt) && !pairs(&received) && !pairs(&attention_text(&received)),
                "the first three readings alone pass it"
            );
            assert!(refuses(prompt), "the segments' reading refuses it");
        }
    }

    /// The fourth reading costs the engine's weighting, quadratic in the brackets, so it reads
    /// a prompt of at most LOCAL_PROMPT_MAX_CHARS characters: the caller refuses a longer one
    /// first, as the local lane does. Beyond the bound it is not made: this test would take
    /// minutes if it were.
    #[test]
    fn the_segment_reading_is_bounded_by_the_prompt_length() {
        let nested = format!("{}x", "(x".repeat(250_000));
        assert!(!refuses(&nested));
        let tricked = format!("se()xy(children) {}", "a".repeat(LOCAL_PROMPT_MAX_CHARS));
        assert!(tricked.chars().count() > LOCAL_PROMPT_MAX_CHARS);
        assert!(!refuses(&tricked), "read only up to the bound");
        assert!(refuses("se()xy(children)"));
    }

    /// The worst bracket nesting for the engine's weighting, at the longest local prompt: as
    /// many brackets as segments, each bracket over every segment, round ones (weights that
    /// overflow) and square ones (weights that sink into the subnormal floats). Each is read
    /// all four ways in a bounded time, so a crafted prompt cannot make the check slow
    /// (about 0.2 s at most in a release build; the bound here is for an unoptimised test
    /// build on a slow machine). Square brackets cost about what round ones do: on a
    /// processor with a slow path for subnormal floats, a plain `*=` made them about ten
    /// times slower (`times` in `runtime::sd::prompt_view`).
    #[test]
    fn the_worst_bracket_nesting_at_the_longest_local_prompt_is_read_in_time() {
        use std::time::{Duration, Instant};
        let half = LOCAL_PROMPT_MAX_CHARS / 2;
        let quarter = LOCAL_PROMPT_MAX_CHARS / 4;
        let worst = [
            format!("{}{}", "(".repeat(half), "B".repeat(half)),
            format!("{}{}", "[".repeat(half), "B".repeat(half)),
            format!("{}{}", "([".repeat(quarter), "B".repeat(half)),
            "[B".repeat(half),
            format!(
                "{}{}{}",
                "[".repeat(quarter),
                "B".repeat(half),
                "]".repeat(quarter)
            ),
        ];
        let took: Vec<Duration> = worst
            .iter()
            .map(|prompt| {
                assert_eq!(prompt.chars().count(), LOCAL_PROMPT_MAX_CHARS);
                let started = Instant::now();
                assert!(!refuses(prompt));
                started.elapsed()
            })
            .collect();
        for (prompt, took) in worst.iter().zip(&took) {
            assert!(
                *took < Duration::from_secs(5),
                "{:?}…: {took:?}",
                &prompt[..8]
            );
        }
        let (round, square) = (took[0], took[1]);
        assert!(
            square < round * 4 + Duration::from_millis(100),
            "square brackets {square:?}, round ones {round:?}"
        );
    }

    /// The model's reading turns each `BREAK` into a space, where the engine may keep a run
    /// of them as text (`BREAK:1)BREAK`). That can hide no listed word only while no listed
    /// word holds "break".
    #[test]
    fn no_listed_word_holds_break() {
        for word in SEXUAL.iter().chain(MINOR) {
            assert!(!word.contains("break"), "{word}");
        }
    }

    #[test]
    fn one_side_alone_is_never_refused() {
        for word in SEXUAL.iter().chain(MINOR) {
            let alone = SEXUAL.contains(word) && MINOR.contains(word);
            assert_eq!(refuses(word), alone, "{word}");
        }
        assert!(!refuses(""));
    }

    #[test]
    fn every_listed_word_can_be_matched() {
        // A word is matched after folding: lower-case ASCII letters, one space between two.
        for word in SEXUAL.iter().chain(MINOR) {
            let parts: Vec<_> = word.split(' ').collect();
            assert!(parts.len() <= 2, "{word}");
            assert!(
                parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase())),
                "{word}"
            );
        }
    }
}
