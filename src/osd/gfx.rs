//! Thin GDI+ layer: antialiased shapes, text, and per-pixel-alpha layered windows.
//! Copied from voicemeeter-osd, minus the helpers only its settings preview used.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Graphics::GdiPlus::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;

pub const ALIGN_NEAR: StringAlignment = 0;
pub const ALIGN_CENTER: StringAlignment = 1;

pub fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

pub fn tint(color: (u8, u8, u8), a: u8) -> u32 {
    argb(a, color.0, color.1, color.2)
}

/// Lightens or darkens toward white/black by `amount` (0.0..1.0).
pub fn shade(color: (u8, u8, u8), amount: f32) -> (u8, u8, u8) {
    let f = |c: u8| {
        let c = c as f32;
        let v = if amount >= 0.0 {
            c + (255.0 - c) * amount
        } else {
            c * (1.0 + amount)
        };
        v.clamp(0.0, 255.0) as u8
    };
    (f(color.0), f(color.1), f(color.2))
}

pub struct Gdiplus(usize);

impl Gdiplus {
    pub fn start() -> Self {
        unsafe {
            let mut token: usize = 0;
            let input = GdiplusStartupInput {
                GdiplusVersion: 1,
                DebugEventCallback: 0,
                SuppressBackgroundThread: 0,
                SuppressExternalCodecs: 0,
            };
            let mut output: GdiplusStartupOutput = std::mem::zeroed();
            GdiplusStartup(&mut token, &input, &mut output);
            Gdiplus(token)
        }
    }
}

impl Drop for Gdiplus {
    fn drop(&mut self) {
        unsafe { GdiplusShutdown(self.0) }
    }
}

// GDI+ objects reused across frames. All drawing happens on the UI thread,
// and the handful of distinct sizes is bounded, so they live for the process
// (deliberately never freed: GDI+ is shut down before TLS teardown).
thread_local! {
    static FONT_FAMILY: Cell<*mut GpFontFamily> = const { Cell::new(null_mut()) };
    static FONTS: RefCell<HashMap<(u32, bool), *mut GpFont>> = RefCell::new(HashMap::new());
    static FORMATS: RefCell<HashMap<StringAlignment, *mut GpStringFormat>> =
        RefCell::new(HashMap::new());
}

unsafe fn font_family() -> *mut GpFontFamily {
    FONT_FAMILY.with(|cell| {
        if cell.get().is_null() {
            let name: Vec<u16> = "Segoe UI"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let mut family = null_mut();
            if GdipCreateFontFamilyFromName(name.as_ptr(), null_mut(), &mut family) != 0 {
                let fallback: Vec<u16> = "Arial".encode_utf16().chain(std::iter::once(0)).collect();
                GdipCreateFontFamilyFromName(fallback.as_ptr(), null_mut(), &mut family);
            }
            cell.set(family);
        }
        cell.get()
    })
}

unsafe fn font(size: f32, bold: bool) -> *mut GpFont {
    let family = font_family();
    if family.is_null() {
        return null_mut();
    }
    FONTS.with(|fonts| {
        *fonts
            .borrow_mut()
            .entry((size.to_bits(), bold))
            .or_insert_with(|| {
                let mut font = null_mut();
                let style = if bold { 1 } else { 0 };
                if GdipCreateFont(family, size, style, UnitPixel, &mut font) != 0 {
                    font = null_mut();
                }
                font
            })
    })
}

unsafe fn string_format(align: StringAlignment) -> *mut GpStringFormat {
    FORMATS.with(|formats| {
        *formats.borrow_mut().entry(align).or_insert_with(|| {
            let mut format = null_mut();
            GdipCreateStringFormat(0, 0, &mut format);
            GdipSetStringFormatAlign(format, align);
            GdipSetStringFormatLineAlign(format, ALIGN_CENTER);
            format
        })
    })
}

