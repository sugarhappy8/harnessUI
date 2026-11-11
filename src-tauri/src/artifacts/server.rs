//! The `artifacts` tool server: `create_artifact`, `update_artifact`, `read_artifact` and
//! `list_artifacts`.
//!
//! These tools read and write the app's own database, and only the rows of the chat whose
//! turn runs them: every store call is made with that chat's id, and an artifact of
//! another chat reads as not there. Nothing outside the app is touched, so they never
//! prompt (`policy::Scope::AppData`); each call is still decided by the policy engine and
//! written to the audit log like any other.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use serde::Serialize;
use serde_json::{Map, Value, json};
use store::{
    Artifact, ArtifactAuthor, ArtifactId, ArtifactKind, ChatId, MAX_ARTIFACT_BYTES,
    MAX_ARTIFACT_TITLE_CHARS, NewArtifact, StoreError,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use super::edit::replace_once;
use super::isolation::Isolation;
use super::render::{NotShown, RENDER_WAIT, RenderReport, report_text};
use super::{CHANGED_EVENT, Shared};
use crate::state::AppState;

/// The server's name, which is also its group in the Tools panel.
pub const SERVER_NAME: &str = "artifacts";
pub const CREATE: &str = "create_artifact";
pub const UPDATE: &str = "update_artifact";
pub const READ: &str = "read_artifact";
pub const LIST: &str = "list_artifacts";

/// Characters of content kept in an audit preview.
const PREVIEW_CHARS: usize = 2000;

/// Sent to the main window when an artifact of a chat was created or got a new version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changed {
    pub chat_id: i64,
    pub id: i64,
    pub version: u32,
    pub kind: ArtifactKind,
    pub title: String,
    /// A tool is waiting for the window to show this version and report what went wrong.
    pub wants_report: bool,
}

// ---- Arguments ---------------------------------------------------------------------

fn text_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn id_arg(args: &Map<String, Value>) -> Result<ArtifactId, String> {
    match args.get("id").and_then(Value::as_i64) {
        Some(id) if id >= 1 => Ok(ArtifactId(id)),
        _ => Err(
            "id must be the number of an artifact of this chat (list_artifacts shows them).".into(),
        ),
    }
}

/// A title as it is stored: one line, nothing invisible, within the limit.
pub fn clean_title(raw: &str) -> Result<String, String> {
    let squeezed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = policy::sanitize::visible(&squeezed);
    if title.is_empty() {
        return Err("The title is empty. Give the artifact a short title.".into());
    }
    let chars = title.chars().count();
    if chars > MAX_ARTIFACT_TITLE_CHARS {
        return Err(format!(
            "The title is {chars} characters long; the limit is {MAX_ARTIFACT_TITLE_CHARS}. Use a shorter title."
        ));
    }
    Ok(title)
}

fn check_content(content: &str) -> Result<(), String> {
    if content.trim().is_empty() {
        return Err("content is empty. Put the whole text of the artifact in content.".into());
    }
    if content.len() > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "The content is {} bytes; the limit is {MAX_ARTIFACT_BYTES} (1 MB). Make it smaller, or split it into several artifacts.",
            content.len()
        ));
    }
    Ok(())
}

fn kind_arg(args: &Map<String, Value>) -> Result<ArtifactKind, String> {
    text_arg(args, "kind")
        .and_then(|k| k.trim().parse::<ArtifactKind>().ok())
        .ok_or_else(|| "kind must be one of: markdown, html, svg, mermaid.".to_owned())
}

/// What an `update_artifact` call asks for.
enum Update<'a> {
    Whole(&'a str),
    Piece { old: &'a str, new: &'a str },
}

fn update_arg(args: &Map<String, Value>) -> Result<Update<'_>, String> {
    let content = text_arg(args, "content");
    let old = text_arg(args, "old_text");
    let new = text_arg(args, "new_text");
    match (content, old, new) {
        (Some(content), None, None) => {
            check_content(content)?;
            Ok(Update::Whole(content))
        }
        (None, Some(old), Some(new)) => {
            if old.is_empty() {
                return Err("old_text is empty. Put the exact text to replace in old_text, or send the whole new text in content.".into());
            }
            Ok(Update::Piece { old, new })
        }
        (Some(_), _, _) => Err("Pass either content (the whole new text) or old_text with new_text (one replacement), not both.".into()),
        (None, Some(_), None) => Err("new_text is missing. Pass the replacement for old_text in new_text.".into()),
        (None, None, Some(_)) => Err("old_text is missing. Pass the exact text to replace in old_text.".into()),
        (None, None, None) => Err("Pass content (the whole new text), or old_text with new_text (one replacement).".into()),
    }
}

fn version_arg(args: &Map<String, Value>) -> Result<Option<u32>, String> {
    match args.get("version") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1)
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| "version must be a version number, from 1.".to_owned()),
    }
}

// ---- What the tools say ------------------------------------------------------------

fn quoted_title(title: &str) -> String {
    format!("\"{title}\"")
}

