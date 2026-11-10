//! Turns a Hugging Face repository's file list into downloadable model variants
//! (ARCHITECTURE.md 11), in a fixed order:
//!
//! 1. not `.gguf` → ignored;
//! 2. projector (`mmproj`) → attached to the models;
//! 3. helper files (importance matrices, draft-model sidecars) → hidden;
//! 4. split shards → grouped and their sizes summed;
//! 5. the quantization label is read from the file name.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// One file in a repository, as the Hugging Face API lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFile {
    /// Path inside the repository, `/`-separated.
    pub path: String,
    pub size: u64,
    /// SHA-256 of the content (the LFS object id), lowercase hex.
    pub sha256: Option<String>,
}

/// A model the user can pick: one file, or every shard of a split model.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    /// Quantization label (`Q4_K_M`, `IQ3_XXS`, `BF16`…) or the file stem when none is found.
    pub label: String,
    /// Shards in order, or the single file.
    pub files: Vec<RepoFile>,
    pub total_size: u64,
    /// Bits per weight implied by the label, for sorting; `None` when unknown.
    pub bits: Option<f32>,
}

impl Variant {
    pub fn is_split(&self) -> bool {
        self.files.len() > 1
    }
    /// The file llama-server is pointed at (the first shard).
    pub fn first_file(&self) -> &RepoFile {
        &self.files[0]
    }
}

/// The classified repository.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoModels {
    pub variants: Vec<Variant>,
    /// Projector files; the preferred one first.
    pub projectors: Vec<RepoFile>,
    /// Shard groups with a missing piece; listed so the UI can say why they're absent.
    pub incomplete: Vec<String>,
}

static SHARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<base>.+)-(?P<n>\d{5})-of-(?P<total>\d{5})\.gguf$").unwrap()
});

/// The quantization token, from the research corpus of 34,052 Hub files
/// (docs/research/huggingface.md): matches about 97% of model files.
static QUANT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[-._/\s])(?P<q>(?:UD-)?(?:(?:IQ[1-4]|T?Q[1-8])_(?:K_(?:XXL|XL|L|M|S|P)|K|XXS|XS|NL|S|M|L|[0-9](?:_[0-9]_[0-9])?)|BF16|F16|F32|FP16|FP32|MXFP4(?:_MOE)?|NVFP4|F8_E4M3))(?:[-._/\s]|$)",
    )
    .unwrap()
});

/// Speculative-decoding sidecars, which llama.cpp does not load as a model.
const SIDECAR_PREFIXES: &[&str] = &["mtp-", "dflash-", "eagle3-", "dspark-"];

fn is_gguf(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".gguf")
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_projector(path: &str) -> bool {
    file_name(path).to_ascii_lowercase().contains("mmproj")
}

