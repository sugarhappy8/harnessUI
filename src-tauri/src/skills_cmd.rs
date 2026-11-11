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

/// The name the skill in `dir` loads with, read as `load_all` reads it: its frontmatter
/// name, or its folder name when it has none. The disabled list matches on this name.
fn loaded_name(dir: &Path) -> Result<String, tools::skills::SkillError> {
    let text = std::fs::read_to_string(dir.join("SKILL.md"))?;
    Ok(tools::skills::parse(&text, dir)?.name)
}

/// A skill unpacked into a staging folder inside the skills folder. A folder whose name
/// starts with `.import-` holds no SKILL.md of its own, so it is never loaded.
struct Staged {
    /// Removed when the import ends, whatever happens.
    stage: PathBuf,
    /// The skill's folder inside `stage`.
    dir: PathBuf,
    /// Its folder name in the skills folder.
    folder: String,
    /// The name it loads with.
    name: String,
}

fn stage_import(src: &Path, zip: bool, root: &Path) -> Result<Staged, tools::skills::SkillError> {
    let stage = root.join(format!(".import-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&stage)?;
    let staged = (|| -> Result<Staged, tools::skills::SkillError> {
        let dir = if zip {
            tools::skills::import_zip(src, &stage)?
        } else {
            tools::skills::import_folder(src, &stage)?
        };
        let folder = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = loaded_name(&dir)?;
        Ok(Staged {
            stage: stage.clone(),
            dir,
            folder,
            name,
        })
    })();
    if staged.is_err() {
        let _ = std::fs::remove_dir_all(&stage);
    }
    staged
}

/// Imports a skill into `root` turned off (ARCHITECTURE.md 10: import is a trust decision).
/// Its name goes on the disabled list before its folder is moved into place, so no reload
/// of the skills can offer it to the model. Returns its folder name.
async fn install_disabled(
    store: &store::Store,
    src: PathBuf,
    zip: bool,
    root: PathBuf,
) -> AppResult<String> {
    let staged = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || stage_import(&src, zip, &root))
            .await
            .map_err(AppError::internal)?
            .map_err(skill_error)?
    };
    let result = finish_install(store, &staged, &root).await;
    let _ = std::fs::remove_dir_all(&staged.stage);
    result.map(|()| staged.folder)
}

async fn finish_install(store: &store::Store, staged: &Staged, root: &Path) -> AppResult<()> {
    let dest = root.join(&staged.folder);
    if dest.exists() {
        return Err(skill_error(tools::skills::SkillError::Import(format!(
            "a skill named \"{}\" is already installed",
            staged.folder
        ))));
    }
    // A list that cannot be read is an error: writing a fresh one would turn back on
    // every skill the user turned off.
    let mut off: Vec<String> = store.get_setting(DISABLED).await?.unwrap_or_default();
    let added = !off.contains(&staged.name);
    if added {
        off.push(staged.name.clone());
        store.set_setting(DISABLED, &off).await?;
    }
    if let Err(e) = std::fs::rename(&staged.dir, &dest) {
        if added {
            off.retain(|n| n != &staged.name);
            let _ = store.set_setting(DISABLED, &off).await;
        }
        return Err(AppError::internal(e));
    }
    Ok(())
}

/// Imports a skill from a folder or a zip the user picks; returns its folder name. The
/// skill starts turned off: its text becomes instructions to the model (10).
#[tauri::command]
pub async fn import_skill(
    app: AppHandle,
    state: State<'_, AppState>,
    zip: bool,
) -> AppResult<Option<String>> {
    let Some(src) = pick(&app, zip).await else {
        return Ok(None);
    };
    let root = skills_root(&app).ok_or_else(|| AppError::internal("no app data folder"))?;
    std::fs::create_dir_all(&root).map_err(AppError::internal)?;
    let folder = install_disabled(&state.store, src, zip, root).await?;
    reload_skills(&app).await;
    Ok(Some(folder))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the system temp folder, removed by the test.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "harness-skills-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(dir: &Path, frontmatter: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\n{frontmatter}\n---\nDo the thing.\n"),
        )
        .unwrap();
    }

    async fn off(store: &store::Store) -> Vec<String> {
        store
            .get_setting(DISABLED)
            .await
            .unwrap()
            .unwrap_or_default()
    }

    /// ARCHITECTURE.md 10: an imported skill starts turned off. The disabled list holds
    /// the name the skill loads with, which can differ from its folder name.
    #[tokio::test]
    async fn an_imported_skill_starts_disabled() {
        let tmp = scratch("import");
        let root = tmp.join("skills");
        std::fs::create_dir_all(&root).unwrap();
        // A frontmatter name that is not a valid folder name.
        let named = tmp.join("src-a");
        write_skill(&named, "name: My Skill\ndescription: Has a name.");
        // No name: it loads with its folder name, which the import changes.
        let unnamed = tmp.join("Cool Tool");
        write_skill(&unnamed, "description: Has no name.");

        let store = store::Store::open_in_memory().unwrap();
        store
            .set_setting(DISABLED, &vec!["older".to_string()])
            .await
            .unwrap();
        let a = install_disabled(&store, named, false, root.clone())
            .await
            .unwrap();
        let b = install_disabled(&store, unnamed, false, root.clone())
            .await
            .unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("my-skill", "cool-tool"));

        let (loaded, _) = tools::skills::load_all(&root);
        let names: Vec<&str> = loaded.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["cool-tool", "My Skill"]);
        let off = off(&store).await;
        for name in &names {
            assert!(
                off.iter().any(|n| n == name),
                "the imported skill \"{name}\" is not turned off: {off:?}"
            );
        }
        assert!(
            off.iter().any(|n| n == "older"),
            "an earlier entry was lost"
        );
        // What `reload_skills` offers to the model: nothing that was just imported.
        assert!(loaded.iter().all(|s| off.contains(&s.name)));
        // No staging folder is left behind.
        let leftovers: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".import-"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Importing a skill that is already installed fails, changes nothing and leaves no
    /// staging folder.
    #[tokio::test]
    async fn a_second_import_of_the_same_skill_is_refused() {
        let tmp = scratch("twice");
        let root = tmp.join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let src = tmp.join("helper");
        write_skill(&src, "name: helper\ndescription: Helps.");
        let store = store::Store::open_in_memory().unwrap();
        install_disabled(&store, src.clone(), false, root.clone())
            .await
            .unwrap();
        // The user turned it on.
        store
            .set_setting(DISABLED, &Vec::<String>::new())
            .await
            .unwrap();
        let err = install_disabled(&store, src, false, root.clone())
            .await
            .unwrap_err();
        assert!(err.details.contains("already installed"), "{err:?}");
        assert!(
            off(&store).await.is_empty(),
            "a refused import changed the disabled list"
        );
        let entries: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["helper".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