/// The first line of the result of a create or an update. The tool card reads the id
/// from it, so its shape is fixed.
pub fn saved_line(artifact: &Artifact) -> String {
    format!(
        "Artifact #{} {} ({}), version {} saved.",
        artifact.id,
        quoted_title(&artifact.title),
        artifact.kind,
        artifact.latest_version
    )
}

fn listed(artifact: &Artifact) -> String {
    format!(
        "#{} {} ({}), {}",
        artifact.id,
        quoted_title(&artifact.title),
        artifact.kind,
        match artifact.versions {
            1 => "1 version".to_owned(),
            n => format!("{n} versions"),
        }
    )
}

fn storage_failed(e: &StoreError) -> ToolOutput {
    tracing::error!("an artifact tool could not use the database: {e}");
    ToolOutput::error("The app could not read or write its database. Try the call again.")
}

fn preview_of(content: &str) -> String {
    let head: String = content.chars().take(PREVIEW_CHARS).collect();
    let mut shown = policy::sanitize::visible(&head);
    if content.chars().count() > PREVIEW_CHARS {
        shown.push_str("\n…");
    }
    shown
}

// ---- Describing a call for the policy engine ------------------------------------------

fn action(tool: &str, summary: String, preview: String, read_only: bool) -> Described {
    Described::Action(policy::Action {
        server: SERVER_NAME.into(),
        tool: tool.into(),
        summary,
        preview,
        // The app's own data for this chat: never prompts, in any mode.
        scope: policy::Scope::AppData,
        risk: policy::Risk::Normal,
        read_only,
        outbound: false,
        keep_asking: None,
        network: false,
        hard_limit: None,
        payload: Value::Null,
    })
}

/// Arguments into an action, or the reason they cannot be one. No side effects and no
/// database: what the artifact is like is found out when the call runs.
pub fn describe(tool: &str, args: &Map<String, Value>) -> Described {
    let invalid = Described::Invalid;
    match tool {
        CREATE => {
            let title = match clean_title(text_arg(args, "title").unwrap_or("")) {
                Ok(t) => t,
                Err(e) => return invalid(e),
            };
            let kind = match kind_arg(args) {
                Ok(k) => k,
                Err(e) => return invalid(e),
            };
            let content = text_arg(args, "content").unwrap_or("");
            if let Err(e) = check_content(content) {
                return invalid(e);
            }
            action(
                tool,
                format!(
                    "Create the artifact {} ({kind})",
                    policy::sanitize::one_line(&quoted_title(&title), 140)
                ),
                format!(
                    "Title: {title}\nKind: {kind}\nSize: {} bytes\n\n{}",
                    content.len(),
                    preview_of(content)
                ),
                false,
            )
        }
        UPDATE => {
            let id = match id_arg(args) {
                Ok(id) => id,
                Err(e) => return invalid(e),
            };
            let preview = match update_arg(args) {
                Ok(Update::Whole(content)) => format!(
                    "New content ({} bytes):\n{}",
                    content.len(),
                    preview_of(content)
                ),
                Ok(Update::Piece { old, new }) => format!(
                    "Replace:\n{}\n\nWith:\n{}",
                    preview_of(old),
                    preview_of(new)
                ),
                Err(e) => return invalid(e),
            };
            action(
                tool,
                format!("Save a new version of artifact #{id}"),
                preview,
                false,
            )
        }
        READ => {
            let id = match id_arg(args) {
                Ok(id) => id,
                Err(e) => return invalid(e),
            };
            let version = match version_arg(args) {
                Ok(v) => v,
                Err(e) => return invalid(e),
            };
            let what = match version {
                Some(v) => format!("Read version {v} of artifact #{id}"),
                None => format!("Read artifact #{id}"),
            };
            action(tool, what.clone(), what, true)
        }
        LIST => action(
            tool,
            "List this chat's artifacts".into(),
            "List this chat's artifacts".into(),
            true,
        ),
        other => invalid(format!("{other} is not a tool of the artifacts server.")),
    }
}

// ---- Running a call ------------------------------------------------------------------

/// Everything the tools need, without the app around it: tests build one with a database
/// in memory.
pub struct Core {
    pub store: store::Store,
    pub shared: Arc<Shared>,
    /// Tells the main window that an artifact changed.
    pub notify: Arc<dyn Fn(Changed) + Send + Sync>,
    /// How long a create or an update waits for the preview's report.
    pub wait: Duration,
}

impl Core {
    /// "No artifact #7 in this chat", with what there is instead.
    async fn missing(&self, chat: ChatId, id: ArtifactId) -> ToolOutput {
        let others = match self.store.list_artifacts(chat).await {
            Ok(list) => list,
            Err(e) => return storage_failed(&e),
        };
        let mut text = format!("No artifact #{id} in this chat.");
        if others.is_empty() {
            text.push_str(" This chat has no artifacts yet; create_artifact makes one.");
        } else {
            let shown: Vec<String> = others.iter().take(20).map(listed).collect();
            text.push_str(&format!(" This chat's artifacts: {}.", shown.join("; ")));
            if others.len() > shown.len() {
                text.push_str(&format!(" ({} more)", others.len() - shown.len()));
            }
        }
        ToolOutput::error(text)
    }

