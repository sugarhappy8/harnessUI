//! File and symbol mentions in the message box (ARCHITECTURE.md 9.3, spec 0.0.4 H).
//!
//! - `search_mentions` answers the picker from the background index of the chat's
//!   approved folders (`tools::symbols::index`). It never names anything outside them.
//! - `resolve_mentions` checks the chips before the composer sends: it returns what each
//!   mention would take (or why it cannot be read), never the file's text.
//! - On send, [`attach`] reads each chip the user chose through the path sandbox
//!   (`tools::symbols::resolve`, the same checks as `read_file`) and stores the excerpt as
//!   a text document of the message. `turns::history_of` turns it into an attachment, so
//!   the model sees it inside the untrusted-data markers and the turn counts as holding
//!   outside content (D9), also after a restart and on Regenerate.
//!
//! Only the chips are resolved. The `@path` text in the message is for people and the
//! model to read and is never parsed back, so a mention typed or pasted by hand, or one
//! that appears in model output or a tool result, reads nothing.

use std::sync::LazyLock;
use std::time::Duration;

use mcp_host::ChatContext;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State};
use tools::symbols::index::{self, Indexes};
use tools::symbols::resolve::{self, MentionRequest, Resolved};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The indexes of the approved folders, shared by every chat.
static INDEXES: LazyLock<Indexes> = LazyLock::new(Indexes::new);
/// How long the first search waits for a small folder to finish indexing.
const FIRST_WAIT: Duration = Duration::from_millis(150);
/// Results returned when the picker asks for none in particular.
const DEFAULT_LIMIT: usize = 30;

/// One picker entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionHit {
    /// `file`, or the kind of the definition (`function`, `class`, ...).
    pub kind: String,
    /// The text of the mention: `src/lib.rs` or `src/lib.rs#parse_args`.
    pub label: String,
    /// The file, absolute.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionSearch {
    pub hits: Vec<MentionHit>,
    /// The index is still being built: asking again may find more.
    pub indexing: bool,
    pub files: usize,
    pub folders: usize,
}

/// A mention as stored with its message and shown as a chip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionRef {
    pub attachment_id: i64,
    pub label: String,
    pub kind: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default)]
    pub truncated: bool,
}

/// What `resolve_mentions` says about one mention.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionCheck {
    pub label: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    pub truncated: bool,
    /// Why it cannot be read; the other fields are then empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The chat's context as the file tools get it in a turn (`turns::run`): approved folders,
/// working folder, forbidden folders and skill folders. With no chat, the global folders.
async fn context_for(
    app: &AppHandle,
    state: &AppState,
    chat_id: Option<store::ChatId>,
) -> AppResult<ChatContext> {
    let (folders, stored_working) = match chat_id {
        Some(id) => {
            let chat = state
                .store
                .get_chat(id)
                .await?
                .ok_or_else(|| AppError::not_found("chat"))?;
            let folders: Vec<String> = state
                .store
                .approved_folders_for_chat(id)
                .await?
                .into_iter()
                .map(|f| f.path)
                .collect();
            (folders, chat.working_folder)
        }
        None => (
            state
                .store
                .list_approved_folders()
                .await?
                .into_iter()
                .filter(|f| f.chat_id.is_none())
                .map(|f| f.path)
                .collect(),
            None,
        ),
    };
    let shared: bool = state
        .store
        .get_setting(crate::toolset::SHARED_SKILLS)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
    let mut skill_roots: Vec<String> = crate::toolset::skills_root(app)
        .map(|r| r.display().to_string())
        .into_iter()
        .collect();
    if shared && let Ok(home) = app.path().home_dir() {
        skill_roots.push(home.join(".agents").join("skills").display().to_string());
    }
    Ok(ChatContext {
        chat_id: chat_id.map_or(0, |c| c.0),
        working_folder: crate::turns::working_folder_for(stored_working.as_deref(), &folders),
        approved_folders: folders,
        forbidden_folders: crate::permissions::forbidden_for_tools(app),
        skill_folders: Vec::new(),
        skill_roots,
        result_cap_tokens: 2000,
        full_access: false,
    })
}

/// Only the main window may search or read mentions (the capability files grant these
/// commands to it alone; this is the same rule checked again here).
fn only_main_window(label: &str) -> AppResult<()> {
    if label == crate::MAIN_WINDOW {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Only the main window can search or read mentions.",
        ))
    }
}

