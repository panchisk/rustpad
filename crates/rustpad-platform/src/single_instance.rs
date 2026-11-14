//! Single-instance guard and activation of the running instance.
//!
//! Soundpad uses `Local\SoundpadSingleInstanceMem` + a mutex [OBS]. RustPad uses a
//! named mutex plus a small `WM_SHOWWINDOW`-style pipe message to the running
//! instance (`\\.\pipe\rustpad_activate`).

#[cfg(not(windows))]
use anyhow::Result;

pub const MUTEX_NAME: &str = r"Local\RustPadSingleInstance";
pub const ACTIVATION_PIPE: &str = r"\\.\pipe\rustpad_activate";

#[cfg(windows)]
mod imp {
    use super::{ACTIVATION_PIPE, MUTEX_NAME};
    use crate::pipe::{create_pipe_instance, open_pipe, pipe_read_line, pipe_wide, pipe_write_all};
    use std::time::Duration;

    use anyhow::{anyhow, Context, Result};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
        HANDLE,
    };
    use windows::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};
    use windows::Win32::System::Threading::CreateMutexW;

    use crate::pipe::pipe_win32_code;

    /// `HANDLE` is not `Send`; kernel handles may be reused across threads.
    struct SendHandle(HANDLE);

    unsafe impl Send for SendHandle {}

    /// Holds the mutex for the process lifetime.
    pub struct InstanceGuard {
        handle: HANDLE,
    }

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.handle) };
        }
    }

    /// Returns `None` when another instance already owns the guard.
    pub fn acquire() -> Result<Option<InstanceGuard>> {
        let name = pipe_wide(MUTEX_NAME);
        let handle = unsafe {
            CreateMutexW(
                None,
                windows::Win32::Foundation::BOOL(0),
                PCWSTR::from_raw(name.as_ptr()),
            )
        }
        .context("cannot create the single-instance mutex")?;

        let last_error = unsafe { GetLastError() };
        if last_error == ERROR_ALREADY_EXISTS {
            let _ = unsafe { CloseHandle(handle) };
            Ok(None)
        } else {
            Ok(Some(InstanceGuard { handle }))
        }
    }

    /// Asks the already-running instance to show its window. Returns false if none.
    pub fn activate_existing(_args: &[String]) -> bool {
        let Ok(handle) = open_pipe(ACTIVATION_PIPE, Duration::from_millis(400)) else {
            return false;
        };
        let written = unsafe { pipe_write_all(handle, b"activate\n") }.is_ok();
        let _ = unsafe { CloseHandle(handle) };
        written
    }

    /// Server side of the activation pipe; calls `on_activate` for each message.
    pub fn spawn_activation_server(on_activate: impl Fn() + Send + 'static) -> Result<()> {
        let name_storage = pipe_wide(ACTIVATION_PIPE);
        let name = PCWSTR::from_raw(name_storage.as_ptr());
        let first = SendHandle(unsafe { create_pipe_instance(name) }?);

        std::thread::Builder::new()
            .name("rustpad-activation".to_string())
            .spawn(move || unsafe { serve(on_activate, first, name_storage) })
            .map(|_| ())
            .map_err(|err| anyhow!("cannot spawn the activation thread: {err}"))
    }

    unsafe fn serve(on_activate: impl Fn(), first: SendHandle, name_storage: Vec<u16>) {
        let mut pipe = first.0;
        let name = PCWSTR::from_raw(name_storage.as_ptr());
        loop {
            let mut connected = false;
            match unsafe { ConnectNamedPipe(pipe, None) } {
                Ok(()) => connected = true,
                Err(err)
                    if pipe_win32_code(&err) == ERROR_PIPE_CONNECTED.0
                        || pipe_win32_code(&err) == ERROR_NO_DATA.0 =>
                {
                    // The client connected (or already closed) before we called
                    // ConnectNamedPipe; its request may still be buffered.
                    connected = true;
                }
                Err(err) => {
                    tracing::warn!("activation pipe: ConnectNamedPipe failed: {err}");
                }
            }

            if connected {
                let line = unsafe { pipe_read_line(pipe, None, Duration::from_secs(10)) };
                let request = line.as_deref().map(str::trim).unwrap_or_default();
                if request.eq_ignore_ascii_case("activate") {
                    on_activate();
                } else if !request.is_empty() {
                    tracing::debug!("activation pipe: ignoring request {request:?}");
                }
                // Acknowledge so the caller knows the request was processed.
                let _ = unsafe {
                    pipe_write_all(
                        pipe, b"ok
",
                    )
                };
                let _ = unsafe { DisconnectNamedPipe(pipe) };
            }

            let _ = unsafe { CloseHandle(pipe) };
            match unsafe { create_pipe_instance(name) } {
                Ok(next) => pipe = next,
                Err(err) => {
                    tracing::error!("activation pipe: cannot recreate the pipe: {err:#}");
                    return;
                }
            }
        }
    }
}

#[cfg(windows)]
pub use imp::{acquire, activate_existing, spawn_activation_server, InstanceGuard};

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;

    /// Holds the mutex for the process lifetime.
    pub struct InstanceGuard {
        _private: (),
    }

    /// Returns `None` when another instance already owns the guard.
    pub fn acquire() -> Result<Option<InstanceGuard>> {
        Ok(Some(InstanceGuard { _private: () }))
    }

    /// Asks the already-running instance to show its window. Returns false if none.
    pub fn activate_existing(_args: &[String]) -> bool {
        false
    }

    /// Server side of the activation pipe; calls `on_activate` for each message.
    pub fn spawn_activation_server(_on_activate: impl Fn() + Send + 'static) -> Result<()> {
        Ok(())
    }
}

#[cfg(not(windows))]
pub use imp::{acquire, activate_existing, spawn_activation_server, InstanceGuard};
