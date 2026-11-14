//! Best-effort import of Soundpad's registry settings and hotkeys.
//!
//! Source: `HKCU\Software\Leppsoft\Soundpad\MainFrame` [OBS, see
//! `docs/evidence/hkcu-leppsoft.txt`]. Only keys with a clear RustPad equivalent are
//! mapped; the modifier bitmask is applied with `rustpad_core::hotkey::from_soundpad`
//! (mapping is inferred, see EVIDENCE.md §6).

use std::collections::HashMap;

pub const SOUNDPAD_KEY: &str = r"Software\Leppsoft\Soundpad";

/// Reads the whole `MainFrame` and `Hotbar` groups as name -> string value.
pub fn read_soundpad_settings() -> Option<HashMap<String, String>> {
    imp::read_all().filter(|map| !map.is_empty())
}

/// True when a Soundpad installation/config was found.
pub fn is_available() -> bool {
    read_soundpad_settings().is_some()
}

#[cfg(windows)]
mod imp {
    use super::SOUNDPAD_KEY;
    use std::collections::HashMap;
    use winreg::enums::{RegType, HKEY_CURRENT_USER};
    use winreg::RegKey;

    /// Flattens a key tree into `"SubKey/Value" -> "text"`; root values are
    /// stored as plain `"Value"`.
    fn walk(key: &RegKey, prefix: &str, out: &mut HashMap<String, String>) {
        for (name, value) in key.enum_values().flatten() {
            let text = match value.vtype {
                RegType::REG_SZ => value.to_string(),
                RegType::REG_DWORD => value.to_string(),
                _ => continue,
            };
            let value_name = if name.is_empty() {
                "Value"
            } else {
                name.as_str()
            };
            let full = if prefix.is_empty() {
                value_name.to_string()
            } else {
                format!("{prefix}/{value_name}")
            };
            out.insert(full, text);
        }

        for sub in key.enum_keys().flatten() {
            let Ok(child) = key.open_subkey(&sub) else {
                continue;
            };
            let child_prefix = if prefix.is_empty() {
                sub.clone()
            } else {
                format!("{prefix}/{sub}")
            };
            walk(&child, &child_prefix, out);
        }
    }

    pub fn read_all() -> Option<HashMap<String, String>> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = hkcu.open_subkey(SOUNDPAD_KEY).ok()?;
        let mut out = HashMap::new();
        walk(&key, "", &mut out);
        Some(out)
    }
}

#[cfg(not(windows))]
mod imp {
    use std::collections::HashMap;

    pub fn read_all() -> Option<HashMap<String, String>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_is_best_effort_and_never_panics() {
        // Either Soundpad is installed (map with keys) or it is not (None); the
        // call must never panic and `is_available` must agree with the map.
        let settings = read_soundpad_settings();
        assert_eq!(is_available(), settings.is_some());
        if let Some(map) = settings {
            assert!(!map.is_empty());
        }
    }
}
