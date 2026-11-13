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
    read_header(&mut r, lenient)
}

/// Reads the header from the start of `r`, leaving `r` just after the metadata (where the
/// tensor table starts).
fn read_header<R: Read + Seek>(r: &mut Reader<R>, lenient: bool) -> Result<GgufHeader, GgufError> {
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
        match read_entry(r) {
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

// ---- 0.0.6 item 6: the tensor table (WP10b) ----------------------------------------------

/// Tensor tables longer than this mean a damaged file (the largest models llama.cpp loads
/// have a few thousand tensors).
const MAX_TENSORS: u64 = 65_536;
/// Tensor names longer than this mean a damaged file (llama.cpp itself allows 63 bytes).
const MAX_TENSOR_NAME: u64 = 256;
/// Dimensions per tensor (ggml's `GGML_MAX_DIMS`).
const MAX_DIMS: u32 = 4;
/// The data alignment when the file names none (`GGUF_DEFAULT_ALIGNMENT`).
const DEFAULT_ALIGNMENT: u64 = 32;
/// The smallest entry of the tensor table: an empty name, no dimensions, a type, an offset.
const MIN_TENSOR_ENTRY: u64 = 8 + 4 + 4 + 8;

/// One tensor of a model file: its name and the bytes it takes in the file, worked out
/// from the gap to the next tensor's offset (the last one runs to the end of the file), so
/// it includes the padding llama.cpp's writer puts after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TensorEntry {
    pub name: String,
    pub bytes: u64,
}

/// A tensor table entry as written in the file, before sizes are known.
#[derive(Clone, Debug, PartialEq, Eq)]
struct RawTensor {
    name: String,
    /// ggml's type number (only the tests look at it, to check the sizes).
    #[cfg_attr(not(test), allow(dead_code))]
    ggml_type: u32,
    /// The element count per dimension (only the tests look at it).
    #[cfg_attr(not(test), allow(dead_code))]
    dims: Vec<u64>,
    /// Offset from the start of the data section.
    offset: u64,
}

/// Reads the tensor table of the GGUF file at `path`: every tensor's name and size.
/// A split model has one table per file; read each file.
///
/// Refused as damaged: more than 65,536 tensors, a name longer than 256 bytes or not UTF-8,
/// more than four dimensions, an alignment that is not a power of two, and offsets that
/// overflow, lie beyond the end of the file or do not increase. A file cut off inside its
/// last tensor is not noticed (no type table is kept); llama.cpp refuses to load it.
pub fn read_tensor_table(path: &Path) -> Result<Vec<TensorEntry>, GgufError> {
    read_file_with_tensors(path).map(|(_, tensors)| tensors)
}

/// [`read_file`] and [`read_tensor_table`] in one pass over the file.
pub fn read_file_with_tensors(path: &Path) -> Result<(GgufHeader, Vec<TensorEntry>), GgufError> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    read_with_tensors(BufReader::with_capacity(256 * 1024, file), len)
}

/// [`read`] followed by the tensor table, from `reader`, which holds `len` bytes in total.
pub fn read_with_tensors<R: Read + Seek>(
    reader: R,
    len: u64,
) -> Result<(GgufHeader, Vec<TensorEntry>), GgufError> {
    let mut r = Reader {
        inner: reader,
        pos: 0,
        len,
    };
    let header = read_header(&mut r, false)?;
    let raw = read_raw_tensors(&mut r, &header)?;
    let tensors = sizes_from_offsets(&raw, r.pos, len, alignment(&header)?)?;
    Ok((header, tensors))
}

/// The data alignment the file names (`general.alignment`, an unsigned integer that is a
/// power of two), or the default.
fn alignment(header: &GgufHeader) -> Result<u64, GgufError> {
    match header.get("general.alignment") {
        None => Ok(DEFAULT_ALIGNMENT),
        Some(Value::U64(a)) if a.is_power_of_two() => Ok(*a),
        Some(_) => Err(GgufError::Corrupt("bad data alignment".into())),
    }
}

/// The tensor table entries, read from just after the metadata.
fn read_raw_tensors<R: Read + Seek>(
    r: &mut Reader<R>,
    header: &GgufHeader,
) -> Result<Vec<RawTensor>, GgufError> {
    let count = header.tensor_count;
    if count > MAX_TENSORS || count.saturating_mul(MIN_TENSOR_ENTRY) > r.remaining() {
        return Err(GgufError::Corrupt(format!("{count} tensors")));
    }
    // At most 65,536 by the check above.
    let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for _ in 0..count {
        let name_len = r.u64()?;
        if name_len > MAX_TENSOR_NAME {
            return Err(GgufError::Corrupt("tensor name too long".into()));
        }
        let name = r.utf8(name_len)?;
        let n_dims = r.u32()?;
        if n_dims > MAX_DIMS {
            return Err(GgufError::Corrupt(format!(
                "tensor with {n_dims} dimensions"
            )));
        }
        let mut dims = Vec::with_capacity(4);
        for _ in 0..n_dims {
            dims.push(r.u64()?);
        }
        let ggml_type = r.u32()?;
        let offset = r.u64()?;
        out.push(RawTensor {
            name,
            ggml_type,
            dims,
            offset,
        });
    }
    Ok(out)
}

