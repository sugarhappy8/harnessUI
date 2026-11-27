//! The set-up planner (design section 7.4): which file fills each slot of a model's recipe,
//! from its main file, the repo's listing, the files already installed and the companion
//! table. The rules, in order:
//!
//! 0. a diffusers folder lists its own files;
//! 1. the main file goes into `model` for a checkpoint, `diffusion_model` for a diffusion
//!    model alone, or the high-noise slot;
//! 2. a high/low pair's other half comes from the same repo;
//! 3. every other required slot, the text encoders and VAE the main file does not carry
//!    (with SD 1.x, 2.x and XL's CLIP encoders, which their recipes have no slot for), and
//!    `vae` for an unmet `required_any`, comes from the first of: the main file's own parts,
//!    an installed file, the same repo, the companion table;
//! 4. optional slots: the vision encoders by the same order for a recipe that takes an input
//!    picture, the others only from the main file's own parts;
//! 5. the person's own choices win. A choice that replaces the main file plans the other
//!    slots as for a main file that carries nothing (ruling S4-main-override). A slot the
//!    plan needs reads required whoever fills it.
//!
//! Companions suit the recipe: a recipe the main file's version does not run (an untested
//! set-up the person chose) takes the companions of a version that runs it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::files::{RepoFile, quant_label};
use crate::sd::FileFacts;
use crate::sd::companions::{Companion, CompanionTable};
use crate::sd::detect::SdVersion;
use crate::sd::inspect::{FileKind, Layout};
use crate::sd::matching::{NoisePart, noise_part, recipes_for, without_part_words};
use crate::sd::names::{
    DiffusersFolder, FolderFile, companion_of, is_shard, is_weights, looks_like_lora,
};
use crate::sd::recipes::{Recipe, SlotId};

/// A file a plan uses: one to fetch from the Hub, or one on this computer.
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedFile {
    Hub {
        repo: String,
        revision: String,
        path: String,
        size: u64,
        sha256: String,
        licence: LicenceRef,
        /// A diffusers folder part's name for the engine, inside the folder.
        save_as: Option<String>,
    },
    Local {
        path: PathBuf,
        size: u64,
        sha256: Option<String>,
    },
}

/// The licence a fetched file comes under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LicenceRef {
    /// The licence of the `PlannedFile`'s own `repo`.
    Repo,
    /// A licence id of the companion table.
    Companion(String),
}

/// Where a slot's file comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PartSource {
    /// The main file.
    Main,
    /// The other half of a high/low pair, from the same repo.
    Sibling,
    /// A file already on this computer.
    Installed,
    /// Another file of the main file's repo.
    Repo,
    /// The companion table.
    Companion,
    /// The person's own choice.
    Chosen,
    /// The main file's own part: nothing to fetch.
    Embedded,
    /// A diffusers folder's own file, downloaded with it.
    Folder,
    /// Nothing fills the slot, so the plan cannot run.
    Missing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedPart {
    pub slot: SlotId,
    /// Whether the plan needs the slot: the recipe requires it, it is a text encoder or VAE
    /// the main file does not carry (SD 1.x, 2.x and XL's CLIP encoders among them), it
    /// fills an unmet `required_any`, or it is a diffusers folder's text encoder.
    pub required: bool,
    pub source: PartSource,
    /// `None` for `Embedded` and `Missing`.
    pub file: Option<PlannedFile>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetupPlan {
    pub recipe_id: String,
    /// In the recipe's slot order, the plan's slots only; a checkpoint's `model` takes the
    /// place of the recipe's `diffusion_model`, a diffusion model's `diffusion_model` that of
    /// the recipe's `model`, and parts that are not slots of the recipe (a folder's, or SD
    /// 1.x, 2.x and XL's CLIP encoders) come last (`vae`, `clip_l`, `clip_g`).
    pub parts: Vec<PlannedPart>,
    pub untested: bool,
}

impl SetupPlan {
    /// The slots nothing fills, in part order.
    pub fn missing(&self) -> Vec<SlotId> {
        self.parts
            .iter()
            .filter(|p| p.source == PartSource::Missing)
            .map(|p| p.slot)
            .collect()
    }

    pub fn can_run(&self) -> bool {
        self.missing().is_empty()
    }

    /// The files to download, in part order.
    pub fn to_fetch(&self) -> Vec<&PlannedFile> {
        self.parts
            .iter()
            .filter_map(|p| p.file.as_ref())
            .filter(|f| matches!(f, PlannedFile::Hub { .. }))
            .collect()
    }
}

/// A file already on this computer, from an installed model.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledFile {
    pub slot: SlotId,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}

/// The person's choice for a slot.
#[derive(Debug, Clone, PartialEq)]
pub enum SlotChoice {
    /// A file of a Hub repo; one without a SHA-256 cannot be pinned, so it counts as `Empty`.
    Hub {
        repo: String,
        revision: String,
        file: RepoFile,
    },
    Local {
        path: PathBuf,
        size: u64,
        sha256: Option<String>,
    },
    /// No file: clears an optional slot, or makes a required one `Missing`.
    Empty,
}

