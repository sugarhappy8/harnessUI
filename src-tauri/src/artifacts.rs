//! Artifacts beside the chat (app 0.0.4): documents, pages, pictures and diagrams the
//! model makes with the `artifacts` tools, shown in the panel beside the chat, where the
//! user can read, edit, restore and export them.
//!
//! - [`server`]: the tools (`create_artifact`, `update_artifact`, `read_artifact`,
//!   `list_artifacts`). They touch only the app's database, for the chat of the turn.
//! - [`protocol`]: the `artifact` scheme a preview frame loads from, with the sandbox and
//!   the rules that keep a page the model wrote apart from the app and off the network.
//! - [`isolation`]: the check that this holds on the system the app runs on. Previews run
//!   scripts only after it has passed.
//! - [`render`]: what a preview reported after it was shown, cleaned, on its way back to
//!   the model as part of the tool's result.
//! - This file: the commands of the panel, and the scheme's handler.
//!
//! What a preview page can do is decided in `protocol.rs` and proven by the `artifacts`
//! scenario of `e2e/scripted_agent.py`.

mod edit;
mod isolation;
mod protocol;
mod render;
mod server;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::Serialize;
use store::{Artifact, ArtifactAuthor, ArtifactId, ArtifactKind, ArtifactVersionInfo, ChatId};
use tauri::http::{Method, Request, Response, StatusCode};
use tauri::{AppHandle, Emitter, Manager, Runtime, State, UriSchemeContext, UriSchemeResponder};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub use isolation::Isolation;
pub use protocol::{SCHEME, is_frame_navigation};
pub use server::ArtifactsServer;

/// Sent to the main window when an artifact was created or got a new version
/// ([`server::Changed`]).
pub const CHANGED_EVENT: &str = "artifact-changed";
/// Sent to the main window when a preview page touched a tripwire outside a check, so
/// scripts are off from now on ([`Isolation`]).
pub const ISOLATION_EVENT: &str = "artifact-isolation";

/// What the tools, the commands and the scheme's handler share.
#[derive(Default)]
pub struct Shared {
    /// Tools waiting for a preview's report.
    pub renders: render::RenderWaits,
    /// Whether previews may run scripts.
    pub gate: isolation::Gate,
    /// Which page a script slice may be cut from.
    pub tokens: protocol::Tokens,
}

/// The one [`Shared`] of the app.
pub(crate) fn shared() -> Arc<Shared> {
    static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
    SHARED.get_or_init(Arc::default).clone()
}

/// Where preview frames load from on this platform.
fn origin<R: Runtime>(app: &AppHandle<R>) -> String {
    let https = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == crate::MAIN_WINDOW)
        .is_some_and(|w| w.use_https_scheme);
    protocol::frame_origin(cfg!(any(windows, target_os = "android")), https)
}

fn announce_isolation<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit_to(crate::MAIN_WINDOW, ISOLATION_EVENT, shared().gate.status());
}

// ---- The scheme's handler --------------------------------------------------------------

/// Answers a request of a webview for the `artifact` scheme.
pub fn serve<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    // The border and the Stop button of computer use are windows too. They show nothing
    // of a chat.
    let from_main = ctx.webview_label() == crate::MAIN_WINDOW;
    tauri::async_runtime::spawn(async move {
        responder.respond(answer(&app, from_main, &request).await);
    });
}

