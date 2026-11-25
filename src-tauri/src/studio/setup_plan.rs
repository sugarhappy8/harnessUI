//! Set-up plans (design section 7.4; Tasks S6 to S9): which file fills each slot of a searched
//! file's, a file on this computer's or an installed model's recipe, and the generic entry the
//! plan makes.
//!
//! - **Planning** ([`plan_source`]): a searched file of its repo at its commit
//!   ([`plan_file`], the repo read through `DetectService`), or a file on this computer
//!   ([`plan_disk`]: checked, then its header read), with the person's choices
//!   ([`slot_choices`]: another recipe, a Hub file pinned by its SHA-256, an installed model's
//!   file, a file on this computer), by `models::sd::setup::plan_setup`. *Set it up
//!   yourself*: a file Studio does not recognise, or one not read, with the recipe the person
//!   chose (untested); an installed model planned again with its own files kept
//!   ([`plan_installed`]).
//! - **The entry it makes** (`plan_with`): one part per planned file, each fetched file under
//!   its companion licence or its own repo's label (ruling S4-licence-ref), a file an
//!   installed model uses as that model has it, and the main file from this computer the
//!   person's own (`local`).
//! - **Plan to download** ([`present_parts`], [`ready_to_fetch`]): the parts a plan takes
//!   from this computer, used where they are, and whether a plan can be fetched at all.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use models::files::{RepoFile, quant_label};
use models::gen_catalog::{
    GateMode, GenCatalog, GenEntry, GenGate, GenLicence, GenPart, GenRole, GenericSpec,
    generic_entry,
};
use models::gen_search::{repo_licence, unread_licence};
use models::hf::{Gated, HfClient, HfError, RepoDetail};
use models::sd::candidates::candidates;
use models::sd::companions::CompanionTable;
use models::sd::detect::SdVersion;
use models::sd::index::{FileFormat, PICKLE_EXTENSIONS, READ_EXTENSIONS};
use models::sd::inspect::{FileKind, MatchFacts, inspect_local};
use models::sd::matching::{NoisePart, out_of_scope, recipes_for};
use models::sd::recipes::{SlotId, recipe};
use models::sd::setup::{
    InstalledFile, LicenceRef, MainFile, PartSource, PlannedFile, PlannedPart, SetupInput,
    SetupPlan, SlotChoice, plan_setup,
};
use models::sd::{CannotRunReason, FileFacts, FileLabel, NotRead, label_for};

use super::detect::{DetectService, RepoReading};
use super::errors;
use super::local_models::{
    InstalledFiles, LocalSource, entry_from_installed, installed_version, rebuilt_licences,
};
use crate::dto::{SetupChoice, SlotFile, SlotOverride};
use crate::error::{AppError, AppResult};

/// What planning a searched or local file's set-up reads beside the file: the Hub (a
/// searched file's repo, a chosen file of another repo), the repos read so far, the
/// catalog's companion table, and the files the installed models use.
pub(crate) struct PlanDeps<'a> {
    pub hf: Option<&'a HfClient>,
    pub detect: &'a DetectService,
    pub catalog: &'a GenCatalog,
    pub installed: &'a InstalledFiles,
}

/// A searched repo's listing at a commit, and what reading its files found
/// ([`DetectService::read_repo`]).
pub(crate) struct ReadRepo {
    pub detail: RepoDetail,
    pub reading: RepoReading,
}

impl ReadRepo {
    /// `repo` at `commit` (its latest when `None`), its files read through `detect` (each
    /// once while it stays in the cache).
    pub(crate) async fn read(
        hf: &HfClient,
        detect: &DetectService,
        repo: &str,
        commit: Option<&str>,
    ) -> AppResult<Self> {
        let detail = hf.repo(repo, commit).await.map_err(crate::hub::hf_error)?;
        Ok(Self::of(detect, hf, detail).await)
    }

    /// `detail`'s files read through `detect`.
    pub(crate) async fn of(detect: &DetectService, hf: &HfClient, detail: RepoDetail) -> Self {
        let reading = detect.read_repo(hf, &detail).await;
        Self { detail, reading }
    }
}

/// A set-up planned for one file, and the generic entry it makes.
pub(crate) struct Planned {
    pub plan: SetupPlan,
    /// Its recipe, and one part per planned file, in the plan's order.
    pub entry: GenEntry,
    /// For each of `plan.parts`, its part's index in `entry.parts`: `None` for a part the
    /// main file carries and for one nothing fills.
    pub part_of: Vec<Option<usize>>,
    /// The repos of the Hub files it fetches whose listing says they are gated (a companion's
    /// repo is not, the table's files being open): Hugging Face serves them only with a token.
    pub gated: BTreeSet<String>,
}

