//! *Set it up yourself* (design 7.5, Task S9): the commands that show and change how a model
//! is set up.
//!
//! - **A set-up plan** ([`setup_plan`]) of a searched file or a file on this computer, with
//!   another recipe or files the person chose (`setup_plan::plan_source`). A file Studio does
//!   not recognise, or one whose header was not read, can be set up with a recipe the person
//!   chooses: untested. Any other file Studio can't run is refused with its label's words
//!   (`studio_cannot_run`). Of an installed model too, with the change the person is about to
//!   make (`setup_plan::plan_installed`, as the change itself plans it): the side panel's Files
//!   shows what it would download, its licences and the total before anything starts (design
//!   7.4.3, Task U8's fix round 1).
//! - **An installed model's files** ([`model_files`]): its recipe, the recipes it can be set
//!   up as, and each slot with its file. Sends nothing.
//! - **Changing them** ([`prepare_set_model_files`]): another recipe or another file for any
//!   slot, planned with the model's own files kept (`setup_plan::plan_installed`). Refused
//!   while a run on this computer uses the model. When a file must be fetched, one download
//!   group brings it and its completion replaces the model's row (same id); otherwise the row
//!   is rewritten at once. A file the old set-up used that nothing uses now is removed, inside
//!   the models folder only; a file from this computer stays the person's own.
//!
//! Each command is a thin wrapper over a plain function taking what it reads
//! ([`LocalCmdDeps`]), as Get models' are, and checks that the main window called it.

use std::sync::Arc;

use models::gen_catalog::GenRole;
use models::sd::setup::PartSource;
use tauri::{AppHandle, Manager, State};

use super::errors;
use super::local_cmd::{
    LocalCmdDeps, app_deps, app_deps_for_writing, plan, plan_deps, recipe_choice, recipe_choices,
    setup_plan_dto, slots_dto,
};
use super::local_models::{
    DownloadDeps, InstalledFiles, LocalSource, PreparedDownload, installed_version,
    prepared_download,
};
use super::manager::StudioManager;
use super::setup_plan::{
    PlanDeps, Planned, own_parts, plan_installed, plan_source, ready_to_fetch, token_for_gated,
};
use crate::commands::only_main_window;
use crate::downloads::{
    DownloadManager, GroupMeta, gen_model_downloading, register_present_gen_model,
};
use crate::dto::{
    ModelFilesDto, PartDto, PlannedSlotDto, SetModelFilesDto, SetupChoice, SetupPlanDto,
};
use crate::error::{AppError, AppResult};

/// The set-up of the file `source` names (a searched file, at its repo's commit, or a file on
/// this computer) with the person's `choice`: another recipe, files for some slots
/// (`setup_plan::plan_source`). For an installed model, the set-up `choice` would give it,
/// planned as *Set it up yourself* plans it (`setup_plan::plan_installed`: its own files kept,
/// the choice on top), so what a change downloads is shown before it starts; one that fetches
/// a gated repository's file with no Hugging Face token is `hf_needs_token`, as the change
/// would be. Nothing is downloaded or changed. A catalog model is `studio_not_supported_yet`.
pub async fn setup_plan(
    deps: &LocalCmdDeps,
    source: &LocalSource,
    choice: &SetupChoice,
) -> AppResult<SetupPlanDto> {
    let catalog = deps.catalog.gen_catalog().await;
    let pd = plan_deps(deps, &catalog);
    let planned = match source {
        LocalSource::Installed { gen_model_id } => {
            let row = installed_row(deps, gen_model_id).await?;
            let planned = plan_installed(&pd, &row, choice).await?;
            token_for_gated(&planned, pd.hf)?;
            planned
        }
        _ => plan_source(&pd, source, choice).await?,
    };
    setup_plan_dto(deps, &planned).await
}

/// Installed model `gen_model_id`'s files, as its set-up has them
/// (`setup_plan::plan_installed` with nothing chosen): each slot of its recipe with its file,
/// the recipes it can be set up as, whether it is untested and whether every slot it needs
/// has a file. A model with no recipe yet lists its own files, with every recipe Studio runs.
/// A model not installed is `not_found`. Sends nothing.
pub async fn model_files(deps: &LocalCmdDeps, gen_model_id: &str) -> AppResult<ModelFilesDto> {
    let row = installed_row(deps, gen_model_id).await?;
    if row.recipe_id.is_none() {
        return unplaced_files(&row);
    }
    let catalog = deps.catalog.gen_catalog().await;
    let planned = plan_installed(&plan_deps(deps, &catalog), &row, &SetupChoice::default()).await?;
    files_dto(deps, &row.id, &planned)
}

/// What *Set it up yourself* takes once planned: the model's files as they are now (its row
/// rewritten already), or as the download that brings the missing ones makes them.
pub(crate) struct PreparedSetUp {
    pub files: ModelFilesDto,
    /// The download whose completion replaces the model's row; `None` when nothing is fetched.
    pub download: Option<PreparedDownload>,
}

/// *Set it up yourself* on installed model `gen_model_id` with the person's `choice`: another
/// recipe, another file for any slot. Refused with `studio_model_in_use` while a run on this
/// computer waits for the model or uses it (`in_use`), before anything else, and with
/// `studio_setup_in_progress` while a download not finished yet adds or replaces it (an
/// earlier change's, *Download again*: whichever completed last would win). Planned with the
/// model's own files kept and the choices on top (`setup_plan::plan_installed`); a set-up that
/// misses a part, or fetches a gated repository's file with no Hugging Face token, is refused
/// as a download's is, and so is one that does not fit on the disk. When a Hub file must be
/// fetched, its download is returned, not started: its completion replaces the row (same id).
/// Otherwise the row is rewritten at once (`downloads::register_present_gen_model`: the model
/// file checked again, then the files only the old set-up used removed once no run holds the
/// model), unless nothing changed. A model not installed is `not_found`.
pub(crate) async fn prepare_set_model_files(
    deps: &LocalCmdDeps,
    in_use: &(dyn Fn(&str) -> bool + Sync),
    gen_model_id: &str,
    choice: &SetupChoice,
) -> AppResult<PreparedSetUp> {
    if in_use(gen_model_id) {
        return Err(errors::model_in_use());
    }
    // A download not finished yet would replace this change when it completes, or this
    // change the files it counts on.
    if gen_model_downloading(&deps.store, gen_model_id).await? {
        return Err(errors::setup_in_progress());
    }
    let row = installed_row(deps, gen_model_id).await?;
    let catalog = deps.catalog.gen_catalog().await;
    let pd = plan_deps(deps, &catalog);
    let planned = plan_installed(&pd, &row, choice).await?;
    ready_to_fetch(&planned, pd.hf)?;
    let files = files_dto(deps, &row.id, &planned)?;
    let download_deps = DownloadDeps {
        plan: pd,
        store: &deps.store,
        models_dir: &deps.models_dir,
        free_bytes: deps.free_disk_bytes,
        engine: &deps.engine_need,
    };
    let prepared = prepared_download(
        &download_deps,
        planned.entry,
        Some(&planned.plan),
        None,
        row.source,
    )?;
    if !prepared.files.is_empty() {
        return Ok(PreparedSetUp {
            files,
            download: Some(prepared),
        });
    }
    if unchanged(&row, &prepared.meta) {
        return Ok(PreparedSetUp {
            files,
            download: None,
        });
    }
    register_present_gen_model(&deps.store, &deps.models_dir, &prepared.meta, in_use).await?;
    // The model's files as its new row has them.
    let installed = InstalledFiles::read(&deps.store).await?;
    let pd = PlanDeps {
        installed: &installed,
        ..plan_deps(deps, &catalog)
    };
    let row = installed_row(deps, gen_model_id).await?;
    let planned = plan_installed(&pd, &row, &SetupChoice::default()).await?;
    Ok(PreparedSetUp {
        files: files_dto(deps, &row.id, &planned)?,
        download: None,
    })
}

