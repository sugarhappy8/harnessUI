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
    /// The app's own data for the current chat: artifacts, the task record, stored tool
    /// output, the chat's tool list. Never prompts (0.0.4): nothing outside the app is
    /// read or changed.
    AppData,
    /// Web search for this chat (0.0.4): `web_search` and `read_source`. One grant covers
    /// both: a search sends the question to the search service the user chose, and reading
    /// a source fetches a page that a search of this chat listed. Unlike [`Scope::AppData`]
    /// it prompts like any other scope.
    WebSearch,
}

/// Why an action still asks while its chat is in Allow all (0.0.4). The describer sets it;
/// the engine only reads it. In Ask mode it changes nothing: there [`Risk`] decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeepAsking {
    /// Deletes files or folders, or leaves a file empty.
    Delete,
    /// Discards work, changes the disk or the system, or runs with elevated rights.
    Destroy,
    /// Writes to a sensitive place: a `.git` folder, shell start-up files, git and ssh
    /// configuration, autostart locations.
    Sensitive,
    /// Spends money, sends or publishes in the user's name: a pay, buy, send or post
    /// button, `git push`.
    Publish,
    /// Input that reaches an application no grant covers (D7): a terminal or shell host,
    /// system settings, a password manager, the system's own shortcuts.
    ProtectedApp,
    /// An external tool its server marks destructive, or the user marked always-ask.
    MarkedDestructive,
    /// A command that hides what it runs (an encoded command), so nothing can be checked.
    Hidden,
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
    /// Allow all (0.0.4): set when the action still asks while its chat is in Allow all,
    /// with the kind of reason. `None` (the default for a new tool): Allow all may run it
    /// without a card. Ask mode does not read this.
    #[serde(default)]
    pub keep_asking: Option<KeepAsking>,
    /// Allow all (0.0.4): the action can send data off the machine (it reaches the
    /// network, names a network program, or is a command that cannot be fully broken
    /// down). In Allow all such an action asks once untrusted content has entered the
    /// turn (D9). `false` for anything that stays on this machine. Ask mode does not read
    /// this: there `outbound` decides.
    #[serde(default)]
    pub network: bool,
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
    /// The chat is in Allow all (0.0.4): the engine answers the card itself, except for
    /// actions that keep asking. `false` is Ask mode, the behaviour of steps 3 to 7.
    pub allow_all: bool,
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
    /// The chat is in Allow all and this is one of the actions that still ask
    /// ([`Action::keep_asking`] says which kind).
    StillAsksInAllowAll,
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
    /// The chat is in Allow all and nothing about the action keeps asking (0.0.4).
    AllowedByMode,
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

