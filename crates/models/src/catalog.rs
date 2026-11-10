//! The curated model catalog (`catalog/models.json`, ARCHITECTURE.md 11): per memory
//! tier, a ranked list of models with pinned commits, files, sizes and SHA-256.
//!
//! No model name appears in code; everything comes from the JSON. A newer catalog is
//! accepted only if its minisign signature verifies against the updater's public key
//! and its sequence number is higher than the current one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::files::RepoFile;
use crate::hf::{validate_repo_id, validate_repo_path};

/// The catalog shipped with the app.
pub const BUNDLED: &str = include_str!("../../../catalog/models.json");

/// Memory assumed for KV cache and compute buffers at the recommended context
/// when an entry has no measured figure.
pub const UNMEASURED_ALLOWANCE_BYTES: u64 = 1_500_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schema_version: u32,
    pub sequence: u64,
    pub generated: String,
    #[serde(default)]
    pub note: Option<String>,
    /// Keyed by memory size in GB ("8", "12", …).
    pub tiers: BTreeMap<String, Tier>,
    pub models: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tier {
    pub recommended_ctx: u64,
    /// Model ids, best first.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct License {
    pub spdx: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// When and where an entry last passed the catalog test (section 17).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tested {
    pub llama_build: String,
    pub date: String,
    #[serde(default)]
    pub hardware: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub repo: String,
    /// 40-hex commit; `/resolve/<commit>/<file>` is immutable.
    pub revision: String,
    /// In order, so split models work.
    pub files: Vec<RepoFile>,
    #[serde(default)]
    pub mmproj: Option<RepoFile>,
    pub quant: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub license: License,
    #[serde(default)]
    pub gated: bool,
    /// Sampling overrides (temperature, top_p, …) sent with each request.
    #[serde(default)]
    pub sampling: BTreeMap<String, serde_json::Value>,
    /// Oldest llama.cpp build known to run the model.
    #[serde(default)]
    pub min_llama_build: Option<String>,
    #[serde(default)]
    pub tested: Option<Tested>,
    /// Measured memory at the tier's recommended context, in bytes.
    #[serde(default)]
    pub memory_at_recommended_ctx: Option<u64>,
}

impl Entry {
    pub fn weights_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    pub fn total_bytes(&self) -> u64 {
        self.weights_bytes() + self.mmproj.as_ref().map_or(0, |m| m.size)
    }
    /// Memory needed at the recommended context: measured, or files + allowance.
    pub fn memory_needed(&self) -> u64 {
        self.memory_at_recommended_ctx
            .unwrap_or_else(|| self.total_bytes() + UNMEASURED_ALLOWANCE_BYTES)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("the catalog is not valid JSON: {0}")]
    Parse(String),
    #[error("the catalog is invalid: {0}")]
    Invalid(String),
    #[error("the catalog signature does not verify")]
    BadSignature,
    #[error("the downloaded catalog is not newer than the current one")]
    NotNewer,
    #[error("no updater key is configured, so catalogs cannot be verified")]
    NoKey,
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl Catalog {
    pub fn parse(json: &str) -> Result<Self, CatalogError> {
        let c: Catalog =
            serde_json::from_str(json).map_err(|e| CatalogError::Parse(e.to_string()))?;
        c.validate()?;
        Ok(c)
    }

    pub fn bundled() -> Self {
        Self::parse(BUNDLED).expect("the bundled catalog is checked by a unit test")
    }

    fn validate(&self) -> Result<(), CatalogError> {
        let bad = |m: String| Err(CatalogError::Invalid(m));
        if self.schema_version != 1 {
            return bad(format!(
                "unsupported schema version {}",
                self.schema_version
            ));
        }
        for (id, e) in &self.models {
            if validate_repo_id(&e.repo).is_err() {
                return bad(format!("{id}: bad repo"));
            }
            if !is_hex(&e.revision, 40) {
                return bad(format!("{id}: revision must be a 40-hex commit"));
            }
            if e.files.is_empty() {
                return bad(format!("{id}: no files"));
            }
            for f in e.files.iter().chain(e.mmproj.iter()) {
                if validate_repo_path(&f.path).is_err()
                    || !f.path.to_ascii_lowercase().ends_with(".gguf")
                {
                    return bad(format!("{id}: bad file path {}", f.path));
                }
                if !f.sha256.as_deref().is_some_and(|s| is_hex(s, 64)) {
                    return bad(format!("{id}: {} needs a lowercase SHA-256", f.path));
                }
                if f.size == 0 {
                    return bad(format!("{id}: {} has no size", f.path));
                }
            }
        }
        for (tier, t) in &self.tiers {
            if tier.parse::<u64>().is_err() {
                return bad(format!("tier {tier} is not a number of GB"));
            }
            for id in &t.models {
                if !self.models.contains_key(id) {
                    return bad(format!("tier {tier} lists unknown model {id}"));
                }
            }
        }
        Ok(())
    }

    /// Tiers in ascending order of memory, as (GB, tier).
    pub fn tiers_ascending(&self) -> Vec<(u64, &Tier)> {
        let mut v: Vec<(u64, &Tier)> = self
            .tiers
            .iter()
            .filter_map(|(k, t)| k.parse::<u64>().ok().map(|gb| (gb, t)))
            .collect();
        v.sort_by_key(|(gb, _)| *gb);
        v
    }
}

/// One suggestion for the first-run screen.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: String,
    pub tier_gb: u64,
    pub recommended_ctx: u64,
    pub fits: bool,
    pub tested: bool,
    pub memory_needed: u64,
}