/// The set-up of the file `source` names, with the person's `choice`: a searched file of
/// its repo at its commit ([`plan_file`]), or a file on this computer ([`plan_disk`]). A
/// catalog or installed model has its parts already: `studio_not_supported_yet`.
pub(crate) async fn plan_source(
    deps: &PlanDeps<'_>,
    source: &LocalSource,
    choice: &SetupChoice,
) -> AppResult<Planned> {
    match source {
        LocalSource::Search { repo, commit, file } => {
            let hf = deps
                .hf
                .ok_or_else(|| AppError::internal("a search source without a Hub client"))?;
            let read = ReadRepo::read(hf, deps.detect, repo, Some(commit)).await?;
            plan_file(deps, &read, file, choice).await
        }
        LocalSource::Disk { path } => plan_disk(deps, path, choice).await,
        LocalSource::Catalog { .. } | LocalSource::Installed { .. } => {
            Err(errors::not_supported_yet(errors::PLAN_FOR_A_FILE_ONLY))
        }
    }
}

/// The set-up of `file` of the repo `read`, with `choice`. The file must be one Studio
/// runs (labelled `Works`): a diffusers folder's UNet plans the folder. *Set it up yourself*:
/// weights the engine does not recognise (`CannotRun { UnknownWeights }`), or a file whose
/// header was not read (past the files read for one repo, or a read that failed), are planned
/// with the recipe the person chose, untested, of no known version; never a pickle or
/// sharded file. Any other file Studio can't run is `studio_cannot_run` with its label's
/// sentence ([`errors::cannot_run`]), a pickle file "Studio can't read .pt files" with its
/// own extension ([`errors::cannot_read_file`]); a file not listed is `not_found`, and another
/// (a LoRA, an extra part, one not read without a recipe) `studio_not_supported_yet`.
pub(crate) async fn plan_file(
    deps: &PlanDeps<'_>,
    read: &ReadRepo,
    file: &str,
    choice: &SetupChoice,
) -> AppResult<Planned> {
    let detail = &read.detail;
    let listed = detail
        .files
        .iter()
        .find(|f| f.path == file)
        .ok_or_else(|| AppError::not_found("model file"))?;
    let by_hand = || match &choice.recipe {
        Some(recipe) => Ok((None, recipe.clone(), None, false)),
        None => Err(errors::not_supported_yet(errors::NOT_A_RUNNABLE_FILE)),
    };
    let (version, recipe_id, part, folder) = match read.reading.labels.get(file) {
        Some(FileLabel::Works {
            version,
            recipe,
            part,
            folder,
            ..
        }) => (Some(*version), recipe.clone(), *part, *folder),
        // A gated repo's file that could not be read says what Hugging Face asks for.
        Some(FileLabel::NotRead {
            reason: NotRead::NeedsToken,
        }) => return Err(crate::hub::hf_error(HfError::NeedsToken)),
        Some(FileLabel::NotRead {
            reason: NotRead::NeedsTerms,
        }) => return Err(crate::hub::hf_error(HfError::NeedsTerms)),
        // Never a pickle or sharded file: those are labelled by their names, never read.
        Some(FileLabel::NotRead {
            reason: NotRead::Budget | NotRead::Failed,
        }) => by_hand()?,
        Some(FileLabel::CannotRun {
            reason: CannotRunReason::UnknownWeights,
            ..
        }) if choice.recipe.is_some() => by_hand()?,
        // A pickle file named by its own extension (ruling S9-pickle-ext).
        Some(FileLabel::CannotRun {
            reason: CannotRunReason::Pickle,
            ..
        }) => {
            return Err(errors::cannot_read_file(
                pickle_extension(file).unwrap_or(".ckpt"),
            ));
        }
        Some(FileLabel::CannotRun { reason, .. }) => return Err(errors::cannot_run(*reason)),
        _ => return Err(errors::not_supported_yet(errors::NOT_A_RUNNABLE_FILE)),
    };
    // A file not read has no facts: planned as weights of no known kind.
    let unread = unread_facts(file);
    let facts = match read.reading.facts.get(file) {
        Some(facts) => facts,
        None if version.is_none() => &unread,
        None => return Err(AppError::internal("a file that works without its facts")),
    };
    let folder = if folder {
        let found = candidates(&detail.files)
            .folder
            .filter(|f| f.unet.file.path == file)
            .ok_or_else(|| AppError::internal("a folder model without its folder"))?;
        Some(found)
    } else {
        None
    };
    let main = match &folder {
        Some(folder) => MainFile::HubFolder {
            repo: &detail.id,
            revision: &detail.commit,
            folder,
        },
        None => MainFile::Hub {
            repo: &detail.id,
            revision: &detail.commit,
            file: listed,
        },
    };
    let mut others = OtherRepos::default();
    let overrides = slot_choices(deps, Some(detail), &choice.overrides, &mut others).await?;
    let quant = match &folder {
        Some(folder) => Some(if folder.fp16 { "fp16" } else { "fp32" }.to_owned()),
        None => quant_label(file),
    };
    let repo_name = detail.id.rsplit('/').next().unwrap_or(&detail.id);
    let named = Named {
        id: format!("hf:{}:{}", detail.id, file),
        name: match quant {
            Some(quant) => format!("{repo_name} ({quant})"),
            None => repo_name.to_owned(),
        },
        publisher: detail.id.split('/').next().unwrap_or_default().to_owned(),
        gate: GenGate {
            mode: GateMode::from(detail.gated),
            collects: Vec::new(),
        },
    };
    plan_with(
        deps,
        PlanMain {
            main,
            facts,
            version,
            recipe_id: &recipe_id,
            part,
            listing: &detail.files,
            listing_facts: &read.reading.facts,
            detail: Some(detail),
            installed: false,
        },
        named,
        choice,
        &overrides,
        &others,
    )
}