async fn answer<R: Runtime>(
    app: &AppHandle<R>,
    from_main: bool,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    use protocol::{Route, Scripts};
    let origin = origin(app);
    let refuse = |status: StatusCode, text: &str| protocol::refusal(status, &origin, text);
    if !from_main {
        return refuse(
            StatusCode::FORBIDDEN,
            "Previews are shown in the main window only.",
        );
    }
    if request.method() != Method::GET {
        return refuse(StatusCode::METHOD_NOT_ALLOWED, "Only GET.");
    }
    let Some(route) = protocol::route(request.uri()) else {
        return refuse(StatusCode::NOT_FOUND, "Nothing here.");
    };
    let outside_a_frame = protocol::is_for_a_whole_window(request.headers());
    let shared = shared();
    let scripts = if shared.gate.scripts_allowed() {
        Scripts::On
    } else {
        Scripts::Off
    };
    match route {
        Route::Library => protocol::library(scripts, &origin),
        Route::Probe { nonce } => {
            if outside_a_frame || !shared.gate.is_current(&nonce) {
                return refuse(StatusCode::NOT_FOUND, "Nothing here.");
            }
            // The check's page is the app's own, and it is how scripts get switched on:
            // it runs its script whatever the state.
            protocol::page(Scripts::On, &origin, protocol::probe_page(&nonce, &origin))
        }
        Route::ProbeHit { nonce, .. } => {
            // A request the check's page must not be able to make. Only the page of the
            // check in progress knows the nonce.
            if shared.gate.is_current(&nonce) {
                shared.gate.tripwire(isolation::Tripwire::Request);
            }
            refuse(StatusCode::NOT_FOUND, "Nothing here.")
        }
        Route::Document { id, version } => {
            if outside_a_frame {
                return refuse(
                    StatusCode::FORBIDDEN,
                    "A preview is only shown inside the app.",
                );
            }
            let Some(state) = app.try_state::<AppState>() else {
                return refuse(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "The app is still starting.",
                );
            };
            let found = state
                .store
                .artifact_for_preview(ArtifactId(id), version)
                .await;
            let (artifact, content) = match found {
                Ok(Some((artifact, v))) => (artifact, v.content),
                Ok(None) => return refuse(StatusCode::NOT_FOUND, "No such version."),
                Err(e) => {
                    tracing::error!("a preview could not be read from the database: {e}");
                    return refuse(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "The preview could not be read.",
                    );
                }
            };
            if artifact.kind == ArtifactKind::Markdown {
                return refuse(
                    StatusCode::NOT_FOUND,
                    "A document is shown by the app itself.",
                );
            }
            if scripts == Scripts::Off {
                return protocol::page(
                    Scripts::Off,
                    &origin,
                    protocol::static_page(artifact.kind, &content),
                );
            }
            let token = shared.tokens.issue(id, version);
            let about = protocol::About {
                id,
                version,
                origin: &origin,
                token: &token,
            };
            let html = match artifact.kind {
                ArtifactKind::Svg => protocol::svg_page(&about, &content),
                ArtifactKind::Mermaid => protocol::mermaid_page(&about, &content),
                ArtifactKind::Html | ArtifactKind::Markdown => {
                    protocol::html_page(&about, &content)
                }
            };
            protocol::page(Scripts::On, &origin, html)
        }
        Route::Slice { token, from, to } => {
            let page = (scripts == Scripts::On)
                .then(|| shared.tokens.page_of(&token))
                .flatten();
            let (Some((id, version)), Some(state)) = (page, app.try_state::<AppState>()) else {
                return refuse(StatusCode::NOT_FOUND, "Nothing here.");
            };
            let content = match state
                .store
                .artifact_for_preview(ArtifactId(id), version)
                .await
            {
                Ok(Some((_, v))) => v.content,
                _ => return refuse(StatusCode::NOT_FOUND, "Nothing here."),
            };
            match protocol::slice(&protocol::normalise_newlines(&content), from, to) {
                Some(text) => protocol::script(Scripts::On, &origin, text),
                None => refuse(StatusCode::NOT_FOUND, "Nothing here."),
            }
        }
    }
}

