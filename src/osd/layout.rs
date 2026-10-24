//! Where windows that belong to the taskbar go, and how big the on-screen display is. Plain
//! numbers, no Win32, so it is tested on any platform. Ported from voicemeeter-osd and extended
//! to handle a taskbar on any edge of the screen.
// Only the Windows code uses these outside tests.
#![cfg_attr(not(windows), allow(dead_code))]

/// Space reserved around the card for the drop shadow, in 96-dpi pixels.
pub const SHADOW: i32 = 10;
/// The callout tail that points at the tray icon.
pub const TAIL_H: i32 = 9;
pub const TAIL_W: i32 = 18;

/// A screen rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Which side of the monitor the taskbar is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

/// The taskbar is whatever the work area leaves out of the monitor. With an auto-hiding taskbar
/// the two are equal, so fall back to whichever screen edge the tray icon is closest to.
pub fn taskbar_edge(full: Area, work: Area, anchor: Option<(i32, i32)>) -> Edge {
    if work.bottom < full.bottom {
        Edge::Bottom
    } else if work.top > full.top {
        Edge::Top
    } else if work.left > full.left {
        Edge::Left
    } else if work.right < full.right {
        Edge::Right
    } else {
        let (x, y) = anchor.unwrap_or((full.right, full.bottom));
        let to = [
            (full.bottom - y, Edge::Bottom),
            (y - full.top, Edge::Top),
            (x - full.left, Edge::Left),
            (full.right - x, Edge::Right),
        ];
        to.into_iter()
            .min_by_key(|(d, _)| *d)
            .map(|(_, e)| e)
            .unwrap_or(Edge::Bottom)
    }
}

/// The tail only makes sense when docked beside a horizontal taskbar, where there is an icon for
/// it to point at.
pub fn has_tail(edge: Edge) -> bool {
    matches!(edge, Edge::Bottom | Edge::Top)
}

/// Full window size including shadow padding and, if wanted, the tail, in physical pixels.
pub fn size_for(vertical: bool, dpi: i32, tail: bool) -> (i32, i32) {
    let (w, h) = if vertical { (66, 226) } else { (300, 86) };
    let pad = SHADOW * 2;
    let tail = if tail { TAIL_H } else { 0 };
    ((w + pad) * dpi / 96, (h + pad + tail) * dpi / 96)
}

/// Where the on-screen display wants to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Next to the tray icon, on the taskbar's edge.
    Dock {
        edge: Edge,
        anchor: Option<(i32, i32)>,
    },
    /// Horizontally centred, low on the screen.
    Centre,
    /// Wherever the user dragged it.
    Custom { x: i32, y: i32 },
}

/// Top-left of the window for `place`, shadow padding accounted for.
pub fn origin(place: Place, work: Area, w: i32, h: i32, dpi: i32) -> (i32, i32) {
    let gap = 10 * dpi / 96;
    let pad = SHADOW * dpi / 96;
    match place {
        Place::Dock { edge, anchor } => {
            // Clamp the position along the taskbar so the card stays on screen.
            let along_x = |ideal: Option<i32>| match ideal {
                Some(x) => {
                    let lo = work.left - pad + gap;
                    let hi = work.right - w + pad - gap;
                    x.clamp(lo.min(hi), hi)
                }
                None => work.right - w + pad - gap,
            };
            let along_y = |ideal: Option<i32>| match ideal {
                Some(y) => {
                    let lo = work.top - pad + gap;
                    let hi = work.bottom - h + pad - gap;
                    y.clamp(lo.min(hi), hi)
                }
                None => work.bottom - h + pad - gap,
            };
            match edge {
                Edge::Bottom => (
                    along_x(anchor.map(|(tx, _)| tx - w / 2)),
                    work.bottom - h + pad - gap,
                ),
                Edge::Top => (
                    along_x(anchor.map(|(tx, _)| tx - w / 2)),
                    work.top - pad + gap,
                ),
                Edge::Left => (
                    work.left - pad + gap,
                    along_y(anchor.map(|(_, ty)| ty - h / 2)),
                ),
                Edge::Right => (
                    work.right - w + pad - gap,
                    along_y(anchor.map(|(_, ty)| ty - h / 2)),
                ),
            }
        }
        // Saved coordinates can outlive the display they were set on; keep enough of the card on
        // screen to grab it back.
        Place::Custom { x, y } => {
            let visible = 48 * dpi / 96;
            let min_x = work.left - w + visible;
            let max_x = work.right - visible;
            let min_y = work.top - h + visible;
            let max_y = work.bottom - visible;
            (
                x.clamp(min_x.min(max_x), max_x),
                y.clamp(min_y.min(max_y), max_y),
            )
        }
        // Sitting low, an eighth of the screen up, so it floats clear of whatever you're looking at.
        Place::Centre => (
            work.left + (work.right - work.left - w) / 2,
            work.bottom - h - (work.bottom - work.top) / 8,
        ),
    }
}

