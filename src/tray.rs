//! Notification-area icon. Events arrive on the shell's thread; they are forwarded over a channel
//! and the UI is woken so it can handle them even while the window is hidden.

use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Where the icon is on screen, in physical pixels: `(x, y, width, height)`.
pub type Anchor = (f64, f64, f64, f64);

#[derive(Debug)]
pub enum TrayAction {
    ToggleFlyout(Anchor),
    OpenWindow,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    rx: Receiver<TrayAction>,
}

const OPEN_ID: &str = "wavemix-open";
const QUIT_ID: &str = "wavemix-quit";

impl Tray {
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        let menu = Menu::new();
        let open = MenuItem::with_id(OPEN_ID, "Open mixer", true, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        menu.append_items(&[&open, &quit]).ok()?;

        let (tx, rx) = mpsc::channel();

        let (t, c) = (tx.clone(), ctx.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                rect,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                let anchor = (
                    rect.position.x,
                    rect.position.y,
                    rect.size.width as f64,
                    rect.size.height as f64,
                );
                let _ = t.send(TrayAction::ToggleFlyout(anchor));
                c.request_repaint();
            }
        }));

        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let action = match e.id.0.as_str() {
                OPEN_ID => TrayAction::OpenWindow,
                QUIT_ID => TrayAction::Quit,
                _ => return,
            };
            let _ = tx.send(action);
            c.request_repaint();
        }));

        let icon = TrayIconBuilder::new()
            .with_icon(make_icon())
            .with_tooltip("wavemix")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;
        Some(Self { _icon: icon, rx })
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        self.rx.try_iter().collect()
    }
}

/// Three mixer faders, drawn at runtime so there's no image asset to ship.
fn make_icon() -> tray_icon::Icon {
    const N: u32 = 32;
    let mut px = vec![0u8; (N * N * 4) as usize];
    // (x start, bar top, knob y)
    for (x0, top) in [(5u32, 8u32), (13, 3), (21, 12)] {
        for y in top..N - 3 {
            for x in x0..x0 + 6 {
                let i = ((y * N + x) * 4) as usize;
                let knob = y >= top && y < top + 5;
                let c = if knob {
                    [0xff, 0xff, 0xff]
                } else {
                    [0x4c, 0xa8, 0xff]
                };
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 0xff;
            }
        }
    }
    tray_icon::Icon::from_rgba(px, N, N).expect("icon dimensions match buffer")
}
