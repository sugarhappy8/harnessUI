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

/// Why a page cannot offer a model, when no update of the app would change it (a kind the
/// model runner has simply not learned yet is not one of these: a newer version may open it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Why {
    /// A picture or video model ([`is_diffusion`]): Studio makes those, the chat does not.
    Picture,
    /// A speech or sound model ([`is_speech`], or an author's task of that kind): it can't chat,
    /// and nothing in this app uses it.
    Speech,
    /// A file in an early layout of a kind the engine does know ([`summary::early_layout`]
    /// (crate::summary::early_layout)): its maker uploaded it before the format was final, the
    /// runner cannot read it, and an update will not change that. A newer upload may work.
    Layout,
}

/// What a repository's page says about the kind of its model, and whether the engine knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageArchitecture {
    /// The name the page shows: the file's own, else the listing's; `None` when neither was read.
    pub name: Option<String>,
    /// `Some(false)` only when the page has enough evidence to say the chat cannot open the
    /// model; `None` when nothing was read or the two witnesses disagree.
    pub supported: Option<bool>,
    /// Set with `supported == Some(false)` when no newer version would change it: "not yet"
    /// would promise what is not true.
    pub why: Option<Why>,
}

impl PageArchitecture {
    /// The page with what the file's own header says about its layout: an early layout the
    /// runner cannot read is not "a kind it does not know yet" (the name is known), and no
    /// update brings it. The header is the smallest file's, the page's witness for the files.
    pub fn with_layout(mut self, early_layout: bool) -> Self {
        if early_layout {
            self.supported = Some(false);
            self.why = Some(Why::Layout);
        }
        self
    }
}

/// A name that was actually read: a file without `general.architecture` reads "unknown" and an
/// empty value is no value, and neither is a verdict.
fn read_name(name: Option<&str>) -> Option<&str> {
    name.map(str::trim)
        .filter(|n| !n.is_empty() && *n != "unknown")
}