fn is_helper(path: &str) -> bool {
    let name = file_name(path).to_ascii_lowercase();
    name.contains("imatrix") || SIDECAR_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// The quantization label in a file name, upper-cased (`UD-Q4_K_XL`, `Q8_0`…).
pub fn quant_label(path: &str) -> Option<String> {
    let name = file_name(path);
    let stem = name
        .strip_suffix(".gguf")
        .or_else(|| name.strip_suffix(".GGUF"))
        .unwrap_or(name);
    let stem = SHARD
        .captures(name)
        .and_then(|c| c.name("base").map(|m| m.as_str()))
        .unwrap_or(stem);
    // Last match wins: "Llama-3-8B-Q4_K_M" has the quant at the end. Fall back to
    // the whole path for "Q4_K_M/model-00001-of-00003.gguf".
    let last = last_quant(stem).or_else(|| {
        let dir = path.rsplit_once('/').map(|(d, _)| d)?;
        last_quant(dir)
    });
    last.map(|q| match q.as_str() {
        "FP16" => "F16".to_string(),
        "FP32" => "F32".to_string(),
        _ => q,
    })
}

fn last_quant(text: &str) -> Option<String> {
    let mut last = None;
    let mut start = 0;
    while start < text.len() {
        let Some(m) = QUANT.captures_at(text, start) else {
            break;
        };
        let q = m.name("q").expect("group q always participates");
        last = Some(q.as_str().to_ascii_uppercase());
        // The trailing separator may start the next match.
        start = q.end();
    }
    last
}

/// Approximate bits per weight for a label, used only for sorting.
pub fn bits_per_weight(label: &str) -> Option<f32> {
    let l = label.trim_start_matches("UD-");
    Some(match l {
        "F32" => 32.0,
        "F16" | "BF16" => 16.0,
        "Q8_0" => 8.5,
        "Q6_K" => 6.56,
        "Q5_K_M" | "Q5_K_L" | "Q5_K_XL" => 5.7,
        "Q5_K_S" | "Q5_0" | "Q5_1" => 5.5,
        "Q4_K_M" | "Q4_K_L" | "Q4_K_XL" => 4.85,
        "Q4_K_S" | "Q4_0" | "Q4_1" | "IQ4_NL" => 4.5,
        "MXFP4" | "MXFP4_MOE" => 4.25,
        "IQ4_XS" => 4.25,
        "Q3_K_L" | "Q3_K_XL" => 4.0,
        "Q3_K_M" => 3.9,
        "IQ3_M" | "IQ3_S" => 3.5,
        "Q3_K_S" => 3.4,
        "IQ3_XS" | "IQ3_XXS" => 3.1,
        "Q2_K" | "Q2_K_L" | "Q2_K_XL" => 2.9,
        "Q2_K_S" | "IQ2_M" | "IQ2_S" => 2.7,
        "IQ2_XS" | "IQ2_XXS" => 2.3,
        "IQ1_M" | "IQ1_S" | "TQ1_0" | "TQ2_0" => 1.8,
        _ => return None,
    })
}

/// Preference order for projectors when a repository offers several precisions.
fn projector_rank(path: &str) -> u8 {
    match quant_label(path).as_deref() {
        Some("F16") => 0,
        Some("BF16") => 1,
        Some("Q8_0") => 2,
        Some("F32") => 3,
        _ => 4,
    }
}

/// Classifies a repository's files.
pub fn classify(files: &[RepoFile]) -> RepoModels {
    let mut projectors = Vec::new();
    let mut singles = Vec::new();
    let mut groups: BTreeMap<(String, u32), Vec<(u32, RepoFile)>> = BTreeMap::new();
    for f in files {
        if !is_gguf(&f.path) {
            continue;
        }
        if is_projector(&f.path) {
            projectors.push(f.clone());
            continue;
        }
        if is_helper(&f.path) {
            continue;
        }
        if let Some(c) = SHARD.captures(file_name(&f.path)) {
            let n: u32 = c["n"].parse().unwrap_or(0);
            let total: u32 = c["total"].parse().unwrap_or(0);
            let dir = f.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            let key = (format!("{dir}/{}", &c["base"]), total);
            groups.entry(key).or_default().push((n, f.clone()));
            continue;
        }
        singles.push(f.clone());
    }

    let mut variants: Vec<Variant> = singles
        .into_iter()
        .map(|f| {
            let label = quant_label(&f.path).unwrap_or_else(|| stem(&f.path));
            Variant {
                bits: bits_per_weight(&label),
                label,
                total_size: f.size,
                files: vec![f],
            }
        })
        .collect();

    let mut incomplete = Vec::new();
    for ((base, total), mut shards) in groups {
        shards.sort_by_key(|(n, _)| *n);
        shards.dedup_by_key(|(n, _)| *n);
        let complete = total > 0
            && shards.len() == total as usize
            && shards
                .iter()
                .enumerate()
                .all(|(i, (n, _))| *n as usize == i + 1);
        if !complete {
            incomplete.push(base.trim_start_matches('/').to_string());
            continue;
        }
        let files: Vec<RepoFile> = shards.into_iter().map(|(_, f)| f).collect();
        let label = quant_label(&files[0].path).unwrap_or_else(|| stem(&files[0].path));
        variants.push(Variant {
            bits: bits_per_weight(&label),
            label,
            total_size: files.iter().map(|f| f.size).sum(),
            files,
        });
    }

    // Distinct labels: when two files share one (different subfolders), keep the
    // path so they stay distinguishable.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for v in &variants {
        *counts.entry(v.label.clone()).or_default() += 1;
    }
    for v in &mut variants {
        if counts[&v.label] > 1 {
            v.label = format!("{} ({})", v.label, stem(&v.first_file().path));
        }
    }

    variants.sort_by(|a, b| {
        b.bits
            .unwrap_or(0.0)
            .partial_cmp(&a.bits.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.total_size.cmp(&b.total_size))
            .then(a.label.cmp(&b.label))
    });
    projectors.sort_by_key(|p| (projector_rank(&p.path), p.size));
    RepoModels {
        variants,
        projectors,
        incomplete,
    }
}

