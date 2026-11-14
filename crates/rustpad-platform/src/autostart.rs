//! Autostart via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use std::path::Path;

use anyhow::Result;

pub const VALUE_NAME: &str = "RustPad";
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
pub fn set_autostart(exe: &Path, enabled: bool) -> Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if enabled {
        let (key, _) = hkcu
            .create_subkey(RUN_KEY)
            .map_err(|err| anyhow::anyhow!("cannot open HKCU\\{RUN_KEY}: {err}"))?;
        let command = format!("\"{}\"", exe.display());
        key.set_value(VALUE_NAME, &command)
            .map_err(|err| anyhow::anyhow!("cannot write autostart value: {err}"))?;
    } else {
        let key = match hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE) {
            Ok(key) => key,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(anyhow::anyhow!(
                    "cannot open HKCU\\{RUN_KEY} for writing: {err}"
                ))
            }
        };
        match key.delete_value(VALUE_NAME) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(anyhow::anyhow!(
                    "cannot remove autostart value {VALUE_NAME}: {err}"
                ))
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn is_autostart_enabled() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey(RUN_KEY) else {
        return false;
    };
    key.get_value::<String, _>(VALUE_NAME)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn set_autostart(_exe: &Path, _enabled: bool) -> Result<()> {
    anyhow::bail!("autostart is only available on Windows")
}

#[cfg(not(windows))]
pub fn is_autostart_enabled() -> bool {
    false
}
