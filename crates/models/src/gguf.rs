//! A reader for the metadata at the start of a GGUF file.
//!
//! Model files come from the internet, so this treats every length and count as
//! hostile: nothing is allocated from a length that the rest of the file cannot hold,
//! large arrays (the tokenizer vocabulary) are skipped without being kept, and only
//! the keys the app needs are stored.
//!
//! Format: <https://github.com/ggml-org/ggml/blob/master/docs/gguf.md>. Only
//! little-endian files are read; big-endian GGUF is reported as unsupported.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const MAGIC: [u8; 4] = *b"GGUF";
/// Metadata entries beyond this are treated as a damaged file.
const MAX_KV: u64 = 1_000_000;
/// String values longer than this are skipped rather than kept (chat templates are
/// tens of kilobytes; nothing the app needs is larger).
const MAX_KEPT_STRING: u64 = 4 * 1024 * 1024;
/// Keys longer than this mean a damaged file.
const MAX_KEY: u64 = 65_536;
/// Arrays with at most this many elements are kept (per-layer head counts and the
/// like); longer ones (vocabularies, merges) are skipped.
const MAX_KEPT_ARRAY: u64 = 4096;
/// Arrays nested deeper than this mean a damaged file.
const MAX_DEPTH: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum GgufError {
    #[error("not a GGUF file")]
    NotGguf,
    #[error("GGUF version {0} is not supported")]
    UnsupportedVersion(u32),
    #[error("big-endian GGUF files are not supported")]
    BigEndian,
    #[error("the file is damaged or truncated: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// A metadata value. Long arrays are summarised as [`Value::SkippedArray`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    String(String),
    Array(Vec<Value>),
    /// An array too long to keep, with its element type and length.
    SkippedArray {
        elem_type: u32,
        len: u64,
    },
    /// A string too long to keep, with its length in bytes.
    SkippedString(u64),
}

impl Value {
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Value::U64(v) => Some(v),
            Value::I64(v) if v >= 0 => Some(v as u64),
            Value::F64(v) if v >= 0.0 && v.fract() == 0.0 && v < 1.8e19 => Some(v as u64),
            Value::Bool(b) => Some(u64::from(b)),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::U64(v) => Some(v as f64),
            Value::I64(v) => Some(v as f64),
            Value::F64(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            Value::U64(v) => Some(v != 0),
            Value::I64(v) => Some(v != 0),
            _ => None,
        }
    }
}

/// The parsed header: version, tensor count and kept metadata.
#[derive(Debug, Clone)]
pub struct GgufHeader {
    pub version: u32,
    pub tensor_count: u64,
    pub metadata: BTreeMap<String, Value>,
}

impl GgufHeader {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.metadata.get(key)
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(Value::as_u64)
    }
    pub fn architecture(&self) -> Option<&str> {
        self.get_str("general.architecture")
    }
    /// `<architecture>.<suffix>`, the convention llama.cpp uses for model parameters.
    pub fn arch_u64(&self, suffix: &str) -> Option<u64> {
        let arch = self.architecture()?;
        self.get_u64(&format!("{arch}.{suffix}"))
    }
}

/// Reads the header of the GGUF file at `path`.
pub fn read_file(path: &Path) -> Result<GgufHeader, GgufError> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    read(BufReader::with_capacity(256 * 1024, file), len)
}

/// Reads as much metadata as `prefix` (the first bytes of a GGUF file, for example
/// from a ranged download) contains. Model parameters come before the tokenizer
/// arrays in practice, so a few megabytes are enough for the fit check.
pub fn read_prefix(prefix: &[u8]) -> Result<GgufHeader, GgufError> {
    read_inner(std::io::Cursor::new(prefix), prefix.len() as u64, true)
}

/// Reads a GGUF header from `reader`, which holds `len` bytes in total.
pub fn read<R: Read + Seek>(reader: R, len: u64) -> Result<GgufHeader, GgufError> {
    read_inner(reader, len, false)
}

