//! The tensor index of a model file: every tensor's name, type and shape, read from the
//! header alone, in the order and shapes stable-diffusion.cpp sees them (design section 7).
//!
//! Safetensors: the engine's header is a `nlohmann::json`, a `std::map`, so names are visited
//! sorted (`safetensors_io.cpp`). GGUF: file order. Every tensor is kept, whatever its
//! dtype: names decide detection.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::gguf::{self, GgufError};

/// The largest safetensors header read (100 MiB).
pub const SAFETENSORS_HEADER_CAP: u64 = 100 * 1024 * 1024;
/// The most GGUF metadata and tensor table read (64 MiB).
pub const GGUF_HEADER_CAP: u64 = 64 * 1024 * 1024;
/// Pickle formats: downloadable, never parsed.
pub const PICKLE_EXTENSIONS: [&str; 4] = [".ckpt", ".pt", ".pth", ".bin"];
/// The formats Studio reads.
pub const READ_EXTENSIONS: [&str; 3] = [".safetensors", ".sft", ".gguf"];

/// The first read of a local file.
const FIRST_READ: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Safetensors,
    Gguf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TensorInfo {
    pub name: String,
    pub dtype: String,
    /// As written in the file: safetensors outermost first, GGUF `ne` order.
    pub shape: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TensorIndex {
    pub format: FileFormat,
    /// GGUF: file order; safetensors: sorted by name.
    pub tensors: Vec<TensorInfo>,
    pub metadata: BTreeMap<String, String>,
    /// The whole file's parameter count when `tensors` is a trimmed list (Task R4's
    /// fixtures); `None` for a real read.
    #[serde(default)]
    pub parameters_hint: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("not a weights file")]
    NotWeights,
    #[error("header larger than {cap} bytes")]
    TooLarge { cap: u64 },
    #[error("needs {need} bytes")]
    Truncated { need: u64 },
    #[error("corrupt header: {0}")]
    Corrupt(String),
    #[error("a pickle file")]
    Pickle,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What a GGUF reader's refusal means for the index.
impl From<GgufError> for IndexError {
    fn from(e: GgufError) -> Self {
        match e {
            GgufError::NotGguf => IndexError::NotWeights,
            GgufError::Truncated { need } if need > GGUF_HEADER_CAP => IndexError::TooLarge {
                cap: GGUF_HEADER_CAP,
            },
            GgufError::Truncated { need } => IndexError::Truncated { need },
            GgufError::Io(e) => IndexError::Io(e),
            e => IndexError::Corrupt(e.to_string()),
        }
    }
}

/// The format `bytes` starts like: the "GGUF" magic, or a `u64` little-endian length
/// followed by `{`.
pub fn sniff(bytes: &[u8]) -> Option<FileFormat> {
    if bytes.starts_with(b"GGUF") {
        Some(FileFormat::Gguf)
    } else if bytes.len() > 8 && bytes[8] == b'{' {
        Some(FileFormat::Safetensors)
    } else {
        None
    }
}

/// The index from the first bytes of a file. [`IndexError::Truncated`] says how many bytes
/// are needed (a lower bound for GGUF); [`IndexError::TooLarge`] when the header is past
/// its cap.
pub fn parse_prefix(bytes: &[u8]) -> Result<TensorIndex, IndexError> {
    match sniff(bytes) {
        Some(FileFormat::Gguf) => parse_gguf(bytes),
        Some(FileFormat::Safetensors) => parse_safetensors(bytes),
        // Too short to tell.
        None if bytes.len() < 9 => Err(IndexError::Truncated { need: 9 }),
        None => Err(IndexError::NotWeights),
    }
}

fn parse_gguf(bytes: &[u8]) -> Result<TensorIndex, IndexError> {
    let (raw, metadata) = gguf::tensor_table_from_prefix(bytes)?;
    Ok(TensorIndex {
        format: FileFormat::Gguf,
        tensors: raw
            .into_iter()
            .map(|t| TensorInfo {
                name: t.name,
                dtype: gguf::ggml_type_name(t.ggml_type),
                shape: t.dims,
            })
            .collect(),
        metadata,
        parameters_hint: None,
    })
}

fn parse_safetensors(bytes: &[u8]) -> Result<TensorIndex, IndexError> {
    let mut len = [0u8; 8];
    len.copy_from_slice(&bytes[..8]);
    let len = u64::from_le_bytes(len);
    if len > SAFETENSORS_HEADER_CAP {
        return Err(IndexError::TooLarge {
            cap: SAFETENSORS_HEADER_CAP,
        });
    }
    let need = 8 + len;
    if (bytes.len() as u64) < need {
        return Err(IndexError::Truncated { need });
    }
    // `need` is at most the cap plus 8, and `bytes` holds it.
    let json = &bytes[8..need as usize];
    let map: Map<String, Value> =
        serde_json::from_slice(json).map_err(|e| IndexError::Corrupt(e.to_string()))?;
    let mut metadata = BTreeMap::new();
    let mut tensors = Vec::with_capacity(map.len());
    for (name, v) in map {
        if name == "__metadata__" {
            if let Value::Object(m) = v {
                metadata.extend(
                    m.into_iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string()))),
                );
            }
            continue;
        }
        let bad = || IndexError::Corrupt(format!("tensor {name:?} has no dtype and shape"));
        let dtype = v.get("dtype").and_then(Value::as_str).ok_or_else(bad)?;
        let shape = v
            .get("shape")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
            .ok_or_else(bad)?;
        tensors.push(TensorInfo {
            dtype: dtype.to_string(),
            shape,
            name,
        });
    }
    // The engine's order, whatever the map keeps.
    tensors.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(TensorIndex {
        format: FileFormat::Safetensors,
        tensors,
        metadata,
        parameters_hint: None,
    })
}