/// What planning knows of a file whose header was not read: weights of no known kind, in
/// the format its name says.
fn unread_facts(file: &str) -> FileFacts {
    FileFacts {
        kind: FileKind::CannotRun {
            reason: CannotRunReason::UnknownWeights,
            version: None,
        },
        facts: MatchFacts::default(),
        parameters: 0,
        format: if file.to_lowercase().ends_with(".gguf") {
            FileFormat::Gguf
        } else {
            FileFormat::Safetensors
        },
    }
}

/// The set-up of installed model `row` planned again (*Set it up yourself*): its recipe, or
/// the one `choice` names; its model file (its part of role `diffusion` or `model`) as the
/// main file, its header read here off the async runtime; each of its other parts' files for
/// its slot; then `choice`'s files on top. The model's own files are used where they are and
/// show as `Installed` (its model file `Main`); its model part stays the part it records (a
/// file from this computer stays the person's own). Its version is the one its header says,
/// else its family column's: a recipe that version does not run is untested. A model with no
/// recipe, and none chosen, is `studio_not_supported_yet`, as is one whose model file's header
/// does not read.
pub(crate) async fn plan_installed(
    deps: &PlanDeps<'_>,
    row: &store::GenModel,
    choice: &SetupChoice,
) -> AppResult<Planned> {
    let recipe_id = choice
        .recipe
        .clone()
        .or_else(|| row.recipe_id.clone())
        .ok_or_else(|| errors::not_supported_yet(errors::NO_RECIPE_YET))?;
    let own = own_parts(row)?;
    let is_main = |p: &GenPart| matches!(p.role, GenRole::Diffusion | GenRole::Model);
    let mut mains = own.iter().filter(|(p, _)| is_main(p));
    let (Some((main, main_file)), None) = (mains.next(), mains.next()) else {
        return Err(AppError::internal(
            "an installed model without exactly one model part",
        ));
    };
    let read_from = main_file.clone();
    let facts = tokio::task::spawn_blocking(move || inspect_local(&read_from))
        .await
        .map_err(|e| AppError::internal(format!("reading a file's header: {e}")))?
        .map_err(|e| errors::not_supported_yet(format!("its model file does not read: {e}")))?;
    let version = match &facts.kind {
        FileKind::Model { version, .. } => Some(*version),
        _ => installed_version(row),
    };
    // Its own row first, so each of its files is the part it records.
    let mut installed = deps.installed.clone();
    installed.gen_models.retain(|m| m.id != row.id);
    installed.gen_models.insert(0, row.clone());
    let deps = PlanDeps {
        installed: &installed,
        ..*deps
    };
    let mut overrides: Vec<(SlotId, SlotChoice)> = own
        .iter()
        .filter(|(p, _)| !is_main(p))
        .map(|(p, file)| {
            let choice = SlotChoice::Local {
                path: file.clone(),
                size: p.size,
                sha256: Some(p.sha256.clone()).filter(|s| !s.is_empty()),
            };
            (p.role.slot(), choice)
        })
        .collect();
    let mut others = OtherRepos::default();
    overrides.extend(slot_choices(&deps, None, &choice.overrides, &mut others).await?);
    let no_facts = BTreeMap::new();
    let mut planned = plan_with(
        &deps,
        PlanMain {
            main: MainFile::Local {
                path: main_file,
                size: main.size,
                sha256: Some(main.sha256.as_str()).filter(|s| !s.is_empty()),
            },
            facts: &facts,
            version,
            recipe_id: &recipe_id,
            part: None,
            listing: &[],
            listing_facts: &no_facts,
            detail: None,
            installed: true,
        },
        Named {
            id: row.id.clone(),
            name: row.name.clone(),
            publisher: String::new(),
            gate: GenGate {
                mode: GateMode::None,
                collects: Vec::new(),
            },
        },
        choice,
        &overrides,
        &others,
    )?;
    // A slot the person did not choose for keeps the model's own file.
    let chosen: BTreeSet<SlotId> = choice.overrides.iter().map(|o| o.role.slot()).collect();
    for part in &mut planned.plan.parts {
        if part.source == PartSource::Chosen && !chosen.contains(&part.slot) {
            part.source = PartSource::Installed;
        }
    }
    Ok(planned)
}

