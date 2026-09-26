use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, MSG, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_KEYUP, WM_QUIT, WM_SYSKEYUP,
};

use super::{INJECTION_MARKER, KeyEvent, KeyboardInterceptor, Vk};

/// Thread message asking the hook thread to install a fresh hook.
const WM_REARM: u32 = WM_APP + 1;

thread_local! {
    static INTERCEPTOR: RefCell<Option<Box<dyn KeyboardInterceptor>>> = const { RefCell::new(None) };
}

/// Global WH_KEYBOARD_LL hook on its own thread with its own message loop, so a busy UI thread can never
/// delay it past LowLevelHooksTimeout.
pub struct KeyboardHook {
    handle: HookHandle,
    thread: Option<JoinHandle<()>>,
}

/// Lets other threads ask the hook to re-arm itself.
#[derive(Clone, Copy)]
pub struct HookHandle {
    thread_id: u32,
}

impl HookHandle {
    /// Windows silently removes a low-level hook that times out (e.g. under heavy load or around sleep),
    /// and there is no way to detect it. Re-arming replaces the hook with a new one; it is cheap and harmless
    /// when the old hook was still fine.
    pub fn rearm(self) {
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_REARM, WPARAM(0), LPARAM(0)) };
    }
}

impl KeyboardHook {
    pub fn start(interceptor: Box<dyn KeyboardInterceptor>) -> windows::core::Result<Self> {
        let (started_tx, started_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("keyboard-hook".into())
            .spawn(move || {
                INTERCEPTOR.with(|slot| *slot.borrow_mut() = Some(interceptor));
                match install() {
                    Ok(hook) => {
                        let _ = started_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                        run(hook);
                    }
                    Err(error) => {
                        let _ = started_tx.send(Err(error));
                    }
                }
            })
            .expect("failed to spawn the keyboard hook thread");

        let thread_id = started_rx.recv().expect("keyboard hook thread exited early")?;
        Ok(Self { handle: HookHandle { thread_id }, thread: Some(thread) })
    }

    pub fn handle(&self) -> HookHandle {
        self.handle
    }
}

impl Drop for KeyboardHook {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.handle.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn install() -> windows::core::Result<HHOOK> {
    let module = unsafe { GetModuleHandleW(None) }.ok().map(Into::into);
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) }
}

fn run(mut hook: HHOOK) {
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        if message.hwnd.is_invalid() && message.message == WM_REARM {
            // The new hook goes in before the old one comes out, so no key slips through in between. None is
            // seen twice either: hook callbacks only run while this thread waits in GetMessage.
            if let Ok(fresh) = install() {
                let _ = unsafe { UnhookWindowsHookEx(hook) };
                hook = fresh;
            }
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let data = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        // Our own injected keys arrive re-entrantly while the interceptor is running, so they must be
        // filtered out before touching it.
        if data.dwExtraInfo != INJECTION_MARKER {
            let event = KeyEvent {
                key: Vk(data.vkCode as u16),
                is_up: matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP),
            };
            if intercept(event) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// A panic must never unwind into user32; on any failure the key is simply let through.
fn intercept(event: KeyEvent) -> bool {
    panic::catch_unwind(AssertUnwindSafe(|| {
        INTERCEPTOR.with(|slot| match slot.try_borrow_mut() {
            Ok(mut interceptor) => interceptor.as_mut().is_some_and(|i| i.intercept(event)),
            Err(_) => false,
        })
    }))
    .unwrap_or(false)
}
