//! Windows SAPI text-to-speech (Soundpad ships `TTS.dll` with the same role).
//!
//! Uses the in-box `SpVoice` / `SpFileStream` COM objects, writes a WAV file and
//! lets the caller play it through the normal engine.

use std::path::Path;

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct TtsVoice {
    /// Token id (registry path) or SAPI token id string.
    pub id: String,
    pub name: String,
    pub language: String,
}

/// Enumerates installed SAPI voices.
pub fn list_voices() -> Result<Vec<TtsVoice>> {
    imp::list_voices()
}

/// Synthesizes `text` to a 16-bit PCM WAV file.
///
/// * `rate_percent`: -10..=10 (SAPI rate), 0 = normal.
/// * `pitch`: -10..=10, applied through SSML `<prosody pitch>`.
pub fn synthesize_to_wav(
    text: &str,
    voice_id: Option<&str>,
    rate_percent: i32,
    pitch: i32,
    out: &Path,
) -> Result<()> {
    imp::synthesize_to_wav(text, voice_id, rate_percent, pitch, out)
}

/// Escapes text for inclusion in an SSML document.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(windows)]
mod imp {
    use super::{xml_escape, TtsVoice};
    use std::path::Path;

    use anyhow::{anyhow, Context, Result};
    use windows::core::{GUID, PCWSTR, PWSTR};
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::Media::Audio::WAVEFORMATEX;
    use windows::Win32::Media::Speech::{
        IEnumSpObjectTokens, ISpObjectToken, ISpObjectTokenCategory, ISpStream, ISpVoice,
        SVSFIsXML, SPCAT_VOICES, SPFM_CREATE_ALWAYS,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_APARTMENTTHREADED,
    };

    /// `CLSID_SpVoice`
    const CLSID_SP_VOICE: GUID = GUID::from_u128(0x96749377_3391_11d2_9ee3_00c04f797396);
    /// `CLSID_SpStream` (`SpFileStream` shares the same coclass)
    const CLSID_SP_STREAM: GUID = GUID::from_u128(0x947812b3_2ae1_4644_ba86_9e90ded7ec91);
    /// `CLSID_SpObjectTokenCategory`
    const CLSID_SP_OBJECT_TOKEN_CATEGORY: GUID =
        GUID::from_u128(0xa910187f_0c7a_45ac_92cc_59edafb77b53);
    /// `CLSID_SpObjectToken`
    const CLSID_SP_OBJECT_TOKEN: GUID = GUID::from_u128(0xef411752_3736_4cb4_9c8c_8ef4ccb58efe);

    /// `SPDFID_WaveFormatEx`
    const SPDFID_WAVEFORMATEX: GUID = GUID::from_u128(0xc31adbae_527f_4ff5_a230_f62bb61ff70c);

