//! What the app needs to know about a model, read from its GGUF header: the badges,
//! the trained context length and the numbers for the memory estimate.

use serde::Serialize;

use crate::gguf::{GgufHeader, Value};

/// A model's GGUF summary. Stored in the `models` table and shown as badges.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
    pub architecture: String,
    pub name: Option<String>,
    pub size_label: Option<String>,
    pub quantization: Option<String>,
    pub license: Option<String>,
    /// Context length the model was trained for.
    pub context_length: Option<u64>,
    pub block_count: u64,
    pub embedding_length: u64,
    pub head_count: u64,
    /// Per-layer key/value head counts. Recurrent (state-space) layers have 0.
    pub head_count_kv: Vec<u64>,
    pub key_length: u64,
    pub value_length: u64,
    pub expert_count: Option<u64>,
    pub expert_used_count: Option<u64>,
    pub vocab_size: Option<u64>,
    pub tensor_count: u64,
    /// The chat template mentions tools.
    pub template_tools: bool,
    /// The chat template has a thinking / reasoning switch or tags.
    pub template_thinking: bool,
    pub has_chat_template: bool,
    /// A vision/audio projector (`mmproj`), not a language model.
    pub is_projector: bool,
    /// `split.count` when the model is split across several files.
    pub split_count: Option<u64>,
}

/// Bytes per KV-cache element for the default f16 cache type.
pub const KV_F16_BYTES: u64 = 2;

/// How the conversation memory (the KV cache) is stored (0.0.5 D2c).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KvCacheType {
    /// llama.cpp's default: 2 bytes per value.
    #[default]
    F16,
    /// Save memory (`-ctk q8_0 -ctv q8_0`): blocks of 32 values in 34 bytes (a 2-byte
    /// scale and 32 one-byte values), about half of f16.
    Q8_0,
}

impl KvCacheType {
    /// Bytes for one row of `values` cache values, as llama.cpp lays a row out.
    pub fn row_bytes(self, values: u64) -> u64 {
        match self {
            Self::F16 => values.saturating_mul(KV_F16_BYTES),
            Self::Q8_0 => values.div_ceil(32).saturating_mul(34),
        }
    }
}

impl ModelSummary {
    pub fn from_header(h: &GgufHeader) -> Self {
        let arch = h.architecture().unwrap_or("unknown").to_string();
        let general_type = h.get_str("general.type");
        let is_projector = arch == "clip" || general_type == Some("mmproj");
        let block_count = h.arch_u64("block_count").unwrap_or(0);
        let embedding_length = h.arch_u64("embedding_length").unwrap_or(0);
        let head_count = per_layer(h, &arch, "attention.head_count", block_count)
            .into_iter()
            .max()
            .unwrap_or(0);
        let mut head_count_kv = per_layer(h, &arch, "attention.head_count_kv", block_count);
        if head_count_kv.is_empty() || head_count_kv.iter().all(|&n| n == 0) {
            // Without the key llama.cpp uses one KV head per attention head.
            head_count_kv = vec![head_count; block_count as usize];
        }
        let default_head_dim = embedding_length.checked_div(head_count).unwrap_or(0);
        let key_length = h
            .arch_u64("attention.key_length")
            .unwrap_or(default_head_dim);
        let value_length = h
            .arch_u64("attention.value_length")
            .unwrap_or(default_head_dim);
        let template = h.get_str("tokenizer.chat_template");
        let template_lower = template.map(str::to_ascii_lowercase).unwrap_or_default();
        let vocab_size = match h.get("tokenizer.ggml.tokens") {
            Some(Value::SkippedArray { len, .. }) => Some(*len),
            Some(Value::Array(items)) => Some(items.len() as u64),
            _ => h.arch_u64("vocab_size"),
        };
        Self {
            architecture: arch,
            name: h.get_str("general.name").map(str::to_string),
            size_label: h.get_str("general.size_label").map(str::to_string),
            quantization: h
                .get_u64("general.file_type")
                .and_then(file_type_label)
                .map(str::to_string),
            license: h.get_str("general.license").map(str::to_string),
            context_length: h.arch_u64("context_length"),
            block_count,
            embedding_length,
            head_count,
            head_count_kv,
            key_length,
            value_length,
            expert_count: h.arch_u64("expert_count").filter(|&n| n > 0),
            expert_used_count: h.arch_u64("expert_used_count").filter(|&n| n > 0),
            vocab_size,
            tensor_count: h.tensor_count,
            template_tools: template_lower.contains("tools"),
            template_thinking: template_lower.contains("<think>")
                || template_lower.contains("enable_thinking")
                || template_lower.contains("reasoning_content")
                || template_lower.contains("thinking"),
            has_chat_template: template.is_some(),
            is_projector,
            split_count: h.get_u64("split.count").filter(|&n| n > 1),
        }
    }

