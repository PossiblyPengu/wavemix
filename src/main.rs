#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod backend;
mod config;
#[cfg(windows)]
mod hotkeys;
mod model;
mod osd;
mod preset;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod tray;
mod ui;

use config::Config;
use ui::MixerApp;

/// `--dump`: write what the audio engine sees to a text file and exit. The release exe has no
/// console, so this is how to report what a given machine looks like.
fn dump() -> std::io::Result<()> {
    use model::{Flow, Snapshot};
    use std::fmt::Write as _;

    let mut engine = engine();
    engine.tick(true);
    let snap: Snapshot = engine.tick(true);
    let mut out = String::new();
    for d in &snap.devices {
        let flow = if d.flow == Flow::Render { "out" } else { "in " };
        let _ = writeln!(
            out,
            "[{flow}] {} vol={:.2} muted={} default={}\n      id={}",
            d.name, d.volume, d.muted, d.is_default, d.id
        );
    }
    for s in &snap.sessions {
        let _ = writeln!(
            out,
            "  app {:?} exe={:?} pid={} vol={:.2} muted={} active={} device={}",
            s.name, s.exe, s.pid, s.volume, s.muted, s.active, s.device_id
        );
    }
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
    let config = Config::load();
    // The real engine needs Windows; elsewhere (and with --demo) run against animated fake data.
    let demo = !cfg!(windows) || std::env::args().any(|a| a == "--demo");

    // Launched at login: start hidden in the tray.
    let background = cfg!(windows) && std::env::args().any(|a| a == "--background");

    let [w, h] = MixerApp::initial_size(config.view);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("wavemix")
            .with_visible(!background)
            .with_inner_size([w, h])
            .with_min_inner_size([340.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "wavemix",
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
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
                cc, handle, config, demo, background,
            )))
        }),
    )
}
