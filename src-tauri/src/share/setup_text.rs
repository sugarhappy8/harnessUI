//! The setup cards of the Other apps page (`share_setup`): how to point Claude Code,
//! Codex or the Claude app at the sharing port.
//!
//! What must hold:
//!
//! - **Real serialisers.** Settings files are written with `serde_json` and `toml`, never by
//!   joining strings, so no value (a port, a key, a model name, a context size) can add a
//!   line or a key of its own. Shell lines quote every value for the shell they are for
//!   (PowerShell or the Mac's zsh), with control characters removed first.
//! - **One session first.** Each app has a way that lasts one window or is used only when
//!   asked (variables for one window; a separate Codex profile), then the settings-file
//!   way, with the file's full path on each system.
//! - **Names from the official documentation** (checked for 0.0.5, see `map-details.md`
//!   item 3):
//!   - Claude Code: `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN` (sent as
//!     `Authorization: Bearer`), `ANTHROPIC_MODEL`, `CLAUDE_CODE_MAX_CONTEXT_TOKENS` (the
//!     window it assumes for a model id that does not start with `claude-`),
//!     `CLAUDE_CODE_MAX_OUTPUT_TOKENS` (its default for an unknown model, 32,000, would
//!     leave almost no room in a small context), `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`;
//!     the settings file `~/.claude/settings.json` (`%USERPROFILE%\.claude\settings.json`
//!     on Windows) with an `env` block.
//!   - Codex: `model_provider`, `model`, `model_context_window`,
//!     `model_auto_compact_token_limit`, `[model_providers.<id>]` with `name`, `base_url`,
//!     `env_key` and `wire_api = "responses"` (the only value); profiles are separate files
//!     next to `config.toml`, `~/.codex/<name>.config.toml`, used with `--profile <name>`
//!     (since Codex 0.134.0; the home is `%USERPROFILE%\.codex` on Windows).
//!   - The Claude app: Help → Troubleshooting → Enable Developer Mode, then Developer →
//!     Configure Third-Party Inference, Inference provider Gateway, Gateway base URL,
//!     Gateway API key, Gateway auth scheme Bearer. Not tried yet (it may want https).

use serde::Serialize;

use super::{SetupStep, SetupSystem, SetupWay, ShareApp, ShareSetup};

/// The environment variable Codex reads the sharing key from (`env_key`).
pub const CODEX_KEY_VARIABLE: &str = "HARNESSUI_SHARE_KEY";
/// The Codex provider id and profile name.
pub const CODEX_PROFILE: &str = "harnessui";
/// The context assumed when no model is loaded.
pub const ASSUMED_CTX: u32 = 32_768;

/// What the setup text is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupInput {
    pub port: u16,
    pub key: String,
    /// The model id the apps ask for (the fixed alias).
    pub model: String,
    /// The loaded model's context; `None` when no model is loaded.
    pub ctx: Option<u32>,
}

impl SetupInput {
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn ctx(&self) -> u32 {
        self.ctx.unwrap_or(ASSUMED_CTX)
    }

    /// A quarter of the context for the answer, between 1,024 and 8,192 tokens, so the
    /// rest is left for the conversation.
    fn output_tokens(&self) -> u32 {
        (self.ctx() / 4).clamp(1024, 8192)
    }
}

/// The card for `app`.
pub fn build(app: ShareApp, input: &SetupInput) -> ShareSetup {
    let (title, not_tried, ways) = match app {
        ShareApp::ClaudeCode => ("Claude Code", false, claude_code(input)),
        ShareApp::Codex => ("Codex", false, codex(input)),
        ShareApp::ClaudeApp => ("Claude app", true, claude_app(input)),
    };
    let mut notes = Vec::new();
    notes.push(format!(
        "Experimental: an update to {title} may stop this from working."
    ));
    if input.ctx.is_none() {
        notes.push(format!(
            "No model is loaded now, so this setup assumes the model can remember {} tokens (about {} words). Copy it again after you load a model.",
            super::proxy::thousands(u64::from(ASSUMED_CTX)),
            super::proxy::thousands(u64::from(about_words(ASSUMED_CTX)))
        ));
    }
    notes.extend([
        format!("To go back to your {title} account, remove these lines (or close the window)."),
        "Keep harnessUI open with a model loaded while you use it.".to_owned(),
        "This stores the key as plain text in that app's settings.".to_owned(),
        "While harnessUI is closed or sharing is off, another program on this computer could pretend to be harnessUI on this port and receive what that app sends, including the key.".to_owned(),
    ]);
    ShareSetup {
        app,
        title: title.to_owned(),
        not_tried,
        ways,
        notes,
    }
}

