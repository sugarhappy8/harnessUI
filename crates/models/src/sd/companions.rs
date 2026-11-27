//! The companion table (design section 7.4): shared extra files (text encoders, decoders,
//! CLIP vision) that a set-up takes when a model's own repo does not carry one, each pinned
//! by repo, revision, path, size and SHA-256, with its licence and the versions it suits,
//! and the default per version and per recipe. The catalog's `companions` section, which
//! [`Catalog`] keeps as raw JSON.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde::de::IntoDeserializer;
use serde_json::Value;

use crate::catalog::{Catalog, is_hex};
use crate::gen_catalog::{ALLOWED_PART_EXTENSIONS, Commercial, GenLicence, check_licences, keyed};
use crate::hf::{validate_repo_id, validate_repo_path};
use crate::sd::detect::SdVersion;
use crate::sd::matching::{out_of_scope, recipes_for};
use crate::sd::recipes::{SlotId, recipe};

/// One shared extra file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Companion {
    pub id: String,
    /// The one slot it fills.
    pub slot: SlotId,
    /// The versions it suits: a companion fits a version only through this list.
    pub versions: Vec<SdVersion>,
    /// `owner/name` on Hugging Face.
    pub repo: String,
    /// 40-hex commit.
    pub revision: String,
    /// Path inside the repository.
    pub path: String,
    pub size: u64,
    /// Hugging Face's LFS SHA-256, lowercase hex.
    pub sha256: String,
    /// A key of the table's `licenses`.
    pub license: String,
}

/// A slot of a recipe the table gives no default for on purpose, and why: a file the person
/// has to choose, or one Studio cannot make run. The guard test
/// (`every_recipe_studio_runs_is_covered_or_says_why`) fails for any other recipe whose
/// required slot has no default, so a gap is never shipped by accident; and a set-up plan
/// says the reason beside the slot, in place of a promise that Studio adds the file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotCovered {
    pub recipe: String,
    pub slot: SlotId,
    /// One plain sentence for the person, in the voice of `src/lib/words.ts`.
    pub reason: String,
}

/// The catalog's `companions` section, checked ([`CompanionTable::from_catalog`]).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompanionTable {
    /// Every licence a file names, by id.
    pub licenses: BTreeMap<String, GenLicence>,
    pub files: Vec<Companion>,
    /// The default companion id per slot, for each version.
    pub by_version: BTreeMap<SdVersion, BTreeMap<SlotId, String>>,
    /// Defaults for one recipe, which win over its version's.
    pub by_recipe: BTreeMap<String, BTreeMap<SlotId, String>>,
    /// The slots no default is given for, on purpose ([`NotCovered`]).
    pub not_covered: Vec<NotCovered>,
    /// What [`CompanionTable::from_catalog`] left out, each also logged as a warning.
    #[serde(skip)]
    pub skipped: Vec<Skipped>,
}

/// Something a newer signed catalog may add that this build leaves out, where it was and
/// why. The bundled catalog skips nothing (`the_bundled_table_skips_nothing`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// `licence <id>`, `file <id>`, `file <id> version <word>`, `byVersion <version>`,
    /// `byVersion <version> <slot>`, `byRecipe <recipe>` or `byRecipe <recipe> <slot>`, in the
    /// catalog's own words.
    pub place: String,
    pub reason: String,
}

/// A file as the catalog writes it: its version and slot words are read as strings, so that
/// a word a newer catalog adds is skipped rather than failing the table.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompanionRaw {
    id: String,
    slot: String,
    versions: Vec<String>,
    repo: String,
    revision: String,
    path: String,
    size: u64,
    sha256: String,
    license: String,
}

/// The section as the catalog writes it, every word a string and each licence raw.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TableRaw {
    licenses: BTreeMap<String, Value>,
    files: Vec<CompanionRaw>,
    by_version: BTreeMap<String, BTreeMap<String, String>>,
    by_recipe: BTreeMap<String, BTreeMap<String, String>>,
    not_covered: Vec<NotCoveredRaw>,
}

/// A [`NotCovered`] as the catalog writes it: the slot word is a string, so that one a
/// newer catalog adds is skipped.
#[derive(Deserialize)]
struct NotCoveredRaw {
    recipe: String,
    slot: String,
    reason: String,
}

/// The value of a unit enum a word names, by the enum's own serde names; `None` for a word
/// this build does not know.
fn known<'a, T: Deserialize<'a>>(word: &'a str) -> Option<T> {
    let word: serde::de::value::StrDeserializer<'a, serde::de::value::Error> =
        word.into_deserializer();
    T::deserialize(word).ok()
}

/// The slots that hold a model's own weights, which no companion fills.
fn is_model_slot(slot: SlotId) -> bool {
    matches!(
        slot,
        SlotId::Model
            | SlotId::DiffusionModel
            | SlotId::HighNoiseDiffusionModel
            | SlotId::UncondDiffusionModel
    )
}

/// What reading leaves out, as it goes.
#[derive(Default)]
struct Skips {
    list: Vec<Skipped>,
    licences: BTreeSet<String>,
    files: BTreeSet<String>,
}

impl Skips {
    fn skip(&mut self, place: String, reason: String) {
        self.list.push(Skipped { place, reason });
    }

    /// The defaults of one row whose slot this build knows and whose file it kept.
    fn defaults(
        &mut self,
        place: &str,
        slots: BTreeMap<String, String>,
    ) -> BTreeMap<SlotId, String> {
        let mut out = BTreeMap::new();
        for (word, id) in slots {
            let at = format!("{place} {word}");
            match known::<SlotId>(&word) {
                None => self.skip(at, "a slot this build does not know".into()),
                Some(_) if self.files.contains(&id) => {
                    self.skip(at, format!("names the skipped file {id}"))
                }
                Some(slot) => {
                    out.insert(slot, id);
                }
            }
        }
        out
    }
}

