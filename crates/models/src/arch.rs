//! Architectures the pinned llama.cpp build can load (`catalog/llama-architectures.txt`,
//! generated from the pinned commit by `scripts/update_llama_archs.py`).
//!
//! The ROCm download is a second llama.cpp, built from another commit. The list above
//! serves every backend, so the other build must know every name in it:
//! `catalog/llama-architectures-rocm.txt` (made by `update_llama_archs.py --pack rocm`)
//! exists for a test that says so, and fails the day a bump of the pin outruns ROCm.

const LIST: &str = include_str!("../../../catalog/llama-architectures.txt");

/// Whether llama-server from the pinned build knows `architecture`
/// (the GGUF `general.architecture` value, or Hugging Face's `gguf.architecture`).
pub fn is_supported(architecture: &str) -> bool {
    let a = architecture.trim();
    LIST.lines()
        .filter(|l| !l.starts_with('#'))
        .any(|l| l.trim() == a)
}

/// What a repository's page says about the kind of its model, and whether the engine knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageArchitecture {
    /// The name the page shows: the file's own, else the listing's; `None` when neither was read.
    pub name: Option<String>,
    /// `Some(false)` only when the page has enough evidence to say the engine cannot open the
    /// model; `None` when nothing was read or the two witnesses disagree.
    pub supported: Option<bool>,
    /// The kind is a picture or video model's ([`is_diffusion`]): no newer version of the model
    /// runner will chat with it, so "not yet" would promise what is not true.
    pub picture: bool,
}

/// A name that was actually read: a file without `general.architecture` reads "unknown" and an
/// empty value is no value, and neither is a verdict.
fn read_name(name: Option<&str>) -> Option<&str> {
    name.map(str::trim)
        .filter(|n| !n.is_empty() && *n != "unknown")
}

