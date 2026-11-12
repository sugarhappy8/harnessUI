//! Sources (app 0.0.4): what web searches retrieved for a chat, numbered so an answer can
//! cite `[n]` and the user can check the citation.
//!
//! - A source belongs to one chat and is deleted with it. Numbers start at 1 in every
//!   chat and are never reused or changed: an address the chat already has keeps its
//!   number when a later search finds it again.
//! - The stored text is what the model was given, nothing more: the search service's
//!   summary (`snippet`) and the passages of the page (`excerpt`). When the model is given
//!   more of a page later, it is added to the excerpt, so the user always sees everything
//!   an answer could rest on.
//! - Every value is text from the web. It is stored and returned as it is; the app
//!   sanitises it before storing and the UI shows it as plain text.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::chats::ensure_chat;
use crate::error::StoreError;
use crate::ids::{ChatId, now_ms};

/// What stands between two parts of an excerpt that were not next to each other on the
/// page. New text is added to a stored excerpt with it.
pub const EXCERPT_GAP: &str = "\n\n…\n\n";

/// An excerpt kept per source at most, in characters. Text given to the model beyond it is
/// not added (the stored excerpt then ends with a note saying so).
pub const MAX_EXCERPT_CHARS: usize = 60_000;
/// A snippet kept per source at most, in characters.
pub const MAX_SNIPPET_CHARS: usize = 2_000;
/// A title kept at most, in characters.
pub const MAX_SOURCE_TITLE_CHARS: usize = 300;
/// An address kept at most, in characters. A longer one is not stored.
pub const MAX_SOURCE_URL_CHARS: usize = 2_000;
/// The searches remembered per source at most, in characters.
pub const MAX_QUERIES_CHARS: usize = 1_000;
/// Sources kept per chat. A search that would go past it stores nothing new.
pub const MAX_SOURCES_PER_CHAT: usize = 1_000;

/// The line a stored excerpt ends with when more was given than is kept.
pub const EXCERPT_FULL_NOTE: &str = "[More of this page was read than is kept here.]";

/// One hit of a search, as it was given to the model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewSource {
    /// The page's address, without a fragment.
    pub url: String,
    /// Its title.
    pub title: String,
    /// The host, without `www.`.
    pub site: String,
    /// The search service's summary.
    pub snippet: String,
    /// The passages of the page the model was given. Empty when the page was not read.
    pub excerpt: String,
    /// SHA-256 of the page's extracted text. `None` when the page was not read.
    pub content_sha256: Option<String>,
    /// Official documentation.
    pub official: bool,
    /// "matches tokio 1.38", "for tokio 0.2".
    pub version_note: Option<String>,
    /// When the page was read, in Unix ms. `None` when it was not read.
    pub fetched_at: Option<i64>,
}

/// A row of `sources`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// Row id.
    pub id: i64,
    /// The chat it belongs to.
    pub chat_id: ChatId,
    /// The number an answer cites it by, from 1.
    pub n: u32,
    /// The page's address.
    pub url: String,
    /// Its title.
    pub title: String,
    /// The host, without `www.`.
    pub site: String,
    /// The search service's summaries the model was given.
    pub snippet: String,
    /// Everything of the page the model was given.
    pub excerpt: String,
    /// SHA-256 of the page's extracted text at the last read.
    pub content_sha256: Option<String>,
    /// Official documentation.
    pub official: bool,
    /// Whether the page is for the library version the project uses.
    pub version_note: Option<String>,
    /// The searches that found it, one per line, oldest first.
    pub query: String,
    /// When the page was last read, in Unix ms.
    pub fetched_at: Option<i64>,
    /// When the source was first found, in Unix ms.
    pub created_at: i64,
}

const COLUMNS: &str = "id, chat_id, n, url, title, site, snippet, excerpt, content_sha256, \
                       official, version_note, query, fetched_at, created_at";

fn from_row(r: &Row<'_>) -> rusqlite::Result<Source> {
    Ok(Source {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        n: r.get(2)?,
        url: r.get(3)?,
        title: r.get(4)?,
        site: r.get(5)?,
        snippet: r.get(6)?,
        excerpt: r.get(7)?,
        content_sha256: r.get(8)?,
        official: r.get(9)?,
        version_note: r.get(10)?,
        query: r.get(11)?,
        fetched_at: r.get(12)?,
        created_at: r.get(13)?,
    })
}