impl CompanionTable {
    /// Reads and checks the catalog's companion table; a catalog without one gives an
    /// empty table.
    ///
    /// What a newer signed catalog may add that this build cannot use is skipped, listed in
    /// `skipped` and logged, so that it never takes the other companions away:
    /// - a licence whose `commercialOutputs` word this build does not know, with the files
    ///   under it;
    /// - an unknown version word in a file's `versions`; a file left with no known version,
    ///   or with an unknown slot word or an adapter slot;
    /// - a `byVersion` row of an unknown version, a `byRecipe` row of an unknown recipe or
    ///   of one Studio leaves out, and a default of an unknown slot or naming a skipped file;
    /// - a `notCovered` entry of an unknown slot or recipe, or of one Studio leaves out.
    ///
    /// Refused, naming what is wrong, when:
    /// - a licence does not read otherwise, or one with bundled text does not pin it;
    /// - a repo id or path is not valid, a revision is not 40 hex, a SHA-256 not 64 hex, a
    ///   size 0, an extension not in [`ALLOWED_PART_EXTENSIONS`], or an id listed twice;
    /// - a file fills a model slot, or names a licence that is not listed;
    /// - a default names no companion, or one of another slot, or one whose `versions`
    ///   leave out the version (for `byRecipe`, every version whose recipes include it);
    /// - a `notCovered` entry with no reason, or listed twice.
    pub fn from_catalog(catalog: &Catalog) -> Result<CompanionTable, String> {
        let raw = Value::Object(catalog.companions.clone().into_iter().collect());
        let raw: TableRaw = serde_json::from_value(raw)
            .map_err(|e| format!("the companion table does not read: {e}"))?;
        let mut skips = Skips::default();

        let mut licenses = BTreeMap::new();
        for (id, raw) in raw.licenses {
            let commercial = raw.get("commercialOutputs").and_then(Value::as_str);
            if let Some(word) = commercial.filter(|w| known::<Commercial>(w).is_none()) {
                skips.skip(
                    format!("licence {id}"),
                    format!("commercial use this build does not know: {word}"),
                );
                skips.licences.insert(id);
                continue;
            }
            let licence: GenLicence = serde_json::from_value(raw)
                .map_err(|e| format!("licence {id} does not read: {e}"))?;
            licenses.insert(id, licence);
        }

        let mut ids = BTreeSet::new();
        let mut files = Vec::new();
        for f in raw.files {
            if !ids.insert(f.id.clone()) {
                return Err(format!("companion {}: listed twice", f.id));
            }
            let place = format!("file {}", f.id);
            let mut versions = Vec::new();
            for word in &f.versions {
                match SdVersion::from_word(word) {
                    Some(v) => versions.push(v),
                    None => skips.skip(
                        format!("{place} version {word}"),
                        "a version this build does not know".into(),
                    ),
                }
            }
            let reason = match known::<SlotId>(&f.slot) {
                None => format!("a slot this build does not know: {}", f.slot),
                Some(slot) if slot.is_adapter() => format!("an adapter slot: {}", f.slot),
                Some(_) if versions.is_empty() => "no version this build knows".into(),
                Some(_) if skips.licences.contains(&f.license) => {
                    format!("its licence {} is skipped", f.license)
                }
                Some(slot) => {
                    files.push(Companion {
                        id: f.id,
                        slot,
                        versions,
                        repo: f.repo,
                        revision: f.revision,
                        path: f.path,
                        size: f.size,
                        sha256: f.sha256,
                        license: f.license,
                    });
                    continue;
                }
            };
            skips.skip(place, reason);
            skips.files.insert(f.id);
        }

        let mut by_version = BTreeMap::new();
        for (word, slots) in raw.by_version {
            let place = format!("byVersion {word}");
            match SdVersion::from_word(&word) {
                Some(v) => {
                    by_version.insert(v, skips.defaults(&place, slots));
                }
                None => skips.skip(place, "a version this build does not know".into()),
            }
        }
        let mut by_recipe = BTreeMap::new();
        for (id, slots) in raw.by_recipe {
            let place = format!("byRecipe {id}");
            if recipe(&id).is_none() {
                skips.skip(place, "a recipe this build does not know".into());
            } else if out_of_scope(&id).is_some() {
                skips.skip(place, "a recipe Studio leaves out".into());
            } else {
                by_recipe.insert(id, skips.defaults(&place, slots));
            }
        }

        let mut not_covered = Vec::new();
        for n in raw.not_covered {
            let place = format!("notCovered {} {}", n.recipe, n.slot);
            match known::<SlotId>(&n.slot) {
                None => skips.skip(place, "a slot this build does not know".into()),
                Some(_) if recipe(&n.recipe).is_none() => {
                    skips.skip(place, "a recipe this build does not know".into())
                }
                Some(_) if out_of_scope(&n.recipe).is_some() => {
                    skips.skip(place, "a recipe Studio leaves out".into())
                }
                Some(slot) => not_covered.push(NotCovered {
                    recipe: n.recipe,
                    slot,
                    reason: n.reason,
                }),
            }
        }

        for s in &skips.list {
            tracing::warn!(
                place = %s.place,
                reason = %s.reason,
                "a companion entry is skipped"
            );
        }
        let table = CompanionTable {
            licenses: keyed(licenses),
            files,
            by_version,
            by_recipe,
            not_covered,
            skipped: skips.list,
        };
        table.check()?;
        Ok(table)
    }

