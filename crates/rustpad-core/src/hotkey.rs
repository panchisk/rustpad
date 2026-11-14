//! Hotkey model: modifiers + a Windows virtual-key code, with text round-trip.
//!
//! `Hotkey { modifiers: Modifiers::CTRL, key: Some(0x70) }` displays as `Ctrl+F1`.
//! A hotkey with `key == None` is a *modifier-only* hotkey (e.g. hold `Ctrl`),
//! which Soundpad uses for push-to-talk.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Bit flags: 1 = Shift, 2 = Ctrl, 4 = Alt, 8 = Win.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const NONE: Modifiers = Modifiers(0);
    pub const SHIFT: Modifiers = Modifiers(1);
    pub const CTRL: Modifiers = Modifiers(2);
    pub const ALT: Modifiers = Modifiers(4);
    pub const WIN: Modifiers = Modifiers(8);

    pub fn empty(self) -> bool {
        self.0 == 0
    }
    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }
    pub fn insert(&mut self, other: Modifiers) {
        self.0 |= other.0;
    }
    pub fn remove(&mut self, other: Modifiers) {
        self.0 &= !other.0;
    }
    pub fn union(self, other: Modifiers) -> Modifiers {
        Modifiers(self.0 | other.0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// Stable order used by `Display`: Ctrl, Alt, Shift, Win.
    pub fn tokens(self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.contains(Modifiers::CTRL) {
            out.push("Ctrl");
        }
        if self.contains(Modifiers::ALT) {
            out.push("Alt");
        }
        if self.contains(Modifiers::SHIFT) {
            out.push("Shift");
        }
        if self.contains(Modifiers::WIN) {
            out.push("Win");
        }
        out
    }
}

/// A global hotkey. `key` is a Windows virtual-key code (`VK_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Option<u32>,
}

impl Hotkey {
    pub fn new(modifiers: Modifiers, key: u32) -> Self {
        Hotkey {
            modifiers,
            key: Some(key),
        }
    }

