# wavemix

A per-app and per-device audio mixer for Windows: a Voicemeeter-style console of fader strips with
live meters, and an EarTrumpet-style compact list. It controls Windows' own audio (Core Audio /
WASAPI sessions), so it needs no driver.

**Status: early.** The mixing engine (DSP, clock-drift handling, mixer, threads) is unit- and
end-to-end-tested against simulated devices. The Windows device I/O, tray, hotkeys and app routing
are only exercised by running it on Windows.

## How it works

wavemix is a software audio mixer in the Voicemeeter Banana mould, with an EarTrumpet-style app
list alongside it.

    mic / line-in ─┐                        ┌─> A1  speakers
    mic / line-in ─┤                        ├─> A2  headphones
    mic / line-in ─┼─> strip: gate -> EQ -> compressor -> gain/pan -> routing ─┼─> A3  another output
    VIRTUAL 1 ─────┤        (per input)     ├─> B1  virtual cable (Discord, OBS, ...)
    VIRTUAL 2 ─────┘                        └─> B2  virtual cable

* **Inputs** are five strips: three hardware (microphones, line-in) and two virtual.
* **Virtual inputs** are virtual audio cables such as [VB-Cable](https://vb-audio.com/Cable/)
  (free). Use the **Apps** view to send any program to the cable's playback end; the mixer picks it
  up from the recording end and treats it like any other input. Nothing else needs a driver.
* **Buses** A1-A3 play to hardware, B1-B2 to virtual cables, so the mix can be heard in the room
  and fed into chat or streaming software at once. Each bus has a gain, mute and a limiter.
* **Each strip** has gain (-60 to +12 dB), balance, mute, solo, mono, a send to every bus, a
  3-band EQ with a live response curve you drag, a noise gate and a compressor.
* Devices run on their own clocks, so each is resampled and drift-corrected continuously. The
  engine mixes at 48 kHz in 10 ms blocks; **Latency** (20-100 ms per device link) trades response
  time against the risk of glitches, and the status bar counts any dropouts.

## Features

- **Mixer view**: the console described above, with stereo peak meters on the same dB scale as the
  faders, magnetic unity (0 dB) on the faders, and status dots showing whether each device is
  running. A first-run setup picks your default microphone and speakers (and a cable if one is
  installed) so it works without any configuration.
- **Feedback guard**: a strip that listens to a cable can't safely feed the bus playing into the
  same cable; the send button turns red and says why.
- **Apps view** (also the tray flyout): per-app volume and mute with a live level, app icons and
  friendly names, and a pill showing which output each app plays through. Click the pill to move
  the app to another device, including a virtual cable (experimental; uses an undocumented
  Windows interface laid out for Windows 10 21H2 / Windows 11).
- Make any device the Windows default.
- Presets: save the volumes and mutes under a name and re-apply them in one click.
- Global hotkeys for the default playback device (defaults `ctrl+alt+up`, `ctrl+alt+down`,
  `ctrl+alt+m`; rebind in Settings), with an on-screen volume bar ported from voicemeeter-osd.
- Tray icon with a compact flyout; closing the window hides it to the tray.
- Start with Windows (`--background` launches straight into the tray).
- `--demo` runs against simulated devices (with deliberately drifting clocks); `--dump` writes the
  devices and apps it sees to `%TEMP%\wavemix-dump.txt`, for bug reports.

## Not yet

Hotkeys that drive bus A1 instead of the Windows volume, macro buttons, a parametric EQ with more
bands, bus EQ, ASIO/WDM-exclusive devices for lower latency, and recording.

## Build

    cargo run --release            # Windows
    cargo run -- --demo            # anywhere eframe runs
