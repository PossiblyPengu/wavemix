//! Shared mixer widgets: the dB fader with its stereo meter, toggles, the painted mute button and
//! speaker icon, and the device "chip" that doubles as a device selector.

use eframe::egui::{
    self, pos2, vec2, Color32, CornerRadius, FontId, Id, Painter, Pos2, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, Ui,
};

use super::theme;
use crate::model::amp_to_db;

/// Fraction of full scale the peak marker falls per second.
const PEAK_FALL_PER_SEC: f32 = 0.35;

#[derive(Default)]
pub struct StripOutput {
    pub volume: Option<f32>,
    pub mute: Option<bool>,
}

/// Meter colour for a level in dB: green, yellow from -18, red from -6 (red above 0 means clipping).
fn db_meter_color(db: f32) -> Color32 {
    if db > -6.0 {
        theme::METER_RED
    } else if db > -18.0 {
        theme::METER_YELLOW
    } else {
        theme::METER_GREEN
    }
}

/// Fill a meter column up to `level_db`, on the same -60..+12 dB axis as the fader beside it, so
/// the bar lines up with the scale labels.
fn draw_db_meter(painter: &Painter, rect: Rect, level_db: f32, dim: bool) {
    let shade = if dim { 0.5 } else { 1.0 };
    let y_of = |db: f32| rect.bottom() - rect.height() * db_to_frac(db);
    for (lo, hi) in [(GAIN_MIN_DB, -18.0), (-18.0, -6.0), (-6.0, GAIN_MAX_DB)] {
        if level_db <= lo {
            break;
        }
        painter.rect_filled(
            Rect::from_min_max(
                pos2(rect.left(), y_of(level_db.min(hi))),
                pos2(rect.right(), y_of(lo)),
            ),
            CornerRadius::ZERO,
            db_meter_color(lo + 0.01).linear_multiply(shade),
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

// --- engine mixer widgets ---------------------------------------------------------------------

use crate::engine::params::{GAIN_MAX_DB, GAIN_MIN_DB};

/// Width of a mixer-console strip.
pub const CONSOLE_STRIP_W: f32 = 116.0;
const DB_TICKS: [f32; 7] = [12.0, 0.0, -12.0, -24.0, -36.0, -48.0, -60.0];

fn db_to_frac(db: f32) -> f32 {
    ((db - GAIN_MIN_DB) / (GAIN_MAX_DB - GAIN_MIN_DB)).clamp(0.0, 1.0)
}

fn frac_to_db(frac: f32) -> f32 {
    GAIN_MIN_DB + frac.clamp(0.0, 1.0) * (GAIN_MAX_DB - GAIN_MIN_DB)
}

/// A vertical fader calibrated in dB, with a labelled scale and a stereo level meter beside it.
/// Drag or scroll to change; it snaps to 0 dB (unity) when you get close; double-click resets.
/// `height` lets the console stretch faders to fill the window. Returns the new gain if the user
/// changed it.
pub fn db_fader(
    ui: &mut Ui,
    id: Id,
    height: f32,
    gain_db: f32,
    peaks: [f32; 2],
    accent: Color32,
    dim: bool,
) -> Option<f32> {
    let (rect, _) = ui.allocate_exact_size(vec2(CONSOLE_STRIP_W, height), Sense::hover());
    let top = rect.top() + 8.0;
    let bottom = rect.bottom() - 8.0;
    let track_x = rect.right() - 30.0;
    let y_of = |db: f32| bottom - (bottom - top) * db_to_frac(db);

    // Scale: tick marks with dB labels on the left.
    {
        let painter = ui.painter();
        for db in DB_TICKS {
            let y = y_of(db);
            let unity = db == 0.0;
            painter.text(
                pos2(rect.left() + 20.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{db:+.0}").replace("+0", "0"),
                FontId::proportional(9.0),
                if unity { theme::TEXT } else { theme::TEXT_DIM },
            );
            painter.line_segment(
                [pos2(rect.left() + 23.0, y), pos2(rect.left() + 29.0, y)],
                Stroke::new(
                    if unity { 1.5 } else { 1.0 },
                    if unity {
                        theme::TEXT_DIM
                    } else {
                        theme::BORDER
                    },
                ),
            );
        }

        // Stereo meter: two thin columns, with peak hold.
        for (ch, &peak) in peaks.iter().enumerate() {
            let x = rect.left() + 32.0 + ch as f32 * 10.0;
            let col = Rect::from_min_max(pos2(x, top), pos2(x + 8.0, bottom));
            painter.rect_filled(col, CornerRadius::same(2), theme::METER_BG);
            draw_db_meter(painter, col.shrink2(vec2(1.0, 1.0)), amp_to_db(peak), dim);
        }
    }
    for (ch, &peak) in peaks.iter().enumerate() {
        let level = db_to_frac(amp_to_db(peak));
        let hold_id = id.with(("hold", ch));
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        let prev = ui.data(|d| d.get_temp::<f32>(hold_id)).unwrap_or(0.0);
        let hold = level.max(prev - PEAK_FALL_PER_SEC * dt);
        ui.data_mut(|d| d.insert_temp(hold_id, hold));
        if hold > 0.01 {
            let x = rect.left() + 32.0 + ch as f32 * 10.0;
            let y = bottom - 1.0 - (bottom - top - 2.0) * hold;
            ui.painter().line_segment(
                [pos2(x + 1.0, y), pos2(x + 7.0, y)],
                Stroke::new(2.0, db_meter_color(frac_to_db(hold))),
            );
        }
    }

    // Track and fill.
    let track = Rect::from_min_max(pos2(track_x - 3.0, top), pos2(track_x + 3.0, bottom));
    let hit = Rect::from_min_max(
        pos2(rect.left() + 56.0, rect.top()),
        pos2(rect.right(), rect.bottom()),
    );
    let resp = ui.interact(hit, id.with("fader"), Sense::click_and_drag());
    let mut db = gain_db;
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.is_pointer_button_down_on() {
            db = frac_to_db((bottom - p.y) / (bottom - top));
            // Magnetic unity.
            if db.abs() < 0.8 {
                db = 0.0;
            }
        }
    }
    if resp.double_clicked() {
        db = 0.0;
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            db = (db + scroll.signum() * 0.5).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        }
    }
    let resp = resp.on_hover_text("Drag or scroll to change. Double-click for 0 dB (unity).");

    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(3), theme::METER_BG);
    painter.rect_stroke(
        track,
        CornerRadius::same(3),
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    let fill_top = y_of(db);
    painter.rect_filled(
        Rect::from_min_max(pos2(track.left(), fill_top), track.max),
        CornerRadius::same(3),
        accent.linear_multiply(0.55),
    );
    // Unity marker across the track.
    painter.line_segment(
        [
            pos2(track_x - 9.0, y_of(0.0)),
            pos2(track_x + 9.0, y_of(0.0)),
        ],
        Stroke::new(1.0, theme::TEXT_DIM),
    );
    let knob = Rect::from_center_size(pos2(track_x, fill_top), vec2(30.0, 17.0));
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

    ((db - gain_db).abs() > f32::EPSILON).then_some(db)
}

/// A small latching button with a text label: filled in `color` when on.
pub fn toggle(ui: &mut Ui, text: &str, on: bool, size: egui::Vec2, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if on {
        color
    } else if resp.hovered() {
        Color32::from_rgb(0x38, 0x3d, 0x4a)
    } else {
        theme::PANEL_HI
    };
    let fg = if on {
        Color32::from_rgb(0x10, 0x12, 0x16)
    } else {
        theme::TEXT_DIM
    };
    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.0),
        fg,
    );
    resp
}

