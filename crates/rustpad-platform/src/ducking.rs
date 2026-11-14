//! Volume ducking of other applications while a sound plays.
//!
//! Windows exposes a per-user communication ducking preference at
//! `HKCU\Software\Microsoft\Multimedia\Audio\UserDuckingPreference` [OBS: Soundpad
//! stores `windowsDuckingPreference=2147483647`]. Values understood by Windows:
//! `0` = mute others, `1` = reduce 80 %, `2` = reduce 50 %, `3` = do nothing.
//!
//! RustPad toggles the preference while a microphone-bound sound is playing and
//! restores the previous value on stop / exit.

use anyhow::Result;

pub const AUDIO_KEY: &str = r"Software\Microsoft\Multimedia\Audio";
pub const VALUE_NAME: &str = "UserDuckingPreference";
/// Windows' default when the value is not configured: "do nothing".
pub const DEFAULT_VALUE: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ducking {
    Mute,
    Reduce80,
    Reduce50,
    None,
}

impl Ducking {
    pub fn registry_value(self) -> u32 {
        match self {
            Ducking::Mute => 0,
            Ducking::Reduce80 => 1,
            Ducking::Reduce50 => 2,
            Ducking::None => 3,
        }
    }

    pub fn from_percent(percent: u8) -> Self {
        match percent {
            0..=55 => Ducking::Mute,
            56..=85 => Ducking::Reduce80,
            _ => Ducking::Reduce50,
        }
    }
}

/// Applies a preference and returns the previous value (to be restored later).
pub fn apply(ducking: Ducking) -> Result<u32> {
    imp::apply(ducking)
}

pub fn restore(previous: u32) -> Result<()> {
    imp::restore(previous)
}

pub fn current() -> Result<u32> {
    imp::current()
}

#[cfg(windows)]
mod imp {
    use super::{Ducking, AUDIO_KEY, DEFAULT_VALUE, VALUE_NAME};
    use anyhow::{anyhow, Result};
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    /// Opens (creating if needed) the audio preference key for reading and writing.
    fn writable_key() -> Result<RegKey> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.create_subkey(AUDIO_KEY)
            .map(|(key, _)| key)
            .map_err(|err| anyhow!("cannot open HKCU\\{AUDIO_KEY}: {err}"))
    }

    fn read_value(key: &RegKey) -> u32 {
        key.get_value::<u32, _>(VALUE_NAME).unwrap_or(DEFAULT_VALUE)
    }

    pub fn apply(ducking: Ducking) -> Result<u32> {
        let key = writable_key()?;
        let previous = read_value(&key);
        key.set_value(VALUE_NAME, &ducking.registry_value())
            .map_err(|err| anyhow!("cannot write {VALUE_NAME}: {err}"))?;
        Ok(previous)
    }

    pub fn restore(previous: u32) -> Result<()> {
        let key = writable_key()?;
        key.set_value(VALUE_NAME, &previous)
            .map_err(|err| anyhow!("cannot restore {VALUE_NAME}: {err}"))?;
        Ok(())
    }

    pub fn current() -> Result<u32> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        match hkcu.open_subkey(AUDIO_KEY) {
            Ok(key) => Ok(read_value(&key)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(DEFAULT_VALUE),
            Err(err) => Err(anyhow!("cannot read HKCU\\{AUDIO_KEY}: {err}")),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Ducking;
    use anyhow::Result;

    pub fn apply(_ducking: Ducking) -> Result<u32> {
        anyhow::bail!("audio ducking is only available on Windows")
    }

    pub fn restore(_previous: u32) -> Result<()> {
        anyhow::bail!("audio ducking is only available on Windows")
    }

    pub fn current() -> Result<u32> {
        Ok(super::DEFAULT_VALUE)
    }
}
