use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteKeyValueW};
use windows::core::w;

const TASK_NAME: &str = "WinSpaceSwitcher";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub trait Autostart {
    fn is_enabled(&self) -> bool;

    /// Requires administrator rights.
    fn set_enabled(&self, enabled: bool) -> io::Result<()>;
}

/// Starts the app at logon through Task Scheduler with the highest available privileges, so it also works in
/// elevated windows without a UAC prompt at every logon (a Run-key entry would always start unelevated).
pub struct ScheduledTaskAutostart {
    executable: PathBuf,
}

impl ScheduledTaskAutostart {
    pub fn for_current_executable() -> Option<Self> {
        Some(Self { executable: std::env::current_exe().ok()? })
    }
}

impl Autostart for ScheduledTaskAutostart {
    fn is_enabled(&self) -> bool {
        schtasks(&["/Query", "/TN", TASK_NAME]).is_ok()
    }

    fn set_enabled(&self, enabled: bool) -> io::Result<()> {
        remove_legacy_run_entry();
        if !enabled {
            return schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]).or_else(|e| if self.is_enabled() { Err(e) } else { Ok(()) });
        }

        let definition = std::env::temp_dir().join("WinSpaceSwitcher.task.xml");
        write_utf16(&definition, &task_xml(&self.executable, &current_user()))?;
        let result = schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &definition.to_string_lossy(), "/F"]);
        let _ = std::fs::remove_file(&definition);
        result
    }
}

fn schtasks(arguments: &[&str]) -> io::Result<()> {
    let status = Command::new("schtasks.exe")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("schtasks {} failed: {status}", arguments[0]))) }
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    }
}

/// Earlier builds registered themselves under the HKCU Run key.
fn remove_legacy_run_entry() {
    let _ = unsafe {
        RegDeleteKeyValueW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows\CurrentVersion\Run"), w!("WinSpaceSwitcher"))
    };
}

fn write_utf16(path: &Path, text: &str) -> io::Result<()> {
    let bytes: Vec<u8> = std::iter::once(0xFEFF_u16).chain(text.encode_utf16()).flat_map(u16::to_le_bytes).collect();
    std::fs::write(path, bytes)
}

/// Normal priority (Task Scheduler defaults to below-normal, which could delay the keyboard hook) and
/// no execution time limit (the default stops tasks after 72 hours).
fn task_xml(executable: &Path, user: &str) -> String {
    let command = escape_xml(&executable.to_string_lossy());
    let directory = escape_xml(&executable.parent().map(|p| p.to_string_lossy()).unwrap_or_default());
    let user = escape_xml(user);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Win+Space keyboard layout switcher</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <WorkingDirectory>{directory}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_runs_elevated_at_logon_without_limits() {
        let xml = task_xml(Path::new(r"C:\Tools & Apps\switcher.exe"), r"PC\Yevhenii");
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<Priority>4</Priority>"));
        assert!(xml.contains(r"<Command>C:\Tools &amp; Apps\switcher.exe</Command>"));
        assert!(xml.contains(r"<WorkingDirectory>C:\Tools &amp; Apps</WorkingDirectory>"));
        assert_eq!(xml.matches(r"<UserId>PC\Yevhenii</UserId>").count(), 2);
    }
}