fn read_inner<R: Read + Seek>(reader: R, len: u64, lenient: bool) -> Result<GgufHeader, GgufError> {
    let mut r = Reader {
        inner: reader,
        pos: 0,
        len,
    };
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).map_err(|_| GgufError::NotGguf)?;
    if magic != MAGIC {
        return Err(GgufError::NotGguf);
    }
    let version = r.u32()?;
    if version == 0 || version > 0xFFFF {
        if version.swap_bytes() <= 3 {
            return Err(GgufError::BigEndian);
        }
        return Err(GgufError::UnsupportedVersion(version));
    }
    if !(2..=3).contains(&version) {
        return Err(GgufError::UnsupportedVersion(version));
    }
    let tensor_count = r.u64()?;
    let kv_count = r.u64()?;
    if kv_count > MAX_KV || (!lenient && kv_count.saturating_mul(13) > r.remaining()) {
        return Err(GgufError::Corrupt(format!("{kv_count} metadata entries")));
    }
    let mut metadata = BTreeMap::new();
    for _ in 0..kv_count {
        match read_entry(&mut r) {
            Ok((key, value)) => {
                metadata.insert(key, value);
            }
            // A prefix ends mid-entry: keep what was read.
            Err(GgufError::Corrupt(_)) if lenient && !metadata.is_empty() => break,
            Err(e) => return Err(e),
        }
    }
    Ok(GgufHeader {
        version,
        tensor_count,
        metadata,
    })
}

fn read_entry<R: Read + Seek>(r: &mut Reader<R>) -> Result<(String, Value), GgufError> {
    let key_len = r.u64()?;
    if key_len > MAX_KEY {
        return Err(GgufError::Corrupt("metadata key too long".into()));
    }
    let key = r.string(key_len)?;
    let ty = r.u32()?;
    let value = r.value(ty, 0)?;
    Ok((key, value))
}

struct Reader<R> {
    inner: R,
    pos: u64,
    len: u64,
}

