//! How long a thinking model may think in one request (0.0.5 D4, "How long it may think").
//!
//! A per-request limit applied by the agent loop (`limit_thinking`, WP3a), not a launch
//! flag. The token number behind each choice is WP3a's; the request sends
//! min(that number, max_tokens / 2).

use serde::{Deserialize, Serialize};

/// The choice for "How long it may think". Serialised camelCase: `"noLimit"`, `"short"`,
/// `"medium"`, `"long"`. The webview shows `NoLimit` as "Default".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingLimit {
    /// No limit from the app: the model and server decide (shown as "Default").
    #[default]
    NoLimit,
    /// A short limit.
    Short,
    /// A medium limit.
    Medium,
    /// A long limit.
    Long,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialised_names() {
        for (limit, name) in [
            (ThinkingLimit::NoLimit, "\"noLimit\""),
            (ThinkingLimit::Short, "\"short\""),
            (ThinkingLimit::Medium, "\"medium\""),
            (ThinkingLimit::Long, "\"long\""),
        ] {
            assert_eq!(serde_json::to_string(&limit).unwrap(), name);
            assert_eq!(serde_json::from_str::<ThinkingLimit>(name).unwrap(), limit);
        }
        assert_eq!(ThinkingLimit::default(), ThinkingLimit::NoLimit);
    }
}