// ---- What the panel is given -----------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDto {
    pub id: i64,
    pub chat_id: i64,
    pub title: String,
    pub kind: ArtifactKind,
    /// Versions kept.
    pub versions: u32,
    /// The number of the newest version.
    pub latest_version: u32,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<Artifact> for ArtifactDto {
    fn from(a: Artifact) -> Self {
        Self {
            id: a.id.0,
            chat_id: a.chat_id.0,
            title: a.title,
            kind: a.kind,
            versions: a.versions,
            latest_version: a.latest_version,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDto {
    pub version: u32,
    pub author: ArtifactAuthor,
    pub note: Option<String>,
    pub created_at: i64,
    pub bytes: u64,
}

impl From<ArtifactVersionInfo> for VersionDto {
    fn from(v: ArtifactVersionInfo) -> Self {
        Self {
            version: v.version,
            author: v.author,
            note: v.note,
            created_at: v.created_at,
            bytes: v.bytes,
        }
    }
}

/// One artifact with its versions and the content of one of them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDetailDto {
    pub artifact: ArtifactDto,
    /// The kept versions, oldest first, without content.
    pub versions: Vec<VersionDto>,
    /// The version whose content this is.
    pub version: u32,
    pub content: String,
    /// What the preview frame loads. `None` for a document, which the app shows itself.
    pub frame_url: Option<String>,
}

/// A check of the sandbox the main window is to run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckDto {
    pub nonce: String,
    /// The page to load into a hidden, sandboxed frame.
    pub url: String,
}

fn no_artifact() -> AppError {
    AppError::not_found("artifact")
}

fn refused(e: store::StoreError) -> AppError {
    match e {
        store::StoreError::NotFound { .. } => no_artifact(),
        store::StoreError::InvalidInput(why) => AppError::new(
            "artifact_refused",
            format!("That could not be saved: {why}."),
            "Change it and try again.",
            String::new(),
        ),
        other => other.into(),
    }
}

// ---- Commands of the panel -------------------------------------------------------------

/// The chat's artifacts, most recently changed first.
#[tauri::command]
pub async fn list_artifacts(
    state: State<'_, AppState>,
    chat_id: i64,
) -> AppResult<Vec<ArtifactDto>> {
    let list = state.store.list_artifacts(ChatId(chat_id)).await?;
    Ok(list.into_iter().map(Into::into).collect())
}

/// One artifact of the chat: its versions, and the content of `version` (the newest when
/// left out).
#[tauri::command]
pub async fn get_artifact(
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
    version: Option<u32>,
) -> AppResult<ArtifactDetailDto> {
    let (chat, id) = (ChatId(chat_id), ArtifactId(id));
    let artifact = state
        .store
        .get_artifact(chat, id)
        .await?
        .ok_or_else(no_artifact)?;
    let shown = state
        .store
        .artifact_version(chat, id, version)
        .await?
        .ok_or_else(|| AppError::not_found("version"))?;
    let versions = state.store.artifact_versions(chat, id).await?;
    let frame_url = (artifact.kind != ArtifactKind::Markdown)
        .then(|| protocol::frame_url(&origin(&app), id.0, shown.version));
    Ok(ArtifactDetailDto {
        artifact: artifact.into(),
        versions: versions.into_iter().map(Into::into).collect(),
        version: shown.version,
        content: shown.content,
        frame_url,
    })
}

/// Saves a version written by the user: `content` as edited in the panel, or the content
/// of the earlier version `restore`. Exactly one of the two.
#[tauri::command]
pub async fn save_artifact_version(
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
    content: Option<String>,
    restore: Option<u32>,
) -> AppResult<ArtifactDto> {
    let (chat, id) = (ChatId(chat_id), ArtifactId(id));
    let (content, note) = match (content, restore) {
        (Some(content), None) => (content, None),
        (None, Some(version)) => {
            let old = state
                .store
                .artifact_version(chat, id, Some(version))
                .await?
                .ok_or_else(|| AppError::not_found("version"))?;
            (
                old.content,
                Some(format!("Restored from version {version}")),
            )
        }
        _ => {
            return Err(AppError::bad_request(
                "Pass the new content, or the version to restore.",
            ));
        }
    };
    let saved = state
        .store
        .add_artifact_version(chat, id, content, ArtifactAuthor::User, note)
        .await
        .map_err(refused)?;
    Ok(saved.into())
}

/// Changes an artifact's title.
#[tauri::command]
pub async fn rename_artifact(
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
    title: String,
) -> AppResult<ArtifactDto> {
    let (chat, id) = (ChatId(chat_id), ArtifactId(id));
    let title = server::clean_title(&title).map_err(AppError::bad_request)?;
    state
        .store
        .rename_artifact(chat, id, title)
        .await
        .map_err(refused)?;
    let artifact = state
        .store
        .get_artifact(chat, id)
        .await?
        .ok_or_else(no_artifact)?;
    Ok(artifact.into())
}

/// Deletes an artifact with all its versions.
#[tauri::command]
pub async fn delete_artifact(state: State<'_, AppState>, chat_id: i64, id: i64) -> AppResult<()> {
    if state
        .store
        .delete_artifact(ChatId(chat_id), ArtifactId(id))
        .await?
    {
        Ok(())
    } else {
        Err(no_artifact())
    }
}

/// The file extension an artifact of this kind is exported with.
pub fn extension(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Markdown => "md",
        ArtifactKind::Html => "html",
        ArtifactKind::Svg => "svg",
        ArtifactKind::Mermaid => "mmd",
    }
}

