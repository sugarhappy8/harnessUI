//! Keys and key combinations for `computer_key`, and the shortcuts the OS handles itself.
//!
//! A combination is written `ctrl+shift+t`, `Enter`, `alt+F4` or `cmd+space`: modifiers
//! first, then at most one other key, joined by `+` (spaces around `+` are allowed). Names
//! are case-insensitive and accept the common spellings models use (xdotool's `Return`,
//! `Page_Down`, `BackSpace`; browser-style `ArrowLeft`; `cmd`, `command`, `win`, `super`,
//! `meta`; `option`, `opt`).

use serde::{Deserialize, Serialize};

/// A key the injector can press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Key {
    Shift,
    Control,
    /// Alt on Windows, Option on macOS.
    Alt,
    /// The Windows key on Windows, Command on macOS.
    Meta,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    CapsLock,
    PrintScreen,
    /// F1 to F20.
    F(u8),
    /// A printable character, sent as the key that types it on the current layout.
    Char(char),
}

impl Key {
    pub fn is_modifier(self) -> bool {
        matches!(self, Key::Shift | Key::Control | Key::Alt | Key::Meta)
    }

    /// Every modifier, in the order they are released on Stop.
    pub const MODIFIERS: [Key; 4] = [Key::Shift, Key::Control, Key::Alt, Key::Meta];

    /// A readable name for cards ("Ctrl", "Enter", "F4", "T").
    pub fn label(self, mac: bool) -> String {
        match self {
            Key::Shift => "Shift".into(),
            Key::Control => "Ctrl".into(),
            Key::Alt => if mac { "Option" } else { "Alt" }.into(),
            Key::Meta => if mac { "Cmd" } else { "Win" }.into(),
            Key::Enter => "Enter".into(),
            Key::Tab => "Tab".into(),
            Key::Escape => "Esc".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Delete".into(),
            Key::Space => "Space".into(),
            Key::Up => "Up".into(),
            Key::Down => "Down".into(),
            Key::Left => "Left".into(),
            Key::Right => "Right".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PageUp".into(),
            Key::PageDown => "PageDown".into(),
            Key::Insert => "Insert".into(),
            Key::CapsLock => "CapsLock".into(),
            Key::PrintScreen => "PrintScreen".into(),
            Key::F(n) => format!("F{n}"),
            Key::Char(c) => c.to_uppercase().collect(),
        }
    }
}

/// Modifiers (in the order written) plus at most one other key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyCombo {
    pub modifiers: Vec<Key>,
    pub key: Option<Key>,
}

impl KeyCombo {
    /// Keys in press order (release order is the reverse).
    pub fn press_order(&self) -> Vec<Key> {
        let mut v = self.modifiers.clone();
        v.extend(self.key);
        v
    }

    pub fn label(&self, mac: bool) -> String {
        self.press_order()
            .iter()
            .map(|k| k.label(mac))
            .collect::<Vec<_>>()
            .join("+")
    }

    pub(crate) fn has(&self, k: Key) -> bool {
        self.modifiers.contains(&k)
    }
}

fn named(name: &str) -> Option<Key> {
    let n = name.to_ascii_lowercase().replace(['_', '-'], "");
    Some(match n.as_str() {
        "shift" | "lshift" | "rshift" | "shiftl" | "shiftr" => Key::Shift,
        "ctrl" | "control" | "lctrl" | "rctrl" | "controll" | "controlr" | "ctrll" | "ctrlr" => {
            Key::Control
        }
        "alt" | "option" | "opt" | "lalt" | "ralt" | "altl" | "altr" | "menu" => Key::Alt,
        "cmd" | "command" | "meta" | "super" | "win" | "windows" | "superl" | "superr"
        | "metal" | "metar" | "lwin" | "rwin" | "os" => Key::Meta,
        "enter" | "return" | "kpenter" => Key::Enter,
        "tab" => Key::Tab,
        "esc" | "escape" => Key::Escape,
        "backspace" | "back" => Key::Backspace,
        "delete" | "del" | "forwarddelete" => Key::Delete,
        "space" | "spacebar" => Key::Space,
        "up" | "arrowup" | "uparrow" => Key::Up,
        "down" | "arrowdown" | "downarrow" => Key::Down,
        "left" | "arrowleft" | "leftarrow" => Key::Left,
        "right" | "arrowright" | "rightarrow" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" | "prior" => Key::PageUp,
        "pagedown" | "pgdn" | "next" => Key::PageDown,
        "insert" | "ins" => Key::Insert,
        "capslock" | "caps" => Key::CapsLock,
        "printscreen" | "print" | "prtsc" | "prtscn" | "snapshot" => Key::PrintScreen,
        "plus" => Key::Char('+'),
        "minus" => Key::Char('-'),
        "comma" => Key::Char(','),
        "period" => Key::Char('.'),
        "slash" => Key::Char('/'),
        "backslash" => Key::Char('\\'),
        "semicolon" => Key::Char(';'),
        "equal" | "equals" => Key::Char('='),
        _ => {
            if let Some(num) = n.strip_prefix('f')
                && let Ok(v) = num.parse::<u8>()
                && (1..=20).contains(&v)
            {
                return Some(Key::F(v));
            }
            return None;
        }
    })
}

