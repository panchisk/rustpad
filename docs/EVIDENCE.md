# Soundpad 4.0.35: reverse-engineering dossier

Target: `C:\Program Files (x86)\Steam\steamapps\common\Soundpad`
Workflow: REA 6.7.0 (skill v42) + static PE inspection. See "Tooling limits" at the end.

Every statement below is tagged:
- **[OBS]** observed in shipped bytes, resources, formats or the live registry/filesystem of this machine.
- **[INF]** inferred from observations; not proven by decompiled control flow.
- **[UNK]** unknown / not established.

## 1. Build identity [OBS]

| File | SHA-256 | Notes |
|---|---|---|
| Soundpad.exe | `7c3249269bd6f33f6826a002b41b93c9f0fdb69b1c32b3d0f47aa039feadab4b` | 12,245,232 B, PE32+ x86-64 GUI |
| SoundpadService.exe | `ea1fe386d62420f945e4e7f877d58e68c7a3a15560ddd155b0874e8774df9c80` | 564,976 B, PE32+ x86-64 GUI |
| TTS.dll | `74b57e2aeb52efbca78af9d5009ffed1130c041deac8888a4e46046af75974a2` | SAPI wrapper, 5 exports |
| UniteFx.dll | `4002fb13f28e0f402f3ea375be22962c36abc46430ac098e97f040b829216570` | 32-bit APO (shipped, unused on x64) |
| UniteFx-ARM.dll | `0cf322f1d1e74b338ff15d2163be9ffc3f7639efd17ae07df355b718037eb5ae` | ARM64 APO |
| steam_api64.dll | `9d6d2972c1571a2a058d79a8376e26a33d33813f29d93c3ca6e35adaddcd56de` | Steamworks |

- Version resource: `FileVersion 4.0.35.0`, Product `Soundpad`, Company `Leppsoft`, `Copyright (C) 2016-2026 Leppsoft`.
- PE time stamp 2026-09-16; MSVC linker 14.44; CFG, NX, ASLR, high-entropy VA.
- Manifest: `requestedExecutionLevel level="asInvoker" uiAccess="true"` [OBS, UTF-16 string in resources]. UIAccess is why hotkeys keep working over elevated foreground windows.
- Steam AppID `629520` (`steam://rungameid/629520`).

## 2. Stack [OBS]

- **wxWidgets** statically linked (wx string literals, `wxFileConfig`, `wxXmlDocument`, `wxDataViewCtrl`, `wxWebRequest`, wx debug strings, `C:\wxwin\include\...` source paths in assert strings). GUI is Win32 common controls + wx, not Qt.
- **Media Foundation** (`MF.dll`, `MFPlat.dll`, `MFReadWrite.dll`) for container/codec probing/transcoding.
- Embedded decoders: **libFLAC**, **libvorbis/ogg**, **Opus** (license strings); MP3 via MF or bundled decoder.
- **WASAPI through COM**, no `mmdevapi` import: `CoCreateInstance` + `AVRT.dll!AvSetMmThreadCharacteristicsW` for the real-time audio thread.
- **Steamworks** for licensing/ownership; license also cached locally (trial limits otherwise).
- Crash reporting via `dbghelp` (`MiniDumpWriteDump`), logs `UniteFx.log`, `UniteFxControl.log` in install dir.

## 3. Component architecture [OBS + INF]

```
Soundpad.exe (GUI + soundlist + hotkeys + pipe server)
   |  WM_COPYDATA / named pipes (Local\SoundpadService*)
   v
SoundpadService.exe  ("Soundpad Client Service", asInvoker+uiAccess)
   |  HKLM writes: APO registration, endpoint FxProperties, audio service restart
   v
audiodg.exe  -- loads  C:\Windows\System32\UniteFx1.8.0.dll  (APO, CLSID {27384E53-...})
   ^
   |  shared memory + named mutexes (Global\UniteFx*1.8.0), chunk upload of PCM
   |
Soundpad.exe audio control (UniteFxControl)
```

