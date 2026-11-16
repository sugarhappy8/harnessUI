//! The conversation memory (llama.cpp's KV cache) a model gets, layer by layer, as the
//! pinned llama.cpp build allocates it with the app's launch words: `-c C -np 1`, no `-b`,
//! `-ub`, `--swa-full` or `--ctx-checkpoints`, flash attention `auto` unless Fine control
//! turns it off, and Save memory's `-fa on -ctk q8_0 -ctv q8_0`
//! (`crates/runtime/src/llama/launch.rs`).
//!
//! Ported from llama.cpp b12110, commit cd959bae99ff4bed22bef2604f45241c09bbb8c8 (file and
//! function names below are that commit's):
//!
//! - Every layer that keeps its own memory gets a key tensor of `n_embd_k_gqa(il)` values
//!   and, unless the model stores keys only (MLA), a value tensor of `n_embd_v_gqa(il)`
//!   values per cell, in the cache type (`src/llama-kv-cache.cpp`, the
//!   `llama_kv_cache::llama_kv_cache` loop: `ggml_new_tensor_3d(ctx, type_k, n_embd_k_gqa,
//!   kv_size, n_stream)`, `has_v = !is_mla`). With flash attention off the values are
//!   stored transposed and every layer's value rows take the widest layer's size
//!   (`[TAG_V_CACHE_VARIABLE]`: `!v_trans ? n_embd_v_gqa(il) : n_embd_v_gqa_max()`).
//! - `n_embd_k_gqa(il) = n_embd_head_k(il) × n_head_kv(il)`, where a short-memory layer uses
//!   the `_swa` head sizes (`src/llama-hparams.cpp`, `n_embd_head_k`, `n_embd_k_gqa`); the
//!   head sizes are `attention.key_length` / `value_length` (default `n_embd / n_head`) and
//!   `attention.key_length_swa` / `value_length_swa` (default: the full ones), read for every
//!   architecture (`src/llama-model.cpp`, `llama_model_base::load_hparams`).
//! - Layers that keep the whole conversation get `GGML_PAD(n_ctx, 256)` cells
//!   (`src/llama-context.cpp`: `cparams.n_ctx = GGML_PAD(cparams.n_ctx, 256)`, and with one
//!   sequence `n_ctx_seq = n_ctx`). Short-memory (sliding-window) layers live in a second
//!   cache of `GGML_PAD(min(n_ctx, n_swa + n_ubatch), 256)` cells
//!   (`src/llama-kv-cache-iswa.cpp`: `size_swa`; `swa_full` is false without
//!   `--swa-full`), where `n_ubatch = min(n_ctx, 2048, 512)` for llama-server's default
//!   batch sizes (`src/llama-context.cpp`: `n_batch = min(n_ctx, n_batch)`, `n_ubatch =
//!   min(n_batch, n_ubatch)`; `common/common.h`: 2048 and 512).
//! - Which layers are short-memory, and their window `n_swa`, is each architecture's rule
//!   (`src/models/<arch>.cpp`, `load_arch_hparams`): the `attention.sliding_window_pattern`
//!   array when the file has one, else `set_swa_pattern` with the pattern number or the
//!   architecture's own (`src/llama-model.cpp`, `load_swa_pattern`;
//!   `src/llama-hparams.cpp`, `set_swa_pattern`). See [`sliding`].
//! - Some layers keep no memory of their own: the layers of Gemma 3n and Gemma 4 from
//!   `n_layer_kv_from_start` on read an earlier layer's (`src/models/gemma3n.cpp`,
//!   `gemma4.cpp`; skipped in `llama_kv_cache::llama_kv_cache`: `if (!hparams.has_kv(il))`),
//!   the linear-attention layers of the Qwen3-Next and Qwen 3.5 families
//!   (`attention.recurrent_layers` or `full_attention_interval`), extra prediction layers
//!   (`nextn_predict_layers`, filtered out of the main context in `create_memory`), and
//!   every layer of a model without this kind of memory (`create_memory`: `res = nullptr`
//!   for the BERT family, embedding and diffusion models; `llm_arch_is_recurrent`).
//! - A quantized (q8_0) cache also gets small rotation matrices for each layer, in the
//!   same buffer as its memory (`llama_kv_cache::init_rotation`).
//!
//! llama-server also keeps up to 32 saved copies ("context checkpoints") of the
//! short-memory layers in computer memory while a chat goes on, so a cut-off answer can be
//! taken back (`tools/server/server-context.cpp`, `create_checkpoint`;
//! `common/common.h`: `n_ctx_checkpoints = 32`). Each holds the cells of the last `n_swa`
//! positions (`llama_kv_cache::state_write` skips cells the window masks), so they are
//! counted as [`KvSize::checkpoints_at`].
//!
//! Not ported (the estimate stays the plain sum over layers, as in 0.0.6): the
//! architectures with an extra indexer or compressed cache (DeepSeek V3.2 and V4,
//! GLM-DSA and GLM-5 Next, Mistral Large 4, MiniMax M3, HY-V4, dots3note, the speculative
//! drafts DFlash and Gemma 4 assistant), Kimi K3 and Kimi Linear's own head sizes,
//! Nanbeige's looped layers, the recurrent state of hybrid models (tens of megabytes,
//! whatever the context) and its checkpoints.
//!
//! Every number from the header is untrusted: products and sums saturate, a window is cut
//! to llama.cpp's 32 bits, at most [`MAX_BLOCKS`] layers are built and nothing is
//! allocated by a header's word.