/// Parses a combination such as `ctrl+shift+t`. Errors are written for the model.
pub fn parse_combo(text: &str) -> Result<KeyCombo, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("keys is empty; write a key such as \"Enter\" or \"ctrl+c\"".into());
    }
    if text.chars().count() > 64 {
        return Err("keys is too long; send one combination per call".into());
    }
    // Split on '+'; "+" alone and a trailing "++" (as in "ctrl++") are the plus key.
    let parts: Vec<&str> = if text == "+" {
        vec!["+"]
    } else if let Some(prefix) = text.strip_suffix("++") {
        prefix
            .split('+')
            .map(str::trim)
            .chain(std::iter::once("+"))
            .collect()
    } else {
        text.split('+').map(str::trim).collect()
    };
    if parts.iter().any(|p| p.is_empty()) {
        return Err(format!(
            "\"{text}\" has an empty key name; write keys as \"ctrl+c\""
        ));
    }
    let mut keys = Vec::with_capacity(parts.len());
    for p in &parts {
        let mut it = p.chars();
        let key = match (it.next(), it.next()) {
            (Some(c), None) if !c.is_control() && !c.is_whitespace() => {
                Key::Char(c.to_ascii_lowercase())
            }
            _ => named(p).ok_or_else(|| {
                format!("unknown key \"{p}\"; use names such as ctrl, shift, alt, cmd, Enter, Tab, Escape, Backspace, Delete, Up, PageDown, F5 or a single character")
            })?,
        };
        keys.push(key);
    }
    let mut modifiers: Vec<Key> = Vec::new();
    let mut key = None;
    for (i, k) in keys.iter().enumerate() {
        let last = i + 1 == keys.len();
        if k.is_modifier() {
            // Modifiers only ("shift", "ctrl+shift") press just those.
            if modifiers.contains(k) {
                return Err(format!("{} is written twice", k.label(false)));
            }
            modifiers.push(*k);
        } else if last {
            key = Some(*k);
        } else {
            return Err(format!(
                "\"{text}\" has a non-modifier key before the end; write modifiers first and send one combination per call"
            ));
        }
    }
    Ok(KeyCombo { modifiers, key })
}

/// Why a combination is handled by the operating system rather than the application in
/// front, if it is. Such shortcuts act outside the granted application (open the Run
/// box, switch applications, lock the screen…), so they always ask.
pub fn system_shortcut(combo: &KeyCombo, mac: bool) -> Option<&'static str> {
    let key = combo.key;
    if mac {
        match key {
            Some(Key::Space) if combo.has(Key::Meta) || combo.has(Key::Control) => {
                return Some("opens Spotlight or switches input source");
            }
            Some(Key::Tab) if combo.has(Key::Meta) => return Some("switches applications"),
            Some(Key::Escape) if combo.has(Key::Meta) && combo.has(Key::Alt) => {
                return Some("opens Force Quit");
            }
            Some(Key::Char('q')) if combo.has(Key::Meta) && combo.has(Key::Control) => {
                return Some("locks the screen");
            }
            Some(Key::Char('q')) if combo.has(Key::Meta) && combo.has(Key::Shift) => {
                return Some("logs out");
            }
            Some(Key::Up | Key::Down | Key::Left | Key::Right) if combo.has(Key::Control) => {
                return Some("switches Spaces or opens Mission Control");
            }
            Some(Key::F(3)) | Some(Key::F(4)) => {
                return Some("opens Mission Control or Launchpad");
            }
            Some(Key::Delete | Key::Backspace)
                if combo.has(Key::Control) && combo.has(Key::Alt) =>
            {
                return Some("is the stop hotkey or a system shortcut");
            }
            _ => {}
        }
        if key == Some(Key::Char('h')) && combo.has(Key::Meta) && combo.has(Key::Alt) {
            return Some("hides every other application");
        }
        None
    } else {
        if combo.has(Key::Meta) || key == Some(Key::Meta) {
            return Some("uses the Windows key, which the Windows shell handles");
        }
        match key {
            Some(Key::Tab) if combo.has(Key::Alt) => Some("switches applications"),
            Some(Key::Escape) if combo.has(Key::Control) || combo.has(Key::Alt) => {
                Some("opens Start, Task Manager or switches windows")
            }
            Some(Key::Delete | Key::Backspace)
                if combo.has(Key::Control) && combo.has(Key::Alt) =>
            {
                Some("is the stop hotkey or a system shortcut")
            }
            Some(Key::Char('l')) if combo.has(Key::Control) && combo.has(Key::Alt) => {
                Some("locks the screen")
            }
            Some(Key::PrintScreen) => Some("captures the screen"),
            _ => None,
        }
    }
}

