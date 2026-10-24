//! Shared mixer widgets: the vertical fader strip (with live meter and peak hold), the painted
//! mute button and speaker icon, and the device "chip" that doubles as an output selector.

use eframe::egui::{
    self, pos2, vec2, Color32, CornerRadius, FontId, Id, Painter, Pos2, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, Ui,
};

use super::theme;
use crate::model::{amp_to_db, db_to_meter, percent};

pub const STRIP_W: f32 = 98.0;
/// Height of the fader track at the smallest sensible window.
pub const MIN_TRACK_H: f32 = 140.0;
/// Height of everything in a strip except the fader track (icon, name, readouts, buttons).
pub const STRIP_CHROME_H: f32 = 150.0;
const METER_W: f32 = 10.0;
pub const ICON: f32 = 24.0;
/// Fraction of full scale the peak marker falls per second.
const PEAK_FALL_PER_SEC: f32 = 0.35;
/// Mouse-wheel step, in volume scalar per notch.
const SCROLL_STEP: f32 = 0.02;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    None,
    Speaker,
    Mic,
}

pub struct Strip<'a> {
    pub id: Id,
    pub title: &'a str,
    /// App icon; the row is reserved even when absent so strips line up.
    pub icon: Option<&'a egui::TextureHandle>,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
    pub accent: Color32,
    /// Fader height; the console grows it to use the window's height.
    pub track_h: f32,
    /// What to draw in the icon slot when there's no app icon (devices).
    pub glyph: Glyph,
    /// Green dot when the app is currently making sound; hollow when idle; none for devices.
    pub status: Option<bool>,
}

#[derive(Default)]
pub struct StripOutput {
    pub volume: Option<f32>,
    pub mute: Option<bool>,
}

