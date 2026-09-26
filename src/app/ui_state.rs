use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::Result;

use super::{
    FRAME_INTERVAL_MS, FRAME_TIMER, HOOK_REARM_INTERVAL_MS, HOOK_REARM_TIMER, LAYOUT_POLL_INTERVAL_MS,
    LAYOUT_POLL_TIMER, OVERLAY_DELAY_TIMER, WM_TRAY,
};
use crate::autostart::Autostart;
use crate::input::HookHandle;
use crate::layouts::{LayoutInfo, LayoutSystem, Win32LayoutSystem};
use crate::settings::SettingsManager;
use crate::switching::{SwitchMode, SwitchService};
use crate::ui::{
    self, Overlay, OverlayItem, Palette, PanelStyle, Renderer, ShowOptions, TrayGlyph, TrayIcon, TrayIcons,
};

const ALL_LANGUAGES_HINT: &str = "Win + Alt + Space: all languages";
const HOTKEYS_TOOLTIP: &str = "Win+Space: required languages\nWin+Alt+Space: all languages";

/// Everything the UI needs from the composition root.
pub(super) struct Services {
    pub window: HWND,
    pub renderer: Renderer,
    pub settings: SettingsManager,
    pub service: SwitchService,
    pub hook: HookHandle,
    pub autostart: Option<Box<dyn Autostart>>,
    pub elevated: bool,
    pub log_enabled: bool,
}

/// UI-thread state: the panel, the tray icon, the palette and the timers that drive them.
pub(super) struct Ui {
    pub(super) window: HWND,
    pub(super) renderer: Renderer,
    pub(super) overlay: Overlay,
    pub(super) tray: TrayIcon,
    pub(super) tray_icons: TrayIcons,
    pub(super) palette: Palette,
    pub(super) settings: SettingsManager,
    pub(super) service: SwitchService,
    hook: HookHandle,
    pub(super) layouts: Win32LayoutSystem,
    pub(super) autostart: Option<Box<dyn Autostart>>,
    pub(super) elevated: bool,
    pub(super) log_enabled: bool,
    /// Selection waiting for the overlay delay to pass.
    pending_overlay: Option<(isize, SwitchMode)>,
    /// Win is held after a switch; the window may not have applied the new layout yet.
    session_active: bool,
    /// Layout currently shown in the tray icon.
    tray_layout: Option<isize>,
}

impl Ui {
    pub(super) fn new(services: Services) -> Result<Self> {
        let settings = services.settings.current();
        ui::apply_menu_theme(settings.theme);
        let palette = Palette::resolve(settings.theme, settings.use_accent_color);
        let layouts = Win32LayoutSystem;
        let tray_layout = Some(layouts.active()).filter(|&layout| layout != 0);

        let mut tray_icons = TrayIcons::default();
        let (glyph, tooltip) = tray_content(tray_layout, settings.tray_shows_language);
        let icon = tray_icons.get(&services.renderer, &glyph, &palette)?;

        let ui = Self {
            window: services.window,
            overlay: Overlay::new(&services.renderer)?,
            tray: TrayIcon::new(services.window, WM_TRAY, icon, tooltip),
            tray_icons,
            palette,
            renderer: services.renderer,
            settings: services.settings,
            service: services.service,
            hook: services.hook,
            layouts,
            autostart: services.autostart,
            elevated: services.elevated,
            log_enabled: services.log_enabled,
            pending_overlay: None,
            session_active: false,
            tray_layout,
        };
        ui.sync_layout_poll_timer();
        unsafe { SetTimer(Some(ui.window), HOOK_REARM_TIMER, HOOK_REARM_INTERVAL_MS, None) };
        Ok(ui)
    }

    pub(super) fn rearm_hook(&mut self) {
        self.hook.rearm();
    }

    pub(super) fn on_layout_selected(&mut self, layout: isize, mode: SwitchMode) {
        self.session_active = true;
        self.tray_layout = Some(layout);
        self.refresh_tray();

        let delay = self.settings.current().overlay_delay_ms;
        if delay == 0 || self.overlay.is_visible() {
            self.show_overlay(layout, mode);
            return;
        }
        if self.pending_overlay.replace((layout, mode)).is_none() {
            unsafe { SetTimer(Some(self.window), OVERLAY_DELAY_TIMER, delay, None) };
        }
    }

    pub(super) fn on_overlay_delay_elapsed(&mut self) {
        let _ = unsafe { KillTimer(Some(self.window), OVERLAY_DELAY_TIMER) };
        if let Some((layout, mode)) = self.pending_overlay.take() {
            self.show_overlay(layout, mode);
        }
    }

