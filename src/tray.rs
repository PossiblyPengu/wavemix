//! Notification-area icon. Events arrive on the shell's thread; they are forwarded over a channel
//! and the UI is woken so it can handle them even while the window is hidden.

use std::sync::mpsc::{self, Receiver};

use crate::config::View;
use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug)]
pub enum TrayAction {
    /// Left-click on the icon.
    Toggle,
    /// "Open Wavemix" in the menu.
    Open,
    SetView(View),
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    rx: Receiver<TrayAction>,
}

const OPEN_ID: &str = "wavemix-open";
const SIMPLE_ID: &str = "wavemix-simple";
const PRO_ID: &str = "wavemix-pro";
const QUIT_ID: &str = "wavemix-quit";

impl Tray {
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        let menu = Menu::new();
        let open = MenuItem::with_id(OPEN_ID, "Open Wavemix", true, None);
        let simple = MenuItem::with_id(SIMPLE_ID, "Simple mode", true, None);
        let pro = MenuItem::with_id(PRO_ID, "Pro mode", true, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        menu.append_items(&[
            &open,
            &simple,
            &pro,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;

        let (tx, rx) = mpsc::channel();

        let (t, c) = (tx.clone(), ctx.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                let _ = t.send(TrayAction::Toggle);
                c.request_repaint();
            }
        }));

        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let action = match e.id.0.as_str() {
                OPEN_ID => TrayAction::Open,
                SIMPLE_ID => TrayAction::SetView(View::Compact),
                PRO_ID => TrayAction::SetView(View::Console),
                QUIT_ID => TrayAction::Quit,
                _ => return,
            };
            let _ = tx.send(action);
            c.request_repaint();
        }));

        let icon = TrayIconBuilder::new()
            .with_icon(make_icon())
            .with_tooltip("Wavemix")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;
        Some(Self { icon, rx })
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        self.rx.try_iter().collect()
    }

    /// Centre of the tray icon in physical screen pixels, if the shell can say where it is. When
    /// the icon is tucked into the overflow flyout this is the chevron's position.
    pub fn center(&self) -> Option<(i32, i32)> {
        let r = self.icon.rect()?;
        if r.size.width == 0 || r.size.height == 0 {
            return None;
        }
        Some((
            (r.position.x + r.size.width as f64 / 2.0) as i32,
            (r.position.y + r.size.height as f64 / 2.0) as i32,
        ))
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
