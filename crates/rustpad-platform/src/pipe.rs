//! Soundpad-compatible remote control.
//!
//! Server: named pipe `\\.\pipe\sp_remote_control` (same name as Soundpad 4.x, [OBS]),
//! line-oriented text requests, one request per connection. Requests are the exact
//! command names found in the shipped binary (see `docs/EVIDENCE.md` §4):
//!
//! ```text
//! DoPlaySound(index)
//! DoPlaySound(index,speakers,mic)
//! DoPlaySoundFromCategory(categoryIndex,soundIndex,speakers,mic)
//! DoPlayRandomSound(speakers,mic)
//! DoPlayRandomSoundFromCategory(categoryIndex,speakers,mic)
//! DoPlayPreviousSound() / DoPlayNextSound() / DoPlaySelectedSound()
//! DoPlayCurrentSoundAgain() / DoPlayPreviouslyPlayedSound()
//! DoAddSound(path) / DoRemoveSelectedEntries() / DoRemoveCategory() / DoSaveSoundlist()
//! GetSoundFileCount() / GetSoundlist() / GetVersion() / GetRemoteControlVersion()
//! ```
//!
//! Soundpad's exact response framing could not be recovered from strings alone
//! ([UNK]); RustPad answers with optional payload lines followed by `R<code>`,
//! where `0` = success and negative values are errors. Clients that fire and forget
//! (the common Stream Deck pattern) are unaffected.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Named pipe used by Soundpad 4.x and by RustPad's compatible server.
pub const PIPE_NAME: &str = r"\\.\pipe\sp_remote_control";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCommand {
    PlaySound {
        index: u32,
        speakers: Option<bool>,
        mic: Option<bool>,
    },
    PlaySoundFromCategory {
        category: i32,
        index: u32,
        speakers: Option<bool>,
        mic: Option<bool>,
    },
    PlayRandomSound {
        speakers: Option<bool>,
        mic: Option<bool>,
    },
    PlayRandomSoundFromCategory {
        category: i32,
        speakers: Option<bool>,
        mic: Option<bool>,
    },
    PlayPreviousSound,
    PlayNextSound,
    PlaySelectedSound,
    PlayCurrentSoundAgain,
    PlayPreviouslyPlayedSound,
    PauseResume,
    Stop,
    AddSound {
        path: String,
    },
    RemoveSelectedEntries,
    RemoveCategory,
    SaveSoundlist,
    GetSoundFileCount,
    GetSoundlist,
    GetVersion,
    GetRemoteControlVersion,
}

#[derive(Debug, Clone, Default)]
pub struct RemoteReply {
    pub code: i32,
    pub lines: Vec<String>,
}

