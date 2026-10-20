//! On-screen volume display, ported from voicemeeter-osd. `layout` is plain geometry and is tested
//! everywhere; the window itself is Windows-only.

#[cfg(windows)]
mod display;
#[cfg(windows)]
mod gfx;
pub mod layout;

#[cfg(windows)]
pub use display::{Osd, OsdSettings};
