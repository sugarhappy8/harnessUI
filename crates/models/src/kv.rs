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
    /// A model all of whose `layers` layers keep the whole conversation, each with
    /// `heads` key/value heads of `head` values.
    pub fn plain(layers: usize, heads: u64, head: u64) -> Self {
        Self {
            layers: vec![
                LayerKv {
                    heads,
                    key_length: head,
                    value_length: head,
                    window: false,
                    own: true,
                };
                layers
            ],
            window: 0,
            values: true,
        }
    }

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
    /// has the same key head size, a key matrix of `n_rot_k²` f32 entries, `n_rot_k`
    /// starting at 64 and doubled while twice it divides that size; when they share a value
    /// head size, a value matrix of 64². Both need layer 0's head size (whatever cache it is
    /// in) to be a multiple of 64. f16 has none.
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
                n_rot
                    .saturating_mul(n_rot)
                    .saturating_mul(ROTATION_ENTRY_BYTES)
            });
        // Every layer of the cache has a value tensor then (none with MLA).
        let value = common(|l| l.value_length)
            .filter(|_| self.values && first_layer.value_length.is_multiple_of(64))
            .map_or(0, |_| 64 * 64 * ROTATION_ENTRY_BYTES);
        (key, value)
    }

    /// The conversation memory of each block at a context of `ctx` tokens, as llama.cpp
    /// allocates it (one entry per block; 0 for a block without memory of its own).
    pub fn per_layer(&self, ctx: u64, layout: impl Into<KvLayout>) -> Vec<u64> {
        let layout = layout.into();
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
    pub fn total(&self, ctx: u64, layout: impl Into<KvLayout>) -> u64 {
        self.per_layer(ctx, layout)
            .into_iter()
            .fold(0, u64::saturating_add)
    }

    /// The same memory as a function of the context ([`KvSize::at`] equals
    /// [`Self::total`] at every context).
    pub fn size(&self, layout: impl Into<KvLayout>) -> KvSize {
        let layout = layout.into();
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
            size.fixed = size.fixed.saturating_add(rot_key).saturating_add(rot_value);
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
fn set_swa_pattern(
    blocks: usize,
    main_layers: u64,
    n_pattern: u64,
    dense_first: bool,
) -> Vec<bool> {
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
            Some(Value::Array(items))
                if u64::try_from(items.len()).ok() == Some(self.block_count) =>
            {
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

/// Clears the flags of the layers from `first` on.
fn none_from(kv_layers: &mut [bool], first: u64) {
    let first = usize::try_from(first).unwrap_or(usize::MAX);
    for own in kv_layers.iter_mut().skip(first) {
        *own = false;
    }
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
    // Layers from `n_layer_kv_from_start` on read an earlier layer's memory.
    match arch {
        "gemma3n" => none_from(&mut kv_layers, 20),
        "gemma4" => none_from(
            &mut kv_layers,
            block_count.saturating_sub(r.number("attention.shared_kv_layers").unwrap_or(0)),
        ),
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
    if nextn > 0
        && r.main_layers > 0
        && arch != "graniteswitch"
        && (!hybrid || interval_default(arch).is_some())
    {
        none_from(&mut kv_layers, r.main_layers);
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
        && r.number("attention.value_length_mla")
            .is_some_and(|n| n > 0);
    ArchKv {
        window,
        swa_layers,
        kv_layers,
        values: !mla,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gguf::testing::Writer;
    use crate::summary::ModelSummary;

    const MIB: u64 = 1024 * 1024;
    const F16: KvCacheType = KvCacheType::F16;
    const Q8: KvCacheType = KvCacheType::Q8_0;

    fn summary(w: Writer) -> ModelSummary {
        let bytes = w.build();
        let len = bytes.len() as u64;
        ModelSummary::from_header(&crate::gguf::read(std::io::Cursor::new(bytes), len).unwrap())
    }

    /// The header of `gemma-4-12B-coder-fable5-composer2.5-v1-uncensored-heretic`
    /// (Q4_K_M, read from Hugging Face): 48 layers in groups of five short-memory layers
    /// (8 KV heads of 256) and one full layer (1 KV head of 512), a window of 1,024.
    fn gemma4_12b() -> Writer {
        gemma4_12b_except(&[])
    }

    /// [`gemma4_12b`] without the keys in `skip` (so a test can write its own).
    fn gemma4_12b_except(skip: &[&str]) -> Writer {
        let pattern: Vec<bool> = (0..48).map(|il| il % 6 != 5).collect();
        let heads: Vec<u32> = (0..48).map(|il| if il % 6 == 5 { 1 } else { 8 }).collect();
        let mut w = Writer::new().str("general.architecture", "gemma4");
        for (key, value) in [
            ("block_count", 48),
            ("context_length", 262_144),
            ("embedding_length", 3840),
            ("attention.head_count", 16),
            ("attention.key_length", 512),
            ("attention.value_length", 512),
            ("attention.sliding_window", 1024),
            ("attention.shared_kv_layers", 0),
            ("attention.key_length_swa", 256),
            ("attention.value_length_swa", 256),
            ("rope.dimension_count", 512),
            ("rope.dimension_count_swa", 256),
        ] {
            if !skip.contains(&key) {
                w = w.u32(&format!("gemma4.{key}"), value);
            }
        }
        if !skip.contains(&"attention.head_count_kv") {
            w = w.u32_array("gemma4.attention.head_count_kv", &heads);
        }
        if !skip.contains(&"attention.sliding_window_pattern") {
            w = w.bool_array("gemma4.attention.sliding_window_pattern", &pattern);
        }
        w
    }

    /// The owner's model, against what llama.cpp b12110 allocates for this header
    /// (`llama-fit-params -c N -np 1 --device none -v` on the file's header: "llama_kv_cache:
    /// size = ..." for its full and its short-memory cache).
    #[test]
    fn gemma4_12b_is_counted_as_llama_cpp_allocates_it() {
        let s = summary(gemma4_12b());
        assert_eq!(s.sliding_window, 1024);
        assert_eq!(s.swa_layers.iter().filter(|&&w| w).count(), 40);
        assert_eq!((s.key_length_swa, s.value_length_swa), (256, 256));
        assert_eq!(s.kv_layers, vec![true; 48]);
        // Full layers: 8 × 1 head × (512 + 512) values × 2 bytes = 16,384 bytes a cell.
        // Short-memory layers: 40 × 8 heads × (256 + 256) × 2 = 327,680 bytes a cell, for
        // GGML_PAD(min(ctx, 1,024 + 512), 256) = 1,536 cells: 503,316,480 bytes (480 MiB).
        let size = s.kv_size(F16);
        assert_eq!(
            size,
            KvSize {
                per_cell: 16_384,
                window_per_cell: 327_680,
                window: 1024,
                fixed: 0,
                checkpoint_per_cell: 327_680,
            }
        );
        // 65,536 tokens: 16,384 × 65,536 + 327,680 × 1,536 = 1,073,741,824 + 503,316,480
        // = 1,577,058,304 bytes (1,024 + 480 = 1,504 MiB, llama.cpp's figure), not the
        // 671,744 × 65,536 = 44,023,414,784 of 0.0.6.
        assert_eq!(s.kv_bytes(65_536, F16), 1_577_058_304);
        assert_eq!(s.kv_bytes(65_536, F16), (1024 + 480) * MIB);
        // 4,096 tokens: 16,384 × 4,096 + 503,316,480 = 570,425,344 (64 + 480 MiB).
        assert_eq!(s.kv_bytes(4096, F16), 570_425_344);
        assert_eq!(s.kv_bytes(4096, F16), (64 + 480) * MIB);
        // 262,144 tokens (the trained length): 16,384 × 262,144 + 503,316,480 =
        // 4,798,283,776 (4,096 + 480 MiB).
        assert_eq!(s.kv_bytes(262_144, F16), 4_798_283_776);
        assert_eq!(s.kv_bytes(262_144, F16), (4096 + 480) * MIB);
        // Layer by layer at 65,536: a short-memory layer 8 × 512 × 2 × 1,536 = 12,582,912,
        // a full layer 1 × 1,024 × 2 × 65,536 = 134,217,728.
        let per = crate::placement::kv_per_layer(&s, 65_536, F16);
        assert_eq!(per.len(), 48);
        for (il, bytes) in per.iter().enumerate() {
            let want = if il % 6 == 5 { 134_217_728 } else { 12_582_912 };
            assert_eq!(*bytes, want, "layer {il}");
        }
        assert_eq!(per.iter().sum::<u64>(), 1_577_058_304);
    }

    /// Save memory (q8_0) and flash attention off, against llama.cpp's figures for the
    /// same header.
    #[test]
    fn gemma4_12b_with_save_memory_and_without_flash_attention() {
        let s = summary(gemma4_12b());
        // q8_0: 32 values in 34 bytes. Full: 8 × (544 + 544) × 65,536 = 570,425,344 (544
        // MiB); short: 40 × (2,176 + 2,176) × 1,536 = 267,386,880 (255 MiB). Rotation
        // matrices: full layers (heads of 512) 512² + 64² f32 = 1,064,960 bytes each, short
        // layers (256) 256² + 64² = 278,528 each: 8,519,680 + 11,141,120 = 19,660,800
        // (18.75 MiB). llama.cpp reports 817 MiB of context: 544 + 255 + 18.75 = 817.75.
        assert_eq!(s.kv_bytes(65_536, Q8), 857_473_024);
        assert_eq!(
            s.kv_bytes(65_536, Q8),
            570_425_344 + 267_386_880 + 19_660_800
        );
        assert_eq!(s.kv_size(Q8).fixed, 19_660_800);
        // `-fa off`: the full layers' value rows are padded to the widest (8 × 256 =
        // 2,048 values): 8 × (1,024 + 4,096) × 65,536 = 2,684,354,560 (2,560 MiB) plus the
        // short layers' 480 MiB, llama.cpp's 3,040 MiB.
        let off = KvLayout::launch(false, true);
        assert_eq!(s.kv_bytes(65_536, off), (2560 + 480) * MIB);
        // Save memory turns flash attention on whatever Fine control says.
        assert_eq!(KvLayout::launch(true, true), KvLayout::from(Q8));
        assert_eq!(KvLayout::launch(false, false), KvLayout::from(F16));
    }

    /// The saved copies llama-server keeps of the short-memory layers: 32 of 1,024 cells
    /// of 327,680 bytes, 10,737,418,240 bytes (10 GiB) at most in a long chat.
    #[test]
    fn gemma4_12b_checkpoints() {
        let size = summary(gemma4_12b()).kv_size(F16);
        assert_eq!(size.checkpoints_at(65_536), 32 * 1024 * 327_680);
        assert_eq!(size.checkpoints_at(65_536), 10 * 1024 * MIB);
        // A context shorter than the window holds fewer cells.
        assert_eq!(size.checkpoints_at(512), 32 * 512 * 327_680);
        // q8_0 copies are q8_0 rows: 40 × 2 × 2,176 bytes a cell.
        let q8 = summary(gemma4_12b()).kv_size(Q8);
        assert_eq!(q8.checkpoints_at(65_536), 32 * 1024 * 40 * 2 * 2176);
        // A model without short-memory layers keeps none.
        assert_eq!(KvSize::per_token(131_072).checkpoints_at(65_536), 0);
    }

    /// Gemma 3 270M (`ggml-org/gemma-3-270m-it-GGUF`, Q8_0): no pattern array, so
    /// llama.cpp's fixed pattern of 6 (five short-memory layers, then a full one), a window
    /// of 512, one KV head of 256. Checked against the real llama-server (b12110):
    /// `-c 4096`: 12 + 15 MiB; `-c 32768`: 96 + 15 MiB; `-c 512`: 1.5 + 7.5 MiB; `-c 4096
    /// -fa on -ctk q8_0 -ctv q8_0`: 6.38 + 7.97 MiB and rotation buffers of 0.80 + 3.98
    /// MiB.
    fn gemma3_270m() -> Writer {
        gemma3_270m_except(&[])
    }

    fn gemma3_270m_except(skip: &[&str]) -> Writer {
        let mut w = Writer::new().str("general.architecture", "gemma3");
        for (key, value) in [
            ("context_length", 32_768),
            ("embedding_length", 640),
            ("block_count", 18),
            ("attention.head_count", 4),
            ("attention.key_length", 256),
            ("attention.value_length", 256),
            ("attention.sliding_window", 512),
            ("attention.head_count_kv", 1),
        ] {
            if !skip.contains(&key) {
                w = w.u32(&format!("gemma3.{key}"), value);
            }
        }
        w
    }

    #[test]
    fn gemma3_uses_llama_cpps_fixed_pattern() {
        let s = summary(gemma3_270m());
        let want: Vec<bool> = (0..18).map(|il| il % 6 < 5).collect();
        assert_eq!(s.swa_layers, want);
        assert_eq!(s.sliding_window, 512);
        // 3 full layers × 1,024 bytes a cell (1 head × 512 values × 2 bytes) × 4,096 cells
        // = 12 MiB; 15 short layers × 1,024 × GGML_PAD(min(4,096, 512 + 512), 256) = 1,024
        // cells = 15 MiB.
        assert_eq!(s.kv_bytes(4096, F16), 27 * MIB);
        assert_eq!(s.kv_bytes(32_768, F16), (96 + 15) * MIB);
        // 512 tokens: 512 cells everywhere: 1.5 + 7.5 MiB.
        assert_eq!(s.kv_bytes(512, F16), 9 * MIB);
        // q8_0: 3 × 544 × 4,096 + 15 × 544 × 1,024 = 6,684,672 + 8,355,840, and rotation
        // matrices of 256² + 64² f32 entries for each of the 18 layers (835,584 + 4,177,920).
        assert_eq!(
            s.kv_bytes(4096, Q8),
            6_684_672 + 8_355_840 + 835_584 + 4_177_920
        );
        // A saved copy: 512 cells × 15 layers × 1,024 bytes = 7,864,320 (llama-server
        // logged "size = 7.506 MiB" with its few bytes a cell of positions), 32 at most.
        assert_eq!(s.kv_size(F16).checkpoints_at(16_384), 32 * 7_864_320);
    }

    /// Without the window llama.cpp's Gemma 3 has no short-memory layers; Phi-3 reads the
    /// window but does not use it; EXAONE 4 uses one only in the 32B (64 layers); Llama 4
    /// uses chunks of 8,192 unless the file says 0; SmallThinker always 4,096.
    #[test]
    fn architecture_rules() {
        let plain = |arch: &str, blocks: u32, extra: fn(Writer, &str) -> Writer| {
            summary(extra(
                Writer::new()
                    .str("general.architecture", arch)
                    .u32(&format!("{arch}.block_count"), blocks)
                    .u32(&format!("{arch}.embedding_length"), 2048)
                    .u32(&format!("{arch}.attention.head_count"), 16)
                    .u32(&format!("{arch}.attention.head_count_kv"), 4),
                arch,
            ))
        };
        let window =
            |w: Writer, arch: &str| w.u32(&format!("{arch}.attention.sliding_window"), 4096);
        let zero = |w: Writer, arch: &str| w.u32(&format!("{arch}.attention.sliding_window"), 0);
        let none = |w: Writer, _: &str| w;
        assert_eq!(plain("gemma3", 26, none).sliding_window, 0);
        assert!(plain("gemma3", 26, zero).swa_layers.is_empty());
        assert_eq!(plain("gemma3", 26, window).sliding_window, 4096);
        assert!(plain("phi3", 32, window).swa_layers.is_empty());
        assert!(plain("llama", 32, window).swa_layers.is_empty());
        assert!(plain("qwen2", 32, window).swa_layers.is_empty());
        assert!(plain("exaone4", 30, window).swa_layers.is_empty());
        let exaone = plain("exaone4", 64, none);
        assert_eq!(exaone.sliding_window, 4096);
        assert_eq!(exaone.swa_layers.iter().filter(|&&w| w).count(), 48);
        // Gemma 2: a window of 4,096 by default and every other layer short-memory.
        let gemma2 = plain("gemma2", 42, none);
        assert_eq!(gemma2.sliding_window, 4096);
        assert_eq!(
            gemma2.swa_layers,
            (0..42).map(|il| il % 2 == 0).collect::<Vec<_>>()
        );
        // Llama 4: chunked attention, 3 chunked layers then a full one.
        let llama4 = plain("llama4", 48, none);
        assert_eq!(llama4.sliding_window, 8192);
        assert_eq!(
            llama4.swa_layers,
            (0..48).map(|il| il % 4 < 3).collect::<Vec<_>>()
        );
        assert!(plain("llama4", 48, zero).swa_layers.is_empty());
        let thinker = plain("smallthinker", 32, |w, a| {
            w.u32(&format!("{a}.attention.sliding_window"), 1)
        });
        assert_eq!(thinker.sliding_window, 4096);
        // dense_first: the first of each group of four keeps the whole conversation.
        assert_eq!(
            thinker.swa_layers,
            (0..32).map(|il| il % 4 != 0).collect::<Vec<_>>()
        );
        // Cohere 2: the window is required, groups of four ending with a full layer.
        assert!(plain("cohere2", 32, none).swa_layers.is_empty());
        assert_eq!(
            plain("cohere2", 32, window).swa_layers,
            (0..32).map(|il| il % 4 < 3).collect::<Vec<_>>()
        );
        // Gemma 4 needs its pattern array.
        assert!(plain("gemma4", 48, window).swa_layers.is_empty());
        // A pattern number in the file replaces the architecture's.
        let numbered = plain("gemma3", 12, |w, a| {
            w.u32(&format!("{a}.attention.sliding_window"), 512)
                .u32(&format!("{a}.attention.sliding_window_pattern"), 3)
        });
        assert_eq!(
            numbered.swa_layers,
            (0..12).map(|il| il % 3 < 2).collect::<Vec<_>>()
        );
    }

    /// gpt-oss-20b (`ggml-org/gpt-oss-20b-GGUF`): every other layer looks back 128
    /// tokens. llama.cpp: 96 + 18 MiB at 4,096 tokens, 768 + 18 MiB at 32,768.
    #[test]
    fn gpt_oss_alternates_short_and_full_layers() {
        let s = summary(
            Writer::new()
                .str("general.architecture", "gpt-oss")
                .u32("gpt-oss.block_count", 24)
                .u32("gpt-oss.context_length", 131_072)
                .u32("gpt-oss.embedding_length", 2880)
                .u32("gpt-oss.attention.head_count", 64)
                .u32("gpt-oss.attention.head_count_kv", 8)
                .u32("gpt-oss.attention.key_length", 64)
                .u32("gpt-oss.attention.value_length", 64)
                .u32("gpt-oss.attention.sliding_window", 128)
                .u32("gpt-oss.expert_count", 32),
        );
        assert_eq!(
            s.swa_layers,
            (0..24).map(|il| il % 2 == 0).collect::<Vec<_>>()
        );
        // 12 full layers × 8 × 128 × 2 = 2,048 bytes a cell; 12 short layers the same for
        // GGML_PAD(128 + 512, 256) = 768 cells (18 MiB).
        assert_eq!(s.kv_bytes(4096, F16), (96 + 18) * MIB);
        assert_eq!(s.kv_bytes(32_768, F16), (768 + 18) * MIB);
    }

    /// Gemma 4 E2B (`ggml-org/gemma-4-E2B-it-GGUF`): the last 20 of its 35 layers read
    /// the memory of earlier ones (`shared_kv_layers`), so 3 full and 12 short-memory
    /// layers hold memory. llama.cpp: 24 + 12 MiB at 4,096 tokens, 192 + 12 at 32,768.
    #[test]
    fn gemma4_shared_layers_hold_nothing() {
        let pattern: Vec<bool> = (0..35).map(|il| il % 5 != 4).collect();
        let s = summary(
            Writer::new()
                .str("general.architecture", "gemma4")
                .u32("gemma4.block_count", 35)
                .u32("gemma4.embedding_length", 1536)
                .u32("gemma4.attention.head_count", 8)
                .u32("gemma4.attention.head_count_kv", 1)
                .u32("gemma4.attention.key_length", 512)
                .u32("gemma4.attention.value_length", 512)
                .u32("gemma4.attention.sliding_window", 512)
                .u32("gemma4.attention.shared_kv_layers", 20)
                .bool_array("gemma4.attention.sliding_window_pattern", &pattern)
                .u32("gemma4.attention.key_length_swa", 256)
                .u32("gemma4.attention.value_length_swa", 256),
        );
        let own: Vec<bool> = (0..35).map(|il| il < 15).collect();
        assert_eq!(s.kv_layers, own);
        // 3 full layers × 2,048 bytes a cell × 4,096 = 24 MiB; 12 short layers × 1,024 ×
        // 1,024 cells = 12 MiB.
        assert_eq!(s.kv_bytes(4096, F16), 36 * MIB);
        assert_eq!(s.kv_bytes(32_768, F16), (192 + 12) * MIB);
        let per = crate::placement::kv_per_layer(&s, 4096, F16);
        assert!(per[15..].iter().all(|&b| b == 0));
    }

    /// Gemma 3n E2B (`ggml-org/gemma-3n-E2B-it-GGUF`): layers from the 20th on share
    /// earlier layers' memory, whatever `shared_kv_layers` says (here the float 10.0).
    /// llama.cpp: 32 + 32 MiB at 4,096 tokens, 256 + 32 at 32,768.
    #[test]
    fn gemma3n_shares_from_layer_20() {
        let pattern: Vec<bool> = (0..30).map(|il| il % 5 != 4).collect();
        let s = summary(
            Writer::new()
                .str("general.architecture", "gemma3n")
                .u32("gemma3n.block_count", 30)
                .u32("gemma3n.embedding_length", 2048)
                .u32("gemma3n.attention.head_count", 8)
                .u32("gemma3n.attention.key_length", 256)
                .u32("gemma3n.attention.value_length", 256)
                .u32("gemma3n.attention.sliding_window", 512)
                .u32("gemma3n.attention.head_count_kv", 2)
                .f32("gemma3n.attention.shared_kv_layers", 10.0)
                .bool_array("gemma3n.attention.sliding_window_pattern", &pattern),
        );
        assert_eq!(s.kv_layers, (0..30).map(|il| il < 20).collect::<Vec<_>>());
        assert_eq!(s.kv_bytes(4096, F16), 64 * MIB);
        assert_eq!(s.kv_bytes(32_768, F16), (256 + 32) * MIB);
    }

    /// Qwen 3.5 9B (`unsloth/Qwen3.5-9B-GGUF`): one head count for every layer, but only
    /// every fourth layer is attention (`full_attention_interval`); the others are linear
    /// attention without this memory. llama.cpp: 128 MiB at 4,096 tokens, 2,048 at 65,536.
    #[test]
    fn qwen35_counts_only_its_attention_layers() {
        let qwen35 = || {
            Writer::new()
                .str("general.architecture", "qwen35")
                .u32("qwen35.block_count", 32)
                .u32("qwen35.embedding_length", 4096)
                .u32("qwen35.attention.head_count", 16)
                .u32("qwen35.attention.head_count_kv", 4)
                .u32("qwen35.attention.key_length", 256)
                .u32("qwen35.attention.value_length", 256)
        };
        let s = summary(qwen35().u32("qwen35.full_attention_interval", 4));
        assert_eq!(
            s.kv_layers,
            (0..32).map(|il| il % 4 == 3).collect::<Vec<_>>()
        );
        // 8 layers × 4 heads × 512 values × 2 bytes = 32,768 bytes a cell (0.0.6 counted all
        // 32 layers, 131,072).
        assert_eq!(s.kv_size(F16), KvSize::per_token(32_768));
        assert_eq!(s.kv_bytes(4096, F16), 128 * MIB);
        assert_eq!(s.kv_bytes(65_536, F16), 2048 * MIB);
        // The interval defaults to 4; a `recurrent_layers` array wins over it.
        assert_eq!(summary(qwen35()).kv_size(F16), KvSize::per_token(32_768));
        let flags: Vec<bool> = (0..32).map(|il| il % 2 == 0).collect();
        let arrayed = summary(qwen35().bool_array("qwen35.attention.recurrent_layers", &flags));
        assert_eq!(arrayed.kv_size(F16), KvSize::per_token(16 * 4096));
    }

    /// GLM-4.7-Flash (`unsloth/GLM-4.7-Flash-GGUF`, DeepSeek 2 layout with MLA): keys only.
    /// llama.cpp: K 211.5 MiB, V 0 at 4,096 tokens; 1,692 MiB at 32,768.
    #[test]
    fn mla_keeps_keys_only() {
        let glm = || {
            Writer::new()
                .str("general.architecture", "deepseek2")
                .u32("deepseek2.block_count", 47)
                .u32("deepseek2.embedding_length", 2048)
                .u32("deepseek2.attention.head_count", 20)
                .u32("deepseek2.attention.head_count_kv", 1)
                .u32("deepseek2.attention.key_length", 576)
                .u32("deepseek2.attention.value_length", 512)
        };
        let s = summary(
            glm()
                .u32("deepseek2.attention.key_length_mla", 256)
                .u32("deepseek2.attention.value_length_mla", 256),
        );
        assert!(!s.kv_values);
        // 47 layers × 576 values × 2 bytes = 54,144 bytes a cell.
        assert_eq!(s.kv_bytes(4096, F16), 47 * 1152 * 4096);
        assert_eq!(s.kv_bytes(4096, F16) * 2, 423 * MIB);
        assert_eq!(s.kv_bytes(32_768, F16), 1692 * MIB);
        // An older file without the MLA sizes stores values too.
        assert_eq!(summary(glm()).kv_size(F16).per_cell, 47 * (576 + 512) * 2);
    }

    /// A model without short-memory layers keeps the 0.0.6 figures (Qwen3 0.6B: 28 layers
    /// of 8 KV heads of 128, 114,688 bytes a token; 469,762,048 bytes at 4,096 tokens, as
    /// llama-server reported for the test model).
    #[test]
    fn qwen3_is_unchanged() {
        let s = summary(
            Writer::new()
                .str("general.architecture", "qwen3")
                .u32("qwen3.block_count", 28)
                .u32("qwen3.embedding_length", 1024)
                .u32("qwen3.attention.head_count", 16)
                .u32("qwen3.attention.head_count_kv", 8)
                .u32("qwen3.attention.key_length", 128)
                .u32("qwen3.attention.value_length", 128),
        );
        assert_eq!(s.kv_size(F16), KvSize::per_token(114_688));
        assert_eq!(s.kv_bytes(4096, F16), 469_762_048);
        assert_eq!(s.kv_bytes(40_960, F16), 114_688 * 40_960);
        assert_eq!(s.kv_size(F16).checkpoints_at(40_960), 0);
    }

    /// Extra prediction layers (`nextn_predict_layers`) are not in the main context, and a
    /// model without this kind of memory has none.
    #[test]
    fn prediction_layers_and_memoryless_models() {
        let glm = summary(
            Writer::new()
                .str("general.architecture", "glm4moe")
                .u32("glm4moe.block_count", 47)
                .u32("glm4moe.embedding_length", 4096)
                .u32("glm4moe.attention.head_count", 96)
                .u32("glm4moe.attention.head_count_kv", 8)
                .u32("glm4moe.attention.key_length", 128)
                .u32("glm4moe.attention.value_length", 128)
                .u32("glm4moe.nextn_predict_layers", 1),
        );
        assert_eq!(glm.kv_layers, (0..47).map(|il| il < 46).collect::<Vec<_>>());
        assert_eq!(glm.kv_size(F16).per_cell, 46 * 8 * 256 * 2);
        for arch in ["bert", "mamba", "rwkv7", "gemma-embedding"] {
            let s = summary(
                Writer::new()
                    .str("general.architecture", arch)
                    .u32(&format!("{arch}.block_count"), 12)
                    .u32(&format!("{arch}.embedding_length"), 768)
                    .u32(&format!("{arch}.attention.head_count"), 12),
            );
            assert_eq!(s.kv_bytes(4096, F16), 0, "{arch}");
        }
    }

    /// The function of the context equals the layer-by-layer sum at every context.
    #[test]
    fn size_equals_the_layers_at_every_context() {
        let models = [
            summary(gemma4_12b()),
            summary(gemma3_270m()),
            summary(
                gemma4_12b_except(&["attention.shared_kv_layers"])
                    .u32("gemma4.attention.shared_kv_layers", 30),
            ),
        ];
        let layouts = [
            KvLayout::from(F16),
            KvLayout::from(Q8),
            KvLayout::launch(false, true),
        ];
        for s in &models {
            let shape = s.kv_shape();
            for layout in layouts {
                let size = shape.size(layout);
                for ctx in (0..3000).chain([4095, 4096, 65_535, 65_536, 1 << 20, u64::MAX]) {
                    assert_eq!(size.at(ctx), shape.total(ctx, layout), "{ctx}");
                }
            }
        }
    }

    #[test]
    fn cells_follow_llama_cpp() {
        assert_eq!(cells(0), 0);
        assert_eq!(cells(1), 256);
        assert_eq!(cells(4096), 4096);
        assert_eq!(cells(4097), 4352);
        assert_eq!(cells(u64::MAX), u64::MAX);
        assert_eq!(window_cells(1024, 65_536), 1536);
        assert_eq!(window_cells(128, 32_768), 768);
        assert_eq!(window_cells(512, 512), 512);
        // Never more than the context's own cells.
        assert_eq!(window_cells(4096, 2048), 2048);
        assert_eq!(window_cells(u64::MAX, 4096), 4096);
        // A context under 512 tokens is its own batch: 300 + 512 window → 812 → 1,024,
        // limited to the context's 512 cells.
        assert_eq!(window_cells(512, 300), 512);
    }

    /// A real model with short-memory layers (`TEST_SWA_GGUF`; skipped when it is not
    /// set): its header gives short-memory layers and a function of the context equal to
    /// its layers' sum. For the pinned Gemma 3 270M file (`ggml-org/gemma-3-270m-it-GGUF`,
    /// `gemma-3-270m-it-Q8_0.gguf`, 291,545,600 bytes), exactly what llama-server b12110
    /// allocated for it on the processor (`llama-server -m <file> -c C -np 1 --device none
    /// -ngl 0 --fit off -v`, "llama_kv_cache: size = ..."): 12 + 15 MiB at 4,096 tokens,
    /// 96 + 15 at 32,768, 1.5 + 7.5 at 512, and with `-fa on -ctk q8_0 -ctv q8_0` at 4,096
    /// 6.38 + 7.97 MiB with rotation buffers of 0.80 + 3.98 MiB; its saved copies were
    /// logged at 7.506 MiB each, 32 kept (7,864,320 bytes of rows and 512 cells' positions).
    #[test]
    fn real_short_memory_model_if_available() {
        let Ok(path) = std::env::var("TEST_SWA_GGUF") else {
            eprintln!("TEST_SWA_GGUF not set: skipped");
            return;
        };
        let path = std::path::Path::new(&path);
        let s = ModelSummary::from_header(&crate::gguf::read_file(path).unwrap());
        assert!(s.sliding_window > 0, "no window");
        assert!(s.swa_layers.contains(&true), "no short-memory layers");
        let shape = s.kv_shape();
        for ctx in [512, 4096, 32_768] {
            for cache in [F16, Q8] {
                assert_eq!(shape.size(cache).at(ctx), shape.total(ctx, cache));
            }
        }
        let pinned = path
            .file_name()
            .is_some_and(|n| n == "gemma-3-270m-it-Q8_0.gguf")
            && std::fs::metadata(path).is_ok_and(|m| m.len() == 291_545_600);
        if !pinned {
            eprintln!("not the pinned Gemma 3 270M file: exact figures not checked");
            return;
        }
        assert_eq!(s.kv_bytes(4096, F16), (12 + 15) * MIB);
        assert_eq!(s.kv_bytes(32_768, F16), (96 + 15) * MIB);
        assert_eq!(s.kv_bytes(512, F16), 9 * MIB);
        assert_eq!(
            s.kv_bytes(4096, Q8),
            6_684_672 + 8_355_840 + 835_584 + 4_177_920
        );
        assert_eq!(s.kv_size(F16).checkpoints_at(16_384), 32 * 7_864_320);
    }

    /// Hostile headers (spec 2.0): any length of pattern array, absurd windows and head
    /// sizes, more shared layers than layers, an interval of 0: no panic, no allocation
    /// by the header's word, and a sane (saturated or safe-side) figure.
    #[test]
    fn hostile_headers() {
        // A pattern array longer than the layers: the extra entries are not used.
        let long: Vec<bool> = (0..3000).map(|il| il % 6 != 5).collect();
        let pattern = "attention.sliding_window_pattern";
        let s = summary(
            gemma4_12b_except(&[pattern])
                .bool_array("gemma4.attention.sliding_window_pattern", &long),
        );
        assert_eq!(s.swa_layers.len(), 48);
        assert_eq!(s.kv_bytes(65_536, F16), 1_577_058_304);
        // Shorter: the layers past its end keep the whole conversation (the safe side).
        let short = summary(
            gemma4_12b_except(&[pattern])
                .bool_array("gemma4.attention.sliding_window_pattern", &[true; 6]),
        );
        assert_eq!(short.swa_layers.iter().filter(|&&w| w).count(), 6);
        assert!(short.kv_bytes(65_536, F16) > 1_577_058_304);
        // Windows of 0 and of u64::MAX.
        let window = "attention.sliding_window";
        let zero = summary(gemma4_12b_except(&[window]).u32("gemma4.attention.sliding_window", 0));
        // llama.cpp still gives the short layers one batch: GGML_PAD(0 + 512, 256) cells.
        assert_eq!(zero.kv_bytes(65_536, F16), 16_384 * 65_536 + 327_680 * 512);
        assert_eq!(zero.kv_size(F16).checkpoints_at(65_536), 0);
        let huge =
            summary(gemma4_12b_except(&[window]).u64("gemma4.attention.sliding_window", u64::MAX));
        assert_eq!(huge.sliding_window, u64::from(u32::MAX));
        // At full length: the safe side.
        assert_eq!(huge.kv_bytes(65_536, F16), (16_384 + 327_680) * 65_536);
        assert!(huge.kv_size(F16).checkpoints_at(u64::MAX) > 0);
        // Absurd short-memory head sizes saturate.
        let wide = summary(
            gemma4_12b_except(&["attention.key_length_swa"])
                .u64("gemma4.attention.key_length_swa", u64::MAX),
        );
        assert_eq!(wide.kv_bytes(65_536, F16), u64::MAX);
        assert_eq!(wide.kv_bytes(65_536, Q8), u64::MAX);
        assert_eq!(wide.kv_size(F16).checkpoints_at(65_536), u64::MAX);
        // More shared layers than layers: none keeps memory (llama.cpp would not load it).
        let shared = summary(
            gemma4_12b_except(&["attention.shared_kv_layers"])
                .u64("gemma4.attention.shared_kv_layers", u64::MAX),
        );
        assert_eq!(shared.kv_bytes(65_536, F16), 0);
        // An interval of 0 is not believed: every layer counts (the safe side).
        let qwen = summary(
            Writer::new()
                .str("general.architecture", "qwen35")
                .u32("qwen35.block_count", 32)
                .u32("qwen35.embedding_length", 4096)
                .u32("qwen35.attention.head_count", 16)
                .u32("qwen35.attention.head_count_kv", 4)
                .u32("qwen35.full_attention_interval", 0),
        );
        assert_eq!(qwen.kv_layers, vec![true; 32]);
        // A huge block count with a pattern number: at most MAX_BLOCKS layers.
        let many = summary(
            gemma3_270m_except(&["block_count"])
                .u64("gemma3.block_count", u64::MAX)
                .u64("gemma3.attention.sliding_window_pattern", u64::MAX),
        );
        assert_eq!(many.swa_layers.len() as u64, MAX_BLOCKS);
        assert_eq!(many.kv_layers.len() as u64, MAX_BLOCKS);
        assert!(many.kv_bytes(u64::MAX, Q8) > 0);
        // More prediction layers than layers: nothing to follow a pattern, no panic.
        let nextn = summary(gemma3_270m().u64("gemma3.nextn_predict_layers", u64::MAX));
        assert!(nextn.swa_layers.iter().all(|&w| !w));
        // A pattern that is text: the architecture's own.
        let text = summary(gemma3_270m().str("gemma3.attention.sliding_window_pattern", "x"));
        assert_eq!(text.swa_layers, summary(gemma3_270m()).swa_layers);
        // Head sizes whose rotation matrix overflows saturate.
        let huge_heads = summary(
            gemma3_270m_except(&["attention.key_length", "attention.value_length"])
                .u64("gemma3.attention.key_length", 1 << 62)
                .u64("gemma3.attention.value_length", 1 << 62),
        );
        assert_eq!(huge_heads.kv_bytes(4096, Q8), u64::MAX);
    }
}