    /// KV-cache bytes per token of context, with an f16 cache:
    /// `Σ layers (key + value length) × KV heads × 2 bytes`, as llama.cpp allocates it.
    ///
    /// Sliding-window layers are counted at full length, so the estimate is on the
    /// safe side for models that use them; the bundled `llama-fit-params` gives the
    /// exact figure once the file is local (ARCHITECTURE.md 11).
    pub fn kv_bytes_per_token(&self) -> u64 {
        let per_head = (self.key_length + self.value_length) * KV_F16_BYTES;
        self.head_count_kv.iter().map(|&n| n * per_head).sum()
    }

    pub fn kv_bytes(&self, ctx: u64) -> u64 {
        self.kv_bytes_per_token().saturating_mul(ctx)
    }

    /// KV-cache bytes per token of context with the cache stored as `cache`: per layer, one
    /// key row (KV heads × key length values) and one value row, each laid out as `cache`
    /// lays out a row. The same sliding-window caveat as [`Self::kv_bytes_per_token`].
    pub fn kv_bytes_per_token_as(&self, cache: KvCacheType) -> u64 {
        self.head_count_kv
            .iter()
            .map(|&n| {
                cache
                    .row_bytes(n.saturating_mul(self.key_length))
                    .saturating_add(cache.row_bytes(n.saturating_mul(self.value_length)))
            })
            .fold(0u64, u64::saturating_add)
    }

    /// Whether this is a mixture-of-experts model.
    pub fn is_moe(&self) -> bool {
        self.expert_count.is_some()
    }
}