use serde::Serialize;

use crate::gguf::{GgufHeader, Value};
use crate::layers::MAX_BLOCKS;
use crate::summary::KvCacheType;

/// Cells are counted in steps of this many (`GGML_PAD(n_ctx, 256)`, and the same for the
/// short-memory cache).
pub const CELL_PAD: u64 = 256;
/// llama-server's batch sizes when no `-b` / `-ub` is given (`common/common.h`).
pub const BATCH: u64 = 2048;
pub const UBATCH: u64 = 512;
/// Saved copies of the short-memory layers llama-server keeps at most per chat
/// (`common/common.h`: `n_ctx_checkpoints = 32`; the app passes no `--ctx-checkpoints`).
pub const CTX_CHECKPOINTS: u64 = 32;
/// llama.cpp keeps the window in 32 bits (`uint32_t n_swa`).
const MAX_WINDOW: u64 = u32::MAX as u64;
/// Bytes of one entry of a rotation matrix (f32).
const ROTATION_ENTRY_BYTES: u64 = 4;

/// The launch words that change the size of the conversation memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KvLayout {
    /// How each value is stored: f16, or q8_0 with Save memory.
    pub cache: KvCacheType,
    /// `-fa off` (Fine control's "Fast attention" off without Save memory): the values are
    /// stored transposed and every layer's value rows take the widest layer's size.
    pub flash_attn_off: bool,
}

impl From<KvCacheType> for KvLayout {
    fn from(cache: KvCacheType) -> Self {
        Self {
            cache,
            flash_attn_off: false,
        }
    }
}

impl KvLayout {
    /// The layout of a launch: Save memory is q8_0 with flash attention on whatever Fine
    /// control says (`build_args`); otherwise f16, transposed when flash attention is off.
    pub fn launch(save_memory: bool, flash_attn_off: bool) -> Self {
        if save_memory {
            KvCacheType::Q8_0.into()
        } else {
            Self {
                cache: KvCacheType::F16,
                flash_attn_off,
            }
        }
    }
}

/// One block's conversation memory, as llama.cpp lays it out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerKv {
    /// Key/value heads of the layer (`n_head_kv(il)`; 0 for a recurrent layer of a hybrid
    /// model).
    pub heads: u64,
    /// Values per head in a key row and in a value row (`n_embd_head_k(il)` /
    /// `n_embd_head_v(il)`: the `_swa` sizes for a short-memory layer).
    pub key_length: u64,
    pub value_length: u64,
    /// A short-memory (sliding-window) layer.
    pub window: bool,
    /// The layer keeps memory of its own (not a linear-attention layer, not one that reads
    /// an earlier layer's, not an extra prediction layer).
    pub own: bool,
}

impl LayerKv {
    fn key_values(&self) -> u64 {
        self.heads.saturating_mul(self.key_length)
    }

