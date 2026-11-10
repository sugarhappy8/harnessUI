//! Settings → Skills (ARCHITECTURE.md 10): list, import (folder or zip, picked in Rust),
//! enable/disable, delete, create and edit with the strict validator, and the shared
//! `~/.agents/skills` folder switch.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::toolset::{SHARED_SKILLS, reload_skills, skills_root};

const DISABLED: &str = "disabledSkills";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub dir: String,
    pub enabled: bool,
    /// Imported into the app (editable, deletable) or read from the shared folder.
    pub shared: bool,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillListDto {
    pub skills: Vec<SkillDto>,
    /// Folders that hold no loadable skill, with the reason.
    pub problems: Vec<SkillProblemDto>,
    pub shared_enabled: bool,
    pub shared_path: Option<String>,
    pub folder: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillProblemDto {
    pub path: String,
    pub error: String,
}

fn shared_root(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .home_dir()
        .ok()
        .map(|h| h.join(".agents").join("skills"))
}

fn skill_error(e: tools::skills::SkillError) -> AppError {
    AppError::new(
        "skill",
        "The skill could not be imported.",
        "Check the folder or zip and try again.",
        e.to_string(),
    )
}

async fn disabled(state: &AppState) -> Vec<String> {
    state
        .store
        .get_setting(DISABLED)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

#[tauri::command]
pub async fn list_skills(app: AppHandle, state: State<'_, AppState>) -> AppResult<SkillListDto> {
    let off = disabled(&state).await;
    let shared_enabled: bool = state
        .store
        .get_setting(SHARED_SKILLS)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
    let mut roots: Vec<(PathBuf, bool)> = Vec::new();
    if let Some(r) = skills_root(&app) {
        roots.push((r, false));
    }
    if shared_enabled && let Some(r) = shared_root(&app) {
        roots.push((r, true));
    }
    let mut skills = Vec::new();
    let mut problems = Vec::new();
    for (root, shared) in roots {
        let (ok, failed) = tokio::task::spawn_blocking(move || tools::skills::load_all(&root))
            .await
            .unwrap_or_default();
        for s in ok {
            if skills.iter().any(|x: &SkillDto| x.name == s.name) {
                problems.push(SkillProblemDto {
                    path: s.dir.display().to_string(),
                    error: format!("another skill is already named \"{}\"", s.name),
                });
                continue;
            }
            skills.push(SkillDto {
                enabled: !off.contains(&s.name),
                name: s.name,
                description: s.description,
                dir: s.dir.display().to_string(),
                shared,
                license: s.license,
                compatibility: s.compatibility,
                allowed_tools: s.allowed_tools,
                warnings: s.warnings,
            });
        }
        problems.extend(
            failed
                .into_iter()
                .filter(|(p, _)| !is_staging(p))
                .map(|(p, e)| SkillProblemDto {
                    path: p.display().to_string(),
                    error: e,
                }),
        );
    }
    Ok(SkillListDto {
        skills,
        problems,
        shared_enabled,
        shared_path: shared_root(&app).map(|p| p.display().to_string()),
        folder: skills_root(&app).map(|p| p.display().to_string()),
    })
}

fn is_staging(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(".import-"))
}

async fn pick(app: &AppHandle, zip: bool) -> Option<PathBuf> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file();
    if zip {
        dialog
            .set_title("Import a skill (.zip)")
            .add_filter("Skill archive", &["zip", "skill"])
            .pick_file(move |p| {
                let _ = tx.send(p);
            });
    } else {
        dialog
            .set_title("Import a skill folder")
            .pick_folder(move |p| {
                let _ = tx.send(p);
            });
    }
    rx.await.ok().flatten().and_then(|p| p.into_path().ok())
}

