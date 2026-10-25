//! One-click bug report: everything that has needed asking for so far, in one text file.

use std::fmt::Write as _;
use std::path::PathBuf;

use super::MixerApp;
use crate::engine::params::{bus_name, strip_name};
use crate::engine::runtime::SlotStatus;
use crate::model::Flow;

fn slot(s: &SlotStatus) -> String {
    match s {
        SlotStatus::Off => "off".into(),
        SlotStatus::Running { rate } => format!("running at {rate} Hz"),
        SlotStatus::Missing => "device not found".into(),
        SlotStatus::Error(e) => format!("error: {e}"),
    }
}

#[cfg(windows)]
fn windows_version() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .map(|k| {
            let get = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
            format!(
                "{} {} (build {})",
                get("ProductName"),
                get("DisplayVersion"),
                get("CurrentBuild")
            )
        })
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(not(windows))]
fn windows_version() -> String {
    "not Windows".into()
}

impl MixerApp {
    /// The report as text. Contains device and program names but nothing secret: no tokens, no
    /// file contents.
    pub(super) fn diagnostics_report(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "Wavemix {}", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(out, "Windows: {}", windows_version());
        let _ = writeln!(
            out,
            "View: {:?}   Theme: {:?}   Interface size: {:.0}%",
            self.config.view,
            self.config.theme,
            self.config.ui_scale * 100.0
        );

        let stats = self.engine.stats();
        let status = self.engine.status();
        let _ = writeln!(
            out,
            "\nEngine: {}   latency setting {} ms   CPU {:.0}%   late blocks {}",
            if self.engine.is_running() { "on" } else { "off" },
            self.config.engine.latency_ms,
            stats.cpu_percent,
            stats.late_blocks
        );
        for i in 0..crate::engine::params::STRIPS {
            let _ = writeln!(
                out,
                "  {:<10} {:<28} {}   underruns {}",
                strip_name(i),
                self.config.engine.io.inputs[i].as_deref().unwrap_or("-"),
                slot(&status.inputs[i]),
                stats.input_underruns[i]
            );
        }
        for b in 0..crate::engine::params::BUSES {
            let _ = writeln!(
                out,
                "  {:<10} {:<28} {}   underruns {} overruns {}",
                bus_name(b),
                self.config.engine.io.outputs[b].as_deref().unwrap_or("-"),
                slot(&status.outputs[b]),
                stats.output_underruns[b],
                stats.output_overruns[b]
            );
        }
        let (routed, cable) = self.route_state();
        let _ = writeln!(
            out,
            "\nRouting: Windows sound {}   cable playback side {}   restore-to {:?}",
            if routed { "goes through Wavemix" } else { "plays directly" },
            if cable.is_some() { "found" } else { "NOT found" },
            self.config.restore_playback
        );

        let snap = self.backend.snapshot();
        let _ = writeln!(out, "\nDevices:");
        for d in &snap.devices {
            let _ = writeln!(
                out,
                "  [{}] {}   vol {:.2}{}{}\n       {}",
                if d.flow == Flow::Render { "out" } else { "in " },
                d.name,
                d.volume,
                if d.muted { " muted" } else { "" },
                if d.is_default { " DEFAULT" } else { "" },
                d.id
            );
        }
        let _ = writeln!(out, "\nPrograms with audio sessions:");
        for s in &snap.sessions {
            let _ = writeln!(
                out,
                "  {} ({}) pid {}   vol {:.2}{}{}{}",
                s.name,
                s.exe,
                s.pid,
                s.volume,
                if s.muted { " muted" } else { "" },
                if s.active { " PLAYING" } else { "" },
                if s.has_window { " window" } else { "" }
            );
        }

        let _ = writeln!(
            out,
            "\nHotkeys: {:?}\nPresets: {}   Names given: {}",
            self.config.hotkeys.entries().map(|(l, k)| format!("{l}={k}")),
            self.config.presets.len(),
            self.config.device_names.len()
        );
        if let Some(log) = std::env::temp_dir()
            .join("wavemix-winosd.txt")
            .to_str()
            .and_then(|p| std::fs::read_to_string(p).ok())
        {
            let _ = writeln!(out, "\nWindows volume popup log:");
            for line in log.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev() {
                let _ = writeln!(out, "  {line}");
            }
        }
        out
    }

    /// Write the report to the desktop (or the temp folder if there is none) and return where.
    pub(super) fn save_diagnostics(&self) -> Result<PathBuf, String> {
        let dir = std::env::var_os("USERPROFILE")
            .map(|p| PathBuf::from(p).join("Desktop"))
            .filter(|p| p.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let path = dir.join("wavemix-diagnostics.txt");
        std::fs::write(&path, self.diagnostics_report()).map_err(|e| e.to_string())?;
        Ok(path)
    }
}
