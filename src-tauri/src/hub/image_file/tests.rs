//! 0.0.7 (P2): which image file a load uses, the ones that could not be read, and the
//! image file a repository offers.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;
use crate::hub::{SavedFacts, register_installed_in};

/// A stamp for a file that exists only in the test.
fn stamp(path: &Path, size: u64, modified_ms: u64) -> FileStamp {
    FileStamp {
        path: path.display().to_string(),
        size,
        modified_ms: Some(modified_ms),
    }
}

fn repo_file(path: &str, size: u64) -> RepoFile {
    RepoFile {
        path: path.into(),
        size,
        sha256: Some("ab".repeat(32)),
    }
}

/// A folder with the model and the given image files (each `size` bytes).
/// A folder of its own for one test, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("image-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn folder(images: &[(&str, usize)]) -> (Scratch, PathBuf) {
    let dir = Scratch::new();
    let model = dir.path().join("Model-Q4_K_M.gguf");
    std::fs::write(&model, model_bytes()).unwrap();
    for (name, size) in images {
        std::fs::write(dir.path().join(name), vec![0u8; *size]).unwrap();
    }
    (dir, model)
}

/// The header of a small model the Installed list accepts.
fn model_bytes() -> Vec<u8> {
    models::gguf::testing::Writer::new()
        .str("general.architecture", "sample")
        .u32("sample.context_length", 4096)
        .u32("sample.block_count", 2)
        .build()
}

/// Sets a file's modification time (so a test can tell a changed file from the same one).
fn set_modified(path: &Path, seconds: u64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn a_stamp_tells_the_same_file_from_a_changed_one() {
    let (dir, _) = folder(&[("mmproj-F16.gguf", 100)]);
    let path = dir.path().join("mmproj-F16.gguf");
    set_modified(&path, 1_700_000_000);
    let first = FileStamp::of(&path).expect("a file");
    assert_eq!(first.size, 100);
    assert_eq!(first.modified_ms, Some(1_700_000_000_000));
    assert!(first.same_file(&FileStamp::of(&path).unwrap()));
    // Another spelling of the same file (a link, a moved folder): the same file.
    let elsewhere = stamp(
        Path::new("/other/place/mmproj-F16.gguf"),
        100,
        1_700_000_000_000,
    );
    assert!(first.same_file(&elsewhere));
    // Replaced or changed: not the same file any more.
    set_modified(&path, 1_700_000_100);
    assert!(!first.same_file(&FileStamp::of(&path).unwrap()));
    std::fs::write(&path, vec![0u8; 101]).unwrap();
    set_modified(&path, 1_700_000_000);
    assert!(!first.same_file(&FileStamp::of(&path).unwrap()));
    // Another name with the same size and time is another file.
    assert!(!first.same_file(&stamp(
        &dir.path().join("mmproj-BF16.gguf"),
        100,
        1_700_000_000_000
    )));
    // Not a file: no stamp.
    assert_eq!(FileStamp::of(dir.path()), None);
    assert_eq!(FileStamp::of(&dir.path().join("gone.gguf")), None);
}

#[test]
fn remembered_files_are_replaced_and_kept_to_a_limit() {
    let a = stamp(Path::new("/m/mmproj-a.gguf"), 1, 1);
    let list = remember(&[], a.clone());
    assert_eq!(list, vec![a.clone()]);
    // The same path again (the file changed and failed again): replaced, not added.
    let a2 = stamp(Path::new("/m/mmproj-a.gguf"), 2, 2);
    assert_eq!(remember(&list, a2.clone()), vec![a2.clone()]);
    assert!(is_unreadable(&[a2.clone()], &a2));
    assert!(!is_unreadable(&[a2], &a));
    let mut list = Vec::new();
    for n in 0..(MAX_UNREADABLE as u64 + 3) {
        list = remember(
            &list,
            stamp(Path::new(&format!("/m/mmproj-{n}.gguf")), n, n),
        );
    }
    assert_eq!(list.len(), MAX_UNREADABLE);
    assert_eq!(list[0].path, "/m/mmproj-3.gguf", "the oldest went first");
}

#[test]
fn image_files_beside_the_model_best_precision_first() {
    let (dir, model) = folder(&[
        ("mmproj-Model-F32.gguf", 4),
        ("mmproj-Model-BF16.gguf", 2),
        ("Model.mmproj-f16.gguf", 2),
        ("mmproj-Model-Q8_0.gguf", 1),
        ("mmproj-unlabelled.gguf", 1),
        // Not image files: a model, a download in progress, other files.
        ("Other-Q8_0.gguf", 1),
        ("mmproj-Model-F16.gguf.part", 1),
        ("mmproj-notes.txt", 1),
    ]);
    std::fs::create_dir(dir.path().join("mmproj-folder.gguf")).unwrap();
    let names: Vec<String> = beside(&model)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        [
            "Model.mmproj-f16.gguf",
            "mmproj-Model-BF16.gguf",
            "mmproj-Model-Q8_0.gguf",
            "mmproj-Model-F32.gguf",
            "mmproj-unlabelled.gguf",
        ]
    );
    // The HauhauCS repository's name for it.
    let (_dir, model) = folder(&[(
        "mmproj-Qwen3.8-27B-Uncensored-HauhauCS-Aggressive-BF16.gguf",
        3,
    )]);
    assert_eq!(beside(&model).len(), 1);
    assert!(beside(Path::new("/no/such/folder/model.gguf")).is_empty());
}