- **Mic injection is an APO (Audio Processing Object), not a kernel driver.** [OBS]
  - `HKLM\Software\Classes\AudioEngine\AudioProcessingObjects\{27384E53-9860-0AC3-9519-C60EBCAA2C71}`:
    `FriendlyName=UniteFx`, `MajorVersion=1`, `MinorVersion=8`, `Flags=14`,
    `MinInputConnections=1`, `MaxInputConnections=1`, `NumAPOInterfaces=1`,
    `APOInterface0={FD7F2B29-24D0-4B5C-B177-592C39F9CA10}` (IAudioProcessingObject, SFX).
  - Installed binary: `C:\Windows\System32\UniteFx1.8.0.dll` (plus `UniteFx-ARM.dll` for ARM64).
  - Capture endpoint `{98f3838a-387c-4add-948d-5826c3c8d3da}` FxProperties contains
    `{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},7 = {27384E53-...}` (SFX slot).
  - UniteFx.dll implements `UniteFx`, `UniteFxAggregate`, `IAudioProcessingObjectRT`,
    `IAudioProcessingObjectConfiguration`, `CBaseAudioProcessingObject`.
  - Data plane between GUI and APO instances (in audiodg): shared mapping objects and
    mutexes named `Global\UniteFxMappingObject1.8.0`, `Global\UniteFxTransferMutex1.8.0`,
    `Global\UniteFxRegisterMappingObject1.8.0`, `Global\UniteFxCallback1.8.0`,
    `Global\UniteFxStopCallback1.8.0`, `Global\UniteFxUploadActiveMutex1.8.0`;
    log strings "Upload of chunk[%d] to APO[%d] complete", "APO[%d] did not respond in time".
  - Consequence: hearing sounds "in the microphone" of any app works without that app
    cooperating. [INF]
  - An APO must be a signed audio driver package on Windows 10/11; a from-scratch clone
    cannot ship one without WHQL signing. This is the single non-replicable piece.
- `TTS.dll` exports: `CleanUp`, `GetAvailableVoiceCount`, `GetAvailableVoices`,
  `GetPreparedSpeech`, `PrepareTextToSpeech` → SAPI voice enumeration + synthesis to PCM. [OBS]
- SoundpadService IPC: `Local\SoundpadServiceInit`, `Local\SoundpadServiceCallback`,
  `Local\SoundpadServiceCallback2`, `Local\SoundpadServiceMem` (memory-mapped struct + events). [OBS]
- Single instance: `Local\SoundpadSingleInstanceMem`, `Local\SoundpadSingleInstanceMemMutex`;
  global task mutex `SoundpadThreadManagerMutex`. [OBS]

## 4. Feature inventory (from the shipped wx translation catalogue, 776 msgids) [OBS]

**Library / list**
- Sound list of files with Title, Artist, Duration, Tag, Added on, Filename, play count, all-time play count.
- Categories: create/rename/delete, icons (stock or custom image), colors, hidden special categories,
  category indexes 01–99 usable in index hotkeys.
- Search panel (typing opens search), sorting per column, disable sorting, alternating row colors,
  compact list, pagination, zoom, columns configuration, "find dead entries" (missing files),
  fix file location, rename file, "select in explorer", drop files, insert position.

**Playback**
- Play on speakers / on microphone / both; per-sound and default play modes.
- Pause/resume, stop, play from beginning, repeat current file, repeat list, continue playback after
  current file, shuffle list, play next/previous/random/random-in-category/recently played.
- Jump back/forward by configured seconds (two steps), seek slider, playback position update rate.
- Volume: master, speakers, microphone; mute; volume up/down/set hotkeys; linear vs non-linear slider.
- "Reduce volume of other applications while playing" (Windows ducking), 50 % / 80 % / mute,
  using `HKCU\Software\Microsoft\Multimedia\Audio\UserDuckingPreference`.
