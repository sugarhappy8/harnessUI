//! Image attachments (ARCHITECTURE.md 8.6, M2): validated, stored once by content hash
//! under the local data folder, referenced from message content by attachment id, and
//! sent to the model as base64 data URIs (never as file or http URLs).

use std::path::PathBuf;

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Largest image accepted (decoded bytes).
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// Images per message.
pub const MAX_IMAGES: usize = 8;

/// An image sent from the composer.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    pub mime: String,
    pub file_name: String,
    /// Base64 of the bytes (no data: prefix).
    pub data: String,
}

/// Checks the bytes really are the claimed image type (magic numbers), so a renamed
/// file cannot be passed off as an image.
fn sniff(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

fn extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        _ => "webp",
    }
}

/// [0.0.6] Widest or tallest WebP picture turned into PNG, in pixels.
const MAX_PICTURE_SIDE: u32 = 8_192;
/// [0.0.6] Most memory the WebP decoder may set aside for one picture's pixels: 128 MiB, a
/// 33-megapixel picture with see-through parts or a 44-megapixel one without.
const MAX_PICTURE_DECODE_BYTES: u64 = 128 * 1024 * 1024;

/// [0.0.6] What the user is told when a WebP picture cannot be turned into PNG (broken, or
/// larger than the limits); what went wrong is in the details.
fn unreadable_picture(details: impl Into<String>) -> AppError {
    AppError::new(
        "picture_unreadable",
        "This picture could not be read.",
        "Try a PNG or JPEG.",
        details,
    )
}

/// [0.0.6] Whether every picture header in a WebP file, its own and each animation frame's,
/// claims at most [`MAX_PICTURE_SIDE`] pixels a side. The decoder checks the limits against
/// the canvas only, but sizes its buffers from each frame's own header, so a small canvas
/// holding a frame that claims 16,383 x 16,383 pixels would cost about 400 MB before it is
/// refused. A header cut short counts as too large.
fn webp_sides_within_limits(bytes: &[u8]) -> bool {
    fn side_ok(side: u32) -> bool {
        side <= MAX_PICTURE_SIDE
    }
    fn chunks_ok(mut data: &[u8], in_frame: bool) -> bool {
        while data.len() >= 8 {
            let size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
            let body = &data[8..];
            let body = &body[..size.min(body.len())];
            let ok = match &data[..4] {
                b"VP8 " => {
                    body.len() >= 10
                        && side_ok(u32::from(u16::from_le_bytes([body[6], body[7]]) & 0x3FFF))
                        && side_ok(u32::from(u16::from_le_bytes([body[8], body[9]]) & 0x3FFF))
                }
                b"VP8L" => {
                    body.len() >= 5 && {
                        let header = u32::from_le_bytes([body[1], body[2], body[3], body[4]]);
                        side_ok((header & 0x3FFF) + 1) && side_ok(((header >> 14) & 0x3FFF) + 1)
                    }
                }
                // A frame's picture follows its 16-byte frame header.
                b"ANMF" if !in_frame => body.len() < 16 || chunks_ok(&body[16..], true),
                _ => true,
            };
            if !ok {
                return false;
            }
            // Each chunk is padded to an even size.
            let next = size.saturating_add(8).saturating_add(size & 1);
            if next >= data.len() {
                break;
            }
            data = &data[next..];
        }
        true
    }
    bytes.len() >= 12 && chunks_ok(&bytes[12..], false)
}

/// [0.0.6] A WebP picture as PNG (the first frame of an animation), decoded within limits so
/// a crafted file cannot exhaust memory: at most [`MAX_PICTURE_SIDE`] pixels a side, the
/// canvas and every frame ([`webp_sides_within_limits`]), and [`MAX_PICTURE_DECODE_BYTES`]
/// for the pixels. Pure Rust (the `image` crate's WebP decoder); nothing else is started.
fn webp_to_png(bytes: &[u8]) -> image::ImageResult<Vec<u8>> {
    use image::error::{LimitError, LimitErrorKind};
    if !webp_sides_within_limits(bytes) {
        return Err(image::ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::DimensionError,
        )));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PICTURE_SIDE);
    limits.max_image_height = Some(MAX_PICTURE_SIDE);
    limits.max_alloc = Some(MAX_PICTURE_DECODE_BYTES);
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::WebP);
    reader.limits(limits);
    let picture = reader.decode()?;
    let mut png = Vec::new();
    picture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    Ok(png)
}

/// [0.0.6] A picture's MIME type and bytes as the model gets them. llama-server reads WebP
/// only through ffmpeg, which the app never lets it start (`--video-ffmpeg-dir` is a fixed
/// empty folder), so a WebP picture becomes PNG ([`webp_to_png`]); PNG, JPEG and GIF are
/// kept byte for byte. A WebP picture that cannot be turned into PNG gives the user's
/// plain message, also if the decoder panics.
fn picture_for_model(mime: String, bytes: Vec<u8>) -> AppResult<(String, Vec<u8>)> {
    if mime != "image/webp" {
        return Ok((mime, bytes));
    }
    match std::panic::catch_unwind(|| webp_to_png(&bytes)) {
        Ok(Ok(png)) => Ok(("image/png".to_owned(), png)),
        Ok(Err(e)) => Err(unreadable_picture(e.to_string())),
        Err(_) => Err(unreadable_picture("the WebP decoder stopped")),
    }
}

/// [0.0.6] [`picture_for_model`] away from the async threads: a large picture takes a while
/// to decode and encode.
async fn picture_for_model_async(mime: String, bytes: Vec<u8>) -> AppResult<(String, Vec<u8>)> {
    if mime != "image/webp" {
        return Ok((mime, bytes));
    }
    tokio::task::spawn_blocking(move || picture_for_model(mime, bytes))
        .await
        .map_err(|e| unreadable_picture(e.to_string()))?
}

/// A composer picture checked (its size, and that its bytes really are the type it claims)
/// and in the form it is stored and sent: its MIME type, file extension and bytes. [0.0.6] A
/// WebP picture comes back as PNG ([`picture_for_model`]).
async fn checked_picture(img: &ImageInput) -> AppResult<(String, &'static str, Vec<u8>)> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(img.data.as_bytes())
        .map_err(|_| AppError::bad_request("An attached image could not be read."))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(AppError::bad_request(format!(
            "{} is larger than {} MB.",
            img.file_name,
            MAX_IMAGE_BYTES / (1024 * 1024)
        )));
    }
    if !sniff(&img.mime, &bytes) {
        return Err(AppError::bad_request(format!(
            "{} is not a PNG, JPEG, GIF or WebP image.",
            img.file_name
        )));
    }
    let (mime, bytes) = picture_for_model_async(img.mime.clone(), bytes).await?;
    let ext = extension(&mime);
    Ok((mime, ext, bytes))
}

