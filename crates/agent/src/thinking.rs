//! How long a thinking model may think in one request (0.0.6 D4, "How long it may think").
//!
//! A per-request limit applied by the agent loop (`limit_thinking`), not a launch flag: the
//! request's thinking budget is min(the choice's tokens, `max_tokens` / 2), never below
//! [`MIN_THINKING_TOKENS`]. [`ThinkingLimit::NoLimit`] keeps the loop's own rule, half the
//! output limit, so a reasoning model can never spend the whole reply allowance on thought.
//! The app fills [`crate::TurnInput::thinking_limit`] from the model's load settings.

use serde::{Deserialize, Serialize};

/// The thinking budget of [`ThinkingLimit::Short`], in tokens (about 750 words).
pub const SHORT_THINKING_TOKENS: u32 = 1_024;
/// The thinking budget of [`ThinkingLimit::Medium`], in tokens (about 3,000 words).
pub const MEDIUM_THINKING_TOKENS: u32 = 4_096;
/// The thinking budget of [`ThinkingLimit::Long`], in tokens (about 12,000 words).
pub const LONG_THINKING_TOKENS: u32 = 16_384;
/// No request gets a smaller thinking budget than this, whatever the choice and the output
/// limit: below it a model can hardly start to think.
pub const MIN_THINKING_TOKENS: u32 = 128;

/// The choice for "How long it may think". Serialised camelCase: `"noLimit"`, `"short"`,
/// `"medium"`, `"long"`. The webview shows `NoLimit` as "Default".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingLimit {
    /// No limit from the user: the loop's own rule, half the output limit (shown as
    /// "Default").
    #[default]
    NoLimit,
    /// At most [`SHORT_THINKING_TOKENS`].
    Short,
    /// At most [`MEDIUM_THINKING_TOKENS`].
    Medium,
    /// At most [`LONG_THINKING_TOKENS`].
    Long,
}

impl ThinkingLimit {
    /// The most thinking tokens this choice allows in one request; `None` for
    /// [`ThinkingLimit::NoLimit`].
    pub const fn tokens(self) -> Option<u32> {
        match self {
            Self::NoLimit => None,
            Self::Short => Some(SHORT_THINKING_TOKENS),
            Self::Medium => Some(MEDIUM_THINKING_TOKENS),
            Self::Long => Some(LONG_THINKING_TOKENS),
        }
    }

    /// The thinking budget for a request whose output limit is `max_tokens`: min(the
    /// choice's tokens, half the output limit), at least [`MIN_THINKING_TOKENS`].
    pub fn budget(self, max_tokens: u32) -> u32 {
        let half = max_tokens / 2;
        self.tokens()
            .map_or(half, |tokens| tokens.min(half))
            .max(MIN_THINKING_TOKENS)
    }
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

    #[test]
    fn each_choice_has_its_number_and_longer_means_more() {
        assert_eq!(ThinkingLimit::NoLimit.tokens(), None);
        assert_eq!(ThinkingLimit::Short.tokens(), Some(1_024));
        assert_eq!(ThinkingLimit::Medium.tokens(), Some(4_096));
        assert_eq!(ThinkingLimit::Long.tokens(), Some(16_384));
        const { assert!(SHORT_THINKING_TOKENS < MEDIUM_THINKING_TOKENS) };
        const { assert!(MEDIUM_THINKING_TOKENS < LONG_THINKING_TOKENS) };
    }

    #[test]
    fn the_budget_is_the_choice_or_half_the_output_limit_whichever_is_less() {
        // A large output limit: the choice decides; without one, half the limit.
        assert_eq!(ThinkingLimit::NoLimit.budget(40_000), 20_000);
        assert_eq!(ThinkingLimit::Short.budget(40_000), 1_024);
        assert_eq!(ThinkingLimit::Medium.budget(40_000), 4_096);
        assert_eq!(ThinkingLimit::Long.budget(40_000), 16_384);
        // A small one: half the limit caps every choice.
        assert_eq!(ThinkingLimit::Short.budget(1_500), 750);
        assert_eq!(ThinkingLimit::Medium.budget(3_000), 1_500);
        assert_eq!(ThinkingLimit::Long.budget(3_000), 1_500);
        assert_eq!(ThinkingLimit::NoLimit.budget(3_000), 1_500);
        // Never below the floor.
        for limit in [
            ThinkingLimit::NoLimit,
            ThinkingLimit::Short,
            ThinkingLimit::Medium,
            ThinkingLimit::Long,
        ] {
            assert_eq!(limit.budget(0), MIN_THINKING_TOKENS);
            assert_eq!(limit.budget(200), MIN_THINKING_TOKENS);
        }
    }
}