unsafe fn round_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> *mut GpPath {
    let mut path = null_mut();
    GdipCreatePath(FillModeAlternate, &mut path);
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.05 {
        GdipAddPathLine(path, x, y, x + w, y);
        GdipAddPathLine(path, x + w, y, x + w, y + h);
        GdipAddPathLine(path, x + w, y + h, x, y + h);
    } else {
        let d = r * 2.0;
        GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
    }
    GdipClosePathFigure(path);
    path
}

pub struct Canvas {
    pub graphics: *mut GpGraphics,
    /// False when the graphics context belongs to something else (a
    /// LayeredSurface), which must outlive this borrow.
    owned: bool,
}

impl Canvas {
    pub unsafe fn fill_round_rect(&self, x: f32, y: f32, w: f32, h: f32, r: f32, color: u32) {
        let path = round_rect_path(x, y, w, h, r);
        let mut brush = null_mut();
        GdipCreateSolidFill(color, &mut brush);
        GdipFillPath(self.graphics, brush as *mut GpBrush, path);
        GdipDeleteBrush(brush as *mut GpBrush);
        GdipDeletePath(path);
    }

    #[allow(clippy::too_many_arguments)]
    pub unsafe fn fill_round_rect_gradient(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        top: u32,
        bottom: u32,
    ) {
        let path = round_rect_path(x, y, w, h, r);
        // Inflated rect avoids GDI+ clamping artifacts at the gradient edges.
        let rect = RectF {
            X: x,
            Y: y - 0.5,
            Width: w.max(1.0),
            Height: h.max(1.0) + 1.0,
        };
        let mut brush = null_mut();
        GdipCreateLineBrushFromRect(
            &rect,
            top,
            bottom,
            LinearGradientModeVertical,
            WrapModeTileFlipXY,
            &mut brush,
        );
        GdipFillPath(self.graphics, brush as *mut GpBrush, path);
        GdipDeleteBrush(brush as *mut GpBrush);
        GdipDeletePath(path);
    }

    #[allow(clippy::too_many_arguments)]
    pub unsafe fn stroke_round_rect(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        color: u32,
        width: f32,
    ) {
        let path = round_rect_path(x, y, w, h, r);
        let mut pen = null_mut();
        GdipCreatePen1(color, width, UnitPixel, &mut pen);
        GdipDrawPath(self.graphics, pen, path);
        GdipDeletePen(pen);
        GdipDeletePath(path);
    }

    pub unsafe fn fill_polygon(&self, points: &[(f32, f32)], color: u32) {
        if points.len() < 3 {
            return;
        }
        let mut path = null_mut();
        GdipCreatePath(FillModeAlternate, &mut path);
        for pair in points.windows(2) {
            GdipAddPathLine(path, pair[0].0, pair[0].1, pair[1].0, pair[1].1);
        }
        GdipClosePathFigure(path);
        let mut brush = null_mut();
        GdipCreateSolidFill(color, &mut brush);
        GdipFillPath(self.graphics, brush as *mut GpBrush, path);
        GdipDeleteBrush(brush as *mut GpBrush);
        GdipDeletePath(path);
    }

    #[allow(clippy::too_many_arguments)]
    pub unsafe fn stroke_arc(
        &self,
        cx: f32,
        cy: f32,
        radius: f32,
        start: f32,
        sweep: f32,
        color: u32,
        width: f32,
    ) {
        let mut pen = null_mut();
        GdipCreatePen1(color, width, UnitPixel, &mut pen);
        GdipSetPenStartCap(pen, LineCapRound);
        GdipSetPenEndCap(pen, LineCapRound);
        GdipDrawArc(
            self.graphics,
            pen,
            cx - radius,
            cy - radius,
            radius * 2.0,
            radius * 2.0,
            start,
            sweep,
        );
        GdipDeletePen(pen);
    }