fn store_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| AppError::internal(e.to_string()))?
        .join("attachments");
    std::fs::create_dir_all(&dir).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(dir)
}

/// Validates and stores the images; returns the content parts for the message.
pub async fn save_images(
    app: &AppHandle,
    state: &AppState,
    chat_id: store::ChatId,
    images: &[ImageInput],
) -> AppResult<Vec<serde_json::Value>> {
    if images.len() > MAX_IMAGES {
        return Err(AppError::bad_request(format!(
            "Attach at most {MAX_IMAGES} images to one message."
        )));
    }
    let dir = store_dir(app)?;
    let mut parts = Vec::new();
    for img in images {
        let (mime, ext, bytes) = checked_picture(img).await?;
        let sha: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = dir.join(format!("{sha}.{ext}"));
        if !path.exists() {
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &bytes).map_err(|e| AppError::internal(e.to_string()))?;
            std::fs::rename(&tmp, &path).map_err(|e| AppError::internal(e.to_string()))?;
        }
        let name: String = img
            .file_name
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        let id = state
            .store
            .insert_attachment(store::NewAttachment {
                chat_id,
                sha256: sha,
                mime,
                file_name: name,
                size_bytes: bytes.len() as u64,
                bytes_path: path.display().to_string(),
                extracted_text: None,
            })
            .await?;
        parts.push(serde_json::json!({ "type": "image", "attachment_id": id.0 }));
    }
    Ok(parts)
}

/// Largest text or PDF document accepted.
pub const MAX_DOCUMENT_BYTES: usize = 32 * 1024 * 1024;
/// Bytes of text kept from one document at most, whatever the context (a last guard for
/// storage: [`document_cap_chars`] keeps every share under it, note included).
const MAX_DOCUMENT_TEXT: usize = 2 * 1024 * 1024;
/// [0.0.6] The most bytes one character takes in UTF-8: a share of at most
/// `MAX_DOCUMENT_TEXT / MAX_CHAR_BYTES` characters is under the storage guard in any language.
const MAX_CHAR_BYTES: usize = 4;

/// [0.0.5 memory] Characters per token when sizing a document to the context: the app's
/// cautious estimate (`agent::estimate`), as for mentions.
const CHARS_PER_TOKEN: usize = 3;
/// [0.0.5 memory] The fewest characters a document keeps, however small the context.
const MIN_DOCUMENT_CHARS: usize = 600;
/// [0.0.5 memory] Room kept in a cut document for the note about what was left out.
const CUT_NOTE_ROOM: usize = 320;

/// [0.0.5 memory] The characters each attached file of one message may keep with a context
/// of `n_ctx` tokens, when `shared_by` files (its documents and mentions together) are
/// attached: a fifth of the context each, and 40% of it shared by them all, the rule mentions
/// are cut by (`tools::symbols::resolve::cap_chars`, which also caps one mention at 8,000
/// tokens, and shares only among the mentions: a message with both documents and mentions
/// can carry more than 40%, at most 72% with 8 documents and 2 mentions).
/// TODO(0.0.6): count documents in the mentions' share too. That is the mentions' cap, in
/// `crates/tools/src/symbols/resolve.rs` and `src-tauri/src/mentions.rs`, which no 0.0.6
/// package owns (WP3b owns the documents' side, which already counts the mentions).
/// The current message is never summarised, so a document larger than this would stop the
/// chat with "too long" instead.
/// [0.0.6] At most a quarter of the storage guard in characters (524,288, about 175,000
/// tokens: contexts above about 870,000 tokens, which endpoints may report), so the text
/// sent, note included, is never cut again by the guard, whatever bytes its characters take.
pub fn document_cap_chars(n_ctx: usize, shared_by: usize) -> usize {
    let shared_by = shared_by.max(1);
    let tokens = (n_ctx / 5).min(n_ctx * 2 / 5 / shared_by);
    (tokens * CHARS_PER_TOKEN).clamp(MIN_DOCUMENT_CHARS, MAX_DOCUMENT_TEXT / MAX_CHAR_BYTES)
}

/// [0.0.5 memory] What was left out of an attached document cut to fit the context; stored
/// with its part of the message (`"cut"`), so a turn can tell the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCut {
    /// Characters of the file's text sent to the model.
    pub kept: usize,
    /// Characters of the file's text in all.
    pub total: usize,
}

/// [0.0.5 memory] `text` cut to at most `cap` characters, note included: the start of the
/// file, ending at a line end when one is near, then a note for the model saying how much
/// was left out and that the file tools can read more (with the line to start from, for a
/// text file; a PDF's text cannot be read by line). `None` when the whole text fits.
pub fn cut_document(text: &str, cap: usize, pdf: bool) -> (String, Option<DocumentCut>) {
    let total = text.chars().count();
    if total <= cap {
        return (text.to_owned(), None);
    }
    let room = cap.saturating_sub(CUT_NOTE_ROOM).max(1);
    // The byte where the `room`-th character starts.
    let mut end = text.char_indices().nth(room).map_or(text.len(), |(i, _)| i);
    // End at a line end in the last fifth of what is kept, so no line is cut in half.
    if let Some(nl) = text[..end].rfind('\n')
        && text[..nl].chars().count() >= room * 4 / 5
    {
        end = nl + 1;
    }
    let kept_text = &text[..end];
    let kept = kept_text.chars().count();
    let left = agent::context::group_digits((total - kept) as u64);
    let all = agent::context::group_digits(total as u64);
    let note = if pdf {
        format!(
            "[The rest of this file's text was left out to fit what the model can remember: \
             {left} of {all} characters. Tell the user if you need a part that is missing.]"
        )
    } else {
        let first = kept_text.matches('\n').count() + 1;
        let lines = text.lines().count().max(first);
        format!(
            "[The rest of this file was left out to fit what the model can remember: lines \
             {first}-{lines}, {left} of {all} characters. If the file is in an approved \
             folder, the file tools can read more: call read_file with its path and \
             offset={first}.]"
        )
    };
    let mut out = kept_text.to_owned();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&note);
    (out, Some(DocumentCut { kept, total }))
}

/// [0.0.6] How many times [`cut_to_share`] measures a cut with the model server's tokens.
const MAX_COUNTS: usize = 4;

