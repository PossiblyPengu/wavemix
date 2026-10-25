#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// Newer clippy wants `as_chunks` for fixed-size chunk loops; the audio code uses `chunks_exact(2)`
// on purpose (it works on every toolchain), so the lint is allowed rather than chased.
#![allow(unknown_lints)]
#![allow(clippy::chunks_exact_to_as_chunks)]

mod backend;
mod config;
mod engine;
#[cfg(windows)]
mod hotkeys;
#[cfg(windows)]
mod instance;
mod model;
mod osd;
mod preset;
mod remote;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod tray;
mod ui;
mod update;
mod voicemeeter;
#[cfg(windows)]
mod winutil;

use config::Config;
use ui::MixerApp;

/// `--dump`: write what the audio engine sees to a text file and exit. The release exe has no
/// console, so this is how to report what a given machine looks like.
fn dump() -> std::io::Result<()> {
    use model::{Flow, Snapshot};
    use std::collections::HashMap;
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    let mut engine = engine();
    engine.tick(true);
    let mut snap: Snapshot = engine.tick(true);
    // Watch the meters for a few seconds, so the report says whether they ever moved. Play some
    // audio while it runs.
    let mut device_peak: HashMap<String, f32> = HashMap::new();
    let mut session_peak: HashMap<String, f32> = HashMap::new();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(4) {
        snap = engine.tick(false);
        for d in &snap.devices {
            let e = device_peak.entry(d.id.clone()).or_default();
            *e = e.max(d.peak);
        }
        for s in &snap.sessions {
            let e = session_peak
                .entry(format!("{}|{}", s.device_id, s.id))
                .or_default();
            *e = e.max(s.peak);
        }
        std::thread::sleep(Duration::from_millis(33));
    }
    let mut out = String::from(
        "Highest meter reading seen over 4 s (0.000 = silent, 1.000 = full scale).\n\n",
    );
    for d in &snap.devices {
        let flow = if d.flow == Flow::Render { "out" } else { "in " };
        let _ = writeln!(
            out,
            "[{flow}] {} vol={:.2} muted={} default={} peak={:.3}\n      id={}",
            d.name,
            d.volume,
            d.muted,
            d.is_default,
            device_peak.get(&d.id).copied().unwrap_or(0.0),
            d.id
        );
    }
    for s in &snap.sessions {
        let _ = writeln!(
            out,
            "  app {:?} exe={:?} pid={} vol={:.2} muted={} active={} peak={:.3} device={}",
            s.name,
            s.exe,
            s.pid,
            s.volume,
            s.muted,
            s.active,
            session_peak
                .get(&format!("{}|{}", s.device_id, s.id))
                .copied()
                .unwrap_or(0.0),
            s.device_id
        );
    }
    let _ = writeln!(out, "\n{}", engine.diagnostics());
    let path = std::env::temp_dir().join("wavemix-dump.txt");
    std::fs::write(&path, out)?;
    println!("{}", path.display());
    Ok(())
}

#[cfg(windows)]
fn engine() -> Box<dyn backend::Engine> {
    Box::new(backend::wasapi::WasapiEngine::new())
}

#[cfg(not(windows))]
fn engine() -> Box<dyn backend::Engine> {
    Box::new(backend::mock::MockEngine::new())
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--dump") {
        if let Err(e) = dump() {
            eprintln!("dump failed: {e}");
        }
        return Ok(());
    }
    // Only one Wavemix at a time; a second launch just brings the first one up.
    #[cfg(windows)]
    if !instance::claim() {
        return Ok(());
    }
    // If something goes badly wrong while Windows' sound is routed through the mixer, put it back
    // on the speakers it came from rather than leaving the system silent.
    #[cfg(windows)]
    {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            backend::wasapi::restore_playback_after_crash();
            previous(info);
        }));
    }
    let config = Config::load();
    // The real engine needs Windows; elsewhere (and with --demo) run against animated fake data.
    let demo = !cfg!(windows) || std::env::args().any(|a| a == "--demo");

    // Launched at login: start hidden in the tray.
    let background = cfg!(windows) && std::env::args().any(|a| a == "--background");
    // Simple mode is the taskbar flyout, so its window starts hidden and rises from the tray when
    // shown; so does a launch at login. (If there's no tray, it is shown straight away.)
    let start_hidden = background || (cfg!(windows) && config.view == config::View::Compact);

    let [w, h] = MixerApp::size_for(&config);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Wavemix")
            .with_visible(!start_hidden)
            .with_inner_size([w, h])
            .with_min_inner_size([340.0, 420.0]),
        ..Default::default()
    };

    // The mixing engine: real devices on Windows, simulated ones elsewhere and in --demo.
    let engine = engine::runtime::Engine::start(
        move || -> Box<dyn engine::io::AudioIo> {
            #[cfg(windows)]
            if !demo {
                return Box::new(engine::cpal_io::CpalIo::new());
            }
            Box::new(engine::sim::SimIo::new())
        },
        config.engine.io.clone(),
        config.engine.params.clone(),
    );
    engine.set_latency_ms(config.engine.latency_ms);

    eframe::run_native(
        "wavemix",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            #[cfg(windows)]
            instance::on_open_request({
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            });
            let repaint = move || ctx.request_repaint();
            let handle = if demo {
                backend::spawn(repaint, backend::mock::MockEngine::new)
            } else {
                #[cfg(windows)]
                {
                    backend::spawn(repaint, backend::wasapi::WasapiEngine::new)
                }
                #[cfg(not(windows))]
                unreachable!("demo is always set off Windows")
            };
            Ok(Box::new(MixerApp::new(
                cc, handle, engine, config, demo, background,
            )))
        }),
    )
}
