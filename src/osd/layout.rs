//! Where the on-screen display goes and how big it is. Plain numbers, no Win32, so it is tested
//! on any platform. Ported from voicemeeter-osd.
// Only the Windows display uses these outside tests.
#![cfg_attr(not(windows), allow(dead_code))]

/// Space reserved around the card for the drop shadow, in 96-dpi pixels.
pub const SHADOW: i32 = 10;

/// A screen rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Full window size including shadow padding, in physical pixels.
pub fn size_for(vertical: bool, dpi: i32) -> (i32, i32) {
    let (w, h) = if vertical { (66, 226) } else { (300, 86) };
    let pad = SHADOW * 2;
    ((w + pad) * dpi / 96, (h + pad) * dpi / 96)
}

/// Top-left of the window: horizontally centred on the work area, sitting low (an eighth of the
/// screen up from the bottom) so it floats clear of whatever you're looking at.
pub fn origin_for(work: Area, w: i32, h: i32) -> (i32, i32) {
    let x = work.left + (work.right - work.left - w) / 2;
    let y = work.bottom - h - (work.bottom - work.top) / 8;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_scales_with_dpi() {
        assert_eq!(size_for(false, 96), (320, 106));
        assert_eq!(size_for(false, 192), (640, 212));
        assert_eq!(size_for(true, 96), (86, 246));
    }

    #[test]
    fn origin_is_centred_and_low() {
        let work = Area {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let (w, h) = size_for(false, 96);
        let (x, y) = origin_for(work, w, h);
        assert_eq!(x, (1920 - w) / 2);
        assert_eq!(y, 1040 - h - 130);
    }

    #[test]
    fn origin_follows_a_second_monitor() {
        let work = Area {
            left: 1920,
            top: 0,
            right: 3840,
            bottom: 1080,
        };
        let (x, _) = origin_for(work, 320, 106);
        assert_eq!(x, 1920 + (1920 - 320) / 2);
    }
}
