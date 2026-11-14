//! Synthetic key press/release for Soundpad's "Auto Keys" feature.
//!
//! Auto Keys holds a configurable key (and optional modifiers) down while a
//! sound is playing, which acts as push-to-talk in games and voice chat.

use anyhow::Result;
use rustpad_core::hotkey::Modifiers;

/// Presses `vk` with `modifiers` held (modifiers first, then the key).
pub fn press(vk: u16, modifiers: Modifiers) -> Result<()> {
    imp::send(vk, modifiers, true)
}

/// Releases `vk` and the modifiers pressed by [`press`].
pub fn release(vk: u16, modifiers: Modifiers) -> Result<()> {
    imp::send(vk, modifiers, false)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::mem::size_of;

    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT,
    };

    fn key_event(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn send_inputs(inputs: &[INPUT]) -> Result<()> {
        let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
        anyhow::ensure!(
            sent as usize == inputs.len(),
            "SendInput sent {sent} of {} events",
            inputs.len()
        );
        Ok(())
    }

    fn modifier_keys(modifiers: Modifiers) -> Vec<u16> {
        let mut keys = Vec::new();
        if modifiers.contains(Modifiers::CTRL) {
            keys.push(VK_CONTROL.0);
        }
        if modifiers.contains(Modifiers::ALT) {
            keys.push(VK_MENU.0);
        }
        if modifiers.contains(Modifiers::SHIFT) {
            keys.push(VK_SHIFT.0);
        }
        if modifiers.contains(Modifiers::WIN) {
            keys.push(VK_LWIN.0);
        }
        keys
    }

    pub fn send(vk: u16, modifiers: Modifiers, down: bool) -> Result<()> {
        // A zero virtual key would be rejected by SendInput; treat it as "off".
        if vk == 0 {
            return Ok(());
        }
        let mut inputs = Vec::new();
        let mods = modifier_keys(modifiers);
        if down {
            for m in mods {
                inputs.push(key_event(m, false));
            }
            inputs.push(key_event(vk, false));
        } else {
            inputs.push(key_event(vk, true));
            for m in mods.into_iter().rev() {
                inputs.push(key_event(m, true));
            }
        }
        send_inputs(&inputs)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn send(_vk: u16, _modifiers: Modifiers, _down: bool) -> Result<()> {
        anyhow::bail!("Auto Keys require Windows SendInput")
    }
}