/// What `choose` reads from disk in a test: the stamps of the files that are there.
fn on_disk(present: Vec<FileStamp>) -> impl Fn(&Path) -> Option<FileStamp> {
    move |p: &Path| present.iter().find(|s| Path::new(&s.path) == p).cloned()
}

/// Which image file a load uses (1c): the recorded one while it is there and readable;
/// else one next to the model, so a file put there later is used; one remembered as
/// unreadable (1b) is skipped, and when nothing else is there the load says why.
#[test]
fn the_choice_for_a_load() {
    let recorded = PathBuf::from("/m/recorded-mmproj-F16.gguf");
    let near = PathBuf::from("/m/mmproj-BF16.gguf");
    let other = PathBuf::from("/m/mmproj-Q8_0.gguf");
    let files = [
        stamp(&recorded, 10, 1),
        stamp(&near, 20, 2),
        stamp(&other, 30, 3),
    ];
    let all = || on_disk(files.to_vec());
    let recorded_text = recorded.display().to_string();

    // Recorded and readable: that one, even with others beside it.
    let c = choose(Some(&recorded_text), &[near.clone()], &[], all());
    assert_eq!(
        c,
        Choice {
            path: Some(recorded_text.clone()),
            found: false,
            left_out: false
        }
    );
    // None recorded (downloaded or imported before the file was put there): beside it.
    let c = choose(None, &[near.clone(), other.clone()], &[], all());
    assert_eq!(
        c,
        Choice {
            path: Some(near.display().to_string()),
            found: true,
            left_out: false
        }
    );
    // The recorded one is gone: beside it.
    let gone = on_disk(vec![files[1].clone()]);
    let c = choose(Some(&recorded_text), &[near.clone()], &[], gone);
    assert_eq!(c.path, Some(near.display().to_string()));
    assert!(c.found && !c.left_out);
    // The recorded one could not be read: another one beside it (the same file there is
    // skipped too), else nothing, and the load says why.
    let bad = [files[0].clone()];
    let c = choose(
        Some(&recorded_text),
        &[recorded.clone(), near.clone()],
        &bad,
        all(),
    );
    assert_eq!(c.path, Some(near.display().to_string()));
    assert!(c.found && !c.left_out);
    let c = choose(Some(&recorded_text), &[recorded.clone()], &bad, all());
    assert_eq!(
        c,
        Choice {
            path: None,
            found: false,
            left_out: true
        }
    );
    // Every one beside it could not be read either.
    let all_bad = files.to_vec();
    let c = choose(None, &[near.clone(), other.clone()], &all_bad, all());
    assert_eq!(
        c,
        Choice {
            path: None,
            found: false,
            left_out: true
        }
    );
    // A file remembered as unreadable that changed since (re-downloaded) is tried again.
    let changed = [stamp(&recorded, 10, 99)];
    let c = choose(Some(&recorded_text), &[], &changed, all());
    assert_eq!(c.path, Some(recorded_text.clone()));
    // Nothing anywhere: no image file, nothing to say.
    assert_eq!(choose(None, &[], &[], all()), Choice::default());
    assert_eq!(
        choose(Some(&recorded_text), &[], &[], on_disk(Vec::new())),
        Choice::default()
    );
}

