//! Key names the model may use with `browser_press_key`, mapped to CDP
//! `Input.dispatchKeyEvent` parameters. Pure logic.

use serde_json::{Value, json};

/// Modifier bits as CDP numbers them.
pub const ALT: i64 = 1;
/// Control.
pub const CTRL: i64 = 2;
/// Meta (Command on macOS).
pub const META: i64 = 4;
/// Shift.
pub const SHIFT: i64 = 8;

/// One key press, ready for CDP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    /// Modifier bits.
    pub modifiers: i64,
    /// DOM `key` value.
    pub key: String,
    /// DOM `code` value.
    pub code: String,
    /// Windows virtual key code (CDP uses it for editing commands).
    pub vk: i64,
    /// Text the key inserts, if any (none when Control, Alt or Meta is held).
    pub text: Option<String>,
}

impl KeyPress {
    /// `keyDown` (or `rawKeyDown` when the key inserts no text) and `keyUp` parameters.
    pub fn events(&self) -> [Value; 2] {
        let down_type = if self.text.is_some() {
            "keyDown"
        } else {
            "rawKeyDown"
        };
        let mut down = json!({
            "type": down_type,
            "modifiers": self.modifiers,
            "key": self.key,
            "code": self.code,
            "windowsVirtualKeyCode": self.vk,
            "nativeVirtualKeyCode": self.vk,
        });
        if let Some(text) = &self.text {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        let up = json!({
            "type": "keyUp",
            "modifiers": self.modifiers,
            "key": self.key,
            "code": self.code,
            "windowsVirtualKeyCode": self.vk,
            "nativeVirtualKeyCode": self.vk,
        });
        [down, up]
    }

    /// The base key name, for intent checks.
    pub fn is_enter(&self) -> bool {
        self.key == "Enter"
    }

    /// The clipboard command this key combination runs in the page, if any. Key events
    /// sent over CDP are trusted input, so the page's editor runs them: paste reads the
    /// user's own clipboard into the page (it may hold a password), and copy or cut
    /// overwrite it with page content. The browser tool refuses all of them.
    pub fn clipboard_command(&self) -> Option<&'static str> {
        let command_key = self.modifiers & (CTRL | META) != 0;
        match self.key.to_ascii_lowercase().as_str() {
            "v" if command_key => Some("paste"),
            "c" if command_key => Some("copy"),
            "x" if command_key => Some("cut"),
            "insert" if self.modifiers & SHIFT != 0 => Some("paste"),
            "insert" if command_key => Some("copy"),
            "delete" if self.modifiers & SHIFT != 0 => Some("cut"),
            _ => None,
        }
    }
}

/// Named keys: (name, key, code, virtual key code, text).
const NAMED: &[(&str, &str, &str, i64, Option<&str>)] = &[
    ("enter", "Enter", "Enter", 13, Some("\r")),
    ("return", "Enter", "Enter", 13, Some("\r")),
    ("tab", "Tab", "Tab", 9, None),
    ("escape", "Escape", "Escape", 27, None),
    ("esc", "Escape", "Escape", 27, None),
    ("backspace", "Backspace", "Backspace", 8, None),
    ("delete", "Delete", "Delete", 46, None),
    ("space", " ", "Space", 32, Some(" ")),
    ("arrowup", "ArrowUp", "ArrowUp", 38, None),
    ("arrowdown", "ArrowDown", "ArrowDown", 40, None),
    ("arrowleft", "ArrowLeft", "ArrowLeft", 37, None),
    ("arrowright", "ArrowRight", "ArrowRight", 39, None),
    ("up", "ArrowUp", "ArrowUp", 38, None),
    ("down", "ArrowDown", "ArrowDown", 40, None),
    ("left", "ArrowLeft", "ArrowLeft", 37, None),
    ("right", "ArrowRight", "ArrowRight", 39, None),
    ("home", "Home", "Home", 36, None),
    ("end", "End", "End", 35, None),
    ("pageup", "PageUp", "PageUp", 33, None),
    ("pagedown", "PageDown", "PageDown", 34, None),
    ("insert", "Insert", "Insert", 45, None),
];

