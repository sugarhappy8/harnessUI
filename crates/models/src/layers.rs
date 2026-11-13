//! What each part of a model weighs, from the tensor tables of all its files (0.0.6 item
//! 6, "Where the model goes"): the input layer, llama.cpp's output unit, every block with
//! its expert parts apart, and the rest.

use serde::Serialize;

use crate::gguf::TensorEntry;

/// Blocks beyond this are not counted as layers. llama.cpp loads at most 512 and its fit
/// handles at most 1,000; Fine control's layer count goes up to 1,000 too.
pub const MAX_BLOCKS: u64 = 1_000;

/// One block (`blk.N.*`): its dense part and its expert parts (mixture of experts).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LayerSize {
    pub dense_bytes: u64,
    pub expert_bytes: u64,
}

impl LayerSize {
    pub fn total(&self) -> u64 {
        self.dense_bytes.saturating_add(self.expert_bytes)
    }
}

/// What each part of the model weighs, in bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LayerSizes {
    /// llama.cpp's input layer, always in computer memory: `token_embd.*` and the other
    /// tensors it reads by row ([`INPUT_TENSORS`]).
    pub input_bytes: u64,
    /// `output.*`, `output_norm.*`: llama.cpp's last unit. With tied embeddings (no
    /// `output.weight`) llama.cpp loads `token_embd.weight` a second time as the output, so
    /// its size is counted here as well. That second copy is real whenever the output unit
    /// is on a graphics device (another buffer) or repacked on the processor; when the
    /// processor keeps the output in the input layer's own buffer (`--no-repack`, or a type
    /// it does not repack, such as Q6_K), llama.cpp reuses the first copy and the estimate
    /// is that much high: the safe side, until the measurement replaces it.
    pub output_bytes: u64,
    /// `blk.0` … `blk.(block_count-1)`.
    pub layers: Vec<LayerSize>,
    /// Anything else (rope tables, extra heads, unknown names, blocks beyond the count).
    pub other_bytes: u64,
}

impl LayerSizes {
    /// Groups the tensors of every file of the model (`tables`, one per split file) into
    /// the input, the output unit, `block_count` blocks (at most [`MAX_BLOCKS`]) and the
    /// rest. Expert parts are the tensors llama.cpp's fit moves with them:
    /// `blk.N.ffn_(up|down|gate|gate_up)_(ch|)exps.*`.
    pub fn from_tables(tables: &[Vec<TensorEntry>], block_count: u64) -> Self {
        // At most MAX_BLOCKS, so the conversion cannot fail.
        let blocks = usize::try_from(block_count.min(MAX_BLOCKS)).unwrap_or(0);
        let mut sizes = Self {
            layers: vec![LayerSize::default(); blocks],
            ..Self::default()
        };
        let mut embedding_weight = 0u64;
        let mut has_output_weight = false;
        for t in tables.iter().flatten() {
            let add = |to: &mut u64| *to = to.saturating_add(t.bytes);
            match part_of(&t.name, blocks) {
                Part::Input => {
                    add(&mut sizes.input_bytes);
                    if t.name == "token_embd.weight" {
                        add(&mut embedding_weight);
                    }
                }
                Part::Output => {
                    add(&mut sizes.output_bytes);
                    has_output_weight |= t.name == "output.weight";
                }
                Part::Block { index, expert } => {
                    let layer = &mut sizes.layers[index];
                    add(if expert {
                        &mut layer.expert_bytes
                    } else {
                        &mut layer.dense_bytes
                    });
                }
                Part::Other => add(&mut sizes.other_bytes),
            }
        }
        if !has_output_weight {
            sizes.output_bytes = sizes.output_bytes.saturating_add(embedding_weight);
        }
        sizes
    }

    /// Everything, the duplicated output of tied embeddings included.
    pub fn total(&self) -> u64 {
        self.layers
            .iter()
            .map(LayerSize::total)
            .fold(self.input_bytes, u64::saturating_add)
            .saturating_add(self.output_bytes)
            .saturating_add(self.other_bytes)
    }

