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

    // ---- 0.0.6 item 4 (comparison): all optional, so older catalogs still read and
    // older parsers (which ignore unknown fields) still read newer catalogs. What a
    // model is good at comes only from the publisher's own model card (D6).
    /// A verbatim quote of at most two sentences from the publisher's model card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Keys from [`GOOD_AT`], each backed by a phrase on the publisher's card. Unknown
    /// keys (and anything that is not a string) are dropped when read, never an error.
    #[serde(
        default,
        deserialize_with = "good_at_keys",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub good_at: Vec<String>,
    /// Parameters, in units (9 billion is 9000000000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<u64>,
    /// For a mixture of experts: the parameters used at a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_parameters: Option<u64>,
    /// Who made the model (not who converted it to GGUF).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Language codes from the publisher's card (`en`, `fr`, or `multilingual`). A single
    /// string is read as a list of one; anything that is not a string is dropped.
    #[serde(
        default,
        deserialize_with = "string_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub languages: Vec<String>,
    /// Release date, `YYYY-MM-DD` or `YYYY-MM` (see [`Entry::released_date`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
    /// The publisher's model card on huggingface.co that `summary` and `goodAt` come
    /// from (see [`Entry::source_repo`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// What a model can be "good at" (0.0.6 item 4), in the order the specification lists
/// them. The webview has a phrase for each (`src/lib/abilities.ts`).
pub const GOOD_AT: [&str; 10] = [
    "chat",
    "writing",
    "coding",
    "reasoning",
    "math",
    "tools",
    "images",
    "long-documents",
    "languages",
    "speed",
];

/// Reads `goodAt` leniently: the known keys of a list, once each, in the order given.
fn good_at_keys<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let mut out: Vec<String> = Vec::new();
    for key in strings_of(&v) {
        if GOOD_AT.contains(&key) && !out.iter().any(|k| k == key) {
            out.push(key.to_string());
        }
    }
    Ok(out)
}

/// Reads a string or a list of strings; other values are dropped.
fn string_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(strings_of(&v).map(str::to_string).collect())
}

