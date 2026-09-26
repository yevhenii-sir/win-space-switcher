use std::collections::VecDeque;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// Exit code of a Rust panic that unwinds out of `main`.
const PANIC_EXIT_CODE: i32 = 101;
const RESTART_DELAY: Duration = Duration::from_secs(2);
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Runs the app as a child process and starts it again if it crashes.
///
/// A normal exit (Exit in the tray, another instance already running) or being ended from Task Manager ends
/// supervision too; only crashes are restarted, and at most `MAX_RESTARTS` times within `RESTART_WINDOW`
/// so a persistent failure cannot loop forever. `restart_args` replaces `first_args` after a crash, dropping
/// one-shot flags such as taking over a running instance.
pub fn supervise(first_args: &[String], restart_args: &[String]) {
    let Ok(executable) = std::env::current_exe() else { return };
    let mut policy = RestartPolicy::new(MAX_RESTARTS, RESTART_WINDOW);
    let mut args = first_args;

    loop {
        let Ok(status) = Command::new(&executable).args(args).status() else { return };
        if !is_crash(status.code()) || !policy.allow(Instant::now()) {
            return;
        }
        thread::sleep(RESTART_DELAY);
        args = restart_args;
    }
}

/// A panic, or an unhandled exception (NTSTATUS error codes such as 0xC0000005 access violation or
/// 0xC0000409 fail-fast/abort). Being killed on purpose is not a crash: Task Manager's "End task" exits with 1,
/// and tools like PowerShell's Stop-Process use -1, which is not a valid NTSTATUS.
fn is_crash(exit_code: Option<i32>) -> bool {
    exit_code.is_some_and(|code| code == PANIC_EXIT_CODE || (code as u32 >= 0xC000_0000 && code != -1))
}

/// Allows at most `limit` restarts within any sliding `window`.
struct RestartPolicy {
    limit: usize,
    window: Duration,
    recent: VecDeque<Instant>,
}

impl RestartPolicy {
    fn new(limit: usize, window: Duration) -> Self {
        Self { limit, window, recent: VecDeque::new() }
    }

    fn allow(&mut self, now: Instant) -> bool {
        while self.recent.front().is_some_and(|&restart| now.duration_since(restart) >= self.window) {
            self.recent.pop_front();
        }
        if self.recent.len() >= self.limit {
            return false;
        }
        self.recent.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_crashes_are_restarted() {
        assert!(is_crash(Some(101)));
        assert!(is_crash(Some(0xC000_0005_u32 as i32)));
        assert!(is_crash(Some(0xC000_0409_u32 as i32)));
        assert!(!is_crash(Some(0)));
        assert!(!is_crash(Some(1)));
        assert!(!is_crash(Some(-1)));
        assert!(!is_crash(None));
    }

    #[test]
    fn restarts_are_limited_within_the_window() {
        let window = Duration::from_secs(300);
        let start = Instant::now();
        let mut policy = RestartPolicy::new(3, window);
        assert!(policy.allow(start));
        assert!(policy.allow(start + Duration::from_secs(10)));
        assert!(policy.allow(start + Duration::from_secs(20)));
        assert!(!policy.allow(start + Duration::from_secs(30)));
        // The first restart has left the window, freeing one slot.
        assert!(policy.allow(start + window + Duration::from_secs(1)));
        assert!(!policy.allow(start + window + Duration::from_secs(2)));
    }
}