/// Where a flyout window sits when it opens from the taskbar, and which way it slides in from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlyoutPlan {
    /// Final top-left, in physical pixels.
    pub x: i32,
    pub y: i32,
    /// Offset from the final position where the slide starts (the flyout rises out of the taskbar).
    pub slide: (i32, i32),
}

/// Plan a flyout of `w` x `h` pixels against the taskbar on `edge`. It hugs the taskbar with a
/// `margin`, centred on the tray icon where there's room and pushed back inside the work area
/// where there isn't, and slides in `slide` pixels from the taskbar side.
pub fn flyout_plan(
    edge: Edge,
    work: Area,
    anchor: Option<(i32, i32)>,
    (w, h): (i32, i32),
    margin: i32,
    slide: i32,
) -> FlyoutPlan {
    let along_x = |center: Option<i32>| match center {
        Some(cx) => {
            let lo = work.left + margin;
            let hi = work.right - w - margin;
            (cx - w / 2).clamp(lo.min(hi), hi)
        }
        None => work.right - w - margin,
    };
    let along_y = |center: Option<i32>| match center {
        Some(cy) => {
            let lo = work.top + margin;
            let hi = work.bottom - h - margin;
            (cy - h / 2).clamp(lo.min(hi), hi)
        }
        None => work.bottom - h - margin,
    };
    let ax = anchor.map(|a| a.0);
    let ay = anchor.map(|a| a.1);
    match edge {
        Edge::Bottom => FlyoutPlan {
            x: along_x(ax),
            y: work.bottom - h - margin,
            slide: (0, slide),
        },
        Edge::Top => FlyoutPlan {
            x: along_x(ax),
            y: work.top + margin,
            slide: (0, -slide),
        },
        Edge::Left => FlyoutPlan {
            x: work.left + margin,
            y: along_y(ay),
            slide: (-slide, 0),
        },
        Edge::Right => FlyoutPlan {
            x: work.right - w - margin,
            y: along_y(ay),
            slide: (slide, 0),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Area = Area {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn work(left: i32, top: i32, right: i32, bottom: i32) -> Area {
        Area {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_taskbar_edge_is_what_the_work_area_leaves_out() {
        assert_eq!(
            taskbar_edge(FULL, work(0, 0, 1920, 1040), None),
            Edge::Bottom
        );
        assert_eq!(taskbar_edge(FULL, work(0, 40, 1920, 1080), None), Edge::Top);
        assert_eq!(
            taskbar_edge(FULL, work(60, 0, 1920, 1080), None),
            Edge::Left
        );
        assert_eq!(
            taskbar_edge(FULL, work(0, 0, 1860, 1080), None),
            Edge::Right
        );
    }

    #[test]
    fn an_auto_hidden_taskbar_is_found_from_the_icon() {
        // Work area equals the monitor, so look at where the tray icon sits.
        assert_eq!(taskbar_edge(FULL, FULL, Some((1800, 1070))), Edge::Bottom);
        assert_eq!(taskbar_edge(FULL, FULL, Some((1800, 10))), Edge::Top);
        assert_eq!(taskbar_edge(FULL, FULL, Some((5, 500))), Edge::Left);
        assert_eq!(taskbar_edge(FULL, FULL, Some((1915, 500))), Edge::Right);
        assert_eq!(taskbar_edge(FULL, FULL, None), Edge::Bottom);
    }

    #[test]
    fn only_horizontal_taskbars_get_a_tail() {
        assert!(has_tail(Edge::Bottom) && has_tail(Edge::Top));
        assert!(!has_tail(Edge::Left) && !has_tail(Edge::Right));
    }

    #[test]
    fn size_scales_with_dpi_and_the_tail_adds_height() {
        assert_eq!(size_for(false, 96, false), (320, 106));
        assert_eq!(size_for(false, 96, true), (320, 115));
        assert_eq!(size_for(false, 192, false), (640, 212));
        assert_eq!(size_for(true, 96, false), (86, 246));
    }

    #[test]
    fn docked_above_a_bottom_taskbar_is_centred_on_the_icon() {
        let wa = work(0, 0, 1920, 1040);
        let (w, h) = size_for(false, 96, true);
        let place = Place::Dock {
            edge: Edge::Bottom,
            anchor: Some((1700, 1060)),
        };
        let (x, y) = origin(place, wa, w, h, 96);
        assert_eq!(x, 1700 - w / 2);
        // Sits just above the taskbar, shadow padding hanging into the margin.
        assert_eq!(y, 1040 - h + 10 - 10);
    }

    #[test]
    fn docking_clamps_to_the_screen_edge() {
        let wa = work(0, 0, 1920, 1040);
        let (w, h) = size_for(false, 96, true);
        let (x, _) = origin(
            Place::Dock {
                edge: Edge::Bottom,
                anchor: Some((1915, 1060)),
            },
            wa,
            w,
            h,
            96,
        );
        assert_eq!(x, 1920 - w + 10 - 10, "pushed back inside the right edge");
        let (x, _) = origin(
            Place::Dock {
                edge: Edge::Bottom,
                anchor: Some((3, 1060)),
            },
            wa,
            w,
            h,
            96,
        );
        assert_eq!(x, 0 - 10 + 10, "and the left edge");
    }

    #[test]
    fn docking_beside_a_top_or_side_taskbar() {
        let (w, h) = size_for(false, 96, false);
        let top = origin(
            Place::Dock {
                edge: Edge::Top,
                anchor: Some((900, 10)),
            },
            work(0, 40, 1920, 1080),
            w,
            h,
            96,
        );
        assert_eq!(top.1, 40 - 10 + 10, "just under the taskbar");
        let right = origin(
            Place::Dock {
                edge: Edge::Right,
                anchor: Some((1910, 500)),
            },
            work(0, 0, 1860, 1080),
            w,
            h,
            96,
        );
        assert_eq!(right.0, 1860 - w + 10 - 10, "just left of the taskbar");
        assert_eq!(right.1, 500 - h / 2);
    }

    #[test]
    fn a_missing_anchor_docks_in_the_corner() {
        let wa = work(0, 0, 1920, 1040);
        let (w, h) = size_for(false, 96, true);
        let (x, _) = origin(
            Place::Dock {
                edge: Edge::Bottom,
                anchor: None,
            },
            wa,
            w,
            h,
            96,
        );
        assert_eq!(x, 1920 - w);
    }

    #[test]
    fn centre_is_centred_and_low() {
        let wa = work(0, 0, 1920, 1040);
        let (w, h) = size_for(false, 96, false);
        let (x, y) = origin(Place::Centre, wa, w, h, 96);
        assert_eq!(x, (1920 - w) / 2);
        assert_eq!(y, 1040 - h - 130);
    }

    #[test]
    fn a_saved_position_off_a_removed_monitor_is_pulled_back() {
        let wa = work(0, 0, 1920, 1040);
        let (w, h) = size_for(false, 96, false);
        let (x, y) = origin(Place::Custom { x: 5000, y: 3000 }, wa, w, h, 96);
        assert_eq!((x, y), (1920 - 48, 1040 - 48));
        let (x, y) = origin(Place::Custom { x: 300, y: 200 }, wa, w, h, 96);
        assert_eq!((x, y), (300, 200), "an on-screen position is kept");
    }

    #[test]
    fn a_flyout_rises_out_of_a_bottom_taskbar_by_the_tray_icon() {
        let wa = work(0, 0, 1920, 1040);
        let p = flyout_plan(Edge::Bottom, wa, Some((1850, 1060)), (370, 500), 12, 48);
        assert_eq!(p.y, 1040 - 500 - 12, "sits just above the taskbar");
        assert_eq!(p.x, 1920 - 370 - 12, "pushed back inside the right edge");
        assert_eq!(p.slide, (0, 48), "starts lower, inside the taskbar");
    }

    #[test]
    fn a_flyout_centres_on_an_icon_that_has_room() {
        let wa = work(0, 0, 1920, 1040);
        let p = flyout_plan(Edge::Bottom, wa, Some((960, 1060)), (370, 500), 12, 48);
        assert_eq!(p.x, 960 - 185);
    }

    #[test]
    fn flyouts_open_away_from_a_top_or_side_taskbar() {
        let (w, h) = (370, 500);
        let top = flyout_plan(
            Edge::Top,
            work(0, 40, 1920, 1080),
            Some((1850, 20)),
            (w, h),
            12,
            48,
        );
        assert_eq!((top.y, top.slide), (40 + 12, (0, -48)));
        let left = flyout_plan(
            Edge::Left,
            work(60, 0, 1920, 1080),
            Some((30, 900)),
            (w, h),
            12,
            48,
        );
        assert_eq!((left.x, left.slide), (60 + 12, (-48, 0)));
        assert_eq!(
            left.y,
            1080 - 500 - 12,
            "centred on the icon would run off the bottom, so it is pushed up"
        );
        let right = flyout_plan(
            Edge::Right,
            work(0, 0, 1860, 1080),
            Some((1890, 100)),
            (w, h),
            12,
            48,
        );
        assert_eq!((right.x, right.slide), (1860 - w - 12, (48, 0)));
        assert_eq!(right.y, 12, "kept inside the top edge");
    }

    #[test]
    fn a_flyout_without_an_icon_goes_in_the_corner() {
        let wa = work(0, 0, 1920, 1040);
        let p = flyout_plan(Edge::Bottom, wa, None, (370, 500), 12, 48);
        assert_eq!((p.x, p.y), (1920 - 370 - 12, 1040 - 500 - 12));
    }
}
