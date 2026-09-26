mod menu;
mod ui_state;

use std::cell::RefCell;
use std::error::Error;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowW, MSGFLT_ALLOW,
    PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, WINDOW_EX_STYLE, WM_APP, WM_DWMCOLORIZATIONCOLORCHANGED, WM_LBUTTONUP, WM_POWERBROADCAST,
    WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_POPUP, WTS_CONSOLE_CONNECT,
    WTS_REMOTE_CONNECT, WTS_SESSION_UNLOCK,
};
use windows::core::{PCWSTR, Result as WinResult, w};

use self::menu::MenuOutcome;
use self::ui_state::Ui;
use crate::autostart::{Autostart, ScheduledTaskAutostart};
use crate::diagnostics::{DiagnosticLog, LoggingInterceptor, LoggingObserver};
use crate::elevation::is_elevated;
use crate::input::{KeyboardHook, KeyboardInterceptor, Win32Keyboard};
use crate::layouts::Win32LayoutSystem;
use crate::native::run_message_loop;
use crate::settings::{JsonSettingsStore, SettingsManager};
use crate::switching::{LayoutHotkeyInterceptor, SwitchMode, SwitchObserver, SwitchService};
use crate::ui::{self, Renderer};
use crate::watchdog;

const ARG_LOG: &str = "--log";
const ARG_REPLACE: &str = "--replace";
const ARG_ENABLE_AUTOSTART: &str = "--enable-autostart";
const ARG_DISABLE_AUTOSTART: &str = "--disable-autostart";
const ARG_CHILD: &str = "--child";

const WM_LAYOUT_SELECTED: u32 = WM_APP + 1;
const WM_SESSION_ENDED: u32 = WM_APP + 2;
const WM_TRAY: u32 = WM_APP + 3;
const WM_EXIT_REQUEST: u32 = WM_APP + 4;

const OVERLAY_DELAY_TIMER: usize = 1;
const FRAME_TIMER: usize = 2;
const LAYOUT_POLL_TIMER: usize = 3;
const HOOK_REARM_TIMER: usize = 4;
/// Safety net for hooks dropped silently at moments without a notification.
const HOOK_REARM_INTERVAL_MS: u32 = 10 * 60 * 1000;
/// USER_TIMER_MINIMUM; the effective rate is bounded by the system timer (~16 ms), plenty for fades and
/// following the cursor. Runs only while the overlay needs it.
const FRAME_INTERVAL_MS: u32 = 10;
/// How often the tray icon checks the active window's layout for changes made outside Win+Space.
const LAYOUT_POLL_INTERVAL_MS: u32 = 250;

const MAIN_WINDOW_CLASS: PCWSTR = w!("WinSpaceSwitcherMain");
const SINGLE_INSTANCE_MUTEX: PCWSTR = w!("Local\\WinSpaceSwitcher");
const REPLACE_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_FILE: &str = "hotkeys.log";

static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

pub struct Options {
    /// Write every key event and switch to hotkeys.log in the working directory.
    log: bool,
    /// Take over from an already running instance (used when relaunching with administrator rights).
    replace_running: bool,
    /// Change autostart before anything else; set when an unelevated instance relaunches us elevated.
    autostart: Option<bool>,
    /// This process is the app itself, started by the supervising watchdog.
    child: bool,
}

impl Options {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        let mut options = Self { log: false, replace_running: false, autostart: None, child: false };
        for arg in args {
            match arg.as_str() {
                ARG_LOG => options.log = true,
                ARG_REPLACE => options.replace_running = true,
                ARG_ENABLE_AUTOSTART => options.autostart = Some(true),
                ARG_DISABLE_AUTOSTART => options.autostart = Some(false),
                ARG_CHILD => options.child = true,
                _ => {}
            }
        }
        options
    }

    fn child_args(&self, first_start: bool) -> Vec<String> {
        let replace = (first_start && self.replace_running).then_some(ARG_REPLACE);
        let log = self.log.then_some(ARG_LOG);
        [Some(ARG_CHILD), replace, log].into_iter().flatten().map(str::to_owned).collect()
    }
}

