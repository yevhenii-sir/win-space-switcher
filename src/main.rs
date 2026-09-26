#![windows_subsystem = "windows"]

mod app;
mod autostart;
mod diagnostics;
mod elevation;
mod input;
mod layouts;
mod native;
mod settings;
mod switching;
mod ui;
mod watchdog;

fn main() {
    let options = app::Options::from_args(std::env::args().skip(1));
    if let Err(error) = app::start(options) {
        ui::show_error(&format!("WinSpaceSwitcher failed to start:\n{error}"));
    }
}