    pub unsafe fn stroke_line(&self, x1: f32, y1: f32, x2: f32, y2: f32, color: u32, width: f32) {
        let mut pen = null_mut();
        GdipCreatePen1(color, width, UnitPixel, &mut pen);
        GdipSetPenStartCap(pen, LineCapRound);
        GdipSetPenEndCap(pen, LineCapRound);
        GdipDrawLine(self.graphics, pen, x1, y1, x2, y2);
        GdipDeletePen(pen);
    }

    #[allow(clippy::too_many_arguments)]
    pub unsafe fn draw_text(
        &self,
        text: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        size: f32,
        color: u32,
        align: StringAlignment,
        bold: bool,
    ) {
        let font = font(size, bold);
        if font.is_null() {
            return;
        }
        let format = string_format(align);

        let mut brush = null_mut();
        GdipCreateSolidFill(color, &mut brush);

        let rect = RectF {
            X: x,
            Y: y,
            Width: w,
            Height: h,
        };
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        GdipDrawString(
            self.graphics,
            wide.as_ptr(),
            -1,
            font,
            &rect,
            format,
            brush as *mut GpBrush,
        );

        GdipDeleteBrush(brush as *mut GpBrush);
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        if self.owned {
            unsafe { GdipDeleteGraphics(self.graphics) };
        }
    }
}

/// An off-screen premultiplied-ARGB surface that can be pushed to a layered
/// window, giving true per-pixel alpha (antialiased corners, soft shadows).
pub struct LayeredSurface {
    pub dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    gp_bitmap: *mut GpBitmap,
    pub graphics: *mut GpGraphics,
    pub width: i32,
    pub height: i32,
}

impl LayeredSurface {
    pub unsafe fn new(width: i32, height: i32) -> Option<Self> {
        let screen_dc = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen_dc);
        ReleaseDC(null_mut(), screen_dc);

        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut core::ffi::c_void = null_mut();
        let bitmap = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        std::ptr::write_bytes(bits as *mut u8, 0, (width * height * 4) as usize);
        let old_bitmap = SelectObject(dc, bitmap);

        // Wrapping the DIB as PARGB makes GDI+ write exactly the format
        // UpdateLayeredWindow expects, with no manual premultiply pass.
        let mut gp_bitmap = null_mut();
        GdipCreateBitmapFromScan0(
            width,
            height,
            width * 4,
            PIXEL_FORMAT_32BPP_PARGB,
            bits as *const u8,
            &mut gp_bitmap,
        );
        let mut graphics = null_mut();
        GdipGetImageGraphicsContext(gp_bitmap as *mut GpImage, &mut graphics);
        GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        GdipSetTextRenderingHint(graphics, TextRenderingHintAntiAlias);

        Some(LayeredSurface {
            dc,
            bitmap,
            old_bitmap,
            gp_bitmap,
            graphics,
            width,
            height,
        })
    }

    /// Borrows the surface's graphics context; dropping it must not destroy
    /// the context, which is reused for every frame.
    pub fn canvas(&self) -> Canvas {
        Canvas {
            graphics: self.graphics,
            owned: false,
        }
    }

    /// Pushes the top-left `width` x `height` region of the surface to the
    /// window at `alpha` overall opacity.
    pub unsafe fn commit(&self, hwnd: HWND, x: i32, y: i32, width: i32, height: i32, alpha: u8) {
        GdipFlush(self.graphics, FlushIntentionSync);
        let pos = POINT { x, y };
        let size = SIZE {
            cx: width.clamp(1, self.width),
            cy: height.clamp(1, self.height),
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            null_mut(),
            &pos,
            &size,
            self.dc,
            &src,
            0,
            &blend,
            ULW_ALPHA,
        );
    }
}

impl Drop for LayeredSurface {
    fn drop(&mut self) {
        unsafe {
            // Graphics must go before the bitmap that backs it.
            GdipDeleteGraphics(self.graphics);
            GdipDisposeImage(self.gp_bitmap as *mut GpImage);
            SelectObject(self.dc, self.old_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}
