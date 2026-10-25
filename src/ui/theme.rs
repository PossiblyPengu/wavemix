//! Two looks, one shape language: a light, airy theme (warm off-white, white cards on soft
//! shadows) and a dark one (deep warm browns), both with a single amber accent.
//!
//! Depth comes from tone and shadow, not outlines. There are three surface layers (`BG()` behind
//! everything, `PANEL()` for sections, `CARD()` for the things inside them), controls sit one step
//! off their surface so they read as pressable, and borders are faint hairlines. Anything small
//! and clickable is a pill; anything that holds other things is a generous rounded card.
//!
//! Colours are functions rather than constants so the theme can change while the app runs.

#![allow(non_snake_case)]

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, epaint::Shadow, Color32, CornerRadius, Stroke};

struct Palette {
    bg: Color32,
    panel: Color32,
    card: Color32,
    panel_hi: Color32,
    hover: Color32,
    active: Color32,
    meter_bg: Color32,
    border: Color32,
    text: Color32,
    text_dim: Color32,
    on_accent: Color32,
    accent: Color32,
    accent_out: Color32,
    accent_in: Color32,
    mute: Color32,
    tint_note: Color32,
    note_text: Color32,
    green: Color32,
    yellow: Color32,
    red: Color32,
    /// Colour of drop shadows (with alpha applied by the caller).
    shadow: [u8; 3],
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

const LIGHT: Palette = Palette {
    bg: rgb(0xf3, 0xef, 0xe9),
    panel: rgb(0xfb, 0xf9, 0xf6),
    card: rgb(0xff, 0xff, 0xff),
    panel_hi: rgb(0xed, 0xe8, 0xe0),
    hover: rgb(0xe3, 0xdc, 0xd1),
    active: rgb(0xd8, 0xd0, 0xc3),
    meter_bg: rgb(0xe4, 0xde, 0xd5),
    border: rgb(0xe6, 0xdf, 0xd5),
    text: rgb(0x2e, 0x27, 0x1f),
    text_dim: rgb(0x85, 0x7a, 0x6d),
    on_accent: rgb(0x2e, 0x1f, 0x05),
    accent: rgb(0xec, 0xa2, 0x2e),
    accent_out: rgb(0xe0, 0x6f, 0x45),
    accent_in: rgb(0x5f, 0xad, 0x66),
    mute: rgb(0xdc, 0x4b, 0x3f),
    tint_note: rgb(0xfb, 0xee, 0xd2),
    note_text: rgb(0x8a, 0x5a, 0x0a),
    green: rgb(0x58, 0xb0, 0x62),
    yellow: rgb(0xe3, 0xa7, 0x2f),
    red: rgb(0xdb, 0x46, 0x40),
    shadow: [0x6b, 0x55, 0x38],
};

const DARK: Palette = Palette {
    bg: rgb(0x12, 0x13, 0x16),
    panel: rgb(0x1a, 0x1b, 0x1f),
    card: rgb(0x23, 0x24, 0x29),
    panel_hi: rgb(0x2d, 0x2f, 0x35),
    hover: rgb(0x38, 0x3a, 0x42),
    active: rgb(0x44, 0x47, 0x50),
    meter_bg: rgb(0x0d, 0x0e, 0x10),
    border: rgb(0x2b, 0x2d, 0x33),
    text: rgb(0xec, 0xed, 0xf0),
    text_dim: rgb(0x9a, 0x9d, 0xa7),
    on_accent: rgb(0x1c, 0x14, 0x08),
    accent: rgb(0xec, 0xa2, 0x2e),
    accent_out: rgb(0xe8, 0x7a, 0x50),
    accent_in: rgb(0x7a, 0xc4, 0x80),
    mute: rgb(0xe0, 0x55, 0x4a),
    tint_note: rgb(0x2a, 0x27, 0x20),
    note_text: rgb(0xe8, 0xb4, 0x50),
    green: rgb(0x66, 0xc0, 0x6c),
    yellow: rgb(0xe0, 0xae, 0x38),
    red: rgb(0xe0, 0x4e, 0x48),
    shadow: [0x00, 0x00, 0x00],
};

/// Does Windows have apps set to dark? (Settings > Personalisation > Colours.)
#[cfg(windows)]
pub fn system_prefers_dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|light| light == 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn system_prefers_dark() -> bool {
    false
}

/// Whether the chosen mode means dark right now.
pub fn wants_dark(mode: crate::config::ThemeMode) -> bool {
    use crate::config::ThemeMode;
    match mode {
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
        ThemeMode::System => system_prefers_dark(),
    }
}

static DARK_MODE: AtomicBool = AtomicBool::new(false);

pub fn is_dark() -> bool {
    DARK_MODE.load(Ordering::Relaxed)
}

/// Switch palettes. Call [`apply`] afterwards so egui's own widgets follow.
pub fn set_dark(dark: bool) {
    DARK_MODE.store(dark, Ordering::Relaxed);
}

fn p() -> &'static Palette {
    if is_dark() {
        &DARK
    } else {
        &LIGHT
    }
}

