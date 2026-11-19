//! Pictures dropped on the main window from the file manager, for the prompt's added pictures
//! (0.1.0). On Windows the webview's own HTML5 drop never carries a file while the window keeps
//! Tauri's drag-and-drop (which the app needs for its native drop events), so Studio reads the
//! files of Tauri's native drop instead.
//!
//! Each native drop on the main window is recorded here with when it happened
//! ([`DroppedFiles::record`]); [`add_dropped`] (the `studio_add_dropped` command) adds one of
//! those files by its place in the drop, and only within [`DROP_KEPT_FOR`] of it. The webview
//! never sends a path: it can add only a file the user has just dropped. The file passes the
//! checks a picture added any other way passes (`Gallery::add_ref`: at most 20 MB, a PNG,
//! JPEG, GIF or WebP by its own bytes), and only so many bytes are read.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use agent::media::RefRole;
use base64::Engine;
use tauri::Manager;

use super::actions::ref_dto;
use super::errors;
use super::gallery::{io_error, sniff_media};
use super::manager::StudioManager;
use crate::attachments::{ImageInput, MAX_IMAGE_BYTES, PICTURE_TYPES, PictureWorker};
use crate::dto::RefDto;
use crate::error::{AppError, AppResult};

/// How long the files of a native drop can be added after it: the webview asks at once, file
/// by file, and a large WebP picture takes a moment to convert.
pub const DROP_KEPT_FOR: Duration = Duration::from_secs(30);

/// The files of the last native drop on the main window, and when it happened.
#[derive(Default)]
pub struct DroppedFiles(Mutex<Option<(Instant, Vec<PathBuf>)>>);

impl DroppedFiles {
    /// Records the files of a native drop at `at`, in place of any earlier drop.
    pub fn record(&self, paths: Vec<PathBuf>, at: Instant) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some((at, paths));
    }

    /// File `index` of the last drop, when it happened at most [`DROP_KEPT_FOR`] before `now`.
    pub fn file_at(&self, index: usize, now: Instant) -> Option<PathBuf> {
        let dropped = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (at, paths) = dropped.as_ref()?;
        if now.saturating_duration_since(*at) > DROP_KEPT_FOR {
            return None;
        }
        paths.get(index).cloned()
    }
}

/// Records a native drop of files on the main window's webview (Tauri's drag-and-drop event),
/// for [`add_dropped`]. Other windows' drops, and the other drag events, are left alone.
pub fn on_webview_event<R: tauri::Runtime>(
    webview: &tauri::Webview<R>,
    event: &tauri::WebviewEvent,
) {
    if let tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event
        && webview.label() == crate::MAIN_WINDOW
        && let Some(dropped) = webview.try_state::<DroppedFiles>()
    {
        dropped.record(paths.clone(), Instant::now());
    }
}

