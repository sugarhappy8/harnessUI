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

use std::fmt;

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

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

/// The most key and value bytes of a safetensors `__metadata__` kept, as GGUF keeps at most
/// 16 MiB of metadata (`gguf::MAX_KEPT_TOTAL`); an entry that would go past it is read past.
const MAX_KEPT_METADATA: u64 = 16 * 1024 * 1024;
/// The most `__metadata__` entries kept; the rest are skipped unread. Real headers have tens.
const MAX_METADATA_ENTRIES: usize = 65_536;
/// A tensor shape longer than this is damage. The engine allows 5 (`SD_MAX_DIMS`).
const MAX_SHAPE_DIMS: usize = 8;

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
    let (raw, metadata, end) = gguf::tensor_table_with_end(bytes)?;
    // A caller may hand over more than the cap; the table still has to end inside it.
    if end > GGUF_HEADER_CAP {
        return Err(IndexError::TooLarge {
            cap: GGUF_HEADER_CAP,
        });
    }
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
    let header = parse_header(json, gguf::MAX_TENSORS)?;
    // A `BTreeMap`: sorted by name, the engine's order, and a repeated name keeps its last
    // value, as the engine's `nlohmann::json` object does.
    let tensors = header
        .tensors
        .into_iter()
        .map(|(name, e)| TensorInfo {
            name,
            dtype: e.dtype,
            shape: e.shape,
        })
        .collect();
    let metadata = header.metadata;
    Ok(TensorIndex {
        format: FileFormat::Safetensors,
        tensors,
        metadata,
        parameters_hint: None,
    })
}

/// What the index keeps of a safetensors header: each tensor's dtype and shape, and the
/// metadata's string values within a budget. Every other field is skipped unread; more than
/// `max_tensors` tensors, or a shape of more than [`MAX_SHAPE_DIMS`] dimensions, is refused as
/// it is read; and `__metadata__` keeps at most [`MAX_METADATA_ENTRIES`] entries and
/// [`MAX_KEPT_METADATA`] bytes of keys and values, reading past the rest. So what a header
/// within the cap keeps is its tensors' names and dtypes, a fixed cost of a few hundred bytes
/// for each of at most 262,144 tensors, and at most 16 MiB of metadata (final review I-1).
struct Header {
    /// Sorted by name; a repeated name keeps its last value, as `nlohmann::json` does.
    tensors: BTreeMap<String, Entry>,
    metadata: BTreeMap<String, String>,
}

/// One tensor's entry: `data_offsets` and anything else is skipped.
#[derive(Deserialize)]
struct Entry {
    dtype: String,
    #[serde(deserialize_with = "bounded_shape")]
    shape: Vec<u64>,
}

/// A shape, refused as damage at its dimension past [`MAX_SHAPE_DIMS`].
fn bounded_shape<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Vec<u64>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a tensor shape")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u64>, A::Error> {
            let mut dims = Vec::new();
            while let Some(d) = seq.next_element::<u64>()? {
                if dims.len() == MAX_SHAPE_DIMS {
                    return Err(de::Error::custom(format!(
                        "a tensor with more than {MAX_SHAPE_DIMS} dimensions"
                    )));
                }
                dims.push(d);
            }
            Ok(dims)
        }
    }
    d.deserialize_seq(V)
}

/// The header in `json`, refused as damage past `max_tensors` entries, counted as they are
/// read, before the rest is parsed.
fn parse_header(json: &[u8], max_tensors: u64) -> Result<Header, IndexError> {
    let mut d = serde_json::Deserializer::from_slice(json);
    let header = d
        .deserialize_map(HeaderVisitor { max_tensors })
        .and_then(|h| d.end().map(|()| h))
        .map_err(|e| IndexError::Corrupt(e.to_string()))?;
    Ok(header)
}

struct HeaderVisitor {
    max_tensors: u64,
}

impl<'de> Visitor<'de> for HeaderVisitor {
    type Value = Header;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a safetensors header object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Header, A::Error> {
        let mut tensors = BTreeMap::new();
        let mut metadata = BTreeMap::new();
        let mut seen = 0u64;
        while let Some(name) = map.next_key::<String>()? {
            if name == "__metadata__" {
                map.next_value_seed(Metadata(&mut metadata))?;
                continue;
            }
            seen += 1;
            if seen > self.max_tensors {
                return Err(de::Error::custom(format!(
                    "more than {} tensors",
                    self.max_tensors
                )));
            }
            let entry: Entry = map.next_value()?;
            tensors.insert(name, entry);
        }
        Ok(Header { tensors, metadata })
    }
}

