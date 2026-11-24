//! The companion table (design section 7.4): shared extra files (text encoders, decoders,
//! CLIP vision) that a set-up takes when a model's own repo does not carry one, each pinned
//! by repo, revision, path, size and SHA-256, with its licence and the versions it suits,
//! and the default per version and per recipe. The catalog's `companions` section, which
//! [`Catalog`] keeps as raw JSON.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::catalog::{Catalog, is_hex};
use crate::gen_catalog::{ALLOWED_PART_EXTENSIONS, GenLicence, check_licences, keyed};
use crate::hf::{validate_repo_id, validate_repo_path};
use crate::sd::detect::SdVersion;
use crate::sd::matching::{in_scope_recipes, recipes_for};
use crate::sd::recipes::SlotId;

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
}

/// The slots a companion may not fill: the model's own weights, and the adapters that put
/// a recipe out of scope.
fn is_companion_slot(slot: SlotId) -> bool {
    !slot.is_adapter()
        && !matches!(
            slot,
            SlotId::Model
                | SlotId::DiffusionModel
                | SlotId::HighNoiseDiffusionModel
                | SlotId::UncondDiffusionModel
        )
}

impl CompanionTable {
    /// Reads and checks the catalog's companion table; a catalog without one gives an
    /// empty table. Refused, naming what is wrong, when:
    /// - a repo id or path is not valid, a revision is not 40 hex, a SHA-256 not 64 hex, a
    ///   size 0, an extension not in [`ALLOWED_PART_EXTENSIONS`], or an id listed twice;
    /// - a file fills a model or adapter slot;
    /// - a licence a file names is missing, or one with bundled text does not pin it;
    /// - a default names no companion, or one of another slot, or one whose `versions`
    ///   leave out the version (for `byRecipe`, every version whose recipes include it);
    /// - a `byRecipe` key is not a recipe Studio runs.
    pub fn from_catalog(catalog: &Catalog) -> Result<CompanionTable, String> {
        let raw = serde_json::Value::Object(catalog.companions.clone().into_iter().collect());
        let mut table: CompanionTable = serde_json::from_value(raw)
            .map_err(|e| format!("the companion table does not read: {e}"))?;
        table.licenses = keyed(table.licenses);
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
        let mut ids = BTreeSet::new();
        for c in &self.files {
            let id = &c.id;
            let path = c.path.to_ascii_lowercase();
            let fault = if !ids.insert(id.as_str()) {
                "listed twice"
            } else if validate_repo_id(&c.repo).is_err() {
                "not a valid repo id"
            } else if validate_repo_path(&c.path).is_err() {
                "not a valid path"
            } else if !ALLOWED_PART_EXTENSIONS
                .iter()
                .any(|ext| path.ends_with(ext))
            {
                "not a .gguf or .safetensors file"
            } else if !is_hex(&c.revision, 40) {
                "the revision is not 40 hex"
            } else if !is_hex(&c.sha256, 64) {
                "the SHA-256 is not 64 hex"
            } else if c.size == 0 {
                "no size"
            } else if !self.licenses.contains_key(&c.license) {
                "its licence is not listed"
            } else if !is_companion_slot(c.slot) {
                "fills a model or adapter slot"
            } else {
                continue;
            };
            return Err(format!("companion {id}: {fault}"));
        }
        for (version, slots) in &self.by_version {
            let place = format!("byVersion {}", version.word());
            self.check_defaults(&place, slots, &[*version])?;
        }
        let in_scope = in_scope_recipes();
        for (recipe_id, slots) in &self.by_recipe {
            let place = format!("byRecipe {recipe_id}");
            if !in_scope.contains(&recipe_id.as_str()) {
                return Err(format!("{place}: not a recipe Studio runs"));
            }
            // Never empty: Task R7's table reaches every recipe Studio runs.
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
    use crate::gen_catalog::{BUNDLED_LICENCE_TEXTS, Badge, Commercial, gen_catalog, sha256_hex};
    use serde_json::{Value, json};

    fn bundled() -> CompanionTable {
        CompanionTable::from_catalog(&Catalog::bundled()).unwrap()
    }

    /// Why the bundled table, with one change to its JSON, is refused.
    fn refused(edit: impl FnOnce(&mut Value)) -> String {
        let mut c = Catalog::bundled();
        let mut v = Value::Object(c.companions.into_iter().collect());
        edit(&mut v);
        c.companions = v.as_object().unwrap().clone().into_iter().collect();
        CompanionTable::from_catalog(&c).unwrap_err()
    }

    fn id(c: Option<&Companion>) -> Option<&str> {
        c.map(|c| c.id.as_str())
    }

    #[test]
    fn the_bundled_table_is_valid_and_complete() {
        let t = CompanionTable::from_catalog(&Catalog::bundled()).unwrap();
        assert_eq!(t.files.len(), 22);
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
        assert_eq!(under_sai, ["clip-g"]);
        assert!(t.licence("mit").is_none());

        let err = refused(|v| v["licenses"]["apache-2.0"]["sha256"] = json!("0".repeat(64)));
        assert!(err.contains("licence text does not match"), "{err}");
        let err = refused(|v| v["licenses"]["stabilityai-ai-community"]["sha256"] = json!("x"));
        assert!(
            err.contains("licence stabilityai-ai-community: the SHA-256 is not 64 hex"),
            "{err}"
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
                "not a .gguf or .safetensors file",
            ),
            ("revision", json!("main"), "the revision is not 40 hex"),
            (
                "sha256",
                json!("AB".repeat(32)),
                "the SHA-256 is not 64 hex",
            ),
            ("size", json!(0), "no size"),
            ("license", json!("mit"), "its licence is not listed"),
            (
                "slot",
                json!("diffusion_model"),
                "fills a model or adapter slot",
            ),
            (
                "slot",
                json!("control_net"),
                "fills a model or adapter slot",
            ),
            (
                "id",
                json!("t5xxl-fp8"),
                "companion t5xxl-fp8: listed twice",
            ),
        ] {
            let err = refused(|v| v["files"][0][key] = value.clone());
            assert!(err.contains(says), "{key} {value}: {err}");
        }
        // A word this build does not know.
        let err = refused(|v| v["files"][0]["versions"][0] = json!("flux9"));
        assert!(err.contains("the companion table does not read"), "{err}");
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
            (
                ("byRecipe", "flux1-fill", "vae", "ae-flux1"),
                "byRecipe flux1-fill vae: ae-flux1 does not suit flux-fill",
            ),
        ] {
            let (section, key, slot, companion) = edit;
            let err = refused(|v| v[section][key][slot] = json!(companion));
            assert!(err.contains(says), "{err}");
        }
        for recipe in ["sd1-controlnet", "no-such-recipe"] {
            let err = refused(|v| v["byRecipe"][recipe] = json!({}));
            assert!(
                err.contains(&format!("byRecipe {recipe}: not a recipe Studio runs")),
                "{err}"
            );
        }
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