    /// llama.cpp's "N/M layers": the blocks and the output unit.
    pub fn units(&self) -> u32 {
        u32::try_from(self.layers.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1)
    }

    /// Whether any block has expert parts.
    pub fn has_experts(&self) -> bool {
        self.layers.iter().any(|l| l.expert_bytes > 0)
    }
}

/// Where one tensor belongs.
#[derive(Debug, PartialEq, Eq)]
enum Part {
    Input,
    Output,
    Block { index: usize, expert: bool },
    Other,
}

/// The tensor names of expert parts after `blk.N.` (llama.cpp's fit:
/// `ffn_(up|down|gate_up|gate)_(ch|)exps`).
const EXPERT_PARTS: [&str; 8] = [
    "ffn_up_exps",
    "ffn_down_exps",
    "ffn_gate_exps",
    "ffn_gate_up_exps",
    "ffn_up_chexps",
    "ffn_down_chexps",
    "ffn_gate_chexps",
    "ffn_gate_up_chexps",
];

/// The tensors of llama.cpp's input layer, which it always keeps on the processor
/// (`LLM_TENSOR_LAYER_INPUT` in llama-arch.cpp, b12110): the token embedding, and the
/// position, token-type, value and per-layer token embeddings (the last one is large in
/// models that have it) and two mask tables.
pub const INPUT_TENSORS: [&str; 8] = [
    "token_embd",
    "position_embd",
    "token_types",
    "value_embd",
    "per_layer_token_embd",
    "masked_embd_centroids",
    "masked_embd_ordering",
    "mask_embd",
];

