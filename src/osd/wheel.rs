//! Mouse-wheel handling for the on-screen display, ported from voicemeeter-osd.
// Only the Windows display uses it outside tests.
#![cfg_attr(not(windows), allow(dead_code))]

/// Standard delta for one detent of a notched mouse wheel.
pub const WHEEL_DELTA: i32 = 120;

/// Turns raw wheel deltas into whole steps. Precision touchpads and free-spinning wheels report
/// fractions of a detent; dividing each event by 120 would throw those away, so the remainder
/// carries over.
#[derive(Default)]
pub struct WheelSteps {
    remainder: i32,
}

impl WheelSteps {
    pub fn feed(&mut self, delta: i32) -> i32 {
        // A change of direction starts fresh, so half a notch one way doesn't swallow the first
        // notch back.
        if (delta > 0 && self.remainder < 0) || (delta < 0 && self.remainder > 0) {
            self.remainder = 0;
        }
        self.remainder += delta;
        let steps = self.remainder / WHEEL_DELTA;
        self.remainder -= steps * WHEEL_DELTA;
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_notches_are_steps() {
        let mut w = WheelSteps::default();
        assert_eq!(w.feed(120), 1);
        assert_eq!(w.feed(-240), -2);
    }

    #[test]
    fn fractional_deltas_accumulate_instead_of_vanishing() {
        let mut w = WheelSteps::default();
        assert_eq!(w.feed(40), 0);
        assert_eq!(w.feed(40), 0);
        assert_eq!(w.feed(40), 1, "three thirds of a notch make one");
    }

    #[test]
    fn reversing_direction_starts_fresh() {
        let mut w = WheelSteps::default();
        assert_eq!(w.feed(100), 0);
        assert_eq!(
            w.feed(-120),
            -1,
            "the leftover upward 100 doesn't eat the first notch down"
        );
    }
}
