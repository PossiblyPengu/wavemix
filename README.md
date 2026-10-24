# wavemix

A per-app and per-device audio mixer for Windows: a Voicemeeter-style console of fader strips with
live meters, and an EarTrumpet-style compact list. It controls Windows' own audio (Core Audio /
WASAPI sessions), so it needs no driver.

**Status: early.** Logic and UI are unit-tested; the Windows audio, tray, hotkey and routing code is
only exercised by running it on Windows.

## Features

- Mixer view: apps on the left, playback devices and microphones on the right, in the spirit of
  Voicemeeter. Every app has a fader, live meter with peak hold, mute, a status dot (playing or idle)
  and a pill showing which device it plays through. Scroll to adjust, double-click for 100%.
- Compact view, in the spirit of EarTrumpet: the output you're on at the top, then every playing app
  with a slider and its device pill; idle apps are tucked under "Not playing".
- Several streams from one app (e.g. a browser) are merged into one strip.
- Make any device the Windows default.
- Tray icon: click it for a compact flyout above the icon (dismissed by clicking away), right-click for
  Open mixer / Quit. Closing the window hides it to the tray.
- `--demo` runs against animated fake devices; this is also the only mode off Windows.

- App icons and friendly names ("Google Chrome", not `chrome`), read from each app's exe.
- Presets: save the current volumes and mutes under a name and re-apply them in one click. Apps or
  devices that aren't present are skipped.
- Global hotkeys for the default playback device (defaults `ctrl+alt+up`, `ctrl+alt+down`,
  `ctrl+alt+m`; rebind in Settings). Reports a hotkey another program already owns.
- On-screen volume display when a hotkey changes the volume: the bar from voicemeeter-osd, ported
  over. It follows the default playback device, shows a live level meter, never takes focus, ignores
  clicks, and stays away from fullscreen games. Horizontal or vertical, adjustable opacity.
- Start with Windows (per-user Run key, launches straight into the tray).
- Per-app output routing (**experimental**, off by default; enable in Settings). Adds an Output menu
  to each app. It uses an undocumented Windows interface laid out for Windows 10 21H2 / Windows 11;
  on other builds it silently does nothing.
- `--background` starts hidden in the tray; `--dump` writes the devices and apps it sees to
  `%TEMP%\wavemix-dump.txt`, for bug reports.

## Not yet

A software mix engine (buses, EQ) on a virtual cable.

## Build

    cargo run --release            # Windows
    cargo run -- --demo            # anywhere eframe runs