    /// The result of a create or an update: the fixed first line, then how the preview
    /// went (for the kinds that are shown as a page).
    async fn saved(
        &self,
        chat: ChatId,
        artifact: &Artifact,
        extra: Option<String>,
        cancel: &CancellationToken,
    ) -> ToolOutput {
        let (id, version) = (artifact.id.0, artifact.latest_version);
        let changed = |wants_report: bool| Changed {
            chat_id: chat.0,
            id,
            version,
            kind: artifact.kind,
            title: artifact.title.clone(),
            wants_report,
        };
        let mut text = saved_line(artifact);
        if let Some(extra) = extra {
            text.push('\n');
            text.push_str(&extra);
        }
        if artifact.kind == ArtifactKind::Markdown {
            (self.notify)(changed(false));
            return ToolOutput::text(text);
        }
        let report = if matches!(self.shared.gate.status(), Isolation::Broken { .. }) {
            // Nothing in the preview runs on this system: there is nothing to wait for.
            (self.notify)(changed(false));
            Some(RenderReport::NotShown(NotShown::ScriptsOff))
        } else {
            let waiting = self.shared.renders.expect(id, version);
            (self.notify)(changed(true));
            let report = tokio::select! {
                answer = tokio::time::timeout(self.wait, waiting) => answer.ok().and_then(Result::ok),
                () = cancel.cancelled() => None,
            };
            self.shared.renders.forget(id, version);
            report
        };
        text.push('\n');
        text.push_str(&report_text(report.as_ref()));
        ToolOutput::text(text)
    }