/// The image file a repository offers: the best one by Browse's ranking (F16 first),
/// skipping one that could not be read here (the same name and size); a newer upload under
/// the same name is another file.
#[test]
fn the_image_file_a_repository_offers() {
    let files = vec![
        repo_file("README.md", 1),
        repo_file("Model-Q4_K_M.gguf", 4_000),
        repo_file("mmproj-Model-F32.gguf", 600),
        repo_file("mmproj-Model-BF16.gguf", 300),
        repo_file("sub/Model.mmproj-f16.gguf", 300),
    ];
    let (offer, file) = pick(&files, &[]);
    assert_eq!(
        offer,
        Offer::Available {
            file: "Model.mmproj-f16.gguf".into(),
            size_bytes: 300
        }
    );
    assert_eq!(file.unwrap().path, "sub/Model.mmproj-f16.gguf");

    let bad_f16 = stamp(Path::new("/models/x/Model.mmproj-f16.gguf"), 300, 1);
    let (offer, file) = pick(&files, std::slice::from_ref(&bad_f16));
    assert_eq!(
        offer,
        Offer::Available {
            file: "mmproj-Model-BF16.gguf".into(),
            size_bytes: 300
        }
    );
    assert_eq!(file.unwrap().path, "mmproj-Model-BF16.gguf");

    // The same name, another size: the repository has a newer upload of it.
    let older = stamp(Path::new("/models/x/Model.mmproj-f16.gguf"), 299, 1);
    assert_eq!(
        pick(&files, &[older]).1.unwrap().path,
        "sub/Model.mmproj-f16.gguf"
    );

    // Only the one that could not be read; none at all.
    let only = vec![
        repo_file("Model-Q4_K_M.gguf", 4_000),
        repo_file("Model.mmproj-f16.gguf", 300),
    ];
    assert_eq!(pick(&only, &[bad_f16]), (Offer::NoOther, None));
    assert_eq!(pick(&only[..1], &[]), (Offer::NoImageFile, None));
    assert_eq!(pick(&[], &[]), (Offer::NoImageFile, None));
}

/// The webview reads these shapes (src/lib/types.ts `ImageFileOffer`).
#[test]
fn offers_serialise_with_a_kind() {
    assert_eq!(
        serde_json::to_value(Offer::Available {
            file: "mmproj-F16.gguf".into(),
            size_bytes: 851_443_712
        })
        .unwrap(),
        serde_json::json!({"kind": "available", "file": "mmproj-F16.gguf", "sizeBytes": 851_443_712u64})
    );
    assert_eq!(
        serde_json::to_value(Offer::NoOther).unwrap(),
        serde_json::json!({"kind": "noOther"})
    );
    assert_eq!(
        serde_json::to_value(Offer::NoImageFile).unwrap(),
        serde_json::json!({"kind": "none"})
    );
}

/// A downloaded image file goes into the model's own folder under the last part of its
/// repository path; a path that could escape the folder is refused.
#[test]
fn a_downloaded_image_file_goes_next_to_the_model() {
    let model = Path::new("/models/someone/Model-GGUF/Q4/Model-Q4_K_M.gguf");
    assert_eq!(
        destination(model, "mmproj-Model-F16.gguf"),
        Some(PathBuf::from(
            "/models/someone/Model-GGUF/Q4/mmproj-Model-F16.gguf"
        ))
    );
    assert_eq!(
        destination(model, "vision/mmproj-Model-F16.gguf"),
        Some(PathBuf::from(
            "/models/someone/Model-GGUF/Q4/mmproj-Model-F16.gguf"
        ))
    );
    for bad in [
        "../mmproj.gguf",
        "a/../../mmproj.gguf",
        "/etc/mmproj.gguf",
        "",
        "mmproj\\..\\x.gguf",
        "CON.gguf",
        "mmproj.gguf.",
    ] {
        assert_eq!(destination(model, bad), None, "{bad:?}");
    }
}