/// What the page of a repository may say, from three witnesses: `header`, the kind in the
/// header of the repository's smallest file; `listing`, Hugging Face's own `gguf.architecture`
/// (its first file); and `task`, what the author says the model does (`pipeline_tag`). Either
/// kind may be a helper (a draft model, a control vector, a patch file), so a verdict needs the
/// evidence to hold together: a witness that read nothing says nothing, one witness decides, two
/// that agree decide, two that disagree about whether the engine knows the kind claim nothing
/// (the name the engine knows is shown, so the page never labels a good repository with a
/// helper's kind). Nothing here blocks one file: the header of the file itself is the exact
/// witness, read when it is downloaded or loaded.
///
/// A kind that is a picture or speech model's ([`Why`]) is not for chat whatever the engine
/// knows. The author's task is weaker (a chat model tagged "text-to-image" exists), so it speaks
/// only where the engine cannot open the model anyway: a kind it does not know, or nothing read
/// (a picture file has no metadata at all). It then says whether an update would help.
pub fn page_architecture(
    header: Option<&str>,
    listing: Option<&str>,
    task: Option<&str>,
) -> PageArchitecture {
    let (header, listing) = (read_name(header), read_name(listing));
    let (name, supported) = match (header, listing) {
        (None, None) => (None, None),
        (Some(a), None) | (None, Some(a)) => (Some(a), Some(is_supported(a))),
        (Some(h), Some(l)) if is_supported(h) == is_supported(l) => {
            (Some(h), Some(is_supported(h)))
        }
        (Some(h), Some(l)) => (Some(if is_supported(h) { h } else { l }), None),
    };
    let by_kind = name.and_then(kind_why);
    let by_task = task.and_then(task_why);
    let (supported, why) = if let Some(why) = by_kind {
        (Some(false), Some(why))
    } else if supported == Some(false) || (name.is_none() && by_task.is_some()) {
        (Some(false), by_task)
    } else {
        (supported, None)
    };
    PageArchitecture {
        name: name.map(str::to_string),
        supported,
        why,
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

/// `general.architecture` values of speech and sound models: the four llama.cpp learned at
/// v1.1.0 and the vocoder it had (`catalog/llama-architectures.txt` lists them, so the engine
/// opens the file and the chat then fails on its first message), and the kinds of other
/// engines (whisper.cpp, parakeet, audio.cpp and their kin) that no update of this app brings.
/// From the repositories of the 2026-11-27 sweep; a kind that is not here is still caught by
/// the author's task where the engine does not know it ([`page_architecture`]).
const SPEECH: [&str; 24] = [
    "kokoro",
    "tiny-voca",
    "zonos3",
    "moshi",
    "wavtokenizer-dec",
    "whisper",
    "parakeet",
    "canary",
    "canary_qwen",
    "audiocpp",
    "asr",
    "qwen3_asr",
    "cohere_asr",
    "gigaam",
    "gigaam_zap",
    "funasr_nano",
    "granite_speech",
    "sortformer",
    "silero_vad",
    "qwen3-tts",
    "higgs_tts",
    "breeze-tts-2",
    "omnivoice-lm",
    "magpietts",
];

/// Whether `architecture` is a speech or sound model's ([`SPEECH`]).
pub fn is_speech(architecture: &str) -> bool {
    let a = architecture.trim();
    SPEECH.iter().any(|d| d.eq_ignore_ascii_case(a))
}

/// What a kind says about the model: a picture or video model, a speech model, or nothing.
fn kind_why(architecture: &str) -> Option<Why> {
    if is_diffusion(architecture) {
        Some(Why::Picture)
    } else if is_speech(architecture) {
        Some(Why::Speech)
    } else {
        None
    }
}

/// Hugging Face tasks (`pipeline_tag`) of models that make pictures, video or 3D shapes.
const PICTURE_TASKS: [&str; 9] = [
    "text-to-image",
    "image-to-image",
    "image-text-to-image",
    "unconditional-image-generation",
    "text-to-video",
    "image-to-video",
    "image-text-to-video",
    "video-to-video",
    "image-to-3d",
];

/// Hugging Face tasks of models that listen or speak and do not chat. `audio-text-to-text`
/// (a chat model that hears) is not one.
const SPEECH_TASKS: [&str; 6] = [
    "text-to-speech",
    "text-to-audio",
    "automatic-speech-recognition",
    "audio-to-audio",
    "voice-activity-detection",
    "audio-classification",
];

/// What an author's task says about the model: it makes pictures, it speaks or listens, or
/// nothing that rules out a chat.
fn task_why(task: &str) -> Option<Why> {
    let t = task.trim();
    if PICTURE_TASKS.contains(&t) {
        Some(Why::Picture)
    } else if SPEECH_TASKS.contains(&t) {
        Some(Why::Speech)
    } else {
        None
    }
}

/// What a listing's kind and task say about a model, without the files' headers: whether it is
/// not for chat at all ([`Why`]), so no update would open it. The search card's hint uses it to
/// stay quiet about a kind that no update brings.
pub fn not_for_chat(architecture: Option<&str>, task: Option<&str>) -> Option<Why> {
    read_name(architecture)
        .and_then(kind_why)
        .or_else(|| task.and_then(task_why))
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
        let p = page_architecture(header, listing, None);
        (p.name, p.supported)
    }

    fn judged(
        header: Option<&str>,
        listing: Option<&str>,
        task: Option<&str>,
    ) -> (Option<bool>, Option<Why>) {
        let p = page_architecture(header, listing, task);
        (p.supported, p.why)
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
            assert_eq!(
                judged(header, listing, None),
                (Some(false), Some(Why::Picture)),
                "{header:?} {listing:?}"
            );
        }
        for (header, listing) in [
            (Some("qwen3"), None),
            (Some("futurearch"), None),
            (None, None),
        ] {
            assert_eq!(judged(header, listing, None).1, None, "{header:?}");
        }
    }

    /// The sweep of 2026-11-27 found 14 of the 48 repositories a default search would still
    /// red-box were speech models of other engines (whisper.cpp's, parakeet's and the like):
    /// an update of this app never brings them, so "a newer version may open it" was a false
    /// hope. The four speech kinds llama.cpp learned at v1.1.0 are in the list, so the engine
    /// knows them and the page used to offer them as chat models.
    #[test]
    fn a_speech_kind_is_not_for_chat_whether_or_not_the_engine_knows_it() {
        for kind in [
            "kokoro",
            "tiny-voca",
            "zonos3",
            "moshi",
            "wavtokenizer-dec",
            "whisper",
            "parakeet",
            "audiocpp",
            "qwen3_asr",
            " Kokoro ",
        ] {
            assert_eq!(
                judged(Some(kind), None, None),
                (Some(false), Some(Why::Speech)),
                "{kind}"
            );
            assert_eq!(
                judged(None, Some(kind), None),
                (Some(false), Some(Why::Speech)),
                "{kind}"
            );
            assert!(is_speech(kind), "{kind}");
        }
        for kind in ["qwen3", "llama", "lfm2", "bert", "kokoroish", ""] {
            assert!(!is_speech(kind), "{kind}");
        }
    }

    /// What the author says the model does is a witness of its own, and a weak one: a chat
    /// model tagged "text-to-image" exists. So it only speaks where the engine does not know
    /// the kind (where the page would promise an update) or nothing was read (a picture file
    /// has no metadata at all): it never switches a download off for a kind the engine can
    /// open, and it never speaks when the two witnesses already disagree.
    #[test]
    fn the_authors_task_decides_only_where_the_engine_cannot_open_it_anyway() {
        // The engine does not know the kind: the task says whether an update would help.
        for (task, why) in [
            ("text-to-image", Some(Why::Picture)),
            ("image-to-video", Some(Why::Picture)),
            ("text-to-video", Some(Why::Picture)),
            ("image-to-3d", Some(Why::Picture)),
            ("text-to-speech", Some(Why::Speech)),
            ("automatic-speech-recognition", Some(Why::Speech)),
            ("audio-to-audio", Some(Why::Speech)),
            ("voice-activity-detection", Some(Why::Speech)),
            // A chat task, an unclear one: a newer version may well open it.
            ("text-generation", None),
            ("image-text-to-text", None),
            ("audio-text-to-text", None),
            ("any-to-any", None),
            ("feature-extraction", None),
        ] {
            assert_eq!(
                judged(Some("futurearch"), None, Some(task)),
                (Some(false), why),
                "{task}"
            );
        }
        // Nothing read (Qwen-Image's smallest file has no metadata, its listing no kind):
        // the task is all there is, and a picture file cannot open in a chat anyway.
        assert_eq!(
            judged(Some("unknown"), None, Some("text-to-image")),
            (Some(false), Some(Why::Picture))
        );
        assert_eq!(
            judged(None, None, Some("text-to-speech")),
            (Some(false), Some(Why::Speech))
        );
        assert_eq!(judged(None, None, Some("text-generation")), (None, None));
        assert_eq!(judged(None, None, None), (None, None));
        // A kind the engine knows is never switched off by a task (a chat model tagged
        // "text-to-image" is still a chat model).
        for task in ["text-to-image", "automatic-speech-recognition"] {
            assert_eq!(
                judged(Some("qwen3"), None, Some(task)),
                (Some(true), None),
                "{task}"
            );
        }
        // Two witnesses that disagree claim nothing, task or not.
        assert_eq!(
            judged(
                Some("deepseek4_mtp_support"),
                Some("deepseek4"),
                Some("text-to-image")
            ),
            (None, None)
        );
    }

    /// The engine knows the name `inkling2` and still cannot load the early uploads of it, so
    /// "can't open yet, a newer version may" would be wrong twice: it is not the kind that is
    /// new, and no update changes what the file holds.
    #[test]
    fn an_early_layout_of_a_known_kind_is_not_for_an_update_to_fix() {
        let p = page_architecture(Some("inkling2"), Some("inkling2"), None);
        assert_eq!((p.supported, p.why), (Some(true), None));
        let p = p.with_layout(true);
        assert_eq!(
            (p.name.as_deref(), p.supported, p.why),
            (Some("inkling2"), Some(false), Some(Why::Layout))
        );
        // Nothing to say about the layout: the page is as it was.
        let p = page_architecture(Some("qwen3"), None, None).with_layout(false);
        assert_eq!((p.supported, p.why), (Some(true), None));
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