/// Three quarters of the tokens, rounded down to the thousand (as `words.ts` says it).
fn about_words(tokens: u32) -> u32 {
    let words = tokens / 4 * 3;
    if words >= 1000 {
        words / 1000 * 1000
    } else {
        words / 100 * 100
    }
}

fn step(text: &str, code: Option<String>, file: Option<&str>) -> SetupStep {
    SetupStep {
        text: text.to_owned(),
        code,
        file: file.map(str::to_owned),
    }
}

// ---- Claude Code -------------------------------------------------------------------

/// Claude Code's variables, in the order they are shown.
fn claude_code_env(input: &SetupInput) -> Vec<(&'static str, String)> {
    vec![
        ("ANTHROPIC_BASE_URL", input.base_url()),
        ("ANTHROPIC_AUTH_TOKEN", input.key.clone()),
        ("ANTHROPIC_MODEL", input.model.clone()),
        ("CLAUDE_CODE_MAX_CONTEXT_TOKENS", input.ctx().to_string()),
        ("CLAUDE_CODE_MAX_OUTPUT_TOKENS", input.output_tokens().to_string()),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_owned()),
    ]
}

/// The `env` block for Claude Code's settings file, as JSON.
pub fn claude_code_settings_json(input: &SetupInput) -> String {
    let env: serde_json::Map<String, serde_json::Value> = claude_code_env(input)
        .into_iter()
        .map(|(k, v)| (k.to_owned(), serde_json::Value::String(v)))
        .collect();
    let mut root = serde_json::Map::new();
    root.insert("env".into(), serde_json::Value::Object(env));
    serde_json::to_string_pretty(&serde_json::Value::Object(root)).unwrap_or_default()
}

fn claude_code(input: &SetupInput) -> Vec<SetupWay> {
    let env = claude_code_env(input);
    let powershell = lines(env.iter().map(|(k, v)| powershell_set(k, v)), "claude");
    let terminal = lines(env.iter().map(|(k, v)| sh_export(k, v)), "claude");
    let json = claude_code_settings_json(input);
    let file_way = |system: SetupSystem, path: &str| SetupWay {
        title: "For every window".into(),
        system,
        steps: vec![
            step(
                "Open this file in a text editor, or create it if it is not there. Paste these lines and save. If the file already has settings, add only the lines inside \"env\".",
                Some(json.clone()),
                Some(path),
            ),
            step("Then start Claude Code as usual.", None, None),
        ],
    };
    vec![
        SetupWay {
            title: "For this window only".into(),
            system: SetupSystem::Windows,
            steps: vec![step(
                "Open Windows PowerShell and paste these lines. They set Claude Code up for this window only and start it.",
                Some(powershell),
                None,
            )],
        },
        SetupWay {
            title: "For this window only".into(),
            system: SetupSystem::Mac,
            steps: vec![step(
                "Open Terminal and paste these lines. They set Claude Code up for this window only and start it.",
                Some(terminal),
                None,
            )],
        },
        file_way(SetupSystem::Windows, r"%USERPROFILE%\.claude\settings.json"),
        file_way(SetupSystem::Mac, "~/.claude/settings.json"),
    ]
}

// ---- Codex -------------------------------------------------------------------------

#[derive(Serialize)]
struct CodexConfig {
    model_provider: String,
    model: String,
    model_context_window: u32,
    model_auto_compact_token_limit: u32,
    model_providers: std::collections::BTreeMap<String, CodexProvider>,
}

#[derive(Serialize)]
struct CodexProvider {
    name: String,
    base_url: String,
    env_key: String,
    wire_api: String,
}

/// The Codex settings, as TOML: the same lines serve as a profile file and as lines for
/// `config.toml` (the plain keys come before the provider table).
pub fn codex_toml(input: &SetupInput) -> String {
    let ctx = input.ctx();
    let mut providers = std::collections::BTreeMap::new();
    providers.insert(
        CODEX_PROFILE.to_owned(),
        CodexProvider {
            name: "harnessUI".into(),
            base_url: format!("{}/v1", input.base_url()),
            env_key: CODEX_KEY_VARIABLE.into(),
            wire_api: "responses".into(),
        },
    );
    let config = CodexConfig {
        model_provider: CODEX_PROFILE.into(),
        model: input.model.clone(),
        model_context_window: ctx,
        // Compact at four fifths, leaving room for the answer.
        model_auto_compact_token_limit: ctx / 5 * 4,
        model_providers: providers,
    };
    toml::to_string(&config).unwrap_or_default()
}

