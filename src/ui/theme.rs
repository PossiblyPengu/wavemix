use eframe::egui::{self, Color32, CornerRadius, Stroke};

// Warm geometry palette — dark warm browns, amber/gold accent, terracotta and leaf green.
pub const BG: Color32 = Color32::from_rgb(0x17, 0x13, 0x0f);
pub const PANEL: Color32 = Color32::from_rgb(0x1f, 0x1b, 0x16);
pub const PANEL_HI: Color32 = Color32::from_rgb(0x29, 0x24, 0x1c);
pub const BORDER: Color32 = Color32::from_rgb(0x3e, 0x34, 0x28);
pub const TEXT: Color32 = Color32::from_rgb(0xed, 0xe8, 0xe0);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9c, 0x92, 0x82);
pub const ACCENT: Color32 = Color32::from_rgb(0xe8, 0xac, 0x38);      // amber gold
pub const ACCENT_OUT: Color32 = Color32::from_rgb(0xd9, 0x6c, 0x3e);  // terracotta (hardware outs)
pub const ACCENT_IN: Color32 = Color32::from_rgb(0x78, 0xc2, 0x6c);   // leaf green (virtual/in)
pub const MUTE: Color32 = Color32::from_rgb(0xd4, 0x46, 0x3a);
pub const METER_GREEN: Color32 = Color32::from_rgb(0x58, 0xba, 0x5a);
pub const METER_YELLOW: Color32 = Color32::from_rgb(0xd6, 0xa2, 0x2c);
pub const METER_RED: Color32 = Color32::from_rgb(0xd6, 0x3e, 0x3a);
pub const METER_BG: Color32 = Color32::from_rgb(0x0e, 0x0b, 0x07);

/// Warm hover fill for interactive widgets.
pub const HOVER: Color32 = Color32::from_rgb(0x36, 0x2e, 0x22);
/// Active/pressed fill.
pub const ACTIVE: Color32 = Color32::from_rgb(0x44, 0x3a, 0x2c);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = METER_BG;
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = PANEL_HI;
    v.widgets.inactive.weak_bg_fill = PANEL_HI;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.active.weak_bg_fill = ACTIVE;
    v.selection.bg_fill = ACCENT.linear_multiply(0.32);
    let r = CornerRadius::same(5);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    ctx.set_visuals(v);
}
