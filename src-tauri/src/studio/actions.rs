//! What Studio's gallery actions do (0.1.0), behind their thin commands in
//! [`super::commands`]: export, with the run's settings beside the copy; a video's poster
//! frame ([`POSTER_CAP`]); *Send to chat* ([`for_chat`]) and *Use in prompt*.

use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::errors::{self, PictureUse};
use super::gallery::{RefStored, io_error, sniff_media};
use super::manager::{StudioManager, output_dto};
use crate::artifacts::export_target;
use crate::attachments::{ImageInput, MAX_IMAGE_BYTES, PICTURE_TYPES, PictureWorker};
use crate::dto::{OutputDto, RefDto};
use crate::error::{AppError, AppResult};

/// The largest poster frame of a video the webview may save as its thumbnail: 1 MiB.
pub const POSTER_CAP: usize = 1024 * 1024;

/// Output `output_id`, refused as `not_found` when it does not exist.
pub(super) async fn output_of(
    studio: &StudioManager,
    output_id: &str,
) -> AppResult<store::CreationOutput> {
    studio
        .deps()
        .store
        .get_creation_output(output_id)
        .await?
        .ok_or_else(|| AppError::not_found("result"))
}

/// Saves `jpeg_base64` (base64, no `data:` prefix) as video output `output_id`'s thumbnail,
/// in the gallery's `.thumbs/`, replacing any earlier one, and gives the output with it.
/// Anything but a JPEG of at most [`POSTER_CAP`] bytes, or a poster for a result that is not
/// a video, is refused before it is written.
pub async fn save_thumb(
    studio: &StudioManager,
    output_id: &str,
    jpeg_base64: &str,
) -> AppResult<OutputDto> {
    // Base64 of more than the cap is longer than this: it is not decoded at all.
    if jpeg_base64.len() > POSTER_CAP.div_ceil(3) * 4 {
        return Err(errors::bad_poster());
    }
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(jpeg_base64)
        .map_err(|_| errors::bad_poster())?;
    if jpeg.len() > POSTER_CAP || sniff_media(&jpeg) != Some("image/jpeg") {
        return Err(errors::bad_poster());
    }
    let mut output = output_of(studio, output_id).await?;
    // A picture's thumbnail is made from the picture itself.
    if !output.mime.starts_with("video/") {
        return Err(errors::poster_only_for_video());
    }
    let (gallery, id) = (studio.gallery(), output.id.clone());
    let path = tokio::task::spawn_blocking(move || gallery.write_thumb(&id, &jpeg))
        .await
        .map_err(AppError::internal)?
        .map_err(|e| errors::from_gallery(&e))?;
    let path = path.display().to_string();
    studio
        .deps()
        .store
        .set_creation_thumb(&output.id, &path)
        .await?;
    output.thumb_path = Some(path);
    Ok(output_dto(&output))
}

/// Copies output `output_id` to `path` and gives where it went. `path` must keep the
/// result's own extension and stay out of the app's folders: `forbidden`, the gallery
/// folder, and the gallery folder the result was kept in when Settings → Studio has moved
/// the gallery since ([`export_target`]); and it must not be the result's own file (through
/// a link), which a copy would empty. With `with_settings`, the run's prompt, model,
/// settings and cost go beside it as `<name>.json` ([`settings_record`]).
pub async fn export(
    studio: &StudioManager,
    output_id: &str,
    path: &str,
    with_settings: bool,
    forbidden: &[PathBuf],
) -> AppResult<String> {
    let output = output_of(studio, output_id).await?;
    export_output(studio, output, path, with_settings, forbidden).await
}