/// Reads the index of the local file at `path`, from its header only. A pickle extension is
/// refused by name, without opening the file.
pub fn read_local(path: &Path) -> Result<TensorIndex, IndexError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if PICKLE_EXTENSIONS.iter().any(|e| name.ends_with(e)) {
        return Err(IndexError::Pickle);
    }
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut want = FIRST_READ;
    loop {
        let take = want.min(file_len);
        let mut buf = Vec::new();
        // Reopen-free: the file is read from its start each round.
        std::io::Seek::rewind(&mut file)?;
        (&mut file).take(take).read_to_end(&mut buf)?;
        match parse_prefix(&buf) {
            Err(IndexError::Truncated { need }) => {
                if take >= file_len {
                    return Err(if file_len < 9 {
                        IndexError::NotWeights
                    } else {
                        IndexError::Corrupt("the file ends inside its header".into())
                    });
                }
                want = need
                    .max(take + 1)
                    .max(take.saturating_mul(2).min(GGUF_HEADER_CAP));
            }
            other => return other,
        }
    }
}

/// The shards a `*.safetensors.index.json` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardIndex {
    /// The distinct files of `weight_map`, sorted.
    pub shards: Vec<String>,
}

pub fn parse_shard_index(json: &[u8]) -> Result<ShardIndex, IndexError> {
    let v: Value = serde_json::from_slice(json).map_err(|e| IndexError::Corrupt(e.to_string()))?;
    let map = v
        .get("weight_map")
        .and_then(Value::as_object)
        .ok_or_else(|| IndexError::Corrupt("no weight_map".into()))?;
    let mut shards = std::collections::BTreeSet::new();
    for file in map.values() {
        let file = file
            .as_str()
            .ok_or_else(|| IndexError::Corrupt("weight_map names a non-file".into()))?;
        shards.insert(file.to_string());
    }
    Ok(ShardIndex {
        shards: shards.into_iter().collect(),
    })
}

impl TensorIndex {
    /// The tensors in the order the engine visits them: safetensors sorted by name, GGUF in
    /// file order.
    pub fn engine_order(&self) -> Vec<&TensorInfo> {
        let mut v: Vec<&TensorInfo> = self.tensors.iter().collect();
        if self.format == FileFormat::Safetensors {
            v.sort_by(|a, b| a.name.cmp(&b.name));
        }
        v
    }

    /// `parameters_hint` when set, else the sum over all tensors of the product of the shape.
    pub fn parameters(&self) -> u64 {
        self.parameters_hint.unwrap_or_else(|| {
            self.tensors
                .iter()
                .map(|t| t.shape.iter().fold(1u64, |a, d| a.saturating_mul(*d)))
                .fold(0u64, u64::saturating_add)
        })
    }
}

