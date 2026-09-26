use std::ffi::c_void;
use std::ptr;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
    IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::core::{PCWSTR, Result};

use windows_numerics::Vector2;

use crate::native::wide;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 }
    }

    pub fn faded(self, opacity: f32) -> Self {
        Self { a: self.a * opacity, ..self }
    }

    /// Packed 0xAARRGGBB, e.g. for use as a cache key.
    pub fn to_argb(self) -> u32 {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
        channel(self.a) << 24 | channel(self.r) << 16 | channel(self.g) << 8 | channel(self.b)
    }

    fn to_d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.r, g: self.g, b: self.b, a: self.a }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn new(left: f32, top: f32, width: f32, height: f32) -> Self {
        Self { left, top, right: left + width, bottom: top + height }
    }

    pub fn inflate(self, by: f32) -> Self {
        Self { left: self.left - by, top: self.top - by, right: self.right + by, bottom: self.bottom + by }
    }

    fn to_rounded(self, radius: f32) -> D2D1_ROUNDED_RECT {
        D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: self.left, top: self.top, right: self.right, bottom: self.bottom },
            radiusX: radius,
            radiusY: radius,
        }
    }
}

pub struct Font(IDWriteTextFormat);

pub struct TextBlock {
    layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
}

/// 32-bit premultiplied top-down DIB selected into a memory DC; the format `UpdateLayeredWindow` expects.
pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: Option<HGDIOBJ>,
    pub width: i32,
    pub height: i32,
}

impl Surface {
    pub fn new(width: i32, height: i32) -> Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = unsafe { CreateCompatibleDC(None) };
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap = unsafe { CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) }.inspect_err(|_| {
            let _ = unsafe { DeleteDC(dc) };
        })?;
        let previous = Some(unsafe { SelectObject(dc, bitmap.into()) });
        Ok(Self { dc, bitmap, previous, width, height })
    }

    pub fn dc(&self) -> HDC {
        self.dc
    }

    /// Deselects the bitmap so it can be handed to APIs that copy it (e.g. `CreateIconIndirect`).
    pub fn into_bitmap_view(mut self) -> BitmapView {
        if let Some(previous) = self.previous.take() {
            unsafe { SelectObject(self.dc, previous) };
        }
        BitmapView(self)
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            if let Some(previous) = self.previous.take() {
                SelectObject(self.dc, previous);
            }
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

pub struct BitmapView(Surface);

impl BitmapView {
    pub fn bitmap(&self) -> HBITMAP {
        self.0.bitmap
    }
}

/// Direct2D + DirectWrite drawing into GDI surfaces. One per UI thread.
pub struct Renderer {
    write: IDWriteFactory,
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        let factory: ID2D1Factory = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }?;
        let write: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }?;
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 0.0,
            dpiY: 0.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let target = unsafe { factory.CreateDCRenderTarget(&properties) }?;
        // ClearType needs an opaque background; the layered window is transparent.
        unsafe { target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE) };
        let brush = unsafe { target.CreateSolidColorBrush(&Color::TRANSPARENT.to_d2d(), None) }?;
        Ok(Self { write, target, brush })
    }

    pub fn font(&self, family: &str, size: f32, weight: DWRITE_FONT_WEIGHT) -> Result<Font> {
        let family = wide(family);
        let locale = wide("en-us");
        let format = unsafe {
            self.write.CreateTextFormat(
                PCWSTR(family.as_ptr()),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                PCWSTR(locale.as_ptr()),
            )
        }?;
        unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) }?;
        Ok(Font(format))
    }

    pub fn text(&self, text: &str, font: &Font) -> Result<TextBlock> {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe { self.write.CreateTextLayout(&utf16, &font.0, f32::MAX, f32::MAX) }?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        unsafe { layout.GetMetrics(&mut metrics) }?;
        Ok(TextBlock { layout, width: metrics.width, height: metrics.height })
    }

    /// Draws in device-independent pixels; `dpi` maps them onto the surface's physical pixels.
    pub fn draw(&self, surface: &Surface, dpi: f32, paint: impl FnOnce(&Canvas)) -> Result<()> {
        let bounds = RECT { left: 0, top: 0, right: surface.width, bottom: surface.height };
        unsafe {
            self.target.BindDC(surface.dc, &bounds)?;
            self.target.SetDpi(dpi, dpi);
            self.target.BeginDraw();
            self.target.Clear(Some(&Color::TRANSPARENT.to_d2d()));
        }
        paint(&Canvas { renderer: self });
        unsafe { self.target.EndDraw(None, None) }
    }
}

pub struct Canvas<'a> {
    renderer: &'a Renderer,
}

impl Canvas<'_> {
    pub fn fill_rounded(&self, rect: Rect, radius: f32, color: Color) {
        unsafe {
            self.renderer.brush.SetColor(&color.to_d2d());
            self.renderer.target.FillRoundedRectangle(&rect.to_rounded(radius), &self.renderer.brush);
        }
    }

    pub fn stroke_rounded(&self, rect: Rect, radius: f32, width: f32, color: Color) {
        unsafe {
            self.renderer.brush.SetColor(&color.to_d2d());
            self.renderer.target.DrawRoundedRectangle(&rect.to_rounded(radius), &self.renderer.brush, width, None);
        }
    }

    pub fn text(&self, block: &TextBlock, x: f32, y: f32, color: Color) {
        unsafe {
            self.renderer.brush.SetColor(&color.to_d2d());
            self.renderer.target.DrawTextLayout(
                Vector2 { X: x, Y: y },
                &block.layout,
                &self.renderer.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
        }
    }
}
