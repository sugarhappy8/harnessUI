//! The policy engine (ARCHITECTURE.md 13.3): every tool call becomes an [`Action`],
//! and [`evaluate`] decides whether it runs, is refused, or needs the user.
//!
//! The engine only reads policy state. Grants change through UI commands, never
//! through a tool or anything the model says (13.5).

use serde::{Deserialize, Serialize};

/// How dangerous an action is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Risk {
    /// May be covered by a grant for this chat.
    Normal,
    /// Always shows a card with Allow once / Deny only.
    AlwaysAsk,
}

/// What a grant for this chat is keyed on (the "covers" column of 13.3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Scope {
    /// Reads inside one approved folder (its canonical path).
    FileRead {
        folder: String,
    },
    /// Writes inside one approved folder.
    FileWrite {
        folder: String,
    },
    /// One exact command: resolved program, normalised arguments, working folder.
    Command {
        program: String,
        args: Vec<String>,
        cwd: String,
    },
    /// A read-only program, any arguments (path arguments held to approved folders).
    ReadOnlyProgram {
        program: String,
    },
    /// A plain sequence of commands (`a && b | c`): covered only when every part is.
    Commands {
        parts: Vec<Scope>,
    },
    GitRead {
        repo: String,
    },
    GitWrite {
        repo: String,
    },
    GitClone {
        host: String,
    },
    /// scheme + host + port.
    Site {
        origin: String,
    },
    Screenshots,
    /// One kind of input to the application that was in front at approval.
    ComputerInput {
        kind: String,
        app: String,
    },
    /// One external tool with exactly these arguments (canonical JSON).
    ExternalExact {
        server: String,
        tool: String,
        args: String,
    },
    /// One external tool, any arguments (the user ticked "allow any arguments").
    ExternalAny {
        server: String,
        tool: String,
    },
    /// Skills never prompt (D11).
    Skill,
}

/// A tool call, described for policy and for the approval card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub server: String,
    pub tool: String,
    /// One line for the card (already sanitised by the describer).
    pub summary: String,
    /// The full preview: canonical path, resolved program, punycode host, diff…
    pub preview: String,
    pub scope: Scope,
    pub risk: Risk,
    /// Only reads (files, git status, page snapshots, screenshots).
    pub read_only: bool,
    /// Sends data off the machine or to another program (network, terminal, MCP).
    pub outbound: bool,
    /// Set when the describer found a hard limit (13.3 step 2): no prompt.
    pub hard_limit: Option<String>,
    /// Tool-specific data the describer computed and the tool re-checks before its side
    /// effect (canonical path, the file hash the diff was computed against, …).
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// What the agent loop knows about this turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnContext {
    /// The tool is enabled for this chat.
    pub tool_enabled: bool,
    /// Its server is running and initialised.
    pub server_ready: bool,
    /// The call was recovered from plain text, not a native tool call (8.3).
    pub recovered_from_text: bool,
    /// Untrusted content (a web page, file contents, a tool result) entered this turn (D9).
    pub untrusted_in_turn: bool,
}

/// The buttons a card offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CardKind {
    /// Allow once / Deny.
    OnceOnly,
    /// Allow once / Allow for this chat / Deny.
    Grantable,
}

/// Why a card is shown, when it is not the ordinary case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskReason {
    Normal,
    AlwaysAsk,
    RecoveredFromText,
    UntrustedContentThenOutbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Decision {
    /// Step 1: the model is told the tool is unavailable.
    Refused { reason: String },
    /// Step 2: a hard limit; no prompt.
    Denied { reason: String },
    /// Step 6.
    AllowedByGrant,
    /// Steps 3, 4, 5, 7.
    Ask { card: CardKind, reason: AskReason },
}

/// Grants for one chat. Read-only to the engine.
pub trait Grants {
    fn covers(&self, scope: &Scope) -> bool;
}

impl Grants for std::collections::BTreeSet<Scope> {
    fn covers(&self, scope: &Scope) -> bool {
        self.contains(scope)
            || match scope {
                // A per-tool "any arguments" grant covers exact-argument calls.
                Scope::ExternalExact { server, tool, .. } => self.contains(&Scope::ExternalAny {
                    server: server.clone(),
                    tool: tool.clone(),
                }),
                Scope::Commands { parts } => {
                    !parts.is_empty() && parts.iter().all(|p| self.covers(p))
                }
                _ => false,
            }
    }
}