/// The approved folders as the sandbox resolved them.
fn roots_of(ctx: &ChatContext) -> Vec<String> {
    resolve::sandbox(ctx)
        .roots()
        .iter()
        .map(|r| r.canonical.to_string())
        .collect()
}

#[tauri::command]
pub async fn search_mentions(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: Option<store::ChatId>,
    query: String,
    limit: Option<usize>,
) -> AppResult<MentionSearch> {
    only_main_window(window.label())?;
    let ctx = context_for(&app, &state, chat_id).await?;
    let query: String = query.chars().take(200).collect();
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 100);
    tokio::task::spawn_blocking(move || {
        let roots = roots_of(&ctx);
        let indexes = INDEXES.for_roots(&roots, &ctx.forbidden_folders, &ctx.skill_roots);
        for idx in &indexes {
            if idx.snapshot().files.is_empty() {
                idx.wait(FIRST_WAIT);
            }
        }
        let found = index::search(&indexes, &query, limit);
        let working = ctx.working_folder.as_deref();
        let hits = found
            .hits
            .into_iter()
            .map(|h| {
                let path = h.path.display().to_string();
                let file = resolve::display_path(&path, &roots, working);
                match h.symbol {
                    None => MentionHit {
                        kind: "file".into(),
                        label: file,
                        path,
                        symbol: None,
                        container: None,
                        line: None,
                        end_line: None,
                    },
                    Some(s) => MentionHit {
                        kind: s.kind.as_str().into(),
                        label: format!("{file}#{}", s.qualified),
                        path,
                        symbol: Some(s.name),
                        container: s.container,
                        line: Some(s.line),
                        end_line: Some(s.end_line),
                    },
                }
            })
            .collect();
        MentionSearch {
            hits,
            indexing: found.indexing,
            files: found.files,
            folders: indexes.len(),
        }
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))
}

/// [0.0.6] A token count for [`read_all`] when there is none (an endpoint, or only checking).
type NoCount = fn(String) -> std::future::Ready<Option<usize>>;

/// Reads the mentions for `ctx`, each cut to its share of `n_ctx`.
/// [0.0.6] The share counts the message's `documents` too (`resolve::cap_chars`), as a
/// document's share counts the mentions (`attachments::document_cap_chars`): together
/// they carry at most 40% of the context. Then each excerpt is held to `cap / 3` tokens as
/// the model server counts them, when it can count (`count`: llama-server's own tokenizer
/// for a local model; `None` for an endpoint, whose requests the loop measures with the
/// same 3-characters-a-token estimate), as documents are (`attachments::cut_to_share`):
/// numbers and some code take more tokens per character than the estimate, and the
/// current message is never summarised, so it could stop the chat with "too long". An
/// excerpt that is over is read again with fewer characters, in proportion with a tenth
/// to spare (`attachments::fewer_chars`), at most 4 counts, never below the 600-character
/// floor; a count that fails keeps the excerpt as it is. A mention that cannot be read
/// again (changed or removed meanwhile) fails like one that could not be read at all.
async fn read_all<F, Fut>(
    ctx: ChatContext,
    mentions: Vec<MentionRequest>,
    n_ctx: usize,
    documents: usize,
    count: Option<F>,
) -> AppResult<Vec<Result<Resolved, String>>>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Option<usize>>,
{
    let cap = resolve::cap_chars(n_ctx, mentions.len() + documents);
    let mut read = read_at(ctx.clone(), mentions.clone(), cap).await?;
    let Some(count) = count else {
        return Ok(read);
    };
    let allowed = (cap / resolve::CHARS_PER_TOKEN).max(1);
    for (mention, got) in mentions.into_iter().zip(read.iter_mut()) {
        let mut chars = cap;
        for _ in 0..crate::attachments::MAX_COUNTS {
            let Ok(r) = got.as_ref() else {
                break;
            };
            let Some(tokens) = count(r.text.clone()).await else {
                break;
            };
            if tokens <= allowed || chars <= resolve::MIN_CHARS {
                break;
            }
            let sent = r.text.chars().count().min(chars);
            chars =
                crate::attachments::fewer_chars(sent, chars, allowed, tokens, resolve::MIN_CHARS);
            if let Some(again) = read_at(ctx.clone(), vec![mention.clone()], chars)
                .await?
                .pop()
            {
                *got = again;
            }
        }
    }
    Ok(read)
}