fn stem(path: &str) -> String {
    let name = file_name(path);
    name.strip_suffix(".gguf")
        .or_else(|| name.strip_suffix(".GGUF"))
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(path: &str, size: u64) -> RepoFile {
        RepoFile {
            path: path.into(),
            size,
            sha256: Some("ab".repeat(32)),
        }
    }

    #[test]
    fn quant_labels_from_names() {
        assert_eq!(
            quant_label("Llama-3.1-8B-Instruct-Q4_K_M.gguf").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(quant_label("model.Q8_0.gguf").as_deref(), Some("Q8_0"));
        assert_eq!(
            quant_label("Qwen3-8B-UD-Q4_K_XL.gguf").as_deref(),
            Some("UD-Q4_K_XL")
        );
        assert_eq!(
            quant_label("gemma-3-27b-it-IQ3_XXS.gguf").as_deref(),
            Some("IQ3_XXS")
        );
        assert_eq!(
            quant_label("gpt-oss-20b-MXFP4.gguf").as_deref(),
            Some("MXFP4")
        );
        assert_eq!(quant_label("Model-bf16.gguf").as_deref(), Some("BF16"));
        assert_eq!(quant_label("Model-fp16.gguf").as_deref(), Some("F16"));
        assert_eq!(
            quant_label("Q4_K_M/Big-Q4_K_M-00001-of-00003.gguf").as_deref(),
            Some("Q4_K_M")
        );
        assert_eq!(
            quant_label("gemma-4-E4B_q4_0-it.gguf").as_deref(),
            Some("Q4_0")
        );
        assert_eq!(
            quant_label("BF16/Model-00001-of-00002.gguf").as_deref(),
            Some("BF16")
        );
        assert_eq!(quant_label("Model-NVFP4.gguf").as_deref(), Some("NVFP4"));
        assert_eq!(quant_label("Model-TQ1_0.gguf").as_deref(), Some("TQ1_0"));
        assert_eq!(quant_label("Model-Q4_K.gguf").as_deref(), Some("Q4_K"));
        assert_eq!(quant_label("plain-model.gguf"), None);
        // "Q4" alone is not a label; neither is a substring inside a word.
        assert_eq!(quant_label("AQ4_K_MX.gguf"), None);
    }

    #[test]
    fn classification_order() {
        let files = vec![
            f("README.md", 10),
            f("config.json", 10),
            f("Model-Q4_K_M.gguf", 4_000),
            f("Model-Q8_0.gguf", 8_000),
            f("Model-IQ2_XXS.gguf", 2_000),
            f("mmproj-Model-F32.gguf", 600),
            f("mmproj-Model-F16.gguf", 300),
            f("Model.imatrix.gguf", 5),
            f("imatrix.dat", 5),
            f("mtp-Model-Q8_0.gguf", 500),
            f("eagle3-Model.gguf", 500),
            f("BF16/Model-BF16-00002-of-00002.gguf", 7_000),
            f("BF16/Model-BF16-00001-of-00002.gguf", 9_000),
            f("Q6_K/Model-Q6_K-00001-of-00002.gguf", 5_000),
        ];
        let r = classify(&files);
        let labels: Vec<&str> = r.variants.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(labels, vec!["BF16", "Q8_0", "Q4_K_M", "IQ2_XXS"]);
        let bf16 = &r.variants[0];
        assert!(bf16.is_split());
        assert_eq!(bf16.total_size, 16_000);
        assert_eq!(
            bf16.first_file().path,
            "BF16/Model-BF16-00001-of-00002.gguf"
        );
        assert_eq!(r.projectors[0].path, "mmproj-Model-F16.gguf");
        assert_eq!(r.projectors.len(), 2);
        assert_eq!(r.incomplete, vec!["Q6_K/Model-Q6_K".to_string()]);
    }

    #[test]
    fn duplicate_labels_stay_distinct() {
        let r = classify(&[f("a/Model-A-Q4_K_M.gguf", 1), f("b/Model-B-Q4_K_M.gguf", 2)]);
        let labels: Vec<&str> = r.variants.iter().map(|v| v.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Q4_K_M (Model-A-Q4_K_M)", "Q4_K_M (Model-B-Q4_K_M)"]
        );
    }

    #[test]
    fn unlabelled_file_uses_its_stem() {
        let r = classify(&[f("tiny-model.gguf", 1)]);
        assert_eq!(r.variants[0].label, "tiny-model");
        assert_eq!(r.variants[0].bits, None);
    }
}