impl Strip<'_> {
    /// Draws the strip, then `footer` underneath it (output chip, default badge, ...).
    pub fn show(self, ui: &mut Ui, footer: impl FnOnce(&mut Ui)) -> StripOutput {
        let mut out = StripOutput::default();
        let idle = self.status == Some(false);
        ui.allocate_ui_with_layout(
            vec2(STRIP_W, 0.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.set_width(STRIP_W);
                ui.spacing_mut().item_spacing = vec2(4.0, 5.0);

                // Icon with a status dot, then the name.
                let (icon_rect, _) = ui.allocate_exact_size(vec2(ICON, ICON), Sense::hover());
                if let Some(tex) = self.icon {
                    let tint = if idle {
                        Color32::from_white_alpha(110)
                    } else {
                        Color32::WHITE
                    };
                    ui.painter().image(
                        tex.id(),
                        icon_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        tint,
                    );
                }
                match self.glyph {
                    Glyph::Speaker => speaker_icon(
                        ui.painter(),
                        icon_rect.center(),
                        ICON * 0.9,
                        false,
                        self.accent,
                    ),
                    Glyph::Mic => {
                        mic_icon(ui.painter(), icon_rect.center(), ICON * 0.9, self.accent)
                    }
                    Glyph::None => {}
                }
                if let Some(active) = self.status {
                    let c = icon_rect.right_bottom() + vec2(1.0, 1.0);
                    let color = if active {
                        theme::METER_GREEN
                    } else {
                        theme::TEXT_DIM
                    };
                    ui.painter().circle_filled(c, 5.0, theme::PANEL);
                    if active {
                        ui.painter().circle_filled(c, 3.5, color);
                    } else {
                        ui.painter().circle_stroke(c, 3.0, Stroke::new(1.2, color));
                    }
                }
                let color = if idle { theme::TEXT_DIM } else { theme::TEXT };
                let name = truncate(ui, self.title, STRIP_W - 6.0, 12.0);
                ui.label(egui::RichText::new(name).size(12.0).color(color))
                    .on_hover_text(self.title);

                let (rect, _) = ui.allocate_exact_size(vec2(STRIP_W, self.track_h), Sense::hover());
                self.fader(ui, rect, idle, &mut out);

                let (txt, col) = if self.muted {
                    ("MUTED".to_string(), theme::MUTE)
                } else {
                    (format!("{}%", percent(self.volume)), theme::TEXT)
                };
                ui.label(egui::RichText::new(txt).size(14.0).strong().color(col));

                let db = amp_to_db(self.peak);
                let db_txt = if db <= -59.9 {
                    "-inf dB".to_string()
                } else {
                    format!("{db:.1} dB")
                };
                ui.label(
                    egui::RichText::new(db_txt)
                        .size(10.0)
                        .monospace()
                        .color(theme::TEXT_DIM),
                );

                if mute_button(ui, self.muted, vec2(40.0, 26.0)).clicked() {
                    out.mute = Some(!self.muted);
                }
                footer(ui);
            },
        );
        out
    }

    fn fader(&self, ui: &mut Ui, rect: Rect, idle: bool, out: &mut StripOutput) {
        let painter = ui.painter();
        let meter_rect = Rect::from_min_size(
            pos2(rect.left() + 10.0, rect.top()),
            vec2(METER_W, rect.height()),
        );
        let track_x = rect.center().x + 10.0;
        let track = Rect::from_min_max(
            pos2(track_x - 3.0, rect.top() + 8.0),
            pos2(track_x + 3.0, rect.bottom() - 8.0),
        );
        let hit = Rect::from_min_max(
            pos2(track_x - 24.0, rect.top()),
            pos2(rect.right(), rect.bottom()),
        );

        // --- meter ---
        painter.rect_filled(meter_rect, CornerRadius::same(3), theme::METER_BG);
        let level = if self.muted {
            0.0
        } else {
            db_to_meter(amp_to_db(self.peak))
        };
        let hold = self.peak_hold(ui, level);
        draw_meter(painter, meter_rect.shrink(1.0), level, idle);
        if hold > 0.01 {
            let y = meter_rect.bottom() - 1.0 - (meter_rect.height() - 2.0) * hold;
            painter.line_segment(
                [
                    pos2(meter_rect.left() + 1.0, y),
                    pos2(meter_rect.right() - 1.0, y),
                ],
                Stroke::new(2.0, meter_color(hold)),
            );
        }

        // --- scale ticks, every 25% with a heavier mark at 100% ---
        for step in 0..=4 {
            let frac = step as f32 / 4.0;
            let y = track.bottom() - track.height() * frac;
            let w = if step == 4 { 9.0 } else { 5.0 };
            painter.line_segment(
                [pos2(track.left() - 5.0 - w, y), pos2(track.left() - 5.0, y)],
                Stroke::new(1.0, theme::BORDER),
            );
        }

        // --- interaction ---
        let resp = ui.interact(hit, self.id.with("fader"), Sense::click_and_drag());
        let mut vol = self.volume;
        if let Some(p) = resp.interact_pointer_pos() {
            if resp.dragged() || resp.is_pointer_button_down_on() {
                vol = ((track.bottom() - p.y) / track.height()).clamp(0.0, 1.0);
            }
        }
        if resp.double_clicked() {
            vol = 1.0;
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                vol = (vol + scroll.signum() * SCROLL_STEP).clamp(0.0, 1.0);
            }
        }
        if (vol - self.volume).abs() > f32::EPSILON {
            out.volume = Some(vol);
        }
        let resp = resp.on_hover_text("Drag or scroll to change volume. Double-click for 100%.");

        // --- track + fill + knob ---
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(3), theme::METER_BG);
        painter.rect_stroke(
            track,
            CornerRadius::same(3),
            Stroke::new(1.0, theme::BORDER),
            StrokeKind::Inside,
        );
        let fill_top = track.bottom() - track.height() * vol;
        let fill = Rect::from_min_max(pos2(track.left(), fill_top), track.max);
        let accent = if self.muted {
            theme::TEXT_DIM
        } else {
            self.accent
        };
        painter.rect_filled(fill, CornerRadius::same(3), accent.linear_multiply(0.55));
        let knob = Rect::from_center_size(pos2(track_x, fill_top), vec2(32.0, 17.0));
        let knob_fill = if resp.dragged() {
            Color32::from_rgb(0x4a, 0x51, 0x62)
        } else if resp.hovered() {
            Color32::from_rgb(0x3c, 0x42, 0x50)
        } else {
            Color32::from_rgb(0x30, 0x35, 0x41)
        };
        painter.rect_filled(knob, CornerRadius::same(4), knob_fill);
        painter.rect_stroke(
            knob,
            CornerRadius::same(4),
            Stroke::new(1.0, accent),
            StrokeKind::Inside,
        );
        painter.line_segment(
            [
                pos2(knob.left() + 5.0, knob.center().y),
                pos2(knob.right() - 5.0, knob.center().y),
            ],
            Stroke::new(2.0, accent),
        );
    }

    /// Peak-hold marker position (0..1): jumps up with the signal, falls away slowly.
    fn peak_hold(&self, ui: &Ui, level: f32) -> f32 {
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let id = self.id.with("hold");
        let prev = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
        let hold = level.max(prev - PEAK_FALL_PER_SEC * dt);
        ui.data_mut(|d| d.insert_temp(id, hold));
        hold
    }
}

