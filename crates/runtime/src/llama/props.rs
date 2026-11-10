//! The parts of `GET /props` the app uses (ARCHITECTURE.md 7.3, 8.1).
//!
//! `/props` needs the API key. After every start the app reads the context the server
//! actually allocated and what the loaded chat template supports from it.

use serde::{Deserialize, Serialize};

/// What the loaded chat template supports (`chat_template_caps`). Missing keys read as
/// `false`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateCaps {
    /// The template renders a tool list.
    pub supports_tools: bool,
    /// The template renders tool calls; native tool calling is enabled only when true.
    pub supports_tool_calls: bool,
    /// Several tool calls in one message.
    pub supports_parallel_tool_calls: bool,
    /// A system message is accepted.
    pub supports_system_role: bool,
    /// Earlier reasoning can be sent back in history.
    pub supports_preserve_reasoning: bool,
    /// The template accepts a reasoning-effort setting.
    pub supports_reasoning_effort: bool,
}

/// Input kinds the loaded model accepts (`modalities`). Missing keys read as `false`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Modalities {
    /// Images (needs a projector).
    pub vision: bool,
    /// Audio input.
    pub audio: bool,
}

/// The fields of `/props` the runtime keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawProps")]
pub struct Props {
    /// Context per slot actually allocated (`default_generation_settings.n_ctx`). With
    /// `-np 1` this is the whole context.
    pub n_ctx: u32,
    /// The model path the server loaded.
    pub model_path: String,
    /// Build string, for example `b12110-cd959bae9`.
    pub build_info: Option<String>,
    /// Number of slots.
    pub total_slots: Option<u32>,
    /// Chat-template capabilities.
    pub chat_template_caps: TemplateCaps,
    /// Accepted input kinds.
    pub modalities: Modalities,
}

#[derive(Deserialize)]
struct RawProps {
    #[serde(default)]
    default_generation_settings: RawGenerationSettings,
    #[serde(default)]
    model_path: String,
    #[serde(default)]
    build_info: Option<String>,
    #[serde(default)]
    total_slots: Option<u32>,
    #[serde(default)]
    chat_template_caps: TemplateCaps,
    #[serde(default)]
    modalities: Modalities,
    /// Present when deserialising a [`Props`] that was serialised by this crate.
    #[serde(default)]
    n_ctx: Option<u32>,
}

#[derive(Default, Deserialize)]
struct RawGenerationSettings {
    #[serde(default)]
    n_ctx: u32,
}

impl From<RawProps> for Props {
    fn from(raw: RawProps) -> Self {
        Self {
            n_ctx: raw.n_ctx.unwrap_or(raw.default_generation_settings.n_ctx),
            model_path: raw.model_path,
            build_info: raw.build_info,
            total_slots: raw.total_slots,
            chat_template_caps: raw.chat_template_caps,
            modalities: raw.modalities,
        }
    }
}

impl Props {
    /// Parses a `/props` response body.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_captured_fixture() {
        let props =
            Props::parse(include_str!("../../tests/fixtures/props.json")).expect("fixture parses");
        assert_eq!(props.n_ctx, 4096);
        assert_eq!(props.model_path, "Qwen3-0.6B-Q8_0.gguf");
        assert_eq!(props.build_info.as_deref(), Some("b12110-cd959bae9"));
        assert_eq!(props.total_slots, Some(1));
        assert_eq!(
            props.chat_template_caps,
            TemplateCaps {
                supports_tools: true,
                supports_tool_calls: true,
                supports_parallel_tool_calls: true,
                supports_system_role: true,
                supports_preserve_reasoning: false,
                supports_reasoning_effort: false,
            }
        );
        assert_eq!(
            props.modalities,
            Modalities {
                vision: false,
                audio: false
            }
        );
    }

    #[test]
    fn missing_fields_default_to_false_or_empty() {
        let props = Props::parse(
            r#"{"default_generation_settings":{"n_ctx":2048},"modalities":{"vision":true}}"#,
        )
        .expect("parses");
        assert_eq!(props.n_ctx, 2048);
        assert!(props.modalities.vision);
        assert!(!props.modalities.audio);
        assert_eq!(props.chat_template_caps, TemplateCaps::default());
        assert_eq!(props.build_info, None);
        assert!(Props::parse("{}").is_ok());
        assert!(Props::parse("not json").is_err());
        assert!(Props::parse("42").is_err());
        assert!(Props::parse(r#"{"default_generation_settings":{"n_ctx":-1}}"#).is_err());
    }

    #[test]
    fn round_trips_through_its_own_serialisation() {
        let props =
            Props::parse(include_str!("../../tests/fixtures/props.json")).expect("fixture parses");
        let json = serde_json::to_string(&props).expect("serialise");
        assert_eq!(Props::parse(&json).expect("re-parse"), props);
    }
}
