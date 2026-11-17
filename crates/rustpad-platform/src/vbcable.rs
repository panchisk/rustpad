//! Automatic installation of the VB-Audio VB-CABLE virtual audio driver.
//!
//! The official driver pack is downloaded to `%TEMP%\rustpad-vbcable`, extracted
//! with PowerShell 5 and handed to `VBCABLE_Setup_x64.exe -h -i` via `Start-Process
//! -Verb RunAs` (UAC). VB-CABLE always needs a reboot before its endpoints appear.
//! Downloaded files stay in `%TEMP%` for diagnostics and re-runs.

use crossbeam_channel::Receiver;

/// Official VB-CABLE driver pack (43, x64 + x86).
pub const DOWNLOAD_URL: &str =
    "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack43.zip";

/// Lowercase fragments identifying a VB-CABLE style endpoint.
const NAME_MARKERS: &[&str] = &[
    "cable input",
    "cable output",
    "vb-audio",
    "vb-cable",
    "virtual cable",
    "voicemeeter input",
    "voicemeeter vaio",
];

/// Result of a one-click driver installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Endpoints (or the driver key) are already present.
    AlreadyInstalled,
    /// Reserved for callers treating an install without pending reboot as final.
    Installed,
    /// Installer succeeded; Windows needs a restart before the cable appears.
    NeedsReboot,
    /// The user dismissed the UAC prompt.
    Cancelled,
    /// Anything else, with a human readable reason.
    Failed(String),
}

/// True if an audio endpoint friendly name looks like VB-CABLE / VoiceMeeter.
pub fn looks_like_vbcable(name: &str) -> bool {
    let lowered = name.to_lowercase();
    NAME_MARKERS.iter().any(|marker| lowered.contains(marker))
}

/// Blocking: checks registry/driver presence, downloads the official zip to
/// `%TEMP%\rustpad-vbcable`, extracts it, and runs
/// `VBCABLE_Setup_x64.exe -h -i` elevated through PowerShell
/// `Start-Process -Verb RunAs -Wait`.
pub fn install_blocking() -> InstallOutcome {
    match imp::install() {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::warn!("VB-CABLE installation failed: {err}");
            InstallOutcome::Failed(err)
        }
    }
}

/// Spawns [`install_blocking`] on a background thread and returns a receiver.
pub fn start_install() -> Receiver<InstallOutcome> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("rustpad-vbcable".to_string())
        .spawn(move || {
            let _ = tx.send(install_blocking());
        });
    if let Err(err) = spawned {
        tracing::warn!("cannot spawn VB-CABLE install thread: {err}");
        let _ = fallback.send(InstallOutcome::Failed(format!(
            "cannot spawn thread: {err}"
        )));
    }
    rx
}

/// Quotes a value for a PowerShell single-quoted literal (`'` -> `''`).
#[cfg_attr(not(windows), allow(dead_code))]
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
mod imp {
    use super::{ps_literal, InstallOutcome, DOWNLOAD_URL};
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;

    const ZIP_NAME: &str = "VBCABLE_Driver_Pack43.zip";
    const INSTALLER_NAME: &str = "VBCABLE_Setup_x64.exe";
    const TEMP_DIR_NAME: &str = "rustpad-vbcable";
    const POLL: Duration = Duration::from_millis(100);
    const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
    const INSTALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
    /// `CREATE_NO_WINDOW`: hides the helper console.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// One subkey per audio render endpoint.
    const RENDER_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
    /// Fallback marker: VB-CABLE's own driver key.
    const VBCABLE_KEY: &str = r"SOFTWARE\VB-Audio\CABLE";
    /// Endpoint friendly names live in `<endpoint>\Properties` as `{guid},6`.
    const FRIENDLY_NAME_SUFFIX: &str = "},6";

    /// Combined result of a hidden PowerShell run.
    struct PsOutput {
        code: Option<i32>,
        stdout: String,
        stderr: String,
        timed_out: bool,
    }

    impl PsOutput {
        /// Whitespace-collapsed output, stderr preferred, for diagnostics.
        fn detail(&self) -> String {
            let source = if self.stderr.trim().is_empty() {
                &self.stdout
            } else {
                &self.stderr
            };
            let joined = source.split_whitespace().collect::<Vec<_>>().join(" ");
            if joined.is_empty() {
                return "no output".to_string();
            }
            let chars: Vec<char> = joined.chars().collect();
            chars[chars.len().saturating_sub(400)..].iter().collect()
        }
    }