fn meter_color(pos: f32) -> Color32 {
    if pos > db_to_meter(-6.0) {
        theme::METER_RED
    } else if pos > db_to_meter(-18.0) {
        theme::METER_YELLOW
    } else {
        theme::METER_GREEN
    }
}

/// Fill the meter bottom-up in three colour bands: green, yellow above -18 dB, red above -6 dB.
fn draw_meter(painter: &Painter, rect: Rect, level: f32, idle: bool) {
    let dim = if idle { 0.5 } else { 1.0 };
    let bands = [
        (0.0, db_to_meter(-18.0), theme::METER_GREEN),
        (db_to_meter(-18.0), db_to_meter(-6.0), theme::METER_YELLOW),
        (db_to_meter(-6.0), 1.0, theme::METER_RED),
    ];
    for (lo, hi, color) in bands {
        if level <= lo {
            break;
        }
        let top = level.min(hi);
        let y0 = rect.bottom() - rect.height() * top;
        let y1 = rect.bottom() - rect.height() * lo;
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.left(), y0), pos2(rect.right(), y1)),
            CornerRadius::ZERO,
            color.linear_multiply(dim),
        );
    }
}

// --- shared small widgets ---------------------------------------------------------------------

/// A speaker glyph, drawn so it never depends on the font having one. Muted shows an X.
pub fn speaker_icon(painter: &Painter, center: Pos2, size: f32, muted: bool, color: Color32) {
    let s = size;
    let (cx, cy) = (center.x, center.y);
    // The body is a box plus a flared cone; two convex pieces tessellate cleanly.
    painter.add(Shape::convex_polygon(
        vec![
            pos2(cx - s * 0.42, cy - s * 0.16),
            pos2(cx - s * 0.18, cy - s * 0.16),
            pos2(cx - s * 0.18, cy + s * 0.16),
            pos2(cx - s * 0.42, cy + s * 0.16),
        ],
        color,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            pos2(cx - s * 0.18, cy - s * 0.16),
            pos2(cx + s * 0.10, cy - s * 0.44),
            pos2(cx + s * 0.10, cy + s * 0.44),
            pos2(cx - s * 0.18, cy + s * 0.16),
        ],
        color,
        Stroke::NONE,
    ));
    let stroke = Stroke::new((s * 0.11).max(1.2), color);
    if muted {
        let x = cx + s * 0.22;
        let r = s * 0.24;
        painter.line_segment([pos2(x, cy - r), pos2(x + r * 1.5, cy + r)], stroke);
        painter.line_segment([pos2(x, cy + r), pos2(x + r * 1.5, cy - r)], stroke);
    } else {
        for radius in [0.28, 0.48] {
            let pts: Vec<Pos2> = (-4..=4)
                .map(|i| {
                    let a = (i as f32 / 4.0) * 0.9;
                    pos2(
                        cx + s * 0.08 + s * radius * a.cos(),
                        cy + s * radius * a.sin(),
                    )
                })
                .collect();
            painter.add(Shape::line(pts, stroke));
        }
    }
}

