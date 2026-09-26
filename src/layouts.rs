use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Globalization::{
    GetLocaleInfoEx, LCIDToLocaleName, LOCALE_ALLOW_NEUTRAL_NAMES, LOCALE_SISO639LANGNAME, LOCALE_SNATIVEDISPLAYNAME,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, PostMessageW,
    WM_INPUTLANGCHANGEREQUEST,
};
use windows::core::PCWSTR;

pub trait LayoutSystem: Send {
    /// Installed layout handles in the order the system cycles through them.
    fn installed(&self) -> Vec<isize>;

    /// Layout of the window that currently receives keyboard input.
    fn active(&self) -> isize;

    /// Asks the window that currently receives keyboard input to switch to `layout`.
    fn activate(&self, layout: isize);
}

pub struct Win32LayoutSystem;

impl LayoutSystem for Win32LayoutSystem {
    fn installed(&self) -> Vec<isize> {
        let count = unsafe { GetKeyboardLayoutList(None) };
        let mut handles = vec![HKL::default(); count.max(0) as usize];
        let written = unsafe { GetKeyboardLayoutList(Some(&mut handles)) }.max(0) as usize;
        handles.truncate(written);
        handles.into_iter().map(|hkl| hkl.0 as isize).collect()
    }

    fn active(&self) -> isize {
        input_window().map_or(0, |window| unsafe { GetKeyboardLayout(GetWindowThreadProcessId(window, None)) }.0 as isize)
    }

    fn activate(&self, layout: isize) {
        if let Some(window) = input_window() {
            let _ = unsafe { PostMessageW(Some(window), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(layout)) };
        }
    }
}

fn input_window() -> Option<HWND> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return None;
    }

    let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    let thread_id = unsafe { GetWindowThreadProcessId(foreground, None) };
    let has_focus = unsafe { GetGUIThreadInfo(thread_id, &mut info) }.is_ok() && !info.hwndFocus.is_invalid();
    Some(if has_focus { info.hwndFocus } else { foreground })
}

#[derive(Clone, Debug)]
pub struct LayoutInfo {
    pub short_name: String,
    pub display_name: String,
}

impl LayoutInfo {
    pub fn describe(handle: isize) -> Self {
        let language_id = (handle as usize & 0xFFFF) as u32;
        let locale = locale_name(language_id);
        let short_name = locale
            .as_deref()
            .and_then(|name| locale_info(name, LOCALE_SISO639LANGNAME))
            .map_or_else(|| format!("{language_id:04X}"), |name| name.to_uppercase());
        let display_name = locale
            .as_deref()
            .and_then(|name| locale_info(name, LOCALE_SNATIVEDISPLAYNAME))
            .map_or_else(|| format!("Language 0x{language_id:04X}"), |name| capitalize(&name));
        Self { short_name, display_name }
    }
}

fn locale_name(language_id: u32) -> Option<Vec<u16>> {
    let mut buffer = [0u16; 85];
    let length = unsafe { LCIDToLocaleName(language_id, Some(&mut buffer), LOCALE_ALLOW_NEUTRAL_NAMES) };
    (length > 0).then(|| buffer[..length as usize].to_vec())
}

fn locale_info(locale: &[u16], info_type: u32) -> Option<String> {
    let mut buffer = [0u16; 128];
    let length = unsafe { GetLocaleInfoEx(PCWSTR(locale.as_ptr()), info_type, Some(&mut buffer)) };
    (length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}
