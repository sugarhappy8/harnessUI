//! Turning MCP call results into [`ToolOutput`] (ARCHITECTURE.md 8.4 "Raw limits", 8.5
//! "Non-text results", 9.1 "Results").
//!
//! - Text blocks become text; images become [`Content::Image`]; audio, binary resources and
//!   resource links become one-line text stubs; embedded text resources become text.
//! - A tool error (`isError: true`) stays a result the model can read.
//! - At most [`MAX_RESULT_BYTES`] of content is accepted from one call. Anything beyond is
//!   discarded, including from the stored copy, and the result says so.

use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use serde_json::json;

use crate::builtin::{Content, ToolOutput};

/// The raw limit on one external tool result (8.4).
pub const MAX_RESULT_BYTES: usize = 1024 * 1024;

/// Structured content larger than this is not attached to the tool card.
const MAX_UI_BYTES: usize = 64 * 1024;

/// Converts an MCP tool result. See the module documentation.
/// Whether `data` decodes as standard base64 (whitespace and padding allowed).
fn is_base64(data: &str) -> bool {
    use base64::Engine;
    let compact: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    !compact.is_empty()
        && base64::engine::general_purpose::STANDARD
            .decode(compact.as_bytes())
            .is_ok()
}

pub(crate) fn map_call_result(result: CallToolResult) -> ToolOutput {
    let mut budget = Budget::new(MAX_RESULT_BYTES);
    let mut content: Vec<Content> = Vec::new();
    let has_blocks = !result.content.is_empty();
    for block in result.content {
        match block {
            ContentBlock::Text(text) => {
                if let Some(text) = budget.take_text(&text.text) {
                    content.push(Content::Text { text });
                }
            }
            ContentBlock::Image(image) => {
                let mime = image.mime_type.trim().to_ascii_lowercase();
                if !mime.starts_with("image/") || mime.len() > 100 {
                    push_stub(
                        &mut content,
                        &mut budget,
                        format!(
                            "[an image block with an invalid type ({}) was left out]",
                            policy::sanitize::one_line(&image.mime_type, 40)
                        ),
                    );
                } else if !is_base64(&image.data) {
                    // A malformed image would make the next model request fail.
                    push_stub(
                        &mut content,
                        &mut budget,
                        format!("[an image ({mime}) with invalid base64 data was left out]"),
                    );
                } else if budget.fits(image.data.len()) {
                    budget.spend(image.data.len());
                    content.push(Content::Image {
                        mime,
                        data_base64: image.data,
                    });
                } else {
                    budget.truncated = true;
                    push_stub(
                        &mut content,
                        &mut budget,
                        format!("[image ({mime}) left out: it exceeds the 1 MiB result limit]"),
                    );
                }
            }
            ContentBlock::Audio(audio) => push_stub(
                &mut content,
                &mut budget,
                format!(
                    "[audio ({}), about {} KB: audio results are not supported]",
                    policy::sanitize::one_line(&audio.mime_type, 40),
                    audio.data.len() * 3 / 4 / 1024
                ),
            ),
            ContentBlock::Resource(embedded) => match embedded.resource {
                ResourceContents::TextResourceContents { uri, text, .. } => {
                    let header = format!("[resource {}]\n", policy::sanitize::one_line(&uri, 200));
                    if let Some(text) = budget.take_text(&format!("{header}{text}")) {
                        content.push(Content::Text { text });
                    }
                }
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => push_stub(
                    &mut content,
                    &mut budget,
                    format!(
                        "[binary resource {} ({}), {} bytes: not shown]",
                        policy::sanitize::one_line(&uri, 200),
                        policy::sanitize::one_line(
                            mime_type.as_deref().unwrap_or("unknown type"),
                            40
                        ),
                        blob.len() * 3 / 4
                    ),
                ),
                _ => push_stub(
                    &mut content,
                    &mut budget,
                    "[a resource of an unknown kind was left out]".into(),
                ),
            },
            ContentBlock::ResourceLink(link) => push_stub(
                &mut content,
                &mut budget,
                format!(
                    "[resource link: {} ({})]",
                    policy::sanitize::one_line(&link.uri, 200),
                    policy::sanitize::one_line(&link.name, 80)
                ),
            ),
            _ => push_stub(
                &mut content,
                &mut budget,
                "[a content block of an unknown kind was left out]".into(),
            ),
        }
    }
    let mut ui = None;
    if let Some(structured) = result.structured_content {
        let text = serde_json::to_string_pretty(&structured).unwrap_or_default();
        if !has_blocks && let Some(text) = budget.take_text(&text) {
            content.push(Content::Text { text });
        }
        if text.len() <= MAX_UI_BYTES {
            ui = Some(json!({ "structuredContent": structured }));
        }
    }
    if budget.truncated {
        content.push(Content::Text {
            text: format!(
                "[result cut: the server returned more than {} KiB; the rest was discarded]",
                MAX_RESULT_BYTES / 1024
            ),
        });
    }
    if content.is_empty() {
        content.push(Content::Text {
            text: "(the tool returned no content)".into(),
        });
    }
    ToolOutput {
        content,
        is_error: result.is_error.unwrap_or(false),
        full_text: None,
        ui,
    }
}