/// Reads a `__metadata__` value straight into the kept map, replacing what an earlier
/// `__metadata__` left there, as a repeated key does. A value that is not an object keeps
/// nothing, as before. Only `ss_base_model_version` is read from it (`lora::base`, a hint
/// after the tensor names), so what does not fit the budget is read past, not refused.
struct Metadata<'a>(&'a mut BTreeMap<String, String>);

impl<'de> DeserializeSeed<'de> for Metadata<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        self.0.clear();
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Metadata<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any value")
    }

    /// Each string entry is kept while it fits [`MAX_KEPT_METADATA`]; one that does not is
    /// read past, and once [`MAX_METADATA_ENTRIES`] are kept the rest are skipped unread.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let kept = self.0;
        // The key and value bytes in `kept`.
        let mut bytes = 0u64;
        while kept.len() < MAX_METADATA_ENTRIES {
            let Some(key) = map.next_key::<String>()? else {
                return Ok(());
            };
            // A repeated key: the file's last value wins, so the earlier one goes either way.
            if let Some(old) = kept.remove(&key) {
                bytes -= kept_cost(&key, &old);
            }
            let value = match MAX_KEPT_METADATA.checked_sub(bytes + key.len() as u64) {
                Some(room) => map.next_value_seed(StringWithin(room))?,
                None => map.next_value::<IgnoredAny>().map(|_| None)?,
            };
            if let Some(value) = value {
                bytes += kept_cost(&key, &value);
                kept.insert(key, value);
            }
        }
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<(), A::Error> {
        skip_seq(seq)
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}

/// What a kept metadata entry takes from [`MAX_KEPT_METADATA`].
fn kept_cost(key: &str, value: &str) -> u64 {
    (key.len() + value.len()) as u64
}

/// A string value of at most `.0` bytes; a longer string, or any other value, is skipped
/// unkept and gives `None`.
struct StringWithin(u64);

