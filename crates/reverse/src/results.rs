//! What comes back from rea (item5-spec.md R13, WP9g `results.rs`): the reverse folder
//! masked as `[harnessUI's rea folder]` in every spelling, `policy::sanitize` on every
//! string that reaches the model, the card, storage or the tab, a hint when rea names one
//! of its settings, and one plain line for each known failure.
//!
//! TODO(0.0.6): WP9g builds the full masking (JSON-escaped, both slash styles, `\\?\`,
//! case-insensitive on Windows and macOS, 8.3 short names), the hint and the mapped
//! failures. This is WP9a's contract stub: it masks the root as written only.

use std::path::Path;

use mcp_host::{Content, ToolOutput};

use crate::platform::Platform;

/// What the reverse folder reads as in anything rea says.
pub const MASK: &str = "[harnessUI's rea folder]";

/// `text` with the reverse folder masked.
///
/// TODO(0.0.6): WP9g. The stub replaces the root as written.
pub fn mask(text: &str, root: &Path) -> String {
    let root = root.display().to_string();
    if root.is_empty() {
        return text.to_string();
    }
    text.replace(&root, MASK)
}

/// A result from rea, cleaned for the model, the card and storage.
///
/// TODO(0.0.6): WP9g. The stub masks text blocks and `full_text` only.
pub fn clean(output: ToolOutput, root: &Path, platform: Platform) -> ToolOutput {
    let _ = platform;
    ToolOutput {
        content: output
            .content
            .into_iter()
            .map(|c| match c {
                Content::Text { text } => Content::Text {
                    text: mask(&text, root),
                },
                other => other,
            })
            .collect(),
        full_text: output.full_text.map(|t| mask(&t, root)),
        ..output
    }
}
