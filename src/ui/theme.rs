//! Soft geometry: warm dark browns with an amber accent, drawn in rounded, low-contrast shapes.
//!
//! Depth comes from tone, not outlines. There are three surface layers (`BG` behind everything,
//! `PANEL` for sections, `CARD` for the things inside them), controls sit one step above their
//! surface, and borders are faint hairlines that only hint at an edge. Anything small and
//! clickable is a pill; anything that holds other things is a generous rounded card.

use eframe::egui::{self, epaint::Shadow, Color32, CornerRadius, Stroke};

// Surfaces, darkest to lightest.
pub const BG: Color32 = Color32::from_rgb(0x15, 0x11, 0x0d);
pub const PANEL: Color32 = Color32::from_rgb(0x1e, 0x19, 0x14);
pub const CARD: Color32 = Color32::from_rgb(0x27, 0x21, 0x1a);
/// Controls resting on a card or panel.
pub const PANEL_HI: Color32 = Color32::from_rgb(0x33, 0x2b, 0x22);
pub const HOVER: Color32 = Color32::from_rgb(0x41, 0x37, 0x2b);
pub const ACTIVE: Color32 = Color32::from_rgb(0x4e, 0x42, 0x32);
pub const METER_BG: Color32 = Color32::from_rgb(0x12, 0x0e, 0x0a);

/// Hairline edges: present enough to separate, quiet enough not to draw a box.
pub const BORDER: Color32 = Color32::from_rgb(0x36, 0x2e, 0x24);

pub const TEXT: Color32 = Color32::from_rgb(0xf0, 0xea, 0xe1);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xa1, 0x96, 0x85);
/// Text on top of a filled accent.
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x1c, 0x14, 0x08);

pub const ACCENT: Color32 = Color32::from_rgb(0xe8, 0xac, 0x38); // amber gold
pub const ACCENT_OUT: Color32 = Color32::from_rgb(0xe0, 0x78, 0x4c); // terracotta (hardware outs)
pub const ACCENT_IN: Color32 = Color32::from_rgb(0x82, 0xc7, 0x77); // leaf green (virtual/in)
pub const MUTE: Color32 = Color32::from_rgb(0xd9, 0x4f, 0x42);
pub const METER_GREEN: Color32 = Color32::from_rgb(0x66, 0xc0, 0x68);
pub const METER_YELLOW: Color32 = Color32::from_rgb(0xdd, 0xab, 0x34);
pub const METER_RED: Color32 = Color32::from_rgb(0xdd, 0x4a, 0x44);

/// Corner radii. Controls use `pill(height)`; containers use these.
pub const R_PANEL: u8 = 22;
pub const R_CARD: u8 = 18;
pub const R_MENU: u8 = 14;

/// A radius that makes a rectangle of this height a capsule.
pub fn pill(height: f32) -> CornerRadius {
    CornerRadius::same((height / 2.0).round().clamp(0.0, 255.0) as u8)
}

/// The soft, low shadow under floating or raised surfaces.
pub fn shadow(blur: u8, alpha: u8) -> Shadow {
    Shadow {
        offset: [0, 3],
        blur,
        spread: 0,
        color: Color32::from_black_alpha(alpha),
    }
}

/// A section: a large rounded surface that floats very slightly above the window.
pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(R_PANEL)
        .shadow(shadow(18, 60))
        .inner_margin(egui::Margin::symmetric(16, 14))
}

/// A card inside a section (a mixer strip, an app row, the output card).
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(R_CARD)
        .inner_margin(egui::Margin::symmetric(12, 10))
}

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = METER_BG;
    v.override_text_color = Some(TEXT);
    v.window_corner_radius = CornerRadius::same(R_PANEL);
    v.menu_corner_radius = CornerRadius::same(R_MENU);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_shadow = shadow(28, 110);
    v.popup_shadow = shadow(22, 110);

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.inactive.bg_fill = PANEL_HI;
    v.widgets.inactive.weak_bg_fill = PANEL_HI;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_fill = HOVER;
    v.widgets.hovered.weak_bg_fill = HOVER;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.bg_fill = ACTIVE;
    v.widgets.active.weak_bg_fill = ACTIVE;
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.open.bg_fill = HOVER;
    v.widgets.open.weak_bg_fill = HOVER;
    v.selection.bg_fill = ACCENT.linear_multiply(0.32);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    // Buttons and other plain widgets are pills; menu rows get a gentler radius.
    let r = CornerRadius::same(13);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.corner_radius = r;
    v.widgets.open.corner_radius = r;
    ctx.set_visuals(v);

    ctx.all_styles_mut(|s| {
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.menu_margin = egui::Margin::same(8);
        s.spacing.slider_rail_height = 8.0;
        s.spacing.scroll.bar_width = 6.0;
        s.spacing.scroll.floating = true;
    });
}