    fn value_values(&self) -> u64 {
        self.heads.saturating_mul(self.value_length)
    }
}

/// What a model's conversation memory is made of: one entry per block (at most
/// [`MAX_BLOCKS`]), the window of its short-memory layers and whether values are stored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KvShape {
    pub layers: Vec<LayerKv>,
    /// llama.cpp's `n_swa` in tokens; 0 when no layer is short-memory.
    pub window: u64,
    /// Value rows are stored (false: MLA, keys only).
    pub values: bool,
}

/// Cells of the layers that keep the whole conversation at a context of `ctx` tokens.
pub fn cells(ctx: u64) -> u64 {
    ctx.div_ceil(CELL_PAD).saturating_mul(CELL_PAD)
}

/// Cells of the short-memory layers at `ctx` tokens with a window of `window` tokens.
/// The sum saturates where llama.cpp's 32-bit sum would wrap (a nonsense window): the
/// layers are then counted at full length, the safe side.
pub fn window_cells(window: u64, ctx: u64) -> u64 {
    let ubatch = ctx.min(BATCH).min(UBATCH);
    cells(ctx)
        .min(window.saturating_add(ubatch))
        .div_ceil(CELL_PAD)
        .saturating_mul(CELL_PAD)
}

/// The conversation memory as a function of the context: layers that keep the whole
/// conversation grow with it, short-memory layers stop at their window, and a quantized
/// cache adds rotation matrices of a fixed size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KvSize {
    /// Bytes per cell of the layers that keep the whole conversation ([`cells`]).
    pub per_cell: u64,
    /// Bytes per cell of the short-memory layers ([`window_cells`]).
    pub window_per_cell: u64,
    /// Their window, llama.cpp's `n_swa`.
    pub window: u64,
    /// Bytes that do not depend on the context.
    pub fixed: u64,
    /// Bytes per cell of one saved copy of the short-memory layers (a context checkpoint).
    pub checkpoint_per_cell: u64,
}

impl KvSize {
    /// The memory of a model that keeps every token in every layer, `per_token` bytes a
    /// token (no window, nothing fixed).
    pub fn per_token(per_token: u64) -> Self {
        Self {
            per_cell: per_token,
            ..Self::default()
        }
    }

    /// The conversation memory llama.cpp allocates at a context of `ctx` tokens.
    pub fn at(&self, ctx: u64) -> u64 {
        self.per_cell
            .saturating_mul(cells(ctx))
            .saturating_add(
                self.window_per_cell
                    .saturating_mul(window_cells(self.window, ctx)),
            )
            .saturating_add(self.fixed)
    }

    /// What the saved copies of the short-memory layers take in computer memory at most,
    /// at a context of `ctx` tokens: [`CTX_CHECKPOINTS`] copies of the last `n_swa` cells
    /// (fewer when the context is shorter). 0 without a window.
    pub fn checkpoints_at(&self, ctx: u64) -> u64 {
        CTX_CHECKPOINTS
            .saturating_mul(self.window.min(cells(ctx)))
            .saturating_mul(self.checkpoint_per_cell)
    }
}

impl KvShape {
    /// The widest value row of any layer (`n_embd_v_gqa_max`, over every layer).
    fn widest_value(&self) -> u64 {
        self.layers
            .iter()
            .map(LayerKv::value_values)
            .max()
            .unwrap_or(0)
    }

    /// Bytes of one cell's key row and value row in `layer`.
    fn rows(&self, layer: &LayerKv, layout: KvLayout, widest: u64) -> (u64, u64) {
        let cache = layout.cache;
        let key = cache.row_bytes(layer.key_values());
        let value = if !self.values {
            0
        } else if layout.flash_attn_off {
            cache.row_bytes(widest)
        } else {
            cache.row_bytes(layer.value_values())
        };
        (key, value)
    }

