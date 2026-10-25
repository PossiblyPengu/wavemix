# Wavemix

A per-app and per-device audio mixer for Windows: a Voicemeeter-style console of fader strips with
live meters, and an EarTrumpet-style compact list. It controls Windows' own audio (Core Audio /
WASAPI sessions), so it needs no driver.

**Status: early.** The mixing engine (DSP, clock-drift handling, mixer, threads) is unit- and
end-to-end-tested against simulated devices. The Windows device I/O, tray, hotkeys and app routing
are only exercised by running it on Windows.

## Two modes

* **Simple** (the default): a small volume control in the spirit of EarTrumpet. Click the tray icon
  and it rises out of the taskbar beside the icon (on whichever edge your taskbar is on), shows the
  output you're listening on and a slider for every app that's making sound, and dismisses when you
  click away or press Esc. It has no taskbar button of its own, and it is as tall as its content.
  The mixing engine stays off.
* **Pro**: a software mixer in the Voicemeeter Banana mould, described below. Opening it for the
  first time picks sensible devices and starts the engine. An optional **Apps panel** beside the
  console sends programs into virtual cables.

Switch with **Simple | Pro** at the top; the window resizes to fit.

## How Pro mode works

    mic / line-in ─┐                        ┌─> A1  speakers
    mic / line-in ─┤                        ├─> A2  headphones
    mic / line-in ─┼─> strip: gate -> EQ -> compressor -> gain/pan -> routing ─┼─> A3  another output
    VIRTUAL 1 ─────┤        (per input)     ├─> B1  virtual cable (Discord, OBS, ...)
    VIRTUAL 2 ─────┘                        └─> B2  virtual cable

* **Inputs** are five strips: three hardware (microphones, line-in) and two virtual.
* **Virtual inputs** are virtual audio cables such as [VB-Cable](https://vb-audio.com/Cable/)
  (free). Use Pro's **Apps panel** (or the output pill in Simple mode) to send any program to the cable's playback end; the mixer picks it
  up from the recording end and treats it like any other input. Nothing else needs a driver.
* **Buses** A1-A3 play to hardware, B1-B2 to virtual cables, so the mix can be heard in the room
  and fed into chat or streaming software at once. Each bus has a gain, mute and a limiter.
* **Each strip** has gain (-60 to +12 dB), balance, mute, solo, mono, a send to every bus, a
  3-band EQ with a live response curve you drag, a noise gate and a compressor.
* Devices run on their own clocks, so each is resampled and drift-corrected continuously. The
  engine mixes at 48 kHz in 10 ms blocks; **Latency** (20-100 ms per device link) trades response
  time against the risk of glitches, and the status bar counts any dropouts.

## Features

- **Pro console**: the mixer described above, with stereo peak meters on the same dB scale as the
  faders, magnetic unity (0 dB) on the faders, and status dots showing whether each device is
  running. A first-run setup picks your default microphone and speakers (and a cable if one is
  installed) so it works without any configuration.
- **Feedback guard**: a strip that listens to a cable can't safely feed the bus playing into the
  same cable; the send button turns red and says why.
- **Simple mode** (also the tray flyout, and Pro's Apps panel): per-app volume and mute with a live level, app icons and
  friendly names, and a pill showing which output each app plays through. Click the pill to move
  the app to another device, including a virtual cable (experimental; uses an undocumented
  Windows interface laid out for Windows 10 21H2 / Windows 11).
- Make any device the Windows default.
- Presets: save the volumes and mutes under a name and re-apply them in one click.
- **Keyboard**: global shortcuts that work with Wavemix hidden, set by pressing Record in
  Settings: volume up/down/mute (defaults `ctrl+alt+up`, `ctrl+alt+down`, `ctrl+alt+m`), mute the
  microphone, next/previous output (speakers, headphones, ...), volume and mute of the app in the
  front window, and show/hide Wavemix. One button binds your keyboard's own volume keys (and
  Ctrl + them for the front app); since Wavemix then owns those keys, Windows' own volume popup
  doesn't appear. Inside the app everything is reachable with Tab, faders and sliders take the
  arrow keys (Shift = fine, Ctrl = coarse), Page Up/Down, Home and End, and `M` / `S` / `E` mute,
  solo and open the editor on the strip you're on.
- **Updates**: Wavemix checks GitHub for a newer release and offers "Update now"; the installer
  replaces the old version, keeps your settings, and Wavemix restarts. A checksum published beside
  each installer is verified before it runs.
- **Setup check** (Settings): is the virtual cable installed and enabled, is the engine on, is
  Windows' sound routed through Wavemix, each with its fix. **Presets** can apply themselves when
  an app starts playing or a device becomes the default. **Bypass FX** and a per-output **test
  click** help when tuning, clip lights hold on to overloads, and light/dark/system themes and an
  interface size are in Settings.
- **On-screen volume bar**, the one from voicemeeter-osd. It pops out of the tray icon with a
  callout tail pointing back at it, whenever any playback device's volume or mute changes (a
  hotkey, a keyboard knob, another app). Hover keeps it up, click opens Wavemix, scroll changes the
  volume, middle-click mutes, and dragging parks it anywhere. Horizontal or vertical, optional
  Windows accent colour, adjustable opacity and timeout, and it stays out of fullscreen games.