    pub(super) fn on_session_ended(&mut self) {
        self.session_active = false;
        if self.pending_overlay.take().is_some() {
            let _ = unsafe { KillTimer(Some(self.window), OVERLAY_DELAY_TIMER) };
        }
        self.overlay.hide(milliseconds(self.settings.current().fade_out_ms));
        self.run_frame_timer();
    }

    pub(super) fn on_frame(&mut self) {
        self.overlay.tick();
        let _ = self.overlay.follow_cursor(&self.renderer);
        if !self.overlay.needs_frames() {
            let _ = unsafe { KillTimer(Some(self.window), FRAME_TIMER) };
        }
    }

    /// Picks up layout changes made outside Win+Space (mouse, other hotkeys, switching windows).
    /// Skipped during a switch session, when the window may still report the previous layout.
    pub(super) fn on_layout_poll(&mut self) {
        if self.session_active {
            return;
        }
        let active = self.layouts.active();
        if active != 0 && Some(active) != self.tray_layout {
            self.tray_layout = Some(active);
            self.refresh_tray();
        }
    }

    /// Windows theme or accent color changed, or our own theme settings did.
    pub(super) fn on_appearance_changed(&mut self) {
        let settings = self.settings.current();
        ui::apply_menu_theme(settings.theme);
        let palette = Palette::resolve(settings.theme, settings.use_accent_color);
        if palette != self.palette {
            self.palette = palette;
            self.refresh_tray();
        }
    }

    pub(super) fn on_tray_language_changed(&mut self) {
        self.tray_layout = Some(self.layouts.active()).filter(|&layout| layout != 0);
        self.refresh_tray();
        self.sync_layout_poll_timer();
    }

    fn show_overlay(&mut self, current: isize, mode: SwitchMode) {
        let settings = self.settings.current();
        if !settings.show_overlay {
            return;
        }

        let installed = self.layouts.installed();
        let required_only = mode == SwitchMode::Required;
        let items: Vec<OverlayItem> = installed
            .iter()
            .map(|&handle| {
                let info = LayoutInfo::describe(handle);
                OverlayItem {
                    short_name: info.short_name,
                    display_name: info.display_name,
                    is_current: handle == current,
                    is_dimmed: required_only && handle != current && settings.is_optional(handle),
                }
            })
            .collect();
        let has_optional = installed.iter().any(|&handle| settings.is_optional(handle));
        let hint = (required_only && has_optional).then_some(ALL_LANGUAGES_HINT);

        let options = ShowOptions {
            position: settings.panel_position,
            all_screens: settings.show_on_all_monitors,
            fade_in: milliseconds(settings.fade_in_ms),
            style: PanelStyle { palette: self.palette, zoom: settings.panel_scale_percent as f32 / 100.0 },
        };
        let _ = self.overlay.show(&self.renderer, &items, hint, &options);
        self.run_frame_timer();
    }

    fn refresh_tray(&mut self) {
        let (glyph, tooltip) = tray_content(self.tray_layout, self.settings.current().tray_shows_language);
        if let Ok(icon) = self.tray_icons.get(&self.renderer, &glyph, &self.palette) {
            self.tray.update(icon, tooltip);
        }
    }

    fn run_frame_timer(&self) {
        if self.overlay.needs_frames() {
            unsafe { SetTimer(Some(self.window), FRAME_TIMER, FRAME_INTERVAL_MS, None) };
        }
    }

    fn sync_layout_poll_timer(&self) {
        unsafe {
            if self.settings.current().tray_shows_language {
                SetTimer(Some(self.window), LAYOUT_POLL_TIMER, LAYOUT_POLL_INTERVAL_MS, None);
            } else {
                let _ = KillTimer(Some(self.window), LAYOUT_POLL_TIMER);
            }
        }
    }
}

fn tray_content(layout: Option<isize>, shows_language: bool) -> (TrayGlyph, String) {
    let info = layout.map(LayoutInfo::describe);
    let tooltip = match &info {
        Some(info) => format!("WinSpaceSwitcher: {}\n{HOTKEYS_TOOLTIP}", info.display_name),
        None => format!("WinSpaceSwitcher\n{HOTKEYS_TOOLTIP}"),
    };
    let glyph = match info {
        Some(info) if shows_language => TrayGlyph::Text(info.short_name),
        _ => TrayGlyph::Keyboard,
    };
    (glyph, tooltip)
}

pub(super) fn milliseconds(value: u32) -> Duration {
    Duration::from_millis(u64::from(value))
}
