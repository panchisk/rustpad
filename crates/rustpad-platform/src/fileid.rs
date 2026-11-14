//! Soundpad's file identity (`hash` attribute in `.spl`).
//!
//! Recovered from live data: the hash is
//!
//! ```text
//! <volume serial number>-<file id high 32 bits>-<file id low 32 bits>
//! ```
//!
//! all printed in decimal. Both parts come straight from
//! `GetFileInformationByHandle` (`dwVolumeSerialNumber`, `nFileIndexHigh`,
//! `nFileIndexLow`), so files imported by RustPad get exactly the same hash
//! Soundpad would assign and `stats.spl` play counts keep working across both
//! applications.

use std::path::Path;

use anyhow::{Context, Result};

/// Computes Soundpad's `hash` string for `path`.
pub fn soundpad_hash(path: &Path) -> Result<String> {
    imp::soundpad_hash(path)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    /// Opens with `FILE_FLAG_BACKUP_SEMANTICS` as well, so directories work.
    fn handle_info(path: &Path) -> Result<BY_HANDLE_FILE_INFORMATION> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)
            .with_context(|| format!("opening {} for file id", path.display()))?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(
                HANDLE(file.as_raw_handle() as *mut core::ffi::c_void),
                &mut info,
            )
            .with_context(|| format!("GetFileInformationByHandle({})", path.display()))?;
        }
        Ok(info)
    }

    pub fn soundpad_hash(path: &Path) -> Result<String> {
        let info = handle_info(path)?;
        Ok(format!(
            "{}-{}-{}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        ))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn soundpad_hash(_path: &Path) -> Result<String> {
        anyhow::bail!("Soundpad hashes are a Windows/NTFS concept")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_has_soundpad_shape_and_is_stable() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("rustpad-hash-{}.tmp", std::process::id()));
        std::fs::write(&path, b"rustpad").expect("write temp file");

        let a = soundpad_hash(&path).expect("hash");
        let b = soundpad_hash(&path).expect("hash again");
        std::fs::remove_file(&path).ok();

        assert_eq!(a, b, "hash must be stable");
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(parts.len(), 3, "A-B-C: {a}");
        for p in parts {
            p.parse::<u64>().expect("decimal parts");
        }
    }

    #[test]
    fn hash_matches_soundpad_for_a_live_soundlist_entry() {
        // Optional: verifies the recovered algorithm against the real Soundpad
        // list when it exists on this machine.
        let list = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .map(|p| p.join("Leppsoft").join("soundlist.spl"));
        let Some(list) = list.filter(|p| p.is_file()) else {
            eprintln!("skipping: no live Soundpad list");
            return;
        };
        let xml = std::fs::read_to_string(&list).expect("read soundlist");
        for line in xml
            .lines()
            .filter(|l| l.trim_start().starts_with("<Sound "))
        {
            let mut url = None;
            let mut hash = None;
            for part in line.split('"').skip(1).step_by(2) {
                // crude attribute scan is enough for this optional check
                if part.starts_with("C:\\") && part.ends_with(".mp3") {
                    url = Some(part.to_string());
                }
            }
            if let Some(h) = line
                .split("hash=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
            {
                hash = Some(h.to_string());
            }
            let (Some(url), Some(hash)) = (url, hash) else {
                continue;
            };
            let path = std::path::Path::new(&url);
            if !path.is_file() {
                continue;
            }
            assert_eq!(soundpad_hash(path).expect("hash"), hash, "hash for {url}");
            return;
        }
        eprintln!("skipping: no existing sound file referenced by the list");
    }
}
