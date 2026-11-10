//! Token estimates for endpoints that cannot count (ARCHITECTURE.md 8.4, "Remote endpoints").
//!
//! The baseline is 3 characters per token, the conservative figure Cline uses: code, JSON and
//! non-English text tokenise denser than prose. [`TokenCalibrator`] then learns a per-model
//! ratio from the prompt sizes the endpoint reports in `usage.prompt_tokens`.

/// The baseline characters-per-token ratio.
pub const BASELINE_CHARS_PER_TOKEN: f64 = 3.0;

/// Estimates the tokens in `text` as `ceil(chars / 3)`, counting Unicode scalar values.
pub fn estimate_tokens(text: &str) -> u32 {
    tokens_for_chars(text.chars().count(), BASELINE_CHARS_PER_TOKEN)
}

fn tokens_for_chars(chars: usize, chars_per_token: f64) -> u32 {
    if chars == 0 {
        return 0;
    }
    // Precision loss above 2^52 characters is irrelevant here.
    #[allow(clippy::cast_precision_loss)]
    let tokens = (chars as f64 / chars_per_token).ceil();
    if tokens >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        // In range and non-negative by the check above.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tokens = tokens as u32;
        tokens
    }
}

/// Learns a characters-per-token ratio for one model from `(estimated, actual)` pairs.
///
/// `estimated` is what [`estimate_tokens`] returned for the prompt that was sent (so it stands
/// for `estimated × 3` characters) and `actual` is the endpoint's `usage.prompt_tokens`. Each
/// sample moves the ratio a fixed fraction of the way towards the sample's ratio, and the result
/// is clamped to `[1.5, 6.0]`. Samples are ignored when flagged truncated (a reply cut off, or
/// an Ollama prompt it silently shortened), when either number is zero, or when the prompt is
/// so small that template overhead dominates.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenCalibrator {
    chars_per_token: f64,
    samples: u32,
}

impl Default for TokenCalibrator {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenCalibrator {
    /// The lowest ratio the calibrator will learn.
    pub const MIN_CHARS_PER_TOKEN: f64 = 1.5;
    /// The highest ratio the calibrator will learn.
    pub const MAX_CHARS_PER_TOKEN: f64 = 6.0;
    /// Prompts reported below this many tokens are ignored: the chat template's own tokens
    /// dominate them.
    pub const MIN_SAMPLE_TOKENS: u32 = 32;
    /// How far each sample moves the ratio (an exponential moving average).
    const WEIGHT: f64 = 0.3;

    /// A calibrator at the 3 characters-per-token baseline.
    pub fn new() -> Self {
        Self {
            chars_per_token: BASELINE_CHARS_PER_TOKEN,
            samples: 0,
        }
    }

    /// Records one sample. Returns whether it was used.
    pub fn observe(&mut self, estimated: u32, actual: u32, truncated: bool) -> bool {
        if truncated || estimated == 0 || actual < Self::MIN_SAMPLE_TOKENS {
            return false;
        }
        let chars = f64::from(estimated) * BASELINE_CHARS_PER_TOKEN;
        let sample =
            (chars / f64::from(actual)).clamp(Self::MIN_CHARS_PER_TOKEN, Self::MAX_CHARS_PER_TOKEN);
        self.chars_per_token = if self.samples == 0 {
            sample
        } else {
            self.chars_per_token + Self::WEIGHT * (sample - self.chars_per_token)
        }
        .clamp(Self::MIN_CHARS_PER_TOKEN, Self::MAX_CHARS_PER_TOKEN);
        self.samples = self.samples.saturating_add(1);
        true
    }

    /// The learned characters-per-token ratio.
    pub fn chars_per_token(&self) -> f64 {
        self.chars_per_token
    }

    /// How many samples were used.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Estimates the tokens in `text` with the learned ratio.
    pub fn estimate(&self, text: &str) -> u32 {
        tokens_for_chars(text.chars().count(), self.chars_per_token)
    }

    /// Re-scales a baseline [`estimate_tokens`] figure to the learned ratio.
    pub fn calibrate(&self, estimated: u32) -> u32 {
        let chars = f64::from(estimated) * BASELINE_CHARS_PER_TOKEN;
        let tokens = (chars / self.chars_per_token).ceil();
        if tokens >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let tokens = tokens as u32;
            tokens
        }
    }

    /// True when the endpoint reports far fewer prompt tokens than any plausible tokenizer
    /// would produce for what was sent: fewer than the estimate at the highest ratio
    /// (6 characters per token). Ollama's OpenAI API drops old messages silently when the
    /// window overflows (ARCHITECTURE.md 8.4), and this is how the app notices. Pass the same
    /// flag to [`TokenCalibrator::observe`].
    pub fn looks_truncated(estimated: u32, actual: u32) -> bool {
        let chars = f64::from(estimated) * BASELINE_CHARS_PER_TOKEN;
        let floor = chars / Self::MAX_CHARS_PER_TOKEN;
        f64::from(actual) < floor * 0.9
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_is_ceil_chars_over_three() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 2);
        assert_eq!(estimate_tokens(&"x".repeat(300)), 100);
        // Characters, not bytes.
        assert_eq!(estimate_tokens("ééé"), 1);
    }

    #[test]
    fn calibrator_learns_and_clamps() {
        let mut calibrator = TokenCalibrator::new();
        // 300 estimated tokens = 900 chars; the server counted 225 tokens: 4 chars/token.
        assert!(calibrator.observe(300, 225, false));
        assert!((calibrator.chars_per_token() - 4.0).abs() < 1e-9);
        assert_eq!(calibrator.estimate(&"x".repeat(400)), 100);
        assert_eq!(calibrator.calibrate(300), 225);
        // Moves towards later samples gradually.
        calibrator.observe(300, 450, false); // 2 chars/token
        assert!((calibrator.chars_per_token() - 3.4).abs() < 1e-9);
        // Extreme samples are clamped.
        let mut low = TokenCalibrator::new();
        low.observe(100, 10_000, false);
        assert!((low.chars_per_token() - TokenCalibrator::MIN_CHARS_PER_TOKEN).abs() < 1e-9);
        let mut high = TokenCalibrator::new();
        high.observe(100_000, 40, false);
        assert!((high.chars_per_token() - TokenCalibrator::MAX_CHARS_PER_TOKEN).abs() < 1e-9);
    }

    #[test]
    fn calibrator_ignores_bad_samples() {
        let mut calibrator = TokenCalibrator::new();
        assert!(!calibrator.observe(300, 100, true));
        assert!(!calibrator.observe(0, 100, false));
        assert!(!calibrator.observe(10, 0, false));
        assert!(!calibrator.observe(10, 12, false));
        assert_eq!(calibrator.samples(), 0);
        assert!((calibrator.chars_per_token() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn truncation_heuristic() {
        // 3,000 estimated tokens = 9,000 chars; no tokenizer gets that below ~1,500 tokens.
        assert!(TokenCalibrator::looks_truncated(3_000, 900));
        assert!(!TokenCalibrator::looks_truncated(3_000, 1_500));
        assert!(!TokenCalibrator::looks_truncated(3_000, 3_200));
    }
}