fn push_stub(content: &mut Vec<Content>, budget: &mut Budget, text: String) {
    // Stubs are tiny; they are always kept but still count against the budget.
    budget.spend(text.len());
    content.push(Content::Text { text });
}

struct Budget {
    left: usize,
    truncated: bool,
}

impl Budget {
    fn new(limit: usize) -> Self {
        Self {
            left: limit,
            truncated: false,
        }
    }

    fn fits(&self, bytes: usize) -> bool {
        bytes <= self.left
    }

    fn spend(&mut self, bytes: usize) {
        self.left = self.left.saturating_sub(bytes);
    }

    /// The part of `text` that fits, cut on a character boundary; `None` when nothing fits.
    fn take_text(&mut self, text: &str) -> Option<String> {
        if text.len() <= self.left {
            self.spend(text.len());
            return Some(text.to_owned());
        }
        self.truncated = true;
        if self.left == 0 {
            return None;
        }
        let mut cut = self.left;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        self.left = 0;
        (cut > 0).then(|| text[..cut].to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ContentBlock, ResourceContents};

    #[test]
    fn maps_text_and_images() {
        let result = CallToolResult::success(vec![
            ContentBlock::text("hello"),
            ContentBlock::image("aGk=", "image/PNG"),
        ]);
        let out = map_call_result(result);
        assert!(!out.is_error);
        assert_eq!(
            out.content,
            vec![
                Content::Text {
                    text: "hello".into()
                },
                Content::Image {
                    mime: "image/png".into(),
                    data_base64: "aGk=".into()
                }
            ]
        );
    }

    #[test]
    fn tool_errors_stay_results() {
        let out = map_call_result(CallToolResult::error(vec![ContentBlock::text(
            "no such file",
        )]));
        assert!(out.is_error);
        assert_eq!(out.joined_text(), "no such file");
    }

    #[test]
    fn audio_and_resources_become_text() {
        let result = CallToolResult::success(vec![
            ContentBlock::audio("AAAA", "audio/wav"),
            ContentBlock::embedded_text("file:///a.txt", "inside"),
            ContentBlock::resource(ResourceContents::BlobResourceContents {
                uri: "file:///b.bin".into(),
                mime_type: Some("application/octet-stream".into()),
                blob: "AAAAAAAA".into(),
                meta: None,
            }),
            ContentBlock::image("x", "text/html"),
            ContentBlock::image("not base64!", "image/png"),
        ]);
        let text = map_call_result(result).joined_text();
        assert!(
            text.contains("[audio (audio/wav), about 0 KB: audio results are not supported]"),
            "{text}"
        );
        assert!(text.contains("[resource file:///a.txt]\ninside"), "{text}");
        assert!(
            text.contains(
                "[binary resource file:///b.bin (application/octet-stream), 6 bytes: not shown]"
            ),
            "{text}"
        );
        assert!(text.contains("invalid type (text/html)"), "{text}");
        assert!(text.contains("invalid base64 data was left out"), "{text}");
    }

    #[test]
    fn caps_results_at_one_mebibyte() {
        let big = "é".repeat(MAX_RESULT_BYTES); // 2 MiB of two-byte characters
        let out = map_call_result(CallToolResult::success(vec![
            ContentBlock::text(big),
            ContentBlock::text("after"),
            ContentBlock::image("A".repeat(1000), "image/png"),
        ]));
        let Content::Text { text } = &out.content[0] else {
            panic!("text first")
        };
        assert_eq!(text.len(), MAX_RESULT_BYTES - (MAX_RESULT_BYTES % 2));
        assert!(text.chars().all(|c| c == 'é'));
        let joined = out.joined_text();
        assert!(!joined.contains("after"));
        assert!(joined.contains("image (image/png) left out"));
        assert!(joined.ends_with(
            "[result cut: the server returned more than 1024 KiB; the rest was discarded]"
        ));
        assert!(
            out.content
                .iter()
                .all(|c| !matches!(c, Content::Image { .. }))
        );
    }

    #[test]
    fn structured_content_is_used_when_there_are_no_blocks() {
        let mut result = CallToolResult::success(vec![]);
        result.structured_content = Some(json!({"a": 1}));
        let out = map_call_result(result);
        assert_eq!(out.joined_text(), "{\n  \"a\": 1\n}");
        assert_eq!(out.ui, Some(json!({"structuredContent": {"a": 1}})));
        let empty = map_call_result(CallToolResult::success(vec![]));
        assert_eq!(empty.joined_text(), "(the tool returned no content)");
    }
}
