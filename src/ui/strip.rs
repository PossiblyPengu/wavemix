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
/// Seconds for a meter bar to fall most of the way after a loud moment (it rises instantly).
const METER_RELEASE_S: f32 = 0.22;

// --- motion -----------------------------------------------------------------------------------

/// 0.0 -> 1.0 as `on` becomes true (and back), eased over `secs`. Keeps the UI repainting while
/// it moves.
pub fn anim(ui: &Ui, id: Id, on: bool, secs: f32) -> f32 {
    let t = ui.ctx().animate_bool_with_time(id, on, secs);
    t * t * (3.0 - 2.0 * t)
}

/// An amber ring around a widget that has keyboard focus (Tab / arrow navigation), since these
/// widgets are drawn by hand and egui's own focus outline doesn't cover them.
pub fn focus_ring(ui: &Ui, resp: &Response, rect: Rect, radius: CornerRadius) {
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            radius,
            Stroke::new(2.0, theme::ACCENT()),
            StrokeKind::Outside,
        );
    }
}

/// Blend two colours by `t`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

#[derive(Default)]
pub struct StripOutput {
    pub volume: Option<f32>,
    pub mute: Option<bool>,
}

/// Meter colour for a level in dB: green, yellow from -18, red from -6 (red above 0 means clipping).
fn db_meter_color(db: f32) -> Color32 {
    if db > -6.0 {
        theme::METER_RED()
    } else if db > -18.0 {
        theme::METER_YELLOW()
    } else {
        theme::METER_GREEN()
    }
}