/// Like [`toggle`], with a small LED showing whether a processor is enabled.
pub fn led_button(ui: &mut Ui, text: &str, on: bool, selected: bool, size: egui::Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let fill = if selected {
        Color32::from_rgb(0x2c, 0x4a, 0x6e)
    } else if resp.hovered() {
        Color32::from_rgb(0x38, 0x3d, 0x4a)
    } else {
        theme::PANEL_HI
    };
    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    let led = pos2(rect.left() + 8.0, rect.center().y);
    if on {
        ui.painter().circle_filled(led, 3.0, theme::METER_GREEN);
    } else {
        ui.painter()
            .circle_stroke(led, 2.6, Stroke::new(1.0, theme::TEXT_DIM));
    }
    ui.painter().text(
        pos2(rect.center().x + 5.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(10.5),
        if on { theme::TEXT } else { theme::TEXT_DIM },
    );
    resp
}

/// Horizontal balance slider with a centre notch. Double-click recentres. Returns the new value.
pub fn balance(ui: &mut Ui, id: Id, pan: f32, width: f32) -> Option<f32> {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let mut v = pan;
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.is_pointer_button_down_on() {
            v = (((p.x - rect.left()) / rect.width()) * 2.0 - 1.0).clamp(-1.0, 1.0);
            if v.abs() < 0.06 {
                v = 0.0;
            }
        }
    }
    if resp.double_clicked() {
        v = 0.0;
    }
    let painter = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 10.0, 4.0));
    painter.rect_filled(track, CornerRadius::same(2), theme::METER_BG);
    painter.line_segment(
        [
            pos2(rect.center().x, rect.center().y - 5.0),
            pos2(rect.center().x, rect.center().y + 5.0),
        ],
        Stroke::new(1.0, theme::TEXT_DIM),
    );
    let x = rect.center().x + v * (track.width() / 2.0);
    painter.circle_filled(pos2(x, rect.center().y), 6.0, theme::TEXT);
    resp.on_hover_text("Balance. Double-click to centre.");
    ((v - pan).abs() > f32::EPSILON).then_some(v)
}

#[cfg(test)]
mod fader_tests {
    use super::*;

    #[test]
    fn fader_maps_db_linearly_and_round_trips() {
        assert_eq!(db_to_frac(GAIN_MIN_DB), 0.0);
        assert_eq!(db_to_frac(GAIN_MAX_DB), 1.0);
        for db in [-60.0, -24.0, 0.0, 6.0, 12.0] {
            assert!((frac_to_db(db_to_frac(db)) - db).abs() < 1e-4);
        }
        // Unity sits a little above three quarters of the way up, as on a hardware mixer.
        let unity = db_to_frac(0.0);
        assert!(unity > 0.8 && unity < 0.85, "{unity}");
    }
}