fn codex(input: &SetupInput) -> Vec<SetupWay> {
    let toml = codex_toml(input);
    let run = format!("codex --profile {CODEX_PROFILE}");
    let profile_file = format!("{CODEX_PROFILE}.config.toml");
    let one_session = |system: SetupSystem, folder: &str, sep: char, key_line: String, shell: &str| SetupWay {
        title: "Only when you ask for it".into(),
        system,
        steps: vec![
            step(
                "Save these lines as a new file. Codex uses it only when you start it with this profile.",
                Some(toml.clone()),
                Some(&format!("{folder}{sep}{profile_file}")),
            ),
            step(
                &format!("Then open {shell} and paste these lines. They give Codex the key for this window only and start it with the profile."),
                Some(format!("{key_line}\n{run}")),
                None,
            ),
        ],
    };
    let every_time = |system: SetupSystem, folder: &str, sep: char, key_step: SetupStep| SetupWay {
        title: "Every time".into(),
        system,
        steps: vec![
            step(
                "Open this file in a text editor, or create it if it is not there. Paste these lines at the very top and save. If the file already has model, model_provider or model_context_window, change those lines instead of adding them again.",
                Some(toml.clone()),
                Some(&format!("{folder}{sep}config.toml")),
            ),
            key_step,
            step("Then start Codex as usual.", None, None),
        ],
    };
    vec![
        one_session(
            SetupSystem::Windows,
            r"%USERPROFILE%\.codex",
            '\\',
            powershell_set(CODEX_KEY_VARIABLE, &input.key),
            "Windows PowerShell",
        ),
        one_session(
            SetupSystem::Mac,
            "~/.codex",
            '/',
            sh_export(CODEX_KEY_VARIABLE, &input.key),
            "Terminal",
        ),
        every_time(
            SetupSystem::Windows,
            r"%USERPROFILE%\.codex",
            '\\',
            step(
                "Open Windows PowerShell and paste this line, so every new window has the key. Then close the window and open a new one.",
                Some(format!(
                    "[Environment]::SetEnvironmentVariable({}, {}, 'User')",
                    powershell_quote(CODEX_KEY_VARIABLE),
                    powershell_quote(&input.key)
                )),
                None,
            ),
        ),
        every_time(
            SetupSystem::Mac,
            "~/.codex",
            '/',
            step(
                "Add this line to the end of this file, so every new Terminal window has the key. Then open a new window.",
                Some(sh_export(CODEX_KEY_VARIABLE, &input.key)),
                Some("~/.zshrc"),
            ),
        ),
    ]
}

// ---- The Claude app ----------------------------------------------------------------

fn claude_app(input: &SetupInput) -> Vec<SetupWay> {
    vec![SetupWay {
        title: "In the Claude app".into(),
        system: SetupSystem::Any,
        steps: vec![
            step(
                "Open Help, then Troubleshooting, then Enable Developer Mode. The app restarts with a Developer menu.",
                None,
                None,
            ),
            step(
                "Open Developer, then Configure Third-Party Inference. Under Connection, set Inference provider to Gateway.",
                None,
                None,
            ),
            step("Set Gateway base URL to:", Some(clean(&input.base_url())), None),
            step("Set Gateway API key to:", Some(clean(&input.key)), None),
            step(
                "Set Credential kind to Static API key and Gateway auth scheme to Bearer, then save.",
                None,
                None,
            ),
            step(
                "To go back, open the same window and set Inference provider back to what it was.",
                None,
                None,
            ),
        ],
    }]
}

// ---- Shell quoting -----------------------------------------------------------------

fn lines(set: impl Iterator<Item = String>, last: &str) -> String {
    let mut all: Vec<String> = set.collect();
    all.push(last.to_owned());
    all.join("\n")
}

