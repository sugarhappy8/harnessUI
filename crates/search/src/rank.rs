//! Passages and their ranking.
//!
//! A page's text is cut into passages of about [`PASSAGE_CHARS`] characters on paragraph
//! boundaries, and the passages are scored against the question with BM25. The model is
//! given the best few of each page, not the page.
//!
//! Everything here is a pure function of its input, so the same page and question always
//! give the same passages.

/// The size passages are cut to, in characters.
pub const PASSAGE_CHARS: usize = 700;

/// What stands between two passages of an excerpt that were not next to each other on the
/// page.
pub const GAP: &str = "…";

/// One passage of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Passage {
    /// Its position on the page, from 0.
    pub index: usize,
    pub text: String,
    /// The BM25 score against the question (0 when no word of it occurs).
    pub score: f32,
}

/// Words that say nothing about what a question is about. They are left out of the
/// question's terms (never out of a page).
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "in", "is", "it", "my", "of", "on", "or", "that", "the", "this", "to", "was", "what", "when",
    "where", "which", "why", "with",
];

/// Lower-cased words of `text`: runs of letters and digits. A plural "s" is dropped from
/// words of four letters or more, so "futures" and "future" are the same term; single
/// letters are left out.
pub fn tokenize(text: &str) -> Vec<String> {
    let _ = text;
    unimplemented!()
}

/// Cuts `text` into passages of about `target` characters. Paragraphs (separated by a
/// blank line) are kept whole and grouped; a paragraph longer than the target is cut at
/// sentence ends, and only a sentence longer than the target is cut inside. A short line
/// that reads like a heading stays with the text under it. Together the passages hold all
/// of the text, in order.
pub fn split_passages(text: &str, target: usize) -> Vec<String> {
    let _ = (text, target);
    unimplemented!()
}

/// Every passage with its BM25 score against `query`, best first. Passages with the same
/// score keep their order on the page.
pub fn rank_passages(passages: &[String], query: &str) -> Vec<Passage> {
    let _ = (passages, query);
    unimplemented!()
}

/// The `keep` best passages of `text` for `query`, best first. When no word of the
/// question occurs on the page, these are its first passages.
pub fn best_passages(text: &str, query: &str, keep: usize) -> Vec<Passage> {
    let _ = (text, query, keep);
    unimplemented!()
}

