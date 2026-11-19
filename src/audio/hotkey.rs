//! The push-to-talk key: modifiers and one key, by its position on the
//! keyboard (W3C key codes, the same names iced and `global-hotkey` use), so
//! it does not change with the keyboard layout.
//!
//! Saved as `ctrl+shift+KeyV`; shown as `Ctrl+Shift+V`.

use std::fmt;

/// Keys a push-to-talk can use. Escape is not one: it cancels the choice.
const KEYS: &[&str] = &[
    "Backquote",
    "Backslash",
    "BracketLeft",
    "BracketRight",
    "Comma",
    "Equal",
    "IntlBackslash",
    "Minus",
    "Period",
    "Quote",
    "Semicolon",
    "Slash",
    "Space",
    "Tab",
    "CapsLock",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Pause",
    "ScrollLock",
    "NumLock",
    "NumpadAdd",
    "NumpadSubtract",
    "NumpadMultiply",
    "NumpadDivide",
    "NumpadDecimal",
    "NumpadEnter",
];

/// Whether `code` is a key a push-to-talk can use.
fn known(code: &str) -> bool {
    let one = |prefix: &str, ok: fn(u8) -> bool| {
        code.strip_prefix(prefix)
            .is_some_and(|rest| rest.len() == 1 && ok(rest.as_bytes()[0]))
    };
    let function_key = code
        .strip_prefix('F')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n) && code == format!("F{n}"));
    KEYS.contains(&code)
        || one("Key", |b| b.is_ascii_uppercase())
        || one("Digit", |b| b.is_ascii_digit())
        || one("Numpad", |b| b.is_ascii_digit())
        || function_key
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A push-to-talk key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    /// W3C key code (`KeyV`, `F13`, `Backquote`).
    pub code: String,
}

impl Hotkey {
    /// From the saved form (`ctrl+shift+KeyV`); modifiers in any case and
    /// order, the key last. `None` if anything is unknown.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() > 64 {
            return None;
        }
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let code = parts.pop()?;
        let mut modifiers = Modifiers::default();
        for m in parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.ctrl = true,
                "shift" => modifiers.shift = true,
                "alt" => modifiers.alt = true,
                _ => return None,
            }
        }
        known(code).then(|| Self {
            modifiers,
            code: code.to_string(),
        })
    }

    /// A key pressed in the window: `code` as iced names it.
    pub fn from_press(code: &str, modifiers: Modifiers) -> Option<Self> {
        known(code).then(|| Self {
            modifiers,
            code: code.to_string(),
        })
    }

    /// For people: `Ctrl+Shift+V`, `F13`, `Numpad 5`.
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.modifiers.ctrl {
            s.push_str("Ctrl+");
        }
        if self.modifiers.shift {
            s.push_str("Shift+");
        }
        if self.modifiers.alt {
            s.push_str("Alt+");
        }
        let key = match self.code.as_str() {
            c if c.starts_with("Key") => c[3..].to_string(),
            c if c.starts_with("Digit") => c[5..].to_string(),
            c if c.starts_with("Numpad") => format!("Numpad {}", &c[6..]),
            "Backquote" => "`".into(),
            "Space" => "Space".into(),
            c => c.to_string(),
        };
        s.push_str(&key);
        s
    }
}

/// The saved form.
impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.ctrl {
            f.write_str("ctrl+")?;
        }
        if self.modifiers.shift {
            f.write_str("shift+")?;
        }
        if self.modifiers.alt {
            f.write_str("alt+")?;
        }
        f.write_str(&self.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_read_and_written_back() {
        let k = Hotkey::parse("Shift+CTRL+KeyV").unwrap();
        assert_eq!(k.to_string(), "ctrl+shift+KeyV");
        assert_eq!(k.label(), "Ctrl+Shift+V");
        assert_eq!(Hotkey::parse("F13").unwrap().label(), "F13");
        assert_eq!(Hotkey::parse("alt+Numpad5").unwrap().label(), "Alt+Numpad 5");
        assert_eq!(Hotkey::parse("Backquote").unwrap().label(), "`");
        for bad in [
            "",
            "ctrl+",
            "Escape",
            "win+KeyA",
            "KeyAA",
            "F25",
            "F0",
            "Digit",
            "ctrl+KeyA+KeyB",
            "Keya",
        ] {
            assert_eq!(Hotkey::parse(bad), None, "{bad}");
        }
        assert!(Hotkey::parse(&"ctrl+".repeat(40)).is_none());
        let m = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        assert_eq!(Hotkey::from_press("Digit4", m).unwrap().to_string(), "alt+Digit4");
        assert!(Hotkey::from_press("Escape", m).is_none());
    }
}
