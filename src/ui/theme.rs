use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub const BG: Color32 = Color32::from_rgb(0x14, 0x16, 0x1b);
pub const PANEL: Color32 = Color32::from_rgb(0x1c, 0x1f, 0x26);
pub const PANEL_HI: Color32 = Color32::from_rgb(0x25, 0x29, 0x32);
pub const BORDER: Color32 = Color32::from_rgb(0x2e, 0x33, 0x3e);
pub const TEXT: Color32 = Color32::from_rgb(0xe4, 0xe7, 0xee);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8b, 0x93, 0xa4);
pub const ACCENT: Color32 = Color32::from_rgb(0x4c, 0xa8, 0xff);
pub const ACCENT_OUT: Color32 = Color32::from_rgb(0xf5, 0xa6, 0x23);
pub const ACCENT_IN: Color32 = Color32::from_rgb(0x5d, 0xd3, 0x9e);
pub const MUTE: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
pub const METER_GREEN: Color32 = Color32::from_rgb(0x3d, 0xcc, 0x6e);
pub const METER_YELLOW: Color32 = Color32::from_rgb(0xf0, 0xc2, 0x2e);
pub const METER_RED: Color32 = Color32::from_rgb(0xf0, 0x4a, 0x4a);
pub const METER_BG: Color32 = Color32::from_rgb(0x0d, 0x0f, 0x13);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = METER_BG;
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = PANEL_HI;
    v.widgets.inactive.weak_bg_fill = PANEL_HI;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x30, 0x35, 0x41);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x3a, 0x40, 0x4e);
    v.selection.bg_fill = ACCENT.linear_multiply(0.45);
    let r = CornerRadius::same(6);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    ctx.set_visuals(v);
}
