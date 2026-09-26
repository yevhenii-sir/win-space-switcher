use super::ui_state::Ui;
use super::{ARG_DISABLE_AUTOSTART, ARG_ENABLE_AUTOSTART, ARG_LOG, ARG_REPLACE};
use crate::elevation::run_self_elevated;
use crate::layouts::{LayoutInfo, LayoutSystem};
use crate::settings::{AppSettings, PanelPosition, ThemeMode};
use crate::ui::{self, MenuEntry};

const PANEL_POSITIONS: [(PanelPosition, &str); 4] = [
    (PanelPosition::RightEdge, "Right edge"),
    (PanelPosition::ScreenCenter, "Screen center"),
    (PanelPosition::NearCursor, "Near cursor"),
    (PanelPosition::FollowCursor, "Follow cursor"),
];
const PANEL_SCALES_PERCENT: [u32; 6] = [75, 90, 100, 125, 150, 200];
const OVERLAY_DELAYS_MS: [u32; 4] = [0, 150, 300, 500];
const FADE_DURATIONS_MS: [u32; 5] = [0, 100, 150, 250, 400];
const THEMES: [(ThemeMode, &str); 3] =
    [(ThemeMode::System, "System theme"), (ThemeMode::Dark, "Dark"), (ThemeMode::Light, "Light")];

#[derive(Clone, Copy)]
pub(super) enum TrayCommand {
    ToggleRequired(isize),
    ToggleOverlay,
    SetPanelPosition(PanelPosition),
    SetPanelScale(u32),
    SetOverlayDelay(u32),
    SetFadeIn(u32),
    SetFadeOut(u32),
    ToggleAllMonitors,
    SetTheme(ThemeMode),
    ToggleAccentColor,
    ToggleTrayLanguage,
    ToggleAutostart,
    RestartElevated,
    Exit,
}

pub(super) enum MenuOutcome {
    KeepOpen,
    Close,
    Exit,
}

type Entry = MenuEntry<TrayCommand>;

impl Ui {
    pub(super) fn menu_entries(&self) -> Vec<Entry> {
        let settings = self.settings.current();
        let mut entries = vec![MenuEntry::Caption("Win+Space cycles through:".into())];
        entries.extend(self.layouts.installed().into_iter().map(|handle| {
            let info = LayoutInfo::describe(handle);
            let text = format!("{}   {}", info.short_name, info.display_name);
            item(text, !settings.is_optional(handle), true, TrayCommand::ToggleRequired(handle))
        }));
        entries.extend([
            MenuEntry::Caption("Unchecked: Win+Alt+Space only".into()),
            MenuEntry::Separator,
            submenu("Language panel", true, self.panel_entries()),
            submenu("Appearance", true, self.appearance_entries()),
        ]);
        if let Some(autostart) = &self.autostart {
            entries.push(item("Start with Windows", autostart.is_enabled(), true, TrayCommand::ToggleAutostart));
        }
        if !self.elevated {
            entries.push(item("Restart as administrator", false, true, TrayCommand::RestartElevated));
        }
        entries.extend([MenuEntry::Separator, item("Exit", false, true, TrayCommand::Exit)]);
        entries
    }

    fn panel_entries(&self) -> Vec<Entry> {
        let settings = self.settings.current();
        let shown = settings.show_overlay;
        vec![
            item("Show panel", shown, true, TrayCommand::ToggleOverlay),
            MenuEntry::Separator,
            submenu(
                "Position",
                shown,
                PANEL_POSITIONS
                    .iter()
                    .map(|&(position, text)| item(text, settings.panel_position == position, true, TrayCommand::SetPanelPosition(position)))
                    .collect(),
            ),
            submenu(
                "Size",
                shown,
                PANEL_SCALES_PERCENT
                    .iter()
                    .map(|&percent| item(format!("{percent} %"), settings.panel_scale_percent == percent, true, TrayCommand::SetPanelScale(percent)))
                    .collect(),
            ),
            submenu("Delay", shown, durations(settings.overlay_delay_ms, "None", TrayCommand::SetOverlayDelay, &OVERLAY_DELAYS_MS)),
            submenu("Fade in", shown, durations(settings.fade_in_ms, "Off", TrayCommand::SetFadeIn, &FADE_DURATIONS_MS)),
            submenu("Fade out", shown, durations(settings.fade_out_ms, "Off", TrayCommand::SetFadeOut, &FADE_DURATIONS_MS)),
            item(
                "Show on all monitors",
                settings.show_on_all_monitors,
                shown && !settings.panel_position.is_cursor_based(),
                TrayCommand::ToggleAllMonitors,
            ),
        ]
    }

