//! The generic describer for external tools (ARCHITECTURE.md 13.3, D6).
//!
//! The app cannot interpret a third-party tool's arguments, so the approval card shows
//! the server, the tool and the full arguments, and a grant covers that tool with exactly
//! those arguments, unless the user ticked "allow any arguments" for it.
//!
//! The server's hints are untrusted: only an explicit destructive mark raises the tool to
//! always-ask, an absent hint is unknown (normal risk), and a read-only hint never skips
//! a prompt, so every external call is treated as outbound and not read-only.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::schema::{canonical_json, sorted};

/// Longest one-line summary on a card.
const SUMMARY_CHARS: usize = 160;

/// The user's per-tool switches for an external tool, set in the server's tool list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPrefs {
    /// "Allow any arguments": a grant for this chat covers the whole tool (D6).
    pub allow_any_args: bool,
    /// "Always ask": the tool never runs under a grant.
    pub always_ask: bool,
}

/// What the generic describer needs to know about one external tool.
#[derive(Debug, Clone, Copy)]
pub struct ExternalTool<'a> {
    /// The server's name.
    pub server: &'a str,
    /// The exposed name (`<server>__<tool>`).
    pub exposed: &'a str,
    /// The tool's own name on its server.
    pub original: &'a str,
    /// The server's destructive hint, if it gave one.
    pub destructive_hint: Option<bool>,
}

/// The action for one call of an external tool. See the module documentation.
///
/// The payload carries the canonical arguments, which the host compares with the
/// arguments it is about to send, so what runs is what was shown.
pub fn describe_external(
    tool: ExternalTool<'_>,
    args: &Map<String, Value>,
    prefs: ToolPrefs,
) -> policy::Action {
    let args_value = Value::Object(args.clone());
    let canonical = canonical_json(&args_value);
    let pretty =
        serde_json::to_string_pretty(&sorted(&args_value)).unwrap_or_else(|_| canonical.clone());
    let summary = policy::sanitize::one_line(
        &format!("{} › {} {}", tool.server, tool.original, canonical),
        SUMMARY_CHARS,
    );
    let preview = policy::sanitize::visible(&format!(
        "Server: {}\nTool: {}\nArguments:\n{}",
        tool.server, tool.original, pretty
    ));
    let scope = if prefs.allow_any_args {
        policy::Scope::ExternalAny {
            server: tool.server.to_owned(),
            tool: tool.exposed.to_owned(),
        }
    } else {
        policy::Scope::ExternalExact {
            server: tool.server.to_owned(),
            tool: tool.exposed.to_owned(),
            args: canonical.clone(),
        }
    };
    policy::Action {
        server: tool.server.to_owned(),
        tool: tool.exposed.to_owned(),
        summary,
        preview,
        scope,
        risk: policy::external_risk(tool.destructive_hint, prefs.always_ask),
        read_only: false,
        outbound: true,
        // Allow all: a tool its server marks destructive, or the user marked always-ask,
        // keeps asking. Every external call may send data out, so it asks once
        // untrusted content has entered the turn.
        keep_asking: policy::allow_all::external(tool.destructive_hint, prefs.always_ask),
        network: true,
        hard_limit: None,
        payload: json!({
            "kind": "external",
            "originalTool": tool.original,
            "args": canonical,
        }),
    }
}