/// Each part of installed model `row` with its file on this computer.
pub(crate) fn own_parts(row: &store::GenModel) -> AppResult<Vec<(GenPart, PathBuf)>> {
    let bad = |e: String| AppError::internal(format!("an installed model's parts: {e}"));
    row.parts
        .as_array()
        .ok_or_else(|| bad("not a list".into()))?
        .iter()
        .map(|p| {
            let file = p
                .get("file")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| bad("a part without its file".into()))?;
            let part: GenPart =
                serde_json::from_value(p.clone()).map_err(|e| bad(e.to_string()))?;
            Ok((part, PathBuf::from(file)))
        })
        .collect()
}

/// The set-up of the file at `path` on this computer (untrusted: [`checked_disk_file`]),
/// read from its header, with `choice`. A file Studio does not run is
/// `studio_not_supported_yet`.
pub(crate) async fn plan_disk(
    deps: &PlanDeps<'_>,
    path: &str,
    choice: &SetupChoice,
) -> AppResult<Planned> {
    let file = read_disk_file(path).await?;
    let setup = disk_setup(deps, &file, choice).await?;
    let id = format!("local:{}", file.path.display());
    plan_disk_with(deps, &file, &setup, (&file.path, None, id), choice)
}

/// A model file on this computer the webview named, checked ([`checked_disk_file`]) and its
/// header read.
pub(crate) struct DiskFile {
    /// Its canonical path.
    pub path: PathBuf,
    pub size: u64,
    /// Its file name.
    pub name: String,
    pub facts: FileFacts,
}

impl DiskFile {
    /// Its label, by its header and its file name.
    pub(crate) fn label(&self) -> FileLabel {
        label_for(&self.facts, &self.name)
    }
}

/// The file at `path` on this computer (untrusted): checked ([`checked_disk_file`]: never
/// opened when it is a pickle file), then its header read off the async runtime. A header
/// that does not read is `studio_not_supported_yet`.
pub(crate) async fn read_disk_file(path: impl AsRef<Path>) -> AppResult<DiskFile> {
    let (path, size) = checked_disk_file(path)?;
    let read_from = path.clone();
    let facts = tokio::task::spawn_blocking(move || inspect_local(&read_from))
        .await
        .map_err(|e| AppError::internal(format!("reading a file's header: {e}")))?
        .map_err(|e| errors::not_supported_yet(format!("its header does not read: {e}")))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(DiskFile {
        path,
        size,
        name,
        facts,
    })
}

/// What planning a file on this computer takes besides where it is: the recipe its label
/// gives, and the person's choices for its slots, read once ([`slot_choices`]: a chosen file
/// of another repo asks for that repo's listing).
pub(crate) struct DiskSetup {
    version: SdVersion,
    recipe_id: String,
    part: Option<NoisePart>,
    overrides: Vec<(SlotId, SlotChoice)>,
    others: OtherRepos,
}

/// [`DiskSetup`] for `file` with `choice`. A file Studio does not run (its label is not
/// *Works in Studio*) is `studio_not_supported_yet`.
pub(crate) async fn disk_setup(
    deps: &PlanDeps<'_>,
    file: &DiskFile,
    choice: &SetupChoice,
) -> AppResult<DiskSetup> {
    let FileLabel::Works {
        version,
        recipe,
        part,
        ..
    } = file.label()
    else {
        return Err(errors::not_supported_yet(errors::NOT_A_RUNNABLE_FILE));
    };
    let mut others = OtherRepos::default();
    let overrides = slot_choices(deps, None, &choice.overrides, &mut others).await?;
    Ok(DiskSetup {
        version,
        recipe_id: recipe,
        part,
        overrides,
        others,
    })
}

