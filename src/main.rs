#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod backend;
mod config;
mod model;
mod ui;

use config::Config;
use ui::MixerApp;

fn main() -> eframe::Result {
    let config = Config::load();
    // The real engine needs Windows; elsewhere (and with --demo) run against animated fake data.
    let demo = !cfg!(windows) || std::env::args().any(|a| a == "--demo");

    let [w, h] = MixerApp::initial_size(config.view);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("wavemix")
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
            Ok(Box::new(MixerApp::new(cc, handle, config, demo)))
        }),
    )
}