/// Sizes from the gaps between the offsets: the data section starts at `table_end`
/// rounded up to `alignment`, offsets must increase, and the last tensor runs to `len`.
fn sizes_from_offsets(
    raw: &[RawTensor],
    table_end: u64,
    len: u64,
    alignment: u64,
) -> Result<Vec<TensorEntry>, GgufError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let data_start = table_end
        .checked_next_multiple_of(alignment)
        .ok_or_else(|| GgufError::Corrupt("data section beyond any file".into()))?;
    let mut starts = Vec::with_capacity(raw.len());
    for t in raw {
        let start = data_start
            .checked_add(t.offset)
            .ok_or_else(|| GgufError::Corrupt("tensor offset overflows".into()))?;
        if start > len {
            return Err(GgufError::Corrupt(
                "tensor beyond the end of the file".into(),
            ));
        }
        if starts.last().is_some_and(|&previous| start <= previous) {
            return Err(GgufError::Corrupt("tensor offsets do not increase".into()));
        }
        starts.push(start);
    }
    Ok(raw
        .iter()
        .zip(&starts)
        .enumerate()
        .map(|(i, (t, &start))| {
            let end = starts.get(i + 1).copied().unwrap_or(len);
            TensorEntry {
                name: t.name.clone(),
                // Offsets increase and none is beyond `len`.
                bytes: end.saturating_sub(start),
            }
        })
        .collect())
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

    /// A string that must be UTF-8 (tensor names), at most `len` bytes as checked by the
    /// caller.
    fn utf8(&mut self, len: u64) -> Result<String, GgufError> {
        if len > self.remaining() {
            return Err(GgufError::Corrupt("string longer than the file".into()));
        }
        let size =
            usize::try_from(len).map_err(|_| GgufError::Corrupt("length overflow".into()))?;
        let mut buf = vec![0u8; size];
        self.read_exact(&mut buf)?;
        String::from_utf8(buf).map_err(|_| GgufError::Corrupt("tensor name is not UTF-8".into()))
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
    /// A minimal GGUF v3 writer for metadata and, with [`Writer::tensor`], a tensor table
    /// and its (zero-filled) data.
    #[derive(Default)]
    pub struct Writer {
        kvs: Vec<u8>,
        count: u64,
        tensors: u64,
        /// Tensors added with [`Writer::tensor`]: name and data size in bytes.
        table: Vec<(String, u64)>,
        /// The data alignment ([`Writer::alignment`]); 0 means the default of 32.
        align: u64,
    }

    impl Writer {
        pub fn new() -> Self {
            Self::default()
        }
        /// Sets the tensor count written in the header (without a table).
        pub fn tensors(mut self, n: u64) -> Self {
            self.tensors = n;
            self
        }
        /// Adds a tensor of `bytes` bytes (written as one dimension of 8-bit integers).
        /// With any tensor added, the header's tensor count is the number added and the
        /// file ends with the table, padding and the data, as llama.cpp writes it.
        pub fn tensor(mut self, name: &str, bytes: u64) -> Self {
            self.table.push((name.to_string(), bytes));
            self
        }
        /// Writes `general.alignment` and pads the data to it.
        pub fn alignment(mut self, a: u32) -> Self {
            self.align = u64::from(a);
            self.u32("general.alignment", a)
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
            let tensors = if self.table.is_empty() {
                self.tensors
            } else {
                self.table.len() as u64
            };
            out.extend(tensors.to_le_bytes());
            out.extend(self.count.to_le_bytes());
            out.extend(self.kvs);
            if self.table.is_empty() {
                return out;
            }
            let align = if self.align == 0 { 32 } else { self.align };
            let pad = |n: u64| n.div_ceil(align) * align;
            let mut offset = 0u64;
            for (name, bytes) in &self.table {
                out.extend((name.len() as u64).to_le_bytes());
                out.extend(name.as_bytes());
                out.extend(1u32.to_le_bytes());
                out.extend(bytes.to_le_bytes());
                // GGML_TYPE_I8: one byte per element.
                out.extend(24u32.to_le_bytes());
                out.extend(offset.to_le_bytes());
                offset += pad(*bytes);
            }
            out.resize(pad(out.len() as u64) as usize, 0);
            for (_, bytes) in &self.table {
                out.resize(out.len() + pad(*bytes) as usize, 0);
            }
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

    // ---- the tensor table (0.0.6 item 6) ------------------------------------------------

    fn table(bytes: &[u8]) -> Result<Vec<TensorEntry>, GgufError> {
        read_with_tensors(Cursor::new(bytes), bytes.len() as u64).map(|(_, t)| t)
    }

    fn entries(list: &[(&str, u64)]) -> Vec<TensorEntry> {
        list.iter()
            .map(|&(name, bytes)| TensorEntry {
                name: name.to_string(),
                bytes,
            })
            .collect()
    }

    #[test]
    fn tensor_table_sizes_come_from_the_offsets() {
        let bytes = Writer::new()
            .str("general.architecture", "llama")
            .u32("llama.block_count", 2)
            .tensor("token_embd.weight", 4096)
            .tensor("blk.0.attn_q.weight", 1024)
            .tensor("blk.1.attn_q.weight", 1024)
            .tensor("output_norm.weight", 64)
            .build();
        let (header, tensors) = read_with_tensors(Cursor::new(&bytes), bytes.len() as u64).unwrap();
        assert_eq!(header.tensor_count, 4);
        assert_eq!(header.arch_u64("block_count"), Some(2));
        assert_eq!(
            tensors,
            entries(&[
                ("token_embd.weight", 4096),
                ("blk.0.attn_q.weight", 1024),
                ("blk.1.attn_q.weight", 1024),
                ("output_norm.weight", 64),
            ])
        );
        // The header reader still reads the same file.
        assert_eq!(parse(&bytes).unwrap().tensor_count, 4);
        // A file with no tensors has an empty table.
        let none = Writer::new().str("general.architecture", "llama").build();
        assert_eq!(table(&none).unwrap(), Vec::new());
    }

    /// Sizes include the padding to the alignment, which the file may choose.
    #[test]
    fn tensor_sizes_include_the_padding() {
        let bytes = Writer::new()
            .str("general.architecture", "llama")
            .tensor("a", 100)
            .tensor("b", 7)
            .build();
        assert_eq!(table(&bytes).unwrap(), entries(&[("a", 128), ("b", 32)]));
        let bytes = Writer::new()
            .str("general.architecture", "llama")
            .alignment(64)
            .tensor("a", 100)
            .tensor("b", 7)
            .build();
        assert_eq!(table(&bytes).unwrap(), entries(&[("a", 128), ("b", 64)]));
        for bad in [Writer::new().alignment(0), Writer::new().alignment(48)] {
            let bytes = bad.tensor("a", 32).build();
            assert!(matches!(table(&bytes), Err(GgufError::Corrupt(_))));
        }
        let bytes = Writer::new()
            .str("general.alignment", "32")
            .tensor("a", 32)
            .build();
        assert!(matches!(table(&bytes), Err(GgufError::Corrupt(_))));
    }

    /// A file whose tensor table is written by hand: each entry's name bytes, dimension
    /// count and offset, then the data section of `data` bytes.
    fn hand_table(list: &[(&[u8], u32, u64)], data: usize) -> Vec<u8> {
        let mut b = Writer::new()
            .str("general.architecture", "llama")
            .tensors(list.len() as u64)
            .build();
        for &(name, n_dims, offset) in list {
            b.extend((name.len() as u64).to_le_bytes());
            b.extend(name);
            b.extend(n_dims.to_le_bytes());
            for _ in 0..n_dims {
                b.extend(1u64.to_le_bytes());
            }
            b.extend(0u32.to_le_bytes());
            b.extend(offset.to_le_bytes());
        }
        b.resize(b.len().div_ceil(32) * 32 + data, 0);
        b
    }

    #[test]
    fn hostile_tensor_tables_are_refused() {
        let corrupt = |bytes: &[u8]| matches!(table(bytes), Err(GgufError::Corrupt(_)));
        // The hand-written table itself is fine.
        assert_eq!(
            table(&hand_table(&[(b"a", 1, 0), (b"b", 2, 64)], 96)).unwrap(),
            entries(&[("a", 64), ("b", 32)])
        );
        // More tensors than allowed, and more than the file can hold.
        assert!(corrupt(
            &Writer::new()
                .str("general.architecture", "llama")
                .tensors(MAX_TENSORS + 1)
                .build()
        ));
        assert_eq!(table(&hand_table(&[], 0)).unwrap(), Vec::new());
        let mut many = Writer::new().tensors(1000).build();
        many.extend([0u8; 512]);
        assert!(corrupt(&many));
        assert!(corrupt(&Writer::new().tensors(u64::MAX).build()));
        // Offsets backwards, equal, beyond the end, overflowing with the data start.
        assert!(corrupt(&hand_table(&[(b"a", 1, 64), (b"b", 1, 0)], 96)));
        assert!(corrupt(&hand_table(&[(b"a", 1, 0), (b"b", 1, 0)], 96)));
        assert!(corrupt(&hand_table(&[(b"a", 1, 0), (b"b", 1, 97)], 96)));
        assert!(corrupt(&hand_table(&[(b"a", 1, u64::MAX - 8)], 96)));
        assert!(corrupt(&hand_table(&[(b"a", 1, u64::MAX)], 96)));
        // A name of 10 KB, a name that is not UTF-8, five dimensions.
        let long = vec![b'x'; 10 * 1024];
        assert!(corrupt(&hand_table(&[(&long, 1, 0)], 96)));
        assert!(corrupt(&hand_table(&[(&[0xff, 0xfe], 1, 0)], 96)));
        assert!(corrupt(&hand_table(&[(b"a", 5, 0)], 96)));
        // A name length of 2^63.
        let mut b = Writer::new().tensors(1).build();
        b.extend((1u64 << 63).to_le_bytes());
        b.extend([0u8; 64]);
        assert!(corrupt(&b));
    }

    /// Cut anywhere before the last tensor's data, the file is refused, never a panic.
    /// Cut inside the last tensor, the last size is short (no type table is kept).
    #[test]
    fn truncated_tensor_table_is_refused() {
        let bytes = Writer::new()
            .str("general.architecture", "qwen3")
            .tensor("token_embd.weight", 256)
            .tensor("blk.0.attn_q.weight", 128)
            .tensor("output.weight", 256)
            .build();
        let last_start = bytes.len() - 256;
        for cut in 0..last_start {
            assert!(table(&bytes[..cut]).is_err(), "cut at {cut} parsed");
        }
        for cut in last_start..=bytes.len() {
            let t = table(&bytes[..cut]).unwrap();
            assert_eq!(t[2].bytes, (cut - last_start) as u64);
            assert_eq!((t[0].bytes, t[1].bytes), (256, 128));
        }
    }

    /// The bytes of a tensor as ggml lays it out, for the types a test model uses.
    fn ggml_nbytes(ggml_type: u32, dims: &[u64]) -> Option<u64> {
        let (block, size) = match ggml_type {
            0 => (1, 4),
            1 | 30 => (1, 2),
            2 => (32, 18),
            3 => (32, 20),
            6 => (32, 22),
            7 => (32, 24),
            8 => (32, 34),
            10 => (256, 84),
            11 => (256, 110),
            12 => (256, 144),
            13 => (256, 176),
            14 => (256, 210),
            _ => return None,
        };
        let (first, rest) = dims.split_first()?;
        Some(first / block * size * rest.iter().product::<u64>())
    }

    /// The real model: every size is the tensor's own bytes plus at most the alignment.
    #[test]
    fn real_model_tensor_table_if_available() {
        let Ok(path) = std::env::var("TEST_GGUF") else {
            return;
        };
        let len = std::fs::metadata(&path).unwrap().len();
        let file = File::open(&path).unwrap();
        let mut r = Reader {
            inner: BufReader::new(file),
            pos: 0,
            len,
        };
        let header = read_header(&mut r, false).unwrap();
        let raw = read_raw_tensors(&mut r, &header).unwrap();
        let data_start = r.pos.next_multiple_of(alignment(&header).unwrap());
        let (_, tensors) = read_file_with_tensors(Path::new(&path)).unwrap();
        assert_eq!(tensors.len() as u64, header.tensor_count);
        assert!(tensors.len() > 100);
        let sum: u64 = tensors.iter().map(|t| t.bytes).sum();
        assert_eq!(sum, len - data_start);
        let mut exact = 0u64;
        for (t, raw) in tensors.iter().zip(&raw) {
            assert_eq!(t.name, raw.name);
            let n = ggml_nbytes(raw.ggml_type, &raw.dims)
                .unwrap_or_else(|| panic!("type {} of {}", raw.ggml_type, raw.name));
            assert!(
                t.bytes >= n && t.bytes - n < 32,
                "{}: {} vs {n}",
                t.name,
                t.bytes
            );
            exact += n;
        }
        // Within 1% of the file size less the header.
        assert!(sum - exact <= (len - data_start) / 100, "{sum} vs {exact}");
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