    pub fn install() -> Result<InstallOutcome, String> {
        if is_installed() {
            tracing::info!("VB-CABLE endpoints already registered, nothing to install");
            return Ok(InstallOutcome::AlreadyInstalled);
        }

        let dir = std::env::temp_dir().join(TEMP_DIR_NAME);
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
        let zip = dir.join(ZIP_NAME);
        let download = format!(
            "$ProgressPreference='SilentlyContinue';\n\
             Invoke-WebRequest -UseBasicParsing -Uri {url} -OutFile {zip};\n\
             Expand-Archive -LiteralPath {zip} -DestinationPath {dir} -Force",
            url = ps_literal(DOWNLOAD_URL),
            zip = ps_path(&zip),
            dir = ps_path(&dir),
        );
        tracing::info!(url = DOWNLOAD_URL, target = %dir.display(), "downloading VB-CABLE pack");
        let out = run_powershell(&download, DOWNLOAD_TIMEOUT)
            .map_err(|err| format!("cannot start PowerShell: {err}"))?;
        if out.timed_out {
            return Err("download timed out".to_string());
        }
        if out.code != Some(0) {
            return Err(format!("download failed: {}", out.detail()));
        }

        let installer = find_file(&dir, INSTALLER_NAME).ok_or_else(|| {
            format!(
                "{INSTALLER_NAME} not found under {}: {}",
                dir.display(),
                out.detail()
            )
        })?;
        tracing::info!(installer = %installer.display(), "running VB-CABLE installer (UAC)");

        // The elevated child's exit code is echoed so the parent can read it.
        let install = format!(
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8;\n\
             $p = Start-Process -FilePath {exe} -ArgumentList '-h','-i' -Verb RunAs -Wait -PassThru;\n\
             Write-Output ('EXITCODE=' + $p.ExitCode);\n\
             $p.ExitCode",
            exe = ps_path(&installer),
        );
        let out = run_powershell(&install, INSTALL_TIMEOUT)
            .map_err(|err| format!("cannot start PowerShell: {err}"))?;
        if out.timed_out {
            return Err(format!(
                "installer timed out after {}s",
                INSTALL_TIMEOUT.as_secs()
            ));
        }

        let combined = format!("{}\n{}", out.stdout, out.stderr).to_lowercase();
        if combined.contains("cancel") || combined.contains("отмен") {
            tracing::info!("VB-CABLE installer cancelled by the user (UAC)");
            return Ok(InstallOutcome::Cancelled);
        }

        let kept = format!(" (files kept in {})", dir.display());
        match exit_code(&out) {
            // VB-CABLE always needs a reboot; 3010/1641 mean "reboot required".
            Some(0) | Some(3010) | Some(1641) => {
                tracing::info!(
                    dir = %dir.display(),
                    "VB-CABLE installer finished, restart required; files kept for a re-run"
                );
                Ok(InstallOutcome::NeedsReboot)
            }
            Some(code) => Err(format!(
                "installer exited with code {code}: {}{kept}",
                out.detail()
            )),
            None => Err(format!(
                "installer produced no exit code: {}{kept}",
                out.detail()
            )),
        }
    }

    /// PowerShell single-quoted literal for a filesystem path.
    fn ps_path(path: &Path) -> String {
        ps_literal(&path.to_string_lossy())
    }

    /// True when a render endpoint or the VB-Audio driver key exists.
    fn is_installed() -> bool {
        use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
        use winreg::RegKey;

        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        // Access problems only mean "cannot tell", never a panic.
        if let Ok(render) = hklm.open_subkey_with_flags(RENDER_KEY, KEY_READ) {
            for endpoint in render.enum_keys().flatten() {
                let Ok(key) = render.open_subkey_with_flags(&endpoint, KEY_READ) else {
                    continue;
                };
                let Ok(props) = key.open_subkey_with_flags("Properties", KEY_READ) else {
                    continue;
                };
                for (value_name, raw) in props.enum_values().flatten() {
                    if !value_name.ends_with(FRIENDLY_NAME_SUFFIX) {
                        continue;
                    }
                    if let Some(name) = friendly_name(&raw) {
                        if super::looks_like_vbcable(&name) {
                            tracing::info!(endpoint = %name, "found VB-CABLE endpoint");
                            return true;
                        }
                    }
                }
            }
        }
        hklm.open_subkey_with_flags(VBCABLE_KEY, KEY_READ).is_ok()
    }