impl RemoteReply {
    pub fn ok() -> Self {
        RemoteReply {
            code: 0,
            lines: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::with_code(-1, message)
    }

    /// Reply with Soundpad's error codes: `-204` malformed call, `-404` unknown
    /// command (both observed as literals in `Soundpad.exe`).
    pub fn with_code(code: i32, message: impl Into<String>) -> Self {
        RemoteReply {
            code,
            lines: vec![message.into()],
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::with_code(-204, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::with_code(-404, message)
    }

    pub fn with_line(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }

    pub fn to_wire(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&format!("R{}\n", self.code));
        out
    }
}

pub type RemoteHandler = Arc<dyn Fn(RemoteCommand) -> RemoteReply + Send + Sync>;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parses one request line. Case-insensitive command names, whitespace tolerated,
/// optional `;` terminator.
pub fn parse_command(line: &str) -> std::result::Result<RemoteCommand, String> {
    let mut s = line.trim();
    while let Some(rest) = s.strip_suffix(';') {
        s = rest.trim_end();
    }
    if s.is_empty() {
        return Err("empty command".to_string());
    }

    let (name, args) = match s.find('(') {
        Some(open) => {
            let name = s[..open].trim();
            let rest = &s[open + 1..];
            let Some(close) = find_closing_paren(rest) else {
                return Err(format!("missing ')' in command: {s}"));
            };
            if !rest[close + 1..].trim().is_empty() {
                return Err(format!(
                    "unexpected text after ')' in command: {}",
                    rest[close + 1..].trim()
                ));
            }
            (name, split_args(&rest[..close])?)
        }
        None => (s, Vec::new()),
    };

    if name.is_empty() {
        return Err(format!("missing command name: {line}"));
    }

    let canonical = name.replace('_', "").to_ascii_lowercase();
    match canonical.as_str() {
        "doplaysound" => {
            let (index, rest) = split_first(&args, name, "1 or 3")?;
            let index = parse_u32(index, 1, name)?;
            let (speakers, mic) = parse_optional_pair(rest, 2, name)?;
            Ok(RemoteCommand::PlaySound {
                index,
                speakers,
                mic,
            })
        }
        "doplaysoundfromcategory" => {
            let (category, rest) = split_first(&args, name, "2 or 4")?;
            let category = parse_i32(category, 1, name)?;
            let (index, rest) = split_first(rest, name, "2 or 4")?;
            let index = parse_u32(index, 2, name)?;
            let (speakers, mic) = parse_optional_pair(rest, 3, name)?;
            Ok(RemoteCommand::PlaySoundFromCategory {
                category,
                index,
                speakers,
                mic,
            })
        }
        "doplayrandomsound" => {
            if !args.is_empty() && args.len() != 2 {
                return Err(arity_error(name, "0 or 2", args.len()));
            }
            let (speakers, mic) = parse_optional_pair(&args, 1, name)?;
            Ok(RemoteCommand::PlayRandomSound { speakers, mic })
        }
        "doplayrandomsoundfromcategory" => {
            let (category, rest) = split_first(&args, name, "1 or 3")?;
            let category = parse_i32(category, 1, name)?;
            let (speakers, mic) = parse_optional_pair(rest, 2, name)?;
            Ok(RemoteCommand::PlayRandomSoundFromCategory {
                category,
                speakers,
                mic,
            })
        }
        "doplayprevioussound" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::PlayPreviousSound)
        }
        "doplaynextsound" => expect_no_args(name, &args).map(|()| RemoteCommand::PlayNextSound),
        "doplayselectedsound" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::PlaySelectedSound)
        }
        "doplaycurrentsoundagain" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::PlayCurrentSoundAgain)
        }
        "doplaypreviouslyplayedsound" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::PlayPreviouslyPlayedSound)
        }
        "dopauseresume" | "pauseresume" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::PauseResume)
        }
        "dostop" | "stop" => expect_no_args(name, &args).map(|()| RemoteCommand::Stop),
        "doaddsound" => {
            if args.len() != 1 {
                return Err(arity_error(name, "1", args.len()));
            }
            let path = args[0].clone();
            if path.is_empty() {
                return Err(format!("{name} requires a non-empty path"));
            }
            Ok(RemoteCommand::AddSound { path })
        }
        "doremoveselectedentries" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::RemoveSelectedEntries)
        }
        "doremovecategory" => expect_no_args(name, &args).map(|()| RemoteCommand::RemoveCategory),
        "dosavesoundlist" => expect_no_args(name, &args).map(|()| RemoteCommand::SaveSoundlist),
        "getsoundfilecount" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::GetSoundFileCount)
        }
        "getsoundlist" => expect_no_args(name, &args).map(|()| RemoteCommand::GetSoundlist),
        "getversion" => expect_no_args(name, &args).map(|()| RemoteCommand::GetVersion),
        "getremotecontrolversion" => {
            expect_no_args(name, &args).map(|()| RemoteCommand::GetRemoteControlVersion)
        }
        _ => Err(format!("unknown command: {name}")),
    }
}

fn find_closing_paren(rest: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut chars = rest.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        match c {
            '\\' if in_quotes => {
                let _ = chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ')' if !in_quotes => return Some(idx),
            _ => {}
        }
    }
    None
}

