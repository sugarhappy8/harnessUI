//! Architectures the pinned llama.cpp build can load (`catalog/llama-architectures.txt`,
//! generated from the pinned commit by `scripts/update_llama_archs.py`).

const LIST: &str = include_str!("../../../catalog/llama-architectures.txt");

/// Whether llama-server from the pinned build knows `architecture`
/// (the GGUF `general.architecture` value, or Hugging Face's `gguf.architecture`).
pub fn is_supported(architecture: &str) -> bool {
    let a = architecture.trim();
    LIST.lines()
        .filter(|l| !l.starts_with('#'))
        .any(|l| l.trim() == a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown() {
        assert!(is_supported("llama"));
        assert!(is_supported("qwen3"));
        assert!(!is_supported("definitely-not-an-arch"));
        assert!(!is_supported(""));
        assert!(!is_supported("# general.architecture values"));
    }
}