    /// The rotation matrices a quantized cache gives each layer of one of its two caches
    /// (`llama_kv_cache` constructor and `init_rotation`): when every layer of that cache
    /// has the same key head size, a key matrix of `n_rot_k²` entries, `n_rot_k` the
    /// largest power of two from 64 that divides it; when they share a value head size, a
    /// value matrix of 64². Both need layer 0's head size (whatever cache it is in) to be a
    /// multiple of 64. f16 has none.
    fn rotation(&self, layout: KvLayout, window: bool) -> (u64, u64) {
        if layout.cache == KvCacheType::F16 {
            return (0, 0);
        }
        let in_cache = || {
            self.layers
                .iter()
                .filter(move |l| l.own && l.window == window)
        };
        let common = |size: fn(&LayerKv) -> u64| {
            let mut sizes = in_cache().map(size);
            let first = sizes.next()?;
            (first > 0 && sizes.all(|s| s == first)).then_some(first)
        };
        let first_layer = self.layers.first().copied().unwrap_or_default();
        let key = common(|l| l.key_length)
            .filter(|_| first_layer.key_length.is_multiple_of(64))
            .map_or(0, |head| {
                let mut n_rot = 64u64;
                while let Some(next) = n_rot.checked_mul(2) {
                    if !head.is_multiple_of(next) {
                        break;
                    }
                    n_rot = next;
                }
                n_rot.saturating_mul(n_rot).saturating_mul(ROTATION_ENTRY_BYTES)
            });
        // Every layer of the cache has a value tensor then (none with MLA).
        let value = common(|l| l.value_length)
            .filter(|_| self.values && first_layer.value_length.is_multiple_of(64))
            .map_or(0, |_| 64 * 64 * ROTATION_ENTRY_BYTES);
        (key, value)
    }

    /// The conversation memory of each block at a context of `ctx` tokens, as llama.cpp
    /// allocates it (one entry per block; 0 for a block without memory of its own).
    pub fn per_layer(&self, ctx: u64, layout: KvLayout) -> Vec<u64> {
        let widest = self.widest_value();
        let full_cells = cells(ctx);
        let short_cells = window_cells(self.window, ctx);
        let rotation = [self.rotation(layout, false), self.rotation(layout, true)];
        self.layers
            .iter()
            .map(|layer| {
                if !layer.own {
                    return 0;
                }
                let (key, value) = self.rows(layer, layout, widest);
                let cells = if layer.window {
                    short_cells
                } else {
                    full_cells
                };
                let (rot_key, rot_value) = rotation[usize::from(layer.window)];
                key.saturating_add(value)
                    .saturating_mul(cells)
                    .saturating_add(rot_key)
                    .saturating_add(rot_value)
            })
            .collect()
    }

    /// The whole conversation memory at a context of `ctx` tokens.
    pub fn total(&self, ctx: u64, layout: KvLayout) -> u64 {
        self.per_layer(ctx, layout)
            .into_iter()
            .fold(0, u64::saturating_add)
    }

    /// The same memory as a function of the context ([`KvSize::at`] equals
    /// [`Self::total`] at every context).
    pub fn size(&self, layout: KvLayout) -> KvSize {
        let widest = self.widest_value();
        let rotation = [self.rotation(layout, false), self.rotation(layout, true)];
        let mut size = KvSize {
            window: self.window,
            ..KvSize::default()
        };
        for layer in self.layers.iter().filter(|l| l.own) {
            let (key, value) = self.rows(layer, layout, widest);
            let row = key.saturating_add(value);
            let (rot_key, rot_value) = rotation[usize::from(layer.window)];
            size.fixed = size
                .fixed
                .saturating_add(rot_key)
                .saturating_add(rot_value);
            if layer.window {
                size.window_per_cell = size.window_per_cell.saturating_add(row);
                // A saved copy holds each layer's own rows (`state_write_data`).
                let own_value = if self.values {
                    layout.cache.row_bytes(layer.value_values())
                } else {
                    0
                };
                size.checkpoint_per_cell = size
                    .checkpoint_per_cell
                    .saturating_add(key.saturating_add(own_value));
            } else {
                size.per_cell = size.per_cell.saturating_add(row);
            }
        }
        size
    }
}

// ---- What the header says, by llama.cpp's rules for each architecture -------------------

/// The conversation-memory facts of a header, read by llama.cpp's rules for its
/// architecture. One entry per block in each list, at most [`MAX_BLOCKS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ArchKv {
    /// llama.cpp's `n_swa`; 0 when no layer is short-memory.
    pub window: u64,
    /// Which layers are short-memory (empty when none is).
    pub swa_layers: Vec<bool>,
    /// Which layers keep memory of their own.
    pub kv_layers: Vec<bool>,
    /// Value rows are stored (false for MLA).
    pub values: bool,
}