impl<R: Read + Seek> Reader<R> {
    fn remaining(&self) -> u64 {
        self.len.saturating_sub(self.pos)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), GgufError> {
        if buf.len() as u64 > self.remaining() {
            return Err(GgufError::Corrupt("unexpected end of file".into()));
        }
        self.inner.read_exact(buf).map_err(|e| match e.kind() {
            io::ErrorKind::UnexpectedEof => GgufError::Corrupt("unexpected end of file".into()),
            _ => GgufError::Io(e),
        })?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    fn skip(&mut self, n: u64) -> Result<(), GgufError> {
        if n > self.remaining() {
            return Err(GgufError::Corrupt("unexpected end of file".into()));
        }
        let n_i64 = i64::try_from(n).map_err(|_| GgufError::Corrupt("length overflow".into()))?;
        self.inner.seek(SeekFrom::Current(n_i64))?;
        self.pos += n;
        Ok(())
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        let mut b = [0u8; N];
        self.read_exact(&mut b)?;
        Ok(b)
    }
    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.bytes()?))
    }
    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.bytes()?))
    }

    fn string(&mut self, len: u64) -> Result<String, GgufError> {
        if len > self.remaining() {
            return Err(GgufError::Corrupt("string longer than the file".into()));
        }
        let mut buf = vec![0u8; len as usize];
        self.read_exact(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    fn value(&mut self, ty: u32, depth: u32) -> Result<Value, GgufError> {
        Ok(match ty {
            0 => Value::U64(u64::from(self.bytes::<1>()?[0])),
            1 => Value::I64(i64::from(self.bytes::<1>()?[0] as i8)),
            2 => Value::U64(u64::from(u16::from_le_bytes(self.bytes()?))),
            3 => Value::I64(i64::from(i16::from_le_bytes(self.bytes()?))),
            4 => Value::U64(u64::from(self.u32()?)),
            5 => Value::I64(i64::from(i32::from_le_bytes(self.bytes()?))),
            6 => Value::F64(f64::from(f32::from_le_bytes(self.bytes()?))),
            7 => Value::Bool(self.bytes::<1>()?[0] != 0),
            8 => {
                let len = self.u64()?;
                if len > MAX_KEPT_STRING {
                    self.skip(len)?;
                    Value::SkippedString(len)
                } else {
                    Value::String(self.string(len)?)
                }
            }
            9 => {
                if depth >= MAX_DEPTH {
                    return Err(GgufError::Corrupt("arrays nested too deeply".into()));
                }
                let elem = self.u32()?;
                let count = self.u64()?;
                let min_size = match (fixed_size(elem), elem) {
                    (Some(size), _) => size,
                    (None, 8) => 8,
                    (None, 9) => 12,
                    _ => return Err(GgufError::Corrupt(format!("unknown value type {elem}"))),
                };
                if count.saturating_mul(min_size) > self.remaining() {
                    return Err(GgufError::Corrupt("array longer than the file".into()));
                }
                if count > MAX_KEPT_ARRAY {
                    match fixed_size(elem) {
                        Some(size) => self.skip(count * size)?,
                        None => {
                            for _ in 0..count {
                                self.skip_value(elem, depth + 1)?;
                            }
                        }
                    }
                    Value::SkippedArray {
                        elem_type: elem,
                        len: count,
                    }
                } else {
                    let mut items = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        items.push(self.value(elem, depth + 1)?);
                    }
                    Value::Array(items)
                }
            }
            10 => Value::U64(self.u64()?),
            11 => Value::I64(i64::from_le_bytes(self.bytes()?)),
            12 => Value::F64(f64::from_le_bytes(self.bytes()?)),
            other => return Err(GgufError::Corrupt(format!("unknown value type {other}"))),
        })
    }

    fn skip_value(&mut self, ty: u32, depth: u32) -> Result<(), GgufError> {
        if let Some(size) = fixed_size(ty) {
            return self.skip(size);
        }
        match ty {
            8 => {
                let len = self.u64()?;
                self.skip(len)
            }
            9 => {
                if depth >= MAX_DEPTH {
                    return Err(GgufError::Corrupt("arrays nested too deeply".into()));
                }
                let elem = self.u32()?;
                let count = self.u64()?;
                match fixed_size(elem) {
                    Some(size) => self.skip(count.saturating_mul(size)),
                    None => {
                        if count.saturating_mul(8) > self.remaining() {
                            return Err(GgufError::Corrupt("array longer than the file".into()));
                        }
                        for _ in 0..count {
                            self.skip_value(elem, depth + 1)?;
                        }
                        Ok(())
                    }
                }
            }
            other => Err(GgufError::Corrupt(format!("unknown value type {other}"))),
        }
    }
}

fn fixed_size(ty: u32) -> Option<u64> {
    match ty {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4..=6 => Some(4),
        10..=12 => Some(8),
        _ => None,
    }
}

/// Builds GGUF headers for tests (here and in other crates' tests).
#[doc(hidden)]
pub mod testing {
    /// A minimal GGUF v3 writer for metadata.
    #[derive(Default)]
    pub struct Writer {
        kvs: Vec<u8>,
        count: u64,
        tensors: u64,
    }