// Surfaces, from the window back to the things on top.
pub fn BG() -> Color32 {
    p().bg
}
pub fn PANEL() -> Color32 {
    p().panel
}
pub fn CARD() -> Color32 {
    p().card
}
/// Controls resting on a card or panel.
pub fn PANEL_HI() -> Color32 {
    p().panel_hi
}
pub fn HOVER() -> Color32 {
    p().hover
}
pub fn ACTIVE() -> Color32 {
    p().active
}
/// The groove a fader or meter sits in.
pub fn METER_BG() -> Color32 {
    p().meter_bg
}
/// Hairline edges: present enough to separate, quiet enough not to draw a box.
pub fn BORDER() -> Color32 {
    p().border
}

pub fn TEXT() -> Color32 {
    p().text
}
pub fn TEXT_DIM() -> Color32 {
    p().text_dim
}
/// Text on top of a filled accent.
pub fn ON_ACCENT() -> Color32 {
    p().on_accent
}

pub fn ACCENT() -> Color32 {
    p().accent
}
/// Hardware outputs.
pub fn ACCENT_OUT() -> Color32 {
    p().accent_out
}
/// Virtual inputs and outputs.
pub fn ACCENT_IN() -> Color32 {
    p().accent_in
}
pub fn MUTE() -> Color32 {
    p().mute
}
pub fn METER_GREEN() -> Color32 {
    p().green
}
pub fn METER_YELLOW() -> Color32 {
    p().yellow
}
pub fn METER_RED() -> Color32 {
    p().red
}

/// A soft wash behind a notice, and the text that goes on it.
pub fn TINT_NOTE() -> Color32 {
    p().tint_note
}
pub fn NOTE_TEXT() -> Color32 {
    p().note_text
}

/// The colour of a drop shadow at the given opacity.
pub fn shadow_color(alpha: u8) -> Color32 {
    let [r, g, b] = p().shadow;
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// Corner radii. Controls use `pill(height)`; containers use these.
pub const R_PANEL: u8 = 22;
pub const R_CARD: u8 = 18;
pub const R_MENU: u8 = 14;

/// A radius that makes a rectangle of this height a capsule.
pub fn pill(height: f32) -> CornerRadius {
    CornerRadius::same((height / 2.0).round().clamp(0.0, 255.0) as u8)
}

/// The soft, wide shadow under floating or raised surfaces. `alpha` is given for the light theme;
/// dark surfaces need a stronger one to show at all.
pub fn shadow(blur: u8, alpha: u8) -> Shadow {
    let alpha = if is_dark() {
        alpha.saturating_mul(2).min(200)
    } else {
        alpha
    };
    Shadow {
        offset: [0, 4],
        blur,
        spread: 0,
        color: shadow_color(alpha),
    }
}

/// A section: a large rounded surface that floats very slightly above the window.
pub fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL())
        .stroke(Stroke::new(1.0, BORDER()))
        .corner_radius(R_PANEL)
        .shadow(shadow(24, 34))
        .inner_margin(egui::Margin::symmetric(18, 16))
}

/// A card inside a section (a mixer strip, an app row, the output card).
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD())
        .corner_radius(R_CARD)
        .inner_margin(egui::Margin::symmetric(12, 10))
}

/// Push the current palette into egui's own widgets (buttons, menus, text boxes, scroll bars).
pub fn apply(ctx: &egui::Context) {
    let mut v = if is_dark() {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = BG();
    v.window_fill = PANEL();
    v.extreme_bg_color = METER_BG();
    v.override_text_color = Some(TEXT());
    v.window_corner_radius = CornerRadius::same(R_PANEL);
    v.menu_corner_radius = CornerRadius::same(R_MENU);
    v.window_stroke = Stroke::new(1.0, BORDER());
    v.window_shadow = shadow(32, 60);
    v.popup_shadow = shadow(26, 60);

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER());
    v.widgets.noninteractive.bg_fill = PANEL();
    v.widgets.inactive.bg_fill = PANEL_HI();
    v.widgets.inactive.weak_bg_fill = PANEL_HI();
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_fill = HOVER();
    v.widgets.hovered.weak_bg_fill = HOVER();
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.bg_fill = ACTIVE();
    v.widgets.active.weak_bg_fill = ACTIVE();
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.open.bg_fill = HOVER();
    v.widgets.open.weak_bg_fill = HOVER();
    v.selection.bg_fill = ACCENT().linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT());

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