fn strings_of(v: &serde_json::Value) -> impl Iterator<Item = &str> {
    let items: Vec<&serde_json::Value> = match v {
        serde_json::Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    items.into_iter().filter_map(serde_json::Value::as_str)
}

impl Entry {
    /// The `owner/name` of the publisher's model card, when `source` is an https link
    /// to a repository page on huggingface.co; `None` otherwise (the link is not shown).
    pub fn source_repo(&self) -> Option<String> {
        let url = reqwest::Url::parse(self.source.as_deref()?.trim()).ok()?;
        if url.scheme() != "https"
            || url.host_str() != Some("huggingface.co")
            || url.port().is_some()
            || !url.username().is_empty()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let path = url.path().trim_matches('/');
        validate_repo_id(path).ok()?;
        Some(path.to_string())
    }

    /// The publisher's model card link, only when [`Self::source_repo`] accepts it.
    pub fn source_url(&self) -> Option<String> {
        self.source_repo()
            .map(|repo| format!("https://huggingface.co/{repo}"))
    }

    /// `released` when it is a date the app can show (`YYYY-MM-DD` or `YYYY-MM`).
    pub fn released_date(&self) -> Option<&str> {
        let r = self.released.as_deref()?.trim();
        let b = r.as_bytes();
        let digits = |range: std::ops::Range<usize>| b[range].iter().all(u8::is_ascii_digit);
        let month_ok = |m: &[u8]| matches!(m, [b'0', b'1'..=b'9'] | [b'1', b'0'..=b'2']);
        let ok = match b.len() {
            7 => digits(0..4) && b[4] == b'-' && month_ok(&b[5..7]),
            10 => {
                digits(0..4)
                    && b[4] == b'-'
                    && month_ok(&b[5..7])
                    && b[7] == b'-'
                    && digits(8..10)
                    && (b"01".as_slice()..=b"31".as_slice()).contains(&&b[8..10])
            }
            _ => false,
        };
        ok.then_some(r)
    }

    /// `active_parameters` only when it is smaller than `parameters` (a mixture of
    /// experts); a value that cannot be right is not shown.
    pub fn active_parameters_checked(&self) -> Option<u64> {
        let active = self.active_parameters?;
        match self.parameters {
            Some(total) if active > 0 && active < total => Some(active),
            _ => None,
        }
    }

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

    // ---- 0.0.6 item 4: comparison fields ---------------------------------------------

    /// A catalog with the comparison fields (made-up names and numbers).
    const COMPARE_CATALOG: &str = include_str!("../tests/fixtures/catalog/compare-fields.json");

    #[test]
    fn new_fields_are_read() {
        let c = Catalog::parse(COMPARE_CATALOG).unwrap();
        let e = &c.models["sample-7b-q4km"];
        assert_eq!(
            e.summary.as_deref(),
            Some(
                "Sample 7B is a small model made for testing. It answers questions and writes code."
            )
        );
        assert_eq!(e.parameters, Some(7_000_000_000));
        assert_eq!(e.active_parameters, None);
        assert_eq!(e.publisher.as_deref(), Some("Example Labs"));
        assert_eq!(e.languages, ["en", "fr"]);
        assert_eq!(e.released_date(), Some("2026-03"));
        assert_eq!(e.source_repo().as_deref(), Some("example-labs/Sample-7B"));
        assert_eq!(
            e.source_url().as_deref(),
            Some("https://huggingface.co/example-labs/Sample-7B")
        );

        let moe = &c.models["sample-moe-q4km"];
        assert_eq!(moe.active_parameters_checked(), Some(3_000_000_000));
        assert_eq!(moe.released_date(), Some("2026-05-02"));
        // A single string is a list of one.
        assert_eq!(moe.languages, ["multilingual"]);
        assert_eq!(moe.license.name.as_deref(), Some("Example Model License"));
    }

    /// `goodAt` keeps the known keys once each, in order, and drops the rest without
    /// rejecting the catalog.
    #[test]
    fn good_at_vocabulary() {
        let c = Catalog::parse(COMPARE_CATALOG).unwrap();
        assert_eq!(
            c.models["sample-7b-q4km"].good_at,
            ["chat", "coding", "long-documents"]
        );
        assert_eq!(
            c.models["sample-moe-q4km"].good_at,
            ["reasoning", "math", "speed"]
        );
        assert!(c.models["sample-plain-q8"].good_at.is_empty());
        // Every key of the vocabulary is accepted; a lone string, an object or null
        // are not lists of keys but do not break the catalog.
        let mut v: serde_json::Value = serde_json::from_str(COMPARE_CATALOG).unwrap();
        v["models"]["sample-plain-q8"]["goodAt"] = serde_json::json!(GOOD_AT);
        let c = Catalog::parse(&v.to_string()).unwrap();
        assert_eq!(c.models["sample-plain-q8"].good_at, GOOD_AT);
        for odd in [
            serde_json::json!("coding"),
            serde_json::json!({"coding": true}),
            serde_json::Value::Null,
        ] {
            v["models"]["sample-plain-q8"]["goodAt"] = odd.clone();
            let c = Catalog::parse(&v.to_string()).unwrap();
            let keys = &c.models["sample-plain-q8"].good_at;
            assert!(keys.is_empty() || keys == &["coding"], "{odd}: {keys:?}");
        }
    }

    /// Values the app cannot show are left out at use, never a reason to reject the
    /// catalog (a bundled catalog that did not parse would stop the app).
    #[test]
    fn unusable_values_are_left_out() {
        let c = Catalog::parse(COMPARE_CATALOG).unwrap();
        let plain = &c.models["sample-plain-q8"];
        assert_eq!(plain.source_repo(), None, "http and another host");
        assert_eq!(plain.source_url(), None);
        assert_eq!(plain.released_date(), None);
        assert_eq!(
            plain.active_parameters_checked(),
            None,
            "more than the total"
        );
        let with = |source: &str| Entry {
            source: Some(source.into()),
            ..plain.clone()
        };
        for bad in [
            "https://huggingface.co/",
            "https://huggingface.co/only-owner",
            "https://huggingface.co/a/b/tree/main",
            "https://huggingface.co.evil.example/a/b",
            "https://user@huggingface.co/a/b",
            "https://huggingface.co:8443/a/b",
            "https://huggingface.co/a/b?x=1",
            "https://huggingface.co/a/..",
            "javascript:alert(1)",
            "",
        ] {
            assert_eq!(with(bad).source_repo(), None, "{bad}");
        }
        assert_eq!(
            with("https://huggingface.co/a-b/c.d_e/")
                .source_repo()
                .as_deref(),
            Some("a-b/c.d_e")
        );
        let on = |released: &str| Entry {
            released: Some(released.into()),
            ..plain.clone()
        };
        for good in ["2026-01", "2026-12", "2026-02-28", "2025-10-31"] {
            assert_eq!(on(good).released_date(), Some(good));
        }
        for bad in [
            "2026-13",
            "2026-00",
            "2026-1",
            "2026-01-32",
            "2026-01-00",
            "26-01",
        ] {
            assert_eq!(on(bad).released_date(), None, "{bad}");
        }
    }

    /// The catalog as 0.0.5 parsed it, field for field (no `deny_unknown_fields`, as
    /// then): a 0.0.5 app reading a 0.0.6 catalog ignores the new fields.
    mod v005 {
        #![allow(dead_code)]
        use std::collections::BTreeMap;

        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Catalog {
            pub schema_version: u32,
            pub sequence: u64,
            pub generated: String,
            #[serde(default)]
            pub note: Option<String>,
            pub tiers: BTreeMap<String, Tier>,
            pub models: BTreeMap<String, Entry>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Tier {
            pub recommended_ctx: u64,
            pub models: Vec<String>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct License {
            pub spdx: String,
            #[serde(default)]
            pub name: Option<String>,
            #[serde(default)]
            pub url: Option<String>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Tested {
            pub llama_build: String,
            pub date: String,
            #[serde(default)]
            pub hardware: Option<String>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct Entry {
            pub name: String,
            pub repo: String,
            pub revision: String,
            pub files: Vec<crate::files::RepoFile>,
            #[serde(default)]
            pub mmproj: Option<crate::files::RepoFile>,
            pub quant: String,
            #[serde(default)]
            pub capabilities: Vec<String>,
            pub license: License,
            #[serde(default)]
            pub gated: bool,
            #[serde(default)]
            pub sampling: BTreeMap<String, serde_json::Value>,
            #[serde(default)]
            pub min_llama_build: Option<String>,
            #[serde(default)]
            pub tested: Option<Tested>,
            #[serde(default)]
            pub memory_at_recommended_ctx: Option<u64>,
        }
    }

    #[test]
    fn an_older_parser_reads_a_catalog_with_the_new_fields() {
        let old: v005::Catalog = serde_json::from_str(COMPARE_CATALOG).unwrap();
        assert_eq!(old.schema_version, 1);
        assert_eq!(old.sequence, 2);
        assert_eq!(old.models.len(), 3);
        let e = &old.models["sample-moe-q4km"];
        assert_eq!(e.files.len(), 2);
        assert_eq!(e.license.spdx, "other");
        assert_eq!(old.tiers["24"].models, ["sample-moe-q4km"]);
        assert_eq!(e.name, "Sample MoE 30B");
        // And the bundled catalog, whatever it holds now.
        let bundled: v005::Catalog = serde_json::from_str(BUNDLED).unwrap();
        assert_eq!(bundled.models.len(), Catalog::bundled().models.len());
    }

    /// A catalog without the new fields (the signed 0.0.5 test catalog) reads with
    /// every new field empty; writing it again leaves them out.
    #[test]
    fn a_catalog_without_the_new_fields_still_reads() {
        let c = Catalog::parse(TEST_CATALOG).unwrap();
        for e in c.models.values() {
            assert!(e.summary.is_none() && e.good_at.is_empty() && e.parameters.is_none());
            assert!(e.publisher.is_none() && e.languages.is_empty() && e.source.is_none());
            assert!(e.released.is_none() && e.active_parameters.is_none());
        }
        let again = serde_json::to_value(&c).unwrap();
        let first = again["models"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        for key in [
            "summary",
            "goodAt",
            "parameters",
            "publisher",
            "languages",
            "source",
        ] {
            assert!(first.get(key).is_none(), "{key}");
        }
        // The new fields survive a round trip.
        let c = Catalog::parse(COMPARE_CATALOG).unwrap();
        let back = Catalog::parse(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    /// What the bundled catalog says in the new fields is what the app shows: a link,
    /// date or count it would leave out is a mistake in the catalog (WP4c's content).
    #[test]
    fn bundled_comparison_fields_are_usable() {
        let c = Catalog::bundled();
        for (id, e) in &c.models {
            if e.source.is_some() {
                assert!(e.source_repo().is_some(), "{id}: source {:?}", e.source);
            }
            if e.released.is_some() {
                assert!(
                    e.released_date().is_some(),
                    "{id}: released {:?}",
                    e.released
                );
            }
            if e.active_parameters.is_some() {
                assert!(
                    e.active_parameters_checked().is_some(),
                    "{id}: active parameters"
                );
            }
            if let Some(s) = &e.summary {
                assert!(
                    !s.trim().is_empty() && s.chars().count() <= 600,
                    "{id}: summary"
                );
                assert!(!s.chars().any(char::is_control), "{id}: summary");
            }
        }
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