/// The first `max` characters of `text`.
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// `have` with the parts of `more` it does not hold yet, parts being what `gap` separates.
/// The result has at most `max` characters. A part that does not fit is left out whole
/// (only a first part is cut), and the result then ends with `full_note`; nothing is added
/// to a text that ends so.
pub fn merge_text(have: &str, more: &str, gap: &str, max: usize, full_note: &str) -> String {
    let mut out = have.trim().to_string();
    let note_chars = if full_note.is_empty() {
        0
    } else {
        full_note.chars().count() + 2
    };
    let was_full = !full_note.is_empty() && out.ends_with(full_note);
    let mut left_out = false;
    for part in more.split(gap).map(str::trim).filter(|p| !p.is_empty()) {
        if out.contains(part) {
            continue;
        }
        if was_full {
            return out;
        }
        let between = if out.is_empty() {
            0
        } else {
            gap.chars().count()
        };
        let needed = out.chars().count() + between + part.chars().count();
        if needed <= max {
            if !out.is_empty() {
                out.push_str(gap);
            }
            out.push_str(part);
            continue;
        }
        left_out = true;
        if out.is_empty() {
            out = cut(part, max.saturating_sub(note_chars))
                .trim_end()
                .to_string();
        }
        break;
    }
    if left_out && !full_note.is_empty() {
        if out.chars().count() + note_chars > max {
            out = cut(&out, max.saturating_sub(note_chars))
                .trim_end()
                .to_string();
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(full_note);
    } else if out.chars().count() > max {
        out = cut(&out, max);
    }
    out
}

/// One line of at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    cut(&text.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

fn by_url(conn: &Connection, chat: ChatId, url: &str) -> Result<Option<Source>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM sources WHERE chat_id = ?1 AND url = ?2");
    Ok(conn
        .query_row(&sql, params![chat, url], from_row)
        .optional()?)
}

fn record(
    conn: &mut Connection,
    chat: ChatId,
    query: &str,
    hits: Vec<NewSource>,
) -> Result<Vec<Source>, StoreError> {
    ensure_chat(conn, chat)?;
    let query = one_line(query, MAX_QUERIES_CHARS);
    let now = now_ms();
    let tx = conn.transaction()?;
    let (mut count, mut next): (i64, i64) = tx.query_row(
        "SELECT count(*), COALESCE(MAX(n), 0) + 1 FROM sources WHERE chat_id = ?1",
        [chat],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let limit = i64::try_from(MAX_SOURCES_PER_CHAT).unwrap_or(i64::MAX);
    let mut numbers: Vec<i64> = Vec::new();
    for hit in hits {
        let url = hit.url.trim();
        if url.is_empty() || url.chars().count() > MAX_SOURCE_URL_CHARS {
            continue;
        }
        let version_note = hit.version_note.as_deref().map(|note| one_line(note, 200));
        let content_sha256 = hit.content_sha256.as_deref().map(|hash| cut(hash, 128));
        match by_url(&tx, chat, url)? {
            Some(known) => {
                // The same source as before: what the model is given now is added to what
                // it was given then.
                let snippet = merge_text(&known.snippet, &hit.snippet, "\n", MAX_SNIPPET_CHARS, "");
                let excerpt = merge_text(
                    &known.excerpt,
                    &hit.excerpt,
                    EXCERPT_GAP,
                    MAX_EXCERPT_CHARS,
                    EXCERPT_FULL_NOTE,
                );
                let mut queries = known.query.clone();
                let is_new = !query.is_empty() && !queries.lines().any(|q| q == query);
                if is_new
                    && queries.chars().count() + 1 + query.chars().count() <= MAX_QUERIES_CHARS
                {
                    if !queries.is_empty() {
                        queries.push('\n');
                    }
                    queries.push_str(&query);
                }
                // The hash and the time belong to the last time the page was read.
                let (hash, fetched_at) = match content_sha256 {
                    Some(hash) => (Some(hash), hit.fetched_at.or(known.fetched_at)),
                    None => (known.content_sha256.clone(), known.fetched_at),
                };
                let title = if known.title.is_empty() {
                    one_line(&hit.title, MAX_SOURCE_TITLE_CHARS)
                } else {
                    known.title.clone()
                };
                tx.execute(
                    "UPDATE sources SET title = ?1, snippet = ?2, excerpt = ?3, query = ?4, \
                     official = ?5, version_note = ?6, content_sha256 = ?7, fetched_at = ?8 \
                     WHERE id = ?9",
                    params![
                        title,
                        snippet,
                        excerpt,
                        queries,
                        known.official || hit.official,
                        version_note.or(known.version_note),
                        hash,
                        fetched_at,
                        known.id
                    ],
                )?;
                numbers.push(i64::from(known.n));
            }
            None => {
                if count >= limit {
                    continue;
                }
                tx.execute(
                    "INSERT INTO sources (chat_id, n, url, title, site, snippet, excerpt, \
                     content_sha256, official, version_note, query, fetched_at, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                    params![
                        chat,
                        next,
                        url,
                        one_line(&hit.title, MAX_SOURCE_TITLE_CHARS),
                        one_line(&hit.site, 255),
                        merge_text("", &hit.snippet, "\n", MAX_SNIPPET_CHARS, ""),
                        merge_text(
                            "",
                            &hit.excerpt,
                            EXCERPT_GAP,
                            MAX_EXCERPT_CHARS,
                            EXCERPT_FULL_NOTE
                        ),
                        content_sha256,
                        hit.official,
                        version_note,
                        query,
                        hit.fetched_at,
                        now
                    ],
                )?;
                numbers.push(next);
                next += 1;
                count += 1;
            }
        }
    }
    let mut stored = Vec::with_capacity(numbers.len());
    for n in numbers {
        let sql = format!("SELECT {COLUMNS} FROM sources WHERE chat_id = ?1 AND n = ?2");
        stored.push(tx.query_row(&sql, params![chat, n], from_row)?);
    }
    tx.commit()?;
    Ok(stored)
}