/// A parameter that is either one number or one number per layer.
fn per_layer(h: &GgufHeader, arch: &str, suffix: &str, layers: u64) -> Vec<u64> {
    match h.get(&format!("{arch}.{suffix}")) {
        Some(Value::Array(items)) => items.iter().map(|v| v.as_u64().unwrap_or(0)).collect(),
        Some(v) => v
            .as_u64()
            .map(|n| vec![n; layers as usize])
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Labels for `general.file_type` (`enum llama_ftype` in llama.h).
pub fn file_type_label(t: u64) -> Option<&'static str> {
    Some(match t {
        0 => "F32",
        1 => "F16",
        2 => "Q4_0",
        3 => "Q4_1",
        7 => "Q8_0",
        8 => "Q5_0",
        9 => "Q5_1",
        10 => "Q2_K",
        11 => "Q3_K_S",
        12 => "Q3_K_M",
        13 => "Q3_K_L",
        14 => "Q4_K_S",
        15 => "Q4_K_M",
        16 => "Q5_K_S",
        17 => "Q5_K_M",
        18 => "Q6_K",
        19 => "IQ2_XXS",
        20 => "IQ2_XS",
        21 => "Q2_K_S",
        22 => "IQ3_XS",
        23 => "IQ3_XXS",
        24 => "IQ1_S",
        25 => "IQ4_NL",
        26 => "IQ3_S",
        27 => "IQ3_M",
        28 => "IQ2_S",
        29 => "IQ2_M",
        30 => "IQ4_XS",
        31 => "IQ1_M",
        32 => "BF16",
        36 => "TQ1_0",
        37 => "TQ2_0",
        38 => "MXFP4_MOE",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gguf::testing::Writer;
    use std::io::Cursor;

    fn summary(bytes: Vec<u8>) -> ModelSummary {
        let len = bytes.len() as u64;
        ModelSummary::from_header(&crate::gguf::read(Cursor::new(bytes), len).unwrap())
    }

    #[test]
    fn dense_model_kv_formula() {
        // Llama-3-8B-like: 32 layers, 32 heads, 8 KV heads, 4096 embedding.
        let s = summary(
            Writer::new()
                .str("general.architecture", "llama")
                .u32("general.file_type", 15)
                .u32("llama.context_length", 8192)
                .u32("llama.block_count", 32)
                .u32("llama.embedding_length", 4096)
                .u32("llama.attention.head_count", 32)
                .u32("llama.attention.head_count_kv", 8)
                .str(
                    "tokenizer.chat_template",
                    "{% for m in messages %}{% endfor %}",
                )
                .build(),
        );
        assert_eq!(s.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(s.key_length, 128);
        // 32 layers × (128 + 128) × 8 heads × 2 bytes = 131072 bytes per token.
        assert_eq!(s.kv_bytes_per_token(), 131_072);
        // 8192 tokens → 1 GiB.
        assert_eq!(s.kv_bytes(8192), 1 << 30);
        assert!(!s.template_tools);
        assert!(s.has_chat_template);
        assert!(!s.is_moe());
    }

    #[test]
    fn explicit_head_dims_and_template_badges() {
        let s = summary(
            Writer::new()
                .str("general.architecture", "qwen3")
                .u32("qwen3.block_count", 28)
                .u32("qwen3.embedding_length", 1024)
                .u32("qwen3.attention.head_count", 16)
                .u32("qwen3.attention.head_count_kv", 8)
                .u32("qwen3.attention.key_length", 128)
                .u32("qwen3.attention.value_length", 128)
                .u32("qwen3.expert_count", 0)
                .str(
                    "tokenizer.chat_template",
                    "{%- if tools %}<tools>{%- endif %}{%- if enable_thinking %}<think>{% endif %}",
                )
                .build(),
        );
        assert_eq!(s.kv_bytes_per_token(), 28 * 256 * 8 * 2);
        assert!(s.template_tools);
        assert!(s.template_thinking);
        assert_eq!(s.expert_count, None);
    }

    #[test]
    fn hybrid_model_counts_only_attention_layers() {
        // Four layers, two of them recurrent (0 KV heads).
        let s = summary(
            Writer::new()
                .str("general.architecture", "lfm2")
                .u32("lfm2.block_count", 4)
                .u32("lfm2.embedding_length", 1024)
                .u32("lfm2.attention.head_count", 16)
                .u32_array("lfm2.attention.head_count_kv", &[8, 0, 8, 0])
                .build(),
        );
        assert_eq!(s.kv_bytes_per_token(), 2 * (64 + 64) * 8 * 2);
    }

    /// Save memory stores the cache as q8_0: 34 bytes per 32 values instead of 64.
    #[test]
    fn q8_0_cache_is_about_half_of_f16() {
        // Llama-3-8B-like: 32 layers, 8 KV heads of 128: 1,024 key and 1,024 value values
        // per layer and token.
        let s = summary(
            Writer::new()
                .str("general.architecture", "llama")
                .u32("llama.block_count", 32)
                .u32("llama.embedding_length", 4096)
                .u32("llama.attention.head_count", 32)
                .u32("llama.attention.head_count_kv", 8)
                .build(),
        );
        assert_eq!(s.kv_bytes_per_token_as(KvCacheType::F16), 131_072);
        assert_eq!(
            s.kv_bytes_per_token_as(KvCacheType::F16),
            s.kv_bytes_per_token()
        );
        // 32 layers × 2 rows × 1,024 / 32 blocks × 34 bytes.
        assert_eq!(s.kv_bytes_per_token_as(KvCacheType::Q8_0), 69_632);
        // A hybrid model: the recurrent layers have no cache in either type.
        let hybrid = summary(
            Writer::new()
                .str("general.architecture", "lfm2")
                .u32("lfm2.block_count", 4)
                .u32("lfm2.embedding_length", 1024)
                .u32("lfm2.attention.head_count", 16)
                .u32_array("lfm2.attention.head_count_kv", &[8, 0, 8, 0])
                .build(),
        );
        assert_eq!(hybrid.kv_bytes_per_token_as(KvCacheType::Q8_0), 2 * 2 * 16 * 34);
        // A row that is not a whole number of blocks is rounded up to one more block.
        assert_eq!(KvCacheType::Q8_0.row_bytes(33), 68);
        assert_eq!(KvCacheType::Q8_0.row_bytes(0), 0);
        assert_eq!(KvCacheType::F16.row_bytes(33), 66);
    }

    #[test]
    fn projector_is_flagged() {
        let s = summary(
            Writer::new()
                .str("general.architecture", "clip")
                .str("general.type", "mmproj")
                .build(),
        );
        assert!(s.is_projector);
        assert_eq!(s.kv_bytes_per_token(), 0);
    }

    #[test]
    fn real_model_if_available() {
        let Ok(path) = std::env::var("TEST_GGUF") else {
            return;
        };
        let h = crate::gguf::read_file(std::path::Path::new(&path)).unwrap();
        let s = ModelSummary::from_header(&h);
        assert!(s.kv_bytes_per_token() > 0);
        assert!(s.vocab_size.unwrap_or(0) > 1000);
        assert!(s.has_chat_template);
    }
}