    fn appearance_entries(&self) -> Vec<Entry> {
        let settings = self.settings.current();
        let mut entries: Vec<Entry> = THEMES
            .iter()
            .map(|&(theme, text)| item(text, settings.theme == theme, true, TrayCommand::SetTheme(theme)))
            .collect();
        entries.extend([
            MenuEntry::Separator,
            item("Windows accent color", settings.use_accent_color, true, TrayCommand::ToggleAccentColor),
            item("Language in tray icon", settings.tray_shows_language, true, TrayCommand::ToggleTrayLanguage),
        ]);
        entries
    }

    pub(super) fn execute(&mut self, command: TrayCommand) -> MenuOutcome {
        match command {
            TrayCommand::ToggleRequired(layout) => {
                let make_optional = !self.settings.current().is_optional(layout);
                // Win+Space needs at least one layout to switch to.
                if make_optional && self.required_count() <= 1 {
                    return MenuOutcome::KeepOpen;
                }
                self.settings.update(|s| s.with_optional(layout, make_optional));
                self.service.set_optional_layouts(self.settings.current().optional_handles());
            }
            TrayCommand::ToggleOverlay => {
                self.settings.update(AppSettings::with_overlay_toggled);
                self.overlay.reset();
            }
            TrayCommand::SetPanelPosition(position) => {
                self.settings.update(|s| AppSettings { panel_position: position, ..s.clone() });
            }
            TrayCommand::SetPanelScale(percent) => {
                self.settings.update(|s| AppSettings { panel_scale_percent: percent, ..s.clone() });
            }
            TrayCommand::SetOverlayDelay(ms) => {
                self.settings.update(|s| AppSettings { overlay_delay_ms: ms, ..s.clone() });
            }
            TrayCommand::SetFadeIn(ms) => {
                self.settings.update(|s| AppSettings { fade_in_ms: ms, ..s.clone() });
            }
            TrayCommand::SetFadeOut(ms) => {
                self.settings.update(|s| AppSettings { fade_out_ms: ms, ..s.clone() });
            }
            TrayCommand::ToggleAllMonitors => {
                self.settings.update(AppSettings::with_all_monitors_toggled);
                self.overlay.reset();
            }
            TrayCommand::SetTheme(theme) => {
                self.settings.update(|s| AppSettings { theme, ..s.clone() });
                self.on_appearance_changed();
            }
            TrayCommand::ToggleAccentColor => {
                self.settings.update(|s| AppSettings { use_accent_color: !s.use_accent_color, ..s.clone() });
                self.on_appearance_changed();
            }
            TrayCommand::ToggleTrayLanguage => {
                self.settings.update(|s| AppSettings { tray_shows_language: !s.tray_shows_language, ..s.clone() });
                self.on_tray_language_changed();
            }
            TrayCommand::ToggleAutostart => return self.toggle_autostart(),
            TrayCommand::RestartElevated => {
                run_self_elevated(&self.relaunch_arguments(&[ARG_REPLACE]));
                return MenuOutcome::Close;
            }
            TrayCommand::Exit => return MenuOutcome::Exit,
        }
        MenuOutcome::KeepOpen
    }

    /// Registering an elevated logon task needs administrator rights. Without them we relaunch elevated: that
    /// instance changes the task and, when enabling, takes over from this one, so a single UAC prompt both
    /// sets up autostart and makes the running switcher work in elevated windows right away.
    fn toggle_autostart(&mut self) -> MenuOutcome {
        let Some(autostart) = &self.autostart else { return MenuOutcome::KeepOpen };
        let enable = !autostart.is_enabled();

        if self.elevated {
            if let Err(error) = autostart.set_enabled(enable) {
                ui::show_error(&format!("Could not change autostart:\n{error}"));
            }
            return MenuOutcome::KeepOpen;
        }

        let arguments = if enable {
            self.relaunch_arguments(&[ARG_ENABLE_AUTOSTART, ARG_REPLACE])
        } else {
            ARG_DISABLE_AUTOSTART.to_string()
        };
        run_self_elevated(&arguments);
        MenuOutcome::Close
    }

    fn relaunch_arguments(&self, arguments: &[&str]) -> String {
        let log = self.log_enabled.then_some(ARG_LOG);
        arguments.iter().copied().chain(log).collect::<Vec<_>>().join(" ")
    }

    fn required_count(&self) -> usize {
        let settings = self.settings.current();
        self.layouts.installed().into_iter().filter(|&handle| !settings.is_optional(handle)).count()
    }
}

fn item(text: impl Into<String>, checked: bool, enabled: bool, command: TrayCommand) -> Entry {
    MenuEntry::Item { text: text.into(), checked, enabled, command }
}

fn submenu(text: &str, enabled: bool, entries: Vec<Entry>) -> Entry {
    MenuEntry::Submenu { text: text.into(), enabled, entries }
}

fn durations(current: u32, off: &str, command: fn(u32) -> TrayCommand, choices: &[u32]) -> Vec<Entry> {
    choices
        .iter()
        .map(|&ms| {
            let text = if ms == 0 { off.to_string() } else { format!("{ms} ms") };
            item(text, current == ms, true, command(ms))
        })
        .collect()
}
