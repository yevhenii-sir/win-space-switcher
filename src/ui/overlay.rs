use std::sync::Once;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, HWND_TOPMOST, MA_NOACTIVATE, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos, ShowWindow, ULW_ALPHA,
    UpdateLayeredWindow, WM_MOUSEACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, Result, w};

use super::fade::Fade;
use super::placement::panel_origin;
use super::renderer::{Canvas, Font, Rect, Renderer, Surface, TextBlock};
use super::theme::{self, Palette};
use super::tray::cursor_position;
use crate::settings::PanelPosition;

const CLASS_NAME: windows::core::PCWSTR = w!("WinSpaceSwitcherOverlay");

/// Distance from the screen edge or from the cursor.
const PANEL_GAP: f32 = 16.0;
const PANEL_RADIUS: f32 = 12.0;
const PANEL_PADDING: f32 = 6.0;
const BORDER: f32 = 1.0;
const ROW_RADIUS: f32 = 8.0;
const ROW_SPACING: f32 = 2.0;
const ROW_PADDING_LEFT: f32 = 12.0;
const ROW_PADDING_RIGHT: f32 = 16.0;
const ROW_PADDING_VERTICAL: f32 = 8.0;
const ROW_MIN_WIDTH: f32 = 200.0;
const SHORT_NAME_COLUMN: f32 = 40.0;
const HINT_MARGIN_TOP: f32 = 6.0;
const HINT_MARGIN_BOTTOM: f32 = 4.0;
const HINT_MARGIN_HORIZONTAL: f32 = 12.0;

#[derive(Clone, Debug)]
pub struct OverlayItem {
    pub short_name: String,
    pub display_name: String,
    pub is_current: bool,
    pub is_dimmed: bool,
}

pub struct ShowOptions {
    pub position: PanelPosition,
    pub all_screens: bool,
    pub fade_in: Duration,
    pub style: PanelStyle,
}

#[derive(Clone, Copy)]
pub struct PanelStyle {
    pub palette: Palette,
    /// User scale on top of the monitor's DPI scaling (1.0 = 100 %).
    pub zoom: f32,
}

/// Layout list shown on one or more screens. The windows are layered, click-through and never activated,
/// otherwise the layout would be switched in them rather than in the window being typed into.
/// Fades change only the windows' constant alpha and following the cursor only moves the window,
/// so neither repaints the panel.
pub struct Overlay {
    short_font: Font,
    name_font: Font,
    hint_font: Font,
    windows: Vec<HWND>,
    visible: bool,
    opacity: f32,
    fade: Option<Fade>,
    follow: Option<Follow>,
}

/// What the single `FollowCursor` window shows and where, so it can be moved or repainted for another DPI.
struct Follow {
    monitor: HMONITOR,
    placed: Placed,
    style: PanelStyle,
    items: Vec<OverlayItem>,
    hint: Option<String>,
}

#[derive(Clone, Copy)]
struct Placed {
    size: SIZE,
    gap: i32,
    origin: POINT,
}