/// The evaluation order of ARCHITECTURE.md 13.3.
pub fn evaluate(action: &Action, turn: &TurnContext, grants: &dyn Grants) -> Decision {
    // 1. Not enabled / server not ready.
    if !turn.tool_enabled {
        return Decision::Refused {
            reason: format!("the tool {} is not enabled in this chat", action.tool),
        };
    }
    if !turn.server_ready {
        return Decision::Refused {
            reason: format!("{} is not available right now", action.server),
        };
    }
    // 2. Hard limits.
    if let Some(reason) = &action.hard_limit {
        return Decision::Denied {
            reason: reason.clone(),
        };
    }
    // Skills never prompt (D11), but only after the checks above.
    if action.scope == Scope::Skill && action.risk == Risk::Normal {
        return Decision::AllowedByGrant;
    }
    // 3. Always ask.
    if action.risk == Risk::AlwaysAsk {
        return Decision::Ask {
            card: CardKind::OnceOnly,
            reason: AskReason::AlwaysAsk,
        };
    }
    // 4. Recovered from text and not a read.
    if turn.recovered_from_text && !action.read_only {
        return Decision::Ask {
            card: CardKind::OnceOnly,
            reason: AskReason::RecoveredFromText,
        };
    }
    // 5. Untrusted content then an outbound action (D9).
    if turn.untrusted_in_turn && action.outbound {
        return Decision::Ask {
            card: CardKind::Grantable,
            reason: AskReason::UntrustedContentThenOutbound,
        };
    }
    // 6. A grant for this chat.
    if grants.covers(&action.scope) {
        return Decision::AllowedByGrant;
    }
    // 7. Ask.
    Decision::Ask {
        card: CardKind::Grantable,
        reason: AskReason::Normal,
    }
}