/// Whether `action` is the one [`describe_external`] produced for this tool and these
/// arguments. `original` is the server's own name for the tool now: if a re-listed tool
/// list moved the exposed name to another tool after the card was shown, the call is
/// refused rather than running a tool the user never saw.
pub(crate) fn action_matches(
    action: &policy::Action,
    server: &str,
    exposed: &str,
    original: &str,
    args: &Map<String, Value>,
) -> bool {
    action.server == server
        && action.tool == exposed
        && action.payload.get("kind").and_then(Value::as_str) == Some("external")
        && action.payload.get("originalTool").and_then(Value::as_str) == Some(original)
        && action.payload.get("args").and_then(Value::as_str)
            == Some(canonical_json(&Value::Object(args.clone())).as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use policy::{Risk, Scope};

    fn tool(destructive_hint: Option<bool>) -> ExternalTool<'static> {
        ExternalTool {
            server: "github",
            exposed: "github__create_issue",
            original: "create_issue",
            destructive_hint,
        }
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn exact_scope_uses_canonical_arguments() {
        let a = args(json!({"title": "x", "body": {"z": 1, "a": 2}}));
        let action = describe_external(tool(None), &a, ToolPrefs::default());
        assert_eq!(
            action.scope,
            Scope::ExternalExact {
                server: "github".into(),
                tool: "github__create_issue".into(),
                args: r#"{"body":{"a":2,"z":1},"title":"x"}"#.into(),
            }
        );
        // The same arguments in another order give the same scope.
        let b = args(json!({"body": {"a": 2, "z": 1}, "title": "x"}));
        assert_eq!(
            describe_external(tool(None), &b, ToolPrefs::default()).scope,
            action.scope
        );
        assert!(!action.read_only);
        assert!(action.outbound);
        assert_eq!(action.hard_limit, None);
        assert_eq!(action.server, "github");
        assert_eq!(action.tool, "github__create_issue");
    }

    #[test]
    fn allow_any_arguments_widens_the_scope() {
        let action = describe_external(
            tool(None),
            &args(json!({"q": 1})),
            ToolPrefs {
                allow_any_args: true,
                always_ask: false,
            },
        );
        assert_eq!(
            action.scope,
            Scope::ExternalAny {
                server: "github".into(),
                tool: "github__create_issue".into()
            }
        );
    }

    #[test]
    fn risk_follows_only_the_destructive_hint_and_user_prefs() {
        let a = args(json!({}));
        let normal = ToolPrefs::default();
        assert_eq!(describe_external(tool(None), &a, normal).risk, Risk::Normal);
        assert_eq!(
            describe_external(tool(Some(false)), &a, normal).risk,
            Risk::Normal
        );
        assert_eq!(
            describe_external(tool(Some(true)), &a, normal).risk,
            Risk::AlwaysAsk
        );
        let ask = ToolPrefs {
            allow_any_args: true,
            always_ask: true,
        };
        assert_eq!(
            describe_external(tool(Some(false)), &a, ask).risk,
            Risk::AlwaysAsk
        );
    }

    /// Allow all: the same two things that make a tool always-ask keep it asking, and
    /// every external call counts as able to send data out.
    #[test]
    fn allow_all_keeps_asking_for_marked_tools_and_treats_every_call_as_network() {
        let a = args(json!({"q": 1}));
        let ask = ToolPrefs {
            allow_any_args: false,
            always_ask: true,
        };
        for (hint, prefs, keeps_asking) in [
            (None, ToolPrefs::default(), false),
            (Some(false), ToolPrefs::default(), false),
            (Some(true), ToolPrefs::default(), true),
            (None, ask, true),
            (Some(false), ask, true),
        ] {
            let action = describe_external(tool(hint), &a, prefs);
            assert_eq!(
                action.keep_asking,
                keeps_asking.then_some(policy::KeepAsking::MarkedDestructive),
                "{hint:?} {prefs:?}"
            );
            assert_eq!(action.keep_asking.is_some(), action.risk == Risk::AlwaysAsk);
            assert!(action.network && action.outbound);
            let turn = |untrusted: bool| policy::TurnContext {
                tool_enabled: true,
                server_ready: true,
                untrusted_in_turn: untrusted,
                allow_all: true,
                ..Default::default()
            };
            let none = std::collections::BTreeSet::<Scope>::new();
            let first = policy::evaluate(&action, &turn(false), &none);
            let later = policy::evaluate(&action, &turn(true), &none);
            if keeps_asking {
                for decision in [first, later] {
                    assert_eq!(
                        decision,
                        policy::Decision::Ask {
                            card: policy::CardKind::OnceOnly,
                            reason: policy::AskReason::StillAsksInAllowAll
                        }
                    );
                }
            } else {
                assert_eq!(first, policy::Decision::AllowedByMode);
                assert_eq!(
                    later,
                    policy::Decision::Ask {
                        card: policy::CardKind::Grantable,
                        reason: policy::AskReason::UntrustedContentThenOutbound
                    }
                );
            }
        }
    }

    #[test]
    fn a_read_only_hint_never_lowers_anything() {
        // The read-only hint is not even an input: the action is the same as with no hint.
        let a = args(json!({"path": "/etc"}));
        let action = describe_external(tool(None), &a, ToolPrefs::default());
        assert!(!action.read_only);
        assert_eq!(action.risk, Risk::Normal);
    }

    #[test]
    fn preview_shows_everything_with_invisible_characters_escaped() {
        let a = args(json!({
            "text": "safe\u{202E}txt.exe",
            "zw": "a\u{200B}b",
            "long": "x".repeat(500),
            "nested": {"k": [1, 2]}
        }));
        let action = describe_external(tool(None), &a, ToolPrefs::default());
        assert!(
            action
                .preview
                .starts_with("Server: github\nTool: create_issue\nArguments:\n{")
        );
        assert!(action.preview.contains("\\u{202E}"), "{}", action.preview);
        assert!(action.preview.contains("\\u{200B}"));
        assert!(!action.preview.contains('\u{202E}'));
        // The full arguments are shown, pretty-printed in key order.
        assert!(action.preview.contains(&"x".repeat(500)));
        let long_pos = action.preview.find("\"long\"").unwrap();
        let text_pos = action.preview.find("\"text\"").unwrap();
        assert!(long_pos < text_pos);
        assert!(action.preview.contains("\n  \"nested\": {"));
        // The summary is one line, capped.
        assert!(!action.summary.contains('\n'));
        assert!(action.summary.chars().count() <= SUMMARY_CHARS);
        assert!(action.summary.starts_with("github › create_issue {"));
        assert!(!action.summary.contains('\u{202E}'));
    }

    #[test]
    fn the_action_is_bound_to_its_arguments() {
        let a = args(json!({"q": "x"}));
        let action = describe_external(tool(None), &a, ToolPrefs::default());
        let matches = |server: &str, exposed: &str, original: &str, a: &Map<String, Value>| {
            action_matches(&action, server, exposed, original, a)
        };
        assert!(matches(
            "github",
            "github__create_issue",
            "create_issue",
            &a
        ));
        // The same arguments in another key order still match.
        let reordered = args(json!({"q": "x"}));
        assert!(matches(
            "github",
            "github__create_issue",
            "create_issue",
            &reordered
        ));
        assert!(!matches(
            "github",
            "github__create_issue",
            "create_issue",
            &args(json!({"q": "y"}))
        ));
        assert!(!matches(
            "gitlab",
            "github__create_issue",
            "create_issue",
            &a
        ));
        assert!(!matches("github", "github__other", "create_issue", &a));
        // The exposed name now belongs to another tool of the server.
        assert!(!matches(
            "github",
            "github__create_issue",
            "create.issue",
            &a
        ));
        // An action from another describer is never accepted.
        let mut foreign = action.clone();
        foreign.payload = json!({"args": canonical_json(&Value::Object(a.clone()))});
        assert!(!action_matches(
            &foreign,
            "github",
            "github__create_issue",
            "create_issue",
            &a
        ));
    }
}