- Voice-activation sound (plays a short sound before the real one to trigger others' PTT).
- Playback delay on microphone (ms) + capture line delay, for Push-to-Talk.
- Block voice while playing sounds on microphone (mic gate), optional.
- Auto stop after hotkey playback.
- Session persistence (restore playback position/status), remember hotkey state.

**Hotkeys**
- Per-sound hotkey (key + modifiers), including "press and hold" semantics; special hotkeys for
  every transport/volume/mode/record/UI action (≈70 configurable actions, see §6).
- Index hotkeys: modifier + 0–9 (two digits 01–99 optional), NumCode modifier hotkeys,
  category index routing, "same hotkeys in all categories", "allow same hotkeys".
- Left/right modifier distinction; allow/deny dangerous modifiers (Ctrl/Alt/Shift/Win) with warning;
  pass hotkeys through to other applications; hotkeys enable/disable/toggle (with sounds);
  hotkey overview window; conflict detection.

**Hotbar**
- Grid of pages (rows × cols configurable), drag & drop sounds, colors, labels, per-item hotkeys,
  page title/icon/color, pagination above/below, same colors/hotkeys on all pages, lock layout,
  play sounds by hotbar position, page navigation hotkeys, "switch hotkey moves color too".

**Sound editor**
- Waveform editing, crop/cut/paste/undo/redo, fade in/out, reverse, trim silence, normalize,
  pitch/speed, boost, karaoke (vocal removal), color tool, external editor integration,
  WAV/M4A save; length warning for very long files.

**Recorder**
- Record microphone or speakers ("what you hear"), max recording time (0 = unlimited),
  auto-normalize to target dB, trim silence, insert at beginning, save location, notification sounds.

**TTS**
- Text-to-speech panel, voice selection (SAPI voices), speed, pitch, preview, play on speakers /
  microphone, generate sound from text ("sound from text"), hotkeys for default/render/capture.
- SSML template observed: `<speak version="1.0" xmlns=...><prosody rate="%s" pitch="%s">%s</prosody></speak>`.

**Auto Keys**
- Automatically press and hold keys while a sound plays (simulated PTT), press before/after,
  press only when played by hotkey, choose target application (foreground/running), enable/disable/toggle.

**UI / system**
- Toolbar (size, colors), categories pane, hotbar pane, details pane, search, status bar mic level,
  always on top, minimize to tray / instead of close, start minimized, autostart with Windows,
  language (38 locales), light/dark/system theme, date format, notifications with sounds,
  update check + self-update, crash report, log level, device configuration check/repair/test wizard,
  `.Soundlist` file association, recent sound lists and backups.

**Remote control**
- Named pipe `\\.\pipe\sp_remote_control` and command line `-rc`. [OBS]
- Commands (from embedded help): `DoPlaySound(index[,speakers,mic])`,
  `DoPlaySoundFromCategory(catIndex,soundIndex[,speakers,mic])`, `DoPlayRandomSound(spk,mic)`,
  `DoPlayRandomSoundFromCategory(cat,spk,mic)`, `DoPlayPreviousSound()`, `DoPlayNextSound()`,
  `DoPlaySelectedSound()`, `DoPlayCurrentSoundAgain()`, `DoPlayPreviouslyPlayedSound()`,
  `DoAddSound(path)`, `DoRemoveSelectedEntries()`, `DoRemoveCategory()`, `DoSaveSoundlist()`,
  `GetSoundFileCount()`, `GetSoundlist()`, `GetVersion()`, `GetRemoteControlVersion()`,
  `GetCategory()`, `GetCategories()`, `GetPlaybackDurationInMs()`, `GetPlaybackPositionInMs()`,
  `GetRecordingPositionInMs()`, `GetRecordingPeak()`, `GetStatusBarText()`, `GetTitleText()`,
  `GetPlayStatus()`, `GetVolume()`, `IsMuted()`, `IsAlive()`, `SetVolume()`, `IsTrial()`,
  `DoRedo()`.
- **Response framing is confirmed**: optional payload line(s), then a line `R<code>`; success is
  `R0`, errors use literal codes `R-200`, `R-204`, `R-400`, `R-403`, `R-404`, `R-409`, `R-500`
  (all present as UTF-16 literals). Duration responses use the line format `": %d ms"`.
  `R-404` = command not found, `R-403` = not available (CLI/trial), `R-204` = malformed call.
- Error strings: `Unknown parameter: %s`, `Call has wrong format: %s`,
  `: Command not found.`, `: Command not available from command line.`,
  `: Command not available in trial version.`, `Remote control receive/response failed: %x`.
- URI scheme `soundpad://` and `soundlist/` handler. [OBS]

## 5. On-disk formats [OBS: live files from this machine]

`%APPDATA%\Leppsoft\soundlist.spl` (also `%USERPROFILE%\Documents\soundpad\soundlist.spl`),
`stats.spl`, `backup-soundlist-YYYY-MM-DD.spl`. XML, UTF-8, CRLF, 2-space indent:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<Soundlist>
  <Sound hash="649904814-131072-126609" url="C:\...\file.mp3" artist=""
         title="file" duration="0:02" addedOn="2026-10-04"
         color="84B940" customTag="free-form tag"/>
  ...
  <Categories>
    <Category type="1" icon="stock_icon_cat_list" hidden="true"/>
    <Category name="Home" icon="stock_icon_cat_home">
      <Sound id="41"/><Sound id="83"/>
    </Category>
    <Category name="Aura" icon="gen_A" color="7300AB"/>
  </Categories>
  <Hotbar>
    <HotbarPage/>
  </Hotbar>
</Soundlist>
```

- A sound's identity is its 0-based position in document order: Soundpad writes **no `id`
  attribute** on `<Sound>` elements (verified on both live lists). `<Sound id="N"/>` inside
  `<Category>` is a reference to that position. [OBS]
- Optional sound attributes seen in a second live list: `color="RRGGBB"` and
  `customTag="..."` (free-form tag shown in the Tag column). [OBS]
- `duration` is `m:ss` (minutes may exceed 59). `addedOn` is `YYYY-MM-DD`. [OBS]
- `hash` = `A-B-C` and the algorithm is **recovered exactly**:
  - `A` = volume serial number of the file's volume, decimal (`0x26BCC2AE` → `649904814`);
  - `B` = `nFileIndexHigh`, `C` = `nFileIndexLow` from `GetFileInformationByHandle`
    (`BY_HANDLE_FILE_INFORMATION`), decimal. `B` is therefore a multiple of 65536.
  - Verified against live list entries (e.g. file id `0x000200000001ee91` →
    `649904814-131072-126609`). RustPad reproduces this, so new files get Soundpad-compatible
    hashes and `stats.spl` works in both directions.
- `color` is `RRGGBB` uppercase hex. Icons: `gen_<letter>`, `stock_icon_cat_*`, custom paths. [OBS]
- `type="1" hidden="true"` is the built-in **"All sounds"** view: it has no `<Sound>`
  children and implicitly contains the whole list. It is not a blacklist. [OBS: it is the
  first category of both live lists and all real categories are separate.]
- Hotbar item schema (from Soundpad's UTF-16 literal pool): element `HotbarItem`, attribute
  `soundId`, plus `altCode`; page element `HotbarPage`. The pool order is
  `SoundFile`, `HotbarItem`, `HotbarPage`, `altCode`, `id`, `soundId`. RustPad writes
  `<HotbarItem soundId="..">` and reads `soundId`/`id`/`<Sound id>`; unknown attributes are
  preserved. `altCode` is preserved but not interpreted.

`stats.spl`:
```xml
<Stats><Sound hash="..." playCount="2" lastPlayedOn="2026-10-16"/></Stats>
```

## 6. Configuration [OBS: live `HKCU\Software\Leppsoft\Soundpad`]

`wxFileConfig` registry layout, groups `MainFrame`, `MainFrame\Hotbar`, `MainFrame\SoundTable`,
`MainFrame\CategoryTable`, `MainFrame\session`, `ConfigFrame`.
Representative keys (complete dump: `docs/evidence/hkcu-leppsoft.txt`):

```
playKey=97 stopKey=98, pauseResumeKey, playNextKey, ... (VK codes; 97='A', 98='B')
*KeyModifiers  (bitmask: 1=Shift? 2=Ctrl? 4=Alt? 8=Win? -> [UNK], import is best-effort)
hotkeyIndexModifier=3, hotkeyNumCodeModifier=1, enableHotkeyIndexModifier, enableHotkeyNumCodeModifier
enableCategoryIndex, enableCategoryIndexTwoDigits, distinguishLeftRightModifiers, allowSameHotkeys
normalizeVolume=1, normalizeTargetDecibel=89, averageSoundfileDecibel=96, fixedVolumeAdjustmentDecibel=-15
filterBlockVoice, delayPlaybackInMs, captureLineDelayInMs, reduceOtherVolume, windowsDuckingPreference
autoNormalizeRecordings=1, recordingTargetDecibel=89, recordingLimitInSeconds=300,
recordingFileFormat=M4A, trimRecordingSilence=1, recordingSaveLocation
playMode=0 (default), volume: playVolume=50
hotbar: pages, rows=2, cols=6, page-title-N, page-icon-N, page-color-N, style, itemLines, locked,
        sameColors, showPagination, selectSoundOnPlayback
toolbarSize, iconColor, zoom, showAlwaysOnTop, minimizeToTray, minimizeInsteadOfClose, showTrayIcon,
startMinimized, language, theme, dateFormat, alternatingRowColors, typingOpensSearch,
autoStopByHotkey, linearVolumeSlider, seekSliderFps, displayToolbarMicVolume, rememberHotkeysEnabled,
passThroughHotkey, hotkeysEnabled, autoKeysEnabled, showNotifications, logRcCalls, quickAccessColors
```

Notification sounds are relative paths under the install dir (`\sounds\notify\*.m4a`).

## 7. Bundled assets [OBS]

- `sounds/*.mp3`: `ba dum tss`, `cue`, `firework`, `scream`, `white_noise`.
- `sounds/notify/*.m4a`: enabled/disabled, auto-keys on/off, hotkeys on/off, speakers/microphone,
  recording start (speakers/mic) and finished, hotbar pages 1–9, default.
- 38 locale directories, each `translation.mo` (wx `msgid` = English UI string).

## 8. What the Rust reconstruction must do differently

| Soundpad mechanism | RustPad approach |
|---|---|
| Signed SFX APO injects PCM into the capture endpoint | Mix microphone + sounds and render to a user-selected **virtual audio device** (VB-CABLE / VoiceMeeter / SteelSeries Sonar). Apps pick that device as microphone. No driver signing required. |
| Steam licensing | Not implemented (no DRM); full features free. |
| MF + bundled codecs | `symphonia` (WAV/MP3/FLAC/OGG-Vorbis/AAC-in-MP4). |
| wxWidgets UI | `egui`/`eframe`. |
| Shared-memory APO transport | In-process mixer feeding WASAPI streams via `cpal`; separate speakers and mic-target streams. |

## 9. Tooling limits (honest coverage statement)

- REA MCP server was not registered in this client; the installed **REA CLI 6.7.0** was used, plus
  direct PE/format/registry inspection. `rea inspect` returned `provider_unavailable`: no Ghidra
  (`GHIDRA_INSTALL_DIR` unset and no Ghidra on disk), Hopper unsupported on win32, IDA/Kuna not
  configured. **No decompilation or control-flow recovery was performed**, so function-level
  behavior is [INF] from strings, exports, imports, resources, formats and runtime state.
- Static JS/Electron routes do not apply (native app).
- Deep analysis of the APO data plane (shared-memory layout, chunk protocol) is [UNK].
- Hotbar `altCode` semantics and the exact page-level attribute set remain [UNK]; they are
  preserved verbatim on round-trip.
- Hotkey modifier bitmask mapping (`Shift=1, Ctrl=2, Alt=4, Win=8`) is [INF] from behaviour.
