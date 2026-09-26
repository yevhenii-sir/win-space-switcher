use windows::core::w;

use super::renderer::Color;
use crate::native::read_user_dword;
use crate::settings::ThemeMode;

pub const DIMMED_OPACITY: f32 = 0.35;
pub const HINT_OPACITY: f32 = 0.8;

pub const FONT_FAMILY: &str = "Segoe UI";
pub const ICON_FONT_FAMILY: &str = "Segoe MDL2 Assets";

const DEFAULT_ACCENT: Color = Color::rgba(0x00, 0x67, 0xC0, 0xFF);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub panel: Color,
    pub panel_border: Color,
    pub accent: Color,
    /// Text drawn on top of `accent`.
    pub on_accent: Color,
    pub muted_text: Color,
}

impl Palette {
    pub fn resolve(mode: ThemeMode, use_accent_color: bool) -> Self {
        let is_dark = match mode {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => !system_uses_light_theme(),
        };
        let accent = if use_accent_color { system_accent_color().unwrap_or(DEFAULT_ACCENT) } else { DEFAULT_ACCENT };
        if is_dark { Self::dark(accent) } else { Self::light(accent) }
    }

    fn dark(accent: Color) -> Self {
        Self {
            panel: Color::rgba(0x20, 0x20, 0x20, 0xF0),
            panel_border: Color::rgba(0xFF, 0xFF, 0xFF, 0x26),
            accent,
            on_accent: readable_on(accent),
            muted_text: Color::rgba(0xFF, 0xFF, 0xFF, 0xB3),
        }
    }

    fn light(accent: Color) -> Self {
        Self {
            // Nearly opaque: dark text behind a light panel would otherwise show through.
            panel: Color::rgba(0xF9, 0xF9, 0xF9, 0xFC),
            panel_border: Color::rgba(0x00, 0x00, 0x00, 0x1F),
            accent,
            on_accent: readable_on(accent),
            muted_text: Color::rgba(0x00, 0x00, 0x00, 0x9E),
        }
    }
}

/// Black or white, whichever reads better on `background` (relative luminance, WCAG weights).
fn readable_on(background: Color) -> Color {
    let luminance = 0.2126 * background.r + 0.7152 * background.g + 0.0722 * background.b;
    if luminance > 0.6 { Color::rgba(0x00, 0x00, 0x00, 0xFF) } else { Color::rgba(0xFF, 0xFF, 0xFF, 0xFF) }
}

fn system_uses_light_theme() -> bool {
    read_user_dword(w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), w!("AppsUseLightTheme"))
        .is_some_and(|value| value != 0)
}

/// The accent color from Settings > Personalization > Colors, stored as 0xAABBGGRR.
fn system_accent_color() -> Option<Color> {
    let abgr = read_user_dword(w!(r"Software\Microsoft\Windows\DWM"), w!("AccentColor"))?;
    let [r, g, b, _] = abgr.to_le_bytes();
    Some(Color::rgba(r, g, b, 0xFF))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_on_accent_stays_readable() {
        assert_eq!(readable_on(DEFAULT_ACCENT), Color::rgba(0xFF, 0xFF, 0xFF, 0xFF));
        assert_eq!(readable_on(Color::rgba(0xFF, 0xE0, 0x40, 0xFF)), Color::rgba(0x00, 0x00, 0x00, 0xFF));
    }
}