/// [0.0.6] `text` cut to its share of the context ([`cut_document`] at `cap` characters),
/// then held to `cap / 3` tokens as the model server counts them, when it can count
/// (`count`: llama-server's own tokenizer for a local model; `None` for an endpoint, whose
/// requests the loop measures with the same 3-characters-a-token estimate). Numbers, code
/// and many languages take more tokens per character than the estimate: a local model's
/// loop counts exactly, so a document measured by the estimate alone could fill the
/// request and stop the turn with "too long" (seen with the real small model: a 1,200-line
/// table of numbers cut by the estimate took about 60% of 4,096 tokens). Each count that
/// is over shrinks the cut in proportion, with a tenth to spare; a count that fails keeps
/// the estimate's cut. Never below [`MIN_DOCUMENT_CHARS`].
pub async fn cut_to_share<F, Fut>(
    text: &str,
    cap: usize,
    pdf: bool,
    count: Option<F>,
) -> (String, Option<DocumentCut>)
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Option<usize>>,
{
    let mut chars = cap;
    let mut out = cut_document(text, chars, pdf);
    let Some(count) = count else {
        return out;
    };
    let allowed = (cap / CHARS_PER_TOKEN).max(1);
    for _ in 0..MAX_COUNTS {
        let Some(tokens) = count(out.0.clone()).await else {
            return out;
        };
        if tokens <= allowed || chars <= MIN_DOCUMENT_CHARS {
            return out;
        }
        let sent = out.0.chars().count().min(chars);
        let fewer = sent.saturating_mul(allowed).saturating_mul(9) / tokens.saturating_mul(10);
        chars = fewer.clamp(
            MIN_DOCUMENT_CHARS,
            chars.saturating_sub(1).max(MIN_DOCUMENT_CHARS),
        );
        out = cut_document(text, chars, pdf);
    }
    out
}

/// [0.0.6] An attached document's text as it is stored and sent: cut to its share of a
/// context of `n_ctx` tokens ([`document_cap_chars`], [`cut_to_share`]), then held under
/// the storage guard ([`MAX_DOCUMENT_TEXT`] bytes).
pub async fn document_for_model<F, Fut>(
    text: &str,
    n_ctx: usize,
    shared_by: usize,
    pdf: bool,
    count: Option<F>,
) -> (String, Option<DocumentCut>)
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Option<usize>>,
{
    let (mut text, cut) =
        cut_to_share(text, document_cap_chars(n_ctx, shared_by), pdf, count).await;
    if text.len() > MAX_DOCUMENT_TEXT {
        let mut end = MAX_DOCUMENT_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    (text, cut)
}

/// [0.0.5 memory] What the user is told when a turn sends a document that was cut to fit
/// (plain words; the model's own note is in the document's text). [0.0.6] In Full access
/// the file tools reach the whole computer except the app's own folders, so the note does
/// not narrow it to the chat's folders (D12); Ask first and Allow all say the same.
/// The file name's hidden characters (bidirectional overrides, zero-width and tag
/// characters) come as visible escapes, as on approval cards, so the note reads as written.
pub fn cut_note(name: &str, cut: DocumentCut, full_access: bool) -> String {
    let name = policy::sanitize::visible(name);
    let percent = (cut.kept * 100 / cut.total.max(1)).clamp(1, 99);
    let rest = if full_access {
        "Full access is on, so the model can read the rest itself if it finds the file on \
         this computer."
    } else {
        "If the file is in one of this chat's folders, the model can read the rest itself."
    };
    format!(
        "Only the first part of {name} was sent (about {percent}%), because the whole file is \
         more than the model can remember at once. {rest}"
    )
}

/// [0.0.5 memory] The documents of a stored message that were cut to fit, with what was
/// left out: (file name, cut).
pub fn document_cuts(content: &serde_json::Value) -> Vec<(String, DocumentCut)> {
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("document"))
                .filter(|p| p.get("mention").is_none())
                .filter_map(|p| {
                    let cut = serde_json::from_value::<DocumentCut>(p.get("cut")?.clone()).ok()?;
                    let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("document");
                    Some((name.to_owned(), cut))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A text or PDF file sent from the composer.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInput {
    pub file_name: String,
    /// Base64 of the bytes.
    pub data: String,
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(8192)];
    !sample.contains(&0) && std::str::from_utf8(sample).is_ok_or_cut()
}

trait Utf8Cut {
    fn is_ok_or_cut(&self) -> bool;
}
impl Utf8Cut for Result<&str, std::str::Utf8Error> {
    /// Valid UTF-8, or valid up to a character cut off by the sample boundary.
    fn is_ok_or_cut(&self) -> bool {
        match self {
            Ok(_) => true,
            Err(e) => e.error_len().is_none(),
        }
    }
}

/// Extracts and stores documents; returns content parts for the message.
///
/// [0.0.5 memory] Each document's text is cut to its share of a context of `n_ctx` tokens
/// ([`document_cap_chars`], shared by the `shared_by` documents and mentions of the
/// message) with a note for the model ([`cut_document`]); what was left out is kept in the
/// part as `"cut"` ([`DocumentCut`]). [0.0.6] `counter` is the local model server, whose
/// own token counts hold each share ([`cut_to_share`]); `None` for an endpoint.
pub async fn save_documents(
    app: &AppHandle,
    state: &AppState,
    chat_id: store::ChatId,
    docs: &[DocumentInput],
    n_ctx: usize,
    shared_by: usize,
    counter: Option<&agent::OpenAiClient>,
) -> AppResult<Vec<serde_json::Value>> {
    if docs.len() > MAX_IMAGES {
        return Err(AppError::bad_request(format!(
            "Attach at most {MAX_IMAGES} files to one message."
        )));
    }
    let dir = store_dir(app)?;
    let mut parts = Vec::new();
    for doc in docs {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(doc.data.as_bytes())
            .map_err(|_| AppError::bad_request("An attached file could not be read."))?;
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(AppError::bad_request(format!(
                "{} is larger than 32 MB.",
                doc.file_name
            )));
        }
        let sha: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let is_pdf = bytes.starts_with(b"%PDF-");
        let ext = if is_pdf { "pdf" } else { "txt" };
        let path = dir.join(format!("{sha}.{ext}"));
        if !path.exists() {
            std::fs::write(&path, &bytes).map_err(|e| AppError::internal(e.to_string()))?;
        }
        let (mime, text) = if is_pdf {
            let exe = std::env::current_exe().map_err(|e| AppError::internal(e.to_string()))?;
            let text = tools::pdf::extract_text(&exe, &path, std::time::Duration::from_secs(30))
                .await
                .map_err(|e| {
                    AppError::new(
                        "pdf_unreadable",
                        format!("{} could not be read.", doc.file_name),
                        "Try another file, or copy its text into the message.",
                        e.to_string(),
                    )
                })?;
            if text.trim().is_empty() {
                return Err(AppError::new(
                    "pdf_no_text",
                    format!(
                        "{} has no text the app can extract (it may be scanned pages).",
                        doc.file_name
                    ),
                    "Attach the pages as images to a model with vision instead.",
                    String::new(),
                ));
            }
            ("application/pdf".to_string(), text)
        } else if looks_like_text(&bytes) {
            (
                "text/plain".to_string(),
                String::from_utf8_lossy(&bytes).into_owned(),
            )
        } else {
            return Err(AppError::bad_request(format!(
                "{} is neither text nor a PDF. Attach images with the image button.",
                doc.file_name
            )));
        };
        // [0.0.5 memory] Its share of the context, with a note on what was left out;
        // [0.0.6] in the local model's own tokens.
        let count = counter.map(|client| {
            move |t: String| async move {
                client
                    .tokenize(&t, false)
                    .await
                    .ok()
                    .flatten()
                    .map(|tokens| tokens.len())
            }
        });
        let (text, cut) = document_for_model(&text, n_ctx, shared_by, is_pdf, count).await;
        let name: String = doc
            .file_name
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        let id = state
            .store
            .insert_attachment(store::NewAttachment {
                chat_id,
                sha256: sha,
                mime,
                file_name: name.clone(),
                size_bytes: bytes.len() as u64,
                bytes_path: path.display().to_string(),
                extracted_text: Some(text),
            })
            .await?;
        let mut part =
            serde_json::json!({ "type": "document", "attachment_id": id.0, "name": name });
        if let Some(cut) = cut {
            part["cut"] = serde_json::json!(cut);
        }
        parts.push(part);
    }
    Ok(parts)
}

/// Stores `text` (a resolved mention) as a text document of the chat and returns its
/// attachment id. The text is copied into the app's own attachments folder `dir`: the row
/// never points at the user's file, because the paths of a deleted chat's attachments are
/// returned for deletion (`Store::delete_chat`).
pub async fn save_text_in(
    store: &store::Store,
    dir: &std::path::Path,
    chat_id: store::ChatId,
    name: &str,
    text: &str,
) -> AppResult<i64> {
    let bytes = text.as_bytes();
    let sha: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = dir.join(format!("{sha}.txt"));
    if !path.exists() {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| AppError::internal(e.to_string()))?;
        std::fs::rename(&tmp, &path).map_err(|e| AppError::internal(e.to_string()))?;
    }
    let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
    let id = store
        .insert_attachment(store::NewAttachment {
            chat_id,
            sha256: sha,
            mime: "text/plain".into(),
            file_name: name,
            size_bytes: bytes.len() as u64,
            bytes_path: path.display().to_string(),
            extracted_text: Some(text.to_string()),
        })
        .await?;
    Ok(id.0)
}