impl<'de> DeserializeSeed<'de> for StringWithin {
    type Value = Option<String>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Option<String>, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for StringWithin {
    type Value = Option<String>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any value")
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Option<String>, E> {
        Ok((s.len() as u64 <= self.0).then(|| s.to_string()))
    }
    fn visit_string<E: de::Error>(self, s: String) -> Result<Option<String>, E> {
        Ok((s.len() as u64 <= self.0).then_some(s))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Option<String>, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(None)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Option<String>, A::Error> {
        skip_seq(seq).map(|()| None)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Option<String>, E> {
        Ok(None)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Option<String>, E> {
        Ok(None)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Option<String>, E> {
        Ok(None)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Option<String>, E> {
        Ok(None)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Option<String>, E> {
        Ok(None)
    }
}

fn skip_seq<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
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

    /// A safetensors header of `n` one-element tensors, each with its `data_offsets`, and
    /// `tail` written after the last entry, inside the object.
    fn many_tensors(n: usize, tail: &str) -> Vec<u8> {
        let mut json = String::from("{");
        for i in 0..n {
            json.push_str(&format!(
                r#""t{i}":{{"dtype":"F16","shape":[1],"data_offsets":[{a},{b}]}},"#,
                a = 2 * i,
                b = 2 * i + 2
            ));
        }
        json.push_str(tail);
        json.push('}');
        let mut out = (json.len() as u64).to_le_bytes().to_vec();
        out.extend(json.into_bytes());
        out
    }

    #[test]
    fn a_safetensors_header_with_more_tensors_than_gguf_allows_is_refused() {
        // Within the 100 MiB cap (about 13 MB), one tensor past GGUF's limit.
        let n = gguf::MAX_TENSORS as usize + 1;
        let file = many_tensors(n, r#""__metadata__":{}"#);
        assert!((file.len() as u64) < SAFETENSORS_HEADER_CAP);
        match parse_prefix(&file) {
            Err(IndexError::Corrupt(why)) => assert!(why.contains("tensors"), "{why}"),
            other => panic!(
                "expected a refusal, got {:?}",
                other.map(|ix| ix.tensors.len())
            ),
        }
    }

    #[test]
    fn the_tensor_count_is_refused_as_it_is_counted_before_the_rest_is_parsed() {
        // Damaged JSON after the entry past the limit: a parser that counted only at the end
        // would report the syntax error, or keep every entry first.
        let file = many_tensors(gguf::MAX_TENSORS as usize + 1, r#""broken": [1, 2,"#);
        match parse_prefix(&file) {
            Err(IndexError::Corrupt(why)) => {
                assert!(why.contains("tensors"), "the count stops it first: {why}")
            }
            other => panic!(
                "expected a refusal, got {:?}",
                other.map(|ix| ix.tensors.len())
            ),
        }
        // At the limit itself, the same entries parse.
        let file = many_tensors(gguf::MAX_TENSORS as usize, r#""__metadata__":{"a":"b"}"#);
        let ix = parse_prefix(&file).unwrap();
        assert_eq!(ix.tensors.len(), gguf::MAX_TENSORS as usize);
        assert_eq!(ix.metadata.get("a").map(String::as_str), Some("b"));
    }

    #[test]
    fn a_safetensors_entry_keeps_only_its_dtype_and_shape_and_a_repeated_name_keeps_the_last() {
        // nlohmann's object keeps the last value of a repeated key, as serde_json's map did.
        let json = br#"{"a":{"dtype":"F16","shape":[2],"data_offsets":[0,4],"extra":{"deep":[1,{"x":2}]}},"a":{"dtype":"BF16","shape":[3],"data_offsets":[0,6]},"__metadata__":{"s":"v","n":3,"o":{"k":"v"}}}"#;
        let mut file = (json.len() as u64).to_le_bytes().to_vec();
        file.extend_from_slice(json);
        let ix = parse_prefix(&file).unwrap();
        assert_eq!(
            ix.tensors,
            [TensorInfo {
                name: "a".into(),
                dtype: "BF16".into(),
                shape: vec![3]
            }]
        );
        assert_eq!(
            ix.metadata.into_iter().collect::<Vec<_>>(),
            [("s".to_string(), "v".to_string())]
        );
        // An entry without a dtype or shape is still damage.
        let json = br#"{"a":{"shape":[2]}}"#;
        let mut file = (json.len() as u64).to_le_bytes().to_vec();
        file.extend_from_slice(json);
        assert!(matches!(parse_prefix(&file), Err(IndexError::Corrupt(_))));
        let json = br#"{"a":{"dtype":"F16","shape":[-1]}}"#;
        let mut file = (json.len() as u64).to_le_bytes().to_vec();
        file.extend_from_slice(json);
        assert!(matches!(parse_prefix(&file), Err(IndexError::Corrupt(_))));
    }

    /// A safetensors file whose header is `__metadata__` holding `entries` in this order,
    /// then one tensor `t`.
    fn metadata_heavy(entries: impl Iterator<Item = (String, String)>) -> Vec<u8> {
        let mut json = String::from(r#"{"__metadata__":{"#);
        for (i, (k, v)) in entries.enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(r#""{k}":"{v}""#));
        }
        json.push_str(r#"},"t":{"dtype":"F16","shape":[2],"data_offsets":[0,4]}}"#);
        let mut out = (json.len() as u64).to_le_bytes().to_vec();
        out.extend(json.into_bytes());
        out
    }

    /// The key and value bytes `metadata` keeps.
    fn kept_bytes(metadata: &BTreeMap<String, String>) -> u64 {
        metadata
            .iter()
            .map(|(k, v)| (k.len() + v.len()) as u64)
            .sum()
    }

    #[test]
    fn metadata_stops_being_kept_at_its_byte_budget() {
        // Entries of exactly 1 KiB of key and value: the budget holds 16,384 of them, and the
        // header (about 17.4 MB) is well within its 100 MiB cap.
        const ENTRY: usize = 1024;
        let fit = (MAX_KEPT_METADATA as usize) / ENTRY;
        let key = |i: usize| format!("k{i:07}");
        let file =
            metadata_heavy((0..fit + 1000).map(|i| (key(i), "v".repeat(ENTRY - key(i).len()))));
        assert!((file.len() as u64) < SAFETENSORS_HEADER_CAP);
        let ix = parse_prefix(&file).unwrap();
        assert_eq!(kept_bytes(&ix.metadata), MAX_KEPT_METADATA);
        assert_eq!(ix.metadata.len(), fit);
        assert!(ix.metadata.contains_key(&key(fit - 1)));
        assert!(!ix.metadata.contains_key(&key(fit)));
        // What follows the metadata still reads.
        assert_eq!(ix.tensors.len(), 1);
        assert_eq!(ix.tensors[0].shape, [2]);
    }

    #[test]
    fn a_metadata_value_past_the_budget_is_read_past_and_later_ones_are_kept() {
        // A kohya-style header whose tag list alone is over the budget still gives its base
        // model, the one key the app reads (`lora::base`).
        let huge = "x".repeat(MAX_KEPT_METADATA as usize + 1);
        let file = metadata_heavy(
            [
                ("ss_tag_frequency".to_string(), huge),
                ("ss_base_model_version".into(), "sdxl_base_v1-0".into()),
            ]
            .into_iter(),
        );
        let ix = parse_prefix(&file).unwrap();
        assert_eq!(
            ix.metadata.keys().collect::<Vec<_>>(),
            ["ss_base_model_version"]
        );
        assert_eq!(ix.metadata["ss_base_model_version"], "sdxl_base_v1-0");
    }

    #[test]
    fn metadata_stops_being_kept_at_its_entry_cap() {
        // The re-review's shape, scaled down: many tiny entries, each nearly free in bytes.
        let n = MAX_METADATA_ENTRIES + 5_000;
        let key = |i: usize| format!("m{i:06}");
        let file = metadata_heavy((0..n).map(|i| (key(i), String::new())));
        let ix = parse_prefix(&file).unwrap();
        assert_eq!(ix.metadata.len(), MAX_METADATA_ENTRIES);
        assert!(ix.metadata.contains_key(&key(MAX_METADATA_ENTRIES - 1)));
        assert!(!ix.metadata.contains_key(&key(MAX_METADATA_ENTRIES)));
        assert_eq!(ix.tensors.len(), 1);
    }

    #[test]
    fn a_shape_of_more_than_8_dimensions_is_damage() {
        let file = |shape: &[u64]| {
            safetensors(&json!({"t": {"dtype": "F16", "shape": shape, "data_offsets": [0, 2]}}))
        };
        let ix = parse_prefix(&file(&[1; 8])).unwrap();
        assert_eq!(ix.tensors[0].shape, [1; 8]);
        match parse_prefix(&file(&[1; 9])) {
            Err(IndexError::Corrupt(why)) => assert!(why.contains("dimensions"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        // The engine's own limit (`SD_MAX_DIMS`) is 5, and a real 5-D file still reads.
        let wan = crate::sd::testing::fixture("wan21-t2v-1.3b");
        let ix = parse_prefix(&crate::sd::testing::file_of(&wan)).unwrap();
        assert_eq!(ix.tensors.iter().map(|t| t.shape.len()).max(), Some(5));
    }

    /// The index the reader gave before its typed visitor, through a `serde_json::Value` map:
    /// every entry but `__metadata__` a tensor, and the metadata's string values.
    fn as_before(file: &[u8]) -> (Vec<TensorInfo>, BTreeMap<String, String>) {
        let v: Value = serde_json::from_slice(&file[8..]).unwrap();
        let obj = v.as_object().unwrap();
        let mut tensors: Vec<TensorInfo> = obj
            .iter()
            .filter(|(k, _)| *k != "__metadata__")
            .map(|(name, e)| TensorInfo {
                name: name.clone(),
                dtype: e["dtype"].as_str().unwrap().into(),
                shape: e["shape"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| d.as_u64().unwrap())
                    .collect(),
            })
            .collect();
        // By name, whichever map `serde_json` was built with.
        tensors.sort_by(|a, b| a.name.cmp(&b.name));
        let metadata = obj
            .get("__metadata__")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        (tensors, metadata)
    }

    #[test]
    fn ordinary_headers_and_their_metadata_read_exactly_as_before() {
        let raw = |json: &str| {
            let mut file = (json.len() as u64).to_le_bytes().to_vec();
            file.extend_from_slice(json.as_bytes());
            file
        };
        // Every safetensors fixture of a real file, written back as a header.
        let mut files: Vec<Vec<u8>> = [
            "ae-flux1",
            "clip-l",
            "flux-lora-realism",
            "flux1-dev",
            "flux2-klein-4b",
            "hunyuan-video-15",
            "lumina2",
            "sd14-diffusers-text-encoder",
            "sd14-diffusers-unet",
            "sd15",
            "sd35-medium",
            "sdxl-lora-pixel-art",
            "sdxl",
            "wan21-t2v-1.3b",
            "z-image-turbo",
        ]
        .iter()
        .map(|name| {
            let ix = crate::sd::testing::fixture(name);
            assert_eq!(ix.format, FileFormat::Safetensors, "{name}");
            crate::sd::testing::file_of(&ix)
        })
        .collect();
        // Kohya metadata with numbers, objects, lists and an escaped string among the strings,
        // and a repeated key; a second `__metadata__` replaces the first, as a JSON object's
        // repeated key does.
        let kohya = raw(
            r#"{"__metadata__":{"old":"gone"},"lora_unet_a.alpha":{"dtype":"F32","shape":[],"data_offsets":[0,4]},"__metadata__":{"ss_base_model_version":"sdxl_base_v1-0","ss_network_dim":"16","ss_tag_frequency":"{\"img\": {\"cat\": 3}}","n":3,"o":{"k":"v"},"l":["a"],"z":null,"t":true,"ss_output_name":"first","ss_output_name":"pixel \u00fc"},"lora_unet_a.lora_down.weight":{"dtype":"F16","shape":[16,320],"data_offsets":[4,10244]}}"#,
        );
        files.push(kohya.clone());
        // `__metadata__` that is not an object is ignored, as before.
        files.push(raw(
            r#"{"__metadata__":"text","t":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}"#,
        ));
        for file in &files {
            let ix = parse_prefix(file).unwrap();
            assert_eq!((ix.tensors, ix.metadata), as_before(file));
        }
        let kohya = parse_prefix(&kohya).unwrap();
        assert_eq!(kohya.metadata["ss_output_name"], "pixel \u{fc}");
        assert_eq!(kohya.metadata["ss_tag_frequency"], r#"{"img": {"cat": 3}}"#);
        assert!(!kohya.metadata.contains_key("old"));
        assert!(!kohya.metadata.contains_key("n"));
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

    /// A GGUF with a string array, a fixed-size array and a tensor table.
    fn gguf_with_arrays() -> Vec<u8> {
        gguf::testing::Writer::new()
            .str("general.architecture", "flux")
            .str_array("general.tags", &["alpha", "beta", "gamma", "delta"])
            .u32_array("x.heads", &[1, 2, 3, 4, 5, 6])
            .shaped_tensor("a.weight", &[4, 4])
            .shaped_tensor("b.weight", &[8])
            .build()
    }

    #[test]
    fn every_cut_inside_arrays_and_the_table_is_truncated() {
        let g = gguf_with_arrays();
        let end = match parse_prefix(&g) {
            Ok(_) => {
                // The first length that parses is where the table ends.
                (0..g.len())
                    .find(|&n| parse_prefix(&g[..n]).is_ok())
                    .unwrap()
            }
            Err(e) => panic!("{e}"),
        };
        assert!(end > 100, "arrays should come before the end");
        for cut in 0..end {
            assert!(
                matches!(parse_prefix(&g[..cut]), Err(IndexError::Truncated { .. })),
                "cut {cut}"
            );
        }
        assert_eq!(parse_prefix(&g[..end]).unwrap().tensors.len(), 2);
    }

    #[test]
    fn a_gguf_table_ending_past_the_cap_is_too_large() {
        // Metadata of a 70 MiB kept string pushes the table end past 64 MiB.
        let big = "x".repeat(70 * 1024 * 1024);
        let mut w = gguf::testing::Writer::new();
        w = w.str("general.architecture", "flux").str("pad", &big);
        let g = w.shaped_tensor("a.weight", &[4]).build();
        assert!(matches!(
            parse_prefix(&g),
            Err(IndexError::TooLarge {
                cap: GGUF_HEADER_CAP
            })
        ));
        // And a prefix that is still inside the metadata past the cap is TooLarge too.
        assert!(matches!(
            parse_prefix(&g[..1024]),
            Err(IndexError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_text_file_is_not_weights() {
        assert!(matches!(
            parse_prefix(b"hello there, this is plain text"),
            Err(IndexError::NotWeights)
        ));
    }
}