/// Where an export may be written: the file the user chose in the save dialog, if it has
/// the kind's extension, its folder exists, and it is not one of the app's own folders.
/// The content is the model's, so the name must not make it anything but a document.
fn export_target(path: &str, kind: ArtifactKind, forbidden: &[PathBuf]) -> Result<PathBuf, String> {
    let chosen = Path::new(path);
    if !chosen.is_absolute() {
        return Err("Choose where to save the file.".into());
    }
    let ext = extension(kind);
    let has_ext = chosen
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ext));
    let (Some(name), Some(folder)) = (chosen.file_name(), chosen.parent()) else {
        return Err("Choose where to save the file.".into());
    };
    if !has_ext {
        return Err(format!("Give the file a name that ends in .{ext}."));
    }
    let folder = dunce::canonicalize(folder)
        .map_err(|_| "That folder does not exist. Choose another one.".to_owned())?;
    if !folder.is_dir() {
        return Err("That folder does not exist. Choose another one.".into());
    }
    for own in forbidden {
        let own = dunce::canonicalize(own).unwrap_or_else(|_| own.clone());
        if folder.starts_with(&own) {
            return Err("That folder belongs to the app. Choose another one.".into());
        }
    }
    let target = folder.join(name);
    if target.is_dir() {
        return Err("There is a folder with that name. Choose another name.".into());
    }
    Ok(target)
}

/// Writes one version's content to the file the user chose in the save dialog, and
/// returns where it was written.
#[tauri::command]
pub async fn export_artifact(
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
    version: u32,
    path: String,
) -> AppResult<String> {
    let (chat, id) = (ChatId(chat_id), ArtifactId(id));
    let artifact = state
        .store
        .get_artifact(chat, id)
        .await?
        .ok_or_else(no_artifact)?;
    let content = state
        .store
        .artifact_version(chat, id, Some(version))
        .await?
        .ok_or_else(|| AppError::not_found("version"))?
        .content;
    let forbidden: Vec<PathBuf> = crate::permissions::forbidden_for_tools(&app)
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let target = export_target(&path, artifact.kind, &forbidden).map_err(AppError::bad_request)?;
    tokio::fs::write(&target, content).await.map_err(|e| {
        AppError::new(
            "export_failed",
            "The file could not be saved.",
            "Choose another folder.",
            e.to_string(),
        )
    })?;
    Ok(target.display().to_string())
}