    async fn create(
        &self,
        chat: ChatId,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> ToolOutput {
        // Checked again here: what runs is what was described.
        let (title, kind, content) = match (
            clean_title(text_arg(args, "title").unwrap_or("")),
            kind_arg(args),
            text_arg(args, "content").unwrap_or(""),
        ) {
            (Ok(title), Ok(kind), content) => (title, kind, content),
            (Err(e), _, _) | (_, Err(e), _) => return ToolOutput::error(e),
        };
        if let Err(e) = check_content(content) {
            return ToolOutput::error(e);
        }
        let created = self
            .store
            .create_artifact(NewArtifact {
                chat_id: chat,
                title,
                kind,
                content: content.to_owned(),
                author: ArtifactAuthor::Model,
            })
            .await;
        match created {
            Ok(artifact) => self.saved(chat, &artifact, None, cancel).await,
            Err(StoreError::InvalidInput(why)) => ToolOutput::error(format!(
                "The artifact was not created: {why}. Update an existing artifact with update_artifact, or ask the user to delete one in the Artifacts panel."
            )),
            Err(e) => storage_failed(&e),
        }
    }

    async fn update(
        &self,
        chat: ChatId,
        args: &Map<String, Value>,
        cancel: &CancellationToken,
    ) -> ToolOutput {
        let (id, update) = match (id_arg(args), update_arg(args)) {
            (Ok(id), Ok(update)) => (id, update),
            (Err(e), _) | (_, Err(e)) => return ToolOutput::error(e),
        };
        let (content, extra) = match update {
            Update::Whole(content) => (content.to_owned(), None),
            Update::Piece { old, new } => {
                let current = match self.store.artifact_version(chat, id, None).await {
                    Ok(Some(v)) => v,
                    Ok(None) => return self.missing(chat, id).await,
                    Err(e) => return storage_failed(&e),
                };
                let what = format!("artifact #{id} (version {})", current.version);
                match replace_once(&current.content, old, new, &what) {
                    Ok(done) => {
                        if let Err(e) = check_content(&done.text) {
                            return ToolOutput::error(e);
                        }
                        let line = done.line;
                        (
                            done.text,
                            Some(format!("Replaced the text at line {line}.")),
                        )
                    }
                    Err(why) => return ToolOutput::error(why),
                }
            }
        };
        let saved = self
            .store
            .add_artifact_version(chat, id, content, ArtifactAuthor::Model, None)
            .await;
        match saved {
            Ok(artifact) => self.saved(chat, &artifact, extra, cancel).await,
            Err(StoreError::NotFound { .. }) => self.missing(chat, id).await,
            Err(StoreError::InvalidInput(why)) => {
                ToolOutput::error(format!("The new version was not saved: {why}."))
            }
            Err(e) => storage_failed(&e),
        }
    }

    async fn read(&self, chat: &ChatContext, args: &Map<String, Value>) -> ToolOutput {
        let chat_id = ChatId(chat.chat_id);
        let (id, wanted) = match (id_arg(args), version_arg(args)) {
            (Ok(id), Ok(version)) => (id, version),
            (Err(e), _) | (_, Err(e)) => return ToolOutput::error(e),
        };
        let artifact = match self.store.get_artifact(chat_id, id).await {
            Ok(Some(a)) => a,
            Ok(None) => return self.missing(chat_id, id).await,
            Err(e) => return storage_failed(&e),
        };
        let version = match self.store.artifact_version(chat_id, id, wanted).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                let kept = self
                    .store
                    .artifact_versions(chat_id, id)
                    .await
                    .unwrap_or_default();
                let range = match (kept.first(), kept.last()) {
                    (Some(a), Some(b)) if a.version != b.version => {
                        format!("Its versions are {} to {}.", a.version, b.version)
                    }
                    (Some(a), _) => format!("Its only version is {}.", a.version),
                    _ => String::new(),
                };
                return ToolOutput::error(format!(
                    "Artifact #{id} has no version {}. {range}",
                    wanted.unwrap_or(artifact.latest_version)
                ));
            }
            Err(e) => return storage_failed(&e),
        };
        let mut head = format!(
            "Artifact #{id} {} ({}), version {} of {}, {}.",
            quoted_title(&artifact.title),
            artifact.kind,
            version.version,
            artifact.latest_version,
            match version.author {
                ArtifactAuthor::Model => "written by you",
                ArtifactAuthor::User => "edited by the user",
            }
        );
        if let Some(note) = &version.note {
            head.push_str(&format!(" {}.", policy::sanitize::one_line(note, 200)));
        }
        // The content is the artifact's own text: shown as it is, sized to the result cap.
        let budget = chat.result_cap_tokens.max(500) * 3;
        let full = format!("{head}\n\n{}", version.content);
        if full.len() <= budget {
            return ToolOutput::text(full);
        }
        let total_lines = version.content.lines().count();
        let mut shown = format!("{head}\n\n");
        let mut lines = 0;
        for line in version.content.split_inclusive('\n') {
            if shown.len() + line.len() > budget {
                break;
            }
            shown.push_str(line);
            lines += 1;
        }
        if lines == 0 {
            // One very long line: cut it at a character.
            let room = budget.saturating_sub(shown.len());
            let cut: String = version
                .content
                .char_indices()
                .take_while(|(at, c)| at + c.len_utf8() <= room)
                .map(|(_, c)| c)
                .collect();
            shown.push_str(&cut);
            shown.push('\n');
        }
        if !shown.ends_with('\n') {
            shown.push('\n');
        }
        shown.push_str(&format!(
            "[Showing the first {lines} of {total_lines} lines of this version.]"
        ));
        ToolOutput {
            full_text: Some(full),
            ..ToolOutput::text(shown)
        }
    }

    async fn list(&self, chat: ChatId) -> ToolOutput {
        match self.store.list_artifacts(chat).await {
            Ok(list) if list.is_empty() => {
                ToolOutput::text("This chat has no artifacts yet. create_artifact makes one.")
            }
            Ok(list) => {
                let mut text = match list.len() {
                    1 => "1 artifact in this chat:".to_owned(),
                    n => format!("{n} artifacts in this chat, most recently changed first:"),
                };
                for a in &list {
                    text.push('\n');
                    text.push_str(&listed(a));
                }
                ToolOutput::text(text)
            }
            Err(e) => storage_failed(&e),
        }
    }

    /// Runs one call for the chat of `ctx`.
    pub async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        ctx: &CallContext,
    ) -> ToolOutput {
        let chat = ChatId(ctx.chat.chat_id);
        match tool {
            CREATE => self.create(chat, args, &ctx.cancel).await,
            UPDATE => self.update(chat, args, &ctx.cancel).await,
            READ => self.read(&ctx.chat, args).await,
            LIST => self.list(chat).await,
            other => ToolOutput::error(format!("{other} is not a tool of the artifacts server.")),
        }
    }
}

// ---- The server ----------------------------------------------------------------------

/// The tools as offered to the model.
pub fn tool_defs() -> Vec<ToolDef> {
    let def = |name: &str, description: &str, schema: Value, read_only: bool| ToolDef {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        destructive: Some(false),
        read_only: Some(read_only),
    };
    vec![
        def(
            CREATE,
            "Make a document, a page, a picture or a diagram that the user sees in a panel beside the chat, instead of pasting it into your answer. kind is one of: markdown (a document), html (one self-contained HTML page with its CSS and JavaScript inline: a page or a small interactive tool), svg (a picture), mermaid (a diagram in Mermaid syntax). Put the complete text in content. A page cannot load anything from the network: no links to scripts, styles, fonts or pictures elsewhere. The result says whether the preview showed it without errors; if it lists problems, fix them with update_artifact.",
            json!({"type":"object","properties":{
                "title": {"type":"string","minLength":1,"description":"A short title, up to 120 characters"},
                "kind": {"type":"string","enum":["markdown","html","svg","mermaid"]},
                "content": {"type":"string","minLength":1,"description":"The complete text of the artifact"}
            },"required":["title","kind","content"],"additionalProperties":false}),
            false,
        ),
        def(
            UPDATE,
            "Save a new version of an artifact of this chat. Either pass content with the complete new text, or pass old_text and new_text to replace one exact piece of the newest version (old_text must be in it exactly once). Earlier versions are kept.",
            json!({"type":"object","properties":{
                "id": {"type":"integer","minimum":1,"description":"The artifact's number"},
                "content": {"type":"string","description":"The complete new text"},
                "old_text": {"type":"string","description":"Exact text to replace"},
                "new_text": {"type":"string","description":"What to put in its place"}
            },"required":["id"],"additionalProperties":false}),
            false,
        ),
        def(
            READ,
            "Read an artifact of this chat: its newest version, or the version you name. The user can edit artifacts, so read one before you change it unless you just wrote it.",
            json!({"type":"object","properties":{
                "id": {"type":"integer","minimum":1,"description":"The artifact's number"},
                "version": {"type":"integer","minimum":1,"description":"A version number (default: the newest)"}
            },"required":["id"],"additionalProperties":false}),
            true,
        ),
        def(
            LIST,
            "List the artifacts of this chat with their numbers, titles, kinds and how many versions each has.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            true,
        ),
    ]
}