fn extend(
    conn: &mut Connection,
    chat: ChatId,
    n: u32,
    more: &str,
    content_sha256: &str,
    fetched_at: i64,
) -> Result<Option<Source>, StoreError> {
    let Some(known) = get(conn, chat, n)? else {
        return Ok(None);
    };
    let excerpt = merge_text(
        &known.excerpt,
        more,
        EXCERPT_GAP,
        MAX_EXCERPT_CHARS,
        EXCERPT_FULL_NOTE,
    );
    conn.execute(
        "UPDATE sources SET excerpt = ?1, content_sha256 = ?2, fetched_at = ?3 WHERE id = ?4",
        params![excerpt, cut(content_sha256, 128), fetched_at, known.id],
    )?;
    get(conn, chat, n)
}

fn get(conn: &Connection, chat: ChatId, n: u32) -> Result<Option<Source>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM sources WHERE chat_id = ?1 AND n = ?2");
    Ok(conn
        .query_row(&sql, params![chat, n], from_row)
        .optional()?)
}

fn list(conn: &Connection, chat: ChatId) -> Result<Vec<Source>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM sources WHERE chat_id = ?1 ORDER BY n");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([chat], from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Store {
    /// Stores the hits of one search for a chat and returns them with their numbers, in
    /// the order given. An address the chat already has keeps its number: its row takes
    /// the new text in addition to what it holds, and remembers this search as well. A
    /// hit whose address is empty or too long, and hits past [`MAX_SOURCES_PER_CHAT`], are
    /// left out of the result.
    pub async fn record_sources(
        &self,
        chat_id: ChatId,
        query: String,
        hits: Vec<NewSource>,
    ) -> Result<Vec<Source>, StoreError> {
        self.call(move |c| record(c, chat_id, &query, hits)).await
    }

    /// The chat's sources by number.
    pub async fn list_sources(&self, chat_id: ChatId) -> Result<Vec<Source>, StoreError> {
        self.call(move |c| list(c, chat_id)).await
    }

    /// The chat's source number `n`, or `None`. A number of another chat is never found.
    pub async fn get_source(&self, chat_id: ChatId, n: u32) -> Result<Option<Source>, StoreError> {
        self.call(move |c| get(c, chat_id, n)).await
    }

    /// Adds text of the page that the model was given later (`read_source`) to the
    /// source's excerpt, with the hash and time of that read. Returns the source as it
    /// now stands, or `None` when the chat has no such source.
    pub async fn extend_source(
        &self,
        chat_id: ChatId,
        n: u32,
        more: String,
        content_sha256: String,
        fetched_at: i64,
    ) -> Result<Option<Source>, StoreError> {
        self.call(move |c| extend(c, chat_id, n, &more, &content_sha256, fetched_at))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chats::NewChat;

    async fn chat(store: &Store) -> ChatId {
        store
            .create_chat(NewChat {
                title: "t".into(),
                ..NewChat::default()
            })
            .await
            .expect("create chat")
    }

    fn hit(url: &str, title: &str) -> NewSource {
        NewSource {
            url: url.into(),
            title: title.into(),
            site: "example.com".into(),
            snippet: format!("snippet of {title}"),
            ..NewSource::default()
        }
    }

    fn read(url: &str, title: &str, excerpt: &str) -> NewSource {
        NewSource {
            excerpt: excerpt.into(),
            content_sha256: Some("aa11".into()),
            fetched_at: Some(1_000),
            ..hit(url, title)
        }
    }

    #[tokio::test]
    async fn hits_are_numbered_from_one_in_each_chat() {
        let store = Store::open_in_memory().expect("open");
        let (a, b) = (chat(&store).await, chat(&store).await);
        let first = store
            .record_sources(
                a,
                "tokio spawn".into(),
                vec![
                    read("https://docs.rs/tokio", "Tokio docs", "Passage one."),
                    hit("https://blog.example.com/post", "A post"),
                ],
            )
            .await
            .expect("record");
        assert_eq!(first.iter().map(|s| s.n).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(first[0].title, "Tokio docs");
        assert_eq!(first[0].excerpt, "Passage one.");
        assert_eq!(first[0].content_sha256.as_deref(), Some("aa11"));
        assert_eq!(first[0].fetched_at, Some(1_000));
        assert_eq!(first[0].query, "tokio spawn");
        assert_eq!(first[0].chat_id, a);
        // A page that was not read: its summary, no text, no hash, no time.
        assert_eq!(first[1].snippet, "snippet of A post");
        assert_eq!(
            (
                first[1].excerpt.as_str(),
                &first[1].content_sha256,
                first[1].fetched_at
            ),
            ("", &None, None)
        );
        assert!(first[1].created_at > 1_700_000_000_000);

        // The next search goes on counting; another chat starts at 1 again.
        let second = store
            .record_sources(a, "more".into(), vec![hit("https://c.example.com/", "C")])
            .await
            .expect("record");
        assert_eq!(second[0].n, 3);
        let other = store
            .record_sources(
                b,
                "x".into(),
                vec![hit("https://docs.rs/tokio", "Same page")],
            )
            .await
            .expect("record");
        assert_eq!(other[0].n, 1);
        assert_eq!(
            store
                .list_sources(a)
                .await
                .expect("list")
                .iter()
                .map(|s| (s.n, s.url.as_str()))
                .collect::<Vec<_>>(),
            [
                (1, "https://docs.rs/tokio"),
                (2, "https://blog.example.com/post"),
                (3, "https://c.example.com/")
            ]
        );
        assert_eq!(store.list_sources(b).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn an_address_the_chat_has_keeps_its_number() {
        let store = Store::open_in_memory().expect("open");
        let a = chat(&store).await;
        store
            .record_sources(
                a,
                "first search".into(),
                vec![
                    hit("https://a.example.com/", "A"),
                    read("https://docs.rs/tokio", "Tokio docs", "Passage one."),
                ],
            )
            .await
            .expect("record");
        // Found again, read this time with other passages, and a new page next to it. The
        // same address twice in one search is one source.
        let mut again = read(
            "https://docs.rs/tokio",
            "Another title",
            "Passage two.\n\n…\n\nPassage one.",
        );
        again.official = true;
        again.version_note = Some("matches tokio 1.38".into());
        again.content_sha256 = Some("bb22".into());
        again.fetched_at = Some(2_000);
        let second = store
            .record_sources(
                a,
                "second search".into(),
                vec![
                    again.clone(),
                    hit("https://new.example.com/", "New"),
                    again,
                    read("https://a.example.com/", "A", "Now it was read."),
                ],
            )
            .await
            .expect("record");
        assert_eq!(second.iter().map(|s| s.n).collect::<Vec<_>>(), [2, 3, 2, 1]);
        let tokio = &second[0];
        // The first title stays: the number means the same source as before.
        assert_eq!(tokio.title, "Tokio docs");
        // Everything the model was given, each passage once.
        assert_eq!(tokio.excerpt, "Passage one.\n\n…\n\nPassage two.");
        assert_eq!(tokio.query, "first search\nsecond search");
        assert!(tokio.official);
        assert_eq!(tokio.version_note.as_deref(), Some("matches tokio 1.38"));
        assert_eq!(tokio.content_sha256.as_deref(), Some("bb22"));
        assert_eq!(tokio.fetched_at, Some(2_000));
        // A page that could not be read before and is read now.
        let first = &second[3];
        assert_eq!(first.excerpt, "Now it was read.");
        assert_eq!(first.snippet, "snippet of A");
        assert_eq!(store.list_sources(a).await.expect("list").len(), 3);

        // Found a third time without being read: nothing it has is lost.
        let third = store
            .record_sources(
                a,
                "second search".into(),
                vec![hit("https://docs.rs/tokio", "T")],
            )
            .await
            .expect("record");
        assert_eq!(third[0].excerpt, "Passage one.\n\n…\n\nPassage two.");
        assert_eq!(third[0].content_sha256.as_deref(), Some("bb22"));
        assert_eq!(
            third[0].query, "first search\nsecond search",
            "each search once"
        );
        assert!(third[0].official, "once official, it stays marked");
    }

    #[tokio::test]
    async fn more_of_a_page_is_added_to_what_the_model_was_given() {
        let store = Store::open_in_memory().expect("open");
        let (a, b) = (chat(&store).await, chat(&store).await);
        store
            .record_sources(
                a,
                "q".into(),
                vec![read("https://docs.rs/tokio", "T", "Passage one.")],
            )
            .await
            .expect("record");
        let extended = store
            .extend_source(
                a,
                1,
                "Passage three.\n\n…\n\nPassage one.".into(),
                "cc33".into(),
                3_000,
            )
            .await
            .expect("extend")
            .expect("the source");
        assert_eq!(extended.excerpt, "Passage one.\n\n…\n\nPassage three.");
        assert_eq!(extended.content_sha256.as_deref(), Some("cc33"));
        assert_eq!(extended.fetched_at, Some(3_000));
        assert_eq!(store.get_source(a, 1).await.expect("get"), Some(extended));
        // No such number, and never another chat's source.
        assert_eq!(
            store
                .extend_source(a, 2, "x".into(), "h".into(), 1)
                .await
                .expect("extend"),
            None
        );
        assert_eq!(
            store
                .extend_source(b, 1, "x".into(), "h".into(), 1)
                .await
                .expect("extend"),
            None
        );
        assert_eq!(store.get_source(b, 1).await.expect("get"), None);
        assert_eq!(store.get_source(a, 0).await.expect("get"), None);
        assert_eq!(
            store
                .get_source(a, 1)
                .await
                .expect("get")
                .expect("source")
                .excerpt,
            "Passage one.\n\n…\n\nPassage three."
        );
    }

    #[test]
    fn text_is_merged_part_by_part_and_capped() {
        let gap = EXCERPT_GAP;
        let merge = |have: &str, more: &str| merge_text(have, more, gap, 60, "[full]");
        assert_eq!(merge("", "one"), "one");
        assert_eq!(merge("one", ""), "one");
        assert_eq!(merge("one", "one"), "one");
        assert_eq!(merge("one", "two"), "one\n\n…\n\ntwo");
        assert_eq!(
            merge("one\n\n…\n\ntwo", "two\n\n…\n\nthree\n\n…\n\none"),
            "one\n\n…\n\ntwo\n\n…\n\nthree"
        );
        // A part that is inside what is there already is not added again.
        assert_eq!(merge("alpha beta gamma", "beta"), "alpha beta gamma");
        assert_eq!(merge("", "  \n "), "");
        // What does not fit is left out, and the text says so.
        let long = "x".repeat(50);
        let capped = merge(&long, "a part that does not fit any more");
        assert_eq!(capped, format!("{long}\n\n[full]"));
        assert!(capped.chars().count() <= 60);
        assert_eq!(merge(&capped, "another part"), capped, "said once");
        // Even a first text longer than the cap is cut, at a character.
        let wide = merge_text("", &"é".repeat(100), gap, 20, "[full]");
        assert!(
            wide.chars().count() <= 20 && wide.ends_with("[full]"),
            "{wide}"
        );
    }

    #[tokio::test]
    async fn what_is_stored_is_capped_and_bad_hits_are_left_out() {
        let store = Store::open_in_memory().expect("open");
        let a = chat(&store).await;
        let long_url = format!("https://example.com/{}", "u".repeat(MAX_SOURCE_URL_CHARS));
        let stored = store
            .record_sources(
                a,
                "q".repeat(5_000),
                vec![
                    hit("", "no address"),
                    hit(&long_url, "too long an address"),
                    NewSource {
                        url: "https://ok.example.com/".into(),
                        title: "t".repeat(5_000),
                        site: "s".repeat(5_000),
                        snippet: "n".repeat(50_000),
                        excerpt: "e".repeat(200_000),
                        version_note: Some("v".repeat(5_000)),
                        content_sha256: Some("h".repeat(5_000)),
                        ..NewSource::default()
                    },
                ],
            )
            .await
            .expect("record");
        assert_eq!(stored.len(), 1, "the two bad hits are left out");
        let s = &stored[0];
        assert_eq!(s.n, 1);
        assert_eq!(s.title.chars().count(), MAX_SOURCE_TITLE_CHARS);
        assert!(s.site.chars().count() <= 255);
        assert!(s.snippet.chars().count() <= MAX_SNIPPET_CHARS);
        assert!(s.excerpt.chars().count() <= MAX_EXCERPT_CHARS);
        assert!(s.excerpt.ends_with(EXCERPT_FULL_NOTE));
        assert!(s.version_note.as_ref().unwrap().chars().count() <= 200);
        assert!(s.content_sha256.as_ref().unwrap().len() <= 128);
        assert!(s.query.chars().count() <= MAX_QUERIES_CHARS);
    }

    #[tokio::test]
    async fn sources_go_with_their_chat_and_need_one() {
        let store = Store::open_in_memory().expect("open");
        let a = chat(&store).await;
        store
            .record_sources(a, "q".into(), vec![hit("https://a.example.com/", "A")])
            .await
            .expect("record");
        store.delete_chat(a).await.expect("delete");
        assert!(store.list_sources(a).await.expect("list").is_empty());
        assert!(matches!(
            store
                .record_sources(
                    ChatId(9_999),
                    "q".into(),
                    vec![hit("https://a.example.com/", "A")]
                )
                .await,
            Err(StoreError::NotFound { what: "chat", .. })
        ));
        // Nothing to store is nothing stored.
        let b = chat(&store).await;
        assert!(
            store
                .record_sources(b, "q".into(), Vec::new())
                .await
                .expect("record")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_chat_holds_a_limited_number_of_sources() {
        let store = Store::open_in_memory().expect("open");
        let a = chat(&store).await;
        let many: Vec<NewSource> = (0..MAX_SOURCES_PER_CHAT + 5)
            .map(|i| hit(&format!("https://example.com/{i}"), "p"))
            .collect();
        let stored = store
            .record_sources(a, "q".into(), many)
            .await
            .expect("record");
        assert_eq!(stored.len(), MAX_SOURCES_PER_CHAT);
        assert_eq!(stored.last().unwrap().n as usize, MAX_SOURCES_PER_CHAT);
        // A known address is still found; a new one is not added.
        let again = store
            .record_sources(
                a,
                "q2".into(),
                vec![
                    hit("https://example.com/0", "p"),
                    hit("https://example.com/new", "p"),
                ],
            )
            .await
            .expect("record");
        assert_eq!(again.iter().map(|s| s.n).collect::<Vec<_>>(), [1]);
    }
}
