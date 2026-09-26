use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Same file and format as the C# version, so both share one configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AppSettings {
    /// Handles of layouts skipped by Win+Space, as hex strings. Everything else is required.
    pub optional_layouts: Vec<String>,
    pub show_overlay: bool,
    pub show_on_all_monitors: bool,
    pub panel_position: PanelPosition,
    /// How long Win must stay held after the first switch before the panel appears; 0 shows it at once.
    pub overlay_delay_ms: u32,
    /// Fade durations; 0 disables the animation.
    pub fade_in_ms: u32,
    pub fade_out_ms: u32,
    /// Panel size on top of the monitor's own scaling.
    pub panel_scale_percent: u32,
    pub theme: ThemeMode,
    /// Use the Windows accent color instead of the built-in blue.
    pub use_accent_color: bool,
    /// Show the current layout (EN, RU, ...) in the tray icon instead of a keyboard glyph.
    pub tray_shows_language: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    /// Follow the Windows light/dark app setting.
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelPosition {
    #[default]
    RightEdge,
    ScreenCenter,
    /// Next to the mouse pointer, on its screen only; placed when shown or switched.
    NearCursor,
    /// Like `NearCursor`, but keeps moving with the pointer while visible.
    FollowCursor,
}

impl PanelPosition {
    pub fn is_cursor_based(self) -> bool {
        matches!(self, Self::NearCursor | Self::FollowCursor)
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            optional_layouts: Vec::new(),
            show_overlay: true,
            show_on_all_monitors: true,
            panel_position: PanelPosition::RightEdge,
            overlay_delay_ms: 0,
            fade_in_ms: 100,
            fade_out_ms: 150,
            panel_scale_percent: 100,
            theme: ThemeMode::System,
            use_accent_color: true,
            tray_shows_language: true,
        }
    }
}

impl AppSettings {
    pub fn is_optional(&self, layout: isize) -> bool {
        self.optional_layouts.contains(&to_key(layout))
    }

    pub fn with_optional(&self, layout: isize, optional: bool) -> Self {
        let key = to_key(layout);
        let mut layouts: Vec<String> = self.optional_layouts.iter().filter(|&k| *k != key).cloned().collect();
        if optional {
            layouts.push(key);
        }
        Self { optional_layouts: layouts, ..self.clone() }
    }

    pub fn with_overlay_toggled(&self) -> Self {
        Self { show_overlay: !self.show_overlay, ..self.clone() }
    }

    pub fn with_all_monitors_toggled(&self) -> Self {
        Self { show_on_all_monitors: !self.show_on_all_monitors, ..self.clone() }
    }

    pub fn optional_handles(&self) -> HashSet<isize> {
        self.optional_layouts.iter().filter_map(|key| u64::from_str_radix(key, 16).ok()).map(|h| h as isize).collect()
    }
}

fn to_key(layout: isize) -> String {
    format!("{:X}", layout as i64)
}

pub trait SettingsStore {
    fn load(&self) -> AppSettings;
    fn save(&self, settings: &AppSettings);
}

pub struct JsonSettingsStore {
    path: PathBuf,
}

impl JsonSettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        let app_data = std::env::var_os("APPDATA").map_or_else(|| PathBuf::from("."), PathBuf::from);
        app_data.join("WinSpaceSwitcher").join("settings.json")
    }
}

impl SettingsStore for JsonSettingsStore {
    fn load(&self) -> AppSettings {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|json| serde_json::from_str(json.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    fn save(&self, settings: &AppSettings) {
        let Ok(json) = serde_json::to_string_pretty(settings) else { return };
        if let Some(directory) = self.path.parent() {
            let _ = fs::create_dir_all(directory);
        }
        // On failure keep running with the in-memory settings.
        let _ = fs::write(&self.path, json);
    }
}

/// Holds the current settings and persists every change. UI thread only.
pub struct SettingsManager {
    store: Box<dyn SettingsStore>,
    current: AppSettings,
}

impl SettingsManager {
    pub fn new(store: Box<dyn SettingsStore>) -> Self {
        let current = store.load();
        Self { store, current }
    }

    pub fn current(&self) -> &AppSettings {
        &self.current
    }

    /// Returns `true` if the settings actually changed.
    pub fn update(&mut self, change: impl FnOnce(&AppSettings) -> AppSettings) -> bool {
        let updated = change(&self.current);
        if updated == self.current {
            return false;
        }
        self.store.save(&updated);
        self.current = updated;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UK: isize = 0xFFFF_FFFF_F0A8_0422_u64 as isize;

    #[test]
    fn keys_match_the_csharp_format() {
        let settings = AppSettings::default().with_optional(UK, true).with_optional(0x0409_0409, true);
        assert_eq!(settings.optional_layouts, ["FFFFFFFFF0A80422", "4090409"]);
        assert_eq!(settings.optional_handles(), HashSet::from([UK, 0x0409_0409]));
    }

    #[test]
    fn with_optional_toggles_membership() {
        let settings = AppSettings::default().with_optional(UK, true);
        assert!(settings.is_optional(UK));
        assert!(!settings.with_optional(UK, false).is_optional(UK));
    }

    #[test]
    fn reads_csharp_file_with_missing_fields_and_bom() {
        let json = "\u{feff}{ \"OptionalLayouts\": [\"FFFFFFFFF0A80422\"], \"ShowOnAllMonitors\": false }";
        let settings: AppSettings = serde_json::from_str(json.trim_start_matches('\u{feff}')).unwrap();
        assert!(settings.is_optional(UK));
        assert!(settings.show_overlay);
        assert!(!settings.show_on_all_monitors);
    }
}
