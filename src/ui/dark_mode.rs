use std::sync::OnceLock;

use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, w};

use crate::settings::ThemeMode;

// Undocumented uxtheme exports (by ordinal) that Explorer, Notepad and most tray apps use to theme popup
// menus. If they are missing, menus simply keep the default light look.
const SET_PREFERRED_APP_MODE: u16 = 135;
const FLUSH_MENU_THEMES: u16 = 136;

const ALLOW_DARK: i32 = 1;
const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

struct UxTheme {
    set_preferred_app_mode: SetPreferredAppMode,
    flush_menu_themes: FlushMenuThemes,
}

/// Makes popup menus light or dark: `System` follows the Windows app theme.
pub fn apply_menu_theme(mode: ThemeMode) {
    let Some(uxtheme) = uxtheme() else { return };
    let app_mode = match mode {
        ThemeMode::System => ALLOW_DARK,
        ThemeMode::Dark => FORCE_DARK,
        ThemeMode::Light => FORCE_LIGHT,
    };
    unsafe {
        (uxtheme.set_preferred_app_mode)(app_mode);
        (uxtheme.flush_menu_themes)();
    }
}

fn uxtheme() -> Option<&'static UxTheme> {
    static UXTHEME: OnceLock<Option<UxTheme>> = OnceLock::new();
    UXTHEME
        .get_or_init(|| unsafe {
            let library = LoadLibraryW(w!("uxtheme.dll")).ok()?;
            let set_preferred_app_mode = GetProcAddress(library, ordinal(SET_PREFERRED_APP_MODE))?;
            let flush_menu_themes = GetProcAddress(library, ordinal(FLUSH_MENU_THEMES))?;
            Some(UxTheme {
                set_preferred_app_mode: std::mem::transmute::<_, SetPreferredAppMode>(set_preferred_app_mode),
                flush_menu_themes: std::mem::transmute::<_, FlushMenuThemes>(flush_menu_themes),
            })
        })
        .as_ref()
}

fn ordinal(value: u16) -> PCSTR {
    PCSTR(usize::from(value) as *const u8)
}