/// What the main window saw when it showed a version: the problems the preview frame
/// posted, or `not_shown` when it could not check it (`scripts-off`, `cannot-draw`).
/// Returns whether a tool was waiting for it.
///
/// The problems are text from a page the model wrote. They are cleaned in `render.rs`
/// and become part of a tool result; they are never anything else.
#[tauri::command]
pub fn report_artifact_render(
    id: i64,
    version: u32,
    problems: Vec<render::ProblemInput>,
    not_shown: Option<String>,
) -> bool {
    let report = match not_shown.as_deref() {
        Some("cannot-draw") => render::RenderReport::NotShown(render::NotShown::CannotDraw),
        Some(_) => render::RenderReport::NotShown(render::NotShown::ScriptsOff),
        None => {
            let (problems, total) = render::clean_problems(&problems);
            render::RenderReport::Shown { problems, total }
        }
    };
    shared().renders.resolve(id, version, report)
}

/// Whether previews run scripts on this system.
#[tauri::command]
pub fn artifact_isolation() -> Isolation {
    shared().gate.status()
}

/// Starts a check of the sandbox. Scripts are off in previews until it has passed.
#[tauri::command]
pub fn begin_artifact_check(app: AppHandle) -> CheckDto {
    let nonce = shared().gate.begin();
    CheckDto {
        url: format!("{}/__probe/{nonce}", origin(&app)),
        nonce,
    }
}

/// Ends a check with what its page posted (`None` when it never did).
#[tauri::command]
pub fn finish_artifact_check(nonce: String, report: Option<isolation::ProbeReport>) -> Isolation {
    let verdict = shared().gate.finish(&nonce, report.as_ref());
    match &verdict {
        Isolation::Proven => tracing::info!("the preview sandbox was checked: scripts are on"),
        Isolation::Broken { reason } => {
            tracing::warn!("the preview sandbox check failed, scripts stay off: {reason}");
        }
        _ => {}
    }
    verdict
}