/// Parses `Enter`, `Shift+Tab`, `Control+a`, `PageDown`, `F5` and single characters.
/// Modifiers: `Control`/`Ctrl`, `Shift`, `Alt`/`Option`, `Meta`/`Cmd`/`Command`.
pub fn parse_key(combo: &str) -> Result<KeyPress, String> {
    let combo = combo.trim();
    if combo.is_empty() {
        return Err("key is empty".into());
    }
    // A lone "+" is the plus key, and "Control++" ends with it.
    let (mods_part, key_part) = if combo == "+" {
        ("", "+")
    } else if let Some(stripped) = combo.strip_suffix("++") {
        (stripped, "+")
    } else {
        match combo.rsplit_once('+') {
            Some((m, k)) => (m, k),
            None => ("", combo),
        }
    };
    // "Shift + Enter" names Enter too; the intent check reads keys through this function,
    // so what is judged is always what is pressed.
    let key_part = if key_part.trim().is_empty() {
        key_part
    } else {
        key_part.trim()
    };
    let mut modifiers = 0;
    for m in mods_part.split('+').filter(|m| !m.trim().is_empty()) {
        modifiers |= match m.trim().to_ascii_lowercase().as_str() {
            "control" | "ctrl" => CTRL,
            "shift" => SHIFT,
            "alt" | "option" => ALT,
            "meta" | "cmd" | "command" | "super" => META,
            other => return Err(format!("unknown modifier {other:?}")),
        };
    }
    let text_allowed = modifiers & (CTRL | ALT | META) == 0;
    let lower = key_part.to_ascii_lowercase();
    if let Some(&(_, key, code, vk, text)) = NAMED.iter().find(|(n, ..)| *n == lower) {
        return Ok(KeyPress {
            modifiers,
            key: key.into(),
            code: code.into(),
            vk,
            text: text.filter(|_| text_allowed).map(str::to_string),
        });
    }
    if let Some(n) = lower.strip_prefix('f')
        && let Ok(n) = n.parse::<i64>()
        && (1..=12).contains(&n)
    {
        return Ok(KeyPress {
            modifiers,
            key: format!("F{n}"),
            code: format!("F{n}"),
            vk: 111 + n,
            text: None,
        });
    }
    let mut chars = key_part.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return Err(format!(
            "unknown key {key_part:?}; use a single character or a name such as Enter, Tab, Escape, Backspace, ArrowDown, PageDown or F5"
        ));
    };
    let shifted = modifiers & SHIFT != 0;
    let shown = if c.is_ascii_alphabetic() && shifted {
        c.to_ascii_uppercase()
    } else {
        c
    };
    let (code, vk) = if c.is_ascii_alphabetic() {
        let up = c.to_ascii_uppercase();
        (format!("Key{up}"), up as i64)
    } else if c.is_ascii_digit() {
        (format!("Digit{c}"), c as i64)
    } else {
        (String::new(), 0)
    };
    Ok(KeyPress {
        modifiers,
        key: shown.to_string(),
        code,
        vk,
        text: text_allowed.then(|| shown.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys() {
        let k = parse_key("Enter").unwrap();
        assert_eq!(
            (k.key.as_str(), k.code.as_str(), k.vk),
            ("Enter", "Enter", 13)
        );
        assert_eq!(k.text.as_deref(), Some("\r"));
        assert!(k.is_enter());
        let k = parse_key("pagedown").unwrap();
        assert_eq!(k.key, "PageDown");
        assert_eq!(k.text, None);
        assert_eq!(parse_key("F5").unwrap().vk, 116);
        assert_eq!(parse_key("Space").unwrap().text.as_deref(), Some(" "));
    }

    #[test]
    fn modifiers_and_characters() {
        let k = parse_key("Control+a").unwrap();
        assert_eq!(k.modifiers, CTRL);
        assert_eq!(k.code, "KeyA");
        assert_eq!(k.vk, 65);
        assert_eq!(k.text, None, "no text with Control held");
        let k = parse_key("Shift+Tab").unwrap();
        assert_eq!(k.modifiers, SHIFT);
        assert_eq!(k.key, "Tab");
        let k = parse_key("shift+a").unwrap();
        assert_eq!(k.key, "A");
        assert_eq!(k.text.as_deref(), Some("A"));
        let k = parse_key("Ctrl+Shift+Alt+Meta+x").unwrap();
        assert_eq!(k.modifiers, CTRL | SHIFT | ALT | META);
        let k = parse_key("+").unwrap();
        assert_eq!(k.key, "+");
        let k = parse_key("Control++").unwrap();
        assert_eq!((k.modifiers, k.key.as_str()), (CTRL, "+"));
        let k = parse_key("7").unwrap();
        assert_eq!(k.code, "Digit7");
    }

    #[test]
    fn spaces_around_names_are_ignored() {
        for combo in [" Enter", "Enter ", "Shift + Enter", "control+ enter "] {
            assert_eq!(parse_key(combo).unwrap().key, "Enter", "{combo:?}");
        }
        assert_eq!(parse_key("Shift + Tab").unwrap().modifiers, SHIFT);
    }

    #[test]
    fn clipboard_shortcuts_are_recognised() {
        for (combo, want) in [
            ("Control+v", Some("paste")),
            ("Ctrl+Shift+V", Some("paste")),
            ("Meta+v", Some("paste")),
            ("Shift+Insert", Some("paste")),
            ("Control+c", Some("copy")),
            ("Cmd+C", Some("copy")),
            ("Control+Insert", Some("copy")),
            ("Control+x", Some("cut")),
            ("Shift+Delete", Some("cut")),
            ("v", None),
            ("Shift+v", None),
            ("Alt+v", None),
            ("Control+a", None),
            ("Delete", None),
            ("Insert", None),
        ] {
            assert_eq!(
                parse_key(combo).unwrap().clipboard_command(),
                want,
                "{combo}"
            );
        }
    }

    #[test]
    fn bad_keys_are_explained() {
        assert!(parse_key("").is_err());
        assert!(parse_key("Hyper+a").unwrap_err().contains("modifier"));
        assert!(parse_key("Banana").unwrap_err().contains("Enter"));
    }

    #[test]
    fn events_pair_down_and_up() {
        let [down, up] = parse_key("Enter").unwrap().events();
        assert_eq!(down["type"], "keyDown");
        assert_eq!(down["text"], "\r");
        assert_eq!(up["type"], "keyUp");
        let [down, _] = parse_key("ArrowDown").unwrap().events();
        assert_eq!(down["type"], "rawKeyDown");
        assert!(down.get("text").is_none());
    }
}
