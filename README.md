# wavemix

A per-app and per-device audio mixer for Windows: a Voicemeeter-style console of fader strips with
live meters, and an EarTrumpet-style compact list. It controls Windows' own audio (Core Audio /
WASAPI sessions), so it needs no driver.

**Status: early.** The model, UI and mock engine are tested here; the WASAPI engine type-checks for
Windows but has not yet been run on a real machine.

## Features

- Console view: every playback and recording device as a group, with one strip per app on that
  device plus the device master. Vertical faders, VU meters with peak hold, mute, scroll to adjust,
  double-click to reset.
- Compact view: pick a device, adjust its volume and each app's volume in a narrow list.
- Several streams from one app (e.g. a browser) are merged into one strip.
- Make any device the Windows default.
- `--demo` runs against animated fake devices; this is also the only mode off Windows.

## Not yet

Tray icon and flyout, per-app output device routing, app icons, hotkeys, profiles/presets,
a software mix engine (buses, EQ) on a virtual cable.

## Build

    cargo run --release            # Windows
    cargo run -- --demo            # anywhere eframe runs
