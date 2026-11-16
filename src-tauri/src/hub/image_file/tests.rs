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
        sha256: None,
    }
}

/// [`stamp`] of a file the app downloaded, with the SHA-256 its repository published.
fn downloaded_stamp(path: &Path, size: u64, sha256: &str) -> FileStamp {
    FileStamp {
        sha256: Some(sha256.into()),
        ..stamp(path, size, 1)
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
    assert!(is_unreadable(std::slice::from_ref(&a2), &a2));
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
    let c = choose(
        Some(&recorded_text),
        std::slice::from_ref(&near),
        &[],
        all(),
    );
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
    let c = choose(Some(&recorded_text), std::slice::from_ref(&near), &[], gone);
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
    let c = choose(
        Some(&recorded_text),
        std::slice::from_ref(&recorded),
        &bad,
        all(),
    );
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

    // Only the one that could not be read. Downloaded by the app from the same upload (the
    // same published SHA-256): nothing else to offer. A copy found next to the model (its
    // SHA-256 unknown), or a newer upload under the same name and size: download it again
    // (M1 of the P2 review). A file that is not one of those still comes first.
    let only = vec![
        repo_file("Model-Q4_K_M.gguf", 4_000),
        repo_file("Model.mmproj-f16.gguf", 300),
    ];
    let same_upload = downloaded_stamp(
        Path::new("/models/x/Model.mmproj-f16.gguf"),
        300,
        &"ab".repeat(32),
    );
    assert_eq!(pick(&only, std::slice::from_ref(&same_upload)), (Offer::NoOther, None));
    let again = Offer::Again {
        file: "Model.mmproj-f16.gguf".into(),
        size_bytes: 300,
    };
    let (offer, file) = pick(&only, std::slice::from_ref(&bad_f16));
    assert_eq!(offer, again);
    assert_eq!(file.unwrap().path, "Model.mmproj-f16.gguf");
    let other_upload = downloaded_stamp(
        Path::new("/models/x/Model.mmproj-f16.gguf"),
        300,
        &"cd".repeat(32),
    );
    assert_eq!(pick(&only, &[other_upload]).0, again);
    let unpublished = vec![RepoFile {
        sha256: None,
        ..repo_file("Model.mmproj-f16.gguf", 300)
    }];
    assert_eq!(pick(&unpublished, std::slice::from_ref(&same_upload)).0, again);
    assert_eq!(
        pick(&files, &[same_upload.clone(), bad_f16.clone()]).0,
        Offer::Available {
            file: "mmproj-Model-BF16.gguf".into(),
            size_bytes: 300
        }
    );
    // None at all.
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
        serde_json::to_value(Offer::Again {
            file: "mmproj-F16.gguf".into(),
            size_bytes: 7
        })
        .unwrap(),
        serde_json::json!({"kind": "again", "file": "mmproj-F16.gguf", "sizeBytes": 7})
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
    let before = store.model_by_path(&model_text).await.unwrap().unwrap();
    attach(&store, &model_text, &downloaded, None).await.unwrap();
    let saved = stored(&store, &model).await;
    assert_eq!(
        saved.projector.as_deref(),
        Some(downloaded.display().to_string().as_str())
    );
    assert!(saved.unreadable_projectors.is_empty());
    let choice = choice_for(&store, &model_text).await;
    assert_eq!(choice.path, Some(downloaded.display().to_string()));
    // The rest of the row stays as it was.
    let rec = store.model_by_path(&model_text).await.unwrap().unwrap();
    assert_eq!(
        (
            rec.id,
            &rec.repo,
            &rec.file_name,
            rec.size_bytes,
            rec.source
        ),
        (
            before.id,
            &before.repo,
            &before.file_name,
            before.size_bytes,
            before.source
        )
    );
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
    attach(&store, &model_text, &image, None).await.unwrap();
    assert!(!forget_unreadable(&store, &model_text).await.unwrap());
    assert_eq!(
        save_repo_image_files(&store, &model_text, &[repo_file("mmproj-F16.gguf", 8)])
            .await
            .unwrap(),
        None
    );
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

// ---- Round 2 of the P2 review -------------------------------------------------------------

/// M1: an image file that failed for a reason that was not its own (a graphics card's
/// error) can be tried again: "Try it again" forgets the files remembered as unreadable,
/// and the next load uses it.
#[tokio::test]
async fn an_unreadable_image_file_can_be_tried_again() {
    let (dir, model) = folder(&[("mmproj-Model-F16.gguf", 8)]);
    let image = dir.path().join("mmproj-Model-F16.gguf");
    let model_text = model.display().to_string();
    let store = installed(&model, Some("someone/Model-GGUF")).await;
    mark_unreadable(&store, &model, &image).await;
    assert!(choice_for(&store, &model_text).await.left_out);
    assert!(forget_unreadable(&store, &model_text).await.unwrap());
    assert!(stored(&store, &model).await.unreadable_projectors.is_empty());
    let choice = choice_for(&store, &model_text).await;
    assert_eq!(choice.path, Some(image.display().to_string()));
    assert!(!choice.left_out);
    // Nothing to forget: nothing changes.
    assert!(!forget_unreadable(&store, &model_text).await.unwrap());
}

/// M1: what the app knows of an image file it downloaded (the SHA-256 its repository
/// published) goes with it when it cannot be read, so Installed can tell whether
/// downloading it again could help; a file found next to the model has none.
#[tokio::test]
async fn the_checksum_of_a_downloaded_image_file_is_remembered_with_it() {
    let (dir, model) = folder(&[]);
    let model_text = model.display().to_string();
    let store = installed(&model, Some("someone/Model-GGUF")).await;
    let downloaded = dir.path().join("mmproj-Model-F16.gguf");
    std::fs::write(&downloaded, vec![2u8; 70]).unwrap();
    let sha = "ef".repeat(32);
    attach(&store, &model_text, &downloaded, Some(sha.clone()))
        .await
        .unwrap();
    assert_eq!(stored(&store, &model).await.projector_sha256, Some(sha.clone()));
    mark_unreadable(&store, &model, &downloaded).await;
    let saved = stored(&store, &model).await;
    assert_eq!(saved.unreadable_projectors[0].sha256, Some(sha));

    // Another one put next to the model by hand and recorded by a load: its checksum is
    // not known, and neither is it when it cannot be read.
    let (dir, model) = folder(&[]);
    let model_text = model.display().to_string();
    let store = installed(&model, Some("someone/Model-GGUF")).await;
    let image = dir.path().join("mmproj-Model-BF16.gguf");
    std::fs::write(&image, vec![0u8; 64]).unwrap();
    attach(&store, &model_text, &dir.path().join("gone.gguf"), Some("aa".repeat(32)))
        .await
        .unwrap();
    let image_text = image.display().to_string();
    assert!(!prepare_for_load(&store, &model_text, Some(&image_text)).await);
    let saved = stored(&store, &model).await;
    assert_eq!(saved.projector.as_deref(), Some(image_text.as_str()));
    assert_eq!(saved.projector_sha256, None);
    mark_unreadable(&store, &model, &image).await;
    assert_eq!(stored(&store, &model).await.unreadable_projectors[0].sha256, None);
}

/// M2: Installed offers an image file from what is saved on the model's row, without
/// asking Hugging Face: the repository's image files saved when it was downloaded (or when
/// Installed was asked to look). Nothing is known for a model without a repository or
/// downloaded before 0.0.7 (Installed then offers to look); a repository without one is
/// known to have none.
#[tokio::test]
async fn the_offer_comes_from_what_is_saved() {
    let files = vec![
        repo_file("mmproj-Model-BF16.gguf", 300),
        repo_file("Model.mmproj-f16.gguf", 300),
    ];
    let mut s = StoredSummary::default();
    assert_eq!(saved_offer(Some("someone/Model-GGUF"), &s), None);
    s.repo_image_files = Some(Vec::new());
    assert_eq!(saved_offer(Some("someone/Model-GGUF"), &s), Some(Offer::NoImageFile));
    assert_eq!(saved_offer(None, &s), None);
    s.repo_image_files = Some(files.clone());
    assert_eq!(
        saved_offer(Some("someone/Model-GGUF"), &s),
        Some(Offer::Available {
            file: "Model.mmproj-f16.gguf".into(),
            size_bytes: 300
        })
    );

    // Downloaded with them: saved on its row, and kept when the model is imported again.
    let (_dir, model) = folder(&[]);
    let model_text = model.display().to_string();
    let store = store::Store::open_in_memory().unwrap();
    let size = std::fs::metadata(&model).unwrap().len();
    register_installed_in(
        &store,
        &model,
        Some("someone/Model-GGUF".into()),
        size,
        None,
        None,
        store::ModelSource::Download,
        SavedFacts {
            image_files: Some(files.clone()),
            ..SavedFacts::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(stored(&store, &model).await.repo_image_files, Some(files.clone()));
    register_installed_in(
        &store,
        &model,
        None,
        size,
        None,
        None,
        store::ModelSource::Import,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    assert_eq!(stored(&store, &model).await.repo_image_files, Some(files.clone()));

    // Looked for later: what the listing has is saved, only its image files, best first.
    let store = installed(&model, Some("someone/Model-GGUF")).await;
    assert_eq!(stored(&store, &model).await.repo_image_files, None);
    let listing = vec![
        repo_file("README.md", 1),
        repo_file("Model-Q4_K_M.gguf", 4_000),
        repo_file("mmproj-Model-BF16.gguf", 300),
        repo_file("Model.mmproj-f16.gguf", 300),
    ];
    let offer = save_repo_image_files(&store, &model_text, &listing)
        .await
        .unwrap();
    assert_eq!(
        offer,
        Some(Offer::Available {
            file: "Model.mmproj-f16.gguf".into(),
            size_bytes: 300
        })
    );
    let saved = stored(&store, &model).await;
    let names: Vec<&str> = saved
        .repo_image_files
        .as_deref()
        .unwrap()
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(names, ["Model.mmproj-f16.gguf", "mmproj-Model-BF16.gguf"]);
    // A downloaded image file keeps them.
    attach(&store, &model_text, &model, None).await.unwrap();
    assert!(stored(&store, &model).await.repo_image_files.is_some());
    // A repository without one.
    let offer = save_repo_image_files(&store, &model_text, &listing[..2])
        .await
        .unwrap();
    assert_eq!(offer, Some(Offer::NoImageFile));
    assert_eq!(stored(&store, &model).await.repo_image_files, Some(Vec::new()));
}

/// The header of a model with this embedding length.
fn model_header(embedding_length: u32) -> Vec<u8> {
    models::gguf::testing::Writer::new()
        .str("general.architecture", "sample")
        .u32("sample.context_length", 4096)
        .u32("sample.block_count", 2)
        .u32("sample.embedding_length", embedding_length)
        .build()
}

/// The header of an image file of this kind and width.
fn image_header(kind: &str, width: u32) -> Vec<u8> {
    models::gguf::testing::Writer::new()
        .str("general.architecture", "clip")
        .str("clip.projector_type", kind)
        .u32("clip.vision.projection_dim", width)
        .build()
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// m4: in a folder several models share, another model's image file is never used: not
/// when its width differs from the model's embedding length (llama.cpp refuses that), and,
/// where the width is unknown, not when its name is another model's. Its own, by width or
/// name, comes first; one whose name names no model is used after them. A text-only model
/// there gets none, and says nothing about the others' files.
#[tokio::test]
async fn image_files_of_other_models_are_not_used() {
    let dir = Scratch::new();
    let at = |name: &str| dir.path().join(name);
    std::fs::write(at("Gemma-3-12B-it-Q4_K_M.gguf"), model_header(3840)).unwrap();
    std::fs::write(at("Qwen3-8B-Q4_K_M.gguf"), model_header(4096)).unwrap();
    std::fs::write(at("mmproj-Gemma-3-12B-it-f16.gguf"), image_header("gemma3", 3840)).unwrap();
    std::fs::write(at("mmproj-Mistral-Small-24B-f16.gguf"), image_header("pixtral", 5120)).unwrap();
    // Renamed by hand, but made for Gemma: the width says so.
    std::fs::write(at("mmproj-renamed-F16.gguf"), image_header("gemma3", 3840)).unwrap();
    // Named after Gemma, but another width: not Gemma's.
    std::fs::write(at("mmproj-Gemma-3-12B-it-BF16.gguf"), image_header("gemma3", 2560)).unwrap();
    // Named after nothing, and the old LLaVA kind that names CLIP's own width there.
    std::fs::write(at("mmproj-model-f16.gguf"), image_header("mlp", 768)).unwrap();
    // Not an image file at all, though named like one.
    std::fs::write(at("mmproj-Gemma-3-12B-it-Q8_0.gguf"), model_header(3840)).unwrap();

    let gemma = at("Gemma-3-12B-it-Q4_K_M.gguf");
    let qwen = at("Qwen3-8B-Q4_K_M.gguf");
    assert_eq!(
        names(&candidates(&gemma, Some(3840))),
        ["mmproj-Gemma-3-12B-it-f16.gguf", "mmproj-renamed-F16.gguf", "mmproj-model-f16.gguf"]
    );
    // The text-only model: only the file that names no model, of a kind whose width the
    // header does not give.
    assert_eq!(names(&candidates(&qwen, Some(4096))), ["mmproj-model-f16.gguf"]);
    // Its width unknown (a row saved before 0.0.7): by name only.
    assert_eq!(
        names(&candidates(&gemma, None)),
        ["mmproj-Gemma-3-12B-it-f16.gguf", "mmproj-Gemma-3-12B-it-BF16.gguf", "mmproj-model-f16.gguf"]
    );
    assert_eq!(names(&candidates(&qwen, None)), ["mmproj-model-f16.gguf"]);
    std::fs::remove_file(at("mmproj-model-f16.gguf")).unwrap();
    assert!(candidates(&qwen, Some(4096)).is_empty());
    assert!(candidates(&qwen, None).is_empty());

    // The text-only model on the Installed list: no image file, and nothing to say about
    // the others' files, even one remembered as unreadable.
    let store = store::Store::open_in_memory().unwrap();
    let size = std::fs::metadata(&qwen).unwrap().len();
    register_installed_in(
        &store,
        &qwen,
        None,
        size,
        None,
        None,
        store::ModelSource::Import,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    let qwen_text = qwen.display().to_string();
    assert_eq!(stored(&store, &qwen).await.embedding_length, Some(4096));
    assert_eq!(stored(&store, &qwen).await.projector, None);
    mark_unreadable(&store, &qwen, &at("mmproj-Gemma-3-12B-it-f16.gguf")).await;
    assert_eq!(choice_for(&store, &qwen_text).await, Choice::default());
    // Gemma gets its own.
    let store = store::Store::open_in_memory().unwrap();
    let size = std::fs::metadata(&gemma).unwrap().len();
    register_installed_in(
        &store,
        &gemma,
        None,
        size,
        None,
        None,
        store::ModelSource::Import,
        SavedFacts::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        stored(&store, &gemma).await.projector,
        Some(at("mmproj-Gemma-3-12B-it-f16.gguf").display().to_string())
    );
}

/// m4: a row saved before the embedding length was kept gets it when the next load asks
/// which image file to use, so the width check applies from then on.
#[tokio::test]
async fn the_embedding_length_is_filled_in_when_a_load_asks() {
    let dir = Scratch::new();
    let model = dir.path().join("Qwen3-8B-Q4_K_M.gguf");
    std::fs::write(&model, model_header(4096)).unwrap();
    // Named after nothing, made for another model: only the width tells.
    std::fs::write(dir.path().join("mmproj-F16.gguf"), image_header("gemma3", 3840)).unwrap();
    let store = installed(&model, None).await;
    let rec = store
        .model_by_path(&model.display().to_string())
        .await
        .unwrap()
        .unwrap();
    let mut old = stored(&store, &model).await;
    old.embedding_length = None;
    old.projector = None;
    save(&store, &rec, &old).await.unwrap();
    assert_eq!(names(&candidates(&model, None)), ["mmproj-F16.gguf"]);
    assert_eq!(choice_for(&store, &model.display().to_string()).await, Choice::default());
    assert_eq!(stored(&store, &model).await.embedding_length, Some(4096));
}

/// m4: the words of a name that name a model, without its precision, kind or quantization.
#[test]
fn the_words_that_name_a_model() {
    for (name, words) in [
        ("mmproj-Gemma-3-12B-it-f16.gguf", vec!["gemma", "12b", "it"]),
        ("Gemma-3-12B-it-Q4_K_M.gguf", vec!["gemma", "12b", "it"]),
        (
            "Ministral-3-14B-Reasoning-2512-Uncensored.mmproj-f16.gguf",
            vec!["ministral", "14b", "reasoning", "2512", "uncensored"],
        ),
        (
            "Ministral-3-14B-Reasoning-2512-Uncensored.Q4_K_M.gguf",
            vec!["ministral", "14b", "reasoning", "2512", "uncensored"],
        ),
        ("mmproj-F16.gguf", vec![]),
        ("mmproj-model-f16.gguf", vec![]),
        ("Model-UD-IQ4_XS.gguf", vec![]),
        ("google_gemma-3-4b-it-Q8_0.gguf", vec!["google", "gemma", "4b", "it"]),
    ] {
        assert_eq!(name_words(name), words, "{name}");
    }
}

/// m9: an image file is used only when it is a plain file next to the model: never a link
/// (which could point anywhere) or a folder.
#[cfg(unix)]
#[test]
fn links_next_to_the_model_are_not_image_files() {
    let (dir, model) = folder(&[("mmproj-Model-F16.gguf", 4)]);
    let elsewhere = Scratch::new();
    let target = elsewhere.path().join("real.gguf");
    std::fs::write(&target, vec![0u8; 4]).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("mmproj-Model-BF16.gguf")).unwrap();
    std::fs::create_dir(dir.path().join("mmproj-Model-Q8_0.gguf")).unwrap();
    assert_eq!(names(&beside(&model)), ["mmproj-Model-F16.gguf"]);
}
