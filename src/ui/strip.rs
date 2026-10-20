//! The mixer strip: a vertical fader with a live VU meter beside it, a peak-hold marker, a percent
//! readout and a mute button.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, Rect, Sense, Stroke, StrokeKind,
    Ui,
};

use super::theme;
use crate::model::{amp_to_db, db_to_meter, percent};

pub const STRIP_W: f32 = 78.0;
const TRACK_H: f32 = 190.0;
const METER_W: f32 = 10.0;
/// Fraction of full scale the peak marker falls per second.
const PEAK_FALL_PER_SEC: f32 = 0.35;
/// Mouse-wheel step, in volume scalar per notch.
const SCROLL_STEP: f32 = 0.02;

pub struct Strip<'a> {
    pub id: Id,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
    pub accent: Color32,
    /// Dim the strip when the app isn't making sound.
    pub idle: bool,
}

#[derive(Default)]
pub struct StripOutput {
    pub volume: Option<f32>,
    pub mute: Option<bool>,
}

impl Strip<'_> {
    pub fn show(self, ui: &mut Ui) -> StripOutput {
        let mut out = StripOutput::default();
        ui.allocate_ui_with_layout(
            vec2(STRIP_W, 0.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                ui.set_width(STRIP_W);
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);

                let title_color = if self.idle {
                    theme::TEXT_DIM
                } else {
                    theme::TEXT
                };
                let title = truncate(ui, self.title, STRIP_W - 4.0, 12.0);
                ui.label(egui::RichText::new(title).size(12.0).color(title_color))
                    .on_hover_text(self.title);
                let sub = truncate(ui, self.subtitle, STRIP_W - 4.0, 10.0);
                ui.label(egui::RichText::new(sub).size(10.0).color(theme::TEXT_DIM));

                let (rect, _) = ui.allocate_exact_size(vec2(STRIP_W, TRACK_H), Sense::hover());
                self.fader(ui, rect, &mut out);

                let pct = percent(self.volume);
                let (txt, col) = if self.muted {
                    ("MUTE".to_string(), theme::MUTE)
                } else {
                    (format!("{pct}%"), theme::TEXT)
                };
                ui.label(egui::RichText::new(txt).size(13.0).strong().color(col));

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

                let (fill, fg) = if self.muted {
                    (theme::MUTE, Color32::WHITE)
                } else {
                    (theme::PANEL_HI, theme::TEXT_DIM)
                };
                let btn = egui::Button::new(egui::RichText::new("M").strong().color(fg))
                    .fill(fill)
                    .min_size(vec2(32.0, 22.0));
                if ui.add(btn).on_hover_text("Mute").clicked() {
                    out.mute = Some(!self.muted);
                }
            },
        );
        out
    }

    fn fader(&self, ui: &mut Ui, rect: Rect, out: &mut StripOutput) {
        let painter = ui.painter();
        let meter_rect = Rect::from_min_size(
            pos2(rect.left() + 6.0, rect.top()),
            vec2(METER_W, rect.height()),
        );
        let track_x = rect.center().x + 6.0;
        let track = Rect::from_min_max(
            pos2(track_x - 3.0, rect.top() + 8.0),
            pos2(track_x + 3.0, rect.bottom() - 8.0),
        );
        let hit = Rect::from_min_max(
            pos2(track_x - 22.0, rect.top()),
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
        draw_meter(painter, meter_rect.shrink(1.0), level, self.idle);
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
        let shown = vol;

        // --- track + fill + knob ---
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(3), theme::METER_BG);
        painter.rect_stroke(
            track,
            CornerRadius::same(3),
            Stroke::new(1.0, theme::BORDER),
            StrokeKind::Inside,
        );
        let fill_top = track.bottom() - track.height() * shown;
        let fill = Rect::from_min_max(pos2(track.left(), fill_top), track.max);
        let accent = if self.muted {
            theme::TEXT_DIM
        } else {
            self.accent
        };
        painter.rect_filled(fill, CornerRadius::same(3), accent.linear_multiply(0.55));
        let knob = Rect::from_center_size(pos2(track_x, fill_top), vec2(30.0, 16.0));
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
        if resp.hovered() || resp.dragged() {
            painter.text(
                pos2(rect.right() - 2.0, fill_top),
                Align2::RIGHT_CENTER,
                format!("{}", percent(shown)),
                FontId::proportional(10.0),
                theme::TEXT_DIM,
            );
        }
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
fn draw_meter(painter: &egui::Painter, rect: Rect, level: f32, idle: bool) {
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