/// Imports a skill from a folder or a zip the user picks; returns its name.
#[tauri::command]
pub async fn import_skill(app: AppHandle, zip: bool) -> AppResult<Option<String>> {
    let Some(src) = pick(&app, zip).await else {
        return Ok(None);
    };
    let root = skills_root(&app).ok_or_else(|| AppError::internal("no app data folder"))?;
    std::fs::create_dir_all(&root).map_err(AppError::internal)?;
    let dest = tokio::task::spawn_blocking(move || {
        if zip {
            tools::skills::import_zip(&src, &root)
        } else {
            tools::skills::import_folder(&src, &root)
        }
    })
    .await
    .map_err(AppError::internal)?
    .map_err(skill_error)?;
    reload_skills(&app).await;
    Ok(dest.file_name().map(|n| n.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn set_skill_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> AppResult<()> {
    let mut off = disabled(&state).await;
    off.retain(|n| n != &name);
    if !enabled {
        off.push(name);
    }
    state.store.set_setting(DISABLED, &off).await?;
    reload_skills(&app).await;
    Ok(())
}

/// The folder of an app-owned skill (never one in the shared folder).
fn own_skill_dir(app: &AppHandle, name: &str) -> AppResult<PathBuf> {
    let root = skills_root(app).ok_or_else(|| AppError::internal("no app data folder"))?;
    let (skills, _) = tools::skills::load_all(&root);
    let s = skills.into_iter().find(|s| s.name == name).ok_or_else(|| {
        AppError::bad_request("Only skills imported into the app can be changed here.")
    })?;
    let dir = dunce::canonicalize(&s.dir).map_err(AppError::internal)?;
    let root = dunce::canonicalize(&root).map_err(AppError::internal)?;
    if dir.parent() != Some(root.as_path()) {
        return Err(AppError::bad_request(
            "That skill is not in the app's skills folder.",
        ));
    }
    Ok(dir)
}

#[tauri::command]
pub async fn delete_skill(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
) -> AppResult<()> {
    let dir = own_skill_dir(&app, &name)?;
    tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir))
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
    let mut off = disabled(&state).await;
    off.retain(|n| n != &name);
    state.store.set_setting(DISABLED, &off).await?;
    reload_skills(&app).await;
    Ok(())
}

/// SKILL.md of an app-owned skill, for the editor.
#[tauri::command]
pub async fn get_skill_source(app: AppHandle, name: String) -> AppResult<String> {
    let dir = own_skill_dir(&app, &name)?;
    std::fs::read_to_string(dir.join("SKILL.md")).map_err(AppError::internal)
}

/// The strict validator's findings for an editor's text, for the skill folder `folder`.
#[tauri::command]
pub fn validate_skill(text: String, folder: String) -> Vec<String> {
    tools::skills::validate_strict(&text, &folder)
}

/// Saves SKILL.md for a new or existing app-owned skill. The text must pass the strict
/// validator with the folder named after the skill; otherwise the findings come back and
/// nothing is written.
#[tauri::command]
pub async fn save_skill(
    app: AppHandle,
    folder: String,
    text: String,
    is_new: bool,
) -> AppResult<Vec<String>> {
    let problems = tools::skills::validate_strict(&text, &folder);
    if !problems.is_empty() {
        return Ok(problems);
    }
    if folder.is_empty()
        || folder.len() > 64
        || !folder
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Ok(vec![
            "the folder name may only use lowercase letters, digits and hyphens".into(),
        ]);
    }
    let root = skills_root(&app).ok_or_else(|| AppError::internal("no app data folder"))?;
    let dir = if is_new {
        let d = root.join(&folder);
        if d.exists() {
            return Ok(vec![format!("a skill named \"{folder}\" already exists")]);
        }
        std::fs::create_dir_all(&d).map_err(AppError::internal)?;
        d
    } else {
        own_skill_dir(&app, &folder)?
    };
    let tmp = dir.join(".SKILL.md.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(AppError::internal)?;
    std::fs::rename(&tmp, dir.join("SKILL.md")).map_err(AppError::internal)?;
    reload_skills(&app).await;
    Ok(Vec::new())
}

/// Also read skills from `~/.agents/skills` (shared with other agent apps).
#[tauri::command]
pub async fn set_shared_skills(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> AppResult<()> {
    state.store.set_setting(SHARED_SKILLS, &enabled).await?;
    reload_skills(&app).await;
    Ok(())
}

/// Opens the app's skills folder in the file manager.
#[tauri::command]
pub async fn open_skills_folder(app: AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let root = skills_root(&app).ok_or_else(|| AppError::internal("no app data folder"))?;
    std::fs::create_dir_all(&root).map_err(AppError::internal)?;
    app.opener()
        .open_path(root.display().to_string(), None::<&str>)
        .map_err(|e| AppError::internal(e.to_string()))
}

/// Skills loaded in a chat (their text is in its system message), and unloading one.
#[tauri::command]
pub fn loaded_skills(state: State<'_, AppState>, chat_id: store::ChatId) -> Vec<String> {
    state
        .skills
        .loaded_in(chat_id.0)
        .into_iter()
        .map(|s| s.name)
        .collect()
}

#[tauri::command]
pub fn unload_skill(state: State<'_, AppState>, chat_id: store::ChatId, name: String) {
    state.skills.unload(chat_id.0, &name);
}