    /// Decodes a `Properties` value into an endpoint name.
    fn friendly_name(raw: &winreg::RegValue) -> Option<String> {
        use winreg::enums::RegType;
        use winreg::types::FromRegValue;

        match raw.vtype {
            RegType::REG_SZ | RegType::REG_EXPAND_SZ | RegType::REG_MULTI_SZ => {
                String::from_reg_value(raw).ok()
            }
            // Some builds store the name as a UTF-16 blob with a binary header.
            _ => {
                let units: Vec<u16> = raw
                    .bytes
                    .chunks(2)
                    .filter_map(|pair| <[u8; 2]>::try_from(pair).ok())
                    .map(u16::from_le_bytes)
                    .collect();
                let text: String = String::from_utf16_lossy(&units)
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .collect();
                Some(text)
            }
        }
    }

    /// Recursive search (depth-first) for `file_name` below `root`.
    fn find_file(root: &Path, file_name: &str) -> Option<PathBuf> {
        let mut pending = vec![root.to_path_buf()];
        for _ in 0..10_000 {
            let dir = pending.pop()?;
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_file() {
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(file_name)
                    {
                        return Some(entry.path());
                    }
                } else if kind.is_dir() {
                    pending.push(entry.path());
                }
            }
        }
        None
    }

    /// Windows PowerShell 5.1 path, falling back to `%PATH%`.
    fn powershell_exe() -> PathBuf {
        if let Some(root) = std::env::var_os("SystemRoot") {
            let candidate =
                Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
            if candidate.is_file() {
                return candidate;
            }
        }
        PathBuf::from("powershell.exe")
    }

    /// Runs a hidden PowerShell script with a hard timeout.
    fn run_powershell(script: &str, timeout: Duration) -> std::io::Result<PsOutput> {
        let mut child = Command::new(powershell_exe())
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;

        let stdout = Arc::new(Mutex::new(String::new()));
        let stderr = Arc::new(Mutex::new(String::new()));
        let readers = [
            read_async(child.stdout.take(), Arc::clone(&stdout)),
            read_async(child.stderr.take(), Arc::clone(&stderr)),
        ];

        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err);
                }
            }
            if Instant::now() >= deadline {
                tracing::warn!("PowerShell helper exceeded {}s, killing", timeout.as_secs());
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break None;
            }
            std::thread::sleep(POLL);
        };

        // Let the pipe readers drain what the child wrote before exiting.
        let drain_until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < drain_until && !readers.iter().all(|r| r.is_finished()) {
            std::thread::sleep(POLL);
        }

        let stdout_text = stdout.lock().clone();
        let stderr_text = stderr.lock().clone();
        Ok(PsOutput {
            code: status.and_then(|status| status.code()),
            stdout: stdout_text,
            stderr: stderr_text,
            timed_out,
        })
    }

    /// Drains a child pipe on its own thread (lossy UTF-8).
    fn read_async<R: Read + Send + 'static>(
        pipe: Option<R>,
        sink: Arc<Mutex<String>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let Some(mut pipe) = pipe else { return };
            let mut buffer = Vec::new();
            if pipe.read_to_end(&mut buffer).is_ok() {
                sink.lock().push_str(&String::from_utf8_lossy(&buffer));
            }
        })
    }

    /// Installer exit code from `EXITCODE=<n>`, else PowerShell's own code.
    fn exit_code(output: &PsOutput) -> Option<i32> {
        if let Some(rest) = output.stdout.split("EXITCODE=").nth(1) {
            let number: String = rest
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
                .collect();
            if let Ok(code) = number.parse::<i32>() {
                return Some(code);
            }
        }
        output.code
    }
}

#[cfg(not(windows))]
mod imp {
    use super::InstallOutcome;

    pub fn install() -> Result<InstallOutcome, String> {
        Err("unsupported".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_vbcable_endpoint_names() {
        for name in [
            "CABLE Input (VB-Audio Virtual Cable)",
            "CABLE Output (VB-Audio Virtual Cable)",
            "cable input",
            "VB-CABLE",
            "Virtual Cable 1",
            "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)",
            "VoiceMeeter VAIO",
        ] {
            assert!(looks_like_vbcable(name), "expected match for {name:?}");
        }
    }

    #[test]
    fn ignores_unrelated_endpoint_names() {
        for name in [
            "Microphone (fifise)",
            "Speakers (Realtek(R) Audio)",
            "Headphones (High Definition Audio Device)",
            "",
        ] {
            assert!(!looks_like_vbcable(name), "unexpected match for {name:?}");
        }
    }

    #[test]
    fn quotes_powershell_literals() {
        assert_eq!(ps_literal(r"C:\Temp\a.zip"), r"'C:\Temp\a.zip'");
        assert_eq!(ps_literal("O'Brien"), "'O''Brien'");
        assert_eq!(
            DOWNLOAD_URL,
            "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack43.zip"
        );
    }
}