impl Overlay {
    pub fn new(renderer: &Renderer) -> Result<Self> {
        register_class()?;
        Ok(Self {
            short_font: renderer.font(theme::FONT_FAMILY, 18.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
            name_font: renderer.font(theme::FONT_FAMILY, 14.0, DWRITE_FONT_WEIGHT_NORMAL)?,
            hint_font: renderer.font(theme::FONT_FAMILY, 12.0, DWRITE_FONT_WEIGHT_NORMAL)?,
            windows: Vec::new(),
            visible: false,
            opacity: 0.0,
            fade: None,
            follow: None,
        })
    }

    /// Also true while fading out.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Whether `tick` and `follow_cursor` currently have work to do on every frame.
    pub fn needs_frames(&self) -> bool {
        self.fade.is_some() || (self.visible && self.follow.is_some())
    }

    pub fn show(&mut self, renderer: &Renderer, items: &[OverlayItem], hint: Option<&str>, options: &ShowOptions) -> Result<()> {
        let now = Instant::now();
        let start = if self.visible { self.opacity_at(now) } else { 0.0 };
        self.fade = (!options.fade_in.is_zero() && start < 1.0).then(|| Fade::new(start, 1.0, options.fade_in, now));
        self.opacity = if self.fade.is_some() { start } else { 1.0 };

        let panel = Panel::layout(renderer, self, items, hint)?;
        let cursor = cursor_position();
        let monitors = match options.position {
            position if position.is_cursor_based() => vec![unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) }],
            _ if options.all_screens => all_monitors(),
            _ => vec![unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) }],
        };
        self.ensure_window_count(monitors.len())?;

        let alpha = alpha_byte(self.opacity);
        let mut last = None;
        for (&monitor, &window) in monitors.iter().zip(&self.windows) {
            last = Some((monitor, present(renderer, &panel, monitor, window, options.style, options.position, cursor, alpha)?));
        }
        self.follow = match (options.position, last) {
            (PanelPosition::FollowCursor, Some((monitor, placed))) => Some(Follow {
                monitor,
                placed,
                style: options.style,
                items: items.to_vec(),
                hint: hint.map(str::to_owned),
            }),
            _ => None,
        };
        self.visible = true;
        Ok(())
    }

    /// Keeps the `FollowCursor` panel next to the pointer; a no-op in other modes.
    pub fn follow_cursor(&mut self, renderer: &Renderer) -> Result<()> {
        let (Some(follow), Some(&window)) = (&self.follow, self.windows.first()) else { return Ok(()) };
        if !self.visible {
            return Ok(());
        }

        let cursor = cursor_position();
        let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
        if monitor != follow.monitor {
            // The new screen may use another DPI, so the panel is repainted for it.
            let (style, items, hint) = (follow.style, follow.items.clone(), follow.hint.clone());
            let panel = Panel::layout(renderer, self, &items, hint.as_deref())?;
            let alpha = alpha_byte(self.opacity);
            let placed = present(renderer, &panel, monitor, window, style, PanelPosition::FollowCursor, cursor, alpha)?;
            self.follow = Some(Follow { monitor, placed, style, items, hint });
            return Ok(());
        }

        let placed = follow.placed;
        let origin = panel_origin(PanelPosition::FollowCursor, work_area(monitor), placed.size, placed.gap, cursor);
        if origin != placed.origin {
            unsafe { SetWindowPos(window, None, origin.x, origin.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) }?;
            if let Some(follow) = &mut self.follow {
                follow.placed.origin = origin;
            }
        }
        Ok(())
    }

    pub fn hide(&mut self, fade_out: Duration) {
        if !self.visible {
            return;
        }
        let now = Instant::now();
        let current = self.opacity_at(now);
        if fade_out.is_zero() || current <= 0.0 {
            self.hide_now();
        } else {
            self.fade = Some(Fade::new(current, 0.0, fade_out, now));
        }
    }

    /// Advances the running fade, if any.
    pub fn tick(&mut self) {
        let Some(fade) = self.fade else { return };
        let now = Instant::now();
        self.opacity = fade.opacity_at(now);
        self.apply_opacity();

        if fade.is_finished_at(now) {
            self.fade = None;
            if fade.target() <= 0.0 {
                self.hide_now();
            }
        }
    }

    /// Drops all windows; they are recreated for the current screens on the next `show`.
    pub fn reset(&mut self) {
        for window in self.windows.drain(..) {
            let _ = unsafe { DestroyWindow(window) };
        }
        self.visible = false;
        self.fade = None;
        self.follow = None;
    }

    fn hide_now(&mut self) {
        for &window in &self.windows {
            let _ = unsafe { ShowWindow(window, SW_HIDE) };
        }
        self.visible = false;
        self.fade = None;
        self.follow = None;
        self.opacity = 0.0;
    }

    fn opacity_at(&self, now: Instant) -> f32 {
        self.fade.map_or(self.opacity, |fade| fade.opacity_at(now))
    }

    fn apply_opacity(&self) {
        let blend = blend_function(alpha_byte(self.opacity));
        for &window in &self.windows {
            let _ = unsafe { UpdateLayeredWindow(window, None, None, None, None, None, COLORREF(0), Some(&blend), ULW_ALPHA) };
        }
    }

    fn ensure_window_count(&mut self, count: usize) -> Result<()> {
        while self.windows.len() > count {
            if let Some(window) = self.windows.pop() {
                let _ = unsafe { DestroyWindow(window) };
            }
        }
        while self.windows.len() < count {
            self.windows.push(create_window()?);
        }
        Ok(())
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.reset();
    }
}

struct Row<'a> {
    item: &'a OverlayItem,
    short_name: TextBlock,
    display_name: TextBlock,
}

/// The panel measured in device-independent pixels, ready to be painted at any DPI.
struct Panel<'a> {
    rows: Vec<Row<'a>>,
    hint: Option<TextBlock>,
    content_width: f32,
    row_height: f32,
    width: f32,
    height: f32,
}

