# Architecture

## Crates

| Crate | Responsibility | Platform |
|---|---|---|
| `rustpad-core` | model, `.spl`/stats XML I/O, config, hotkey model | any |
| `rustpad-audio` | decoding, DSP, mixer, WASAPI engine via cpal | any (audio on Windows) |
| `rustpad-platform` | pipe server/client, keyboard hook, SAPI TTS, registry | Windows |
| `rustpad-app` | egui UI, CLI, wiring | Windows |

Dependency direction: `app → audio, core, platform`; `platform → core`; `audio → core` (types only).
The audio engine has no knowledge of the sound list or the UI.

## Runtime data flow

```
              ┌──────────────────────────── rustpad-app (UI thread) ───────────────────────────┐
              │  egui  ◄── AppState ──► Soundlist/Stats/AppConfig (files, registry-imported)    │
              │             │  ▲                                                               │
              │             │  └── EngineEvent / PlaybackStatus / levels                      │
              │             ▼                                                                   │
              │        EngineHandle (channels)                                                  │
              └─────────────┬───────────────────────────────────────────────────────────────────┘
                            │ EngineCommand (play, stop, volume, devices, record)
                            ▼
              ┌──────────────── rustpad-audio engine thread ────────────────┐
              │  speakers Output: Mixer ─► cpal/WASAPI render stream        │
              │  mic Output:      Mixer + MicBus ─► cpal/WASAPI render      │
              │  capture: cpal/WASAPI input ─► resample ─► MicBus/Recorder  │
              └──────────────────────────────────────────────────────────────┘

 hotkey thread  ─┐
 pipe thread    ─┼─► AppState::tick() ─► EngineHandle / list mutations
 tray/UI events ─┘
```

### Playback

`PlayRequest { sound_id, source, gain, targets, looping, start_ms }` is sent to the engine.
- `source = File(path)` → the engine decodes on a helper thread.
- `source = Decoded(Arc<DecodedSound>)` → used for TTS and editor previews.

Each target has its own `Mixer` (they may run at different sample rates). The mixer keeps
voices at native source rate and performs Catmull-Rom interpolation per output frame, so a
44.1 kHz MP3 plays correctly on a 48 kHz cable and a 48 kHz speaker at the same time.
Gain is computed in the app (normalisation) and per-target volume is the mixer master.

### Microphone path

The capture callback downmixes to mono, resamples to the microphone output rate, and pushes
into a bounded ring buffer (`MicBus`, 3 s) that the microphone output callback drains and adds
to the mix. When `block_voice` is on and a sound is audible, passthrough is muted for those
frames. The same resampled stream feeds the recorder.

The *Delay* setting is applied to **microphone-targeted sounds**: `Mixer::play_delayed` emits
silence for the configured time before the first sample, which is what makes push-to-talk
timing work. The voice path itself is not delayed.

RustPad auto-selects a known virtual cable (`CABLE Input`, `VoiceMeeter Input`, `Virtual Audio
Cable`, …) as the microphone output at startup and after a device rescan, but never falls back to
speakers. If no cable is installed, microphone passthrough has no destination: sounds still play
on the speakers, and a banner/`How to set up` dialog explains that a cable is required (Soundpad
does not need one because it uses a signed APO).

### Play generations and "stop previous"

When *Stop previous sound* is enabled (default), the app sends `StopAll` immediately before
`Play`. Because file decoding runs on worker threads, every `Play` and `StopAll` also bumps an
engine generation; a decode that finishes for an older generation is dropped instead of
starting mid-way through the new sound. `StopSound(id)` stops only the voices tagged with that
sound id, so push-to-talk release cannot silence overlapping sounds.

### Auto Keys, loopback and file identity

**Auto Keys** (`rustpad-platform::autokeys`) synthesizes a configurable key + modifiers with
`SendInput` for as long as a sound plays and releases it on stop/finish. It can be limited to
hotkey-triggered playback, which is the usual push-to-talk setup. Because the global hook never
swallows keys, the synthetic key still reaches the focused game/voice app.

