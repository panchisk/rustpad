# RustPad

A Rust soundboard for Windows that loads **Soundpad 4.x sound lists** (`.spl`), speaks the
**Soundpad remote-control protocol**, and plays sounds into your **speakers and microphone**
without a kernel driver.

Built after a full reverse-engineering pass of `Soundpad.exe 4.0.35` (Leppsoft) — see
`docs/EVIDENCE.md` for the observed architecture, formats, protocols and the honest list of
unknowns.

![status](https://img.shields.io/badge/status-alpha-orange)

## Why a virtual cable instead of a driver

Soundpad injects audio into every application's microphone through **UniteFx**, a signed
Audio Processing Object (`C:\Windows\System32\UniteFx1.8.0.dll`, CLSID
`{27384E53-9860-0AC3-9519-C60EBCAA2C71}`) registered as an SFX on the capture endpoint.
Windows requires WHQL signing for such an APO, which a hobby project cannot obtain.

RustPad does the same job in user mode:

```
real microphone ──► capture ──┐
                              ├──► mixer ──► "CABLE Input" (virtual device)
sounds flagged "microphone" ──┘                     │
                                                    ▼
                                   apps select "CABLE Output" as microphone
sounds flagged "speakers" ──────► mixer ──► speakers
```

Install any virtual audio cable once (VB-CABLE, VoiceMeeter, SteelSeries Sonar, …), select its
**input** as *Microphone output* in RustPad, and its **output** as the microphone in Discord,
OBS, CS2, etc. Your voice and the soundboard are mixed together.

## Features

- Sound list compatible with Soundpad: sounds, categories, icons, colors, `customTag` tags,
  hotbar pages, `.spl` reading/writing, `stats.spl` play counts, daily backups. Sound identity
  follows Soundpad's rule (0-based document order), so lists stay readable by Soundpad after
  edits and deletions. The file `hash` is Soundpad's real volume-serial + NTFS file-id tuple,
  so imported and newly added sounds share play counts with Soundpad.
- Audio: WAV / MP3 / FLAC / OGG-Vorbis / AAC (M4A) decoding, on-the-fly resampling, polyphonic
  mixing, per-sound and master volume, pause/resume, seek, next/previous/random. Starting a new
  sound stops the previous one by default (switchable in *Settings → Audio* for overlapping
  playback); pending decodes of older requests are discarded so a slow file cannot start late.
- Routing: speakers, microphone (virtual cable), or both; microphone passthrough with gain;
  optional playback delay for microphone-target sounds; optional voice block while a sound
  plays.
- Global hotkeys: per-sound keys, modifier-only push-to-talk, index hotkeys (`Ctrl+1..0`),
  numpad hotkeys, hotkey enable/disable toggle, hotkey overview.
- Auto Keys: holds a configurable key + modifiers with `SendInput` while a sound plays
  (simulated push-to-talk), optionally only for hotkey-triggered playback.
- Hotbar: pages, 6-column grid, per-cell colors and hotkeys.
- Loudness: off / fixed dB adjustment / dynamic per-sound normalisation to a target level.
- Recorder: microphone **or** system output (WASAPI loopback, "what you hear") to WAV with
  silence trimming and peak normalisation.
- Text-to-speech: Windows SAPI voices, rate/pitch, play on speakers/mic/both.
- Sound editor: waveform, selection, crop, fades, reverse, normalise, trim silence, karaoke,
  speed, boost, save as WAV / add to list.
- Remote control: named pipe `\\.\pipe\sp_remote_control`, Soundpad command names and
  response framing (payload lines + `R<code>`), plus `rustpad -rc "DoPlaySound(1)"` on the
  command line.
- Windows integration: single instance, autostart, dark/light theme, Russian/English UI,
  Windows "duck other sounds" preference, drag & drop, file dialogs, open another `.spl`
  (`Ctrl+O` or `rustpad --list file.spl`).

## Build

Requirements: Rust 1.80+ with the MSVC toolchain (`x86_64-pc-windows-msvc`), Windows 10/11.

```powershell
cd C:\Users\admin\Documents\rustpad
cargo test --workspace
cargo build --release
target\release\rustpad.exe
```

The binary is self-contained; no installer, no driver, no Steam.

## Quick start

1. Start RustPad. It opens your existing Soundpad list from
   `%APPDATA%\Leppsoft\soundlist.spl` when present, otherwise
   `%USERPROFILE%\Documents\soundpad\soundlist.spl`, otherwise an empty list.
2. **Settings → Audio**: pick your speakers as *Output device*.
3. Install a virtual cable (VB-CABLE, VoiceMeeter, …). RustPad detects common cables and selects
   their input (e.g. `CABLE Input`) as *Microphone output* automatically; your real microphone is
   the *Capture device*. If no cable is found, a banner explains what is missing.
4. In the voice app, select the cable's output (`CABLE Output`) as the microphone.
5. Click a sound. Set hotkeys with right-click → **Set hotkey…**.

### Microphone is silent in other apps

Soundpad can mix audio into the real microphone because it installs a signed APO driver. RustPad
does not install a driver; it mixes into a virtual cable instead. Without a cable, *Microphone
passthrough* has nowhere to send your voice, so other apps hear only the physical microphone.
Install VB-CABLE, press **Rescan devices** (RustPad selects `CABLE Input` itself), then select
`CABLE Output` as the microphone in Discord/OBS/game. The banner at the top of the window opens
step-by-step instructions and the download link.

## Remote control

Same pipe name and command names as Soundpad ([OBS] in the binary):

```powershell
rustpad -rc "DoPlaySound(1)"
rustpad -rc "DoPlaySoundFromCategory(2,1,true,false)"
rustpad -rc "GetSoundFileCount()"
```

Supported: `DoPlaySound`, `DoPlaySoundFromCategory`, `DoPlayRandomSound`,
`DoPlayRandomSoundFromCategory`, `DoPlayPreviousSound`, `DoPlayNextSound`,
`DoPlaySelectedSound`, `DoPlayCurrentSoundAgain`, `DoPlayPreviouslyPlayedSound`,
`PauseResume`, `Stop`, `DoAddSound(path)`, `DoRemoveSelectedEntries`, `DoRemoveCategory`,
`DoSaveSoundlist`, `GetSoundFileCount`, `GetSoundlist`, `GetVersion`,
`GetRemoteControlVersion`.

Response framing matches Soundpad (confirmed from its UTF-16 literals): optional payload
line(s), then `R<code>` — `R0` for success, `R-204` for a malformed call and `R-404` for an
unknown command. Error codes `R-200/-400/-403/-409/-500` are defined in `docs/EVIDENCE.md`.

## Verification

Performed on this machine against real Soundpad data:

| Check | Result |
|---|---|
| `cargo test --workspace` | **77 tests pass** (41 audio/DSP + loopback, 12 sound-list/stats, 15 remote-control, 7 Windows integration, 2 microphone-routing) |
| Parse `%APPDATA%\Leppsoft\soundlist.spl` | 158 sounds, 9 categories, 1 hotbar page |
| Parse `%USERPROFILE%\Documents\soundpad\soundlist.spl` | 114 sounds, 11 categories, tags/colors preserved |
| Decode WAV / MP3 (48 kHz stereo, 44.1 kHz mono) | correct duration, rate, channels, peak/RMS |
| Round-trip `.spl` | 158/114 sounds, same categories/attributes; unknown attributes preserved |
| Remote control | `GetVersion()` → `0.1.0`, `GetSoundFileCount()` → 158/114, `DoPlaySoundFromCategory(...)` starts playback, `Stop()` stops |
| Audio engine | tone playback with position/level metering, microphone and 1 s loopback recordings to WAV |
| Auto Keys | app held F13 during playback and released it after (`GetAsyncKeyState`); the global hook receives injected Ctrl+Alt+F1 |
| Soundpad hash | computed hash equals the live list's `hash` for existing files |
| Remote framing | payload + `R0`; malformed calls `R-204`, unknown commands `R-404` |
| TTS | SAPI voice enumeration + WAV synthesis (RIFF/PCM/mono/22050 Hz/16-bit) |
| GUI | main window, settings (6 tabs), TTS, editor, hotkeys overview and hotbar rendered and exercised |

There is no automated GUI test; UI checks were manual screenshots during development.

## Compatibility notes

- `.spl` files are read and written with the observed Soundpad schema. Unknown attributes are
  preserved. Hotbar items use Soundpad's own element/attribute names (`<HotbarItem soundId=…>`,
  recovered from the binary's literal pool); `altCode` is preserved but not interpreted.
- File hashes are Soundpad's real volume-serial + NTFS file-id tuple, so imported and newly
  added sounds share the same `stats.spl` play counts.
- There is no DRM, licensing or Steam integration: every feature works without Soundpad.
- Microphone-target sounds are delayed by *Settings → Audio → Delay*; microphone passthrough
  itself is not delayed. Hotkeys are observed, never swallowed (`CallNextHookEx` always runs),
  so a bound key still reaches the focused application.
- Loopback recording captures the default output device mix, not individual applications.
  Opus decoding remains disabled; WAV/MP3/FLAC/OGG-Vorbis/AAC are supported.

## Licence

MIT. Soundpad is © Leppsoft; this project contains no Leppsoft code or assets.