/// Reads the mentions for `ctx` in a blocking task, each cut to `cap` characters.
async fn read_at(
    ctx: ChatContext,
    mentions: Vec<MentionRequest>,
    cap: usize,
) -> AppResult<Vec<Result<Resolved, String>>> {
    tokio::task::spawn_blocking(move || {
        mentions
            .iter()
            .map(|m| resolve::resolve(m, &ctx, cap))
            .collect()
    })
    .await
    .map_err(|e| AppError::internal(e.to_string()))
}

fn too_many(n: usize) -> AppResult<()> {
    if n > resolve::MAX_MENTIONS {
        return Err(AppError::bad_request(format!(
            "Mention at most {} files or definitions in one message.",
            resolve::MAX_MENTIONS
        )));
    }
    Ok(())
}

#[tauri::command]
pub async fn resolve_mentions(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    mentions: Vec<MentionRequest>,
) -> AppResult<Vec<MentionCheck>> {
    only_main_window(window.label())?;
    too_many(mentions.len())?;
    let ctx = context_for(&app, &state, Some(chat_id)).await?;
    let n_ctx = crate::settings::DEFAULT_REMOTE_CONTEXT as usize;
    Ok(read_all(ctx, mentions, n_ctx, 0, None::<NoCount>)
        .await?
        .into_iter()
        .map(|r| match r {
            Ok(r) => MentionCheck {
                label: r.label,
                kind: r.kind,
                start_line: r.start_line,
                end_line: r.end_line,
                total_lines: r.total_lines,
                truncated: r.truncated,
                error: None,
            },
            Err(why) => MentionCheck {
                label: String::new(),
                kind: String::new(),
                start_line: 0,
                end_line: 0,
                total_lines: 0,
                truncated: false,
                error: Some(why),
            },
        })
        .collect())
}

/// Reads the chips the user chose for a message and stores each excerpt as a text
/// document of the chat. Returns the content parts for the message. Any mention that
/// cannot be read fails the send with the reason, before anything is stored. [0.0.6] Each
/// excerpt's share of `n_ctx` counts the message's `documents` (attached files) too, and
/// `counter` is the local model server, whose own token counts hold each share
/// ([`read_all`]); `None` for an endpoint.
pub async fn attach(
    app: &AppHandle,
    state: &AppState,
    chat_id: store::ChatId,
    mentions: Vec<MentionRequest>,
    n_ctx: usize,
    documents: usize,
    counter: Option<&agent::OpenAiClient>,
) -> AppResult<Vec<Value>> {
    if mentions.is_empty() {
        return Ok(Vec::new());
    }
    too_many(mentions.len())?;
    let ctx = context_for(app, state, Some(chat_id)).await?;
    let count = counter.map(|client| move |t: String| crate::attachments::server_tokens(client, t));
    let mut resolved = Vec::new();
    for r in read_all(ctx, mentions, n_ctx, documents, count).await? {
        resolved.push(r.map_err(|why| {
            AppError::new(
                "mention_unreadable",
                why,
                "Remove that mention and send again.",
                String::new(),
            )
        })?);
    }
    let mut parts = Vec::new();
    for r in resolved {
        let id = crate::attachments::save_text(app, state, chat_id, &r.label, &r.text).await?;
        parts.push(part_for(id, &r));
    }
    Ok(parts)
}

/// The content part of a stored mention: a document, with what the chip shows.
fn part_for(attachment_id: i64, r: &Resolved) -> Value {
    json!({
        "type": "document",
        "attachment_id": attachment_id,
        "name": r.label,
        "mention": {
            "path": r.canonical,
            "kind": r.kind,
            "symbol": r.symbol,
            "container": r.container,
            "startLine": r.start_line,
            "endLine": r.end_line,
            "truncated": r.truncated,
        },
    })
}