/// [`export`] of `output`, as it was looked up. Nothing is written until every check has
/// passed. With `with_settings`, the run is read first, so a run deleted meanwhile leaves
/// no copy without its settings; and a `<name>.json` already there is refused, never
/// replaced, since the save dialog asked only about the copy.
async fn export_output(
    studio: &StudioManager,
    output: store::CreationOutput,
    path: &str,
    with_settings: bool,
    forbidden: &[PathBuf],
) -> AppResult<String> {
    let run = if with_settings {
        let run = studio
            .deps()
            .store
            .get_creation_run(&output.run_id)
            .await?
            .ok_or_else(|| AppError::not_found("run"))?;
        Some(run)
    } else {
        None
    };
    let source = PathBuf::from(&output.path);
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let mut forbidden = forbidden.to_vec();
    forbidden.push(studio.gallery().root().to_owned());
    // `<gallery>/YYYY-MM/<file>`: the result's own gallery folder, with its sidecars,
    // `.thumbs/` and `refs/`, which stays where it was when the gallery moves.
    if let Some(kept_in) = source.parent().and_then(Path::parent) {
        forbidden.push(kept_in.to_owned());
    }
    let target = export_target(path, ext, &forbidden).map_err(AppError::bad_request)?;
    let source_at = tokio::fs::canonicalize(&source)
        .await
        .map_err(|e| errors::from_gallery(&io_error(e)))?;
    if tokio::fs::canonicalize(&target)
        .await
        .is_ok_and(|target_at| target_at == source_at)
    {
        return Err(errors::export_onto_itself());
    }
    let settings_at = target.with_extension("json");
    // A link counts as a file there, even one that leads nowhere.
    if run.is_some() && tokio::fs::symlink_metadata(&settings_at).await.is_ok() {
        let name = settings_at
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        return Err(errors::settings_file_exists(&name));
    }
    tokio::fs::copy(&source, &target)
        .await
        .map_err(|e| errors::not_exported(e.to_string()))?;
    if let Some(run) = run {
        let json = serde_json::to_vec_pretty(&settings_record(&run, &output))
            .map_err(AppError::internal)?;
        // Made new: a file that appeared there since the check is not replaced either.
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&settings_at)
            .await
            .map_err(|e| errors::not_exported(e.to_string()))?;
        // Flushed, so a failed write is seen here rather than lost when the file is closed.
        async {
            file.write_all(&json).await?;
            file.flush().await
        }
        .await
        .map_err(|e| errors::not_exported(e.to_string()))?;
    }
    Ok(target.display().to_string())
}

/// What an export writes beside a result: how it was made (the run's endpoint, model,
/// prompt, settings, estimate and cost, and the result's seed). No path, and no key (a run
/// holds none).
fn settings_record(run: &store::CreationRun, output: &store::CreationOutput) -> Value {
    let made = jiff::Timestamp::from_millisecond(run.created_at)
        .map(|at| at.to_string())
        .ok();
    json!({
        "kind": media_kind_name(run.kind),
        "runsOn": run.runs_on.as_str(),
        "endpoint": run.provider_name,
        "host": run.provider_host,
        "upstream": run.upstream,
        "model": run.model,
        "modelLabel": run.model_label,
        "prompt": run.prompt,
        "params": run.params.get("params"),
        "extra": run.params.get("extra"),
        "routing": run.params.get("routing"),
        "seed": output.seed,
        "estimateUsd": run.estimate_usd,
        "costUsd": run.cost_usd,
        "made": made,
    })
}

/// A run's kind as an export names it.
fn media_kind_name(kind: store::CreationKind) -> &'static str {
    match kind {
        store::CreationKind::Image => "picture",
        store::CreationKind::Video => "video",
    }
}

/// Whether result `output` may go into a chat's message box: a PNG, JPEG, GIF or WebP
/// picture of at most 20 MB, the composer's own limits (a chat model takes no video).
pub(crate) fn for_chat(output: &store::CreationOutput) -> AppResult<()> {
    picture_check(output, PictureUse::Chat)
}

/// Whether result `output` may go to `to`: a chat and the prompt's added pictures take the
/// same pictures.
fn picture_check(output: &store::CreationOutput, to: PictureUse) -> AppResult<()> {
    if !PICTURE_TYPES.contains(&output.mime.as_str()) {
        return Err(errors::not_a_picture_for(
            to,
            output.mime.starts_with("video/"),
        ));
    }
    if output.size_bytes > MAX_IMAGE_BYTES as u64 {
        return Err(errors::picture_too_big_for(to));
    }
    Ok(())
}