/// [`save_text_in`] in the app's attachments folder.
pub async fn save_text(
    app: &AppHandle,
    state: &AppState,
    chat_id: store::ChatId,
    name: &str,
    text: &str,
) -> AppResult<i64> {
    let dir = store_dir(app)?;
    save_text_in(&state.store, &dir, chat_id, name, text).await
}

/// The documents referenced by a message's content: (attachment id, file name). Mentions
/// are documents too, but are listed by `mentions::mention_refs`.
pub fn document_refs(content: &serde_json::Value) -> Vec<(i64, String)> {
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("document"))
                .filter(|p| p.get("mention").is_none())
                .filter_map(|p| {
                    Some((
                        p.get("attachment_id")?.as_i64()?,
                        p.get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("document")
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The attachment ids referenced by a message's content.
pub fn image_ids(content: &serde_json::Value) -> Vec<i64> {
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("image"))
                .filter_map(|p| p.get("attachment_id").and_then(|i| i.as_i64()))
                .collect()
        })
        .unwrap_or_default()
}

/// Base64 of an attachment's bytes with its MIME type, as the model gets them: [0.0.6] a
/// WebP picture stored by an earlier version is turned into PNG here ([`picture_for_model`]).
pub async fn load_base64(state: &AppState, id: i64) -> AppResult<(String, String)> {
    let (mime, bytes) = load_bytes(state, id).await?;
    let (mime, bytes) = picture_for_model_async(mime, bytes).await?;
    Ok((
        mime,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

/// An attachment's MIME type and bytes as stored.
async fn load_bytes(state: &AppState, id: i64) -> AppResult<(String, Vec<u8>)> {
    let att = state
        .store
        .get_attachment(store::AttachmentId(id))
        .await?
        .ok_or_else(|| AppError::not_found("attachment"))?;
    let bytes = tokio::fs::read(&att.bytes_path).await.map_err(|e| {
        AppError::new(
            "attachment_missing",
            "An attached image is no longer on disk.",
            "Attach it again.",
            e.to_string(),
        )
    })?;
    Ok((att.mime, bytes))
}

/// A data: URI for showing an attachment in the chat (CSP allows `img-src data:`): the bytes
/// as stored ([0.0.6] the webview shows a WebP picture from an earlier version itself).
#[tauri::command]
pub async fn get_attachment(state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let (mime, bytes) = load_bytes(&state, id).await?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_numbers() {
        assert!(sniff("image/png", b"\x89PNG\r\n\x1a\n...."));
        assert!(!sniff("image/png", b"GIF89a"));
        assert!(sniff("image/jpeg", &[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(sniff("image/webp", b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!sniff("image/svg+xml", b"<svg"));
        assert!(!sniff("text/plain", b"hello"));
    }

    #[test]
    fn text_detection() {
        assert!(looks_like_text(b"hello\nworld"));
        assert!(looks_like_text("héllo".as_bytes()));
        assert!(!looks_like_text(&[0, 1, 2]));
        assert!(!looks_like_text(&[0xff, 0xfe, 0x00, 0x41]));
    }

    #[test]
    fn ids_from_content() {
        let c = serde_json::json!([
            {"type": "text", "text": "look"},
            {"type": "image", "attachment_id": 7},
            {"type": "image", "attachment_id": 9}
        ]);
        assert_eq!(image_ids(&c), vec![7, 9]);
        assert!(image_ids(&serde_json::json!("plain")).is_empty());
    }

    /// [0.0.5 memory] A document's share: a fifth of the context, 40% shared by the files
    /// of the message, never below the floor; no 8,000-token ceiling as for one mention.
    #[test]
    fn a_document_gets_its_share_of_the_context() {
        assert_eq!(document_cap_chars(8_192, 1), 1_638 * 3);
        assert_eq!(document_cap_chars(8_192, 2), 1_638 * 3);
        assert_eq!(document_cap_chars(8_192, 4), 819 * 3);
        assert_eq!(document_cap_chars(4_096, 1), 819 * 3);
        assert_eq!(document_cap_chars(4_096, 0), 819 * 3);
        assert_eq!(document_cap_chars(200_000, 1), 40_000 * 3);
        assert_eq!(document_cap_chars(100, 8), 600);
        // Every context keeps the share under the storage guard, in bytes ([0.0.6]: 4 bytes
        // a character at most).
        assert!(document_cap_chars(1_048_576, 1) * MAX_CHAR_BYTES <= MAX_DOCUMENT_TEXT);
        assert_eq!(document_cap_chars(2_000_000, 1), MAX_DOCUMENT_TEXT / 4);
        // A share is what the turn can carry: with the reply kept free and tools at their
        // 40%, a document still fits under the point where room is made.
        for n_ctx in [4_096u32, 8_192, 32_768] {
            let budget = agent::ContextBudget::new(n_ctx, 0);
            let tools = budget.budget * agent::context::TOOLS_FIT_PERCENT / 100;
            let document = u32::try_from(document_cap_chars(n_ctx as usize, 1) / 3).unwrap();
            assert!(tools + document <= budget.trigger, "{n_ctx}");
        }
    }

    fn numbered_lines(n: usize) -> String {
        (1..=n)
            .map(|i| format!("line {i} of the report\n"))
            .collect()
    }

    /// [0.0.5 memory] A document larger than its share is cut at a line end, with a note
    /// that says which lines and how much were left out and where the file tools can go on;
    /// a document that fits is sent whole.
    #[test]
    fn a_long_document_is_cut_with_a_note() {
        let short = numbered_lines(10);
        assert_eq!(cut_document(&short, 4_000, false), (short.clone(), None));

        let text = numbered_lines(1_000);
        let total = text.chars().count();
        let (out, cut) = cut_document(&text, 2_457, false);
        let cut = cut.expect("cut");
        assert!(out.chars().count() <= 2_457, "{}", out.chars().count());
        assert_eq!(cut.total, total);
        assert!(text.starts_with(&out[..cut.kept]));
        // Whole lines only.
        assert!(out[..cut.kept].ends_with('\n'));
        let first = out[..cut.kept].matches('\n').count() + 1;
        let note = &out[cut.kept..];
        assert!(
            note.starts_with("[The rest of this file was left out"),
            "{note}"
        );
        assert!(note.contains(&format!("lines {first}-1000")), "{note}");
        assert!(
            note.contains(&format!(
                "{} of {} characters",
                agent::context::group_digits((total - cut.kept) as u64),
                agent::context::group_digits(total as u64)
            )),
            "{note}"
        );
        assert!(note.contains(&format!("offset={first}")), "{note}");
        assert!(note.trim_end().ends_with(".]"), "{note}");

        // A PDF's text has no lines to read on from.
        let (out, cut) = cut_document(&text, 2_457, true);
        assert!(cut.is_some());
        assert!(out.contains("this file's text was left out"), "{out}");
        assert!(!out.contains("offset="), "{out}");
    }

    /// [0.0.5 memory] One long line (or text with no line breaks) is cut inside the line,
    /// never inside a character, and the note still fits in the share.
    #[test]
    fn a_document_without_line_ends_is_cut_between_characters() {
        let text = "é".repeat(5_000);
        let (out, cut) = cut_document(&text, 600, false);
        let cut = cut.expect("cut");
        assert!(out.chars().count() <= 600, "{}", out.chars().count());
        assert_eq!(cut.total, 5_000);
        assert!(cut.kept >= 200, "{}", cut.kept);
        assert!(out.contains("lines 1-1"), "{out}");
    }

    /// [0.0.5 memory] What the user is told about a cut document, read back from the
    /// stored message; mentions (cut by their own rule) and whole documents say nothing.
    #[test]
    fn the_user_is_told_which_documents_were_cut() {
        let content = serde_json::json!([
            {"type": "text", "text": "read these"},
            {"type": "document", "attachment_id": 1, "name": "report.txt", "cut": {"kept": 2_400, "total": 10_000}},
            {"type": "document", "attachment_id": 2, "name": "short.txt"},
            {"type": "document", "attachment_id": 3, "name": "src/a.rs", "mention": {"path": "/w/src/a.rs"}, "cut": {"kept": 1, "total": 2}}
        ]);
        let cuts = document_cuts(&content);
        assert_eq!(
            cuts,
            vec![(
                "report.txt".to_owned(),
                DocumentCut {
                    kept: 2_400,
                    total: 10_000
                }
            )]
        );
        // Ask first and Allow all: the chat's folders.
        assert_eq!(
            cut_note(&cuts[0].0, cuts[0].1, false),
            "Only the first part of report.txt was sent (about 24%), because the whole file is \
             more than the model can remember at once. If the file is in one of this chat's \
             folders, the model can read the rest itself."
        );
        // [0.0.6] Full access: the file tools reach the whole computer (D12).
        assert_eq!(
            cut_note(&cuts[0].0, cuts[0].1, true),
            "Only the first part of report.txt was sent (about 24%), because the whole file is \
             more than the model can remember at once. Full access is on, so the model can read \
             the rest itself if it finds the file on this computer."
        );
        assert!(document_cuts(&serde_json::json!("plain")).is_empty());
        // A stored part round-trips the cut as the save path writes it.
        let part = serde_json::json!({"cut": DocumentCut { kept: 5, total: 9 }});
        assert_eq!(part["cut"], serde_json::json!({"kept": 5, "total": 9}));
    }

    /// A made-up model server's count for [`cut_to_share`]: `per_char` tokens for each
    /// character of a digit, one for every three other characters (the estimate), and the
    /// texts it was asked to count.
    fn counter(
        per_char: usize,
        asked: &std::sync::Mutex<Vec<usize>>,
    ) -> impl Fn(String) -> std::future::Ready<Option<usize>> + '_ {
        move |t: String| {
            asked.lock().unwrap().push(t.chars().count());
            let digits = t.chars().filter(char::is_ascii_digit).count();
            let other = t.chars().count() - digits;
            std::future::ready(Some(digits * per_char + other.div_ceil(3)))
        }
    }

    fn table(rows: usize) -> String {
        (1..=rows)
            .map(|i| {
                format!(
                    "{i},{},{}\n",
                    (i * 7_919) % 100_003,
                    (i * 104_729) % 1_000_003
                )
            })
            .collect()
    }

    /// [0.0.6] With a local model the share is held in the model server's own tokens: a
    /// table of numbers (a token per digit, as the small test model counts them) is cut
    /// further than the estimate alone would, to at most its share of tokens, still at a
    /// line end with the note; prose that the estimate already covers is cut as before and
    /// counted once.
    #[tokio::test]
    async fn a_documents_share_is_held_in_the_models_own_tokens() {
        let cap = document_cap_chars(4_096, 1);
        let allowed = cap / 3;
        let numbers = table(1_200);

        // An endpoint: the estimate alone (no count).
        type Count = fn(String) -> std::future::Ready<Option<usize>>;
        let by_estimate = cut_to_share(&numbers, cap, false, None::<Count>).await;
        assert_eq!(by_estimate, cut_document(&numbers, cap, false));

        // A local model whose tokenizer splits digits.
        let asked = std::sync::Mutex::new(Vec::new());
        let count = counter(1, &asked);
        let (out, cut) = cut_to_share(&numbers, cap, false, Some(&count)).await;
        let cut = cut.expect("cut");
        let tokens = count(out.clone()).await.unwrap();
        assert!(tokens <= allowed, "{tokens} > {allowed}");
        // Not cut to nothing: more than half the share is used.
        assert!(tokens * 2 > allowed, "{tokens} of {allowed}");
        assert!(cut.kept < by_estimate.1.unwrap().kept);
        assert!(out[..cut.kept].ends_with('\n'));
        assert!(out[cut.kept..].starts_with("[The rest of this file was left out"));
        assert_eq!(cut.total, numbers.chars().count());
        // Counted only a few times.
        assert!(asked.lock().unwrap().len() <= MAX_COUNTS + 1);

        // Prose the estimate covers: the same cut, counted once.
        let prose = "The river runs past the old mill and on to the sea.\n".repeat(400);
        let asked = std::sync::Mutex::new(Vec::new());
        let count = counter(1, &asked);
        assert_eq!(
            cut_to_share(&prose, cap, false, Some(&count)).await,
            cut_document(&prose, cap, false)
        );
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    /// [0.0.6] A short document that fits by characters but not in tokens is cut too; a
    /// server that cannot count leaves the estimate's cut; nothing goes below the floor,
    /// even for a tokenizer far denser than any real one.
    #[tokio::test]
    async fn the_models_count_cuts_short_dense_text_and_never_below_the_floor() {
        let cap = document_cap_chars(8_192, 1);
        let dense = "1234567890\n".repeat(cap / 11);
        assert_eq!(
            cut_document(&dense, cap, false).1,
            None,
            "fits by characters"
        );
        let asked = std::sync::Mutex::new(Vec::new());
        let count = counter(1, &asked);
        let (out, cut) = cut_to_share(&dense, cap, false, Some(&count)).await;
        assert!(cut.is_some());
        assert!(count(out).await.unwrap() <= cap / 3);

        // The server could not count: the estimate's cut.
        let failed = |_: String| std::future::ready(None::<usize>);
        let numbers = table(1_200);
        assert_eq!(
            cut_to_share(&numbers, cap, false, Some(failed)).await,
            cut_document(&numbers, cap, false)
        );

        // Fifty tokens a digit: the floor, and a bounded number of counts.
        let asked = std::sync::Mutex::new(Vec::new());
        let count = counter(50, &asked);
        let (out, cut) = cut_to_share(&numbers, cap, false, Some(&count)).await;
        assert!(cut.is_some());
        assert!(
            out.chars().count() <= MIN_DOCUMENT_CHARS,
            "{}",
            out.chars().count()
        );
        assert!(out.chars().count() > 0);
        assert!(asked.lock().unwrap().len() <= MAX_COUNTS);
    }

    /// `n` characters of `c`, a line break after every 99.
    fn lines_of(c: char, n: usize) -> String {
        (0..n)
            .map(|i| if i % 100 == 99 { '\n' } else { c })
            .collect()
    }

    /// [0.0.6] The share is held in bytes too: an endpoint may report a context of a
    /// million tokens or more, and text of 3 or 4 bytes a character (Chinese, Japanese,
    /// emoji) would otherwise pass the 2 MiB storage guard, which then cut off the model's
    /// note, made the user's "about N%" wrong, or dropped text with no note at all.
    #[tokio::test]
    async fn a_documents_share_fits_the_storage_guard_in_bytes() {
        type Count = fn(String) -> std::future::Ready<Option<usize>>;
        for n_ctx in [1_048_576usize, 2_000_000, u32::MAX as usize] {
            assert!(
                document_cap_chars(n_ctx, 1) * 4 <= MAX_DOCUMENT_TEXT,
                "{n_ctx}"
            );
        }
        // Longer than the share and than the guard: cut, with the note, under the guard,
        // and what is stored as kept is what was sent.
        for (c, chars) in [('漢', 1_200_000), ('𝄞', 600_000), ('漢', 984_000)] {
            let text = lines_of(c, chars);
            assert!(text.len() > MAX_DOCUMENT_TEXT);
            let (out, cut) = document_for_model(&text, 2_000_000, 1, false, None::<Count>).await;
            let cut = cut.unwrap_or_else(|| panic!("{c} {chars}: not cut"));
            assert!(out.len() <= MAX_DOCUMENT_TEXT, "{c} {chars}: {}", out.len());
            assert_eq!(cut.total, chars);
            let kept_end = out
                .char_indices()
                .nth(cut.kept)
                .map_or(out.len(), |(i, _)| i);
            assert!(text.starts_with(&out[..kept_end]), "{c} {chars}");
            let note = &out[kept_end..];
            assert!(
                note.starts_with("[The rest of this file was left out"),
                "{c} {chars}: {}",
                &note[..note.len().min(80)]
            );
            assert!(note.ends_with(".]"), "{c} {chars}");
        }
        // Short text is sent whole, as before.
        let short = lines_of('漢', 5_000);
        assert_eq!(
            document_for_model(&short, 2_000_000, 1, false, None::<Count>).await,
            (short.clone(), None)
        );
    }

    /// [0.0.6] A file name holding characters that change how text is shown (a right-to-left
    /// override, zero-width or tag characters) is shown with them as visible escapes in the
    /// user's note, as approval cards show them, so the rest of the note reads as written.
    #[test]
    fn the_cut_note_shows_hidden_characters_in_the_file_name() {
        let cut = DocumentCut { kept: 1, total: 4 };
        for full_access in [false, true] {
            let note = cut_note("evil\u{202E}txt.exe\u{200B}\u{E0041}", cut, full_access);
            assert!(
                note.starts_with(
                    "Only the first part of evil\\u{202E}txt.exe\\u{200B}\\u{E0041} was sent"
                ),
                "{note}"
            );
            assert!(
                !note.contains(['\u{202E}', '\u{200B}', '\u{E0041}']),
                "{note}"
            );
        }
    }

    /// A 4 x 3 lossy WebP picture (made with Pillow): left half red, right half blue.
    const LOSSY_WEBP: &[u8] = &[
        0x52, 0x49, 0x46, 0x46, 0x5c, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38,
        0x20, 0x50, 0x00, 0x00, 0x00, 0x70, 0x03, 0x00, 0x9d, 0x01, 0x2a, 0x04, 0x00, 0x03, 0x00,
        0x00, 0xc0, 0x12, 0x25, 0xa8, 0x02, 0x74, 0xba, 0x01, 0x40, 0x03, 0xf0, 0x02, 0xb4, 0x03,
        0xf8, 0x06, 0x50, 0x07, 0xe8, 0x00, 0x2d, 0xda, 0xcb, 0xc2, 0xc0, 0x00, 0xfe, 0xfe, 0x7a,
        0x36, 0x21, 0xbf, 0xaa, 0x18, 0xff, 0xfd, 0x1f, 0x89, 0x32, 0x46, 0xb5, 0xff, 0xf8, 0x33,
        0x1f, 0xfd, 0x0d, 0x46, 0x1a, 0xb0, 0x5c, 0xff, 0xf0, 0x66, 0x3f, 0xb4, 0x89, 0x9b, 0x6b,
        0xdf, 0xbb, 0x70, 0x64, 0xb6, 0x15, 0x01, 0xe7, 0x40, 0x00,
    ];
    /// A 3 x 2 lossless WebP picture with see-through parts (made with Pillow): green, the
    /// top left pixel clear and the bottom right one (10, 20, 30) half see-through.
    const LOSSLESS_WEBP: &[u8] = &[
        0x52, 0x49, 0x46, 0x46, 0x36, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38,
        0x4c, 0x2a, 0x00, 0x00, 0x00, 0x2f, 0x02, 0x40, 0x00, 0x10, 0x17, 0x20, 0x90, 0x4d, 0x0e,
        0x11, 0x76, 0xfe, 0x13, 0x12, 0x24, 0x64, 0xff, 0x6f, 0x35, 0x0b, 0x04, 0x92, 0x48, 0xf6,
        0xe7, 0x9a, 0x44, 0x40, 0x50, 0xf4, 0x9c, 0xe9, 0x01, 0xd5, 0x7f, 0xb8, 0x06, 0x4a, 0x44,
        0xff, 0x23,
    ];
    /// A 4 x 4 animated WebP picture (made with Pillow): a yellow frame, then a blue one.
    const ANIMATED_WEBP: &[u8] = &[
        0x52, 0x49, 0x46, 0x46, 0x84, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50, 0x38,
        0x58, 0x0a, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x41, 0x4e, 0x49, 0x4d, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x41,
        0x4e, 0x4d, 0x46, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00,
        0x00, 0x03, 0x00, 0x00, 0x64, 0x00, 0x00, 0x02, 0x56, 0x50, 0x38, 0x4c, 0x10, 0x00, 0x00,
        0x00, 0x2f, 0x03, 0xc0, 0x00, 0x00, 0x07, 0x50, 0xe4, 0xea, 0x47, 0xff, 0x03, 0x11, 0xd1,
        0xff, 0x00, 0x41, 0x4e, 0x4d, 0x46, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x00, 0x64, 0x00, 0x00, 0x00, 0x56, 0x50, 0x38, 0x4c,
        0x0f, 0x00, 0x00, 0x00, 0x2f, 0x03, 0xc0, 0x00, 0x00, 0x07, 0x10, 0x51, 0xfd, 0xfe, 0x07,
        0x22, 0xa2, 0xff, 0x01, 0x00,
    ];

    fn picture(mime: &str, bytes: &[u8]) -> ImageInput {
        ImageInput {
            mime: mime.to_owned(),
            file_name: "picture".to_owned(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    fn png_pixels(bytes: &[u8]) -> image::RgbaImage {
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .expect("a valid PNG")
            .to_rgba8()
    }

    /// A WebP file holding `chunks` (name and body), with the sizes filled in and odd
    /// bodies padded, as the format has it.
    fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        for (name, data) in chunks {
            body.extend_from_slice(*name);
            body.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
        out.extend(body);
        out
    }

    /// The start of a lossy (VP8) key frame claiming `w` x `h` pixels, and no picture data.
    fn vp8_claiming(w: u16, h: u16) -> Vec<u8> {
        let mut d = vec![0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a];
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.extend_from_slice(&[0; 32]);
        d
    }

    /// The start of a lossless (VP8L) picture with see-through parts claiming `w` x `h`
    /// pixels, and no picture data.
    fn vp8l_claiming(w: u32, h: u32) -> Vec<u8> {
        let header = (w - 1) | ((h - 1) << 14) | (1 << 28);
        let mut d = vec![0x2f];
        d.extend_from_slice(&header.to_le_bytes());
        d.extend_from_slice(&[0; 32]);
        d
    }

    /// An extended-format header (VP8X): `flags` (2 animated, 16 see-through) and a canvas
    /// of `w` x `h` pixels.
    fn vp8x(flags: u8, w: u32, h: u32) -> Vec<u8> {
        let mut d = vec![flags, 0, 0, 0];
        d.extend_from_slice(&(w - 1).to_le_bytes()[..3]);
        d.extend_from_slice(&(h - 1).to_le_bytes()[..3]);
        d
    }

    /// An animation frame (ANMF) of 1 x 1 pixels at the top left holding `name` / `data`.
    fn frame_holding(name: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut d = vec![0; 12];
        d.extend_from_slice(&[100, 0, 0, 0]);
        d.extend_from_slice(name);
        d.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
        d.extend_from_slice(data);
        d
    }

    /// A valid lossless WebP picture of one colour, `w` x `h` pixels: tiny however large.
    fn plain_webp(w: u32, h: u32) -> Vec<u8> {
        let pixels = vec![90u8; (w * h * 3) as usize];
        let mut out = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut out)
            .encode(&pixels, w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    /// [0.0.6] llama-server cannot read WebP (it would need ffmpeg, which the app never lets
    /// it start), so a WebP picture is stored and sent as PNG: the same size and colours,
    /// see-through parts kept, and the first frame of an animation.
    #[tokio::test]
    async fn a_webp_picture_is_stored_and_sent_as_png() {
        let (mime, ext, bytes) = checked_picture(&picture("image/webp", LOSSY_WEBP))
            .await
            .unwrap();
        assert_eq!((mime.as_str(), ext), ("image/png", "png"));
        let px = png_pixels(&bytes);
        assert_eq!(px.dimensions(), (4, 3));
        let [r, _, b, a] = px.get_pixel(0, 0).0;
        assert!(r > 180 && b < 80 && a == 255, "{:?}", px.get_pixel(0, 0));
        let [r, _, b, _] = px.get_pixel(3, 0).0;
        assert!(b > 180 && r < 80, "{:?}", px.get_pixel(3, 0));

        let (mime, _, bytes) = checked_picture(&picture("image/webp", LOSSLESS_WEBP))
            .await
            .unwrap();
        assert_eq!(mime, "image/png");
        let px = png_pixels(&bytes);
        assert_eq!(px.dimensions(), (3, 2));
        assert_eq!(px.get_pixel(1, 0).0, [0, 160, 0, 255]);
        assert_eq!(px.get_pixel(2, 1).0, [10, 20, 30, 128]);
        assert_eq!(px.get_pixel(0, 0).0[3], 0);

        let (mime, _, bytes) = checked_picture(&picture("image/webp", ANIMATED_WEBP))
            .await
            .unwrap();
        assert_eq!(mime, "image/png");
        let px = png_pixels(&bytes);
        assert_eq!(px.dimensions(), (4, 4));
        assert_eq!(px.get_pixel(2, 2).0, [250, 200, 0, 255]);
    }

    /// [0.0.6] PNG, JPEG and GIF pictures are stored and sent byte for byte as they came.
    #[tokio::test]
    async fn png_jpeg_and_gif_pictures_are_kept_as_they_are() {
        for (mime, ext, bytes) in [
            ("image/png", "png", &b"\x89PNG\r\n\x1a\nnot decoded"[..]),
            ("image/jpeg", "jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3][..]),
            ("image/gif", "gif", &b"GIF89a not decoded"[..]),
        ] {
            let got = checked_picture(&picture(mime, bytes)).await.unwrap();
            assert_eq!(got, (mime.to_owned(), ext, bytes.to_vec()), "{mime}");
        }
    }

    /// [0.0.6] A WebP picture that is broken, or larger than the decoder may take, gives the
    /// user a plain message, and nothing panics: cut short, not a picture inside, wider than
    /// 8,192 pixels, more pixels than the memory limit, or a frame inside claiming more than
    /// its canvas.
    #[tokio::test]
    async fn a_broken_or_oversized_webp_gives_the_plain_message() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("cut short", LOSSY_WEBP[..40].to_vec()),
            (
                "bad frame",
                riff(&[(b"VP8 ", vec![0x10, 0x02, 0x00, 1, 2, 3, 4])]),
            ),
            ("no picture", riff(&[(b"JUNK", vec![0; 16])])),
            ("too wide", plain_webp(10_000, 8)),
            (
                "too many pixels",
                riff(&[(b"VP8L", vp8l_claiming(8_000, 8_000))]),
            ),
            (
                "largest lossy",
                riff(&[(b"VP8 ", vp8_claiming(16_383, 16_383))]),
            ),
            (
                "frame larger than its canvas",
                riff(&[
                    (b"VP8X", vp8x(0, 1, 1)),
                    (b"VP8 ", vp8_claiming(16_383, 16_383)),
                ]),
            ),
            (
                "animation frame larger than its canvas",
                riff(&[
                    (b"VP8X", vp8x(2, 2, 2)),
                    (b"ANIM", vec![0; 6]),
                    (
                        b"ANMF",
                        frame_holding(b"VP8 ", &vp8_claiming(16_383, 16_383)),
                    ),
                ]),
            ),
        ];
        for (what, bytes) in cases {
            let err = checked_picture(&picture("image/webp", &bytes))
                .await
                .expect_err(what);
            assert_eq!(err.code, "picture_unreadable", "{what}");
            assert_eq!(err.message, "This picture could not be read.", "{what}");
            assert_eq!(err.next_step, "Try a PNG or JPEG.", "{what}");
        }
    }

    /// [0.0.6] The limits refuse an oversized WebP picture before it is decoded (a limit
    /// error, not a decoding error after the buffers were made), including a frame that
    /// claims more than its small canvas; a picture at the limit is still turned into PNG.
    #[test]
    fn the_webp_limits_refuse_before_decoding() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("too wide", plain_webp(10_000, 8)),
            ("too tall", plain_webp(8, 10_000)),
            (
                "too many pixels",
                riff(&[(b"VP8L", vp8l_claiming(8_000, 8_000))]),
            ),
            (
                "largest lossy",
                riff(&[(b"VP8 ", vp8_claiming(16_383, 16_383))]),
            ),
            (
                "frame larger than its canvas",
                riff(&[
                    (b"VP8X", vp8x(0, 1, 1)),
                    (b"VP8 ", vp8_claiming(16_383, 16_383)),
                ]),
            ),
            (
                "animation frame larger than its canvas",
                riff(&[
                    (b"VP8X", vp8x(2, 2, 2)),
                    (b"ANIM", vec![0; 6]),
                    (
                        b"ANMF",
                        frame_holding(b"VP8L", &vp8l_claiming(16_384, 16_384)),
                    ),
                ]),
            ),
            (
                "canvas too large",
                riff(&[
                    (b"VP8X", vp8x(16, 9_000, 9_000)),
                    (b"VP8L", vp8l_claiming(1, 1)),
                ]),
            ),
        ];
        for (what, bytes) in cases {
            match webp_to_png(&bytes) {
                Err(image::ImageError::Limits(_)) => {}
                other => panic!("{what}: {other:?}"),
            }
        }
        let png = webp_to_png(&plain_webp(MAX_PICTURE_SIDE, 2)).unwrap();
        assert_eq!(png_pixels(&png).dimensions(), (MAX_PICTURE_SIDE, 2));
    }

    /// [0.0.6] No WebP file, however damaged, makes the decoder panic: every cut and many
    /// changed bytes of the three sample pictures give a PNG or an error.
    #[test]
    fn damaged_webp_files_never_panic() {
        let mut tried = 0;
        for sample in [LOSSY_WEBP, LOSSLESS_WEBP, ANIMATED_WEBP] {
            for end in 0..sample.len() {
                if let Ok(png) = webp_to_png(&sample[..end]) {
                    png_pixels(&png);
                }
                tried += 1;
            }
            for at in 0..sample.len() {
                for change in [0x00, 0xFF, sample[at] ^ 0x55, sample[at].wrapping_add(1)] {
                    let mut bytes = sample.to_vec();
                    bytes[at] = change;
                    if let Ok(png) = webp_to_png(&bytes) {
                        png_pixels(&png);
                    }
                    tried += 1;
                }
            }
        }
        assert!(tried > 1_000, "{tried}");
    }
}