/// Installed model `id`'s row; `not_found` when it is not installed.
async fn installed_row(deps: &LocalCmdDeps, id: &str) -> AppResult<store::GenModel> {
    deps.store
        .get_gen_model(id)
        .await?
        .ok_or_else(|| AppError::not_found("model"))
}

/// A planned set-up of installed model `id` as its files: each slot with its file (as its
/// download would take it here: `local_cmd::plan`), its recipe and the recipes offered.
fn files_dto(deps: &LocalCmdDeps, id: &str, planned: &Planned) -> AppResult<ModelFilesDto> {
    let download = plan(deps, planned.entry.clone(), Some(&planned.plan))?;
    let untested = planned.plan.untested;
    Ok(ModelFilesDto {
        gen_model_id: id.to_owned(),
        recipe: recipe_choice(&planned.plan.recipe_id),
        recipes: recipe_choices(planned.entry.version, untested),
        slots: slots_dto(planned, &download),
        untested,
        can_run: planned.plan.can_run(),
    })
}

/// The files of a model with no recipe yet: its model file `main`, each other file
/// `installed`, none required; every recipe Studio runs offered; untested, and it cannot run.
fn unplaced_files(row: &store::GenModel) -> AppResult<ModelFilesDto> {
    let slots = own_parts(row)?
        .into_iter()
        .map(|(part, file)| {
            let main = matches!(part.role, GenRole::Diffusion | GenRole::Model);
            PlannedSlotDto {
                role: part.role,
                required: false,
                source: if main {
                    PartSource::Main
                } else {
                    PartSource::Installed
                },
                part: Some(PartDto {
                    role: part.role,
                    repo: part.repo.clone(),
                    path: match &part.local {
                        Some(_) => file.display().to_string(),
                        None => part.path.clone(),
                    },
                    size_bytes: part.size,
                    licence_id: part.license.clone(),
                    already_here: true,
                    local: part.local.is_some(),
                }),
            }
        })
        .collect();
    Ok(ModelFilesDto {
        gen_model_id: row.id.clone(),
        recipe: None,
        recipes: recipe_choices(installed_version(row), true),
        slots,
        untested: true,
        can_run: false,
    })
}

/// Whether the row `meta`'s group would write is `row` as it is: the same recipe, and each
/// part of the same role at the same file, in order. Nothing is rewritten then, so a model of
/// the recommended list or a 0.1.x family keeps its row.
fn unchanged(row: &store::GenModel, meta: &GroupMeta) -> bool {
    let Some(group) = &meta.generator else {
        return false;
    };
    let old: Vec<(Option<&str>, Option<&str>)> = row
        .parts
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            (
                p.get("role").and_then(serde_json::Value::as_str),
                p.get("file").and_then(serde_json::Value::as_str),
            )
        })
        .collect();
    let new: Vec<(Option<&str>, Option<&str>)> = group
        .parts
        .iter()
        .map(|p| (Some(p.role.as_str()), p.present.as_deref()))
        .collect();
    row.recipe_id == group.recipe_id && old == new
}

/// The set-up of a searched file or a file on this computer, with the person's `choice`
/// (another recipe, files for some slots). Asks Hugging Face for a searched file's listing
/// (its headers come from the cache once its repo was opened) and for the listing of a file
/// the person chose from another repo.
#[tauri::command]
pub async fn studio_setup_plan(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
    source: LocalSource,
    choice: SetupChoice,
) -> AppResult<SetupPlanDto> {
    only_main_window(window.label())?;
    let hf = crate::hub::hf_client(&app).await?;
    setup_plan(&app_deps(&app, &studio, Some(hf)).await, &source, &choice).await
}

/// *Set it up yourself*: installed model `gen_model_id`'s files. Sends nothing.
#[tauri::command]
pub async fn studio_model_files(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
    gen_model_id: String,
) -> AppResult<ModelFilesDto> {
    only_main_window(window.label())?;
    model_files(&app_deps(&app, &studio, None).await, &gen_model_id).await
}