/// The mentions of a stored message, for its chips.
pub fn mention_refs(content: &Value) -> Vec<MentionRef> {
    let Some(parts) = content.as_array() else {
        return Vec::new();
    };
    parts
        .iter()
        .filter(|p| p.get("type").and_then(Value::as_str) == Some("document"))
        .filter_map(|p| {
            let m = p.get("mention")?;
            let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
            let n = |k: &str| m.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
            Some(MentionRef {
                attachment_id: p.get("attachment_id")?.as_i64()?,
                label: p.get("name")?.as_str()?.to_string(),
                kind: s("kind").unwrap_or_else(|| "file".into()),
                path: s("path").unwrap_or_default(),
                symbol: s("symbol"),
                container: s("container"),
                start_line: n("startLine"),
                end_line: n("endLine"),
                truncated: m.get("truncated").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::MAX_COUNTS;

    fn resolved(label: &str, text: &str) -> Resolved {
        Resolved {
            canonical: "/w/calc.py".into(),
            label: label.into(),
            kind: "function".into(),
            symbol: Some("add".into()),
            container: None,
            start_line: 4,
            end_line: 5,
            total_lines: 9,
            truncated: false,
            text: text.into(),
        }
    }

    /// The two commands are granted to the main window only, and each one checks the
    /// calling window again.
    #[test]
    fn the_commands_are_for_the_main_window_only() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let read = |p: std::path::PathBuf| std::fs::read_to_string(&p).unwrap();
        let caps = manifest.join("capabilities");
        let main = read(caps.join("main.json"));
        let list = read(manifest.join("commands.list"));
        for command in ["search_mentions", "resolve_mentions"] {
            let grant = format!("\"allow-{}\"", command.replace('_', "-"));
            assert!(main.contains(&grant), "main.json lacks {grant}");
            assert!(list.lines().any(|l| l.trim() == command), "{command}");
        }
        for other in ["overlay.json", "indicator.json"] {
            assert!(!read(caps.join(other)).contains("mention"), "{other}");
        }
        assert!(only_main_window(crate::MAIN_WINDOW).is_ok());
        for label in ["overlay", "indicator", "artifact", ""] {
            assert_eq!(
                only_main_window(label).expect_err(label).code,
                "bad_request"
            );
        }
    }

    /// [0.0.6] The documents and the mentions of one message share 40% of the context: with
    /// 8 documents and 2 mentions, each mention is cut to the same share as each document,
    /// not to a share of the mentions alone (the current message is never summarised, so a
    /// larger one would stop the chat with "too long").
    #[tokio::test]
    async fn documents_and_mentions_share_forty_percent_of_the_context() {
        let base = std::env::temp_dir().join(format!(
            "mention-share-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let base = dunce::canonicalize(&base).unwrap();
        // Removed also when an assertion fails.
        struct Gone(std::path::PathBuf);
        impl Drop for Gone {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _gone = Gone(base.clone());
        // Two files far larger than any share (about 780 KB each, under the 1 MB limit).
        let long: String = (0..20_000)
            .map(|n| format!("line {n:>6} of a long file to mention\n"))
            .collect();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(base.join(name), &long).unwrap();
        }
        let folder = base.display().to_string();
        let ctx = ChatContext {
            chat_id: 1,
            approved_folders: vec![folder.clone()],
            working_folder: Some(folder),
            result_cap_tokens: 2000,
            ..Default::default()
        };
        let mentions: Vec<MentionRequest> = ["a.txt", "b.txt"]
            .into_iter()
            .map(|p| MentionRequest {
                path: p.into(),
                ..Default::default()
            })
            .collect();
        for n_ctx in [8_192, 16_384, 32_768, 65_536, 131_072, 262_144] {
            let read = read_all(ctx.clone(), mentions.clone(), n_ctx, 8, None::<NoCount>)
                .await
                .unwrap();
            let mentioned: usize = read
                .iter()
                .map(|r| {
                    let r = r.as_ref().unwrap();
                    assert!(r.truncated, "{n_ctx}: {} was not cut", r.label);
                    r.text.chars().count()
                })
                .sum();
            let documents = 8 * crate::attachments::document_cap_chars(n_ctx, 8 + 2);
            // 40% of the context, at 3 characters per token.
            let share = n_ctx * 2 / 5 * 3;
            assert!(
                mentioned + documents <= share,
                "{n_ctx}: mentions {mentioned} + documents {documents} > {share}"
            );
        }
    }

    /// A folder for one test, removed when it ends, also when an assertion fails.
    struct Folder(std::path::PathBuf);
    impl Folder {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&base).unwrap();
            Self(dunce::canonicalize(&base).unwrap())
        }
        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.0.join(name), text).unwrap();
        }
        /// The chat context with this folder approved and as the working folder.
        fn chat(&self) -> ChatContext {
            let folder = self.0.display().to_string();
            ChatContext {
                chat_id: 1,
                approved_folders: vec![folder.clone()],
                working_folder: Some(folder),
                result_cap_tokens: 2000,
                ..Default::default()
            }
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn files(paths: &[&str]) -> Vec<MentionRequest> {
        paths
            .iter()
            .map(|p| MentionRequest {
                path: (*p).into(),
                ..Default::default()
            })
            .collect()
    }

    /// A table of numbers, the kind of text a tokenizer splits finely: `rows` lines.
    fn table(rows: usize) -> String {
        (1..=rows)
            .map(|i| {
                format!(
                    "{i},{},{}\n",
                    (i * 7_919) % 100_003,
                    (i * 104_729) % 1_000_003
                )
            })
            .collect()
    }

    /// A made-up model server's count: `per_digit` tokens for each digit, one for every
    /// three other characters (the estimate), and how many texts it was asked to count.
    fn counter(
        per_digit: usize,
        asked: &std::sync::Mutex<usize>,
    ) -> impl Fn(String) -> std::future::Ready<Option<usize>> + '_ {
        move |t: String| {
            *asked.lock().unwrap() += 1;
            let digits = t.chars().filter(char::is_ascii_digit).count();
            let other = t.chars().count() - digits;
            std::future::ready(Some(digits * per_digit + other.div_ceil(3)))
        }
    }

    fn texts(read: Vec<Result<Resolved, String>>) -> Vec<Resolved> {
        read.into_iter().map(|r| r.unwrap()).collect()
    }

    /// [0.0.6] With a local model a mention's share is held in the model server's own
    /// tokens, as a document's is (`attachments::cut_to_share`): numbers mentioned beside 8
    /// documents, or 8 of them alone, take more tokens than the 3-characters-a-token
    /// estimate allows, and are read again with fewer lines until each is within its share,
    /// still with the note on what was left out. So documents and mentions together stay
    /// within 40% of the context in the model's own tokens. Where the share is close to the
    /// 600-character floor (8,192 tokens shared by 8 or 10 files, the review's cases, which
    /// took about 51% and almost all of the context by the estimate alone) the excerpt
    /// stops at the floor, as a document does. Text the estimate already covers is read
    /// once and counted once.
    #[tokio::test]
    async fn a_mentions_share_is_held_in_the_models_own_tokens() {
        let folder = Folder::new("mention-tokens");
        let names: Vec<String> = (0..8).map(|n| format!("data{n}.csv")).collect();
        for name in &names {
            folder.write(name, &table(3_000));
        }
        let prose = "The river runs past the old mill and on to the sea.\n".repeat(2_000);
        folder.write("notes.txt", &prose);
        let names: Vec<&str> = names.iter().map(String::as_str).collect();

        // (context, mentions, documents, whether the share is too close to the floor)
        for (n_ctx, mentions, documents, at_floor) in [
            (8_192, &names[..2], 8, true),
            (16_384, &names[..2], 8, false),
            (32_768, &names[..2], 8, false),
            (131_072, &names[..2], 8, false),
            (8_192, &names[..], 0, true),
            (32_768, &names[..], 0, false),
            (65_536, &names[..], 0, false),
        ] {
            let what = format!("{n_ctx} tokens, {} mentions", mentions.len());
            let cap = resolve::cap_chars(n_ctx, mentions.len() + documents);
            let allowed = cap / resolve::CHARS_PER_TOKEN;
            let by_estimate = texts(
                read_all(
                    folder.chat(),
                    files(mentions),
                    n_ctx,
                    documents,
                    None::<NoCount>,
                )
                .await
                .unwrap(),
            );
            let asked = std::sync::Mutex::new(0);
            let count = counter(1, &asked);
            let held = texts(
                read_all(
                    folder.chat(),
                    files(mentions),
                    n_ctx,
                    documents,
                    Some(&count),
                )
                .await
                .unwrap(),
            );
            assert!(
                *asked.lock().unwrap() <= mentions.len() * MAX_COUNTS,
                "{what}"
            );
            let mut mentioned = 0;
            for (estimate, r) in by_estimate.iter().zip(&held) {
                // The estimate alone is over: the case this holds.
                let over = count(estimate.text.clone()).await.unwrap();
                assert!(over > allowed, "{what}: {over} <= {allowed}");
                let tokens = count(r.text.clone()).await.unwrap();
                let chars = r.text.chars().count();
                if at_floor {
                    // As few characters as any file keeps, and fewer tokens than before.
                    assert!(chars <= resolve::MIN_CHARS, "{what}: {chars}");
                    assert!(chars * 2 > resolve::MIN_CHARS, "{what}: {chars}");
                    assert!(tokens < over, "{what}: {tokens} >= {over}");
                } else {
                    assert!(
                        tokens <= allowed,
                        "{what}: {} took {tokens} > {allowed}",
                        r.label
                    );
                    // Not cut to nothing: more than half the share is used.
                    assert!(tokens * 2 > allowed, "{what}: {tokens} of {allowed}");
                    assert!(chars > resolve::MIN_CHARS, "{what}: {chars}");
                }
                assert!(r.text.len() < estimate.text.len(), "{what}");
                assert!(r.truncated, "{what}");
                assert!(r.text.starts_with("File: "), "{what}");
                assert!(r.text.contains("of the file were left out"), "{what}");
                assert_eq!(
                    (r.label.as_str(), r.kind.as_str()),
                    (estimate.label.as_str(), "file")
                );
                mentioned += tokens;
            }
            // Each document is held to its share the same way (`cut_to_share`).
            let documents = documents
                * (crate::attachments::document_cap_chars(n_ctx, mentions.len() + documents) / 3);
            assert!(
                at_floor || mentioned + documents <= n_ctx * 2 / 5,
                "{what}: mentions {mentioned} + documents {documents} > 40%"
            );
        }

        // A server that counts as the estimate does (the line numbers and the path are
        // digits, which `counter` makes dear): the same excerpt, counted once.
        let asked = std::sync::Mutex::new(0);
        let count = |t: String| {
            *asked.lock().unwrap() += 1;
            std::future::ready(Some(t.chars().count().div_ceil(3)))
        };
        let mentions = files(&["notes.txt"]);
        assert_eq!(
            read_all(folder.chat(), mentions.clone(), 8_192, 8, Some(&count))
                .await
                .unwrap(),
            read_all(folder.chat(), mentions, 8_192, 8, None::<NoCount>)
                .await
                .unwrap()
        );
        assert_eq!(*asked.lock().unwrap(), 1);
    }

    /// [0.0.6] A definition read again keeps its definition; a server that cannot count
    /// leaves the estimate's excerpt; nothing goes below the 600-character floor, even for
    /// a tokenizer far denser than any real one, and the counts are bounded.
    #[tokio::test]
    async fn a_held_mention_keeps_its_definition_and_never_goes_below_the_floor() {
        let folder = Folder::new("mention-floor");
        let body: String = table(2_000)
            .lines()
            .map(|l| format!("        ({l}),\n"))
            .collect();
        folder.write(
            "calc.py",
            &format!(
                "def add(a, b):\n    return a + b\n\n\ndef rows():\n    return [\n{body}    ]\n"
            ),
        );
        folder.write("data.csv", &table(3_000));
        let rows = vec![MentionRequest {
            path: "calc.py".into(),
            symbol: Some("rows".into()),
            kind: Some("function".into()),
            ..Default::default()
        }];
        let (n_ctx, documents) = (8_192, 3);
        let allowed = resolve::cap_chars(n_ctx, 1 + documents) / resolve::CHARS_PER_TOKEN;
        let asked = std::sync::Mutex::new(0);
        let count = counter(1, &asked);
        let held = texts(
            read_all(folder.chat(), rows.clone(), n_ctx, documents, Some(&count))
                .await
                .unwrap(),
        );
        let estimate = texts(
            read_all(folder.chat(), rows, n_ctx, documents, None::<NoCount>)
                .await
                .unwrap(),
        );
        assert!(count(estimate[0].text.clone()).await.unwrap() > allowed);
        let r = &held[0];
        assert!(count(r.text.clone()).await.unwrap() <= allowed);
        assert_eq!(
            (r.label.as_str(), r.kind.as_str(), r.symbol.as_deref()),
            ("calc.py#rows", "function", Some("rows"))
        );
        assert_eq!(
            (r.start_line, r.end_line),
            (estimate[0].start_line, estimate[0].end_line)
        );
        assert!(r.text.starts_with("File: ") && r.text.contains("(function rows)"));
        assert!(
            r.text.contains("of this definition were left out"),
            "{}",
            r.text
        );

        // The server could not count: the estimate's excerpt.
        let failed = |_: String| std::future::ready(None::<usize>);
        let data = files(&["data.csv"]);
        assert_eq!(
            read_all(folder.chat(), data.clone(), n_ctx, documents, Some(failed))
                .await
                .unwrap(),
            read_all(
                folder.chat(),
                data.clone(),
                n_ctx,
                documents,
                None::<NoCount>
            )
            .await
            .unwrap()
        );

        // Fifty tokens a digit: the floor, and a bounded number of counts.
        let asked = std::sync::Mutex::new(0);
        let count = counter(50, &asked);
        let held = texts(
            read_all(folder.chat(), data, n_ctx, documents, Some(&count))
                .await
                .unwrap(),
        );
        let chars = held[0].text.chars().count();
        assert!(chars <= resolve::MIN_CHARS, "{chars}");
        assert!(held[0].text.contains("\n     1\t1,"), "{}", held[0].text);
        assert!(*asked.lock().unwrap() <= MAX_COUNTS);
    }

    /// [0.0.6] Against the pinned llama-server and the real small model (set
    /// `LLAMA_SERVER_BIN` and `TEST_GGUF`; skipped without them, and off Linux like the
    /// other tests that start it here): tables of numbers mentioned on a local model are
    /// held to their share in the server's own tokens, which the 3-characters-a-token
    /// estimate alone is not (the review counted 767 tokens against a share of 327), or,
    /// where the share is close to the 600-character floor, cut to the floor.
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread")]
    async fn mentions_are_held_in_llama_servers_own_tokens() {
        let bin = std::env::var_os("LLAMA_SERVER_BIN").filter(|v| !v.is_empty());
        let gguf = std::env::var_os("TEST_GGUF").filter(|v| !v.is_empty());
        let (Some(bin), Some(gguf)) = (bin, gguf) else {
            eprintln!("skipping: set LLAMA_SERVER_BIN and TEST_GGUF to run it");
            return;
        };
        let dir = std::path::PathBuf::from(bin)
            .parent()
            .expect("a folder")
            .to_path_buf();
        let layout = runtime::RuntimeLayout {
            installed_dir: None,
            packs_dir: dir.join("no-packs-here"),
            dev_override: Some(dir),
        };
        let backend = runtime::discover(&layout)
            .into_iter()
            .find(|b| b.kind == runtime::BackendKind::Cpu)
            .expect("the processor backend next to LLAMA_SERVER_BIN");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let launch = runtime::LaunchConfig {
            gpu: runtime::GpuUse::Off,
            cache_ram_mib: 0,
            ..runtime::LaunchConfig::new(
                std::path::PathBuf::from(gguf),
                2_048,
                runtime::free_loopback_port().expect("a port"),
                runtime::random_api_key(),
            )
        };
        let spec = runtime::StartSpec {
            backend,
            launch,
            load_timeout: std::time::Duration::from_secs(180),
        };
        let server = match runtime::start(spec, tx).await {
            Ok(server) => server,
            Err(e) => panic!("start failed: {e}\n{}", e.stderr_tail().join("\n")),
        };
        let client =
            crate::runtime_manager::local_client(server.base_url(), server.api_key().to_string())
                .unwrap();
        let count = |t: String| crate::attachments::server_tokens(&client, t);

        let folder = Folder::new("mention-llama");
        let names: Vec<String> = (0..8).map(|n| format!("data{n}.csv")).collect();
        for name in &names {
            folder.write(name, &table(3_000));
        }
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        // (context, mentions, documents, whether the share is too close to the floor)
        for (n_ctx, mentions, documents, at_floor) in [
            (8_192, &names[..2], 8, true),
            (8_192, &names[..], 0, true),
            (32_768, &names[..2], 8, false),
            (32_768, &names[..], 0, false),
        ] {
            let what = format!(
                "{n_ctx} tokens, {} mentions, {documents} documents",
                mentions.len()
            );
            let allowed = resolve::cap_chars(n_ctx, mentions.len() + documents) / 3;
            let estimate = texts(
                read_all(
                    folder.chat(),
                    files(mentions),
                    n_ctx,
                    documents,
                    None::<NoCount>,
                )
                .await
                .unwrap(),
            );
            let held = texts(
                read_all(
                    folder.chat(),
                    files(mentions),
                    n_ctx,
                    documents,
                    Some(&count),
                )
                .await
                .unwrap(),
            );
            let mut mentioned = 0;
            for (e, r) in estimate.iter().zip(&held) {
                let before = count(e.text.clone()).await.expect("a count");
                let after = count(r.text.clone()).await.expect("a count");
                eprintln!(
                    "{what}: {} by the estimate {} characters, {before} tokens; held {} characters, {after} tokens (share {allowed})",
                    r.label,
                    e.text.chars().count(),
                    r.text.chars().count()
                );
                assert!(before > allowed, "{what}: {before} <= {allowed}");
                if at_floor {
                    assert!(r.text.chars().count() <= resolve::MIN_CHARS, "{what}");
                    assert!(after < before, "{what}: {after} >= {before}");
                } else {
                    assert!(after <= allowed, "{what}: {after} > {allowed}");
                    assert!(after * 2 > allowed, "{what}: {after} of {allowed}");
                }
                mentioned += after;
            }
            let documents = documents
                * (crate::attachments::document_cap_chars(n_ctx, mentions.len() + documents) / 3);
            eprintln!("{what}: mentions {mentioned} + documents {documents} of {n_ctx}");
            assert!(
                at_floor || mentioned + documents <= n_ctx * 2 / 5,
                "{what}: mentions {mentioned} + documents {documents} > 40%"
            );
        }
        server.stop().await;
    }

    /// A stored mention comes back as its chip, and is not listed again as a document.
    #[test]
    fn a_stored_mention_is_a_chip_and_not_a_document() {
        let content = json!([
            {"type": "text", "text": "see @calc.py#add"},
            part_for(7, &resolved("calc.py#add", "File: ...")),
            {"type": "document", "attachment_id": 8, "name": "notes.txt"}
        ]);
        assert_eq!(
            mention_refs(&content),
            vec![MentionRef {
                attachment_id: 7,
                label: "calc.py#add".into(),
                kind: "function".into(),
                path: "/w/calc.py".into(),
                symbol: Some("add".into()),
                container: None,
                start_line: 4,
                end_line: 5,
                truncated: false,
            }]
        );
        assert_eq!(
            crate::attachments::document_refs(&content),
            vec![(8, "notes.txt".to_string())]
        );
    }

    /// The excerpt is a copy in the app's attachments folder: deleting the chat, and then
    /// deleting every file the store says is no longer used, leaves the user's file alone.
    #[tokio::test]
    async fn deleting_a_chat_with_a_mention_does_not_touch_the_mentioned_file() {
        let base = std::env::temp_dir().join(format!(
            "mention-delete-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project = base.join("project");
        let attachments = base.join("attachments");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&attachments).unwrap();
        let user_file = project.join("calc.py");
        std::fs::write(&user_file, "def add(a, b):\n    return a + b\n").unwrap();

        let store = store::Store::open_in_memory().unwrap();
        let chat = store
            .create_chat(store::NewChat {
                title: "t".into(),
                model_ref: None,
                system_prompt: None,
            })
            .await
            .unwrap();
        let mut r = resolved("calc.py#add", "File: calc.py, lines 1-2 (function add)\n");
        r.canonical = user_file.display().to_string();
        let id = crate::attachments::save_text_in(&store, &attachments, chat, &r.label, &r.text)
            .await
            .unwrap();
        store
            .insert_message(store::NewMessage {
                chat_id: chat,
                parent_id: None,
                role: store::Role::User,
                content: json!([{"type": "text", "text": "see"}, part_for(id, &r)]),
                reasoning: None,
                status: store::MessageStatus::Complete,
                model_ref: None,
                search_text: "see".into(),
            })
            .await
            .unwrap();
        let att = store
            .get_attachment(store::AttachmentId(id))
            .await
            .unwrap()
            .unwrap();
        assert!(std::path::Path::new(&att.bytes_path).starts_with(&attachments));
        assert_eq!(att.extracted_text.as_deref(), Some(r.text.as_str()));

        let orphaned = store.delete_chat(chat).await.unwrap();
        assert!(
            !orphaned
                .iter()
                .any(|p| std::path::Path::new(p) == user_file)
        );
        for p in &orphaned {
            assert!(std::path::Path::new(p).starts_with(&attachments), "{p}");
            std::fs::remove_file(p).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&user_file).unwrap(),
            "def add(a, b):\n    return a + b\n"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