/// The evaluation order of ARCHITECTURE.md 13.3. Steps 1 and 2 hold in every mode. In
/// Allow all ([`TurnContext::allow_all`]) steps 3 to 7 are replaced: an action that keeps
/// asking ([`Action::keep_asking`]) asks with Allow once / Deny, an action that can send
/// data out ([`Action::network`]) asks once untrusted content is in the turn, and
/// everything else is allowed by the mode.
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
    // The app's own data for the chat never prompts either (0.0.4).
    if matches!(action.scope, Scope::Skill | Scope::AppData) && action.risk == Risk::Normal {
        return Decision::AllowedByGrant;
    }
    // Allow all (0.0.4): the engine answers the card itself, unless the action is one that
    // keeps asking, or can send data out after untrusted content entered the turn (D9).
    // A call recovered from text is treated like a native one, and grants are not read.
    if turn.allow_all {
        if action.keep_asking.is_some() {
            return Decision::Ask {
                card: CardKind::OnceOnly,
                reason: AskReason::StillAsksInAllowAll,
            };
        }
        if turn.untrusted_in_turn && action.network {
            return Decision::Ask {
                card: CardKind::Grantable,
                reason: AskReason::UntrustedContentThenOutbound,
            };
        }
        return Decision::AllowedByMode;
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
            keep_asking: None,
            network: false,
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

    /// The app's own per-chat data (artifacts, task, stored output, tool list) behaves like
    /// skills: no card, but a tool that is off or a hard limit still stops the call, and a
    /// describer can still raise one of these to always-ask.
    #[test]
    fn app_data_does_not_prompt_but_respects_steps_1_to_3() {
        let a = action(Scope::AppData, Risk::Normal, false, false);
        assert_eq!(
            evaluate(&a, &ready(), &BTreeSet::new()),
            Decision::AllowedByGrant
        );
        let mut untrusted = ready();
        untrusted.untrusted_in_turn = true;
        untrusted.recovered_from_text = true;
        assert_eq!(
            evaluate(&a, &untrusted, &BTreeSet::new()),
            Decision::AllowedByGrant
        );
        let mut off = ready();
        off.tool_enabled = false;
        assert!(matches!(
            evaluate(&a, &off, &BTreeSet::new()),
            Decision::Refused { .. }
        ));
        let mut limited = a.clone();
        limited.hard_limit = Some("too large".into());
        assert!(matches!(
            evaluate(&limited, &ready(), &BTreeSet::new()),
            Decision::Denied { .. }
        ));
        let always = action(Scope::AppData, Risk::AlwaysAsk, false, false);
        assert!(matches!(
            evaluate(&always, &ready(), &BTreeSet::new()),
            Decision::Ask {
                card: CardKind::OnceOnly,
                ..
            }
        ));
    }

    /// Allow all (0.0.4), in the order of `evaluate`: what keeps asking asks with Allow
    /// once / Deny only; what can send data out asks once untrusted content is in the
    /// turn; everything else is allowed by the mode, grant or no grant, native call or
    /// recovered from text. The rows of what keeps asking are in `allow_all::tests`.
    #[test]
    fn allow_all_replaces_steps_3_to_7() {
        let allow_all = |untrusted: bool, recovered: bool| TurnContext {
            allow_all: true,
            untrusted_in_turn: untrusted,
            recovered_from_text: recovered,
            ..ready()
        };
        let none = BTreeSet::new();
        let granted: BTreeSet<Scope> = [write_scope()].into();
        let still_asks = Decision::Ask {
            card: CardKind::OnceOnly,
            reason: AskReason::StillAsksInAllowAll,
        };

        // A plain write: no card, also for a call recovered from text and after untrusted
        // content (it sends nothing out).
        let write = action(write_scope(), Risk::Normal, false, false);
        for (untrusted, recovered) in [(false, false), (true, false), (false, true), (true, true)] {
            for grants in [&none, &granted] {
                assert_eq!(
                    evaluate(&write, &allow_all(untrusted, recovered), grants),
                    Decision::AllowedByMode
                );
            }
        }
        // In Ask mode the same call asks, or runs under its grant.
        assert_eq!(
            evaluate(&write, &ready(), &none),
            Decision::Ask {
                card: CardKind::Grantable,
                reason: AskReason::Normal
            }
        );
        assert_eq!(
            evaluate(&write, &ready(), &granted),
            Decision::AllowedByGrant
        );

        // Every kind of "keeps asking" asks, whatever was granted.
        for kind in [
            KeepAsking::Delete,
            KeepAsking::Destroy,
            KeepAsking::Sensitive,
            KeepAsking::Publish,
            KeepAsking::ProtectedApp,
            KeepAsking::MarkedDestructive,
            KeepAsking::Hidden,
        ] {
            for risk in [Risk::Normal, Risk::AlwaysAsk] {
                let mut a = action(write_scope(), risk, false, false);
                a.keep_asking = Some(kind);
                for (untrusted, recovered) in [(false, false), (true, true)] {
                    assert_eq!(
                        evaluate(&a, &allow_all(untrusted, recovered), &granted),
                        still_asks,
                        "{kind:?}"
                    );
                }
            }
        }

        // An action that can send data out: allowed until untrusted content is in the
        // turn, then it asks as D9 does, grant or not.
        let mut fetch = action(write_scope(), Risk::Normal, false, true);
        fetch.network = true;
        assert_eq!(
            evaluate(&fetch, &allow_all(false, false), &none),
            Decision::AllowedByMode
        );
        assert_eq!(
            evaluate(&fetch, &allow_all(true, false), &granted),
            Decision::Ask {
                card: CardKind::Grantable,
                reason: AskReason::UntrustedContentThenOutbound
            }
        );
        // Outbound alone (a terminal command that stays on this machine) is not enough.
        let local = action(write_scope(), Risk::Normal, false, true);
        assert_eq!(
            evaluate(&local, &allow_all(true, false), &none),
            Decision::AllowedByMode
        );

        // Steps 1 and 2 come first.
        let mut off = allow_all(false, false);
        off.tool_enabled = false;
        assert!(matches!(
            evaluate(&write, &off, &granted),
            Decision::Refused { .. }
        ));
        let mut limited = write.clone();
        limited.hard_limit = Some("outside approved folders".into());
        assert!(matches!(
            evaluate(&limited, &allow_all(false, false), &granted),
            Decision::Denied { .. }
        ));
        // The app's own data and skills are recorded as before, not as allowed by the mode.
        for scope in [Scope::AppData, Scope::Skill] {
            assert_eq!(
                evaluate(
                    &action(scope, Risk::Normal, false, false),
                    &allow_all(true, true),
                    &none
                ),
                Decision::AllowedByGrant
            );
        }
    }

    /// The new facts have serde defaults: an action stored or sent without them reads as
    /// "nothing keeps asking, nothing leaves the machine".
    #[test]
    fn the_allow_all_facts_default_to_nothing() {
        let old = serde_json::json!({
            "server": "files", "tool": "read_file", "summary": "", "preview": "",
            "scope": {"type": "appData"}, "risk": "normal", "readOnly": true,
            "outbound": false, "hardLimit": null
        });
        let a: Action = serde_json::from_value(old).unwrap();
        assert_eq!((a.keep_asking, a.network), (None, false));
        let mut b = a.clone();
        b.keep_asking = Some(KeepAsking::MarkedDestructive);
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["keepAsking"], "markedDestructive");
        assert_eq!(json["network"], false);
        assert_eq!(
            serde_json::to_value(AskReason::StillAsksInAllowAll).unwrap(),
            "stillAsksInAllowAll"
        );
        assert!(!TurnContext::default().allow_all, "Ask is the default mode");
    }

    /// Web search (0.0.4) is an ordinary grantable scope: it asks first, "Allow for this
    /// chat" covers later searches of that chat, and because a search sends the question
    /// out, it asks again once untrusted content is in the turn (D9), grant or not.
    #[test]
    fn web_search_is_grantable_and_asks_again_after_untrusted_content() {
        let search = Action {
            server: "search".into(),
            tool: "web_search".into(),
            network: true,
            ..action(Scope::WebSearch, Risk::Normal, true, true)
        };
        let none: BTreeSet<Scope> = BTreeSet::new();
        let granted: BTreeSet<Scope> = [Scope::WebSearch].into();
        // The first search in a chat: a card that offers "Allow for this chat".
        assert_eq!(
            evaluate(&search, &ready(), &none),
            Decision::Ask {
                card: CardKind::Grantable,
                reason: AskReason::Normal
            }
        );
        assert_eq!(
            evaluate(&search, &ready(), &granted),
            Decision::AllowedByGrant
        );
        // Never without a card or a grant, unlike the app's own data.
        assert!(!none.covers(&Scope::WebSearch));
        assert!(!BTreeSet::from([Scope::AppData, Scope::Skill]).covers(&Scope::WebSearch));
        // After a tool result or an attachment entered the turn, it asks whatever was granted.
        let mut untrusted = ready();
        untrusted.untrusted_in_turn = true;
        assert_eq!(
            evaluate(&search, &untrusted, &granted),
            Decision::Ask {
                card: CardKind::Grantable,
                reason: AskReason::UntrustedContentThenOutbound
            }
        );
        // The same in Allow all: allowed by the mode, except after untrusted content.
        let mut allow_all = ready();
        allow_all.allow_all = true;
        assert_eq!(
            evaluate(&search, &allow_all, &none),
            Decision::AllowedByMode
        );
        allow_all.untrusted_in_turn = true;
        assert!(matches!(
            evaluate(&search, &allow_all, &granted),
            Decision::Ask {
                reason: AskReason::UntrustedContentThenOutbound,
                ..
            }
        ));
        // Switched off in the chat, it is refused like any tool.
        let mut off = ready();
        off.tool_enabled = false;
        assert!(matches!(
            evaluate(&search, &off, &granted),
            Decision::Refused { .. }
        ));
        assert_eq!(
            serde_json::to_value(Scope::WebSearch).unwrap(),
            serde_json::json!({"type": "webSearch"})
        );
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
