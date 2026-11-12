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

/// Reads the mentions for `ctx` in a blocking task, each cut to its share of `n_ctx`.
async fn read_all(
    ctx: ChatContext,
    mentions: Vec<MentionRequest>,
    n_ctx: usize,
) -> AppResult<Vec<Result<Resolved, String>>> {
    tokio::task::spawn_blocking(move || {
        let cap = resolve::cap_chars(n_ctx, mentions.len());
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
    Ok(read_all(ctx, mentions, n_ctx)
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
/// cannot be read fails the send with the reason, before anything is stored.
pub async fn attach(
    app: &AppHandle,
    state: &AppState,
    chat_id: store::ChatId,
    mentions: Vec<MentionRequest>,
    n_ctx: usize,
) -> AppResult<Vec<Value>> {
    if mentions.is_empty() {
        return Ok(Vec::new());
    }
    too_many(mentions.len())?;
    let ctx = context_for(app, state, Some(chat_id)).await?;
    let mut resolved = Vec::new();
    for r in read_all(ctx, mentions, n_ctx).await? {
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