/// What the page of a repository may say, from two witnesses: `header`, the kind in the header
/// of the repository's smallest file, and `listing`, Hugging Face's own `gguf.architecture`
/// (its first file). Either may be a helper (a draft model, a control vector, a patch file), so
/// a verdict needs the evidence to hold together: a witness that read nothing says nothing, one
/// witness decides, two that agree decide, two that disagree about whether the engine knows the
/// kind claim nothing (the name the engine knows is shown, so the page never labels a good
/// repository with a helper's kind). Nothing here blocks one file: the header of the file
/// itself is the exact witness, read when it is downloaded or loaded.
pub fn page_architecture(header: Option<&str>, listing: Option<&str>) -> PageArchitecture {
    let (header, listing) = (read_name(header), read_name(listing));
    let (name, supported) = match (header, listing) {
        (None, None) => (None, None),
        (Some(a), None) | (None, Some(a)) => (Some(a), Some(is_supported(a))),
        (Some(h), Some(l)) if is_supported(h) == is_supported(l) => {
            (Some(h), Some(is_supported(h)))
        }
        (Some(h), Some(l)) => (Some(if is_supported(h) { h } else { l }), None),
    };
    PageArchitecture {
        picture: name.is_some_and(is_diffusion),
        name: name.map(str::to_string),
        supported,
    }
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

    /// The names the ROCm download's llama.cpp knows, from the commit its lock entry
    /// records.
    const ROCM_LIST: &str = include_str!("../../../catalog/llama-architectures-rocm.txt");

    fn names(list: &str) -> Vec<&str> {
        list.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect()
    }

    fn rocm_pack() -> serde_json::Value {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../llama.lock.json")).unwrap();
        lock["packs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "rocm")
            .expect("the lock has a ROCm pack")
            .clone()
    }

    /// A ROCm pack change without `python3 scripts/update_llama_archs.py --pack rocm`
    /// leaves the names of the old build in the file, and the next test proves nothing.
    #[test]
    fn the_rocm_list_was_made_from_the_commit_of_the_download() {
        let pack = rocm_pack();
        let build = pack["build"].as_str().unwrap();
        let commit = pack["upstream_llama_cpp_commit"].as_str().unwrap();
        let header = ROCM_LIST.lines().next().unwrap();
        assert!(
            header.contains(&format!("the ROCm download {build} ({commit})")),
            "catalog/llama-architectures-rocm.txt is not from the ROCm download {build}; \
             run python3 scripts/update_llama_archs.py --pack rocm (header: {header})"
        );
    }

    /// Whatever the list claims, the ROCm download can load: a name it lacks would show a
    /// green Download for a model that fails to open on the backend the user chose.
    #[test]
    fn the_rocm_download_knows_every_architecture_the_list_claims() {
        let rocm = names(ROCM_LIST);
        let lacking: Vec<&str> = names(LIST)
            .into_iter()
            .filter(|n| !rocm.contains(n))
            .collect();
        assert!(
            lacking.is_empty(),
            "the ROCm download ({}) cannot load {} architectures the model browser calls \
             supported: {}. Move the ROCm pack in llama.lock.json to a lemonade build of a \
             newer llama.cpp commit, then run python3 scripts/update_llama_archs.py --pack rocm \
             (ARCHITECTURE.md 7.1, \"How to bump the llama.cpp pin\", step 8)",
            rocm_pack()["build"].as_str().unwrap(),
            lacking.len(),
            lacking.join(" ")
        );
    }

    fn page(header: Option<&str>, listing: Option<&str>) -> (Option<String>, Option<bool>) {
        let p = page_architecture(header, listing);
        (p.name, p.supported)
    }

    /// "Not yet" is for chat models only: a newer version will not chat with a picture or video
    /// model, so the page says what it is instead of promising an update.
    #[test]
    fn a_picture_or_video_kind_is_marked_so_the_page_does_not_promise_an_update() {
        for (header, listing) in [
            (Some("flux"), None),
            (None, Some("wan")),
            (Some("sd3"), Some("sd3")),
        ] {
            let p = page_architecture(header, listing);
            assert!(p.picture, "{header:?} {listing:?}");
            assert_eq!(p.supported, Some(false), "{header:?} {listing:?}");
        }
        for (header, listing) in [
            (Some("qwen3"), None),
            (Some("futurearch"), None),
            (None, None),
        ] {
            assert!(!page_architecture(header, listing).picture, "{header:?}");
        }
    }

    fn some(name: &str, supported: Option<bool>) -> (Option<String>, Option<bool>) {
        (Some(name.to_string()), supported)
    }

    /// A file with no `general.architecture` key reads "unknown". That is "not read", never a
    /// verdict: Huihui-Qwen3.8-27B-abliterated-GGUF's smallest file is a patch file with no
    /// key, and its repository is `qwen35`, which the engine knows.
    #[test]
    fn a_header_without_a_kind_is_not_read_and_the_listing_decides() {
        assert_eq!(
            page(Some("unknown"), Some("qwen35")),
            some("qwen35", Some(true))
        );
        assert_eq!(page(Some(""), Some("qwen35")), some("qwen35", Some(true)));
        assert_eq!(
            page(Some(" unknown "), Some("futurearch")),
            some("futurearch", Some(false))
        );
        // Nothing read at all: nothing to say, so no box.
        assert_eq!(page(Some("unknown"), None), (None, None));
        assert_eq!(page(None, None), (None, None));
        assert_eq!(page(Some("unknown"), Some("  ")), (None, None));
    }

    #[test]
    fn one_witness_is_enough_when_the_other_is_silent() {
        // Header only (the listing has no gguf block), listing only (the header read failed).
        assert_eq!(page(Some("qwen3"), None), some("qwen3", Some(true)));
        assert_eq!(
            page(Some("futurearch"), None),
            some("futurearch", Some(false))
        );
        assert_eq!(page(None, Some("inkling2")), some("inkling2", Some(true)));
        assert_eq!(
            page(None, Some("futurearch")),
            some("futurearch", Some(false))
        );
    }

    #[test]
    fn two_witnesses_that_agree_decide() {
        assert_eq!(
            page(Some("futurearch"), Some("futurearch")),
            some("futurearch", Some(false))
        );
        assert_eq!(
            page(Some("qwen2"), Some("qwen3")),
            some("qwen2", Some(true))
        );
        // Both say the engine does not know it, by different names: the file's own name shows.
        assert_eq!(
            page(Some("futurearch"), Some("otherarch")),
            some("futurearch", Some(false))
        );
    }

    /// The smallest file of a repository is often a helper (a draft model, a control vector,
    /// a patch) and the listing describes the repository's first file: when they disagree
    /// about whether the engine knows the kind, one of them looked at a helper, so the page
    /// claims nothing and shows the name the engine knows. The click on one file decides.
    #[test]
    fn two_witnesses_that_disagree_claim_nothing() {
        // Huihui-DeepSeek-V4-Flash-abliterated-ds4: a helper file's kind, then the model's.
        assert_eq!(
            page(Some("deepseek4_mtp_support"), Some("deepseek4")),
            some("deepseek4", None)
        );
        // Inkling-2-Mini-Maya: the listing looked at a control vector.
        assert_eq!(
            page(Some("inkling2"), Some("controlvector")),
            some("inkling2", None)
        );
        assert_eq!(
            page(Some("controlvector"), Some("inkling2")),
            some("inkling2", None)
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
