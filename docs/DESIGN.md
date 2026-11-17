# RustPad interface direction

Reading this as: a Windows soundboard for streamers and voice-chat users, in a studio-console visual language.

Dial: ENERGY 2 / RHYTHM 2 / MOTION 1.

## Why this direction

Soundpad users work in two modes: browsing a library and firing sounds while a game or call is
running. The interface therefore has one dense administration surface (the list) and one
persistent performance surface (transport and hotbar). The visual language comes from a mixing
desk: channel numbers, monospace time, signal bars, clear sections and very little decoration.

## Decisions

- **Black field, warm graphite surfaces.** The app is used beside a game, where a true black
  field avoids glare next to a bright scene. Warm white text reduces the blue cast of a pure
  greyscale UI.
- **Warm white is the interaction colour.** White is used for play, primary actions and the
  selected row marker. Colour is reserved for state: green for microphone and output signal,
  amber for warnings, red for missing files and recording.
- **Segoe UI and Consolas.** Both ship with Windows. Segoe UI Semibold makes hierarchy without
  loading a webfont; Consolas makes durations, counters and hotkeys align in columns.
- **Signal bars are the identity motif.** The same three-bar mark appears in the logo, the sound
  tiles and the playing indicator. It is specific to audio and costs one painter call.
- **The table is the library, the dock is the instrument.** The table is quiet and scannable;
  the transport carries the only glow and the largest control.
- **Cards are flat.** A one-pixel top highlight and a hairline border separate surfaces. No
  page-wide gradients, glass, grid backgrounds or shadow stacks.
- **Dark only.** The custom shell is painted for one palette. Shipping a light toggle that was
  never painted would be a broken mode, not a feature.

## Data honesty

Every number in the UI comes from the loaded sound list or the audio engine. The screenshot in
the README uses a clearly labelled demo library, not a customer list. There are no testimonials,
uptime claims or invented usage numbers.

## Motion

Motion is limited to a short entrance, hover transitions and live playback feedback. The
playing halo and equalizer are state, not decoration. No perpetual loops when nothing is playing.