/// The tensor's `ne` as the engine holds it (innermost first, four values).
///
/// Safetensors: a 5-D shape first folds to `[d0*d1, d2, d3, d4]`
/// (`safetensors_io.cpp:330-344`), then the shape is reversed and padded with 1. GGUF:
/// the dimensions as written, those past the fourth multiplying into `ne[3]`
/// (`gguf_reader_ext.h:171-178`).
pub fn engine_ne(t: &TensorInfo, format: FileFormat) -> [u64; 4] {
    let mut dims: Vec<u64> = t.shape.clone();
    if format == FileFormat::Safetensors {
        if dims.len() == 5 {
            let folded = dims[0].saturating_mul(dims[1]);
            dims.splice(0..2, [folded]);
        }
        dims.reverse();
    }
    let mut ne = [1u64; 4];
    for (i, d) in dims.into_iter().enumerate() {
        let slot = i.min(3);
        ne[slot] = ne[slot].saturating_mul(d);
    }
    ne
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd::testing::{big_header, safetensors};
    use serde_json::json;

    #[test]
    fn a_safetensors_header_is_read_and_visited_in_engine_order() {
        let ix = parse_prefix(&safetensors(&json!({"b": {"dtype": "F16", "shape": [2, 3], "data_offsets": [0, 12]},
            "a": {"dtype": "BF16", "shape": [4], "data_offsets": [12, 20]}, "__metadata__": {"ss_base_model_version": "sdxl_base_v1-0", "n": 3}}))).unwrap();
        assert_eq!(
            ix.engine_order()
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(
            engine_ne(
                ix.tensors.iter().find(|t| t.name == "b").unwrap(),
                FileFormat::Safetensors
            ),
            [3, 2, 1, 1]
        );
        assert_eq!(
            ix.metadata.get("ss_base_model_version").map(String::as_str),
            Some("sdxl_base_v1-0")
        );
        assert!(!ix.metadata.contains_key("n"));
    }

    #[test]
    fn a_5d_safetensors_shape_folds_like_the_engine() {
        let t = TensorInfo {
            name: "x".into(),
            dtype: "F16".into(),
            shape: vec![3072, 48, 1, 2, 2],
        };
        assert_eq!(engine_ne(&t, FileFormat::Safetensors), [2, 2, 1, 147456]);
    }

    #[test]
    fn a_gguf_5d_tensor_stacks_into_ne3() {
        let t = TensorInfo {
            name: "x".into(),
            dtype: "f16".into(),
            shape: vec![2, 2, 1, 48, 3072],
        };
        assert_eq!(engine_ne(&t, FileFormat::Gguf), [2, 2, 1, 147456]);
    }

    #[test]
    fn a_header_over_the_cap_is_refused_before_it_is_read() {
        let mut b = (SAFETENSORS_HEADER_CAP + 1).to_le_bytes().to_vec();
        b.push(b'{');
        assert!(matches!(
            parse_prefix(&b),
            Err(IndexError::TooLarge {
                cap: SAFETENSORS_HEADER_CAP
            })
        ));
    }

    #[test]
    fn a_short_prefix_asks_for_exactly_what_it_needs() {
        let file = safetensors(&big_header(2_000));
        assert!(matches!(
            parse_prefix(&file[..100]),
            Err(IndexError::Truncated { need: 2_008 })
        ));
        let g = gguf::testing::diffusion_gguf("klein4b-tensors.txt");
        assert!(matches!(
            parse_prefix(&g[..64]),
            Err(IndexError::Truncated { .. })
        ));
    }

    #[test]
    fn pickle_files_are_named_and_never_opened() {
        assert!(matches!(
            read_local(Path::new("/does/not/exist/model.ckpt")),
            Err(IndexError::Pickle)
        ));
    }

    #[test]
    fn unicode_and_spaces_in_paths_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ü dir").join("モデル 1.safetensors");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, safetensors(&json!({}))).unwrap();
        assert_eq!(read_local(&p).unwrap().format, FileFormat::Safetensors);
    }

    #[test]
    fn a_sharded_index_lists_its_shards() {
        let s = parse_shard_index(br#"{"weight_map": {"a": "m-00002-of-00002.safetensors", "b": "m-00001-of-00002.safetensors", "c": "m-00001-of-00002.safetensors"}}"#).unwrap();
        assert_eq!(
            s.shards,
            [
                "m-00001-of-00002.safetensors",
                "m-00002-of-00002.safetensors"
            ]
        );
    }

    #[test]
    fn ggml_type_names_follow_the_pinned_ggml() {
        assert_eq!(
            (
                gguf::ggml_type_name(0),
                gguf::ggml_type_name(1),
                gguf::ggml_type_name(2),
                gguf::ggml_type_name(12),
                gguf::ggml_type_name(30),
                gguf::ggml_type_name(999)
            ),
            (
                "f32".into(),
                "f16".into(),
                "q4_0".into(),
                "q4_K".into(),
                "bf16".into(),
                "type999".into()
            )
        );
    }

    // Beyond the brief: the GGUF path end to end, in file order, and the local reader.
    #[test]
    fn a_whole_gguf_reads_in_file_order_and_a_local_one_reads_too() {
        let g = gguf::testing::diffusion_gguf("klein4b-tensors.txt");
        let ix = parse_prefix(&g).unwrap();
        let want = gguf::testing::shapes("klein4b-tensors.txt");
        assert_eq!(ix.format, FileFormat::Gguf);
        assert_eq!(
            ix.tensors.iter().map(|t| &t.name).collect::<Vec<_>>(),
            want.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        assert_eq!(ix.tensors[0].shape, want[0].dims);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.gguf");
        std::fs::write(&p, &g).unwrap();
        assert_eq!(read_local(&p).unwrap(), ix);
        // A cut inside the table is Truncated all the way, never Corrupt.
        for cut in (0..g.len().min(4000)).step_by(7) {
            assert!(
                matches!(parse_prefix(&g[..cut]), Err(IndexError::Truncated { .. })),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn a_text_file_is_not_weights() {
        assert!(matches!(
            parse_prefix(b"hello there, this is plain text"),
            Err(IndexError::NotWeights)
        ));
    }
}