/// Splits a comma-separated argument list, honouring `"quoted, args"` and
/// backslash escapes (`\"`, `\\`) inside quotes.
fn split_args(input: &str) -> std::result::Result<Vec<String>, String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut quoted_arg = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                quoted_arg = true;
            }
            '\\' if in_quotes => match chars.peek().copied() {
                Some('"') | Some('\\') => current.push(chars.next().expect("peeked")),
                _ => current.push('\\'),
            },
            ',' if !in_quotes => {
                args.push(current.trim().to_string());
                current.clear();
                quoted_arg = false;
            }
            _ => current.push(c),
        }
    }

    if in_quotes {
        return Err("unterminated quoted argument".to_string());
    }

    args.push(current.trim().to_string());

    // `DoPlaySound()` and `DoPlaySound( )` mean "no arguments", while
    // `DoAddSound("")` is one (empty) argument.
    if args.len() == 1 && args[0].is_empty() && !quoted_arg {
        args.clear();
    }

    Ok(args)
}

fn split_first<'a>(
    args: &'a [String],
    name: &str,
    expected: &str,
) -> std::result::Result<(&'a str, &'a [String]), String> {
    match args.split_first() {
        Some((first, rest)) => Ok((first.as_str(), rest)),
        None => Err(arity_error(name, expected, 0)),
    }
}

fn arity_error(name: &str, expected: &str, got: usize) -> String {
    format!("{name} expects {expected} argument(s), got {got}")
}

fn expect_no_args(name: &str, args: &[String]) -> std::result::Result<(), String> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(arity_error(name, "0", args.len()))
    }
}

/// Parses the optional trailing `(speakers, mic)` pair.
fn parse_optional_pair(
    rest: &[String],
    first_index: usize,
    name: &str,
) -> std::result::Result<(Option<bool>, Option<bool>), String> {
    match rest.len() {
        0 => Ok((None, None)),
        2 => Ok((
            Some(parse_bool(&rest[0], first_index, name)?),
            Some(parse_bool(&rest[1], first_index + 1, name)?),
        )),
        n => Err(format!(
            "{name} expects an optional speakers,mic argument pair (2 values), got {n}"
        )),
    }
}

fn parse_u32(value: &str, arg: usize, name: &str) -> std::result::Result<u32, String> {
    value.trim().parse::<u32>().map_err(|_| {
        format!(
            "invalid number '{}' for argument {arg} of {name}: expected a non-negative integer",
            value.trim()
        )
    })
}

fn parse_i32(value: &str, arg: usize, name: &str) -> std::result::Result<i32, String> {
    value.trim().parse::<i32>().map_err(|_| {
        format!(
            "invalid number '{}' for argument {arg} of {name}: expected an integer",
            value.trim()
        )
    })
}