/// Architectures whose llama.cpp context has no memory of this kind (`create_memory`:
/// `res = nullptr`; `llm_arch_is_recurrent`: a recurrent state instead).
const NO_MEMORY: &[&str] = &[
    "bert",
    "jina-bert-v2",
    "jina-bert-v3",
    "nomic-bert",
    "nomic-bert-moe",
    "neo-bert",
    "eurobert",
    "wavtokenizer-dec",
    "modern-bert",
    "gemma-embedding",
    "gemma-embedding2",
    "dream",
    "llada",
    "llada-moe",
    "rnd1",
    "clef",
    "mamba",
    "mamba2",
    "rwkv6",
    "rwkv6qwen2",
    "rwkv7",
    "arwkv7",
];

/// Hybrid architectures (`llm_arch_is_hybrid`).
const HYBRID: &[&str] = &[
    "jamba",
    "falcon-h1",
    "plamo2",
    "granitehybrid",
    "lfm2",
    "lfm2moe",
    "lfm3",
    "nemotron_h",
    "nemotron_h_moe",
    "metis16",
    "qwen3next",
    "kimi-linear",
    "bailingmoe3",
    "kimi-k3",
    "glm5-next",
    "qwen35",
    "qwen35moe",
    "clover_moe",
    "qwen4exp",
    "deepseek4",
    "deepseek41",
    "minimax-01",
];

/// Hybrid architectures whose linear-attention layers are set by
/// `attention.recurrent_layers` or `full_attention_interval` (with this default; `None`:
/// the file must say) rather than by a head count of 0, and whose attention filter leaves
/// out the extra prediction layers (`create_memory`: `il < hparams.n_layer() &&
/// !hparams.is_recr(il)`).
fn interval_default(arch: &str) -> Option<Option<u64>> {
    Some(match arch {
        "qwen3next" | "qwen35" | "qwen35moe" | "qwen4exp" => Some(4),
        "minimax-01" => Some(8),
        "clover_moe" => None,
        _ => return None,
    })
}

/// Whether a value of a flag array is set (llama.cpp copies booleans as 0/1 and integers
/// as they are; other types refuse to load).
fn truthy(value: &Value) -> bool {
    match *value {
        Value::Bool(b) => b,
        Value::U64(n) => n != 0,
        Value::I64(n) => n != 0,
        _ => false,
    }
}

/// `n` flags from an array value: entries past the array's end are clear, extra entries
/// are not used (`llama_model_loader::get_arr` copies the array over cleared flags).
fn flags(items: &[Value], n: usize) -> Vec<bool> {
    (0..n).map(|il| items.get(il).is_some_and(truthy)).collect()
}

/// `set_swa_pattern(n_pattern, dense_first)` (`src/llama-hparams.cpp`) for `blocks`
/// entries, of which the first `main_layers` follow the pattern: every `n_pattern`th layer
/// keeps the whole conversation (the first of each group with `dense_first`, else the
/// last); 0 makes every layer short-memory. The extra prediction layers are full.
fn set_swa_pattern(blocks: usize, main_layers: u64, n_pattern: u64, dense_first: bool) -> Vec<bool> {
    (0..blocks)
        .map(|il| {
            let il = u64::try_from(il).unwrap_or(u64::MAX);
            il < main_layers
                && (n_pattern == 0
                    || if dense_first {
                        il % n_pattern != 0
                    } else {
                        il % n_pattern < n_pattern - 1
                    })
        })
        .collect()
}

struct Reader<'a> {
    h: &'a GgufHeader,
    arch: &'a str,
    blocks: usize,
    block_count: u64,
    /// `n_layer()`: the blocks less the extra prediction layers.
    main_layers: u64,
}