/// Registers the model at `model` (downloaded from `repo`) on a fresh store.
async fn installed(model: &Path, repo: Option<&str>) -> store::Store {
    let store = store::Store::open_in_memory().unwrap();
    let size = std::fs::metadata(model).unwrap().len();
    register_installed_in(
        &store,
        model,
        repo.map(str::to_owned),
        size,
        None,
        None,
        store::ModelSource::Download,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    store
}

async fn stored(store: &store::Store, model: &Path) -> StoredSummary {
    let rec = store
        .model_by_path(&model.display().to_string())
        .await
        .unwrap()
        .unwrap();
    serde_json::from_value(rec.gguf_summary).unwrap()
}

/// The owner's two models, end to end on the store: an image file put next to the model
/// after it was registered is used and recorded by the next load (1c); one that could not
/// be read is remembered and left out of the next load, which says why, until the file
/// changes or another one is downloaded (1b).
#[tokio::test]
async fn a_later_image_file_is_used_and_an_unreadable_one_is_left_out() {
    let (dir, model) = folder(&[]);
    let model_text = model.display().to_string();
    let store = installed(&model, Some("someone/Model-GGUF")).await;
    assert_eq!(choice_for(&store, &model_text).await, Choice::default());
    assert_eq!(stored(&store, &model).await.projector, None);

    // Put there later (downloaded by hand): the next load uses it and records it.
    let image = dir.path().join("mmproj-Model-BF16.gguf");
    std::fs::write(&image, vec![0u8; 64]).unwrap();
    let image_text = image.display().to_string();
    let choice = choice_for(&store, &model_text).await;
    assert_eq!(choice.path.as_deref(), Some(image_text.as_str()));
    assert!(choice.found);
    assert!(!prepare_for_load(&store, &model_text, Some(&image_text)).await);
    assert_eq!(
        stored(&store, &model).await.projector.as_deref(),
        Some(image_text.as_str())
    );
    let choice = choice_for(&store, &model_text).await;
    assert_eq!(choice.path.as_deref(), Some(image_text.as_str()));
    assert!(!choice.found, "recorded now");

    // It could not be read: remembered, and the next load goes without it and says so.
    mark_unreadable(&store, &model, &image).await;
    let saved = stored(&store, &model).await;
    assert_eq!(saved.unreadable_projectors.len(), 1);
    assert_eq!(saved.unreadable_projectors[0].path, image_text);
    assert_eq!(saved.unreadable_projectors[0].size, 64);
    let choice = choice_for(&store, &model_text).await;
    assert_eq!(choice.path, None);
    assert!(choice.left_out);
    assert!(prepare_for_load(&store, &model_text, None).await);
    // Remembering it twice changes nothing.
    mark_unreadable(&store, &model, &image).await;
    assert_eq!(stored(&store, &model).await.unreadable_projectors.len(), 1);

    // Imported again (the same file): still remembered.
    register_installed_in(
        &store,
        &model,
        None,
        std::fs::metadata(&model).unwrap().len(),
        None,
        None,
        store::ModelSource::Import,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    assert!(choice_for(&store, &model_text).await.left_out);

    // The file changed (replaced by another copy): tried again.
    std::fs::write(&image, vec![1u8; 65]).unwrap();
    let choice = choice_for(&store, &model_text).await;
    assert_eq!(choice.path.as_deref(), Some(image_text.as_str()));
    assert!(!choice.left_out);

    // Unreadable again, then another image file downloaded for it: recorded, and the
    // remembered ones are forgotten.
    mark_unreadable(&store, &model, &image).await;
    assert!(choice_for(&store, &model_text).await.left_out);
    let downloaded = dir.path().join("mmproj-Model-F16.gguf");
    std::fs::write(&downloaded, vec![2u8; 70]).unwrap();
    attach(&store, &model_text, &downloaded).await.unwrap();
    let saved = stored(&store, &model).await;
    assert_eq!(
        saved.projector.as_deref(),
        Some(downloaded.display().to_string().as_str())
    );
    assert!(saved.unreadable_projectors.is_empty());
    let choice = choice_for(&store, &model_text).await;
    assert_eq!(choice.path, Some(downloaded.display().to_string()));
    // The other facts of the row stay as they were.
    let rec = store.model_by_path(&model_text).await.unwrap().unwrap();
    assert_eq!(rec.repo.as_deref(), Some("someone/Model-GGUF"));
    assert_eq!(rec.source, store::ModelSource::Download);
    assert_eq!(saved.architecture.as_deref(), Some("sample"));
}

/// A model that is not on the Installed list keeps loading without an image file, as
/// before 0.0.7, and nothing is written for it; a model removed while its image file
/// downloaded is not added back.
#[tokio::test]
async fn a_model_not_on_the_list_is_left_alone() {
    let (dir, model) = folder(&[("mmproj-Model-F16.gguf", 8)]);
    let image = dir.path().join("mmproj-Model-F16.gguf");
    let store = store::Store::open_in_memory().unwrap();
    let model_text = model.display().to_string();
    assert_eq!(choice_for(&store, &model_text).await, Choice::default());
    assert!(!prepare_for_load(&store, &model_text, None).await);
    mark_unreadable(&store, &model, &image).await;
    attach(&store, &model_text, &image).await.unwrap();
    assert!(store.list_models().await.unwrap().is_empty());
}

/// Registering a model records the best image file next to it, skipping one that could not
/// be read with it before (an import of the same file keeps that memory).
#[tokio::test]
async fn registering_skips_an_image_file_that_could_not_be_read() {
    let (dir, model) = folder(&[("mmproj-Model-F16.gguf", 8), ("mmproj-Model-BF16.gguf", 8)]);
    let f16 = dir.path().join("mmproj-Model-F16.gguf");
    let bf16 = dir.path().join("mmproj-Model-BF16.gguf");
    let store = installed(&model, None).await;
    assert_eq!(
        stored(&store, &model).await.projector,
        Some(f16.display().to_string())
    );
    mark_unreadable(&store, &model, &f16).await;
    register_installed_in(
        &store,
        &model,
        None,
        std::fs::metadata(&model).unwrap().len(),
        None,
        None,
        store::ModelSource::Import,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        stored(&store, &model).await.projector,
        Some(bf16.display().to_string())
    );
}

/// The remembered files are saved in the row's summary as camelCase, left out when there
/// are none, and a list this version cannot read loses only itself.
#[test]
fn the_remembered_files_in_the_saved_summary() {
    let s: StoredSummary = serde_json::from_value(serde_json::json!({
        "architecture": "sample", "projector": "/m/mmproj.gguf",
        "unreadableProjectors": [{"path": "/m/mmproj.gguf", "size": 5, "modifiedMs": 7}]
    }))
    .unwrap();
    assert_eq!(
        s.unreadable_projectors,
        vec![stamp(Path::new("/m/mmproj.gguf"), 5, 7)]
    );
    let json = serde_json::to_value(&s).unwrap();
    assert_eq!(json["unreadableProjectors"][0]["modifiedMs"], 7);
    let none = serde_json::to_value(StoredSummary::default()).unwrap();
    assert!(none.get("unreadableProjectors").is_none());
    for bad in [
        serde_json::json!("text"),
        serde_json::json!([{"path": 3}]),
        serde_json::Value::Null,
    ] {
        let s: StoredSummary = serde_json::from_value(serde_json::json!({
            "architecture": "sample", "projector": "/m/mmproj.gguf", "unreadableProjectors": bad
        }))
        .unwrap_or_else(|e| panic!("{bad}: {e}"));
        assert!(s.unreadable_projectors.is_empty(), "{bad}");
        assert_eq!(s.projector.as_deref(), Some("/m/mmproj.gguf"));
    }
}