/// What first run offers on hardware with `usable_bytes` for the model.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunPick {
    /// The automatic pick: the highest-ranked *tested* entry that fits. `None` when
    /// nothing tested fits; the UI then lists `suggestions` without preselecting.
    pub pick: Option<String>,
    /// Every entry from the largest tier that has a fitting model downward, best first.
    pub suggestions: Vec<Suggestion>,
}

/// First-run choice (ARCHITECTURE.md 11): the highest-ranked tested entry that is
/// green on the detected hardware. Hardware above the largest tested tier gets the
/// largest tested entry; an untested entry is never picked automatically.
pub fn first_run(catalog: &Catalog, usable_bytes: u64) -> FirstRunPick {
    let mut suggestions = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (gb, tier) in catalog.tiers_ascending().into_iter().rev() {
        for id in &tier.models {
            let Some(e) = catalog.models.get(id) else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            let needed = e.memory_needed();
            suggestions.push(Suggestion {
                id: id.clone(),
                tier_gb: gb,
                recommended_ctx: tier.recommended_ctx,
                fits: needed + crate::fit::SPARE_BYTES <= usable_bytes,
                tested: e.tested.is_some(),
                memory_needed: needed,
            });
        }
    }
    let pick = suggestions
        .iter()
        .find(|s| s.fits && s.tested)
        .map(|s| s.id.clone());
    // Keep the fitting entries (best first), then the rest for "show all".
    suggestions.sort_by_key(|s| !s.fits);
    FirstRunPick { pick, suggestions }
}

/// Verifies a downloaded catalog with the updater key and checks it is newer.
/// `public_key` is the minisign public key as Tauri's updater stores it (the
/// base64 of the key file) or the key file text itself.
pub fn verify_refresh(
    json: &str,
    signature: &str,
    public_key: Option<&str>,
    current_sequence: u64,
) -> Result<Catalog, CatalogError> {
    let key_text = public_key
        .filter(|k| !k.trim().is_empty())
        .ok_or(CatalogError::NoKey)?;
    let pk = decode_public_key(key_text).ok_or(CatalogError::BadSignature)?;
    let sig_text = decode_maybe_base64(signature);
    let sig =
        minisign_verify::Signature::decode(&sig_text).map_err(|_| CatalogError::BadSignature)?;
    pk.verify(json.as_bytes(), &sig, false)
        .map_err(|_| CatalogError::BadSignature)?;
    let catalog = Catalog::parse(json)?;
    if catalog.sequence <= current_sequence {
        return Err(CatalogError::NotNewer);
    }
    Ok(catalog)
}

/// Tauri stores keys and signatures base64-encoded; accept both forms.
fn decode_maybe_base64(text: &str) -> String {
    use base64::Engine;
    let t = text.trim();
    if t.starts_with("untrusted comment:") {
        return t.to_string();
    }
    base64::engine::general_purpose::STANDARD
        .decode(t)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| t.to_string())
}