/// *Set it up yourself*: installed model `gen_model_id` set up with the person's `choice`
/// (another recipe, another file for any slot), rewritten at once or by the download it
/// starts (the Studio engine's pack with it where it is needed). Refused while a run on this
/// computer uses the model; a failed read of the installed models' files refuses it before
/// anything changes (`local_cmd::app_deps_for_writing`). Asks Hugging Face only for the
/// listing of a file the person chose from a repo.
#[tauri::command]
pub async fn studio_set_model_files(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
    gen_model_id: String,
    choice: SetupChoice,
) -> AppResult<SetModelFilesDto> {
    only_main_window(window.label())?;
    let hf = crate::hub::hf_client(&app).await?;
    // Strict: the installed models' files guard which files the change keeps or removes.
    let deps = app_deps_for_writing(&app, &studio, Some(hf)).await?;
    let in_use = |id: &str| studio.local_uses_model(id);
    let prepared = prepare_set_model_files(&deps, &in_use, &gen_model_id, &choice).await?;
    let download_group = match prepared.download {
        None => None,
        Some(download) => {
            let downloads = app.state::<Arc<DownloadManager>>().inner().clone();
            let group = downloads.start(&app, download.meta, download.files).await?;
            if download.needs_engine
                && let Err(e) = super::engine_cmd::start_engine_download(&app).await
            {
                tracing::warn!(code = %e.code, "the Studio engine did not download with the model");
            }
            Some(group)
        }
    };
    Ok(SetModelFilesDto {
        files: prepared.files,
        download_group,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use models::download::destination;
    use models::gen_catalog::{GenPart, GenRole};
    use models::sd::detect::SdVersion;
    use models::sd::matching::in_scope_recipes;
    use models::sd::setup::PartSource;
    use models::sd::testing::{file_of, fixture, safetensors};
    use serde_json::{Value, json};

    use super::*;
    use crate::downloads::{DownloadManager, finish_gen_group, finish_gen_group_while};
    use crate::dto::LocalModelsDto;
    use crate::dto::{SlotFile, SlotOverride};
    use crate::error::AppError;
    use crate::studio::local_cmd::local_models;
    use crate::studio::local_cmd::tests::{
        COMMIT, RepoHub, TestRepo, bundled, gpu, hub, repo_deps,
    };
    use crate::studio::local_models::tests::small_weights;
    use crate::studio::local_models::{
        DownloadDeps, InstalledFiles, KeptFiles, LocalSource, entry_from_installed,
        prepare_download, remove_kept_files,
    };
    use crate::studio::setup_plan::PlanDeps;
    use crate::studio::test_support::Scratch;

    const FLUX: &str = "hf:o/r:f.gguf";
    const MYSTERY: &str = "someone/mystery-model";

    /// Get models asking a Hub on this computer that serves `repos`, with the bundled
    /// catalog, a models folder of its own and a download manager that records groups (never
    /// runs them); `held` is the model a run on this computer uses, if any.
    struct T {
        repos: Vec<TestRepo>,
        _hub: RepoHub,
        deps: LocalCmdDeps,
        downloads: DownloadManager,
        held: Option<String>,
        scratch: Scratch,
    }

    async fn harness(repos: Vec<TestRepo>) -> T {
        let scratch = Scratch::new();
        let served = hub(repos.clone()).await;
        let deps = repo_deps(&served, gpu(24, 64), &scratch);
        T {
            repos,
            _hub: served,
            deps,
            downloads: DownloadManager::default(),
            held: None,
            scratch,
        }
    }

    /// [`harness`] with nothing served and the row `install` writes (its files in the models
    /// folder) installed.
    async fn harness_with(install: fn(&Path) -> store::NewGenModel) -> T {
        let t = harness(Vec::new()).await;
        let row = install(&t.deps.models_dir);
        t.deps.store.insert_gen_model(row).await.unwrap();
        t
    }

    impl T {
        /// The installed models' files read again, as each command reads them.
        async fn refresh(&mut self) {
            self.deps.installed_files = InstalledFiles::read(&self.deps.store).await.unwrap();
        }

        async fn model_files(&mut self, id: &str) -> AppResult<ModelFilesDto> {
            self.refresh().await;
            model_files(&self.deps, id).await
        }

        /// `studio_set_model_files`, a download it needs recorded but not run.
        async fn set_model_files(
            &mut self,
            id: &str,
            choice: SetupChoice,
        ) -> AppResult<SetModelFilesDto> {
            self.refresh().await;
            let held = self.held.clone();
            let in_use = move |m: &str| held.as_deref() == Some(m);
            let prepared = prepare_set_model_files(&self.deps, &in_use, id, &choice).await?;
            let download_group = match prepared.download {
                Some(d) => Some(
                    self.downloads
                        .record_group(&self.deps.store, &d.meta, &d.files)
                        .await?,
                ),
                None => None,
            };
            Ok(SetModelFilesDto {
                files: prepared.files,
                download_group,
            })
        }

        /// A run on this computer uses model `id`.
        fn hold_run_on(&mut self, id: &str) {
            self.held = Some(id.into());
        }

        /// A file outside the models folder.
        fn outside_file(&self, name: &str, bytes: Vec<u8>) -> PathBuf {
            let dir = self.scratch.path().join("outside ü");
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }

        async fn local_models(&mut self) -> LocalModelsDto {
            self.refresh().await;
            local_models(&self.deps).await.unwrap()
        }

        async fn setup_plan(&mut self, source: LocalSource, choice: SetupChoice) -> SetupPlanDto {
            self.refresh().await;
            setup_plan(&self.deps, &source, &choice).await.unwrap()
        }

        async fn setup_plan_err(&mut self, source: LocalSource, choice: SetupChoice) -> AppError {
            self.refresh().await;
            setup_plan(&self.deps, &source, &choice).await.unwrap_err()
        }

        async fn row(&self, id: &str) -> store::GenModel {
            self.deps.store.get_gen_model(id).await.unwrap().unwrap()
        }

        /// Download on a searched file with `choice`, the group recorded, not run.
        async fn download(&mut self, source: LocalSource, choice: SetupChoice) -> String {
            self.refresh().await;
            let catalog = bundled();
            let deps = DownloadDeps {
                plan: PlanDeps {
                    hf: self.deps.hf.as_ref(),
                    detect: &self.deps.detect,
                    catalog: &catalog,
                    installed: &self.deps.installed_files,
                },
                store: &self.deps.store,
                models_dir: &self.deps.models_dir,
                free_bytes: self.deps.free_disk_bytes,
                engine: &self.deps.engine_need,
            };
            let prepared = prepare_download(&deps, &source, Some(&choice))
                .await
                .unwrap();
            self.downloads
                .record_group(&self.deps.store, &prepared.meta, &prepared.files)
                .await
                .unwrap()
        }

        /// Puts each file `group` fetches in its place (what its repo serves, else a small
        /// safetensors file) and finishes it as its task does.
        async fn complete(&self, group: &str) {
            self.write_files_of(group).await;
            finish_gen_group(&self.deps.store, &self.deps.models_dir, group)
                .await
                .unwrap();
        }

        /// Puts each file `group` fetches in its place, at its pinned size (what is served
        /// first, then a sparse rest), as a finished download leaves it.
        async fn write_files_of(&self, group: &str) {
            let rows = self
                .deps
                .store
                .list_downloads(Some(group.to_owned()))
                .await
                .unwrap();
            for row in rows {
                let bytes = self
                    .repos
                    .iter()
                    .find(|r| r.id() == row.repo)
                    .and_then(|r| r.served(&row.file))
                    .map_or_else(small_weights, <[u8]>::to_vec);
                let dest = Path::new(&row.dest_path);
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                std::fs::write(dest, &bytes).unwrap();
                if row.size_bytes > bytes.len() as u64 {
                    std::fs::OpenOptions::new()
                        .write(true)
                        .open(dest)
                        .unwrap()
                        .set_len(row.size_bytes)
                        .unwrap();
                }
            }
        }

        /// [`T::complete`] while a run on this computer holds the model the test holds.
        async fn complete_while_held(&self, group: &str) {
            let held = self.held.clone();
            let in_use = move |m: &str| held.as_deref() == Some(m);
            self.write_files_of(group).await;
            finish_gen_group_while(&self.deps.store, &self.deps.models_dir, group, &in_use)
                .await
                .unwrap();
        }

        /// The files kept back for later removal, by model.
        async fn kept(&self) -> Vec<KeptFiles> {
            self.deps
                .store
                .get_setting("studioKeptFiles")
                .await
                .unwrap()
                .unwrap_or_default()
        }
    }

    /// The bytes of a file whose header is the detection fixture `name`'s.
    fn fixture_bytes(name: &str) -> Vec<u8> {
        file_of(&fixture(name))
    }

    /// Writes `bytes` at `<models>/<repo>/<path>`; returns where.
    fn place(models: &Path, repo: &str, path: &str, bytes: &[u8]) -> PathBuf {
        let file = destination(models, repo, path).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, bytes).unwrap();
        file
    }

    /// The companion table's file `id` for `role`, in its place in `models` at its pinned
    /// size (a sparse file).
    fn companion_part(models: &Path, role: GenRole, id: &str) -> (GenPart, PathBuf) {
        let c = bundled()
            .companions
            .files
            .into_iter()
            .find(|c| c.id == id)
            .unwrap();
        let file = destination(models, &c.repo, &c.path).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::File::create(&file)
            .unwrap()
            .set_len(c.size)
            .unwrap();
        let part = GenPart {
            role,
            repo: c.repo,
            revision: c.revision,
            path: c.path,
            size: c.size,
            sha256: c.sha256,
            license: c.license,
            local: None,
            folder: None,
        };
        (part, file)
    }

    /// An installed row of `parts`, each saved with its file.
    fn row_of(
        id: &str,
        family: &str,
        recipe_id: Option<&str>,
        source: store::GenSource,
        parts: Vec<(GenPart, PathBuf)>,
    ) -> store::NewGenModel {
        let size_bytes = parts.iter().map(|(p, _)| p.size).sum();
        let saved: Vec<Value> = parts
            .iter()
            .map(|(part, file)| {
                let mut v = serde_json::to_value(part).unwrap();
                v["file"] = json!(file.display().to_string());
                v
            })
            .collect();
        store::NewGenModel {
            id: id.into(),
            catalog_id: None,
            name: "r (f)".into(),
            kind: "image-edit".into(),
            family: family.into(),
            recipe_id: recipe_id.map(str::to_owned),
            parts: Value::Array(saved),
            size_bytes,
            source,
        }
    }

    /// A FLUX.1 Kontext GGUF downloaded from `o/r` as a generic model (`hf:o/r:f.gguf`, its
    /// recipe `flux1-kontext`), with the companion table's CLIP-L, T5 and decoder.
    fn installed_generic_flux(models: &Path) -> store::NewGenModel {
        let bytes = fixture_bytes("flux1-kontext");
        let file = place(models, "o/r", "f.gguf", &bytes);
        let main = GenPart {
            role: GenRole::Diffusion,
            repo: "o/r".into(),
            revision: COMMIT.into(),
            path: "f.gguf".into(),
            size: bytes.len() as u64,
            sha256: "a".repeat(64),
            license: "apache-2.0".into(),
            local: None,
            folder: None,
        };
        row_of(
            FLUX,
            "flux",
            Some("flux1-kontext"),
            store::GenSource::Search,
            vec![
                (main, file),
                companion_part(models, GenRole::ClipL, "clip-l"),
                companion_part(models, GenRole::T5xxl, "t5xxl-fp8"),
                companion_part(models, GenRole::Vae, "ae-flux1"),
            ],
        )
    }

    /// A model with no recipe yet: one diffusion file, of no family this build knows.
    fn row_without_recipe(models: &Path) -> store::NewGenModel {
        let bytes = fixture_bytes("flux1-kontext");
        let file = place(models, "o/old", "old.gguf", &bytes);
        let main = GenPart {
            role: GenRole::Diffusion,
            repo: "o/old".into(),
            revision: COMMIT.into(),
            path: "old.gguf".into(),
            size: bytes.len() as u64,
            sha256: "b".repeat(64),
            license: "apache-2.0".into(),
            local: None,
            folder: None,
        };
        row_of(
            "hf:o/old:old.gguf",
            "unknown",
            None,
            store::GenSource::Search,
            vec![(main, file)],
        )
    }

    /// [`installed_generic_flux`]'s file imported from this computer, used where it is
    /// (outside the models folder): the person's own.
    fn imported_flux(models: &Path) -> store::NewGenModel {
        let outside = models.parent().unwrap().join("my files");
        std::fs::create_dir_all(&outside).unwrap();
        let file = outside.join("my kontext.gguf");
        let bytes = fixture_bytes("flux1-kontext");
        std::fs::write(&file, &bytes).unwrap();
        let main = GenPart {
            role: GenRole::Diffusion,
            repo: String::new(),
            revision: String::new(),
            path: "my kontext.gguf".into(),
            size: bytes.len() as u64,
            sha256: "c".repeat(64),
            license: String::new(),
            local: Some(file.clone()),
            folder: None,
        };
        row_of(
            "local:cccccccccccccccc",
            "flux",
            Some("flux1-kontext"),
            store::GenSource::Import,
            vec![
                (main, file),
                companion_part(models, GenRole::ClipL, "clip-l"),
                companion_part(models, GenRole::T5xxl, "t5xxl-fp8"),
                companion_part(models, GenRole::Vae, "ae-flux1"),
            ],
        )
    }

    fn choice_override(role: GenRole, file: SlotFile) -> SetupChoice {
        SetupChoice {
            recipe: None,
            overrides: vec![SlotOverride { role, file }],
        }
    }

    fn recipe(id: &str) -> SetupChoice {
        SetupChoice {
            recipe: Some(id.into()),
            overrides: Vec::new(),
        }
    }

    fn mystery(file: &str) -> LocalSource {
        LocalSource::Search {
            repo: MYSTERY.into(),
            commit: COMMIT.into(),
            file: file.into(),
        }
    }

    /// A repo of files Studio does not recognise: weights of no known kind, a file whose
    /// header is not served (so it is not read), a Lumina 2 model the engine misruns, two
    /// pickle files, and a decoder by its name.
    fn unknown_weights_repo() -> TestRepo {
        let weights = safetensors(&json!({
            "mystery.block.0.weight": {"dtype": "F16", "shape": [1], "data_offsets": [0, 2]}
        }));
        TestRepo::new(MYSTERY)
            .serving("mystery.safetensors", 6_000_000_000, weights)
            .file("broken.safetensors", 6_000_000_000.0, None)
            .file("lumina_2.safetensors", 5_219_585_680.0, Some("lumina2"))
            .file("old.ckpt", 4_265_380_512.0, None)
            .file("weights.PT", 4_265_380_512.0, None)
            .file("my_vae.safetensors", 334_643_268.0, None)
    }

    /// The files of `row`'s parts, by role.
    fn files_of(row: &store::GenModel) -> Vec<(String, String)> {
        row.parts
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["role"].as_str().unwrap().to_owned(),
                    p["file"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn files_show_each_slot_of_the_recipe_with_its_file() {
        let mut t = harness_with(installed_generic_flux).await;
        let f = t.model_files(FLUX).await.unwrap();
        assert_eq!(f.gen_model_id, FLUX);
        assert_eq!(
            f.slots
                .iter()
                .map(|s| (s.role, s.source))
                .collect::<Vec<_>>(),
            [
                (GenRole::Diffusion, PartSource::Main),
                (GenRole::ClipL, PartSource::Installed),
                (GenRole::T5xxl, PartSource::Installed),
                (GenRole::Vae, PartSource::Installed)
            ]
        );
        assert!(f.can_run && !f.untested);
        assert_eq!(
            f.recipe.as_ref().map(|r| r.id.as_str()),
            Some("flux1-kontext")
        );
        assert_eq!(
            f.recipes.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            models::sd::matching::recipes_for(SdVersion::Flux)
        );
        // Each of its files is here, the model's own (not the person's: none is local).
        for slot in &f.slots {
            let part = slot.part.as_ref().unwrap();
            assert!(part.already_here && !part.local, "{slot:?}");
        }
        // The decoder of a diffusion model alone fills `required_any [vae, tae]`: required,
        // as every other slot is, though the model's own file fills it.
        assert!(f.slots.iter().all(|s| s.required), "{:?}", f.slots);
        assert_eq!(
            t.model_files("hf:o/r:none").await.unwrap_err().code,
            "not_found"
        );
    }

    #[tokio::test]
    async fn changing_the_vae_to_a_disk_file_keeps_the_model_and_marks_it_local() {
        let mut t = harness_with(installed_generic_flux).await;
        let old_ae = PathBuf::from(&files_of(&t.row(FLUX).await)[3].1);
        let ae = t.outside_file("my ae.safetensors", fixture_bytes("ae-flux1"));
        let r = t
            .set_model_files(
                FLUX,
                choice_override(
                    GenRole::Vae,
                    SlotFile::Disk {
                        path: ae.display().to_string(),
                    },
                ),
            )
            .await
            .unwrap();
        assert!(r.download_group.is_none());
        assert!(
            r.files
                .slots
                .iter()
                .any(|s| s.role == GenRole::Vae && s.part.as_ref().unwrap().local)
        );
        // The row was rewritten at once, under the same id: the decoder is the person's file,
        // used where it is; the old decoder, which nothing uses now, is gone, and the person's
        // file stays.
        let row = t.row(FLUX).await;
        assert_eq!(row.recipe_id.as_deref(), Some("flux1-kontext"));
        let vae = &row.parts.as_array().unwrap()[3];
        assert_eq!(vae["role"], "vae");
        assert_eq!(
            Path::new(vae["file"].as_str().unwrap()),
            dunce::canonicalize(&ae).unwrap()
        );
        assert!(!vae["local"].is_null());
        assert!(!old_ae.exists() && ae.exists());
        let entry = entry_from_installed(&row, &bundled()).unwrap();
        assert!(!entry.untested);
        assert_eq!(t.local_models().await.installed[0].id, FLUX);
    }

    #[tokio::test]
    async fn choosing_a_recipe_the_detection_did_not_pick_marks_the_setup_untested() {
        let mut t = harness_with(installed_generic_flux).await;
        let before = files_of(&t.row(FLUX).await);
        let r = t.set_model_files(FLUX, recipe("z-image")).await.unwrap();
        assert!(r.files.untested);
        assert_eq!(
            r.files.recipe.as_ref().map(|r| r.id.as_str()),
            Some("z-image")
        );
        // Z-Image's own encoder is fetched; the model's file and its decoder stay.
        assert_eq!(
            r.files
                .slots
                .iter()
                .map(|s| (s.role, s.source))
                .collect::<Vec<_>>(),
            [
                (GenRole::Diffusion, PartSource::Main),
                (GenRole::Llm, PartSource::Companion),
                (GenRole::Vae, PartSource::Installed)
            ]
        );
        // An untested set-up offers every recipe Studio runs.
        assert_eq!(r.files.recipes.len(), in_scope_recipes().len());
        let group = r.download_group.expect("its encoder downloads");
        // Until the download is done the model keeps its old set-up.
        assert_eq!(files_of(&t.row(FLUX).await), before);
        t.complete(&group).await;
        let row = t.row(FLUX).await;
        assert_eq!(row.recipe_id.as_deref(), Some("z-image"));
        assert_eq!(row.family, "flux");
        assert_eq!(
            files_of(&row)
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            ["diffusion", "llm", "vae"]
        );
        assert!(entry_from_installed(&row, &bundled()).unwrap().untested);
        // The encoders the old set-up used, which nothing uses now, are gone; the model file
        // and the decoder stay.
        for (role, file) in &before {
            let kept = matches!(role.as_str(), "diffusion" | "vae");
            assert_eq!(Path::new(file).exists(), kept, "{role}");
        }
        let installed = &t.local_models().await.installed[0];
        assert!(installed.untested && !installed.needs_setup);
        assert_eq!(installed.version, Some(SdVersion::Flux));
    }

    fn installed(id: &str) -> LocalSource {
        LocalSource::Installed {
            gen_model_id: id.into(),
        }
    }

    /// Task U8's fix round 1 (design 7.4.3): a change of an installed model's set-up is planned
    /// before anything starts, as the change itself plans it, so the side panel's Files can show
    /// what it would download, its licences and the total first.
    #[tokio::test]
    async fn an_installed_models_change_is_planned_before_anything_downloads() {
        let mut t = harness_with(installed_generic_flux).await;
        let before = files_of(&t.row(FLUX).await);
        let p = t.setup_plan(installed(FLUX), recipe("z-image")).await;
        assert_eq!(p.recipe.id, "z-image");
        assert!(p.untested && p.can_download);
        assert_eq!(
            p.slots
                .iter()
                .map(|s| (s.role, s.source))
                .collect::<Vec<_>>(),
            [
                (GenRole::Diffusion, PartSource::Main),
                (GenRole::Llm, PartSource::Companion),
                (GenRole::Vae, PartSource::Installed)
            ]
        );
        // Z-Image's encoder is what it fetches: its size is the total to download, and its
        // licence is listed. The model's own files are already here.
        let llm = p.slots[1].part.as_ref().unwrap();
        assert!(!llm.already_here);
        assert_eq!(p.to_fetch_bytes, llm.size_bytes);
        assert!(
            p.licences.iter().any(|l| l.id == llm.licence_id),
            "{:?}",
            p.licences
        );
        for i in [0, 2] {
            assert!(
                p.slots[i].part.as_ref().unwrap().already_here,
                "{:?}",
                p.slots[i]
            );
        }
        // Nothing started, and the model is as it was.
        assert_eq!(files_of(&t.row(FLUX).await), before);
        assert!(t.deps.store.list_downloads(None).await.unwrap().is_empty());
        // Planned as it is, it fetches nothing.
        let same = t.setup_plan(installed(FLUX), SetupChoice::default()).await;
        assert_eq!(same.recipe.id, "flux1-kontext");
        assert_eq!(same.to_fetch_bytes, 0);
        assert_eq!(
            t.setup_plan_err(installed("hf:o/r:none"), SetupChoice::default())
                .await
                .code,
            "not_found"
        );
    }

    /// The plan of a change that fetches a gated repository's file, with no Hugging Face token,
    /// is refused as the change would be: nothing is offered to download.
    #[tokio::test]
    async fn a_planned_change_that_fetches_a_gated_file_without_a_token_is_refused() {
        let other = "3".repeat(40);
        let t5 = TestRepo::new("someone/gated-t5")
            .at(&other)
            .file("t5xxl.safetensors", 4_893_934_904.0, None)
            .gated();
        let mut t = harness(vec![t5]).await;
        let row = installed_generic_flux(&t.deps.models_dir);
        t.deps.store.insert_gen_model(row).await.unwrap();
        let choice = choice_override(
            GenRole::T5xxl,
            SlotFile::Hub {
                repo: "someone/gated-t5".into(),
                commit: other,
                path: "t5xxl.safetensors".into(),
            },
        );
        let e = t.setup_plan_err(installed(FLUX), choice).await;
        assert_eq!(e.code, "hf_needs_token");
        assert!(t.deps.store.list_downloads(None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unknown_file_can_be_set_up_by_hand_and_is_untested() {
        let mut t = harness(vec![unknown_weights_repo()]).await;
        let p = t
            .setup_plan(mystery("mystery.safetensors"), recipe("sdxl-checkpoint"))
            .await;
        assert!(p.untested && p.can_download);
        assert_eq!(
            p.slots
                .iter()
                .map(|s| (s.role, s.source))
                .collect::<Vec<_>>(),
            [
                (GenRole::Model, PartSource::Main),
                (GenRole::Vae, PartSource::Repo)
            ]
        );
        assert_eq!(p.recipes.len(), in_scope_recipes().len());
        // A file whose header was not read is set up by hand the same way.
        let p = t
            .setup_plan(mystery("broken.safetensors"), recipe("sdxl-checkpoint"))
            .await;
        assert!(p.untested && p.can_download);
        // Without a model type it has none.
        let e = t
            .setup_plan_err(mystery("mystery.safetensors"), SetupChoice::default())
            .await;
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "studio_cannot_run",
                "The Studio engine doesn't recognise this model."
            )
        );
        // A model the engine is known to misrun, and a pickle file, are never set up.
        let e = t
            .setup_plan_err(mystery("lumina_2.safetensors"), recipe("z-image"))
            .await;
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_cannot_run",
                "This is a Lumina 2 model, which the Studio engine can't run.",
                "Download the file anyway to use it elsewhere."
            )
        );
        // A pickle file is named by its own extension (ruling S9-pickle-ext).
        for (file, message) in [
            ("old.ckpt", "Studio can't read .ckpt files"),
            ("weights.PT", "Studio can't read .pt files"),
        ] {
            let e = t
                .setup_plan_err(mystery(file), recipe("sd1-checkpoint"))
                .await;
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                ("bad_request", message),
                "{file}"
            );
        }
    }

    /// A file set up by hand downloads as its plan says and joins the installed models with
    /// the recipe the person chose, untested: its version unknown.
    #[tokio::test]
    async fn an_unknown_file_set_up_by_hand_downloads_and_registers_untested() {
        let mut t = harness(vec![unknown_weights_repo()]).await;
        let group = t
            .download(mystery("mystery.safetensors"), recipe("sdxl-checkpoint"))
            .await;
        t.complete(&group).await;
        let id = format!("hf:{MYSTERY}:mystery.safetensors");
        let row = t.row(&id).await;
        assert_eq!(
            (row.recipe_id.as_deref(), row.family.as_str()),
            (Some("sdxl-checkpoint"), "unknown")
        );
        let entry = entry_from_installed(&row, &bundled()).unwrap();
        assert!(entry.untested && entry.version.is_none());
        let installed = &t.local_models().await.installed[0];
        assert!(installed.untested && installed.version.is_none());
        let f = t.model_files(&id).await.unwrap();
        assert!(f.untested && f.can_run);
        assert_eq!(f.recipes.len(), in_scope_recipes().len());
    }

    #[tokio::test]
    async fn a_model_in_use_is_not_changed() {
        let mut t = harness_with(installed_generic_flux).await;
        let before = t.row(FLUX).await;
        t.hold_run_on(FLUX);
        assert_eq!(
            t.set_model_files(FLUX, recipe("z-image"))
                .await
                .unwrap_err()
                .code,
            "studio_model_in_use"
        );
        assert_eq!(t.row(FLUX).await, before);
        assert!(t.deps.store.list_downloads(None).await.unwrap().is_empty());
    }

    /// Setting a model up as it is changes nothing: its row stays as it was (a model of the
    /// recommended list keeps its catalog id).
    #[tokio::test]
    async fn setting_a_model_up_as_it_is_changes_nothing() {
        fn listed_flux(models: &Path) -> store::NewGenModel {
            store::NewGenModel {
                catalog_id: Some("an-old-entry".into()),
                ..installed_generic_flux(models)
            }
        }
        let mut t = harness_with(listed_flux).await;
        let before = t.row(FLUX).await;
        let r = t
            .set_model_files(FLUX, SetupChoice::default())
            .await
            .unwrap();
        assert!(r.download_group.is_none() && r.files.can_run);
        assert_eq!(t.row(FLUX).await, before);
    }

    /// A set-up that misses a part is refused, and the model keeps its own.
    #[tokio::test]
    async fn a_set_up_missing_a_part_is_refused_and_changes_nothing() {
        let mut t = harness_with(installed_generic_flux).await;
        let before = t.row(FLUX).await;
        let e = t
            .set_model_files(
                FLUX,
                SetupChoice {
                    recipe: Some("z-image".into()),
                    overrides: vec![SlotOverride {
                        role: GenRole::Llm,
                        file: SlotFile::None,
                    }],
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.details.as_str()),
            ("studio_setup_incomplete", "no file for: llm")
        );
        assert_eq!(t.row(FLUX).await, before);
        assert!(t.deps.store.list_downloads(None).await.unwrap().is_empty());
    }

    /// Finding 1: a set-up download that completes while a run on this computer holds the
    /// model replaces its row, but keeps the files only the old set-up used until no run holds
    /// it; then they go, every guard checked again, and one another model has taken up since
    /// is never removed.
    #[tokio::test]
    async fn files_a_run_still_holds_stay_until_it_ends() {
        let mut t = harness_with(installed_generic_flux).await;
        let before = files_of(&t.row(FLUX).await);
        let (clip, t5) = (PathBuf::from(&before[1].1), PathBuf::from(&before[2].1));
        let group = t
            .set_model_files(FLUX, recipe("z-image"))
            .await
            .unwrap()
            .download_group
            .unwrap();
        t.hold_run_on(FLUX);
        t.complete_while_held(&group).await;
        assert_eq!(t.row(FLUX).await.recipe_id.as_deref(), Some("z-image"));
        assert!(clip.exists() && t5.exists());
        let kept = t.kept().await;
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].model, FLUX);
        let mut files = kept[0].files.clone();
        files.sort();
        let mut want = vec![clip.display().to_string(), t5.display().to_string()];
        want.sort();
        assert_eq!(files, want);
        // Still held: a later sweep keeps them.
        let held = |m: &str| m == FLUX;
        let removed = remove_kept_files(&t.deps.store, &t.deps.models_dir, &held, None)
            .await
            .unwrap();
        assert!(removed.is_empty() && clip.exists() && t5.exists());

        // Meanwhile another model takes up the T5 file (only that one).
        let (t5_part, _) = companion_part(&t.deps.models_dir, GenRole::T5xxl, "t5xxl-fp8");
        let bytes = fixture_bytes("flux1-kontext");
        let other_file = place(&t.deps.models_dir, "o/other", "other.gguf", &bytes);
        let other_main = GenPart {
            role: GenRole::Diffusion,
            repo: "o/other".into(),
            revision: COMMIT.into(),
            path: "other.gguf".into(),
            size: bytes.len() as u64,
            sha256: "e".repeat(64),
            license: "apache-2.0".into(),
            local: None,
            folder: None,
        };
        let other = row_of(
            "hf:o/other:other.gguf",
            "flux",
            Some("flux1-kontext"),
            store::GenSource::Search,
            vec![(other_main, other_file), (t5_part, t5.clone())],
        );
        t.deps.store.insert_gen_model(other).await.unwrap();

        // The run ended (at the next start no run holds anything): the CLIP file goes, the T5
        // file another model uses stays, and nothing is kept any more.
        let removed = remove_kept_files(&t.deps.store, &t.deps.models_dir, &|_| false, None)
            .await
            .unwrap();
        assert_eq!(removed, std::slice::from_ref(&clip));
        assert!(!clip.exists() && t5.exists());
        assert!(t.kept().await.is_empty());
    }

    /// Finding 3: a second change while the first change's download is unfinished is refused
    /// (whichever completed last would win), and changes nothing; once it is done, it is taken.
    #[tokio::test]
    async fn a_second_change_waits_for_the_first_ones_download() {
        let mut t = harness_with(installed_generic_flux).await;
        let group = t
            .set_model_files(FLUX, recipe("z-image"))
            .await
            .unwrap()
            .download_group
            .unwrap();
        let before = t.row(FLUX).await;
        let ae = t.outside_file("my ae.safetensors", fixture_bytes("ae-flux1"));
        let vae = || {
            choice_override(
                GenRole::Vae,
                SlotFile::Disk {
                    path: ae.display().to_string(),
                },
            )
        };
        let e = t.set_model_files(FLUX, vae()).await.unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "studio_setup_in_progress",
                "This model's files are still downloading for an earlier change."
            )
        );
        assert_eq!(t.row(FLUX).await, before);
        t.complete(&group).await;
        let r = t.set_model_files(FLUX, vae()).await.unwrap();
        assert!(r.download_group.is_none());
        let row = t.row(FLUX).await;
        assert_eq!(row.recipe_id.as_deref(), Some("z-image"));
        let vae_file = files_of(&row).last().unwrap().1.clone();
        assert_eq!(Path::new(&vae_file), dunce::canonicalize(&ae).unwrap());
    }

    /// Finding 2: a model of the recommended list the person set up again is still installed
    /// in that list (its row keeps the entry's id, with no catalog id now), so its card offers
    /// no Download that would replace their set-up; its name stays.
    #[tokio::test]
    async fn an_edited_catalog_model_is_still_installed_in_the_recommended_list() {
        const ZIMAGE: &str = "z-image-turbo-q4";
        fn installed_zimage(models: &Path) -> store::NewGenModel {
            let entry = bundled().entries.remove(ZIMAGE).unwrap();
            let header = file_of(&models::sd::testing::gguf_txt_fixture("zimage"));
            let mut parts = Vec::new();
            for (i, part) in entry.parts.iter().enumerate() {
                let mut part = part.clone();
                let file = destination(models, &part.repo, &part.path).unwrap();
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                if i == 0 {
                    std::fs::write(&file, &header).unwrap();
                    part.size = header.len() as u64;
                } else {
                    std::fs::File::create(&file)
                        .unwrap()
                        .set_len(part.size)
                        .unwrap();
                }
                parts.push((part, file));
            }
            store::NewGenModel {
                catalog_id: Some(ZIMAGE.into()),
                name: entry.name.clone(),
                kind: "image".into(),
                ..row_of(
                    ZIMAGE,
                    &crate::studio::local_models::family_word(&entry),
                    Some(&entry.recipe),
                    store::GenSource::Catalog,
                    parts,
                )
            }
        }
        let mut t = harness_with(installed_zimage).await;
        let name = t.row(ZIMAGE).await.name;
        let entry_of = |local: &LocalModelsDto| {
            local
                .recommended
                .iter()
                .chain(&local.not_here)
                .find(|e| e.id == ZIMAGE)
                .cloned()
                .unwrap()
        };
        assert!(entry_of(&t.local_models().await).installed);
        let ae = t.outside_file("my ae.safetensors", fixture_bytes("ae-flux1"));
        let r = t
            .set_model_files(
                ZIMAGE,
                choice_override(
                    GenRole::Vae,
                    SlotFile::Disk {
                        path: ae.display().to_string(),
                    },
                ),
            )
            .await
            .unwrap();
        assert!(r.download_group.is_none());
        let row = t.row(ZIMAGE).await;
        assert_eq!(
            (row.catalog_id.as_deref(), row.name.as_str()),
            (None, name.as_str())
        );
        assert!(entry_of(&t.local_models().await).installed);
    }

    #[tokio::test]
    async fn a_row_without_a_recipe_is_listed_as_needing_set_up() {
        let mut t = harness_with(row_without_recipe).await;
        let installed = t.local_models().await.installed[0].clone();
        assert!(installed.needs_setup);
        assert_eq!(installed.recipe, None);
        assert_eq!(installed.fit.word, "notHere");
        assert_eq!(installed.source, "search");
        // Its files: no recipe yet, every recipe Studio runs offered, by kind then name.
        let id = "hf:o/old:old.gguf";
        let f = t.model_files(id).await.unwrap();
        assert!(f.recipe.is_none() && !f.can_run && f.untested);
        assert_eq!(
            f.slots
                .iter()
                .map(|s| (s.role, s.source))
                .collect::<Vec<_>>(),
            [(GenRole::Diffusion, PartSource::Main)]
        );
        let kinds: Vec<_> = f.recipes.iter().map(|r| r.kind).collect();
        assert_eq!(f.recipes.len(), in_scope_recipes().len());
        let rank = |k: &models::sd::matching::RecipeKind| *k as u8;
        assert!(kinds.windows(2).all(|w| rank(&w[0]) <= rank(&w[1])));
        for pair in f.recipes.windows(2) {
            assert!(pair[0].kind != pair[1].kind || pair[0].name <= pair[1].name);
        }
        // Set up by hand: its encoders and decoder come from the table.
        let r = t.set_model_files(id, recipe("flux1-dev")).await.unwrap();
        assert!(!r.files.untested && r.files.can_run);
        let group = r.download_group.expect("its companions download");
        t.complete(&group).await;
        let row = t.row(id).await;
        assert_eq!(row.recipe_id.as_deref(), Some("flux1-dev"));
        let installed = &t.local_models().await.installed[0];
        assert!(!installed.needs_setup && !installed.untested);
        assert_eq!(installed.recipe.as_ref().unwrap().id, "flux1-dev");
    }

    /// An imported model's file stays the person's own when its set-up changes: its part is
    /// still local, and the file stays where it is.
    #[tokio::test]
    async fn an_imported_models_own_file_stays_its_own_when_its_set_up_changes() {
        let mut t = harness_with(imported_flux).await;
        let id = "local:cccccccccccccccc";
        let installed = t.local_models().await.installed[0].clone();
        assert!(installed.in_place);
        assert_eq!(installed.source, "import");
        let mine = PathBuf::from(&files_of(&t.row(id).await)[0].1);
        let f = t.model_files(id).await.unwrap();
        assert!(f.slots[0].part.as_ref().unwrap().local);
        let ae = t.outside_file("my ae.safetensors", fixture_bytes("ae-flux1"));
        let r = t
            .set_model_files(
                id,
                choice_override(
                    GenRole::Vae,
                    SlotFile::Disk {
                        path: ae.display().to_string(),
                    },
                ),
            )
            .await
            .unwrap();
        assert!(r.download_group.is_none());
        assert_eq!(r.files.slots[0].source, PartSource::Main);
        let row = t.row(id).await;
        assert_eq!(row.source, store::GenSource::Import);
        let main = &row.parts.as_array().unwrap()[0];
        assert_eq!(Path::new(main["local"].as_str().unwrap()), mine);
        assert_eq!(
            (main["repo"].as_str(), main["license"].as_str()),
            (Some(""), Some(""))
        );
        // Set up as Z-Image: when its encoder is in, the old encoders go; the person's file
        // never does.
        let r = t.set_model_files(id, recipe("z-image")).await.unwrap();
        t.complete(&r.download_group.unwrap()).await;
        assert!(mine.exists());
        let row = t.row(id).await;
        assert_eq!(
            Path::new(row.parts.as_array().unwrap()[0]["local"].as_str().unwrap()),
            mine
        );
    }

    /// The installed list says what each model is set up as: its type, version, LoRAs and
    /// where it came from.
    #[tokio::test]
    async fn an_installed_model_says_what_it_is_set_up_as() {
        let mut t = harness_with(installed_generic_flux).await;
        let m = t.local_models().await.installed[0].clone();
        assert_eq!(
            m.recipe.as_ref().map(|r| r.id.as_str()),
            Some("flux1-kontext")
        );
        assert_eq!(m.version, Some(SdVersion::Flux));
        assert!(!m.untested && !m.needs_setup && !m.high_noise && m.takes_loras);
        assert_eq!(m.lora_base, Some(SdVersion::Flux));
        assert_eq!((m.source.as_str(), m.in_place), ("search", false));
        let wire = serde_json::to_value(&m).unwrap();
        for key in [
            "recipe",
            "version",
            "untested",
            "needsSetup",
            "highNoise",
            "takesLoras",
            "loraBase",
            "source",
            "inPlace",
        ] {
            assert!(wire.get(key).is_some(), "{key}");
        }
    }
}