**Recorder** has two sources. Microphone capture uses the existing cpal input stream;
*Speakers* starts `rustpad-audio::loopback`, a dedicated WASAPI thread on the default render
endpoint (`AUDCLNT_STREAMFLAGS_LOOPBACK`, with `AUTOCONVERTPCM` when available) that converts the
mix format to f32 and writes a WAV on stop. Both sources share the same limit/trim/normalise path.

**Sound identity** is not a content hash: `rustpad-platform::fileid::soundpad_hash` returns
`<volume serial>-<file id high>-<file id low>` from `GetFileInformationByHandle`, exactly the
`hash` Soundpad writes, so imported and newly added files share one play-count database.

### Threading and real-time safety

- The audio callbacks lock one `Mixer`/`MicBus` mutex and render into a scratch Vec; the mixer
  never allocates in `render` after warm-up (finished voices go to a fixed array).
- The engine thread handles commands every 10 ms, publishes status, enforces recording limits
  and writes the recording WAV outside the audio callback.
- Hotkeys arrive from a `WH_KEYBOARD_LL` hook thread over a channel; the pipe server answers on
  its own thread and synchronises with the UI through a request/reply channel with a 3 s timeout.

## Sound list format

See `docs/EVIDENCE.md` §5. Key mapping:

| `.spl` | model |
|---|---|
| `<Sound hash url artist title duration addedOn color customTag>` (no id) | `Sound` |
| `<Categories><Category name icon color type hidden>` + `<Sound id>` refs | `Category` |
| `<Hotbar><HotbarPage>` + `<HotbarItem>`/`<Sound id>` | `HotbarPage`, `HotbarItem` |
| `<Stats><Sound hash playCount lastPlayedOn>` | `Stats` |

IDs are assigned at import (`next_id`); the writer emits `id` in document order and references
by those ids. Deleted sounds are removed from categories and hotbar.

## Configuration

`%APPDATA%\RustPad\config.json` (`AppConfig`). Defaults point the sound list and stats at
`%USERPROFILE%\Documents\soundpad\` so an existing Soundpad installation is picked up. Audio
devices are stored by WASAPI friendly name; if a device disappears the engine falls back to the
system default and reports an event.

## Feature parity and gaps

Implemented: list/categories/hotbar, playback routing, volumes, normalisation, delay, block
voice, hotkeys (combos, modifier-only, index, numpad), Auto Keys (`SendInput`), recorder
(microphone **or** WASAPI loopback), exact Soundpad file hashes, TTS, editor, remote control,
single instance, autostart, ducking, themes, Russian/English UI.

Deliberate differences and remaining gaps:

| Soundpad feature | Status |
|---|---|
| Signed SFX APO injection into every app's mic | replaced by virtual-cable routing; driver signing not feasible |
| Steam licensing / trial limits | intentionally absent |
| Opus decoding | Symphonia codec not enabled |
| Hotbar `altCode` / page attribute semantics | preserved verbatim, not interpreted |
| Hotkey modifier bitmask import | inferred mapping (`Shift=1, Ctrl=2, Alt=4, Win=8`) |
| Category index (two-digit) hotkey routing | config stored, not applied |
| Tray icon / minimize to tray | no tray integration yet (window minimize only) |
| Update checker, crash reporter, `.Soundlist` file association | not implemented |
| UIAccess hotkeys over elevated windows | requires signed manifest/UIAccess |

## Testing

- `cargo test`: unit tests for the XML formats, DSP, resampler, mixer, matrix parser.
- `cargo run -- --list-devices`: enumerates WASAPI endpoints.
- `cargo run -- --selftest-audio`: opens the default output/capture, plays a generated tone,
  records 1 s and writes a WAV (requires audio devices).
- `rustpad -rc "GetVersion()"` against a running instance.