/// Dropped file `path` as a picture to add: at most one byte more than the cap is read (a
/// larger file is then refused for its size), its type is what its own bytes say, and its name
/// is the file's own, for the refusal's words.
fn dropped_picture(path: &Path) -> AppResult<ImageInput> {
    let file_name = path.file_name().map_or_else(
        || "The dropped file".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let unreadable = |e: std::io::Error| errors::from_gallery(&io_error(e));
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(unreadable)?
        .take(MAX_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    // Anything else is refused by the picture check, by name.
    let mime = sniff_media(&bytes)
        .filter(|mime| PICTURE_TYPES.contains(mime))
        .unwrap_or("application/octet-stream")
        .to_owned();
    Ok(ImageInput {
        mime,
        file_name,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Adds file `index` of the last native drop on the main window (`dropped`) to the prompt, for
/// `role`: kept in the gallery's `refs/` as any added picture is ([`StudioManager::add_ref`],
/// with its checks), and given back with `role`. A drop older than [`DROP_KEPT_FOR`] at `now`,
/// or no file at `index`, is refused (`studio_drop_gone`); so is a folder.
pub async fn add_dropped(
    studio: &StudioManager,
    worker: &PictureWorker,
    dropped: &DroppedFiles,
    index: usize,
    role: Option<RefRole>,
    now: Instant,
) -> AppResult<RefDto> {
    let path = dropped
        .file_at(index, now)
        .filter(|path| !path.is_dir())
        .ok_or_else(errors::drop_gone)?;
    let image = tokio::task::spawn_blocking(move || dropped_picture(&path))
        .await
        .map_err(AppError::internal)??;
    let stored = studio.add_ref(worker, &image).await?;
    Ok(RefDto {
        role,
        ..ref_dto(stored)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::commands::tests::harness;
    use crate::studio::test_support::{Scratch, png};

    /// A drop is kept for [`DROP_KEPT_FOR`], file by file; a later drop replaces it.
    #[test]
    fn a_drop_is_kept_for_a_short_while_file_by_file() {
        let dropped = DroppedFiles::default();
        let at = Instant::now();
        assert_eq!(dropped.file_at(0, at), None, "nothing was dropped");
        dropped.record(vec!["/a.png".into(), "/b.png".into()], at);
        assert_eq!(dropped.file_at(1, at), Some("/b.png".into()));
        assert_eq!(dropped.file_at(2, at), None);
        assert_eq!(
            dropped.file_at(0, at + DROP_KEPT_FOR),
            Some("/a.png".into())
        );
        assert_eq!(
            dropped.file_at(0, at + DROP_KEPT_FOR + Duration::from_millis(1)),
            None
        );
        dropped.record(vec!["/c.png".into()], at + DROP_KEPT_FOR);
        assert_eq!(
            dropped.file_at(0, at + DROP_KEPT_FOR),
            Some("/c.png".into())
        );
        assert_eq!(dropped.file_at(1, at + DROP_KEPT_FOR), None);
    }

    /// Finding deferred-windows-drop (backend half): a file of the last native drop is added
    /// as any picture is, with the role asked for; only the recorded files can be added, only
    /// for a short while, and each passes the picture check (its type by its own bytes, its
    /// size under the cap).
    #[tokio::test]
    async fn a_dropped_picture_is_added_from_the_recorded_drop_only() {
        let docs = Scratch::new();
        let h = harness().await;
        let worker = PictureWorker::app();
        let picture = png(4, 4);
        let kite = docs.write("kite.png", &picture);
        let notes = docs.write("notes.png", b"not a picture at all");
        let big = docs.path().join("big.png");
        let mut too_big = picture.clone();
        too_big.resize(MAX_IMAGE_BYTES + 1, 0);
        std::fs::write(&big, &too_big).unwrap();
        let folder = docs.path().join("a folder");
        std::fs::create_dir(&folder).unwrap();
        let dropped = DroppedFiles::default();
        let at = Instant::now();
        dropped.record(vec![kite.clone(), notes, big, folder], at);

        let added = add_dropped(
            &h.manager,
            &worker,
            &dropped,
            0,
            Some(RefRole::StartFrame),
            at,
        )
        .await
        .unwrap();
        assert_eq!(added.role, Some(RefRole::StartFrame));
        assert_eq!(added.mime, "image/png");
        assert_eq!(added.size_bytes, picture.len() as u64);
        let (kept, _) = h.manager.gallery().ref_file(&added.sha256).unwrap();
        assert_eq!(std::fs::read(kept).unwrap(), picture);

        let not_a_picture = add_dropped(&h.manager, &worker, &dropped, 1, None, at)
            .await
            .unwrap_err();
        assert_eq!(
            not_a_picture.message,
            "notes.png is not a PNG, JPEG, GIF or WebP image."
        );
        let too_big = add_dropped(&h.manager, &worker, &dropped, 2, None, at)
            .await
            .unwrap_err();
        assert_eq!(too_big.message, "big.png is larger than 20 MB.");
        for (index, now) in [
            // A folder, no file at that place, and a drop too long ago.
            (3, at),
            (4, at),
            (0, at + DROP_KEPT_FOR + Duration::from_secs(1)),
        ] {
            let gone = add_dropped(&h.manager, &worker, &dropped, index, None, now)
                .await
                .unwrap_err();
            assert_eq!(gone.code, "studio_drop_gone", "{index}");
        }
        // Nothing dropped at all.
        let nothing = add_dropped(&h.manager, &worker, &DroppedFiles::default(), 0, None, at)
            .await
            .unwrap_err();
        assert_eq!(nothing.code, "studio_drop_gone");
    }
}
