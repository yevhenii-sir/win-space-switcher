use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::native::wide;

/// Whether this process runs with administrator rights. Without them the keyboard hook does not see input
/// sent to elevated windows, and layout requests to those windows are blocked.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }

    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    let _ = unsafe { CloseHandle(token) };
    queried.is_ok() && elevation.TokenIsElevated != 0
}

/// Starts this executable again with administrator rights (shows the UAC prompt).
/// Returns `false` if the user declined or the launch failed.
pub fn run_self_elevated(arguments: &str) -> bool {
    let Ok(executable) = std::env::current_exe() else { return false };
    let executable = wide(&executable.to_string_lossy());
    let arguments = wide(arguments);

    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(executable.as_ptr()),
        lpParameters: PCWSTR(arguments.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.is_ok()
}
