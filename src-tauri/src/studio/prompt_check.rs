//! The small, always-on prompt check for local runs (0.1.0): a prompt that pairs a sexual word
//! with a word for children is refused ("Studio won't make this."), never retried, with no way
//! round it. It is a narrow check, not a filter: it knows only the words in [`SEXUAL`] and
//! [`MINOR`] and the tricks below.
//!
//! The prompt is lower-cased; look-alike letters (accented, full-width, circled, mathematical,
//! small capitals, and Cyrillic and Greek letters that look like Latin ones) become the ASCII
//! letter they look like; invisible characters and combining marks go; and anything else that
//! is not an ASCII letter or digit separates words. The words are then read three ways: as
//! written, and with digits and symbols read as the letters they stand for (`s3xy`, `k!d`, a
//! `1` as an i or as an l). Letters spelled out one by one (`s e x y`, `n.u.d.e`) are read
//! together, and two words in a row are read as a pair (`under-age`, `little girl`). A prompt
//! is refused when a word of [`SEXUAL`] and a word of [`MINOR`] both occur, each as a whole
//! word ("Essex" holds no sexual word), or a sexual word and an age under 18 ("12-year-old",
//! "9yo"). A few words name a sexualised child by themselves, so they are in both lists and
//! refused alone.

use std::collections::BTreeSet;

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
    "minor",
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
    "baby",
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

/// Ages under 18 written as words.
const AGES_IN_WORDS: [&str; 17] = [
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
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

/// Whether `prompt` pairs a sexual word with a word for children, or with an age under 18.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "Task L15's local start checks each prompt")
)]
pub fn refuses(prompt: &str) -> bool {
    let text = folded(prompt);
    let as_written = tokens(&text, None);
    let as_letters = [Some('i'), Some('l')].map(|one| tokens(&text, one));
    let mut words = BTreeSet::new();
    for tokens in std::iter::once(&as_written).chain(&as_letters) {
        words.extend(tokens.windows(2).map(|pair| pair.join(" ")));
        words.extend(spelled_out(tokens));
        words.extend(tokens.iter().cloned());
    }
    let has = |list: &[&str]| list.iter().any(|word| words.contains(*word));
    has(SEXUAL) && (has(MINOR) || names_a_young_age(&as_written))
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

/// The letter a digit or symbol stands for, a `1` standing for `one`.
fn as_letter(c: char, one: char) -> Option<char> {
    Some(match c {
        '0' => 'o',
        '1' => one,
        '3' => 'e',
        '4' => 'a',
        '5' => 's',
        '7' => 't',
        '8' => 'b',
        '9' => 'g',
        '@' => 'a',
        '$' => 's',
        '!' => 'i',
        '|' => 'l',
        '+' => 't',
        _ => return None,
    })
}

/// The words of `text`: runs of ASCII letters and digits. With `one` set, digits and symbols
/// are first read as the letters they stand for (see [`as_letter`]).
fn tokens(text: &str, one: Option<char>) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        let c = one.and_then(|one| as_letter(c, one)).unwrap_or(c);
        if c.is_ascii_alphanumeric() {
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

/// Whether `tokens` (read as written) name an age under 18: a number from 1 to 17, in digits
/// or words, followed by "yo" or "y o", or by "year", "years", "yr", "yrs" or "y" and then
/// "old".
fn names_a_young_age(tokens: &[String]) -> bool {
    // "12yo" and "12yr" read as "12 yo" and "12 yr".
    let mut words: Vec<&str> = Vec::new();
    for t in tokens {
        let digits = t.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && digits < t.len() {
            words.extend([&t[..digits], &t[digits..]]);
        } else {
            words.push(t);
        }
    }
    let young = |w: &str| {
        w.parse::<u32>().is_ok_and(|n| (1..18).contains(&n)) || AGES_IN_WORDS.contains(&w)
    };
    words.windows(3).any(|w| {
        young(w[0])
            && (matches!(w[1], "year" | "years" | "yr" | "yrs" | "y") && w[2] == "old"
                || w[1] == "y" && w[2] == "o")
    }) || words.windows(2).any(|w| young(w[0]) && w[1] == "yo")
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
            "s3xy k1ds",
            "naked toddler",
            "erotic photo of a minor",
            "lewd loli",
            "hentai schoolgirl",
            "porn with children",
            "p0rn ch1ld",
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
            "a s e x y baby",
            "\u{24E2}\u{24D4}\u{24E7}\u{24E8} \u{24DA}\u{24D8}\u{24D3}",
            "$exy k!d",
            "1oli hentai",
            "seductive 12-year-old",
            "naked 9yo",
            "erotic ten year old",
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
        ];
        assert!(refused.len() >= 20 && passed.len() >= 20);
        for prompt in refused {
            assert!(refuses(prompt), "should be refused: {prompt:?}");
        }
        for prompt in passed {
            assert!(!refuses(prompt), "should pass: {prompt:?}");
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