impl Reader<'_> {
    fn get(&self, suffix: &str) -> Option<&Value> {
        self.h.get(&format!("{}.{suffix}", self.arch))
    }

    fn number(&self, suffix: &str) -> Option<u64> {
        self.get(suffix).and_then(Value::as_u64)
    }

    /// `attention.sliding_window`, when the file has it.
    fn window(&self) -> Option<u64> {
        self.number("attention.sliding_window")
    }

    /// `load_swa_pattern(ml, n_pattern, dense_first)`: the pattern array, else the
    /// pattern number from the file, else `n_pattern`.
    fn pattern(&self, n_pattern: u64, dense_first: bool) -> Vec<bool> {
        match self.get("attention.sliding_window_pattern") {
            Some(Value::Array(items)) => flags(items, self.blocks),
            // Longer than any model (llama.cpp refuses more than 512 entries).
            Some(Value::SkippedArray { .. }) => vec![false; self.blocks],
            Some(v) => set_swa_pattern(
                self.blocks,
                self.main_layers,
                v.as_u64().unwrap_or(n_pattern),
                dense_first,
            ),
            None => set_swa_pattern(self.blocks, self.main_layers, n_pattern, dense_first),
        }
    }

    /// The pattern array an architecture requires (`ml.get_arr(...)` without `false`);
    /// `None` without one (llama.cpp would not load the file).
    fn pattern_array(&self) -> Option<Vec<bool>> {
        match self.get("attention.sliding_window_pattern") {
            Some(Value::Array(items)) => Some(flags(items, self.blocks)),
            _ => None,
        }
    }

    /// The pattern array with exactly one entry per block and at least one short-memory
    /// layer (Beam and Maion-Coder refuse anything else).
    fn pattern_array_per_block(&self) -> Option<Vec<bool>> {
        match self.get("attention.sliding_window_pattern") {
            Some(Value::Array(items)) if u64::try_from(items.len()).ok() == Some(self.block_count) => {
                let layers = flags(items, self.blocks);
                layers.contains(&true).then_some(layers)
            }
            _ => None,
        }
    }
}

/// The window and short-memory layers llama.cpp gives an architecture
/// (`src/models/<arch>.cpp`, `load_arch_hparams`); `None` when it gives none: an
/// architecture without them, one whose window is read but not used (Phi-3, EXAONE 4 but
/// the 32B), a file without the window an architecture needs to turn them on, or a file
/// llama.cpp would refuse (a required window or pattern missing). `head_count_kv` is per
/// block (for LFM2, whose attention layers are its short-memory layers).
fn sliding(r: &Reader, head_count_kv: &[u64]) -> Option<(u64, Vec<bool>)> {
    let window = r.window();
    let positive = window.filter(|&w| w > 0);
    let (window, layers) = match r.arch {
        // A default window the file may change (gemma2.cpp, exaone4.cpp: only the 32B).
        "gemma2" => (window.unwrap_or(4096), r.pattern(2, false)),
        "exaone4" if r.main_layers == 64 => (window.unwrap_or(4096), r.pattern(4, false)),
        // Short-memory layers only with a window above 0 in the file.
        "gemma3" => (positive?, r.pattern(6, false)),
        "afmoe" | "mellum" | "olmo2" => (positive?, r.pattern(4, false)),
        "laguna" => (positive?, r.pattern(4, true)),
        "plamo3" => (positive?, r.pattern(8, false)),
        // smallthinker.cpp: a window in the file turns them on, at 4,096 tokens.
        "smallthinker" => {
            positive?;
            (4096, r.pattern(4, true))
        }
        // llama4.cpp: chunks of 8,192 tokens, unless the file's window is 0.
        "llama4" => {
            if window == Some(0) {
                return None;
            }
            (8192, r.pattern(4, false))
        }
        // The window is required; the pattern may come from the file.
        "gemma3n" => (window?, r.pattern(5, false)),
        "gpt-oss" => (window?, r.pattern(2, false)),
        "cohere2" | "limite" | "muse-glimmer" | "exaone-moe" => (window?, r.pattern(4, false)),
        "cohere2moe" => (window?, r.pattern(4, true)),
        // The window and the pattern array are required.
        "gemma4" | "flint" | "granite_swa" | "maple" | "mimo2" | "muse-spark" | "spark2_5"
        | "step35" => (window?, r.pattern_array()?),
        "kolibri1" => (positive?, r.pattern_array()?),
        "lfm3" => {
            let layers = r.pattern_array()?;
            layers.contains(&true).then_some(())?;
            (window?, layers)
        }
        "beam" | "maion-coder" => (positive?, r.pattern_array_per_block()?),
        // lfm2.cpp: with a window, every attention layer is short-memory.
        "lfm2" => {
            let main = r.main_layers;
            let layers = (0..r.blocks)
                .map(|il| {
                    u64::try_from(il).is_ok_and(|i| i < main)
                        && head_count_kv.get(il).is_some_and(|&n| n > 0)
                })
                .collect();
            (positive?, layers)
        }
        _ => return None,
    };
    Some((window.min(MAX_WINDOW), layers))
}