/// Splits text for `computer_type`: printable runs are typed as text, `\n` becomes
/// Enter and `\t` becomes Tab. Other control characters are refused (they would type
/// nothing visible or something unexpected). Runs are at most `chunk` characters, so the
/// target is re-checked often while typing.
pub fn text_events(text: &str, chunk: usize) -> Result<Vec<TextPart>, String> {
    let chunk = chunk.max(1);
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut count = 0usize;
    let normalised = text.replace("\r\n", "\n");
    for c in normalised.chars() {
        let special = match c {
            '\n' => Some(Key::Enter),
            '\t' => Some(Key::Tab),
            '\r' => Some(Key::Enter),
            c if c.is_control() => {
                return Err(format!(
                    "the text contains the control character U+{:04X}; only newlines and tabs are allowed",
                    c as u32
                ));
            }
            _ => None,
        };
        if let Some(k) = special {
            if !cur.is_empty() {
                parts.push(TextPart::Text(std::mem::take(&mut cur)));
                count = 0;
            }
            parts.push(TextPart::Key(k));
            continue;
        }
        cur.push(c);
        count += 1;
        if count >= chunk {
            parts.push(TextPart::Text(std::mem::take(&mut cur)));
            count = 0;
        }
    }
    if !cur.is_empty() {
        parts.push(TextPart::Text(cur));
    }
    Ok(parts)
}

