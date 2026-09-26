mod dark_mode;
mod fade;
mod overlay;
mod placement;
mod renderer;
mod theme;
mod tray;

pub use dark_mode::apply_menu_theme;
pub use overlay::{Overlay, OverlayItem, PanelStyle, ShowOptions};
pub use renderer::Renderer;
pub use theme::Palette;
pub use tray::{MenuEntry, TrayGlyph, TrayIcon, TrayIcons, cursor_position, show_menu};

use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::{PCWSTR, w};

use crate::native::wide;

pub fn show_error(message: &str) {
    let text = wide(message);
    unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), w!("WinSpaceSwitcher"), MB_OK | MB_ICONERROR) };
}
