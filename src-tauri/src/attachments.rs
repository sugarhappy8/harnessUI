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
        let sha: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = dir.join(format!("{sha}.{}", extension(&img.mime)));
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
                mime: img.mime.clone(),
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

/// Base64 of an attachment's bytes with its MIME type.
pub async fn load_base64(state: &AppState, id: i64) -> AppResult<(String, String)> {
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
    Ok((
        att.mime,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ))
}

/// A data: URI for showing an attachment in the chat (CSP allows `img-src data:`).
#[tauri::command]
pub async fn get_attachment(state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let (mime, b64) = load_base64(&state, id).await?;
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
}