/// The value without control characters (a line break would split a pasted line).
fn clean(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

/// A PowerShell single-quoted string: nothing inside is expanded; a quote is doubled.
/// PowerShell also takes the typographic single quotes as quotes, so they are doubled too.
pub fn powershell_quote(value: &str) -> String {
    let mut out = String::from("'");
    for c in clean(value).chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// A POSIX single-quoted string: nothing inside is expanded; a quote ends the string, is
/// written escaped and the string starts again.
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", clean(value).replace('\'', r"'\''"))
}

/// `$env:NAME = 'value'` (PowerShell, this window only).
pub fn powershell_set(name: &str, value: &str) -> String {
    format!("$env:{name} = {}", powershell_quote(value))
}

/// `export NAME='value'` (zsh and bash, this window only).
pub fn sh_export(name: &str, value: &str) -> String {
    format!("export {name}={}", sh_quote(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "harnessui-share-0123456789abcdef0123456789abcdef";

    fn input() -> SetupInput {
        SetupInput {
            port: 51_234,
            key: KEY.into(),
            model: "harnessui-local".into(),
            ctx: Some(32_768),
        }
    }

    /// Model names a hostile model file or a bug could produce.
    const HOSTILE: &[&str] = &[
        "x\"\n[evil]\nkey = \"1",
        "x'\n$(rm -rf ~)\n'",
        "x\" ; Remove-Item -Recurse ~ ; \"",
        "x`whoami`",
        "x\u{2019}; Remove-Item ~; \u{2019}",
        "}\n{\"env\":{\"PATH\":\"/tmp\"}}",
        "x\r\nANTHROPIC_BASE_URL=http://evil",
        "x\\\"\\n",
    ];

    fn all_codes(setup: &ShareSetup) -> Vec<String> {
        setup
            .ways
            .iter()
            .flat_map(|w| w.steps.iter().filter_map(|s| s.code.clone()))
            .collect()
    }

    #[test]
    fn claude_code_settings_parse_and_hold_exactly_the_values() {
        let json: serde_json::Value = serde_json::from_str(&claude_code_settings_json(&input())).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"env": {
                "ANTHROPIC_BASE_URL": "http://127.0.0.1:51234",
                "ANTHROPIC_AUTH_TOKEN": KEY,
                "ANTHROPIC_MODEL": "harnessui-local",
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "32768",
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS": "8192",
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            }})
        );
    }

    #[test]
    fn codex_settings_parse_and_hold_exactly_the_values() {
        let parsed: toml::Table = toml::from_str(&codex_toml(&input())).unwrap();
        let expected: toml::Table = toml::from_str(
            r#"
            model_provider = "harnessui"
            model = "harnessui-local"
            model_context_window = 32768
            model_auto_compact_token_limit = 26212

            [model_providers.harnessui]
            name = "harnessUI"
            base_url = "http://127.0.0.1:51234/v1"
            env_key = "HARNESSUI_SHARE_KEY"
            wire_api = "responses"
            "#,
        )
        .unwrap();
        assert_eq!(parsed, expected);
        // The plain keys come first, so the lines can go at the top of config.toml.
        let text = codex_toml(&input());
        assert!(text.find("model_provider").unwrap() < text.find("[model_providers").unwrap());
        // The key itself is never in the Codex file: it comes from the variable.
        assert!(!text.contains(KEY));
    }

    #[test]
    fn a_hostile_model_name_injects_nothing() {
        for model in HOSTILE {
            let mut i = input();
            i.model = (*model).to_owned();
            let json: serde_json::Value = serde_json::from_str(&claude_code_settings_json(&i)).unwrap();
            let env = json["env"].as_object().unwrap();
            assert_eq!(json.as_object().unwrap().len(), 1, "{model:?}");
            assert_eq!(env.len(), 6, "{model:?}");
            assert_eq!(env["ANTHROPIC_MODEL"], serde_json::json!(model), "{model:?}");
            assert_eq!(env["ANTHROPIC_BASE_URL"], serde_json::json!("http://127.0.0.1:51234"));

            let parsed: toml::Table = toml::from_str(&codex_toml(&i)).unwrap();
            assert_eq!(parsed.len(), 5, "{model:?}");
            assert_eq!(parsed["model"].as_str(), Some(*model));
            let providers = parsed["model_providers"].as_table().unwrap();
            assert_eq!(providers.len(), 1);
            assert_eq!(providers["harnessui"].as_table().unwrap().len(), 4);

            // Shell lines: one line per variable plus the command, whatever the value.
            for app in [ShareApp::ClaudeCode, ShareApp::Codex, ShareApp::ClaudeApp] {
                let setup = build(app, &i);
                for code in all_codes(&setup) {
                    if code.starts_with("$env:") || code.starts_with("export ") {
                        for line in code.lines() {
                            assert!(
                                line.starts_with("$env:")
                                    || line.starts_with("export ")
                                    || line == "claude"
                                    || line == "codex --profile harnessui",
                                "{model:?}: {line:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Reads back a PowerShell single-quoted string the way PowerShell does.
    fn powershell_unquote(quoted: &str) -> String {
        let inner = quoted.strip_prefix('\'').unwrap().strip_suffix('\'').unwrap();
        let mut out = String::new();
        let mut chars = inner.chars().peekable();
        let is_quote = |c: char| matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}');
        while let Some(c) = chars.next() {
            if is_quote(c) {
                let next = chars.next().expect("a lone quote would end the string");
                assert!(is_quote(next), "a lone quote ends the string: {quoted:?}");
            }
            out.push(c);
        }
        out
    }

    /// Reads back a POSIX single-quoted word made of '…' and \' pieces.
    fn sh_unquote(quoted: &str) -> String {
        let mut out = String::new();
        let mut rest = quoted;
        while !rest.is_empty() {
            if let Some(r) = rest.strip_prefix("\\'") {
                out.push('\'');
                rest = r;
            } else {
                let r = rest.strip_prefix('\'').expect("a quoted piece");
                let end = r.find('\'').expect("the piece ends");
                out.push_str(&r[..end]);
                rest = &r[end + 1..];
            }
        }
        out
    }

    #[test]
    fn shell_quoting_round_trips() {
        for value in HOSTILE.iter().copied().chain([KEY, "", "plain", "it's", "''", "a\u{2018}b\u{201B}c"]) {
            let without_controls: String = value.chars().filter(|c| !c.is_control()).collect();
            let ps = powershell_quote(value);
            assert!(!ps.contains('\n') && !ps.contains('\r'));
            assert_eq!(powershell_unquote(&ps), without_controls, "{value:?}");
            let sh = sh_quote(value);
            assert!(!sh.contains('\n'));
            assert_eq!(sh_unquote(&sh), without_controls, "{value:?}");
        }
        assert_eq!(powershell_set("A", "it's"), "$env:A = 'it''s'");
        assert_eq!(sh_export("A", "it's"), r"export A='it'\''s'");
    }

    #[test]
    fn claude_code_card() {
        let setup = build(ShareApp::ClaudeCode, &input());
        assert_eq!(setup.title, "Claude Code");
        assert!(!setup.not_tried);
        // One-session ways first, Windows and Mac, then the settings files.
        let ways: Vec<(&str, SetupSystem)> = setup.ways.iter().map(|w| (w.title.as_str(), w.system)).collect();
        assert_eq!(
            ways,
            vec![
                ("For this window only", SetupSystem::Windows),
                ("For this window only", SetupSystem::Mac),
                ("For every window", SetupSystem::Windows),
                ("For every window", SetupSystem::Mac),
            ]
        );
        assert_eq!(
            setup.ways[0].steps[0].code.as_deref(),
            Some(concat!(
                "$env:ANTHROPIC_BASE_URL = 'http://127.0.0.1:51234'\n",
                "$env:ANTHROPIC_AUTH_TOKEN = 'harnessui-share-0123456789abcdef0123456789abcdef'\n",
                "$env:ANTHROPIC_MODEL = 'harnessui-local'\n",
                "$env:CLAUDE_CODE_MAX_CONTEXT_TOKENS = '32768'\n",
                "$env:CLAUDE_CODE_MAX_OUTPUT_TOKENS = '8192'\n",
                "$env:CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = '1'\n",
                "claude"
            ))
        );
        assert_eq!(
            setup.ways[1].steps[0].code.as_deref(),
            Some(concat!(
                "export ANTHROPIC_BASE_URL='http://127.0.0.1:51234'\n",
                "export ANTHROPIC_AUTH_TOKEN='harnessui-share-0123456789abcdef0123456789abcdef'\n",
                "export ANTHROPIC_MODEL='harnessui-local'\n",
                "export CLAUDE_CODE_MAX_CONTEXT_TOKENS='32768'\n",
                "export CLAUDE_CODE_MAX_OUTPUT_TOKENS='8192'\n",
                "export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1'\n",
                "claude"
            ))
        );
        assert_eq!(setup.ways[2].steps[0].file.as_deref(), Some(r"%USERPROFILE%\.claude\settings.json"));
        assert_eq!(setup.ways[3].steps[0].file.as_deref(), Some("~/.claude/settings.json"));
        assert!(setup.ways[2].steps[0].text.contains("If the file already has settings, add only the lines inside \"env\"."));
        assert_eq!(setup.ways[2].steps[0].code, Some(claude_code_settings_json(&input())));
    }

    #[test]
    fn codex_card() {
        let setup = build(ShareApp::Codex, &input());
        assert_eq!(setup.title, "Codex");
        let windows = &setup.ways[0];
        assert_eq!((windows.title.as_str(), windows.system), ("Only when you ask for it", SetupSystem::Windows));
        assert_eq!(windows.steps[0].file.as_deref(), Some(r"%USERPROFILE%\.codex\harnessui.config.toml"));
        assert_eq!(
            windows.steps[1].code.as_deref(),
            Some("$env:HARNESSUI_SHARE_KEY = 'harnessui-share-0123456789abcdef0123456789abcdef'\ncodex --profile harnessui")
        );
        let mac = &setup.ways[1];
        assert_eq!(mac.steps[0].file.as_deref(), Some("~/.codex/harnessui.config.toml"));
        assert_eq!(
            mac.steps[1].code.as_deref(),
            Some("export HARNESSUI_SHARE_KEY='harnessui-share-0123456789abcdef0123456789abcdef'\ncodex --profile harnessui")
        );
        assert_eq!(setup.ways[2].steps[0].file.as_deref(), Some(r"%USERPROFILE%\.codex\config.toml"));
        assert_eq!(
            setup.ways[2].steps[1].code.as_deref(),
            Some("[Environment]::SetEnvironmentVariable('HARNESSUI_SHARE_KEY', 'harnessui-share-0123456789abcdef0123456789abcdef', 'User')")
        );
        assert_eq!(setup.ways[3].steps[0].file.as_deref(), Some("~/.codex/config.toml"));
        assert_eq!(setup.ways[3].steps[1].file.as_deref(), Some("~/.zshrc"));
        for way in &setup.ways {
            assert_eq!(way.steps[0].code, Some(codex_toml(&input())));
        }
    }

    #[test]
    fn claude_app_card_is_not_tried() {
        let setup = build(ShareApp::ClaudeApp, &input());
        assert_eq!(setup.title, "Claude app");
        assert!(setup.not_tried);
        assert_eq!(setup.ways.len(), 1);
        assert_eq!(setup.ways[0].system, SetupSystem::Any);
        let codes = all_codes(&setup);
        assert_eq!(codes, vec!["http://127.0.0.1:51234".to_owned(), KEY.to_owned()]);
    }

    #[test]
    fn every_card_ends_with_the_four_notes() {
        for (app, name) in [
            (ShareApp::ClaudeCode, "Claude Code"),
            (ShareApp::Codex, "Codex"),
            (ShareApp::ClaudeApp, "Claude app"),
        ] {
            let setup = build(app, &input());
            let n = setup.notes.len();
            assert_eq!(
                setup.notes[n - 4..],
                [
                    format!("To go back to your {name} account, remove these lines (or close the window)."),
                    "Keep harnessUI open with a model loaded while you use it.".to_owned(),
                    "This stores the key as plain text in that app's settings.".to_owned(),
                    "While harnessUI is closed or sharing is off, another program on this computer could pretend to be harnessUI on this port and receive what that app sends, including the key.".to_owned(),
                ]
            );
            assert!(setup.notes[0].starts_with("Experimental"));
            for text in setup.notes.iter().chain(setup.ways.iter().flat_map(|w| w.steps.iter().map(|s| &s.text))) {
                assert!(!text.contains('!'), "{text}");
            }
        }
    }

    #[test]
    fn the_context_follows_the_loaded_model() {
        let mut small = input();
        small.ctx = Some(8192);
        let json: serde_json::Value = serde_json::from_str(&claude_code_settings_json(&small)).unwrap();
        assert_eq!(json["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], serde_json::json!("8192"));
        assert_eq!(json["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], serde_json::json!("2048"));
        let parsed: toml::Table = toml::from_str(&codex_toml(&small)).unwrap();
        assert_eq!(parsed["model_context_window"].as_integer(), Some(8192));
        assert_eq!(parsed["model_auto_compact_token_limit"].as_integer(), Some(6552));

        // None loaded: 32,768 is assumed and the card says so.
        let mut none = input();
        none.ctx = None;
        let setup = build(ShareApp::ClaudeCode, &none);
        assert!(setup.notes[1].contains("32,768 tokens (about 24,000 words)"), "{:?}", setup.notes);
        let json: serde_json::Value = serde_json::from_str(&claude_code_settings_json(&none)).unwrap();
        assert_eq!(json["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"], serde_json::json!("32768"));
        // A tiny context still leaves the answer some room.
        let mut tiny = input();
        tiny.ctx = Some(2048);
        assert_eq!(tiny.output_tokens(), 1024);
    }
}