/// Fill a meter column up to `level_db`, on the same -60..+12 dB axis as the fader beside it, so
/// the bar lines up with the scale labels. The column is a capsule, so its ends are rounded.
fn draw_db_meter(painter: &Painter, rect: Rect, level_db: f32, dim: bool) {
    let shade = if dim { 0.45 } else { 1.0 };
    let y_of = |db: f32| rect.bottom() - rect.height() * db_to_frac(db);
    let r = (rect.width() / 2.0).round().min(255.0) as u8;
    let bands = [(GAIN_MIN_DB, -18.0), (-18.0, -6.0), (-6.0, GAIN_MAX_DB)];
    for (n, (lo, hi)) in bands.into_iter().enumerate() {
        if level_db <= lo {
            break;
        }
        let top = level_db.min(hi);
        let is_top = level_db <= hi;
        let radius = CornerRadius {
            nw: if is_top { r } else { 0 },
            ne: if is_top { r } else { 0 },
            sw: if n == 0 { r } else { 0 },
            se: if n == 0 { r } else { 0 },
        };
        painter.rect_filled(
            Rect::from_min_max(pos2(rect.left(), y_of(top)), pos2(rect.right(), y_of(lo))),
            radius,
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

/// A mute toggle: a red filled pill with a crossed speaker when muted, a quiet one otherwise.
pub fn mute_button(ui: &mut Ui, muted: bool, size: egui::Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = anim(ui, resp.id.with("h"), resp.hovered(), 0.12);
    let on = anim(ui, resp.id.with("on"), muted, 0.16);
    let rest = mix(theme::PANEL_HI(), theme::HOVER(), hover);
    let fill = mix(rest, theme::MUTE(), on);
    let fg = mix(theme::TEXT_DIM(), Color32::WHITE, on);
    ui.painter().rect_filled(rect, theme::pill(size.y), fill);
    focus_ring(ui, &resp, rect, theme::pill(size.y));
    speaker_icon(ui.painter(), rect.center(), size.y * 0.66, muted, fg);
    resp.on_hover_text(if muted { "Unmute" } else { "Mute" })
}

/// A pill showing a name with a small drop-down arrow. Clicking it is how you pick a device.
pub fn chip(ui: &mut Ui, text: &str, width: f32, accent: bool) -> Response {
    const H: f32 = 28.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, H), Sense::click());
    let hover = anim(
        ui,
        resp.id.with("h"),
        resp.hovered() || resp.is_pointer_button_down_on(),
        0.12,
    );
    let fill = mix(theme::PANEL_HI(), theme::HOVER(), hover);
    ui.painter().rect_filled(rect, theme::pill(H), fill);
    if accent {
        // A soft amber ring marks a slot that has something assigned.
        ui.painter().rect_stroke(
            rect,
            theme::pill(H),
            Stroke::new(1.0, theme::ACCENT().linear_multiply(0.55)),
            StrokeKind::Inside,
        );
    }
    focus_ring(ui, &resp, rect, theme::pill(H));
    let label = truncate(ui, text, width - 36.0, 11.5);
    ui.painter().text(
        pos2(rect.left() + 13.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(11.5),
        theme::TEXT(),
    );
    // The arrow brightens and nudges down a touch when hovered.
    let a = pos2(rect.right() - 14.0, rect.center().y + hover);
    ui.painter().add(Shape::convex_polygon(
        vec![
            pos2(a.x - 3.5, a.y - 2.0),
            pos2(a.x + 3.5, a.y - 2.0),
            pos2(a.x, a.y + 2.5),
        ],
        mix(theme::TEXT_DIM(), theme::TEXT(), hover),
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

/// A device's short name, kept unique among `all`: two "Speakers (...)" devices become
/// "Speakers · Logi Z407" and "Speakers · Razer USB Sound Card" instead of both reading "Speakers".
pub fn label_among<'a>(name: &str, all: impl IntoIterator<Item = &'a str>) -> String {
    let short = short_device_name(name);
    let clash = all
        .into_iter()
        .any(|other| other != name && short_device_name(other) == short);
    if !clash {
        return short.to_string();
    }
    match (name.find(" ("), name.rfind(')')) {
        (Some(open), Some(close)) if close > open + 2 => {
            format!("{short} · {}", &name[open + 2..close])
        }
        _ => name.to_string(),
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
    fn clashing_short_names_keep_their_driver() {
        let all = [
            "Speakers (Logi Z407)",
            "Speakers (Razer USB Sound Card)",
            "Headphones (USB)",
        ];
        assert_eq!(super::label_among(all[0], all), "Speakers · Logi Z407");
        assert_eq!(
            super::label_among(all[1], all),
            "Speakers · Razer USB Sound Card"
        );
        assert_eq!(super::label_among(all[2], all), "Headphones");
    }

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
const DB_TICKS: [f32; 5] = [12.0, 0.0, -12.0, -24.0, -48.0];

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
/// Is the fader with this id the one holding keyboard focus?
pub fn fader_focused(ui: &Ui, id: Id) -> bool {
    ui.memory(|m| m.has_focus(id.with("fader")))
}

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
    // Scale, meters and knob together are a little left-heavy; nudge the group to the middle.
    let rect = rect.translate(vec2(6.0, 0.0));
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
                if unity {
                    theme::TEXT()
                } else {
                    theme::TEXT_DIM()
                },
            );
            painter.line_segment(
                [pos2(rect.left() + 23.0, y), pos2(rect.left() + 29.0, y)],
                Stroke::new(
                    if unity { 1.5 } else { 1.0 },
                    if unity {
                        theme::TEXT_DIM()
                    } else {
                        theme::BORDER()
                    },
                ),
            );
        }

        // Stereo meter: two thin columns, with peak hold.
        for (ch, &peak) in peaks.iter().enumerate() {
            let x = rect.left() + 32.0 + ch as f32 * 10.0;
            let col = Rect::from_min_max(pos2(x, top), pos2(x + 8.0, bottom));
            painter.rect_filled(col, CornerRadius::same(2), theme::METER_BG());
            // Ballistics: up at once, down smoothly, so the bar flows instead of flickering.
            let bar_id = id.with(("bar", ch));
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            let now = amp_to_db(peak);
            let prev = ui.data(|d| d.get_temp::<f32>(bar_id)).unwrap_or(now);
            let shown = if now >= prev {
                now
            } else {
                prev - (prev - now) * (1.0 - (-dt / METER_RELEASE_S).exp())
            };
            ui.data_mut(|d| d.insert_temp(bar_id, shown));
            draw_db_meter(painter, col.shrink2(vec2(1.0, 1.0)), shown, dim);
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

    // Track and fill: a slim capsule, filled from the bottom up to the knob.
    let track = Rect::from_min_max(pos2(track_x - 4.0, top), pos2(track_x + 4.0, bottom));
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
    // From the keyboard: arrows nudge (Shift for fine, Ctrl for coarse), Page keys jump, Home is
    // unity and End is silence.
    if resp.has_focus() {
        use egui::{Key, Modifiers};
        let step = ui.input(|i| {
            if i.modifiers.shift {
                0.1
            } else if i.modifiers.ctrl {
                3.0
            } else {
                0.5
            }
        });
        let pressed = |ui: &mut Ui, key: Key| {
            ui.input_mut(|i| {
                i.consume_key(Modifiers::NONE, key)
                    || i.consume_key(Modifiers::SHIFT, key)
                    || i.consume_key(Modifiers::CTRL, key)
            })
        };
        for (key, delta) in [
            (Key::ArrowUp, step),
            (Key::ArrowRight, step),
            (Key::ArrowDown, -step),
            (Key::ArrowLeft, -step),
            (Key::PageUp, 3.0),
            (Key::PageDown, -3.0),
        ] {
            if pressed(ui, key) {
                db = (db + delta).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
            }
        }
        if pressed(ui, Key::Home) {
            db = 0.0;
        }
        if pressed(ui, Key::End) {
            db = GAIN_MIN_DB;
        }
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            db = (db + scroll.signum() * 0.5).clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        }
    }
    focus_ring(
        ui,
        &resp,
        Rect::from_center_size(
            pos2(track_x, (top + bottom) / 2.0),
            vec2(34.0, bottom - top),
        ),
        CornerRadius::same(10),
    );
    let resp = resp.on_hover_text(
        "Drag or scroll to change. Double-click for 0 dB (unity).\n\
         Keyboard: arrows nudge, Page Up/Down jump, Home = 0 dB, M mute, S solo.",
    );

    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(4), theme::METER_BG());
    let fill_top = y_of(db);
    painter.rect_filled(
        Rect::from_min_max(pos2(track.left(), fill_top), track.max),
        CornerRadius::same(4),
        accent.linear_multiply(if dim { 0.25 } else { 0.6 }),
    );
    // Unity marker: a small notch on the track.
    painter.circle_filled(pos2(track_x + 11.0, y_of(0.0)), 1.8, theme::TEXT_DIM());
    painter.circle_filled(pos2(track_x - 11.0, y_of(0.0)), 1.8, theme::TEXT_DIM());

    // Round knob with a soft drop shadow and a ring in the strip's colour.
    let centre = pos2(track_x, fill_top);
    // The knob swells a little as the pointer arrives and more while it is held.
    let swell = anim(ui, id.with("swell"), resp.hovered() || resp.dragged(), 0.14);
    let lift = swell * 1.0 + if resp.dragged() { 1.0 } else { 0.0 };
    painter.circle_filled(
        pos2(centre.x, centre.y + 2.5),
        13.0 + lift,
        theme::shadow_color(60),
    );
    let knob_fill = if resp.dragged() {
        theme::PANEL_HI()
    } else if resp.hovered() {
        theme::PANEL()
    } else {
        theme::CARD()
    };
    painter.circle_filled(centre, 12.0 + lift, knob_fill);
    painter.circle_stroke(
        centre,
        12.0 + lift,
        Stroke::new(2.0, accent.linear_multiply(if dim { 0.5 } else { 1.0 })),
    );
    painter.circle_filled(centre, 3.0, accent);

    ((db - gain_db).abs() > f32::EPSILON).then_some(db)
}

/// A small latching pill with a text label: filled in `color` when on.
pub fn toggle(ui: &mut Ui, text: &str, on: bool, size: egui::Vec2, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = anim(ui, resp.id.with("h"), resp.hovered(), 0.12);
    let lit = anim(ui, resp.id.with("on"), on, 0.16);
    let rest = mix(theme::PANEL_HI(), theme::HOVER(), hover);
    let fill = mix(rest, color, lit);
    let fg = mix(theme::TEXT_DIM(), theme::ON_ACCENT(), lit);
    // A press squeezes the pill in slightly.
    let body = rect.shrink(if resp.is_pointer_button_down_on() {
        1.0
    } else {
        0.0
    });
    ui.painter().rect_filled(body, theme::pill(size.y), fill);
    focus_ring(ui, &resp, rect, theme::pill(size.y));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.5),
        fg,
    );
    resp
}