/// External tool hints (13.3): only an explicit destructive mark, or the user's own
/// "always ask", raises a tool to always-ask. A read-only hint never skips a prompt,
/// and an absent hint is unknown, not destructive.
pub fn external_risk(destructive_hint: Option<bool>, user_always_ask: bool) -> Risk {
    if user_always_ask || destructive_hint == Some(true) {
        Risk::AlwaysAsk
    } else {
        Risk::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn action(scope: Scope, risk: Risk, read_only: bool, outbound: bool) -> Action {
        Action {
            server: "files".into(),
            tool: "write_file".into(),
            summary: String::new(),
            preview: String::new(),
            scope,
            risk,
            read_only,
            outbound,
            hard_limit: None,
            payload: serde_json::Value::Null,
        }
    }

    fn ready() -> TurnContext {
        TurnContext {
            tool_enabled: true,
            server_ready: true,
            ..Default::default()
        }
    }

    fn write_scope() -> Scope {
        Scope::FileWrite {
            folder: "/p".into(),
        }
    }

    #[test]
    fn step1_disabled_or_not_ready_is_refused() {
        let a = action(write_scope(), Risk::Normal, false, false);
        let mut t = ready();
        t.tool_enabled = false;
        assert!(matches!(
            evaluate(&a, &t, &BTreeSet::new()),
            Decision::Refused { .. }
        ));
        let mut t = ready();
        t.server_ready = false;
        assert!(matches!(
            evaluate(&a, &t, &BTreeSet::new()),
            Decision::Refused { .. }
        ));
    }

    #[test]
    fn step2_hard_limit_denies_even_with_a_grant() {
        let mut a = action(write_scope(), Risk::Normal, false, false);
        a.hard_limit = Some("outside approved folders".into());
        let grants: BTreeSet<Scope> = [write_scope()].into();
        assert_eq!(
            evaluate(&a, &ready(), &grants),
            Decision::Denied {
                reason: "outside approved folders".into()
            }
        );
    }

    #[test]
    fn step3_always_ask_ignores_grants() {
        let a = action(write_scope(), Risk::AlwaysAsk, false, false);
        let grants: BTreeSet<Scope> = [write_scope()].into();
        assert_eq!(
            evaluate(&a, &ready(), &grants),
            Decision::Ask {
                card: CardKind::OnceOnly,
                reason: AskReason::AlwaysAsk
            }
        );
    }

    #[test]
    fn step4_recovered_calls_ask_unless_reads() {
        let mut t = ready();
        t.recovered_from_text = true;
        let grants: BTreeSet<Scope> = [
            write_scope(),
            Scope::FileRead {
                folder: "/p".into(),
            },
        ]
        .into();
        let w = action(write_scope(), Risk::Normal, false, false);
        assert_eq!(
            evaluate(&w, &t, &grants),
            Decision::Ask {
                card: CardKind::OnceOnly,
                reason: AskReason::RecoveredFromText
            }
        );
        let r = action(
            Scope::FileRead {
                folder: "/p".into(),
            },
            Risk::Normal,
            true,
            false,
        );
        assert_eq!(evaluate(&r, &t, &grants), Decision::AllowedByGrant);
    }

    #[test]
    fn step5_untrusted_then_outbound_asks_even_with_grant() {
        let mut t = ready();
        t.untrusted_in_turn = true;
        let site = Scope::Site {
            origin: "https://example.com".into(),
        };
        let grants: BTreeSet<Scope> = [site.clone()].into();
        let nav = action(site.clone(), Risk::Normal, false, true);
        assert_eq!(
            evaluate(&nav, &t, &grants),
            Decision::Ask {
                card: CardKind::Grantable,
                reason: AskReason::UntrustedContentThenOutbound
            }
        );
        // Without untrusted content the grant applies.
        assert_eq!(evaluate(&nav, &ready(), &grants), Decision::AllowedByGrant);
        // Local writes are not outbound.
        let w = action(write_scope(), Risk::Normal, false, false);
        let wg: BTreeSet<Scope> = [write_scope()].into();
        assert_eq!(evaluate(&w, &t, &wg), Decision::AllowedByGrant);
    }

    #[test]
    fn step6_grant_scope_must_match_exactly() {
        let grants: BTreeSet<Scope> = [Scope::FileWrite {
            folder: "/p".into(),
        }]
        .into();
        let other = action(
            Scope::FileWrite {
                folder: "/q".into(),
            },
            Risk::Normal,
            false,
            false,
        );
        assert_eq!(
            evaluate(&other, &ready(), &grants),
            Decision::Ask {
                card: CardKind::Grantable,
                reason: AskReason::Normal
            }
        );
        // A read grant does not cover writes in the same folder.
        let rg: BTreeSet<Scope> = [Scope::FileRead {
            folder: "/p".into(),
        }]
        .into();
        let w = action(write_scope(), Risk::Normal, false, false);
        assert!(matches!(evaluate(&w, &ready(), &rg), Decision::Ask { .. }));
        // Commands: arguments and folder are part of the scope.
        let cmd = |args: &[&str], cwd: &str| Scope::Command {
            program: "/usr/bin/cargo".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: cwd.into(),
        };
        let g: BTreeSet<Scope> = [cmd(&["test"], "/p")].into();
        assert_eq!(
            evaluate(
                &action(cmd(&["test"], "/p"), Risk::Normal, false, true),
                &ready(),
                &g
            ),
            Decision::AllowedByGrant
        );
        assert!(matches!(
            evaluate(
                &action(cmd(&["test", "--release"], "/p"), Risk::Normal, false, true),
                &ready(),
                &g
            ),
            Decision::Ask { .. }
        ));
        assert!(matches!(
            evaluate(
                &action(cmd(&["test"], "/q"), Risk::Normal, false, true),
                &ready(),
                &g
            ),
            Decision::Ask { .. }
        ));
    }

    #[test]
    fn external_any_arguments_grant_covers_exact_calls() {
        let any: BTreeSet<Scope> = [Scope::ExternalAny {
            server: "s".into(),
            tool: "t".into(),
        }]
        .into();
        let call = action(
            Scope::ExternalExact {
                server: "s".into(),
                tool: "t".into(),
                args: "{\"a\":1}".into(),
            },
            Risk::Normal,
            false,
            true,
        );
        assert_eq!(evaluate(&call, &ready(), &any), Decision::AllowedByGrant);
        let other_tool = action(
            Scope::ExternalExact {
                server: "s".into(),
                tool: "u".into(),
                args: "{}".into(),
            },
            Risk::Normal,
            false,
            true,
        );
        assert!(matches!(
            evaluate(&other_tool, &ready(), &any),
            Decision::Ask { .. }
        ));
        let exact: BTreeSet<Scope> = [Scope::ExternalExact {
            server: "s".into(),
            tool: "t".into(),
            args: "{\"a\":1}".into(),
        }]
        .into();
        let different_args = action(
            Scope::ExternalExact {
                server: "s".into(),
                tool: "t".into(),
                args: "{\"a\":2}".into(),
            },
            Risk::Normal,
            false,
            true,
        );
        assert!(matches!(
            evaluate(&different_args, &ready(), &exact),
            Decision::Ask { .. }
        ));
    }

    #[test]
    fn command_sequences_need_every_part_covered() {
        let ls = Scope::ReadOnlyProgram {
            program: "/bin/ls".into(),
        };
        let test = Scope::Command {
            program: "/usr/bin/cargo".into(),
            args: vec!["test".into()],
            cwd: "/p".into(),
        };
        let seq = Scope::Commands {
            parts: vec![ls.clone(), test.clone()],
        };
        let only_ls: BTreeSet<Scope> = [ls.clone()].into();
        assert!(!only_ls.covers(&seq));
        let both: BTreeSet<Scope> = [ls, test].into();
        assert!(both.covers(&seq));
        assert!(!both.covers(&Scope::Commands { parts: vec![] }));
    }

    #[test]
    fn external_hints() {
        assert_eq!(external_risk(Some(true), false), Risk::AlwaysAsk);
        assert_eq!(external_risk(None, false), Risk::Normal);
        assert_eq!(external_risk(Some(false), false), Risk::Normal);
        assert_eq!(external_risk(Some(false), true), Risk::AlwaysAsk);
    }

    #[test]
    fn skills_do_not_prompt_but_respect_steps_1_and_2() {
        let s = action(Scope::Skill, Risk::Normal, true, false);
        assert_eq!(
            evaluate(&s, &ready(), &BTreeSet::new()),
            Decision::AllowedByGrant
        );
        let mut t = ready();
        t.tool_enabled = false;
        assert!(matches!(
            evaluate(&s, &t, &BTreeSet::new()),
            Decision::Refused { .. }
        ));
        let mut s2 = s.clone();
        s2.hard_limit = Some("outside".into());
        assert!(matches!(
            evaluate(&s2, &ready(), &BTreeSet::new()),
            Decision::Denied { .. }
        ));
    }
}