/// The bytes of picture `output` (which passed [`picture_check`] for `to`); a file that has
/// grown past the limit since it was recorded is refused the same way.
async fn picture_bytes(output: &store::CreationOutput, to: PictureUse) -> AppResult<Vec<u8>> {
    let path = PathBuf::from(&output.path);
    let bytes = tokio::task::spawn_blocking(move || {
        let unreachable = |e: std::io::Error| errors::from_gallery(&io_error(e));
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(unreachable)?
            .take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(unreachable)?;
        Ok::<_, AppError>(bytes)
    })
    .await
    .map_err(AppError::internal)??;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(errors::picture_too_big_for(to));
    }
    Ok(bytes)
}

/// Picture output `output_id` as the composer takes a picture (refused as [`for_chat`]
/// says), named for what it is rather than by its file's id.
pub async fn output_for_chat(studio: &StudioManager, output_id: &str) -> AppResult<ImageInput> {
    let output = output_of(studio, output_id).await?;
    for_chat(&output)?;
    let bytes = picture_bytes(&output, PictureUse::Chat).await?;
    let ext = Path::new(&output.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    Ok(ImageInput {
        file_name: format!("Studio picture.{ext}"),
        mime: output.mime,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Adds picture output `output_id` to the prompt: kept in the gallery's `refs/` as a picture
/// the user added is (`Gallery::add_ref`, through `worker` for a WebP picture).
pub async fn ref_from_output(
    studio: &StudioManager,
    worker: &PictureWorker,
    output_id: &str,
) -> AppResult<RefDto> {
    let output = output_of(studio, output_id).await?;
    picture_check(&output, PictureUse::Prompt)?;
    let bytes = picture_bytes(&output, PictureUse::Prompt).await?;
    let image = ImageInput {
        mime: output.mime,
        file_name: "This picture".to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    };
    let stored = studio.gallery().add_ref(worker, &image).await?;
    Ok(ref_dto(stored))
}

/// An added picture as kept, before any run records what it is for.
pub(super) fn ref_dto(stored: RefStored) -> RefDto {
    RefDto {
        sha256: stored.sha256,
        mime: stored.mime,
        size_bytes: stored.size_bytes,
        role: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use agent::media::MediaKind;

    use super::*;
    use crate::studio::commands::tests::{harness, now};
    use crate::studio::test_support::{Scratch, png};

    #[tokio::test]
    async fn export_with_settings_replaces_no_file_and_leaves_no_copy_without_them() {
        // Made first, so it goes after the harness.
        let docs = Scratch::new();
        let h = harness().await;
        h.seed_run("p1", MediaKind::Image, "a red kite", now())
            .await;
        let picture = png(4, 4);
        let out = h
            .seed_output("p1", "image/png", &picture, picture.len() as u64)
            .await;
        let at = |name: &str| docs.path().join(name).display().to_string();

        // A `<name>.json` already there: the save dialog asked only about the copy, so
        // nothing is copied and the file is kept as it was.
        std::fs::write(docs.path().join("kite.json"), b"my own notes").unwrap();
        let err = export(&h.manager, &out, &at("kite.png"), true, &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert_eq!(err.message, "A file named kite.json is already there.");
        assert_eq!(err.next_step, "Choose another name.");
        assert!(!docs.path().join("kite.png").exists());
        assert_eq!(
            std::fs::read(docs.path().join("kite.json")).unwrap(),
            b"my own notes"
        );
        // Without its settings, the copy is made and the file is not touched.
        export(&h.manager, &out, &at("kite.png"), false, &[])
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(docs.path().join("kite.png")).unwrap(),
            picture
        );
        assert_eq!(
            std::fs::read(docs.path().join("kite.json")).unwrap(),
            b"my own notes"
        );

        // The run is deleted once its result was looked up (its rows go before its files):
        // no copy is left without its settings.
        let output = h.store.get_creation_output(&out).await.unwrap().unwrap();
        h.store
            .delete_creation_runs(vec!["p1".into()], 0)
            .await
            .unwrap();
        assert!(Path::new(&output.path).exists());
        let err = export_output(&h.manager, output, &at("lonely.png"), true, &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "not_found");
        assert!(!docs.path().join("lonely.png").exists());
        assert!(!docs.path().join("lonely.json").exists());
    }
}