/// A processor button (EQ, gate, compressor). Amber text on a tint means the processor is
/// working; a solid amber pill means its editor is open.
pub fn process_button(ui: &mut Ui, text: &str, on: bool, open: bool, size: egui::Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = anim(ui, resp.id.with("h"), resp.hovered(), 0.12);
    let working = anim(ui, resp.id.with("on"), on, 0.16);
    let opened = anim(ui, resp.id.with("open"), open, 0.18);
    let rest = mix(theme::PANEL_HI(), theme::HOVER(), hover);
    let tinted = mix(rest, theme::ACCENT().linear_multiply(0.22), working);
    let fill = mix(tinted, theme::ACCENT(), opened);
    let fg = mix(
        mix(theme::TEXT_DIM(), theme::ACCENT(), working),
        theme::ON_ACCENT(),
        opened,
    );
    let body = rect.shrink(if resp.is_pointer_button_down_on() {
        1.0
    } else {
        0.0
    });
    ui.painter().rect_filled(body, theme::pill(size.y), fill);
    focus_ring(ui, &resp, rect, theme::pill(size.y));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(11.0),
        fg,
    );
    resp
}

/// A read-only pill of text, for status read-outs.
pub fn stat_chip(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(11.5);
    let w = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), color)
        .size()
        .x;
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, 24.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        theme::pill(24.0),
        theme::PANEL_HI().linear_multiply(0.7),
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        color,
    );
    resp
}