fn decode_public_key(text: &str) -> Option<minisign_verify::PublicKey> {
    let t = decode_maybe_base64(text);
    if t.starts_with("untrusted comment:") {
        minisign_verify::PublicKey::decode(&t).ok()
    } else {
        minisign_verify::PublicKey::from_base64(t.trim()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn bundled_catalog_is_valid() {
        let c = Catalog::bundled();
        assert!(c.sequence >= 1);
        assert!(!c.tiers.is_empty());
        for t in c.tiers.values() {
            assert!(!t.models.is_empty());
        }
    }

    #[test]
    fn untested_entries_are_never_picked() {
        let c = Catalog::bundled();
        let r = first_run(&c, 64 * GB);
        if c.models.values().all(|e| e.tested.is_none()) {
            assert_eq!(r.pick, None);
        }
        assert!(r.suggestions.iter().any(|s| s.fits));
    }

    fn with_tested(mut c: Catalog, ids: &[&str]) -> Catalog {
        for id in ids {
            c.models.get_mut(*id).unwrap().tested = Some(Tested {
                llama_build: "b12110".into(),
                date: "2026-11-10".into(),
                hardware: None,
            });
        }
        c
    }

    #[test]
    fn picks_highest_ranked_tested_entry_that_fits() {
        let c = Catalog::bundled();
        let small_tier = c.tiers_ascending()[0].1.models.clone();
        let all: Vec<String> = c.models.keys().cloned().collect();
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        let c = with_tested(c, &refs);
        // Lots of memory: the first entry of the largest tier.
        let big = first_run(&c, 200 * GB);
        let largest = c.tiers_ascending().last().unwrap().1.models[0].clone();
        assert_eq!(big.pick.as_deref(), Some(largest.as_str()));
        // Little memory: something from the smallest tier that fits.
        let r = first_run(&c, 6 * GB);
        let pick = r.pick.unwrap();
        assert!(small_tier.contains(&pick), "{pick}");
        assert!(c.models[&pick].memory_needed() + crate::fit::SPARE_BYTES <= 6 * GB);
        // Nothing fits.
        assert_eq!(first_run(&c, GB).pick, None);
    }

    #[test]
    fn rejects_bad_catalogs() {
        let mut v: serde_json::Value = serde_json::from_str(BUNDLED).unwrap();
        let first = v["models"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        v["models"][&first]["revision"] = "main".into();
        assert!(Catalog::parse(&v.to_string()).is_err());
        let mut v: serde_json::Value = serde_json::from_str(BUNDLED).unwrap();
        v["models"][&first]["files"][0]["path"] = "../x.gguf".into();
        assert!(Catalog::parse(&v.to_string()).is_err());
        let mut v: serde_json::Value = serde_json::from_str(BUNDLED).unwrap();
        v["tiers"]["8"]["models"][0] = "missing".into();
        assert!(Catalog::parse(&v.to_string()).is_err());
    }

    // A minisign key pair and signature made for this test only (see
    // tests/fixtures/catalog/README.md); not the release key.
    const TEST_PUBKEY: &str = include_str!("../tests/fixtures/catalog/test.pub");
    const TEST_CATALOG: &str = include_str!("../tests/fixtures/catalog/models.json");
    const TEST_SIG: &str = include_str!("../tests/fixtures/catalog/models.json.minisig");

    #[test]
    fn refresh_accepts_a_signed_newer_catalog() {
        let c = verify_refresh(TEST_CATALOG, TEST_SIG, Some(TEST_PUBKEY), 1).unwrap();
        assert_eq!(c.sequence, 2);
        // Tauri-style base64 forms work too.
        use base64::Engine;
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        assert!(verify_refresh(TEST_CATALOG, &b64(TEST_SIG), Some(&b64(TEST_PUBKEY)), 1).is_ok());
    }

    #[test]
    fn refresh_rejects_tampering_old_sequence_and_missing_key() {
        let tampered = TEST_CATALOG.replace("\"sequence\": 2", "\"sequence\": 3");
        assert!(matches!(
            verify_refresh(&tampered, TEST_SIG, Some(TEST_PUBKEY), 1),
            Err(CatalogError::BadSignature)
        ));
        assert!(matches!(
            verify_refresh(TEST_CATALOG, TEST_SIG, Some(TEST_PUBKEY), 2),
            Err(CatalogError::NotNewer)
        ));
        assert!(matches!(
            verify_refresh(TEST_CATALOG, TEST_SIG, None, 1),
            Err(CatalogError::NoKey)
        ));
        assert!(matches!(
            verify_refresh(TEST_CATALOG, "garbage", Some(TEST_PUBKEY), 1),
            Err(CatalogError::BadSignature)
        ));
    }
}