/// The built-in server registered with the tool host.
pub struct ArtifactsServer {
    app: AppHandle,
}

impl ArtifactsServer {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl BuiltinServer for ArtifactsServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        tool_defs()
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        _chat: &ChatContext,
    ) -> Described {
        describe(tool, args)
    }

    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        _action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        let Some(state) = self.app.try_state::<AppState>() else {
            return ToolOutput::error("The app is still starting. Try the call again.");
        };
        let app = self.app.clone();
        let core = Core {
            store: state.store.clone(),
            shared: super::shared(),
            notify: Arc::new(move |changed: Changed| {
                let _ = app.emit_to(crate::MAIN_WINDOW, CHANGED_EVENT, changed);
            }),
            wait: RENDER_WAIT,
        };
        core.call(tool, args, &ctx).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::artifacts::isolation::Tripwire;
    use crate::artifacts::render::{ProblemInput, clean_problems};

    struct Fixture {
        core: Core,
        seen: Arc<Mutex<Vec<Changed>>>,
        chat: i64,
        other: i64,
    }

    async fn fixture() -> Fixture {
        let store = store::Store::open_in_memory().expect("open");
        let mut chats = Vec::new();
        for title in ["mine", "other"] {
            let id = store
                .create_chat(store::NewChat {
                    title: title.into(),
                    ..Default::default()
                })
                .await
                .expect("chat");
            chats.push(id.0);
        }
        let seen: Arc<Mutex<Vec<Changed>>> = Arc::default();
        let sink = seen.clone();
        Fixture {
            core: Core {
                store,
                shared: Arc::default(),
                notify: Arc::new(move |c| sink.lock().unwrap().push(c)),
                wait: Duration::from_millis(60),
            },
            seen,
            chat: chats[0],
            other: chats[1],
        }
    }

    fn ctx(chat: i64) -> CallContext {
        CallContext {
            chat: ChatContext {
                chat_id: chat,
                result_cap_tokens: 2000,
                ..ChatContext::default()
            },
            cancel: CancellationToken::new(),
            progress: None,
        }
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().expect("an object").clone()
    }

    async fn run(f: &Fixture, chat: i64, tool: &str, a: Value) -> ToolOutput {
        f.core.call(tool, &args(a), &ctx(chat)).await
    }

    fn text(out: &ToolOutput) -> String {
        out.joined_text()
    }

    #[tokio::test]
    async fn a_markdown_artifact_is_saved_and_announced_without_waiting() {
        let f = fixture().await;
        let out = run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "  Trip   plan\n", "kind": "markdown", "content": "# Plan\n"}),
        )
        .await;
        assert!(!out.is_error);
        assert_eq!(
            text(&out),
            "Artifact #1 \"Trip plan\" (markdown), version 1 saved."
        );
        let seen = f.seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![Changed {
                chat_id: f.chat,
                id: 1,
                version: 1,
                kind: ArtifactKind::Markdown,
                title: "Trip plan".into(),
                wants_report: false,
            }]
        );
    }

    #[tokio::test]
    async fn a_page_waits_for_the_previews_report_and_passes_it_on() {
        let f = fixture().await;
        // Nobody shows it: the wait runs out.
        let out = run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "Tips", "kind": "html", "content": "<p>x</p>"}),
        )
        .await;
        assert_eq!(
            text(&out),
            "Artifact #1 \"Tips\" (html), version 1 saved.\nNot rendered yet: open it to check."
        );
        assert!(f.seen.lock().unwrap()[0].wants_report);
        assert!(
            !f.core.shared.renders.is_expected(1, 1),
            "the wait was left behind"
        );

        // The window reports problems for version 2 while the tool waits.
        let shared = f.core.shared.clone();
        let reporter = tokio::spawn(async move {
            for _ in 0..200 {
                if shared.renders.is_expected(1, 2) {
                    let (problems, total) = clean_problems(&[ProblemInput {
                        kind: "error".into(),
                        message: "ReferenceError: totl is not defined".into(),
                        line: Some(3),
                        col: Some(7),
                    }]);
                    return shared
                        .renders
                        .resolve(1, 2, RenderReport::Shown { problems, total });
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            false
        });
        let out = run(&f, f.chat, UPDATE, json!({"id": 1, "content": "<p>y</p>"})).await;
        assert!(reporter.await.unwrap(), "the report found nobody waiting");
        assert_eq!(
            text(&out),
            "Artifact #1 \"Tips\" (html), version 2 saved.\n\
             The preview reported 1 problem:\n\
             - 3:7 ReferenceError: totl is not defined\n\
             Fix it and save a corrected version with update_artifact."
        );
        assert!(
            !out.is_error,
            "a saved version with problems is still saved"
        );
    }

    #[tokio::test]
    async fn a_clean_preview_and_a_stopped_turn() {
        let f = fixture().await;
        let shared = f.core.shared.clone();
        tokio::spawn(async move {
            for _ in 0..200 {
                if shared.renders.resolve(
                    1,
                    1,
                    RenderReport::Shown {
                        problems: vec![],
                        total: 0,
                    },
                ) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let out = run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "Shape", "kind": "svg", "content": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>"}),
        )
        .await;
        assert_eq!(
            text(&out),
            "Artifact #1 \"Shape\" (svg), version 1 saved.\nRendered without errors."
        );

        // Stop ends the wait at once; the version is saved all the same.
        let mut slow = fixture().await;
        slow.core.wait = Duration::from_secs(30);
        let c = ctx(slow.chat);
        c.cancel.cancel();
        let started = std::time::Instant::now();
        let out = slow
            .core
            .call(
                CREATE,
                &args(json!({"title": "D", "kind": "mermaid", "content": "graph TD\n A-->B"})),
                &c,
            )
            .await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(text(&out).ends_with("Not rendered yet: open it to check."));
        assert_eq!(
            slow.core
                .store
                .list_artifacts(ChatId(slow.chat))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn with_scripts_off_nothing_is_waited_for() {
        let mut f = fixture().await;
        f.core.wait = Duration::from_secs(30);
        f.core.shared.gate.tripwire(Tripwire::Command);
        let started = std::time::Instant::now();
        let out = run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "Tips", "kind": "html", "content": "<p>x</p>"}),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(5));
        let t = text(&out);
        assert!(t.starts_with("Artifact #1 \"Tips\" (html), version 1 saved.\nNot checked: scripts are switched off"), "{t}");
        assert!(!f.seen.lock().unwrap()[0].wants_report);
    }

    /// The tools work for the chat of the turn that runs them, and for no other.
    #[tokio::test]
    async fn an_artifact_of_another_chat_cannot_be_read_listed_or_changed() {
        let f = fixture().await;
        let made = run(
            &f,
            f.other,
            CREATE,
            json!({"title": "Private", "kind": "markdown", "content": "the secret word is heron"}),
        )
        .await;
        assert!(text(&made).starts_with("Artifact #1 "));
        run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "Mine", "kind": "markdown", "content": "mine"}),
        )
        .await;

        let read = run(&f, f.chat, READ, json!({"id": 1})).await;
        assert!(read.is_error);
        assert_eq!(
            text(&read),
            "No artifact #1 in this chat. This chat's artifacts: #2 \"Mine\" (markdown), 1 version."
        );
        assert!(!text(&read).contains("heron") && !text(&read).contains("Private"));
        let read = run(&f, f.chat, READ, json!({"id": 1, "version": 1})).await;
        assert!(read.is_error && !text(&read).contains("heron"));

        for attempt in [
            json!({"id": 1, "content": "overwritten"}),
            json!({"id": 1, "old_text": "heron", "new_text": "owl"}),
            json!({"id": 1, "old_text": "no such text", "new_text": "x"}),
        ] {
            let out = run(&f, f.chat, UPDATE, attempt).await;
            assert!(out.is_error);
            assert!(
                text(&out).starts_with("No artifact #1 in this chat."),
                "{}",
                text(&out)
            );
            assert!(
                !text(&out).contains("heron"),
                "an error quoted another chat's text"
            );
        }
        let list = text(&run(&f, f.chat, LIST, json!({})).await);
        assert_eq!(
            list,
            "1 artifact in this chat:\n#2 \"Mine\" (markdown), 1 version"
        );

        // Untouched, and still readable from its own chat.
        let theirs = text(&run(&f, f.other, READ, json!({"id": 1})).await);
        assert_eq!(
            theirs,
            "Artifact #1 \"Private\" (markdown), version 1 of 1, written by you.\n\nthe secret word is heron"
        );
        assert_eq!(
            f.core
                .store
                .artifact_versions(ChatId(f.other), ArtifactId(1))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn one_piece_of_text_is_replaced_and_failures_say_what_to_do() {
        let f = fixture().await;
        let page = "<h1>Tips</h1>\n<p class=\"a\">Total: 0</p>\n<p class=\"b\">Total: 0</p>\n";
        run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "Doc", "kind": "markdown", "content": page}),
        )
        .await;

        let twice = run(
            &f,
            f.chat,
            UPDATE,
            json!({"id": 1, "old_text": "Total: 0", "new_text": "Total: 1"}),
        )
        .await;
        assert!(twice.is_error);
        assert!(text(&twice).starts_with(
            "old_text was found 2 times in artifact #1 (version 1), so it is not clear which one to change. Matches:\nline 2: <p class=\"a\">Total: 0</p>\nline 3: "
        ), "{}", text(&twice));

        let missing = run(
            &f,
            f.chat,
            UPDATE,
            json!({"id": 1, "old_text": "<H1>tips</H1>", "new_text": "x"}),
        )
        .await;
        assert!(missing.is_error);
        assert!(
            text(&missing)
                .contains("The text at line 1 is the same apart from upper and lower case.")
        );

        let done = run(
            &f,
            f.chat,
            UPDATE,
            json!({"id": 1, "old_text": "<p class=\"b\">Total: 0</p>", "new_text": "<p class=\"b\">Total: 5</p>"}),
        )
        .await;
        assert_eq!(
            text(&done),
            "Artifact #1 \"Doc\" (markdown), version 2 saved.\nReplaced the text at line 3."
        );
        let now = f
            .core
            .store
            .artifact_version(ChatId(f.chat), ArtifactId(1), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            now.content,
            page.replace("class=\"b\">Total: 0", "class=\"b\">Total: 5")
        );
        assert_eq!(now.author, ArtifactAuthor::Model);
        // Nothing was saved by the failed calls.
        assert_eq!(now.version, 2);
    }

    #[tokio::test]
    async fn reading_names_the_version_and_who_wrote_it() {
        let f = fixture().await;
        run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "Doc", "kind": "markdown", "content": "one"}),
        )
        .await;
        f.core
            .store
            .add_artifact_version(
                ChatId(f.chat),
                ArtifactId(1),
                "two".into(),
                ArtifactAuthor::User,
                Some("Restored from version 1".into()),
            )
            .await
            .unwrap();
        assert_eq!(
            text(&run(&f, f.chat, READ, json!({"id": 1})).await),
            "Artifact #1 \"Doc\" (markdown), version 2 of 2, edited by the user. Restored from version 1.\n\ntwo"
        );
        assert_eq!(
            text(&run(&f, f.chat, READ, json!({"id": 1, "version": 1})).await),
            "Artifact #1 \"Doc\" (markdown), version 1 of 2, written by you.\n\none"
        );
        let none = run(&f, f.chat, READ, json!({"id": 1, "version": 9})).await;
        assert!(none.is_error);
        assert_eq!(
            text(&none),
            "Artifact #1 has no version 9. Its versions are 1 to 2."
        );
        let empty = fixture().await;
        assert_eq!(
            text(&run(&empty, empty.chat, READ, json!({"id": 3})).await),
            "No artifact #3 in this chat. This chat has no artifacts yet; create_artifact makes one."
        );
        assert_eq!(
            text(&run(&empty, empty.chat, LIST, json!({})).await),
            "This chat has no artifacts yet. create_artifact makes one."
        );
    }

    #[tokio::test]
    async fn a_long_artifact_is_read_in_part_with_the_whole_kept_for_later() {
        let f = fixture().await;
        let long: String = (1..=4000)
            .map(|i| format!("line {i} of the document\n"))
            .collect();
        run(
            &f,
            f.chat,
            CREATE,
            json!({"title": "Long", "kind": "markdown", "content": long}),
        )
        .await;
        let mut c = ctx(f.chat);
        c.chat.result_cap_tokens = 500;
        let out = f.core.call(READ, &args(json!({"id": 1})), &c).await;
        let shown = text(&out);
        assert!(shown.len() <= 500 * 3 + 80, "{}", shown.len());
        assert!(shown.starts_with("Artifact #1 \"Long\" (markdown), version 1 of 1, written by you.\n\nline 1 of the document\n"));
        let last = shown.lines().last().unwrap();
        assert!(
            last.starts_with("[Showing the first ")
                && last.ends_with(" of 4000 lines of this version.]"),
            "{last}"
        );
        // Whole lines only.
        let before = shown.lines().rev().nth(1).unwrap();
        assert!(before.ends_with("of the document"), "{before}");
        let full = out.full_text.expect("the whole text for the stored output");
        assert!(full.ends_with("line 4000 of the document\n"));
    }

    #[test]
    fn arguments_are_checked_before_anything_runs() {
        let invalid = |tool: &str, a: Value| match describe(tool, &args(a)) {
            Described::Invalid(why) => why,
            Described::Action(a) => panic!("accepted: {}", a.summary),
        };
        assert!(
            invalid(
                CREATE,
                json!({"title": " \n ", "kind": "html", "content": "x"})
            )
            .starts_with("The title is empty")
        );
        let long = "t".repeat(121);
        assert_eq!(
            invalid(
                CREATE,
                json!({"title": long, "kind": "html", "content": "x"})
            ),
            "The title is 121 characters long; the limit is 120. Use a shorter title."
        );
        assert_eq!(
            invalid(CREATE, json!({"title": "T", "kind": "pdf", "content": "x"})),
            "kind must be one of: markdown, html, svg, mermaid."
        );
        assert!(
            invalid(
                CREATE,
                json!({"title": "T", "kind": "html", "content": "  "})
            )
            .starts_with("content is empty")
        );
        let big = "x".repeat(MAX_ARTIFACT_BYTES + 1);
        assert_eq!(
            invalid(
                CREATE,
                json!({"title": "T", "kind": "html", "content": big})
            ),
            "The content is 1048577 bytes; the limit is 1048576 (1 MB). Make it smaller, or split it into several artifacts."
        );
        assert!(invalid(UPDATE, json!({"content": "x"})).starts_with("id must be the number"));
        assert!(
            invalid(UPDATE, json!({"id": 0, "content": "x"})).starts_with("id must be the number")
        );
        assert!(invalid(UPDATE, json!({"id": 1})).starts_with("Pass content"));
        assert!(
            invalid(
                UPDATE,
                json!({"id": 1, "content": "x", "old_text": "a", "new_text": "b"})
            )
            .starts_with("Pass either content")
        );
        assert!(
            invalid(UPDATE, json!({"id": 1, "old_text": "a"})).starts_with("new_text is missing")
        );
        assert!(
            invalid(UPDATE, json!({"id": 1, "new_text": "a"})).starts_with("old_text is missing")
        );
        assert!(
            invalid(UPDATE, json!({"id": 1, "old_text": "", "new_text": "b"}))
                .starts_with("old_text is empty")
        );
        assert!(invalid(READ, json!({"id": 1, "version": 0})).starts_with("version must be"));
        assert!(invalid("delete_artifact", json!({})).contains("not a tool"));
        // A title is made one visible line.
        assert_eq!(
            clean_title("  A\ttitle\u{202e}\n here ").unwrap(),
            "A title\\u{202E} here"
        );
    }

    /// Every tool is the app's own data: no prompt, nothing outbound, and only the two
    /// that read are marked as reads.
    #[test]
    fn every_tool_is_described_as_the_apps_own_data() {
        let calls = [
            (
                CREATE,
                json!({"title": "T", "kind": "html", "content": "<p>x</p>"}),
                false,
            ),
            (UPDATE, json!({"id": 3, "content": "<p>y</p>"}), false),
            (
                UPDATE,
                json!({"id": 3, "old_text": "a", "new_text": "b"}),
                false,
            ),
            (READ, json!({"id": 3}), true),
            (READ, json!({"id": 3, "version": 2}), true),
            (LIST, json!({}), true),
        ];
        for (tool, a, read_only) in calls {
            let Described::Action(action) = describe(tool, &args(a)) else {
                panic!("{tool} was not described");
            };
            assert_eq!(action.server, SERVER_NAME);
            assert_eq!(action.tool, tool);
            assert_eq!(action.scope, policy::Scope::AppData);
            assert_eq!(action.risk, policy::Risk::Normal);
            assert_eq!(action.read_only, read_only, "{tool}");
            assert!(!action.outbound && !action.network, "{tool}");
            assert!(action.keep_asking.is_none() && action.hard_limit.is_none());
            assert!(!action.summary.is_empty() && !action.summary.contains('\n'));
            // The engine never asks for it: not in Ask mode, not for a recovered call,
            // not after untrusted content, and with no grant at all.
            for (recovered, untrusted) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let turn = policy::TurnContext {
                    tool_enabled: true,
                    server_ready: true,
                    recovered_from_text: recovered,
                    untrusted_in_turn: untrusted,
                    ..Default::default()
                };
                let none = std::collections::BTreeSet::<policy::Scope>::new();
                assert_eq!(
                    policy::evaluate(&action, &turn, &none),
                    policy::Decision::AllowedByGrant,
                    "{tool} would show a card"
                );
            }
        }
    }

    #[test]
    fn the_offered_tools_match_the_agreed_names_and_shapes() {
        let defs = tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, [CREATE, UPDATE, READ, LIST]);
        let required = |i: usize| defs[i].input_schema["required"].clone();
        assert_eq!(required(0), json!(["title", "kind", "content"]));
        assert_eq!(required(1), json!(["id"]));
        assert_eq!(required(2), json!(["id"]));
        assert_eq!(
            defs[0].input_schema["properties"]["kind"]["enum"],
            json!(["markdown", "html", "svg", "mermaid"])
        );
        for d in &defs {
            assert_eq!(d.input_schema["type"], "object");
            assert_eq!(d.input_schema["additionalProperties"], false);
            assert_eq!(d.destructive, Some(false));
            assert!(d.description.len() > 40);
        }
        assert_eq!(defs[2].read_only, Some(true));
        assert_eq!(defs[3].read_only, Some(true));
    }

    #[test]
    fn the_first_line_of_a_saved_result_has_a_fixed_shape() {
        let artifact = Artifact {
            id: ArtifactId(12),
            chat_id: ChatId(1),
            title: "Say \"hi\" (twice)".into(),
            kind: ArtifactKind::Mermaid,
            created_at: 0,
            updated_at: 0,
            latest_version: 4,
            versions: 4,
        };
        assert_eq!(
            saved_line(&artifact),
            "Artifact #12 \"Say \"hi\" (twice)\" (mermaid), version 4 saved."
        );
    }
}