/// One step of typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextPart {
    Text(String),
    Key(Key),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(m: &[Key], k: Option<Key>) -> KeyCombo {
        KeyCombo {
            modifiers: m.to_vec(),
            key: k,
        }
    }

    #[test]
    fn parses_common_spellings() {
        assert_eq!(
            parse_combo("ctrl+shift+t").unwrap(),
            combo(&[Key::Control, Key::Shift], Some(Key::Char('t')))
        );
        assert_eq!(
            parse_combo("Ctrl + C").unwrap(),
            combo(&[Key::Control], Some(Key::Char('c')))
        );
        assert_eq!(parse_combo("Return").unwrap(), combo(&[], Some(Key::Enter)));
        assert_eq!(parse_combo("enter").unwrap(), combo(&[], Some(Key::Enter)));
        assert_eq!(
            parse_combo("alt+F4").unwrap(),
            combo(&[Key::Alt], Some(Key::F(4)))
        );
        assert_eq!(
            parse_combo("cmd+space").unwrap(),
            combo(&[Key::Meta], Some(Key::Space))
        );
        assert_eq!(
            parse_combo("command+option+esc").unwrap(),
            combo(&[Key::Meta, Key::Alt], Some(Key::Escape))
        );
        assert_eq!(
            parse_combo("Page_Down").unwrap(),
            combo(&[], Some(Key::PageDown))
        );
        assert_eq!(
            parse_combo("BackSpace").unwrap(),
            combo(&[], Some(Key::Backspace))
        );
        assert_eq!(
            parse_combo("ArrowLeft").unwrap(),
            combo(&[], Some(Key::Left))
        );
        assert_eq!(parse_combo("a").unwrap(), combo(&[], Some(Key::Char('a'))));
        assert_eq!(
            parse_combo("ctrl+A").unwrap(),
            combo(&[Key::Control], Some(Key::Char('a')))
        );
        assert_eq!(
            parse_combo("ctrl++").unwrap(),
            combo(&[Key::Control], Some(Key::Char('+')))
        );
        assert_eq!(parse_combo("+").unwrap(), combo(&[], Some(Key::Char('+'))));
        assert_eq!(
            parse_combo("ctrl+plus").unwrap(),
            combo(&[Key::Control], Some(Key::Char('+')))
        );
        assert_eq!(
            parse_combo("ctrl+/").unwrap(),
            combo(&[Key::Control], Some(Key::Char('/')))
        );
        assert_eq!(parse_combo("F12").unwrap(), combo(&[], Some(Key::F(12))));
        // A lone modifier is a key press of that modifier.
        assert_eq!(parse_combo("shift").unwrap(), combo(&[Key::Shift], None));
        assert_eq!(
            parse_combo("ctrl+shift").unwrap(),
            combo(&[Key::Control, Key::Shift], None)
        );
    }

    #[test]
    fn rejects_unknown_and_ambiguous_input() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("hyperdrive").is_err());
        assert!(parse_combo("F25").is_err());
        assert!(parse_combo("ctrl+a+b").is_err());
        assert!(parse_combo("a+ctrl").is_err());
        assert!(parse_combo("ctrl+ctrl+c").is_err());
        assert!(parse_combo(&"a+".repeat(40)).is_err());
        let err = parse_combo("ctrl+foo").unwrap_err();
        assert!(err.contains("unknown key \"foo\""), "{err}");
    }

    #[test]
    fn press_order_and_labels() {
        let c = parse_combo("ctrl+shift+t").unwrap();
        assert_eq!(
            c.press_order(),
            vec![Key::Control, Key::Shift, Key::Char('t')]
        );
        assert_eq!(c.label(false), "Ctrl+Shift+T");
        assert_eq!(
            parse_combo("cmd+alt+x").unwrap().label(true),
            "Cmd+Option+X"
        );
    }

    #[test]
    fn system_shortcuts_are_detected() {
        let p = |s: &str| parse_combo(s).unwrap();
        // Windows.
        assert!(system_shortcut(&p("win+r"), false).is_some());
        assert!(system_shortcut(&p("win"), false).is_some());
        assert!(system_shortcut(&p("alt+tab"), false).is_some());
        assert!(system_shortcut(&p("ctrl+shift+esc"), false).is_some());
        assert!(system_shortcut(&p("ctrl+esc"), false).is_some());
        assert!(system_shortcut(&p("ctrl+alt+backspace"), false).is_some());
        assert!(system_shortcut(&p("printscreen"), false).is_some());
        assert!(system_shortcut(&p("ctrl+c"), false).is_none());
        assert!(system_shortcut(&p("alt+f4"), false).is_none());
        assert!(system_shortcut(&p("enter"), false).is_none());
        // macOS.
        assert!(system_shortcut(&p("cmd+space"), true).is_some());
        assert!(system_shortcut(&p("cmd+tab"), true).is_some());
        assert!(system_shortcut(&p("cmd+option+esc"), true).is_some());
        assert!(system_shortcut(&p("ctrl+cmd+q"), true).is_some());
        assert!(system_shortcut(&p("ctrl+left"), true).is_some());
        assert!(system_shortcut(&p("ctrl+option+delete"), true).is_some());
        assert!(system_shortcut(&p("cmd+c"), true).is_none());
        assert!(system_shortcut(&p("cmd+q"), true).is_none());
        assert!(system_shortcut(&p("cmd+shift+t"), true).is_none());
    }

    #[test]
    fn text_is_split_into_short_runs_and_keys() {
        assert_eq!(
            text_events("ab\ncd\te", 8).unwrap(),
            vec![
                TextPart::Text("ab".into()),
                TextPart::Key(Key::Enter),
                TextPart::Text("cd".into()),
                TextPart::Key(Key::Tab),
                TextPart::Text("e".into()),
            ]
        );
        assert_eq!(
            text_events("abcdefghij", 4).unwrap(),
            vec![
                TextPart::Text("abcd".into()),
                TextPart::Text("efgh".into()),
                TextPart::Text("ij".into()),
            ]
        );
        assert_eq!(
            text_events("a\r\nb", 8).unwrap(),
            vec![
                TextPart::Text("a".into()),
                TextPart::Key(Key::Enter),
                TextPart::Text("b".into()),
            ]
        );
        // Multi-byte characters are counted as characters.
        assert_eq!(text_events("日本語テキスト", 3).unwrap().len(), 3);
        assert!(text_events("bell\u{7}", 8).is_err());
        assert!(text_events("esc\u{1b}[", 8).is_err());
        assert_eq!(text_events("", 8).unwrap(), vec![]);
    }
}