impl<'a> Panel<'a> {
    fn layout(renderer: &Renderer, overlay: &Overlay, items: &'a [OverlayItem], hint: Option<&str>) -> Result<Self> {
        let rows = items
            .iter()
            .map(|item| {
                Ok(Row {
                    item,
                    short_name: renderer.text(&item.short_name, &overlay.short_font)?,
                    display_name: renderer.text(&item.display_name, &overlay.name_font)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let hint = hint.map(|text| renderer.text(text, &overlay.hint_font)).transpose()?;

        let text_height = rows.iter().map(|r| r.short_name.height.max(r.display_name.height)).fold(0.0, f32::max);
        let row_height = text_height + 2.0 * ROW_PADDING_VERTICAL;
        let widest_name = rows.iter().map(|r| r.display_name.width).fold(0.0, f32::max);
        let row_width = (ROW_PADDING_LEFT + SHORT_NAME_COLUMN + widest_name + ROW_PADDING_RIGHT).max(ROW_MIN_WIDTH);
        let hint_width = hint.as_ref().map_or(0.0, |h| h.width + 2.0 * HINT_MARGIN_HORIZONTAL);
        let hint_height = hint.as_ref().map_or(0.0, |h| HINT_MARGIN_TOP + h.height + HINT_MARGIN_BOTTOM);
        let content_width = row_width.max(hint_width).ceil();
        let rows_height = rows.len() as f32 * (row_height + 2.0 * ROW_SPACING);
        let inset = 2.0 * (BORDER + PANEL_PADDING);

        Ok(Self {
            width: content_width + inset,
            height: (rows_height + hint_height + inset).ceil(),
            rows,
            hint,
            content_width,
            row_height,
        })
    }

    fn paint(&self, canvas: &Canvas, palette: &Palette) {
        let panel = Rect::new(0.0, 0.0, self.width, self.height);
        canvas.fill_rounded(panel, PANEL_RADIUS, palette.panel);
        canvas.stroke_rounded(panel.inflate(-0.5), PANEL_RADIUS, BORDER, palette.panel_border);

        let left = panel.left + BORDER + PANEL_PADDING;
        let mut top = panel.top + BORDER + PANEL_PADDING;
        for row in &self.rows {
            top += ROW_SPACING;
            self.paint_row(canvas, palette, row, left, top);
            top += self.row_height + ROW_SPACING;
        }

        if let Some(hint) = &self.hint {
            let color = palette.muted_text.faded(theme::HINT_OPACITY);
            canvas.text(hint, left + HINT_MARGIN_HORIZONTAL, top + HINT_MARGIN_TOP, color);
        }
    }

    fn paint_row(&self, canvas: &Canvas, palette: &Palette, row: &Row, left: f32, top: f32) {
        let opacity = if row.item.is_dimmed { theme::DIMMED_OPACITY } else { 1.0 };
        let text_color = if row.item.is_current { palette.on_accent } else { palette.muted_text }.faded(opacity);

        if row.item.is_current {
            let bounds = Rect::new(left, top, self.content_width, self.row_height);
            canvas.fill_rounded(bounds, ROW_RADIUS, palette.accent.faded(opacity));
        }

        let text_left = left + ROW_PADDING_LEFT;
        let center = |block: &TextBlock| top + (self.row_height - block.height) / 2.0;
        canvas.text(&row.short_name, text_left, center(&row.short_name), text_color);
        canvas.text(&row.display_name, text_left + SHORT_NAME_COLUMN, center(&row.display_name), text_color);
    }
}

fn present(
    renderer: &Renderer,
    panel: &Panel,
    monitor: HMONITOR,
    window: HWND,
    style: PanelStyle,
    position: PanelPosition,
    cursor: POINT,
    alpha: u8,
) -> Result<Placed> {
    let monitor_scale = monitor_dpi(monitor) as f32 / 96.0;
    // The user zoom is applied as extra DPI, so text and shapes are rendered crisply at the larger size.
    let scale = monitor_scale * style.zoom;
    let surface = Surface::new((panel.width * scale).ceil() as i32, (panel.height * scale).ceil() as i32)?;
    renderer.draw(&surface, 96.0 * scale, |canvas| panel.paint(canvas, &style.palette))?;

    let size = SIZE { cx: surface.width, cy: surface.height };
    let gap = (PANEL_GAP * monitor_scale).round() as i32;
    let origin = panel_origin(position, work_area(monitor), size, gap, cursor);
    let blend = blend_function(alpha);

    unsafe {
        UpdateLayeredWindow(
            window,
            None,
            Some(&origin),
            Some(&size),
            Some(surface.dc()),
            Some(&POINT::default()),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        )?;
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        SetWindowPos(window, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)?;
    }
    Ok(Placed { size, gap, origin })
}

fn blend_function(alpha: u8) -> BLENDFUNCTION {
    BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: alpha,
        AlphaFormat: AC_SRC_ALPHA as u8,
    }
}

fn alpha_byte(opacity: f32) -> u8 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn register_class() -> Result<()> {
    static REGISTER: Once = Once::new();
    let mut result = Ok(());
    REGISTER.call_once(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            result = Err(windows::core::Error::from_thread());
        }
    });
    result
}

fn create_window() -> Result<HWND> {
    unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            CLASS_NAME,
            None,
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            GetModuleHandleW(None).ok().map(Into::into),
            None,
        )
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

fn all_monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let monitors = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        monitors.push(monitor);
        true.into()
    }

    let mut monitors = Vec::new();
    let _ = unsafe { EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut monitors as *mut _ as isize)) };
    monitors
}

fn monitor_dpi(monitor: HMONITOR) -> u32 {
    let (mut x, mut y) = (96, 96);
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) };
    x
}

fn work_area(monitor: HMONITOR) -> RECT {
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = unsafe { GetMonitorInfoW(monitor, &mut info) };
    info.rcWork
}