    const WAVE_FORMAT_PCM: u16 = 1;
    const SAMPLE_RATE: u32 = 22_050;
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn take_pwstr(value: PWSTR) -> String {
        if value.is_null() {
            return String::new();
        }
        let text = unsafe { value.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(value.0 as *const core::ffi::c_void)) };
        text
    }

    /// RAII COM initialization. `owned` is false when COM was already
    /// initialized with a different threading model.
    struct ComInit {
        owned: bool,
    }

    impl ComInit {
        fn new() -> Self {
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            ComInit { owned: hr.is_ok() }
        }
    }

    impl Drop for ComInit {
        fn drop(&mut self) {
            if self.owned {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn registry_voices() -> Vec<TtsVoice> {
        let mut voices = Vec::new();
        let roots = [
            r"SOFTWARE\Microsoft\Speech\Voices\Tokens",
            r"SOFTWARE\WOW6432Node\Microsoft\Speech\Voices\Tokens",
        ];
        for root in roots {
            let Ok(tokens) =
                winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE).open_subkey(root)
            else {
                continue;
            };
            for token in tokens.enum_keys().flatten() {
                let Ok(key) = tokens.open_subkey(&token) else {
                    continue;
                };
                // SAPI 5.1+ keeps the display strings under `Attributes`.
                let attributes = key.open_subkey("Attributes").ok();
                let read = |name: &str| -> String {
                    key.get_value::<String, _>(name)
                        .ok()
                        .or_else(|| {
                            attributes
                                .as_ref()
                                .and_then(|attrs| attrs.get_value(name).ok())
                        })
                        .unwrap_or_default()
                };
                let name = read("Name");
                let language = read("Language");
                if name.is_empty() && language.is_empty() {
                    continue;
                }
                voices.push(TtsVoice {
                    id: format!(r"HKEY_LOCAL_MACHINE\{root}\{token}"),
                    name: if name.is_empty() { token.clone() } else { name },
                    language,
                });
            }
        }
        voices
    }

    /// Reads a SAPI token attribute such as `Name` or `Language`. Modern SAPI
    /// stores these under the token's `Attributes` subkey; older voices keep them
    /// directly on the token key.
    unsafe fn token_attr(token: &ISpObjectToken, name: PCWSTR) -> String {
        unsafe {
            if let Ok(attributes) = token.OpenKey(windows::core::w!("Attributes")) {
                if let Ok(value) = attributes.GetStringValue(name) {
                    let text = take_pwstr(value);
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            match token.GetStringValue(name) {
                Ok(value) => take_pwstr(value),
                Err(_) => String::new(),
            }
        }
    }

    /// Preferred path: SAPI COM token enumeration.
    fn com_voices() -> Result<Vec<TtsVoice>> {
        let _com = ComInit::new();

        unsafe {
            let category: ISpObjectTokenCategory =
                CoCreateInstance(&CLSID_SP_OBJECT_TOKEN_CATEGORY, None, CLSCTX_ALL)
                    .context("cannot create SAPI object token category")?;
            category
                .SetId(SPCAT_VOICES, false)
                .context("cannot open the SAPI voices category")?;
            let tokens: IEnumSpObjectTokens = category
                .EnumTokens(None, None)
                .context("cannot enumerate SAPI voices")?;

            let mut count = 0u32;
            tokens.GetCount(&mut count).ok();

            let mut voices = Vec::new();
            for index in 0..count {
                let token = match tokens.Item(index) {
                    Ok(token) => token,
                    Err(_) => continue,
                };
                let id = take_pwstr(token.GetId().unwrap_or_else(|_| PWSTR::null()));
                let name = token_attr(&token, windows::core::w!("Name"));
                let language = token_attr(&token, windows::core::w!("Language"));
                if id.is_empty() && name.is_empty() {
                    continue;
                }
                voices.push(TtsVoice { id, name, language });
            }

            if voices.is_empty() {
                return Err(anyhow!("SAPI reported no voices"));
            }
            Ok(voices)
        }
    }

    pub fn list_voices() -> Result<Vec<TtsVoice>> {
        match com_voices() {
            Ok(voices) => Ok(voices),
            Err(err) => {
                tracing::debug!("TTS: SAPI COM voice enumeration failed ({err:#}); using registry");
                let voices = registry_voices();
                if voices.is_empty() {
                    Err(anyhow!("no SAPI voices are installed"))
                } else {
                    Ok(voices)
                }
            }
        }
    }

    fn pcm_format() -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM,
            nChannels: CHANNELS,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * CHANNELS as u32 * BITS_PER_SAMPLE as u32 / 8,
            nBlockAlign: CHANNELS * BITS_PER_SAMPLE / 8,
            wBitsPerSample: BITS_PER_SAMPLE,
            cbSize: 0,
        }
    }

    /// Returns the text to speak plus the SAPI speak flags.
    ///
    /// Plain text is passed through untouched (SAPI would otherwise read
    /// `&amp;` aloud); only the pitch path wraps the escaped text in SSML.
    pub(super) fn speak_markup(text: &str, rate_percent: i32, pitch: i32) -> (String, u32) {
        if pitch == 0 {
            (text.to_string(), 0)
        } else {
            let escaped = xml_escape(text);
            let markup = format!(
                "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" xml:lang=\"en-US\"><prosody rate=\"{:+}%\" pitch=\"{:+}%\">{escaped}</prosody></speak>",
                rate_percent * 10,
                pitch * 10
            );
            (markup, SVSFIsXML.0 as u32)
        }
    }

    pub fn synthesize_to_wav(
        text: &str,
        voice_id: Option<&str>,
        rate_percent: i32,
        pitch: i32,
        out: &Path,
    ) -> Result<()> {
        if text.trim().is_empty() {
            return Err(anyhow!("cannot synthesize empty text"));
        }
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create {}", parent.display()))?;
            }
        }
        // Recreate the target so a stale file cannot pass the size check.
        match std::fs::remove_file(out) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| format!("cannot replace {}", out.display()))
            }
        }

        let _com = ComInit::new();
        let path_wide = wide(&out.to_string_lossy());

        unsafe {
            let voice: ISpVoice = CoCreateInstance(&CLSID_SP_VOICE, None, CLSCTX_ALL)
                .context("cannot create SAPI voice (SpVoice)")?;
            let stream: ISpStream = CoCreateInstance(&CLSID_SP_STREAM, None, CLSCTX_ALL)
                .context("cannot create SAPI stream (SpFileStream)")?;

            let format = pcm_format();
            stream
                .BindToFile(
                    PCWSTR::from_raw(path_wide.as_ptr()),
                    SPFM_CREATE_ALWAYS,
                    Some(&SPDFID_WAVEFORMATEX),
                    Some(&format),
                    0,
                )
                .context("cannot bind the SAPI stream to the output file")?;

            voice
                .SetOutput(&stream, BOOL(1))
                .context("cannot attach the SAPI voice to the output stream")?;

            let rate = rate_percent.clamp(-10, 10);
            if let Err(err) = voice.SetRate(rate) {
                tracing::debug!("TTS: SetRate({rate}) failed: {err}");
            }

            // Voice selection is best-effort: a missing or invalid token must not
            // break synthesis with the default voice.
            if let Some(voice_id) = voice_id {
                if let Err(err) = apply_voice(&voice, voice_id) {
                    tracing::debug!("TTS: cannot select voice {voice_id}: {err:#}");
                }
            }

            let pitch = pitch.clamp(-10, 10);
            let (markup, flags) = speak_markup(text, rate, pitch);
            let markup_wide = wide(&markup);

            // Synchronous speak; SAPI waits for it when SVSFlagsAsync is absent.
            voice
                .Speak(PCWSTR::from_raw(markup_wide.as_ptr()), flags, None)
                .context("SAPI Speak failed")?;

            if let Err(err) = voice.WaitUntilDone(120_000) {
                tracing::debug!("TTS: WaitUntilDone returned {err}");
            }

            // Flush the WAV header before the stream is released.
            let _ = stream.Close();
            drop(voice);
            drop(stream);
        }

        let meta = std::fs::metadata(out)
            .with_context(|| format!("TTS produced no file at {}", out.display()))?;
        if meta.len() == 0 {
            return Err(anyhow!("TTS produced an empty file at {}", out.display()));
        }
        Ok(())
    }

    /// Applies `voice_id` (a registry token path) to the voice, ignoring failures.
    fn apply_voice(voice: &ISpVoice, voice_id: &str) -> Result<()> {
        let token: ISpObjectToken =
            unsafe { CoCreateInstance(&CLSID_SP_OBJECT_TOKEN, None, CLSCTX_ALL) }
                .map_err(|err| anyhow!("cannot create SAPI token object: {err}"))?;
        let id = wide(voice_id);
        unsafe {
            token
                .SetId(SPCAT_VOICES, PCWSTR::from_raw(id.as_ptr()), false)
                .map_err(|err| anyhow!("cannot open voice token {voice_id}: {err}"))?;
            voice
                .SetVoice(&token)
                .map_err(|err| anyhow!("cannot select voice {voice_id}: {err}"))?;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::TtsVoice;
    use anyhow::Result;
    use std::path::Path;

    pub fn list_voices() -> Result<Vec<TtsVoice>> {
        anyhow::bail!("SAPI voices are only available on Windows")
    }

    pub fn synthesize_to_wav(
        _text: &str,
        _voice_id: Option<&str>,
        _rate_percent: i32,
        _pitch: i32,
        _out: &Path,
    ) -> Result<()> {
        anyhow::bail!("SAPI synthesis is only available on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::xml_escape;

    #[test]
    fn escapes_xml_specials() {
        assert_eq!(
            xml_escape(r#"a & b < c > d " e ' f"#),
            "a &amp; b &lt; c &gt; d &quot; e &apos; f"
        );
    }

    #[cfg(windows)]
    #[test]
    fn plain_text_is_not_ssml() {
        let (markup, flags) = super::imp::speak_markup("Tom & Jerry <3", 0, 0);
        assert_eq!(markup, "Tom & Jerry <3");
        assert_eq!(flags, 0);
    }

    #[cfg(windows)]
    #[test]
    fn pitch_uses_escaped_ssml() {
        let (markup, flags) = super::imp::speak_markup("a & b", 2, 3);
        assert!(markup.starts_with("<speak version=\"1.0\""), "{markup}");
        assert!(markup.contains("a &amp; b"), "{markup}");
        assert!(markup.contains("pitch=\"+30%\""), "{markup}");
        assert_eq!(flags, windows::Win32::Media::Speech::SVSFIsXML.0 as u32);
    }
}