/// Every launch first becomes a small watchdog that runs the app as a child process and restarts it after a
/// crash. One-shot autostart changes are done here, in the (elevated) process that was asked to make them.
pub fn start(options: Options) -> Result<(), Box<dyn Error>> {
    if options.child {
        return run(&options);
    }

    if let Some(enabled) = options.autostart {
        if let Err(error) = ScheduledTaskAutostart::for_current_executable().map_or(Ok(()), |a| a.set_enabled(enabled)) {
            ui::show_error(&format!("Could not change autostart:\n{error}"));
            return Ok(());
        }
        if !enabled {
            return Ok(());
        }
    }

    watchdog::supervise(&options.child_args(true), &options.child_args(false));
    Ok(())
}

/// Composition root: wires the hook, the switch service and the UI, then runs the UI message loop.
fn run(options: &Options) -> Result<(), Box<dyn Error>> {
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    if options.replace_running {
        request_running_instance_exit();
    }
    let wait = if options.replace_running { REPLACE_TIMEOUT } else { Duration::ZERO };
    let Some(_instance) = SingleInstance::acquire(wait) else { return Ok(()) };

    let elevated = is_elevated();
    let window = create_main_window(elevated)?;
    let settings = SettingsManager::new(Box::new(JsonSettingsStore::new(JsonSettingsStore::default_path())));
    let log = options.log.then(|| DiagnosticLog::create(Path::new(LOG_FILE))).transpose()?;

    let notifier = UiNotifier(window.0 as isize);
    let service = match &log {
        Some(log) => SwitchService::start(Win32LayoutSystem, LoggingObserver::new(notifier, log.clone())),
        None => SwitchService::start(Win32LayoutSystem, notifier),
    };
    service.set_optional_layouts(settings.current().optional_handles());

    let interceptor = LayoutHotkeyInterceptor::new(Win32Keyboard, service.commands());
    let interceptor: Box<dyn KeyboardInterceptor> = match &log {
        Some(log) => Box::new(LoggingInterceptor::new(interceptor, log.clone())),
        None => Box::new(interceptor),
    };
    let hook = KeyboardHook::start(interceptor)?;

    let ui = Ui::new(ui_state::Services {
        window,
        renderer: Renderer::new()?,
        settings,
        service,
        hook: hook.handle(),
        autostart: ScheduledTaskAutostart::for_current_executable().map(|a| Box::new(a) as Box<dyn Autostart>),
        elevated,
        log_enabled: options.log,
    })?;
    UI.with(|slot| *slot.borrow_mut() = Some(ui));
    // Session unlock/reconnect notifications (WM_WTSSESSION_CHANGE) are used to re-arm the hook.
    let _ = unsafe { WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) };

    run_message_loop();

    // The hook holds a sender into the service, so it must stop before the service is dropped with the UI.
    drop(hook);
    UI.with(|slot| slot.borrow_mut().take());
    unsafe {
        let _ = WTSUnRegisterSessionNotification(window);
        let _ = DestroyWindow(window);
    }
    Ok(())
}

/// Runs `action` on the UI state unless it is unavailable or already borrowed (re-entrancy from a modal loop).
fn with_ui<R>(action: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|slot| slot.try_borrow_mut().ok()?.as_mut().map(action))
}

/// Forwards worker-thread events to the UI thread through the main window's message queue.
struct UiNotifier(isize);

impl UiNotifier {
    fn post(&self, message: u32, wparam: usize, lparam: isize) {
        let window = HWND(self.0 as *mut _);
        let _ = unsafe { PostMessageW(Some(window), message, WPARAM(wparam), LPARAM(lparam)) };
    }
}

impl SwitchObserver for UiNotifier {
    fn layout_selected(&self, layout: isize, mode: SwitchMode) {
        self.post(WM_LAYOUT_SELECTED, (mode == SwitchMode::All) as usize, layout);
    }

    fn session_ended(&self) {
        self.post(WM_SESSION_ENDED, 0, 0);
    }
}

