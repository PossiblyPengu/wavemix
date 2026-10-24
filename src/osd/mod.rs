//! On-screen volume display, ported from voicemeeter-osd. `layout` is plain geometry and is tested
//! everywhere; the window itself is Windows-only.

#[cfg(windows)]
mod display;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod winosd;
pub mod layout;
pub mod wheel;

#[cfg(windows)]
pub use display::{MonitorInfo, Osd, OsdEvent, OsdSettings};