/// The set-up of `file` with `setup` and `choice`, its main file at `at`: `(path, its
/// SHA-256 when known, the entry's id)`. The path is the file's own, or a copy of it (an
/// imported file copied into the models folder, which has its name and header).
pub(crate) fn plan_disk_with(
    deps: &PlanDeps<'_>,
    file: &DiskFile,
    setup: &DiskSetup,
    at: (&Path, Option<&str>, String),
    choice: &SetupChoice,
) -> AppResult<Planned> {
    let (path, sha256, id) = at;
    let named = Named {
        id,
        name: file.name.clone(),
        publisher: String::new(),
        gate: GenGate {
            mode: GateMode::None,
            collects: Vec::new(),
        },
    };
    let no_facts = BTreeMap::new();
    plan_with(
        deps,
        PlanMain {
            main: MainFile::Local {
                path,
                size: file.size,
                sha256,
            },
            facts: &file.facts,
            version: Some(setup.version),
            recipe_id: &setup.recipe_id,
            part: setup.part,
            listing: &[],
            listing_facts: &no_facts,
            detail: None,
            installed: false,
        },
        named,
        choice,
        &setup.overrides,
        &setup.others,
    )
}

/// The main file of a plan and what was read about it.
struct PlanMain<'a> {
    main: MainFile<'a>,
    facts: &'a FileFacts,
    /// The engine version it was detected as; `None` for weights of no known kind (set up by
    /// hand: untested).
    version: Option<SdVersion>,
    /// The recipe its label gives.
    recipe_id: &'a str,
    part: Option<NoisePart>,
    listing: &'a [RepoFile],
    listing_facts: &'a BTreeMap<String, FileFacts>,
    /// Its repo's listing; `None` for a file on this computer.
    detail: Option<&'a RepoDetail>,
    /// A file from this computer that is an installed model's own model file (planned again:
    /// *Set it up yourself*): its part is the one that model records, never made the person's
    /// own.
    installed: bool,
}

/// How the entry a plan makes is known.
struct Named {
    id: String,
    name: String,
    publisher: String,
    gate: GenGate,
}

/// The listings of other repos a choice names, by repo and commit: `None` when the read
/// failed.
pub(crate) type OtherRepos = BTreeMap<(String, String), Option<RepoDetail>>;

/// Plans `main` with `choice`'s recipe (an in-scope recipe; untested when its version's
/// recipes do not include it) and `overrides`, and makes its entry.
fn plan_with(
    deps: &PlanDeps<'_>,
    m: PlanMain<'_>,
    named: Named,
    choice: &SetupChoice,
    overrides: &[(SlotId, SlotChoice)],
    others: &OtherRepos,
) -> AppResult<Planned> {
    let recipe_id = choice.recipe.as_deref().unwrap_or(m.recipe_id);
    let recipe = recipe(recipe_id)
        .filter(|r| out_of_scope(&r.id).is_none())
        .ok_or_else(errors::recipe_not_offered)?;
    let untested = m
        .version
        .is_none_or(|v| !recipes_for(v).contains(&recipe_id));
    let installed = installed_files(deps.installed);
    let plan = plan_setup(&SetupInput {
        main: m.main,
        facts: m.facts,
        recipe,
        version: m.version,
        part: m.part,
        listing: m.listing,
        listing_facts: m.listing_facts,
        installed: &installed,
        companions: &deps.catalog.companions,
        overrides,
        untested,
    });
    let mut licenses = BTreeMap::new();
    let mut parts = Vec::new();
    let mut part_of = Vec::with_capacity(plan.parts.len());
    let mut gated = BTreeSet::new();
    for planned in &plan.parts {
        let (Some(file), Some(role)) = (&planned.file, GenRole::from_slot(planned.slot)) else {
            part_of.push(None);
            continue;
        };
        if let PlannedFile::Hub { repo, revision, .. } = file {
            let listing = m
                .detail
                .filter(|d| d.id == *repo && d.commit == *revision)
                .or_else(|| others.get(&(repo.clone(), revision.clone()))?.as_ref());
            if listing.is_some_and(|d| d.gated != Gated::No) {
                gated.insert(repo.clone());
            }
        }
        let part = match file {
            // The main file from this computer is the person's own, wherever it lies (an
            // installed model's file included): it names no licence and is never deleted. An
            // installed model planned again keeps its own model part.
            PlannedFile::Local { path, size, sha256 }
                if planned.source == PartSource::Main && !m.installed =>
            {
                local_part(role, path, *size, sha256.as_deref())
            }
            _ => gen_part(deps, m.detail, others, &mut licenses, role, file),
        };
        part_of.push(Some(parts.len()));
        parts.push(part);
    }
    let entry = generic_entry(GenericSpec {
        id: named.id,
        name: named.name,
        publisher: named.publisher,
        recipe,
        version: m.version,
        parts,
        licenses,
        gate: named.gate,
        untested,
    });
    Ok(Planned {
        plan,
        entry,
        part_of,
        gated,
    })
}

