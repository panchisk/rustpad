//! Global low-level keyboard hook.
//!
//! `RegisterHotKey` cannot express Soundpad-style *modifier-only* push-to-talk
//! hotkeys and cannot observe key-up, so RustPad uses `WH_KEYBOARD_LL` instead.
//! The hook runs on its own thread with a Win32 message loop and forwards events
//! over a channel. `injected` is set for events with `LLKHF_INJECTED`, which lets
//! the Auto-Keys feature avoid feeding its own synthetic input back.
//!
//! Swallowing keys (when "pass hotkeys" is off) is intentionally *not* done here:
//! the app decides, and the hook currently never returns 1.

use rustpad_core::Modifiers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Windows virtual-key code.
    pub vk: u32,
    pub modifiers: Modifiers,
    /// True for key-down, false for key-up.
    pub down: bool,
    /// Injected by software (SendInput), not a physical key.
    pub injected: bool,
}

#[cfg(windows)]
mod imp {
    use super::KeyEvent;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use anyhow::{anyhow, Context, Result};
    use crossbeam_channel::{bounded, Receiver, Sender};
    use rustpad_core::Modifiers;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PeekMessageW, PostThreadMessageW, SetWindowsHookExW,
        UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_QUIT, WM_SYSKEYDOWN, WM_USER,
    };

    /// Channel size; a burst of keystrokes larger than this simply drops events
    /// rather than blocking the hook (which would freeze input system-wide).
    const CHANNEL_CAPACITY: usize = 512;

    static EVENT_SENDER: OnceLock<Mutex<Option<Sender<KeyEvent>>>> = OnceLock::new();

    pub struct KeyboardHook {
        rx: Receiver<KeyEvent>,
        stop: Arc<AtomicBool>,
        thread_id: u32,
        join: Option<JoinHandle<()>>,
    }

    fn sender_slot() -> &'static Mutex<Option<Sender<KeyEvent>>> {
        EVENT_SENDER.get_or_init(|| Mutex::new(None))
    }

    fn async_key_down(vk: u16) -> bool {
        (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
    }

    fn current_modifiers() -> Modifiers {
        let mut modifiers = Modifiers::NONE;
        if async_key_down(VK_SHIFT.0) {
            modifiers.insert(Modifiers::SHIFT);
        }
        if async_key_down(VK_CONTROL.0) {
            modifiers.insert(Modifiers::CTRL);
        }
        if async_key_down(VK_MENU.0) {
            modifiers.insert(Modifiers::ALT);
        }
        if async_key_down(VK_LWIN.0) || async_key_down(VK_RWIN.0) {
            modifiers.insert(Modifiers::WIN);
        }
        modifiers
    }

    unsafe extern "system" fn low_level_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // Never swallow keys: always hand the event on.
        if code == HC_ACTION as i32 && lparam.0 != 0 {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let message = wparam.0 as u32;
            let event = KeyEvent {
                vk: info.vkCode,
                modifiers: current_modifiers(),
                down: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
                injected: (info.flags.0 & LLKHF_INJECTED.0) != 0,
            };
            if let Ok(slot) = sender_slot().lock() {
                if let Some(tx) = slot.as_ref() {
                    // try_send: dropping a hotkey beats blocking the input chain.
                    let _ = tx.try_send(event);
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    fn hook_thread(tx: Sender<KeyEvent>, init_tx: std::sync::mpsc::Sender<Result<u32>>) {
        {
            let mut slot = match sender_slot().lock() {
                Ok(slot) => slot,
                Err(poisoned) => poisoned.into_inner(),
            };
            *slot = Some(tx);
        }

        unsafe {
            let module = match GetModuleHandleW(None) {
                Ok(module) => module,
                Err(err) => {
                    let _ = init_tx.send(Err(anyhow!("GetModuleHandleW failed: {err}")));
                    return;
                }
            };
            let hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_proc), module, 0) {
                Ok(hook) => hook,
                Err(err) => {
                    let _ = init_tx.send(Err(anyhow!(
                        "SetWindowsHookExW(WH_KEYBOARD_LL) failed: {err}"
                    )));
                    return;
                }
            };

            // Make sure the thread owns a message queue before the owner may
            // post WM_QUIT to it.
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
            let thread_id = GetCurrentThreadId();
            if init_tx.send(Ok(thread_id)).is_err() {
                let _ = UnhookWindowsHookEx(hook);
                return;
            }

            loop {
                let received = GetMessageW(&mut msg, None, 0, 0);
                if received.0 <= 0 {
                    // 0 == WM_QUIT, -1 == error: either way stop pumping.
                    break;
                }
            }

            let _ = UnhookWindowsHookEx(hook);
        }

        if let Ok(mut slot) = sender_slot().lock() {
            *slot = None;
        }
    }

    impl KeyboardHook {
        pub fn start() -> Result<Self> {
            let (tx, rx) = bounded(CHANNEL_CAPACITY);
            let (init_tx, init_rx) = std::sync::mpsc::channel();

            let join = std::thread::Builder::new()
                .name("rustpad-hotkeys".to_string())
                .spawn(move || hook_thread(tx, init_tx))
                .context("cannot spawn global hotkey thread")?;

            match init_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(thread_id)) => Ok(KeyboardHook {
                    rx,
                    stop: Arc::new(AtomicBool::new(false)),
                    thread_id,
                    join: Some(join),
                }),
                Ok(Err(err)) => {
                    let _ = join.join();
                    Err(err)
                }
                Err(err) => {
                    let _ = std::thread::Builder::new()
                        .spawn(move || {
                            let _ = join.join();
                        })
                        .map(|_| ());
                    Err(anyhow!("global hotkey thread did not start: {err}"))
                }
            }
        }

        pub fn try_event(&self) -> Option<KeyEvent> {
            self.rx.try_recv().ok()
        }

        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let Some(join) = self.join.take() else {
                return;
            };

            // Post WM_QUIT to the hook thread's queue; retry briefly in case the
            // thread is between creating the queue and pumping messages.
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let posted =
                    unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
                if posted.is_ok() {
                    break;
                }
                if Instant::now() >= deadline {
                    tracing::warn!(
                        "hotkeys: cannot post WM_QUIT to thread {}: {:?}",
                        self.thread_id,
                        posted
                    );
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }

            let _ = join.join();
        }

        pub fn is_running(&self) -> bool {
            !self.stop.load(Ordering::SeqCst)
        }
    }

    impl Drop for KeyboardHook {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(windows)]
pub use imp::KeyboardHook;

#[cfg(not(windows))]
mod imp {
    use super::KeyEvent;
    use anyhow::Result;
    use crossbeam_channel::Receiver;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    pub struct KeyboardHook {
        rx: Receiver<KeyEvent>,
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    impl KeyboardHook {
        pub fn start() -> Result<Self> {
            anyhow::bail!("global hotkeys are only available on Windows")
        }

        pub fn try_event(&self) -> Option<KeyEvent> {
            self.rx.try_recv().ok()
        }

        pub fn stop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }

        pub fn is_running(&self) -> bool {
            !self.stop.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for KeyboardHook {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(not(windows))]
pub use imp::KeyboardHook;