/// The file a set-up is planned around.
#[derive(Debug, Clone, Copy)]
pub enum MainFile<'a> {
    Hub {
        repo: &'a str,
        revision: &'a str,
        file: &'a RepoFile,
    },
    Local {
        path: &'a Path,
        size: u64,
        sha256: Option<&'a str>,
    },
    HubFolder {
        repo: &'a str,
        revision: &'a str,
        folder: &'a DiffusersFolder,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct SetupInput<'a> {
    pub main: MainFile<'a>,
    pub facts: &'a FileFacts,
    pub recipe: &'a Recipe,
    pub version: Option<SdVersion>,
    pub part: Option<NoisePart>,
    /// The main file's repo; empty for a local file.
    pub listing: &'a [RepoFile],
    /// Facts of listed files read so far, by path.
    pub listing_facts: &'a BTreeMap<String, FileFacts>,
    pub installed: &'a [InstalledFile],
    pub companions: &'a CompanionTable,
    pub overrides: &'a [(SlotId, SlotChoice)],
    pub untested: bool,
}

/// Plans a model's set-up by the rules of design section 7.4 (see the module's docs).
pub fn plan_setup(input: &SetupInput<'_>) -> SetupPlan {
    let recipe = input.recipe;
    let companion_version = companion_version(recipe, input.version);
    let main_slot = Planner {
        input,
        main_replaced: false,
        companion_version,
    }
    .main_slot();
    // A checkpoint in `model` fills the recipe's `diffusion_model`, and a diffusion model in
    // `diffusion_model` the recipe's `model`.
    let stood_in = match (main_slot, recipe.main_slot()) {
        (SlotId::Model, SlotId::DiffusionModel) => Some(SlotId::DiffusionModel),
        (SlotId::DiffusionModel, SlotId::Model) => Some(SlotId::Model),
        _ => None,
    };
    let planner = Planner {
        input,
        companion_version,
        main_replaced: input
            .overrides
            .iter()
            .any(|(s, _)| *s == main_slot || stood_in == Some(*s)),
    };
    let mut parts: BTreeMap<SlotId, PlannedPart> = BTreeMap::new();
    let put = |parts: &mut BTreeMap<SlotId, PlannedPart>, slot, required, source, file| {
        parts.insert(
            slot,
            PlannedPart {
                slot,
                required,
                source,
                file,
            },
        );
    };

    // Rules 0 and 1: the main file, and a folder's own files unless a choice replaces it.
    put(
        &mut parts,
        main_slot,
        true,
        PartSource::Main,
        Some(planner.main_file()),
    );
    if let MainFile::HubFolder { folder, .. } = input.main
        && !planner.main_replaced
    {
        let sdxl = input.version.is_some_and(SdVersion::is_sdxl);
        for (slot, file, required) in [
            (
                SlotId::Vae,
                &folder.vae,
                planner.recipe_requires(SlotId::Vae),
            ),
            (SlotId::ClipL, &folder.text_encoder, true),
            (SlotId::ClipG, &folder.text_encoder_2, sdxl),
        ] {
            match file {
                Some(f) => put(
                    &mut parts,
                    slot,
                    required,
                    PartSource::Folder,
                    Some(planner.folder_file(f)),
                ),
                None if required => put(&mut parts, slot, true, PartSource::Missing, None),
                None => {}
            }
        }
    }

    // Rule 2: a high/low pair's other half.
    if let MainFile::Hub {
        repo,
        revision,
        file,
    } = input.main
        && recipe.slot(SlotId::HighNoiseDiffusionModel).is_some()
    {
        let (slot, want) = if main_slot == SlotId::HighNoiseDiffusionModel {
            (SlotId::DiffusionModel, NoisePart::Low)
        } else {
            (SlotId::HighNoiseDiffusionModel, NoisePart::High)
        };
        if let Some(sibling) = planner.sibling(file, want) {
            let file = planner.repo_file(repo, revision, sibling, None);
            put(&mut parts, slot, true, PartSource::Sibling, Some(file));
        }
    }

    // Rules 3 and 4: the recipe's other slots, `stood_in` filled by the main file.
    let required_any = planner.unmet_required_any(&parts, true);
    let needed = |s: SlotId| {
        planner.recipe_requires(s) || planner.own_part_missing(s) || required_any.contains(&s)
    };
    let takes_picture = recipe.feature("ref_images") || recipe.feature("init_image");
    for s in planner.recipe_slots() {
        if parts.contains_key(&s) || stood_in == Some(s) {
            continue;
        }
        let required = needed(s);
        let vision = matches!(s, SlotId::LlmVision | SlotId::ClipVision);
        let found = if required || (vision && takes_picture) {
            planner.fill(s)
        } else {
            planner.embedded(s).then_some((PartSource::Embedded, None))
        };
        match found {
            Some((source, file)) => put(&mut parts, s, required, source, file),
            None if required => put(&mut parts, s, true, PartSource::Missing, None),
            None => {}
        }
    }

    // The CLIP encoders an SD 1.x, 2.x or XL main file does not carry, which its recipe has
    // no slot for (ruling S4-unet-encoders).
    for s in planner.text_encoder_slots() {
        if parts.contains_key(&s) || planner.recipe_slots().any(|r| r == s) {
            continue;
        }
        if planner.own_part_missing(s) {
            match planner.fill(s) {
                Some((source, file)) => put(&mut parts, s, true, source, file),
                None => put(&mut parts, s, true, PartSource::Missing, None),
            }
        }
    }

    // Rule 5: the person's own choices, a later one for a slot winning. A slot the plan
    // needs without them (the first slot of a `required_any` group the main file, its folder
    // and its sibling leave unmet) stays required when the person's file fills it.
    let required_by_file = planner.unmet_required_any(&parts, false);
    for (s, choice) in input.overrides {
        let s = *s;
        if stood_in == Some(s) {
            // The choice is the main file, in the recipe's own slot.
            parts.remove(&main_slot);
        }
        let required = match parts.get(&s) {
            Some(part) => part.required,
            None if planner.recipe_slots().any(|r| r == s) => {
                needed(s) || (required_by_file.contains(&s) && planner.chosen(s, choice).is_some())
            }
            // A slot neither the recipe nor the plan has.
            None => continue,
        };
        match planner.chosen(s, choice) {
            Some(file) => put(&mut parts, s, required, PartSource::Chosen, Some(file)),
            None if required => put(&mut parts, s, true, PartSource::Missing, None),
            None => {
                parts.remove(&s);
            }
        }
    }

    SetupPlan {
        recipe_id: recipe.id.clone(),
        parts: planner
            .order()
            .into_iter()
            .filter_map(|s| parts.remove(&s))
            .collect(),
        untested: input.untested,
    }
}

/// The version whose companions suit `recipe`: the main file's `version` when it runs the
/// recipe, else the first version that does (a recipe the person chose for a file of another
/// version, or of none known), else the file's own.
fn companion_version(recipe: &Recipe, version: Option<SdVersion>) -> Option<SdVersion> {
    let runs = |v: &SdVersion| recipes_for(*v).contains(&recipe.id.as_str());
    match version {
        Some(v) if runs(&v) => Some(v),
        _ => SdVersion::ALL.into_iter().find(runs).or(version),
    }
}

struct Planner<'i, 'a> {
    input: &'i SetupInput<'a>,
    /// A choice replaces the main file, in its slot or the one it stands in for: the other
    /// slots are planned as for a main file that carries nothing, so no part is `Embedded`
    /// or a folder's own (ruling S4-main-override).
    main_replaced: bool,
    /// The version companions are chosen for ([`companion_version`]).
    companion_version: Option<SdVersion>,
}

impl Planner<'_, '_> {
    /// The recipe's slots Studio fills, in its order.
    fn recipe_slots(&self) -> impl Iterator<Item = SlotId> + '_ {
        self.input
            .recipe
            .slots
            .iter()
            .map(|s| s.id)
            .filter(|s| !s.is_adapter())
    }

    fn recipe_requires(&self, slot: SlotId) -> bool {
        self.input.recipe.slot(slot).is_some_and(|s| s.required)
    }

    /// The order parts are listed in: the recipe's slots, `model` and `diffusion_model`
    /// each just before the other, which they can take the place of, then the parts that
    /// are not slots of the recipe.
    fn order(&self) -> Vec<SlotId> {
        let mut order = Vec::new();
        for s in self.recipe_slots() {
            match s {
                SlotId::DiffusionModel => order.push(SlotId::Model),
                SlotId::Model => order.push(SlotId::DiffusionModel),
                _ => {}
            }
            order.push(s);
        }
        order.extend([SlotId::Vae, SlotId::ClipL, SlotId::ClipG]);
        let mut seen = Vec::new();
        order.retain(|s| {
            let first = !seen.contains(s);
            seen.push(*s);
            first
        });
        order
    }

    /// Rule 1. A checkpoint takes `-m` for any version, so it fills `model`, in place of the
    /// recipe's `diffusion_model` where the recipe has that. A diffusion model alone takes
    /// `--diffusion-model`, which loads it under `model.diffusion_model.` for any version:
    /// `-m` loads with the prefix `""`, and the engine knows a diffusion model only by
    /// `model.diffusion_model.` or `unet.` names (`src/model_loader.cpp:801`). So it fills
    /// `diffusion_model`, in place of the recipe's `model` where the recipe has that
    /// (ruling S4-diffusion-only).
    fn main_slot(&self) -> SlotId {
        let recipe = self.input.recipe;
        if matches!(self.input.main, MainFile::HubFolder { .. }) {
            return SlotId::Model;
        }
        if self.main_part() == Some(NoisePart::High)
            && recipe.slot(SlotId::HighNoiseDiffusionModel).is_some()
        {
            return SlotId::HighNoiseDiffusionModel;
        }
        match self.input.facts.kind {
            FileKind::Model {
                layout: Layout::Checkpoint,
                ..
            } => SlotId::Model,
            FileKind::Model {
                layout: Layout::DiffusionOnly,
                ..
            } => SlotId::DiffusionModel,
            _ => recipe.main_slot(),
        }
    }

    /// The main file's noise part: the one detection gave, else the one its name says (a
    /// file planned by hand has no known part): a Hub file's path in its repo, a local
    /// file's name, never the folders it is in on this computer (final review, area 1, I-3).
    fn main_part(&self) -> Option<NoisePart> {
        self.input.part.or_else(|| match self.input.main {
            MainFile::Hub { file, .. } => noise_part(&file.path),
            MainFile::Local { path, .. } => path
                .file_name()
                .and_then(|n| noise_part(&n.to_string_lossy())),
            MainFile::HubFolder { .. } => None,
        })
    }

    fn main_file(&self) -> PlannedFile {
        match self.input.main {
            MainFile::Hub {
                repo,
                revision,
                file,
            } => self.repo_file(repo, revision, file, None),
            MainFile::Local { path, size, sha256 } => PlannedFile::Local {
                path: path.to_path_buf(),
                size,
                sha256: sha256.map(str::to_string),
            },
            MainFile::HubFolder { folder, .. } => self.folder_file(&folder.unet),
        }
    }

    /// The main file's quant label, which breaks a tie between repo files.
    fn main_quant(&self) -> Option<String> {
        match self.input.main {
            MainFile::Hub { file, .. } => quant_label(&file.path),
            MainFile::Local { path, .. } => quant_label(&path.to_string_lossy()),
            MainFile::HubFolder { folder, .. } => quant_label(&folder.unet.file.path),
        }
    }

    fn main_repo(&self) -> Option<(&str, &str)> {
        match self.input.main {
            MainFile::Hub { repo, revision, .. } | MainFile::HubFolder { repo, revision, .. } => {
                Some((repo, revision))
            }
            MainFile::Local { .. } => None,
        }
    }

    /// Whether `path` is the main file or one of its folder's files.
    fn is_own(&self, path: &str) -> bool {
        match self.input.main {
            MainFile::Hub { file, .. } => file.path == path,
            MainFile::Local { .. } => false,
            MainFile::HubFolder { folder, .. } => [
                Some(&folder.unet),
                folder.vae.as_ref(),
                folder.text_encoder.as_ref(),
                folder.text_encoder_2.as_ref(),
            ]
            .into_iter()
            .flatten()
            .any(|f| f.file.path == path),
        }
    }

    /// A file of the main repo, under its licence. Only files with a SHA-256 are taken from
    /// a listing, so the empty one never reaches a plan.
    fn repo_file(
        &self,
        repo: &str,
        revision: &str,
        file: &RepoFile,
        save_as: Option<&str>,
    ) -> PlannedFile {
        PlannedFile::Hub {
            repo: repo.to_string(),
            revision: revision.to_string(),
            path: file.path.clone(),
            size: file.size,
            sha256: file.sha256.clone().unwrap_or_default(),
            licence: LicenceRef::Repo,
            save_as: save_as.map(str::to_string),
        }
    }

    /// A diffusers folder's file, saved under the engine's name for it.
    fn folder_file(&self, f: &FolderFile) -> PlannedFile {
        let (repo, revision) = self.main_repo().unwrap_or_default();
        self.repo_file(repo, revision, &f.file, Some(f.save_as))
    }

    /// Rule 2: the listed file of the other noise part with the main file's quant label at
    /// its folder depth, else the smallest of that part. Among equals, the one whose name
    /// is the main file's with the part words swapped wins, so a repo holding several pairs
    /// gives the pair's own half.
    fn sibling(&self, main: &RepoFile, want: NoisePart) -> Option<&RepoFile> {
        let depth = |path: &str| path.matches('/').count();
        let quant = quant_label(&main.path);
        let mirror = without_part_words(&main.path);
        // Never a file of the main file's own part: two high-noise halves are no pair.
        if noise_part(&main.path) == Some(want) {
            return None;
        }
        let others: Vec<&RepoFile> = self
            .input
            .listing
            .iter()
            .filter(|f| {
                f.path != main.path
                    && f.sha256.is_some()
                    && is_weights(&f.path)
                    && !is_shard(&f.path)
                    && !looks_like_lora(&f.path)
                    && noise_part(&f.path) == Some(want)
                    && self
                        .input
                        .listing_facts
                        .get(&f.path)
                        .is_none_or(|facts| matches!(facts.kind, FileKind::Model { .. }))
            })
            .collect();
        others
            .iter()
            .filter(|f| quant_label(&f.path) == quant && depth(&f.path) == depth(&main.path))
            .min_by_key(|f| (without_part_words(&f.path) != mirror, f.size, &f.path))
            .or_else(|| others.iter().min_by_key(|f| (f.size, &f.path)))
            .copied()
    }

    /// Whether the main file carries this slot's part: a file's embedded part, or a
    /// folder's own file; nothing once a choice replaces the main file.
    fn carries(&self, slot: SlotId) -> bool {
        if self.main_replaced {
            return false;
        }
        match self.input.main {
            MainFile::HubFolder { folder, .. } => match slot {
                SlotId::Vae => folder.vae.is_some(),
                SlotId::ClipL => folder.text_encoder.is_some(),
                SlotId::ClipG => folder.text_encoder_2.is_some(),
                _ => false,
            },
            _ => self.embedded(slot),
        }
    }

    /// The text-encoder slots the main file's version reads: the recipe's own, and for SD
    /// 1.x and 2.x `clip_l`, for SDXL `clip_l` and `clip_g`, which their checkpoint recipes
    /// have no slot for. The engine loads `--clip_l` and `--clip_g` under `clip_l.` and
    /// `clip_g.` (`src/pipeline/diffusion_engine.cpp:719`) and maps those onto a
    /// checkpoint's `cond_stage_model.` names for these versions
    /// (`src/name_conversion.cpp:1773-1775`; ruling S4-unet-encoders).
    fn text_encoder_slots(&self) -> Vec<SlotId> {
        let version = self.input.version;
        TEXT_ENCODERS
            .into_iter()
            .filter(|s| {
                self.input.recipe.slot(*s).is_some()
                    || match s {
                        SlotId::ClipL => version.is_some_and(SdVersion::is_unet),
                        SlotId::ClipG => version.is_some_and(SdVersion::is_sdxl),
                        _ => false,
                    }
            })
            .collect()
    }

    /// A text-encoder slot the main file's version reads ([`Planner::text_encoder_slots`])
    /// but the file does not carry, which the plan then needs (rulings S4-diffusion-only and
    /// S4-unet-encoders). The VAE is needed the same way, as one of the VAE/TAE pair
    /// ([`Planner::unmet_required_any`]).
    fn own_part_missing(&self, slot: SlotId) -> bool {
        self.text_encoder_slots().contains(&slot) && !self.carries(slot)
    }

    /// The groups of `required_any` none of whose slots the main file, its folder, its
    /// sibling or (with `choices`) the person's choices fill: their first slot is then
    /// required, as `vae` is for `[vae, tae]`. A main file that carries no VAE adds the group
    /// `[vae, tae]` (those of its slots the recipe has); a full checkpoint's own VAE stays
    /// optional.
    fn unmet_required_any(
        &self,
        parts: &BTreeMap<SlotId, PlannedPart>,
        choices: bool,
    ) -> Vec<SlotId> {
        let filled = |s: &SlotId| {
            parts
                .get(s)
                .is_some_and(|p| p.source != PartSource::Missing)
                || choices
                    && self
                        .input
                        .overrides
                        .iter()
                        .rev()
                        .find(|(o, _)| o == s)
                        .is_some_and(|(o, choice)| self.chosen(*o, choice).is_some())
        };
        let recipe = self.input.recipe;
        let own_vae =
            (recipe.slot(SlotId::Vae).is_some() && !self.carries(SlotId::Vae)).then(|| {
                [SlotId::Vae, SlotId::Tae]
                    .into_iter()
                    .filter(|s| recipe.slot(*s).is_some())
                    .collect::<Vec<_>>()
            });
        recipe
            .required_any
            .iter()
            .chain(own_vae.as_ref())
            .filter(|group| !group.iter().any(filled))
            .filter_map(|group| group.first().copied())
            .collect()
    }

    /// Rule 3's sources, first match.
    fn fill(&self, slot: SlotId) -> Option<(PartSource, Option<PlannedFile>)> {
        if self.embedded(slot) {
            return Some((PartSource::Embedded, None));
        }
        let repo = self.repo_pick(slot);
        let companion = self.default_companion(slot);
        if let Some(file) = self.installed(slot, repo, companion) {
            return Some((
                PartSource::Installed,
                Some(PlannedFile::Local {
                    path: file.path.clone(),
                    size: file.size,
                    sha256: Some(file.sha256.clone()),
                }),
            ));
        }
        if let (Some(file), Some((name, revision))) = (repo, self.main_repo()) {
            return Some((
                PartSource::Repo,
                Some(self.repo_file(name, revision, file, None)),
            ));
        }
        companion.map(|c| (PartSource::Companion, Some(companion_file(c))))
    }

    /// The main file carries the slot's part itself. A folder's parts are its own files
    /// (`Folder`), never embedded, and a main file a choice replaces embeds nothing.
    fn embedded(&self, slot: SlotId) -> bool {
        !self.main_replaced
            && !matches!(self.input.main, MainFile::HubFolder { .. })
            && matches!(&self.input.facts.kind, FileKind::Model { embeds, .. } if embeds.contains(&slot))
    }

    /// The companion rows with this SHA-256: `by_sha` gives only the first.
    fn rows_with_sha<'t>(&'t self, sha256: &'t str) -> impl Iterator<Item = &'t Companion> + 't {
        self.input
            .companions
            .files
            .iter()
            .filter(move |c| c.sha256.eq_ignore_ascii_case(sha256))
    }

    /// A companion row of this slot that suits the companions' version.
    fn suits(&self, c: &Companion, slot: SlotId) -> bool {
        c.slot == slot
            && self
                .companion_version
                .is_some_and(|v| c.versions.contains(&v))
    }

    fn default_companion(&self, slot: SlotId) -> Option<&Companion> {
        let version = self.companion_version?;
        self.input
            .companions
            .default_for(&self.input.recipe.id, version, slot)
    }

    /// Rule 3's installed source: a file with the SHA-256 the repo or the table would give,
    /// or with a companion's that suits the version and is the slot's default or a variant
    /// of it (the same repo's file for the same slot, such as 0.1.x's Q4_K_M encoders for
    /// the Q8_0 defaults). A file is known by its SHA-256 alone, never by its slot: an
    /// installed VAE of another family is not taken.
    fn installed(
        &self,
        slot: SlotId,
        repo: Option<&RepoFile>,
        default: Option<&Companion>,
    ) -> Option<&InstalledFile> {
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        let rank = |f: &InstalledFile| {
            if repo
                .and_then(|r| r.sha256.as_deref())
                .is_some_and(|sha| same(sha, &f.sha256))
            {
                Some(0)
            } else if default.is_some_and(|d| same(&d.sha256, &f.sha256)) {
                Some(1)
            } else {
                let d = default?;
                self.rows_with_sha(&f.sha256)
                    .any(|c| self.suits(c, slot) && c.repo == d.repo)
                    .then_some(2)
            }
        };
        self.input
            .installed
            .iter()
            .filter_map(|f| rank(f).map(|r| (r, f)))
            .min_by_key(|(r, _)| *r)
            .map(|(_, f)| f)
    }

    /// Rule 3's repo source: a listed file whose facts, or else whose name, say it fills the
    /// slot, with a SHA-256, not a shard; the smallest, then the main file's quant label.
    /// A file the table knows by its SHA-256 as a companion that does not suit the version
    /// is left out: a repo can hold another version's VAE.
    fn repo_pick(&self, slot: SlotId) -> Option<&RepoFile> {
        let main_quant = self.main_quant();
        self.input
            .listing
            .iter()
            .filter(|f| {
                let Some(sha) = f.sha256.as_deref() else {
                    return false;
                };
                let says = match self.input.listing_facts.get(&f.path) {
                    Some(facts) => facts.kind == FileKind::Extra { slot },
                    None => companion_of(f) == Some(slot),
                };
                let other_versions = self.companion_version.is_some()
                    && self.rows_with_sha(sha).next().is_some()
                    && !self.rows_with_sha(sha).any(|c| self.suits(c, slot));
                says && !other_versions && !self.is_own(&f.path) && !is_shard(&f.path)
            })
            .min_by_key(|f| (f.size, quant_label(&f.path) != main_quant, &f.path))
    }

    /// Rule 5: the file a choice gives; `None` for `Empty` and for a Hub file without a
    /// SHA-256, which cannot be pinned.
    fn chosen(&self, slot: SlotId, choice: &SlotChoice) -> Option<PlannedFile> {
        match choice {
            SlotChoice::Hub {
                repo,
                revision,
                file,
            } => {
                let sha256 = file.sha256.clone()?;
                let licence = self
                    .rows_with_sha(&sha256)
                    .find(|c| c.slot == slot)
                    .map_or(LicenceRef::Repo, |c| {
                        LicenceRef::Companion(c.license.clone())
                    });
                Some(PlannedFile::Hub {
                    repo: repo.clone(),
                    revision: revision.clone(),
                    path: file.path.clone(),
                    size: file.size,
                    sha256,
                    licence,
                    save_as: None,
                })
            }
            SlotChoice::Local { path, size, sha256 } => Some(PlannedFile::Local {
                path: path.clone(),
                size: *size,
                sha256: sha256.clone(),
            }),
            SlotChoice::Empty => None,
        }
    }
}

