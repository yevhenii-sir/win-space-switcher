use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::DirectWrite::{DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos, HICON, HMENU, ICONINFO,
    MENU_ITEM_FLAGS, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SM_CXSMICON,
    SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::{PCWSTR, Result};

use super::renderer::{Rect, Renderer, Surface};
use super::theme::{self, Palette};
use crate::native::{copy_wide, wide};

const ICON_ID: u32 = 1;
const KEYBOARD_GLYPH: &str = "\u{E765}";

pub enum MenuEntry<C> {
    Caption(String),
    Separator,
    Item { text: String, checked: bool, enabled: bool, command: C },
    Submenu { text: String, enabled: bool, entries: Vec<MenuEntry<C>> },
}

/// The notification-area icon. It only displays icons; they are owned by [`TrayIcons`].
pub struct TrayIcon {
    window: HWND,
    callback_message: u32,
    icon: HICON,
    tooltip: String,
}

impl TrayIcon {
    pub fn new(window: HWND, callback_message: u32, icon: HICON, tooltip: String) -> Self {
        let tray = Self { window, callback_message, icon, tooltip };
        tray.add();
        tray
    }

    /// Also called after Explorer restarts, which forgets every tray icon.
    pub fn add(&self) {
        let mut data = self.notify_data();
        data.uFlags |= NIF_MESSAGE;
        data.uCallbackMessage = self.callback_message;
        let _ = unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
    }

    pub fn update(&mut self, icon: HICON, tooltip: String) {
        if icon == self.icon && tooltip == self.tooltip {
            return;
        }
        self.icon = icon;
        self.tooltip = tooltip;
        let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.notify_data()) };
    }

    fn notify_data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window,
            uID: ICON_ID,
            uFlags: NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            hIcon: self.icon,
            ..Default::default()
        };
        copy_wide(&self.tooltip, &mut data.szTip);
        data
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &self.notify_data()) };
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TrayGlyph {
    Keyboard,
    Text(String),
}

/// Renders tray icons on demand and keeps them, so each glyph/color combination is drawn only once.
#[derive(Default)]
pub struct TrayIcons {
    cache: HashMap<(TrayGlyph, u32, u32), HICON>,
}

impl TrayIcons {
    pub fn get(&mut self, renderer: &Renderer, glyph: &TrayGlyph, palette: &Palette) -> Result<HICON> {
        let key = (glyph.clone(), palette.accent.to_argb(), palette.on_accent.to_argb());
        if let Some(&icon) = self.cache.get(&key) {
            return Ok(icon);
        }
        let icon = render_icon(renderer, glyph, palette)?;
        self.cache.insert(key, icon);
        Ok(icon)
    }
}

impl Drop for TrayIcons {
    fn drop(&mut self) {
        for icon in self.cache.values() {
            let _ = unsafe { DestroyIcon(*icon) };
        }
    }
}

/// A glyph on an accent-colored tile, readable on light and dark taskbars.
fn render_icon(renderer: &Renderer, glyph: &TrayGlyph, palette: &Palette) -> Result<HICON> {
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }.max(16);
    let pixels = size as f32;
    let text = match glyph {
        TrayGlyph::Keyboard => {
            let font = renderer.font(theme::ICON_FONT_FAMILY, pixels * 0.53, DWRITE_FONT_WEIGHT_NORMAL)?;
            renderer.text(KEYBOARD_GLYPH, &font)?
        }
        TrayGlyph::Text(label) => {
            // Start large and shrink until the label fits the tile (short names are 2 letters, fallbacks up to 4).
            let mut font_size = pixels * 0.62;
            loop {
                let font = renderer.font(theme::FONT_FAMILY, font_size, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
                let block = renderer.text(label, &font)?;
                if block.width <= pixels * 0.86 || font_size <= 6.0 {
                    break block;
                }
                font_size *= 0.9;
            }
        }
    };

    let surface = Surface::new(size, size)?;
    renderer.draw(&surface, 96.0, |canvas| {
        let inset = pixels / 32.0;
        let tile = Rect::new(inset, inset, pixels - 2.0 * inset, pixels - 2.0 * inset);
        canvas.fill_rounded(tile, pixels * 0.22, palette.accent);
        canvas.text(&text, (pixels - text.width) / 2.0, (pixels - text.height) / 2.0, palette.on_accent);
    })?;

    let color = surface.into_bitmap_view();
    let mask = unsafe { CreateBitmap(size, size, 1, 1, None) };
    let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color.bitmap() };
    let icon = unsafe { CreateIconIndirect(&info) };
    let _ = unsafe { DeleteObject(mask.into()) };
    icon
}

pub fn cursor_position() -> POINT {
    let mut point = POINT::default();
    let _ = unsafe { GetCursorPos(&mut point) };
    point
}

/// Shows a native popup menu at `at` and returns the chosen command. Runs a modal loop, so the caller must
/// not hold any borrow the window procedure might need meanwhile.
pub fn show_menu<C: Copy>(owner: HWND, at: POINT, entries: &[MenuEntry<C>]) -> Option<C> {
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    let mut commands = Vec::new();
    append_entries(menu, entries, &mut commands);

    // Without becoming foreground first, the menu would not close when clicking elsewhere.
    let _ = unsafe { SetForegroundWindow(owner) };
    let chosen = unsafe {
        TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, at.x, at.y, None, owner, None)
    };
    unsafe {
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        // Destroys submenus too.
        let _ = DestroyMenu(menu);
    }

    let index = usize::try_from(chosen.0).ok()?.checked_sub(1)?;
    commands.get(index).copied()
}

/// Appends `entries` to `menu`; command ids are 1-based indices into `commands`, unique across submenus.
fn append_entries<C: Copy>(menu: HMENU, entries: &[MenuEntry<C>], commands: &mut Vec<C>) {
    for entry in entries {
        let (flags, id, text) = match entry {
            MenuEntry::Caption(text) => (MF_STRING | MF_GRAYED, 0, Some(text)),
            MenuEntry::Separator => (MF_SEPARATOR, 0, None),
            MenuEntry::Item { text, checked, enabled, command } => {
                commands.push(*command);
                (item_flags(*checked, *enabled), commands.len(), Some(text))
            }
            MenuEntry::Submenu { text, enabled, entries } => {
                let Ok(submenu) = (unsafe { CreatePopupMenu() }) else { continue };
                append_entries(submenu, entries, commands);
                (MF_POPUP | item_flags(false, *enabled), submenu.0 as usize, Some(text))
            }
        };
        let text = text.map(|t| wide(t));
        let text = text.as_ref().map_or(PCWSTR::null(), |t| PCWSTR(t.as_ptr()));
        let _ = unsafe { AppendMenuW(menu, flags, id, text) };
    }
}

fn item_flags(checked: bool, enabled: bool) -> MENU_ITEM_FLAGS {
    let mut flags = MF_STRING;
    if checked {
        flags |= MF_CHECKED;
    }
    if !enabled {
        flags |= MF_GRAYED;
    }
    flags
}
