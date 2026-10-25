# Wavemix: what's left, for a Windows machine

Written at 0.16.0 (commit `b568f6b`). Everything here needs real Windows, real hardware or a decision
from the owner, none of which the Linux build environment had.

**State of play.** CI (windows-latest: `fmt --check`, `clippy -D warnings`, `cargo test`,
`cargo build --release`) is green on 0.16.0, so the code compiles natively and the unit tests pass on
Windows, including the VBAN loopback test. What has *never* run is the app itself on a real
machine: audio devices, tray, the Windows volume popup, the updater against GitHub, a MIDI
controller, a second PC. Part 1 is that gap. Parts 2 and 3 are admin and new work.

Suggested order: Part 1 (fix what breaks) → Part 2 (release) → Part 3 in the order given.

---

## 0. Get set up (15 minutes)

- Install Rust (rustup, with the "Desktop development with C++" build tools), then
  `git clone https://github.com/PossiblyPengu/wavemix` and `cargo test`.
- Install [VB-Cable](https://vb-audio.com/Cable/). Wavemix needs it for "Route Windows audio".
- Useful extras: Audacity (check recordings), Voicemeeter (VBAN interop), loopMIDI (a virtual MIDI
  port, so MIDI can be tested without a controller), a MIDI controller if you own one.
- Run with `cargo run --release`. `--demo` runs against fake devices, `--dump` writes what Wavemix
  sees to `%TEMP%\wavemix-dump.txt`, `--settings` opens Settings straight away.
- Files to know: config `%APPDATA%\wavemix\config.json` (a broken one is kept as `config.json.bad`),
  popup-hider log `%TEMP%\wavemix-winosd.txt`, recordings `Music\Wavemix`.

---

## Part 1. Verify on real Windows

Do these in order; the first block decides whether anything else is worth testing. Note what
fails; for each failure, send the log named in brackets.

### 1a. The audio path (highest risk)

- [ ] Pro mode, engine on, mic as Input 1, headphones as A1: you hear yourself, no clicks.
- [ ] "Route Windows audio" moves Windows' default playback onto the cable; app sound plays through
      the mixer. **Quit normally, kill the process in Task Manager, and force a crash**: in all
      three cases the previous default device should come back (the crash path is a panic hook;
      Task Manager kill can't be caught, so expect to rely on the restore-on-startup path there).
- [ ] Leave it running 30 minutes with a game or video playing. The engine bar shows the dropout
      count; it should stay at 0 or settle after a few. If it climbs, note the Latency setting,
      CPU % and the device pair. (Link buffers grow by 1.5x per dropout up to 150 ms.)
- [ ] Clock drift: run a bus to a USB or Bluetooth device for an hour; no periodic crackle.
- [ ] Latency choices (Ultra 10 ms to Safe): which ones hold without dropouts on your hardware?
      Record the answers; they set the default and the README's advice.

### 1b. Known open reports

- [ ] **Windows' volume popup still appears** when Wavemix's volume bar shows. Code:
      `src/osd/winosd.rs` (a thread that hides the popup window). Run with the bar visible, change
      volume with a hotkey, then send `%TEMP%\wavemix-winosd.txt`. Check the Windows version too;
      the popup's window class has changed between Windows 10 and 11 builds.
- [ ] **Flat meters**. Run `wavemix.exe --dump`, play audio, send `%TEMP%\wavemix-dump.txt`.
      Code: `src/backend/wasapi.rs` (peak meter polling per session/device).
- [ ] App list: only open or audible apps, no duplicates across devices, Wavemix not listed. The
      Settings > "Also list background programs and services" brings the idle ones back.

### 1c. Features added since the last Windows check

- [ ] **Keyboard**: record each shortcut in Settings; the "use my keyboard's volume keys" button
      takes over `AudioVolumeUp/Down/Mute` (Windows' own popup should not appear); Ctrl + volume
      keys act on the app in front.
- [ ] **Single instance**: start it twice; the second opens the first's window and exits.
- [ ] **Tray flyout**: Simple mode rises from the tray icon on the correct monitor and edge, at
      100%, 125%, 150% and with Settings > Interface size at 1.25 and 1.5.
- [ ] **Theme**: System / Light / Dark follow Windows live, including the title bar.
- [ ] **Sound shaping** (listen, don't just look): bus EQ, noise removal on a real mic (watch CPU),
      ducking, **Bypass FX** (A/B), the output **test click** with two outputs and a delay set.
- [ ] **New in 0.16**: the **ChatMix** dial changes game and chat strips in opposite directions and
      centre is unity; **crossfeed** audibly narrows headphone stereo; **width 0%** is mono.
- [ ] **Recording**: record a bus for a minute; open the WAV in Audacity: 24-bit, 48 kHz, correct
      length, no gaps. Then quit the app *while* recording and open that file too. **Likely to fail
      today**: the WAV header (sizes) is only written when the recording is stopped
      (`WavWriter::finish` in `src/engine/record.rs`), and nothing finishes it on quit or crash. Fix:
      stop any recording in the quit path and the panic hook, and/or patch the header every few
      seconds.
- [ ] **Remote control**: enable it, then `curl "http://127.0.0.1:8765/api/state?token=..."`. Try a
      Stream Deck "website" action for `/api/preset?name=...`. Confirm requests without the token
      get 401 and that the port isn't reachable from another PC.
- [ ] **Macros**: a three-line macro runs from the Macros button and from `/api/macro?name=`.

### 1d. MIDI (new; never touched a device)

Without hardware: install loopMIDI, make a port, send messages with any MIDI utility.

- [ ] The controller or loopMIDI port is listed in Settings > MIDI control ("Connected: ...").
- [ ] **Learn**: Add a binding, move a fader, the label shows `CC n, channel m`. A pad shows `Note n`.
      A motorised fader that sends pitch bend shows `Pitch bend`.
- [ ] A fader bound to input volume moves the strip smoothly (no stutter at fast moves; the code
      coalesces a burst to its last value each frame). 100 is 0 dB, 127 is +12 dB, 0 is silence.
- [ ] A pad bound to mute toggles once per press (not on release, not when held).
- [ ] **Unplug and replug** the controller with the app running; it reconnects within ~3 seconds.
- [ ] Known risk: Windows' classic MIDI API lets only one program open an input at a time. If a
      DAW has it, Wavemix will show no connection. If that matters, switch `midir` to its `winrt`
      feature (Windows MIDI Services sharing), a small change in `Cargo.toml` and
      `src/midi_device.rs`, but re-test, as port names differ between the two backends.
- [ ] Not done: sending MIDI *back* to motorised faders / LEDs. Add it if you have such a device.

### 1e. VBAN (new)

Without a second PC: add a **receiver** (port 6980) and a **sender** to `127.0.0.1:6980` in
Settings > Network audio; pick the sender as an output bus and the receiver as an input strip. Do
not route that strip back to the same bus (the feedback guard doesn't know about network loops).

- [ ] Windows' firewall prompt appears when the receiver first listens; allow it (private networks).
      Without that, packets from another PC are silently dropped. Decide whether the installer should
      add a firewall rule (needs admin; the installer is per-user, so probably document it instead).
- [ ] Loopback: you hear the bus through the strip. Note the extra latency (a ring of the link
      latency + one packet of ~2.7 ms).
- [ ] **Voicemeeter interop** on a second PC (or the same one): Voicemeeter's VBAN receiver hears
      Wavemix's sender (stream name, port, IP), and Wavemix hears Voicemeeter's VBAN out.
- [ ] A 44.1 kHz source sounds right (cubic conversion to 48 kHz); a mono source plays on both sides.
- [ ] Over Wi-Fi: stutter? The receiver link grows its buffer on dropouts; check the count settles.
- [ ] Port in use: a second receiver on the same port shows an error on the strip, not a crash.
- [ ] Change or remove an endpoint while it is the active device; the strip should show it missing.

### 1f. Installer and updater

- [ ] Install `wavemix-setup-0.16.0.exe` on a clean user account: no admin prompt; Start menu entry;
      optional desktop shortcut; "Start with Windows" works (starts hidden in the tray).
- [ ] SmartScreen warns about the unsigned installer: expected until Part 2's signing is done.
- [ ] Upgrade from an installed 0.15.0: old one removed, settings kept, no duplicate Apps & Features
      entry. Uninstall removes the startup entry.
- [ ] **Updater, end to end** (needs two published releases): publish 0.16.0 (Part 2), install it,
      bump to 0.16.1, publish, and the running 0.16.0 should show the banner, verify the SHA-256
      sidecar, run the installer silently and relaunch with `--background`. Also test "Later",
      offline, and a corrupted download (rename the sidecar's hash) which must refuse to run.

---

## Part 2. Release and repository admin

- [ ] **Publish 0.16.0**: GitHub > Actions > Release > Run workflow > version `0.16.0`. (Nothing has
      been released since the early 0.x tags, so the in-app updater has nothing to offer yet.)
- [ ] **Licence**: the repo has none, so by default nobody may reuse it. Choose MIT or Apache-2.0
      (permissive), or GPL-3.0 (copyleft). *Check this before deciding on ASIO or VST below: some
      SDK licences constrain which licence you can ship under.* Add `LICENSE`, the `license` field in
      `Cargo.toml` and a line in the README.
- [ ] **Repo description and topics**: the description still says "OSD". Suggest: "A per-app and
      per-device audio mixer for Windows: EarTrumpet-style flyout plus a Voicemeeter-style console."
- [ ] **Code signing** (removes the SmartScreen "unknown publisher" wall once reputation builds).
      The release workflow already signs with a `.pfx` if secrets `SIGN_PFX_BASE64` and
      `SIGN_PFX_PASSWORD` exist. Options, cheapest first:
      1. **SignPath Foundation**: free signing for open-source projects (apply; needs a public repo
         with a licence, which is another reason to do the licence first). Needs a workflow change.
      2. **Microsoft's Trusted Signing** (cloud signing, low monthly cost; eligibility depends on
         country and organisation verification). Needs a workflow change.
      3. An **OV/EV certificate** from a CA (a few hundred dollars a year), then just add the secrets.
      Whichever is chosen: sign `wavemix.exe` *and* the installer, and note that SmartScreen
      reputation still takes time to accrue.
- [ ] Add a "Windows smoke test" checklist (Part 1) to the release process, or tick it per release.
- [ ] Turn on branch protection for `master` once more than one person is pushing.

---

## Part 3. New work, ranked by value for effort

### 3.1 Lower latency without ASIO: WASAPI `IAudioClient3` (medium; do this first)

Shared-mode streams can ask Windows for much smaller periods than cpal's default (often 10 ms; many
drivers allow ~2.7-5 ms) through `IAudioClient3::InitializeSharedAudioStream`, without taking the
device exclusively, so other apps keep working. This is the biggest latency win that doesn't change
the architecture.

- Implement a second `AudioIo` (`src/engine/wasapi_lowlat.rs`) next to `CpalIo`, using the `windows`
  crate directly: `GetSharedModeEnginePeriod` to read min/default period, event-driven buffer
  (`AUDCLNT_STREAMFLAGS_EVENTCALLBACK`), thread in MMCSS "Pro Audio" (already done for the mixer).
- Fall back to the cpal path per device when `IAudioClient3` isn't supported or fails.
- Re-use the existing `RateReader` for drift; make sure the link targets (`runtime.rs`
  `latency_ms`) can go below 10 ms, and re-check that "Ultra 10 ms" stays stable.
- Surface the actual period in the engine bar (the `~N ms` figure is currently an estimate:
  `latency_ms * 2 + 10`).
- **Test**: loopback measurement (play the test click out of one bus into an input via a cable and
  time it, or use a tool such as Reaper's latency tester); compare with Voicemeeter's WDM numbers.
- Exclusive mode is a separate, smaller follow-up on the same code (`AUDCLNT_SHAREMODE_EXCLUSIVE`),
  but it blocks every other app on that device. Offer it only as an explicit per-device option.

### 3.2 Multichannel audio interfaces: channel pairs (medium)

Today every device is read and written as its first two channels, which is why an 8-channel
interface is only half usable. Do this in phases:

1. **Channel-pair choice per slot**: a strip picks a device *and* a pair (1-2, 3-4, ...); a bus
   picks the output pair it plays on. Extend `io.rs` (`open_input`/`open_output` gain a channel
   offset), `cpal_io.rs` (`build_input/build_output` currently take `channels` and read `frame[0]`,
   `frame[1]`), and the config (`IoConfig` stores a device name per slot; add an offset).
   Several slots can share one open stream: open each device once.
2. **Surround to stereo**: let a strip take a 5.1/7.1 source and fold it down (standard ITU
   coefficients: centre and surrounds at -3 dB, LFE dropped or low-passed). Pure DSP, unit-testable.
3. **True multichannel buses** (a 5.1 output): only if someone needs it; it touches the mixer's
   fixed stereo buffers everywhere, so it is the large part.

### 3.3 ASIO (medium to large; only if the licence works)

- cpal has an `asio` feature (needs the Steinberg ASIO SDK, LLVM/clang for bindgen, and the
  `CPAL_ASIO_DIR` environment variable).
- **Licence**: ASIO has long been proprietary; Steinberg's current terms (including any
  open-source option such as GPLv3) must be checked before shipping. That may force the project's
  licence, so settle Part 2's licence first.
- **Architecture**: an ASIO driver is one duplex device on one clock, opened by one app. The
  engine currently opens inputs and outputs independently. Plan an "ASIO device" mode: one driver
  provides all of its channels as inputs and outputs together (pairs, as in 3.2), with the mix
  thread driven by the driver's callback rather than by `spin_sleep`. Do 3.2 first.
- Worth doing only if IAudioClient3 numbers (3.1) aren't good enough for your use.

### 3.4 Plug-in hosting: VST3 and/or CLAP (large)

- **CLAP is the cheaper start**: a simple C ABI with a permissive licence, and the Rust crate
  `clack-host` implements the host side. **VST3** has far more plug-ins but a COM-style C++ API; the
  SDK's licence has been relaxed recently (verify the current terms) and Rust bindings exist (the
  `vst3` crate), but hosting is mostly hand-written.
- Scope an MVP: one plug-in **insert slot per strip and per bus**, after the existing chain; load
  from a scan of the standard folders; processing on the mix thread with fixed 480-frame blocks;
  parameters saved as the plug-in's opaque state blob in `config.json` (or a sidecar file).
- Hard parts: the plug-in's own **GUI window** (embed in a native child `HWND` hosted next to egui,
  on the UI thread); **latency compensation** for plug-ins that report latency; **crashes**, since a
  plug-in fault takes the whole process down and `panic = "abort"` is set. The safe answer is a
  separate plug-in host process with shared-memory audio; do that only after the in-process MVP.
- Test with free plug-ins first (an EQ and a compressor), then something heavy (a reverb) for CPU.

### 3.5 Our own virtual audio device (very large; probably don't)

Today Wavemix depends on VB-Cable (detected, explained in Settings > Setup check, not bundled; its
licence doesn't allow redistribution). Options:

1. **Keep VB-Cable** (current). Cost: one extra install, and a donationware dependency.
2. **Write a driver**: Microsoft's open-source **SYSVAD** sample shows a virtual WDM audio device;
   Rust driver crates exist but are early. A kernel driver needs the Windows Driver Kit,
   test-signing for development, and for distribution **Microsoft hardware-program signing** (a
   Partner Center account and an EV certificate). That is ongoing cost, driver-testing across
   Windows versions, and security-review exposure. Only worth it if "install VB-Cable first" is the
   main reason people don't use Wavemix.
3. **Soften option 1**: detect VB-Cable and offer a one-click "install it for me" that opens the
   download page, then re-checks; plus first-run guidance. Cheap, and probably enough.

### 3.6 Smaller follow-ups noticed along the way

- Feedback guard for network loops (VBAN in and out on the same machine); today only cables and
  loopbacks are recognised (`feeds_itself` in `engine/io.rs`).
- Per-device latency readout and a "measure my latency" button (builds on the test click).
- MIDI output (LED rings, motorised-fader feedback) and MIDI-controlled macros with values.
- A tidy-up of the engine bar: it is nearly full at the default Pro width (Ducking, ChatMix,
  Record, output choice, Macros, Bypass FX); a "More" menu would make room.
- An automated Windows test job for the installer (the Wine script covers upgrade logic, not
  SmartScreen, shortcuts or the real registry).

---

## What to send back

If something in Part 1 fails, the useful artefacts are: `%TEMP%\wavemix-dump.txt` (`--dump`),
`%TEMP%\wavemix-winosd.txt`, Settings > Help > "Save a diagnostics report" (on the desktop), your
Windows version, and the device names. With those a fix can usually be written without needing the
machine.