/// A quiet pill button with no fill until hovered or active, for secondary actions.
pub fn ghost_button(ui: &mut Ui, text: &str, active: bool) -> Response {
    let font = FontId::proportional(13.0);
    let w = ui
        .painter()
        .layout_no_wrap(text.to_string(), font.clone(), theme::TEXT())
        .size()
        .x;
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 26.0, 30.0), Sense::click());
    let hover = anim(ui, resp.id.with("h"), resp.hovered(), 0.12);
    let on = anim(ui, resp.id.with("on"), active, 0.16);
    let wash = mix(
        theme::PANEL_HI().linear_multiply(0.0),
        theme::PANEL_HI(),
        hover,
    );
    let fill = mix(wash, theme::ACTIVE(), on);
    ui.painter().rect_filled(rect, theme::pill(30.0), fill);
    focus_ring(ui, &resp, rect, theme::pill(30.0));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        mix(theme::TEXT_DIM(), theme::TEXT(), hover.max(on)),
    );
    resp
}

/// A capsule holding mutually exclusive options, the chosen one raised in amber. Returns the index
/// the user clicked.
pub fn segmented(ui: &mut Ui, id: Id, labels: &[(&str, &str)], selected: usize) -> Option<usize> {
    const SEG_W: f32 = 66.0;
    const H: f32 = 34.0;
    let n = labels.len();
    let (rect, _) = ui.allocate_exact_size(vec2(SEG_W * n as f32 + 8.0, H), Sense::hover());
    ui.painter()
        .rect_filled(rect, theme::pill(H), theme::PANEL());
    // The amber thumb glides to the chosen segment.
    let at = ui
        .ctx()
        .animate_value_with_time(id.with("thumb"), selected as f32, 0.2);
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let frac = at.clamp(0.0, (n - 1) as f32);
    let thumb = Rect::from_min_size(
        rect.min + vec2(4.0 + frac * SEG_W, 4.0),
        vec2(SEG_W, H - 8.0),
    );
    ui.painter()
        .rect_filled(thumb, theme::pill(H - 8.0), theme::ACCENT());
    let mut clicked = None;
    for (i, (label, tip)) in labels.iter().enumerate() {
        let seg = Rect::from_min_size(
            rect.min + vec2(4.0 + i as f32 * SEG_W, 4.0),
            vec2(SEG_W, H - 8.0),
        );
        let resp = ui
            .interact(seg, id.with(i), Sense::click())
            .on_hover_text(*tip);
        // How much of this segment the thumb covers decides its text colour.
        let cover = ease((1.0 - (frac - i as f32).abs()).clamp(0.0, 1.0));
        let hover = anim(ui, id.with(("h", i)), resp.hovered(), 0.12);
        let idle = mix(theme::TEXT_DIM(), theme::TEXT(), hover);
        ui.painter().text(
            seg.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            FontId::proportional(13.0),
            mix(idle, theme::ON_ACCENT(), cover),
        );
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// A small dim heading above a group of controls.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(9.5)
            .strong()
            .color(theme::TEXT_DIM().linear_multiply(0.85)),
    );
}

/// Horizontal balance slider with a centre notch. Double-click recentres. Returns the new value.
pub fn balance(ui: &mut Ui, id: Id, pan: f32, width: f32) -> Option<f32> {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
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
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 16.0, 6.0));
    painter.rect_filled(track, CornerRadius::same(3), theme::METER_BG());
    painter.circle_filled(rect.center(), 1.8, theme::TEXT_DIM());
    let x = rect.center().x + v * (track.width() / 2.0);
    let c = pos2(x, rect.center().y);
    painter.circle_filled(pos2(c.x, c.y + 1.5), 8.5, theme::shadow_color(50));
    painter.circle_filled(c, 8.0, theme::CARD());
    painter.circle_stroke(c, 8.0, Stroke::new(2.0, theme::ACCENT()));
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
