//! Light and airy: a warm off-white window, white sections and cards that float on soft
//! shadows, dark-brown text and one amber accent.
//!
//! Depth comes from tone and shadow, not outlines. There are three surface layers (`BG` behind
//! everything, `PANEL` for sections, `CARD` for the things inside them), controls sit one step
//! darker than their surface so they read as pressable, and borders are faint hairlines. Anything
//! small and clickable is a pill; anything that holds other things is a generous rounded card.

use eframe::egui::{self, epaint::Shadow, Color32, CornerRadius, Stroke};

// Surfaces, from the window back to the things on top.
pub const BG: Color32 = Color32::from_rgb(0xf3, 0xef, 0xe9);
pub const PANEL: Color32 = Color32::from_rgb(0xfb, 0xf9, 0xf6);
pub const CARD: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
/// Controls resting on a card or panel.
pub const PANEL_HI: Color32 = Color32::from_rgb(0xed, 0xe8, 0xe0);
pub const HOVER: Color32 = Color32::from_rgb(0xe3, 0xdc, 0xd1);
pub const ACTIVE: Color32 = Color32::from_rgb(0xd8, 0xd0, 0xc3);
/// The groove a fader or meter sits in.
pub const METER_BG: Color32 = Color32::from_rgb(0xe4, 0xde, 0xd5);

/// Hairline edges: present enough to separate, quiet enough not to draw a box.
pub const BORDER: Color32 = Color32::from_rgb(0xe6, 0xdf, 0xd5);

pub const TEXT: Color32 = Color32::from_rgb(0x2e, 0x27, 0x1f);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x85, 0x7a, 0x6d);
/// Text on top of a filled accent.
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x2e, 0x1f, 0x05);

pub const ACCENT: Color32 = Color32::from_rgb(0xec, 0xa2, 0x2e); // amber
pub const ACCENT_OUT: Color32 = Color32::from_rgb(0xe0, 0x6f, 0x45); // terracotta (hardware outs)
pub const ACCENT_IN: Color32 = Color32::from_rgb(0x5f, 0xad, 0x66); // leaf green (virtual/in)
pub const MUTE: Color32 = Color32::from_rgb(0xdc, 0x4b, 0x3f);
pub const METER_GREEN: Color32 = Color32::from_rgb(0x58, 0xb0, 0x62);
pub const METER_YELLOW: Color32 = Color32::from_rgb(0xe3, 0xa7, 0x2f);
pub const METER_RED: Color32 = Color32::from_rgb(0xdb, 0x46, 0x40);

/// Soft washes behind a notice: amber for a nudge, and the darker text that goes on it.
pub const TINT_NOTE: Color32 = Color32::from_rgb(0xfb, 0xee, 0xd2);
pub const NOTE_TEXT: Color32 = Color32::from_rgb(0x8a, 0x5a, 0x0a);

/// Corner radii. Controls use `pill(height)`; containers use these.
pub const R_PANEL: u8 = 22;
pub const R_CARD: u8 = 18;
pub const R_MENU: u8 = 14;

/// A radius that makes a rectangle of this height a capsule.
pub fn pill(height: f32) -> CornerRadius {
    CornerRadius::same((height / 2.0).round().clamp(0.0, 255.0) as u8)
}

/// The soft, wide shadow under floating or raised surfaces. `alpha` is tuned for a light page.
pub fn shadow(blur: u8, alpha: u8) -> Shadow {
    Shadow {
        offset: [0, 4],
        blur,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0x6b, 0x55, 0x38, alpha),
    }
}

/// A section: a large rounded surface that floats very slightly above the window.
pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(R_PANEL)
        .shadow(shadow(24, 34))
        .inner_margin(egui::Margin::symmetric(18, 16))
}

/// A card inside a section (a mixer strip, an app row, the output card).
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(R_CARD)
        .inner_margin(egui::Margin::symmetric(12, 10))
}

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::light();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = METER_BG;
    v.override_text_color = Some(TEXT);
    v.window_corner_radius = CornerRadius::same(R_PANEL);
    v.menu_corner_radius = CornerRadius::same(R_MENU);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_shadow = shadow(32, 60);
    v.popup_shadow = shadow(26, 60);

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
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
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
        s.spacing.button_padding = egui::vec2(14.0, 6.0);
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.menu_margin = egui::Margin::same(8);
        s.spacing.slider_rail_height = 8.0;
        s.spacing.scroll.bar_width = 6.0;
        s.spacing.scroll.floating = true;
    });
}