/// The part a planned `file` makes for `role`, its licence added to `licenses`: a Hub file
/// under its companion licence or its own repo's ([`file_licence`]); a file an installed
/// model uses, as that model has it (so it is checked in place, not fetched); else a file
/// of this computer, the person's own, which names no licence.
fn gen_part(
    deps: &PlanDeps<'_>,
    main: Option<&RepoDetail>,
    others: &OtherRepos,
    licenses: &mut BTreeMap<String, GenLicence>,
    role: GenRole,
    file: &PlannedFile,
) -> GenPart {
    match file {
        PlannedFile::Hub {
            repo,
            revision,
            path,
            size,
            sha256,
            licence,
            save_as,
        } => {
            let licence = file_licence(
                &deps.catalog.companions,
                main,
                others,
                repo,
                revision,
                licence,
            );
            GenPart {
                role,
                repo: repo.clone(),
                revision: revision.clone(),
                path: path.clone(),
                size: *size,
                sha256: sha256.clone(),
                license: add_licence(licenses, licence),
                local: None,
                folder: save_as.clone(),
            }
        }
        PlannedFile::Local { path, size, sha256 } => {
            let spelled = path.display().to_string();
            let installed = deps.installed.gen_models.iter().find_map(|row| {
                let part = row.parts.as_array()?.iter().find(|p| {
                    p.get("file")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|f| crate::model_options::same_file(f, &spelled))
                })?;
                Some((row, serde_json::from_value::<GenPart>(part.clone()).ok()?))
            });
            match installed {
                Some((row, mut part)) if part.local.is_none() => {
                    let licence = installed_licence(deps, main, row, &part);
                    part.role = role;
                    part.license = add_licence(licenses, licence);
                    part
                }
                _ => local_part(role, path, *size, sha256.as_deref()),
            }
        }
    }
}

/// The part a file of this computer makes, the person's own: no repository, revision or
/// licence, its file name as its path, used where it is (`local`).
fn local_part(role: GenRole, path: &Path, size: u64, sha256: Option<&str>) -> GenPart {
    GenPart {
        role,
        repo: String::new(),
        revision: String::new(),
        path: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size,
        sha256: sha256.unwrap_or_default().to_owned(),
        license: String::new(),
        local: Some(path.to_path_buf()),
        folder: None,
    }
}

/// The licence of a Hub file a plan fetches (ruling S4-licence-ref): a companion's, from the
/// table; else its own repo's label ([`repo_licence`]): the main repo's listing for a file
/// of the main repo, the chosen repo's listing at its commit for a file the person chose,
/// and "Check the licence" (red) when that listing could not be read. A file of another
/// repo never shows the main repo's licence.
pub(crate) fn file_licence(
    companions: &CompanionTable,
    main: Option<&RepoDetail>,
    others: &OtherRepos,
    repo: &str,
    revision: &str,
    licence: &LicenceRef,
) -> GenLicence {
    match licence {
        LicenceRef::Companion(id) => match companions.licence(id) {
            Some(l) => l.clone(),
            None => unread_licence(repo, revision),
        },
        LicenceRef::Repo => {
            let listing = main
                .filter(|d| d.id == repo && d.commit == revision)
                .or_else(|| {
                    others
                        .get(&(repo.to_owned(), revision.to_owned()))?
                        .as_ref()
                });
            match listing {
                Some(detail) => repo_licence(detail),
                None => unread_licence(repo, revision),
            }
        }
    }
}

/// The licence of a file an installed model uses: a companion's, by its SHA-256; else the
/// one that model records for it (worked out again from the part's repository, revision and
/// licence id for a model that has no recipe yet: `local_models::rebuilt_licences`); else,
/// for a file of the main repo, that repo's label; else "Check the licence".
fn installed_licence(
    deps: &PlanDeps<'_>,
    main: Option<&RepoDetail>,
    row: &store::GenModel,
    part: &GenPart,
) -> GenLicence {
    let companions = &deps.catalog.companions;
    if let Some(l) = companions
        .by_sha(&part.sha256)
        .and_then(|c| companions.licence(&c.license))
    {
        return l.clone();
    }
    let recorded = match entry_from_installed(row, deps.catalog) {
        Ok(entry) => entry.licenses.get(&part.license).cloned(),
        Err(_) if row.recipe_id.is_none() => {
            rebuilt_licences(deps.catalog, std::slice::from_ref(part)).remove(&part.license)
        }
        Err(_) => None,
    };
    if let Some(l) = recorded {
        return l;
    }
    match main.filter(|d| d.id == part.repo && d.commit == part.revision) {
        Some(detail) => repo_licence(detail),
        None => unread_licence(&part.repo, &part.revision),
    }
}

