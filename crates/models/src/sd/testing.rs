//! Builders for tests of the tensor index (here and, with `test-support`, in other crates).

use serde_json::{Value, json};

/// A safetensors file with `header` as its JSON header and no tensor data: the header's
/// byte length as a little-endian `u64`, then the JSON.
pub fn safetensors(header: &Value) -> Vec<u8> {
    let json = serde_json::to_vec(header).expect("a JSON value serializes");
    let mut out = (json.len() as u64).to_le_bytes().to_vec();
    out.extend(json);
    out
}

/// A one-tensor header whose JSON is exactly `json_bytes` bytes, padded in `__metadata__`.
///
/// # Panics
/// When `json_bytes` is smaller than the header without padding.
pub fn big_header(json_bytes: usize) -> Value {
    let build = |pad: usize| {
        json!({
            "t": {"dtype": "F16", "shape": [1], "data_offsets": [0, 2]},
            "__metadata__": {"pad": "x".repeat(pad)},
        })
    };
    let base = serde_json::to_vec(&build(0))
        .expect("a JSON value serializes")
        .len();
    assert!(json_bytes >= base, "a header needs at least {base} bytes");
    build(json_bytes - base)
}
