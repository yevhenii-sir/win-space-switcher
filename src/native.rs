use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, TranslateMessage};
use windows::core::PCWSTR;

/// Reads a DWORD from HKEY_CURRENT_USER.
pub fn read_user_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key, value, RRF_RT_REG_DWORD, None, Some((&mut data as *mut u32).cast()), Some(&mut size))
    };
    (status == ERROR_SUCCESS).then_some(data)
}

pub fn run_message_loop() {
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// NUL-terminated UTF-16 for Win32 string parameters.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Fills a fixed-size UTF-16 buffer (e.g. `NOTIFYICONDATAW::szTip`), truncating and NUL-terminating.
pub fn copy_wide(text: &str, buffer: &mut [u16]) {
    let capacity = buffer.len().saturating_sub(1);
    let mut length = 0;
    for (slot, unit) in buffer.iter_mut().zip(text.encode_utf16().take(capacity)) {
        *slot = unit;
        length += 1;
    }
    if let Some(terminator) = buffer.get_mut(length) {
        *terminator = 0;
    }
}