/// Adds `licence` to `licenses` and gives the id its parts name: the id of the same licence
/// (by SHA-256) when it is there already; its own id when that is free; else its id with a
/// number, so two repos' different licences with one label never share an id.
pub(crate) fn add_licence(
    licenses: &mut BTreeMap<String, GenLicence>,
    mut licence: GenLicence,
) -> String {
    if let Some((id, _)) = licenses.iter().find(|(_, l)| l.sha256 == licence.sha256) {
        return id.clone();
    }
    let id = (1..)
        .map(|n| match n {
            1 => licence.id.clone(),
            n => format!("{}-{n}", licence.id),
        })
        .find(|id| !licenses.contains_key(id))
        .unwrap_or_default();
    licence.id.clone_from(&id);
    licenses.insert(id.clone(), licence);
    id
}

/// The files the installed picture and video models use that are on this computer with
/// their recorded size, each once: what a plan takes before fetching anything.
fn installed_files(installed: &InstalledFiles) -> Vec<InstalledFile> {
    let mut seen = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    for row in &installed.gen_models {
        for part in row.parts.as_array().into_iter().flatten() {
            let (Some(file), Ok(part)) = (
                part.get("file").and_then(serde_json::Value::as_str),
                serde_json::from_value::<GenPart>(part.clone()),
            ) else {
                continue;
            };
            let path = PathBuf::from(file);
            let here = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == part.size);
            if here && !part.sha256.is_empty() && seen.insert(file.to_owned()) {
                files.push(InstalledFile {
                    slot: part.role.slot(),
                    path,
                    sha256: part.sha256,
                    size: part.size,
                });
            }
        }
    }
    files
}

/// What `overrides` give each slot: a Hub file (of the main repo `main`, else of its own
/// repo at its commit, whose listing is read once into `others`); a file an installed model
/// uses; a file on this computer ([`checked_disk_file`]); or none (`SlotChoice::Empty`, the
/// only choice the planner leaves out). A Hub file is always pinned by its SHA-256, never
/// handed over without one:
/// - its listing read, the file is the listing's: `not_found` when it is not listed,
///   `studio_no_checksum` when it has no published SHA-256;
/// - its listing not read, a companion row with that repo, revision and path pins it; with
///   none, the read's own error (a temporary failure is never taken for a choice).
async fn slot_choices(
    deps: &PlanDeps<'_>,
    main: Option<&RepoDetail>,
    overrides: &[SlotOverride],
    others: &mut OtherRepos,
) -> AppResult<Vec<(SlotId, SlotChoice)>> {
    let mut choices = Vec::with_capacity(overrides.len());
    // Why each listing in `others` that is `None` was not read.
    let mut failed: BTreeMap<(String, String), AppError> = BTreeMap::new();
    for o in overrides {
        let choice = match &o.file {
            SlotFile::Hub { repo, commit, path } => {
                let key = (repo.clone(), commit.clone());
                if main.is_none_or(|d| d.id != *repo || d.commit != *commit)
                    && !others.contains_key(&key)
                {
                    let read = match deps.hf {
                        Some(hf) => hf
                            .repo(repo, Some(commit))
                            .await
                            .map_err(crate::hub::hf_error),
                        None => Err(AppError::internal("a chosen Hub file without a Hub client")),
                    };
                    match read {
                        Ok(detail) => {
                            others.insert(key.clone(), Some(detail));
                        }
                        Err(e) => {
                            tracing::warn!(%repo, code = %e.code, "a chosen file's listing was not read");
                            failed.insert(key.clone(), e);
                            others.insert(key.clone(), None);
                        }
                    }
                }
                let listing = main
                    .filter(|d| d.id == *repo && d.commit == *commit)
                    .or_else(|| others.get(&key).and_then(Option::as_ref));
                let file = match listing {
                    Some(detail) => {
                        let file = detail
                            .files
                            .iter()
                            .find(|f| f.path == *path)
                            .ok_or_else(|| AppError::not_found("model file"))?;
                        if file.sha256.is_none() {
                            return Err(errors::no_published_checksum());
                        }
                        file.clone()
                    }
                    None => {
                        let pinned =
                            deps.catalog.companions.files.iter().find(|c| {
                                c.repo == *repo && c.revision == *commit && c.path == *path
                            });
                        match pinned {
                            Some(c) => RepoFile {
                                path: c.path.clone(),
                                size: c.size,
                                sha256: Some(c.sha256.clone()),
                            },
                            None => {
                                return Err(failed.get(&key).cloned().unwrap_or_else(|| {
                                    AppError::internal("a chosen file's listing was not read")
                                }));
                            }
                        }
                    }
                };
                SlotChoice::Hub {
                    repo: repo.clone(),
                    revision: commit.clone(),
                    file,
                }
            }
            SlotFile::Installed { gen_model_id, role } => {
                let part = deps
                    .installed
                    .gen_models
                    .iter()
                    .find(|row| row.id == *gen_model_id)
                    .and_then(|row| {
                        row.parts.as_array()?.iter().find_map(|p| {
                            let part: GenPart = serde_json::from_value(p.clone()).ok()?;
                            let file = p.get("file")?.as_str()?.to_owned();
                            (part.role == *role).then_some((part, file))
                        })
                    })
                    .ok_or_else(|| AppError::not_found("model file"))?;
                SlotChoice::Local {
                    path: PathBuf::from(part.1),
                    size: part.0.size,
                    sha256: Some(part.0.sha256).filter(|s| !s.is_empty()),
                }
            }
            SlotFile::Disk { path } => {
                let (path, size) = checked_disk_file(path)?;
                SlotChoice::Local {
                    path,
                    size,
                    sha256: None,
                }
            }
            SlotFile::None => SlotChoice::Empty,
        };
        choices.push((o.role.slot(), choice));
    }
    Ok(choices)
}