/// Puts passages together as one excerpt of at most `max_chars` characters: the best ones
/// that fit, in the order they have on the page, with [`GAP`] on its own line where text
/// between two of them was left out. The best passage is always there; when it alone is
/// too long it is cut at a word.
pub fn excerpt(passages: &[Passage], max_chars: usize) -> String {
    let _ = (passages, max_chars);
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sentence(word: &str, n: usize) -> String {
        format!("{} ends here.", vec![word; n].join(" "))
    }

    #[test]
    fn tokenize_lowercases_and_splits_identifiers() {
        assert_eq!(
            tokenize("Tokio's spawn_blocking(f) in v1.38!"),
            ["tokio", "spawn", "blocking", "in", "v1", "38"]
        );
        assert_eq!(tokenize("useEffect"), ["useeffect"]);
        assert_eq!(
            tokenize("Futures and classes"),
            ["future", "and", "classe"]
        );
        // Short words keep their "s"; "ss" is not a plural.
        assert_eq!(tokenize("is as process"), ["is", "as", "process"]);
        assert_eq!(tokenize("  \n\t "), Vec::<String>::new());
        assert_eq!(tokenize("naïve Größe"), ["naïve", "größe"]);
    }

    #[test]
    fn a_short_text_is_one_passage() {
        assert_eq!(
            split_passages("One paragraph.\n\nAnother one.", 700),
            ["One paragraph.\n\nAnother one."]
        );
        assert_eq!(split_passages("", 700), Vec::<String>::new());
        assert_eq!(split_passages(" \n\n \n", 700), Vec::<String>::new());
    }

    #[test]
    fn paragraphs_are_grouped_up_to_the_target() {
        let paragraphs: Vec<String> = (0..6).map(|i| sentence(&format!("p{i}"), 70)).collect();
        let text = paragraphs.join("\n\n");
        let passages = split_passages(&text, 700);
        assert_eq!(passages.len(), 2, "three paragraphs fit one passage");
        for p in &passages {
            assert!(p.chars().count() <= 700, "{}", p.chars().count());
        }
        // Nothing is lost and nothing is repeated.
        assert_eq!(passages.join("\n\n"), text);
        // Paragraphs are not cut.
        for p in &paragraphs {
            assert!(passages.iter().any(|passage| passage.contains(p.as_str())));
        }
    }

    #[test]
    fn a_long_paragraph_is_cut_at_sentence_ends() {
        let text = (0..40)
            .map(|i| sentence(&format!("w{i}"), 20))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.len() > 2500);
        let passages = split_passages(&text, 700);
        assert!(passages.len() >= 4);
        for p in &passages {
            assert!(p.chars().count() <= 700, "{}", p.chars().count());
            assert!(p.ends_with("ends here."), "cut inside a sentence: {p:?}");
        }
        assert_eq!(passages.join(" "), text);
    }

    #[test]
    fn text_without_sentences_is_cut_at_words_and_then_anywhere() {
        let words = vec!["word"; 500].join(" ");
        for p in split_passages(&words, 700) {
            assert!(p.chars().count() <= 700);
            assert!(p.starts_with("word") && p.ends_with("word"), "{p:?}");
        }
        let solid = "x".repeat(3000);
        let passages = split_passages(&solid, 700);
        assert_eq!(passages.concat(), solid);
        assert!(passages.iter().all(|p| p.chars().count() <= 700));
        // Characters of several bytes are never cut in half.
        let wide = "é".repeat(1500);
        assert_eq!(split_passages(&wide, 700).concat(), wide);
    }

    #[test]
    fn a_heading_stays_with_the_text_under_it() {
        let first = sentence("alpha", 100);
        let second = sentence("beta", 60);
        let text = format!("{first}\n\nInstalling the package\n\n{second}");
        let passages = split_passages(&text, 700);
        assert_eq!(passages.len(), 2, "{passages:?}");
        assert!(!passages[0].contains("Installing"));
        assert!(passages[1].starts_with("Installing the package\n\nbeta"));
    }

    #[test]
    fn code_keeps_its_lines() {
        let code = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}";
        let text = format!("{}\n\n{code}\n\n{}", sentence("a", 150), sentence("b", 150));
        let passages = split_passages(&text, 700);
        assert!(passages.iter().any(|p| p.contains(code)), "{passages:?}");
    }

    fn page() -> Vec<String> {
        vec![
            "Tokio is an asynchronous runtime for Rust. Tokio provides tasks, timers and IO."
                .to_string(),
            "The spawn_blocking function runs a closure on a thread where blocking is fine."
                .to_string(),
            "Tokio tasks are cheap. Tokio tasks are scheduled by the runtime.".to_string(),
            "Licence: MIT. Copyright the contributors.".to_string(),
        ]
    }

    #[test]
    fn the_passage_about_the_question_ranks_first() {
        let ranked = rank_passages(&page(), "how does spawn_blocking work");
        assert_eq!(ranked.len(), 4);
        assert_eq!(ranked[0].index, 1);
        assert!(ranked[0].score > 0.0);
        // Passages without any word of the question score zero and keep their order.
        let zeros: Vec<usize> = ranked
            .iter()
            .filter(|p| p.score == 0.0)
            .map(|p| p.index)
            .collect();
        assert_eq!(zeros, [0, 2, 3]);
    }

    #[test]
    fn a_rare_word_counts_for_more_than_a_common_one() {
        // "tokio" is in two passages, "blocking" in one.
        let ranked = rank_passages(&page(), "tokio blocking");
        assert_eq!(ranked[0].index, 1);
        assert!(ranked[0].score > ranked[1].score);
        // Scores go down the list.
        assert!(ranked.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn ranking_is_the_same_every_time() {
        let a = rank_passages(&page(), "tokio tasks runtime");
        let b = rank_passages(&page(), "tokio tasks runtime");
        assert_eq!(a, b);
        // Words of the question that say nothing ("the", "of") change nothing.
        let plain = rank_passages(&page(), "tasks");
        let padded = rank_passages(&page(), "what is the tasks of");
        let order = |r: &[Passage]| r.iter().map(|p| p.index).collect::<Vec<_>>();
        assert_eq!(order(&plain), order(&padded));
    }

    #[test]
    fn a_question_with_no_word_on_the_page_gets_the_start_of_the_page() {
        let text = page().join("\n\n");
        let best = best_passages(&text, "kubernetes ingress", 2);
        assert_eq!(best.len(), 1, "the short page is one passage");
        assert_eq!(best[0].index, 0);
        let long = (0..10)
            .map(|i| sentence(&format!("w{i}"), 120))
            .collect::<Vec<_>>()
            .join("\n\n");
        let best = best_passages(&long, "kubernetes ingress", 3);
        assert_eq!(
            best.iter().map(|p| p.index).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert!(best_passages("", "x", 3).is_empty());
        assert!(best_passages(&long, "w3", 0).is_empty());
    }

    #[test]
    fn best_passages_keeps_the_best_few() {
        let long = (0..10)
            .map(|i| match i {
                3 => format!("{} One needle.", sentence("w3", 100)),
                7 => sentence("needle", 50),
                _ => sentence(&format!("w{i}"), 120),
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let best = best_passages(&long, "needle", 2);
        assert_eq!(best.len(), 2);
        assert!(best[0].text.starts_with("needle needle"), "{best:?}");
        assert!(best[1].text.contains("One needle."), "{best:?}");
        assert!(best[0].score > best[1].score);
        assert!(best[1].score > 0.0);
    }

    fn passage(index: usize, text: &str, score: f32) -> Passage {
        Passage {
            index,
            text: text.to_string(),
            score,
        }
    }

    #[test]
    fn an_excerpt_is_in_page_order_and_marks_what_was_left_out() {
        let ranked = [
            passage(4, "fourth", 3.0),
            passage(1, "first", 2.0),
            passage(2, "second", 1.0),
        ];
        assert_eq!(
            excerpt(&ranked, 1000),
            "first\n\nsecond\n\n…\n\nfourth"
        );
        // Only what fits, best first.
        assert_eq!(excerpt(&ranked, 16), "first\n\n…\n\nfourth");
        assert_eq!(excerpt(&ranked, 15), "fourth");
        assert_eq!(excerpt(&ranked, 6), "fourth");
        assert_eq!(excerpt(&[], 100), "");
    }

    #[test]
    fn the_best_passage_is_cut_at_a_word_when_it_alone_is_too_long() {
        let ranked = [passage(0, "one two three four five six", 1.0)];
        let cut = excerpt(&ranked, 14);
        assert_eq!(cut, "one two three…");
        assert!(cut.chars().count() <= 14);
        assert_eq!(excerpt(&ranked, 0), "");
    }
}