- Tray icon: click for Simple mode, right-click for Open / Simple / Pro / Quit. Closing the Pro
  window hides it to the tray.
- Start with Windows (`--background` launches straight into the tray).
- `--demo` runs against simulated devices (with deliberately drifting clocks); `--dump` writes the
  devices and apps it sees to `%TEMP%\wavemix-dump.txt`, for bug reports.

- **Sound shaping**: each strip has a gate, a 3-band EQ, a compressor and **noise removal**
  (a small neural network trained on speech, for microphones); each output has a **six-band
  parametric EQ**, a limiter and a delay. **Ducking** turns app sound down while you talk.
  **Bypass FX** skips every gate, EQ and compressor so you can hear what they do.
- **Recording**: record any output to a 24-bit WAV in `Music\Wavemix`.
- **Remote control**: an optional local web interface (off by default, this PC only, secret token)
  so a Stream Deck or script can change volumes, presets, ducking, recording and the keyboard
  actions. See Settings > Remote control for the addresses.

## Not yet

Wavemix runs in Windows' shared audio mode through a virtual cable, so it can't match Voicemeeter's
ASIO / exclusive-mode latency, and it ships no virtual-device driver of its own (install VB-Cable).
Also missing: VST plug-ins, surround or multi-channel inputs, MIDI control, a spatial-audio
effect, and a code-signed installer (the release workflow signs automatically once a certificate
is added as a repository secret).

## Switching from Voicemeeter

If Voicemeeter is installed, Wavemix offers to bring your setup over (also in Settings > Switching
from Voicemeeter). With Voicemeeter running, it reads your strips and buses through Voicemeeter's
own Remote API and translates them: gain, mute, solo, mono, balance, A/B routing, bass/mid/treble,
compressor and gate (approximated from Voicemeeter's knobs), and the hardware devices you picked.
Standard, Banana and Potato are all understood. Bus EQ, delays and macros are not imported, and
anything that doesn't fit Wavemix's 3 + 2 inputs and A1-A3 / B1-B2 buses is listed rather than
dropped silently.

It then moves Windows over so nothing goes quiet: if the default playback or recording device is
one of Voicemeeter's virtual ones, it is changed to your real speakers / a virtual cable / your
microphone *before* Voicemeeter is closed. Optionally it also stops Voicemeeter starting with
Windows and starts Wavemix instead. Every step is a checkbox.

One thing can't be translated: Voicemeeter's own virtual inputs (VAIO) can't be captured by other
programs, so apps you sent into them need a virtual cable (VB-Cable) instead.

## Levels

Meters read Windows' own level meters (apps and devices) in Simple mode, and the mixer's own
signal in Pro mode. Pro's strips only see audio routed into them, so a "System audio" input
(capturing everything a playback device is playing) is offered in every input's device menu and
set up on Virtual 2 by default, with no sends. If meters sit at zero, run `wavemix.exe --dump`
while audio is playing: it writes the highest level seen on every device and app to
`%TEMP%\wavemix-dump.txt`.

## Install

Run `wavemix-setup-<version>.exe`. It installs just for you (no administrator rights) into
`%LOCALAPPDATA%\Programs\wavemix`, adds a Start Menu shortcut, and optionally starts Wavemix with
Windows and adds a desktop shortcut. Uninstall from Settings > Apps; it asks whether to keep your
settings and presets. Upgrading is just running a newer installer over the old one. Windows
SmartScreen may warn that the installer isn't signed; choose "More info" then "Run anyway".

## Build

    cargo run --release            # Windows
    cargo run -- --demo            # anywhere eframe runs
    makensis -DVERSION=0.7.0 -DEXE=target\release\wavemix.exe installer\wavemix.nsi   # the installer
