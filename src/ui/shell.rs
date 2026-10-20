//! Windows shell integration: the tray icon, the flyout popup, and hiding to the tray on close.

use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2};

use super::{MixerApp, Mode};
use crate::tray::{Anchor, Tray, TrayAction};

/// Flyout popup size, in points.
const FLYOUT_SIZE: [f32; 2] = [360.0, 520.0];
/// A click on the tray icon steals focus, which dismisses the flyout, and the click's release then
/// arrives as a "toggle". Releases this soon after a dismissal are that same click.
const DISMISS_GRACE: Duration = Duration::from_millis(350);
/// Focus can take a moment to arrive after showing; don't dismiss before it has.
const FOCUS_GRACE: Duration = Duration::from_millis(400);

pub struct ShellState {
    tray: Option<Tray>,
    visible: bool,
    quitting: bool,
    shown_at: Instant,
    hidden_at: Option<Instant>,
    focused_once: bool,
}

impl ShellState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            tray: Tray::new(ctx),
            visible: true,
            quitting: false,
            shown_at: Instant::now(),
            hidden_at: None,
            focused_once: false,
        }
    }
}

impl MixerApp {
    pub(super) fn handle_shell(&mut self, ctx: &egui::Context) {
        let actions = self
            .shell
            .tray
            .as_ref()
            .map(|t| t.poll())
            .unwrap_or_default();
        for action in actions {
            match action {
                TrayAction::ToggleFlyout(anchor) => {
                    let just_dismissed = self
                        .shell
                        .hidden_at
                        .is_some_and(|t| t.elapsed() < DISMISS_GRACE);
                    if self.shell.visible && self.mode == Mode::Flyout {
                        self.hide(ctx);
                    } else if !just_dismissed {
                        self.show_flyout(ctx, anchor);
                    }
                }
                TrayAction::OpenWindow => self.show_window(ctx),
                TrayAction::Quit => {
                    self.shell.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // The close button hides to the tray, but only if there is a tray to come back from.
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.shell.quitting && self.shell.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }

        // Dismiss the flyout when the user clicks elsewhere.
        if self.mode == Mode::Flyout && self.shell.visible {
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if focused {
                self.shell.focused_once = true;
            } else if self.shell.focused_once && self.shell.shown_at.elapsed() > FOCUS_GRACE {
                self.hide(ctx);
            }
        }
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.shell.visible = false;
        self.shell.hidden_at = Some(Instant::now());
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn reveal(&mut self, ctx: &egui::Context) {
        self.shell.visible = true;
        self.shell.shown_at = Instant::now();
        self.shell.focused_once = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub(super) fn show_window(&mut self, ctx: &egui::Context) {
        self.mode = Mode::Window;
        let [w, h] = Self::initial_size(self.config.view);
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::Normal,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
        self.reveal(ctx);
    }

    fn show_flyout(&mut self, ctx: &egui::Context, anchor: Anchor) {
        self.mode = Mode::Flyout;
        let [w, h] = FLYOUT_SIZE;
        let ppp = ctx.pixels_per_point();
        let (ax, ay, aw, ah) = anchor;
        // Right-aligned to the icon, sitting above it (the taskbar is at the bottom). If the icon is
        // in the top half of the screen the taskbar is on top, so open downward instead.
        let x = ((ax + aw) as f32 / ppp - w).max(0.0);
        let above = ay as f32 / ppp - h - 8.0;
        let y = if above >= 0.0 {
            above
        } else {
            (ay as f32 + ah as f32) / ppp + 8.0
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(x, y)));
        self.reveal(ctx);
    }
}