/// The tripwire. Nothing in the app calls this command. The check's page calls it from
/// inside the preview frame with whatever script interface it finds there; when the
/// sandbox holds, that call never arrives. If it does arrive, a page in a preview frame
/// can call the app, and scripts are switched off in previews.
#[tauri::command]
pub fn artifact_canary(app: AppHandle, nonce: Option<String>) {
    tracing::error!(
        nonce = nonce.as_deref().unwrap_or(""),
        "a page in a preview frame called the app: scripts are switched off in previews"
    );
    shared().gate.tripwire(isolation::Tripwire::Command);
    announce_isolation(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_exports_with_its_extension() {
        assert_eq!(extension(ArtifactKind::Markdown), "md");
        assert_eq!(extension(ArtifactKind::Html), "html");
        assert_eq!(extension(ArtifactKind::Svg), "svg");
        assert_eq!(extension(ArtifactKind::Mermaid), "mmd");
    }

    #[test]
    fn an_export_goes_only_where_a_document_of_its_kind_may_go() {
        let base = std::env::temp_dir().join(format!("artifact-export-{}", uuid::Uuid::new_v4()));
        let docs = base.join("docs");
        let own = base.join("app-data");
        std::fs::create_dir_all(docs.join("sub.html")).unwrap();
        std::fs::create_dir_all(own.join("skills")).unwrap();
        let forbidden = vec![own.clone()];
        let at = |p: &Path| p.display().to_string();
        let target = |path: &str, kind| export_target(path, kind, &forbidden);

        let ok = target(&at(&docs.join("page.html")), ArtifactKind::Html).unwrap();
        assert_eq!(ok, dunce::canonicalize(&docs).unwrap().join("page.html"));
        assert!(target(&at(&docs.join("Page.HTML")), ArtifactKind::Html).is_ok());
        assert!(target(&at(&docs.join("notes.md")), ArtifactKind::Markdown).is_ok());
        assert!(target(&at(&docs.join("d.mmd")), ArtifactKind::Mermaid).is_ok());

        // The name decides what the system does with the file: only the kind's extension.
        for (name, kind) in [
            ("page.htm", ArtifactKind::Html),
            ("page", ArtifactKind::Html),
            ("page.html.exe", ArtifactKind::Html),
            ("run.bat", ArtifactKind::Markdown),
            (".bashrc", ArtifactKind::Markdown),
            ("picture.html", ArtifactKind::Svg),
        ] {
            let err = target(&at(&docs.join(name)), kind).unwrap_err();
            assert!(
                err.starts_with("Give the file a name that ends in ."),
                "{name}: {err}"
            );
        }
        // Not relative, not into a folder that is not there, not onto a folder.
        assert!(target("page.html", ArtifactKind::Html).is_err());
        assert!(
            target(
                &at(&base.join("missing").join("page.html")),
                ArtifactKind::Html
            )
            .is_err()
        );
        assert_eq!(
            target(&at(&docs.join("sub.html")), ArtifactKind::Html).unwrap_err(),
            "There is a folder with that name. Choose another name."
        );
        // Never into the app's own folders, however the path is spelled.
        for inside in [
            own.join("x.md"),
            own.join("skills").join("x.md"),
            docs.join("..").join("app-data").join("skills").join("x.md"),
        ] {
            assert_eq!(
                target(&at(&inside), ArtifactKind::Markdown).unwrap_err(),
                "That folder belongs to the app. Choose another one."
            );
        }
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The app's own page may frame exactly the origin previews load from, in both
    /// spellings, and the two places that name it cannot drift apart.
    #[test]
    fn the_app_frames_exactly_the_preview_origin() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().unwrap();
        let frame_src: Vec<&str> = csp
            .split(';')
            .map(str::trim)
            .find_map(|d| d.strip_prefix("frame-src "))
            .expect("a frame-src directive")
            .split_whitespace()
            .collect();
        assert_eq!(
            frame_src,
            [
                protocol::frame_origin(false, false),
                protocol::frame_origin(true, false)
            ]
        );
        // The main window loads over http on Windows; a preview address over https would
        // not be framed, so the two settings have to change together.
        assert!(
            conf["app"]["windows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|w| w.get("useHttpsScheme").is_none()),
            "with useHttpsScheme, frame-src must name the https origin"
        );
    }

    /// What keeps a preview from calling the app on Windows: its address is not the app's
    /// own, and no capability grants anything to an address that is not.
    #[test]
    fn no_capability_grants_anything_to_another_origin() {
        for (name, text) in [
            ("main", include_str!("../capabilities/main.json")),
            ("overlay", include_str!("../capabilities/overlay.json")),
            ("indicator", include_str!("../capabilities/indicator.json")),
        ] {
            let capability: serde_json::Value = serde_json::from_str(text).unwrap();
            assert!(
                capability.get("remote").is_none(),
                "{name} grants to remote origins"
            );
            assert_eq!(capability["local"], serde_json::json!(true), "{name}");
            assert!(capability.get("webviews").is_none(), "{name}");
        }
    }

    #[test]
    fn what_the_panel_gets_is_spelled_in_camel_case() {
        let dto = ArtifactDto {
            id: 3,
            chat_id: 1,
            title: "T".into(),
            kind: ArtifactKind::Html,
            versions: 2,
            latest_version: 5,
            created_at: 10,
            updated_at: 20,
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            serde_json::json!({"id": 3, "chatId": 1, "title": "T", "kind": "html", "versions": 2,
                "latestVersion": 5, "createdAt": 10, "updatedAt": 20})
        );
        let version = VersionDto {
            version: 2,
            author: ArtifactAuthor::User,
            note: None,
            created_at: 1,
            bytes: 9,
        };
        assert_eq!(
            serde_json::to_value(&version).unwrap(),
            serde_json::json!({"version": 2, "author": "user", "note": null, "createdAt": 1, "bytes": 9})
        );
    }
}
