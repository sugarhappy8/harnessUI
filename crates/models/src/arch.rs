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

/// `general.architecture` values of picture and video models (stable-diffusion.cpp's, and
/// the GGUF converters' for them), which llama-server cannot load. Most diffusion files
/// carry no architecture at all, so this is a hint when present, never proof.
const DIFFUSION: [&str; 10] = [
    "flux",
    "sd1",
    "sdxl",
    "sd3",
    "wan",
    "ltxv",
    "hunyuan-video",
    "qwen_image",
    "lumina2",
    "chroma",
];

/// Whether `architecture` (a GGUF `general.architecture`, or Hugging Face's
/// `gguf.architecture`) is a picture or video model's.
pub fn is_diffusion(architecture: &str) -> bool {
    let a = architecture.trim();
    DIFFUSION.iter().any(|d| d.eq_ignore_ascii_case(a))
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

    /// The model that showed the red box while the pin was v0.9.0: its architecture came
    /// into llama.cpp after that build.
    #[test]
    fn inkling_two_is_known_since_the_v1_1_0_pin() {
        assert!(is_supported("inkling2"));
        assert!(is_supported("inkling2_loop"));
    }

    /// A pin change without `python3 scripts/update_llama_archs.py` leaves the list of
    /// the old build in the app: every newer architecture would show the red box.
    #[test]
    fn the_list_was_made_from_the_pinned_commit() {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../llama.lock.json")).unwrap();
        let tag = lock["llama_cpp"]["tag"].as_str().unwrap();
        let commit = lock["llama_cpp"]["commit"].as_str().unwrap();
        let header = LIST.lines().next().unwrap();
        assert!(
            header.contains(&format!("llama.cpp {tag} ({commit})")),
            "catalog/llama-architectures.txt is not from the pinned build {tag}; \
             run python3 scripts/update_llama_archs.py (header: {header})"
        );
    }

    #[test]
    fn picture_and_video_architectures_are_known() {
        for a in [
            "flux",
            "wan",
            "sd3",
            "hunyuan-video",
            " sdxl ",
            "qwen_image",
        ] {
            assert!(is_diffusion(a), "{a}");
        }
        for a in ["llama", "qwen3", "t5encoder", "", "fluxx"] {
            assert!(!is_diffusion(a), "{a}");
        }
    }
}