/// Reads the conversation-memory facts of a header for architecture `arch` with
/// `block_count` blocks and the per-block KV head counts `head_count_kv`.
pub(crate) fn read_arch(
    h: &GgufHeader,
    arch: &str,
    block_count: u64,
    head_count_kv: &[u64],
) -> ArchKv {
    let blocks = usize::try_from(block_count.min(MAX_BLOCKS)).unwrap_or(0);
    let nextn = h
        .get_u64(&format!("{arch}.nextn_predict_layers"))
        .unwrap_or(0);
    let r = Reader {
        h,
        arch,
        blocks,
        block_count,
        main_layers: block_count.saturating_sub(nextn),
    };
    let (window, swa_layers) = sliding(&r, head_count_kv).unwrap_or_default();
    let mut kv_layers = vec![!NO_MEMORY.contains(&arch); blocks];
    let mut keep_from = |first: u64| {
        let first = usize::try_from(first).unwrap_or(usize::MAX);
        for own in kv_layers.iter_mut().skip(first) {
            *own = false;
        }
    };
    // Layers from `n_layer_kv_from_start` on read an earlier layer's memory.
    match arch {
        "gemma3n" => keep_from(20),
        "gemma4" => keep_from(block_count.saturating_sub(
            r.number("attention.shared_kv_layers").unwrap_or(0),
        )),
        _ => {}
    }
    // A hybrid model's attention cache holds only its attention layers (`create_memory`'s
    // layer filters): a layer without key/value heads is a recurrent (or, in Nemotron-H, a
    // feed-forward) layer.
    let hybrid = HYBRID.contains(&arch);
    if hybrid {
        for (il, own) in kv_layers.iter_mut().enumerate() {
            if head_count_kv.get(il).is_none_or(|&n| n == 0) {
                *own = false;
            }
        }
    }
    // Extra prediction layers are left out of the main context (`create_memory`), except
    // where Granite Switch uses the count for its router layer.
    if nextn > 0 && r.main_layers > 0 && arch != "graniteswitch" && (!hybrid || interval_default(arch).is_some()) {
        keep_from(r.main_layers);
    }
    // Linear-attention layers of the Qwen3-Next / Qwen 3.5 kind.
    if let Some(default) = interval_default(arch) {
        let recurrent: Option<Vec<bool>> = match r.get("attention.recurrent_layers") {
            Some(Value::Array(items)) => Some(flags(items, blocks)),
            Some(Value::SkippedArray { .. }) => None,
            Some(v) => Some(vec![truthy(v); blocks]),
            None => r
                .number("full_attention_interval")
                .or(default)
                // 0 would divide by zero; such a file is not believed.
                .filter(|&n| n > 0)
                .map(|n| {
                    (0..blocks)
                        .map(|il| {
                            let il = u64::try_from(il).unwrap_or(u64::MAX);
                            (arch == "clover_moe" || il < r.main_layers)
                                && !(il.saturating_add(1)).is_multiple_of(n)
                        })
                        .collect()
                }),
        };
        for (own, recurrent) in kv_layers.iter_mut().zip(recurrent.unwrap_or_default()) {
            if recurrent {
                *own = false;
            }
        }
    }
    // MLA keeps keys only (`has_v = !is_mla`): both MLA head sizes in the file.
    let mla = matches!(arch, "deepseek2" | "bailingmoe3")
        && r.number("attention.key_length_mla").is_some_and(|n| n > 0)
        && r.number("attention.value_length_mla").is_some_and(|n| n > 0);
    ArchKv {
        window,
        swa_layers,
        kv_layers,
        values: !mla,
    }
}
