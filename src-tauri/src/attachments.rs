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
/// Text kept from one document before the prompt builder trims it to the budget.
const MAX_DOCUMENT_TEXT: usize = 2 * 1024 * 1024;

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
pub async fn save_documents(
    app: &AppHandle,
    state: &AppState,
    chat_id: store::ChatId,
    docs: &[DocumentInput],
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
        let mut text = text;
        if text.len() > MAX_DOCUMENT_TEXT {
            let mut cut = MAX_DOCUMENT_TEXT;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
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
        parts.push(serde_json::json!({ "type": "document", "attachment_id": id.0, "name": name }));
    }
    Ok(parts)
}

/// The documents referenced by a message's content: (attachment id, file name).
pub fn document_refs(content: &serde_json::Value) -> Vec<(i64, String)> {
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("document"))
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
}