    /// The companion a recipe of `version` takes in `slot` by default: the recipe's own
    /// default wins over the version's. Only a companion whose `versions` include
    /// `version` is ever given.
    pub fn default_for(
        &self,
        recipe_id: &str,
        version: SdVersion,
        slot: SlotId,
    ) -> Option<&Companion> {
        let by_recipe = self.by_recipe.get(recipe_id).and_then(|s| s.get(&slot));
        let by_version = self.by_version.get(&version).and_then(|s| s.get(&slot));
        by_recipe
            .into_iter()
            .chain(by_version)
            .filter_map(|id| self.file(id))
            .find(|c| c.versions.contains(&version))
    }

    /// Why the table gives a recipe no default for `slot` ([`NotCovered`]), if it is one of
    /// the slots it leaves out on purpose.
    pub fn not_covered_reason(&self, recipe_id: &str, slot: SlotId) -> Option<&str> {
        self.not_covered
            .iter()
            .find(|n| n.recipe == recipe_id && n.slot == slot)
            .map(|n| n.reason.as_str())
    }

    /// The companion with this SHA-256, whatever repo it was installed from.
    pub fn by_sha(&self, sha256: &str) -> Option<&Companion> {
        self.files
            .iter()
            .find(|c| c.sha256.eq_ignore_ascii_case(sha256))
    }

    pub fn licence(&self, id: &str) -> Option<&GenLicence> {
        self.licenses.get(id)
    }

    fn file(&self, id: &str) -> Option<&Companion> {
        self.files.iter().find(|c| c.id == id)
    }

    fn check(&self) -> Result<(), String> {
        check_licences(&self.licenses).map_err(|e| format!("companion licences: {e}"))?;
        for l in self.licenses.values() {
            if !is_hex(&l.sha256, 64) {
                return Err(format!("licence {}: the SHA-256 is not 64 hex", l.id));
            }
        }
        for c in &self.files {
            let id = &c.id;
            let path = c.path.to_ascii_lowercase();
            let fault = if validate_repo_id(&c.repo).is_err() {
                "not a valid repo id"
            } else if validate_repo_path(&c.path).is_err() {
                "not a valid path"
            } else if !ALLOWED_PART_EXTENSIONS
                .iter()
                .any(|ext| path.ends_with(ext))
            {
                "not a .safetensors, .sft or .gguf file"
            } else if !is_hex(&c.revision, 40) {
                "the revision is not 40 hex"
            } else if !is_hex(&c.sha256, 64) {
                "the SHA-256 is not 64 hex"
            } else if c.size == 0 {
                "no size"
            } else if !self.licenses.contains_key(&c.license) {
                "its licence is not listed"
            } else if is_model_slot(c.slot) {
                "fills a model slot"
            } else {
                continue;
            };
            return Err(format!("companion {id}: {fault}"));
        }
        let mut seen = BTreeSet::new();
        for n in &self.not_covered {
            let place = format!("notCovered {} {}", n.recipe, n.slot.word());
            if n.reason.trim().is_empty() {
                return Err(format!("{place}: no reason"));
            }
            if !seen.insert((n.recipe.as_str(), n.slot)) {
                return Err(format!("{place}: listed twice"));
            }
        }
        for (version, slots) in &self.by_version {
            let place = format!("byVersion {}", version.word());
            self.check_defaults(&place, slots, &[*version])?;
        }
        for (recipe_id, slots) in &self.by_recipe {
            let place = format!("byRecipe {recipe_id}");
            // Never empty: reading keeps only recipes Studio runs, and Task R7's table
            // reaches every one of them.
            let versions: Vec<SdVersion> = SdVersion::ALL
                .into_iter()
                .filter(|v| recipes_for(*v).contains(&recipe_id.as_str()))
                .collect();
            self.check_defaults(&place, slots, &versions)?;
        }
        Ok(())
    }