fn create_main_window(elevated: bool) -> WinResult<HWND> {
    let instance = unsafe { GetModuleHandleW(None) }?.into();
    let class = WNDCLASSW {
        lpfnWndProc: Some(main_window_proc),
        hInstance: instance,
        lpszClassName: MAIN_WINDOW_CLASS,
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    let taskbar_created = *TASKBAR_CREATED.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) });

    // A hidden top-level window rather than a message-only one: only top-level windows receive broadcasts
    // such as TaskbarCreated (to restore the tray icon after Explorer restarts) and theme changes.
    let window = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), MAIN_WINDOW_CLASS, w!("WinSpaceSwitcher"), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance), None)
    }?;

    // Explorer runs unelevated, and Windows drops messages from lower-integrity processes by default,
    // which would make the tray icon ignore clicks when we run as administrator.
    if elevated {
        for message in [WM_TRAY, taskbar_created, WM_SETTINGCHANGE, WM_DWMCOLORIZATIONCOLORCHANGED] {
            let _ = unsafe { ChangeWindowMessageFilterEx(window, message, MSGFLT_ALLOW, None) };
        }
    }
    Ok(window)
}

unsafe extern "system" fn main_window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_LAYOUT_SELECTED => {
            let mode = if wparam.0 == 1 { SwitchMode::All } else { SwitchMode::Required };
            with_ui(|ui| ui.on_layout_selected(lparam.0, mode));
        }
        WM_SESSION_ENDED => {
            with_ui(Ui::on_session_ended);
        }
        WM_TIMER => match wparam.0 {
            OVERLAY_DELAY_TIMER => with_ui(Ui::on_overlay_delay_elapsed).unwrap_or_default(),
            FRAME_TIMER => with_ui(Ui::on_frame).unwrap_or_default(),
            LAYOUT_POLL_TIMER => with_ui(Ui::on_layout_poll).unwrap_or_default(),
            HOOK_REARM_TIMER => with_ui(Ui::rearm_hook).unwrap_or_default(),
            _ => {}
        },
        // Moments when Windows is most likely to have dropped the hook.
        WM_POWERBROADCAST if matches!(wparam.0 as u32, PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND) => {
            with_ui(Ui::rearm_hook);
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
        WM_WTSSESSION_CHANGE if matches!(wparam.0 as u32, WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT) => {
            with_ui(Ui::rearm_hook);
        }
        WM_SETTINGCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
            with_ui(Ui::on_appearance_changed);
            return unsafe { DefWindowProcW(window, message, wparam, lparam) };
        }
        WM_TRAY if matches!(lparam.0 as u32, WM_LBUTTONUP | WM_RBUTTONUP) => run_tray_menu(window),
        WM_EXIT_REQUEST => unsafe { PostQuitMessage(0) },
        _ if Some(&message) == TASKBAR_CREATED.get() => {
            with_ui(|ui| ui.tray.add());
        }
        _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
    LRESULT(0)
}

/// Toggles keep the menu open by showing it again at the same spot, so several can be changed in a row.
fn run_tray_menu(window: HWND) {
    let position = ui::cursor_position();
    loop {
        let Some(entries) = with_ui(|ui| ui.menu_entries()) else { return };
        let Some(command) = ui::show_menu(window, position, &entries) else { return };
        match with_ui(|ui| ui.execute(command)) {
            Some(MenuOutcome::KeepOpen) => continue,
            Some(MenuOutcome::Exit) => unsafe { PostQuitMessage(0) },
            Some(MenuOutcome::Close) | None => {}
        }
        return;
    }
}

fn request_running_instance_exit() {
    if let Ok(running) = unsafe { FindWindowW(MAIN_WINDOW_CLASS, PCWSTR::null()) } {
        let _ = unsafe { PostMessageW(Some(running), WM_EXIT_REQUEST, WPARAM(0), LPARAM(0)) };
    }
}

/// Two hooks would fight over Win+Space; the name is shared with the C# version on purpose.
struct SingleInstance(HANDLE);

impl SingleInstance {
    /// Waits up to `timeout` for another instance to exit; an abandoned mutex (crashed owner) counts as free.
    fn acquire(timeout: Duration) -> Option<Self> {
        let handle = unsafe { CreateMutexW(None, false, SINGLE_INSTANCE_MUTEX) }.ok()?;
        let result = unsafe { WaitForSingleObject(handle, timeout.as_millis() as u32) };
        if result == WAIT_OBJECT_0 || result == WAIT_ABANDONED {
            Some(Self(handle))
        } else {
            let _ = unsafe { CloseHandle(handle) };
            None
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}