fn parse_bool(value: &str, arg: usize, name: &str) -> std::result::Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(format!(
            "invalid boolean '{other}' for argument {arg} of {name}: expected true/false/1/0"
        )),
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use super::{parse_command, RemoteHandler, RemoteReply, PIPE_NAME};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use anyhow::{anyhow, Context, Result};
    use windows::core::{Error as WinError, PCWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ,
        GENERIC_WRITE, HANDLE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, WaitNamedPipeW,
        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    const BUFFER_SIZE: u32 = 64 * 1024;
    const MAX_REQUEST: usize = 1024 * 1024;

    /// `HANDLE` is a raw pointer and therefore not `Send`; kernel handles may be
    /// used from any thread, so move it between threads explicitly.
    pub(crate) struct SendHandle(pub HANDLE);

    unsafe impl Send for SendHandle {}

    pub(crate) fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Low 16 bits of a `HRESULT`-wrapped Win32 error code.
    pub(crate) fn win32_code(err: &WinError) -> u32 {
        (err.code().0 as u32) & 0xFFFF
    }

    pub(crate) unsafe fn create_instance(name: PCWSTR) -> Result<HANDLE> {
        let handle = unsafe {
            CreateNamedPipeW(
                name,
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                BUFFER_SIZE,
                BUFFER_SIZE,
                0,
                None,
            )
        };
        if handle.is_invalid() {
            Err(anyhow!(
                "CreateNamedPipeW failed: {}",
                WinError::from_win32()
            ))
        } else {
            Ok(handle)
        }
    }

    pub(crate) unsafe fn write_all(handle: HANDLE, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let mut written = 0u32;
            unsafe { WriteFile(handle, Some(data), Some(&mut written), None) }
                .map_err(|err| anyhow!("WriteFile failed: {err}"))?;
            if written == 0 {
                return Err(anyhow!("WriteFile wrote 0 bytes"));
            }
            data = &data[written as usize..];
        }
        Ok(())
    }

    /// Reads one line (up to and excluding `\n`) from the pipe.
    ///
    /// Polls instead of issuing a blocking read so a client that connects and
    /// then goes silent can neither hang shutdown nor leak the thread forever.
    pub(crate) unsafe fn read_line(
        handle: HANDLE,
        stop: Option<&AtomicBool>,
        idle_timeout: Duration,
    ) -> Option<String> {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        let deadline = Instant::now() + idle_timeout;
        loop {
            let mut available = 0u32;
            match unsafe { PeekNamedPipe(handle, None, 0, None, Some(&mut available), None) } {
                Ok(()) => {}
                Err(err) => {
                    tracing::debug!("remote control: read ended: {err}");
                    break;
                }
            }

            if available == 0 {
                if stop.is_some_and(|flag| flag.load(Ordering::SeqCst))
                    || Instant::now() >= deadline
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }

            let want = (available as usize).min(chunk.len());
            let mut read = 0u32;
            match unsafe { ReadFile(handle, Some(&mut chunk[..want]), Some(&mut read), None) } {
                Ok(()) => {
                    if read == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..read as usize]);
                    if buf.contains(&b'\n') || buf.len() >= MAX_REQUEST {
                        break;
                    }
                }
                Err(err) => {
                    tracing::debug!("remote control: read ended: {err}");
                    break;
                }
            }
        }
        if buf.is_empty() {
            return None;
        }
        let text = String::from_utf8_lossy(&buf).into_owned();
        Some(match text.find('\n') {
            Some(pos) => text[..pos].to_string(),
            None => text,
        })
    }

    unsafe fn serve(
        handler: RemoteHandler,
        stop: Arc<AtomicBool>,
        first: SendHandle,
        name_storage: Vec<u16>,
    ) {
        let mut pipe = first.0;
        let name = PCWSTR::from_raw(name_storage.as_ptr());
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }

            let mut connected = false;
            match unsafe { ConnectNamedPipe(pipe, None) } {
                Ok(()) => connected = true,
                Err(err)
                    if win32_code(&err) == ERROR_PIPE_CONNECTED.0
                        || win32_code(&err) == ERROR_NO_DATA.0 =>
                {
                    // Client already connected (or closed) before this call; a
                    // fire-and-forget request may still be buffered.
                    connected = true;
                }
                Err(err) => {
                    tracing::warn!("remote control: ConnectNamedPipe failed: {err}");
                }
            }

            if connected {
                let line = unsafe { read_line(pipe, Some(&stop), Duration::from_secs(10)) };
                if stop.load(Ordering::SeqCst) {
                    let _ = unsafe { DisconnectNamedPipe(pipe) };
                    break;
                }
                if let Some(line) = line {
                    let line = line.trim();
                    if !line.is_empty() {
                        let reply = match parse_command(line) {
                            Ok(command) => handler(command),
                            Err(err) => {
                                tracing::debug!("remote control: rejected request: {err}");
                                if err.starts_with("unknown command") {
                                    RemoteReply::not_found(err)
                                } else {
                                    RemoteReply::invalid(err)
                                }
                            }
                        };
                        let wire = reply.to_wire();
                        if let Err(err) = unsafe { write_all(pipe, wire.as_bytes()) } {
                            tracing::warn!("remote control: cannot reply: {err:#}");
                        }
                        if let Err(err) = unsafe { FlushFileBuffers(pipe) } {
                            tracing::debug!("remote control: flush failed: {err}");
                        }
                    }
                }
                let _ = unsafe { DisconnectNamedPipe(pipe) };
            }

            let _ = unsafe { CloseHandle(pipe) };
            if stop.load(Ordering::SeqCst) {
                break;
            }

            match unsafe { create_instance(name) } {
                Ok(next) => pipe = next,
                Err(err) => {
                    tracing::error!("remote control: cannot recreate pipe: {err:#}");
                    return;
                }
            }
        }
        let _ = unsafe { CloseHandle(pipe) };
    }

    /// Owns the server thread; the pipe instance is created up front so that the
    /// caller learns about permission problems immediately.
    pub struct PipeServer {
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    impl PipeServer {
        pub fn start(handler: RemoteHandler) -> Result<Self> {
            let name_storage = wide(PIPE_NAME);
            let name = PCWSTR::from_raw(name_storage.as_ptr());
            let first = unsafe { create_instance(name) }
                .with_context(|| format!("cannot create named pipe {PIPE_NAME}"))?;

            let first = SendHandle(first);
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = Arc::clone(&stop);
            let join = std::thread::Builder::new()
                .name("rustpad-remote-control".to_string())
                .spawn(move || unsafe { serve(handler, thread_stop, first, name_storage) })
                .context("cannot spawn remote-control thread")?;

            Ok(PipeServer {
                stop,
                join: Some(join),
            })
        }

        pub fn stop(&mut self) {
            let Some(join) = self.join.take() else {
                return;
            };
            self.stop.store(true, Ordering::SeqCst);
            // Wake the server out of ConnectNamedPipe; it then observes the flag
            // and returns from the thread.
            if let Ok(handle) = connect(PIPE_NAME, Duration::from_millis(500)) {
                let _ = unsafe { CloseHandle(handle) };
            }
            let _ = join.join();
        }
    }

    impl Drop for PipeServer {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// Connects to the pipe name, waiting for an instance up to `timeout`.
    pub(crate) fn connect(pipe_name: &str, timeout: Duration) -> Result<HANDLE> {
        let name_storage = wide(pipe_name);
        let name = PCWSTR::from_raw(name_storage.as_ptr());
        let deadline = Instant::now() + timeout;

        loop {
            let result = unsafe {
                CreateFileW(
                    name,
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            };
            match result {
                Ok(handle) => return Ok(handle),
                Err(err) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(anyhow!("cannot connect to {pipe_name}: {err}"));
                    }
                    if win32_code(&err) == ERROR_PIPE_BUSY.0 {
                        let remaining = deadline.saturating_duration_since(now);
                        let wait = remaining.as_millis().clamp(1, 250) as u32;
                        let _ = unsafe { WaitNamedPipeW(name, wait) };
                    } else {
                        // Pipe missing or access denied; retry a few times so a
                        // just-starting server is not missed.
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
            }
        }
    }

    fn reply_complete(data: &[u8]) -> bool {
        if !data.ends_with(
            b"
",
        ) {
            return false;
        }
        let text = String::from_utf8_lossy(data);
        let Some(last) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
            return false;
        };
        let last = last.trim();
        match last.strip_prefix('R') {
            Some(rest) => rest.parse::<i32>().is_ok(),
            None => false,
        }
    }

    /// Client used by `rustpad -rc <command>`: connects to a running instance.
    pub fn send_command(command: &str, timeout: Duration) -> Result<String> {
        let handle = connect(PIPE_NAME, timeout)?;

        let result = (|| -> Result<String> {
            let mut payload = command.trim_end().as_bytes().to_vec();
            payload.push(b'\n');
            unsafe { write_all(handle, &payload) }?;
            if let Err(err) = unsafe { FlushFileBuffers(handle) } {
                tracing::debug!("remote control: flush failed: {err}");
            }

            let deadline = Instant::now() + timeout;
            let mut out: Vec<u8> = Vec::new();
            loop {
                let mut available = 0u32;
                match unsafe { PeekNamedPipe(handle, None, 0, None, Some(&mut available), None) } {
                    Ok(()) => {}
                    Err(err) => {
                        if out.is_empty() {
                            return Err(anyhow!("cannot read reply: {err}"));
                        }
                        break;
                    }
                }

                if available > 0 {
                    let mut chunk = vec![0u8; available.min(8192) as usize];
                    let mut read = 0u32;
                    match unsafe { ReadFile(handle, Some(&mut chunk), Some(&mut read), None) } {
                        Ok(()) if read > 0 => {
                            out.extend_from_slice(&chunk[..read as usize]);
                            if reply_complete(&out) {
                                break;
                            }
                        }
                        _ => break,
                    }
                } else {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }

            if out.is_empty() {
                return Err(anyhow!(
                    "remote control closed the connection without a reply"
                ));
            }
            Ok(String::from_utf8_lossy(&out).into_owned())
        })();

        let _ = unsafe { CloseHandle(handle) };
        result
    }
}

#[cfg(windows)]
pub use imp::{send_command, PipeServer};

/// IPC helpers shared with the single-instance activation pipe.
#[cfg(windows)]
pub(crate) use imp::{
    connect as open_pipe, create_instance as create_pipe_instance, read_line as pipe_read_line,
    wide as pipe_wide, win32_code as pipe_win32_code, write_all as pipe_write_all,
};

#[cfg(not(windows))]
mod imp {
    use super::RemoteHandler;
    use anyhow::Result;
    use std::time::Duration;

    pub struct PipeServer {
        _private: (),
    }

    impl PipeServer {
        pub fn start(_handler: RemoteHandler) -> Result<Self> {
            anyhow::bail!("remote control is only available on Windows")
        }

        pub fn stop(&mut self) {}
    }

    pub fn send_command(_command: &str, _timeout: Duration) -> Result<String> {
        anyhow::bail!("remote control is only available on Windows")
    }
}

#[cfg(not(windows))]
pub use imp::{send_command, PipeServer};

#[allow(dead_code)]
pub(crate) fn stop_flag_is_set(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

#[allow(dead_code)]
fn _assert_send() {
    fn is_send<T: Send>() {}
    is_send::<JoinHandle<()>>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_play_sound() {
        assert_eq!(
            parse_command("DoPlaySound(3)").unwrap(),
            RemoteCommand::PlaySound {
                index: 3,
                speakers: None,
                mic: None
            }
        );
        assert_eq!(
            parse_command("DoPlaySound(3,true,0)").unwrap(),
            RemoteCommand::PlaySound {
                index: 3,
                speakers: Some(true),
                mic: Some(false)
            }
        );
        assert_eq!(
            parse_command("DoPlaySound(3, false, true)").unwrap(),
            RemoteCommand::PlaySound {
                index: 3,
                speakers: Some(false),
                mic: Some(true)
            }
        );
    }

    #[test]
    fn parses_case_insensitive_names_and_terminator() {
        assert_eq!(
            parse_command("  doplaynextsound();  ").unwrap(),
            RemoteCommand::PlayNextSound
        );
        assert_eq!(
            parse_command("DOPLAYPREVIOUSSOUND;").unwrap(),
            RemoteCommand::PlayPreviousSound
        );
        assert_eq!(
            parse_command("getversion()").unwrap(),
            RemoteCommand::GetVersion
        );
        assert_eq!(
            parse_command("getremotecontrolversion").unwrap(),
            RemoteCommand::GetRemoteControlVersion
        );
    }

    #[test]
    fn parses_category_commands() {
        assert_eq!(
            parse_command("DoPlaySoundFromCategory(1, 2, true, false)").unwrap(),
            RemoteCommand::PlaySoundFromCategory {
                category: 1,
                index: 2,
                speakers: Some(true),
                mic: Some(false)
            }
        );
        assert_eq!(
            parse_command("DoPlaySoundFromCategory(-1,2)").unwrap(),
            RemoteCommand::PlaySoundFromCategory {
                category: -1,
                index: 2,
                speakers: None,
                mic: None
            }
        );
        assert_eq!(
            parse_command("DoPlayRandomSoundFromCategory(0,1,1)").unwrap(),
            RemoteCommand::PlayRandomSoundFromCategory {
                category: 0,
                speakers: Some(true),
                mic: Some(true)
            }
        );
        assert_eq!(
            parse_command("DoPlayRandomSound()").unwrap(),
            RemoteCommand::PlayRandomSound {
                speakers: None,
                mic: None
            }
        );
        assert_eq!(
            parse_command("DoPlayRandomSound(true,false)").unwrap(),
            RemoteCommand::PlayRandomSound {
                speakers: Some(true),
                mic: Some(false)
            }
        );
    }

    #[test]
    fn parses_no_argument_commands() {
        let cases = [
            ("DoPlaySelectedSound()", RemoteCommand::PlaySelectedSound),
            (
                "DoPlayCurrentSoundAgain()",
                RemoteCommand::PlayCurrentSoundAgain,
            ),
            (
                "DoPlayPreviouslyPlayedSound()",
                RemoteCommand::PlayPreviouslyPlayedSound,
            ),
            ("DoPauseResume()", RemoteCommand::PauseResume),
            ("DoStop()", RemoteCommand::Stop),
            (
                "DoRemoveSelectedEntries()",
                RemoteCommand::RemoveSelectedEntries,
            ),
            ("DoRemoveCategory()", RemoteCommand::RemoveCategory),
            ("DoSaveSoundlist()", RemoteCommand::SaveSoundlist),
            ("GetSoundFileCount()", RemoteCommand::GetSoundFileCount),
            ("GetSoundlist()", RemoteCommand::GetSoundlist),
        ];
        for (line, expected) in cases {
            assert_eq!(parse_command(line).unwrap(), expected, "line: {line}");
        }
        assert_eq!(
            parse_command("DoPlaySelectedSound").unwrap(),
            RemoteCommand::PlaySelectedSound
        );
    }

    #[test]
    fn parses_add_sound_paths() {
        assert_eq!(
            parse_command(r#"DoAddSound("C:\Sounds\boom.wav")"#).unwrap(),
            RemoteCommand::AddSound {
                path: r"C:\Sounds\boom.wav".to_string()
            }
        );
        assert_eq!(
            parse_command(r#"DoAddSound("C:\Sounds\a, b.wav")"#).unwrap(),
            RemoteCommand::AddSound {
                path: r"C:\Sounds\a, b.wav".to_string()
            }
        );
        assert_eq!(
            parse_command(r#"DoAddSound("C:\Sounds\a\"b.wav")"#).unwrap(),
            RemoteCommand::AddSound {
                path: r#"C:\Sounds\a"b.wav"#.to_string()
            }
        );
        assert_eq!(
            parse_command(r#"doaddsound(  "C:\a.wav" )"#).unwrap(),
            RemoteCommand::AddSound {
                path: r"C:\a.wav".to_string()
            }
        );
    }

    #[test]
    fn rejects_bad_arity() {
        assert!(parse_command("DoPlaySound()").is_err());
        assert!(parse_command("DoPlaySound(1,2)").is_err());
        assert!(parse_command("DoPlaySound(1,true,false,true)").is_err());
        assert!(parse_command("DoAddSound()").is_err());
        assert!(parse_command("DoAddSound(a.wav, b.wav)").is_err());
        assert!(parse_command("DoStop(1)").is_err());
        assert!(parse_command("DoPlaySoundFromCategory(1)").is_err());
        let err = parse_command("DoPlaySound(1,2)").unwrap_err();
        assert!(err.contains("DoPlaySound"), "err: {err}");
        assert!(err.contains("argument"), "err: {err}");
    }

    #[test]
    fn rejects_bad_booleans_and_numbers() {
        let err = parse_command("DoPlaySound(1, maybe, true)").unwrap_err();
        assert!(err.contains("invalid boolean"), "err: {err}");
        assert!(parse_command("DoPlaySound(1,true)").is_err());
        assert!(parse_command("DoPlaySound(-4)").is_err());
        assert!(parse_command("DoPlaySound(abc)").is_err());
        assert!(parse_command("DoPlaySoundFromCategory(x,1)").is_err());
    }

    #[test]
    fn rejects_unknown_and_malformed() {
        let err = parse_command("DoSomethingElse()").unwrap_err();
        assert!(err.contains("unknown command"), "err: {err}");
        assert!(parse_command("").is_err());
        assert!(parse_command("   ;  ").is_err());
        assert!(parse_command("DoPlaySound(1").is_err());
        assert!(parse_command(r#"DoAddSound("C:\a.wav)"#).is_err());
        assert!(parse_command("DoPlaySound(1) trailing").is_err());
    }

    #[test]
    fn reply_wire_format() {
        assert_eq!(RemoteReply::ok().to_wire(), "R0\n");
        assert_eq!(RemoteReply::ok().with_line("42").to_wire(), "42\nR0\n");
        assert_eq!(RemoteReply::error("bad").to_wire(), "bad\nR-1\n");
    }
}