    /// Each default names a companion of its slot that suits every one of `versions`.
    fn check_defaults(
        &self,
        place: &str,
        slots: &BTreeMap<SlotId, String>,
        versions: &[SdVersion],
    ) -> Result<(), String> {
        for (slot, id) in slots {
            let slot_word = slot.word();
            let Some(c) = self.file(id) else {
                return Err(format!("{place} {slot_word}: {id} is not a companion"));
            };
            if c.slot != *slot {
                return Err(format!("{place} {slot_word}: {id} fills {}", c.slot.word()));
            }
            if let Some(v) = versions.iter().find(|v| !c.versions.contains(v)) {
                return Err(format!(
                    "{place} {slot_word}: {id} does not suit {}",
                    v.word()
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_catalog::{BUNDLED_LICENCE_TEXTS, Badge, gen_catalog, sha256_hex};
    use serde_json::json;

    fn bundled() -> CompanionTable {
        CompanionTable::from_catalog(&Catalog::bundled()).unwrap()
    }

    /// The bundled table with one change to its JSON.
    fn changed(edit: impl FnOnce(&mut Value)) -> Result<CompanionTable, String> {
        let mut c = Catalog::bundled();
        let mut v = Value::Object(c.companions.into_iter().collect());
        edit(&mut v);
        c.companions = v.as_object().unwrap().clone().into_iter().collect();
        CompanionTable::from_catalog(&c)
    }

    /// Why the bundled table, with one change to its JSON, is refused.
    fn refused(edit: impl FnOnce(&mut Value)) -> String {
        changed(edit).unwrap_err()
    }

    /// The index in `files` of the companion `id`.
    fn at(v: &Value, id: &str) -> usize {
        v["files"]
            .as_array()
            .unwrap()
            .iter()
            .position(|f| f["id"] == id)
            .unwrap()
    }

    /// How many entries of the bundled catalog's `companions` section `key` holds, as the
    /// catalog writes them: what a table that skips nothing keeps.
    fn raw_len(key: &str) -> usize {
        let c = Catalog::bundled();
        match &c.companions[key] {
            Value::Array(a) => a.len(),
            Value::Object(o) => o.len(),
            other => panic!("companions.{key} is {other}"),
        }
    }

    fn id(c: Option<&Companion>) -> Option<&str> {
        c.map(|c| c.id.as_str())
    }

    /// Where each skipped item was, in order.
    fn places(t: &CompanionTable) -> Vec<&str> {
        t.skipped.iter().map(|s| s.place.as_str()).collect()
    }

    /// The table without its list of what it skipped.
    fn kept(t: &CompanionTable) -> CompanionTable {
        CompanionTable {
            skipped: Vec::new(),
            ..t.clone()
        }
    }

    /// Decoders that look alike are not interchangeable (design 7.4: a file suits a version
    /// only through its `versions`): PixArt alpha takes the SD 1.x-style VAE and Sigma the SDXL
    /// one (a different scale), SDXS only its own tiny decoder, and SD3 its own 16-channel VAE
    /// (never FLUX's, which is also 16-channel with different weights).
    #[test]
    fn look_alike_decoders_stay_with_their_models() {
        let t = bundled();
        use SdVersion::*;
        for (recipe, version, slot, want) in [
            ("pixart-alpha-512", Pixart, SlotId::Vae, "sd-vae-ft-ema"),
            ("pixart-alpha-1024", Pixart, SlotId::Vae, "sd-vae-ft-ema"),
            ("pixart-sigma", Pixart, SlotId::Vae, "sdxl-vae-fp16-fix"),
            ("pixart-sigma", Pixart, SlotId::T5xxl, "t5xxl-fp8"),
            ("sdxs", Sdxs512Ds, SlotId::Vae, "sdxs-vae"),
            ("sdxs", Sdxs09, SlotId::Vae, "sdxs-vae"),
            ("sd1-checkpoint", Sd1, SlotId::Vae, "sd-vae-ft-mse"),
            ("sdxl-checkpoint", Sdxl, SlotId::Vae, "sdxl-vae-fp16-fix"),
            ("sdxl-checkpoint", Sdxl, SlotId::ClipG, "clip-g-sdxl"),
            ("sd35-medium", Sd3, SlotId::Vae, "sd3-vae"),
            ("minit2i", Minit2i, SlotId::T5xxl, "flan-t5-large"),
        ] {
            assert_eq!(
                id(t.default_for(recipe, version, slot)),
                Some(want),
                "{recipe} {slot:?}"
            );
        }
        let by = |id: &str| t.files.iter().find(|c| c.id == id).unwrap();
        assert!(!by("sd-vae-ft-mse").versions.contains(&Sdxs512Ds));
        assert!(!by("sd3-vae").versions.contains(&Flux));
        assert!(!by("ae-flux1").versions.contains(&Sd3));
        // SDXL's own CLIP-G is the same bytes as SD3's, under its own licence.
        assert_eq!(by("clip-g-sdxl").sha256, by("clip-g").sha256);
        assert_eq!(by("clip-g-sdxl").license, "openrail++");
        assert_eq!(by("clip-g").license, "stabilityai-ai-community");
    }

    // ---- The guard: no recipe Studio runs may be a dead end ----------------------------------

    /// The recipe's slots that stay empty when its model is one lone file: no other file in
    /// its repository, nothing installed, nothing the file carries (a diffusion model alone,
    /// the worst case). Each is a place where Studio would show *Choose a file* and nothing
    /// else, and the model could not start. A high/low pair is given its sibling file, so the
    /// pair does not hide a gap in the companions.
    fn gaps(table: &CompanionTable) -> BTreeMap<(String, SlotId), Vec<SdVersion>> {
        use crate::files::RepoFile;
        use crate::sd::FileFacts;
        use crate::sd::index::FileFormat;
        use crate::sd::inspect::{FileKind, Layout, MatchFacts};
        use crate::sd::matching::{NoisePart, in_scope_recipes};
        use crate::sd::setup::{MainFile, PartSource, SetupInput, plan_setup};

        let file = |path: &str| RepoFile {
            path: path.into(),
            size: 1000,
            sha256: Some("a".repeat(64)),
        };
        let mut found: BTreeMap<(String, SlotId), Vec<SdVersion>> = BTreeMap::new();
        for id in in_scope_recipes() {
            let r = recipe(id).unwrap();
            let paired = r.slot(SlotId::HighNoiseDiffusionModel).is_some();
            let (main, listing, part) = if paired {
                (
                    file("wan2.2_t2v_high_noise_14B_fp8.safetensors"),
                    vec![file("wan2.2_t2v_low_noise_14B_fp8.safetensors")],
                    Some(NoisePart::High),
                )
            } else {
                (file("model.safetensors"), Vec::new(), None)
            };
            for v in SdVersion::ALL
                .into_iter()
                .filter(|v| recipes_for(*v).contains(&id))
            {
                let facts = FileFacts {
                    kind: FileKind::Model {
                        version: v,
                        layout: Layout::DiffusionOnly,
                        embeds: Vec::new(),
                    },
                    facts: MatchFacts::default(),
                    parameters: 1_000_000_000,
                    format: FileFormat::Safetensors,
                };
                let plan = plan_setup(&SetupInput {
                    main: MainFile::Hub {
                        repo: "someone/model",
                        revision: "0123456789abcdef0123456789abcdef01234567",
                        file: &main,
                    },
                    facts: &facts,
                    recipe: r,
                    version: Some(v),
                    part,
                    listing: &listing,
                    listing_facts: &BTreeMap::new(),
                    installed: &[],
                    companions: table,
                    overrides: &[],
                    untested: false,
                });
                for p in plan
                    .parts
                    .iter()
                    .filter(|p| p.source == PartSource::Missing)
                {
                    let versions = found.entry((id.to_owned(), p.slot)).or_default();
                    if !versions.contains(&v) {
                        versions.push(v);
                    }
                }
            }
        }
        found
    }

    /// Every recipe Studio runs can start from its main file alone: each required slot, and
    /// the VAE or tiny decoder a required pair asks for, has a default in the table for every
    /// version that reaches the recipe (`byVersion`, or `byRecipe` for one recipe), unless the
    /// recipe and slot are listed in the catalog's `notCovered` with a reason. This is what
    /// stops a release from offering "Works in Studio" for a model that then asks the person
    /// for a file Studio could have named (design section 4: no dead end).
    #[test]
    fn every_recipe_studio_runs_is_covered_or_says_why() {
        let t = bundled();
        let mut dead_ends = Vec::new();
        for ((recipe_id, slot), versions) in gaps(&t) {
            if t.not_covered_reason(&recipe_id, slot).is_some() {
                continue;
            }
            let words: Vec<&str> = versions.iter().map(|v| v.word()).collect();
            dead_ends.push(format!(
                "{recipe_id} ({}) has no default for {}",
                words.join(", "),
                slot.word()
            ));
        }
        assert!(
            dead_ends.is_empty(),
            "{} slots would leave a model that cannot start, and no way to fix it in the app. \
             Add a verified companion to catalog/models.json (byVersion or byRecipe), or list the \
             recipe and slot under companions.notCovered with a plain reason:\n{}",
            dead_ends.len(),
            dead_ends.join("\n")
        );
    }

    /// The allowlist holds only slots that really have no default, each with its reason: an
    /// entry the table now covers is stale and must go, so the page never says Studio cannot
    /// find a file it can.
    #[test]
    fn the_allowlist_holds_only_real_gaps_with_reasons() {
        let t = bundled();
        let gaps = gaps(&t);
        for n in &t.not_covered {
            assert!(
                gaps.contains_key(&(n.recipe.clone(), n.slot)),
                "notCovered {} {}: the table gives it a default now, so remove the entry",
                n.recipe,
                n.slot.word()
            );
            assert!(
                n.reason.ends_with('.') && n.reason.len() < 200,
                "notCovered {} {}: the reason is one short sentence",
                n.recipe,
                n.slot.word()
            );
        }
    }

    /// The strict guard: a misspelt word in the catalog we ship is never skipped quietly.
    #[test]
    fn the_bundled_table_skips_nothing() {
        assert_eq!(bundled().skipped, []);
    }

    #[test]
    fn the_bundled_table_is_valid_and_complete() {
        let t = CompanionTable::from_catalog(&Catalog::bundled()).unwrap();
        assert_eq!(t.files.len(), raw_len("files"));
        assert_eq!(
            t.default_for("flux1-dev", SdVersion::Flux, SlotId::T5xxl)
                .unwrap()
                .id,
            "t5xxl-fp8"
        );
        assert_eq!(
            t.default_for("flux2-klein-9b", SdVersion::Flux2Klein, SlotId::Llm)
                .unwrap()
                .id,
            "qwen3-8b-q8"
        );
        assert_eq!(
            t.default_for("flux2-klein-4b", SdVersion::Flux2Klein, SlotId::Llm)
                .unwrap()
                .id,
            "qwen3-4b-q8"
        );
        assert_eq!(
            t.default_for("wan22-ti2v-5b", SdVersion::Wan22Ti2v, SlotId::Vae)
                .unwrap()
                .sha256,
            "e40321bd36b9709991dae2530eb4ac303dd168276980d3e9bc4b6e2b75fed156"
        );
        assert_eq!(
            t.licence(
                &t.default_for("sd35-medium", SdVersion::Sd3, SlotId::ClipG)
                    .unwrap()
                    .license
            )
            .unwrap()
            .revenue_limit_usd,
            Some(1_000_000)
        );
        assert!(
            t.default_for("hunyuan-video-15-t2v", SdVersion::HunyuanVideo, SlotId::Vae)
                .is_none()
        );
    }

    #[test]
    fn a_default_for_the_wrong_slot_or_version_is_refused() {
        let mut c = Catalog::bundled();
        c.companions.get_mut("byVersion").unwrap()["sd3"]["t5xxl"] = json!("umt5-xxl-q8");
        assert!(
            CompanionTable::from_catalog(&c)
                .unwrap_err()
                .contains("umt5-xxl-q8")
        );
    }

    #[test]
    fn the_01x_catalog_parts_are_known_companions_by_sha() {
        // so a generic model reuses what a 0.1.x model installed, even from another repo
        // (Z-Image's ae.safetensors, FastWan's VAE)
        let t = CompanionTable::from_catalog(&Catalog::bundled()).unwrap();
        for (sha, id) in [
            (
                "f6f851777709861056efcdad3af01da38b31223a3ba26e61a4f8bf3a2195813a",
                "qwen3-4b-q4km",
            ),
            (
                "868fe7b343cc8f3a19dbcfcafbc3d5f888802be3f89bd81b65b3621a066ce8f3",
                "flux2-vae",
            ),
            (
                "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
                "qwen3-4b-2507-q4km",
            ),
            (
                "afc8e28272cd15db3919bacdb6918ce9c1ed22e96cb12c4d5ed0fba823529e38",
                "ae-flux1",
            ),
            (
                "eaea358bb438c5d211721a4feecc162000e3636e9cb96f51e216f1f44ebd12ce",
                "umt5-xxl-q5km",
            ),
            (
                "e40321bd36b9709991dae2530eb4ac303dd168276980d3e9bc4b6e2b75fed156",
                "wan22-vae",
            ),
        ] {
            assert_eq!(t.by_sha(sha).map(|c| c.id.as_str()), Some(id));
        }
        for e in gen_catalog(&Catalog::bundled(), 1106).entries.values() {
            for p in &e.parts {
                if let Some(c) = t.by_sha(&p.sha256) {
                    assert_eq!(c.size, p.size, "{}", c.id);
                }
            }
        }
    }

    #[test]
    fn the_licences_pin_their_texts() {
        let t = bundled();
        let apache = t.licence("apache-2.0").unwrap();
        assert_eq!(apache.id, "apache-2.0");
        assert_eq!(
            apache.sha256,
            sha256_hex(BUNDLED_LICENCE_TEXTS[0].1.as_bytes())
        );
        assert_eq!(apache.badge(), Badge::Green);
        // No bundled text: the 0.1.x rule, the SHA-256 of `<repo>@<commit>:<id>`.
        let sai = t.licence("stabilityai-ai-community").unwrap();
        assert_eq!(
            sai.url,
            "https://huggingface.co/Comfy-Org/stable-diffusion-3.5/blob/934251d1e6168bcb45a069abc5bd5cdd5f8fea00/README.md"
        );
        assert_eq!(
            sai.sha256,
            sha256_hex(
                b"Comfy-Org/stable-diffusion-3.5@934251d1e6168bcb45a069abc5bd5cdd5f8fea00:stabilityai-ai-community"
            )
        );
        assert_eq!(
            (sai.commercial_outputs, sai.revenue_limit_usd, sai.badge()),
            (Commercial::UnderRevenue, Some(1_000_000), Badge::Amber)
        );
        let under_sai: Vec<&str> = t
            .files
            .iter()
            .filter(|c| c.license == sai.id)
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(under_sai, ["clip-g", "sd3-vae"]);
        assert!(t.licence("unlisted-licence").is_none());

        let err = refused(|v| v["licenses"]["apache-2.0"]["sha256"] = json!("0".repeat(64)));
        assert!(err.contains("licence text does not match"), "{err}");
        let err = refused(|v| v["licenses"]["stabilityai-ai-community"]["sha256"] = json!("x"));
        assert!(
            err.contains("licence stabilityai-ai-community: the SHA-256 is not 64 hex"),
            "{err}"
        );
        // A licence that does not read for any other reason than a newer word.
        for (key, value) in [("name", json!(5)), ("commercialOutputs", json!(5))] {
            let err = refused(|v| v["licenses"]["apache-2.0"][key] = value);
            assert!(err.contains("licence apache-2.0 does not read"), "{err}");
        }
    }

    /// The licence asks for registration with Stability AI before commercial use
    /// (`https://stability.ai/community-license-agreement`: "If You are using or distributing
    /// the Stability AI Materials for a Commercial Purpose, You must register with Stability
    /// AI."). Studio's amber sentence lists each obligation after the revenue limit
    /// (`LICENCE_WORDS.amber`, `src/lib/words.ts`): "… The licence asks you to register with
    /// Stability AI before any commercial use."
    #[test]
    fn the_sd35_licence_asks_for_registration_before_commercial_use() {
        let t = bundled();
        let sai = t.licence("stabilityai-ai-community").unwrap();
        assert_eq!(sai.badge(), Badge::Amber);
        assert_eq!(
            sai.obligations,
            ["Register with Stability AI before any commercial use."]
        );
        // Written as the 0.1.x licences' obligations are: one sentence, a capital to a stop.
        let apache = t.licence("apache-2.0").unwrap();
        assert_eq!(
            apache.obligations,
            ["Keep the licence and copyright notices with the files."]
        );
    }

    #[test]
    fn a_file_that_is_not_pinned_or_allowed_is_refused() {
        // files[0] is clip-l.
        for (key, value, says) in [
            (
                "repo",
                json!("no-owner"),
                "companion clip-l: not a valid repo id",
            ),
            ("path", json!("../clip_l.safetensors"), "not a valid path"),
            (
                "path",
                json!("clip_l.ckpt"),
                "not a .safetensors, .sft or .gguf file",
            ),
            ("revision", json!("main"), "the revision is not 40 hex"),
            (
                "sha256",
                json!("AB".repeat(32)),
                "the SHA-256 is not 64 hex",
            ),
            ("size", json!(0), "no size"),
            (
                "license",
                json!("unlisted-licence"),
                "its licence is not listed",
            ),
            ("slot", json!("diffusion_model"), "fills a model slot"),
            (
                "id",
                json!("t5xxl-fp8"),
                "companion t5xxl-fp8: listed twice",
            ),
            ("size", json!("big"), "the companion table does not read"),
        ] {
            let err = refused(|v| v["files"][0][key] = value.clone());
            assert!(err.contains(says), "{key} {value}: {err}");
        }
    }

    #[test]
    fn a_default_must_name_a_companion_of_its_slot_that_suits_its_version() {
        for (edit, says) in [
            (
                ("byVersion", "flux", "vae", "no-such-file"),
                "byVersion flux vae: no-such-file is not a companion",
            ),
            (
                ("byVersion", "flux", "vae", "clip-l"),
                "byVersion flux vae: clip-l fills clip_l",
            ),
            // A VAE fits a version only through its `versions`, never by its slot.
            (
                ("byVersion", "flux", "vae", "wan21-vae"),
                "byVersion flux vae: wan21-vae does not suit flux",
            ),
            (
                ("byVersion", "qwen-image-2.1", "vae", "qwen-image-vae"),
                "byVersion qwen-image-2.1 vae: qwen-image-vae does not suit qwen-image-2.1",
            ),
            // A recipe's default suits the versions R7's table gives the recipe.
            (
                ("byRecipe", "flux2-klein-9b", "llm", "qwen3-4b-2507-q8"),
                "byRecipe flux2-klein-9b llm: qwen3-4b-2507-q8 does not suit flux2-klein",
            ),
        ] {
            let (section, key, slot, companion) = edit;
            let err = refused(|v| v[section][key][slot] = json!(companion));
            assert!(err.contains(says), "{err}");
        }
        // flux1-fill is reached from flux and flux-fill: its own default must suit both.
        let err = refused(|v| {
            let ae = at(v, "ae-flux1");
            v["files"][ae]["versions"]
                .as_array_mut()
                .unwrap()
                .retain(|w| w != "flux-fill");
            v["byVersion"]["flux-fill"]
                .as_object_mut()
                .unwrap()
                .remove("vae");
            v["byRecipe"]["flux1-fill"] = json!({ "vae": "ae-flux1" });
        });
        assert!(
            err.contains("byRecipe flux1-fill vae: ae-flux1 does not suit flux-fill"),
            "{err}"
        );
    }

    /// The engine runs Flux Fill and Flux Controls as Flux (`sd_version_is_flux`,
    /// `src/model.h:139-150`): the same encoders (`build_flux`,
    /// `src/model/integrations/flux.cpp:28-33`) and VAE (`vae_kind = ModelVAEKind::FLUX`,
    /// `flux.cpp:53`, `68`, `84`).
    #[test]
    fn native_flux_fill_and_controls_files_take_flux_companions() {
        let t = bundled();
        for (recipe, version) in [
            ("flux1-fill", SdVersion::FluxFill),
            ("flux1-canny", SdVersion::FluxControls),
            ("flux1-depth", SdVersion::FluxControls),
        ] {
            assert!(recipes_for(version).contains(&recipe), "{recipe}");
            for (slot, want) in [
                (SlotId::ClipL, "clip-l"),
                (SlotId::T5xxl, "t5xxl-fp8"),
                (SlotId::Vae, "ae-flux1"),
            ] {
                assert_eq!(
                    id(t.default_for(recipe, version, slot)),
                    Some(want),
                    "{recipe} {slot:?}"
                );
            }
        }
    }

    /// A newer signed catalog never takes the companions this build knows away.
    #[test]
    fn an_unknown_version_word_is_dropped_and_the_file_kept() {
        let t = changed(|v| {
            let clip = at(v, "clip-l");
            v["files"][clip]["versions"]
                .as_array_mut()
                .unwrap()
                .push(json!("flux9"));
        })
        .unwrap();
        assert_eq!(t.files.len(), raw_len("files"));
        let clip = t.files.iter().find(|c| c.id == "clip-l").unwrap();
        use SdVersion::*;
        assert_eq!(clip.versions, bundled().files[0].versions);
        assert_eq!(
            &clip.versions[..5],
            [Flux, FluxFill, FluxControls, Flex2, Sd3]
        );
        assert_eq!(
            id(t.default_for("flux1-dev", Flux, SlotId::ClipL)),
            Some("clip-l")
        );
        assert_eq!(
            t.skipped,
            [Skipped {
                place: "file clip-l version flux9".into(),
                reason: "a version this build does not know".into(),
            }]
        );
    }

    #[test]
    fn a_file_of_only_unknown_versions_or_an_unknown_slot_is_skipped() {
        let t = changed(|v| {
            let fp8 = at(v, "t5xxl-fp8");
            let mut new_version = v["files"][fp8].clone();
            new_version["id"] = json!("t5xxl-flux9");
            new_version["versions"] = json!(["flux9"]);
            let mut new_slot = v["files"][fp8].clone();
            new_slot["id"] = json!("encoder-9");
            new_slot["slot"] = json!("text_encoder_9");
            let files = v["files"].as_array_mut().unwrap();
            files.push(new_version);
            files.push(new_slot);
            // Defaults naming a skipped file are skipped too.
            v["byVersion"]["chroma"]["t5xxl"] = json!("t5xxl-flux9");
            v["byRecipe"]["flux1-dev"] = json!({ "t5xxl": "encoder-9" });
        })
        .unwrap();
        assert_eq!(t.files.len(), raw_len("files"));
        assert!(
            t.files
                .iter()
                .all(|c| c.id != "t5xxl-flux9" && c.id != "encoder-9")
        );
        assert_eq!(
            t.default_for("chroma", SdVersion::Chroma, SlotId::T5xxl),
            None
        );
        assert_eq!(
            id(t.default_for("flux1-dev", SdVersion::Flux, SlotId::T5xxl)),
            Some("t5xxl-fp8")
        );
        assert_eq!(
            places(&t),
            [
                "file t5xxl-flux9 version flux9",
                "file t5xxl-flux9",
                "file encoder-9",
                "byVersion chroma t5xxl",
                "byRecipe flux1-dev t5xxl",
            ]
        );
        assert_eq!(t.skipped[1].reason, "no version this build knows");
        assert_eq!(
            t.skipped[2].reason,
            "a slot this build does not know: text_encoder_9"
        );
        assert_eq!(t.skipped[3].reason, "names the skipped file t5xxl-flux9");
        // A default naming a file that was never listed is still refused.
        let err = refused(|v| v["byVersion"]["chroma"]["t5xxl"] = json!("never-listed"));
        assert!(
            err.contains("byVersion chroma t5xxl: never-listed is not a companion"),
            "{err}"
        );
    }

    #[test]
    fn a_default_keyed_by_an_unknown_word_is_skipped() {
        let t = changed(|v| {
            v["byVersion"]["flux9"] = json!({ "t5xxl": "t5xxl-fp8" });
            v["byVersion"]["flux"]["text_encoder_9"] = json!("t5xxl-fp8");
            v["byRecipe"]["flux9-dev"] = json!({ "t5xxl": "t5xxl-fp8" });
        })
        .unwrap();
        assert_eq!(
            (t.files.len(), t.by_version.len()),
            (raw_len("files"), raw_len("byVersion"))
        );
        assert_eq!(
            t.by_version[&SdVersion::Flux]
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [SlotId::ClipL, SlotId::T5xxl, SlotId::Vae]
        );
        assert!(!t.by_recipe.contains_key("flux9-dev"));
        assert_eq!(kept(&t), bundled());
        assert_eq!(
            places(&t),
            [
                "byVersion flux text_encoder_9",
                "byVersion flux9",
                "byRecipe flux9-dev"
            ]
        );
        assert_eq!(t.skipped[2].reason, "a recipe this build does not know");
    }

    /// What a later build may bring in scope, or a later licence word, is skipped with
    /// everything that depends on it; the rest of the table loads.
    #[test]
    fn later_scope_and_licence_words_are_skipped_with_what_depends_on_them() {
        let t = changed(|v| {
            // A file in an adapter slot, and a default for it.
            let clip = at(v, "clip-vision-h");
            let mut adapter = v["files"][clip].clone();
            adapter["id"] = json!("ip-adapter-x");
            adapter["slot"] = json!("ip_adapter");
            adapter["versions"] = json!(["sdxl"]);
            v["files"].as_array_mut().unwrap().push(adapter);
            v["byVersion"]["sdxl"] = json!({ "ip_adapter": "ip-adapter-x" });
            // A recipe Studio leaves out.
            v["byRecipe"]["sd1-controlnet"] = json!({ "vae": "ae-flux1" });
            // A licence word this build does not know: clip-g and sd3's clip_g default go.
            v["licenses"]["stabilityai-ai-community"]["commercialOutputs"] = json!("attribution");
        })
        .unwrap();
        // The files under the skipped licence go, and nothing else.
        let under_it = Catalog::bundled().companions["files"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["license"] == "stabilityai-ai-community")
            .count();
        assert_eq!(t.files.len(), raw_len("files") - under_it);
        assert!(t.licence("stabilityai-ai-community").is_none());
        assert!(t.files.iter().all(|c| c.id != "clip-g"));
        assert_eq!(
            t.default_for("sd35-medium", SdVersion::Sd3, SlotId::ClipG),
            None
        );
        assert_eq!(
            id(t.default_for("sd35-medium", SdVersion::Sd3, SlotId::ClipL)),
            Some("clip-l")
        );
        assert!(t.by_version[&SdVersion::Sdxl].is_empty());
        assert!(!t.by_recipe.contains_key("sd1-controlnet"));
        let skipped: Vec<(&str, &str)> = t
            .skipped
            .iter()
            .map(|s| (s.place.as_str(), s.reason.as_str()))
            .collect();
        assert_eq!(
            skipped,
            [
                (
                    "licence stabilityai-ai-community",
                    "commercial use this build does not know: attribution"
                ),
                (
                    "file clip-g",
                    "its licence stabilityai-ai-community is skipped"
                ),
                (
                    "file sd3-vae",
                    "its licence stabilityai-ai-community is skipped"
                ),
                ("file ip-adapter-x", "an adapter slot: ip_adapter"),
                ("byVersion sd3 clip_g", "names the skipped file clip-g"),
                ("byVersion sd3 vae", "names the skipped file sd3-vae"),
                (
                    "byVersion sdxl ip_adapter",
                    "names the skipped file ip-adapter-x"
                ),
                ("byRecipe sd1-controlnet", "a recipe Studio leaves out"),
            ]
        );
    }

    #[test]
    fn a_recipes_own_default_wins_and_only_where_it_suits_the_version() {
        let t = bundled();
        use SdVersion::{Flux, Flux2, Flux2Klein};
        assert_eq!(
            id(t.default_for("flux2-klein-base-9b", Flux2Klein, SlotId::Llm)),
            Some("qwen3-8b-q8")
        );
        assert_eq!(
            id(t.default_for("flux2-klein-base-4b", Flux2Klein, SlotId::Llm)),
            Some("qwen3-4b-q8")
        );
        assert_eq!(
            id(t.default_for("flux2-klein-9b", Flux2Klein, SlotId::Vae)),
            Some("flux2-vae")
        );
        // qwen3-8b-q8 does not suit flux2, so that version's own default is given.
        assert_eq!(
            id(t.default_for("flux2-klein-9b", Flux2, SlotId::Llm)),
            Some("mistral-small-3.2-24b-q4km")
        );
        assert_eq!(t.default_for("flux1-dev", Flux, SlotId::Llm), None);
        assert_eq!(
            id(t.default_for("flux1-kontext", Flux, SlotId::Vae)),
            Some("ae-flux1")
        );
    }

    #[test]
    fn by_sha_ignores_case_and_knows_only_companions() {
        let t = bundled();
        let sha = "660C6F5B1ABAE9DC498AC2D21E1347D2ABDB0CF6C0C0C8576CD796491D9A6CDD";
        assert_eq!(id(t.by_sha(sha)), Some("clip-l"));
        assert_eq!(t.by_sha(&"0".repeat(64)), None);
    }

    #[test]
    fn a_catalog_without_companions_has_an_empty_table() {
        let c = Catalog::parse(include_str!("../../tests/fixtures/catalog/models.json")).unwrap();
        assert!(c.companions.is_empty());
        let t = CompanionTable::from_catalog(&c).unwrap();
        assert_eq!(t, CompanionTable::default());
        assert_eq!(
            t.default_for("flux1-dev", SdVersion::Flux, SlotId::T5xxl),
            None
        );
    }
}