    impl Writer {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn tensors(mut self, n: u64) -> Self {
            self.tensors = n;
            self
        }
        fn key(&mut self, k: &str, ty: u32) {
            self.kvs.extend((k.len() as u64).to_le_bytes());
            self.kvs.extend(k.as_bytes());
            self.kvs.extend(ty.to_le_bytes());
            self.count += 1;
        }
        pub fn u32(mut self, k: &str, v: u32) -> Self {
            self.key(k, 4);
            self.kvs.extend(v.to_le_bytes());
            self
        }
        pub fn u64(mut self, k: &str, v: u64) -> Self {
            self.key(k, 10);
            self.kvs.extend(v.to_le_bytes());
            self
        }
        pub fn f32(mut self, k: &str, v: f32) -> Self {
            self.key(k, 6);
            self.kvs.extend(v.to_le_bytes());
            self
        }
        pub fn bool(mut self, k: &str, v: bool) -> Self {
            self.key(k, 7);
            self.kvs.push(u8::from(v));
            self
        }
        pub fn str(mut self, k: &str, v: &str) -> Self {
            self.key(k, 8);
            self.kvs.extend((v.len() as u64).to_le_bytes());
            self.kvs.extend(v.as_bytes());
            self
        }
        pub fn u32_array(mut self, k: &str, v: &[u32]) -> Self {
            self.key(k, 9);
            self.kvs.extend(4u32.to_le_bytes());
            self.kvs.extend((v.len() as u64).to_le_bytes());
            for x in v {
                self.kvs.extend(x.to_le_bytes());
            }
            self
        }
        pub fn str_array(mut self, k: &str, v: &[&str]) -> Self {
            self.key(k, 9);
            self.kvs.extend(8u32.to_le_bytes());
            self.kvs.extend((v.len() as u64).to_le_bytes());
            for s in v {
                self.kvs.extend((s.len() as u64).to_le_bytes());
                self.kvs.extend(s.as_bytes());
            }
            self
        }
        pub fn build(self) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend(*b"GGUF");
            out.extend(3u32.to_le_bytes());
            out.extend(self.tensors.to_le_bytes());
            out.extend(self.count.to_le_bytes());
            out.extend(self.kvs);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Writer;
    use super::*;
    use std::io::Cursor;

    fn parse(bytes: &[u8]) -> Result<GgufHeader, GgufError> {
        read(Cursor::new(bytes), bytes.len() as u64)
    }

    #[test]
    fn reads_scalars_strings_and_arrays() {
        let tokens: Vec<String> = (0..5000).map(|i| format!("t{i}")).collect();
        let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let bytes = Writer::new()
            .tensors(7)
            .str("general.architecture", "llama")
            .u32("llama.context_length", 131072)
            .u64("llama.block_count", 32)
            .f32("llama.rope.freq_base", 500000.0)
            .bool("general.flag", true)
            .u32_array("llama.attention.head_count_kv", &[8, 8, 0, 8])
            .str_array("tokenizer.ggml.tokens", &token_refs)
            .str("tokenizer.chat_template", "{% if tools %}x{% endif %}")
            .build();
        let h = parse(&bytes).unwrap();
        assert_eq!(h.version, 3);
        assert_eq!(h.tensor_count, 7);
        assert_eq!(h.architecture(), Some("llama"));
        assert_eq!(h.arch_u64("context_length"), Some(131072));
        assert_eq!(h.arch_u64("block_count"), Some(32));
        assert_eq!(
            h.get("llama.rope.freq_base").and_then(Value::as_f64),
            Some(500000.0)
        );
        assert_eq!(h.get("general.flag").and_then(Value::as_bool), Some(true));
        assert_eq!(
            h.get("llama.attention.head_count_kv"),
            Some(&Value::Array(vec![
                Value::U64(8),
                Value::U64(8),
                Value::U64(0),
                Value::U64(8)
            ]))
        );
        assert_eq!(
            h.get("tokenizer.ggml.tokens"),
            Some(&Value::SkippedArray {
                elem_type: 8,
                len: 5000
            })
        );
        assert!(
            h.get_str("tokenizer.chat_template")
                .unwrap()
                .contains("tools")
        );
    }