/// A microphone glyph: a capsule, a cradle under it and a stand.
pub fn mic_icon(painter: &Painter, center: Pos2, size: f32, color: Color32) {
    let s = size;
    let (cx, cy) = (center.x, center.y);
    let capsule = Rect::from_center_size(pos2(cx, cy - s * 0.12), vec2(s * 0.34, s * 0.56));
    painter.rect_filled(capsule, CornerRadius::same((s * 0.17) as u8), color);
    let stroke = Stroke::new((s * 0.1).max(1.2), color);
    let pts: Vec<Pos2> = (0..=8)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 8.0);
            pos2(cx + s * 0.30 * a.cos(), cy + s * 0.02 + s * 0.28 * a.sin())
        })
        .collect();
    painter.add(Shape::line(pts, stroke));
    painter.line_segment([pos2(cx, cy + s * 0.30), pos2(cx, cy + s * 0.46)], stroke);
    painter.line_segment(
        [
            pos2(cx - s * 0.16, cy + s * 0.46),
            pos2(cx + s * 0.16, cy + s * 0.46),
        ],
        stroke,
    );
}

/// A mute toggle: a red filled button with a crossed speaker when muted, dim otherwise.
pub fn mute_button(ui: &mut Ui, muted: bool, size: egui::Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if muted {
        theme::MUTE
    } else if resp.hovered() {
        Color32::from_rgb(0x38, 0x3d, 0x4a)
    } else {
        theme::PANEL_HI
    };
    let fg = if muted {
        Color32::WHITE
    } else {
        theme::TEXT_DIM
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    speaker_icon(ui.painter(), rect.center(), size.y * 0.7, muted, fg);
    resp.on_hover_text(if muted { "Unmute" } else { "Mute" })
}

/// A pill showing a name with a small drop-down arrow. Clicking it is how you pick a device.
pub fn chip(ui: &mut Ui, text: &str, width: f32, accent: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());
    let fill = if resp.hovered() || resp.is_pointer_button_down_on() {
        Color32::from_rgb(0x38, 0x3d, 0x4a)
    } else {
        theme::PANEL_HI
    };
    let border = if accent { theme::ACCENT } else { theme::BORDER };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let label = truncate(ui, text, width - 26.0, 11.0);
    ui.painter().text(
        pos2(rect.left() + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.0),
        theme::TEXT,
    );
    // Down arrow.
    let a = pos2(rect.right() - 12.0, rect.center().y);
    ui.painter().add(Shape::convex_polygon(
        vec![
            pos2(a.x - 3.5, a.y - 2.0),
            pos2(a.x + 3.5, a.y - 2.0),
            pos2(a.x, a.y + 2.5),
        ],
        theme::TEXT_DIM,
        Stroke::NONE,
    ));
    resp
}

/// "Speakers (Realtek Audio)" -> "Speakers": the part before the driver name.
pub fn short_device_name(name: &str) -> &str {
    match name.find(" (") {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// Shorten `text` with an ellipsis so it fits in `max_w` points at `size`.
pub fn truncate(ui: &Ui, text: &str, max_w: f32, size: f32) -> String {
    let font = FontId::proportional(size);
    let width = |s: &str| {
        ui.painter()
            .layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let s: String = chars[..n].iter().collect::<String>() + "…";
        if width(&s) <= max_w {
            return s;
        }
    }
    "…".to_string()
}

#[cfg(test)]
mod tests {
    use super::short_device_name;

    #[test]
    fn short_names_drop_the_driver() {
        assert_eq!(short_device_name("Speakers (Realtek Audio)"), "Speakers");
        assert_eq!(short_device_name("CABLE Input (VB-Audio)"), "CABLE Input");
        assert_eq!(short_device_name("Headphones"), "Headphones");
        assert_eq!(short_device_name("(weird)"), "(weird)");
    }
}