/// The text-encoder slots a recipe can have.
const TEXT_ENCODERS: [SlotId; 4] = [SlotId::ClipL, SlotId::ClipG, SlotId::T5xxl, SlotId::Llm];

fn companion_file(c: &Companion) -> PlannedFile {
    PlannedFile::Hub {
        repo: c.repo.clone(),
        revision: c.revision.clone(),
        path: c.path.clone(),
        size: c.size,
        sha256: c.sha256.clone(),
        licence: LicenceRef::Companion(c.license.clone()),
        save_as: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::gen_catalog::sha256_hex;
    use crate::sd::detect::FolderPart;
    use crate::sd::index::FileFormat;
    use crate::sd::inspect::{MatchFacts, inspect, inspect_folder};
    use crate::sd::matching::{Match, match_recipe};
    use crate::sd::names::diffusers_folder;
    use crate::sd::recipes::recipe;
    use crate::sd::testing::{fixture, fixture_source};

    const REV: &str = "0123456789abcdef0123456789abcdef01234567";

    fn table() -> CompanionTable {
        CompanionTable::from_catalog(&Catalog::bundled()).unwrap()
    }

    /// The bundled table's companion `id`.
    fn companion(id: &str) -> Companion {
        table().files.into_iter().find(|c| c.id == id).unwrap()
    }

    /// A main file on the Hub: `path` with the size and SHA-256 of the file the fixture was
    /// read from, and the fixture's facts.
    struct Main {
        repo: String,
        file: RepoFile,
        facts: FileFacts,
        part: Option<NoisePart>,
    }

    fn main_hub(repo: &str, path: &str, fixture_name: &str) -> Main {
        let source = fixture_source(fixture_name);
        Main {
            repo: repo.into(),
            file: RepoFile {
                path: path.into(),
                size: source.size,
                sha256: Some(source.sha256),
            },
            facts: inspect(&fixture(fixture_name)),
            part: None,
        }
    }

    fn main_hub_part(repo: &str, path: &str, fixture_name: &str, part: NoisePart) -> Main {
        Main {
            part: Some(part),
            ..main_hub(repo, path, fixture_name)
        }
    }

    fn version_of(facts: &FileFacts) -> Option<SdVersion> {
        match facts.kind {
            FileKind::Model { version, .. } => Some(version),
            _ => None,
        }
    }

    /// Everything a plan can be given beyond the main file and the recipe.
    #[derive(Default)]
    struct With<'a> {
        listing: &'a [RepoFile],
        listing_facts: BTreeMap<String, FileFacts>,
        installed: &'a [InstalledFile],
        companions: Option<CompanionTable>,
        overrides: Vec<(SlotId, SlotChoice)>,
    }

    fn plan_given(main: &Main, recipe_id: &str, with: With<'_>) -> SetupPlan {
        let companions = with.companions.unwrap_or_else(table);
        plan_setup(&SetupInput {
            main: MainFile::Hub {
                repo: &main.repo,
                revision: REV,
                file: &main.file,
            },
            facts: &main.facts,
            recipe: recipe(recipe_id).unwrap(),
            version: version_of(&main.facts),
            part: main.part,
            listing: with.listing,
            listing_facts: &with.listing_facts,
            installed: with.installed,
            companions: &companions,
            overrides: &with.overrides,
            untested: false,
        })
    }

    fn plan(
        main: Main,
        recipe_id: &str,
        listing: &[RepoFile],
        installed: &[InstalledFile],
    ) -> SetupPlan {
        plan_given(
            &main,
            recipe_id,
            With {
                listing,
                installed,
                ..With::default()
            },
        )
    }

    fn plan_overriding(slot: SlotId, choice: SlotChoice) -> SetupPlan {
        plan_given(
            &main_hub(
                "QuantStack/FLUX.1-Kontext-dev-GGUF",
                "flux1-kontext-dev-Q4_K_M.gguf",
                "flux1-kontext",
            ),
            "flux1-kontext",
            With {
                overrides: vec![(slot, choice)],
                ..With::default()
            },
        )
    }

    fn plan_folder(
        repo: &str,
        folder: &(DiffusersFolder, FileFacts),
        recipe_id: &str,
    ) -> SetupPlan {
        plan_folder_overriding(repo, folder, recipe_id, &[])
    }

    fn plan_folder_overriding(
        repo: &str,
        folder: &(DiffusersFolder, FileFacts),
        recipe_id: &str,
        overrides: &[(SlotId, SlotChoice)],
    ) -> SetupPlan {
        let (folder, facts) = folder;
        plan_setup(&SetupInput {
            main: MainFile::HubFolder {
                repo,
                revision: REV,
                folder,
            },
            facts,
            recipe: recipe(recipe_id).unwrap(),
            version: version_of(facts),
            part: None,
            listing: &[],
            listing_facts: &BTreeMap::new(),
            installed: &[],
            companions: &table(),
            overrides,
            untested: false,
        })
    }

    /// A listed file with a made-up SHA-256.
    fn rf(path: &str, size: u64) -> RepoFile {
        RepoFile {
            path: path.into(),
            size,
            sha256: Some(sha256_hex(path.as_bytes())),
        }
    }

    fn rf_sha(path: &str, size: u64, sha: &str) -> RepoFile {
        RepoFile {
            sha256: Some(sha.into()),
            ..rf(path, size)
        }
    }

    fn installed_file(slot: SlotId, sha: &str) -> InstalledFile {
        InstalledFile {
            slot,
            path: PathBuf::from(format!("/models/{}.gguf", &sha[..8])),
            sha256: sha.into(),
            size: 1,
        }
    }

    fn sources(p: &SetupPlan) -> Vec<(SlotId, PartSource)> {
        p.parts.iter().map(|x| (x.slot, x.source)).collect()
    }

    fn slot(p: &SetupPlan, s: SlotId) -> Option<&PlannedPart> {
        p.parts.iter().find(|x| x.slot == s)
    }

    fn source(p: &SetupPlan, s: SlotId) -> PartSource {
        slot(p, s).unwrap_or_else(|| panic!("no {s:?} part")).source
    }

    fn hub_path(p: &SetupPlan, s: SlotId) -> &str {
        match slot(p, s).and_then(|x| x.file.as_ref()) {
            Some(PlannedFile::Hub { path, .. }) => path,
            other => panic!("{s:?} is not a Hub file: {other:?}"),
        }
    }

    fn save_as(p: &SetupPlan, s: SlotId) -> Option<&str> {
        match slot(p, s)?.file.as_ref()? {
            PlannedFile::Hub { save_as, .. } => save_as.as_deref(),
            PlannedFile::Local { .. } => None,
        }
    }

    /// QuantStack's two-part Wan 2.2 repo: a high-noise and a low-noise file per quant.
    fn wan_a14b_listing(quants: &[&str]) -> Vec<RepoFile> {
        let size = |q: &str| match q {
            "Q4_K_M" => 9_651_327_232,
            "Q5_K_M" => 10_793_520_384,
            _ => 15_406_469_248,
        };
        quants
            .iter()
            .flat_map(|q| {
                [
                    rf(
                        &format!("HighNoise/Wan2.2-T2V-A14B-HighNoise-{q}.gguf"),
                        size(q),
                    ),
                    rf(
                        &format!("LowNoise/Wan2.2-T2V-A14B-LowNoise-{q}.gguf"),
                        size(q),
                    ),
                ]
            })
            .collect()
    }

    /// Comfy-Org's repackaged HunyuanVideo 1.5: diffusion models, its VAE and its ByT5.
    fn hunyuan_listing() -> Vec<RepoFile> {
        vec![
            rf(
                "split_files/diffusion_models/hunyuanvideo1.5_720p_t2v_fp16.safetensors",
                16_700_000_000,
            ),
            rf(
                "split_files/diffusion_models/hunyuanvideo1.5_720p_i2v_fp16.safetensors",
                16_700_000_000,
            ),
            rf(
                "split_files/vae/hunyuanvideo15_vae_fp16.safetensors",
                1_260_000_000,
            ),
            rf(
                "split_files/text_encoders/byt5_small_glyphxl_fp16.safetensors",
                439_000_000,
            ),
            RepoFile {
                path: "README.md".into(),
                size: 2_000,
                sha256: None,
            },
        ]
    }

    /// CompVis/stable-diffusion-v1-4's folder, with the facts `inspect_folder` gives its UNet
    /// and text encoder fixtures.
    fn sd14_folder() -> (DiffusersFolder, FileFacts) {
        let folder = diffusers_folder(&[
            rf("unet/diffusion_pytorch_model.safetensors", 3_438_167_534),
            rf("vae/diffusion_pytorch_model.safetensors", 334_643_268),
            rf("text_encoder/model.safetensors", 492_265_879),
            rf("model_index.json", 543),
        ])
        .unwrap();
        let (unet, te) = (
            fixture("sd14-diffusers-unet"),
            fixture("sd14-diffusers-text-encoder"),
        );
        let facts = inspect_folder(&[(FolderPart::Unet, &unet), (FolderPart::TextEncoder, &te)]);
        (folder, facts)
    }

    /// An SDXL folder whose second text encoder is not listed.
    fn sdxl_folder_without_text_encoder_2() -> (DiffusersFolder, FileFacts) {
        let folder = diffusers_folder(&[
            rf("unet/diffusion_pytorch_model.safetensors", 10_270_077_736),
            rf(
                "unet/diffusion_pytorch_model.fp16.safetensors",
                5_135_149_760,
            ),
            rf("vae/diffusion_pytorch_model.safetensors", 334_643_268),
            rf("text_encoder/model.safetensors", 492_265_879),
        ])
        .unwrap();
        let facts = FileFacts {
            kind: FileKind::Model {
                version: SdVersion::Sdxl,
                layout: Layout::Folder,
                embeds: vec![SlotId::ClipL, SlotId::Vae],
            },
            facts: MatchFacts::default(),
            parameters: 2_567_463_684,
            format: FileFormat::Safetensors,
        };
        (folder, facts)
    }

    /// A fixture's diffusion model alone, under its bare names, as a diffusion-only GGUF
    /// holds it.
    fn diffusion_only(name: &str) -> FileFacts {
        let mut index = fixture(name);
        index
            .tensors
            .retain(|t| t.name.starts_with("model.diffusion_model."));
        for t in &mut index.tensors {
            t.name = t.name["model.diffusion_model.".len()..].to_string();
        }
        inspect(&index)
    }

    /// An SDXL folder without its `vae/`.
    fn sdxl_folder_without_vae() -> (DiffusersFolder, FileFacts) {
        let folder = diffusers_folder(&[
            rf(
                "unet/diffusion_pytorch_model.fp16.safetensors",
                5_135_149_760,
            ),
            rf("text_encoder/model.fp16.safetensors", 246_144_352),
            rf("text_encoder_2/model.fp16.safetensors", 1_389_382_176),
        ])
        .unwrap();
        let facts = FileFacts {
            kind: FileKind::Model {
                version: SdVersion::Sdxl,
                layout: Layout::Folder,
                embeds: vec![SlotId::ClipL, SlotId::ClipG],
            },
            facts: MatchFacts::default(),
            parameters: 3_470_000_000,
            format: FileFormat::Safetensors,
        };
        (folder, facts)
    }

    #[test]
    fn a_flux_diffusion_file_gets_its_three_companions() {
        let p = plan(
            main_hub(
                "QuantStack/FLUX.1-Kontext-dev-GGUF",
                "flux1-kontext-dev-Q4_K_M.gguf",
                "flux1-kontext",
            ),
            "flux1-kontext",
            &[],
            &[],
        );
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::ClipL, PartSource::Companion),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Vae, PartSource::Companion)
            ]
        );
        assert_eq!(hub_path(&p, SlotId::T5xxl), "t5xxl_fp8_e4m3fn.safetensors");
        assert!(p.can_run() && !p.untested);
        assert_eq!(p.recipe_id, "flux1-kontext");
        let t5 = companion("t5xxl-fp8");
        assert_eq!(
            slot(&p, SlotId::T5xxl).unwrap().file,
            Some(PlannedFile::Hub {
                repo: t5.repo,
                revision: t5.revision,
                path: t5.path,
                size: t5.size,
                sha256: t5.sha256,
                licence: LicenceRef::Companion("apache-2.0".into()),
                save_as: None,
            })
        );
        assert_eq!(
            slot(&p, SlotId::DiffusionModel).unwrap().file,
            Some(PlannedFile::Hub {
                repo: "QuantStack/FLUX.1-Kontext-dev-GGUF".into(),
                revision: REV.into(),
                path: "flux1-kontext-dev-Q4_K_M.gguf".into(),
                size: fixture_source("flux1-kontext").size,
                sha256: fixture_source("flux1-kontext").sha256,
                licence: LicenceRef::Repo,
                save_as: None,
            })
        );
        // The vae fills `required_any [vae, tae]`, so the plan needs it.
        assert!(p.parts.iter().all(|x| x.required));
        assert_eq!(p.to_fetch().len(), 4);
    }

    /// The 0.2.1 report: Qwen Image 2.1's page showed a text encoder and a decoder with "No
    /// file yet", and a Download that could not start, because the table had nothing for the
    /// version. A lone diffusion file (the NF4 or Q8_0 file of the report) now plans all
    /// three: Qwen 3 VL 8B as the text encoder, Qwen Image 2.1's own decoder (not Qwen Image
    /// 1's: another file) and the vision projector the GGUF encoder needs for reference
    /// pictures, and the Turbo recipe plans the same files.
    #[test]
    fn a_qwen_image_21_file_gets_its_encoder_decoder_and_vision_projector() {
        let facts = FileFacts {
            kind: FileKind::Model {
                version: SdVersion::QwenImage21,
                layout: Layout::DiffusionOnly,
                embeds: Vec::new(),
            },
            facts: MatchFacts::default(),
            parameters: 20_000_000_000,
            format: FileFormat::Gguf,
        };
        let main = || Main {
            repo: "chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized".into(),
            file: rf("qwen-image-2.1-step90000-Q8_0.gguf", 7_687_183_136),
            facts: facts.clone(),
            part: None,
        };
        for recipe_id in ["qwen-image-21", "qwen-image-21-turbo"] {
            let p = plan(main(), recipe_id, &[], &[]);
            assert!(p.can_run(), "{recipe_id}: {:?}", p.missing());
            assert_eq!(
                sources(&p),
                [
                    (SlotId::DiffusionModel, PartSource::Main),
                    (SlotId::Llm, PartSource::Companion),
                    (SlotId::LlmVision, PartSource::Companion),
                    (SlotId::Vae, PartSource::Companion),
                ],
                "{recipe_id}"
            );
            assert_eq!(hub_path(&p, SlotId::Llm), "Qwen3VL-8B-Instruct-Q8_0.gguf");
            assert_eq!(
                hub_path(&p, SlotId::LlmVision),
                "mmproj-Qwen3VL-8B-Instruct-F16.gguf"
            );
            assert_eq!(
                hub_path(&p, SlotId::Vae),
                "vae/qwen_image_2.1_vae_bf16.safetensors"
            );
            // The decoder is Qwen Image 2.1's own, under Qwen's research licence; the
            // encoder and projector are Apache-2.0.
            let licence = |s| match &slot(&p, s).unwrap().file {
                Some(PlannedFile::Hub { licence, .. }) => licence.clone(),
                other => panic!("{other:?}"),
            };
            assert_eq!(
                licence(SlotId::Vae),
                LicenceRef::Companion("qwen-research".into())
            );
            assert_eq!(
                licence(SlotId::Llm),
                LicenceRef::Companion("apache-2.0".into())
            );
            assert_ne!(
                companion("qwen-image-vae").sha256,
                companion("qwen-image-21-vae").sha256
            );
        }
    }

    #[test]
    fn an_sd15_checkpoint_needs_nothing_else_and_shows_its_own_vae() {
        let p = plan(
            main_hub(
                "stable-diffusion-v1-5/stable-diffusion-v1-5",
                "v1-5-pruned-emaonly.safetensors",
                "sd15",
            ),
            "sd1-checkpoint",
            &[],
            &[],
        );
        // An optional vae, embedded (rule 4).
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::Vae, PartSource::Embedded)
            ]
        );
        assert!(p.to_fetch().len() == 1 && p.can_run());
        let vae = slot(&p, SlotId::Vae).unwrap();
        assert!(!vae.required && vae.file.is_none());
    }

    #[test]
    fn a_diffusers_folder_plan_lists_its_own_files() {
        let p = plan_folder(
            "CompVis/stable-diffusion-v1-4",
            &sd14_folder(),
            "sd1-checkpoint",
        );
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::Vae, PartSource::Folder),
                (SlotId::ClipL, PartSource::Folder)
            ]
        );
        assert_eq!(
            save_as(&p, SlotId::ClipL),
            Some("text_encoder/model.safetensors")
        );
        assert_eq!(
            save_as(&p, SlotId::Model),
            Some("unet/diffusion_pytorch_model.safetensors")
        );
        assert_eq!(
            hub_path(&p, SlotId::Vae),
            "vae/diffusion_pytorch_model.safetensors"
        );
        assert!(p.can_run());
        assert_eq!(p.to_fetch().len(), 3);
        assert!(slot(&p, SlotId::ClipL).unwrap().required);
        assert_eq!(
            plan_folder(
                "o/sdxl-base",
                &sdxl_folder_without_text_encoder_2(),
                "sdxl-checkpoint"
            )
            .missing(),
            [SlotId::ClipG]
        );
        // The fp16 UNet is fetched, under the engine's name.
        let sdxl = plan_folder(
            "o/sdxl-base",
            &sdxl_folder_without_text_encoder_2(),
            "sdxl-checkpoint",
        );
        assert_eq!(
            hub_path(&sdxl, SlotId::Model),
            "unet/diffusion_pytorch_model.fp16.safetensors"
        );
        assert_eq!(
            sources(&sdxl),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::Vae, PartSource::Folder),
                (SlotId::ClipL, PartSource::Folder),
                (SlotId::ClipG, PartSource::Missing)
            ]
        );
    }

    #[test]
    fn sd35_medium_embeds_its_vae_and_fetches_three_encoders() {
        let p = plan(
            main_hub(
                "second-state/stable-diffusion-3.5-medium-GGUF",
                "sd3.5_medium.safetensors",
                "sd35-medium",
            ),
            "sd35-medium",
            &[],
            &[],
        );
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::ClipL, PartSource::Companion),
                (SlotId::ClipG, PartSource::Companion),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Vae, PartSource::Embedded)
            ]
        );
        assert_eq!(
            slot(&p, SlotId::ClipG)
                .unwrap()
                .file
                .as_ref()
                .map(|f| match f {
                    PlannedFile::Hub { licence, .. } => licence.clone(),
                    PlannedFile::Local { .. } => LicenceRef::Repo,
                }),
            Some(LicenceRef::Companion("stabilityai-ai-community".into()))
        );
    }

    /// Ruling R6-wan-layout: a checkpoint-layout Wan 2.1 file goes into `model`, where the
    /// recipe has `diffusion_model`; the engine's `-m` reads `model.diffusion_model.*` names
    /// for any version.
    #[test]
    fn a_wan_checkpoint_goes_into_model_in_place_of_the_diffusion_model() {
        let path = "split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors";
        let main = main_hub(
            "Comfy-Org/Wan_2.1_ComfyUI_repackaged",
            path,
            "wan21-t2v-1.3b",
        );
        assert!(matches!(
            main.facts.kind,
            FileKind::Model {
                version: SdVersion::Wan2,
                layout: Layout::Checkpoint,
                ..
            }
        ));
        assert_eq!(
            match_recipe(SdVersion::Wan2, &main.facts.facts, &[], path),
            Match::Recipe {
                id: "wan21-t2v",
                part: None
            }
        );
        let p = plan(main, "wan21-t2v", &[], &[]);
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Vae, PartSource::Companion)
            ]
        );
        assert!(slot(&p, SlotId::DiffusionModel).is_none());
        assert_eq!(hub_path(&p, SlotId::Model), path);
        assert_eq!(
            hub_path(&p, SlotId::Vae),
            "split_files/vae/wan_2.1_vae.safetensors"
        );
        assert!(p.can_run());

        // Choosing the recipe's diffusion model replaces the checkpoint.
        let p = plan_given(
            &main_hub("o/r", path, "wan21-t2v-1.3b"),
            "wan21-t2v",
            With {
                overrides: vec![(
                    SlotId::DiffusionModel,
                    SlotChoice::Local {
                        path: "/d/wan.gguf".into(),
                        size: 1,
                        sha256: None,
                    },
                )],
                ..With::default()
            },
        );
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Chosen),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Vae, PartSource::Companion)
            ]
        );
    }

    /// Ruling S4-diffusion-only: `-m` loads with the prefix `""` and the engine knows a
    /// diffusion model only by `model.diffusion_model.` or `unet.` names
    /// (`src/model_loader.cpp:801`), so a diffusion model alone always takes
    /// `--diffusion-model`, also for a recipe whose main slot is `model`. As it carries no
    /// encoder or VAE, the recipe's text encoders and its VAE (or the VAE/TAE pair) are
    /// required.
    #[test]
    fn a_diffusion_only_file_goes_into_diffusion_model_and_needs_its_encoders_and_vae() {
        let main = |facts| Main {
            repo: "o/sd3.5-medium-GGUF".into(),
            file: rf("sd3.5_medium-Q4_0.gguf", 2_500_000_000),
            facts,
            part: None,
        };
        let sd3 = diffusion_only("sd35-medium");
        assert_eq!(
            sd3.kind,
            FileKind::Model {
                version: SdVersion::Sd3,
                layout: Layout::DiffusionOnly,
                embeds: vec![]
            }
        );
        // The table has SD3's encoders and no SD3 VAE.
        let p = plan(main(sd3.clone()), "sd35-medium", &[], &[]);
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::ClipL, PartSource::Companion),
                (SlotId::ClipG, PartSource::Companion),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Vae, PartSource::Missing)
            ]
        );
        assert!(slot(&p, SlotId::Model).is_none() && !p.can_run());
        assert!(p.parts.iter().all(|x| x.required));

        // The repo's own files, a diffusers VAE among them.
        let listing = [
            rf("sd3.5_medium-Q4_0.gguf", 2_500_000_000),
            rf("text_encoders/clip_l.safetensors", 246_144_152),
            rf("text_encoders/clip_g.safetensors", 1_389_382_176),
            rf("text_encoders/t5xxl_fp8_e4m3fn.safetensors", 4_893_934_904),
            rf("vae/diffusion_pytorch_model.safetensors", 167_666_902),
        ];
        let p = plan(main(sd3.clone()), "sd35-medium", &listing, &[]);
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::ClipL, PartSource::Repo),
                (SlotId::ClipG, PartSource::Repo),
                (SlotId::T5xxl, PartSource::Repo),
                (SlotId::Vae, PartSource::Repo)
            ]
        );
        assert_eq!(
            hub_path(&p, SlotId::Vae),
            "vae/diffusion_pytorch_model.safetensors"
        );
        assert!(p.can_run());

        // A chosen TAE meets the VAE/TAE pair; choosing the recipe's `model` replaces the
        // diffusion model.
        let local = |path: &str| SlotChoice::Local {
            path: path.into(),
            size: 1,
            sha256: None,
        };
        let p = plan_given(
            &main(sd3.clone()),
            "sd35-medium",
            With {
                overrides: vec![(SlotId::Tae, local("/d/taesd3.safetensors"))],
                ..With::default()
            },
        );
        assert_eq!(source(&p, SlotId::Tae), PartSource::Chosen);
        assert!(slot(&p, SlotId::Vae).is_none() && p.can_run());
        let p = plan_given(
            &main(sd3),
            "sd35-medium",
            With {
                overrides: vec![(SlotId::Model, local("/d/sd3.5_medium.safetensors"))],
                ..With::default()
            },
        );
        assert_eq!(
            sources(&p)[0],
            (SlotId::Model, PartSource::Chosen),
            "{:?}",
            sources(&p)
        );
        assert!(slot(&p, SlotId::DiffusionModel).is_none());
    }

    /// Ruling S4-unet-encoders: the SD 1.x, 2.x and XL checkpoint recipes have no CLIP
    /// slots, but the engine loads `--clip_l` and `--clip_g` under names it maps onto a
    /// checkpoint's for those versions (`src/pipeline/diffusion_engine.cpp:719`,
    /// `src/name_conversion.cpp:1773-1775`). A main file that carries none needs them.
    #[test]
    fn a_diffusion_only_sd_file_needs_its_clip_encoders() {
        let main = |name: &str| Main {
            repo: "o/sdxl-unet-GGUF".into(),
            file: rf("sdxl-unet-Q8_0.gguf", 2_600_000_000),
            facts: diffusion_only(name),
            part: None,
        };
        assert!(matches!(
            main("sdxl").facts.kind,
            FileKind::Model {
                version: SdVersion::Sdxl,
                layout: Layout::DiffusionOnly,
                ..
            }
        ));
        // The table has no SDXL encoders or VAE: all three are missing.
        let p = plan(main("sdxl"), "sdxl-checkpoint", &[], &[]);
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::Vae, PartSource::Missing),
                (SlotId::ClipL, PartSource::Missing),
                (SlotId::ClipG, PartSource::Missing)
            ]
        );
        assert!(p.parts.iter().all(|x| x.required));

        // A chosen VAE alone does not make it runnable; its encoders too do.
        let local = |path: &str| SlotChoice::Local {
            path: path.into(),
            size: 1,
            sha256: None,
        };
        let vae = (SlotId::Vae, local("/d/sdxl_vae.safetensors"));
        let p = plan_given(
            &main("sdxl"),
            "sdxl-checkpoint",
            With {
                overrides: vec![vae.clone()],
                ..With::default()
            },
        );
        assert_eq!(p.missing(), [SlotId::ClipL, SlotId::ClipG]);
        assert!(!p.can_run());
        let p = plan_given(
            &main("sdxl"),
            "sdxl-checkpoint",
            With {
                overrides: vec![
                    vae,
                    (SlotId::ClipL, local("/d/clip_l.safetensors")),
                    (SlotId::ClipG, local("/d/clip_g.safetensors")),
                ],
                ..With::default()
            },
        );
        assert_eq!(
            (source(&p, SlotId::ClipL), source(&p, SlotId::ClipG)),
            (PartSource::Chosen, PartSource::Chosen)
        );
        assert!(p.can_run());
        // Emptying one leaves it missing.
        let p = plan_given(
            &main("sdxl"),
            "sdxl-checkpoint",
            With {
                overrides: vec![(SlotId::ClipG, SlotChoice::Empty)],
                ..With::default()
            },
        );
        assert_eq!(p.missing(), [SlotId::Vae, SlotId::ClipL, SlotId::ClipG]);

        // From the same repo, by name.
        let listing = [
            rf("sdxl-unet-Q8_0.gguf", 2_600_000_000),
            rf("clip_l.safetensors", 246_144_152),
            rf("clip_g.safetensors", 1_389_382_176),
            rf("sdxl_vae.safetensors", 334_643_268),
        ];
        let p = plan(main("sdxl"), "sdxl-checkpoint", &listing, &[]);
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::Vae, PartSource::Repo),
                (SlotId::ClipL, PartSource::Repo),
                (SlotId::ClipG, PartSource::Repo)
            ]
        );
        assert!(p.can_run());

        // SD 1.x takes `clip_l` alone. Without its text encoder the engine does not name a
        // bare SD 1.x UNet's version, so these facts are built (as an untested set-up's).
        let sd1 = Main {
            facts: FileFacts {
                kind: FileKind::Model {
                    version: SdVersion::Sd1,
                    layout: Layout::DiffusionOnly,
                    embeds: vec![],
                },
                facts: MatchFacts::default(),
                parameters: 859_520_964,
                format: FileFormat::Gguf,
            },
            ..main("sdxl")
        };
        assert_eq!(
            sources(&plan(sd1, "sd1-checkpoint", &[], &[])),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::Vae, PartSource::Missing),
                (SlotId::ClipL, PartSource::Missing)
            ]
        );

        // A full SDXL checkpoint carries its own: no CLIP parts.
        let p = plan(
            main_hub(
                "stabilityai/stable-diffusion-xl-base-1.0",
                "sd_xl_base_1.0.safetensors",
                "sdxl",
            ),
            "sdxl-checkpoint",
            &[],
            &[],
        );
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::Vae, PartSource::Embedded)
            ]
        );
        assert!(p.can_run());
    }

    #[test]
    fn an_sdxl_folder_without_a_vae_needs_one() {
        let p = plan_folder("o/sdxl-base", &sdxl_folder_without_vae(), "sdxl-checkpoint");
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::Vae, PartSource::Missing),
                (SlotId::ClipL, PartSource::Folder),
                (SlotId::ClipG, PartSource::Folder)
            ]
        );
        assert!(slot(&p, SlotId::Vae).unwrap().required && !p.can_run());
        // The folder's files decide, never facts that say it embeds a VAE.
        let (folder, mut facts) = sdxl_folder_without_vae();
        facts.kind = FileKind::Model {
            version: SdVersion::Sdxl,
            layout: Layout::Folder,
            embeds: vec![SlotId::ClipL, SlotId::ClipG, SlotId::Vae],
        };
        let p = plan_folder("o/sdxl-base", &(folder, facts), "sdxl-checkpoint");
        assert_eq!(source(&p, SlotId::Vae), PartSource::Missing);
    }

    #[test]
    fn a_wan_a14b_high_noise_file_brings_its_low_noise_sibling_of_the_same_quant() {
        // HighNoise/… and LowNoise/… for each
        let listing = wan_a14b_listing(&["Q4_K_M", "Q8_0"]);
        let p = plan(
            main_hub_part(
                "QuantStack/Wan2.2-T2V-A14B-GGUF",
                "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf",
                "wan22-t2v-a14b-high",
                NoisePart::High,
            ),
            "wan22-t2v",
            &listing,
            &[],
        );
        assert_eq!(
            hub_path(&p, SlotId::HighNoiseDiffusionModel),
            "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf"
        );
        assert_eq!(
            hub_path(&p, SlotId::DiffusionModel),
            "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf"
        );
        assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Sibling);
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Sibling),
                (SlotId::HighNoiseDiffusionModel, PartSource::Main),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Vae, PartSource::Companion)
            ]
        );

        // A low-noise main file brings its high-noise sibling.
        let p = plan(
            main_hub_part(
                "QuantStack/Wan2.2-T2V-A14B-GGUF",
                "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q8_0.gguf",
                "wan22-t2v-a14b-low",
                NoisePart::Low,
            ),
            "wan22-t2v",
            &listing,
            &[],
        );
        assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Main);
        assert_eq!(
            hub_path(&p, SlotId::HighNoiseDiffusionModel),
            "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q8_0.gguf"
        );
    }

    #[test]
    fn without_the_same_quant_the_sibling_is_the_smallest_of_the_other_part() {
        let mut listing = wan_a14b_listing(&["Q8_0", "Q5_K_M"]);
        listing.retain(|f| !f.path.contains("HighNoise-Q"));
        listing.push(rf(
            "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf",
            9_651_327_232,
        ));
        // Never a LoRA, a file without a SHA-256 or another depth's file of the same quant.
        listing.push(rf("loras/lightx2v_lora_low_noise.safetensors", 1_000));
        listing.push(rf("Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf", 9_651_327_232));
        listing.push(RepoFile {
            path: "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_S.gguf".into(),
            size: 8_000_000_000,
            sha256: None,
        });
        let main = || {
            main_hub_part(
                "QuantStack/Wan2.2-T2V-A14B-GGUF",
                "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf",
                "wan22-t2v-a14b-high",
                NoisePart::High,
            )
        };
        let p = plan(main(), "wan22-t2v", &listing, &[]);
        assert_eq!(
            hub_path(&p, SlotId::DiffusionModel),
            "Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf"
        );
        listing.retain(|f| f.path != "Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf");
        let p = plan(main(), "wan22-t2v", &listing, &[]);
        assert_eq!(
            hub_path(&p, SlotId::DiffusionModel),
            "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q5_K_M.gguf"
        );
        assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Sibling);

        // No sibling at all: the other slot is missing.
        let p = plan(main(), "wan22-t2v", &[], &[]);
        assert_eq!(p.missing(), [SlotId::DiffusionModel]);
    }

    #[test]
    fn among_equal_quants_the_sibling_mirrors_the_main_files_name() {
        // Comfy-Org's repackaged repo holds the text-to-video and picture-to-video pairs.
        let dir = "split_files/diffusion_models";
        let listing = [
            rf(
                &format!("{dir}/wan2.2_i2v_low_noise_14B_fp8_scaled.safetensors"),
                14_000_000_000,
            ),
            rf(
                &format!("{dir}/wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors"),
                14_300_000_000,
            ),
            rf(
                &format!("{dir}/wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors"),
                14_300_000_000,
            ),
            rf(
                &format!("{dir}/wan2.2_i2v_high_noise_14B_fp8_scaled.safetensors"),
                14_000_000_000,
            ),
        ];
        let p = plan(
            main_hub_part(
                "Comfy-Org/Wan_2.2_ComfyUI_Repackaged",
                &format!("{dir}/wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors"),
                "wan22-t2v-a14b-high",
                NoisePart::High,
            ),
            "wan22-t2v",
            &listing,
            &[],
        );
        assert_eq!(
            hub_path(&p, SlotId::DiffusionModel),
            format!("{dir}/wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors")
        );
    }

    const LOCAL_SHA: &str = "abababababababababababababababababababababababababababababababab";

    /// A file planned by hand (*Set it up yourself*, a file not read): no facts, no part.
    fn by_hand<'a>(
        main: MainFile<'a>,
        facts: &'a FileFacts,
        recipe_id: &str,
        listing: &'a [RepoFile],
        companions: &'a CompanionTable,
    ) -> SetupPlan {
        plan_setup(&SetupInput {
            main,
            facts,
            recipe: recipe(recipe_id).unwrap(),
            version: None,
            part: None,
            listing,
            listing_facts: &BTreeMap::new(),
            installed: &[],
            companions,
            overrides: &[],
            untested: true,
        })
    }

    fn unread_facts() -> FileFacts {
        FileFacts {
            kind: FileKind::CannotRun {
                reason: crate::sd::CannotRunReason::UnknownWeights,
                version: None,
            },
            facts: MatchFacts::default(),
            parameters: 0,
            format: FileFormat::Safetensors,
        }
    }

    /// Final review, area 1, I-3: a high-noise file set up by hand, with no known part,
    /// goes in the high-noise slot by its name, and its sibling is its own model's low half,
    /// never another high-noise file.
    #[test]
    fn a_high_noise_file_set_up_by_hand_takes_the_high_noise_slot_and_its_own_low_half() {
        let listing = crate::sd::testing::comfy_wan22_listing();
        let companions = table();
        let facts = unread_facts();
        let dir = "split_files/diffusion_models/";
        let listed = |name: &str| {
            listing
                .iter()
                .find(|f| f.path == format!("{dir}{name}"))
                .unwrap()
        };
        let hub = |name: &str| MainFile::Hub {
            repo: "Comfy-Org/Wan-2.2",
            revision: REV,
            file: listed(name),
        };
        let p = by_hand(
            hub("wan2.2_t2v_high_noise_14B_fp16.safetensors"),
            &facts,
            "wan22-t2v",
            &listing,
            &companions,
        );
        assert_eq!(
            hub_path(&p, SlotId::HighNoiseDiffusionModel),
            format!("{dir}wan2.2_t2v_high_noise_14B_fp16.safetensors")
        );
        assert_eq!(
            source(&p, SlotId::HighNoiseDiffusionModel),
            PartSource::Main
        );
        assert_eq!(
            hub_path(&p, SlotId::DiffusionModel),
            format!("{dir}wan2.2_t2v_low_noise_14B_fp16.safetensors")
        );
        assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Sibling);

        // The low half by hand: the low slot, and the T2V high half.
        let p = by_hand(
            hub("wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors"),
            &facts,
            "wan22-t2v",
            &listing,
            &companions,
        );
        assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Main);
        assert_eq!(
            hub_path(&p, SlotId::HighNoiseDiffusionModel),
            format!("{dir}wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors")
        );

        // A file of this computer is placed by its file name, not its folders.
        let path = Path::new("/home/high/wan2.2_t2v_low_noise_14B_fp16.safetensors");
        let local = MainFile::Local {
            path,
            size: 28_577_095_592,
            sha256: Some(LOCAL_SHA),
        };
        let p = by_hand(local, &facts, "wan22-t2v", &[], &companions);
        assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Main);
        assert_eq!(
            source(&p, SlotId::HighNoiseDiffusionModel),
            PartSource::Missing
        );
        let path = Path::new("/home/me/Wan2_2-T2V-A14B-HIGH_fp8_e4m3fn_scaled_KJ.safetensors");
        let local = MainFile::Local {
            path,
            size: 15_000_000_000,
            sha256: Some(LOCAL_SHA),
        };
        let p = by_hand(local, &facts, "wan22-t2v", &[], &companions);
        assert_eq!(
            source(&p, SlotId::HighNoiseDiffusionModel),
            PartSource::Main
        );
        assert_eq!(p.missing(), [SlotId::DiffusionModel]);
    }

    #[test]
    fn a_sibling_is_never_of_the_main_files_own_noise_part() {
        // A caller that says "low" for a file named high noise: the file fills the low slot,
        // and no other high-noise file is taken as its high half; the plan can't run.
        let listing = crate::sd::testing::comfy_wan22_listing();
        let main = listing
            .iter()
            .find(|f| {
                f.path
                    .ends_with("wan2.2_t2v_high_noise_14B_fp16.safetensors")
            })
            .unwrap();
        let companions = table();
        let facts = unread_facts();
        let p = plan_setup(&SetupInput {
            main: MainFile::Hub {
                repo: "Comfy-Org/Wan-2.2",
                revision: REV,
                file: main,
            },
            facts: &facts,
            recipe: recipe("wan22-t2v").unwrap(),
            version: None,
            part: Some(NoisePart::Low),
            listing: &listing,
            listing_facts: &BTreeMap::new(),
            installed: &[],
            companions: &companions,
            overrides: &[],
            untested: true,
        });
        assert_eq!(source(&p, SlotId::DiffusionModel), PartSource::Main);
        assert_eq!(p.missing(), [SlotId::HighNoiseDiffusionModel]);
        assert!(!p.can_run());
    }

    #[test]
    fn an_installed_01x_encoder_is_reused_instead_of_a_download() {
        // klein's Qwen3-4B Q4_K_M
        let installed = [installed_file(
            SlotId::Llm,
            "f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a",
        )];
        let p = plan(
            main_hub(
                "o/FLUX.2-klein-4B-GGUF",
                "flux-2-klein-4b-Q8_0.gguf",
                "flux2-klein-4b",
            ),
            "flux2-klein-4b",
            &[],
            &installed,
        );
        assert_eq!(source(&p, SlotId::Llm), PartSource::Installed);
        assert!(
            p.to_fetch()
                .iter()
                .all(|f| !matches!(f, PlannedFile::Hub { path, .. } if path.contains("Qwen3")))
        );
        assert_eq!(
            slot(&p, SlotId::Llm).unwrap().file,
            Some(PlannedFile::Local {
                path: installed[0].path.clone(),
                size: 1,
                sha256: Some(installed[0].sha256.clone()),
            })
        );
    }

    #[test]
    fn an_installed_file_is_reused_only_where_it_suits_the_recipe_and_version() {
        let kontext = || {
            main_hub(
                "QuantStack/FLUX.1-Kontext-dev-GGUF",
                "flux1-kontext-dev-Q4_K_M.gguf",
                "flux1-kontext",
            )
        };
        // Klein 9B takes Qwen3-8B: the 4B encoder klein 4B installed is no variant of it.
        let qwen3_4b = [installed_file(
            SlotId::Llm,
            &companion("qwen3-4b-q4km").sha256,
        )];
        let p = plan(
            main_hub(
                "o/FLUX.2-klein-9B-GGUF",
                "flux-2-klein-9b-Q8_0.gguf",
                "flux2-klein-4b",
            ),
            "flux2-klein-9b",
            &[],
            &qwen3_4b,
        );
        assert_eq!(source(&p, SlotId::Llm), PartSource::Companion);
        assert_eq!(hub_path(&p, SlotId::Llm), "Qwen3-8B-Q8_0.gguf");

        // An installed VAE of another family is not taken because the slot matches, nor is
        // one Studio does not know; FLUX's own is.
        for other in [
            companion("wan21-vae").sha256,
            companion("qwen-image-vae").sha256,
            "ab".repeat(32),
        ] {
            let p = plan(
                kontext(),
                "flux1-kontext",
                &[],
                &[installed_file(SlotId::Vae, &other)],
            );
            assert_eq!(source(&p, SlotId::Vae), PartSource::Companion, "{other}");
        }
        let ae = companion("ae-flux1").sha256.to_uppercase();
        let p = plan(
            kontext(),
            "flux1-kontext",
            &[],
            &[installed_file(SlotId::Vae, &ae)],
        );
        assert_eq!(source(&p, SlotId::Vae), PartSource::Installed);
        // An installed T5 variant (fp16 for the fp8 default) is reused too.
        let fp16 = [installed_file(
            SlotId::T5xxl,
            &companion("t5xxl-fp16").sha256,
        )];
        let p = plan(kontext(), "flux1-kontext", &[], &fp16);
        assert_eq!(source(&p, SlotId::T5xxl), PartSource::Installed);

        // A file installed from the same repo before: the SHA-256 the Repo source picks.
        let listing = hunyuan_listing();
        let vae_sha = listing[2].sha256.clone().unwrap();
        let p = plan(
            main_hub(
                "Comfy-Org/Hunyuan-Video-1.5",
                "split_files/diffusion_models/hunyuanvideo1.5_720p_t2v_fp16.safetensors",
                "hunyuan-video-15",
            ),
            "hunyuan-video-15-t2v",
            &listing,
            &[installed_file(SlotId::Vae, &vae_sha)],
        );
        assert_eq!(source(&p, SlotId::Vae), PartSource::Installed);
        assert_eq!(source(&p, SlotId::T5xxl), PartSource::Repo);
    }

    /// `by_sha` gives the first row with a SHA-256; a SHA on two rows takes the row that
    /// suits the recipe and version.
    #[test]
    fn a_sha_on_two_companion_rows_takes_the_row_that_suits_the_recipe() {
        let q4km = companion("qwen3-4b-q4km");
        let mut companions = table();
        // A first row with the same SHA-256, for Qwen-Image, as a variant of its encoder.
        companions.files.insert(
            0,
            Companion {
                id: "qwen-image-llm-twin".into(),
                versions: vec![SdVersion::QwenImage],
                repo: "unsloth/Qwen2.5-VL-7B-Instruct-GGUF".into(),
                ..q4km.clone()
            },
        );
        assert_eq!(
            companions.by_sha(&q4km.sha256).map(|c| c.id.as_str()),
            Some("qwen-image-llm-twin")
        );
        let installed = [installed_file(SlotId::Llm, &q4km.sha256)];
        for (main, recipe_id) in [
            (
                main_hub(
                    "o/FLUX.2-klein-4B-GGUF",
                    "flux-2-klein-4b-Q8_0.gguf",
                    "flux2-klein-4b",
                ),
                "flux2-klein-4b",
            ),
            (
                main_hub("o/Qwen-Image-GGUF", "q.gguf", "qwen-image"),
                "qwen-image",
            ),
        ] {
            let p = plan_given(
                &main,
                recipe_id,
                With {
                    installed: &installed,
                    companions: Some(companions.clone()),
                    ..With::default()
                },
            );
            assert_eq!(
                source(&p, SlotId::Llm),
                PartSource::Installed,
                "{recipe_id}"
            );
        }
        // Neither row suits Z-Image.
        let p = plan_given(
            &main_hub("o/Z-Image-Turbo-GGUF", "z.gguf", "z-image-turbo"),
            "z-image-turbo",
            With {
                installed: &installed,
                companions: Some(companions),
                ..With::default()
            },
        );
        assert_eq!(source(&p, SlotId::Llm), PartSource::Companion);
    }

    #[test]
    fn a_file_in_the_same_repo_beats_a_companion() {
        // Comfy-Org/Hunyuan-Video-1.5 ships its VAE and ByT5: both come from the repo, the llm
        // from the table
        let p = plan(
            main_hub(
                "Comfy-Org/Hunyuan-Video-1.5",
                "split_files/diffusion_models/hunyuanvideo1.5_720p_t2v_fp16.safetensors",
                "hunyuan-video-15",
            ),
            "hunyuan-video-15-t2v",
            &hunyuan_listing(),
            &[],
        );
        assert_eq!(
            (
                source(&p, SlotId::Vae),
                source(&p, SlotId::T5xxl),
                source(&p, SlotId::Llm)
            ),
            (PartSource::Repo, PartSource::Repo, PartSource::Companion)
        );
        assert_eq!(
            hub_path(&p, SlotId::T5xxl),
            "split_files/text_encoders/byt5_small_glyphxl_fp16.safetensors"
        );
        assert!(matches!(
            slot(&p, SlotId::Vae).unwrap().file,
            Some(PlannedFile::Hub { ref repo, licence: LicenceRef::Repo, .. }) if repo == "Comfy-Org/Hunyuan-Video-1.5"
        ));
        assert!(p.can_run());
    }

    #[test]
    fn the_repo_source_takes_the_smallest_pinned_file_that_fits() {
        let main = || {
            main_hub(
                "o/Wan2.1-T2V-14B-GGUF",
                "wan2.1-t2v-14b-Q4_K_M.gguf",
                "wan21-i2v-14b",
            )
        };
        let t5 = |path: &str, size| rf(path, size);
        // The smallest; on a tie, the main file's quant; never a file without a SHA-256, a
        // shard, a VAE by name over 2 GB, or the main file itself.
        let listing = [
            rf("wan2.1-t2v-14b-Q4_K_M.gguf", 9_000_000_000),
            t5("umt5-xxl-encoder-Q8_0.gguf", 6_043_068_256),
            t5("umt5-xxl-encoder-Q5_K_M.gguf", 4_145_878_880),
            t5("umt5-xxl-encoder-Q4_K_M.gguf", 4_145_878_880),
            RepoFile {
                path: "umt5-xxl-encoder-Q3_K_S.gguf".into(),
                size: 2_000_000_000,
                sha256: None,
            },
            rf(
                "text_encoders/umt5-xxl-00001-of-00002.safetensors",
                1_000_000,
            ),
            rf("wan2.1-14b-with-vae.safetensors", 1_000),
            rf("model-vae.safetensors", 6_000_000_000),
        ];
        let p = plan(main(), "wan21-t2v", &listing, &[]);
        assert_eq!(hub_path(&p, SlotId::T5xxl), "umt5-xxl-encoder-Q4_K_M.gguf");
        assert_eq!(source(&p, SlotId::Vae), PartSource::Companion);

        // A file whose facts say it is an extra part, whatever its name.
        let mut listing_facts = BTreeMap::new();
        listing_facts.insert("enc.gguf".to_string(), inspect(&fixture("umt5-xxl-q5km")));
        // Facts win over a name.
        listing_facts.insert(
            "tiny-vae.safetensors".to_string(),
            inspect(&fixture("clip-l")),
        );
        let listing = [
            rf("enc.gguf", 4_000_000_000),
            rf("tiny-vae.safetensors", 1_000),
        ];
        let p = plan_given(
            &main(),
            "wan21-t2v",
            With {
                listing: &listing,
                listing_facts,
                ..With::default()
            },
        );
        assert_eq!(
            (hub_path(&p, SlotId::T5xxl), source(&p, SlotId::Vae)),
            ("enc.gguf", PartSource::Companion)
        );
    }

    /// Comfy-Org's Wan 2.2 repo holds both VAEs: a known companion that does not suit the
    /// version is never taken from the repo because the slot matches.
    #[test]
    fn a_repo_file_known_as_another_versions_companion_is_skipped() {
        let (wan21, wan22) = (companion("wan21-vae"), companion("wan22-vae"));
        let listing = [
            rf_sha(
                "split_files/vae/wan_2.1_vae.safetensors",
                wan21.size,
                &wan21.sha256,
            ),
            rf_sha(
                "split_files/vae/wan2.2_vae.safetensors",
                wan22.size,
                &wan22.sha256,
            ),
        ];
        let repo = "Comfy-Org/Wan_2.2_ComfyUI_Repackaged";
        let ti2v = plan(
            main_hub(
                repo,
                "split_files/diffusion_models/wan2.2_ti2v_5B_fp16.safetensors",
                "wan22-ti2v-5b",
            ),
            "wan22-ti2v-5b",
            &listing,
            &[],
        );
        assert_eq!(
            (source(&ti2v, SlotId::Vae), hub_path(&ti2v, SlotId::Vae)),
            (PartSource::Repo, "split_files/vae/wan2.2_vae.safetensors")
        );
        let a14b = plan(
            main_hub_part(
                repo,
                "split_files/diffusion_models/wan2.2_t2v_high_noise_14B_fp16.safetensors",
                "wan22-t2v-a14b-high",
                NoisePart::High,
            ),
            "wan22-t2v",
            &listing,
            &[],
        );
        assert_eq!(
            hub_path(&a14b, SlotId::Vae),
            "split_files/vae/wan_2.1_vae.safetensors"
        );
    }

    #[test]
    fn a_slot_nothing_can_fill_is_missing_and_the_plan_cannot_run() {
        let p = plan(
            main_hub("o/HunyuanVideo-1.5-GGUF", "h-Q4_0.gguf", "hunyuan-video-15"),
            "hunyuan-video-15-t2v",
            &[],
            &[],
        );
        assert_eq!(p.missing(), [SlotId::T5xxl, SlotId::Vae]);
        assert!(!p.can_run());
        assert!(slot(&p, SlotId::Vae).unwrap().required);
    }

    #[test]
    fn the_vision_encoder_is_added_for_an_edit_recipe_only() {
        assert_eq!(
            source(
                &plan(
                    main_hub("o/r", "q.gguf", "qwen-image-edit-2509"),
                    "qwen-image-edit-2509",
                    &[],
                    &[]
                ),
                SlotId::LlmVision
            ),
            PartSource::Companion
        );
        assert!(
            slot(
                &plan(
                    main_hub("o/r", "q.gguf", "qwen-image"),
                    "qwen-image",
                    &[],
                    &[]
                ),
                SlotId::LlmVision
            )
            .is_none()
        );
        // Wan 2.1 picture-to-video reads its start frame through CLIP vision.
        let p = plan(
            main_hub("o/r", "w.gguf", "wan21-i2v-14b"),
            "wan21-i2v",
            &[],
            &[],
        );
        assert_eq!(source(&p, SlotId::ClipVision), PartSource::Companion);
        assert!(!slot(&p, SlotId::ClipVision).unwrap().required);
    }

    #[test]
    fn an_override_wins_and_empty_makes_a_required_slot_missing() {
        let p = plan_overriding(
            SlotId::Vae,
            SlotChoice::Local {
                path: "/d/ae.safetensors".into(),
                size: 1,
                sha256: None,
            },
        );
        assert_eq!(source(&p, SlotId::Vae), PartSource::Chosen);
        assert_eq!(
            plan_overriding(SlotId::T5xxl, SlotChoice::Empty).missing(),
            [SlotId::T5xxl]
        );

        // A Hub file: pinned, with a companion's licence when its SHA-256 is one's; one
        // without a SHA-256 cannot be pinned, so a required slot is missing.
        let t5 = companion("t5xxl-fp16");
        let hub = |sha256: Option<String>| SlotChoice::Hub {
            repo: "someone/encoders".into(),
            revision: REV.into(),
            file: RepoFile {
                path: "t5.safetensors".into(),
                size: t5.size,
                sha256,
            },
        };
        let p = plan_overriding(SlotId::T5xxl, hub(Some(t5.sha256.clone())));
        assert_eq!(
            slot(&p, SlotId::T5xxl).unwrap().file,
            Some(PlannedFile::Hub {
                repo: "someone/encoders".into(),
                revision: REV.into(),
                path: "t5.safetensors".into(),
                size: t5.size,
                sha256: t5.sha256,
                licence: LicenceRef::Companion("apache-2.0".into()),
                save_as: None,
            })
        );
        assert_eq!(
            plan_overriding(SlotId::T5xxl, hub(None)).missing(),
            [SlotId::T5xxl]
        );

        // A decoder of the other kind fills `required_any`: no vae is added for it, and an
        // emptied vae is then simply not there.
        let tae = SlotChoice::Local {
            path: "/d/taef1.safetensors".into(),
            size: 1,
            sha256: None,
        };
        let p = plan_given(
            &main_hub("o/r", "flux1-kontext-dev-Q4_K_M.gguf", "flux1-kontext"),
            "flux1-kontext",
            With {
                overrides: vec![(SlotId::Vae, SlotChoice::Empty), (SlotId::Tae, tae)],
                ..With::default()
            },
        );
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::ClipL, PartSource::Companion),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Tae, PartSource::Chosen)
            ]
        );
        assert!(p.can_run() && !slot(&p, SlotId::Tae).unwrap().required);
        // Without one, emptying the vae leaves `required_any` unmet.
        assert_eq!(
            plan_overriding(SlotId::Vae, SlotChoice::Empty).missing(),
            [SlotId::Vae]
        );

        // Empty clears an optional slot the checkpoint embeds; a slot the recipe does not
        // have is not added.
        let p = plan_given(
            &main_hub("o/sd15", "v1-5-pruned-emaonly.safetensors", "sd15"),
            "sd1-checkpoint",
            With {
                overrides: vec![
                    (SlotId::Vae, SlotChoice::Empty),
                    (
                        SlotId::Llm,
                        SlotChoice::Local {
                            path: "/d/llm.gguf".into(),
                            size: 1,
                            sha256: None,
                        },
                    ),
                ],
                ..With::default()
            },
        );
        assert_eq!(sources(&p), [(SlotId::Model, PartSource::Main)]);
    }

    /// A slot the plan needs reads required whoever fills it: the vae of a diffusion model
    /// alone, which fills `required_any [vae, tae]`, is required when the person's file fills
    /// it too (S6/S4 minor). A decoder of the other kind that fills the group stays optional,
    /// and an emptied vae beside it is not added.
    #[test]
    fn a_slot_the_plan_needs_reads_required_when_the_persons_file_fills_it() {
        let file = |path: &str| SlotChoice::Local {
            path: path.into(),
            size: 1,
            sha256: None,
        };
        let p = plan_overriding(SlotId::Vae, file("/d/my ae.safetensors"));
        let vae = slot(&p, SlotId::Vae).unwrap();
        assert_eq!(vae.source, PartSource::Chosen);
        assert!(vae.required, "{vae:?}");
        assert!(p.parts.iter().all(|x| x.required) && p.can_run());

        let p = plan_given(
            &main_hub("o/r", "flux1-kontext-dev-Q4_K_M.gguf", "flux1-kontext"),
            "flux1-kontext",
            With {
                overrides: vec![
                    (SlotId::Vae, SlotChoice::Empty),
                    (SlotId::Tae, file("/d/taef1.safetensors")),
                ],
                ..With::default()
            },
        );
        assert!(slot(&p, SlotId::Vae).is_none());
        assert!(!slot(&p, SlotId::Tae).unwrap().required && p.can_run());
    }

    /// A recipe the person chose that the main file's version does not run (an untested
    /// set-up) takes its own version's companions: a FLUX.1 file set up as Z-Image gets
    /// Z-Image's Qwen3 encoder, and its decoder (one both versions use) from the table.
    #[test]
    fn a_recipe_of_another_version_takes_its_own_versions_companions() {
        let main = main_hub("o/r", "flux1-kontext-dev-Q4_K_M.gguf", "flux1-kontext");
        let companions = table();
        let p = plan_setup(&SetupInput {
            main: MainFile::Hub {
                repo: &main.repo,
                revision: REV,
                file: &main.file,
            },
            facts: &main.facts,
            recipe: recipe("z-image").unwrap(),
            version: version_of(&main.facts),
            part: None,
            listing: &[],
            listing_facts: &BTreeMap::new(),
            installed: &[],
            companions: &companions,
            overrides: &[],
            untested: true,
        });
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Main),
                (SlotId::Llm, PartSource::Companion),
                (SlotId::Vae, PartSource::Companion)
            ]
        );
        assert_eq!(
            hub_path(&p, SlotId::Llm),
            companion("qwen3-4b-2507-q8").path
        );
        assert_eq!(hub_path(&p, SlotId::Vae), companion("ae-flux1").path);
        assert!(p.can_run() && p.untested);

        // A file of no known version, set up as SDXL: the table has no SDXL decoder, so the
        // vae it needs comes from the person.
        let unknown = FileFacts {
            kind: FileKind::CannotRun {
                reason: crate::sd::CannotRunReason::UnknownWeights,
                version: None,
            },
            facts: MatchFacts::default(),
            parameters: 0,
            format: FileFormat::Safetensors,
        };
        let file = rf("mystery.safetensors", 6_000_000_000);
        let p = plan_setup(&SetupInput {
            main: MainFile::Hub {
                repo: "o/r",
                revision: REV,
                file: &file,
            },
            facts: &unknown,
            recipe: recipe("sdxl-checkpoint").unwrap(),
            version: None,
            part: None,
            listing: &[],
            listing_facts: &BTreeMap::new(),
            installed: &[],
            companions: &companions,
            overrides: &[],
            untested: true,
        });
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::Vae, PartSource::Missing)
            ]
        );
    }

    /// Ruling S4-main-override: a choice that replaces the main file (in its slot, or the
    /// slot it stands in for) plans the other slots again as for a main file that carries
    /// nothing: no part stays `Embedded` or `Folder`, and what that main would need is
    /// required.
    #[test]
    fn replacing_the_main_file_plans_the_other_slots_as_for_one_that_carries_nothing() {
        let local = |path: &str| SlotChoice::Local {
            path: path.into(),
            size: 1,
            sha256: None,
        };
        let overriding = |main: &Main, recipe_id: &str, slot: SlotId| {
            plan_given(
                main,
                recipe_id,
                With {
                    overrides: vec![(slot, local("/d/unet-Q8_0.gguf"))],
                    ..With::default()
                },
            )
        };
        let embedded = |p: &SetupPlan| {
            p.parts
                .iter()
                .filter(|x| matches!(x.source, PartSource::Embedded | PartSource::Folder))
                .count()
        };

        // A full SDXL checkpoint, its `model` replaced by a diffusion model alone: the empty
        // table has no SDXL VAE or encoders.
        let sdxl = main_hub(
            "stabilityai/stable-diffusion-xl-base-1.0",
            "sd_xl_base_1.0.safetensors",
            "sdxl",
        );
        assert_eq!(
            sources(&plan_given(&sdxl, "sdxl-checkpoint", With::default())),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::Vae, PartSource::Embedded)
            ]
        );
        let p = overriding(&sdxl, "sdxl-checkpoint", SlotId::Model);
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Chosen),
                (SlotId::Vae, PartSource::Missing),
                (SlotId::ClipL, PartSource::Missing),
                (SlotId::ClipG, PartSource::Missing)
            ]
        );
        assert!(p.parts.iter().all(|x| x.required) && !p.can_run());

        // A FLUX checkpoint with its encoders and VAE, replaced in the recipe's own
        // `diffusion_model`: they come from the table.
        let flux = Main {
            repo: "o/flux1-dev-checkpoint".into(),
            file: rf("flux1-dev-fp8-full.safetensors", 17_200_000_000),
            facts: FileFacts {
                kind: FileKind::Model {
                    version: SdVersion::Flux,
                    layout: Layout::Checkpoint,
                    embeds: vec![SlotId::ClipL, SlotId::T5xxl, SlotId::Vae],
                },
                facts: MatchFacts::default(),
                parameters: 17_000_000_000,
                format: FileFormat::Safetensors,
            },
            part: None,
        };
        assert_eq!(
            sources(&plan_given(&flux, "flux1-dev", With::default())),
            [
                (SlotId::Model, PartSource::Main),
                (SlotId::ClipL, PartSource::Embedded),
                (SlotId::T5xxl, PartSource::Embedded),
                (SlotId::Vae, PartSource::Embedded)
            ]
        );
        let p = overriding(&flux, "flux1-dev", SlotId::DiffusionModel);
        assert_eq!(
            sources(&p),
            [
                (SlotId::DiffusionModel, PartSource::Chosen),
                (SlotId::ClipL, PartSource::Companion),
                (SlotId::T5xxl, PartSource::Companion),
                (SlotId::Vae, PartSource::Companion)
            ]
        );
        assert_eq!(embedded(&p), 0);
        assert!(p.can_run());

        // A folder's own files go with it.
        let p = plan_folder_overriding(
            "CompVis/stable-diffusion-v1-4",
            &sd14_folder(),
            "sd1-checkpoint",
            &[(SlotId::Model, local("/d/sd14-unet.gguf"))],
        );
        assert_eq!(
            sources(&p),
            [
                (SlotId::Model, PartSource::Chosen),
                (SlotId::Vae, PartSource::Missing),
                (SlotId::ClipL, PartSource::Missing)
            ]
        );

        // A choice for another slot keeps the main file's own parts.
        let p = overriding(&flux, "flux1-dev", SlotId::Tae);
        assert_eq!(embedded(&p), 3);
    }

    #[test]
    fn a_local_main_file_plans_from_the_table_and_is_not_fetched() {
        let facts = inspect(&fixture("flux1-kontext"));
        let p = plan_setup(&SetupInput {
            main: MainFile::Local {
                path: Path::new("/home/ü/flux1-kontext-dev-Q4_K_M.gguf"),
                size: 6_930_000_000,
                sha256: None,
            },
            facts: &facts,
            recipe: recipe("flux1-kontext").unwrap(),
            version: version_of(&facts),
            part: None,
            listing: &[],
            listing_facts: &BTreeMap::new(),
            installed: &[],
            companions: &table(),
            overrides: &[],
            untested: true,
        });
        assert_eq!(
            slot(&p, SlotId::DiffusionModel).unwrap().file,
            Some(PlannedFile::Local {
                path: "/home/ü/flux1-kontext-dev-Q4_K_M.gguf".into(),
                size: 6_930_000_000,
                sha256: None,
            })
        );
        assert_eq!(p.to_fetch().len(), 3);
        assert!(p.untested && p.can_run());
    }

    #[test]
    fn part_sources_serialize_for_the_webview() {
        assert_eq!(
            serde_json::to_value([PartSource::Main, PartSource::Embedded, PartSource::Missing])
                .unwrap(),
            serde_json::json!(["main", "embedded", "missing"])
        );
    }
}