/// A file on this computer the webview names (untrusted): made canonical, a regular file,
/// of a format Studio reads; with its size. A pickle file is refused by its extension
/// ("Studio can't read .ckpt files"), never opened.
pub(crate) fn checked_disk_file(path: impl AsRef<Path>) -> AppResult<(PathBuf, u64)> {
    let real = dunce::canonicalize(path).map_err(|_| AppError::not_found("file"))?;
    let meta = std::fs::metadata(&real).map_err(|_| AppError::not_found("file"))?;
    if !meta.is_file() {
        return Err(AppError::not_found("file"));
    }
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if let Some(ext) = pickle_extension(&name) {
        return Err(errors::cannot_read_file(ext));
    }
    if !READ_EXTENSIONS.iter().any(|ext| name.ends_with(ext)) {
        return Err(errors::not_a_model_format());
    }
    Ok((real, meta.len()))
}

/// The pickle extension (`.ckpt`, `.pt`, `.pth`, `.bin`) a file name ends with, in any case.
fn pickle_extension(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    PICKLE_EXTENSIONS
        .into_iter()
        .find(|ext| lower.ends_with(ext))
}

/// For each of `entry`'s parts, the file on this computer it uses where it is: a part from
/// this computer's own (`local`), and with the set-up plan that made the entry, each part the
/// plan takes from this computer (`PlannedFile::Local`: an installed model's file, or the
/// person's). The entry's parts are the plan's files that fill a slot, in its order
/// (`plan_with`): each pair of the same role, and a file from this computer at the part's own
/// path or with its SHA-256 (when the plan knows none, a file an installed model uses, whose
/// part has one). A plan that does not match is an internal error.
pub(crate) fn present_parts(
    entry: &GenEntry,
    setup: Option<&SetupPlan>,
) -> AppResult<Vec<Option<PathBuf>>> {
    let mut present: Vec<Option<PathBuf>> = entry.parts.iter().map(|p| p.local.clone()).collect();
    let Some(setup) = setup else {
        return Ok(present);
    };
    let files: Vec<(&PlannedPart, &PlannedFile)> = setup
        .parts
        .iter()
        .filter(|p| GenRole::from_slot(p.slot).is_some())
        .filter_map(|p| Some((p, p.file.as_ref()?)))
        .collect();
    let not_its = || AppError::internal("a set-up plan that is not its entry's");
    if files.len() != present.len() {
        return Err(not_its());
    }
    for ((here, part), (planned, file)) in present.iter_mut().zip(&entry.parts).zip(files) {
        if GenRole::from_slot(planned.slot) != Some(part.role) {
            return Err(not_its());
        }
        if let PlannedFile::Local { path, sha256, .. } = file {
            let same_path = part.local.as_ref() == Some(path);
            let same_sha = match sha256 {
                Some(sha) => sha.eq_ignore_ascii_case(&part.sha256),
                None => part.local.is_none() && !part.sha256.is_empty(),
            };
            if !(same_path || same_sha) {
                return Err(not_its());
            }
            *here = Some(path.clone());
        }
    }
    Ok(present)
}

/// A planned set-up that can be fetched: one that misses a part is `studio_setup_incomplete`,
/// and one that fetches a gated repository's file with no Hugging Face token (`hf`) the Hub's
/// own `hf_needs_token`.
pub(crate) fn ready_to_fetch(planned: &Planned, hf: Option<&HfClient>) -> AppResult<()> {
    if !planned.plan.can_run() {
        return Err(errors::setup_incomplete(&planned.plan.missing()));
    }
    if !planned.gated.is_empty() && !hf.is_some_and(HfClient::has_token) {
        return Err(crate::hub::hf_error(HfError::NeedsToken));
    }
    Ok(())
}