    pub fn modifier_only(modifiers: Modifiers) -> Self {
        Hotkey {
            modifiers,
            key: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.modifiers.is_empty() && self.key.is_none()
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut modifiers = Modifiers::NONE;
        let mut key = None;
        for part in text.split('+') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "lctrl" | "rctrl" => modifiers.insert(Modifiers::CTRL),
                "alt" | "lalt" | "ralt" => modifiers.insert(Modifiers::ALT),
                "shift" | "lshift" | "rshift" => modifiers.insert(Modifiers::SHIFT),
                "win" | "meta" | "super" | "lwin" | "rwin" => modifiers.insert(Modifiers::WIN),
                _ => key = parse_vk(part),
            }
        }
        if modifiers.is_empty() && key.is_none() {
            return None;
        }
        Some(Hotkey { modifiers, key })
    }

    pub fn display(self) -> String {
        let mut parts = self
            .modifiers
            .tokens()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if let Some(k) = self.key {
            parts.push(vk_name(k));
        }
        parts.join("+")
    }

    pub fn matches(&self, modifiers: Modifiers, key: Option<u32>) -> bool {
        self.modifiers == modifiers && self.key == key
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

impl From<(Modifiers, u32)> for Hotkey {
    fn from((m, k): (Modifiers, u32)) -> Self {
        Hotkey::new(m, k)
    }
}

/// Best-effort conversion from Soundpad's `*KeyModifiers` registry bitmask.
/// The exact bit assignment was not recovered (see EVIDENCE.md §6); this mapping
/// (Shift=1, Ctrl=2, Alt=4, Win=8) matches the documented `Modifiers` bits and is
/// applied only when importing Soundpad settings.
pub fn from_soundpad(modifiers: u32, key: u32) -> Option<Hotkey> {
    if key == 0 {
        return None;
    }
    let mut m = Modifiers::NONE;
    if modifiers & 1 != 0 {
        m.insert(Modifiers::SHIFT);
    }
    if modifiers & 2 != 0 {
        m.insert(Modifiers::CTRL);
    }
    if modifiers & 4 != 0 {
        m.insert(Modifiers::ALT);
    }
    if modifiers & 8 != 0 {
        m.insert(Modifiers::WIN);
    }
    Some(Hotkey::new(m, key))
}

/// Names for the keys we support in hotkey strings. VK codes are Win32 values.
pub fn vk_name(vk: u32) -> String {
    match vk {
        0x01 => "Mouse1".into(),
        0x02 => "Mouse2".into(),
        0x04 => "Mouse3".into(),
        0x05 => "Mouse4".into(),
        0x06 => "Mouse5".into(),
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x13 => "Pause".into(),
        0x14 => "CapsLock".into(),
        0x1B => "Escape".into(),
        0x20 => "Space".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2C => "PrintScreen".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x5B => "LWin".into(),
        0x5C => "RWin".into(),
        0x5D => "Menu".into(),
        0x90 => "NumLock".into(),
        0x91 => "ScrollLock".into(),
        0xA0 => "LShift".into(),
        0xA1 => "RShift".into(),
        0xA2 => "LCtrl".into(),
        0xA3 => "RCtrl".into(),
        0xA4 => "LAlt".into(),
        0xA5 => "RAlt".into(),
        0x60..=0x69 => format!("Numpad{}", vk - 0x60),
        0x6A => "Numpad*".into(),
        0x6B => "Numpad+".into(),
        0x6D => "Numpad-".into(),
        0x6E => "Numpad.".into(),
        0x6F => "Numpad/".into(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x30..=0x39 => char::from_u32(vk)
            .map(|c| c.to_string())
            .unwrap_or_default(),
        0x41..=0x5A => char::from_u32(vk)
            .map(|c| c.to_string())
            .unwrap_or_default(),
        _ => format!("VK{vk:02X}"),
    }
}

/// Parses a key name (without modifiers) to a VK code.
pub fn parse_vk(name: &str) -> Option<u32> {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    match lower.as_str() {
        "backspace" | "bksp" => return Some(0x08),
        "tab" => return Some(0x09),
        "enter" | "return" => return Some(0x0D),
        "pause" | "break" => return Some(0x13),
        "capslock" | "caps" => return Some(0x14),
        "escape" | "esc" => return Some(0x1B),
        "space" | "spacebar" => return Some(0x20),
        "pageup" | "pgup" => return Some(0x21),
        "pagedown" | "pgdn" => return Some(0x22),
        "end" => return Some(0x23),
        "home" => return Some(0x24),
        "left" | "arrowleft" => return Some(0x25),
        "up" | "arrowup" => return Some(0x26),
        "right" | "arrowright" => return Some(0x27),
        "down" | "arrowdown" => return Some(0x28),
        "printscreen" | "prtsc" => return Some(0x2C),
        "insert" | "ins" => return Some(0x2D),
        "delete" | "del" => return Some(0x2E),
        "lwin" => return Some(0x5B),
        "rwin" => return Some(0x5C),
        "menu" | "apps" => return Some(0x5D),
        "numlock" => return Some(0x90),
        "scrolllock" | "scroll" => return Some(0x91),
        "lshift" => return Some(0xA0),
        "rshift" => return Some(0xA1),
        "lctrl" | "lcontrol" => return Some(0xA2),
        "rctrl" | "rcontrol" => return Some(0xA3),
        "lalt" => return Some(0xA4),
        "ralt" => return Some(0xA5),
        "numpad*" | "numpadmultiply" => return Some(0x6A),
        "numpad+" | "numpadplus" => return Some(0x6B),
        "numpad-" | "numpadminus" => return Some(0x6D),
        "numpad." | "numpaddecimal" => return Some(0x6E),
        "numpad/" | "numpaddivide" => return Some(0x6F),
        "mouse1" | "lmb" => return Some(0x01),
        "mouse2" | "rmb" => return Some(0x02),
        "mouse3" | "mmb" => return Some(0x04),
        "mouse4" => return Some(0x05),
        "mouse5" => return Some(0x06),
        _ => {}
    }
    if let Some(num) = lower.strip_prefix("numpad") {
        if let Ok(d) = num.parse::<u32>() {
            if d <= 9 {
                return Some(0x60 + d);
            }
        }
    }
    if let Some(f) = lower.strip_prefix('f') {
        if let Ok(d) = f.parse::<u32>() {
            if (1..=24).contains(&d) {
                return Some(0x6F + d);
            }
        }
    }
    if lower.len() == 1 {
        let c = lower.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(c.to_ascii_uppercase() as u32);
        }
        if c.is_ascii_digit() {
            return Some(c as u32);
        }
    }
    if let Some(hex) = lower.strip_prefix("vk") {
        return u32::from_str_radix(hex, 16).ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let h = Hotkey::parse("Ctrl+Shift+F1").unwrap();
        assert_eq!(h.modifiers, Modifiers(2 | 1));
        assert_eq!(h.key, Some(0x70));
        assert_eq!(h.display(), "Ctrl+Shift+F1");

        let h = Hotkey::parse("alt+1").unwrap();
        assert_eq!(h.display(), "Alt+1");
        assert_eq!(h.key, Some(0x31));

        let h = Hotkey::parse("Numpad7").unwrap();
        assert_eq!(h.key, Some(0x67));
        assert_eq!(h.display(), "Numpad7");

        assert!(Hotkey::parse("Ctrl").unwrap().is_empty() == false);
        assert!(Hotkey::parse("Ctrl").unwrap().key.is_none());
    }

    #[test]
    fn vk_roundtrip() {
        for name in ["A", "z", "0", "F12", "Numpad3", "Space", "Left", "Mouse4"] {
            let vk = parse_vk(name).unwrap();
            assert_eq!(vk_name(vk).to_ascii_lowercase(), name.to_ascii_lowercase());
        }
    }

    #[test]
    fn soundpad_import() {
        assert_eq!(
            from_soundpad(0, 97),
            Some(Hotkey::new(Modifiers::NONE, 0x61))
        );
        assert_eq!(from_soundpad(2, 0x70).unwrap().display(), "Ctrl+F1");
        assert!(from_soundpad(0, 0).is_none());
    }
}