fn part_of(name: &str, blocks: usize) -> Part {
    if INPUT_TENSORS.iter().any(|input| {
        name.strip_prefix(input)
            .is_some_and(|rest| rest.starts_with('.'))
    }) {
        return Part::Input;
    }
    if name.starts_with("output.") || name.starts_with("output_norm.") {
        return Part::Output;
    }
    let Some(rest) = name.strip_prefix("blk.") else {
        return Part::Other;
    };
    let (number, tail) = rest.split_once('.').unwrap_or((rest, ""));
    // Digits only: `str::parse` would also take a leading '+'.
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Part::Other;
    }
    match number.parse::<usize>() {
        Ok(index) if index < blocks => Part::Block {
            index,
            expert: EXPERT_PARTS.iter().any(|part| {
                tail.strip_prefix(part)
                    .is_some_and(|after| after.is_empty() || after.starts_with('.'))
            }),
        },
        _ => Part::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gguf::testing::Writer;

    fn t(name: &str, bytes: u64) -> TensorEntry {
        TensorEntry {
            name: name.to_string(),
            bytes,
        }
    }

    fn layer(dense: u64, expert: u64) -> LayerSize {
        LayerSize {
            dense_bytes: dense,
            expert_bytes: expert,
        }
    }

    #[test]
    fn a_dense_model_is_grouped() {
        let table = vec![
            t("token_embd.weight", 1000),
            t("blk.0.attn_norm.weight", 10),
            t("blk.0.attn_q.weight", 100),
            t("blk.0.ffn_up.weight", 300),
            t("blk.1.attn_q.weight", 100),
            t("blk.1.ffn_down.weight", 300),
            t("output_norm.weight", 10),
            t("output.weight", 900),
            t("rope_freqs.weight", 5),
        ];
        let s = LayerSizes::from_tables(&[table], 2);
        assert_eq!(
            s,
            LayerSizes {
                input_bytes: 1000,
                output_bytes: 910,
                layers: vec![layer(410, 0), layer(400, 0)],
                other_bytes: 5,
            }
        );
        assert_eq!(s.units(), 3);
        assert_eq!(s.total(), 1000 + 910 + 810 + 5);
        assert!(!s.has_experts());
    }

    /// Every expert pattern llama.cpp's fit uses, and the names that only look like one.
    #[test]
    fn expert_parts_are_kept_apart() {
        let table = vec![
            t("blk.0.ffn_gate_inp.weight", 1),
            t("blk.0.ffn_up_exps.weight", 10),
            t("blk.0.ffn_down_exps.weight", 20),
            t("blk.0.ffn_gate_exps.weight", 40),
            t("blk.0.ffn_gate_up_exps.weight", 80),
            t("blk.0.ffn_up_chexps.weight", 160),
            t("blk.0.ffn_down_chexps.weight", 320),
            t("blk.0.ffn_gate_chexps.weight", 640),
            t("blk.0.ffn_gate_up_chexps.weight", 1280),
            t("blk.0.ffn_down_exps.bias", 2560),
            t("blk.0.ffn_down_exps.scale", 5120),
            // Shared experts and the router stay with the dense part.
            t("blk.0.ffn_up_shexp.weight", 2),
            t("blk.0.ffn_down_shexp.weight", 4),
            t("blk.0.exp_probs_b.bias", 8),
            t("blk.0.ffn_up_expsx.weight", 16),
            t("blk.0.attn_q.weight", 32),
        ];
        let s = LayerSizes::from_tables(&[table], 1);
        assert_eq!(s.layers, vec![layer(1 + 2 + 4 + 8 + 16 + 32, 10_230)]);
        assert!(s.has_experts());
    }

    /// Every tensor of llama.cpp's input layer stays in computer memory, not only the token
    /// embedding; names that only start the same way do not.
    #[test]
    fn the_whole_input_layer_is_input() {
        let table = vec![
            t("token_embd.weight", 1),
            t("position_embd.weight", 2),
            t("token_types.weight", 4),
            t("value_embd.weight", 8),
            t("per_layer_token_embd.weight", 16),
            t("masked_embd_centroids.weight", 32),
            t("masked_embd_ordering.weight", 64),
            t("mask_embd.weight", 128),
            t("token_embd_norm.weight", 256),
            t("per_layer_token_embdx.weight", 512),
            t("output.weight", 1024),
        ];
        let s = LayerSizes::from_tables(&[table], 0);
        assert_eq!(s.input_bytes, 255);
        assert_eq!(s.other_bytes, 768);
        assert_eq!(s.output_bytes, 1024);
    }

    /// Without `output.weight`, the output unit weighs `token_embd.weight` again.
    #[test]
    fn tied_embeddings_count_the_input_again_as_the_output() {
        let tied = vec![
            t("token_embd.weight", 1000),
            t("blk.0.attn_q.weight", 100),
            t("output_norm.weight", 10),
        ];
        let s = LayerSizes::from_tables(&[tied], 1);
        assert_eq!((s.input_bytes, s.output_bytes), (1000, 1010));
        assert_eq!(s.total(), 2110);
        // An output weight in another split file is still found.
        let first = vec![t("token_embd.weight", 1000), t("blk.0.attn_q.weight", 100)];
        let second = vec![t("output_norm.weight", 10), t("output.weight", 700)];
        let s = LayerSizes::from_tables(&[first, second], 1);
        assert_eq!((s.input_bytes, s.output_bytes), (1000, 710));
    }

    #[test]
    fn split_files_are_summed() {
        let first = vec![
            t("token_embd.weight", 1000),
            t("blk.0.attn_q.weight", 100),
            t("blk.0.ffn_up_exps.weight", 500),
        ];
        let second = vec![
            t("blk.0.ffn_down_exps.weight", 500),
            t("blk.1.attn_q.weight", 100),
            t("blk.1.ffn_up_exps.weight", 400),
            t("output.weight", 900),
        ];
        let s = LayerSizes::from_tables(&[first, second], 2);
        assert_eq!(s.layers, vec![layer(100, 1000), layer(100, 400)]);
        assert_eq!(s.output_bytes, 900);
        assert_eq!(s.total(), 1000 + 900 + 1100 + 500);
    }

    /// Block numbers the header does not count, names that are not numbers, a block count
    /// far beyond any model, and sizes near u64::MAX never panic.
    #[test]
    fn odd_names_and_counts_are_other() {
        let table = vec![
            t("blk.2.attn_q.weight", 7),
            t("blk.+1.attn_q.weight", 11),
            t("blk..attn_q.weight", 13),
            t("blk.1x.attn_q.weight", 17),
            t("blk.99999999999999999999999.attn_q.weight", 19),
            t("blk.0", 23),
            t("token_embd_norm.weight", 29),
            t("outputs.weight", 31),
        ];
        let s = LayerSizes::from_tables(&[table], 2);
        assert_eq!(s.layers, vec![layer(23, 0), layer(0, 0)]);
        assert_eq!(s.other_bytes, 7 + 11 + 13 + 17 + 19 + 29 + 31);
        let huge = LayerSizes::from_tables(&[vec![t("blk.5000.attn_q.weight", 1)]], u64::MAX);
        assert_eq!(huge.layers.len() as u64, MAX_BLOCKS);
        assert_eq!(huge.units(), 1001);
        assert_eq!(huge.other_bytes, 1);
        let big = vec![
            t("token_embd.weight", u64::MAX),
            t("blk.0.ffn_up_exps.weight", u64::MAX),
            t("blk.0.ffn_down_exps.weight", u64::MAX),
            t("blk.0.attn_q.weight", u64::MAX),
        ];
        let s = LayerSizes::from_tables(&[big], 1);
        assert_eq!(s.total(), u64::MAX);
        assert_eq!(s.layers[0], layer(u64::MAX, u64::MAX));
        assert_eq!(LayerSizes::from_tables(&[], 0).units(), 1);
    }

    /// From a file written with the tensor table writer, through the reader.
    #[test]
    fn from_a_written_file() {
        let bytes = Writer::new()
            .str("general.architecture", "qwen3moe")
            .u32("qwen3moe.block_count", 2)
            .tensor("token_embd.weight", 4096)
            .tensor("blk.0.attn_q.weight", 512)
            .tensor("blk.0.ffn_gate_exps.weight", 2048)
            .tensor("blk.1.attn_q.weight", 512)
            .tensor("blk.1.ffn_gate_exps.weight", 2048)
            .tensor("output_norm.weight", 32)
            .build();
        let (header, table) =
            crate::gguf::read_with_tensors(std::io::Cursor::new(&bytes), bytes.len() as u64)
                .unwrap();
        let s = LayerSizes::from_tables(&[table], header.arch_u64("block_count").unwrap());
        assert_eq!(s.layers, vec![layer(512, 2048), layer(512, 2048)]);
        assert_eq!((s.input_bytes, s.output_bytes), (4096, 4096 + 32));
    }

    /// The real model: one entry per block, every block has a dense part, the tied output
    /// is counted once more than the file holds.
    #[test]
    fn real_model_if_available() {
        let Ok(path) = std::env::var("TEST_GGUF") else {
            return;
        };
        let (header, table) =
            crate::gguf::read_file_with_tensors(std::path::Path::new(&path)).unwrap();
        let blocks = header.arch_u64("block_count").unwrap();
        let in_file: u64 = table.iter().map(|t| t.bytes).sum();
        let tied = !table.iter().any(|t| t.name == "output.weight");
        let embd = table
            .iter()
            .find(|t| t.name == "token_embd.weight")
            .map_or(0, |t| t.bytes);
        let s = LayerSizes::from_tables(&[table], blocks);
        assert_eq!(u64::from(s.units()), blocks + 1);
        assert!(s.layers.iter().all(|l| l.dense_bytes > 0));
        assert_eq!(s.total(), in_file + if tied { embd } else { 0 });
        assert_eq!(s.other_bytes, 0, "{s:?}");
    }
}