    #[test]
    fn rejects_non_gguf_and_bad_versions() {
        assert!(matches!(parse(b"GGML\0\0\0\0"), Err(GgufError::NotGguf)));
        assert!(matches!(parse(b"GG"), Err(GgufError::NotGguf)));
        let mut be = Vec::from(*b"GGUF");
        be.extend(3u32.to_be_bytes());
        be.extend([0u8; 16]);
        assert!(matches!(parse(&be), Err(GgufError::BigEndian)));
        let mut v9 = Vec::from(*b"GGUF");
        v9.extend(9u32.to_le_bytes());
        v9.extend([0u8; 16]);
        assert!(matches!(parse(&v9), Err(GgufError::UnsupportedVersion(9))));
    }

    #[test]
    fn hostile_lengths_fail_without_allocating() {
        // A metadata count far larger than the file.
        let mut b = Vec::from(*b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(u64::MAX.to_le_bytes());
        assert!(matches!(parse(&b), Err(GgufError::Corrupt(_))));

        // A string length of 2^62.
        let mut b = Vec::from(*b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.push(b'k');
        b.extend(8u32.to_le_bytes());
        b.extend((1u64 << 62).to_le_bytes());
        b.extend([0u8; 64]);
        assert!(matches!(parse(&b), Err(GgufError::Corrupt(_))));

        // An array claiming 2^60 u32 elements.
        let mut b = Vec::from(*b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.push(b'k');
        b.extend(9u32.to_le_bytes());
        b.extend(4u32.to_le_bytes());
        b.extend((1u64 << 60).to_le_bytes());
        b.extend([0u8; 64]);
        assert!(matches!(parse(&b), Err(GgufError::Corrupt(_))));

        // Unknown value type.
        let mut b = Vec::from(*b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.push(b'k');
        b.extend(99u32.to_le_bytes());
        b.extend([0u8; 16]);
        assert!(matches!(parse(&b), Err(GgufError::Corrupt(_))));
    }

    #[test]
    fn truncated_file_is_corrupt() {
        let bytes = Writer::new()
            .str("general.architecture", "qwen3")
            .u32("qwen3.block_count", 28)
            .build();
        for cut in 24..bytes.len() {
            assert!(parse(&bytes[..cut]).is_err(), "cut at {cut} parsed");
        }
        assert!(parse(&bytes).is_ok());
    }

    #[test]
    fn prefix_keeps_entries_before_the_cut() {
        let tokens: Vec<String> = (0..20_000).map(|i| format!("token{i}")).collect();
        let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let bytes = Writer::new()
            .str("general.architecture", "llama")
            .u32("llama.block_count", 32)
            .str_array("tokenizer.ggml.tokens", &refs)
            .u32("after", 1)
            .build();
        let h = read_prefix(&bytes[..4096]).unwrap();
        assert_eq!(h.arch_u64("block_count"), Some(32));
        assert!(h.get("after").is_none());
        assert!(read_prefix(&bytes[..10]).is_err());
        // The strict reader refuses the same prefix.
        assert!(read(Cursor::new(&bytes[..4096]), 4096).is_err());
    }

    #[test]
    fn deep_nesting_is_rejected() {
        let mut b = Vec::from(*b"GGUF");
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.extend(1u64.to_le_bytes());
        b.push(b'k');
        b.extend(9u32.to_le_bytes());
        for _ in 0..10 {
            b.extend(9u32.to_le_bytes());
            b.extend(1u64.to_le_bytes());
        }
        b.extend([0u8; 64]);
        assert!(parse(&b).is_err());
    }

    /// Reads a real model when `TEST_GGUF` points at one (CI sets it).
    #[test]
    fn real_model_if_available() {
        let Ok(path) = std::env::var("TEST_GGUF") else {
            return;
        };
        let h = read_file(Path::new(&path)).unwrap();
        assert!(h.architecture().is_some());
        assert!(h.arch_u64("context_length").unwrap() >= 2048);
        assert!(h.arch_u64("block_count").unwrap() > 0);
        assert!(h.get_str("tokenizer.chat_template").is_some());
    }
}
